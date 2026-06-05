use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{
        Path as AxumPath, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::{
    app_state::{
        AuthorizedRemoteControlClient, PendingRemoteRequest, RemoteControlClientState,
        RemoteControlInner, RemoteControlRecentEvent, RemoteControlStreamDiagnostics, SharedState,
    },
    chain_log,
    codex::CodexNotification,
    types::{InboundAttachment, now_ms},
};

static REMOTE_REQUEST_ID: AtomicU64 = AtomicU64::new(200_000);
static REMOTE_SUBSCRIBE_CURSOR_ID: AtomicU64 = AtomicU64::new(1);
const PROTOCOL_VERSION: &str = "3";
const REMOTE_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const REMOTE_DISCOVERY_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const REMOTE_CONTROL_WEBSOCKET_PING_INTERVAL: Duration = Duration::from_secs(10);
const REMOTE_CONTROL_APP_PING_INTERVAL: Duration = Duration::from_secs(10);
const REMOTE_CONTROL_PONG_TIMEOUT: Duration = Duration::from_secs(60);
const REMOTE_CONTROL_STALE_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const REMOTE_CONTROL_REINITIALIZE_RETRY_TIMEOUT: Duration = REMOTE_REQUEST_TIMEOUT;
const REMOTE_CONTROL_UNKNOWN_REINITIALIZE_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_CONTROL_CLIENT_REINITIALIZED_ERROR: &str =
    "remote-control client was reported unknown by app-server";
const REMOTE_CONTROL_SEGMENT_TARGET_BYTES: usize = 100 * 1024;
const REMOTE_CONTROL_SEGMENT_MAX_BYTES: usize = 150 * 1024;
const REMOTE_CONTROL_REASSEMBLED_MAX_BYTES: usize = 100 * 1024 * 1024;
const REMOTE_CONTROL_SEGMENT_COUNT_MAX: usize = 1024;
const REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY: usize = 4096;
const REMOTE_CONTROL_RECENT_EVENT_LIMIT: usize = 96;
const REMOTE_CONTROL_DIAGNOSTIC_WINDOW_MS: u128 = 10_000;
const FEISHU_BRIDGE_CLIENT_ID: &str = "codex-remote-feishu";
const FEISHU_BRIDGE_ENV_ID: &str = "env_codex_remote_feishu_bridge";
const FEISHU_BRIDGE_INSTALLATION_ID: &str = "codex-remote-feishu-bridge";
const DEFAULT_REMOTE_CLIENT_KEY: &str = "default";

pub(crate) enum OutboundWsMessage {
    Text(Value),
    Ping(axum::body::Bytes),
    Pong(axum::body::Bytes),
    Close(String),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlStatusResponse {
    pub connected: bool,
    pub initialized: bool,
    pub client_id: String,
    pub stream_id: Option<String>,
    pub server_id: Option<String>,
    pub environment_id: Option<String>,
    pub server_name: Option<String>,
    pub installation_id: Option<String>,
    pub account_id: Option<String>,
    pub current_thread_id: Option<String>,
    pub current_turn_id: Option<String>,
    pub last_error: Option<String>,
    pub healthy: bool,
    pub stale: bool,
    pub connected_at_ms: Option<u128>,
    pub last_ws_inbound_at_ms: Option<u128>,
    pub last_ws_ping_at_ms: Option<u128>,
    pub last_ws_pong_at_ms: Option<u128>,
    pub last_app_ping_at_ms: Option<u128>,
    pub last_app_pong_at_ms: Option<u128>,
    pub last_app_pong_status: Option<String>,
    pub last_initialize_sent_at_ms: Option<u128>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum IncomingServerEvent {
    ServerMessage {
        message: Value,
    },
    ServerMessageChunk {
        segment_id: usize,
        segment_count: usize,
        message_size_bytes: usize,
        message_chunk_base64: String,
    },
    Ack,
    Pong {
        status: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct IncomingServerEnvelope {
    #[serde(flatten)]
    event: IncomingServerEvent,
    client_id: String,
    stream_id: String,
    seq_id: u64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutgoingClientEvent {
    ClientMessage {
        message: Value,
    },
    ClientMessageChunk {
        segment_id: usize,
        segment_count: usize,
        message_size_bytes: usize,
        message_chunk_base64: String,
    },
    Ack {
        segment_id: Option<usize>,
    },
    Ping,
    ClientClosed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct OutgoingClientEnvelope {
    #[serde(flatten)]
    event: OutgoingClientEvent,
    client_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seq_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct EnrollRequest {
    name: Option<String>,
    os: Option<String>,
    arch: Option<String>,
    app_server_version: Option<String>,
    installation_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct EnrollResponse {
    server_id: String,
    environment_id: String,
    remote_control_token: String,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct RefreshRequest {
    server_id: Option<String>,
    installation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RenameEnvironmentRequest {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RemoteControlClientFinishRequest {
    client_id: String,
    step_up_token: Option<String>,
    device_identity: Option<RemoteControlDeviceIdentity>,
    device_key_proof: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct RemoteControlDeviceIdentity {
    algorithm: String,
    key_id: String,
    protection_class: String,
    public_key_spki_der_base64: String,
}

struct ServerChunkAssembly {
    segment_count: usize,
    message_size_bytes: usize,
    raw: Vec<u8>,
    next_segment_id: usize,
}

enum ServerChunkObservation {
    Pending,
    Complete(Value),
    Dropped,
}

enum RemoteServerWorkItem {
    ServerMessage {
        connection_epoch: u64,
        seq_id: u64,
        client_id: String,
        stream_id: String,
        message: Value,
    },
    ServerAck {
        connection_epoch: u64,
        seq_id: u64,
        client_id: String,
        stream_id: String,
    },
    ServerPong {
        connection_epoch: u64,
        seq_id: u64,
        client_id: String,
        stream_id: String,
        status: String,
        should_reinitialize: bool,
    },
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/backend-api/wham/accounts/check", get(accounts_check))
        .route("/backend-api/accounts/check", get(accounts_check))
        .route("/backend-api/wham/usage", get(usage))
        .route("/backend-api/usage", get(usage))
        .route("/backend-api/wham/tasks/list", get(tasks_list))
        .route("/backend-api/wham/environments", get(wham_environments))
        .route("/backend-api/wham/apps", post(wham_apps))
        .route(
            "/backend-api/connectors/directory/list",
            get(connectors_directory_list),
        )
        .route(
            "/backend-api/connectors/directory/list_workspace",
            get(connectors_directory_list),
        )
        .route("/connectors/directory/list", get(connectors_directory_list))
        .route(
            "/connectors/directory/list_workspace",
            get(connectors_directory_list),
        )
        .route(
            "/backend-api/codex/analytics-events/events",
            post(analytics_events),
        )
        .route("/backend-api/beacons/home", get(beacons_home))
        .route("/backend-api/beacons/event", post(beacons_event))
        .route(
            "/backend-api/wham/onboarding/context",
            get(onboarding_context),
        )
        .route("/backend-api/onboarding/context", get(onboarding_context))
        .route("/backend-api/accounts/mfa_info", get(accounts_mfa_info))
        .route(
            "/backend-api/wham/accounts/mfa_info",
            get(accounts_mfa_info),
        )
        .route(
            "/backend-api/wham/remote/control/mfa_requirement",
            get(remote_control_mfa_requirement),
        )
        .route(
            "/backend-api/remote/control/mfa_requirement",
            get(remote_control_mfa_requirement),
        )
        .route(
            "/backend-api/codex/remote/control/mfa_requirement",
            get(remote_control_mfa_requirement),
        )
        .route(
            "/backend-api/wham/remote/control/clients",
            get(remote_control_clients),
        )
        .route(
            "/backend-api/codex/remote/control/clients",
            get(remote_control_clients),
        )
        .route(
            "/backend-api/remote/control/clients",
            get(remote_control_clients),
        )
        .route(
            "/backend-api/wham/remote/control/clients/{client_id}",
            axum::routing::delete(delete_remote_control_client),
        )
        .route(
            "/backend-api/codex/remote/control/clients/{client_id}",
            axum::routing::delete(delete_remote_control_client),
        )
        .route(
            "/backend-api/remote/control/clients/{client_id}",
            axum::routing::delete(delete_remote_control_client),
        )
        .route(
            "/backend-api/codex/remote/control/environments",
            get(remote_control_environments),
        )
        .route(
            "/backend-api/remote/control/environments",
            get(remote_control_environments),
        )
        .route(
            "/backend-api/codex/remote/control/environments/{env_id}",
            axum::routing::patch(rename_remote_control_environment)
                .delete(delete_remote_control_environment),
        )
        .route(
            "/backend-api/remote/control/environments/{env_id}",
            axum::routing::patch(rename_remote_control_environment)
                .delete(delete_remote_control_environment),
        )
        .route(
            "/backend-api/wham/remote/control/server/enroll",
            post(enroll),
        )
        .route(
            "/backend-api/wham/remote/control/server/refresh",
            post(refresh),
        )
        .route(
            "/backend-api/codex/remote/control/client/enroll/start",
            post(remote_control_client_enroll_start),
        )
        .route(
            "/backend-api/codex/remote/control/client/enroll/finish",
            post(remote_control_client_enroll_finish),
        )
        .route(
            "/backend-api/codex/remote/control/client/refresh/start",
            post(remote_control_client_refresh_start),
        )
        .route(
            "/backend-api/codex/remote/control/client/refresh/finish",
            post(remote_control_client_refresh_finish),
        )
        .route(
            "/backend-api/codex/remote/control/client",
            get(client_websocket),
        )
        .route("/backend-api/remote/control/server/enroll", post(enroll))
        .route("/backend-api/remote/control/server/refresh", post(refresh))
        .route("/backend-api/wham/remote/control/server", get(websocket))
        .route("/backend-api/remote/control/server", get(websocket))
        .route("/api/remote-control/status", get(status))
}

pub fn subscribe(state: &SharedState) -> tokio::sync::broadcast::Receiver<CodexNotification> {
    state.remote_control.notifications.subscribe()
}

pub async fn status(State(state): State<SharedState>) -> Json<RemoteControlStatusResponse> {
    Json(status_snapshot(&state).await)
}

pub async fn status_snapshot(state: &SharedState) -> RemoteControlStatusResponse {
    let remote = state.remote_control.inner.lock().await;
    let stale = remote_control_stale_reason_locked(&remote, now_ms()).is_some();
    let default_client = remote.clients.get(DEFAULT_REMOTE_CLIENT_KEY);
    let initialized = default_client
        .map(|client| client.initialized)
        .unwrap_or(remote.initialized);
    let client_id = default_client
        .map(|client| client.client_id.clone())
        .unwrap_or_else(|| remote.client_id.clone());
    let stream_id = default_client
        .map(|client| client.stream_id.clone())
        .unwrap_or_else(|| remote.stream_id.clone());
    let current_thread_id = default_client
        .and_then(|client| client.current_thread_id.clone())
        .or_else(|| remote.current_thread_id.clone());
    let current_turn_id = default_client
        .and_then(|client| client.current_turn_id.clone())
        .or_else(|| remote.current_turn_id.clone());
    let last_app_ping_at_ms = default_client
        .and_then(|client| client.last_app_ping_at_ms)
        .or(remote.last_app_ping_at_ms);
    let last_app_pong_at_ms = default_client
        .and_then(|client| client.last_app_pong_at_ms)
        .or(remote.last_app_pong_at_ms);
    let last_app_pong_status = default_client
        .and_then(|client| client.last_app_pong_status.clone())
        .or_else(|| remote.last_app_pong_status.clone());
    let last_initialize_sent_at_ms = default_client
        .and_then(|client| client.last_initialize_sent_at_ms)
        .or(remote.last_initialize_sent_at_ms);
    let healthy = remote.connected && initialized && !stale;
    RemoteControlStatusResponse {
        connected: remote.connected,
        initialized,
        client_id,
        stream_id: (!stream_id.is_empty()).then_some(stream_id),
        server_id: remote.server_id.clone(),
        environment_id: remote.environment_id.clone(),
        server_name: remote.server_name.clone(),
        installation_id: remote.installation_id.clone(),
        account_id: remote.account_id.clone(),
        current_thread_id,
        current_turn_id,
        last_error: remote.last_error.clone(),
        healthy,
        stale,
        connected_at_ms: remote.connected_at_ms,
        last_ws_inbound_at_ms: remote.last_ws_inbound_at_ms,
        last_ws_ping_at_ms: remote.last_ws_ping_at_ms,
        last_ws_pong_at_ms: remote.last_ws_pong_at_ms,
        last_app_ping_at_ms,
        last_app_pong_at_ms,
        last_app_pong_status,
        last_initialize_sent_at_ms,
    }
}

fn remote_control_stale_reason_locked(remote: &RemoteControlInner, now_ms: u128) -> Option<String> {
    if !remote.connected {
        return None;
    }
    let timeout_ms = REMOTE_CONTROL_PONG_TIMEOUT.as_millis();
    let initialize_started_at_ms = remote.last_initialize_sent_at_ms.or(remote.connected_at_ms);
    if !remote.initialized
        && initialize_started_at_ms
            .is_some_and(|started_at_ms| now_ms.saturating_sub(started_at_ms) >= timeout_ms)
    {
        return Some(format!(
            "remote-control initialize timed out after {}s",
            REMOTE_CONTROL_PONG_TIMEOUT.as_secs()
        ));
    }
    if let Some(last_ping_at_ms) = remote.last_ws_ping_at_ms {
        let last_pong_or_connect_at_ms = remote
            .last_ws_pong_at_ms
            .or(remote.connected_at_ms)
            .unwrap_or(last_ping_at_ms);
        if last_ping_at_ms > last_pong_or_connect_at_ms
            && now_ms.saturating_sub(last_pong_or_connect_at_ms) >= timeout_ms
        {
            return Some(format!(
                "websocket pong timed out after {}s",
                REMOTE_CONTROL_PONG_TIMEOUT.as_secs()
            ));
        }
    }
    None
}

fn ensure_client_state_locked<'a>(
    remote: &'a mut RemoteControlInner,
    client_key: &str,
) -> &'a mut RemoteControlClientState {
    let client_key = normalize_remote_client_key(client_key);
    if !remote.clients.contains_key(&client_key) {
        let client_id = remote.client_id.clone();
        let stream_id = if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            if remote.stream_id.is_empty() {
                remote.stream_id = uuid_like();
            }
            remote.stream_id.clone()
        } else {
            if remote.stream_id.is_empty() {
                remote.stream_id = uuid_like();
            }
            stable_id("stream", &format!("{}:{client_key}", remote.stream_id))
        };
        remote.clients.insert(
            client_key.clone(),
            RemoteControlClientState {
                client_id,
                stream_id,
                initialized: false,
                next_seq_id: 1,
                pending: HashMap::new(),
                current_thread_id: None,
                current_turn_id: None,
                last_app_ping_at_ms: None,
                last_app_pong_at_ms: None,
                last_app_pong_status: None,
                last_initialize_sent_at_ms: None,
                recovery_attempt: 0,
                recovery_started_at_ms: None,
            },
        );
    }
    remote
        .clients
        .get_mut(&client_key)
        .expect("remote client state should exist")
}

fn normalize_remote_client_key(client_key: &str) -> String {
    let client_key = client_key.trim();
    if client_key.is_empty() {
        DEFAULT_REMOTE_CLIENT_KEY.to_string()
    } else {
        client_key.to_string()
    }
}

fn remote_client_key_for_stream_locked(
    remote: &RemoteControlInner,
    client_id: &str,
    stream_id: &str,
) -> Option<String> {
    remote
        .clients
        .iter()
        .find(|(_, client)| client.client_id == client_id && client.stream_id == stream_id)
        .map(|(client_key, _)| client_key.clone())
}

fn sync_default_client_legacy_locked(remote: &mut RemoteControlInner) {
    let Some(default_client) = remote.clients.get(DEFAULT_REMOTE_CLIENT_KEY) else {
        return;
    };
    let initialized = default_client.initialized;
    let client_id = default_client.client_id.clone();
    let stream_id = default_client.stream_id.clone();
    let current_thread_id = default_client.current_thread_id.clone();
    let current_turn_id = default_client.current_turn_id.clone();
    let last_app_ping_at_ms = default_client.last_app_ping_at_ms;
    let last_app_pong_at_ms = default_client.last_app_pong_at_ms;
    let last_app_pong_status = default_client.last_app_pong_status.clone();
    let last_initialize_sent_at_ms = default_client.last_initialize_sent_at_ms;
    remote.initialized = initialized;
    remote.client_id = client_id;
    remote.stream_id = stream_id;
    remote.current_thread_id = current_thread_id;
    remote.current_turn_id = current_turn_id;
    remote.last_app_ping_at_ms = last_app_ping_at_ms;
    remote.last_app_pong_at_ms = last_app_pong_at_ms;
    remote.last_app_pong_status = last_app_pong_status;
    remote.last_initialize_sent_at_ms = last_initialize_sent_at_ms;
}

fn reset_remote_clients_for_connection_locked(remote: &mut RemoteControlInner) -> Vec<String> {
    let ack_cursor_keys = remote
        .clients
        .values()
        .map(|client| server_ack_cursor_key(&client.client_id, &client.stream_id))
        .collect::<Vec<_>>();
    for client in remote.clients.values_mut() {
        client.initialized = false;
        client.last_app_ping_at_ms = None;
        client.last_app_pong_at_ms = None;
        client.last_app_pong_status = None;
        client.last_initialize_sent_at_ms = None;
        client.recovery_started_at_ms = None;
        client
            .pending
            .retain(|_, pending| pending.method != "initialize");
    }
    ack_cursor_keys
}

fn push_remote_recent_event_locked(
    remote: &mut RemoteControlInner,
    direction: &'static str,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    seq_id: Option<u64>,
    kind: impl Into<String>,
    summary: impl Into<String>,
) {
    remote.recent_events.push_back(RemoteControlRecentEvent {
        ts_ms: now_ms(),
        direction,
        connection_epoch,
        client_id: client_id.to_string(),
        stream_id: stream_id.to_string(),
        seq_id,
        kind: kind.into(),
        summary: summary.into(),
    });
    while remote.recent_events.len() > REMOTE_CONTROL_RECENT_EVENT_LIMIT {
        remote.recent_events.pop_front();
    }
}

async fn record_remote_recent_event(
    state: &SharedState,
    direction: &'static str,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    seq_id: Option<u64>,
    kind: impl Into<String>,
    summary: impl Into<String>,
) {
    let mut remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch != connection_epoch {
        return;
    }
    push_remote_recent_event_locked(
        &mut remote,
        direction,
        connection_epoch,
        client_id,
        stream_id,
        seq_id,
        kind,
        summary,
    );
}

async fn accounts_check() -> Json<Value> {
    Json(json!({
        "account_ordering": ["acct_codex_remote_local"],
        "current_account_id": "acct_codex_remote_local",
        "accounts": [{
            "id": "acct_codex_remote_local",
            "account_id": "acct_codex_remote_local",
            "account_user_id": "user_codex_remote_local__acct_codex_remote_local",
            "user_id": "user_codex_remote_local",
            "name": "Codex Remote Local",
            "title": "Codex Remote Local",
            "email": "codex-remote-local@example.local",
            "plan_type": "pro",
            "structure": "personal",
            "role": "owner",
            "is_default": true,
            "is_deactivated": false,
            "is_paid": true,
        }],
    }))
}

async fn onboarding_context() -> Json<Value> {
    Json(json!({
        "account_id": "acct_codex_remote_local",
        "account_user_id": "user_codex_remote_local__acct_codex_remote_local",
        "completed": true,
        "requires_onboarding": false,
        "items": [],
    }))
}

async fn usage() -> Json<Value> {
    Json(json!({
        "plan_type": "pro",
        "rate_limit": {
            "allowed": true,
            "limit_reached": false,
        },
        "credits": {
            "has_credits": true,
            "unlimited": true,
        },
    }))
}

async fn beacons_home() -> Json<Value> {
    Json(json!({ "beacon_ui_response": Value::Null }))
}

async fn beacons_event() -> Json<Value> {
    Json(json!({ "ok": true }))
}

async fn tasks_list() -> Json<Value> {
    Json(json!({
        "items": [],
        "cursor": Value::Null,
    }))
}

async fn wham_environments() -> Json<Value> {
    Json(json!({
        "items": [],
        "cursor": Value::Null,
    }))
}

async fn wham_apps() -> Json<Value> {
    Json(json!({
        "items": [],
        "cursor": Value::Null,
    }))
}

async fn connectors_directory_list() -> Json<Value> {
    Json(json!({
        "apps": [],
        "nextToken": Value::Null,
    }))
}

async fn analytics_events() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn accounts_mfa_info() -> Json<Value> {
    Json(json!({ "mfa_enabled_v2": true }))
}

async fn remote_control_mfa_requirement() -> Json<Value> {
    Json(json!({ "requirement": "not_required" }))
}

async fn remote_control_clients(State(state): State<SharedState>) -> Json<Value> {
    let remote = state.remote_control.inner.lock().await;
    let mut items = remote
        .authorized_clients
        .values()
        .map(remote_control_client_item)
        .collect::<Vec<_>>();
    let feishu_revoked = remote.revoked_clients.contains(FEISHU_BRIDGE_CLIENT_ID);
    drop(remote);

    let config = state.config.lock().await.clone();
    if config.bridge.enabled
        && !config.feishu.app_id.trim().is_empty()
        && !feishu_revoked
        && !items.iter().any(|item| {
            item.get("client_id").and_then(Value::as_str) == Some(FEISHU_BRIDGE_CLIENT_ID)
        })
    {
        let ws = state.feishu_ws.lock().await.clone();
        items.push(feishu_bridge_client_item(ws.connected));
    }
    items.sort_by(|left, right| {
        left.get("display_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    let aliases = items.clone();
    chain_log::write_line(format!(
        "[remote_control] event=clients_list count={} bridge_enabled={} feishu_configured={} feishu_revoked={}",
        items.len(),
        config.bridge.enabled,
        !config.feishu.app_id.trim().is_empty(),
        feishu_revoked
    ));
    Json(json!({
        "items": items,
        "clients": aliases.clone(),
        "data": aliases,
        "cursor": Value::Null,
    }))
}

async fn delete_remote_control_client(
    State(state): State<SharedState>,
    AxumPath(client_id): AxumPath<String>,
) -> StatusCode {
    let mut remote = state.remote_control.inner.lock().await;
    remote.authorized_clients.remove(&client_id);
    if client_id == FEISHU_BRIDGE_CLIENT_ID {
        remote.revoked_clients.insert(client_id);
    }
    StatusCode::NO_CONTENT
}

async fn remote_control_environments(State(state): State<SharedState>) -> Json<Value> {
    let snapshot = status_snapshot(&state).await;
    let items = if snapshot.connected {
        vec![remote_control_environment_item(&snapshot)]
    } else {
        Vec::new()
    };
    Json(json!({
        "items": items,
        "cursor": Value::Null,
    }))
}

async fn rename_remote_control_environment(
    State(state): State<SharedState>,
    AxumPath(_env_id): AxumPath<String>,
    Json(request): Json<RenameEnvironmentRequest>,
) -> Json<Value> {
    if let Some(name) = request.name.map(|name| name.trim().to_string())
        && !name.is_empty()
    {
        state.remote_control.inner.lock().await.server_name = Some(name);
    }
    let snapshot = status_snapshot(&state).await;
    Json(remote_control_environment_item(&snapshot))
}

async fn delete_remote_control_environment(AxumPath(_env_id): AxumPath<String>) -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn remote_control_client_enroll_start(headers: HeaderMap) -> impl IntoResponse {
    Json(remote_control_client_start_response(
        &headers,
        None,
        "enroll/finish",
        None,
    ))
}

async fn remote_control_client_refresh_start(
    State(state): State<SharedState>,
    headers: HeaderMap,
    payload: Option<Json<RemoteControlClientFinishRequest>>,
) -> impl IntoResponse {
    let client_id = payload.map(|Json(value)| value.client_id);
    let device_identity_hash = if let Some(client_id) = client_id.as_deref() {
        state
            .remote_control
            .inner
            .lock()
            .await
            .authorized_clients
            .get(client_id)
            .and_then(|client| {
                client
                    .device_identity
                    .as_ref()
                    .and_then(remote_control_device_identity_hash)
            })
    } else {
        None
    };
    Json(remote_control_client_start_response(
        &headers,
        client_id,
        "refresh/finish",
        device_identity_hash,
    ))
}

async fn remote_control_client_enroll_finish(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<RemoteControlClientFinishRequest>,
) -> impl IntoResponse {
    let token = remote_control_client_token_response(&headers, &request.client_id);
    remember_remote_control_client(&state, &headers, &request).await;
    Json(token)
}

async fn remote_control_client_refresh_finish(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<RemoteControlClientFinishRequest>,
) -> impl IntoResponse {
    let token = remote_control_client_token_response(&headers, &request.client_id);
    remember_remote_control_client(&state, &headers, &request).await;
    Json(token)
}

async fn client_websocket(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(|mut socket| async move {
        while let Some(message) = socket.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let _ = socket.send(Message::Text(text)).await;
                }
                Ok(Message::Ping(payload)) => {
                    let _ = socket.send(Message::Pong(payload)).await;
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    })
}

fn remote_control_client_start_response(
    headers: &HeaderMap,
    client_id: Option<String>,
    finish_path: &str,
    device_identity_hash: Option<String>,
) -> Value {
    let account_user_id = remote_control_account_user_id(headers);
    let client_id = client_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| stable_id("client", &account_user_id));
    let challenge = remote_control_device_key_challenge(
        headers,
        &account_user_id,
        &client_id,
        finish_path,
        device_identity_hash,
    );
    json!({
        "client_id": client_id,
        "account_user_id": account_user_id,
        "device_key_challenge": challenge,
    })
}

fn remote_control_client_token_response(headers: &HeaderMap, client_id: &str) -> Value {
    let account_user_id = remote_control_account_user_id(headers);
    let expires_at = iso8601_after(Duration::from_secs(24 * 60 * 60));
    json!({
        "client_id": client_id,
        "account_user_id": account_user_id,
        "remote_control_token": local_remote_control_client_token(headers, client_id),
        "expires_at": expires_at,
        "scopes": ["remote_control_controller_websocket"],
    })
}

fn remote_control_device_key_challenge(
    headers: &HeaderMap,
    account_user_id: &str,
    client_id: &str,
    finish_path: &str,
    device_identity_hash: Option<String>,
) -> Value {
    let challenge_id = stable_id(
        "challenge",
        &format!("{account_user_id}:{client_id}:{finish_path}"),
    );
    let target_origin = request_origin(headers);
    json!({
        "challenge_id": challenge_id,
        "challenge_token": local_remote_control_client_token(headers, client_id),
        "nonce": stable_base64url_32("nonce", &challenge_id),
        "purpose": "remote_control_client_enrollment",
        "audience": "remote_control_client_enrollment",
        "account_user_id": account_user_id,
        "client_id": client_id,
        "target_origin": target_origin,
        "target_path": format!("/backend-api/codex/remote/control/client/{finish_path}"),
        "challenge_expires_at": iso8601_after(Duration::from_secs(5 * 60)),
        "device_identity_hash": device_identity_hash,
    })
}

async fn remember_remote_control_client(
    state: &SharedState,
    headers: &HeaderMap,
    request: &RemoteControlClientFinishRequest,
) {
    let account_user_id = remote_control_account_user_id(headers);
    let display_name = "Codex App Remote Control".to_string();
    let device_identity = request
        .device_identity
        .as_ref()
        .map(remote_control_device_identity_json);
    let mut remote = state.remote_control.inner.lock().await;
    remote.authorized_clients.insert(
        request.client_id.clone(),
        AuthorizedRemoteControlClient {
            client_id: request.client_id.clone(),
            account_user_id,
            device_identity,
            display_name,
            last_seen_at_ms: unix_now_u64().saturating_mul(1000),
        },
    );
}

fn local_remote_control_client_token(headers: &HeaderMap, client_id: &str) -> String {
    let now = unix_now_u64();
    let account_id = header_str(headers, "chatgpt-account-id")
        .unwrap_or_else(|| "acct_codex_remote_local".into());
    let account_user_id = remote_control_account_user_id(headers);
    let payload = json!({
        "iss": "codex-remote-local",
        "aud": "remote_control_controller_websocket",
        "iat": now,
        "nbf": now,
        "exp": now + 24 * 60 * 60,
        "sub": client_id,
        "scope": "remote_control_controller_websocket",
        "scp": ["remote_control_controller_websocket"],
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "account_id": account_id,
            "chatgpt_account_user_id": account_user_id,
            "account_user_id": account_user_id,
            "user_id": jwt_bearer_claim(headers, "user_id")
                .or_else(|| jwt_bearer_claim(headers, "chatgpt_user_id"))
                .unwrap_or_else(|| "user_codex_remote_local".into()),
        },
    });
    format!(
        "{}.{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({ "alg": "none", "typ": "JWT" })).unwrap_or_default()
        ),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap_or_default()),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
    )
}

fn local_remote_control_server_token(headers: &HeaderMap, server_id: &str) -> String {
    let now = unix_now_u64();
    let account_id = header_str(headers, "chatgpt-account-id")
        .unwrap_or_else(|| "acct_codex_remote_local".into());
    let account_user_id = remote_control_account_user_id(headers);
    let payload = json!({
        "iss": "codex-remote-local",
        "aud": "remote_control_server_websocket",
        "iat": now,
        "nbf": now,
        "exp": now + 24 * 60 * 60,
        "sub": server_id,
        "scope": "remote_control_server_websocket",
        "scp": ["remote_control_server_websocket"],
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "account_id": account_id,
            "chatgpt_account_user_id": account_user_id,
            "account_user_id": account_user_id,
            "user_id": jwt_bearer_claim(headers, "user_id")
                .or_else(|| jwt_bearer_claim(headers, "chatgpt_user_id"))
                .unwrap_or_else(|| "user_codex_remote_local".into()),
        },
    });
    format!(
        "{}.{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({ "alg": "none", "typ": "JWT" })).unwrap_or_default()
        ),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap_or_default()),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
    )
}

fn remote_control_account_user_id(headers: &HeaderMap) -> String {
    jwt_bearer_claim(headers, "chatgpt_account_user_id")
        .or_else(|| jwt_bearer_claim(headers, "account_user_id"))
        .or_else(|| jwt_bearer_claim(headers, "user_id"))
        .unwrap_or_else(|| "user_codex_remote_local__acct_codex_remote_local".into())
}

fn jwt_bearer_claim(headers: &HeaderMap, claim: &str) -> Option<String> {
    let token = authorization_bearer(headers)?;
    jwt_payload(&token).and_then(|payload| {
        payload
            .get("https://api.openai.com/auth")?
            .get(claim)?
            .as_str()
            .map(str::to_string)
    })
}

fn authorization_bearer(headers: &HeaderMap) -> Option<String> {
    let value = header_str(headers, "authorization")?;
    let mut parts = value.splitn(2, char::is_whitespace);
    let scheme = parts.next()?;
    let token = parts.next()?.trim();
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.to_string())
}

fn request_origin(headers: &HeaderMap) -> String {
    header_str(headers, "origin")
        .or_else(|| header_str(headers, "referer").and_then(|value| origin_from_url(&value)))
        .or_else(|| header_str(headers, "host").map(|host| format!("http://{host}")))
        .unwrap_or_else(default_request_origin)
}

#[cfg(target_os = "windows")]
fn default_request_origin() -> String {
    "http://127.0.0.1:3847".into()
}

#[cfg(not(target_os = "windows"))]
fn default_request_origin() -> String {
    "http://127.0.0.1:3847".into()
}

fn origin_from_url(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    Some(url.origin().ascii_serialization())
}

fn iso8601_after(duration: Duration) -> String {
    let timestamp = unix_now_u64().saturating_add(duration.as_secs());
    format_rfc3339_utc(timestamp)
}

fn format_rfc3339_utc(timestamp: u64) -> String {
    let days = (timestamp / 86_400) as i64;
    let seconds_of_day = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
}

fn unix_now_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn remote_control_environment_item(snapshot: &RemoteControlStatusResponse) -> Value {
    let host_name = snapshot
        .server_name
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("飞书 Bridge ({})", local_host_name()));

    json!({
        "id": FEISHU_BRIDGE_ENV_ID,
        "hostId": FEISHU_BRIDGE_ENV_ID,
        "host_id": FEISHU_BRIDGE_ENV_ID,
        "envId": FEISHU_BRIDGE_ENV_ID,
        "env_id": FEISHU_BRIDGE_ENV_ID,
        "displayName": host_name,
        "display_name": host_name,
        "hostName": host_name,
        "host_name": host_name,
        "name": host_name,
        "title": host_name,
        "kind": "remote-control",
        "type": "remote-control",
        "clientType": "CODEX_DESKTOP_APP",
        "client_type": "CODEX_DESKTOP_APP",
        "online": snapshot.connected,
        "busy": snapshot.current_turn_id.is_some(),
        "os": local_platform_os(),
        "arch": local_arch(),
        "appServerVersion": env!("CARGO_PKG_VERSION"),
        "app_server_version": env!("CARGO_PKG_VERSION"),
        "installationId": FEISHU_BRIDGE_INSTALLATION_ID,
        "installation_id": FEISHU_BRIDGE_INSTALLATION_ID,
        "autoConnect": true,
        "auto_connect": true,
        "lastSeenAt": Value::Null,
        "last_seen_at": Value::Null,
    })
}

fn remote_control_client_item(client: &AuthorizedRemoteControlClient) -> Value {
    remote_control_client_json(RemoteControlClientJson {
        client_id: client.client_id.clone(),
        account_user_id: client.account_user_id.clone(),
        display_name: client.display_name.clone(),
        device_model: local_device_model(),
        device_type: local_device_type(),
        platform: local_client_platform(),
        client_type: "CODEX_DESKTOP_APP",
        enrollment_status: "enrolled",
        online: true,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        last_seen_at: format_rfc3339_utc(client.last_seen_at_ms / 1000),
    })
}

fn feishu_bridge_client_item(connected: bool) -> Value {
    remote_control_client_json(RemoteControlClientJson {
        client_id: FEISHU_BRIDGE_CLIENT_ID.to_string(),
        account_user_id: "user_codex_remote_local__acct_codex_remote_local".to_string(),
        display_name: "飞书 Bridge".to_string(),
        device_model: local_device_model(),
        device_type: local_device_type(),
        platform: local_client_platform(),
        client_type: "CODEX_DESKTOP_APP",
        enrollment_status: "enrolled",
        online: connected,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        last_seen_at: format_rfc3339_utc(unix_now_u64()),
    })
}

struct RemoteControlClientJson {
    client_id: String,
    account_user_id: String,
    display_name: String,
    device_model: String,
    device_type: &'static str,
    platform: String,
    client_type: &'static str,
    enrollment_status: &'static str,
    online: bool,
    app_version: String,
    last_seen_at: String,
}

fn remote_control_client_json(client: RemoteControlClientJson) -> Value {
    let status = if client.online { "online" } else { "offline" };
    json!({
        "id": client.client_id,
        "client_id": client.client_id,
        "clientId": client.client_id,
        "account_user_id": client.account_user_id,
        "accountUserId": client.account_user_id,
        "enrollment_status": client.enrollment_status,
        "enrollmentStatus": client.enrollment_status,
        "display_name": client.display_name,
        "displayName": client.display_name,
        "name": client.display_name,
        "title": client.display_name,
        "device_type": client.device_type,
        "deviceType": client.device_type,
        "platform": client.platform,
        "os": client.platform,
        "os_version": Value::Null,
        "device_model": client.device_model,
        "deviceModel": client.device_model,
        "device_name": client.display_name,
        "deviceName": client.display_name,
        "client_type": client.client_type,
        "clientType": client.client_type,
        "status": status,
        "online": client.online,
        "app_version": client.app_version,
        "appVersion": client.app_version,
        "last_seen_at": client.last_seen_at,
        "lastSeenAt": client.last_seen_at,
        "last_used_at": client.last_seen_at,
        "lastUsedAt": client.last_seen_at,
        "last_seen_city": Value::Null,
        "lastSeenCity": Value::Null,
        "last_seen_region_code": Value::Null,
        "lastSeenRegionCode": Value::Null,
        "last_seen_country": Value::Null,
        "lastSeenCountry": Value::Null,
    })
}

#[cfg(target_os = "macos")]
fn local_client_platform() -> String {
    "macintosh".to_string()
}

#[cfg(target_os = "windows")]
fn local_client_platform() -> String {
    "windows".to_string()
}

#[cfg(target_os = "linux")]
fn local_client_platform() -> String {
    "linux".to_string()
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn local_client_platform() -> String {
    local_platform_os()
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn local_device_type() -> &'static str {
    "desktop"
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn local_device_type() -> &'static str {
    "desktop"
}

fn local_device_model() -> String {
    format!("{} {}", local_platform_os(), local_arch())
}

fn remote_control_device_identity_json(identity: &RemoteControlDeviceIdentity) -> Value {
    json!({
        "algorithm": identity.algorithm.as_str(),
        "key_id": identity.key_id.as_str(),
        "protection_class": identity.protection_class.as_str(),
        "public_key_spki_der_base64": identity.public_key_spki_der_base64.as_str(),
    })
}

fn remote_control_device_identity_hash(identity: &Value) -> Option<String> {
    let algorithm = identity.get("algorithm")?.as_str()?;
    let key_id = identity.get("key_id")?.as_str()?;
    let protection_class = identity.get("protection_class")?.as_str()?;
    let public_key_spki_der_base64 = identity.get("public_key_spki_der_base64")?.as_str()?;
    let canonical = json!({
        "algorithm": algorithm,
        "keyId": key_id,
        "protectionClass": protection_class,
        "publicKeySpkiDerBase64": public_key_spki_der_base64,
    });
    Some(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(canonical.to_string().as_bytes())),
    )
}

fn local_host_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Codex App".to_string())
}

fn local_platform_os() -> String {
    match std::env::consts::OS {
        "macos" => "darwin".to_string(),
        other => other.to_string(),
    }
}

fn local_arch() -> String {
    match std::env::consts::ARCH {
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

async fn enroll(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<EnrollRequest>,
) -> impl IntoResponse {
    let installation_id = request
        .installation_id
        .clone()
        .or_else(|| header_str(&headers, "x-codex-installation-id"))
        .unwrap_or_else(|| "unknown-installation".to_string());
    let server_id = stable_id("srv", &installation_id);
    let environment_id = stable_id("env", &installation_id);
    let expires_at = iso8601_after(Duration::from_secs(24 * 60 * 60));
    let remote_control_token = local_remote_control_server_token(&headers, &server_id);
    {
        let mut remote = state.remote_control.inner.lock().await;
        remote.server_id = Some(server_id.clone());
        remote.environment_id = Some(environment_id.clone());
        remote.server_name = request.name.clone();
        remote.installation_id = Some(installation_id.clone());
        remote.account_id = header_str(&headers, "chatgpt-account-id");
        remote.last_error = None;
    }
    state
        .push_event(
            "info",
            "remote_control_enrolled",
            format!(
                "server={} env={} name={} os={} arch={} version={}",
                server_id,
                environment_id,
                request.name.unwrap_or_default(),
                request.os.unwrap_or_default(),
                request.arch.unwrap_or_default(),
                request.app_server_version.unwrap_or_default()
            ),
        )
        .await;
    (
        StatusCode::OK,
        Json(EnrollResponse {
            server_id,
            environment_id,
            remote_control_token,
            expires_at,
        }),
    )
}

async fn refresh(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<RefreshRequest>,
) -> impl IntoResponse {
    let installation_id = request
        .installation_id
        .clone()
        .or_else(|| header_str(&headers, "x-codex-installation-id"))
        .unwrap_or_else(|| "unknown-installation".to_string());
    let expected_server_id = stable_id("srv", &installation_id);
    let server_id = request
        .server_id
        .clone()
        .unwrap_or(expected_server_id.clone());
    if server_id != expected_server_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "remote control server refresh returned mismatched enrollment: expected server_id={expected_server_id}; got server_id={server_id}"
                )
            })),
        );
    }
    let environment_id = stable_id("env", &installation_id);
    let expires_at = iso8601_after(Duration::from_secs(24 * 60 * 60));
    let remote_control_token = local_remote_control_server_token(&headers, &server_id);
    {
        let mut remote = state.remote_control.inner.lock().await;
        remote.server_id = Some(server_id.clone());
        remote.environment_id = Some(environment_id.clone());
        remote.installation_id = Some(installation_id.clone());
        remote.account_id =
            header_str(&headers, "chatgpt-account-id").or(remote.account_id.clone());
        remote.last_error = None;
    }
    state
        .push_event(
            "info",
            "remote_control_refreshed",
            format!("server={} env={}", server_id, environment_id),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({
            "server_id": server_id,
            "environment_id": environment_id,
            "remote_control_token": remote_control_token,
            "expires_at": expires_at,
        })),
    )
}

async fn ensure_remote_control_client_initialized(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
) -> Result<()> {
    let client_key = normalize_remote_client_key(client_key);
    let should_initialize = {
        let mut remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch || !remote.connected {
            false
        } else {
            let client = ensure_client_state_locked(&mut remote, &client_key);
            if client.initialized
                || client
                    .pending
                    .values()
                    .any(|pending| pending.method == "initialize")
            {
                false
            } else {
                client.last_app_pong_status = None;
                if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                    sync_default_client_legacy_locked(&mut remote);
                }
                true
            }
        }
    };
    if should_initialize {
        send_initialize_for_client(state, &client_key).await?;
    }
    Ok(())
}

async fn ensure_remote_control_client_ready(state: &SharedState, client_key: &str) -> Result<()> {
    let connection_epoch = {
        let remote = state.remote_control.inner.lock().await;
        if !remote.connected {
            return Err(anyhow!(
                "Codex app-server remote-control 尚未连接。请在项目目录运行 codex，确认它已经连接到 codex-remote 的 /backend-api。"
            ));
        }
        remote.connection_epoch
    };
    ensure_remote_control_client_initialized(state, connection_epoch, client_key).await?;
    wait_for_remote_control_initialized(state, client_key).await
}

async fn wait_for_recovery_if_needed(state: &SharedState, client_key: &str) -> Result<()> {
    let client_key = normalize_remote_client_key(client_key);
    let recovering = {
        let remote = state.remote_control.inner.lock().await;
        remote
            .clients
            .get(&client_key)
            .is_some_and(|client| client.recovery_started_at_ms.is_some())
    };
    if recovering {
        wait_for_remote_control_initialized(state, &client_key).await?;
    }
    Ok(())
}

async fn replay_pending_requests(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
) -> Result<()> {
    let client_key = normalize_remote_client_key(client_key);
    let pending = {
        let remote = state.remote_control.inner.lock().await;
        let Some(client) = remote.clients.get(&client_key) else {
            return Ok(());
        };
        if remote.connection_epoch != connection_epoch || !remote.connected || !client.initialized {
            return Ok(());
        }
        client
            .pending
            .values()
            .filter(|pending| pending.method != "initialize")
            .map(|pending| {
                (
                    pending.method.clone(),
                    request_id_key(pending.message.get("id").unwrap_or(&Value::Null)),
                    pending.envelopes.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    for (method, request_key, envelopes) in pending {
        chain_log::write_line(format!(
            "[remote_control] event=pending_replay request_key={} method={} envelope_count={}",
            request_key,
            method,
            envelopes.len()
        ));
        send_envelopes(state, envelopes).await?;
    }
    Ok(())
}

async fn reset_remote_control_client_for_key(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
) -> Result<()> {
    let client_key = normalize_remote_client_key(client_key);
    let (pending, client_id, stream_id, pending_summary) = {
        let mut remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch {
            return Ok(());
        }
        let client = ensure_client_state_locked(&mut remote, &client_key);
        client.initialized = false;
        let client_id = client.client_id.clone();
        let stream_id = client.stream_id.clone();
        let pending_summary = pending_requests_summary(&client.pending);
        let pending = std::mem::take(&mut client.pending);
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
        (pending, client_id, stream_id, pending_summary)
    };
    chain_log::write_line(format!(
        "[remote_control] event=remote_control_client_reset connection_epoch={} client_key={} client_id={} stream_id={} pending_count={} pending={}",
        connection_epoch,
        client_key,
        client_id,
        stream_id,
        pending.len(),
        pending_summary
    ));
    for (_, pending) in pending {
        let _ = pending
            .response_tx
            .send(Err(anyhow!(REMOTE_CONTROL_CLIENT_REINITIALIZED_ERROR)));
    }
    send_initialize_for_client(state, &client_key)
        .await
        .map(|_| ())
}

async fn start_remote_control_client_recovery(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    client_id: &str,
    stream_id: &str,
) -> Result<()> {
    let client_key = normalize_remote_client_key(client_key);
    let (attempt, should_spawn) = {
        let mut remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch || !remote.connected {
            return Ok(());
        }
        let client = ensure_client_state_locked(&mut remote, &client_key);
        if client.client_id != client_id || client.stream_id != stream_id {
            return Ok(());
        }
        if client.recovery_started_at_ms.is_some() {
            (client.recovery_attempt, false)
        } else {
            client.recovery_attempt = client.recovery_attempt.saturating_add(1);
            client.recovery_started_at_ms = Some(now_ms());
            client.initialized = false;
            client.last_app_pong_status = Some("unknown".to_string());
            let attempt = client.recovery_attempt;
            if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                sync_default_client_legacy_locked(&mut remote);
            }
            (attempt, true)
        }
    };
    if !should_spawn {
        chain_log::write_line(format!(
            "[remote_control] event=recovery_already_running connection_epoch={} client_key={} client_id={} stream_id={} attempt={}",
            connection_epoch, client_key, client_id, stream_id, attempt
        ));
        return Ok(());
    }
    chain_log::write_line(format!(
        "[remote_control] event=recovery_start connection_epoch={} client_key={} client_id={} stream_id={} attempt={} strategy=same_stream_reinitialize",
        connection_epoch, client_key, client_id, stream_id, attempt
    ));
    state
        .push_event(
            "warn",
            "remote_control_recovery_start",
            format!(
                "client_key={} stream_id={} attempt={} strategy=same_stream_reinitialize",
                client_key, stream_id, attempt
            ),
        )
        .await;
    let recovery_state = state.clone();
    tokio::spawn(async move {
        if let Err(err) =
            run_remote_control_client_recovery(recovery_state.clone(), connection_epoch, client_key)
                .await
        {
            recovery_state
                .push_event("error", "remote_control_recovery_failed", err.to_string())
                .await;
        }
    });
    Ok(())
}

async fn run_remote_control_client_recovery(
    state: SharedState,
    connection_epoch: u64,
    client_key: String,
) -> Result<()> {
    reset_remote_control_client_for_key(&state, connection_epoch, &client_key).await?;
    let initialize_result = tokio::time::timeout(
        REMOTE_CONTROL_UNKNOWN_REINITIALIZE_TIMEOUT,
        wait_for_remote_control_initialized(&state, &client_key),
    )
    .await;
    match initialize_result {
        Ok(Ok(())) => {
            let (client_id, stream_id, attempt) =
                finish_remote_control_client_recovery(&state, connection_epoch, &client_key)
                    .await?;
            chain_log::write_line(format!(
                "[remote_control] event=recovery_ready connection_epoch={} client_key={} client_id={} stream_id={} attempt={}",
                connection_epoch, client_key, client_id, stream_id, attempt
            ));
            state
                .push_event(
                    "info",
                    "remote_control_recovery_ready",
                    format!(
                        "client_key={} stream_id={} attempt={}",
                        client_key, stream_id, attempt
                    ),
                )
                .await;
            if let Err(err) = resubscribe_current_thread_after_recovery(
                &state,
                connection_epoch,
                &client_key,
                attempt,
            )
            .await
            {
                chain_log::write_line(format!(
                    "[remote_control] event=recovery_thread_resubscribe_failed connection_epoch={} client_key={} attempt={} err={}",
                    connection_epoch, client_key, attempt, err
                ));
                state
                    .push_event(
                        "warn",
                        "remote_control_recovery_thread_resubscribe_failed",
                        format!("client_key={} attempt={} err={}", client_key, attempt, err),
                    )
                    .await;
            }
            Ok(())
        }
        Ok(Err(err)) => {
            chain_log::write_line(format!(
                "[remote_control] event=recovery_initialize_failed connection_epoch={} client_key={} err={}",
                connection_epoch, client_key, err
            ));
            force_remote_control_ws_reconnect(
                &state,
                connection_epoch,
                &client_key,
                "same-stream initialize failed",
            )
            .await
        }
        Err(_) => {
            chain_log::write_line(format!(
                "[remote_control] event=recovery_initialize_timeout connection_epoch={} client_key={} timeout_ms={}",
                connection_epoch,
                client_key,
                REMOTE_CONTROL_UNKNOWN_REINITIALIZE_TIMEOUT.as_millis()
            ));
            force_remote_control_ws_reconnect(
                &state,
                connection_epoch,
                &client_key,
                "same-stream initialize timed out",
            )
            .await
        }
    }
}

async fn finish_remote_control_client_recovery(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
) -> Result<(String, String, u64)> {
    let mut remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch != connection_epoch {
        return Err(anyhow!("remote-control recovery epoch changed"));
    }
    let client = remote
        .clients
        .get_mut(client_key)
        .ok_or_else(|| anyhow!("remote-control recovery client disappeared: {client_key}"))?;
    let client_id = client.client_id.clone();
    let stream_id = client.stream_id.clone();
    let attempt = client.recovery_attempt;
    client.recovery_started_at_ms = None;
    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
        sync_default_client_legacy_locked(&mut remote);
    }
    remote.last_error = None;
    Ok((client_id, stream_id, attempt))
}

async fn resubscribe_current_thread_after_recovery(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    attempt: u64,
) -> Result<()> {
    let client_key = normalize_remote_client_key(client_key);
    let Some((thread_id, turn_id, client_id, stream_id)) = ({
        let remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch || !remote.connected {
            None
        } else {
            remote.clients.get(&client_key).and_then(|client| {
                if client.initialized {
                    client.current_thread_id.clone().map(|thread_id| {
                        (
                            thread_id,
                            client.current_turn_id.clone(),
                            client.client_id.clone(),
                            client.stream_id.clone(),
                        )
                    })
                } else {
                    None
                }
            })
        }
    }) else {
        chain_log::write_line(format!(
            "[remote_control] event=recovery_thread_resubscribe_skipped connection_epoch={} client_key={} attempt={} reason=no_current_thread",
            connection_epoch, client_key, attempt
        ));
        return Ok(());
    };

    chain_log::write_line(format!(
        "[remote_control] event=recovery_thread_resubscribe_start connection_epoch={} client_key={} client_id={} stream_id={} thread={} turn={} attempt={} method=thread/resume exclude_turns=true",
        connection_epoch,
        client_key,
        client_id,
        stream_id,
        thread_id,
        turn_id.as_deref().unwrap_or(""),
        attempt
    ));
    state
        .push_event(
            "info",
            "remote_control_recovery_thread_resubscribe_start",
            format!(
                "client_key={} thread={} turn={} attempt={}",
                client_key,
                thread_id,
                turn_id.as_deref().unwrap_or(""),
                attempt
            ),
        )
        .await;

    let response = request_with_timeout_for_client(
        state,
        &client_key,
        "thread/resume",
        json!({
            "threadId": thread_id.clone(),
            "excludeTurns": true,
        }),
        REMOTE_DISCOVERY_REQUEST_TIMEOUT,
    )
    .await?;
    let status_type = response
        .get("thread")
        .and_then(thread_status_type_from_payload)
        .or_else(|| thread_status_type_from_payload(&response))
        .unwrap_or_default();
    chain_log::write_line(format!(
        "[remote_control] event=recovery_thread_resubscribe_ready connection_epoch={} client_key={} thread={} turn={} attempt={} status={}",
        connection_epoch,
        client_key,
        thread_id,
        turn_id.as_deref().unwrap_or(""),
        attempt,
        status_type
    ));
    state
        .push_event(
            "info",
            "remote_control_recovery_thread_resubscribe_ready",
            format!(
                "client_key={} thread={} turn={} attempt={} status={}",
                client_key,
                thread_id,
                turn_id.as_deref().unwrap_or(""),
                attempt,
                status_type
            ),
        )
        .await;
    if is_terminal_or_inactive_thread_status(&status_type) {
        observe_thread_status_changed(state, Some(&client_key), &thread_id, &status_type).await;
    }
    Ok(())
}

async fn force_remote_control_ws_reconnect(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    reason: &str,
) -> Result<()> {
    let outbound_tx = {
        let mut remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch || !remote.connected {
            return Ok(());
        }
        remote.last_error = Some(format!(
            "remote-control recovery forcing websocket reconnect: client_key={} reason={}",
            client_key, reason
        ));
        remote
            .outbound_tx
            .clone()
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    chain_log::write_line(format!(
        "[remote_control] event=force_ws_reconnect connection_epoch={} client_key={} reason={}",
        connection_epoch, client_key, reason
    ));
    outbound_tx
        .send(OutboundWsMessage::Close(reason.to_string()))
        .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    Ok(())
}

async fn mark_server_envelope_acked(
    state: &SharedState,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: Option<usize>,
) {
    let mut remote = state.remote_control.inner.lock().await;
    let key = server_ack_cursor_key(client_id, stream_id);
    let next = (seq_id, segment_id);
    let should_update = remote
        .server_ack_cursors
        .get(&key)
        .is_none_or(|current| ack_cursor_gt(next, *current));
    if should_update {
        remote.server_ack_cursors.insert(key, next);
    }
}

async fn next_remote_subscribe_cursor(state: &SharedState) -> String {
    let cursor = format!(
        "codex-remote:{}",
        REMOTE_SUBSCRIBE_CURSOR_ID.fetch_add(1, Ordering::Relaxed)
    );
    let mut remote = state.remote_control.inner.lock().await;
    remote.subscribe_cursor = Some(cursor.clone());
    cursor
}

async fn is_duplicate_server_envelope(
    state: &SharedState,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: Option<usize>,
) -> bool {
    let remote = state.remote_control.inner.lock().await;
    let key = server_ack_cursor_key(client_id, stream_id);
    remote
        .server_ack_cursors
        .get(&key)
        .is_some_and(|current| !ack_cursor_gt((seq_id, segment_id), *current))
}

fn server_ack_cursor_key(client_id: &str, stream_id: &str) -> String {
    format!("{client_id}\n{stream_id}")
}

fn ack_cursor_gt(left: (u64, Option<usize>), right: (u64, Option<usize>)) -> bool {
    let left = (left.0, left.1.unwrap_or(usize::MAX));
    let right = (right.0, right.1.unwrap_or(usize::MAX));
    left > right
}

async fn next_client_envelope_parts(
    state: &SharedState,
    client_key: &str,
) -> Result<(String, String, u64)> {
    let client_key = normalize_remote_client_key(client_key);
    let mut remote = state.remote_control.inner.lock().await;
    if !remote.connected {
        return Err(anyhow!("remote-control websocket is not connected"));
    }
    let client = ensure_client_state_locked(&mut remote, &client_key);
    let seq_id = client.next_seq_id;
    client.next_seq_id = client.next_seq_id.saturating_add(1);
    let parts = (client.client_id.clone(), client.stream_id.clone(), seq_id);
    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
        sync_default_client_legacy_locked(&mut remote);
    }
    Ok(parts)
}

fn build_pending_message(method: &str, id: u64, params: Value) -> Value {
    json!({ "id": id, "method": method, "params": params })
}

async fn websocket(
    State(state): State<SharedState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let protocol_version = header_str(&headers, "x-codex-protocol-version").unwrap_or_default();
    if protocol_version != PROTOCOL_VERSION {
        state
            .push_event(
                "warn",
                "remote_control_protocol_version",
                format!("expected={} got={}", PROTOCOL_VERSION, protocol_version),
            )
            .await;
    }
    ws.max_message_size(128 << 20)
        .on_upgrade(move |socket| async move {
            if let Err(err) = run_websocket(state.clone(), headers, socket).await {
                let message = err.to_string();
                {
                    let mut remote = state.remote_control.inner.lock().await;
                    remote.connected = false;
                    remote.outbound_tx = None;
                    remote.last_error = Some(message.clone());
                }
                state
                    .push_event("error", "remote_control_ws_failed", message)
                    .await;
            }
        })
}

async fn run_websocket(state: SharedState, headers: HeaderMap, socket: WebSocket) -> Result<()> {
    let server_id = header_str(&headers, "x-codex-server-id");
    let server_name = header_str(&headers, "x-codex-name")
        .and_then(|value| base64::engine::general_purpose::STANDARD.decode(value).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let installation_id = header_str(&headers, "x-codex-installation-id");
    let account_id = header_str(&headers, "chatgpt-account-id");
    let subscribe_cursor = header_str(&headers, "x-codex-subscribe-cursor");
    let (outbound_tx, mut outbound_rx) =
        tokio::sync::mpsc::unbounded_channel::<OutboundWsMessage>();
    let (connection_epoch, client_id, stream_id) = {
        let mut remote = state.remote_control.inner.lock().await;
        let connected_at_ms = now_ms();
        remote.connected = true;
        remote.initialized = false;
        remote.connection_epoch = remote.connection_epoch.saturating_add(1);
        if remote.stream_id.is_empty() {
            remote.stream_id = uuid_like();
        }
        let ack_cursor_keys = reset_remote_clients_for_connection_locked(&mut remote);
        for key in ack_cursor_keys {
            remote.server_ack_cursors.remove(&key);
        }
        remote.stream_diagnostics.clear();
        let default_client = ensure_client_state_locked(&mut remote, DEFAULT_REMOTE_CLIENT_KEY);
        let client_id = default_client.client_id.clone();
        let stream_id = default_client.stream_id.clone();
        remote.outbound_tx = Some(outbound_tx);
        remote.server_id = server_id.clone().or(remote.server_id.clone());
        remote.server_name = server_name.clone().or(remote.server_name.clone());
        remote.installation_id = installation_id.clone().or(remote.installation_id.clone());
        remote.account_id = account_id.clone().or(remote.account_id.clone());
        remote.subscribe_cursor = subscribe_cursor.clone();
        remote.last_error = None;
        remote.connected_at_ms = Some(connected_at_ms);
        remote.last_ws_inbound_at_ms = Some(connected_at_ms);
        remote.last_ws_ping_at_ms = None;
        remote.last_ws_pong_at_ms = None;
        remote.last_app_ping_at_ms = None;
        remote.last_app_pong_at_ms = None;
        remote.last_app_pong_status = None;
        remote.last_initialize_sent_at_ms = None;
        sync_default_client_legacy_locked(&mut remote);
        (remote.connection_epoch, client_id, stream_id)
    };
    state
        .push_event(
            "info",
            "remote_control_connected",
            format!(
                "server={} name={} installation={} account={}",
                server_id.as_deref().unwrap_or_default(),
                server_name.as_deref().unwrap_or_default(),
                installation_id.as_deref().unwrap_or_default(),
                account_id.as_deref().unwrap_or_default()
            ),
        )
        .await;
    chain_log::write_line(format!(
        "[remote_control] event=ws_open connection_epoch={} client_id={} stream_id={} server_id={} server_name={} installation_id={} account_id={} subscribe_cursor={}",
        connection_epoch,
        client_id,
        stream_id,
        server_id.as_deref().unwrap_or_default(),
        server_name.as_deref().unwrap_or_default(),
        installation_id.as_deref().unwrap_or_default(),
        account_id.as_deref().unwrap_or_default(),
        subscribe_cursor.as_deref().unwrap_or_default()
    ));
    info!(
        target: "codex_remote::remote_control",
        event = "remote_control_ws_open",
        connection_epoch,
        client_id,
        stream_id,
        server_id = server_id.as_deref().unwrap_or_default(),
        server_name = server_name.as_deref().unwrap_or_default(),
        installation_id = installation_id.as_deref().unwrap_or_default(),
        account_id = account_id.as_deref().unwrap_or_default(),
        "remote-control websocket opened"
    );

    let (mut writer, mut reader) = socket.split();
    initialize_remote_clients_for_connection(&state, connection_epoch).await?;
    let (server_work_tx, server_work_rx) = tokio::sync::mpsc::channel::<RemoteServerWorkItem>(
        REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY,
    );
    let server_work_state = state.clone();
    let server_work_task = tokio::spawn(async move {
        run_remote_server_work_queue(server_work_state, server_work_rx).await;
        Ok::<(), anyhow::Error>(())
    });

    let mut writer_task = tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            match message {
                OutboundWsMessage::Text(value) => {
                    writer
                        .send(Message::Text(value.to_string().into()))
                        .await
                        .map_err(|err| anyhow!("remote-control websocket writer failed: {err}"))?;
                }
                OutboundWsMessage::Ping(data) => {
                    writer
                        .send(Message::Ping(data))
                        .await
                        .map_err(|err| anyhow!("remote-control websocket writer failed: {err}"))?;
                }
                OutboundWsMessage::Pong(data) => {
                    writer
                        .send(Message::Pong(data))
                        .await
                        .map_err(|err| anyhow!("remote-control websocket writer failed: {err}"))?;
                }
                OutboundWsMessage::Close(reason) => {
                    writer
                        .send(Message::Close(None))
                        .await
                        .map_err(|err| anyhow!("remote-control websocket writer failed: {err}"))?;
                    return Err(anyhow!(
                        "remote-control websocket close requested: {reason}"
                    ));
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let reader_state = state.clone();
    let mut reader_task = tokio::spawn(async move {
        let mut ws_ping_interval = tokio::time::interval_at(
            tokio::time::Instant::now() + REMOTE_CONTROL_WEBSOCKET_PING_INTERVAL,
            REMOTE_CONTROL_WEBSOCKET_PING_INTERVAL,
        );
        ws_ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut app_ping_interval = tokio::time::interval_at(
            tokio::time::Instant::now() + REMOTE_CONTROL_APP_PING_INTERVAL,
            REMOTE_CONTROL_APP_PING_INTERVAL,
        );
        app_ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut stale_check_interval = tokio::time::interval(REMOTE_CONTROL_STALE_CHECK_INTERVAL);
        stale_check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut chunks = HashMap::<(String, String, u64), ServerChunkAssembly>::new();
        loop {
            tokio::select! {
                    _ = ws_ping_interval.tick() => {
                        mark_remote_ws_ping(&reader_state, connection_epoch).await;
                        send_ws_control_ping(&reader_state).await?;
                    }
                    _ = app_ping_interval.tick() => {
                        let targets = remote_app_ping_targets(&reader_state, connection_epoch).await;
                        for (client_key, client_id, stream_id) in targets {
                            mark_remote_app_ping(&reader_state, connection_epoch, &client_key).await;
            let cursor = Some(next_remote_subscribe_cursor(&reader_state).await);
            let envelope = json!(OutgoingClientEnvelope {
                event: OutgoingClientEvent::Ping,
                client_id,
                stream_id: Some(stream_id),
                seq_id: None,
                cursor,
            });
                            send_envelope(&reader_state, envelope).await?;
                        }
                    }
                    _ = stale_check_interval.tick() => {
                        if let Some(reason) = remote_control_stale_reason(&reader_state, connection_epoch).await {
                            reader_state
                                .push_event("warn", "remote_control_heartbeat_timeout", reason.clone())
                                .await;
                            return Err(anyhow!("remote-control websocket stale: {reason}"));
                        }
                    }
                    incoming = reader.next() => {
                        let Some(incoming) = incoming else {
                            return Ok(());
                        };
                        match incoming.context("failed to read remote-control websocket")? {
                            Message::Text(text) => {
                                mark_remote_ws_inbound(&reader_state, connection_epoch).await;
                                handle_server_envelope(&reader_state, connection_epoch, &text, &mut chunks, &server_work_tx).await?;
                            }
                            Message::Ping(data) => {
                                mark_remote_ws_inbound(&reader_state, connection_epoch).await;
                                send_ws_control_pong(&reader_state, data).await?;
                            }
                            Message::Pong(_) => {
                                mark_remote_ws_pong(&reader_state, connection_epoch).await;
                            }
                            Message::Binary(_) => {}
                            Message::Close(_) => return Ok::<(), anyhow::Error>(()),
                        }
                    }
                }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    });

    let (connection_result, ended_task): (Result<()>, &'static str) = tokio::select! {
        result = &mut writer_task => (
            match result {
                Ok(inner) => inner,
                Err(err) => Err(anyhow!("remote-control websocket writer task failed: {err}")),
            },
            "writer",
        ),
        result = &mut reader_task => (
            match result {
                Ok(inner) => inner,
                Err(err) => Err(anyhow!("remote-control websocket reader task failed: {err}")),
            },
            "reader",
        ),
    };

    writer_task.abort();
    reader_task.abort();
    server_work_task.abort();
    {
        let mut remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch == connection_epoch {
            remote.connected = false;
            remote.outbound_tx = None;
            remote.last_error = connection_result.as_ref().err().map(|err| err.to_string());
        }
    }
    state
        .push_event(
            "warn",
            "remote_control_disconnected",
            format!(
                "ended_task={} reason={}",
                ended_task,
                connection_result
                    .as_ref()
                    .err()
                    .map(|err| err.to_string())
                    .unwrap_or_else(|| "websocket closed".to_string())
            ),
        )
        .await;
    connection_result
}

async fn initialize_remote_clients_for_connection(
    state: &SharedState,
    connection_epoch: u64,
) -> Result<()> {
    let client_keys = {
        let remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch || !remote.connected {
            return Ok(());
        }
        let mut client_keys = remote.clients.keys().cloned().collect::<Vec<_>>();
        if !client_keys
            .iter()
            .any(|client_key| client_key == DEFAULT_REMOTE_CLIENT_KEY)
        {
            client_keys.push(DEFAULT_REMOTE_CLIENT_KEY.to_string());
        }
        client_keys.sort();
        client_keys
    };
    for client_key in client_keys {
        ensure_remote_control_client_initialized(state, connection_epoch, &client_key).await?;
    }
    Ok(())
}

async fn handle_server_envelope(
    state: &SharedState,
    connection_epoch: u64,
    text: &str,
    chunks: &mut HashMap<(String, String, u64), ServerChunkAssembly>,
    server_work_tx: &tokio::sync::mpsc::Sender<RemoteServerWorkItem>,
) -> Result<()> {
    if !is_active_connection_epoch(state, connection_epoch).await {
        return Ok(());
    }
    let envelope: IncomingServerEnvelope =
        serde_json::from_str(text).with_context(|| format!("invalid server envelope: {text}"))?;
    let IncomingServerEnvelope {
        event,
        client_id,
        stream_id,
        seq_id,
    } = envelope;
    let received_at_ms = now_ms();
    let segment_id = server_event_segment_id(&event);
    let event_kind = server_event_kind(&event);
    let event_summary = server_event_recent_summary(&event);
    observe_server_envelope_window(
        state,
        connection_epoch,
        &client_id,
        &stream_id,
        seq_id,
        received_at_ms,
    )
    .await;
    record_remote_recent_event(
        state,
        "server_in",
        connection_epoch,
        &client_id,
        &stream_id,
        Some(seq_id),
        event_kind,
        event_summary.clone(),
    )
    .await;
    if !matches!(event, IncomingServerEvent::Pong { .. }) {
        chain_log::write_line(format!(
            "[remote_control] event=server_envelope_in connection_epoch={} seq_id={} client_id={} stream_id={} kind={} summary={}",
            connection_epoch, seq_id, client_id, stream_id, event_kind, event_summary
        ));
    }
    if !is_current_remote_stream(state, connection_epoch, &client_id, &stream_id).await {
        ack_server_envelope(
            state,
            connection_epoch,
            &client_id,
            &stream_id,
            seq_id,
            segment_id,
        )
        .await?;
        record_server_ack_diagnostics(
            state,
            connection_epoch,
            &client_id,
            &stream_id,
            seq_id,
            received_at_ms,
        )
        .await;
        mark_server_envelope_acked(state, &client_id, &stream_id, seq_id, segment_id).await;
        observe_stale_server_envelope(
            state,
            connection_epoch,
            seq_id,
            &client_id,
            &stream_id,
            &event,
            true,
        )
        .await;
        return Ok(());
    }
    if is_duplicate_server_envelope(state, &client_id, &stream_id, seq_id, segment_id).await {
        ack_server_envelope(
            state,
            connection_epoch,
            &client_id,
            &stream_id,
            seq_id,
            segment_id,
        )
        .await?;
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[remote_control] event=server_envelope_duplicate connection_epoch={} seq_id={} client_id={} stream_id={} kind={} segment_id={}",
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
                server_event_kind(&event),
                segment_id
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            )
        });
        return Ok(());
    }
    match event {
        IncomingServerEvent::ServerMessage { message } => {
            let summary = message_summary(&message);
            if is_command_execution_output_delta_message(&message) {
                observe_command_output_delta_received(
                    state,
                    connection_epoch,
                    &client_id,
                    &stream_id,
                    seq_id,
                    &message,
                    server_work_tx.capacity(),
                )
                .await;
            }
            enqueue_remote_server_work(
                server_work_tx,
                RemoteServerWorkItem::ServerMessage {
                    connection_epoch,
                    seq_id,
                    client_id: client_id.clone(),
                    stream_id: stream_id.clone(),
                    message,
                },
            )?;
            ack_server_envelope(
                state,
                connection_epoch,
                &client_id,
                &stream_id,
                seq_id,
                None,
            )
            .await?;
            record_server_ack_diagnostics(
                state,
                connection_epoch,
                &client_id,
                &stream_id,
                seq_id,
                received_at_ms,
            )
            .await;
            mark_server_envelope_acked(state, &client_id, &stream_id, seq_id, None).await;
            log_server_envelope_handoff(
                connection_epoch,
                seq_id,
                &client_id,
                &stream_id,
                "server_message",
                "",
                &summary,
            );
        }
        IncomingServerEvent::ServerMessageChunk {
            segment_id,
            segment_count,
            message_size_bytes,
            message_chunk_base64,
        } => {
            let mut summary = String::new();
            let observation = observe_server_chunk(
                chunks,
                &client_id,
                &stream_id,
                seq_id,
                segment_id,
                segment_count,
                message_size_bytes,
                &message_chunk_base64,
            );
            match observation {
                ServerChunkObservation::Complete(message) => {
                    summary = message_summary(&message);
                    enqueue_remote_server_work(
                        server_work_tx,
                        RemoteServerWorkItem::ServerMessage {
                            connection_epoch,
                            seq_id,
                            client_id: client_id.clone(),
                            stream_id: stream_id.clone(),
                            message,
                        },
                    )?;
                }
                ServerChunkObservation::Pending => {}
                ServerChunkObservation::Dropped => {
                    summary = "dropped".to_string();
                    chain_log::write_diagnostic_lazy(|| {
                        format!(
                            "[remote_control] event=server_chunk_dropped connection_epoch={} client_id={} stream_id={} seq_id={} segment={}/{}",
                            connection_epoch,
                            client_id,
                            stream_id,
                            seq_id,
                            segment_id,
                            segment_count
                        )
                    });
                }
            }
            ack_server_envelope(
                state,
                connection_epoch,
                &client_id,
                &stream_id,
                seq_id,
                Some(segment_id),
            )
            .await?;
            record_server_ack_diagnostics(
                state,
                connection_epoch,
                &client_id,
                &stream_id,
                seq_id,
                received_at_ms,
            )
            .await;
            mark_server_envelope_acked(state, &client_id, &stream_id, seq_id, Some(segment_id))
                .await;
            log_server_envelope_handoff(
                connection_epoch,
                seq_id,
                &client_id,
                &stream_id,
                "server_message_chunk",
                &format!("{}/{}", segment_id, segment_count),
                &summary,
            );
        }
        IncomingServerEvent::Ack => {
            enqueue_remote_server_work(
                server_work_tx,
                RemoteServerWorkItem::ServerAck {
                    connection_epoch,
                    seq_id,
                    client_id: client_id.clone(),
                    stream_id: stream_id.clone(),
                },
            )?;
            ack_server_envelope(
                state,
                connection_epoch,
                &client_id,
                &stream_id,
                seq_id,
                None,
            )
            .await?;
            record_server_ack_diagnostics(
                state,
                connection_epoch,
                &client_id,
                &stream_id,
                seq_id,
                received_at_ms,
            )
            .await;
            mark_server_envelope_acked(state, &client_id, &stream_id, seq_id, None).await;
            log_server_envelope_handoff(
                connection_epoch,
                seq_id,
                &client_id,
                &stream_id,
                "ack",
                "",
                "",
            );
        }
        IncomingServerEvent::Pong { status } => {
            let should_reinitialize =
                record_remote_app_pong(state, connection_epoch, &client_id, &stream_id, &status)
                    .await?;
            enqueue_remote_server_work(
                server_work_tx,
                RemoteServerWorkItem::ServerPong {
                    connection_epoch,
                    seq_id,
                    client_id: client_id.clone(),
                    stream_id: stream_id.clone(),
                    status: status.clone(),
                    should_reinitialize,
                },
            )?;
            ack_server_envelope(
                state,
                connection_epoch,
                &client_id,
                &stream_id,
                seq_id,
                None,
            )
            .await?;
            record_server_ack_diagnostics(
                state,
                connection_epoch,
                &client_id,
                &stream_id,
                seq_id,
                received_at_ms,
            )
            .await;
            mark_server_envelope_acked(state, &client_id, &stream_id, seq_id, None).await;
            log_server_envelope_handoff(
                connection_epoch,
                seq_id,
                &client_id,
                &stream_id,
                "pong",
                "",
                &format!("status={status}"),
            );
        }
    }
    Ok(())
}

fn enqueue_remote_server_work(
    server_work_tx: &tokio::sync::mpsc::Sender<RemoteServerWorkItem>,
    item: RemoteServerWorkItem,
) -> Result<()> {
    let kind = remote_server_work_item_kind(&item);
    server_work_tx.try_send(item).map_err(|err| match err {
        tokio::sync::mpsc::error::TrySendError::Full(_) => {
            anyhow!("remote-control server work queue full while enqueueing {kind}")
        }
        tokio::sync::mpsc::error::TrySendError::Closed(_) => {
            anyhow!("remote-control server work queue closed while enqueueing {kind}")
        }
    })
}

fn remote_server_work_item_kind(item: &RemoteServerWorkItem) -> &'static str {
    match item {
        RemoteServerWorkItem::ServerMessage { .. } => "server_message",
        RemoteServerWorkItem::ServerAck { .. } => "ack",
        RemoteServerWorkItem::ServerPong { .. } => "pong",
    }
}

async fn run_remote_server_work_queue(
    state: SharedState,
    mut server_work_rx: tokio::sync::mpsc::Receiver<RemoteServerWorkItem>,
) {
    while let Some(item) = server_work_rx.recv().await {
        match item {
            RemoteServerWorkItem::ServerMessage {
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
                message,
            } => {
                log_server_work_begin(
                    connection_epoch,
                    seq_id,
                    &client_id,
                    &stream_id,
                    "server_message",
                    &message_summary(&message),
                );
                observe_app_server_message(
                    &state,
                    connection_epoch,
                    &client_id,
                    &stream_id,
                    &message,
                )
                .await;
            }
            RemoteServerWorkItem::ServerAck {
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
            } => {
                log_server_work_begin(connection_epoch, seq_id, &client_id, &stream_id, "ack", "");
                state
                    .push_event("info", "remote_control_ack", format!("seq={seq_id}"))
                    .await;
            }
            RemoteServerWorkItem::ServerPong {
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
                status,
                should_reinitialize,
            } => {
                log_server_work_begin(
                    connection_epoch,
                    seq_id,
                    &client_id,
                    &stream_id,
                    "pong",
                    &format!("status={status}"),
                );
                if let Err(err) = handle_remote_app_pong_after_ack(
                    &state,
                    connection_epoch,
                    &client_id,
                    &stream_id,
                    &status,
                    should_reinitialize,
                )
                .await
                {
                    state
                        .push_event("error", "remote_control_pong_failed", err.to_string())
                        .await;
                }
            }
        }
    }
}

fn log_server_envelope_handoff(
    connection_epoch: u64,
    seq_id: u64,
    client_id: &str,
    stream_id: &str,
    kind: &str,
    segment: &str,
    summary: &str,
) {
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[remote_control] event=server_envelope_acked connection_epoch={} seq_id={} client_id={} stream_id={} kind={} segment={} summary={}",
            connection_epoch, seq_id, client_id, stream_id, kind, segment, summary
        )
    });
}

fn log_server_work_begin(
    connection_epoch: u64,
    seq_id: u64,
    client_id: &str,
    stream_id: &str,
    kind: &str,
    summary: &str,
) {
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[remote_control] event=server_work_begin connection_epoch={} seq_id={} client_id={} stream_id={} kind={} summary={}",
            connection_epoch, seq_id, client_id, stream_id, kind, summary
        )
    });
}

fn server_event_segment_id(event: &IncomingServerEvent) -> Option<usize> {
    match event {
        IncomingServerEvent::ServerMessageChunk { segment_id, .. } => Some(*segment_id),
        IncomingServerEvent::ServerMessage { .. }
        | IncomingServerEvent::Ack
        | IncomingServerEvent::Pong { .. } => None,
    }
}

fn server_event_kind(event: &IncomingServerEvent) -> &'static str {
    match event {
        IncomingServerEvent::ServerMessage { .. } => "server_message",
        IncomingServerEvent::ServerMessageChunk { .. } => "server_message_chunk",
        IncomingServerEvent::Ack => "ack",
        IncomingServerEvent::Pong { .. } => "pong",
    }
}

fn server_event_recent_summary(event: &IncomingServerEvent) -> String {
    match event {
        IncomingServerEvent::ServerMessage { message } => message_summary(message),
        IncomingServerEvent::ServerMessageChunk {
            segment_id,
            segment_count,
            message_size_bytes,
            ..
        } => format!(
            "segment={}/{} message_size_bytes={}",
            segment_id, segment_count, message_size_bytes
        ),
        IncomingServerEvent::Ack => String::new(),
        IncomingServerEvent::Pong { status } => format!("status={status}"),
    }
}

async fn observe_stale_server_envelope(
    state: &SharedState,
    connection_epoch: u64,
    seq_id: u64,
    client_id: &str,
    stream_id: &str,
    event: &IncomingServerEvent,
    processed: bool,
) {
    if !matches!(event, IncomingServerEvent::Pong { .. }) {
        let (resolved_client_key, registered_streams) =
            remote_stream_diagnostic_context(state, connection_epoch, client_id, stream_id).await;
        chain_log::write_line(format!(
            "[remote_control] event=stale_server_envelope connection_epoch={} seq_id={} client_id={} stream_id={} kind={} action={} resolved_client_key={} registered_streams={}",
            connection_epoch,
            seq_id,
            client_id,
            stream_id,
            server_event_kind(event),
            if processed { "processed" } else { "ignored" },
            resolved_client_key.unwrap_or_default(),
            registered_streams
        ));
    }
}

async fn is_current_remote_stream(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
) -> bool {
    let remote = state.remote_control.inner.lock().await;
    remote.connection_epoch == connection_epoch
        && remote_client_key_for_stream_locked(&remote, client_id, stream_id).is_some()
}

async fn remote_stream_diagnostic_context(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
) -> (Option<String>, String) {
    let remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch != connection_epoch {
        return (None, String::new());
    }
    let resolved_client_key = remote_client_key_for_stream_locked(&remote, client_id, stream_id);
    let registered_streams = remote
        .clients
        .iter()
        .map(|(client_key, client)| {
            format!("{}:{}:{}", client_key, client.client_id, client.stream_id)
        })
        .collect::<Vec<_>>()
        .join(",");
    (resolved_client_key, registered_streams)
}

async fn mark_remote_ws_inbound(state: &SharedState, connection_epoch: u64) {
    let mut remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch == connection_epoch {
        remote.last_ws_inbound_at_ms = Some(now_ms());
    }
}

async fn mark_remote_ws_ping(state: &SharedState, connection_epoch: u64) {
    let mut remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch == connection_epoch {
        remote.last_ws_ping_at_ms = Some(now_ms());
    }
}

async fn mark_remote_ws_pong(state: &SharedState, connection_epoch: u64) {
    let mut remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch == connection_epoch {
        let now = now_ms();
        remote.last_ws_inbound_at_ms = Some(now);
        remote.last_ws_pong_at_ms = Some(now);
    }
}

async fn mark_remote_app_ping(state: &SharedState, connection_epoch: u64, client_key: &str) {
    let client_key = normalize_remote_client_key(client_key);
    let mut remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch == connection_epoch {
        let now = now_ms();
        let client = ensure_client_state_locked(&mut remote, &client_key);
        client.last_app_ping_at_ms = Some(now);
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
    }
}

async fn remote_app_ping_targets(
    state: &SharedState,
    connection_epoch: u64,
) -> Vec<(String, String, String)> {
    let remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch != connection_epoch
        || !remote.connected
        || remote.outbound_tx.is_none()
    {
        return Vec::new();
    }
    remote
        .clients
        .iter()
        .filter(|(_, client)| client.initialized)
        .map(|(client_key, client)| {
            (
                client_key.clone(),
                client.client_id.clone(),
                client.stream_id.clone(),
            )
        })
        .collect()
}

async fn record_remote_app_pong(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    status: &str,
) -> Result<bool> {
    let normalized_status = status.trim().to_ascii_lowercase();
    Ok({
        let mut remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch {
            return Ok(false);
        }
        let Some(client_key) = remote_client_key_for_stream_locked(&remote, client_id, stream_id)
        else {
            return Ok(false);
        };
        let now = now_ms();
        let client = ensure_client_state_locked(&mut remote, &client_key);
        client.last_app_pong_at_ms = Some(now);
        client.last_app_pong_status = Some(normalized_status.clone());
        let should_reinitialize = normalized_status == "unknown" && client.initialized;
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
        should_reinitialize
    })
}

async fn handle_remote_app_pong_after_ack(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    status: &str,
    should_reinitialize: bool,
) -> Result<()> {
    let client_key = {
        let remote = state.remote_control.inner.lock().await;
        remote_client_key_for_stream_locked(&remote, client_id, stream_id)
    };
    state
        .push_event(
            "info",
            "remote_control_pong",
            format!(
                "client_key={} status={status}",
                client_key.as_deref().unwrap_or("")
            ),
        )
        .await;

    if should_reinitialize {
        let Some(client_key) = client_key else {
            return Ok(());
        };
        log_remote_control_unknown_context(
            state,
            connection_epoch,
            &client_key,
            client_id,
            stream_id,
        )
        .await;
        state
            .push_event(
                "warn",
                "remote_control_client_unknown",
                format!(
                    "app-server reported remote-control client as unknown; client_key={} recovering",
                    client_key
                ),
            )
            .await;
        start_remote_control_client_recovery(
            state,
            connection_epoch,
            &client_key,
            client_id,
            stream_id,
        )
        .await?;
    }
    Ok(())
}

async fn observe_command_output_delta_received(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    message: &Value,
    worker_capacity: usize,
) {
    let message = message.get("message").unwrap_or(message);
    let params = message.get("params");
    let thread_id = params
        .and_then(thread_id_from_payload)
        .or_else(|| thread_id_from_payload(message));
    let item_id = params
        .and_then(|params| params.get("itemId"))
        .and_then(|value| value.as_str())
        .map(str::to_string);

    let summary = {
        let mut remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch {
            return;
        }
        let key = server_ack_cursor_key(client_id, stream_id);
        let diagnostics = remote.stream_diagnostics.entry(key).or_default();
        observe_stream_window_event(
            diagnostics,
            now_ms(),
            seq_id,
            StreamWindowEvent::OutputDelta,
        );
        diagnostics.output_delta_count = diagnostics.output_delta_count.saturating_add(1);
        diagnostics.output_delta_last_seq_id = Some(seq_id);
        diagnostics.output_delta_last_item_id = item_id.clone();
        diagnostics.output_delta_last_thread_id = thread_id.clone();
        diagnostics.output_delta_last_seen_at_ms = Some(now_ms());
        diagnostics.output_delta_last_worker_capacity = Some(worker_capacity);
        command_output_delta_log_summary(diagnostics, worker_capacity)
    };

    if let Some(summary) = summary {
        chain_log::write_line(format!(
            "[remote_control] event=command_output_delta_pressure connection_epoch={} client_id={} stream_id={} seq_id={} thread={} item={} {}",
            connection_epoch,
            client_id,
            stream_id,
            seq_id,
            thread_id.as_deref().unwrap_or_default(),
            item_id.as_deref().unwrap_or_default(),
            summary
        ));
    }
}

async fn observe_server_envelope_window(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    received_at_ms: u128,
) {
    let mut remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch != connection_epoch {
        return;
    }
    let key = server_ack_cursor_key(client_id, stream_id);
    let diagnostics = remote.stream_diagnostics.entry(key).or_default();
    observe_stream_window_event(
        diagnostics,
        received_at_ms,
        seq_id,
        StreamWindowEvent::ServerIn,
    );
}

enum StreamWindowEvent {
    ServerIn,
    OutputDelta,
    Ack,
}

fn observe_stream_window_event(
    diagnostics: &mut RemoteControlStreamDiagnostics,
    now_ms: u128,
    seq_id: u64,
    event: StreamWindowEvent,
) {
    let should_reset_window = diagnostics.window_started_at_ms.is_none_or(|started_at| {
        now_ms.saturating_sub(started_at) > REMOTE_CONTROL_DIAGNOSTIC_WINDOW_MS
    });
    if should_reset_window {
        diagnostics.window_started_at_ms = Some(now_ms);
        diagnostics.window_server_in_count = 0;
        diagnostics.window_output_delta_count = 0;
        diagnostics.window_ack_count = 0;
        diagnostics.window_first_seq_id = Some(seq_id);
    }
    diagnostics.window_last_seq_id = Some(seq_id);
    match event {
        StreamWindowEvent::ServerIn => {
            diagnostics.window_server_in_count =
                diagnostics.window_server_in_count.saturating_add(1);
        }
        StreamWindowEvent::OutputDelta => {
            diagnostics.window_output_delta_count =
                diagnostics.window_output_delta_count.saturating_add(1);
        }
        StreamWindowEvent::Ack => {
            diagnostics.window_ack_count = diagnostics.window_ack_count.saturating_add(1);
        }
    }
    if diagnostics.window_server_in_count > diagnostics.max_window_server_in_count
        || diagnostics.window_output_delta_count > diagnostics.max_window_output_delta_count
    {
        diagnostics.max_window_server_in_count = diagnostics.window_server_in_count;
        diagnostics.max_window_output_delta_count = diagnostics.window_output_delta_count;
        diagnostics.max_window_ack_count = diagnostics.window_ack_count;
        diagnostics.max_window_started_at_ms = diagnostics.window_started_at_ms;
        diagnostics.max_window_last_at_ms = Some(now_ms);
    }
}

fn command_output_delta_log_summary(
    diagnostics: &RemoteControlStreamDiagnostics,
    worker_capacity: usize,
) -> Option<String> {
    let count = diagnostics.output_delta_count;
    let should_log = count == 1
        || count % 50 == 0
        || worker_capacity <= REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY / 8;
    should_log.then(|| {
        format!(
            "output_delta_count={} worker_capacity={} ack_count={} last_ack_seq_id={} last_ack_elapsed_ms={} max_ack_elapsed_ms={}",
            count,
            worker_capacity,
            diagnostics.ack_count,
            diagnostics
                .last_ack_seq_id
                .map(|value| value.to_string())
                .unwrap_or_default(),
            diagnostics
                .last_ack_elapsed_ms
                .map(|value| value.to_string())
                .unwrap_or_default(),
            diagnostics.max_ack_elapsed_ms
        )
    })
}

async fn record_server_ack_diagnostics(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    received_at_ms: u128,
) {
    let ack_at_ms = now_ms();
    let elapsed_ms = ack_at_ms.saturating_sub(received_at_ms);
    let should_log = {
        let mut remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch {
            return;
        }
        let key = server_ack_cursor_key(client_id, stream_id);
        let diagnostics = remote.stream_diagnostics.entry(key).or_default();
        observe_stream_window_event(diagnostics, ack_at_ms, seq_id, StreamWindowEvent::Ack);
        diagnostics.ack_count = diagnostics.ack_count.saturating_add(1);
        diagnostics.last_ack_elapsed_ms = Some(elapsed_ms);
        diagnostics.last_ack_seq_id = Some(seq_id);
        if elapsed_ms > diagnostics.max_ack_elapsed_ms {
            diagnostics.max_ack_elapsed_ms = elapsed_ms;
        }
        elapsed_ms >= 50
    };
    if should_log {
        chain_log::write_line(format!(
            "[remote_control] event=server_ack_slow connection_epoch={} client_id={} stream_id={} seq_id={} elapsed_ms={}",
            connection_epoch, client_id, stream_id, seq_id, elapsed_ms
        ));
    }
}

async fn log_remote_control_unknown_context(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    client_id: &str,
    stream_id: &str,
) {
    let (diagnostics, registered_streams, recent_events) = {
        let remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch != connection_epoch {
            return;
        }
        let key = server_ack_cursor_key(client_id, stream_id);
        let diagnostics = remote
            .stream_diagnostics
            .get(&key)
            .map(format_stream_diagnostics)
            .unwrap_or_else(|| "no_stream_diagnostics=true".to_string());
        let registered_streams = remote
            .clients
            .iter()
            .map(|(client_key, client)| {
                format!("{}:{}:{}", client_key, client.client_id, client.stream_id)
            })
            .collect::<Vec<_>>()
            .join(",");
        let recent_events = remote
            .recent_events
            .iter()
            .filter(|event| {
                event.connection_epoch == connection_epoch
                    && (event.stream_id == stream_id || event.client_id == client_id)
            })
            .map(format_recent_event)
            .collect::<Vec<_>>();
        (diagnostics, registered_streams, recent_events)
    };
    chain_log::write_line(format!(
        "[remote_control] event=remote_control_client_unknown_context connection_epoch={} client_key={} client_id={} stream_id={} {} registered_streams={}",
        connection_epoch, client_key, client_id, stream_id, diagnostics, registered_streams
    ));
    for (index, event) in recent_events.iter().enumerate() {
        chain_log::write_line(format!(
            "[remote_control] event=remote_control_client_unknown_recent index={} {}",
            index, event
        ));
    }
}

fn format_stream_diagnostics(diagnostics: &RemoteControlStreamDiagnostics) -> String {
    format!(
        "output_delta_count={} output_delta_last_seq_id={} output_delta_last_thread={} output_delta_last_item={} output_delta_last_seen_at_ms={} output_delta_last_worker_capacity={} window_started_at_ms={} window_server_in_count={} window_output_delta_count={} window_ack_count={} window_first_seq_id={} window_last_seq_id={} max_window_started_at_ms={} max_window_last_at_ms={} max_window_server_in_count={} max_window_output_delta_count={} max_window_ack_count={} ack_count={} last_ack_seq_id={} last_ack_elapsed_ms={} max_ack_elapsed_ms={}",
        diagnostics.output_delta_count,
        diagnostics
            .output_delta_last_seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .output_delta_last_thread_id
            .as_deref()
            .unwrap_or_default(),
        diagnostics
            .output_delta_last_item_id
            .as_deref()
            .unwrap_or_default(),
        diagnostics
            .output_delta_last_seen_at_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .output_delta_last_worker_capacity
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .window_started_at_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics.window_server_in_count,
        diagnostics.window_output_delta_count,
        diagnostics.window_ack_count,
        diagnostics
            .window_first_seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .window_last_seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .max_window_started_at_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .max_window_last_at_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics.max_window_server_in_count,
        diagnostics.max_window_output_delta_count,
        diagnostics.max_window_ack_count,
        diagnostics.ack_count,
        diagnostics
            .last_ack_seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics
            .last_ack_elapsed_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
        diagnostics.max_ack_elapsed_ms
    )
}

async fn remote_control_stale_reason(state: &SharedState, connection_epoch: u64) -> Option<String> {
    let remote = state.remote_control.inner.lock().await;
    if remote.connection_epoch != connection_epoch {
        return None;
    }
    remote_control_stale_reason_locked(&remote, now_ms())
}

async fn observe_app_server_message(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    message: &Value,
) {
    if !is_active_connection_epoch(state, connection_epoch).await {
        return;
    }
    let client_key = {
        let remote = state.remote_control.inner.lock().await;
        remote_client_key_for_stream_locked(&remote, client_id, stream_id)
    };
    let message = message.get("message").unwrap_or(message);
    chain_log::write_line(format!(
        "[remote_control] event=server_message_in connection_epoch={} client_key={} client_id={} stream_id={} summary={}",
        connection_epoch,
        client_key.as_deref().unwrap_or(""),
        client_id,
        stream_id,
        message_summary(message)
    ));
    log_codex_to_remote_message(connection_epoch, message);
    if let Some(id) = message.get("id") {
        if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
            if method == "account/chatgptAuthTokens/refresh" {
                match local_chatgpt_auth_tokens_response(state).await {
                    Ok(result) => {
                        if let Err(err) = send_response_for_stream(
                            state,
                            client_id,
                            stream_id,
                            id.clone(),
                            result,
                        )
                        .await
                        {
                            state
                                .push_event(
                                    "error",
                                    "remote_control_auth_refresh_failed",
                                    err.to_string(),
                                )
                                .await;
                        } else {
                            state
                                .push_event(
                                    "info",
                                    "remote_control_auth_refresh",
                                    format!("id={id}"),
                                )
                                .await;
                        }
                    }
                    Err(err) => {
                        state
                            .push_event(
                                "error",
                                "remote_control_auth_refresh_failed",
                                err.to_string(),
                            )
                            .await;
                    }
                }
                return;
            }
            let params = message.get("params").cloned();
            state
                .push_event(
                    "info",
                    "remote_control_server_request",
                    format!("method={method} id={id}"),
                )
                .await;
            let _ = state.remote_control.notifications.send(CodexNotification {
                method: method.to_string(),
                params,
                request_id: Some(id.clone()),
                remote_client_key: client_key.clone(),
                remote_client_id: Some(client_id.to_string()),
                remote_stream_id: Some(stream_id.to_string()),
            });
            return;
        }

        let request_key = request_id_key(id);
        let pending = {
            let mut remote = state.remote_control.inner.lock().await;
            let pending = client_key
                .as_ref()
                .and_then(|client_key| remote.clients.get_mut(client_key))
                .and_then(|client| client.pending.remove(&request_key));
            if client_key.as_deref() == Some(DEFAULT_REMOTE_CLIENT_KEY) {
                sync_default_client_legacy_locked(&mut remote);
            }
            pending
        };
        let client_method = pending.as_ref().map(|pending| pending.method.clone());
        let client_thread_id = pending
            .as_ref()
            .and_then(|pending| pending.thread_id.clone());
        if let Some(method) = client_method.as_deref() {
            state
                .push_event(
                    "info",
                    "remote_control_response",
                    format!("method={method} id={id}"),
                )
                .await;
        }
        if let Some(result) = message.get("result") {
            if client_method.as_deref() == Some("initialize") {
                {
                    let mut remote = state.remote_control.inner.lock().await;
                    if let Some(client_key) = client_key.as_deref() {
                        if let Some(client) = remote.clients.get_mut(client_key) {
                            client.initialized = true;
                            client.last_app_pong_status = Some("active".to_string());
                            client.recovery_started_at_ms = None;
                        }
                        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                            sync_default_client_legacy_locked(&mut remote);
                        }
                    }
                    remote.last_error = None;
                }
                if let Err(err) = send_initialized_for_stream(state, client_id, stream_id).await {
                    state
                        .push_event(
                            "error",
                            "remote_control_initialized_send_failed",
                            err.to_string(),
                        )
                        .await;
                } else if let Some(client_key) = client_key.as_deref()
                    && let Err(err) =
                        replay_pending_requests(state, connection_epoch, client_key).await
                {
                    state
                        .push_event(
                            "error",
                            "remote_control_pending_replay_failed",
                            err.to_string(),
                        )
                        .await;
                }
            }
            if let Some(thread_id) = thread_id_from_payload(result) {
                mark_thread_active_for_client(state, client_key.as_deref(), &thread_id).await;
            }
            if client_method
                .as_deref()
                .is_some_and(|method| matches!(method, "turn/start" | "turn/steer"))
                && let Some(turn_id) = result
                    .get("turn")
                    .and_then(|v| v.get("id"))
                    .and_then(|v| v.as_str())
            {
                let thread_id = client_thread_id.or_else(|| {
                    result
                        .get("turn")
                        .and_then(|turn| turn.get("threadId"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                });
                let mut remote = state.remote_control.inner.lock().await;
                if let Some(client_key) = client_key.as_deref() {
                    if let Some(client) = remote.clients.get_mut(client_key) {
                        client.current_turn_id = Some(turn_id.to_string());
                        if let Some(thread_id) = thread_id.clone() {
                            client.current_thread_id = Some(thread_id);
                        }
                    }
                    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                        sync_default_client_legacy_locked(&mut remote);
                    }
                } else {
                    remote.current_turn_id = Some(turn_id.to_string());
                    if let Some(thread_id) = thread_id {
                        remote.current_thread_id = Some(thread_id);
                    }
                }
            }
        }
        if message.get("error").is_some() {
            state
                .push_event(
                    "error",
                    "remote_control_app_server_error",
                    format!("id={id} error={}", message["error"]),
                )
                .await;
        }
        if let Some(pending) = pending {
            let result = if let Some(error) = message.get("error") {
                Err(anyhow!("remote-control request failed: {error}"))
            } else {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = pending.response_tx.send(result);
        }
        return;
    }

    if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
        if method == "initialized" {
            {
                let mut remote = state.remote_control.inner.lock().await;
                if let Some(client_key) = client_key.as_deref() {
                    if let Some(client) = remote.clients.get_mut(client_key) {
                        client.initialized = true;
                        client.last_app_pong_status = Some("active".to_string());
                        client.recovery_started_at_ms = None;
                    }
                    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                        sync_default_client_legacy_locked(&mut remote);
                    }
                }
                remote.last_error = None;
            }
            state
                .push_event("info", "remote_control_initialized", "initialized")
                .await;
            return;
        }
        if method == "item/commandExecution/outputDelta" {
            return;
        }
        let params = message.get("params").cloned();
        if method == "remoteControl/status/changed" {
            observe_remote_control_status_changed(state, params.as_ref()).await;
        }
        if method == "thread/started" {
            if let Some(thread_id) = params.as_ref().and_then(thread_id_from_payload) {
                mark_notification_thread_active_for_client(
                    state,
                    client_key.as_deref(),
                    &thread_id,
                )
                .await;
            }
        } else if method == "thread/status/changed" {
            if let Some(params) = params.as_ref()
                && let (Some(thread_id), Some(status_type)) = (
                    thread_id_from_payload(params),
                    thread_status_type_from_payload(params),
                )
            {
                observe_thread_status_changed(
                    state,
                    client_key.as_deref(),
                    &thread_id,
                    &status_type,
                )
                .await;
            }
        } else if method == "turn/started" {
            let thread_id = params.as_ref().and_then(thread_id_from_payload);
            let turn_id = params
                .as_ref()
                .and_then(|p| {
                    p.get("turn")
                        .and_then(|turn| turn.get("id"))
                        .and_then(|v| v.as_str())
                        .or_else(|| p.get("turnId").and_then(|v| v.as_str()))
                })
                .map(str::to_string);
            let should_track = if let Some(thread_id) = thread_id.as_deref() {
                should_track_notification_thread_for_client(state, client_key.as_deref(), thread_id)
                    .await
            } else {
                true
            };
            if should_track {
                let mut remote = state.remote_control.inner.lock().await;
                if let Some(client_key) = client_key.as_deref() {
                    if let Some(client) = remote.clients.get_mut(client_key) {
                        if let Some(thread_id) = thread_id.clone() {
                            client.current_thread_id = Some(thread_id);
                        }
                        if let Some(turn_id) = turn_id.clone() {
                            client.current_turn_id = Some(turn_id);
                        }
                    }
                    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                        sync_default_client_legacy_locked(&mut remote);
                    }
                } else {
                    if let Some(thread_id) = thread_id {
                        remote.current_thread_id = Some(thread_id);
                    }
                    if let Some(turn_id) = turn_id {
                        remote.current_turn_id = Some(turn_id);
                    }
                }
            }
        } else if method == "turn/completed" {
            let thread_id = params.as_ref().and_then(thread_id_from_payload);
            let turn_id = params.as_ref().and_then(turn_id_from_payload);
            if let Some(thread_id) = thread_id.as_deref() {
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(thread_id, turn_id.as_deref());
            }
            let mut remote = state.remote_control.inner.lock().await;
            if let Some(client_key) = client_key.as_deref() {
                if let Some(client) = remote.clients.get_mut(client_key) {
                    if thread_id.is_none()
                        || client.current_thread_id.as_deref() == thread_id.as_deref()
                    {
                        client.current_turn_id = None;
                    }
                }
                if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                    sync_default_client_legacy_locked(&mut remote);
                }
            } else if thread_id.is_none()
                || remote.current_thread_id.as_deref() == thread_id.as_deref()
            {
                remote.current_turn_id = None;
            }
        } else if method == "thread/closed"
            && let Some(thread_id) = params.as_ref().and_then(thread_id_from_payload)
        {
            let mut remote = state.remote_control.inner.lock().await;
            if let Some(client_key) = client_key.as_deref() {
                if let Some(client) = remote.clients.get_mut(client_key)
                    && client.current_thread_id.as_deref() == Some(thread_id.as_str())
                {
                    client.current_thread_id = None;
                    client.current_turn_id = None;
                }
                if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                    sync_default_client_legacy_locked(&mut remote);
                }
            } else if remote.current_thread_id.as_deref() == Some(thread_id.as_str()) {
                remote.current_thread_id = None;
                remote.current_turn_id = None;
            }
        }
        state
            .push_event(
                "info",
                "remote_control_notification",
                format!("method={method}"),
            )
            .await;
        let _ = state.remote_control.notifications.send(CodexNotification {
            method: method.to_string(),
            params,
            request_id: message.get("id").cloned(),
            remote_client_key: client_key,
            remote_client_id: Some(client_id.to_string()),
            remote_stream_id: Some(stream_id.to_string()),
        });
    }
}

async fn observe_thread_status_changed(
    state: &SharedState,
    client_key: Option<&str>,
    thread_id: &str,
    status_type: &str,
) {
    if !is_terminal_or_inactive_thread_status(status_type) {
        return;
    }
    state
        .runtime
        .lock()
        .await
        .mark_turn_completed(thread_id, None);
    let normalized_client_key = client_key.map(normalize_remote_client_key);
    let cleared_turn_id = {
        let mut remote = state.remote_control.inner.lock().await;
        if let Some(client_key) = normalized_client_key.as_deref() {
            let mut cleared_turn_id = None;
            if let Some(client) = remote.clients.get_mut(client_key)
                && client.current_thread_id.as_deref() == Some(thread_id)
            {
                cleared_turn_id = client.current_turn_id.take();
            }
            if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                sync_default_client_legacy_locked(&mut remote);
            }
            cleared_turn_id
        } else if remote.current_thread_id.as_deref() == Some(thread_id) {
            remote.current_turn_id.take()
        } else {
            None
        }
    };
    if let Some(turn_id) = cleared_turn_id {
        state
            .runtime
            .lock()
            .await
            .mark_turn_completed(thread_id, Some(&turn_id));
        chain_log::write_line(format!(
            "[remote_control] event=thread_status_cleared_current_turn client_key={} thread={} turn={} status={}",
            normalized_client_key.as_deref().unwrap_or(""),
            thread_id,
            turn_id,
            status_type
        ));
        state
            .push_event(
                "warn",
                "remote_control_thread_status_cleared_current_turn",
                format!(
                    "client_key={} thread={} turn={} status={}",
                    normalized_client_key.as_deref().unwrap_or(""),
                    thread_id,
                    turn_id,
                    status_type
                ),
            )
            .await;
    }
}

async fn observe_remote_control_status_changed(state: &SharedState, params: Option<&Value>) {
    let Some(params) = params else {
        return;
    };
    let mut remote = state.remote_control.inner.lock().await;
    if let Some(server_name) = json_string(params, "serverName") {
        remote.server_name = Some(server_name);
    }
    if let Some(installation_id) = json_string(params, "installationId") {
        remote.installation_id = Some(installation_id);
    }
    if let Some(environment_id) = json_string(params, "environmentId") {
        remote.environment_id = Some(environment_id);
    }
    if let Some(status) = json_string(params, "status")
        && status == "connected"
    {
        remote.last_error = None;
    }
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn observe_server_chunk(
    chunks: &mut HashMap<(String, String, u64), ServerChunkAssembly>,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    message_chunk_base64: &str,
) -> ServerChunkObservation {
    let key = (client_id.to_string(), stream_id.to_string(), seq_id);
    if chunks
        .get(&key)
        .is_some_and(|assembly| segment_id < assembly.next_segment_id)
    {
        warn!(
            "dropping duplicate remote-control server chunk: next={} got={} seq={seq_id}",
            chunks
                .get(&key)
                .map(|assembly| assembly.next_segment_id)
                .unwrap_or_default(),
            segment_id
        );
        return ServerChunkObservation::Dropped;
    }
    if segment_count == 0
        || segment_count > REMOTE_CONTROL_SEGMENT_COUNT_MAX
        || segment_id >= segment_count
        || message_size_bytes == 0
        || message_size_bytes > REMOTE_CONTROL_REASSEMBLED_MAX_BYTES
        || message_chunk_base64.is_empty()
    {
        warn!(
            "invalid remote-control server chunk metadata: segment={segment_id}/{segment_count} size={message_size_bytes}"
        );
        chunks.remove(&key);
        return ServerChunkObservation::Dropped;
    }
    let assembly = chunks
        .entry(key.clone())
        .or_insert_with(|| ServerChunkAssembly {
            segment_count,
            message_size_bytes,
            raw: Vec::new(),
            next_segment_id: 0,
        });
    let expected_segment_id = assembly.next_segment_id;
    if assembly.segment_count != segment_count
        || assembly.message_size_bytes != message_size_bytes
        || expected_segment_id != segment_id
    {
        let _ = assembly;
        chunks.remove(&key);
        warn!(
            "out-of-order remote-control server chunk: expected={} got={} seq={seq_id}",
            expected_segment_id, segment_id
        );
        return ServerChunkObservation::Dropped;
    }
    let chunk = match base64::engine::general_purpose::STANDARD.decode(message_chunk_base64) {
        Ok(chunk) => chunk,
        Err(err) => {
            let _ = assembly;
            chunks.remove(&key);
            warn!("invalid remote-control server chunk base64: {err}");
            return ServerChunkObservation::Dropped;
        }
    };
    if assembly.raw.len().saturating_add(chunk.len()) > assembly.message_size_bytes {
        let _ = assembly;
        chunks.remove(&key);
        warn!("remote-control server chunk size overflow: seq={seq_id}");
        return ServerChunkObservation::Dropped;
    }
    assembly.raw.extend_from_slice(&chunk);
    assembly.next_segment_id += 1;
    if assembly.next_segment_id < assembly.segment_count {
        return ServerChunkObservation::Pending;
    }
    let Some(assembly) = chunks.remove(&key) else {
        warn!("missing completed remote-control server chunk assembly");
        return ServerChunkObservation::Dropped;
    };
    if assembly.raw.len() != assembly.message_size_bytes {
        warn!(
            "remote-control server chunk size mismatch: expected={} got={}",
            assembly.message_size_bytes,
            assembly.raw.len()
        );
        return ServerChunkObservation::Dropped;
    }
    match serde_json::from_slice::<Value>(&assembly.raw) {
        Ok(message) => ServerChunkObservation::Complete(message),
        Err(err) => {
            warn!("invalid reassembled remote-control server message: {err}");
            ServerChunkObservation::Dropped
        }
    }
}

pub async fn send_response_for_client(
    state: &SharedState,
    client_key: &str,
    request_id: Value,
    result: Value,
) -> Result<()> {
    ensure_remote_control_client_ready(state, client_key).await?;
    let (client_id, stream_id, seq_id) = next_client_envelope_parts(state, client_key).await?;
    let cursor = next_remote_subscribe_cursor(state).await;
    let envelopes = build_client_message_envelopes(
        &client_id,
        &stream_id,
        seq_id,
        json!({ "id": request_id, "result": result }),
        Some(&cursor),
    )?;
    send_envelopes(state, envelopes).await
}

async fn send_response_for_stream(
    state: &SharedState,
    client_id: &str,
    stream_id: &str,
    request_id: Value,
    result: Value,
) -> Result<()> {
    let seq_id = {
        let mut remote = state.remote_control.inner.lock().await;
        let Some(client_key) = remote_client_key_for_stream_locked(&remote, client_id, stream_id)
        else {
            return Err(anyhow!(
                "remote-control response target is not registered: client_id={client_id} stream_id={stream_id}"
            ));
        };
        let client = ensure_client_state_locked(&mut remote, &client_key);
        let seq_id = client.next_seq_id;
        client.next_seq_id = client.next_seq_id.saturating_add(1);
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
        seq_id
    };
    let cursor = next_remote_subscribe_cursor(state).await;
    let envelopes = build_client_message_envelopes(
        client_id,
        stream_id,
        seq_id,
        json!({ "id": request_id, "result": result }),
        Some(&cursor),
    )?;
    send_envelopes(state, envelopes).await
}

async fn local_chatgpt_auth_tokens_response(state: &SharedState) -> Result<Value> {
    let codex_home = std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join(".codex"))
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|home| std::path::PathBuf::from(home).join(".codex"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".codex"));
    let auth_path = codex_home.join("auth.json");
    let auth = read_auth_json(&auth_path)?;
    let account_id = auth
        .pointer("/tokens/account_id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| {
            auth.pointer("/tokens/access_token")
                .and_then(|value| value.as_str())
                .and_then(jwt_chatgpt_account_id)
        })
        .or_else(|| {
            state
                .remote_control
                .inner
                .try_lock()
                .ok()
                .and_then(|remote| remote.account_id.clone())
        })
        .unwrap_or_else(|| "acct_codex_remote_local".to_string());
    let plan_type = auth
        .pointer("/tokens/access_token")
        .and_then(|value| value.as_str())
        .and_then(jwt_chatgpt_plan_type)
        .unwrap_or_else(|| "pro".to_string());
    let access_token = auth
        .pointer("/tokens/access_token")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| local_chatgpt_jwt(&account_id, &plan_type));
    Ok(json!({
        "accessToken": access_token,
        "chatgptAccountId": account_id,
        "chatgptPlanType": plan_type,
    }))
}

fn read_auth_json(path: &Path) -> Result<Value> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Codex App auth {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn jwt_chatgpt_account_id(jwt: &str) -> Option<String> {
    jwt_payload(jwt).and_then(|payload| {
        payload
            .get("https://api.openai.com/auth")?
            .get("chatgpt_account_id")?
            .as_str()
            .map(str::to_string)
    })
}

fn jwt_chatgpt_plan_type(jwt: &str) -> Option<String> {
    jwt_payload(jwt).and_then(|payload| {
        payload
            .get("https://api.openai.com/auth")?
            .get("chatgpt_plan_type")?
            .as_str()
            .map(str::to_string)
    })
}

fn jwt_payload(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn local_chatgpt_jwt(account_id: &str, plan_type: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let exp = now + 10 * 365 * 24 * 60 * 60;
    let payload = json!({
        "iss": "https://auth.openai.com",
        "aud": ["https://api.openai.com/v1"],
        "iat": now,
        "nbf": now,
        "exp": exp,
        "sub": "local|user_codex_remote_local",
        "email": "codex-remote-local@example.local",
        "email_verified": true,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "account_id": account_id,
            "chatgpt_account_user_id": format!("user_codex_remote_local__{account_id}"),
            "account_user_id": format!("user_codex_remote_local__{account_id}"),
            "chatgpt_plan_type": plan_type,
            "chatgpt_user_id": "user_codex_remote_local",
            "user_id": "user_codex_remote_local",
            "chatgpt_account_is_fedramp": false,
            "localhost": true,
            "groups": [],
            "organizations": [{
                "id": account_id,
                "is_default": true,
                "role": "owner",
                "title": "Codex Remote Local"
            }]
        },
        "scp": ["openid", "profile", "email", "offline_access"],
    });
    format!(
        "{}.{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({ "alg": "none", "typ": "JWT" })).unwrap_or_default()
        ),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap_or_default()),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
    )
}

async fn send_initialize_for_client(state: &SharedState, client_key: &str) -> Result<u64> {
    let client_key = normalize_remote_client_key(client_key);
    let initialize_id = next_request_id();
    let request_key = initialize_id.to_string();
    let message = json!({
        "id": initialize_id,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "codex-remote",
                "title": "Codex Remote",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {
                "experimentalApi": true,
                "requestAttestation": false
            }
        }
    });
    let cursor = next_remote_subscribe_cursor(state).await;
    let (client_id, stream_id, seq_id, envelopes) = {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let mut remote = state.remote_control.inner.lock().await;
        let client = ensure_client_state_locked(&mut remote, &client_key);
        client.last_initialize_sent_at_ms = Some(now_ms());
        let seq_id = client.next_seq_id;
        client.next_seq_id = client.next_seq_id.saturating_add(1);
        let client_id = client.client_id.clone();
        let stream_id = client.stream_id.clone();
        let envelopes = build_client_message_envelopes(
            &client_id,
            &stream_id,
            seq_id,
            message.clone(),
            Some(&cursor),
        )?;
        client
            .pending
            .retain(|_, pending| pending.method != "initialize");
        client.pending.insert(
            request_key.clone(),
            PendingRemoteRequest {
                method: "initialize".to_string(),
                thread_id: None,
                response_tx: tx,
                message: message.clone(),
                envelopes: envelopes.clone(),
            },
        );
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
        (client_id, stream_id, seq_id, envelopes)
    };
    chain_log::write_line(format!(
        "[remote_control] event=initialize_send client_key={} client_id={} stream_id={} seq_id={} request_id={}",
        client_key, client_id, stream_id, seq_id, initialize_id
    ));
    if let Err(err) = send_envelopes(state, envelopes).await {
        let mut remote = state.remote_control.inner.lock().await;
        if let Some(client) = remote.clients.get_mut(&client_key) {
            client.pending.remove(&request_key);
        }
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
        return Err(err);
    }
    Ok(initialize_id)
}

async fn send_initialized_for_stream(
    state: &SharedState,
    client_id: &str,
    stream_id: &str,
) -> Result<()> {
    let seq_id = {
        let mut remote = state.remote_control.inner.lock().await;
        let Some(client_key) = remote_client_key_for_stream_locked(&remote, client_id, stream_id)
        else {
            return Err(anyhow!(
                "remote-control initialized target is not registered: client_id={client_id} stream_id={stream_id}"
            ));
        };
        let client = ensure_client_state_locked(&mut remote, &client_key);
        let seq_id = client.next_seq_id;
        client.next_seq_id = client.next_seq_id.saturating_add(1);
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
        seq_id
    };
    let cursor = next_remote_subscribe_cursor(state).await;
    let envelopes = build_client_message_envelopes(
        client_id,
        stream_id,
        seq_id,
        json!({
            "method": "initialized",
        }),
        Some(&cursor),
    )?;
    send_envelopes(state, envelopes).await
}

fn thread_id_from_payload(value: &Value) -> Option<String> {
    value
        .get("threadId")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("thread_id").and_then(|v| v.as_str()))
        .or_else(|| {
            value
                .get("thread")
                .and_then(|thread| thread.get("id"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            value
                .get("turn")
                .and_then(|turn| turn.get("threadId"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_string)
}

fn thread_status_type_from_payload(value: &Value) -> Option<String> {
    value
        .get("status")
        .and_then(|status| {
            status
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| status.as_str())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn is_terminal_or_inactive_thread_status(status_type: &str) -> bool {
    matches!(status_type, "idle" | "notLoaded" | "systemError")
}

async fn mark_thread_active(state: &SharedState, thread_id: &str) {
    let mut remote = state.remote_control.inner.lock().await;
    if remote.current_thread_id.as_deref() == Some(thread_id) {
        return;
    }
    remote.current_thread_id = Some(thread_id.to_string());
    drop(remote);
    state
        .push_event("info", "remote_control_thread_active", thread_id)
        .await;
}

async fn mark_thread_active_for_client(
    state: &SharedState,
    client_key: Option<&str>,
    thread_id: &str,
) {
    if let Some(client_key) = client_key {
        let client_key = normalize_remote_client_key(client_key);
        let mut remote = state.remote_control.inner.lock().await;
        let client = ensure_client_state_locked(&mut remote, &client_key);
        if client.current_thread_id.as_deref() == Some(thread_id) {
            return;
        }
        client.current_thread_id = Some(thread_id.to_string());
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
        drop(remote);
        state
            .push_event(
                "info",
                "remote_control_thread_active",
                format!("client_key={client_key} thread={thread_id}"),
            )
            .await;
        return;
    }
    mark_thread_active(state, thread_id).await;
}

async fn mark_notification_thread_active_for_client(
    state: &SharedState,
    client_key: Option<&str>,
    thread_id: &str,
) {
    if should_track_notification_thread_for_client(state, client_key, thread_id).await {
        mark_thread_active_for_client(state, client_key, thread_id).await;
    } else {
        chain_log::write_line(format!(
            "[remote_control] level=warn event=notification_thread_active_skipped reason=unbound_thread client_key={} thread={}",
            client_key.unwrap_or(""),
            thread_id
        ));
    }
}

async fn should_track_notification_thread_for_client(
    state: &SharedState,
    client_key: Option<&str>,
    thread_id: &str,
) -> bool {
    let Some(client_key) = client_key else {
        return true;
    };
    let client_key = normalize_remote_client_key(client_key);
    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
        return true;
    }
    let is_bound_thread = {
        let runtime = state.runtime.lock().await;
        runtime.route_for_thread(thread_id).is_some()
    };
    if is_bound_thread {
        return true;
    }
    let is_pending_request_thread = {
        let remote = state.remote_control.inner.lock().await;
        remote
            .clients
            .get(&client_key)
            .map(|client| {
                client.pending.values().any(|pending| {
                    pending
                        .thread_id
                        .as_deref()
                        .is_some_and(|pending_thread_id| pending_thread_id == thread_id)
                })
            })
            .unwrap_or(false)
    };
    is_pending_request_thread
}

pub async fn request_for_client(
    state: &SharedState,
    client_key: &str,
    method: &str,
    params: Value,
) -> Result<Value> {
    request_with_timeout_for_client(state, client_key, method, params, REMOTE_REQUEST_TIMEOUT).await
}

async fn request_with_timeout_for_client(
    state: &SharedState,
    client_key: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    let client_key = normalize_remote_client_key(client_key);
    let mut retry_after_reinitialize = true;
    loop {
        match request_once_with_timeout_for_client(
            state,
            &client_key,
            method,
            params.clone(),
            timeout,
        )
        .await
        {
            Ok(value) => return Ok(value),
            Err(err)
                if retry_after_reinitialize
                    && should_retry_request_after_reinitialize(method)
                    && err
                        .to_string()
                        .contains(REMOTE_CONTROL_CLIENT_REINITIALIZED_ERROR) =>
            {
                retry_after_reinitialize = false;
                wait_for_remote_control_initialized(state, &client_key).await?;
                state
                    .push_event(
                        "warn",
                        "remote_control_request_retry_after_reinitialize",
                        format!("client_key={} method={method}", client_key),
                    )
                    .await;
                continue;
            }
            Err(err)
                if err
                    .to_string()
                    .contains(REMOTE_CONTROL_CLIENT_REINITIALIZED_ERROR) =>
            {
                chain_log::write_line(format!(
                    "[remote_control] event=request_not_retried_after_reinitialize method={} err={}",
                    method, err
                ));
                state
                    .push_event(
                        "warn",
                        "remote_control_request_not_retried_after_reinitialize",
                        format!("method={} err={}", method, err),
                    )
                    .await;
                return Err(anyhow!(
                    "remote-control reinitialized while non-idempotent request was in flight; not replaying method={method}"
                ));
            }
            Err(err) => return Err(err),
        }
    }
}

fn should_retry_request_after_reinitialize(method: &str) -> bool {
    !matches!(
        method,
        "thread/start" | "thread/fork" | "turn/start" | "turn/steer"
    )
}

async fn request_once_with_timeout_for_client(
    state: &SharedState,
    client_key: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    let client_key = normalize_remote_client_key(client_key);
    wait_for_recovery_if_needed(state, &client_key).await?;
    ensure_remote_control_client_ready(state, &client_key).await?;
    let id = next_request_id();
    let request_key = id.to_string();
    let method_name = method.to_string();
    let thread_id = params
        .get("threadId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let message = build_pending_message(method, id, params);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let cursor = next_remote_subscribe_cursor(state).await;
    let (client_id, stream_id, seq_id, envelope) = {
        let mut remote = state.remote_control.inner.lock().await;
        if !remote.connected {
            return Err(anyhow!(
                "Codex app-server remote-control 尚未连接。请在项目目录运行 codex，确认它已经连接到 codex-remote 的 /backend-api。"
            ));
        }
        let stale_reason = remote_control_stale_reason_locked(&remote, now_ms());
        if let Some(reason) = stale_reason {
            remote.last_error = Some(reason.clone());
            return Err(anyhow!(
                "Codex app-server remote-control 连接已失活：{reason}。请稍等自动重连后重试。"
            ));
        }
        let client = ensure_client_state_locked(&mut remote, &client_key);
        if !client.initialized {
            return Err(anyhow!(
                "Codex app-server remote-control 已连接，但还没有完成初始化。请稍等几秒后重试；如果一直如此，请在 Codex App 里关闭再打开 remote-control。"
            ));
        }
        let seq_id = client.next_seq_id;
        client.next_seq_id = client.next_seq_id.saturating_add(1);
        let client_id = client.client_id.clone();
        let stream_id = client.stream_id.clone();
        let envelopes = build_client_message_envelopes(
            &client_id,
            &stream_id,
            seq_id,
            message.clone(),
            Some(&cursor),
        )?;
        client.pending.insert(
            request_key.clone(),
            PendingRemoteRequest {
                method: method.to_string(),
                thread_id: thread_id.clone(),
                response_tx: tx,
                message: message.clone(),
                envelopes: envelopes.clone(),
            },
        );
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
        (client_id, stream_id, seq_id, envelopes)
    };
    chain_log::write_line(format!(
        "[remote_control] event=request_send client_key={} client_id={} stream_id={} seq_id={} request_id={} method={} thread={}",
        client_key,
        client_id,
        stream_id,
        seq_id,
        id,
        method_name,
        thread_id.as_deref().unwrap_or("")
    ));
    if let Err(err) = send_envelopes(state, envelope).await {
        let mut remote = state.remote_control.inner.lock().await;
        if let Some(client) = remote.clients.get_mut(&client_key) {
            client.pending.remove(&request_key);
        }
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
        return Err(err);
    }
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(anyhow!("remote-control response channel closed")),
        Err(_) => {
            {
                let mut remote = state.remote_control.inner.lock().await;
                if let Some(client) = remote.clients.get_mut(&client_key) {
                    client.pending.remove(&request_key);
                }
                if client_key == DEFAULT_REMOTE_CLIENT_KEY {
                    sync_default_client_legacy_locked(&mut remote);
                }
            }
            state
                .push_event(
                    "warn",
                    "remote_control_request_timeout",
                    format!(
                        "client_key={} method={} id={} timeout_secs={}",
                        client_key,
                        method_name,
                        id,
                        timeout.as_secs()
                    ),
                )
                .await;
            Err(anyhow!(
                "remote-control request timed out: client_key={} method={} id={} after {}s",
                client_key,
                method_name,
                id,
                timeout.as_secs()
            ))
        }
    }
}

async fn wait_for_remote_control_initialized(state: &SharedState, client_key: &str) -> Result<()> {
    let client_key = normalize_remote_client_key(client_key);
    let start = tokio::time::Instant::now();
    loop {
        {
            let remote = state.remote_control.inner.lock().await;
            if remote
                .clients
                .get(&client_key)
                .is_some_and(|client| remote.connected && client.initialized)
            {
                return Ok(());
            }
            if !remote.connected {
                return Err(anyhow!(
                    "Codex app-server remote-control disconnected during reinitialize"
                ));
            }
        }
        if start.elapsed() >= REMOTE_CONTROL_REINITIALIZE_RETRY_TIMEOUT {
            return Err(anyhow!(
                "remote-control client initialize did not complete within {}s: client_key={}",
                REMOTE_CONTROL_REINITIALIZE_RETRY_TIMEOUT.as_secs(),
                client_key
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadStartOptions {
    pub cwd: Option<String>,
    pub model_provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permissions: Option<String>,
    pub approval_policy: Option<String>,
    pub approvals_reviewer: Option<String>,
}

impl ThreadStartOptions {
    fn to_params(&self) -> Value {
        let mut params = serde_json::Map::new();
        if let Some(cwd) = non_empty(self.cwd.as_deref()) {
            params.insert("cwd".to_string(), json!(cwd));
            params.insert("runtimeWorkspaceRoots".to_string(), json!([cwd]));
        }
        if let Some(model_provider) = non_empty(self.model_provider.as_deref()) {
            params.insert("modelProvider".to_string(), json!(model_provider));
        }
        if let Some(model) = non_empty(self.model.as_deref()) {
            params.insert("model".to_string(), json!(model));
        }
        if let Some(effort) = non_empty(self.reasoning_effort.as_deref()) {
            params.insert(
                "config".to_string(),
                json!({
                    "model_reasoning_effort": effort,
                }),
            );
        }
        if let Some(permissions) = non_empty(self.permissions.as_deref()) {
            params.insert("permissions".to_string(), json!(permissions));
        }
        if let Some(approval_policy) = non_empty(self.approval_policy.as_deref()) {
            params.insert("approvalPolicy".to_string(), json!(approval_policy));
        }
        if let Some(approvals_reviewer) = non_empty(self.approvals_reviewer.as_deref()) {
            params.insert("approvalsReviewer".to_string(), json!(approvals_reviewer));
        }
        Value::Object(params)
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub async fn start_thread_for_client(
    state: &SharedState,
    client_key: &str,
    options: ThreadStartOptions,
) -> Result<String> {
    let response =
        request_for_client(state, client_key, "thread/start", options.to_params()).await?;
    let thread_id = response
        .get("thread")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("thread/start response missing thread.id: {response}"))?;
    mark_thread_active_for_client(state, Some(client_key), &thread_id).await;
    Ok(thread_id)
}

pub async fn config_read_for_client(
    state: &SharedState,
    client_key: &str,
    cwd: Option<&str>,
    include_layers: bool,
) -> Result<Value> {
    let mut params = json!({});
    if let Some(cwd) = non_empty(cwd) {
        params["cwd"] = json!(cwd);
    }
    if include_layers {
        params["includeLayers"] = json!(true);
    }
    request_for_client(state, client_key, "config/read", params).await
}

pub async fn model_list_for_client(
    state: &SharedState,
    client_key: &str,
    include_hidden: bool,
    limit: Option<u32>,
) -> Result<Value> {
    let mut params = json!({
        "includeHidden": include_hidden,
    });
    if let Some(limit) = limit {
        params["limit"] = json!(limit);
    }
    request_for_client(state, client_key, "model/list", params).await
}

pub async fn thread_list_for_client(
    state: &SharedState,
    client_key: &str,
    cursor: Option<&str>,
    limit: Option<u32>,
    cwd: Option<&str>,
    model_provider: Option<&str>,
) -> Result<Value> {
    let mut params = json!({});
    if let Some(cursor) = cursor {
        params["cursor"] = json!(cursor);
    }
    if let Some(limit) = limit {
        params["limit"] = json!(limit);
    }
    params["sortKey"] = json!("updated_at");
    params["sourceKinds"] = json!(["cli", "vscode", "appServer"]);
    params["archived"] = json!(false);
    if let Some(cwd) = cwd {
        params["cwd"] = json!(cwd);
    }
    if let Some(model_provider) = non_empty(model_provider) {
        params["modelProviders"] = json!([model_provider]);
    }
    request_with_timeout_for_client(
        state,
        client_key,
        "thread/list",
        params,
        REMOTE_DISCOVERY_REQUEST_TIMEOUT,
    )
    .await
}

pub async fn thread_loaded_list_for_client(
    state: &SharedState,
    client_key: &str,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<Value> {
    let mut params = json!({});
    if let Some(cursor) = cursor {
        params["cursor"] = json!(cursor);
    }
    if let Some(limit) = limit {
        params["limit"] = json!(limit);
    }
    request_with_timeout_for_client(
        state,
        client_key,
        "thread/loaded/list",
        params,
        REMOTE_DISCOVERY_REQUEST_TIMEOUT,
    )
    .await
}

pub async fn resume_thread_for_client(
    state: &SharedState,
    client_key: &str,
    thread_id: &str,
    exclude_turns: bool,
) -> Result<Value> {
    let response = request_for_client(
        state,
        client_key,
        "thread/resume",
        json!({
            "threadId": thread_id,
            "excludeTurns": exclude_turns,
        }),
    )
    .await?;
    mark_thread_active_for_client(state, Some(client_key), thread_id).await;
    Ok(response)
}

pub async fn start_turn_for_client(
    state: &SharedState,
    client_key: &str,
    thread_id: &str,
    text: &str,
    attachments: &[InboundAttachment],
) -> Result<String> {
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[im_trace] event=remote_to_codex_turn_start client_key={} thread={} text_len={} attachments={} preview={}",
            client_key,
            thread_id,
            text.chars().count(),
            attachments.len(),
            log_text_preview(text, 360)
        )
    });
    let response = request_for_client(
        state,
        client_key,
        "turn/start",
        json!({
            "threadId": thread_id,
            "input": turn_input_items(text, attachments),
        }),
    )
    .await?;
    let turn_id = response
        .get("turn")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("turn/start response missing turn.id: {response}"))?;
    {
        let mut remote = state.remote_control.inner.lock().await;
        let client_key = normalize_remote_client_key(client_key);
        let client = ensure_client_state_locked(&mut remote, &client_key);
        client.current_thread_id = Some(thread_id.to_string());
        client.current_turn_id = Some(turn_id.clone());
        if client_key == DEFAULT_REMOTE_CLIENT_KEY {
            sync_default_client_legacy_locked(&mut remote);
        }
    }
    Ok(turn_id)
}

pub async fn interrupt_turn_for_client(
    state: &SharedState,
    client_key: &str,
    thread_id: &str,
    turn_id: &str,
) -> Result<()> {
    request_for_client(
        state,
        client_key,
        "turn/interrupt",
        json!({
            "threadId": thread_id,
            "turnId": turn_id,
        }),
    )
    .await
    .map(|_| ())
}

pub async fn current_thread_for_client(state: &SharedState, client_key: &str) -> Option<String> {
    let client_key = normalize_remote_client_key(client_key);
    state
        .remote_control
        .inner
        .lock()
        .await
        .clients
        .get(&client_key)
        .and_then(|client| client.current_thread_id.clone())
}

pub async fn clear_turn_for_client(state: &SharedState, client_key: &str, turn_id: Option<&str>) {
    let client_key = normalize_remote_client_key(client_key);
    let mut remote = state.remote_control.inner.lock().await;
    if let Some(client) = remote.clients.get_mut(&client_key) {
        if turn_id.is_none() || client.current_turn_id.as_deref() == turn_id {
            client.current_turn_id = None;
        }
    }
    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
        sync_default_client_legacy_locked(&mut remote);
    }
}

pub async fn clear_thread_for_client(
    state: &SharedState,
    client_key: &str,
    thread_id: Option<&str>,
) {
    let client_key = normalize_remote_client_key(client_key);
    let mut remote = state.remote_control.inner.lock().await;
    if let Some(client) = remote.clients.get_mut(&client_key) {
        if thread_id.is_none() || client.current_thread_id.as_deref() == thread_id {
            client.current_thread_id = None;
            client.current_turn_id = None;
        }
    }
    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
        sync_default_client_legacy_locked(&mut remote);
    }
}

async fn ack_server_envelope(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: Option<usize>,
) -> Result<()> {
    let outbound_tx = {
        let remote = state.remote_control.inner.lock().await;
        remote
            .outbound_tx
            .clone()
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    outbound_tx
        .send(OutboundWsMessage::Text(json!(OutgoingClientEnvelope {
            event: OutgoingClientEvent::Ack { segment_id },
            client_id: client_id.to_string(),
            stream_id: Some(stream_id.to_string()),
            seq_id: Some(seq_id),
            cursor: None,
        })))
        .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    record_remote_recent_event(
        state,
        "client_out",
        connection_epoch,
        client_id,
        stream_id,
        Some(seq_id),
        "ack",
        format!(
            "segment_id={}",
            segment_id
                .map(|value| value.to_string())
                .unwrap_or_default()
        ),
    )
    .await;
    Ok(())
}

async fn send_envelope(state: &SharedState, envelope: Value) -> Result<()> {
    let client_id = envelope
        .get("client_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let stream_id = envelope
        .get("stream_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let seq_id = envelope.get("seq_id").and_then(Value::as_u64);
    let seq_id_text = seq_id.map(|value| value.to_string()).unwrap_or_default();
    let summary = message_summary(&envelope);
    chain_log::write_line(format!(
        "[remote_control] event=client_envelope client_id={} stream_id={} seq_id={} summary={}",
        client_id, stream_id, seq_id_text, summary
    ));
    info!(
        target: "codex_remote::remote_control",
        event = "remote_control_client_envelope",
        summary = %summary,
        "remote-control client envelope"
    );
    let (connection_epoch, outbound_tx) = {
        let remote = state.remote_control.inner.lock().await;
        (
            remote.connection_epoch,
            remote
                .outbound_tx
                .clone()
                .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?,
        )
    };
    record_remote_recent_event(
        state,
        "client_out",
        connection_epoch,
        client_id,
        stream_id,
        seq_id,
        client_envelope_recent_kind(&envelope),
        summary,
    )
    .await;
    outbound_tx
        .send(OutboundWsMessage::Text(envelope))
        .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    Ok(())
}

async fn send_envelopes(state: &SharedState, envelopes: Vec<Value>) -> Result<()> {
    let outbound_tx = {
        let remote = state.remote_control.inner.lock().await;
        remote
            .outbound_tx
            .clone()
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    let envelope_count = envelopes.len();
    for envelope in envelopes {
        let client_id = envelope
            .get("client_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let stream_id = envelope
            .get("stream_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let seq_id = envelope.get("seq_id").and_then(Value::as_u64);
        let seq_id_text = seq_id.map(|value| value.to_string()).unwrap_or_default();
        let summary = message_summary(&envelope);
        chain_log::write_line(format!(
            "[remote_control] event=client_envelope envelope_count={} client_id={} stream_id={} seq_id={} summary={}",
            envelope_count, client_id, stream_id, seq_id_text, summary
        ));
        info!(
            target: "codex_remote::remote_control",
            event = "remote_control_client_envelope",
            envelope_count,
            summary = %summary,
            "remote-control client envelope"
        );
        let connection_epoch = state.remote_control.inner.lock().await.connection_epoch;
        record_remote_recent_event(
            state,
            "client_out",
            connection_epoch,
            client_id,
            stream_id,
            seq_id,
            client_envelope_recent_kind(&envelope),
            summary,
        )
        .await;
        outbound_tx
            .send(OutboundWsMessage::Text(envelope))
            .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    }
    Ok(())
}

async fn send_ws_control_ping(state: &SharedState) -> Result<()> {
    chain_log::write_line("[remote_control] event=client_ping payload_len=0".to_string());
    info!(
        target: "codex_remote::remote_control",
        event = "remote_control_client_ping",
        payload_len = 0usize,
        "remote-control client ping"
    );
    let outbound_tx = {
        let remote = state.remote_control.inner.lock().await;
        remote
            .outbound_tx
            .clone()
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    outbound_tx
        .send(OutboundWsMessage::Ping(axum::body::Bytes::new()))
        .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    Ok(())
}

async fn send_ws_control_pong(state: &SharedState, data: axum::body::Bytes) -> Result<()> {
    chain_log::write_line(format!(
        "[remote_control] event=client_pong payload_len={}",
        data.len()
    ));
    info!(
        target: "codex_remote::remote_control",
        event = "remote_control_client_pong",
        payload_len = data.len(),
        "remote-control client pong"
    );
    let outbound_tx = {
        let remote = state.remote_control.inner.lock().await;
        remote
            .outbound_tx
            .clone()
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    outbound_tx
        .send(OutboundWsMessage::Pong(data))
        .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    Ok(())
}

fn build_client_message_envelopes(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    message: Value,
    cursor: Option<&str>,
) -> Result<Vec<Value>> {
    let envelope =
        build_client_envelope(client_id, Some(stream_id), seq_id, message.clone(), cursor);
    if serialized_json_len(&envelope)? <= REMOTE_CONTROL_SEGMENT_MAX_BYTES {
        return Ok(vec![envelope]);
    }

    let raw = serde_json::to_vec(&message).context("failed to serialize remote-control message")?;
    let message_size_bytes = raw.len();
    if message_size_bytes > REMOTE_CONTROL_REASSEMBLED_MAX_BYTES {
        anyhow::bail!(
            "remote-control message exceeds reassembled size limit: {} bytes",
            message_size_bytes
        );
    }

    let minimal_segment_count =
        usize::min(message_size_bytes.max(1), REMOTE_CONTROL_SEGMENT_COUNT_MAX);
    let minimal_chunk = &raw[..usize::min(raw.len(), 1)];
    if serialized_client_chunk_len(
        client_id,
        stream_id,
        seq_id,
        0,
        minimal_segment_count,
        message_size_bytes,
        minimal_chunk,
        cursor,
    )? > REMOTE_CONTROL_SEGMENT_MAX_BYTES
    {
        anyhow::bail!("remote-control message cannot fit within segment size limit");
    }

    let mut segment_count = usize::max(
        2,
        message_size_bytes.div_ceil(REMOTE_CONTROL_SEGMENT_TARGET_BYTES),
    );
    loop {
        if segment_count > REMOTE_CONTROL_SEGMENT_COUNT_MAX {
            anyhow::bail!(
                "remote-control segment count exceeds maximum: {}",
                segment_count
            );
        }
        let chunk_size = usize::max(1, message_size_bytes.div_ceil(segment_count));
        segment_count = message_size_bytes.div_ceil(chunk_size);
        let segments_fit = raw
            .chunks(chunk_size)
            .enumerate()
            .all(|(segment_id, chunk)| {
                serialized_client_chunk_len(
                    client_id,
                    stream_id,
                    seq_id,
                    segment_id,
                    segment_count,
                    message_size_bytes,
                    chunk,
                    cursor,
                )
                .is_ok_and(|size| size <= REMOTE_CONTROL_SEGMENT_MAX_BYTES)
            });
        if segments_fit {
            chain_log::write_line(format!(
                "[remote_control] event=client_segmented client_id={} stream_id={} seq_id={} bytes={} segment_count={} summary={}",
                client_id,
                stream_id,
                seq_id,
                message_size_bytes,
                segment_count,
                message_summary(&message)
            ));
            warn!(
                target: "codex_remote::remote_control",
                event = "remote_control_client_segmented",
                client_id,
                stream_id,
                seq_id,
                bytes = message_size_bytes,
                segment_count,
                summary = %message_summary(&message),
                "remote-control client message segmented"
            );
            return raw
                .chunks(chunk_size)
                .enumerate()
                .map(|(segment_id, chunk)| {
                    build_client_chunk_envelope(
                        client_id,
                        stream_id,
                        seq_id,
                        segment_id,
                        segment_count,
                        message_size_bytes,
                        chunk,
                        cursor,
                    )
                })
                .collect();
        }
        if chunk_size == 1 {
            anyhow::bail!("remote-control message cannot fit within segment size limit");
        }
        let next_segment_count = segment_count + 1;
        let next_chunk_size = usize::max(1, message_size_bytes.div_ceil(next_segment_count));
        segment_count = if next_chunk_size == chunk_size {
            message_size_bytes
        } else {
            next_segment_count
        };
    }
}

fn serialized_client_chunk_len(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    chunk: &[u8],
    cursor: Option<&str>,
) -> Result<usize> {
    serialized_json_len(&build_client_chunk_envelope(
        client_id,
        stream_id,
        seq_id,
        segment_id,
        segment_count,
        message_size_bytes,
        chunk,
        cursor,
    )?)
}

fn build_client_chunk_envelope(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    chunk: &[u8],
    cursor: Option<&str>,
) -> Result<Value> {
    if segment_count > REMOTE_CONTROL_SEGMENT_COUNT_MAX {
        anyhow::bail!(
            "remote-control segment count exceeds maximum: {}",
            segment_count
        );
    }
    Ok(json!(OutgoingClientEnvelope {
        event: OutgoingClientEvent::ClientMessageChunk {
            segment_id,
            segment_count,
            message_size_bytes,
            message_chunk_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
        },
        client_id: client_id.to_string(),
        stream_id: Some(stream_id.to_string()),
        seq_id: Some(seq_id),
        cursor: cursor.map(str::to_string),
    }))
}

fn serialized_json_len(value: &Value) -> Result<usize> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .context("failed to serialize remote-control envelope")
}

fn json_preview(text: &str) -> String {
    const LIMIT: usize = 220;
    let compact = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut out = String::new();
    for ch in compact.chars().take(LIMIT) {
        out.push(ch);
    }
    if compact.chars().count() > LIMIT {
        out.push_str("...");
    }
    out
}

fn log_text_preview(text: &str, limit: usize) -> String {
    let compact = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut out = String::new();
    for ch in compact.chars().take(limit) {
        out.push(ch);
    }
    if compact.chars().count() > limit {
        out.push_str("...");
    }
    out
}

fn log_codex_to_remote_message(connection_epoch: u64, message: &Value) {
    if !chain_log::diagnostic_enabled() {
        return;
    }
    let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("");
    if method.is_empty() {
        return;
    }
    let params = message.get("params");
    let thread_id = params
        .and_then(thread_id_from_payload)
        .or_else(|| thread_id_from_payload(message))
        .unwrap_or_default();
    let turn_id = params
        .and_then(turn_id_from_payload)
        .or_else(|| turn_id_from_payload(message))
        .unwrap_or_default();
    let item = params.and_then(|p| p.get("item"));
    let item_id = item
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .and_then(|p| p.get("itemId"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("");
    let item_type = item
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let text = codex_message_text_for_log(method, params, item);
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[im_trace] event=codex_to_remote connection_epoch={} method={} thread={} turn={} item={} type={} text_len={} preview={}",
            connection_epoch,
            method,
            thread_id,
            turn_id,
            item_id,
            item_type,
            text.chars().count(),
            log_text_preview(&text, 500)
        )
    });
}

fn turn_id_from_payload(value: &Value) -> Option<String> {
    value
        .get("turnId")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_string)
}

fn codex_message_text_for_log(
    method: &str,
    params: Option<&Value>,
    item: Option<&Value>,
) -> String {
    if let Some(delta) = params.and_then(|p| p.get("delta")).and_then(|v| v.as_str()) {
        return delta.to_string();
    }
    if let Some(message) = params
        .and_then(|p| p.get("message"))
        .and_then(|v| v.as_str())
    {
        return message.to_string();
    }
    if let Some(item) = item {
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            return text.to_string();
        }
        if let Some(text) = item.get("aggregatedOutput").and_then(|v| v.as_str()) {
            return text.to_string();
        }
        if method.contains("commandExecution")
            && let Some(command) = item
                .get("commandActions")
                .and_then(|v| v.as_array())
                .and_then(|actions| actions.first())
                .and_then(|action| action.get("command"))
                .and_then(|v| v.as_str())
                .or_else(|| item.get("command").and_then(|v| v.as_str()))
        {
            return command.to_string();
        }
        return item.to_string();
    }
    params.map(Value::to_string).unwrap_or_default()
}

fn is_command_execution_output_delta_message(message: &Value) -> bool {
    let message = message.get("message").unwrap_or(message);
    message.get("method").and_then(|value| value.as_str())
        == Some("item/commandExecution/outputDelta")
}

fn message_summary(value: &Value) -> String {
    if let Some(message) = value.get("message") {
        return message_summary(message);
    }
    if let Some(method) = value.get("method").and_then(|v| v.as_str()) {
        let id = value.get("id").map(|v| v.to_string()).unwrap_or_default();
        let thread_id = thread_id_from_payload(value)
            .or_else(|| value.get("params").and_then(thread_id_from_payload))
            .unwrap_or_default();
        return format!("method={method} id={id} thread={thread_id}");
    }
    if let Some(id) = value.get("id") {
        if value.get("result").is_some() {
            let thread_id = value
                .get("result")
                .and_then(thread_id_from_payload)
                .unwrap_or_default();
            return format!("response id={} thread={thread_id}", id);
        }
        if let Some(error) = value.get("error") {
            return format!("error id={} body={}", id, json_preview(&error.to_string()));
        }
    }
    json_preview(&value.to_string())
}

fn client_envelope_recent_kind(envelope: &Value) -> String {
    envelope
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("client_message")
        .to_string()
}

fn pending_requests_summary(pending: &HashMap<String, PendingRemoteRequest>) -> String {
    if pending.is_empty() {
        return String::new();
    }
    pending
        .iter()
        .map(|(request_key, pending)| {
            format!(
                "{}:{}:thread={}:envelopes={}",
                request_key,
                pending.method,
                pending.thread_id.as_deref().unwrap_or_default(),
                pending.envelopes.len()
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn format_recent_event(event: &RemoteControlRecentEvent) -> String {
    format!(
        "ts_ms={} direction={} connection_epoch={} client_id={} stream_id={} seq_id={} kind={} summary={}",
        event.ts_ms,
        event.direction,
        event.connection_epoch,
        event.client_id,
        event.stream_id,
        event
            .seq_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        event.kind,
        event.summary
    )
}

fn build_client_envelope(
    client_id: &str,
    stream_id: Option<&str>,
    seq_id: u64,
    message: Value,
    cursor: Option<&str>,
) -> Value {
    json!(OutgoingClientEnvelope {
        event: OutgoingClientEvent::ClientMessage { message },
        client_id: client_id.to_string(),
        stream_id: stream_id.map(str::to_string),
        seq_id: Some(seq_id),
        cursor: cursor.map(str::to_string),
    })
}

fn turn_input_items(text: &str, attachments: &[InboundAttachment]) -> Vec<Value> {
    let mut items = Vec::new();
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        items.push(json!({
            "type": "text",
            "text": trimmed,
            "text_elements": [],
        }));
    }
    for attachment in attachments {
        let Some(local_path) = attachment
            .local_path
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        match attachment.kind.as_str() {
            "image" => items.push(json!({
                "type": "localImage",
                "path": local_path,
            })),
            "file" | "text" | "video" => items.push(json!({
                "type": "text",
                "text": format!("File: {local_path}"),
                "text_elements": [],
            })),
            _ => {}
        }
    }
    if items.is_empty() {
        items.push(json!({
            "type": "text",
            "text": "",
            "text_elements": [],
        }));
    }
    items
}

async fn is_active_connection_epoch(state: &SharedState, connection_epoch: u64) -> bool {
    state.remote_control.inner.lock().await.connection_epoch == connection_epoch
}

fn next_request_id() -> u64 {
    REMOTE_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

fn request_id_key(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn stable_id(prefix: &str, seed: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in seed.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{prefix}_{hash:016x}")
}

fn stable_base64url_32(prefix: &str, seed: &str) -> String {
    let mut bytes = [0u8; 32];
    for chunk in 0..4 {
        let id = stable_id(prefix, &format!("{seed}:{chunk}"));
        let hex = id.rsplit('_').next().unwrap_or_default();
        let value = u64::from_str_radix(hex, 16).unwrap_or_default();
        bytes[(chunk * 8)..((chunk + 1) * 8)].copy_from_slice(&value.to_be_bytes());
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn uuid_like() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = REMOTE_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    format!("{now:032x}-{counter:016x}")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use base64::Engine;
    use serde_json::{Value, json};

    use super::*;
    use crate::{app_state::AppState, config::AppConfig};

    fn test_state() -> SharedState {
        let mut config = AppConfig::default();
        config.state_path =
            std::env::temp_dir().join(format!("codex-remote-test-{}.json", uuid_like()));
        AppState::new(
            std::env::temp_dir().join("codex-remote-test-config.toml"),
            config,
            None,
        )
    }

    fn remote_inner_for_test(stream_id: &str) -> RemoteControlInner {
        RemoteControlInner {
            connected: false,
            initialized: false,
            client_id: FEISHU_BRIDGE_CLIENT_ID.to_string(),
            stream_id: stream_id.to_string(),
            server_id: None,
            environment_id: None,
            server_name: None,
            installation_id: None,
            account_id: None,
            current_thread_id: None,
            current_turn_id: None,
            last_error: None,
            connected_at_ms: None,
            last_ws_inbound_at_ms: None,
            last_ws_ping_at_ms: None,
            last_ws_pong_at_ms: None,
            last_app_ping_at_ms: None,
            last_app_pong_at_ms: None,
            last_app_pong_status: None,
            last_initialize_sent_at_ms: None,
            subscribe_cursor: None,
            server_ack_cursors: HashMap::new(),
            outbound_tx: None,
            connection_epoch: 0,
            clients: HashMap::new(),
            authorized_clients: HashMap::new(),
            revoked_clients: std::collections::HashSet::new(),
            stream_diagnostics: HashMap::new(),
            recent_events: std::collections::VecDeque::new(),
        }
    }

    fn test_server_message_envelope(
        client_id: &str,
        stream_id: &str,
        seq_id: u64,
        message: Value,
    ) -> String {
        json!({
            "type": "server_message",
            "client_id": client_id,
            "stream_id": stream_id,
            "seq_id": seq_id,
            "message": message,
        })
        .to_string()
    }

    fn test_server_chunk_envelope(
        client_id: &str,
        stream_id: &str,
        seq_id: u64,
        segment_id: usize,
        segment_count: usize,
        message_size_bytes: usize,
        chunk: &[u8],
    ) -> String {
        json!({
            "type": "server_message_chunk",
            "client_id": client_id,
            "stream_id": stream_id,
            "seq_id": seq_id,
            "segment_id": segment_id,
            "segment_count": segment_count,
            "message_size_bytes": message_size_bytes,
            "message_chunk_base64": base64::engine::general_purpose::STANDARD.encode(chunk),
        })
        .to_string()
    }

    fn take_text_envelopes(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<OutboundWsMessage>,
    ) -> Vec<Value> {
        let mut values = Vec::new();
        while let Ok(message) = rx.try_recv() {
            if let OutboundWsMessage::Text(value) = message {
                values.push(value);
            }
        }
        values
    }

    fn envelope_message_method(envelope: &Value) -> Option<&str> {
        envelope
            .get("message")
            .and_then(|message| message.get("method"))
            .and_then(Value::as_str)
    }

    fn envelope_is_ack(envelope: &Value) -> bool {
        envelope.get("type").and_then(Value::as_str) == Some("ack")
    }

    async fn setup_connected_default_client(
        state: &SharedState,
    ) -> (
        tokio::sync::mpsc::UnboundedReceiver<OutboundWsMessage>,
        String,
        String,
        u64,
    ) {
        let (outbound_tx, outbound_rx) = tokio::sync::mpsc::unbounded_channel();
        let (client_id, stream_id, connection_epoch) = {
            let mut remote = state.remote_control.inner.lock().await;
            remote.connected = true;
            remote.connection_epoch = 7;
            remote.outbound_tx = Some(outbound_tx);
            remote.stream_id = "stream-test".to_string();
            let client = ensure_client_state_locked(&mut remote, DEFAULT_REMOTE_CLIENT_KEY);
            client.initialized = true;
            let client_id = client.client_id.clone();
            let stream_id = client.stream_id.clone();
            sync_default_client_legacy_locked(&mut remote);
            (client_id, stream_id, remote.connection_epoch)
        };
        (outbound_rx, client_id, stream_id, connection_epoch)
    }

    #[test]
    fn ack_cursor_orders_whole_message_after_chunks() {
        assert!(ack_cursor_gt((2, None), (1, None)));
        assert!(ack_cursor_gt((2, None), (2, Some(7))));
        assert!(ack_cursor_gt((2, Some(8)), (2, Some(7))));
        assert!(!ack_cursor_gt((2, Some(7)), (2, None)));
        assert!(!ack_cursor_gt((1, None), (2, Some(0))));
    }

    #[test]
    fn observe_server_chunk_reassembles_json_message() {
        let message = json!({
            "method": "turn/completed",
            "params": {
                "threadId": "thread-1"
            }
        });
        let raw = serde_json::to_vec(&message).expect("serialize message");
        let split_at = raw.len() / 2;
        let first = base64::engine::general_purpose::STANDARD.encode(&raw[..split_at]);
        let second = base64::engine::general_purpose::STANDARD.encode(&raw[split_at..]);
        let mut chunks = HashMap::new();

        let pending = observe_server_chunk(
            &mut chunks,
            "client-1",
            "stream-1",
            1,
            0,
            2,
            raw.len(),
            &first,
        );
        assert!(matches!(pending, ServerChunkObservation::Pending));

        let complete = observe_server_chunk(
            &mut chunks,
            "client-1",
            "stream-1",
            1,
            1,
            2,
            raw.len(),
            &second,
        );
        match complete {
            ServerChunkObservation::Complete(complete) => assert_eq!(complete, message),
            ServerChunkObservation::Pending | ServerChunkObservation::Dropped => {
                panic!("expected complete message")
            }
        }
        assert!(chunks.is_empty());
    }

    #[test]
    fn observe_server_chunk_rejects_size_overflow() {
        let chunk = base64::engine::general_purpose::STANDARD.encode(b"too-large");
        let mut chunks = HashMap::new();
        let observation =
            observe_server_chunk(&mut chunks, "client-1", "stream-1", 1, 0, 1, 1, &chunk);
        assert!(matches!(observation, ServerChunkObservation::Dropped));
        assert!(chunks.is_empty());
    }

    #[test]
    fn observe_server_chunk_ignores_duplicate_without_dropping_current_assembly() {
        let message = json!({"method": "turn/completed", "params": {"threadId": "thread-1"}});
        let raw = serde_json::to_vec(&message).expect("serialize message");
        let split_at = raw.len() / 2;
        let first = base64::engine::general_purpose::STANDARD.encode(&raw[..split_at]);
        let second = base64::engine::general_purpose::STANDARD.encode(&raw[split_at..]);
        let mut chunks = HashMap::new();

        assert!(matches!(
            observe_server_chunk(
                &mut chunks,
                "client-1",
                "stream-1",
                8,
                0,
                2,
                raw.len(),
                &first,
            ),
            ServerChunkObservation::Pending
        ));
        assert!(matches!(
            observe_server_chunk(&mut chunks, "client-1", "stream-1", 8, 0, 2, raw.len(), "",),
            ServerChunkObservation::Dropped
        ));
        match observe_server_chunk(
            &mut chunks,
            "client-1",
            "stream-1",
            8,
            1,
            2,
            raw.len(),
            &second,
        ) {
            ServerChunkObservation::Complete(complete) => assert_eq!(complete, message),
            ServerChunkObservation::Pending | ServerChunkObservation::Dropped => {
                panic!("duplicate chunk should not drop current assembly")
            }
        }
    }

    #[test]
    fn recovery_retry_policy_does_not_replay_non_idempotent_requests() {
        assert!(!should_retry_request_after_reinitialize("turn/start"));
        assert!(!should_retry_request_after_reinitialize("turn/steer"));
        assert!(!should_retry_request_after_reinitialize("thread/start"));
        assert!(!should_retry_request_after_reinitialize("thread/fork"));
        assert!(should_retry_request_after_reinitialize("thread/list"));
        assert!(should_retry_request_after_reinitialize("thread/resume"));
    }

    #[test]
    fn virtual_remote_clients_share_enrolled_client_id_and_use_distinct_streams() {
        let mut remote = remote_inner_for_test("default-stream");

        let feishu = ensure_client_state_locked(&mut remote, "feishu:default:chat-1");
        let feishu_client_id = feishu.client_id.clone();
        let feishu_stream_id = feishu.stream_id.clone();
        let wechat = ensure_client_state_locked(&mut remote, "wechat:bot:user-1");
        let wechat_client_id = wechat.client_id.clone();
        let wechat_stream_id = wechat.stream_id.clone();

        assert_eq!(feishu_client_id, FEISHU_BRIDGE_CLIENT_ID);
        assert_eq!(wechat_client_id, FEISHU_BRIDGE_CLIENT_ID);
        assert_ne!(feishu_stream_id, wechat_stream_id);
        assert_eq!(
            remote_client_key_for_stream_locked(&remote, &feishu_client_id, &feishu_stream_id)
                .as_deref(),
            Some("feishu:default:chat-1")
        );
        assert_eq!(
            remote_client_key_for_stream_locked(&remote, &wechat_client_id, &wechat_stream_id)
                .as_deref(),
            Some("wechat:bot:user-1")
        );
    }

    #[test]
    fn virtual_remote_client_stream_is_namespaced_by_connection_stream() {
        let mut first = remote_inner_for_test("default-stream-1");
        let mut second = remote_inner_for_test("default-stream-2");
        let client_key = "wechat:bot:user-1";
        let first_stream = ensure_client_state_locked(&mut first, client_key)
            .stream_id
            .clone();
        let second_stream = ensure_client_state_locked(&mut second, client_key)
            .stream_id
            .clone();

        assert_ne!(first_stream, second_stream);
    }

    #[test]
    fn connection_reset_removes_stale_initialize_state_but_keeps_replayable_requests() {
        let mut remote = RemoteControlInner {
            connected: false,
            initialized: false,
            client_id: FEISHU_BRIDGE_CLIENT_ID.to_string(),
            stream_id: "default-stream".to_string(),
            server_id: None,
            environment_id: None,
            server_name: None,
            installation_id: None,
            account_id: None,
            current_thread_id: None,
            current_turn_id: None,
            last_error: None,
            connected_at_ms: None,
            last_ws_inbound_at_ms: None,
            last_ws_ping_at_ms: None,
            last_ws_pong_at_ms: None,
            last_app_ping_at_ms: None,
            last_app_pong_at_ms: None,
            last_app_pong_status: None,
            last_initialize_sent_at_ms: None,
            subscribe_cursor: None,
            server_ack_cursors: HashMap::new(),
            outbound_tx: None,
            connection_epoch: 0,
            clients: HashMap::new(),
            authorized_clients: HashMap::new(),
            revoked_clients: std::collections::HashSet::new(),
            stream_diagnostics: HashMap::new(),
            recent_events: std::collections::VecDeque::new(),
        };
        let client = ensure_client_state_locked(&mut remote, DEFAULT_REMOTE_CLIENT_KEY);
        client.initialized = true;
        client.last_app_ping_at_ms = Some(10);
        client.last_app_pong_at_ms = Some(11);
        client.last_app_pong_status = Some("active".to_string());
        client.last_initialize_sent_at_ms = Some(12);
        let (initialize_tx, _initialize_rx) = tokio::sync::oneshot::channel();
        client.pending.insert(
            "1".to_string(),
            PendingRemoteRequest {
                method: "initialize".to_string(),
                thread_id: None,
                response_tx: initialize_tx,
                message: json!({"id": 1, "method": "initialize"}),
                envelopes: Vec::new(),
            },
        );
        let (request_tx, _request_rx) = tokio::sync::oneshot::channel();
        client.pending.insert(
            "2".to_string(),
            PendingRemoteRequest {
                method: "thread/list".to_string(),
                thread_id: None,
                response_tx: request_tx,
                message: json!({"id": 2, "method": "thread/list"}),
                envelopes: Vec::new(),
            },
        );

        let ack_keys = reset_remote_clients_for_connection_locked(&mut remote);
        let client = remote
            .clients
            .get(DEFAULT_REMOTE_CLIENT_KEY)
            .expect("default client");

        assert_eq!(ack_keys.len(), 1);
        assert!(!client.initialized);
        assert!(client.last_app_ping_at_ms.is_none());
        assert!(client.last_app_pong_at_ms.is_none());
        assert!(client.last_app_pong_status.is_none());
        assert!(client.last_initialize_sent_at_ms.is_none());
        assert!(!client.pending.contains_key("1"));
        assert!(client.pending.contains_key("2"));
    }

    #[tokio::test]
    async fn record_remote_app_pong_unknown_requests_reinitialize_after_initialize() {
        let state = test_state();
        {
            let mut remote = state.remote_control.inner.lock().await;
            remote.connection_epoch = 7;
            remote.connected = true;
            let client = ensure_client_state_locked(&mut remote, DEFAULT_REMOTE_CLIENT_KEY);
            client.initialized = true;
            let client_id = client.client_id.clone();
            let stream_id = client.stream_id.clone();
            sync_default_client_legacy_locked(&mut remote);
            drop(remote);
            assert!(
                record_remote_app_pong(&state, 7, &client_id, &stream_id, "unknown")
                    .await
                    .expect("record pong")
            );
        }
        let remote = state.remote_control.inner.lock().await;
        assert_eq!(
            remote
                .clients
                .get(DEFAULT_REMOTE_CLIENT_KEY)
                .and_then(|client| client.last_app_pong_status.as_deref()),
            Some("unknown")
        );
    }

    #[tokio::test]
    async fn unknown_reinitializes_same_stream_without_client_closed() {
        let state = test_state();
        let (mut outbound_rx, client_id, stream_id, connection_epoch) =
            setup_connected_default_client(&state).await;

        start_remote_control_client_recovery(
            &state,
            connection_epoch,
            DEFAULT_REMOTE_CLIENT_KEY,
            &client_id,
            &stream_id,
        )
        .await
        .expect("recovery should start");

        let envelopes = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let envelopes = take_text_envelopes(&mut outbound_rx);
                if envelopes
                    .iter()
                    .any(|envelope| envelope_message_method(envelope) == Some("initialize"))
                {
                    return envelopes;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("initialize should be sent");

        let initialize = envelopes
            .iter()
            .find(|envelope| envelope_message_method(envelope) == Some("initialize"))
            .expect("initialize envelope");
        assert_eq!(initialize["client_id"], client_id);
        assert_eq!(initialize["stream_id"], stream_id);
        assert!(
            envelopes
                .iter()
                .all(|envelope| envelope.get("type").and_then(Value::as_str)
                    != Some("client_closed"))
        );

        let remote = state.remote_control.inner.lock().await;
        let client = remote
            .clients
            .get(DEFAULT_REMOTE_CLIENT_KEY)
            .expect("default client");
        assert_eq!(client.stream_id, stream_id);
        assert!(!client.initialized);
        assert_eq!(client.recovery_attempt, 1);
        assert!(client.recovery_started_at_ms.is_some());
    }

    #[tokio::test]
    async fn recovery_resubscribes_current_thread_without_replaying_turn_start_and_clears_idle_turn()
     {
        let state = test_state();
        let (mut outbound_rx, client_id, stream_id, connection_epoch) =
            setup_connected_default_client(&state).await;
        {
            let mut remote = state.remote_control.inner.lock().await;
            let client = remote
                .clients
                .get_mut(DEFAULT_REMOTE_CLIENT_KEY)
                .expect("default client");
            client.current_thread_id = Some("thread-1".to_string());
            client.current_turn_id = Some("turn-1".to_string());
            sync_default_client_legacy_locked(&mut remote);
        }
        state
            .runtime
            .lock()
            .await
            .mark_turn_started("thread-1", "turn-1");

        let resubscribe_state = state.clone();
        let resubscribe = tokio::spawn(async move {
            resubscribe_current_thread_after_recovery(
                &resubscribe_state,
                connection_epoch,
                DEFAULT_REMOTE_CLIENT_KEY,
                1,
            )
            .await
        });

        let envelopes = tokio::time::timeout(Duration::from_secs(1), async {
            let mut seen = Vec::new();
            loop {
                seen.extend(take_text_envelopes(&mut outbound_rx));
                if seen
                    .iter()
                    .any(|envelope| envelope_message_method(envelope) == Some("thread/resume"))
                {
                    return seen;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("thread/resume should be sent");

        assert!(
            envelopes
                .iter()
                .all(|envelope| envelope_message_method(envelope) != Some("turn/start"))
        );
        let resume = envelopes
            .iter()
            .find(|envelope| envelope_message_method(envelope) == Some("thread/resume"))
            .expect("thread/resume envelope");
        assert_eq!(resume["client_id"], client_id);
        assert_eq!(resume["stream_id"], stream_id);
        assert_eq!(resume["message"]["params"]["threadId"], "thread-1");
        assert_eq!(resume["message"]["params"]["excludeTurns"], true);
        let request_id = resume["message"]["id"].clone();

        observe_app_server_message(
            &state,
            connection_epoch,
            &client_id,
            &stream_id,
            &json!({
                "id": request_id,
                "result": {
                    "thread": {
                        "id": "thread-1",
                        "status": {
                            "type": "idle"
                        }
                    }
                }
            }),
        )
        .await;

        tokio::time::timeout(Duration::from_secs(1), resubscribe)
            .await
            .expect("resubscribe task should finish")
            .expect("resubscribe task should not panic")
            .expect("resubscribe should succeed");

        let remote = state.remote_control.inner.lock().await;
        let client = remote
            .clients
            .get(DEFAULT_REMOTE_CLIENT_KEY)
            .expect("default client");
        assert_eq!(client.current_thread_id.as_deref(), Some("thread-1"));
        assert!(client.current_turn_id.is_none());
        drop(remote);
        assert!(
            state
                .runtime
                .lock()
                .await
                .current_turn_by_thread
                .get("thread-1")
                .is_none()
        );
    }

    #[tokio::test]
    async fn initialize_remote_clients_for_connection_sends_all_known_clients() {
        let state = test_state();
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel();
        let connection_epoch = {
            let mut remote = state.remote_control.inner.lock().await;
            remote.connected = true;
            remote.connection_epoch = 11;
            remote.outbound_tx = Some(outbound_tx);
            remote.stream_id = "stream-root".to_string();
            ensure_client_state_locked(&mut remote, DEFAULT_REMOTE_CLIENT_KEY);
            ensure_client_state_locked(&mut remote, "feishu:default:chat-1");
            ensure_client_state_locked(&mut remote, "wechat:bot:user-1");
            remote.connection_epoch
        };

        initialize_remote_clients_for_connection(&state, connection_epoch)
            .await
            .expect("initialize all clients");

        let envelopes = take_text_envelopes(&mut outbound_rx);
        let initialize_streams = envelopes
            .iter()
            .filter(|envelope| envelope_message_method(envelope) == Some("initialize"))
            .map(|envelope| {
                envelope
                    .get("stream_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect::<std::collections::HashSet<_>>();
        let expected_streams = {
            let remote = state.remote_control.inner.lock().await;
            remote
                .clients
                .values()
                .map(|client| client.stream_id.clone())
                .collect::<std::collections::HashSet<_>>()
        };
        assert_eq!(initialize_streams, expected_streams);
    }

    #[tokio::test]
    async fn server_flood_fast_ack_does_not_wait_for_work_queue_drain() {
        let state = test_state();
        let (mut outbound_rx, client_id, stream_id, connection_epoch) =
            setup_connected_default_client(&state).await;
        let (server_work_tx, mut server_work_rx) = tokio::sync::mpsc::channel::<RemoteServerWorkItem>(
            REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY,
        );
        let mut chunks = HashMap::new();

        for seq_id in 1..=300 {
            let message = json!({
                "method": "item/commandExecution/outputDelta",
                "params": {
                    "threadId": "thread-1",
                    "itemId": format!("item-{seq_id}"),
                    "delta": "x"
                }
            });
            handle_server_envelope(
                &state,
                connection_epoch,
                &test_server_message_envelope(&client_id, &stream_id, seq_id, message),
                &mut chunks,
                &server_work_tx,
            )
            .await
            .expect("server envelope should be acked");
        }

        let ack_count = take_text_envelopes(&mut outbound_rx)
            .iter()
            .filter(|envelope| envelope_is_ack(envelope))
            .count();
        assert_eq!(ack_count, 300);
        assert_eq!(
            server_work_tx.capacity(),
            REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY - 300
        );
        assert_eq!(
            server_work_rx
                .try_recv()
                .ok()
                .map(|item| remote_server_work_item_kind(&item)),
            Some("server_message")
        );

        let remote = state.remote_control.inner.lock().await;
        let key = server_ack_cursor_key(&client_id, &stream_id);
        assert_eq!(remote.server_ack_cursors.get(&key), Some(&(300, None)));
        assert_eq!(
            remote
                .stream_diagnostics
                .get(&key)
                .map(|diagnostics| diagnostics.ack_count),
            Some(300)
        );
    }

    #[tokio::test]
    async fn bad_server_chunk_is_acked_without_closing_connection() {
        let state = test_state();
        let (mut outbound_rx, client_id, stream_id, connection_epoch) =
            setup_connected_default_client(&state).await;
        let (server_work_tx, mut server_work_rx) = tokio::sync::mpsc::channel::<RemoteServerWorkItem>(
            REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY,
        );
        let mut chunks = HashMap::new();
        let message = json!({"method": "turn/completed", "params": {"threadId": "thread-1"}});
        let raw = serde_json::to_vec(&message).expect("serialize message");
        let split_at = raw.len() / 2;

        handle_server_envelope(
            &state,
            connection_epoch,
            &test_server_chunk_envelope(
                &client_id,
                &stream_id,
                1,
                0,
                2,
                raw.len(),
                &raw[..split_at],
            ),
            &mut chunks,
            &server_work_tx,
        )
        .await
        .expect("first chunk should be accepted");
        handle_server_envelope(
            &state,
            connection_epoch,
            &test_server_chunk_envelope(&client_id, &stream_id, 1, 0, 2, raw.len(), b""),
            &mut chunks,
            &server_work_tx,
        )
        .await
        .expect("duplicate bad chunk should be dropped but acked");

        let ack_count = take_text_envelopes(&mut outbound_rx)
            .iter()
            .filter(|envelope| envelope_is_ack(envelope))
            .count();
        assert_eq!(ack_count, 2);
        assert!(server_work_rx.try_recv().is_err());
        assert!(chunks.contains_key(&(client_id, stream_id, 1)));
    }

    #[tokio::test]
    async fn force_ws_reconnect_sends_close_message() {
        let state = test_state();
        let (mut outbound_rx, _client_id, _stream_id, connection_epoch) =
            setup_connected_default_client(&state).await;

        force_remote_control_ws_reconnect(
            &state,
            connection_epoch,
            DEFAULT_REMOTE_CLIENT_KEY,
            "test reconnect",
        )
        .await
        .expect("force reconnect should enqueue close");

        match outbound_rx.try_recv().expect("outbound close message") {
            OutboundWsMessage::Close(reason) => assert_eq!(reason, "test reconnect"),
            OutboundWsMessage::Text(_)
            | OutboundWsMessage::Ping(_)
            | OutboundWsMessage::Pong(_) => {
                panic!("expected close message")
            }
        }
    }
}

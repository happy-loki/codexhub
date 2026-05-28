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
use tracing::{info, warn};

use crate::{
    app_state::SharedState, chain_log, codex::CodexNotification, types::InboundAttachment,
};

static REMOTE_REQUEST_ID: AtomicU64 = AtomicU64::new(200_000);
const PROTOCOL_VERSION: &str = "3";
const REMOTE_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

pub(crate) enum OutboundWsMessage {
    Text(Value),
    Pong(axum::body::Bytes),
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
}

#[derive(Debug, Deserialize)]
struct RenameEnvironmentRequest {
    name: Option<String>,
}

struct ServerChunkAssembly {
    segment_count: usize,
    message_size_bytes: usize,
    raw: Vec<u8>,
    next_segment_id: usize,
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
            "/backend-api/wham/remote/control/mfa_requirement",
            get(remote_control_mfa_requirement),
        )
        .route(
            "/backend-api/remote/control/mfa_requirement",
            get(remote_control_mfa_requirement),
        )
        .route(
            "/backend-api/wham/remote/control/clients",
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
        .route("/backend-api/remote/control/server/enroll", post(enroll))
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
    RemoteControlStatusResponse {
        connected: remote.connected,
        initialized: remote.initialized,
        client_id: remote.client_id.clone(),
        stream_id: (!remote.stream_id.is_empty()).then(|| remote.stream_id.clone()),
        server_id: remote.server_id.clone(),
        environment_id: remote.environment_id.clone(),
        server_name: remote.server_name.clone(),
        installation_id: remote.installation_id.clone(),
        account_id: remote.account_id.clone(),
        current_thread_id: remote.current_thread_id.clone(),
        current_turn_id: remote.current_turn_id.clone(),
        last_error: remote.last_error.clone(),
    }
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
    let snapshot = status_snapshot(&state).await;
    let items = if snapshot.connected {
        vec![remote_control_client_item(&snapshot)]
    } else {
        Vec::new()
    };
    Json(json!({
        "items": items,
        "cursor": Value::Null,
    }))
}

async fn delete_remote_control_client(AxumPath(_client_id): AxumPath<String>) -> StatusCode {
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

fn remote_control_environment_item(snapshot: &RemoteControlStatusResponse) -> Value {
    let installation_id = snapshot
        .installation_id
        .clone()
        .unwrap_or_else(|| "local-installation".to_string());
    let env_id = snapshot
        .environment_id
        .clone()
        .unwrap_or_else(|| stable_id("env", &installation_id));
    let host_name = snapshot
        .server_name
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(local_host_name);

    json!({
        "env_id": env_id,
        "display_name": host_name,
        "host_name": host_name,
        "name": host_name,
        "kind": "remote-control",
        "client_type": "CODEX_DESKTOP_APP",
        "online": snapshot.connected,
        "busy": snapshot.current_turn_id.is_some(),
        "os": local_platform_os(),
        "arch": local_arch(),
        "app_server_version": env!("CARGO_PKG_VERSION"),
        "installation_id": installation_id,
        "last_seen_at": Value::Null,
    })
}

fn remote_control_client_item(snapshot: &RemoteControlStatusResponse) -> Value {
    let client_id = if snapshot.client_id.trim().is_empty() {
        "codex-remote-feishu".to_string()
    } else {
        snapshot.client_id.clone()
    };
    let display_name = "Codex Remote Feishu".to_string();

    json!({
        "client_id": client_id,
        "display_name": display_name,
        "device_model": display_name,
        "device_type": "desktop",
        "platform": local_platform_os(),
        "client_type": "CODEX_DESKTOP_APP",
        "enrollment_status": "enrolled",
        "last_seen_at": Value::Null,
    })
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
        }),
    )
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
                    remote.initialized = false;
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
    let (outbound_tx, mut outbound_rx) =
        tokio::sync::mpsc::unbounded_channel::<OutboundWsMessage>();
    let initial_outbound_tx = outbound_tx.clone();
    let (connection_epoch, client_id, stream_id) = {
        let mut remote = state.remote_control.inner.lock().await;
        remote.connected = true;
        remote.initialized = false;
        remote.connection_epoch = remote.connection_epoch.saturating_add(1);
        remote.stream_id = uuid_like();
        remote.next_seq_id = 1;
        remote.outbound_tx = Some(outbound_tx);
        remote.server_id = server_id.clone().or(remote.server_id.clone());
        remote.server_name = server_name.clone().or(remote.server_name.clone());
        remote.installation_id = installation_id.clone().or(remote.installation_id.clone());
        remote.account_id = account_id.clone().or(remote.account_id.clone());
        remote.last_error = None;
        (
            remote.connection_epoch,
            remote.client_id.clone(),
            remote.stream_id.clone(),
        )
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
        "[remote_control] event=ws_open connection_epoch={} client_id={} stream_id={} server_id={} server_name={} installation_id={} account_id={}",
        connection_epoch,
        client_id,
        stream_id,
        server_id.as_deref().unwrap_or_default(),
        server_name.as_deref().unwrap_or_default(),
        installation_id.as_deref().unwrap_or_default(),
        account_id.as_deref().unwrap_or_default()
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
    let initialize_id = next_request_id();
    {
        let mut remote = state.remote_control.inner.lock().await;
        remote
            .client_request_methods
            .insert(initialize_id.to_string(), "initialize".to_string());
    }
    let initialize = build_client_envelope(
        &client_id,
        Some(&stream_id),
        next_client_seq_id(&state).await,
        json!({
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
        }),
    );
    initial_outbound_tx
        .send(OutboundWsMessage::Text(initialize))
        .map_err(|_| anyhow!("remote-control outbound channel closed"))?;

    let mut writer_task = tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            match message {
                OutboundWsMessage::Text(value) => {
                    writer
                        .send(Message::Text(value.to_string().into()))
                        .await
                        .map_err(|err| anyhow!("remote-control websocket writer failed: {err}"))?;
                }
                OutboundWsMessage::Pong(data) => {
                    writer
                        .send(Message::Pong(data))
                        .await
                        .map_err(|err| anyhow!("remote-control websocket writer failed: {err}"))?;
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let reader_state = state.clone();
    let mut reader_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
        let mut chunks = HashMap::<(String, String, u64), ServerChunkAssembly>::new();
        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    let envelope = json!(OutgoingClientEnvelope {
                        event: OutgoingClientEvent::Ping,
                        client_id: client_id.clone(),
                        stream_id: Some(stream_id.clone()),
                        seq_id: None,
                        cursor: None,
                    });
                    send_envelope(&reader_state, envelope).await?;
                }
                incoming = reader.next() => {
                    let Some(incoming) = incoming else {
                        return Ok(());
                    };
                    match incoming.context("failed to read remote-control websocket")? {
                        Message::Text(text) => {
                            handle_server_envelope(&reader_state, connection_epoch, &text, &mut chunks).await?;
                        }
                        Message::Ping(data) => {
                            send_ws_control_pong(&reader_state, data).await?;
                        }
                        Message::Pong(_) => {}
                        Message::Binary(_) => {}
                        Message::Close(_) => return Ok::<(), anyhow::Error>(()),
                    }
                }
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    });

    tokio::select! {
        result = &mut writer_task => result??,
        result = &mut reader_task => result??,
    }

    writer_task.abort();
    reader_task.abort();
    {
        let mut remote = state.remote_control.inner.lock().await;
        if remote.connection_epoch == connection_epoch {
            remote.connected = false;
            remote.initialized = false;
            remote.outbound_tx = None;
            remote.pending.clear();
            remote.client_request_methods.clear();
            remote.client_request_thread_ids.clear();
        }
    }
    state
        .push_event("warn", "remote_control_disconnected", "websocket closed")
        .await;
    Ok(())
}

async fn handle_server_envelope(
    state: &SharedState,
    connection_epoch: u64,
    text: &str,
    chunks: &mut HashMap<(String, String, u64), ServerChunkAssembly>,
) -> Result<()> {
    if !is_active_connection_epoch(state, connection_epoch).await {
        return Ok(());
    }
    chain_log::write_line(format!(
        "[remote_control] event=ws_inbound_raw connection_epoch={} payload_len={} preview={}",
        connection_epoch,
        text.len(),
        json_preview(text)
    ));
    info!(
        target: "codex_remote::remote_control",
        event = "remote_control_ws_inbound_raw",
        connection_epoch,
        payload_len = text.len(),
        preview = %json_preview(text),
        "remote-control websocket inbound frame"
    );
    let envelope: IncomingServerEnvelope =
        serde_json::from_str(text).with_context(|| format!("invalid server envelope: {text}"))?;
    let IncomingServerEnvelope {
        event,
        client_id,
        stream_id,
        seq_id,
    } = envelope;
    ack_server_envelope(
        state,
        &client_id,
        &stream_id,
        seq_id,
        server_event_segment_id(&event),
    )
    .await?;
    if !is_current_remote_stream(state, connection_epoch, &client_id, &stream_id).await {
        observe_stale_server_envelope(
            state,
            connection_epoch,
            seq_id,
            &client_id,
            &stream_id,
            event,
        )
        .await;
        return Ok(());
    }
    match event {
        IncomingServerEvent::ServerMessage { message } => {
            chain_log::write_line(format!(
                "[remote_control] event=server_message connection_epoch={} seq_id={} client_id={} stream_id={} summary={}",
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
                message_summary(&message)
            ));
            info!(
                target: "codex_remote::remote_control",
                event = "remote_control_server_message",
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
                summary = %message_summary(&message),
                "remote-control server message"
            );
            observe_app_server_message(state, connection_epoch, &message).await;
        }
        IncomingServerEvent::ServerMessageChunk {
            segment_id,
            segment_count,
            message_size_bytes,
            message_chunk_base64,
        } => {
            chain_log::write_line(format!(
                "[remote_control] event=server_chunk connection_epoch={} seq_id={} client_id={} stream_id={} segment_id={} segment_count={} message_size_bytes={}",
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
                segment_id,
                segment_count,
                message_size_bytes
            ));
            info!(
                target: "codex_remote::remote_control",
                event = "remote_control_server_chunk",
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
                segment_id,
                segment_count,
                message_size_bytes,
                "remote-control server chunk"
            );
            if let Some(message) = observe_server_chunk(
                chunks,
                &client_id,
                &stream_id,
                seq_id,
                segment_id,
                segment_count,
                message_size_bytes,
                &message_chunk_base64,
            )? {
                observe_app_server_message(state, connection_epoch, &message).await;
            }
        }
        IncomingServerEvent::Ack => {
            chain_log::write_line(format!(
                "[remote_control] event=server_ack connection_epoch={} seq_id={} client_id={} stream_id={}",
                connection_epoch, seq_id, client_id, stream_id
            ));
            info!(
                target: "codex_remote::remote_control",
                event = "remote_control_server_ack",
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
                "remote-control server ack"
            );
            state
                .push_event("info", "remote_control_ack", format!("seq={seq_id}"))
                .await;
        }
        IncomingServerEvent::Pong { status } => {
            chain_log::write_line(format!(
                "[remote_control] event=server_pong connection_epoch={} seq_id={} client_id={} stream_id={} status={}",
                connection_epoch, seq_id, client_id, stream_id, status
            ));
            info!(
                target: "codex_remote::remote_control",
                event = "remote_control_server_pong",
                connection_epoch,
                seq_id,
                client_id,
                stream_id,
                status,
                "remote-control server pong"
            );
            state
                .push_event("info", "remote_control_pong", format!("status={status}"))
                .await;
        }
    }
    Ok(())
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

async fn observe_stale_server_envelope(
    state: &SharedState,
    connection_epoch: u64,
    seq_id: u64,
    client_id: &str,
    stream_id: &str,
    event: IncomingServerEvent,
) {
    if !matches!(event, IncomingServerEvent::Pong { .. }) {
        chain_log::write_line(format!(
            "[remote_control] event=stale_server_envelope connection_epoch={} seq_id={} client_id={} stream_id={} kind={} current_stream_id={}",
            connection_epoch,
            seq_id,
            client_id,
            stream_id,
            server_event_kind(&event),
            current_stream_id(state).await.unwrap_or_default()
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
        && remote.client_id == client_id
        && remote.stream_id == stream_id
}

async fn current_stream_id(state: &SharedState) -> Option<String> {
    let stream_id = state.remote_control.inner.lock().await.stream_id.clone();
    (!stream_id.is_empty()).then_some(stream_id)
}

async fn observe_app_server_message(state: &SharedState, connection_epoch: u64, message: &Value) {
    if !is_active_connection_epoch(state, connection_epoch).await {
        return;
    }
    let message = message.get("message").unwrap_or(message);
    if let Some(id) = message.get("id") {
        if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
            if method == "account/chatgptAuthTokens/refresh" {
                match local_chatgpt_auth_tokens_response(state).await {
                    Ok(result) => {
                        if let Err(err) = send_response(state, id.clone(), result).await {
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
            });
            return;
        }

        let request_key = request_id_key(id);
        let client_method = {
            let mut remote = state.remote_control.inner.lock().await;
            remote.client_request_methods.remove(&request_key)
        };
        let client_thread_id = {
            let mut remote = state.remote_control.inner.lock().await;
            remote.client_request_thread_ids.remove(&request_key)
        };
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
                state.remote_control.inner.lock().await.initialized = true;
                if let Err(err) = send_initialized(state).await {
                    state
                        .push_event(
                            "error",
                            "remote_control_initialized_send_failed",
                            err.to_string(),
                        )
                        .await;
                }
            }
            if let Some(thread_id) = thread_id_from_payload(result) {
                mark_thread_active(state, &thread_id).await;
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
                remote.current_turn_id = Some(turn_id.to_string());
                if let Some(thread_id) = thread_id {
                    remote.current_thread_id = Some(thread_id);
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
        if let Some(tx) = state
            .remote_control
            .inner
            .lock()
            .await
            .pending
            .remove(&request_key)
        {
            let result = if let Some(error) = message.get("error") {
                Err(anyhow!("remote-control request failed: {error}"))
            } else {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = tx.send(result);
        }
        return;
    }

    if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
        if method == "initialized" {
            state.remote_control.inner.lock().await.initialized = true;
            state
                .push_event("info", "remote_control_initialized", "initialized")
                .await;
            return;
        }
        let params = message.get("params").cloned();
        if method == "remoteControl/status/changed" {
            observe_remote_control_status_changed(state, params.as_ref()).await;
        }
        if method == "thread/started" {
            if let Some(thread_id) = params.as_ref().and_then(thread_id_from_payload) {
                mark_thread_active(state, &thread_id).await;
            }
        } else if method == "thread/status/changed" {
            // Status changes are emitted for any loaded thread, including idle/notLoaded
            // transitions. They are not a reliable signal for the foreground thread.
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
            let mut remote = state.remote_control.inner.lock().await;
            if let Some(thread_id) = thread_id {
                remote.current_thread_id = Some(thread_id);
            }
            if let Some(turn_id) = turn_id {
                remote.current_turn_id = Some(turn_id);
            }
        } else if method == "turn/completed" {
            state.remote_control.inner.lock().await.current_turn_id = None;
        } else if method == "thread/closed"
            && let Some(thread_id) = params.as_ref().and_then(thread_id_from_payload)
        {
            let mut remote = state.remote_control.inner.lock().await;
            if remote.current_thread_id.as_deref() == Some(thread_id.as_str()) {
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
        });
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
) -> Result<Option<Value>> {
    if segment_count == 0 || segment_id >= segment_count || message_size_bytes == 0 {
        return Err(anyhow!(
            "invalid remote-control server chunk metadata: segment={segment_id}/{segment_count} size={message_size_bytes}"
        ));
    }
    let key = (client_id.to_string(), stream_id.to_string(), seq_id);
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
        return Err(anyhow!(
            "out-of-order remote-control server chunk: expected={} got={} seq={seq_id}",
            expected_segment_id,
            segment_id
        ));
    }
    let chunk = base64::engine::general_purpose::STANDARD
        .decode(message_chunk_base64)
        .context("invalid remote-control server chunk base64")?;
    assembly.raw.extend_from_slice(&chunk);
    assembly.next_segment_id += 1;
    if assembly.next_segment_id < assembly.segment_count {
        return Ok(None);
    }
    let assembly = chunks
        .remove(&key)
        .ok_or_else(|| anyhow!("missing completed remote-control server chunk assembly"))?;
    if assembly.raw.len() != assembly.message_size_bytes {
        return Err(anyhow!(
            "remote-control server chunk size mismatch: expected={} got={}",
            assembly.message_size_bytes,
            assembly.raw.len()
        ));
    }
    let message = serde_json::from_slice::<Value>(&assembly.raw)
        .context("invalid reassembled remote-control server message")?;
    Ok(Some(message))
}

pub async fn send_response(state: &SharedState, request_id: Value, result: Value) -> Result<()> {
    send_raw_message(state, json!({ "id": request_id, "result": result })).await
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

async fn send_initialized(state: &SharedState) -> Result<()> {
    send_raw_message(
        state,
        json!({
            "method": "initialized",
        }),
    )
    .await
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

pub async fn request(state: &SharedState, method: &str, params: Value) -> Result<Value> {
    let id = next_request_id();
    let request_key = id.to_string();
    let method_name = method.to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut remote = state.remote_control.inner.lock().await;
        if !remote.connected {
            return Err(anyhow!(
                "Codex app-server remote-control 尚未连接。请在项目目录运行 codex，确认它已经连接到 codex-remote 的 /backend-api。"
            ));
        }
        if !remote.initialized {
            return Err(anyhow!(
                "Codex app-server remote-control 已连接，但还没有完成初始化。请稍等几秒后重试；如果一直如此，请在 Codex App 里关闭再打开 remote-control。"
            ));
        }
        remote.pending.insert(request_key.clone(), tx);
        remote
            .client_request_methods
            .insert(request_key.clone(), method.to_string());
        if let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) {
            remote
                .client_request_thread_ids
                .insert(request_key.clone(), thread_id.to_string());
        }
    }
    if let Err(err) = send_raw_message(
        state,
        json!({ "id": id, "method": method, "params": params }),
    )
    .await
    {
        let mut remote = state.remote_control.inner.lock().await;
        remote.pending.remove(&request_key);
        remote.client_request_methods.remove(&request_key);
        remote.client_request_thread_ids.remove(&request_key);
        return Err(err);
    }
    match tokio::time::timeout(REMOTE_REQUEST_TIMEOUT, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(anyhow!("remote-control response channel closed")),
        Err(_) => {
            {
                let mut remote = state.remote_control.inner.lock().await;
                remote.pending.remove(&request_key);
                remote.client_request_methods.remove(&request_key);
                remote.client_request_thread_ids.remove(&request_key);
            }
            state
                .push_event(
                    "warn",
                    "remote_control_request_timeout",
                    format!(
                        "method={} id={} timeout_secs={}",
                        method_name,
                        id,
                        REMOTE_REQUEST_TIMEOUT.as_secs()
                    ),
                )
                .await;
            Err(anyhow!(
                "remote-control request timed out: method={} id={} after {}s",
                method_name,
                id,
                REMOTE_REQUEST_TIMEOUT.as_secs()
            ))
        }
    }
}

pub async fn start_thread(state: &SharedState) -> Result<String> {
    let response = request(state, "thread/start", json!({})).await?;
    response
        .get("thread")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("thread/start response missing thread.id: {response}"))
}

pub async fn thread_list(
    state: &SharedState,
    cursor: Option<&str>,
    limit: Option<u32>,
    cwd: Option<&str>,
) -> Result<Value> {
    let mut params = json!({});
    if let Some(cursor) = cursor {
        params["cursor"] = json!(cursor);
    }
    if let Some(limit) = limit {
        params["limit"] = json!(limit);
    }
    if let Some(cwd) = cwd {
        params["cwd"] = json!(cwd);
    }
    request(state, "thread/list", params).await
}

pub async fn thread_loaded_list(
    state: &SharedState,
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
    request(state, "thread/loaded/list", params).await
}

pub async fn resume_thread(
    state: &SharedState,
    thread_id: &str,
    exclude_turns: bool,
) -> Result<Value> {
    request(
        state,
        "thread/resume",
        json!({
            "threadId": thread_id,
            "excludeTurns": exclude_turns,
        }),
    )
    .await
}

pub async fn start_turn(
    state: &SharedState,
    thread_id: &str,
    text: &str,
    attachments: &[InboundAttachment],
) -> Result<String> {
    let response = request(
        state,
        "turn/start",
        json!({
            "threadId": thread_id,
            "input": turn_input_items(text, attachments),
        }),
    )
    .await?;
    response
        .get("turn")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("turn/start response missing turn.id: {response}"))
}

pub async fn interrupt_turn(state: &SharedState, thread_id: &str, turn_id: &str) -> Result<()> {
    request(
        state,
        "turn/interrupt",
        json!({
            "threadId": thread_id,
            "turnId": turn_id,
        }),
    )
    .await
    .map(|_| ())
}

async fn send_raw_message(state: &SharedState, message: Value) -> Result<()> {
    let (client_id, stream_id, seq_id, outbound_tx) = {
        let mut remote = state.remote_control.inner.lock().await;
        let outbound_tx = remote
            .outbound_tx
            .clone()
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?;
        let seq_id = remote.next_seq_id;
        remote.next_seq_id = remote.next_seq_id.saturating_add(1);
        (
            remote.client_id.clone(),
            remote.stream_id.clone(),
            seq_id,
            outbound_tx,
        )
    };
    chain_log::write_line(format!(
        "[remote_control] event=client_message seq_id={} client_id={} stream_id={} summary={}",
        seq_id,
        client_id,
        stream_id,
        message_summary(&message)
    ));
    info!(
        target: "codex_remote::remote_control",
        event = "remote_control_client_message",
        seq_id,
        client_id,
        stream_id,
        summary = %message_summary(&message),
        "remote-control client message"
    );
    send_client_message(&outbound_tx, &client_id, &stream_id, seq_id, message)?;
    Ok(())
}

async fn ack_server_envelope(
    state: &SharedState,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: Option<usize>,
) -> Result<()> {
    chain_log::write_line(format!(
        "[remote_control] event=client_ack seq_id={} client_id={} stream_id={} segment_id={}",
        seq_id,
        client_id,
        stream_id,
        segment_id.map(|v| v.to_string()).unwrap_or_default()
    ));
    info!(
        target: "codex_remote::remote_control",
        event = "remote_control_client_ack",
        seq_id,
        client_id,
        stream_id,
        segment_id = segment_id.map(|v| v.to_string()).unwrap_or_default(),
        "remote-control client ack"
    );
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
    Ok(())
}

async fn send_envelope(state: &SharedState, envelope: Value) -> Result<()> {
    chain_log::write_line(format!(
        "[remote_control] event=client_envelope summary={}",
        message_summary(&envelope)
    ));
    info!(
        target: "codex_remote::remote_control",
        event = "remote_control_client_envelope",
        summary = %message_summary(&envelope),
        "remote-control client envelope"
    );
    let outbound_tx = {
        let remote = state.remote_control.inner.lock().await;
        remote
            .outbound_tx
            .clone()
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    outbound_tx
        .send(OutboundWsMessage::Text(envelope))
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

async fn next_client_seq_id(state: &SharedState) -> u64 {
    let mut remote = state.remote_control.inner.lock().await;
    let seq_id = remote.next_seq_id;
    remote.next_seq_id = remote.next_seq_id.saturating_add(1);
    seq_id
}

fn send_client_message(
    outbound_tx: &tokio::sync::mpsc::UnboundedSender<OutboundWsMessage>,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    message: Value,
) -> Result<()> {
    const SEGMENT_TARGET_BYTES: usize = 100 * 1024;
    const SEGMENT_COUNT_MAX: usize = 1024;

    let raw = serde_json::to_vec(&message).context("failed to serialize remote-control message")?;
    if raw.len() <= SEGMENT_TARGET_BYTES {
        outbound_tx
            .send(OutboundWsMessage::Text(build_client_envelope(
                client_id,
                Some(stream_id),
                seq_id,
                message,
            )))
            .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
        return Ok(());
    }

    let segment_count = raw.len().div_ceil(SEGMENT_TARGET_BYTES);
    chain_log::write_line(format!(
        "[remote_control] event=client_segmented client_id={} stream_id={} seq_id={} bytes={} segment_count={} summary={}",
        client_id,
        stream_id,
        seq_id,
        raw.len(),
        segment_count,
        message_summary(&message)
    ));
    warn!(
        target: "codex_remote::remote_control",
        event = "remote_control_client_segmented",
        client_id,
        stream_id,
        seq_id,
        bytes = raw.len(),
        segment_count,
        summary = %message_summary(&message),
        "remote-control client message segmented"
    );
    if segment_count > SEGMENT_COUNT_MAX {
        anyhow::bail!(
            "remote-control message is too large to segment: {} bytes",
            raw.len()
        );
    }
    for (segment_id, chunk) in raw.chunks(SEGMENT_TARGET_BYTES).enumerate() {
        let envelope = json!(OutgoingClientEnvelope {
            event: OutgoingClientEvent::ClientMessageChunk {
                segment_id,
                segment_count,
                message_size_bytes: raw.len(),
                message_chunk_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
            },
            client_id: client_id.to_string(),
            stream_id: Some(stream_id.to_string()),
            seq_id: Some(seq_id),
            cursor: None,
        });
        outbound_tx
            .send(OutboundWsMessage::Text(envelope))
            .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    }
    Ok(())
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

fn build_client_envelope(
    client_id: &str,
    stream_id: Option<&str>,
    seq_id: u64,
    message: Value,
) -> Value {
    json!(OutgoingClientEnvelope {
        event: OutgoingClientEvent::ClientMessage { message },
        client_id: client_id.to_string(),
        stream_id: stream_id.map(str::to_string),
        seq_id: Some(seq_id),
        cursor: None,
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

fn uuid_like() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = REMOTE_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    format!("{now:032x}-{counter:016x}")
}

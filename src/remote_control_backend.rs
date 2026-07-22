use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Result, anyhow};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::{
    app_state::{
        RemoteControlInner, RemoteControlRecentEvent, RemoteControlServerConnection, SharedState,
    },
    chain_log,
    codex::CodexNotification,
    types::now_ms,
};

mod auth_tokens;
mod client_state;
mod clients;
mod compatibility;
mod diagnostics;
mod enrollment;
mod log_format;
mod outbound;
mod protocol;
mod recovery;
mod server_envelopes;
mod server_messages;
mod server_work;
mod session_api;
mod status;
mod utils;
mod websocket;

use client_state::*;
#[cfg(test)]
use diagnostics::record_remote_app_pong;
use log_format::*;
pub use outbound::send_response_for_client;
use outbound::{send_envelopes_on_connection, send_initialize_for_client_on_connection};
#[cfg(test)]
use protocol::{ServerChunkObservation, observe_server_chunk};
#[cfg(test)]
use recovery::{
    force_remote_control_ws_reconnect, resubscribe_bound_threads_after_recovery,
    start_remote_control_client_recovery,
};
#[cfg(test)]
use server_envelopes::{ack_cursor_gt, handle_server_envelope, server_ack_cursor_key};
#[cfg(test)]
use server_messages::observe_app_server_message;
#[cfg(test)]
use server_work::{RemoteServerWorkItem, remote_server_work_item_kind};
#[allow(unused_imports)]
pub use session_api::request_for_client;
#[cfg(test)]
use session_api::should_retry_request_after_reinitialize;
use session_api::wait_for_remote_control_initialized;
pub use session_api::{
    ThreadStartOptions, clear_thread_for_client, clear_turn_for_client, config_read_for_client,
    current_thread_for_client, interrupt_turn_for_client, model_list_for_client,
    resume_thread_for_client, session_history_threads, start_thread_for_client,
    start_turn_for_client, thread_list_for_client, thread_loaded_list_for_client,
};
pub use status::{RemoteControlStatusResponse, status_snapshot};
use utils::*;
#[cfg(test)]
use websocket::initialize_remote_clients_for_connection;

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
const REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY: usize = 4096;
const REMOTE_CONTROL_RECENT_EVENT_LIMIT: usize = 96;
const REMOTE_CONTROL_DIAGNOSTIC_WINDOW_MS: u128 = 10_000;
const REMOTE_CONTROL_SOURCE_HINT_TTL_MS: u128 = 30_000;
const FEISHU_BRIDGE_CLIENT_ID: &str = "codexhub-feishu";
const FEISHU_BRIDGE_ENV_ID: &str = "env_codexhub_feishu_bridge";
const FEISHU_BRIDGE_INSTALLATION_ID: &str = "codexhub-feishu-bridge";
const DEFAULT_REMOTE_CLIENT_KEY: &str = "default";

#[allow(dead_code)]
pub fn default_remote_client_key() -> &'static str {
    DEFAULT_REMOTE_CLIENT_KEY
}

#[allow(dead_code)]
pub async fn select_remote_client_key(state: &SharedState) -> Result<String> {
    let mut remote = state.remote_control.inner.lock().await;
    active_connection_epoch_locked(&mut remote)
        .map(|connection_epoch| default_client_key_for_connection_locked(&remote, connection_epoch))
        .ok_or_else(|| anyhow!("remote-control websocket is not connected"))
}

pub(crate) enum OutboundWsMessage {
    Text(Value),
    Ping(axum::body::Bytes),
    Pong(axum::body::Bytes),
    Close(String),
}

pub fn router() -> Router<SharedState> {
    Router::new()
        .route(
            "/codex-app/statsig/v1/initialize",
            post(compatibility::statsig_initialize)
                .options(compatibility::statsig_initialize_options),
        )
        .route(
            "/api/wham/accounts/check",
            get(compatibility::accounts_check),
        )
        .route(
            "/api/wham/statsig/bootstrap",
            post(compatibility::statsig_bootstrap),
        )
        .route(
            "/api/codex-app/statsig/v1/initialize",
            post(compatibility::statsig_initialize)
                .options(compatibility::statsig_initialize_options),
        )
        .route("/api/accounts/check", get(compatibility::accounts_check))
        .route("/api/wham/usage", get(compatibility::usage))
        .route("/api/usage", get(compatibility::usage))
        .route("/api/wham/tasks/list", get(compatibility::tasks_list))
        .route(
            "/api/wham/environments",
            get(compatibility::wham_environments),
        )
        .route("/api/wham/apps", post(compatibility::wham_apps))
        .route(
            "/api/connectors/directory/list",
            get(compatibility::connectors_directory_list),
        )
        .route(
            "/api/connectors/directory/list_workspace",
            get(compatibility::connectors_directory_list),
        )
        .route(
            "/api/codex/analytics-events/events",
            post(compatibility::analytics_events),
        )
        .route("/api/beacons/home", get(compatibility::beacons_home))
        .route("/api/beacons/event", post(compatibility::beacons_event))
        .route(
            "/api/wham/onboarding/context",
            get(compatibility::onboarding_context),
        )
        .route(
            "/api/onboarding/context",
            get(compatibility::onboarding_context),
        )
        .route(
            "/api/accounts/mfa_info",
            get(compatibility::accounts_mfa_info),
        )
        .route(
            "/api/wham/accounts/mfa_info",
            get(compatibility::accounts_mfa_info),
        )
        .route(
            "/api/wham/remote/control/mfa_requirement",
            get(compatibility::remote_control_mfa_requirement),
        )
        .route(
            "/api/remote/control/mfa_requirement",
            get(compatibility::remote_control_mfa_requirement),
        )
        .route(
            "/api/codex/remote/control/mfa_requirement",
            get(compatibility::remote_control_mfa_requirement),
        )
        .route(
            "/api/wham/remote/control/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/api/codex/remote/control/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/api/remote/control/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/api/wham/remote/control/environments/{env_id}/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/api/codex/remote/control/environments/{env_id}/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/api/remote/control/environments/{env_id}/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/api/wham/remote/control/clients/{client_id}",
            axum::routing::delete(clients::delete_remote_control_client),
        )
        .route(
            "/api/codex/remote/control/clients/{client_id}",
            axum::routing::delete(clients::delete_remote_control_client),
        )
        .route(
            "/api/remote/control/clients/{client_id}",
            axum::routing::delete(clients::delete_remote_control_client),
        )
        .route(
            "/api/codex/remote/control/environments",
            get(clients::remote_control_environments),
        )
        .route(
            "/api/remote/control/environments",
            get(clients::remote_control_environments),
        )
        .route(
            "/api/codex/remote/control/environments/{env_id}",
            axum::routing::patch(clients::rename_remote_control_environment)
                .delete(clients::delete_remote_control_environment),
        )
        .route(
            "/api/remote/control/environments/{env_id}",
            axum::routing::patch(clients::rename_remote_control_environment)
                .delete(clients::delete_remote_control_environment),
        )
        .route(
            "/api/codex/remote/control/client/enroll/start",
            post(enrollment::remote_control_client_enroll_start),
        )
        .route(
            "/api/codex/remote/control/client/enroll/finish",
            post(enrollment::remote_control_client_enroll_finish),
        )
        .route(
            "/api/codex/remote/control/client/refresh/start",
            post(enrollment::remote_control_client_refresh_start),
        )
        .route(
            "/api/codex/remote/control/client/refresh/finish",
            post(enrollment::remote_control_client_refresh_finish),
        )
        .route(
            "/api/codex/remote/control/client",
            get(clients::client_websocket),
        )
        .route(
            "/api/wham/remote/control/server/enroll",
            post(enrollment::enroll),
        )
        .route(
            "/api/wham/remote/control/server/refresh",
            post(enrollment::refresh),
        )
        .route(
            "/api/remote/control/server/enroll",
            post(enrollment::enroll),
        )
        .route(
            "/api/remote/control/server/refresh",
            post(enrollment::refresh),
        )
        .route("/api/wham/remote/control/server", get(websocket::websocket))
        .route("/api/remote/control/server", get(websocket::websocket))
        .route(
            "/backend-api/wham/accounts/check",
            get(compatibility::accounts_check),
        )
        .route(
            "/backend-api/wham/statsig/bootstrap",
            post(compatibility::statsig_bootstrap),
        )
        .route(
            "/backend-api/codex-app/statsig/v1/initialize",
            post(compatibility::statsig_initialize)
                .options(compatibility::statsig_initialize_options),
        )
        .route(
            "/backend-api/accounts/check",
            get(compatibility::accounts_check),
        )
        .route("/backend-api/wham/usage", get(compatibility::usage))
        .route("/backend-api/usage", get(compatibility::usage))
        .route(
            "/backend-api/wham/tasks/list",
            get(compatibility::tasks_list),
        )
        .route(
            "/backend-api/wham/environments",
            get(compatibility::wham_environments),
        )
        .route("/backend-api/wham/apps", post(compatibility::wham_apps))
        .route(
            "/backend-api/connectors/directory/list",
            get(compatibility::connectors_directory_list),
        )
        .route(
            "/backend-api/connectors/directory/list_workspace",
            get(compatibility::connectors_directory_list),
        )
        .route(
            "/connectors/directory/list",
            get(compatibility::connectors_directory_list),
        )
        .route(
            "/connectors/directory/list_workspace",
            get(compatibility::connectors_directory_list),
        )
        .route(
            "/backend-api/codex/analytics-events/events",
            post(compatibility::analytics_events),
        )
        .route(
            "/backend-api/beacons/home",
            get(compatibility::beacons_home),
        )
        .route(
            "/backend-api/beacons/event",
            post(compatibility::beacons_event),
        )
        .route(
            "/backend-api/wham/onboarding/context",
            get(compatibility::onboarding_context),
        )
        .route(
            "/backend-api/onboarding/context",
            get(compatibility::onboarding_context),
        )
        .route(
            "/backend-api/accounts/mfa_info",
            get(compatibility::accounts_mfa_info),
        )
        .route(
            "/backend-api/wham/accounts/mfa_info",
            get(compatibility::accounts_mfa_info),
        )
        .route(
            "/backend-api/wham/remote/control/mfa_requirement",
            get(compatibility::remote_control_mfa_requirement),
        )
        .route(
            "/backend-api/remote/control/mfa_requirement",
            get(compatibility::remote_control_mfa_requirement),
        )
        .route(
            "/backend-api/codex/remote/control/mfa_requirement",
            get(compatibility::remote_control_mfa_requirement),
        )
        .route(
            "/backend-api/wham/remote/control/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/backend-api/codex/remote/control/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/backend-api/remote/control/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/backend-api/wham/remote/control/environments/{env_id}/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/backend-api/codex/remote/control/environments/{env_id}/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/backend-api/remote/control/environments/{env_id}/clients",
            get(clients::remote_control_clients),
        )
        .route(
            "/backend-api/wham/remote/control/clients/{client_id}",
            axum::routing::delete(clients::delete_remote_control_client),
        )
        .route(
            "/backend-api/codex/remote/control/clients/{client_id}",
            axum::routing::delete(clients::delete_remote_control_client),
        )
        .route(
            "/backend-api/remote/control/clients/{client_id}",
            axum::routing::delete(clients::delete_remote_control_client),
        )
        .route(
            "/backend-api/codex/remote/control/environments",
            get(clients::remote_control_environments),
        )
        .route(
            "/backend-api/remote/control/environments",
            get(clients::remote_control_environments),
        )
        .route(
            "/backend-api/codex/remote/control/environments/{env_id}",
            axum::routing::patch(clients::rename_remote_control_environment)
                .delete(clients::delete_remote_control_environment),
        )
        .route(
            "/backend-api/remote/control/environments/{env_id}",
            axum::routing::patch(clients::rename_remote_control_environment)
                .delete(clients::delete_remote_control_environment),
        )
        .route(
            "/backend-api/wham/remote/control/server/enroll",
            post(enrollment::enroll),
        )
        .route(
            "/backend-api/wham/remote/control/server/refresh",
            post(enrollment::refresh),
        )
        .route(
            "/backend-api/codex/remote/control/client/enroll/start",
            post(enrollment::remote_control_client_enroll_start),
        )
        .route(
            "/backend-api/codex/remote/control/client/enroll/finish",
            post(enrollment::remote_control_client_enroll_finish),
        )
        .route(
            "/backend-api/codex/remote/control/client/refresh/start",
            post(enrollment::remote_control_client_refresh_start),
        )
        .route(
            "/backend-api/codex/remote/control/client/refresh/finish",
            post(enrollment::remote_control_client_refresh_finish),
        )
        .route(
            "/backend-api/codex/remote/control/client",
            get(clients::client_websocket),
        )
        .route(
            "/backend-api/remote/control/server/enroll",
            post(enrollment::enroll),
        )
        .route(
            "/backend-api/remote/control/server/refresh",
            post(enrollment::refresh),
        )
        .route(
            "/backend-api/wham/remote/control/server",
            get(websocket::websocket),
        )
        .route(
            "/backend-api/remote/control/server",
            get(websocket::websocket),
        )
        .route("/api/remote-control/status", get(status))
}

pub fn subscribe(state: &SharedState) -> tokio::sync::broadcast::Receiver<CodexNotification> {
    state.remote_control.notifications.subscribe()
}

pub async fn status(State(state): State<SharedState>) -> Json<RemoteControlStatusResponse> {
    Json(status_snapshot(&state).await)
}

pub(super) fn remote_control_stale_reason_locked(
    remote: &RemoteControlInner,
    now_ms: u128,
) -> Option<String> {
    remote_control_liveness_stale_reason(
        remote.connected,
        remote.initialized,
        remote.last_initialize_sent_at_ms.or(remote.connected_at_ms),
        remote.last_ws_ping_at_ms,
        remote.last_ws_pong_at_ms,
        remote.connected_at_ms,
        now_ms,
    )
}

pub(super) fn remote_control_connection_stale_reason_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
    now_ms: u128,
) -> Option<String> {
    if remote.connections.is_empty() {
        return (remote.connection_epoch == connection_epoch)
            .then(|| remote_control_stale_reason_locked(remote, now_ms))
            .flatten();
    }
    remote
        .connections
        .values()
        .find(|connection| connection.connection_epoch == connection_epoch)
        .and_then(|connection| remote_control_server_connection_stale_reason(connection, now_ms))
}

fn remote_control_server_connection_stale_reason(
    connection: &RemoteControlServerConnection,
    now_ms: u128,
) -> Option<String> {
    remote_control_liveness_stale_reason(
        connection.connected,
        connection.initialized,
        connection.connected_at_ms,
        connection.last_ws_ping_at_ms,
        connection.last_ws_pong_at_ms,
        connection.connected_at_ms,
        now_ms,
    )
}

fn remote_control_liveness_stale_reason(
    connected: bool,
    initialized: bool,
    initialize_started_at_ms: Option<u128>,
    last_ping_at_ms: Option<u128>,
    last_pong_at_ms: Option<u128>,
    connected_at_ms: Option<u128>,
    now_ms: u128,
) -> Option<String> {
    if !connected {
        return None;
    }
    let timeout_ms = REMOTE_CONTROL_PONG_TIMEOUT.as_millis();
    if !initialized
        && initialize_started_at_ms
            .is_some_and(|started_at_ms| now_ms.saturating_sub(started_at_ms) >= timeout_ms)
    {
        return Some(format!(
            "remote-control initialize timed out after {}s",
            REMOTE_CONTROL_PONG_TIMEOUT.as_secs()
        ));
    }
    if let Some(last_ping_at_ms) = last_ping_at_ms {
        let last_pong_or_connect_at_ms = last_pong_at_ms
            .or(connected_at_ms)
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
    if !connection_exists_locked(&remote, connection_epoch) {
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

fn try_record_remote_recent_event(
    state: &SharedState,
    direction: &'static str,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    seq_id: Option<u64>,
    kind: impl Into<String>,
    summary: impl Into<String>,
) {
    let Ok(mut remote) = state.remote_control.inner.try_lock() else {
        return;
    };
    if !connection_exists_locked(&remote, connection_epoch) {
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

async fn ensure_remote_control_client_initialized(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
) -> Result<()> {
    let requested_client_key = normalize_remote_client_key(client_key);
    let client_key = {
        let remote = state.remote_control.inner.lock().await;
        resolve_remote_client_key_for_connection_locked(
            &remote,
            connection_epoch,
            &requested_client_key,
        )
    };
    let should_initialize = {
        let mut remote = state.remote_control.inner.lock().await;
        if !connection_exists_locked(&remote, connection_epoch) {
            false
        } else {
            let has_connection_map = !remote.connections.is_empty();
            let connection_initialized = remote
                .connections
                .is_empty()
                .then_some(false)
                .unwrap_or_else(|| {
                    remote
                        .connections
                        .values()
                        .find(|connection| connection.connection_epoch == connection_epoch)
                        .is_some_and(|connection| connection.initialized)
                });
            let client = ensure_client_state_locked(&mut remote, &client_key);
            let client_initializing = client.pending.values().any(|pending| {
                pending.method == "initialize" && pending.connection_epoch == connection_epoch
            });
            let should_skip_initialize = client_initializing
                || if !has_connection_map {
                    client.initialized
                } else {
                    client.initialized && connection_initialized
                };
            if should_skip_initialize {
                false
            } else {
                client.last_app_pong_status = None;
                if is_legacy_default_client_key(&client_key) {
                    sync_default_client_legacy_locked(&mut remote);
                }
                true
            }
        }
    };
    if should_initialize {
        send_initialize_for_client_on_connection(state, connection_epoch, &client_key).await?;
    }
    Ok(())
}

async fn ensure_remote_control_client_ready(state: &SharedState, client_key: &str) -> Result<()> {
    let requested_client_key = normalize_remote_client_key(client_key);
    let (connection_epoch, resolved_client_key) = {
        let mut remote = state.remote_control.inner.lock().await;
        if !remote.connected {
            return Err(anyhow!(
                "Codex app-server remote-control 尚未连接。请在项目目录运行 codex，确认它已经连接到 codexhub 的 /backend-api。"
            ));
        }
        let connection_epoch = connection_epoch_for_client_key_locked(
            &mut remote,
            &requested_client_key,
        )
        .ok_or_else(|| {
            anyhow!(
                "remote-control websocket is not connected for client_key={requested_client_key}"
            )
        })?;
        let resolved_client_key = resolve_remote_client_key_for_connection_locked(
            &remote,
            connection_epoch,
            &requested_client_key,
        );
        (connection_epoch, resolved_client_key)
    };
    ensure_remote_control_client_initialized(state, connection_epoch, &resolved_client_key).await?;
    wait_for_remote_control_initialized(state, &resolved_client_key).await
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
        if !connection_exists_locked(&remote, connection_epoch) || !client.initialized {
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
        send_envelopes_on_connection(state, connection_epoch, envelopes).await?;
    }
    Ok(())
}

async fn next_remote_subscribe_cursor(state: &SharedState) -> String {
    let cursor = format!(
        "codexhub:{}",
        REMOTE_SUBSCRIBE_CURSOR_ID.fetch_add(1, Ordering::Relaxed)
    );
    let mut remote = state.remote_control.inner.lock().await;
    remote.subscribe_cursor = Some(cursor.clone());
    cursor
}

fn build_pending_message(method: &str, id: u64, params: Value) -> Value {
    json!({ "id": id, "method": method, "params": params })
}

fn next_request_id() -> u64 {
    REMOTE_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

fn request_id_key(id: &Value) -> String {
    id.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests;

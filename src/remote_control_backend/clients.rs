use axum::{
    Json,
    extract::{
        Path as AxumPath, State,
        ws::{Message, WebSocketUpgrade},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    app_state::{AuthorizedRemoteControlClient, SharedState},
    chain_log,
};

use super::{
    FEISHU_BRIDGE_CLIENT_ID, FEISHU_BRIDGE_ENV_ID, FEISHU_BRIDGE_INSTALLATION_ID,
    RemoteControlStatusResponse, format_rfc3339_utc, status_snapshot, unix_now_u64,
};

const CODEXHUB_REMOTE_DISPLAY_NAME: &str = "codexhub";

#[derive(Debug, Deserialize)]
pub(super) struct RenameEnvironmentRequest {
    name: Option<String>,
}

pub(super) async fn client_websocket(ws: WebSocketUpgrade) -> impl IntoResponse {
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

pub(super) async fn remote_control_clients(State(state): State<SharedState>) -> Json<Value> {
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

pub(super) async fn delete_remote_control_client(
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

pub(super) async fn remote_control_environments(State(state): State<SharedState>) -> Json<Value> {
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

pub(super) async fn rename_remote_control_environment(
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

pub(super) async fn delete_remote_control_environment(
    AxumPath(_env_id): AxumPath<String>,
) -> StatusCode {
    StatusCode::NO_CONTENT
}

fn remote_control_environment_item(snapshot: &RemoteControlStatusResponse) -> Value {
    let host_name = snapshot
        .server_name
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| CODEXHUB_REMOTE_DISPLAY_NAME.to_string());

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
        account_user_id: "user_codexhub_local__acct_codexhub_local".to_string(),
        display_name: CODEXHUB_REMOTE_DISPLAY_NAME.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feishu_bridge_client_uses_codexhub_display_name() {
        let client = feishu_bridge_client_item(true);

        for key in [
            "display_name",
            "displayName",
            "name",
            "title",
            "device_name",
            "deviceName",
        ] {
            assert_eq!(client[key], CODEXHUB_REMOTE_DISPLAY_NAME);
        }
    }

    #[test]
    fn remote_control_environment_fallback_uses_codexhub_display_name() {
        let environment = remote_control_environment_item(&RemoteControlStatusResponse {
            connected: true,
            initialized: true,
            active_connection_id: None,
            active_source_kind: None,
            active_user_agent: None,
            connections: Vec::new(),
            client_id: FEISHU_BRIDGE_CLIENT_ID.to_string(),
            stream_id: Some("stream-test".to_string()),
            server_id: None,
            environment_id: None,
            server_name: None,
            installation_id: None,
            account_id: None,
            current_thread_id: None,
            current_turn_id: None,
            last_error: None,
            healthy: true,
            stale: false,
            connected_at_ms: None,
            last_ws_inbound_at_ms: None,
            last_ws_ping_at_ms: None,
            last_ws_pong_at_ms: None,
            last_app_ping_at_ms: None,
            last_app_pong_at_ms: None,
            last_app_pong_status: None,
            last_initialize_sent_at_ms: None,
        });

        for key in [
            "display_name",
            "displayName",
            "host_name",
            "hostName",
            "name",
            "title",
        ] {
            assert_eq!(environment[key], CODEXHUB_REMOTE_DISPLAY_NAME);
        }
    }
}

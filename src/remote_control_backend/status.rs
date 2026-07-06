use serde::Serialize;

use crate::{
    app_state::{RemoteControlSourceKind, SharedState},
    types::now_ms,
};

use super::{
    DEFAULT_REMOTE_CLIENT_KEY, prune_inactive_remote_connections_locked,
    remote_control_stale_reason_locked, source_default_client_key,
    sync_legacy_from_active_connection_locked,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlStatusResponse {
    pub connected: bool,
    pub initialized: bool,
    pub active_connection_id: Option<String>,
    pub active_source_kind: Option<RemoteControlSourceKind>,
    pub active_user_agent: Option<String>,
    pub connections: Vec<RemoteControlConnectionStatusResponse>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteControlConnectionStatusResponse {
    pub id: String,
    pub connection_epoch: u64,
    pub connected: bool,
    pub initialized: bool,
    pub healthy: bool,
    pub source_kind: RemoteControlSourceKind,
    pub user_agent: Option<String>,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    pub installation_id: Option<String>,
    pub account_id: Option<String>,
    pub connected_at_ms: Option<u128>,
    pub last_ws_inbound_at_ms: Option<u128>,
    pub last_ws_ping_at_ms: Option<u128>,
    pub last_ws_pong_at_ms: Option<u128>,
    pub last_error: Option<String>,
}

pub async fn status_snapshot(state: &SharedState) -> RemoteControlStatusResponse {
    let mut remote = state.remote_control.inner.lock().await;
    prune_inactive_remote_connections_locked(&mut remote);
    sync_legacy_from_active_connection_locked(&mut remote);
    let stale = remote_control_stale_reason_locked(&remote, now_ms()).is_some();
    let active_connection = remote
        .active_connection_id
        .as_ref()
        .and_then(|connection_id| remote.connections.get(connection_id));
    let active_source_kind = active_connection.map(|connection| connection.source_kind);
    let active_user_agent = active_connection.and_then(|connection| connection.user_agent.clone());
    let mut connection_values = remote.connections.values().collect::<Vec<_>>();
    connection_values.sort_by_key(|connection| {
        std::cmp::Reverse((
            connection.connected && connection.outbound_tx.is_some(),
            connection.initialized,
            connection
                .last_ws_inbound_at_ms
                .or(connection.connected_at_ms)
                .unwrap_or_default(),
            connection.connection_epoch,
        ))
    });
    let connections = connection_values
        .into_iter()
        .map(|connection| RemoteControlConnectionStatusResponse {
            id: connection.connection_id.clone(),
            connection_epoch: connection.connection_epoch,
            connected: connection.connected,
            initialized: connection.initialized,
            healthy: connection.connected
                && connection.initialized
                && connection.outbound_tx.is_some(),
            source_kind: connection.source_kind,
            user_agent: connection.user_agent.clone(),
            server_id: connection.server_id.clone(),
            server_name: connection.server_name.clone(),
            installation_id: connection.installation_id.clone(),
            account_id: connection.account_id.clone(),
            connected_at_ms: connection.connected_at_ms,
            last_ws_inbound_at_ms: connection.last_ws_inbound_at_ms,
            last_ws_ping_at_ms: connection.last_ws_ping_at_ms,
            last_ws_pong_at_ms: connection.last_ws_pong_at_ms,
            last_error: connection.last_error.clone(),
        })
        .collect::<Vec<_>>();
    let active_client_key = active_connection
        .map(|connection| source_default_client_key(connection.source_kind))
        .unwrap_or_else(|| DEFAULT_REMOTE_CLIENT_KEY.to_string());
    let default_client = remote.clients.get(&active_client_key);
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
        active_connection_id: remote.active_connection_id.clone(),
        active_source_kind,
        active_user_agent,
        connections,
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

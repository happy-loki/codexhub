use std::collections::HashMap;

use crate::{
    app_state::{RemoteControlClientState, RemoteControlInner, RemoteControlSourceKind},
    chain_log,
};

use super::server_envelopes::server_ack_cursor_key;
use super::{DEFAULT_REMOTE_CLIENT_KEY, OutboundWsMessage, stable_id, uuid_like};

pub(in crate::remote_control_backend) fn ensure_client_state_locked<'a>(
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

pub(in crate::remote_control_backend) fn is_legacy_default_client_key(client_key: &str) -> bool {
    client_key == DEFAULT_REMOTE_CLIENT_KEY || client_key.starts_with("default:")
}

pub(in crate::remote_control_backend) fn source_default_client_key(
    source_kind: RemoteControlSourceKind,
) -> String {
    match source_kind {
        RemoteControlSourceKind::CodexApp => "default:codex_app",
        RemoteControlSourceKind::Vscode => "default:vscode",
        RemoteControlSourceKind::Cli => "default:cli",
        RemoteControlSourceKind::Unknown => "default:unknown",
    }
    .to_string()
}

fn source_kind_from_default_client_key(client_key: &str) -> Option<RemoteControlSourceKind> {
    match client_key {
        "default:codex_app" => Some(RemoteControlSourceKind::CodexApp),
        "default:vscode" => Some(RemoteControlSourceKind::Vscode),
        "default:cli" => Some(RemoteControlSourceKind::Cli),
        "default:unknown" => Some(RemoteControlSourceKind::Unknown),
        _ => None,
    }
}

pub(in crate::remote_control_backend) fn default_client_key_for_connection_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
) -> String {
    remote
        .connections
        .values()
        .find(|connection| connection.connection_epoch == connection_epoch)
        .map(|connection| source_default_client_key(connection.source_kind))
        .unwrap_or_else(|| DEFAULT_REMOTE_CLIENT_KEY.to_string())
}

pub(in crate::remote_control_backend) fn migrate_source_default_client_key_locked(
    remote: &mut RemoteControlInner,
    old_client_key: &str,
    new_source_kind: RemoteControlSourceKind,
    client_id: &str,
    stream_id: &str,
) -> String {
    let old_client_key = normalize_remote_client_key(old_client_key);
    if new_source_kind == RemoteControlSourceKind::Unknown
        || !is_legacy_default_client_key(&old_client_key)
    {
        return old_client_key;
    }

    let new_client_key = source_default_client_key(new_source_kind);
    if old_client_key == new_client_key {
        return old_client_key;
    }

    let old_matches_stream = remote
        .clients
        .get(&old_client_key)
        .is_some_and(|client| client.client_id == client_id && client.stream_id == stream_id);
    if !old_matches_stream {
        return old_client_key;
    }

    let Some(old_client) = remote.clients.remove(&old_client_key) else {
        return old_client_key;
    };
    let replaced_existing = remote
        .clients
        .insert(new_client_key.clone(), old_client)
        .is_some();
    chain_log::write_line(format!(
        "[remote_control] event=source_default_client_key_migrated old_client_key={} new_client_key={} source_kind={:?} client_id={} stream_id={} replaced_existing={}",
        old_client_key, new_client_key, new_source_kind, client_id, stream_id, replaced_existing
    ));
    new_client_key
}

pub(in crate::remote_control_backend) fn active_default_client_key_locked(
    remote: &mut RemoteControlInner,
) -> String {
    if remote.connections.is_empty() {
        return DEFAULT_REMOTE_CLIENT_KEY.to_string();
    }
    select_active_connection_id_locked(remote)
        .as_ref()
        .and_then(|connection_id| remote.connections.get(connection_id))
        .map(|connection| source_default_client_key(connection.source_kind))
        .unwrap_or_else(|| DEFAULT_REMOTE_CLIENT_KEY.to_string())
}

pub(in crate::remote_control_backend) fn connection_epoch_for_client_key_locked(
    remote: &mut RemoteControlInner,
    client_key: &str,
) -> Option<u64> {
    if client_key == DEFAULT_REMOTE_CLIENT_KEY {
        return active_connection_epoch_locked(remote);
    }
    let Some(source_kind) = source_kind_from_default_client_key(client_key) else {
        return active_connection_epoch_locked(remote);
    };
    remote
        .connections
        .values()
        .filter(|connection| {
            connection.connected
                && connection.initialized
                && connection.outbound_tx.is_some()
                && connection.source_kind == source_kind
        })
        .max_by_key(|connection| {
            (
                connection
                    .last_ws_inbound_at_ms
                    .or(connection.connected_at_ms)
                    .unwrap_or_default(),
                connection.connection_epoch,
            )
        })
        .map(|connection| connection.connection_epoch)
}

pub(in crate::remote_control_backend) fn normalize_remote_client_key(client_key: &str) -> String {
    let client_key = client_key.trim();
    if client_key.is_empty() {
        DEFAULT_REMOTE_CLIENT_KEY.to_string()
    } else {
        client_key.to_string()
    }
}

pub(in crate::remote_control_backend) fn remote_client_key_for_stream_locked(
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

pub(in crate::remote_control_backend) fn sync_default_client_legacy_locked(
    remote: &mut RemoteControlInner,
) {
    let active_client_key = active_default_client_key_locked(remote);
    let Some(default_client) = remote.clients.get(&active_client_key) else {
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

pub(in crate::remote_control_backend) fn source_kind_from_user_agent(
    user_agent: &str,
) -> RemoteControlSourceKind {
    let user_agent = user_agent.trim();
    if user_agent.starts_with("Codex Desktop/") {
        RemoteControlSourceKind::CodexApp
    } else if user_agent.starts_with("codex_vscode/") {
        RemoteControlSourceKind::Vscode
    } else if user_agent.starts_with("codexhub/")
        || user_agent.contains("WindowsTerminal")
        || user_agent.contains("Terminal")
    {
        RemoteControlSourceKind::Cli
    } else {
        RemoteControlSourceKind::Unknown
    }
}

fn source_kind_priority(kind: RemoteControlSourceKind) -> u8 {
    match kind {
        RemoteControlSourceKind::CodexApp => 40,
        RemoteControlSourceKind::Vscode => 30,
        RemoteControlSourceKind::Cli => 20,
        RemoteControlSourceKind::Unknown => 10,
    }
}

pub(in crate::remote_control_backend) fn prune_inactive_remote_connections_locked(
    remote: &mut RemoteControlInner,
) {
    let had_connections = !remote.connections.is_empty();
    remote
        .connections
        .retain(|_, connection| connection.connected && connection.outbound_tx.is_some());
    if remote
        .active_connection_id
        .as_ref()
        .is_some_and(|connection_id| !remote.connections.contains_key(connection_id))
    {
        remote.active_connection_id = None;
    }
    if had_connections && remote.connections.is_empty() {
        clear_active_connection_legacy_locked(remote);
    }
}

fn clear_active_connection_legacy_locked(remote: &mut RemoteControlInner) {
    remote.active_connection_id = None;
    remote.connected = false;
    remote.initialized = false;
    remote.outbound_tx = None;
    remote.server_id = None;
    remote.environment_id = None;
    remote.server_name = None;
    remote.installation_id = None;
    remote.account_id = None;
    remote.subscribe_cursor = None;
    remote.connected_at_ms = None;
    remote.last_ws_inbound_at_ms = None;
    remote.last_ws_ping_at_ms = None;
    remote.last_ws_pong_at_ms = None;
}

pub(in crate::remote_control_backend) fn select_active_connection_id_locked(
    remote: &RemoteControlInner,
) -> Option<String> {
    remote
        .connections
        .values()
        .filter(|connection| connection.connected && connection.outbound_tx.is_some())
        .max_by_key(|connection| {
            (
                connection.initialized,
                source_kind_priority(connection.source_kind),
                connection
                    .last_ws_inbound_at_ms
                    .or(connection.connected_at_ms)
                    .unwrap_or_default(),
                connection.connection_epoch,
            )
        })
        .map(|connection| connection.connection_id.clone())
}

pub(in crate::remote_control_backend) fn sync_legacy_from_active_connection_locked(
    remote: &mut RemoteControlInner,
) {
    if remote.connections.is_empty() {
        remote.active_connection_id = None;
        if remote.connected && remote.outbound_tx.is_some() {
            sync_default_client_legacy_locked(remote);
        } else {
            remote.initialized = false;
            remote.outbound_tx = None;
        }
        return;
    }
    remote.active_connection_id = select_active_connection_id_locked(remote);
    let Some(active_connection_id) = remote.active_connection_id.clone() else {
        remote.connected = remote
            .connections
            .values()
            .any(|connection| connection.connected && connection.outbound_tx.is_some());
        remote.initialized = false;
        remote.outbound_tx = None;
        return;
    };

    let Some(connection) = remote.connections.get(&active_connection_id) else {
        return;
    };
    remote.connected = connection.connected;
    remote.initialized = connection
        .clients
        .get(DEFAULT_REMOTE_CLIENT_KEY)
        .is_some_and(|client| client.initialized);
    remote.server_id = connection.server_id.clone();
    remote.environment_id = connection.environment_id.clone();
    remote.server_name = connection.server_name.clone();
    remote.installation_id = connection.installation_id.clone();
    remote.account_id = connection.account_id.clone();
    remote.subscribe_cursor = connection.subscribe_cursor.clone();
    remote.outbound_tx = connection.outbound_tx.clone();
    remote.connection_epoch = connection.connection_epoch;
    remote.connected_at_ms = connection.connected_at_ms;
    remote.last_ws_inbound_at_ms = connection.last_ws_inbound_at_ms;
    remote.last_ws_ping_at_ms = connection.last_ws_ping_at_ms;
    remote.last_ws_pong_at_ms = connection.last_ws_pong_at_ms;
    remote.last_error = connection.last_error.clone();
    if let Some(default_client) = remote.clients.get_mut(DEFAULT_REMOTE_CLIENT_KEY) {
        default_client.initialized = connection.initialized;
    }
    remote.initialized = connection.initialized;
}

pub(in crate::remote_control_backend) fn active_connection_epoch_locked(
    remote: &mut RemoteControlInner,
) -> Option<u64> {
    sync_legacy_from_active_connection_locked(remote);
    remote
        .active_connection_id
        .as_ref()
        .and_then(|connection_id| remote.connections.get(connection_id))
        .map(|connection| connection.connection_epoch)
}

pub(in crate::remote_control_backend) fn outbound_tx_for_connection_epoch_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
) -> Option<tokio::sync::mpsc::UnboundedSender<OutboundWsMessage>> {
    if remote.connections.is_empty()
        && remote.connection_epoch == connection_epoch
        && remote.connected
        && remote.outbound_tx.is_some()
    {
        return remote.outbound_tx.clone();
    }
    remote
        .connections
        .values()
        .find(|connection| {
            connection.connection_epoch == connection_epoch
                && connection.connected
                && connection.outbound_tx.is_some()
        })
        .and_then(|connection| connection.outbound_tx.clone())
}

pub(in crate::remote_control_backend) fn connection_exists_locked(
    remote: &RemoteControlInner,
    connection_epoch: u64,
) -> bool {
    if remote.connections.is_empty() {
        return remote.connection_epoch == connection_epoch && remote.connected;
    }
    remote
        .connections
        .values()
        .any(|connection| connection.connection_epoch == connection_epoch && connection.connected)
}

#[allow(dead_code)]
pub(in crate::remote_control_backend) fn reset_remote_clients_for_connection_locked(
    remote: &mut RemoteControlInner,
) -> Vec<String> {
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

use anyhow::Result;
use serde_json::Value;

use crate::{
    app_state::{RemoteControlStreamDiagnostics, SharedState},
    chain_log,
    types::now_ms,
};

use super::client_state::{
    connection_exists_locked, ensure_client_state_locked, is_legacy_default_client_key,
    normalize_remote_client_key, outbound_tx_for_connection_epoch_locked,
    remote_client_key_for_stream_locked, sync_default_client_legacy_locked,
    sync_legacy_from_active_connection_locked,
};
use super::log_format::{format_recent_event, format_stream_diagnostics, thread_id_from_payload};
use super::protocol::IncomingServerEvent;
use super::recovery::start_remote_control_client_recovery;
use super::server_envelopes::{server_ack_cursor_key, server_event_kind};
use super::{
    REMOTE_CONTROL_DIAGNOSTIC_WINDOW_MS, REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY,
    remote_control_stale_reason_locked,
};

pub(super) async fn observe_stale_server_envelope(
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

pub(super) async fn is_current_remote_stream(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
) -> bool {
    let remote = state.remote_control.inner.lock().await;
    connection_exists_locked(&remote, connection_epoch)
        && remote_client_key_for_stream_locked(&remote, client_id, stream_id).is_some()
}

async fn remote_stream_diagnostic_context(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
) -> (Option<String>, String) {
    let remote = state.remote_control.inner.lock().await;
    if !connection_exists_locked(&remote, connection_epoch) {
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

pub(super) async fn mark_remote_ws_inbound(state: &SharedState, connection_epoch: u64) {
    let mut remote = state.remote_control.inner.lock().await;
    if let Some(connection) = remote
        .connections
        .values_mut()
        .find(|connection| connection.connection_epoch == connection_epoch)
    {
        let now = now_ms();
        connection.last_ws_inbound_at_ms = Some(now);
        remote.last_ws_inbound_at_ms = Some(now);
        sync_legacy_from_active_connection_locked(&mut remote);
    }
}

pub(super) async fn mark_remote_ws_ping(state: &SharedState, connection_epoch: u64) {
    let mut remote = state.remote_control.inner.lock().await;
    if let Some(connection) = remote
        .connections
        .values_mut()
        .find(|connection| connection.connection_epoch == connection_epoch)
    {
        let now = now_ms();
        connection.last_ws_ping_at_ms = Some(now);
        remote.last_ws_ping_at_ms = Some(now);
        sync_legacy_from_active_connection_locked(&mut remote);
    }
}

pub(super) async fn mark_remote_ws_pong(state: &SharedState, connection_epoch: u64) {
    let mut remote = state.remote_control.inner.lock().await;
    if let Some(connection) = remote
        .connections
        .values_mut()
        .find(|connection| connection.connection_epoch == connection_epoch)
    {
        let now = now_ms();
        connection.last_ws_inbound_at_ms = Some(now);
        connection.last_ws_pong_at_ms = Some(now);
        remote.last_ws_inbound_at_ms = Some(now);
        remote.last_ws_pong_at_ms = Some(now);
        sync_legacy_from_active_connection_locked(&mut remote);
    }
}

pub(super) async fn mark_remote_app_ping(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
) {
    let client_key = normalize_remote_client_key(client_key);
    let mut remote = state.remote_control.inner.lock().await;
    if connection_exists_locked(&remote, connection_epoch) {
        let now = now_ms();
        let client = ensure_client_state_locked(&mut remote, &client_key);
        client.last_app_ping_at_ms = Some(now);
        if is_legacy_default_client_key(&client_key) {
            sync_default_client_legacy_locked(&mut remote);
        }
    }
}

pub(super) async fn remote_app_ping_targets(
    state: &SharedState,
    connection_epoch: u64,
) -> Vec<(String, String, String)> {
    let remote = state.remote_control.inner.lock().await;
    if !connection_exists_locked(&remote, connection_epoch)
        || outbound_tx_for_connection_epoch_locked(&remote, connection_epoch).is_none()
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

pub(super) async fn record_remote_app_pong(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    status: &str,
) -> Result<bool> {
    let normalized_status = status.trim().to_ascii_lowercase();
    Ok({
        let mut remote = state.remote_control.inner.lock().await;
        if !connection_exists_locked(&remote, connection_epoch) {
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
        if is_legacy_default_client_key(&client_key) {
            sync_default_client_legacy_locked(&mut remote);
        }
        should_reinitialize
    })
}

pub(in crate::remote_control_backend) async fn handle_remote_app_pong_after_ack(
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

pub(super) async fn observe_command_output_delta_received(
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
        if !connection_exists_locked(&remote, connection_epoch) {
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

pub(super) async fn observe_server_envelope_window(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    received_at_ms: u128,
) {
    let mut remote = state.remote_control.inner.lock().await;
    if !connection_exists_locked(&remote, connection_epoch) {
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

pub(super) async fn record_server_ack_diagnostics(
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
        if !connection_exists_locked(&remote, connection_epoch) {
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
        if !connection_exists_locked(&remote, connection_epoch) {
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

pub(super) async fn remote_control_stale_reason(
    state: &SharedState,
    connection_epoch: u64,
) -> Option<String> {
    let remote = state.remote_control.inner.lock().await;
    if !connection_exists_locked(&remote, connection_epoch) {
        return None;
    }
    remote_control_stale_reason_locked(&remote, now_ms())
}

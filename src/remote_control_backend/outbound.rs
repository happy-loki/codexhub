use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use tracing::info;

use crate::{
    app_state::{PendingRemoteRequest, SharedState},
    chain_log,
    types::now_ms,
};

use super::client_state::{
    active_connection_epoch_locked, active_default_client_key_locked,
    outbound_tx_for_connection_epoch_locked, remote_client_key_for_stream_locked,
};
use super::client_state::{
    ensure_client_state_locked, is_legacy_default_client_key, normalize_remote_client_key,
    sync_default_client_legacy_locked,
};
use super::log_format::{client_envelope_recent_kind, message_summary};
use super::protocol::{build_client_ack_envelope, build_client_message_envelopes};
use super::{
    DEFAULT_REMOTE_CLIENT_KEY, OutboundWsMessage, ensure_remote_control_client_ready,
    next_remote_subscribe_cursor, next_request_id, record_remote_recent_event,
};

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

pub(super) async fn send_response_for_stream(
    state: &SharedState,
    connection_epoch: u64,
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
        if is_legacy_default_client_key(&client_key) {
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
    send_envelopes_on_connection(state, connection_epoch, envelopes).await
}

#[allow(dead_code)]
pub(super) async fn send_initialize_for_client(
    state: &SharedState,
    client_key: &str,
) -> Result<u64> {
    let connection_epoch = {
        let mut remote = state.remote_control.inner.lock().await;
        active_connection_epoch_locked(&mut remote)
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    send_initialize_for_client_on_connection(state, connection_epoch, client_key).await
}

pub(super) async fn send_initialize_for_client_on_connection(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
) -> Result<u64> {
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
        client.pending.insert(
            request_key.clone(),
            PendingRemoteRequest {
                method: "initialize".to_string(),
                thread_id: None,
                track_thread_active: false,
                response_tx: tx,
                message: message.clone(),
                envelopes: envelopes.clone(),
            },
        );
        if is_legacy_default_client_key(&client_key) {
            sync_default_client_legacy_locked(&mut remote);
        }
        (client_id, stream_id, seq_id, envelopes)
    };
    chain_log::write_line(format!(
        "[remote_control] event=initialize_send client_key={} client_id={} stream_id={} seq_id={} request_id={}",
        client_key, client_id, stream_id, seq_id, initialize_id
    ));
    if let Err(err) = send_envelopes_on_connection(state, connection_epoch, envelopes).await {
        let mut remote = state.remote_control.inner.lock().await;
        if let Some(client) = remote.clients.get_mut(&client_key) {
            client.pending.remove(&request_key);
        }
        if is_legacy_default_client_key(&client_key) {
            sync_default_client_legacy_locked(&mut remote);
        }
        return Err(err);
    }
    Ok(initialize_id)
}

pub(super) async fn send_initialized_for_stream(
    state: &SharedState,
    connection_epoch: u64,
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
        if is_legacy_default_client_key(&client_key) {
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
    send_envelopes_on_connection(state, connection_epoch, envelopes).await
}

pub(super) async fn ack_server_envelope(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: Option<usize>,
) -> Result<()> {
    let outbound_tx = {
        let remote = state.remote_control.inner.lock().await;
        outbound_tx_for_connection_epoch_locked(&remote, connection_epoch)
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    outbound_tx
        .send(OutboundWsMessage::Text(build_client_ack_envelope(
            client_id, stream_id, seq_id, segment_id,
        )))
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

#[allow(dead_code)]
pub(super) async fn send_envelope(state: &SharedState, envelope: Value) -> Result<()> {
    let connection_epoch = {
        let mut remote = state.remote_control.inner.lock().await;
        active_connection_epoch_locked(&mut remote)
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    send_envelope_on_connection(state, connection_epoch, envelope).await
}

pub(super) async fn send_envelope_on_connection(
    state: &SharedState,
    connection_epoch: u64,
    envelope: Value,
) -> Result<()> {
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
    let outbound_tx = {
        let remote = state.remote_control.inner.lock().await;
        outbound_tx_for_connection_epoch_locked(&remote, connection_epoch)
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
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
    let connection_epoch = {
        let mut remote = state.remote_control.inner.lock().await;
        active_connection_epoch_locked(&mut remote)
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    send_envelopes_on_connection(state, connection_epoch, envelopes).await
}

pub(super) async fn send_envelopes_on_connection(
    state: &SharedState,
    connection_epoch: u64,
    envelopes: Vec<Value>,
) -> Result<()> {
    let outbound_tx = {
        let remote = state.remote_control.inner.lock().await;
        outbound_tx_for_connection_epoch_locked(&remote, connection_epoch)
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

pub(super) async fn send_ws_control_ping(state: &SharedState, connection_epoch: u64) -> Result<()> {
    chain_log::write_line("[remote_control] event=client_ping payload_len=0".to_string());
    info!(
        target: "codex_remote::remote_control",
        event = "remote_control_client_ping",
        payload_len = 0usize,
        "remote-control client ping"
    );
    let outbound_tx = {
        let remote = state.remote_control.inner.lock().await;
        outbound_tx_for_connection_epoch_locked(&remote, connection_epoch)
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    outbound_tx
        .send(OutboundWsMessage::Ping(axum::body::Bytes::new()))
        .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    Ok(())
}

pub(super) async fn send_ws_control_pong(
    state: &SharedState,
    connection_epoch: u64,
    data: axum::body::Bytes,
) -> Result<()> {
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
        outbound_tx_for_connection_epoch_locked(&remote, connection_epoch)
            .ok_or_else(|| anyhow!("remote-control websocket is not connected"))?
    };
    outbound_tx
        .send(OutboundWsMessage::Pong(data))
        .map_err(|_| anyhow!("remote-control outbound channel closed"))?;
    Ok(())
}

async fn next_client_envelope_parts(
    state: &SharedState,
    client_key: &str,
) -> Result<(String, String, u64)> {
    let requested_client_key = normalize_remote_client_key(client_key);
    let mut remote = state.remote_control.inner.lock().await;
    if !remote.connected {
        return Err(anyhow!("remote-control websocket is not connected"));
    }
    let client_key = if requested_client_key == DEFAULT_REMOTE_CLIENT_KEY {
        active_default_client_key_locked(&mut remote)
    } else {
        requested_client_key
    };
    let client = ensure_client_state_locked(&mut remote, &client_key);
    let seq_id = client.next_seq_id;
    client.next_seq_id = client.next_seq_id.saturating_add(1);
    let parts = (client.client_id.clone(), client.stream_id.clone(), seq_id);
    if is_legacy_default_client_key(&client_key) {
        sync_default_client_legacy_locked(&mut remote);
    }
    Ok(parts)
}

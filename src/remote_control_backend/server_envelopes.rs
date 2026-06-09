use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::{app_state::SharedState, chain_log, types::now_ms};

use super::diagnostics::{
    is_current_remote_stream, observe_command_output_delta_received,
    observe_server_envelope_window, observe_stale_server_envelope, record_remote_app_pong,
    record_server_ack_diagnostics,
};
use super::log_format::{is_command_execution_output_delta_message, message_summary};
use super::outbound::ack_server_envelope;
use super::protocol::{
    IncomingServerEnvelope, IncomingServerEvent, ServerChunkAssembly, ServerChunkObservation,
    observe_server_chunk,
};
use super::record_remote_recent_event;
use super::server_work::{RemoteServerWorkItem, enqueue_remote_server_work};

pub(in crate::remote_control_backend) type ServerChunkMap =
    HashMap<(String, String, u64), ServerChunkAssembly>;

pub(super) async fn handle_server_envelope(
    state: &SharedState,
    connection_epoch: u64,
    text: &str,
    chunks: &mut ServerChunkMap,
    server_work_tx: &tokio::sync::mpsc::Sender<RemoteServerWorkItem>,
) -> Result<()> {
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

pub(in crate::remote_control_backend) fn server_ack_cursor_key(
    client_id: &str,
    stream_id: &str,
) -> String {
    format!("{client_id}\n{stream_id}")
}

pub(super) fn ack_cursor_gt(left: (u64, Option<usize>), right: (u64, Option<usize>)) -> bool {
    let left = (left.0, left.1.unwrap_or(usize::MAX));
    let right = (right.0, right.1.unwrap_or(usize::MAX));
    left > right
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

fn server_event_segment_id(event: &IncomingServerEvent) -> Option<usize> {
    match event {
        IncomingServerEvent::ServerMessageChunk { segment_id, .. } => Some(*segment_id),
        IncomingServerEvent::ServerMessage { .. }
        | IncomingServerEvent::Ack
        | IncomingServerEvent::Pong { .. } => None,
    }
}

pub(in crate::remote_control_backend) fn server_event_kind(
    event: &IncomingServerEvent,
) -> &'static str {
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

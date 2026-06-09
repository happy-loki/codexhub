use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::{app_state::SharedState, chain_log};

use super::{
    diagnostics::handle_remote_app_pong_after_ack, log_format::message_summary,
    server_messages::observe_app_server_message,
};

pub(super) enum RemoteServerWorkItem {
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

pub(super) fn enqueue_remote_server_work(
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

pub(super) fn remote_server_work_item_kind(item: &RemoteServerWorkItem) -> &'static str {
    match item {
        RemoteServerWorkItem::ServerMessage { .. } => "server_message",
        RemoteServerWorkItem::ServerAck { .. } => "ack",
        RemoteServerWorkItem::ServerPong { .. } => "pong",
    }
}

pub(super) async fn run_remote_server_work_queue(
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

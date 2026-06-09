use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::IntoResponse,
};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tracing::info;

use crate::{
    app_state::{RemoteControlServerConnection, RemoteControlSourceKind, SharedState},
    chain_log,
    types::now_ms,
};

use super::client_state::{
    connection_exists_locked, default_client_key_for_connection_locked, ensure_client_state_locked,
    source_default_client_key, source_kind_from_user_agent, sync_default_client_legacy_locked,
    sync_legacy_from_active_connection_locked,
};
use super::diagnostics::{
    mark_remote_app_ping, mark_remote_ws_inbound, mark_remote_ws_ping, mark_remote_ws_pong,
    remote_app_ping_targets, remote_control_stale_reason,
};
use super::outbound::{send_envelope_on_connection, send_ws_control_ping, send_ws_control_pong};
use super::protocol::build_client_ping_envelope;
use super::server_envelopes::{ServerChunkMap, handle_server_envelope};
use super::server_work::{RemoteServerWorkItem, run_remote_server_work_queue};
use super::{
    OutboundWsMessage, PROTOCOL_VERSION, REMOTE_CONTROL_APP_PING_INTERVAL,
    REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY, REMOTE_CONTROL_SOURCE_HINT_TTL_MS,
    REMOTE_CONTROL_STALE_CHECK_INTERVAL, REMOTE_CONTROL_WEBSOCKET_PING_INTERVAL,
    ensure_remote_control_client_initialized, header_str, log_remote_control_entry_headers,
    next_remote_subscribe_cursor, stable_id, uuid_like,
};

pub(super) async fn websocket(
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
                    remote.last_error = Some(message.clone());
                    sync_legacy_from_active_connection_locked(&mut remote);
                }
                state
                    .push_event("error", "remote_control_ws_failed", message)
                    .await;
            }
        })
}

async fn run_websocket(state: SharedState, headers: HeaderMap, socket: WebSocket) -> Result<()> {
    log_remote_control_entry_headers("server_ws_open", &headers);
    let server_id = header_str(&headers, "x-codex-server-id");
    let server_name = header_str(&headers, "x-codex-name")
        .and_then(|value| base64::engine::general_purpose::STANDARD.decode(value).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let installation_id = header_str(&headers, "x-codex-installation-id");
    let account_id = header_str(&headers, "chatgpt-account-id");
    let subscribe_cursor = header_str(&headers, "x-codex-subscribe-cursor");
    let user_agent = header_str(&headers, "user-agent");
    let origin = header_str(&headers, "origin");
    let host = header_str(&headers, "host");
    let (outbound_tx, mut outbound_rx) =
        tokio::sync::mpsc::unbounded_channel::<OutboundWsMessage>();
    let (connection_id, connection_epoch, client_id, stream_id, source_kind, source_user_agent) = {
        let mut remote = state.remote_control.inner.lock().await;
        let connected_at_ms = now_ms();
        remote.connected = true;
        remote.initialized = false;
        remote.next_connection_epoch = remote.next_connection_epoch.saturating_add(1);
        remote.connection_epoch = remote.next_connection_epoch;
        let connection_epoch = remote.connection_epoch;
        let connection_id = stable_id(
            "conn",
            &format!(
                "{}:{}:{}:{}",
                server_id.as_deref().unwrap_or_default(),
                installation_id.as_deref().unwrap_or_default(),
                subscribe_cursor.as_deref().unwrap_or_default(),
                connection_epoch
            ),
        );
        remote.outbound_tx = Some(outbound_tx.clone());
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
        let source_hint = installation_id.as_ref().and_then(|installation_id| {
            remote
                .pending_source_hints_by_installation
                .remove(installation_id)
                .filter(|hint| {
                    connected_at_ms.saturating_sub(hint.captured_at_ms)
                        <= REMOTE_CONTROL_SOURCE_HINT_TTL_MS
                })
        });
        let source_kind = source_hint
            .as_ref()
            .map(|hint| hint.source_kind)
            .or_else(|| user_agent.as_deref().map(source_kind_from_user_agent))
            .unwrap_or(RemoteControlSourceKind::Unknown);
        let source_user_agent = source_hint
            .as_ref()
            .and_then(|hint| hint.user_agent.clone())
            .or_else(|| user_agent.clone());
        if remote.stream_id.is_empty() {
            remote.stream_id = uuid_like();
        }
        let default_client_key = source_default_client_key(source_kind);
        let default_client = ensure_client_state_locked(&mut remote, &default_client_key);
        let client_id = default_client.client_id.clone();
        let stream_id = default_client.stream_id.clone();
        let environment_id = remote.environment_id.clone();
        remote.connections.insert(
            connection_id.clone(),
            RemoteControlServerConnection {
                connection_id: connection_id.clone(),
                connection_epoch,
                connected: true,
                initialized: false,
                source_kind,
                user_agent: source_user_agent.clone(),
                server_id: server_id.clone(),
                environment_id,
                server_name: server_name.clone(),
                installation_id: installation_id.clone(),
                account_id: account_id.clone(),
                subscribe_cursor: subscribe_cursor.clone(),
                outbound_tx: Some(outbound_tx),
                connected_at_ms: Some(connected_at_ms),
                last_ws_inbound_at_ms: Some(connected_at_ms),
                last_ws_ping_at_ms: None,
                last_ws_pong_at_ms: None,
                last_error: None,
                clients: HashMap::new(),
                stream_diagnostics: HashMap::new(),
            },
        );
        sync_default_client_legacy_locked(&mut remote);
        (
            connection_id,
            connection_epoch,
            client_id,
            stream_id,
            source_kind,
            source_user_agent,
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
        "[remote_control] event=ws_open connection_id={} connection_epoch={} client_id={} stream_id={} source_kind={:?} server_id={} server_name={} installation_id={} account_id={} subscribe_cursor={} user_agent={} origin={} host={}",
        connection_id,
        connection_epoch,
        client_id,
        stream_id,
        source_kind,
        server_id.as_deref().unwrap_or_default(),
        server_name.as_deref().unwrap_or_default(),
        installation_id.as_deref().unwrap_or_default(),
        account_id.as_deref().unwrap_or_default(),
        subscribe_cursor.as_deref().unwrap_or_default(),
        source_user_agent.as_deref().unwrap_or_default(),
        origin.as_deref().unwrap_or_default(),
        host.as_deref().unwrap_or_default()
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
        let mut chunks = ServerChunkMap::new();
        loop {
            tokio::select! {
                    _ = ws_ping_interval.tick() => {
                        mark_remote_ws_ping(&reader_state, connection_epoch).await;
                        send_ws_control_ping(&reader_state, connection_epoch).await?;
                    }
                    _ = app_ping_interval.tick() => {
                        let targets = remote_app_ping_targets(&reader_state, connection_epoch).await;
                        for (client_key, client_id, stream_id) in targets {
                            mark_remote_app_ping(&reader_state, connection_epoch, &client_key).await;
            let cursor = next_remote_subscribe_cursor(&reader_state).await;
            let envelope = build_client_ping_envelope(&client_id, &stream_id, Some(&cursor));
                            send_envelope_on_connection(&reader_state, connection_epoch, envelope).await?;
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
                                send_ws_control_pong(&reader_state, connection_epoch, data).await?;
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
        let last_error = connection_result.as_ref().err().map(|err| err.to_string());
        if let Some(connection) = remote.connections.get_mut(&connection_id) {
            connection.connected = false;
            connection.outbound_tx = None;
            connection.last_error = last_error.clone();
        }
        remote.last_error = last_error;
        sync_legacy_from_active_connection_locked(&mut remote);
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

pub(super) async fn initialize_remote_clients_for_connection(
    state: &SharedState,
    connection_epoch: u64,
) -> Result<()> {
    let client_keys = {
        let remote = state.remote_control.inner.lock().await;
        if !connection_exists_locked(&remote, connection_epoch) {
            return Ok(());
        }
        vec![default_client_key_for_connection_locked(
            &remote,
            connection_epoch,
        )]
    };
    for client_key in client_keys {
        ensure_remote_control_client_initialized(state, connection_epoch, &client_key).await?;
    }
    Ok(())
}

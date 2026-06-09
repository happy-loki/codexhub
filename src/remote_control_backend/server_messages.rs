use anyhow::anyhow;
use serde_json::Value;

use crate::{
    app_state::{RemoteControlSourceKind, SharedState},
    chain_log,
    codex::CodexNotification,
};

use super::client_state::{
    is_legacy_default_client_key, migrate_source_default_client_key_locked,
    normalize_remote_client_key, remote_client_key_for_stream_locked,
    select_active_connection_id_locked, source_kind_from_user_agent,
    sync_default_client_legacy_locked, sync_legacy_from_active_connection_locked,
};
use super::log_format::{
    json_preview, log_codex_to_remote_message, message_summary, thread_id_from_payload,
    turn_id_from_payload,
};
use super::outbound::{send_initialized_for_stream, send_response_for_stream};
use super::session_api::{
    is_terminal_or_inactive_thread_status, mark_notification_thread_active_for_client,
    mark_thread_active_for_client, should_track_notification_thread_for_client,
    thread_status_type_from_payload,
};
use super::{
    DEFAULT_REMOTE_CLIENT_KEY, auth_tokens, json_object_keys, replay_pending_requests,
    request_id_key,
};

pub(super) async fn observe_app_server_message(
    state: &SharedState,
    connection_epoch: u64,
    client_id: &str,
    stream_id: &str,
    message: &Value,
) {
    let is_selected_connection = is_selected_active_connection_epoch(state, connection_epoch).await;
    let client_key = {
        let remote = state.remote_control.inner.lock().await;
        remote_client_key_for_stream_locked(&remote, client_id, stream_id)
    };
    let message = message.get("message").unwrap_or(message);
    chain_log::write_line(format!(
        "[remote_control] event=server_message_in connection_epoch={} client_key={} client_id={} stream_id={} summary={}",
        connection_epoch,
        client_key.as_deref().unwrap_or(""),
        client_id,
        stream_id,
        message_summary(message)
    ));
    if let Some(id) = message.get("id") {
        if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
            if method == "account/chatgptAuthTokens/refresh" {
                match auth_tokens::local_chatgpt_auth_tokens_response(state).await {
                    Ok(result) => {
                        if let Err(err) = send_response_for_stream(
                            state,
                            connection_epoch,
                            client_id,
                            stream_id,
                            id.clone(),
                            result,
                        )
                        .await
                        {
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
            if !is_selected_connection {
                chain_log::write_line(format!(
                    "[remote_control] event=non_active_connection_event_ignored connection_epoch={} client_key={} method={} request_id={}",
                    connection_epoch,
                    client_key.as_deref().unwrap_or(""),
                    method,
                    id
                ));
                return;
            }
            let params = message.get("params").cloned();
            if !should_forward_server_notification_to_im(
                state,
                client_key.as_deref(),
                method,
                params.as_ref(),
            )
            .await
            {
                return;
            }
            log_codex_to_remote_message(connection_epoch, message);
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
                remote_client_key: client_key.clone(),
                remote_client_id: Some(client_id.to_string()),
                remote_stream_id: Some(stream_id.to_string()),
            });
            return;
        }

        let request_key = request_id_key(id);
        let pending = {
            let mut remote = state.remote_control.inner.lock().await;
            let pending = client_key
                .as_ref()
                .and_then(|client_key| remote.clients.get_mut(client_key))
                .and_then(|client| client.pending.remove(&request_key));
            if client_key.as_deref() == Some(DEFAULT_REMOTE_CLIENT_KEY) {
                sync_default_client_legacy_locked(&mut remote);
            }
            pending
        };
        let client_method = pending.as_ref().map(|pending| pending.method.clone());
        let client_thread_id = pending
            .as_ref()
            .and_then(|pending| pending.thread_id.clone());
        let track_thread_active = pending
            .as_ref()
            .map(|pending| pending.track_thread_active)
            .unwrap_or(true);
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
                let user_agent = result
                    .get("userAgent")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let result_source_kind = user_agent
                    .as_deref()
                    .map(source_kind_from_user_agent)
                    .unwrap_or(RemoteControlSourceKind::Unknown);
                chain_log::write_line(format!(
                    "[remote_control] event=initialize_result client_key={} client_id={} stream_id={} request_id={} source_kind={:?} result_keys={} preview={}",
                    client_key.as_deref().unwrap_or_default(),
                    client_id,
                    stream_id,
                    id,
                    result_source_kind,
                    json_object_keys(result),
                    json_preview(&result.to_string())
                ));
                let mut initialized_client_key = client_key.clone();
                {
                    let mut remote = state.remote_control.inner.lock().await;
                    let mut connection_source_kind = result_source_kind;
                    if let Some(connection) = remote
                        .connections
                        .values_mut()
                        .find(|connection| connection.connection_epoch == connection_epoch)
                    {
                        connection.initialized = true;
                        if connection.user_agent.is_none() {
                            connection.user_agent = user_agent.clone();
                        }
                        if connection.source_kind == RemoteControlSourceKind::Unknown {
                            connection.source_kind = user_agent
                                .as_deref()
                                .map(source_kind_from_user_agent)
                                .unwrap_or(RemoteControlSourceKind::Unknown);
                        }
                        connection_source_kind = connection.source_kind;
                        connection.last_error = None;
                    }
                    if let Some(client_key) = client_key.as_deref() {
                        let migrated_client_key = migrate_source_default_client_key_locked(
                            &mut remote,
                            client_key,
                            connection_source_kind,
                            client_id,
                            stream_id,
                        );
                        initialized_client_key = Some(migrated_client_key.clone());
                        if let Some(client) = remote.clients.get_mut(&migrated_client_key) {
                            client.initialized = true;
                            client.last_app_pong_status = Some("active".to_string());
                            client.recovery_started_at_ms = None;
                        }
                        if is_legacy_default_client_key(&migrated_client_key) {
                            sync_default_client_legacy_locked(&mut remote);
                        }
                    }
                    remote.last_error = None;
                    sync_legacy_from_active_connection_locked(&mut remote);
                }
                if let Err(err) =
                    send_initialized_for_stream(state, connection_epoch, client_id, stream_id).await
                {
                    state
                        .push_event(
                            "error",
                            "remote_control_initialized_send_failed",
                            err.to_string(),
                        )
                        .await;
                } else if let Some(client_key) = client_key.as_deref()
                    && let Err(err) = replay_pending_requests(
                        state,
                        connection_epoch,
                        initialized_client_key.as_deref().unwrap_or(client_key),
                    )
                    .await
                {
                    state
                        .push_event(
                            "error",
                            "remote_control_pending_replay_failed",
                            err.to_string(),
                        )
                        .await;
                }
            }
            if track_thread_active && let Some(thread_id) = thread_id_from_payload(result) {
                mark_thread_active_for_client(state, client_key.as_deref(), &thread_id).await;
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
                if let Some(client_key) = client_key.as_deref() {
                    if let Some(client) = remote.clients.get_mut(client_key) {
                        client.current_turn_id = Some(turn_id.to_string());
                        if let Some(thread_id) = thread_id.clone() {
                            client.current_thread_id = Some(thread_id);
                        }
                    }
                    if is_legacy_default_client_key(&client_key) {
                        sync_default_client_legacy_locked(&mut remote);
                    }
                } else {
                    remote.current_turn_id = Some(turn_id.to_string());
                    if let Some(thread_id) = thread_id {
                        remote.current_thread_id = Some(thread_id);
                    }
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
        if let Some(pending) = pending {
            let result = if let Some(error) = message.get("error") {
                Err(anyhow!("remote-control request failed: {error}"))
            } else {
                Ok(message.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = pending.response_tx.send(result);
        }
        return;
    }

    if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
        if method == "initialized" {
            {
                let mut remote = state.remote_control.inner.lock().await;
                if let Some(client_key) = client_key.as_deref() {
                    if let Some(client) = remote.clients.get_mut(client_key) {
                        client.initialized = true;
                        client.last_app_pong_status = Some("active".to_string());
                        client.recovery_started_at_ms = None;
                    }
                    if is_legacy_default_client_key(&client_key) {
                        sync_default_client_legacy_locked(&mut remote);
                    }
                }
                remote.last_error = None;
            }
            state
                .push_event("info", "remote_control_initialized", "initialized")
                .await;
            return;
        }
        if !is_selected_connection {
            chain_log::write_line(format!(
                "[remote_control] event=non_active_connection_event_ignored connection_epoch={} client_key={} method={}",
                connection_epoch,
                client_key.as_deref().unwrap_or(""),
                method
            ));
            return;
        }
        let params = message.get("params").cloned();
        if !should_forward_server_notification_to_im(
            state,
            client_key.as_deref(),
            method,
            params.as_ref(),
        )
        .await
        {
            return;
        }
        log_codex_to_remote_message(connection_epoch, message);
        if method == "item/commandExecution/outputDelta" {
            return;
        }
        if method == "remoteControl/status/changed" {
            observe_remote_control_status_changed(state, params.as_ref()).await;
        }
        if method == "thread/started" {
            if let Some(thread_id) = params.as_ref().and_then(thread_id_from_payload) {
                mark_notification_thread_active_for_client(
                    state,
                    client_key.as_deref(),
                    &thread_id,
                )
                .await;
            }
        } else if method == "thread/status/changed" {
            if let Some(params) = params.as_ref()
                && let (Some(thread_id), Some(status_type)) = (
                    thread_id_from_payload(params),
                    thread_status_type_from_payload(params),
                )
            {
                observe_thread_status_changed(
                    state,
                    client_key.as_deref(),
                    &thread_id,
                    &status_type,
                )
                .await;
            }
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
            let should_track = if let Some(thread_id) = thread_id.as_deref() {
                should_track_notification_thread_for_client(state, client_key.as_deref(), thread_id)
                    .await
            } else {
                true
            };
            if should_track {
                let mut remote = state.remote_control.inner.lock().await;
                if let Some(client_key) = client_key.as_deref() {
                    if let Some(client) = remote.clients.get_mut(client_key) {
                        if let Some(thread_id) = thread_id.clone() {
                            client.current_thread_id = Some(thread_id);
                        }
                        if let Some(turn_id) = turn_id.clone() {
                            client.current_turn_id = Some(turn_id);
                        }
                    }
                    if is_legacy_default_client_key(&client_key) {
                        sync_default_client_legacy_locked(&mut remote);
                    }
                } else {
                    if let Some(thread_id) = thread_id {
                        remote.current_thread_id = Some(thread_id);
                    }
                    if let Some(turn_id) = turn_id {
                        remote.current_turn_id = Some(turn_id);
                    }
                }
            }
        } else if method == "turn/completed" {
            let thread_id = params.as_ref().and_then(thread_id_from_payload);
            let turn_id = params.as_ref().and_then(turn_id_from_payload);
            if let Some(thread_id) = thread_id.as_deref() {
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(thread_id, turn_id.as_deref());
            }
            let mut remote = state.remote_control.inner.lock().await;
            if let Some(client_key) = client_key.as_deref() {
                if let Some(client) = remote.clients.get_mut(client_key) {
                    if thread_id.is_none()
                        || client.current_thread_id.as_deref() == thread_id.as_deref()
                    {
                        client.current_turn_id = None;
                    }
                }
                if is_legacy_default_client_key(&client_key) {
                    sync_default_client_legacy_locked(&mut remote);
                }
            } else if thread_id.is_none()
                || remote.current_thread_id.as_deref() == thread_id.as_deref()
            {
                remote.current_turn_id = None;
            }
        } else if method == "thread/closed"
            && let Some(thread_id) = params.as_ref().and_then(thread_id_from_payload)
        {
            let mut remote = state.remote_control.inner.lock().await;
            if let Some(client_key) = client_key.as_deref() {
                if let Some(client) = remote.clients.get_mut(client_key)
                    && client.current_thread_id.as_deref() == Some(thread_id.as_str())
                {
                    client.current_thread_id = None;
                    client.current_turn_id = None;
                }
                if is_legacy_default_client_key(&client_key) {
                    sync_default_client_legacy_locked(&mut remote);
                }
            } else if remote.current_thread_id.as_deref() == Some(thread_id.as_str()) {
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
            remote_client_key: client_key,
            remote_client_id: Some(client_id.to_string()),
            remote_stream_id: Some(stream_id.to_string()),
        });
    }
}

async fn should_forward_server_notification_to_im(
    state: &SharedState,
    client_key: Option<&str>,
    method: &str,
    params: Option<&Value>,
) -> bool {
    let Some(thread_id) = params.and_then(thread_id_from_payload) else {
        return true;
    };
    if should_track_notification_thread_for_client(state, client_key, &thread_id).await {
        return true;
    }
    chain_log::write_line(format!(
        "[remote_control] event=notification_broadcast_skipped reason=non_owner_thread client_key={} method={} thread={}",
        client_key.unwrap_or(""),
        method,
        thread_id
    ));
    false
}

pub(super) async fn observe_thread_status_changed(
    state: &SharedState,
    client_key: Option<&str>,
    thread_id: &str,
    status_type: &str,
) {
    if !is_terminal_or_inactive_thread_status(status_type) {
        return;
    }
    state
        .runtime
        .lock()
        .await
        .mark_turn_completed(thread_id, None);
    let normalized_client_key = client_key.map(normalize_remote_client_key);
    let cleared_turn_id = {
        let mut remote = state.remote_control.inner.lock().await;
        if let Some(client_key) = normalized_client_key.as_deref() {
            let mut cleared_turn_id = None;
            if let Some(client) = remote.clients.get_mut(client_key)
                && client.current_thread_id.as_deref() == Some(thread_id)
            {
                cleared_turn_id = client.current_turn_id.take();
            }
            if is_legacy_default_client_key(&client_key) {
                sync_default_client_legacy_locked(&mut remote);
            }
            cleared_turn_id
        } else if remote.current_thread_id.as_deref() == Some(thread_id) {
            remote.current_turn_id.take()
        } else {
            None
        }
    };
    if let Some(turn_id) = cleared_turn_id {
        state
            .runtime
            .lock()
            .await
            .mark_turn_completed(thread_id, Some(&turn_id));
        chain_log::write_line(format!(
            "[remote_control] event=thread_status_cleared_current_turn client_key={} thread={} turn={} status={}",
            normalized_client_key.as_deref().unwrap_or(""),
            thread_id,
            turn_id,
            status_type
        ));
        state
            .push_event(
                "warn",
                "remote_control_thread_status_cleared_current_turn",
                format!(
                    "client_key={} thread={} turn={} status={}",
                    normalized_client_key.as_deref().unwrap_or(""),
                    thread_id,
                    turn_id,
                    status_type
                ),
            )
            .await;
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

async fn is_selected_active_connection_epoch(state: &SharedState, connection_epoch: u64) -> bool {
    let remote = state.remote_control.inner.lock().await;
    if remote.connections.is_empty() {
        return remote.connection_epoch == connection_epoch && remote.connected;
    }
    let Some(active_connection_id) = select_active_connection_id_locked(&remote) else {
        return false;
    };
    remote
        .connections
        .get(&active_connection_id)
        .is_some_and(|connection| connection.connection_epoch == connection_epoch)
}

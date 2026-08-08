use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::{
    app_state::{PendingRemoteRequest, RemoteControlInner, SharedState},
    chain_log,
    types::{InboundAttachment, now_ms},
};

use super::client_state::{
    connection_epoch_for_client_key_locked, connection_exists_locked, ensure_client_state_locked,
    is_legacy_default_client_key, normalize_remote_client_key,
    resolve_remote_client_key_for_connection_locked, resolve_remote_client_key_locked,
    select_active_connection_id_locked, sync_default_client_legacy_locked,
};
use super::log_format::log_text_preview;
use super::protocol::build_client_message_envelopes;
use super::{
    REMOTE_CONTROL_CLIENT_REINITIALIZED_ERROR, REMOTE_CONTROL_REINITIALIZE_RETRY_TIMEOUT,
    REMOTE_DISCOVERY_REQUEST_TIMEOUT, REMOTE_REQUEST_TIMEOUT, build_pending_message,
    ensure_remote_control_client_initialized, ensure_remote_control_client_ready,
    next_remote_subscribe_cursor, next_request_id, remote_control_connection_stale_reason_locked,
    remote_control_stale_reason_locked, send_envelopes_on_connection,
};

const INTERACTIVE_ROOT_SOURCE_KINDS: [&str; 2] = ["cli", "vscode"];

pub async fn request_for_client(
    state: &SharedState,
    client_key: &str,
    method: &str,
    params: Value,
) -> Result<Value> {
    request_with_timeout_for_client(state, client_key, method, params, REMOTE_REQUEST_TIMEOUT).await
}

async fn request_with_timeout_for_client(
    state: &SharedState,
    client_key: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    let client_key = normalize_remote_client_key(client_key);
    let mut retry_after_reinitialize = true;
    loop {
        match request_once_with_timeout_for_client(
            state,
            &client_key,
            method,
            params.clone(),
            timeout,
        )
        .await
        {
            Ok(value) => return Ok(value),
            Err(err)
                if retry_after_reinitialize
                    && should_retry_request_after_reinitialize(method)
                    && err
                        .to_string()
                        .contains(REMOTE_CONTROL_CLIENT_REINITIALIZED_ERROR) =>
            {
                retry_after_reinitialize = false;
                wait_for_remote_control_initialized(state, &client_key).await?;
                state
                    .push_event(
                        "warn",
                        "remote_control_request_retry_after_reinitialize",
                        format!("client_key={} method={method}", client_key),
                    )
                    .await;
                continue;
            }
            Err(err)
                if err
                    .to_string()
                    .contains(REMOTE_CONTROL_CLIENT_REINITIALIZED_ERROR) =>
            {
                chain_log::write_line(format!(
                    "[remote_control] event=request_not_retried_after_reinitialize method={} err={}",
                    method, err
                ));
                state
                    .push_event(
                        "warn",
                        "remote_control_request_not_retried_after_reinitialize",
                        format!("method={} err={}", method, err),
                    )
                    .await;
                return Err(anyhow!(
                    "remote-control reinitialized while non-idempotent request was in flight; not replaying method={method}"
                ));
            }
            Err(err) => return Err(err),
        }
    }
}

pub(super) fn should_retry_request_after_reinitialize(method: &str) -> bool {
    !matches!(
        method,
        "thread/start" | "thread/fork" | "turn/start" | "turn/steer"
    )
}

async fn request_once_with_timeout_for_client(
    state: &SharedState,
    client_key: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    request_once_with_timeout_for_client_inner(
        state, None, true, true, client_key, method, params, timeout,
    )
    .await
}

pub(super) async fn request_once_with_timeout_for_client_on_connection(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    request_once_with_timeout_for_client_inner(
        state,
        Some(connection_epoch),
        false,
        false,
        client_key,
        method,
        params,
        timeout,
    )
    .await
}

async fn request_once_with_timeout_for_client_inner(
    state: &SharedState,
    target_connection_epoch: Option<u64>,
    wait_for_recovery: bool,
    track_thread_active: bool,
    client_key: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value> {
    let requested_client_key = normalize_remote_client_key(client_key);
    let client_key = {
        let mut remote = state.remote_control.inner.lock().await;
        if let Some(connection_epoch) = target_connection_epoch {
            resolve_remote_client_key_for_connection_locked(
                &remote,
                connection_epoch,
                &requested_client_key,
            )
        } else {
            resolve_remote_client_key_locked(&mut remote, &requested_client_key)
        }
    };
    if wait_for_recovery {
        wait_for_recovery_if_needed(state, &client_key).await?;
    }
    if let Some(connection_epoch) = target_connection_epoch {
        ensure_remote_control_client_initialized(state, connection_epoch, &client_key).await?;
    } else {
        ensure_remote_control_client_ready(state, &client_key).await?;
    }
    let id = next_request_id();
    let request_key = id.to_string();
    let method_name = method.to_string();
    let thread_id = params
        .get("threadId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let message = build_pending_message(method, id, params);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let cursor = next_remote_subscribe_cursor(state).await;
    let (connection_epoch, client_id, stream_id, seq_id, envelope) = {
        let mut remote = state.remote_control.inner.lock().await;
        let connection_epoch = if let Some(connection_epoch) = target_connection_epoch {
            if !connection_exists_locked(&remote, connection_epoch) {
                return Err(anyhow!("remote-control websocket is not connected"));
            }
            connection_epoch
        } else {
            connection_epoch_for_client_key_locked(&mut remote, &client_key).ok_or_else(|| {
                anyhow!("remote-control websocket is not connected for client_key={client_key}")
            })?
        };
        let stale_reason = if let Some(connection_epoch) = target_connection_epoch {
            if !remote.connections.is_empty()
                && !remote.connections.values().any(|connection| {
                    connection.connection_epoch == connection_epoch
                        && connection.connected
                        && connection.initialized
                        && connection.outbound_tx.is_some()
                })
            {
                return Err(anyhow!(
                    "remote-control connection is not ready: connection_epoch={connection_epoch}"
                ));
            }
            remote_control_connection_stale_reason_locked(&remote, connection_epoch, now_ms())
        } else {
            if !remote.connected {
                return Err(anyhow!(
                    "Codex app-server remote-control 尚未连接。请在项目目录运行 codex，确认它已经连接到 codexhub 的 /backend-api。"
                ));
            }
            remote_control_stale_reason_locked(&remote, now_ms())
        };
        if let Some(reason) = stale_reason {
            remote.last_error = Some(reason.clone());
            return Err(anyhow!(
                "Codex app-server remote-control 连接已失活：{reason}。请稍等自动重连后重试。"
            ));
        }
        let client = ensure_client_state_locked(&mut remote, &client_key);
        if !client.initialized {
            return Err(anyhow!(
                "Codex app-server remote-control 已连接，但还没有完成初始化。请稍等几秒后重试；如果一直如此，请在 Codex App 里关闭再打开 remote-control。"
            ));
        }
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
                connection_epoch,
                method: method.to_string(),
                thread_id: thread_id.clone(),
                track_thread_active,
                response_tx: tx,
                message: message.clone(),
                envelopes: envelopes.clone(),
            },
        );
        if is_legacy_default_client_key(&client_key) {
            sync_default_client_legacy_locked(&mut remote);
        }
        (connection_epoch, client_id, stream_id, seq_id, envelopes)
    };
    chain_log::write_line(format!(
        "[remote_control] event=request_send client_key={} client_id={} stream_id={} seq_id={} request_id={} method={} thread={}",
        client_key,
        client_id,
        stream_id,
        seq_id,
        id,
        method_name,
        thread_id.as_deref().unwrap_or("")
    ));
    if let Err(err) = send_envelopes_on_connection(state, connection_epoch, envelope).await {
        let mut remote = state.remote_control.inner.lock().await;
        if let Some(client) = remote.clients.get_mut(&client_key) {
            client.pending.remove(&request_key);
        }
        if is_legacy_default_client_key(&client_key) {
            sync_default_client_legacy_locked(&mut remote);
        }
        return Err(err);
    }
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(anyhow!("remote-control response channel closed")),
        Err(_) => {
            {
                let mut remote = state.remote_control.inner.lock().await;
                if let Some(client) = remote.clients.get_mut(&client_key) {
                    client.pending.remove(&request_key);
                }
                if is_legacy_default_client_key(&client_key) {
                    sync_default_client_legacy_locked(&mut remote);
                }
            }
            let health = remote_control_request_timeout_health(state, &client_key).await;
            state
                .push_event(
                    "warn",
                    "remote_control_request_timeout",
                    format!(
                        "client_key={} method={} id={} timeout_secs={} {}",
                        client_key,
                        method_name,
                        id,
                        timeout.as_secs(),
                        health
                    ),
                )
                .await;
            Err(anyhow!(
                "remote-control request timed out: client_key={} method={} id={} after {}s",
                client_key,
                method_name,
                id,
                timeout.as_secs()
            ))
        }
    }
}

/// Snapshot of the bound client's liveness at the moment a request timed out.
///
/// This is diagnostics-only: it does not change control flow. It lets an uploaded
/// log distinguish "app-server went silent" from "recovery was mid-flight" without
/// exposing any conversation content.
async fn remote_control_request_timeout_health(state: &SharedState, client_key: &str) -> String {
    let client_key = normalize_remote_client_key(client_key);
    let remote = state.remote_control.inner.lock().await;
    let now = now_ms();
    let elapsed = |value: Option<u128>| {
        value
            .map(|at_ms| now.saturating_sub(at_ms).to_string())
            .unwrap_or_else(|| "none".to_string())
    };
    let client = remote.clients.get(&client_key);
    format!(
        "ws_connected={} client_initialized={} client_recovering={} last_app_pong_status={} app_ping_age_ms={} app_pong_age_ms={} pending={} ws_ping_age_ms={} ws_pong_age_ms={}",
        remote.connected,
        client.map(|client| client.initialized).unwrap_or(false),
        client
            .map(|client| client.recovery_started_at_ms.is_some())
            .unwrap_or(false),
        client
            .and_then(|client| client.last_app_pong_status.clone())
            .unwrap_or_else(|| "none".to_string()),
        elapsed(client.and_then(|client| client.last_app_ping_at_ms)),
        elapsed(client.and_then(|client| client.last_app_pong_at_ms)),
        client.map(|client| client.pending.len()).unwrap_or(0),
        elapsed(remote.last_ws_ping_at_ms),
        elapsed(remote.last_ws_pong_at_ms),
    )
}

pub(super) async fn wait_for_remote_control_initialized(
    state: &SharedState,
    client_key: &str,
) -> Result<()> {
    let client_key = normalize_remote_client_key(client_key);
    let start = tokio::time::Instant::now();
    loop {
        {
            let remote = state.remote_control.inner.lock().await;
            if remote
                .clients
                .get(&client_key)
                .is_some_and(|client| remote.connected && client.initialized)
            {
                return Ok(());
            }
            if !remote.connected {
                return Err(anyhow!(
                    "Codex app-server remote-control disconnected during reinitialize"
                ));
            }
        }
        if start.elapsed() >= REMOTE_CONTROL_REINITIALIZE_RETRY_TIMEOUT {
            return Err(anyhow!(
                "remote-control client initialize did not complete within {}s: client_key={}",
                REMOTE_CONTROL_REINITIALIZE_RETRY_TIMEOUT.as_secs(),
                client_key
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_recovery_if_needed(state: &SharedState, client_key: &str) -> Result<()> {
    let client_key = normalize_remote_client_key(client_key);
    let recovering = {
        let remote = state.remote_control.inner.lock().await;
        remote
            .clients
            .get(&client_key)
            .is_some_and(|client| client.recovery_started_at_ms.is_some())
    };
    if recovering {
        wait_for_remote_control_initialized(state, &client_key).await?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct ThreadStartOptions {
    pub cwd: Option<String>,
    pub model_provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permissions: Option<String>,
    pub approval_policy: Option<String>,
    pub approvals_reviewer: Option<String>,
}

impl ThreadStartOptions {
    fn to_params(&self) -> Value {
        let mut params = serde_json::Map::new();
        if let Some(cwd) = non_empty(self.cwd.as_deref()) {
            params.insert("cwd".to_string(), json!(cwd));
            params.insert("runtimeWorkspaceRoots".to_string(), json!([cwd]));
        }
        if let Some(model_provider) = non_empty(self.model_provider.as_deref()) {
            params.insert("modelProvider".to_string(), json!(model_provider));
        }
        if let Some(model) = non_empty(self.model.as_deref()) {
            params.insert("model".to_string(), json!(model));
        }
        if let Some(effort) = non_empty(self.reasoning_effort.as_deref()) {
            params.insert(
                "config".to_string(),
                json!({
                    "model_reasoning_effort": effort,
                }),
            );
        }
        if let Some(permissions) = non_empty(self.permissions.as_deref()) {
            params.insert("permissions".to_string(), json!(permissions));
        }
        if let Some(approval_policy) = non_empty(self.approval_policy.as_deref()) {
            params.insert("approvalPolicy".to_string(), json!(approval_policy));
        }
        if let Some(approvals_reviewer) = non_empty(self.approvals_reviewer.as_deref()) {
            params.insert("approvalsReviewer".to_string(), json!(approvals_reviewer));
        }
        Value::Object(params)
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

pub async fn start_thread_for_client(
    state: &SharedState,
    client_key: &str,
    options: ThreadStartOptions,
) -> Result<String> {
    let response =
        request_for_client(state, client_key, "thread/start", options.to_params()).await?;
    let thread_id = response
        .get("thread")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("thread/start response missing thread.id: {response}"))?;
    mark_thread_active_for_client(state, Some(client_key), &thread_id).await;
    Ok(thread_id)
}

pub async fn config_read_for_client(
    state: &SharedState,
    client_key: &str,
    cwd: Option<&str>,
    include_layers: bool,
) -> Result<Value> {
    let mut params = json!({});
    if let Some(cwd) = non_empty(cwd) {
        params["cwd"] = json!(cwd);
    }
    if include_layers {
        params["includeLayers"] = json!(true);
    }
    request_for_client(state, client_key, "config/read", params).await
}

pub async fn model_list_for_client(
    state: &SharedState,
    client_key: &str,
    include_hidden: bool,
    limit: Option<u32>,
) -> Result<Value> {
    let mut params = json!({
        "includeHidden": include_hidden,
    });
    if let Some(limit) = limit {
        params["limit"] = json!(limit);
    }
    request_for_client(state, client_key, "model/list", params).await
}

pub async fn thread_list_for_client(
    state: &SharedState,
    client_key: &str,
    cursor: Option<&str>,
    limit: Option<u32>,
    cwd: Option<&str>,
    model_provider: Option<&str>,
) -> Result<Value> {
    let mut params = json!({});
    if let Some(cursor) = cursor {
        params["cursor"] = json!(cursor);
    }
    if let Some(limit) = limit {
        params["limit"] = json!(limit);
    }
    params["sortKey"] = json!("updated_at");
    params["sourceKinds"] = json!(INTERACTIVE_ROOT_SOURCE_KINDS);
    params["archived"] = json!(false);
    params["useStateDbOnly"] = json!(true);
    if let Some(cwd) = cwd {
        params["cwd"] = json!(cwd);
    }
    if let Some(model_provider) = non_empty(model_provider) {
        params["modelProviders"] = json!([model_provider]);
    }
    request_thread_list_with_params(state, client_key, params).await
}

fn session_history_thread_list_params(
    cursor: Option<&str>,
    limit: Option<u32>,
    archived: bool,
) -> Value {
    let mut params = json!({
        "sortKey": "updated_at",
        "sourceKinds": INTERACTIVE_ROOT_SOURCE_KINDS,
        "archived": archived,
        "modelProviders": [],
        "useStateDbOnly": true,
    });
    if let Some(cursor) = cursor {
        params["cursor"] = json!(cursor);
    }
    if let Some(limit) = limit {
        params["limit"] = json!(limit);
    }
    params
}

fn discovery_connection_candidates_locked(remote: &mut RemoteControlInner) -> Vec<u64> {
    let active_connection_id = select_active_connection_id_locked(remote);
    if remote.connections.is_empty() {
        return (remote.connected && remote.initialized && remote.outbound_tx.is_some())
            .then_some(remote.connection_epoch)
            .into_iter()
            .collect();
    }

    let now = now_ms();
    let mut candidates = remote
        .connections
        .values()
        .filter(|connection| {
            connection.connected
                && connection.initialized
                && connection.outbound_tx.is_some()
                && remote_control_connection_stale_reason_locked(
                    remote,
                    connection.connection_epoch,
                    now,
                )
                .is_none()
        })
        .map(|connection| {
            (
                connection.connection_id == active_connection_id.as_deref().unwrap_or_default(),
                connection
                    .last_ws_inbound_at_ms
                    .or(connection.connected_at_ms)
                    .unwrap_or_default(),
                connection.connection_epoch,
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(*candidate));
    candidates
        .into_iter()
        .map(|(_, _, connection_epoch)| connection_epoch)
        .collect()
}

async fn session_history_threads_on_connection(
    state: &SharedState,
    connection_epoch: u64,
    client_key: &str,
    page_limit: u32,
    max_pages: usize,
    archived: bool,
) -> Result<Vec<Value>> {
    let mut cursor: Option<String> = None;
    let mut threads = Vec::new();
    for _ in 0..max_pages {
        let response = request_once_with_timeout_for_client_on_connection(
            state,
            connection_epoch,
            client_key,
            "thread/list",
            session_history_thread_list_params(cursor.as_deref(), Some(page_limit), archived),
            REMOTE_DISCOVERY_REQUEST_TIMEOUT,
        )
        .await?;
        let items = response
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("thread/list response missing data array"))?;
        threads.extend(items.iter().cloned());
        cursor = response
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    Ok(threads)
}

pub async fn session_history_threads(
    state: &SharedState,
    client_key: &str,
    page_limit: u32,
    max_pages: usize,
    archived: bool,
) -> Result<Vec<Value>> {
    let candidates = {
        let mut remote = state.remote_control.inner.lock().await;
        discovery_connection_candidates_locked(&mut remote)
    };
    if candidates.is_empty() {
        return Err(anyhow!(
            "remote-control has no initialized healthy connection for session history"
        ));
    }

    let mut failures = Vec::new();
    for (attempt, connection_epoch) in candidates.iter().copied().enumerate() {
        match session_history_threads_on_connection(
            state,
            connection_epoch,
            client_key,
            page_limit,
            max_pages,
            archived,
        )
        .await
        {
            Ok(threads) => return Ok(threads),
            Err(err) => {
                let message = format!(
                    "attempt={} connection_epoch={} err={}",
                    attempt + 1,
                    connection_epoch,
                    err
                );
                chain_log::write_line(format!(
                    "[remote_control] event=session_history_connection_fallback {message}"
                ));
                state
                    .push_event(
                        "warn",
                        "session_history_connection_fallback",
                        message.clone(),
                    )
                    .await;
                failures.push(message);
            }
        }
    }

    Err(anyhow!(
        "session history failed on all healthy connections: {}",
        failures.join("; ")
    ))
}

async fn request_thread_list_with_params(
    state: &SharedState,
    client_key: &str,
    params: Value,
) -> Result<Value> {
    request_with_timeout_for_client(
        state,
        client_key,
        "thread/list",
        params,
        REMOTE_DISCOVERY_REQUEST_TIMEOUT,
    )
    .await
}

pub async fn thread_loaded_list_for_client(
    state: &SharedState,
    client_key: &str,
    cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<Value> {
    let mut params = json!({});
    if let Some(cursor) = cursor {
        params["cursor"] = json!(cursor);
    }
    if let Some(limit) = limit {
        params["limit"] = json!(limit);
    }
    request_with_timeout_for_client(
        state,
        client_key,
        "thread/loaded/list",
        params,
        REMOTE_DISCOVERY_REQUEST_TIMEOUT,
    )
    .await
}

pub async fn resume_thread_for_client(
    state: &SharedState,
    client_key: &str,
    thread_id: &str,
    exclude_turns: bool,
) -> Result<Value> {
    let response = request_for_client(
        state,
        client_key,
        "thread/resume",
        json!({
            "threadId": thread_id,
            "excludeTurns": exclude_turns,
        }),
    )
    .await?;
    mark_thread_active_for_client(state, Some(client_key), thread_id).await;
    Ok(response)
}

pub async fn start_turn_for_client(
    state: &SharedState,
    client_key: &str,
    thread_id: &str,
    text: &str,
    attachments: &[InboundAttachment],
) -> Result<String> {
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[im_trace] event=remote_to_codex_turn_start client_key={} thread={} text_len={} attachments={} preview={}",
            client_key,
            thread_id,
            text.chars().count(),
            attachments.len(),
            log_text_preview(text, 360)
        )
    });
    let response = request_for_client(
        state,
        client_key,
        "turn/start",
        json!({
            "threadId": thread_id,
            "input": turn_input_items(text, attachments),
        }),
    )
    .await?;
    let turn_id = response
        .get("turn")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("turn/start response missing turn.id: {response}"))?;
    {
        let mut remote = state.remote_control.inner.lock().await;
        let client_key = normalize_remote_client_key(client_key);
        let client = ensure_client_state_locked(&mut remote, &client_key);
        client.current_thread_id = Some(thread_id.to_string());
        client.current_turn_id = Some(turn_id.clone());
        if is_legacy_default_client_key(&client_key) {
            sync_default_client_legacy_locked(&mut remote);
        }
    }
    Ok(turn_id)
}

pub async fn interrupt_turn_for_client(
    state: &SharedState,
    client_key: &str,
    thread_id: &str,
    turn_id: &str,
) -> Result<()> {
    request_for_client(
        state,
        client_key,
        "turn/interrupt",
        json!({
            "threadId": thread_id,
            "turnId": turn_id,
        }),
    )
    .await
    .map(|_| ())
}

pub async fn current_thread_for_client(state: &SharedState, client_key: &str) -> Option<String> {
    let requested_client_key = normalize_remote_client_key(client_key);
    let mut remote = state.remote_control.inner.lock().await;
    let client_key = resolve_remote_client_key_locked(&mut remote, &requested_client_key);
    remote
        .clients
        .get(&client_key)
        .and_then(|client| client.current_thread_id.clone())
}

pub async fn clear_turn_for_client(state: &SharedState, client_key: &str, turn_id: Option<&str>) {
    let requested_client_key = normalize_remote_client_key(client_key);
    let mut remote = state.remote_control.inner.lock().await;
    let client_key = resolve_remote_client_key_locked(&mut remote, &requested_client_key);
    if let Some(client) = remote.clients.get_mut(&client_key) {
        if turn_id.is_none() || client.current_turn_id.as_deref() == turn_id {
            client.current_turn_id = None;
        }
    }
    if is_legacy_default_client_key(&client_key) {
        sync_default_client_legacy_locked(&mut remote);
    }
}

pub async fn clear_thread_for_client(
    state: &SharedState,
    client_key: &str,
    thread_id: Option<&str>,
) {
    let requested_client_key = normalize_remote_client_key(client_key);
    let mut remote = state.remote_control.inner.lock().await;
    let client_key = resolve_remote_client_key_locked(&mut remote, &requested_client_key);
    if let Some(client) = remote.clients.get_mut(&client_key) {
        if thread_id.is_none() || client.current_thread_id.as_deref() == thread_id {
            client.current_thread_id = None;
            client.current_turn_id = None;
        }
    }
    if is_legacy_default_client_key(&client_key) {
        sync_default_client_legacy_locked(&mut remote);
    }
}

pub(super) fn thread_status_type_from_payload(value: &Value) -> Option<String> {
    value
        .get("status")
        .and_then(|status| {
            status
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| status.as_str())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn is_terminal_or_inactive_thread_status(status_type: &str) -> bool {
    matches!(status_type, "idle" | "notLoaded" | "systemError")
}

async fn mark_thread_active(state: &SharedState, thread_id: &str) {
    let mut remote = state.remote_control.inner.lock().await;
    if remote.current_thread_id.as_deref() == Some(thread_id) {
        return;
    }
    remote.current_thread_id = Some(thread_id.to_string());
    remote.current_turn_id = None;
    drop(remote);
    state
        .push_event("info", "remote_control_thread_active", thread_id)
        .await;
}

pub(super) async fn mark_thread_active_for_client(
    state: &SharedState,
    client_key: Option<&str>,
    thread_id: &str,
) {
    if let Some(client_key) = client_key {
        let client_key = normalize_remote_client_key(client_key);
        let mut remote = state.remote_control.inner.lock().await;
        let client = ensure_client_state_locked(&mut remote, &client_key);
        if client.current_thread_id.as_deref() == Some(thread_id) {
            return;
        }
        client.current_thread_id = Some(thread_id.to_string());
        client.current_turn_id = None;
        if is_legacy_default_client_key(&client_key) {
            sync_default_client_legacy_locked(&mut remote);
        }
        drop(remote);
        state
            .push_event(
                "info",
                "remote_control_thread_active",
                format!("client_key={client_key} thread={thread_id}"),
            )
            .await;
        return;
    }
    mark_thread_active(state, thread_id).await;
}

pub(super) async fn mark_notification_thread_active_for_client(
    state: &SharedState,
    client_key: Option<&str>,
    thread_id: &str,
) {
    if should_track_notification_thread_for_client(state, client_key, thread_id).await {
        mark_thread_active_for_client(state, client_key, thread_id).await;
    } else {
        chain_log::write_line(format!(
            "[remote_control] level=warn event=notification_thread_active_skipped reason=unbound_thread client_key={} thread={}",
            client_key.unwrap_or(""),
            thread_id
        ));
    }
}

pub(super) async fn should_track_notification_thread_for_client(
    state: &SharedState,
    client_key: Option<&str>,
    thread_id: &str,
) -> bool {
    let Some(client_key) = client_key.map(normalize_remote_client_key) else {
        let runtime = state.runtime.lock().await;
        return runtime.route_for_thread(thread_id).is_some();
    };
    let is_bound_thread = {
        let runtime = state.runtime.lock().await;
        let mut is_bound_thread = false;
        for (bound_thread_id, route) in &runtime.route_by_thread {
            if !route_remote_client_key_matches(&route.remote_client_key, &client_key) {
                continue;
            }
            if bound_thread_id == thread_id {
                is_bound_thread = true;
            }
        }
        is_bound_thread
    };
    if is_bound_thread {
        return true;
    }
    let is_current_or_pending_request_thread = {
        let remote = state.remote_control.inner.lock().await;
        remote
            .clients
            .get(&client_key)
            .map(|client| {
                client.current_thread_id.as_deref() == Some(thread_id)
                    || client.pending.values().any(|pending| {
                        pending
                            .thread_id
                            .as_deref()
                            .is_some_and(|pending_thread_id| pending_thread_id == thread_id)
                    })
            })
            .unwrap_or(false)
    };
    is_current_or_pending_request_thread
}

pub(super) fn route_remote_client_key_matches(route_client_key: &str, client_key: &str) -> bool {
    let client_key = normalize_remote_client_key(client_key);
    let route_client_key = normalize_remote_client_key(route_client_key);
    route_client_key == client_key
}

fn turn_input_items(text: &str, attachments: &[InboundAttachment]) -> Vec<Value> {
    let mut items = Vec::new();
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        items.push(json!({
            "type": "text",
            "text": trimmed,
            "text_elements": [],
        }));
    }
    for attachment in attachments {
        let Some(local_path) = attachment
            .local_path
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        match attachment.kind.as_str() {
            "image" => items.push(json!({
                "type": "localImage",
                "path": local_path,
            })),
            "file" | "text" | "video" | "audio" => items.push(json!({
                "type": "text",
                "text": format!("File: {local_path}"),
                "text_elements": [],
            })),
            _ => {}
        }
    }
    if items.is_empty() {
        items.push(json!({
            "type": "text",
            "text": "",
            "text_elements": [],
        }));
    }
    items
}

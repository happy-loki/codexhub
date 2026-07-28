use anyhow::{Result, anyhow};
use tokio::{
    sync::mpsc,
    time::{Duration, sleep},
};

use crate::{
    app_state::{ImAccountRuntimeState, SharedState, im_account_key},
    im::core::outbound::ImOutboundSender,
    types::{ChatType, ImPlatformKind, InboundMessage, now_ms},
};

use super::{
    api::{WechatApi, default_long_poll_timeout_ms},
    store,
    types::{WechatMessage, WechatMessageItem, WechatSettings},
};

const WECHAT_SESSION_EXPIRED_ERRCODE: i64 = -14;
const WECHAT_RETRY_DELAY_MS: u64 = 2_000;
const WECHAT_BACKOFF_DELAY_MS: u64 = 30_000;
const WECHAT_MAX_CONSECUTIVE_FAILURES: usize = 3;
const WECHAT_CONTEXT_REFRESH_MARKERS: &[&str] = &[".", "!", "?", "。", "！", "？"];

pub async fn listen_polling(
    state: SharedState,
    api: WechatApi,
    tx: mpsc::Sender<InboundMessage>,
    outbound_tx: ImOutboundSender,
) -> Result<()> {
    let account_id = api.settings().account_id();
    let mut get_updates_buf = store::load_sync_buf(&state, &account_id).await;
    let mut next_timeout_ms = default_long_poll_timeout_ms();
    let mut consecutive_failures = 0usize;
    set_polling_state(&state, &account_id, true, true, None).await;
    match api.notify_start().await {
        Ok(()) => {}
        Err(err) => {
            set_polling_state(&state, &account_id, true, false, Some(err.to_string())).await;
            state
                .push_event("warn", "wechat_notify_start_failed", err.to_string())
                .await;
        }
    }

    loop {
        let response = match api.get_updates(&get_updates_buf, next_timeout_ms).await {
            Ok(response) => response,
            Err(err) => {
                consecutive_failures += 1;
                let delay = if consecutive_failures >= WECHAT_MAX_CONSECUTIVE_FAILURES {
                    consecutive_failures = 0;
                    WECHAT_BACKOFF_DELAY_MS
                } else {
                    WECHAT_RETRY_DELAY_MS
                };
                set_polling_state(&state, &account_id, true, false, Some(err.to_string())).await;
                state
                    .push_event(
                        "warn",
                        "wechat_poll_failed",
                        format!("retry_in_ms={delay} err={err}"),
                    )
                    .await;
                sleep(Duration::from_millis(delay)).await;
                continue;
            }
        };

        if response.ret.unwrap_or(0) != 0 || response.errcode.unwrap_or(0) != 0 {
            let errcode = response.errcode.or(response.ret).unwrap_or_default();
            let errmsg = response.errmsg.unwrap_or_default();
            if errcode == WECHAT_SESSION_EXPIRED_ERRCODE {
                set_polling_state(
                    &state,
                    &account_id,
                    true,
                    false,
                    Some("wechat session expired".to_string()),
                )
                .await;
                state
                    .push_event(
                        "error",
                        "wechat_session_expired",
                        "bot token expired or session paused; please scan WeChat again",
                    )
                    .await;
                sleep(Duration::from_millis(WECHAT_BACKOFF_DELAY_MS)).await;
                continue;
            }
            consecutive_failures += 1;
            set_polling_state(
                &state,
                &account_id,
                true,
                false,
                Some(format!("ret={errcode} {errmsg}")),
            )
            .await;
            state
                .push_event(
                    "warn",
                    "wechat_poll_api_error",
                    format!("ret={errcode} errmsg={errmsg}"),
                )
                .await;
            sleep(Duration::from_millis(WECHAT_RETRY_DELAY_MS)).await;
            continue;
        }

        consecutive_failures = 0;
        set_polling_state(&state, &account_id, true, true, None).await;
        if let Some(timeout_ms) = response.longpolling_timeout_ms.filter(|value| *value > 0) {
            next_timeout_ms = timeout_ms;
        }
        if let Some(next_buf) = response
            .get_updates_buf
            .filter(|value| !value.trim().is_empty())
        {
            get_updates_buf = next_buf.clone();
            if let Err(err) = store::save_sync_buf(&state, &account_id, next_buf).await {
                state
                    .push_event("warn", "wechat_sync_buf_save_failed", err.to_string())
                    .await;
            }
        }
        let messages = response.msgs.unwrap_or_default();
        let message_count = messages.len();
        for message in messages {
            if let Some(inbound) =
                inbound_from_message(&state, &outbound_tx, api.settings(), &account_id, message)
                    .await?
            {
                tx.send(inbound)
                    .await
                    .map_err(|_| anyhow!("wechat inbound pump closed"))?;
            }
        }
        if message_count > 0 {
            state
                .push_event(
                    "info",
                    "wechat_poll_ok",
                    format!("messages={message_count}"),
                )
                .await;
        }
    }
}

async fn inbound_from_message(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    settings: &WechatSettings,
    account_id: &str,
    message: WechatMessage,
) -> Result<Option<InboundMessage>> {
    if message.message_type == Some(2) {
        return Ok(None);
    }
    let Some(peer_id) = message
        .from_user_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        return Ok(None);
    };
    if !user_allowed(settings, &peer_id) {
        state
            .push_event("info", "wechat_message_ignored", format!("peer={peer_id}"))
            .await;
        return Ok(None);
    }
    let text = message_text(&message);
    if let Some(context_token) = message.context_token.as_deref() {
        store::remember_context_token(state, account_id, &peer_id, context_token).await?;
        let replayed = crate::im::core::outbound::replay_wechat_pending_for_peer(
            state,
            outbound_tx,
            account_id,
            &peer_id,
        )
        .await;
        if replayed > 0 {
            update_last_inbound(state, account_id).await;
            state
                .push_event(
                    "info",
                    "wechat_refresh_message_consumed",
                    format!("account={account_id} peer={peer_id} replayed={replayed}"),
                )
                .await;
            if text.trim().is_empty() || is_context_refresh_marker(&text) {
                return Ok(None);
            }
        }
    }
    if text.trim().is_empty() {
        return Ok(None);
    }
    if is_context_refresh_marker(&text) {
        state
            .push_event(
                "info",
                "wechat_context_refresh_marker_ignored",
                format!("account={account_id} peer={peer_id} marker={}", text.trim()),
            )
            .await;
        return Ok(None);
    }
    update_last_inbound(state, account_id).await;
    let message_id = message
        .message_id
        .map(|value| value.to_string())
        .or_else(|| message.seq.map(|value| format!("seq-{value}")))
        .or(message.client_id)
        .unwrap_or_else(|| now_ms().to_string());
    Ok(Some(InboundMessage {
        platform: ImPlatformKind::Wechat,
        account_id: account_id.to_string(),
        sender_id: peer_id.clone(),
        chat_id: peer_id,
        chat_type: ChatType::Direct,
        message_id,
        received_at_ms: now_ms(),
        text,
        mentioned: true,
        approval_request_key: None,
        action: None,
        card_message_id: None,
        callback_req_id: None,
        callback_kind: None,
        attachments: vec![],
    }))
}

fn is_context_refresh_marker(text: &str) -> bool {
    let marker = text.trim();
    WECHAT_CONTEXT_REFRESH_MARKERS
        .iter()
        .any(|candidate| marker == *candidate)
}

fn message_text(message: &WechatMessage) -> String {
    let Some(items) = message.item_list.as_ref() else {
        return String::new();
    };
    items
        .iter()
        .filter_map(item_text)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn item_text(item: &WechatMessageItem) -> Option<String> {
    match item.item_type {
        Some(1) => item
            .text_item
            .as_ref()
            .and_then(|text| text.text.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        Some(3) => item
            .voice_item
            .as_ref()
            .and_then(|voice| voice.text.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

fn user_allowed(settings: &WechatSettings, peer_id: &str) -> bool {
    settings.allowed_user_ids.is_empty()
        || settings
            .allowed_user_ids
            .iter()
            .any(|allowed| allowed == peer_id)
}

async fn set_polling_state(
    state: &SharedState,
    account_id: &str,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
) {
    let now = now_ms();
    let mut wechat = state.wechat.lock().await;
    wechat.polling = polling;
    wechat.connected = connected;
    wechat.last_error = last_error.clone();
    wechat.last_event_at_ms = Some(now);
    let key = im_account_key(ImPlatformKind::Wechat, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Wechat, account_id));
    entry.polling = polling;
    entry.connecting = false;
    entry.connected = connected;
    entry.last_error = last_error;
    entry.last_event_at_ms = Some(now);
}

async fn update_last_inbound(state: &SharedState, account_id: &str) {
    let mut wechat = state.wechat.lock().await;
    let now = now_ms();
    wechat.last_event_at_ms = Some(now);
    wechat.last_inbound_at_ms = Some(now);
    let key = im_account_key(ImPlatformKind::Wechat, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Wechat, account_id));
    entry.last_event_at_ms = Some(now);
    entry.last_inbound_at_ms = Some(now);
}

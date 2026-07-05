use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

use crate::{
    app_state::{ImAccountRuntimeState, SharedState, im_account_key},
    types::{
        ChatType, ImPlatformKind, InboundAction, InboundMessage, ThreadRouteDirection, now_ms,
    },
};

use super::{
    api::{TelegramApi, TelegramApiError, TelegramCallbackQuery, TelegramMessage},
    types::TelegramSettings,
};

const TELEGRAM_LONG_POLL_TIMEOUT_SECONDS: u32 = 25;
const TELEGRAM_STARTUP_PROBE_RETRY_SECONDS: u64 = 5;
const TELEGRAM_CONFLICT_BACKOFF_SECONDS: u64 = 35;
const TELEGRAM_GENERIC_RETRY_SECONDS: u64 = 5;

pub async fn listen_polling(
    state: SharedState,
    api: TelegramApi,
    tx: mpsc::Sender<InboundMessage>,
) -> Result<()> {
    let account_id = api.settings().account_id();
    let mut chat_access = TelegramChatAccess::new(api.settings().allowed_chat_ids.clone());
    let mut offset = None;
    set_polling_state(&state, &account_id, true, false, None).await;
    claim_polling_slot(&state, &api, &mut offset).await;
    loop {
        let updates = match api
            .get_updates(offset, TELEGRAM_LONG_POLL_TIMEOUT_SECONDS)
            .await
        {
            Ok(updates) => updates,
            Err(err) => {
                handle_polling_error(&state, &account_id, &err).await;
                continue;
            }
        };
        set_polling_state(&state, &account_id, true, true, None).await;
        let update_count = updates.len();
        for update in updates {
            offset = Some(update.update_id + 1);
            if let Some(callback) = update.callback_query {
                let callback_id = callback.id.clone();
                let access = ensure_callback_chat_allowed(
                    &state,
                    &api,
                    &mut chat_access,
                    callback.message.as_ref().map(|message| &message.chat),
                )
                .await;
                if access == TelegramChatAccessDecision::Allowed
                    && let Some(inbound) = inbound_from_callback(
                        api.settings(),
                        &chat_access.allowed_chat_ids,
                        callback,
                    )
                {
                    let _ = api
                        .answer_callback_query(&callback_id, Some("已收到"))
                        .await;
                    tx.send(inbound)
                        .await
                        .map_err(|_| anyhow::anyhow!("telegram inbound pump closed"))?;
                    update_last_inbound(&state, &account_id).await;
                } else {
                    let message = match access {
                        TelegramChatAccessDecision::Denied => "当前聊天未授权",
                        _ => "这个操作不可用",
                    };
                    let _ = api.answer_callback_query(&callback_id, Some(message)).await;
                }
                continue;
            }
            if let Some(message) = update.message {
                match ensure_message_chat_allowed(&state, &api, &mut chat_access, &message).await {
                    TelegramChatAccessDecision::Allowed => {
                        if let Some(inbound) = inbound_from_message(
                            api.settings(),
                            &chat_access.allowed_chat_ids,
                            message,
                        ) {
                            let _ = api.send_chat_action(&inbound.chat_id, "typing").await;
                            tx.send(inbound)
                                .await
                                .map_err(|_| anyhow::anyhow!("telegram inbound pump closed"))?;
                            update_last_inbound(&state, &account_id).await;
                        }
                    }
                    TelegramChatAccessDecision::Denied => {
                        let chat_id = message.chat.id.to_string();
                        let _ = api
                            .send_text(
                                &chat_id,
                                "当前 Telegram 私聊未授权。请在本机 Codex Remote Gateway 配置 allowedChatIds。",
                            )
                            .await;
                    }
                    TelegramChatAccessDecision::Ignored => {}
                }
            }
        }
        if update_count > 0 {
            state
                .push_event(
                    "info",
                    "telegram_poll_ok",
                    format!("updates={update_count}"),
                )
                .await;
        }
    }
}

async fn claim_polling_slot(state: &SharedState, api: &TelegramApi, offset: &mut Option<i64>) {
    loop {
        match api.get_updates(*offset, 0).await {
            Ok(updates) => {
                for update in updates {
                    *offset = Some(update.update_id + 1);
                }
                set_polling_state(state, &api.settings().account_id(), true, true, None).await;
                state
                    .push_event(
                        "info",
                        "telegram_poll_ready",
                        "startup probe ok".to_string(),
                    )
                    .await;
                return;
            }
            Err(err) => {
                let delay = retry_delay_seconds(&err, TELEGRAM_STARTUP_PROBE_RETRY_SECONDS);
                set_polling_state(
                    state,
                    &api.settings().account_id(),
                    true,
                    false,
                    Some(err.to_string()),
                )
                .await;
                state
                    .push_event(
                        "warn",
                        "telegram_poll_probe_failed",
                        format!("retry_in={delay}s err={err}"),
                    )
                    .await;
                sleep(Duration::from_secs(delay)).await;
            }
        }
    }
}

async fn handle_polling_error(state: &SharedState, account_id: &str, err: &anyhow::Error) {
    let delay = retry_delay_seconds(err, TELEGRAM_GENERIC_RETRY_SECONDS);
    let kind = err
        .downcast_ref::<TelegramApiError>()
        .filter(|api_error| api_error.is_conflict())
        .map(|_| "telegram_poll_conflict")
        .unwrap_or("telegram_poll_failed");
    set_polling_state(state, account_id, true, false, Some(err.to_string())).await;
    state
        .push_event("warn", kind, format!("retry_in={delay}s err={err}"))
        .await;
    sleep(Duration::from_secs(delay)).await;
}

fn retry_delay_seconds(err: &anyhow::Error, default_delay: u64) -> u64 {
    if let Some(api_error) = err.downcast_ref::<TelegramApiError>() {
        if api_error.is_conflict() {
            return TELEGRAM_CONFLICT_BACKOFF_SECONDS;
        }
        if let Some(retry_after) = api_error.retry_after {
            return retry_after.max(1);
        }
    }
    default_delay
}

fn inbound_from_message(
    settings: &TelegramSettings,
    allowed_chat_ids: &[String],
    message: TelegramMessage,
) -> Option<InboundMessage> {
    if message.chat.kind != "private" {
        return None;
    }
    let text = message.text?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let chat_id = message.chat.id.to_string();
    if !chat_allowed(allowed_chat_ids, &chat_id) {
        return None;
    }
    let sender_id = message
        .from
        .as_ref()
        .map(|user| user.id.to_string())
        .unwrap_or_else(|| chat_id.clone());

    Some(InboundMessage {
        platform: ImPlatformKind::Telegram,
        account_id: settings.account_id(),
        sender_id,
        chat_id,
        chat_type: ChatType::Direct,
        message_id: message.message_id.to_string(),
        received_at_ms: now_ms(),
        text,
        mentioned: true,
        approval_request_key: None,
        action: None,
        card_message_id: None,
        attachments: vec![],
    })
}

fn inbound_from_callback(
    settings: &TelegramSettings,
    allowed_chat_ids: &[String],
    callback: TelegramCallbackQuery,
) -> Option<InboundMessage> {
    let data = callback.data?;
    let action = action_from_callback_data(&data)?;
    let message = callback.message?;
    if message.chat.kind != "private" {
        return None;
    }
    let chat_id = message.chat.id.to_string();
    if !chat_allowed(allowed_chat_ids, &chat_id) {
        return None;
    }

    Some(InboundMessage {
        platform: ImPlatformKind::Telegram,
        account_id: settings.account_id(),
        sender_id: callback.from.id.to_string(),
        chat_id,
        chat_type: ChatType::Direct,
        message_id: message.message_id.to_string(),
        received_at_ms: now_ms(),
        text: data,
        mentioned: true,
        approval_request_key: None,
        action: Some(action),
        card_message_id: Some(message.message_id.to_string()),
        attachments: vec![],
    })
}

async fn set_polling_state(
    state: &SharedState,
    account_id: &str,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
) {
    let now = now_ms();
    let mut telegram = state.telegram.lock().await;
    telegram.polling = polling;
    telegram.connected = connected;
    telegram.last_error = last_error.clone();
    telegram.last_event_at_ms = Some(now);
    let key = im_account_key(ImPlatformKind::Telegram, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Telegram, account_id));
    entry.polling = polling;
    entry.connecting = false;
    entry.connected = connected;
    entry.last_error = last_error;
    entry.last_event_at_ms = Some(now);
}

async fn update_last_inbound(state: &SharedState, account_id: &str) {
    let mut telegram = state.telegram.lock().await;
    let now = now_ms();
    telegram.last_event_at_ms = Some(now);
    telegram.last_inbound_at_ms = Some(now);
    let key = im_account_key(ImPlatformKind::Telegram, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Telegram, account_id));
    entry.last_event_at_ms = Some(now);
    entry.last_inbound_at_ms = Some(now);
}

#[derive(Debug, Clone)]
struct TelegramChatAccess {
    allowed_chat_ids: Vec<String>,
}

impl TelegramChatAccess {
    fn new(allowed_chat_ids: Vec<String>) -> Self {
        Self { allowed_chat_ids }
    }

    fn is_allowed(&self, chat_id: &str) -> bool {
        chat_allowed(&self.allowed_chat_ids, chat_id)
    }

    fn remember(&mut self, chat_id: &str) {
        if !self.is_allowed(chat_id) {
            self.allowed_chat_ids.push(chat_id.to_string());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelegramChatAccessDecision {
    Allowed,
    Denied,
    Ignored,
}

async fn ensure_message_chat_allowed(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    message: &TelegramMessage,
) -> TelegramChatAccessDecision {
    ensure_chat_allowed(state, api, access, &message.chat).await
}

async fn ensure_callback_chat_allowed(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    chat: Option<&super::api::TelegramChat>,
) -> TelegramChatAccessDecision {
    let Some(chat) = chat else {
        return TelegramChatAccessDecision::Ignored;
    };
    ensure_chat_allowed(state, api, access, chat).await
}

async fn ensure_chat_allowed(
    state: &SharedState,
    api: &TelegramApi,
    access: &mut TelegramChatAccess,
    chat: &super::api::TelegramChat,
) -> TelegramChatAccessDecision {
    if chat.kind != "private" {
        return TelegramChatAccessDecision::Ignored;
    }
    let account_id = api.settings().account_id();
    let chat_id = chat.id.to_string();
    if access.is_allowed(&chat_id) {
        return TelegramChatAccessDecision::Allowed;
    }
    if !access.allowed_chat_ids.is_empty() {
        log_denied_chat(state, &account_id, &chat_id).await;
        return TelegramChatAccessDecision::Denied;
    }

    let (bind_result, save_error) = {
        let mut config = state.config.lock().await;
        let result = config.ensure_telegram_allowed_chat_id(&account_id, &chat_id);
        let save_error = if result.should_save() {
            config
                .save(&state.config_path)
                .err()
                .map(|err| err.to_string())
        } else {
            None
        };
        (result, save_error)
    };
    if let Some(err) = save_error {
        state
            .push_event(
                "error",
                "telegram_chat_bind_failed",
                format!("account={account_id} chat={chat_id} err={err}"),
            )
            .await;
        return TelegramChatAccessDecision::Denied;
    }

    match bind_result {
        crate::config::TelegramChatAllowResult::Allowed
        | crate::config::TelegramChatAllowResult::Bound => {
            access.remember(&chat_id);
            if bind_result == crate::config::TelegramChatAllowResult::Bound {
                state
                    .push_event(
                        "info",
                        "telegram_chat_bound",
                        format!("account={account_id} chat={chat_id}"),
                    )
                    .await;
            }
            TelegramChatAccessDecision::Allowed
        }
        crate::config::TelegramChatAllowResult::Denied => {
            log_denied_chat(state, &account_id, &chat_id).await;
            TelegramChatAccessDecision::Denied
        }
        crate::config::TelegramChatAllowResult::AccountNotFound => {
            state
                .push_event(
                    "warn",
                    "telegram_chat_bind_account_missing",
                    format!("account={account_id} chat={chat_id}"),
                )
                .await;
            TelegramChatAccessDecision::Denied
        }
    }
}

async fn log_denied_chat(state: &SharedState, account_id: &str, chat_id: &str) {
    state
        .push_event(
            "warn",
            "telegram_chat_denied",
            format!("account={account_id} chat={chat_id}"),
        )
        .await;
}

fn chat_allowed(allowed_chat_ids: &[String], chat_id: &str) -> bool {
    allowed_chat_ids
        .iter()
        .any(|allowed| allowed.trim() == chat_id)
}

fn action_from_callback_data(data: &str) -> Option<InboundAction> {
    let parts = data.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        ["ap", request_fingerprint, option_index] => Some(InboundAction::ApprovalDecision {
            request_fingerprint: (*request_fingerprint).to_string(),
            option_index: option_index.parse().ok()?,
        }),
        ["trc", request_id, action] => Some(InboundAction::ThreadRouteChoice {
            request_id: (*request_id).to_string(),
            action: match *action {
                "new" => "create_new",
                "load" => "resume_history",
                "back" => "back",
                _ => return None,
            }
            .to_string(),
        }),
        ["trd", request_id] => Some(InboundAction::ThreadRouteCreateDefault {
            request_id: (*request_id).to_string(),
        }),
        ["tcc", request_id] => Some(InboundAction::ThreadRouteCreateConfigured {
            request_id: (*request_id).to_string(),
        }),
        ["tce", request_id, field] => Some(InboundAction::ThreadRouteCreateEdit {
            request_id: (*request_id).to_string(),
            field: (*field).to_string(),
        }),
        ["tcs", request_id, field, page, index] => Some(InboundAction::ThreadRouteCreateSetIndex {
            request_id: (*request_id).to_string(),
            field: (*field).to_string(),
            page: page.parse().ok()?,
            index: index.parse().ok()?,
        }),
        ["tcv", request_id, field, value] => Some(InboundAction::ThreadRouteCreateSetValue {
            request_id: (*request_id).to_string(),
            field: (*field).to_string(),
            value: (*value).to_string(),
        }),
        ["tcp", request_id, field, direction] => {
            Some(InboundAction::ThreadRouteCreateOptionsPage {
                request_id: (*request_id).to_string(),
                field: (*field).to_string(),
                direction: match *direction {
                    "prev" => ThreadRouteDirection::Prev,
                    "next" => ThreadRouteDirection::Next,
                    _ => return None,
                },
            })
        }
        ["trs", request_id, page, index] => Some(InboundAction::ThreadRouteResumeIndex {
            request_id: (*request_id).to_string(),
            page: page.parse().ok()?,
            index: index.parse().ok()?,
        }),
        ["tlp", request_id, direction] => Some(InboundAction::ThreadRouteListPage {
            request_id: (*request_id).to_string(),
            direction: match *direction {
                "prev" => ThreadRouteDirection::Prev,
                "next" => ThreadRouteDirection::Next,
                _ => return None,
            },
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::im::telegram::api::{TelegramChat, TelegramUser};

    #[test]
    fn converts_private_message_to_inbound() {
        let settings = TelegramSettings::default();
        let inbound = inbound_from_message(
            &settings,
            &["42".to_string()],
            TelegramMessage {
                message_id: 9,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: 42,
                    kind: "private".to_string(),
                    title: None,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                },
                text: Some("/status".to_string()),
            },
        )
        .expect("inbound message");

        assert_eq!(inbound.platform, ImPlatformKind::Telegram);
        assert_eq!(inbound.conversation_key(), "telegram:telegram:42");
        assert_eq!(inbound.chat_type, ChatType::Direct);
        assert_eq!(inbound.text, "/status");
    }

    #[test]
    fn empty_allowed_chat_ids_do_not_pass_message_conversion() {
        let settings = TelegramSettings::default();
        let inbound = inbound_from_message(
            &settings,
            &[],
            TelegramMessage {
                message_id: 9,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: 42,
                    kind: "private".to_string(),
                    title: None,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                },
                text: Some("/status".to_string()),
            },
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn rejects_private_message_from_unlisted_chat() {
        let settings = TelegramSettings::default();
        let inbound = inbound_from_message(
            &settings,
            &["99".to_string()],
            TelegramMessage {
                message_id: 9,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: 42,
                    kind: "private".to_string(),
                    title: None,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                },
                text: Some("/status".to_string()),
            },
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn ignores_group_messages() {
        let settings = TelegramSettings {
            mention_only: true,
            ..TelegramSettings::default()
        };
        let message = TelegramMessage {
            message_id: 10,
            from: Some(TelegramUser {
                id: 42,
                is_bot: false,
                username: Some("ada".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: None,
            }),
            chat: TelegramChat {
                id: -100,
                kind: "group".to_string(),
                title: Some("Codex".to_string()),
                username: None,
                first_name: None,
                last_name: None,
            },
            text: Some("hello".to_string()),
        };

        assert!(inbound_from_message(&settings, &["42".to_string()], message).is_none());
    }

    #[test]
    fn ignores_group_messages_even_when_mentioned() {
        let settings = TelegramSettings {
            mention_only: true,
            ..TelegramSettings::default()
        };
        let inbound = inbound_from_message(
            &settings,
            &["42".to_string()],
            TelegramMessage {
                message_id: 11,
                from: Some(TelegramUser {
                    id: 42,
                    is_bot: false,
                    username: Some("ada".to_string()),
                    first_name: Some("Ada".to_string()),
                    last_name: None,
                }),
                chat: TelegramChat {
                    id: -100,
                    kind: "group".to_string(),
                    title: Some("Codex".to_string()),
                    username: None,
                    first_name: None,
                    last_name: None,
                },
                text: Some("@codex_bot hello".to_string()),
            },
        );

        assert!(inbound.is_none());
    }

    #[test]
    fn parses_thread_route_callback_data() {
        let action = action_from_callback_data("trs:thread-route-7:2:3").expect("resume action");
        match action {
            InboundAction::ThreadRouteResumeIndex {
                request_id,
                page,
                index,
            } => {
                assert_eq!(request_id, "thread-route-7");
                assert_eq!(page, 2);
                assert_eq!(index, 3);
            }
            other => panic!("unexpected action: {other:?}"),
        }

        let action = action_from_callback_data("ap:abc123:2").expect("approval action");
        match action {
            InboundAction::ApprovalDecision {
                request_fingerprint,
                option_index,
            } => {
                assert_eq!(request_fingerprint, "abc123");
                assert_eq!(option_index, 2);
            }
            other => panic!("unexpected action: {other:?}"),
        }

        let action = action_from_callback_data("tlp:thread-route-7:next").expect("page action");
        match action {
            InboundAction::ThreadRouteListPage {
                request_id,
                direction,
            } => {
                assert_eq!(request_id, "thread-route-7");
                assert_eq!(direction, ThreadRouteDirection::Next);
            }
            other => panic!("unexpected action: {other:?}"),
        }
    }
}

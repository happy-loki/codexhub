use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{
    app_state::{ImAccountRuntimeState, SharedState, im_account_key},
    types::{
        ChatType, ImPlatformKind, InboundAction, InboundAttachment, InboundCallbackKind,
        InboundMessage, ThreadRouteDirection, now_ms,
    },
};

use super::api::{WecomApi, WecomSendCommand};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const AUTH_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn listen_ws(
    state: SharedState,
    api: WecomApi,
    account_id: String,
    inbound_tx: mpsc::Sender<InboundMessage>,
) -> Result<()> {
    set_account_ws_state(&state, &account_id, true, false, None).await;
    let url = if api.settings.websocket_url.trim().is_empty() {
        "wss://openws.work.weixin.qq.com"
    } else {
        api.settings.websocket_url.trim()
    };
    let (stream, _) = connect_async(url)
        .await
        .with_context(|| format!("failed to connect WeCom WebSocket host={}", host_label(url)))?;
    let (mut sink, mut source) = stream.split();

    let auth_req_id = request_id("aibot_subscribe");
    let auth = json!({
        "cmd": "aibot_subscribe",
        "headers": { "req_id": auth_req_id },
        "body": {
            "bot_id": api.settings.bot_id,
            "secret": api.settings.secret,
            "scene": 1,
            "plug_version": env!("CARGO_PKG_VERSION")
        }
    });
    sink.send(Message::Text(auth.to_string().into())).await?;
    wait_for_auth(&mut source, &auth_req_id).await?;

    let (send_tx, mut send_rx) = mpsc::unbounded_channel();
    api.install_sender(Some(send_tx)).await;
    set_account_ws_state(&state, &account_id, false, true, None).await;
    state
        .push_event(
            "info",
            "wecom_ws_connected",
            format!("account={account_id}"),
        )
        .await;

    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut pending: HashMap<String, oneshot::Sender<Result<Value>>> = HashMap::new();

    let result = loop {
        tokio::select! {
            frame = source.next() => {
                let Some(frame) = frame else {
                    break Err(anyhow::anyhow!("WeCom WebSocket closed"));
                };
                match frame? {
                    Message::Text(text) => {
                        let value: Value = serde_json::from_str(text.as_ref())
                            .context("invalid WeCom WebSocket JSON")?;
                        if let Some(req_id) = response_req_id(&value)
                            && let Some(reply) = pending.remove(req_id)
                        {
                            let result = if response_ok(&value) {
                                Ok(value)
                            } else {
                                Err(anyhow::anyhow!("WeCom send failed: {}", response_error(&value)))
                            };
                            let _ = reply.send(result);
                            continue;
                        }
                        if is_event_frame(&value) {
                            if let Some(message) = normalize_card_event(&account_id, &value) {
                                state
                                    .push_event(
                                        "info",
                                        "wecom_card_event_received",
                                        format!(
                                            "account={} chat={} action={:?}",
                                            account_id, message.chat_id, message.action
                                        ),
                                    )
                                    .await;
                                update_last_inbound(&state, &account_id).await;
                                inbound_tx.send(message).await.context("WeCom inbound channel closed")?;
                            } else {
                                        state
                                            .push_event(
                                                "warn",
                                                "wecom_event_ignored",
                                                format!("{} raw={value}", event_summary(&value)),
                                            )
                                            .await;
                            }
                        } else if value.get("cmd").and_then(Value::as_str) == Some("aibot_msg_callback") {
                            if let Some(message) = normalize_message(&state, &api, &account_id, &value).await {
                                update_last_inbound(&state, &account_id).await;
                                inbound_tx.send(message).await.context("WeCom inbound channel closed")?;
                            }
                        }
                    }
                    Message::Ping(payload) => sink.send(Message::Pong(payload)).await?,
                    Message::Close(frame) => break Err(anyhow::anyhow!("WeCom WebSocket closed: {frame:?}")),
                    _ => {}
                }
            }
            command = send_rx.recv() => {
                let Some(command) = command else {
                    break Err(anyhow::anyhow!("WeCom outbound channel closed"));
                };
                send_command(&mut sink, &mut pending, command).await?;
            }
            _ = heartbeat.tick() => {
                let ping = json!({
                    "cmd": "ping",
                    "headers": { "req_id": request_id("ping") }
                });
                sink.send(Message::Text(ping.to_string().into())).await?;
            }
        }
    };

    api.install_sender(None).await;
    for (_, reply) in pending {
        let _ = reply.send(Err(anyhow::anyhow!("WeCom WebSocket disconnected")));
    }
    set_account_ws_state(
        &state,
        &account_id,
        false,
        false,
        result.as_ref().err().map(ToString::to_string),
    )
    .await;
    result
}

async fn wait_for_auth<S>(source: &mut S, expected_req_id: &str) -> Result<()>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    tokio::time::timeout(AUTH_TIMEOUT, async {
        while let Some(frame) = source.next().await {
            if let Message::Text(text) = frame? {
                let value: Value = serde_json::from_str(text.as_ref())?;
                if response_req_id(&value) == Some(expected_req_id) {
                    if response_ok(&value) {
                        return Ok(());
                    }
                    bail!("WeCom authentication failed: {}", response_error(&value));
                }
            }
        }
        bail!("WeCom WebSocket closed during authentication")
    })
    .await
    .context("WeCom authentication timed out")?
}

async fn send_command<S>(
    sink: &mut S,
    pending: &mut HashMap<String, oneshot::Sender<Result<Value>>>,
    command: WecomSendCommand,
) -> Result<()>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let req_id = command.req_id.unwrap_or_else(|| request_id(&command.cmd));
    let frame = json!({
        "cmd": command.cmd,
        "headers": { "req_id": req_id },
        "body": command.body
    });
    sink.send(Message::Text(frame.to_string().into())).await?;
    pending.insert(req_id, command.result);
    Ok(())
}

async fn normalize_message(
    state: &SharedState,
    api: &WecomApi,
    account_id: &str,
    value: &Value,
) -> Option<InboundMessage> {
    let body = value.get("body")?;
    let msgtype = body.get("msgtype")?.as_str()?;
    let sender_id = body.pointer("/from/userid")?.as_str()?.trim();
    let chat_type = match body.get("chattype").and_then(Value::as_str)? {
        "single" => ChatType::Direct,
        "group" => ChatType::Group,
        _ => return None,
    };
    let chat_id = match chat_type {
        ChatType::Direct => sender_id,
        ChatType::Group => body.get("chatid")?.as_str()?.trim(),
    };
    if !allowed(&api.settings.allowed_user_ids, sender_id)
        || !allowed(&api.settings.allowed_chat_ids, chat_id)
    {
        return None;
    }
    let attachments = collect_attachments(state, msgtype, body).await;
    let text = match msgtype {
        "text" => body.pointer("/text/content")?.as_str()?.to_string(),
        "image" => "请分析这张图片。".to_string(),
        "file" => format!(
            "请处理附件：{}",
            attachments
                .first()
                .and_then(|attachment| attachment.name.as_deref())
                .unwrap_or("attachment")
        ),
        _ => return None,
    };
    let callback_req_id = response_req_id(value).unwrap_or_default().to_string();
    let message_id = body
        .get("msgid")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if msgtype == "text" {
        remember_stream(state, account_id, chat_id, &message_id, &callback_req_id).await;
    }
    Some(InboundMessage {
        platform: ImPlatformKind::Wecom,
        account_id: account_id.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        chat_type,
        message_id,
        received_at_ms: now_ms(),
        text,
        mentioned: true,
        approval_request_key: None,
        action: None,
        card_message_id: None,
        callback_req_id: (!callback_req_id.is_empty()).then_some(callback_req_id),
        callback_kind: Some(InboundCallbackKind::Message),
        attachments,
    })
}

fn normalize_card_event(account_id: &str, value: &Value) -> Option<InboundMessage> {
    let body = value.get("body")?;
    let event = body.get("event")?;
    let event_type = event.get("eventtype")?.as_str()?;
    let action = match event_type {
        "enter_chat" => InboundAction::ThreadRouteOpen,
        "template_card_event" => {
            let card_event = event.get("template_card_event").unwrap_or(event);
            let event_key = card_event.get("event_key")?.as_str()?;
            parse_card_event_action(event_key, card_event)?
        }
        _ => return None,
    };
    let sender_id = body.pointer("/from/userid")?.as_str()?.to_string();
    let chat_type = match body
        .get("chattype")
        .and_then(Value::as_str)
        .unwrap_or("single")
    {
        "group" => ChatType::Group,
        _ => ChatType::Direct,
    };
    let chat_id = match chat_type {
        ChatType::Direct => sender_id.clone(),
        ChatType::Group => body.get("chatid")?.as_str()?.to_string(),
    };
    Some(InboundMessage {
        platform: ImPlatformKind::Wecom,
        account_id: account_id.to_string(),
        sender_id,
        chat_id,
        chat_type,
        message_id: body
            .get("msgid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        received_at_ms: now_ms(),
        text: String::new(),
        mentioned: true,
        approval_request_key: None,
        action: Some(action),
        card_message_id: response_req_id(value).map(|req_id| {
            let card_event = event.get("template_card_event").unwrap_or(event);
            let task_id = card_event
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("{req_id}|{task_id}")
        }),
        callback_req_id: response_req_id(value).map(str::to_string),
        callback_kind: Some(if event_type == "enter_chat" {
            InboundCallbackKind::Welcome
        } else {
            InboundCallbackKind::CardEvent
        }),
        attachments: Vec::new(),
    })
}

fn is_event_frame(value: &Value) -> bool {
    value.get("cmd").and_then(Value::as_str) == Some("aibot_event_callback")
        || value.pointer("/body/msgtype").and_then(Value::as_str) == Some("event")
        || value.pointer("/body/event").is_some_and(Value::is_object)
}

fn event_summary(value: &Value) -> String {
    let cmd = value.get("cmd").and_then(Value::as_str).unwrap_or("");
    let event_type = value
        .pointer("/body/event/eventtype")
        .and_then(Value::as_str)
        .unwrap_or("");
    let event_key = value
        .pointer("/body/event/event_key")
        .or_else(|| value.pointer("/body/event/template_card_event/event_key"))
        .and_then(Value::as_str)
        .unwrap_or("");
    format!("cmd={cmd} event_type={event_type} event_key={event_key}")
}

fn parse_card_event_key(value: &str) -> Option<InboundAction> {
    let parts = value.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        ["approval", fingerprint, index] => Some(InboundAction::ApprovalDecision {
            request_fingerprint: (*fingerprint).to_string(),
            option_index: index.parse().ok()?,
        }),
        ["thread-choice", request_id, action] => Some(InboundAction::ThreadRouteChoice {
            request_id: (*request_id).to_string(),
            action: (*action).to_string(),
        }),
        ["thread-create-default", request_id] => Some(InboundAction::ThreadRouteCreateDefault {
            request_id: (*request_id).to_string(),
        }),
        ["thread-create-custom-cwd", request_id] => Some(InboundAction::ThreadRouteCreateEdit {
            request_id: (*request_id).to_string(),
            field: "cwd".to_string(),
        }),
        ["thread-resume", request_id, page, index] => Some(InboundAction::ThreadRouteResumeIndex {
            request_id: (*request_id).to_string(),
            page: page.parse().ok()?,
            index: index.parse().ok()?,
        }),
        ["thread-page", request_id, direction] => {
            let direction = match *direction {
                "prev" => ThreadRouteDirection::Prev,
                "next" => ThreadRouteDirection::Next,
                _ => return None,
            };
            Some(InboundAction::ThreadRouteListPage {
                request_id: (*request_id).to_string(),
                direction,
            })
        }
        _ => None,
    }
}

fn parse_card_event_action(value: &str, event: &Value) -> Option<InboundAction> {
    let parts = value.split(':').collect::<Vec<_>>();
    if let ["thread-create-custom-cwd", request_id, permission] = parts.as_slice() {
        return Some(InboundAction::ThreadRouteCreateSubmit {
            request_id: (*request_id).to_string(),
            cwd_choice: Some("__custom__".to_string()),
            cwd_custom: None,
            model: selected_card_option(event, "model"),
            effort: selected_card_option(event, "effort"),
            permission: Some((*permission).to_string()),
        });
    }
    if let ["thread-create-submit", request_id, permission] = parts.as_slice() {
        return Some(InboundAction::ThreadRouteCreateSubmit {
            request_id: (*request_id).to_string(),
            cwd_choice: selected_card_option(event, "cwd"),
            cwd_custom: None,
            model: selected_card_option(event, "model"),
            effort: selected_card_option(event, "effort"),
            permission: Some((*permission).to_string()),
        });
    }
    if let ["thread-select", request_id] = parts.as_slice() {
        return selected_card_option(event, "thread_session").map(|thread_id| {
            InboundAction::ThreadRouteResumeSelected {
                request_id: (*request_id).to_string(),
                thread_id,
            }
        });
    }
    parse_card_event_key(value)
}

fn selected_card_option(event: &Value, question_key: &str) -> Option<String> {
    event
        .pointer("/selected_items/selected_item")
        .and_then(Value::as_array)?
        .iter()
        .find(|item| item.get("question_key").and_then(Value::as_str) == Some(question_key))?
        .pointer("/option_ids/option_id")
        .and_then(Value::as_array)?
        .first()
        .and_then(Value::as_str)
        .map(str::to_string)
}

async fn remember_stream(
    state: &SharedState,
    account_id: &str,
    chat_id: &str,
    message_id: &str,
    req_id: &str,
) {
    if req_id.is_empty() {
        return;
    }
    let conversation_key = format!("wecom:{account_id}:{chat_id}");
    state.runtime.lock().await.wecom_streams_by_thread.insert(
        conversation_key,
        crate::im_runtime::WecomStreamState {
            req_id: req_id.to_string(),
            stream_id: format!("stream_{}", sanitize_id(message_id)),
            content: String::new(),
            finished: false,
        },
    );
}

fn sanitize_id(value: &str) -> String {
    let filtered = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '@'))
        .take(100)
        .collect::<String>();
    if filtered.is_empty() {
        uuid::Uuid::new_v4().simple().to_string()
    } else {
        filtered
    }
}

async fn collect_attachments(
    state: &SharedState,
    msgtype: &str,
    body: &Value,
) -> Vec<InboundAttachment> {
    let Some(media) = body.get(msgtype) else {
        return Vec::new();
    };
    let Some(url) = media.get("url").and_then(Value::as_str) else {
        return Vec::new();
    };
    let bytes = match crate::outbound_http::get().get(url).send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.bytes().await {
                Ok(bytes) => bytes.to_vec(),
                Err(_) => return Vec::new(),
            },
            Err(_) => return Vec::new(),
        },
        Err(_) => return Vec::new(),
    };
    let bytes = match media.get("aeskey").and_then(Value::as_str) {
        Some(key) => decrypt_media(&bytes, key).unwrap_or(bytes),
        None => bytes,
    };
    let fallback_name = if msgtype == "image" {
        "image.png"
    } else {
        "attachment.bin"
    };
    let name = media
        .get("filename")
        .or_else(|| media.get("file_name"))
        .and_then(Value::as_str)
        .unwrap_or(fallback_name);
    let content_type = mime_guess::from_path(name).first_raw().map(str::to_string);
    let root = state
        .config
        .lock()
        .await
        .state_path
        .parent()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".im")
        .join("attachments")
        .join("wecom")
        .join(if msgtype == "image" {
            "images"
        } else {
            "files"
        });
    if tokio::fs::create_dir_all(&root).await.is_err() {
        return Vec::new();
    }
    let path = root.join(format!("{}_{}", now_ms(), sanitize_filename(name)));
    if tokio::fs::write(&path, bytes).await.is_err() {
        return Vec::new();
    }
    vec![InboundAttachment {
        kind: msgtype.to_string(),
        name: Some(name.to_string()),
        mime_type: content_type,
        text_hint: None,
        local_path: Some(path.to_string_lossy().to_string()),
    }]
}

fn decrypt_media(encrypted: &[u8], aes_key: &str) -> Result<Vec<u8>> {
    use aes::cipher::{BlockDecrypt, KeyInit};
    use base64::Engine as _;
    let key = base64::engine::general_purpose::STANDARD
        .decode(aes_key)
        .context("invalid WeCom media AES key")?;
    anyhow::ensure!(key.len() == 32, "invalid WeCom media AES key length");
    anyhow::ensure!(
        encrypted.len() % 16 == 0,
        "invalid WeCom media ciphertext length"
    );
    let cipher = aes::Aes256::new_from_slice(&key)?;
    let mut buffer = encrypted.to_vec();
    let mut previous = key[..16].to_vec();
    for block in buffer.chunks_exact_mut(16) {
        let ciphertext = block.to_vec();
        cipher.decrypt_block(block.into());
        for (byte, iv_byte) in block.iter_mut().zip(previous.iter()) {
            *byte ^= *iv_byte;
        }
        previous = ciphertext;
    }
    let decrypted = buffer.as_slice();
    let padding = *decrypted.last().context("empty WeCom media payload")? as usize;
    anyhow::ensure!(
        padding > 0 && padding <= 32 && padding <= decrypted.len(),
        "invalid WeCom media padding"
    );
    anyhow::ensure!(
        decrypted[decrypted.len() - padding..]
            .iter()
            .all(|byte| *byte as usize == padding),
        "invalid WeCom media padding bytes"
    );
    Ok(decrypted[..decrypted.len() - padding].to_vec())
}

fn sanitize_filename(value: &str) -> String {
    let value = std::path::Path::new(value)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment.bin");
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn allowed(allowlist: &[String], value: &str) -> bool {
    allowlist.is_empty() || allowlist.iter().any(|allowed| allowed.trim() == value)
}

fn request_id(prefix: &str) -> String {
    format!("{prefix}_{}_{}", now_ms(), uuid::Uuid::new_v4().simple())
}

fn response_req_id(value: &Value) -> Option<&str> {
    value.pointer("/headers/req_id").and_then(Value::as_str)
}

fn response_ok(value: &Value) -> bool {
    value
        .get("errcode")
        .or_else(|| value.pointer("/body/errcode"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
        == 0
}

fn response_error(value: &Value) -> String {
    value
        .get("errmsg")
        .or_else(|| value.pointer("/body/errmsg"))
        .and_then(Value::as_str)
        .unwrap_or("unknown error")
        .to_string()
}

fn host_label(raw: &str) -> String {
    url::Url::parse(raw)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "invalid-url".to_string())
}

pub(crate) async fn set_account_ws_state(
    state: &SharedState,
    account_id: &str,
    connecting: bool,
    connected: bool,
    last_error: Option<String>,
) {
    let key = im_account_key(ImPlatformKind::Wecom, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Wecom, account_id));
    entry.connecting = connecting;
    entry.polling = false;
    entry.connected = connected;
    entry.last_error = last_error;
    entry.last_event_at_ms = Some(now_ms());
}

async fn update_last_inbound(state: &SharedState, account_id: &str) {
    let key = im_account_key(ImPlatformKind::Wecom, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Wecom, account_id));
    let now = now_ms();
    entry.last_event_at_ms = Some(now);
    entry.last_inbound_at_ms = Some(now);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::im::wecom::types::WecomSettings;
    use crate::{app_state::AppState, config::AppConfig};

    fn api() -> WecomApi {
        WecomApi::new(WecomSettings {
            bot_id: "bot".into(),
            secret: "secret".into(),
            websocket_url: String::new(),
            allowed_user_ids: Vec::new(),
            allowed_chat_ids: Vec::new(),
        })
    }

    fn state() -> SharedState {
        AppState::new(
            std::env::temp_dir().join("codexhub-wecom-test.toml"),
            AppConfig::default(),
            None,
            None,
        )
    }

    #[tokio::test]
    async fn normalizes_direct_and_group_text_callbacks() {
        let direct = json!({"headers":{"req_id":"r1"},"body":{"msgid":"m1","chattype":"single","from":{"userid":"u1"},"msgtype":"text","text":{"content":"hello"}}});
        let direct = normalize_message(&state(), &api(), "a1", &direct)
            .await
            .unwrap();
        assert_eq!(direct.platform, ImPlatformKind::Wecom);
        assert_eq!(direct.chat_id, "u1");
        assert_eq!(direct.chat_type, ChatType::Direct);
        assert_eq!(direct.callback_req_id.as_deref(), Some("r1"));
        assert_eq!(direct.callback_kind, Some(InboundCallbackKind::Message));

        let group = json!({"headers":{"req_id":"r2"},"body":{"msgid":"m2","chattype":"group","chatid":"g1","from":{"userid":"u2"},"msgtype":"text","text":{"content":"hi"}}});
        let group = normalize_message(&state(), &api(), "a1", &group)
            .await
            .unwrap();
        assert_eq!(group.chat_id, "g1");
        assert_eq!(group.chat_type, ChatType::Group);
        assert_eq!(group.callback_req_id.as_deref(), Some("r2"));
        assert_eq!(group.callback_kind, Some(InboundCallbackKind::Message));
    }

    #[test]
    fn normalizes_approval_card_event() {
        let event = json!({
            "cmd": "aibot_event_callback",
            "headers": { "req_id": "event-1" },
            "body": {
                "msgid": "m3",
                "chattype": "single",
                "from": { "userid": "u3" },
                "event": {
                    "eventtype": "template_card_event",
                    "event_key": "approval:abc123:2",
                    "task_id": "approval_abc123"
                }
            }
        });
        let inbound = normalize_card_event("a1", &event).expect("card event");
        assert_eq!(inbound.chat_id, "u3");
        assert!(matches!(
            inbound.action,
            Some(InboundAction::ApprovalDecision {
                request_fingerprint,
                option_index: 2
            }) if request_fingerprint == "abc123"
        ));
        assert_eq!(
            inbound.card_message_id.as_deref(),
            Some("event-1|approval_abc123")
        );
    }

    #[test]
    fn normalizes_nested_template_card_event() {
        let event = json!({
            "cmd": "aibot_event_callback",
            "headers": { "req_id": "event-nested" },
            "body": {
                "msgid": "m-nested",
                "chattype": "single",
                "from": { "userid": "u4" },
                "event": {
                    "eventtype": "template_card_event",
                    "template_card_event": {
                        "event_key": "thread-choice:thread-route-9:resume_history",
                        "task_id": "route_thread_route_9_choice"
                    }
                }
            }
        });
        let inbound = normalize_card_event("a1", &event).expect("nested card event");
        assert!(matches!(
            inbound.action,
            Some(InboundAction::ThreadRouteChoice { request_id, action })
                if request_id == "thread-route-9" && action == "resume_history"
        ));
        assert_eq!(
            inbound.card_message_id.as_deref(),
            Some("event-nested|route_thread_route_9_choice")
        );
    }

    #[test]
    fn normalizes_history_selection_card_event() {
        let event = json!({
            "cmd": "aibot_event_callback",
            "headers": { "req_id": "event-select" },
            "body": {
                "msgid": "m-select",
                "chattype": "single",
                "from": { "userid": "u5" },
                "event": {
                    "eventtype": "template_card_event",
                    "template_card_event": {
                        "event_key": "thread-select:thread-route-10",
                        "task_id": "route_thread_route_10_list",
                        "selected_items": {
                            "selected_item": [{
                                "question_key": "thread_session",
                                "option_ids": { "option_id": ["019f-thread-id"] }
                            }]
                        }
                    }
                }
            }
        });
        let inbound = normalize_card_event("a1", &event).expect("selection card event");
        assert!(matches!(
            inbound.action,
            Some(InboundAction::ThreadRouteResumeSelected { request_id, thread_id })
                if request_id == "thread-route-10" && thread_id == "019f-thread-id"
        ));
    }

    #[test]
    fn normalizes_thread_create_form_card_event() {
        let event = json!({
            "cmd": "aibot_event_callback",
            "headers": { "req_id": "event-create" },
            "body": {
                "msgid": "m-create",
                "chattype": "single",
                "from": { "userid": "u6" },
                "event": {
                    "eventtype": "template_card_event",
                    "template_card_event": {
                        "event_key": "thread-create-submit:thread-route-12:full_access",
                        "task_id": "route_thread_route_12_create",
                        "selected_items": {
                            "selected_item": [
                                {
                                    "question_key": "cwd",
                                    "option_ids": { "option_id": ["D:/work/codexhub"] }
                                },
                                {
                                    "question_key": "model",
                                    "option_ids": { "option_id": ["gpt-5.6-sol"] }
                                },
                                {
                                    "question_key": "effort",
                                    "option_ids": { "option_id": ["xhigh"] }
                                }
                            ]
                        }
                    }
                }
            }
        });
        let inbound = normalize_card_event("a1", &event).expect("create form event");
        assert!(matches!(
            inbound.action,
            Some(InboundAction::ThreadRouteCreateSubmit {
                request_id,
                cwd_choice: Some(cwd),
                model: Some(model),
                effort: Some(effort),
                permission: Some(permission),
                ..
            }) if request_id == "thread-route-12"
                && cwd == "D:/work/codexhub"
                && model == "gpt-5.6-sol"
                && effort == "xhigh"
                && permission == "full_access"
        ));
    }

    #[test]
    fn normalizes_thread_create_custom_cwd_card_event() {
        let event = json!({
            "cmd": "aibot_event_callback",
            "headers": { "req_id": "event-custom-cwd" },
            "body": {
                "msgid": "m-custom-cwd",
                "chattype": "single",
                "from": { "userid": "u7" },
                "event": {
                    "eventtype": "template_card_event",
                    "template_card_event": {
                        "event_key": "thread-create-custom-cwd:thread-route-13:full_access",
                        "task_id": "route_thread_route_13_create",
                        "selected_items": {
                            "selected_item": [
                                {
                                    "question_key": "model",
                                    "option_ids": { "option_id": ["gpt-5.6-sol"] }
                                },
                                {
                                    "question_key": "effort",
                                    "option_ids": { "option_id": ["xhigh"] }
                                }
                            ]
                        }
                    }
                }
            }
        });
        let inbound = normalize_card_event("a1", &event).expect("custom cwd event");
        assert!(matches!(
            inbound.action,
            Some(InboundAction::ThreadRouteCreateSubmit {
                request_id,
                cwd_choice: Some(cwd),
                cwd_custom: None,
                model: Some(model),
                effort: Some(effort),
                permission: Some(permission),
            }) if request_id == "thread-route-13"
                && cwd == "__custom__"
                && model == "gpt-5.6-sol"
                && effort == "xhigh"
                && permission == "full_access"
        ));
    }

    #[test]
    fn normalizes_enter_chat_event() {
        let event = json!({
            "cmd": "aibot_event_callback",
            "headers": { "req_id": "event-enter" },
            "body": {
                "msgid": "m-enter",
                "chattype": "single",
                "from": { "userid": "noreply", "corpid": "external" },
                "msgtype": "event",
                "event": { "eventtype": "enter_chat" }
            }
        });
        let inbound = normalize_card_event("a1", &event).expect("enter chat event");
        assert_eq!(inbound.chat_id, "noreply");
        assert!(matches!(
            inbound.action,
            Some(InboundAction::ThreadRouteOpen)
        ));
        assert_eq!(inbound.callback_req_id.as_deref(), Some("event-enter"));
        assert_eq!(inbound.callback_kind, Some(InboundCallbackKind::Welcome));
    }

    #[test]
    fn parses_thread_routing_card_event_keys() {
        assert!(matches!(
            parse_card_event_key("thread-choice:thread-route-1:create_new"),
            Some(InboundAction::ThreadRouteChoice { request_id, action })
                if request_id == "thread-route-1" && action == "create_new"
        ));
        assert!(matches!(
            parse_card_event_key("thread-resume:thread-route-2:3:7"),
            Some(InboundAction::ThreadRouteResumeIndex { request_id, page: 3, index: 7 })
                if request_id == "thread-route-2"
        ));
        assert!(matches!(
            parse_card_event_key("thread-page:thread-route-3:prev"),
            Some(InboundAction::ThreadRouteListPage {
                request_id,
                direction: ThreadRouteDirection::Prev
            }) if request_id == "thread-route-3"
        ));
        assert!(matches!(
            parse_card_event_key("thread-page:thread-route-3:next"),
            Some(InboundAction::ThreadRouteListPage {
                request_id,
                direction: ThreadRouteDirection::Next
            }) if request_id == "thread-route-3"
        ));
        assert!(matches!(
            parse_card_event_key("thread-create-default:thread-route-4"),
            Some(InboundAction::ThreadRouteCreateDefault { request_id })
                if request_id == "thread-route-4"
        ));
        assert!(matches!(
            parse_card_event_key("thread-create-custom-cwd:thread-route-5"),
            Some(InboundAction::ThreadRouteCreateEdit { request_id, field })
                if request_id == "thread-route-5" && field == "cwd"
        ));
        assert!(parse_card_event_key("thread-page:thread-route-3:sideways").is_none());
    }

    #[test]
    fn recognizes_event_frames_by_body_shape() {
        assert!(is_event_frame(&json!({
            "cmd": "aibot_msg_callback",
            "body": {
                "msgtype": "event",
                "event": { "eventtype": "template_card_event" }
            }
        })));
        assert!(is_event_frame(&json!({
            "body": { "event": { "eventtype": "template_card_event" } }
        })));
        assert!(!is_event_frame(&json!({
            "cmd": "aibot_msg_callback",
            "body": { "msgtype": "text" }
        })));
    }
}

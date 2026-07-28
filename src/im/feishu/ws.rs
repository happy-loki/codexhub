use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prost::Message as ProstMessage;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
use tracing::{info, warn};

use crate::{
    app_state::{ImAccountRuntimeState, SharedState, im_account_key},
    types::{
        ChatType, ImPlatformKind, InboundAction, InboundAttachment, InboundMessage,
        ThreadRouteDirection, now_ms,
    },
};

use super::api::FeishuApi;

const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
static ATTACHMENT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, PartialEq, prost::Message)]
struct PbHeader {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(string, tag = "2")]
    value: String,
}

#[derive(Clone, PartialEq, prost::Message)]
struct PbFrame {
    #[prost(uint64, tag = "1")]
    seq_id: u64,
    #[prost(uint64, tag = "2")]
    log_id: u64,
    #[prost(int32, tag = "3")]
    service: i32,
    #[prost(int32, tag = "4")]
    method: i32,
    #[prost(message, repeated, tag = "5")]
    headers: Vec<PbHeader>,
    #[prost(bytes = "vec", optional, tag = "8")]
    payload: Option<Vec<u8>>,
}

impl PbFrame {
    fn header_value(&self, key: &str) -> &str {
        self.headers
            .iter()
            .find(|header| header.key == key)
            .map(|header| header.value.as_str())
            .unwrap_or("")
    }
}

#[derive(Debug, Deserialize)]
struct FeishuWsEvent {
    header: FeishuEventHeader,
    event: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct FeishuEventHeader {
    event_type: String,
}

#[derive(Debug, Deserialize)]
struct CardActionPayload {
    operator: CardActionOperator,
    action: CardAction,
    context: Option<CardActionContext>,
}

#[derive(Debug, Deserialize, Default)]
struct CardActionOperator {
    open_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct CardAction {
    #[serde(default)]
    value: serde_json::Value,
    #[serde(default)]
    form_value: serde_json::Value,
}

#[derive(Debug, Deserialize, Default)]
struct CardActionContext {
    open_chat_id: Option<String>,
    open_message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageReceivePayload {
    sender: EventSender,
    message: EventMessage,
}

#[derive(Debug, Deserialize)]
struct EventSender {
    sender_id: SenderId,
    #[serde(default)]
    sender_type: String,
}

#[derive(Debug, Deserialize, Default)]
struct SenderId {
    open_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventMessage {
    message_id: String,
    chat_id: String,
    chat_type: String,
    message_type: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    mentions: Vec<serde_json::Value>,
}

pub async fn listen_ws(
    state: SharedState,
    api: FeishuApi,
    account_id: String,
    attachment_root: PathBuf,
    tx: mpsc::Sender<InboundMessage>,
) -> Result<()> {
    let (wss_url, client_config) = api.get_ws_endpoint().await?;
    let service_id = service_id_from_ws_url(&wss_url);
    info!(
        "connecting feishu ws account={} url={} ping_interval={}",
        account_id,
        summarize_ws_target(&wss_url),
        client_config.ping_interval.unwrap_or_default()
    );

    let (stream, _) = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(&wss_url))
        .await
        .map_err(|_| anyhow!("feishu websocket connect timeout"))??;
    let (mut write, mut read) = stream.split();
    {
        let mut ws = state.feishu_ws.lock().await;
        ws.connecting = false;
        ws.connected = true;
        ws.last_error = None;
    }
    set_account_ws_state(&state, &account_id, false, true, None).await;
    state
        .push_event(
            "info",
            "feishu_ws_connected",
            format!(
                "account={account_id} service_url={}",
                summarize_ws_target(&wss_url)
            ),
        )
        .await;

    let mut ping_secs = client_config.ping_interval.unwrap_or(120).max(10);
    let mut hb_interval = tokio::time::interval(Duration::from_secs(ping_secs));
    let mut timeout_check = tokio::time::interval(Duration::from_secs(10));
    hb_interval.tick().await;
    let mut seq = 0_u64;
    let mut last_recv = Instant::now();

    send_ping(&mut write, service_id, &mut seq).await?;
    loop {
        tokio::select! {
            _ = hb_interval.tick() => {
                send_ping(&mut write, service_id, &mut seq).await?;
            }
            _ = timeout_check.tick() => {
                if last_recv.elapsed() > HEARTBEAT_TIMEOUT {
                    return Err(anyhow!("feishu websocket heartbeat timeout"));
                }
            }
            msg = read.next() => {
                let raw = match msg {
                    Some(Ok(WsMessage::Binary(data))) => {
                        last_recv = Instant::now();
                        data
                    }
                    Some(Ok(WsMessage::Text(text))) => {
                        last_recv = Instant::now();
                        if let Ok(event) = serde_json::from_str::<FeishuWsEvent>(&text) {
                            handle_event(&state, &account_id, &api, &attachment_root, event, tx.clone()).await?;
                        }
                        continue;
                    }
                    Some(Ok(WsMessage::Ping(data))) => {
                        last_recv = Instant::now();
                        write.send(WsMessage::Pong(data)).await?;
                        continue;
                    }
                    Some(Ok(WsMessage::Pong(_))) => {
                        last_recv = Instant::now();
                        continue;
                    }
                    Some(Ok(WsMessage::Close(frame))) => {
                        warn!("feishu websocket closed: {:?}", frame);
                        break;
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => return Err(anyhow!(err)),
                    None => break,
                };

                let frame = PbFrame::decode(&raw[..])?;
                if frame.method == 0 {
                    if frame.header_value("type") == "pong"
                        && let Some(payload) = &frame.payload
                        && let Ok(cfg) = serde_json::from_slice::<super::api::WsClientConfig>(payload)
                        && let Some(secs) = cfg.ping_interval
                    {
                        let secs = secs.max(10);
                        if secs != ping_secs {
                            ping_secs = secs;
                            hb_interval = tokio::time::interval(Duration::from_secs(ping_secs));
                        }
                    }
                    continue;
                }
                if frame.header_value("type") != "event" {
                    continue;
                }

                let mut ack = frame.clone();
                ack.payload = Some(br#"{"code":200,"headers":{},"data":[]}"#.to_vec());
                ack.headers.push(PbHeader {
                    key: "biz_rt".to_string(),
                    value: "0".to_string(),
                });
                let _ = write.send(WsMessage::Binary(ack.encode_to_vec().into())).await;

                let payload = frame.payload.unwrap_or_default();
                match serde_json::from_slice::<FeishuWsEvent>(&payload) {
                    Ok(event) => handle_event(&state, &account_id, &api, &attachment_root, event, tx.clone()).await?,
                    Err(err) => {
                        state
                            .push_event(
                                "warn",
                                "feishu_ws_event_parse_failed",
                                format!("account={account_id} err={err}"),
                            )
                            .await;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn send_ping<S>(write: &mut S, service_id: i32, seq: &mut u64) -> Result<()>
where
    S: futures_util::Sink<WsMessage> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    *seq = seq.wrapping_add(1);
    let ping = PbFrame {
        seq_id: *seq,
        log_id: 0,
        service: service_id,
        method: 0,
        headers: vec![PbHeader {
            key: "type".to_string(),
            value: "ping".to_string(),
        }],
        payload: None,
    };
    write
        .send(WsMessage::Binary(ping.encode_to_vec().into()))
        .await?;
    Ok(())
}

async fn handle_event(
    state: &SharedState,
    account_id: &str,
    api: &FeishuApi,
    attachment_root: &Path,
    event: FeishuWsEvent,
    tx: mpsc::Sender<InboundMessage>,
) -> Result<()> {
    if event.header.event_type == "card.action.trigger" {
        return handle_card_action_event(state, account_id, event.event, tx).await;
    }

    if event.header.event_type != "im.message.receive_v1" {
        state
            .push_event(
                "info",
                "feishu_ws_event_unhandled",
                format!(
                    "account={} event_type={}",
                    account_id, event.header.event_type
                ),
            )
            .await;
        return Ok(());
    }

    let receive: MessageReceivePayload = serde_json::from_value(event.event)?;
    if receive.sender.sender_type == "app" || receive.sender.sender_type == "bot" {
        return Ok(());
    }
    let sender_id = receive
        .sender
        .sender_id
        .open_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    state
        .push_event(
            "info",
            "feishu_event_received",
            format!(
                "chat={} sender={} type={}",
                receive.message.chat_id, sender_id, receive.message.message_type
            ),
        )
        .await;

    let settings = state
        .config
        .lock()
        .await
        .feishu_account(account_id)
        .unwrap_or_default();
    if !settings.allowed_open_ids.is_empty() && !settings.allowed_open_ids.contains(&sender_id) {
        state
            .push_event(
                "warn",
                "feishu_message_ignored",
                format!("sender not allowed: {sender_id}"),
            )
            .await;
        return Ok(());
    }
    if !settings.allowed_chat_ids.is_empty()
        && !settings.allowed_chat_ids.contains(&receive.message.chat_id)
    {
        state
            .push_event(
                "warn",
                "feishu_message_ignored",
                format!("chat not allowed: {}", receive.message.chat_id),
            )
            .await;
        return Ok(());
    }

    let text = extract_text_content(&receive.message.content, &receive.message.message_type);
    let attachments = collect_attachments(api, attachment_root, &receive.message).await;
    if text.trim().is_empty() && attachments.is_empty() {
        state
            .push_event(
                "warn",
                "feishu_message_ignored",
                "empty text and no attachment",
            )
            .await;
        return Ok(());
    }

    let mentioned = receive.message.chat_type == "p2p" || !receive.message.mentions.is_empty();
    if receive.message.chat_type != "p2p" && settings.mention_only && !mentioned {
        state
            .push_event(
                "warn",
                "feishu_message_ignored",
                format!("group message without mention: {}", receive.message.chat_id),
            )
            .await;
        return Ok(());
    }

    update_last_inbound(state, account_id).await;
    tx.send(InboundMessage {
        platform: ImPlatformKind::Feishu,
        account_id: account_id.to_string(),
        sender_id,
        chat_id: receive.message.chat_id,
        chat_type: if receive.message.chat_type == "p2p" {
            ChatType::Direct
        } else {
            ChatType::Group
        },
        message_id: receive.message.message_id,
        received_at_ms: now_ms(),
        text,
        mentioned,
        approval_request_key: None,
        action: None,
        card_message_id: None,
        callback_req_id: None,
        callback_kind: None,
        attachments,
    })
    .await
    .map_err(|_| anyhow!("feishu inbound pump closed"))
}

async fn handle_card_action_event(
    state: &SharedState,
    account_id: &str,
    event: serde_json::Value,
    tx: mpsc::Sender<InboundMessage>,
) -> Result<()> {
    let payload: CardActionPayload = serde_json::from_value(event)?;
    let sender_id = payload
        .operator
        .open_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let settings = state
        .config
        .lock()
        .await
        .feishu_account(account_id)
        .unwrap_or_default();
    if !settings.allowed_open_ids.is_empty() && !settings.allowed_open_ids.contains(&sender_id) {
        state
            .push_event(
                "warn",
                "feishu_card_action_ignored",
                format!("sender not allowed: {sender_id}"),
            )
            .await;
        return Ok(());
    }

    let Some(value) = payload.action.value.as_object() else {
        state
            .push_event("warn", "feishu_card_action_ignored", "missing action.value")
            .await;
        return Ok(());
    };
    let chat_id = payload
        .context
        .as_ref()
        .and_then(|context| context.open_chat_id.clone())
        .ok_or_else(|| anyhow!("card action missing context.open_chat_id"))?;
    if !settings.allowed_chat_ids.is_empty() && !settings.allowed_chat_ids.contains(&chat_id) {
        state
            .push_event(
                "warn",
                "feishu_card_action_ignored",
                format!("chat not allowed: {chat_id}"),
            )
            .await;
        return Ok(());
    }
    state
        .push_event(
            "info",
            "feishu_card_action_received",
            format!(
                "chat={} sender={} kind={} message={}",
                chat_id,
                sender_id,
                value
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
                payload
                    .context
                    .as_ref()
                    .and_then(|context| context.open_message_id.as_deref())
                    .unwrap_or_default()
            ),
        )
        .await;
    let kind = value
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let (text, approval_request_key, action) = match kind {
        "codex_approval_decision" => {
            let option = value
                .get("option")
                .and_then(|v| v.as_u64())
                .filter(|value| *value > 0)
                .unwrap_or(1);
            let request_key = value
                .get("requestKey")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            (format!("/{option}"), request_key, None)
        }
        "thread_route_choice" => {
            let request_id = value
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("thread route choice action missing requestId"))?
                .to_string();
            let route_action = value
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("thread route choice action missing action"))?
                .to_string();
            (
                String::new(),
                None,
                Some(InboundAction::ThreadRouteChoice {
                    request_id,
                    action: route_action,
                }),
            )
        }
        "thread_route_create_submit" => {
            let request_id = value
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("thread route create action missing requestId"))?
                .to_string();
            (
                String::new(),
                None,
                Some(InboundAction::ThreadRouteCreateSubmit {
                    request_id,
                    cwd_choice: form_string(&payload.action.form_value, "cwd_choice")
                        .or_else(|| form_string(&payload.action.form_value, "cwd")),
                    cwd_custom: form_string(&payload.action.form_value, "cwd_custom"),
                    model: form_string(&payload.action.form_value, "model"),
                    effort: form_string(&payload.action.form_value, "effort"),
                    permission: form_string(&payload.action.form_value, "permission"),
                }),
            )
        }
        "thread_route_create_default" => {
            let request_id = value
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("thread route create action missing requestId"))?
                .to_string();
            (
                String::new(),
                None,
                Some(InboundAction::ThreadRouteCreateDefault { request_id }),
            )
        }
        "thread_route_resume_selected" => {
            let request_id = value
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("thread route action missing requestId"))?
                .to_string();
            let thread_id = value
                .get("threadId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("thread route action missing threadId"))?
                .to_string();
            (
                String::new(),
                None,
                Some(InboundAction::ThreadRouteResumeSelected {
                    request_id,
                    thread_id,
                }),
            )
        }
        "thread_route_list_page" => {
            let request_id = value
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("thread route page action missing requestId"))?
                .to_string();
            let direction = match value
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
            {
                "prev" => ThreadRouteDirection::Prev,
                "next" => ThreadRouteDirection::Next,
                other => return Err(anyhow!("unsupported thread route page direction: {other}")),
            };
            (
                String::new(),
                None,
                Some(InboundAction::ThreadRouteListPage {
                    request_id,
                    direction,
                }),
            )
        }
        _ => {
            state
                .push_event(
                    "info",
                    "feishu_card_action_unhandled",
                    format!("value={}", serde_json::Value::Object(value.clone())),
                )
                .await;
            return Ok(());
        }
    };
    update_last_inbound(state, account_id).await;
    tx.send(InboundMessage {
        platform: ImPlatformKind::Feishu,
        account_id: account_id.to_string(),
        sender_id,
        chat_id,
        chat_type: ChatType::Direct,
        message_id: payload
            .context
            .as_ref()
            .and_then(|context| context.open_message_id.clone())
            .unwrap_or_else(|| format!("card-action-{kind}")),
        received_at_ms: now_ms(),
        text,
        mentioned: true,
        approval_request_key,
        action,
        card_message_id: payload
            .context
            .as_ref()
            .and_then(|context| context.open_message_id.clone()),
        callback_req_id: None,
        callback_kind: None,
        attachments: vec![],
    })
    .await
    .map_err(|_| anyhow!("feishu inbound pump closed"))
}

fn form_string(form_value: &serde_json::Value, name: &str) -> Option<String> {
    form_value
        .get(name)
        .and_then(first_form_string)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn first_form_string(value: &serde_json::Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        return Some(value.to_string());
    }
    if let Some(value) = value.get("value").and_then(|value| value.as_str()) {
        return Some(value.to_string());
    }
    if let Some(value) = value
        .get("selected_option")
        .and_then(|option| option.get("value"))
        .and_then(|value| value.as_str())
    {
        return Some(value.to_string());
    }
    value
        .as_array()
        .and_then(|items| items.iter().find_map(first_form_string))
}

pub(crate) async fn set_account_ws_state(
    state: &SharedState,
    account_id: &str,
    connecting: bool,
    connected: bool,
    last_error: Option<String>,
) {
    let now = now_ms();
    let key = im_account_key(ImPlatformKind::Feishu, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Feishu, account_id));
    entry.connecting = connecting;
    entry.polling = false;
    entry.connected = connected;
    entry.last_error = last_error;
    entry.last_event_at_ms = Some(now);
}

async fn update_last_inbound(state: &SharedState, account_id: &str) {
    let now = now_ms();
    let key = im_account_key(ImPlatformKind::Feishu, account_id);
    let mut accounts = state.im_accounts.lock().await;
    let entry = accounts
        .entry(key)
        .or_insert_with(|| ImAccountRuntimeState::new(ImPlatformKind::Feishu, account_id));
    entry.last_event_at_ms = Some(now);
    entry.last_inbound_at_ms = Some(now);
}

fn service_id_from_ws_url(wss_url: &str) -> i32 {
    wss_url
        .split('?')
        .nth(1)
        .and_then(|qs| {
            qs.split('&')
                .find(|kv| kv.starts_with("service_id="))
                .and_then(|kv| kv.split('=').nth(1))
                .and_then(|v| v.parse::<i32>().ok())
        })
        .unwrap_or(0)
}

fn summarize_ws_target(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(url) => {
            let host = url.host_str().unwrap_or_default();
            let path = url.path();
            format!("host={host} path={path}")
        }
        Err(_) => raw.to_string(),
    }
}

fn extract_text_content(raw_content: &str, message_type: &str) -> String {
    if raw_content.trim().is_empty() {
        return String::new();
    }
    match message_type {
        "text" => serde_json::from_str::<serde_json::Value>(raw_content)
            .ok()
            .and_then(|v| v.get("text").and_then(|v| v.as_str()).map(str::to_string))
            .unwrap_or_else(|| raw_content.to_string()),
        "image" | "file" | "audio" | "video" | "media" | "sticker" => String::new(),
        _ => raw_content.to_string(),
    }
}

async fn collect_attachments(
    api: &FeishuApi,
    attachment_root: &Path,
    message: &EventMessage,
) -> Vec<InboundAttachment> {
    let mut attachments = Vec::new();
    let (image_key, file_key, file_name) =
        parse_media_keys(&message.content, &message.message_type);
    if message.message_type == "image" {
        if let Some(image_key) = image_key {
            match api.download_image(&message.message_id, &image_key).await {
                Ok((bytes, content_type)) => {
                    let preferred_name = format!("image.{}", extension_for_mime(&content_type));
                    match persist_inbound_attachment_bytes(
                        attachment_root,
                        AttachmentKind::Image,
                        &preferred_name,
                        &bytes,
                        Some(content_type),
                        "image",
                        None,
                    )
                    .await
                    {
                        Ok(attachment) => attachments.push(attachment),
                        Err(err) => warn!("failed to persist feishu image attachment: {}", err),
                    }
                }
                Err(err) => warn!("failed to download feishu image attachment: {}", err),
            }
        }
    } else if matches!(
        message.message_type.as_str(),
        "file" | "audio" | "video" | "media" | "sticker"
    ) && let Some(file_key) = file_key
    {
        let file_name = file_name.unwrap_or_else(|| "attachment.bin".to_string());
        match api.download_file(&message.message_id, &file_key).await {
            Ok(bytes) => {
                let attachment_kind = if matches!(message.message_type.as_str(), "video" | "media")
                {
                    AttachmentKind::Video
                } else {
                    AttachmentKind::File
                };
                let attachment_label = if matches!(message.message_type.as_str(), "video" | "media")
                {
                    "video"
                } else {
                    "file"
                };
                match persist_inbound_attachment_bytes(
                    attachment_root,
                    attachment_kind,
                    &file_name,
                    &bytes,
                    None,
                    attachment_label,
                    Some(file_name.clone()),
                )
                .await
                {
                    Ok(attachment) => attachments.push(attachment),
                    Err(err) => warn!("failed to persist feishu file attachment: {}", err),
                }
            }
            Err(err) => warn!("failed to download feishu file attachment: {}", err),
        }
    }
    attachments
}

fn parse_media_keys(
    raw_content: &str,
    message_type: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw_content) else {
        return (None, None, None);
    };
    let image_key = parsed
        .get("image_key")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let file_key = parsed
        .get("file_key")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let file_name = parsed
        .get("file_name")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    match message_type {
        "image" => (image_key, None, None),
        "file" => (
            None,
            file_key,
            file_name.or(Some("attachment.bin".to_string())),
        ),
        "audio" => (None, file_key, file_name.or(Some("audio.bin".to_string()))),
        "video" | "media" => (
            image_key,
            file_key,
            file_name.or(Some("media.bin".to_string())),
        ),
        "sticker" => (
            None,
            file_key,
            file_name.or(Some("sticker.bin".to_string())),
        ),
        _ => (None, None, None),
    }
}

#[derive(Debug, Clone, Copy)]
enum AttachmentKind {
    Image,
    File,
    Video,
}

impl AttachmentKind {
    fn as_dir(self) -> &'static str {
        match self {
            Self::Image => "images",
            Self::File => "files",
            Self::Video => "videos",
        }
    }
}

async fn persist_inbound_attachment_bytes(
    root: &Path,
    storage_kind: AttachmentKind,
    preferred_name: &str,
    bytes: &[u8],
    mime_type: Option<String>,
    attachment_kind: &str,
    display_name: Option<String>,
) -> Result<InboundAttachment> {
    let dir = root.join("feishu").join(storage_kind.as_dir());
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("failed to create attachment dir {}", dir.display()))?;
    let path = dir.join(build_attachment_file_name(preferred_name));
    tokio::fs::write(&path, bytes)
        .await
        .with_context(|| format!("failed to write attachment {}", path.display()))?;
    Ok(InboundAttachment {
        kind: attachment_kind.to_string(),
        name: display_name.or_else(|| {
            path.file_name()
                .and_then(|v| v.to_str())
                .map(str::to_string)
        }),
        mime_type,
        text_hint: None,
        local_path: Some(path.to_string_lossy().to_string()),
    })
}

fn build_attachment_file_name(preferred_name: &str) -> String {
    let source = preferred_name.trim();
    let sanitized = sanitize_file_name(if source.is_empty() {
        "attachment.bin"
    } else {
        source
    });
    let path = Path::new(&sanitized);
    let stem = path
        .file_stem()
        .and_then(|v| v.to_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("attachment");
    match path
        .extension()
        .and_then(|v| v.to_str())
        .filter(|v| !v.trim().is_empty())
    {
        Some(ext) => format!("{}-{}.{}", stem, attachment_suffix(), ext),
        None => format!("{}-{}", stem, attachment_suffix()),
    }
}

fn attachment_suffix() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or_default();
    let seq = ATTACHMENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{millis:x}-{seq:x}")
}

fn sanitize_file_name(name: &str) -> String {
    name.chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

fn extension_for_mime(content_type: &str) -> &'static str {
    if content_type.contains("jpeg") {
        "jpg"
    } else if content_type.contains("webp") {
        "webp"
    } else if content_type.contains("gif") {
        "gif"
    } else {
        "png"
    }
}

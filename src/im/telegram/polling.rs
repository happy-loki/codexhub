use anyhow::Result;
use sha2::Digest;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

use crate::{
    app_state::{ImAccountRuntimeState, SharedState, im_account_key},
    chain_log,
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
    attachment_root: PathBuf,
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
                        if let Some(mut inbound) = inbound_from_message(
                            api.settings(),
                            &chat_access.allowed_chat_ids,
                            &message,
                        ) {
                            let collection =
                                collect_telegram_attachments(&api, &attachment_root, &message)
                                    .await;
                            let media_not_delivered =
                                message_has_media(&message) && collection.attachments.is_empty();
                            if !collection.failures.is_empty() {
                                let notice = attachment_failure_notice(
                                    &collection.failures,
                                    media_not_delivered && !inbound.text.trim().is_empty(),
                                );
                                if let Err(err) = api.send_text(&inbound.chat_id, &notice).await {
                                    chain_log::write_diagnostic_lazy(|| {
                                        format!(
                                            "[telegram_attachment] event=failure_notice_failed message={} chat={} err={}",
                                            message.message_id, inbound.chat_id, err
                                        )
                                    });
                                }
                            }
                            inbound.attachments = collection.attachments;
                            if media_not_delivered {
                                update_last_inbound(&state, &account_id).await;
                                continue;
                            }
                            if inbound.text.trim().is_empty() && inbound.attachments.is_empty() {
                                continue;
                            }
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
                                "当前 Telegram 私聊未授权。请在本机 CodexHub 配置 allowedChatIds。",
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
    message: &TelegramMessage,
) -> Option<InboundMessage> {
    if message.chat.kind != "private" {
        return None;
    }
    let text = message
        .text
        .as_deref()
        .or(message.caption.as_deref())
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() && !message_has_media(message) {
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
        callback_req_id: None,
        callback_kind: None,
        attachments: vec![],
    })
}

const TELEGRAM_MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const TELEGRAM_MAX_ATTACHMENTS_PER_MESSAGE: usize = 8;

fn message_has_media(message: &TelegramMessage) -> bool {
    message.photo.is_some()
        || message.document.is_some()
        || message.audio.is_some()
        || message.video.is_some()
        || message.animation.is_some()
        || message.voice.is_some()
        || message.video_note.is_some()
        || message.sticker.is_some()
}

#[derive(Debug, Clone)]
struct TelegramAttachmentSpec {
    file_id: String,
    kind: &'static str,
    directory: &'static str,
    name: String,
    mime_type: Option<String>,
    file_size: Option<u64>,
}

fn attachment_specs(message: &TelegramMessage) -> Vec<TelegramAttachmentSpec> {
    let mut specs = Vec::new();
    if let Some(photo) = message.photo.as_ref().and_then(|photos| {
        photos
            .iter()
            .max_by_key(|photo| u64::from(photo.width) * u64::from(photo.height))
    }) {
        specs.push(TelegramAttachmentSpec {
            file_id: photo.file_id.clone(),
            kind: "image",
            directory: "images",
            name: format!("telegram-{}.jpg", message.message_id),
            mime_type: Some("image/jpeg".to_string()),
            file_size: photo.file_size,
        });
    }
    if let Some(audio) = message.audio.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: audio.file_id.clone(),
            kind: "audio",
            directory: "files",
            name: audio
                .file_name
                .clone()
                .unwrap_or_else(|| format!("telegram-{}-audio.bin", message.message_id)),
            mime_type: audio.mime_type.clone(),
            file_size: audio.file_size,
        });
    }
    if let Some(video) = message.video.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: video.file_id.clone(),
            kind: "video",
            directory: "videos",
            name: format!("telegram-{}.mp4", message.message_id),
            mime_type: video.mime_type.clone(),
            file_size: video.file_size,
        });
    }
    if let Some(animation) = message.animation.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: animation.file_id.clone(),
            kind: "video",
            directory: "videos",
            name: animation
                .file_name
                .clone()
                .unwrap_or_else(|| format!("telegram-{}-animation.mp4", message.message_id)),
            mime_type: animation.mime_type.clone(),
            file_size: animation.file_size,
        });
    }
    if let Some(voice) = message.voice.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: voice.file_id.clone(),
            kind: "audio",
            directory: "files",
            name: format!("telegram-{}-voice.ogg", message.message_id),
            mime_type: voice.mime_type.clone(),
            file_size: voice.file_size,
        });
    }
    if let Some(video_note) = message.video_note.as_ref() {
        specs.push(TelegramAttachmentSpec {
            file_id: video_note.file_id.clone(),
            kind: "video",
            directory: "videos",
            name: format!("telegram-{}-video-note.mp4", message.message_id),
            mime_type: Some("video/mp4".to_string()),
            file_size: video_note.file_size,
        });
    }
    if let Some(sticker) = message.sticker.as_ref() {
        let (kind, extension, mime_type) = if sticker.is_animated {
            ("file", "tgs", "application/x-tgsticker")
        } else if sticker.is_video {
            ("video", "webm", "video/webm")
        } else {
            ("image", "webp", "image/webp")
        };
        specs.push(TelegramAttachmentSpec {
            file_id: sticker.file_id.clone(),
            kind,
            directory: match kind {
                "image" => "images",
                "video" => "videos",
                _ => "files",
            },
            name: format!("telegram-{}-sticker.{extension}", message.message_id),
            mime_type: Some(mime_type.to_string()),
            file_size: sticker.file_size,
        });
    }
    // Telegram includes a compatibility `document` alongside some dedicated
    // media fields, notably `animation`. Add the generic representation last
    // so de-duplication preserves the richer media kind and filename.
    if let Some(document) = message.document.as_ref() {
        let is_image = document
            .mime_type
            .as_deref()
            .is_some_and(|mime_type| mime_type.starts_with("image/"))
            || document.file_name.as_deref().is_some_and(|file_name| {
                mime_guess::from_path(file_name)
                    .first()
                    .is_some_and(|mime_type| mime_type.essence_str().starts_with("image/"))
            });
        specs.push(TelegramAttachmentSpec {
            file_id: document.file_id.clone(),
            kind: if is_image { "image" } else { "file" },
            directory: if is_image { "images" } else { "files" },
            name: document
                .file_name
                .clone()
                .unwrap_or_else(|| format!("telegram-{}-document.bin", message.message_id)),
            mime_type: document.mime_type.clone(),
            file_size: document.file_size,
        });
    }
    let mut seen_file_ids = HashSet::new();
    specs.retain(|spec| seen_file_ids.insert(spec.file_id.clone()));
    specs.truncate(TELEGRAM_MAX_ATTACHMENTS_PER_MESSAGE);
    specs
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TelegramAttachmentFailureReason {
    TooLarge { bytes: Option<u64> },
    MetadataUnavailable,
    DownloadFailed,
    PersistFailed,
}

impl TelegramAttachmentFailureReason {
    fn user_description(&self) -> String {
        match self {
            Self::TooLarge { bytes } => bytes.map_or_else(
                || "超过 Telegram Bot 的 20 MB 下载上限".to_string(),
                |bytes| {
                    format!(
                        "大小为 {:.1} MB，超过 Telegram Bot 的 20 MB 下载上限",
                        bytes as f64 / (1024.0 * 1024.0)
                    )
                },
            ),
            Self::MetadataUnavailable => "无法读取 Telegram 文件信息".to_string(),
            Self::DownloadFailed => "从 Telegram 下载失败".to_string(),
            Self::PersistFailed => "下载后无法保存到本机".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TelegramAttachmentFailure {
    name: String,
    reason: TelegramAttachmentFailureReason,
}

#[derive(Debug, Default)]
struct TelegramAttachmentCollection {
    attachments: Vec<crate::types::InboundAttachment>,
    failures: Vec<TelegramAttachmentFailure>,
}

fn attachment_failure(
    spec: &TelegramAttachmentSpec,
    reason: TelegramAttachmentFailureReason,
) -> TelegramAttachmentFailure {
    TelegramAttachmentFailure {
        name: spec.name.clone(),
        reason,
    }
}

fn attachment_failure_notice(
    failures: &[TelegramAttachmentFailure],
    caption_was_blocked: bool,
) -> String {
    let mut lines = vec!["附件未成功交给 Agent：".to_string()];
    lines.extend(failures.iter().map(|failure| {
        let name = failure
            .name
            .chars()
            .filter(|ch| !ch.is_control())
            .take(120)
            .collect::<String>();
        format!("- {name}：{}", failure.reason.user_description())
    }));
    if caption_was_blocked {
        lines.push("为避免 Agent 在缺少附件时误处理，这条消息的说明文字也没有提交。".to_string());
    }
    lines.push("请修正后重新发送。".to_string());
    lines.join("\n")
}

async fn collect_telegram_attachments(
    api: &TelegramApi,
    attachment_root: &Path,
    message: &TelegramMessage,
) -> TelegramAttachmentCollection {
    let mut collection = TelegramAttachmentCollection::default();
    let specs = attachment_specs(message);
    if specs.is_empty() && message_has_media(message) {
        collection.failures.push(TelegramAttachmentFailure {
            name: "Telegram 附件".to_string(),
            reason: TelegramAttachmentFailureReason::MetadataUnavailable,
        });
        return collection;
    }
    for spec in specs {
        if spec
            .file_size
            .is_some_and(|size| size > TELEGRAM_MAX_FILE_BYTES)
        {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=download_skipped message={} file_id={} reason=size_limit size={}",
                    message.message_id,
                    spec.file_id,
                    spec.file_size.unwrap_or_default()
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::TooLarge {
                    bytes: spec.file_size,
                },
            ));
            continue;
        }
        let file = match api.get_file(&spec.file_id).await {
            Ok(file) => file,
            Err(err) => {
                chain_log::write_diagnostic_lazy(|| {
                    format!(
                        "[telegram_attachment] event=get_file_failed message={} file_id={} err={}",
                        message.message_id, spec.file_id, err
                    )
                });
                collection.failures.push(attachment_failure(
                    &spec,
                    TelegramAttachmentFailureReason::MetadataUnavailable,
                ));
                continue;
            }
        };
        if file
            .file_size
            .is_some_and(|size| size > TELEGRAM_MAX_FILE_BYTES)
        {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=download_skipped message={} file_id={} reason=size_limit size={}",
                    message.message_id,
                    spec.file_id,
                    file.file_size.unwrap_or_default()
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::TooLarge {
                    bytes: file.file_size,
                },
            ));
            continue;
        }
        let Some(file_path) = file.file_path.as_deref() else {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=get_file_missing_path message={} file_id={}",
                    message.message_id, spec.file_id
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::MetadataUnavailable,
            ));
            continue;
        };
        let bytes = match api.download_file(file_path).await {
            Ok(bytes) if (bytes.len() as u64) <= TELEGRAM_MAX_FILE_BYTES => bytes,
            Ok(bytes) => {
                chain_log::write_diagnostic_lazy(|| {
                    format!(
                        "[telegram_attachment] event=download_skipped message={} file_id={} reason=size_limit size={}",
                        message.message_id,
                        spec.file_id,
                        bytes.len()
                    )
                });
                collection.failures.push(attachment_failure(
                    &spec,
                    TelegramAttachmentFailureReason::TooLarge {
                        bytes: Some(bytes.len() as u64),
                    },
                ));
                continue;
            }
            Err(err) => {
                chain_log::write_diagnostic_lazy(|| {
                    format!(
                        "[telegram_attachment] event=download_failed message={} file_id={} err={}",
                        message.message_id, spec.file_id, err
                    )
                });
                collection.failures.push(attachment_failure(
                    &spec,
                    TelegramAttachmentFailureReason::DownloadFailed,
                ));
                continue;
            }
        };
        let account_directory = sanitized_path_component(&api.settings().account_id(), "default");
        let dir = attachment_root
            .join("telegram")
            .join(account_directory)
            .join(spec.directory);
        if let Err(err) = tokio::fs::create_dir_all(&dir).await {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=directory_failed path={} err={}",
                    dir.display(),
                    err
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::PersistFailed,
            ));
            continue;
        }
        let file_name = unique_attachment_name(&spec.name, &spec.file_id);
        let path = dir.join(file_name);
        if let Err(err) = tokio::fs::write(&path, &bytes).await {
            chain_log::write_diagnostic_lazy(|| {
                format!(
                    "[telegram_attachment] event=persist_failed path={} err={}",
                    path.display(),
                    err
                )
            });
            collection.failures.push(attachment_failure(
                &spec,
                TelegramAttachmentFailureReason::PersistFailed,
            ));
            continue;
        }
        collection
            .attachments
            .push(crate::types::InboundAttachment {
                kind: spec.kind.to_string(),
                name: Some(spec.name),
                mime_type: spec
                    .mime_type
                    .or_else(|| mime_guess::from_path(&path).first().map(|m| m.to_string())),
                text_hint: None,
                local_path: Some(path.to_string_lossy().to_string()),
            });
    }
    collection
}

fn unique_attachment_name(name: &str, file_id: &str) -> String {
    let sanitized = sanitized_path_component(name, "attachment.bin");
    let digest = sha2::Sha256::digest(file_id.as_bytes());
    let suffix = hex::encode(digest);
    if let Some((stem, extension)) = sanitized.rsplit_once('.') {
        let stem = stem.chars().take(120).collect::<String>();
        let extension = extension.chars().take(16).collect::<String>();
        format!("{stem}-{suffix}.{extension}")
    } else {
        let sanitized = sanitized.chars().take(120).collect::<String>();
        format!("{sanitized}-{suffix}")
    }
}

fn sanitized_path_component(value: &str, fallback: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        fallback.to_string()
    } else {
        sanitized
    }
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
        callback_req_id: None,
        callback_kind: None,
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
    use crate::im::telegram::api::{
        TelegramChat, TelegramMediaFile, TelegramPhotoSize, TelegramStickerFile, TelegramUser,
    };

    #[test]
    fn converts_private_message_to_inbound() {
        let settings = TelegramSettings::default();
        let inbound = inbound_from_message(
            &settings,
            &["42".to_string()],
            &TelegramMessage {
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
                ..TelegramMessage::default()
            },
        )
        .expect("inbound message");

        assert_eq!(inbound.platform, ImPlatformKind::Telegram);
        assert_eq!(inbound.conversation_key(), "telegram:telegram:42");
        assert_eq!(inbound.chat_type, ChatType::Direct);
        assert_eq!(inbound.text, "/status");
    }

    #[test]
    fn accepts_photo_without_text_and_selects_largest_size() {
        let settings = TelegramSettings::default();
        let message = TelegramMessage {
            message_id: 12,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            photo: Some(vec![
                TelegramPhotoSize {
                    file_id: "wide".to_string(),
                    file_unique_id: "wide-unique".to_string(),
                    width: 1280,
                    height: 720,
                    file_size: Some(1_000),
                },
                TelegramPhotoSize {
                    file_id: "large".to_string(),
                    file_unique_id: "large-unique".to_string(),
                    width: 1024,
                    height: 1024,
                    file_size: Some(10_000),
                },
            ]),
            ..TelegramMessage::default()
        };

        let inbound = inbound_from_message(&settings, &["42".to_string()], &message)
            .expect("photo message should be accepted");
        let specs = attachment_specs(&message);

        assert!(inbound.text.is_empty());
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].file_id, "large");
        assert_eq!(specs[0].kind, "image");
    }

    #[test]
    fn treats_image_documents_as_visual_input() {
        let message = TelegramMessage {
            message_id: 14,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            document: Some(TelegramMediaFile {
                file_id: "image-document".to_string(),
                file_unique_id: "image-document-unique".to_string(),
                file_size: Some(8_192),
                file_name: Some("diagram.png".to_string()),
                mime_type: Some("image/png".to_string()),
            }),
            ..TelegramMessage::default()
        };

        let specs = attachment_specs(&message);

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].kind, "image");
        assert_eq!(specs[0].directory, "images");
    }

    #[test]
    fn deduplicates_animation_compatibility_document_by_file_id() {
        let message = TelegramMessage {
            message_id: 15,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            animation: Some(TelegramMediaFile {
                file_id: "shared-animation-file".to_string(),
                file_unique_id: "shared-animation-unique".to_string(),
                file_size: Some(8_192),
                file_name: Some("demo.mp4".to_string()),
                mime_type: Some("video/mp4".to_string()),
            }),
            document: Some(TelegramMediaFile {
                file_id: "shared-animation-file".to_string(),
                file_unique_id: "shared-animation-unique".to_string(),
                file_size: Some(8_192),
                file_name: Some("compatibility.mp4".to_string()),
                mime_type: Some("video/mp4".to_string()),
            }),
            ..TelegramMessage::default()
        };

        let specs = attachment_specs(&message);

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].file_id, "shared-animation-file");
        assert_eq!(specs[0].kind, "video");
        assert_eq!(specs[0].directory, "videos");
        assert_eq!(specs[0].name, "demo.mp4");
    }

    #[test]
    fn preserves_each_telegram_sticker_format() {
        let cases = [
            (false, false, "image", "images", ".webp", "image/webp"),
            (
                true,
                false,
                "file",
                "files",
                ".tgs",
                "application/x-tgsticker",
            ),
            (false, true, "video", "videos", ".webm", "video/webm"),
        ];

        for (is_animated, is_video, kind, directory, extension, mime_type) in cases {
            let message = TelegramMessage {
                message_id: 18,
                chat: TelegramChat {
                    id: 42,
                    kind: "private".to_string(),
                    ..TelegramChat::default()
                },
                sticker: Some(TelegramStickerFile {
                    file_id: format!("sticker-{extension}"),
                    file_unique_id: format!("sticker-unique-{extension}"),
                    file_size: Some(4_096),
                    is_animated,
                    is_video,
                }),
                ..TelegramMessage::default()
            };

            let specs = attachment_specs(&message);

            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0].kind, kind);
            assert_eq!(specs[0].directory, directory);
            assert!(specs[0].name.ends_with(extension));
            assert_eq!(specs[0].mime_type.as_deref(), Some(mime_type));
        }
    }

    #[tokio::test]
    async fn oversized_attachment_returns_user_visible_failure_without_network() {
        let api = TelegramApi::new(TelegramSettings {
            account_id: "test".to_string(),
            bot_token: String::new(),
            allowed_chat_ids: vec!["42".to_string()],
            ..TelegramSettings::default()
        });
        let message = TelegramMessage {
            message_id: 16,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            caption: Some("review this archive".to_string()),
            document: Some(TelegramMediaFile {
                file_id: "oversized-file".to_string(),
                file_unique_id: "oversized-unique".to_string(),
                file_size: Some(TELEGRAM_MAX_FILE_BYTES + 1),
                file_name: Some("archive.zip".to_string()),
                mime_type: Some("application/zip".to_string()),
            }),
            ..TelegramMessage::default()
        };

        let collection =
            collect_telegram_attachments(&api, Path::new("/unused-test-root"), &message).await;
        let notice = attachment_failure_notice(&collection.failures, true);

        assert!(collection.attachments.is_empty());
        assert_eq!(collection.failures.len(), 1);
        assert_eq!(
            collection.failures[0].reason,
            TelegramAttachmentFailureReason::TooLarge {
                bytes: Some(TELEGRAM_MAX_FILE_BYTES + 1),
            }
        );
        assert!(notice.contains("archive.zip"));
        assert!(notice.contains("20 MB"));
        assert!(notice.contains("说明文字也没有提交"));
    }

    #[test]
    fn failure_notice_explains_metadata_download_and_persist_errors() {
        let failures = [
            TelegramAttachmentFailure {
                name: "metadata.bin".to_string(),
                reason: TelegramAttachmentFailureReason::MetadataUnavailable,
            },
            TelegramAttachmentFailure {
                name: "download.bin".to_string(),
                reason: TelegramAttachmentFailureReason::DownloadFailed,
            },
            TelegramAttachmentFailure {
                name: "persist.bin".to_string(),
                reason: TelegramAttachmentFailureReason::PersistFailed,
            },
        ];

        let notice = attachment_failure_notice(&failures, false);

        assert!(notice.contains("metadata.bin：无法读取 Telegram 文件信息"));
        assert!(notice.contains("download.bin：从 Telegram 下载失败"));
        assert!(notice.contains("persist.bin：下载后无法保存到本机"));
        assert!(!notice.contains("说明文字也没有提交"));
    }

    #[tokio::test]
    async fn malformed_media_returns_metadata_failure_without_network() {
        let api = TelegramApi::new(TelegramSettings::default());
        let message = TelegramMessage {
            message_id: 17,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            photo: Some(Vec::new()),
            ..TelegramMessage::default()
        };

        let collection =
            collect_telegram_attachments(&api, Path::new("/unused-test-root"), &message).await;

        assert!(collection.attachments.is_empty());
        assert_eq!(
            collection.failures,
            vec![TelegramAttachmentFailure {
                name: "Telegram 附件".to_string(),
                reason: TelegramAttachmentFailureReason::MetadataUnavailable,
            }]
        );
    }

    #[test]
    fn attachment_names_hash_the_complete_telegram_file_id() {
        let first = unique_attachment_name("report.pdf", "shared-prefix-file-a");
        let second = unique_attachment_name("report.pdf", "shared-prefix-file-b");

        assert_ne!(first, second);
        assert!(first.starts_with("report-"));
        assert!(first.ends_with(".pdf"));
        assert_eq!(first.len(), "report-".len() + 64 + ".pdf".len());
    }

    #[test]
    fn uses_document_caption_as_turn_text() {
        let settings = TelegramSettings::default();
        let message = TelegramMessage {
            message_id: 13,
            chat: TelegramChat {
                id: 42,
                kind: "private".to_string(),
                ..TelegramChat::default()
            },
            caption: Some("review this file".to_string()),
            document: Some(TelegramMediaFile {
                file_id: "document".to_string(),
                file_unique_id: "document-unique".to_string(),
                file_size: Some(4_096),
                file_name: Some("report.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
            }),
            ..TelegramMessage::default()
        };

        let inbound = inbound_from_message(&settings, &["42".to_string()], &message)
            .expect("document message should be accepted");
        let specs = attachment_specs(&message);

        assert_eq!(inbound.text, "review this file");
        assert_eq!(specs[0].name, "report.pdf");
        assert_eq!(specs[0].kind, "file");
    }

    #[test]
    fn empty_allowed_chat_ids_do_not_pass_message_conversion() {
        let settings = TelegramSettings::default();
        let inbound = inbound_from_message(
            &settings,
            &[],
            &TelegramMessage {
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
                ..TelegramMessage::default()
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
            &TelegramMessage {
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
                ..TelegramMessage::default()
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
            ..TelegramMessage::default()
        };

        assert!(inbound_from_message(&settings, &["42".to_string()], &message).is_none());
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
            &TelegramMessage {
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
                ..TelegramMessage::default()
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

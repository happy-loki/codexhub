use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use tokio::time::{Duration, sleep};

use crate::{
    app_state::SharedState, chain_log, im::core::i18n::ImText, im_runtime::PendingApproval,
};

use super::{api::WechatApi, store};
use crate::im::core::text_adapter::TextChatAdapter;

pub(crate) const WECHAT_TEXT_CHUNK_CHARS: usize = 3500;
const WECHAT_CHUNK_DELAY_MS: u64 = 120;
const WECHAT_CONTEXT_TOKEN_STALE_WARN_MS: u128 = 60_000;

#[derive(Clone)]
pub struct WechatAdapter {
    api: WechatApi,
}

impl WechatAdapter {
    pub fn new(api: WechatApi) -> Self {
        Self { api }
    }

    pub async fn send_text(
        &self,
        state: &SharedState,
        account_id: &str,
        target: &str,
        text: &str,
    ) -> Result<String> {
        self.send_text_inner(state, account_id, target, text, true)
            .await
    }

    pub(crate) async fn send_text_without_context_token(
        &self,
        state: &SharedState,
        account_id: &str,
        target: &str,
        text: &str,
    ) -> Result<String> {
        self.send_text_inner(state, account_id, target, text, false)
            .await
    }

    async fn send_text_inner(
        &self,
        state: &SharedState,
        account_id: &str,
        target: &str,
        text: &str,
        use_context_token: bool,
    ) -> Result<String> {
        let context_token = if use_context_token {
            store::context_token_record(state, account_id, target).await
        } else {
            None
        };
        let chunks = wechat_text_chunks(text);
        let mut last_message_id = String::new();
        log_adapter(
            "send_text_begin",
            format!(
                "account={} target={} chars={} chunks={} context_token={} token_age_ms={} token_stale={} context_mode={}",
                account_id,
                target,
                text.chars().count(),
                chunks.len(),
                if context_token.is_some() {
                    "present"
                } else {
                    "missing"
                },
                token_age_label(context_token.as_ref()),
                token_stale_label(context_token.as_ref()),
                if use_context_token { "stored" } else { "none" }
            ),
        );
        for (index, chunk) in chunks.iter().enumerate() {
            last_message_id = self
                .api
                .send_text(
                    target,
                    context_token.as_ref().map(|record| record.token.as_str()),
                    chunk,
                )
                .await?;
            log_adapter(
                "send_text_chunk_sent",
                format!(
                    "account={} target={} chunk={}/{} message={}",
                    account_id,
                    target,
                    index + 1,
                    chunks.len(),
                    last_message_id
                ),
            );
            if index + 1 < chunks.len() {
                sleep(Duration::from_millis(WECHAT_CHUNK_DELAY_MS)).await;
            }
        }
        Ok(last_message_id)
    }

    pub async fn send_turn_completed(
        &self,
        state: &SharedState,
        account_id: &str,
        target: &str,
        reply_text: &str,
    ) -> Result<String> {
        self.send_text(state, account_id, target, reply_text).await
    }

    pub async fn send_image_path(
        &self,
        state: &SharedState,
        account_id: &str,
        target: &str,
        local_path: &Path,
        caption: Option<&str>,
        fallback_text: Option<&str>,
    ) -> Result<String> {
        let context_token = store::context_token_record(state, account_id, target).await;
        if let Some(text) = caption
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                fallback_text
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            })
        {
            self.send_text(state, account_id, target, text).await?;
        }
        log_adapter(
            "send_image_begin",
            format!(
                "account={} target={} path={} context_token={} token_age_ms={} token_stale={}",
                account_id,
                target,
                local_path.display(),
                if context_token.is_some() {
                    "present"
                } else {
                    "missing"
                },
                token_age_label(context_token.as_ref()),
                token_stale_label(context_token.as_ref())
            ),
        );
        let message_id = self
            .api
            .send_image_file(
                target,
                context_token.as_ref().map(|record| record.token.as_str()),
                local_path,
            )
            .await?;
        log_adapter(
            "send_image_sent",
            format!(
                "account={} target={} message={}",
                account_id, target, message_id
            ),
        );
        Ok(message_id)
    }
}

#[async_trait]
impl TextChatAdapter for WechatAdapter {
    async fn send_text(
        &self,
        state: &SharedState,
        account_id: &str,
        target: &str,
        text: &str,
    ) -> Result<String> {
        WechatAdapter::send_text(self, state, account_id, target, text).await
    }
}

pub(crate) fn approval_text(approval: &PendingApproval, text: ImText) -> String {
    let mut lines = vec![
        text.approval_request_heading().to_string(),
        format!("request_kind: `{}`", approval.request_kind),
        String::new(),
        approval.summary.trim().to_string(),
        String::new(),
        format!("{}:", text.available_decisions_label()),
    ];
    if approval.decisions.is_empty() {
        lines.push("/y".to_string());
        lines.push("/n".to_string());
    } else {
        lines.extend(
            approval
                .decisions
                .iter()
                .enumerate()
                .map(|(index, decision)| format!("/{} {}", index + 1, decision.label)),
        );
    }
    lines.push(String::new());
    lines.push(text.approval_reply_footer(&text.approval_reply_hint(approval)));
    lines.join("\n")
}

fn wechat_text_chunks(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![" ".to_string()];
    }
    if trimmed.chars().count() <= WECHAT_TEXT_CHUNK_CHARS {
        return vec![trimmed.to_string()];
    }
    split_message(trimmed)
}

fn split_message(message: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut remaining = message;
    while !remaining.is_empty() {
        if remaining.chars().count() <= WECHAT_TEXT_CHUNK_CHARS {
            chunks.push(remaining.to_string());
            break;
        }
        let hard_split = remaining
            .char_indices()
            .nth(WECHAT_TEXT_CHUNK_CHARS)
            .map_or(remaining.len(), |(idx, _)| idx);
        let search_area = &remaining[..hard_split];
        let split_at = search_area
            .rfind('\n')
            .filter(|pos| search_area[..*pos].chars().count() > WECHAT_TEXT_CHUNK_CHARS / 2)
            .or_else(|| {
                search_area
                    .rfind(' ')
                    .filter(|pos| search_area[..*pos].chars().count() > WECHAT_TEXT_CHUNK_CHARS / 2)
            })
            .unwrap_or(hard_split);
        chunks.push(remaining[..split_at].trim_end().to_string());
        remaining = remaining[split_at..].trim_start();
    }
    let chunk_count = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            if chunk_count == 1 {
                chunk
            } else {
                format!("({}/{})\n\n{}", index + 1, chunk_count, chunk)
            }
        })
        .collect()
}

fn log_adapter(event: &str, message: impl AsRef<str>) {
    chain_log::write_line(format!(
        "[wechat_adapter] event={} {}",
        event,
        message.as_ref()
    ));
}

fn token_age_label(record: Option<&store::WechatContextTokenRecord>) -> String {
    record
        .and_then(|record| record.age_ms())
        .map(|age_ms| age_ms.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn token_stale_label(record: Option<&store::WechatContextTokenRecord>) -> &'static str {
    match record.and_then(|record| record.age_ms()) {
        Some(age_ms) if age_ms >= WECHAT_CONTEXT_TOKEN_STALE_WARN_MS => "true",
        Some(_) => "false",
        None => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::{WECHAT_TEXT_CHUNK_CHARS, wechat_text_chunks};

    #[test]
    fn chunks_long_unicode_text() {
        let chunks = wechat_text_chunks(&"你好".repeat(2400));

        assert!(chunks.len() > 1);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= WECHAT_TEXT_CHUNK_CHARS + 16)
        );
    }
}

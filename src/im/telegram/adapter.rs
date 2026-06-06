use anyhow::Result;
use serde_json::json;
use std::path::Path;
use tokio::time::{Duration, sleep};

use crate::{
    chain_log,
    im::core::thread::ThreadCreateOption,
    im_runtime::{PendingApproval, approval_request_fingerprint},
};

use super::api::{TelegramApi, TelegramParseMode};

const TELEGRAM_MAX_MESSAGE_CHARS: usize = 4096;
const TELEGRAM_CONTINUATION_OVERHEAD: usize = 30;
const TELEGRAM_CHUNK_DELAY_MS: u64 = 100;

#[derive(Clone)]
pub struct TelegramAdapter {
    api: TelegramApi,
}

#[derive(Debug, Clone)]
pub struct TelegramThreadListEntry {
    pub title: String,
    pub state: String,
    pub cwd: Option<String>,
}

impl TelegramAdapter {
    pub fn new(api: TelegramApi) -> Self {
        Self { api }
    }

    pub async fn send_text(&self, target: &str, text: &str) -> Result<String> {
        let mut last_message_id = 0;
        let chunks = telegram_text_chunks(text);
        log_adapter(
            "send_text_begin",
            format!(
                "chat={} chars={} chunks={} preview={}",
                target,
                text.chars().count(),
                chunks.len(),
                log_text_preview(text, 500)
            ),
        );
        for (index, chunk) in chunks.iter().enumerate() {
            let html = telegram_markdown_to_html(chunk);
            log_adapter(
                "send_text_chunk_begin",
                format!(
                    "chat={} chunk={}/{} chars={} preview={}",
                    target,
                    index + 1,
                    chunks.len(),
                    chunk.chars().count(),
                    log_text_preview(chunk, 500)
                ),
            );
            last_message_id = match self
                .api
                .send_text_parse_mode(target, &html, TelegramParseMode::Html)
                .await
            {
                Ok(message_id) => {
                    log_adapter(
                        "send_text_chunk_sent",
                        format!(
                            "chat={} chunk={}/{} mode=html message={}",
                            target,
                            index + 1,
                            chunks.len(),
                            message_id
                        ),
                    );
                    message_id
                }
                Err(err) => {
                    log_adapter(
                        "send_text_html_failed",
                        format!(
                            "chat={} chunk={}/{} fallback=plain err={}",
                            target,
                            index + 1,
                            chunks.len(),
                            err
                        ),
                    );
                    let message_id = self.api.send_text(target, &chunk).await?;
                    log_adapter(
                        "send_text_chunk_sent",
                        format!(
                            "chat={} chunk={}/{} mode=plain message={}",
                            target,
                            index + 1,
                            chunks.len(),
                            message_id
                        ),
                    );
                    message_id
                }
            };
            if index + 1 < chunks.len() {
                sleep(Duration::from_millis(TELEGRAM_CHUNK_DELAY_MS)).await;
            }
        }
        log_adapter(
            "send_text_done",
            format!(
                "chat={} chunks={} message={}",
                target,
                chunks.len(),
                last_message_id
            ),
        );
        Ok(last_message_id.to_string())
    }

    pub async fn send_turn_completed(&self, target: &str, reply_text: &str) -> Result<String> {
        self.send_text(target, reply_text).await
    }

    pub async fn send_image_path(
        &self,
        target: &str,
        local_path: &Path,
        caption: Option<&str>,
    ) -> Result<String> {
        let caption_html = caption.map(telegram_markdown_to_html);
        log_adapter(
            "send_image_begin",
            format!(
                "chat={} path={} caption_chars={}",
                target,
                local_path.display(),
                caption.map(|value| value.chars().count()).unwrap_or(0)
            ),
        );
        match self
            .api
            .send_photo_file(
                target,
                local_path,
                caption_html.as_deref(),
                Some(TelegramParseMode::Html),
            )
            .await
        {
            Ok(message_id) => {
                log_adapter(
                    "send_image_sent",
                    format!("chat={} method=sendPhoto message={}", target, message_id),
                );
                Ok(message_id.to_string())
            }
            Err(photo_err) => match self
                .api
                .send_document_file(
                    target,
                    local_path,
                    caption_html.as_deref(),
                    Some(TelegramParseMode::Html),
                )
                .await
            {
                Ok(message_id) => {
                    log_adapter(
                        "send_image_sent",
                        format!("chat={} method=sendDocument message={}", target, message_id),
                    );
                    Ok(message_id.to_string())
                }
                Err(document_err) => {
                    log_adapter(
                        "send_image_failed",
                        format!(
                            "chat={} path={} photo_err={} document_err={}",
                            target,
                            local_path.display(),
                            photo_err,
                            document_err
                        ),
                    );
                    Err(photo_err)
                }
            },
        }
    }

    pub async fn send_approval(&self, target: &str, approval: &PendingApproval) -> Result<String> {
        let text = approval_text(approval);
        let Some(keyboard) = approval_keyboard(approval) else {
            return self.send_text(target, &text).await;
        };
        let chunks = telegram_text_chunks(&text);
        let mut last_message_id = 0;
        log_adapter(
            "send_approval_begin",
            format!(
                "chat={} request={} chars={} chunks={} decisions={}",
                target,
                approval.request_id,
                text.chars().count(),
                chunks.len(),
                approval.decisions.len()
            ),
        );
        for (index, chunk) in chunks.iter().enumerate() {
            let is_last = index + 1 == chunks.len();
            if is_last {
                let html = telegram_markdown_to_html(chunk);
                last_message_id = match self
                    .api
                    .send_text_with_reply_markup_parse_mode(
                        target,
                        &html,
                        keyboard.clone(),
                        TelegramParseMode::Html,
                    )
                    .await
                {
                    Ok(message_id) => message_id,
                    Err(err) => {
                        log_adapter(
                            "send_approval_html_failed",
                            format!(
                                "chat={} request={} chunk={}/{} fallback=plain err={}",
                                target,
                                approval.request_id,
                                index + 1,
                                chunks.len(),
                                err
                            ),
                        );
                        self.api
                            .send_text_with_reply_markup(target, chunk, keyboard.clone())
                            .await?
                    }
                };
            } else {
                let html = telegram_markdown_to_html(chunk);
                last_message_id = match self
                    .api
                    .send_text_parse_mode(target, &html, TelegramParseMode::Html)
                    .await
                {
                    Ok(message_id) => message_id,
                    Err(err) => {
                        log_adapter(
                            "send_approval_html_failed",
                            format!(
                                "chat={} request={} chunk={}/{} fallback=plain err={}",
                                target,
                                approval.request_id,
                                index + 1,
                                chunks.len(),
                                err
                            ),
                        );
                        self.api.send_text(target, chunk).await?
                    }
                };
                sleep(Duration::from_millis(TELEGRAM_CHUNK_DELAY_MS)).await;
            }
        }
        log_adapter(
            "send_approval_done",
            format!(
                "chat={} request={} chunks={} message={}",
                target,
                approval.request_id,
                chunks.len(),
                last_message_id
            ),
        );
        Ok(last_message_id.to_string())
    }

    pub async fn answer_callback_query(&self, callback_query_id: &str, text: &str) -> Result<()> {
        log_adapter(
            "answer_callback_begin",
            format!(
                "callback_query={} text_len={}",
                callback_query_id,
                text.chars().count()
            ),
        );
        self.api
            .answer_callback_query(callback_query_id, Some(text))
            .await?;
        log_adapter(
            "answer_callback_done",
            format!("callback_query={}", callback_query_id),
        );
        Ok(())
    }

    pub async fn send_thread_routing_choice(
        &self,
        target: &str,
        request_id: &str,
    ) -> Result<String> {
        let keyboard = inline_keyboard(vec![
            vec![button("创建新会话", &format!("trc:{request_id}:new"))],
            vec![button("恢复历史会话", &format!("trc:{request_id}:load"))],
        ]);
        let text =
            "当前 Telegram 会话还没有接入 Codex thread。\n请选择创建新会话，或恢复一个历史会话。";
        log_adapter(
            "send_thread_routing_choice_begin",
            format!("chat={} request={}", target, request_id),
        );
        let message_id = self
            .api
            .send_text_with_reply_markup(target, text, keyboard)
            .await?;
        log_adapter(
            "send_thread_routing_choice_done",
            format!(
                "chat={} request={} message={}",
                target, request_id, message_id
            ),
        );
        Ok(message_id.to_string())
    }

    pub async fn send_thread_create_settings(
        &self,
        target: &str,
        request_id: &str,
        text: &str,
    ) -> Result<String> {
        let keyboard = inline_keyboard(vec![
            vec![
                button("目录", &format!("tce:{request_id}:cwd")),
                button("模型", &format!("tce:{request_id}:model")),
            ],
            vec![
                button("推理强度", &format!("tce:{request_id}:effort")),
                button("权限", &format!("tce:{request_id}:perm")),
            ],
            vec![button("创建", &format!("tcc:{request_id}"))],
            vec![button("恢复历史会话", &format!("trc:{request_id}:load"))],
        ]);
        log_adapter(
            "send_thread_create_settings_begin",
            format!(
                "chat={} request={} text_len={}",
                target,
                request_id,
                text.chars().count()
            ),
        );
        let message_id = self
            .api
            .send_text_with_reply_markup(target, text, keyboard)
            .await?;
        log_adapter(
            "send_thread_create_settings_done",
            format!(
                "chat={} request={} message={}",
                target, request_id, message_id
            ),
        );
        Ok(message_id.to_string())
    }

    pub async fn send_thread_create_options(
        &self,
        target: &str,
        request_id: &str,
        field: &str,
        title: &str,
        body: &str,
        options: &[ThreadCreateOption],
        page: usize,
        has_prev: bool,
        has_next: bool,
    ) -> Result<String> {
        let mut rows = Vec::new();
        let mut nav = Vec::new();
        if has_prev {
            nav.push(button("上一页", &format!("tcp:{request_id}:{field}:prev")));
        }
        if has_next {
            nav.push(button("下一页", &format!("tcp:{request_id}:{field}:next")));
        }
        if !nav.is_empty() {
            rows.push(nav);
        }
        if field == "cwd" {
            rows.push(vec![button(
                "自定义或新建目录",
                &format!("tcv:{request_id}:cwd:__custom__"),
            )]);
        }
        rows.push(vec![button(
            "返回创建设置",
            &format!("trc:{request_id}:new"),
        )]);

        let options_html = create_options_table_html(options);
        let text = create_options_html_text(title, body, page, options.len(), &options_html);
        log_adapter(
            "send_thread_create_options_begin",
            format!(
                "chat={} request={} field={} page={} options={} text_len={}",
                target,
                request_id,
                field,
                page,
                options.len(),
                text.chars().count()
            ),
        );
        let message_id = self
            .api
            .send_text_with_reply_markup_parse_mode(
                target,
                &text,
                inline_keyboard(rows),
                TelegramParseMode::Html,
            )
            .await?;
        log_adapter(
            "send_thread_create_options_done",
            format!(
                "chat={} request={} field={} page={} message={}",
                target, request_id, field, page, message_id
            ),
        );
        Ok(message_id.to_string())
    }

    pub async fn send_thread_list(
        &self,
        target: &str,
        request_id: &str,
        title: &str,
        body: &str,
        entries: &[TelegramThreadListEntry],
        page: usize,
        has_prev: bool,
        has_next: bool,
    ) -> Result<String> {
        let mut rows = Vec::new();
        let mut nav = Vec::new();
        if has_prev {
            nav.push(button("上一页", &format!("tlp:{request_id}:prev")));
        }
        if has_next {
            nav.push(button("下一页", &format!("tlp:{request_id}:next")));
        }
        if !nav.is_empty() {
            rows.push(nav);
        }
        rows.push(vec![button("创建新会话", &format!("trc:{request_id}:new"))]);

        let entries_html = if entries.is_empty() {
            telegram_html_escape("当前没有可恢复的历史会话。")
        } else {
            thread_entries_table_html(entries)
        };
        let text = thread_list_html_text(title, body, page, &entries_html);
        log_adapter(
            "send_thread_list_begin",
            format!(
                "chat={} request={} page={} entries={} text_len={}",
                target,
                request_id,
                page,
                entries.len(),
                text.chars().count()
            ),
        );
        let message_id = self
            .api
            .send_text_with_reply_markup_parse_mode(
                target,
                &text,
                inline_keyboard(rows),
                TelegramParseMode::Html,
            )
            .await?;
        log_adapter(
            "send_thread_list_done",
            format!(
                "chat={} request={} page={} entries={} message={}",
                target,
                request_id,
                page,
                entries.len(),
                message_id
            ),
        );
        Ok(message_id.to_string())
    }

    pub async fn send_thread_routing_result(
        &self,
        target: &str,
        title: &str,
        body: &str,
    ) -> Result<String> {
        self.send_text(target, &format!("{title}\n\n{body}")).await
    }
}

fn approval_text(approval: &PendingApproval) -> String {
    let mut lines = vec![
        "approval request".to_string(),
        format!("request_kind: `{}`", approval.request_kind),
        String::new(),
        approval.summary.trim().to_string(),
        String::new(),
        "availableDecisions:".to_string(),
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
    lines.join("\n")
}

fn approval_keyboard(approval: &PendingApproval) -> Option<serde_json::Value> {
    let fingerprint = approval_request_fingerprint(&approval.request_key());
    let rows = approval
        .decisions
        .iter()
        .enumerate()
        .map(|(index, decision)| {
            vec![approval_button(
                &decision.label,
                &format!("ap:{fingerprint}:{}", index + 1),
            )]
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then(|| inline_keyboard(rows))
}

fn inline_keyboard(rows: Vec<Vec<serde_json::Value>>) -> serde_json::Value {
    json!({ "inline_keyboard": rows })
}

fn log_adapter(event: &str, message: impl AsRef<str>) {
    chain_log::write_diagnostic_lazy(|| {
        format!("[telegram_adapter] event={} {}", event, message.as_ref())
    });
}

fn log_text_preview(text: &str, limit: usize) -> String {
    let compact = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut out = String::new();
    for ch in compact.chars().take(limit) {
        out.push(ch);
    }
    if compact.chars().count() > limit {
        out.push_str("...");
    }
    out
}

fn button(text: &str, callback_data: &str) -> serde_json::Value {
    json!({
        "text": truncate_button_text(text),
        "callback_data": callback_data,
    })
}

fn approval_button(text: &str, callback_data: &str) -> serde_json::Value {
    json!({
        "text": text.trim(),
        "callback_data": callback_data,
    })
}

fn thread_entries_table_html(entries: &[TelegramThreadListEntry]) -> String {
    let mut lines = Vec::new();
    let mut current_cwd: Option<&str> = None;
    for (index, entry) in entries.iter().enumerate() {
        let cwd = entry
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if current_cwd != cwd {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(project_header_html(cwd));
            current_cwd = cwd;
        }
        lines.push(thread_entry_table_html(index, entry));
    }
    lines.join("\n")
}

fn create_options_table_html(options: &[ThreadCreateOption]) -> String {
    let mut lines = Vec::new();
    for (index, option) in options.iter().enumerate() {
        lines.push(create_option_row_html(index, option));
        lines.push(String::new());
    }
    lines.join("\n")
}

fn create_option_row_html(index: usize, option: &ThreadCreateOption) -> String {
    let label = truncate_display_text(option.label.trim(), 34);
    let mut row = format!("/{} <b>{}</b>", index + 1, telegram_html_escape(&label));
    if let Some(summary) = option
        .summary
        .as_deref()
        .map(telegram_cleanup_text)
        .filter(|v| !v.is_empty())
    {
        row.push('\n');
        row.push_str(&option_summary_html(&summary));
    }
    row
}

fn option_summary_html(summary: &str) -> String {
    let summary = truncate_middle(summary, 56);
    if looks_like_path(&summary) {
        format!("<code>{}</code>", telegram_html_escape(&summary))
    } else {
        telegram_html_escape(&summary)
    }
}

fn create_options_html_text(
    title: &str,
    body: &str,
    page: usize,
    option_count: usize,
    options_html: &str,
) -> String {
    let hint = if option_count == 0 {
        "当前没有可选项。".to_string()
    } else {
        format!("第 {} 页 · 点击 /1 ~ /{} 选择", page.max(1), option_count)
    };
    format!(
        "<b>{}</b>\n{}\n\n{}\n<code>{}</code>",
        telegram_html_escape(title),
        telegram_markdown_to_html(&telegram_cleanup_text(body)),
        options_html.trim_end(),
        telegram_html_escape(&hint)
    )
}

fn thread_entry_table_html(index: usize, entry: &TelegramThreadListEntry) -> String {
    let title = entry.title.trim();
    let title = if title.is_empty() {
        "未命名会话"
    } else {
        title
    };
    let title = truncate_display_text(title, 22);
    let state = thread_state_suffix(&entry.state)
        .map(|state| format!(" <code>{}</code>", telegram_html_escape(state)))
        .unwrap_or_default();
    format!(
        "/{} <b>{}</b>{state}",
        index + 1,
        telegram_html_escape(&title)
    )
}

fn project_header_html(cwd: Option<&str>) -> String {
    match cwd {
        Some(cwd) => {
            let name = project_name(cwd);
            format!(
                "<b>项目：{}</b>\n<code>{}</code>",
                telegram_html_escape(&truncate_display_text(&name, 32)),
                telegram_html_escape(&truncate_middle(cwd, 68))
            )
        }
        None => "<b>项目：未知目录</b>".to_string(),
    }
}

fn thread_state_suffix(state: &str) -> Option<&'static str> {
    if state.contains("当前会话") {
        Some("当前")
    } else if state.contains("已加载") {
        Some("已加载")
    } else {
        None
    }
}

fn thread_list_html_text(title: &str, body: &str, page: usize, entries_html: &str) -> String {
    format!(
        "<b>{}</b>\n{}\n\n{}\n\n<code>第 {} 页</code>",
        telegram_html_escape(title),
        telegram_markdown_to_html(&telegram_cleanup_text(body)),
        entries_html,
        page.max(1)
    )
}

fn truncate_display_text(text: &str, max_chars: usize) -> String {
    let text = text
        .replace('\r', " ")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

fn project_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

fn looks_like_path(value: &str) -> bool {
    let value = value.trim();
    value.contains('\\') || value.contains('/') || value.starts_with('~')
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head_len = max_chars.saturating_sub(3) / 2;
    let tail_len = max_chars.saturating_sub(3 + head_len);
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head}...{tail}")
}

fn telegram_markdown_to_html(text: &str) -> String {
    let text = telegram_cleanup_text(text);
    let mut html = String::new();
    let mut in_code_block = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if in_code_block {
                html.push_str("</code></pre>\n");
            } else {
                html.push_str("<pre><code>");
            }
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            html.push_str(&telegram_html_escape(line));
            html.push('\n');
        } else {
            html.push_str(&telegram_inline_markdown_to_html(line));
            html.push('\n');
        }
    }
    if in_code_block {
        html.push_str("</code></pre>");
    }
    html.trim_end().to_string()
}

fn telegram_inline_markdown_to_html(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**")
        {
            out.push_str("<b>");
            out.push_str(&telegram_html_escape(&after[..end]));
            out.push_str("</b>");
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
        {
            out.push_str("<code>");
            out.push_str(&telegram_html_escape(&after[..end]));
            out.push_str("</code>");
            rest = &after[end + 1..];
            continue;
        }
        if let Some(after_label) = rest.strip_prefix('[')
            && let Some(label_end) = after_label.find("](")
            && let Some(url_end) = after_label[label_end + 2..].find(')')
        {
            let label = &after_label[..label_end];
            let url = &after_label[label_end + 2..label_end + 2 + url_end];
            if url.starts_with("http://") || url.starts_with("https://") {
                out.push_str("<a href=\"");
                out.push_str(&telegram_html_attr_escape(url));
                out.push_str("\">");
                out.push_str(&telegram_html_escape(label));
                out.push_str("</a>");
            } else {
                out.push_str(&telegram_html_escape(label));
            }
            rest = &after_label[label_end + 2 + url_end + 1..];
            continue;
        }
        let ch = rest.chars().next().expect("rest is non-empty");
        out.push_str(&telegram_html_escape(&ch.to_string()));
        rest = &rest[ch.len_utf8()..];
    }
    out
}

fn telegram_cleanup_text(text: &str) -> String {
    text.replace("<font color='grey'>", "")
        .replace("<font color=\"grey\">", "")
        .replace("</font>", "")
}

fn telegram_html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn telegram_html_attr_escape(text: &str) -> String {
    telegram_html_escape(text).replace('"', "&quot;")
}

fn truncate_button_text(text: &str) -> String {
    const MAX: usize = 48;
    let text = text.trim();
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let mut output = text.chars().take(MAX.saturating_sub(1)).collect::<String>();
    output.push('…');
    output
}

fn telegram_text_chunks(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![" ".to_string()];
    }
    if trimmed.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS {
        return vec![trimmed.to_string()];
    }

    let chunks = split_message_for_telegram(trimmed);
    let chunk_count = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            if index == 0 {
                format!("{chunk}\n\n(continues...)")
            } else if index + 1 == chunk_count {
                format!("(continued)\n\n{chunk}")
            } else {
                format!("(continued)\n\n{chunk}\n\n(continues...)")
            }
        })
        .collect()
}

fn split_message_for_telegram(message: &str) -> Vec<String> {
    let content_limit = TELEGRAM_MAX_MESSAGE_CHARS - TELEGRAM_CONTINUATION_OVERHEAD;

    let mut chunks = Vec::new();
    let mut remaining = message;
    while !remaining.is_empty() {
        if remaining.chars().count() <= content_limit {
            chunks.push(remaining.to_string());
            break;
        }

        let hard_split = remaining
            .char_indices()
            .nth(content_limit)
            .map_or(remaining.len(), |(idx, _)| idx);
        let search_area = &remaining[..hard_split];
        let chunk_end = best_split_point(search_area, hard_split, content_limit);

        chunks.push(remaining[..chunk_end].trim_end().to_string());
        remaining = remaining[chunk_end..].trim_start();
    }
    chunks
}

fn best_split_point(search_area: &str, hard_split: usize, content_limit: usize) -> usize {
    if let Some(pos) = search_area.rfind('\n')
        && search_area[..pos].chars().count() >= content_limit / 2
    {
        return pos + 1;
    }
    if let Some(pos) = search_area.rfind(' ')
        && search_area[..pos].chars().count() >= content_limit / 2
    {
        return pos + 1;
    }
    hard_split
}

#[cfg(test)]
mod tests {
    use super::{TELEGRAM_MAX_MESSAGE_CHARS, telegram_text_chunks};

    #[test]
    fn chunks_long_text_on_char_boundaries() {
        let chunks = telegram_text_chunks(&"你好世界".repeat(1100));

        assert!(chunks.len() > 1);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= TELEGRAM_MAX_MESSAGE_CHARS)
        );
        assert!(chunks[0].ends_with("(continues...)"));
        assert!(chunks[1].starts_with("(continued)"));
    }

    #[test]
    fn keeps_single_message_when_within_limit() {
        let text = "hello";
        let chunks = telegram_text_chunks(text);

        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn empty_message_uses_space_placeholder() {
        let chunks = telegram_text_chunks("  \n ");

        assert_eq!(chunks, vec![" "]);
    }

    #[test]
    fn prefers_newline_split_for_long_text() {
        let first = "a".repeat(3000);
        let second = "b".repeat(3000);
        let chunks = telegram_text_chunks(&format!("{first}\n{second}"));

        assert!(chunks[0].contains("(continues...)"));
        assert!(chunks[0].contains('\n'));
        assert!(chunks[0].trim_start().starts_with('a'));
        assert!(chunks[1].contains('b'));
    }
}

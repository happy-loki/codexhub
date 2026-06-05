use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{info, warn};

use crate::{
    app_state::SharedState,
    chain_log,
    im::feishu::{
        FeishuApi,
        renderer::{
            self, FEISHU_CARDKIT_STREAMING_ELEMENT_ID, build_cardkit_streaming_agent_message_card,
            build_cardkit_streaming_reply_card, build_cardkit_streaming_tool_card,
        },
        types::FeishuStreamingCardState,
    },
};

const FEISHU_CARDKIT_THROTTLE_MS: u64 = 100;
const FEISHU_CARDKIT_LONG_GAP_THRESHOLD_MS: u64 = 2000;
const FEISHU_CARDKIT_BATCH_AFTER_GAP_MS: u64 = 300;
const FEISHU_LOG_PREVIEW_CHARS: usize = 240;

fn streaming_card_key(thread_id: &str, item_id: &str) -> String {
    format!("{thread_id}:{item_id}")
}

fn feishu_receive_target(target: &str) -> (&'static str, &str) {
    target
        .strip_prefix("open_id:")
        .map(|open_id| ("open_id", open_id))
        .unwrap_or(("chat_id", target))
}

fn log_preview(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut out = String::new();
    for ch in normalized.chars().take(FEISHU_LOG_PREVIEW_CHARS) {
        out.push(ch);
    }
    if normalized.chars().count() > FEISHU_LOG_PREVIEW_CHARS {
        out.push_str("...");
    }
    out
}

fn log_tail(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\n', "\\n");
    let chars = normalized.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(FEISHU_LOG_PREVIEW_CHARS);
    let mut out = chars[start..].iter().collect::<String>();
    if start > 0 {
        out.insert_str(0, "...");
    }
    out
}

fn write_stream_log(
    event: &str,
    thread_id: &str,
    item_id: &str,
    state: &FeishuStreamingCardState,
    extra: impl AsRef<str>,
) {
    chain_log::write_line(format!(
        "[feishu_stream] event={} thread_id={} item_id={} kind={} account_id={} chat_id={} receive_id_type={} receive_id={} message_id={} card_id={} sequence={} text_len={} sent_text_len={} completed={} sending={} dirty={} preview={} tail={} {}",
        event,
        thread_id,
        item_id,
        state.kind,
        state.account_id,
        state.chat_id,
        state.receive_id_type,
        state.receive_id,
        state.message_id.as_deref().unwrap_or_default(),
        state.card_id.as_deref().unwrap_or_default(),
        state.sequence,
        state.text.len(),
        state.sent_text.len(),
        state.completed,
        state.sending,
        state.dirty,
        log_preview(&state.text),
        log_tail(&state.text),
        extra.as_ref()
    ));
}

pub async fn upsert_streaming_card_state(
    state: SharedState,
    api: FeishuApi,
    thread_id: &str,
    item_id: &str,
    kind: &str,
    account_id: &str,
    chat_id: &str,
    delta: &str,
    completed: bool,
) {
    if delta.is_empty() && !completed {
        return;
    }
    let state_key = streaming_card_key(thread_id, item_id);
    {
        let mut runtime = state.runtime.lock().await;
        let entry = runtime
            .feishu_streaming_cards_by_item
            .entry(state_key)
            .or_insert_with(|| FeishuStreamingCardState {
                account_id: account_id.to_string(),
                chat_id: chat_id.to_string(),
                receive_id_type: feishu_receive_target(chat_id).0.to_string(),
                receive_id: feishu_receive_target(chat_id).1.to_string(),
                kind: kind.to_string(),
                message_id: None,
                card_id: None,
                sequence: 0,
                text: String::new(),
                sent_text: String::new(),
                completed: false,
                sending: false,
                dirty: false,
                last_sent_at: None,
            });

        entry.account_id = account_id.to_string();
        entry.chat_id = chat_id.to_string();
        let (receive_id_type, receive_id) = feishu_receive_target(chat_id);
        entry.receive_id_type = receive_id_type.to_string();
        entry.receive_id = receive_id.to_string();
        entry.kind = kind.to_string();
        if completed {
            if !delta.is_empty() {
                entry.text = delta.to_string();
            }
            entry.completed = true;
        } else if kind == "fileSummary" {
            entry.text = delta.to_string();
        } else {
            entry.text.push_str(delta);
        }
        entry.dirty = true;
        write_stream_log(
            "upsert",
            thread_id,
            item_id,
            entry,
            format!("delta_len={}", delta.len()),
        );
        info!(
            target: "codex_remote::feishu",
            event = "feishu_stream_upsert",
            thread_id,
            item_id,
            kind = entry.kind.as_str(),
            account_id = entry.account_id.as_str(),
            chat_id = entry.chat_id.as_str(),
            receive_id_type = entry.receive_id_type.as_str(),
            receive_id = entry.receive_id.as_str(),
            delta_len = delta.len(),
            text_len = entry.text.len(),
            completed = entry.completed,
            sending = entry.sending,
            dirty = entry.dirty,
            preview = %log_preview(&entry.text),
            tail = %log_tail(&entry.text),
            "Feishu streaming state upserted"
        );
    }
    spawn_streaming_card_driver(state, api, thread_id, item_id);
}

pub async fn ensure_started_streaming_card_state(
    state: SharedState,
    api: FeishuApi,
    thread_id: &str,
    item_id: &str,
    kind: &str,
    account_id: &str,
    chat_id: &str,
    initial_text: Option<String>,
) {
    let state_key = streaming_card_key(thread_id, item_id);
    {
        let mut runtime = state.runtime.lock().await;
        let entry = runtime
            .feishu_streaming_cards_by_item
            .entry(state_key)
            .or_insert_with(|| FeishuStreamingCardState {
                account_id: account_id.to_string(),
                chat_id: chat_id.to_string(),
                receive_id_type: feishu_receive_target(chat_id).0.to_string(),
                receive_id: feishu_receive_target(chat_id).1.to_string(),
                kind: kind.to_string(),
                message_id: None,
                card_id: None,
                sequence: 0,
                text: String::new(),
                sent_text: String::new(),
                completed: false,
                sending: false,
                dirty: false,
                last_sent_at: None,
            });

        entry.account_id = account_id.to_string();
        entry.chat_id = chat_id.to_string();
        let (receive_id_type, receive_id) = feishu_receive_target(chat_id);
        entry.receive_id_type = receive_id_type.to_string();
        entry.receive_id = receive_id.to_string();
        entry.kind = kind.to_string();
        if let Some(text) = initial_text.filter(|value| !value.trim().is_empty()) {
            if entry.text.trim().is_empty() {
                entry.text = text;
            }
        }
        entry.dirty = true;
        write_stream_log("started", thread_id, item_id, entry, "");
        info!(
            target: "codex_remote::feishu",
            event = "feishu_stream_started",
            thread_id,
            item_id,
            kind = entry.kind.as_str(),
            account_id = entry.account_id.as_str(),
            chat_id = entry.chat_id.as_str(),
            receive_id_type = entry.receive_id_type.as_str(),
            receive_id = entry.receive_id.as_str(),
            text_len = entry.text.len(),
            completed = entry.completed,
            sending = entry.sending,
            dirty = entry.dirty,
            preview = %log_preview(&entry.text),
            tail = %log_tail(&entry.text),
            "Feishu streaming state started"
        );
    }
    spawn_streaming_card_driver(state, api, thread_id, item_id);
}

pub async fn complete_existing_item_card(
    state: SharedState,
    api: FeishuApi,
    thread_id: &str,
    item_id: &str,
    kind: &str,
    account_id: &str,
    chat_id: &str,
    text: Option<String>,
) -> bool {
    let state_key = streaming_card_key(thread_id, item_id);
    let updated = {
        let mut runtime = state.runtime.lock().await;
        match runtime.feishu_streaming_cards_by_item.get_mut(&state_key) {
            Some(entry) => {
                entry.account_id = account_id.to_string();
                entry.chat_id = chat_id.to_string();
                let (receive_id_type, receive_id) = feishu_receive_target(chat_id);
                entry.receive_id_type = receive_id_type.to_string();
                entry.receive_id = receive_id.to_string();
                entry.kind = kind.to_string();
                if let Some(text) = text.filter(|value| !value.trim().is_empty()) {
                    entry.text = text;
                }
                entry.completed = true;
                entry.dirty = true;
                write_stream_log("completed", thread_id, item_id, entry, "");
                info!(
                    target: "codex_remote::feishu",
                    event = "feishu_stream_completed",
                    thread_id,
                    item_id,
                    kind = entry.kind.as_str(),
                    account_id = entry.account_id.as_str(),
                    chat_id = entry.chat_id.as_str(),
                    receive_id_type = entry.receive_id_type.as_str(),
                    receive_id = entry.receive_id.as_str(),
                    text_len = entry.text.len(),
                    completed = entry.completed,
                    sending = entry.sending,
                    dirty = entry.dirty,
                    preview = %log_preview(&entry.text),
                    tail = %log_tail(&entry.text),
                    "Feishu streaming state completed"
                );
                true
            }
            None => false,
        }
    };
    if updated {
        spawn_streaming_card_driver(state, api, thread_id, item_id);
    }
    updated
}

fn spawn_streaming_card_driver(state: SharedState, api: FeishuApi, thread_id: &str, item_id: &str) {
    let thread_id = thread_id.to_string();
    let item_id = item_id.to_string();
    tokio::spawn(async move {
        if let Err(err) = drive_streaming_card_state(state.clone(), api, &thread_id, &item_id).await
        {
            state
                .push_event(
                    "error",
                    "feishu_stream_drive_failed",
                    format!("thread={thread_id} item={item_id} err={err}"),
                )
                .await;
        }
    });
}

async fn drive_streaming_card_state(
    state: SharedState,
    api: FeishuApi,
    thread_id: &str,
    item_id: &str,
) -> Result<(), String> {
    let state_key = streaming_card_key(thread_id, item_id);
    loop {
        let mut state_snapshot = {
            let mut runtime = state.runtime.lock().await;
            let Some(current) = runtime.feishu_streaming_cards_by_item.get_mut(&state_key) else {
                return Ok(());
            };
            if current.sending || !current.dirty {
                return Ok(());
            }
            current.sending = true;
            current.dirty = false;
            current.clone()
        };

        if should_skip_empty_streaming_send(&state_snapshot) {
            let mut runtime = state.runtime.lock().await;
            if let Some(current) = runtime.feishu_streaming_cards_by_item.get_mut(&state_key) {
                current.sending = false;
                if current.completed
                    && current.text.trim().is_empty()
                    && current.sent_text.is_empty()
                {
                    write_stream_log("skipped_empty", thread_id, item_id, current, "");
                    runtime.feishu_streaming_cards_by_item.remove(&state_key);
                    return Ok(());
                }
                write_stream_log("skipped_empty", thread_id, item_id, current, "");
                return Ok(());
            }
            return Ok(());
        }

        if state_snapshot.message_id.is_some() {
            let elapsed = state_snapshot
                .last_sent_at
                .map(|last_sent_at| last_sent_at.elapsed())
                .unwrap_or_else(|| Duration::from_millis(FEISHU_CARDKIT_LONG_GAP_THRESHOLD_MS + 1));

            let wait_duration =
                if elapsed >= Duration::from_millis(FEISHU_CARDKIT_LONG_GAP_THRESHOLD_MS) {
                    Some(Duration::from_millis(FEISHU_CARDKIT_BATCH_AFTER_GAP_MS))
                } else if elapsed < Duration::from_millis(FEISHU_CARDKIT_THROTTLE_MS) {
                    Some(Duration::from_millis(FEISHU_CARDKIT_THROTTLE_MS) - elapsed)
                } else {
                    None
                };

            if let Some(wait_duration) = wait_duration {
                tokio::time::sleep(wait_duration).await;
                let mut runtime = state.runtime.lock().await;
                let Some(current) = runtime.feishu_streaming_cards_by_item.get_mut(&state_key)
                else {
                    return Ok(());
                };
                state_snapshot = current.clone();
                current.dirty = false;
            }
        }

        let send_result = if state_snapshot.kind == "agentMessage" {
            write_stream_log("send_begin", thread_id, item_id, &state_snapshot, "");
            info!(
                target: "codex_remote::feishu",
                event = "feishu_stream_send_begin",
                thread_id,
                item_id,
                kind = state_snapshot.kind.as_str(),
                account_id = state_snapshot.account_id.as_str(),
                chat_id = state_snapshot.chat_id.as_str(),
                receive_id_type = state_snapshot.receive_id_type.as_str(),
                receive_id = state_snapshot.receive_id.as_str(),
                message_id = state_snapshot.message_id.as_deref().unwrap_or(""),
                card_id = state_snapshot.card_id.as_deref().unwrap_or(""),
                sequence = state_snapshot.sequence,
                text_len = state_snapshot.text.len(),
                sent_text_len = state_snapshot.sent_text.len(),
                completed = state_snapshot.completed,
                preview = %log_preview(&state_snapshot.text),
                tail = %log_tail(&state_snapshot.text),
                "Sending Feishu CardKit streaming card"
            );
            send_agent_message_cardkit(&api, &state_snapshot).await
        } else {
            write_stream_log("send_begin", thread_id, item_id, &state_snapshot, "");
            info!(
                target: "codex_remote::feishu",
                event = "feishu_stream_send_begin",
                thread_id,
                item_id,
                kind = state_snapshot.kind.as_str(),
                account_id = state_snapshot.account_id.as_str(),
                chat_id = state_snapshot.chat_id.as_str(),
                receive_id_type = state_snapshot.receive_id_type.as_str(),
                receive_id = state_snapshot.receive_id.as_str(),
                message_id = state_snapshot.message_id.as_deref().unwrap_or(""),
                card_id = state_snapshot.card_id.as_deref().unwrap_or(""),
                sequence = state_snapshot.sequence,
                text_len = state_snapshot.text.len(),
                sent_text_len = state_snapshot.sent_text.len(),
                completed = state_snapshot.completed,
                preview = %log_preview(&state_snapshot.text),
                tail = %log_tail(&state_snapshot.text),
                "Sending Feishu interactive streaming card"
            );
            send_interactive_streaming_card(&api, &state_snapshot).await
        };

        match send_result {
            Ok((message_id, card_id, sequence)) => {
                let mut runtime = state.runtime.lock().await;
                let mut should_remove = false;
                let mut should_continue = false;
                if let Some(current) = runtime.feishu_streaming_cards_by_item.get_mut(&state_key) {
                    if let Some(message_id) = message_id {
                        current.message_id = Some(message_id);
                    }
                    if let Some(card_id) = card_id {
                        current.card_id = Some(card_id);
                    }
                    current.sequence = sequence;
                    current.sent_text = state_snapshot.text.clone();
                    current.last_sent_at = Some(Instant::now());
                    current.sending = false;
                    should_remove =
                        current.completed && !current.dirty && current.sent_text == current.text;
                    should_continue = current.dirty;
                    write_stream_log(
                        "send_ok",
                        thread_id,
                        item_id,
                        current,
                        format!(
                            "should_continue={} should_remove={}",
                            should_continue, should_remove
                        ),
                    );
                    info!(
                        target: "codex_remote::feishu",
                        event = "feishu_stream_send_ok",
                        thread_id,
                        item_id,
                        kind = current.kind.as_str(),
                        account_id = current.account_id.as_str(),
                        chat_id = current.chat_id.as_str(),
                        receive_id_type = current.receive_id_type.as_str(),
                        receive_id = current.receive_id.as_str(),
                        message_id = current.message_id.as_deref().unwrap_or(""),
                        card_id = current.card_id.as_deref().unwrap_or(""),
                        sequence = current.sequence,
                        text_len = current.text.len(),
                        sent_text_len = current.sent_text.len(),
                        completed = current.completed,
                        dirty = current.dirty,
                        should_continue,
                        should_remove,
                        preview = %log_preview(&current.text),
                        tail = %log_tail(&current.text),
                        "Feishu streaming send succeeded"
                    );
                }
                if should_remove {
                    runtime.feishu_streaming_cards_by_item.remove(&state_key);
                    return Ok(());
                }
                if should_continue {
                    continue;
                }
                return Ok(());
            }
            Err(err) => {
                let mut runtime = state.runtime.lock().await;
                if let Some(current) = runtime.feishu_streaming_cards_by_item.get_mut(&state_key) {
                    current.sending = false;
                    current.dirty = true;
                    write_stream_log(
                        "send_failed",
                        thread_id,
                        item_id,
                        current,
                        format!("err={}", err),
                    );
                    warn!(
                        target: "codex_remote::feishu",
                        event = "feishu_stream_send_failed",
                        thread_id,
                        item_id,
                        kind = current.kind.as_str(),
                        account_id = current.account_id.as_str(),
                        chat_id = current.chat_id.as_str(),
                        receive_id_type = current.receive_id_type.as_str(),
                        receive_id = current.receive_id.as_str(),
                        message_id = current.message_id.as_deref().unwrap_or(""),
                        card_id = current.card_id.as_deref().unwrap_or(""),
                        sequence = current.sequence,
                        text_len = current.text.len(),
                        sent_text_len = current.sent_text.len(),
                        completed = current.completed,
                        err = %err,
                        preview = %log_preview(&current.text),
                        tail = %log_tail(&current.text),
                        "Feishu streaming send failed"
                    );
                }
                return Err(err.to_string());
            }
        }
    }
}

async fn send_agent_message_cardkit(
    api: &FeishuApi,
    state_snapshot: &FeishuStreamingCardState,
) -> Result<(Option<String>, Option<String>, u64)> {
    let (card_id, message_id, next_sequence) =
        ensure_cardkit_streaming_card(api, state_snapshot).await?;
    if state_snapshot.completed {
        let final_card = build_streaming_card(&state_snapshot.kind, &state_snapshot.text, true);
        api.update_cardkit_card(&card_id, &final_card, next_sequence)
            .await?;
        api.set_cardkit_streaming_mode(&card_id, false, next_sequence.saturating_add(1))
            .await?;
        Ok((message_id, Some(card_id), next_sequence.saturating_add(1)))
    } else if state_snapshot.text.trim().is_empty() {
        Ok((message_id, Some(card_id), next_sequence))
    } else {
        api.stream_cardkit_element_content(
            &card_id,
            FEISHU_CARDKIT_STREAMING_ELEMENT_ID,
            &state_snapshot.text,
            next_sequence,
        )
        .await?;
        Ok((message_id, Some(card_id), next_sequence))
    }
}

async fn ensure_cardkit_streaming_card(
    api: &FeishuApi,
    state_snapshot: &FeishuStreamingCardState,
) -> Result<(String, Option<String>, u64)> {
    if let Some(card_id) = state_snapshot.card_id.clone() {
        return Ok((
            card_id,
            state_snapshot.message_id.clone(),
            state_snapshot.sequence.saturating_add(1),
        ));
    }
    let card = build_cardkit_streaming_card(&state_snapshot.kind);
    let card_id = api.create_cardkit_card(&card).await?;
    let message_id = api
        .send_cardkit_message_to(
            &state_snapshot.receive_id_type,
            &state_snapshot.receive_id,
            &card_id,
        )
        .await?;
    Ok((card_id, Some(message_id), 1))
}

async fn send_interactive_streaming_card(
    api: &FeishuApi,
    state_snapshot: &FeishuStreamingCardState,
) -> Result<(Option<String>, Option<String>, u64)> {
    let card = build_streaming_card(
        &state_snapshot.kind,
        &state_snapshot.text,
        state_snapshot.completed,
    );
    match state_snapshot.message_id.as_deref() {
        Some(message_id) => {
            api.update_interactive_message(message_id, &card).await?;
            Ok((Some(message_id.to_string()), None, state_snapshot.sequence))
        }
        None => {
            let message_id = api
                .send_interactive_message_to(
                    &state_snapshot.receive_id_type,
                    &state_snapshot.receive_id,
                    &card,
                )
                .await?;
            Ok((Some(message_id), None, state_snapshot.sequence))
        }
    }
}

fn build_streaming_card(kind: &str, text: &str, completed: bool) -> serde_json::Value {
    match kind {
        "plan" => renderer::build_streaming_plan_card(text, completed),
        "reasoning" => renderer::build_streaming_reasoning_card(text, completed),
        "commandExecution" => renderer::build_streaming_command_card(text, completed),
        "fileChange" => renderer::build_streaming_file_change_card(text, completed),
        "fileSummary" => renderer::build_streaming_file_summary_card(text, completed),
        "mcpToolCall" => renderer::build_streaming_mcp_tool_card(text, completed),
        _ => renderer::build_streaming_reply_card(text, completed),
    }
}

fn should_skip_empty_streaming_send(state: &FeishuStreamingCardState) -> bool {
    state.text.trim().is_empty()
        && state.sent_text.is_empty()
        && state.message_id.is_none()
        && state.card_id.is_none()
}

fn build_cardkit_streaming_card(kind: &str) -> serde_json::Value {
    match kind {
        "agentMessage" => build_cardkit_streaming_agent_message_card(),
        "commandExecution" => build_cardkit_streaming_tool_card("命令执行", "lime", "状态：执行中"),
        "fileChange" => build_cardkit_streaming_tool_card("文件变更", "green", "状态：处理中"),
        "fileSummary" => build_cardkit_streaming_reply_card(),
        _ => build_cardkit_streaming_reply_card(),
    }
}

#[cfg(test)]
mod tests {
    use super::should_skip_empty_streaming_send;
    use crate::im::feishu::types::FeishuStreamingCardState;

    fn streaming_state() -> FeishuStreamingCardState {
        FeishuStreamingCardState {
            account_id: "default".to_string(),
            chat_id: "chat".to_string(),
            receive_id_type: "chat_id".to_string(),
            receive_id: "chat".to_string(),
            kind: "reasoning".to_string(),
            message_id: None,
            card_id: None,
            sequence: 0,
            text: String::new(),
            sent_text: String::new(),
            completed: false,
            sending: false,
            dirty: true,
            last_sent_at: None,
        }
    }

    #[test]
    fn empty_first_send_is_skipped() {
        let state = streaming_state();
        assert!(should_skip_empty_streaming_send(&state));
    }

    #[test]
    fn empty_update_after_message_exists_is_not_skipped() {
        let mut state = streaming_state();
        state.message_id = Some("om_123".to_string());
        assert!(!should_skip_empty_streaming_send(&state));
    }
}

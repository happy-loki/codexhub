use serde_json::Value;

use crate::{
    im::core::i18n::ImText,
    im_runtime::{
        TelegramCollabProgressSnapshot, TelegramCollabProgressStatus, TelegramCollabProgressUpdate,
    },
};

const TELEGRAM_COLLAB_VISIBLE_AGENTS: usize = 8;
const TELEGRAM_COLLAB_NAME_CHARS: usize = 72;
const TELEGRAM_COLLAB_DETAIL_CHARS: usize = 180;
pub(crate) const TELEGRAM_COLLAB_PROGRESS_MAX_CHARS: usize = 3_800;

pub(crate) fn is_collab_item_type(item_type: &str) -> bool {
    matches!(item_type, "subAgentActivity" | "collabAgentToolCall")
}

pub(crate) fn updates_for_item(item: &Value, now_ms: u128) -> Vec<TelegramCollabProgressUpdate> {
    match item.get("type").and_then(Value::as_str) {
        Some("subAgentActivity") => subagent_activity_update(item, now_ms).into_iter().collect(),
        Some("collabAgentToolCall") => collab_tool_updates(item, now_ms),
        _ => Vec::new(),
    }
}

pub(crate) fn render_collab_progress(
    snapshot: &TelegramCollabProgressSnapshot,
    text: ImText,
) -> String {
    let running = snapshot
        .entries
        .iter()
        .filter(|entry| entry.status == TelegramCollabProgressStatus::Running)
        .count();
    let failed = snapshot
        .entries
        .iter()
        .filter(|entry| entry.status == TelegramCollabProgressStatus::Failed)
        .count();
    let interrupted = snapshot
        .entries
        .iter()
        .filter(|entry| entry.status == TelegramCollabProgressStatus::Interrupted)
        .count();
    let total = snapshot
        .dropped_entries
        .saturating_add(snapshot.entries.len());
    let mut sections = vec![text.telegram_collab_progress_title(
        snapshot.completed,
        total,
        failed,
        running,
        interrupted,
    )];

    let selected = selected_entry_indices(snapshot);
    for index in &selected {
        let entry = &snapshot.entries[*index];
        let icon = match entry.status {
            TelegramCollabProgressStatus::Running => "⏳",
            TelegramCollabProgressStatus::Responded => "✅",
            TelegramCollabProgressStatus::Succeeded => "✅",
            TelegramCollabProgressStatus::Failed => "❌",
            TelegramCollabProgressStatus::Interrupted => "⚠️",
        };
        let name = truncate_inline(&entry.name, TELEGRAM_COLLAB_NAME_CHARS);
        let mut line = format!("{icon} `{name}`");
        if entry.status != TelegramCollabProgressStatus::Running {
            let duration_ms = entry.updated_at_ms.saturating_sub(entry.started_at_ms);
            if duration_ms > 0 {
                line.push_str(" · ");
                line.push_str(&text.telegram_collab_duration(duration_ms));
            }
        }
        if let Some(detail) = entry.detail.as_deref() {
            line.push_str("\n└ ");
            line.push_str(&truncate_inline(detail, TELEGRAM_COLLAB_DETAIL_CHARS));
        }
        sections.push(line);
    }

    let omitted = total.saturating_sub(selected.len());
    if omitted > 0 {
        sections.push(text.telegram_collab_progress_omitted(omitted));
    }
    let rendered = sections.join("\n\n");
    debug_assert!(rendered.chars().count() <= TELEGRAM_COLLAB_PROGRESS_MAX_CHARS);
    rendered
}

fn subagent_activity_update(item: &Value, now_ms: u128) -> Option<TelegramCollabProgressUpdate> {
    let agent_id = string_field(item, "agentThreadId")?;
    let name = string_field(item, "agentPath").and_then(|path| {
        path.rsplit('/')
            .find(|part| !part.trim().is_empty())
            .map(str::to_string)
    });
    let kind = string_field(item, "kind")?.to_ascii_lowercase();
    let status = match kind.as_str() {
        "started" => TelegramCollabProgressStatus::Running,
        "completed" => TelegramCollabProgressStatus::Succeeded,
        "failed" | "blocked" => TelegramCollabProgressStatus::Failed,
        "interrupted" | "cancelled" | "canceled" => TelegramCollabProgressStatus::Interrupted,
        // "interacted" means the agent returned a message to its parent. It
        // is useful progress without claiming that the child thread closed.
        "interacted" => TelegramCollabProgressStatus::Responded,
        _ => return None,
    };
    let detail = first_nonempty_string(item, &["message", "summary", "error"])
        .map(|value| truncate_inline(&value, TELEGRAM_COLLAB_DETAIL_CHARS));
    Some(TelegramCollabProgressUpdate {
        agent_id,
        name,
        status: Some(status),
        detail,
        occurred_at_ms: timestamp_field(item).unwrap_or(now_ms),
        create_if_missing: true,
        restart: false,
    })
}

fn collab_tool_updates(item: &Value, now_ms: u128) -> Vec<TelegramCollabProgressUpdate> {
    let mut updates = Vec::new();
    if let Some(states) = item.get("agentsStates").and_then(Value::as_object) {
        for (agent_id, state) in states {
            let status_text = state
                .get("status")
                .and_then(Value::as_str)
                .or_else(|| state.as_str());
            let status = status_text.and_then(agent_status);
            let name =
                first_nonempty_string(state, &["agentPath", "path", "name"]).and_then(|path| {
                    path.rsplit('/')
                        .find(|part| !part.trim().is_empty())
                        .map(str::to_string)
                });
            let detail = first_nonempty_string(state, &["message", "summary", "error"])
                .map(|value| truncate_inline(&value, TELEGRAM_COLLAB_DETAIL_CHARS));
            updates.push(TelegramCollabProgressUpdate {
                agent_id: agent_id.clone(),
                create_if_missing: name.is_some() && status.is_some(),
                name,
                status,
                detail,
                occurred_at_ms: timestamp_field(state).unwrap_or(now_ms),
                restart: false,
            });
        }
    }

    let tool = string_field(item, "tool")
        .map(|tool| normalize_tool_name(&tool))
        .unwrap_or_default();
    let mut detail = first_nonempty_string(item, &["prompt", "message"])
        .map(|value| truncate_inline(&value, TELEGRAM_COLLAB_DETAIL_CHARS));
    let (mut status, mut restart) = match tool.as_str() {
        "spawnagent" => (Some(TelegramCollabProgressStatus::Running), false),
        "resumeagent" => (Some(TelegramCollabProgressStatus::Running), true),
        "closeagent" | "interruptagent" => (Some(TelegramCollabProgressStatus::Interrupted), false),
        "sendinput" | "sendmessage" => (Some(TelegramCollabProgressStatus::Running), true),
        // wait/list calls only synchronize agentsStates above. Empty polling
        // results intentionally produce no visible Telegram update.
        _ => return updates,
    };
    let tool_failed = string_field(item, "status").is_some_and(|value| {
        matches!(
            normalize_tool_name(&value).as_str(),
            "failed" | "failure" | "error" | "errored" | "blocked"
        )
    });
    if tool_failed {
        status = Some(TelegramCollabProgressStatus::Failed);
        restart = false;
        detail = collab_tool_error_detail(item);
    }
    let timestamp = timestamp_field(item).unwrap_or(now_ms);
    let mut receiver_count = 0;
    if let Some(receivers) = item.get("receiverThreadIds").and_then(Value::as_array) {
        for agent_id in receivers.iter().filter_map(Value::as_str) {
            receiver_count += 1;
            updates.push(TelegramCollabProgressUpdate {
                agent_id: agent_id.to_string(),
                name: None,
                status,
                detail: detail.clone(),
                occurred_at_ms: timestamp,
                create_if_missing: false,
                restart,
            });
        }
    }
    if tool_failed
        && tool == "spawnagent"
        && receiver_count == 0
        && let Some(agent_id) = first_nonempty_string(item, &["agentThreadId", "id"])
    {
        let name = first_nonempty_string(item, &["agentPath", "path", "name"])
            .and_then(|path| {
                path.rsplit('/')
                    .find(|part| !part.trim().is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "spawn_agent".to_string());
        updates.push(TelegramCollabProgressUpdate {
            agent_id,
            name: Some(name),
            status,
            detail,
            occurred_at_ms: timestamp,
            create_if_missing: true,
            restart: false,
        });
    }
    updates
}

fn collab_tool_error_detail(item: &Value) -> Option<String> {
    let text = item
        .get("error")
        .filter(|value| !value.is_null())
        .and_then(|error| {
            error
                .as_str()
                .map(str::to_string)
                .or_else(|| {
                    first_nonempty_string(error, &["message", "error", "detail", "summary"])
                })
                .or_else(|| serde_json::to_string(error).ok())
        })
        .or_else(|| first_nonempty_string(item, &["message", "summary"]))?;
    Some(truncate_inline(&text, TELEGRAM_COLLAB_DETAIL_CHARS))
}

fn selected_entry_indices(snapshot: &TelegramCollabProgressSnapshot) -> Vec<usize> {
    let mut selected = Vec::new();
    for status in [
        TelegramCollabProgressStatus::Running,
        TelegramCollabProgressStatus::Failed,
        TelegramCollabProgressStatus::Interrupted,
    ] {
        for (index, entry) in snapshot.entries.iter().enumerate() {
            if entry.status == status && selected.len() < TELEGRAM_COLLAB_VISIBLE_AGENTS {
                selected.push(index);
            }
        }
    }
    for (index, entry) in snapshot.entries.iter().enumerate().rev() {
        if matches!(
            entry.status,
            TelegramCollabProgressStatus::Responded | TelegramCollabProgressStatus::Succeeded
        ) && selected.len() < TELEGRAM_COLLAB_VISIBLE_AGENTS
        {
            selected.push(index);
        }
    }
    selected.sort_unstable();
    selected
}

fn agent_status(value: &str) -> Option<TelegramCollabProgressStatus> {
    match value.trim().to_ascii_lowercase().as_str() {
        "running" | "inprogress" | "in_progress" | "pending" | "waiting" => {
            Some(TelegramCollabProgressStatus::Running)
        }
        "completed" | "complete" | "succeeded" | "success" | "done" => {
            Some(TelegramCollabProgressStatus::Succeeded)
        }
        "failed" | "failure" | "error" | "errored" | "blocked" => {
            Some(TelegramCollabProgressStatus::Failed)
        }
        "interrupted" | "cancelled" | "canceled" | "closed" | "shutdown" => {
            Some(TelegramCollabProgressStatus::Interrupted)
        }
        _ => None,
    }
}

fn timestamp_field(item: &Value) -> Option<u128> {
    item.get("occurredAtMs")
        .or_else(|| item.get("updatedAtMs"))
        .or_else(|| item.get("completedAtMs"))
        .and_then(Value::as_u64)
        .map(u128::from)
}

fn first_nonempty_string(item: &Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| string_field(item, field))
}

fn string_field(item: &Value, field: &str) -> Option<String> {
    item.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_tool_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn truncate_inline(value: &str, max_chars: usize) -> String {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "'");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::im_runtime::TelegramCollabProgressEntry;

    #[test]
    fn parses_subagent_activity_without_exposing_path_or_call_id() {
        let updates = updates_for_item(
            &json!({
                "type": "subAgentActivity",
                "id": "call-secret",
                "agentThreadId": "thread-secret",
                "agentPath": "/root/trace_collab_render",
                "kind": "started"
            }),
            1_000,
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].agent_id, "thread-secret");
        assert_eq!(updates[0].name.as_deref(), Some("trace_collab_render"));
        assert_eq!(
            updates[0].status,
            Some(TelegramCollabProgressStatus::Running)
        );
    }

    #[test]
    fn unknown_subagent_activity_is_silent() {
        assert!(
            updates_for_item(
                &json!({
                    "type": "subAgentActivity",
                    "agentThreadId": "thread-secret",
                    "agentPath": "/root/review",
                    "kind": "futureKind"
                }),
                1_000,
            )
            .is_empty()
        );
    }

    #[test]
    fn failed_collab_tool_does_not_apply_its_optimistic_status() {
        let updates = updates_for_item(
            &json!({
                "type": "collabAgentToolCall",
                "tool": "resumeAgent",
                "status": "failed",
                "receiverThreadIds": ["agent-a"],
                "error": {"message": "service unavailable"},
                "occurredAtMs": 2_000
            }),
            3_000,
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].agent_id, "agent-a");
        assert_eq!(
            updates[0].status,
            Some(TelegramCollabProgressStatus::Failed)
        );
        assert!(!updates[0].restart);
        assert_eq!(updates[0].detail.as_deref(), Some("service unavailable"));
        assert_eq!(updates[0].occurred_at_ms, 2_000);
    }

    #[test]
    fn failed_spawn_without_receiver_is_visible_without_rendering_its_call_id() {
        let updates = updates_for_item(
            &json!({
                "type": "collabAgentToolCall",
                "id": "call-secret",
                "tool": "spawnAgent",
                "status": "failed",
                "receiverThreadIds": [],
                "error": "503 Service Unavailable"
            }),
            4_000,
        );
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].agent_id, "call-secret");
        assert_eq!(updates[0].name.as_deref(), Some("spawn_agent"));
        assert_eq!(
            updates[0].status,
            Some(TelegramCollabProgressStatus::Failed)
        );
        assert_eq!(
            updates[0].detail.as_deref(),
            Some("503 Service Unavailable")
        );
    }

    #[test]
    fn empty_wait_is_silent_and_wait_states_batch_updates() {
        assert!(
            updates_for_item(
                &json!({
                    "type": "collabAgentToolCall",
                    "tool": "wait",
                    "receiverThreadIds": [],
                    "agentsStates": {}
                }),
                1_000,
            )
            .is_empty()
        );
        let updates = updates_for_item(
            &json!({
                "type": "collabAgentToolCall",
                "tool": "wait",
                "agentsStates": {
                    "agent-a": {"status": "completed", "message": "done"},
                    "agent-b": {"status": "blocked", "message": "needs input"}
                }
            }),
            2_000,
        );
        assert_eq!(updates.len(), 2);
        assert_eq!(
            updates[0].status,
            Some(TelegramCollabProgressStatus::Succeeded)
        );
        assert_eq!(
            updates[1].status,
            Some(TelegramCollabProgressStatus::Failed)
        );
    }

    #[test]
    fn render_is_compact_and_hides_internal_ids() {
        let entries = (0..20)
            .map(|index| TelegramCollabProgressEntry {
                agent_id: format!("secret-thread-{index}"),
                name: format!("review_agent_{index}"),
                status: if index == 0 {
                    TelegramCollabProgressStatus::Failed
                } else {
                    TelegramCollabProgressStatus::Succeeded
                },
                detail: Some("x".repeat(1_000)),
                started_at_ms: 1_000,
                updated_at_ms: 43_000,
            })
            .collect();
        let rendered = render_collab_progress(
            &TelegramCollabProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: Some("77".to_string()),
                entries,
                dropped_entries: 0,
                completed: true,
            },
            ImText::zh_cn(),
        );
        assert!(rendered.contains("review_agent_0"));
        assert!(!rendered.contains("secret-thread"));
        assert!(!rendered.contains("call-secret"));
        assert!(rendered.chars().count() <= TELEGRAM_COLLAB_PROGRESS_MAX_CHARS);
        assert!(rendered.contains("另外 12 个子代理"));
    }
}

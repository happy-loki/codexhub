use serde_json::Value;

use crate::{
    im::core::i18n::ImText,
    im_runtime::{
        TelegramCommandProgressEntry, TelegramCommandProgressSnapshot,
        TelegramCommandProgressStatus, TelegramDiffSummary, TelegramPlanStep,
        TelegramPlanStepStatus,
    },
};

const TELEGRAM_COMMAND_PROGRESS_VISIBLE_STEPS: usize = 5;
const TELEGRAM_COMMAND_PROGRESS_COMMAND_CHARS: usize = 180;
const TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS: usize = 480;
const TELEGRAM_COMMAND_PROGRESS_FAILURE_LINES: usize = 6;
const TELEGRAM_COMMAND_PROGRESS_RETRY_ERROR_CHARS: usize = 600;
const TELEGRAM_REASONING_RENDER_CHARS: usize = 720;
const TELEGRAM_PLAN_RENDER_STEPS: usize = 6;
const TELEGRAM_PLAN_STEP_CHARS: usize = 180;
const TELEGRAM_DIFF_RENDER_PATHS: usize = 8;
const TELEGRAM_DIFF_PATH_CHARS: usize = 180;
const TELEGRAM_DIFF_MAX_PATHS: usize = 128;
pub(crate) const TELEGRAM_COMMAND_PROGRESS_MAX_CHARS: usize = 3_800;

pub(crate) fn reasoning_summary_from_item(item: &Value) -> Option<String> {
    let mut parts = Vec::new();
    for key in ["summary", "content"] {
        let Some(values) = item.get(key).and_then(Value::as_array) else {
            continue;
        };
        for value in values {
            let text = value
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
                .map(str::trim)
                .filter(|text| !text.is_empty());
            if let Some(text) = text
                && !parts.iter().any(|part: &String| part == text)
            {
                parts.push(text.to_string());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

pub(crate) fn plan_from_params(params: &Value) -> (Option<String>, Vec<TelegramPlanStep>) {
    let explanation = params
        .get("explanation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let steps = params
        .get("plan")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let step = value
                .get("step")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let status = match value.get("status").and_then(Value::as_str) {
                Some("completed" | "done") => TelegramPlanStepStatus::Completed,
                Some("inProgress" | "in_progress") => TelegramPlanStepStatus::InProgress,
                _ => TelegramPlanStepStatus::Pending,
            };
            Some(TelegramPlanStep {
                step: truncate_middle(
                    &single_line(step).replace('`', "'"),
                    TELEGRAM_PLAN_STEP_CHARS,
                ),
                status,
            })
        })
        .collect();
    (explanation, steps)
}

pub(crate) fn parse_plan_update(params: &Value) -> (Option<String>, Vec<TelegramPlanStep>) {
    plan_from_params(params)
}

pub(crate) fn plan_from_item(item: &Value) -> (Option<String>, Vec<TelegramPlanStep>) {
    let explanation = item
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| item.get("summary").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    (explanation, Vec::new())
}

pub(crate) fn diff_summary_from_item(item: &Value) -> Option<TelegramDiffSummary> {
    let changes = item.get("changes").and_then(Value::as_array)?;
    let mut paths = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for change in changes {
        let path = change
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .unwrap_or("unknown");
        let move_path = change
            .get("kind")
            .and_then(|kind| kind.get("move_path"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty());
        let display_path = match move_path {
            Some(move_path) if move_path != path => format!("{path} -> {move_path}"),
            _ => path.to_string(),
        };
        if paths.len() < TELEGRAM_DIFF_MAX_PATHS {
            push_unique_path(&mut paths, display_path);
        }
        if let Some(diff) = change.get("diff").and_then(Value::as_str) {
            let (added, removed) = diff_line_stats(diff);
            additions = additions.saturating_add(added);
            deletions = deletions.saturating_add(removed);
        }
    }
    let file_count = changes.len().max(paths.len());
    (file_count > 0).then(|| TelegramDiffSummary {
        file_count,
        additions,
        deletions,
        omitted_paths: file_count.saturating_sub(paths.len()),
        paths,
    })
}

pub(crate) fn file_change_diff_summary(item: &Value) -> Option<TelegramDiffSummary> {
    diff_summary_from_item(item)
}

pub(crate) fn diff_summary_from_diff(diff: &str) -> Option<TelegramDiffSummary> {
    if diff.trim().is_empty() {
        return None;
    }
    let mut paths = Vec::new();
    let mut file_count = 0usize;
    let mut pending_old_path = None;
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut in_hunk = false;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            file_count = file_count.saturating_add(1);
            if let Some(path) = diff_git_target_path(rest) {
                push_unique_path(&mut paths, path);
            }
            pending_old_path = None;
            in_hunk = false;
        } else if let Some(rest) = line.strip_prefix("--- ") {
            pending_old_path = diff_header_path(rest, 'a');
            if pending_old_path.as_deref() == Some("/dev/null") {
                pending_old_path = None;
            }
            if file_count == 0 {
                file_count = file_count.saturating_add(1);
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            let new_path = diff_header_path(rest, 'b');
            let path = new_path
                .filter(|path| path != "/dev/null")
                .or(pending_old_path.take());
            if let Some(path) = path.filter(|path| path != "/dev/null") {
                push_unique_path(&mut paths, path);
            }
        } else if line.starts_with("@@") {
            in_hunk = true;
        } else if let Some(rest) = line.strip_prefix("rename to ") {
            push_unique_path(&mut paths, rest.trim().to_string());
        } else if in_hunk && line.starts_with('+') {
            additions = additions.saturating_add(1);
        } else if in_hunk && line.starts_with('-') {
            deletions = deletions.saturating_add(1);
        }
    }
    file_count = file_count.max(paths.len());
    (file_count > 0).then(|| TelegramDiffSummary {
        file_count,
        additions,
        deletions,
        omitted_paths: file_count.saturating_sub(paths.len()),
        paths,
    })
}

fn diff_line_stats(diff: &str) -> (usize, usize) {
    let mut in_hunk = false;
    let mut additions = 0usize;
    let mut deletions = 0usize;
    for line in diff.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk && line.starts_with('+') {
            additions = additions.saturating_add(1);
        } else if in_hunk && line.starts_with('-') {
            deletions = deletions.saturating_add(1);
        }
    }
    (additions, deletions)
}

fn diff_git_target_path(rest: &str) -> Option<String> {
    let index = rest.rfind(" b/")?;
    Some(rest[index + 3..].trim_matches('"').to_string())
}

fn diff_header_path(rest: &str, prefix: char) -> Option<String> {
    let raw = rest.split('\t').next()?.trim().trim_matches('"');
    let prefix = format!("{prefix}/");
    Some(raw.strip_prefix(&prefix).unwrap_or(raw).to_string())
}

fn push_unique_path(paths: &mut Vec<String>, path: String) {
    let path = path.trim().trim_matches('"');
    if path.is_empty()
        || path == "/dev/null"
        || paths.len() >= TELEGRAM_DIFF_MAX_PATHS
        || paths.iter().any(|current| current == path)
    {
        return;
    }
    paths.push(path.to_string());
}

pub(crate) fn running_entry(item_id: &str, item: &Value) -> TelegramCommandProgressEntry {
    TelegramCommandProgressEntry {
        item_id: item_id.to_string(),
        command: command_text(item),
        status: TelegramCommandProgressStatus::Running,
        exit_code: None,
        duration_ms: item.get("durationMs").and_then(Value::as_u64),
        failure_output: None,
    }
}

pub(crate) fn completed_entry(item_id: &str, item: &Value) -> TelegramCommandProgressEntry {
    let status = completed_status(item);
    TelegramCommandProgressEntry {
        item_id: item_id.to_string(),
        command: command_text(item),
        status,
        exit_code: item.get("exitCode").and_then(Value::as_i64),
        duration_ms: item.get("durationMs").and_then(Value::as_u64),
        failure_output: (status == TelegramCommandProgressStatus::Failed)
            .then(|| failure_output_tail(item))
            .flatten(),
    }
}

pub(crate) fn render_command_progress(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> String {
    let mut selected = selected_entry_indices(&snapshot.entries);
    let mut failure_output_chars = TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS;
    let mut retry_error_chars = TELEGRAM_COMMAND_PROGRESS_RETRY_ERROR_CHARS;

    loop {
        let rendered = render_command_progress_with_limits(
            snapshot,
            text,
            &selected,
            failure_output_chars,
            retry_error_chars,
        );
        if rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
            return rendered;
        }

        let has_failure_output = selected.iter().any(|index| {
            snapshot.entries[*index]
                .failure_output
                .as_deref()
                .is_some_and(|output| !output.is_empty())
        });
        if failure_output_chars > 0 && has_failure_output {
            let without_failure_output = render_command_progress_with_limits(
                snapshot,
                text,
                &selected,
                0,
                retry_error_chars,
            );
            if without_failure_output.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
                let mut lower = 0;
                let mut upper = failure_output_chars;
                while lower < upper {
                    let candidate = lower + (upper - lower).div_ceil(2);
                    let candidate_rendered = render_command_progress_with_limits(
                        snapshot,
                        text,
                        &selected,
                        candidate,
                        retry_error_chars,
                    );
                    if candidate_rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
                        lower = candidate;
                    } else {
                        upper = candidate - 1;
                    }
                }
                return render_command_progress_with_limits(
                    snapshot,
                    text,
                    &selected,
                    lower,
                    retry_error_chars,
                );
            }
            failure_output_chars = 0;
        }

        if let Some(position) = least_important_entry_position(snapshot, &selected) {
            selected.remove(position);
            failure_output_chars = TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS;
            continue;
        }

        if retry_error_chars > 0 && snapshot.retry_count > 0 && snapshot.retry_error.is_some() {
            let without_retry_error =
                render_command_progress_with_limits(snapshot, text, &selected, 0, 0);
            if without_retry_error.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
                let mut lower = 0;
                let mut upper = retry_error_chars;
                while lower < upper {
                    let candidate = lower + (upper - lower).div_ceil(2);
                    let candidate_rendered = render_command_progress_with_limits(
                        snapshot, text, &selected, 0, candidate,
                    );
                    if candidate_rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS {
                        lower = candidate;
                    } else {
                        upper = candidate - 1;
                    }
                }
                return render_command_progress_with_limits(snapshot, text, &selected, 0, lower);
            }
            retry_error_chars = 0;
            continue;
        }

        return truncate_middle(&rendered, TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
    }
}

fn render_command_progress_with_limits(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
    selected: &[usize],
    failure_output_chars: usize,
    retry_error_chars: usize,
) -> String {
    let total = snapshot
        .dropped_entries
        .saturating_add(snapshot.entries.len());
    let failed = snapshot
        .entries
        .iter()
        .filter(|entry| entry.status == TelegramCommandProgressStatus::Failed)
        .count();
    let running = snapshot
        .entries
        .iter()
        .filter(|entry| entry.status == TelegramCommandProgressStatus::Running)
        .count();
    let interrupted = snapshot
        .entries
        .iter()
        .filter(|entry| entry.status == TelegramCommandProgressStatus::Interrupted)
        .count();
    let omitted = total.saturating_sub(selected.len());
    let has_supplemental = snapshot.reasoning_summary.is_some()
        || snapshot.plan_explanation.is_some()
        || !snapshot.plan.is_empty()
        || snapshot.diff_summary.is_some();
    let mut sections = if total == 0 && snapshot.retry_count > 0 {
        vec![text.telegram_retry_progress_title(
            snapshot.completed,
            snapshot.failed,
            snapshot.retry_count,
        )]
    } else if total == 0 && has_supplemental {
        vec![
            text.telegram_task_progress_title(snapshot.completed, snapshot.failed)
                .to_string(),
        ]
    } else {
        vec![text.telegram_command_progress_title(
            snapshot.completed,
            snapshot.failed,
            total,
            failed,
            running,
            interrupted,
        )]
    };
    if total > 0 && snapshot.retry_count > 0 {
        sections.push(text.telegram_retry_progress_summary(snapshot.retry_count));
    }
    if snapshot.retry_count > 0
        && let Some(error) = snapshot.retry_error.as_deref()
        && retry_error_chars > 0
    {
        let error = truncate_middle(&error.replace("```", "'''"), retry_error_chars);
        sections.push(format!(
            "{}\n```text\n{}\n```",
            text.telegram_retry_error_summary(),
            error
        ));
    }
    if omitted > 0 {
        sections.push(text.telegram_command_progress_omitted(omitted));
    }
    if let Some(supplemental) = render_supplemental_progress(snapshot, text) {
        sections.push(supplemental);
    }
    for index in selected {
        sections.push(render_entry(
            &snapshot.entries[*index],
            text,
            failure_output_chars,
        ));
    }
    sections.join("\n\n")
}

fn render_supplemental_progress(
    snapshot: &TelegramCommandProgressSnapshot,
    text: ImText,
) -> Option<String> {
    let mut sections = Vec::new();
    if snapshot.plan_explanation.is_some() || !snapshot.plan.is_empty() {
        let completed = snapshot
            .plan
            .iter()
            .filter(|step| step.status == TelegramPlanStepStatus::Completed)
            .count();
        let total = snapshot.plan.len();
        let mut lines = vec![text.telegram_plan_heading(completed, total)];
        if let Some(explanation) = snapshot.plan_explanation.as_deref() {
            lines.push(compact_text(explanation, TELEGRAM_PLAN_STEP_CHARS));
        }
        for step in snapshot.plan.iter().take(TELEGRAM_PLAN_RENDER_STEPS) {
            let status = match step.status {
                TelegramPlanStepStatus::Pending => "pending",
                TelegramPlanStepStatus::InProgress => "in_progress",
                TelegramPlanStepStatus::Completed => "completed",
            };
            lines.push(format!(
                "{} {}",
                text.telegram_plan_step_icon(status),
                compact_text(&step.step, TELEGRAM_PLAN_STEP_CHARS)
            ));
        }
        if snapshot.plan.len() > TELEGRAM_PLAN_RENDER_STEPS {
            lines
                .push(text.telegram_plan_omitted(snapshot.plan.len() - TELEGRAM_PLAN_RENDER_STEPS));
        }
        sections.push(lines.join("\n"));
    }
    if let Some(reasoning) = snapshot.reasoning_summary.as_deref() {
        let reasoning = compact_text(reasoning, TELEGRAM_REASONING_RENDER_CHARS);
        sections.push(format!(
            "{}\n{}",
            text.telegram_reasoning_heading(),
            reasoning
        ));
    }
    if let Some(diff) = snapshot.diff_summary.as_ref() {
        let mut lines =
            vec![text.telegram_diff_heading(diff.file_count, diff.additions, diff.deletions)];
        for path in diff.paths.iter().take(TELEGRAM_DIFF_RENDER_PATHS) {
            lines.push(format!(
                "• {}",
                compact_text(path, TELEGRAM_DIFF_PATH_CHARS)
            ));
        }
        let omitted = diff
            .omitted_paths
            .saturating_add(diff.paths.len().saturating_sub(TELEGRAM_DIFF_RENDER_PATHS));
        if omitted > 0 {
            lines.push(text.telegram_diff_omitted(omitted));
        }
        sections.push(lines.join("\n"));
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

fn selected_entry_indices(entries: &[TelegramCommandProgressEntry]) -> Vec<usize> {
    let mut selected = Vec::new();
    for status in [
        TelegramCommandProgressStatus::Running,
        TelegramCommandProgressStatus::Failed,
        TelegramCommandProgressStatus::Interrupted,
    ] {
        for (index, entry) in entries.iter().enumerate().rev() {
            if entry.status == status && !selected.contains(&index) {
                selected.push(index);
                if selected.len() == TELEGRAM_COMMAND_PROGRESS_VISIBLE_STEPS {
                    selected.sort_unstable();
                    return selected;
                }
            }
        }
    }
    for index in (0..entries.len()).rev() {
        if !selected.contains(&index) {
            selected.push(index);
            if selected.len() == TELEGRAM_COMMAND_PROGRESS_VISIBLE_STEPS {
                break;
            }
        }
    }
    selected.sort_unstable();
    selected
}

fn least_important_entry_position(
    snapshot: &TelegramCommandProgressSnapshot,
    selected: &[usize],
) -> Option<usize> {
    selected
        .iter()
        .enumerate()
        .min_by_key(|(_, index)| {
            let priority = match snapshot.entries[**index].status {
                TelegramCommandProgressStatus::Succeeded => 0,
                TelegramCommandProgressStatus::Interrupted => 1,
                TelegramCommandProgressStatus::Failed => 2,
                TelegramCommandProgressStatus::Running => 3,
            };
            (priority, **index)
        })
        .map(|(position, _)| position)
}

fn render_entry(
    entry: &TelegramCommandProgressEntry,
    text: ImText,
    failure_output_chars: usize,
) -> String {
    let icon = match entry.status {
        TelegramCommandProgressStatus::Running => "⏳",
        TelegramCommandProgressStatus::Interrupted => "⚠️",
        TelegramCommandProgressStatus::Succeeded => "✅",
        TelegramCommandProgressStatus::Failed => "❌",
    };
    let mut line = icon.to_string();
    if let Some(duration) = entry.duration_ms {
        line.push_str(" · ");
        line.push_str(&format_duration(duration));
    }
    if let Some(exit_code) = entry
        .exit_code
        .filter(|_| entry.status == TelegramCommandProgressStatus::Failed)
    {
        line.push_str(&format!(" · exit {exit_code}"));
    }
    line.push_str("\n```shell\n");
    line.push_str(&entry.command);
    line.push_str("\n```");
    if let Some(output) = entry.failure_output.as_deref()
        && failure_output_chars > 0
    {
        line.push_str("\n");
        line.push_str(text.telegram_command_progress_error_summary());
        line.push_str("\n```text\n");
        line.push_str(&truncate_tail(output, failure_output_chars));
        line.push_str("\n```");
    }
    line
}

fn command_text(item: &Value) -> String {
    let text = item
        .get("commandActions")
        .and_then(Value::as_array)
        .and_then(|actions| actions.first())
        .and_then(|action| action.get("command"))
        .and_then(command_value_text)
        .or_else(|| item.get("command").and_then(command_value_text))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "command".to_string());
    truncate_middle(
        &single_line(&text).replace('`', "'"),
        TELEGRAM_COMMAND_PROGRESS_COMMAND_CHARS,
    )
}

fn command_value_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.trim().to_string());
    }
    value.as_array().map(|parts| {
        parts
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn completed_status(item: &Value) -> TelegramCommandProgressStatus {
    if let Some(exit_code) = item.get("exitCode").and_then(Value::as_i64) {
        return if exit_code == 0 {
            TelegramCommandProgressStatus::Succeeded
        } else {
            TelegramCommandProgressStatus::Failed
        };
    }
    match item
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
    {
        "failed" | "error" | "canceled" | "cancelled" | "timed_out" | "timedout" => {
            TelegramCommandProgressStatus::Failed
        }
        _ => TelegramCommandProgressStatus::Succeeded,
    }
}

fn failure_output_tail(item: &Value) -> Option<String> {
    let output = item.get("aggregatedOutput").and_then(Value::as_str)?.trim();
    if output.is_empty() {
        return None;
    }
    let lines = output.lines().collect::<Vec<_>>();
    let start = lines
        .len()
        .saturating_sub(TELEGRAM_COMMAND_PROGRESS_FAILURE_LINES);
    let tail = lines[start..].join("\n").replace("```", "'''");
    Some(truncate_tail(
        &tail,
        TELEGRAM_COMMAND_PROGRESS_FAILURE_CHARS,
    ))
}

fn single_line(text: &str) -> String {
    text.replace("\r\n", " ")
        .replace(['\r', '\n'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact_text(text: &str, max_chars: usize) -> String {
    let normalized = text
        .replace("```", "'''")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let lines = normalized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    truncate_middle(&lines.join("\n"), max_chars)
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return text.chars().take(max_chars).collect();
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

fn truncate_tail(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return chars[chars.len().saturating_sub(max_chars)..]
            .iter()
            .collect();
    }
    format!(
        "...{}",
        chars[chars.len() - max_chars.saturating_sub(3)..]
            .iter()
            .collect::<String>()
    )
}

fn format_duration(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms}ms")
    } else {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{
        im::core::i18n::ImText,
        im_runtime::{TelegramCommandProgressSnapshot, TelegramCommandProgressStatus},
    };

    use super::{
        TELEGRAM_COMMAND_PROGRESS_MAX_CHARS, completed_entry, diff_summary_from_diff,
        file_change_diff_summary, parse_plan_update, reasoning_summary_from_item,
        render_command_progress, running_entry,
    };

    use crate::im_runtime::{TelegramDiffSummary, TelegramPlanStep, TelegramPlanStepStatus};

    #[test]
    fn parses_array_commands_and_keeps_failure_tail() {
        let item = json!({
            "commandActions": [{"command": ["cargo", "test", "--all"]}],
            "exitCode": 2,
            "durationMs": 1_250,
            "aggregatedOutput": "one\ntwo\nthree\nfour\nfive\nsix\nseven"
        });

        let entry = completed_entry("item-1", &item);

        assert_eq!(entry.command, "cargo test --all");
        assert_eq!(entry.status, TelegramCommandProgressStatus::Failed);
        assert_eq!(entry.duration_ms, Some(1_250));
        let output = entry.failure_output.expect("failure output");
        assert!(!output.contains("one"));
        assert!(output.contains("two"));
        assert!(output.contains("seven"));
    }

    #[test]
    fn render_prioritizes_running_and_failed_steps() {
        let mut entries = (0..8)
            .map(|index| {
                completed_entry(
                    &format!("item-{index}"),
                    &json!({"command": format!("command {index}"), "exitCode": 0}),
                )
            })
            .collect::<Vec<_>>();
        entries[1] = completed_entry(
            "item-1",
            &json!({"command": "failed early", "exitCode": 1, "aggregatedOutput": "boom"}),
        );
        entries[2] = running_entry("item-2", &json!({"command": "still running"}));
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 9,
                message_id: Some("42".to_string()),
                entries,
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("执行中"));
        assert!(rendered.contains("failed early"));
        assert!(rendered.contains("still running"));
        assert!(rendered.contains("另外 3 个较早步骤"));
        assert!(!rendered.contains("command 0"));
    }

    #[test]
    fn render_is_bounded_to_one_telegram_message() {
        let output = "x".repeat(20_000);
        let entries = (0..128)
            .map(|index| {
                completed_entry(
                    &format!("item-{index}"),
                    &json!({
                        "command": format!("{} {index}", "c".repeat(2_000)),
                        "exitCode": 1,
                        "aggregatedOutput": output
                    }),
                )
            })
            .collect();
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 128,
                message_id: None,
                entries,
                dropped_entries: 25,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        assert!(rendered.contains("执行完成"));
        assert!(rendered.contains("另外 148 个较早步骤"));
    }

    #[test]
    fn render_bounds_five_max_failures_with_a_max_retry_error() {
        let entries = (0..5)
            .map(|index| {
                completed_entry(
                    &format!("item-{index}"),
                    &json!({
                        "command": format!("{} command-tail-{index}", "命".repeat(2_000)),
                        "exitCode": 1,
                        "aggregatedOutput": format!(
                            "{} failure-tail-{index}",
                            "误".repeat(2_000)
                        )
                    }),
                )
            })
            .collect();
        let retry_error = "错".repeat(600);
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 10,
                message_id: Some("42".to_string()),
                entries,
                dropped_entries: 0,
                retry_count: 5,
                retry_error: Some(retry_error.clone()),
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                completed: true,
                failed: true,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        assert!(rendered.contains(&retry_error));
        for index in 0..5 {
            assert!(rendered.contains(&format!("command-tail-{index}")));
            assert!(rendered.contains(&format!("failure-tail-{index}")));
        }
    }

    #[test]
    fn render_distinguishes_an_interrupted_terminal_step() {
        let mut entry = running_entry("item", &json!({"command": "cargo test"}));
        entry.status = TelegramCommandProgressStatus::Interrupted;
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 2,
                message_id: Some("42".to_string()),
                entries: vec![entry],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("⚠️ 执行结束 · 1 步 · 1 个中断"));
        assert!(rendered.contains("⚠️\n```shell\ncargo test\n```"));
        assert!(!rendered.contains("进行中"));
    }

    #[test]
    fn render_marks_a_failed_turn_even_when_commands_succeeded() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 3,
                message_id: Some("42".to_string()),
                entries: vec![completed_entry(
                    "item",
                    &json!({"command": "cargo test", "exitCode": 0}),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                completed: true,
                failed: true,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("❌ 执行失败 · 1 步"));
        assert!(!rendered.contains("执行完成"));
    }

    #[test]
    fn render_retry_only_progress_and_terminal_state() {
        let mut snapshot = TelegramCommandProgressSnapshot {
            turn_id: "turn".to_string(),
            revision: 2,
            message_id: Some("42".to_string()),
            entries: Vec::new(),
            dropped_entries: 0,
            retry_count: 2,
            retry_error: Some("503 Service Unavailable".to_string()),
            reasoning_summary: None,
            plan_explanation: None,
            plan: Vec::new(),
            diff_summary: None,
            completed: false,
            failed: false,
        };

        let running = render_command_progress(&snapshot, ImText::zh_cn());
        assert!(running.contains("模型请求重试中 · 第 2 次"));
        assert!(running.contains("```text\n503 Service Unavailable\n```"));

        snapshot.completed = true;
        snapshot.failed = true;
        let failed = render_command_progress(&snapshot, ImText::zh_cn());
        assert!(failed.contains("模型请求失败 · 已重试 2 次"));
    }

    #[test]
    fn render_uses_native_shell_blocks_for_commands() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 1,
                message_id: None,
                entries: vec![running_entry(
                    "item",
                    &json!({"command": "printf 12345678"}),
                )],
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: None,
                plan_explanation: None,
                plan: Vec::new(),
                diff_summary: None,
                completed: false,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.contains("```shell\nprintf 12345678\n```"));
        assert!(!rendered.contains("`printf 12345678`"));
    }

    #[test]
    fn parse_plan_update_maps_protocol_statuses_and_skips_blank_steps() {
        let params = json!({
            "threadId": "thread",
            "turnId": "turn",
            "explanation": "  inspect, implement, verify  ",
            "plan": [
                {"step": " inspect ", "status": "completed"},
                {"step": "implement", "status": "inProgress"},
                {"step": "verify", "status": "pending"},
                {"step": " ", "status": "completed"}
            ]
        });

        let (explanation, plan) = parse_plan_update(&params);

        assert_eq!(explanation.as_deref(), Some("inspect, implement, verify"));
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].status, TelegramPlanStepStatus::Completed);
        assert_eq!(plan[1].status, TelegramPlanStepStatus::InProgress);
        assert_eq!(plan[2].status, TelegramPlanStepStatus::Pending);
        assert_eq!(plan[0].step, "inspect");
    }

    #[test]
    fn reasoning_summary_parser_accepts_protocol_strings_and_deduplicates_parts() {
        let item = json!({
            "type": "reasoning",
            "summary": ["first", "first", {"text": "second"}],
            "content": ["second", {"text": "third"}]
        });

        assert_eq!(
            reasoning_summary_from_item(&item).as_deref(),
            Some("first\n\nsecond\n\nthird")
        );
    }

    #[test]
    fn diff_summary_counts_only_unified_hunk_lines() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,3 @@\n-old\n+new\n+another\n+metadata-like-line\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1,2 +1,1 @@\n-removed\n-removed-again\n kept\n";

        let summary = diff_summary_from_diff(diff).expect("diff summary");

        assert_eq!(summary.file_count, 2);
        assert_eq!(summary.additions, 3);
        assert_eq!(summary.deletions, 3);
        assert_eq!(summary.paths, vec!["src/a.rs", "src/b.rs"]);
        assert_eq!(summary.omitted_paths, 0);
    }

    #[test]
    fn file_change_summary_includes_move_path_and_unified_stats() {
        let item = json!({
            "type": "fileChange",
            "changes": [{
                "path": "src/old.rs",
                "kind": {"type": "update", "move_path": "src/new.rs"},
                "diff": "--- a/src/old.rs\n+++ b/src/new.rs\n@@ -1 +1 @@\n-old\n+new\n"
            }]
        });

        let summary = file_change_diff_summary(&item).expect("file change summary");

        assert_eq!(summary.file_count, 1);
        assert_eq!(summary.additions, 1);
        assert_eq!(summary.deletions, 1);
        assert_eq!(summary.paths, vec!["src/old.rs -> src/new.rs"]);
    }

    #[test]
    fn render_includes_reasoning_plan_and_diff_as_compact_text() {
        let rendered = render_command_progress(
            &TelegramCommandProgressSnapshot {
                turn_id: "turn".to_string(),
                revision: 4,
                message_id: None,
                entries: Vec::new(),
                dropped_entries: 0,
                retry_count: 0,
                retry_error: None,
                reasoning_summary: Some("first thought\n\nsecond thought".to_string()),
                plan_explanation: Some("work in order".to_string()),
                plan: vec![
                    TelegramPlanStep {
                        step: "inspect".to_string(),
                        status: TelegramPlanStepStatus::Completed,
                    },
                    TelegramPlanStep {
                        step: "verify".to_string(),
                        status: TelegramPlanStepStatus::InProgress,
                    },
                ],
                diff_summary: Some(TelegramDiffSummary {
                    file_count: 1,
                    additions: 2,
                    deletions: 1,
                    paths: vec!["src/main.rs".to_string()],
                    omitted_paths: 0,
                }),
                completed: true,
                failed: false,
            },
            ImText::zh_cn(),
        );

        assert!(rendered.chars().count() <= TELEGRAM_COMMAND_PROGRESS_MAX_CHARS);
        assert!(rendered.contains("🧠 思考摘要"));
        assert!(rendered.contains("📋 计划 · 1/2"));
        assert!(rendered.contains("📝 文件修改 · 1 个文件 · +2 -1"));
        assert!(rendered.contains("• src/main.rs"));
        assert!(!rendered.contains("```diff"));
    }
}

use serde_json::Value as JsonValue;

use crate::im::core::thread::ThreadCreateDefaults;
use crate::im::feishu::types::FeishuUserInputQuestion;
use crate::im_runtime::ApprovalDecisionOption;

const DEFAULT_CARD_TEMPLATE: &str = "blue";
const APPROVAL_CARD_TEMPLATE: &str = "orange";
pub const FEISHU_CARDKIT_STREAMING_ELEMENT_ID: &str = "streaming_content";
const FEISHU_STREAMING_COMMAND_COMMAND_CHARS: usize = 600;
const FEISHU_STREAMING_COMMAND_OUTPUT_CHARS: usize = 2400;
const FEISHU_STREAMING_COMMAND_META_CHARS: usize = 320;

fn normalize_card_markdown(text: &str) -> String {
    text.replace("\r\n", "\n").trim().to_string()
}

fn pretty_json(value: &JsonValue) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn markdown_code_block(lang: &str, text: &str) -> String {
    format!("```{lang}\n{}\n```", normalize_card_markdown(text))
}

fn parse_unified_diff_stats(diff: &str) -> (usize, usize) {
    let normalized = diff.replace("\r\n", "\n");
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut in_hunk = false;

    for line in normalized.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        }
    }

    (additions, deletions)
}

fn count_text_lines(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let normalized = text.replace("\r\n", "\n");
    let parts = normalized.split('\n').collect::<Vec<_>>();
    if parts.is_empty() {
        0
    } else if parts.last().copied() == Some("") {
        parts.len().saturating_sub(1)
    } else {
        parts.len()
    }
}

fn file_change_stats(change: &JsonValue) -> (usize, usize) {
    let diff = change
        .get("diff")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    match change
        .get("kind")
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
    {
        Some("add") => (count_text_lines(diff), 0),
        Some("delete") => (0, count_text_lines(diff)),
        _ => parse_unified_diff_stats(diff),
    }
}

fn truncate_lines(text: &str, max_lines: usize, max_chars_per_line: usize) -> Option<String> {
    let normalized = text.replace("\r\n", "\n");
    let mut lines = Vec::new();
    for line in normalized.lines() {
        if lines.len() >= max_lines {
            break;
        }
        let mut out = String::new();
        let mut count = 0usize;
        for ch in line.chars() {
            if count >= max_chars_per_line {
                out.push('…');
                break;
            }
            out.push(ch);
            count += 1;
        }
        lines.push(out);
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let single_line = text
        .replace("\r\n", " ")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = String::new();
    let mut count = 0usize;
    for ch in single_line.chars() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
        count += 1;
    }
    if out.is_empty() {
        "command".to_string()
    } else {
        out
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut out = String::new();
    let mut count = 0usize;
    for ch in normalized.chars() {
        if count >= max_chars {
            out.push_str("…（已截断）");
            break;
        }
        out.push(ch);
        count += 1;
    }
    out
}

fn looks_like_image_data_url(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("data:image/") && trimmed.contains(";base64,")
}

fn looks_like_image_base64(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("iVBORw0KGgo")
        || trimmed.starts_with("/9j/")
        || trimmed.starts_with("R0lGOD")
        || trimmed.starts_with("UklGR")
}

fn image_element(image_key: &str, alt: &str) -> serde_json::Value {
    serde_json::json!({
        "tag": "img",
        "img_key": image_key,
        "alt": {
            "tag": "plain_text",
            "content": alt
        },
        "mode": "fit_horizontal",
        "preview": true
    })
}

fn summarize_command_header(text: &str) -> String {
    let single_line = text
        .replace("\r\n", " ")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if single_line.is_empty() {
        return "Ran: command".to_string();
    }

    let words = single_line.split_whitespace().collect::<Vec<_>>();
    let compact = if words.len() >= 4 {
        format!("{} {} {} {}", words[0], words[1], words[2], words[3])
    } else if words.len() >= 3 {
        format!("{} {} {}", words[0], words[1], words[2])
    } else if words.len() >= 2 {
        format!("{} {}", words[0], words[1])
    } else {
        words[0].to_string()
    };
    let suffix = if compact.chars().count() < single_line.chars().count() {
        " ..."
    } else {
        ""
    };
    format!(
        "{} {}{}",
        semantic_prefix("Ran", "green"),
        truncate_single_line(&compact, 44),
        suffix
    )
}

fn tool_header_background(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "commandExecution" => None,
        "read_file" => None,
        "list_dir" => None,
        "mcpToolCall" => None,
        _ => None,
    }
}

fn semantic_prefix(label: &str, color: &str) -> String {
    format!(
        "<font color='{color}'>{}</font>",
        normalize_card_markdown(label)
    )
}

fn collab_status_badge(status: &str) -> (&'static str, &'static str) {
    // Align with Codex protocol: CollabAgentToolCallStatus = inProgress | completed | failed
    match status {
        "completed" => ("green", "completed"),
        "failed" => ("red", "failed"),
        "inProgress" => ("blue", "inProgress"),
        _ => ("grey", "unknown"),
    }
}

fn collab_header_summary(item: &JsonValue) -> String {
    let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
    let status = item
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let receiver_count = item
        .get("receiverThreadIds")
        .and_then(|v| v.as_array())
        .map(|v| v.len())
        .unwrap_or(0);
    let model = item
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let effort = item
        .get("reasoningEffort")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let model_tag = match (model, effort) {
        (Some(m), Some(e)) => Some(format!(
            "{} {}",
            normalize_card_markdown(m),
            normalize_card_markdown(e)
        )),
        (Some(m), None) => Some(normalize_card_markdown(m)),
        (None, Some(e)) => Some(normalize_card_markdown(e)),
        _ => None,
    };

    let title = match tool {
        "spawnAgent" => {
            if let Some(tag) = model_tag {
                format!("Spawned subagent `{}`", tag)
            } else {
                "Spawned subagent".to_string()
            }
        }
        "wait" => {
            if let Some(states) = item.get("agentsStates").and_then(|v| v.as_object()) {
                if !states.is_empty() {
                    format!("Waiting on {} agent(s)", states.len())
                } else if receiver_count > 0 {
                    format!("Waiting on {} agent(s)", receiver_count)
                } else {
                    "Waiting on agents".to_string()
                }
            } else if receiver_count > 0 {
                format!("Waiting on {} agent(s)", receiver_count)
            } else {
                "Waiting on agents".to_string()
            }
        }
        "sendInput" => {
            if receiver_count > 0 {
                format!("Sent input to {} agent(s)", receiver_count)
            } else {
                "Sent input".to_string()
            }
        }
        "resumeAgent" => {
            if receiver_count > 0 {
                format!("Resumed {} agent(s)", receiver_count)
            } else {
                "Resumed agent".to_string()
            }
        }
        "closeAgent" => {
            if receiver_count > 0 {
                format!("Closed {} agent(s)", receiver_count)
            } else {
                "Closed agent".to_string()
            }
        }
        _ => format!("Collab: {}", normalize_card_markdown(tool)),
    };

    let (color, label) = collab_status_badge(status);
    // Feishu collapsible_panel.header.title does NOT support column_set; keep it as markdown.
    format!("{title} <font color='{color}'>· {label}</font>")
}

fn build_collab_agent_tool_call_card(item: &JsonValue) -> serde_json::Value {
    let header = collab_header_summary(item);
    let prompt = item
        .get("prompt")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| truncate_single_line(v, 260));

    let mut elements = Vec::new();
    if let Some(prompt) = prompt {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": format!("<font color='grey'>└ {}</font>", normalize_card_markdown(&prompt))
        }));
    }

    if item.get("tool").and_then(|v| v.as_str()) == Some("wait") {
        if let Some(states) = item.get("agentsStates").and_then(|v| v.as_object()) {
            let mut lines = Vec::new();
            for (agent_id, state) in states.iter().take(6) {
                let status = state
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let message = state
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(|v| truncate_single_line(v, 120));
                let mut line = format!(
                    "- `{}`: `{}`",
                    normalize_card_markdown(agent_id),
                    normalize_card_markdown(status)
                );
                if let Some(message) = message {
                    line.push_str(&format!(
                        " <font color='grey'>· {}</font>",
                        normalize_card_markdown(&message)
                    ));
                }
                lines.push(line);
            }
            if !lines.is_empty() {
                if !elements.is_empty() {
                    elements.push(serde_json::json!({ "tag": "hr" }));
                }
                elements.push(serde_json::json!({
                    "tag": "markdown",
                    "content": lines.join("\n")
                }));
            }
        }
    }

    if elements.is_empty() {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": "_暂无内容_"
        }));
    }

    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "padding": "0px 8px 0px 8px",
            "elements": [
                {
                    "tag": "collapsible_panel",
                    "element_id": "collab_agent_panel",
                    "direction": "vertical",
                    "vertical_spacing": "6px",
                    "padding": "0px 0px 0px 0px",
                    "margin": "0px 0px 0px 0px",
                    "expanded": false,
                    "header": build_light_collapsible_header(serde_json::json!({
                        "tag": "markdown",
                        "content": header
                    })),
                    "elements": elements
                }
            ]
        }
    })
}

pub fn build_image_generation_summary_card(
    status: &str,
    revised_prompt: Option<&str>,
    saved_path: Option<&str>,
) -> serde_json::Value {
    // User-facing IM summary: only keep the final revised prompt.
    // Everything else (status/path/result) is noisy for chat.
    let _ = status;
    let _ = saved_path;

    // IMPORTANT: do NOT wrap in ``` fences; Feishu renders code blocks with a horizontal scrollbar.
    let content = revised_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_card_markdown)
        .unwrap_or_else(|| "_未提供修订提示词_".to_string());

    build_markdown_card(&content, None, None)
}

fn build_light_collapsible_header(title: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "title": title,
        "padding": "6px 0px 6px 0px",
        "icon": {
            "tag": "standard_icon",
            "token": "down_outlined",
            "color": "grey",
            "size": "14px 14px"
        },
        "icon_position": "right",
        "icon_expanded_angle": 180
    })
}

#[derive(Debug, Clone)]
pub struct FeishuThreadRoutingAction {
    pub label: String,
    pub description: String,
    pub value: serde_json::Value,
    pub primary: bool,
    pub selected: bool,
    pub resolved: bool,
}

#[derive(Debug, Clone)]
pub struct FeishuThreadListEntry {
    pub thread_id: String,
    pub title: String,
    pub state: String,
    pub cwd: Option<String>,
}

fn build_interactive_choice_block(
    label: &str,
    description: &str,
    value: serde_json::Value,
    _primary: bool,
    selected: bool,
    resolved: bool,
) -> serde_json::Value {
    let title_content = if selected {
        format!(
            "**<font color='blue'>已选择 · {}</font>**",
            normalize_card_markdown(label)
        )
    } else {
        normalize_card_markdown(label)
    };
    let mut container = serde_json::json!({
        "tag": "interactive_container",
        "width": "fill",
        "height": "auto",
        "horizontal_align": "left",
        "background_style": "default",
        "has_border": true,
        "border_color": if selected { "blue" } else { "grey" },
        "corner_radius": "8px",
        "padding": "10px 12px 10px 12px",
        "elements": [
            {
                "tag": "markdown",
                "content": title_content
            },
            {
                "tag": "markdown",
                "content": format!("<font color='grey'>{}</font>", normalize_card_markdown(description))
            }
        ],
        "behaviors": if resolved { serde_json::json!([]) } else { serde_json::json!([
            {
                "type": "callback",
                "value": value
            }
        ]) }
    });
    if description.trim().is_empty() {
        container["elements"] = serde_json::json!([
            {
                "tag": "markdown",
                "content": title_content
            }
        ]);
    }
    container
}

fn build_thread_list_row(request_id: &str, entry: &FeishuThreadListEntry) -> serde_json::Value {
    let primary_text = if entry.title.trim().is_empty() {
        format!("会话 {}", normalize_card_markdown(&entry.thread_id))
    } else {
        normalize_card_markdown(&entry.title)
    };
    let mut details = Vec::new();
    if let Some(state) = thread_state_suffix(&entry.state) {
        details.push(state.to_string());
    }
    let mut text_elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": primary_text
    })];
    if !details.is_empty() {
        text_elements.push(serde_json::json!({
            "tag": "markdown",
            "content": format!("<font color='grey'>{}</font>", normalize_card_markdown(&details.join(" · ")))
        }));
    }
    serde_json::json!({
        "tag": "interactive_container",
        "width": "fill",
        "height": "auto",
        "horizontal_align": "left",
        "background_style": "default",
        "has_border": true,
        "border_color": "grey",
        "corner_radius": "8px",
        "padding": "10px 12px 10px 12px",
        "elements": text_elements,
        "behaviors": [
            {
                "type": "callback",
                "value": {
                    "kind": "thread_route_resume_selected",
                    "requestId": request_id,
                    "threadId": entry.thread_id
                }
            }
        ]
    })
}

fn thread_project_header(cwd: Option<&str>) -> serde_json::Value {
    let content = match cwd {
        Some(cwd) => {
            let name = project_name(cwd);
            format!(
                "**项目：{}**\n<font color='grey'>{}</font>",
                normalize_card_markdown(&name),
                normalize_card_markdown(cwd)
            )
        }
        None => "**项目：未知目录**".to_string(),
    };
    serde_json::json!({
        "tag": "markdown",
        "content": content
    })
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

fn project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

fn file_change_kind_label(change: &JsonValue) -> &'static str {
    match change
        .get("kind")
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("update")
    {
        "add" => "add",
        "delete" => "delete",
        _ => "edit",
    }
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn summarize_file_change_card(content: &str) -> (Option<String>, String) {
    let normalized = normalize_card_markdown(content);
    let mut lines = normalized.lines();
    let Some(first_line) = lines.next() else {
        return (None, String::new());
    };

    let title = if first_line.starts_with("**") {
        let cleaned = first_line.replace("**", "").replace('`', "");
        let compact = cleaned.split_whitespace().collect::<Vec<_>>();
        if compact.len() >= 4 {
            let kind = compact[0];
            let path = compact[1];
            let plus = compact[2];
            let minus = compact[3];
            Some(format!("{kind} {} {plus} {minus}", basename(path)))
        } else {
            Some(first_line.replace("**", "").replace('`', ""))
        }
    } else {
        None
    };

    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    (title, body)
}

fn colorize_change_title(title: &str) -> String {
    let parts = title.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 4 {
        return normalize_card_markdown(title);
    }
    let action = parts[0];
    let file = parts[1];
    let plus = parts[2];
    let minus = parts[3];
    let action_color = match action {
        "add" => "green",
        "delete" => "red",
        _ => "blue",
    };
    format!(
        "<font color='{action_color}'>{}</font> {}&nbsp;&nbsp;&nbsp;<font color='green'>{}</font> <font color='red'>{}</font>",
        normalize_card_markdown(action),
        normalize_card_markdown(file),
        normalize_card_markdown(plus),
        normalize_card_markdown(minus)
    )
}

#[derive(Debug, Clone)]
struct TurnDiffSummaryEntry {
    path: String,
    additions: usize,
    deletions: usize,
    diff: String,
}

fn parse_turn_diff_summary(diff: &str) -> Option<(usize, usize, Vec<TurnDiffSummaryEntry>)> {
    let normalized = diff.replace("\r\n", "\n").trim().to_string();
    if normalized.is_empty() {
        return None;
    }

    let mut sections = Vec::new();
    let mut current = Vec::new();
    let mut saw_header = false;
    for line in normalized.lines() {
        if line.starts_with("diff --git ")
            || (line.starts_with("--- ") && !saw_header && current.is_empty())
        {
            if !current.is_empty() {
                sections.push(current.join("\n"));
                current.clear();
            }
            saw_header = true;
        }
        current.push(line.to_string());
    }
    if !current.is_empty() {
        sections.push(current.join("\n"));
    }
    if sections.is_empty() {
        sections.push(normalized);
    }

    let mut total_additions = 0usize;
    let mut total_deletions = 0usize;
    let mut files = Vec::new();

    for section in sections {
        let mut path = None;
        for line in section.lines() {
            if let Some(rest) = line.strip_prefix("+++ ") {
                let trimmed = rest.trim();
                if !trimmed.is_empty() && trimmed != "/dev/null" {
                    path = Some(trimmed.strip_prefix("b/").unwrap_or(trimmed).to_string());
                    break;
                }
            }
        }
        if path.is_none() {
            for line in section.lines() {
                if let Some(rest) = line.strip_prefix("diff --git ") {
                    let parts = rest.split_whitespace().collect::<Vec<_>>();
                    if let Some(raw) = parts.get(1) {
                        path = Some(raw.strip_prefix("b/").unwrap_or(raw).to_string());
                        break;
                    }
                }
            }
        }

        let path = path.unwrap_or_else(|| "unknown".to_string());
        let (additions, deletions) = parse_unified_diff_stats(&section);
        total_additions += additions;
        total_deletions += deletions;

        files.push(TurnDiffSummaryEntry {
            path,
            additions,
            deletions,
            diff: section,
        });
    }

    Some((total_additions, total_deletions, files))
}

fn extract_plan_title_and_steps(text: &str) -> (Option<String>, Vec<String>) {
    let normalized = normalize_card_markdown(text);
    let mut title = None;
    let mut steps = Vec::new();
    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if title.is_none() && trimmed.starts_with('#') {
            title = Some(trimmed.trim_start_matches('#').trim().to_string());
            continue;
        }
        if let Some(step) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            steps.push(step.to_string());
        }
    }
    (title, steps)
}

fn request_input_field_name(index: usize) -> String {
    format!("q_{}", index + 1)
}

fn request_input_other_field_name(index: usize) -> String {
    format!("q_{}_other", index + 1)
}

pub fn should_render_as_card(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed.contains("```") || trimmed.contains('|') || trimmed.contains('\n')
}

pub fn build_markdown_card(
    text: &str,
    title: Option<&str>,
    template: Option<&str>,
) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    let mut card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": content
                }
            ]
        }
    });
    if let Some(title) = title.map(str::trim).filter(|value| !value.is_empty()) {
        card["header"] = serde_json::json!({
            "title": {
                "tag": "plain_text",
                "content": title
            },
            "template": template.unwrap_or(DEFAULT_CARD_TEMPLATE)
        });
    }
    card
}

pub fn build_approval_card(
    kind_label: &str,
    summary: &str,
    decisions: &[ApprovalDecisionOption],
    request_key: &str,
) -> serde_json::Value {
    let content = normalize_card_markdown(summary);
    let mut elements = vec![
        {
            serde_json::json!({
                "tag": "markdown",
                "content": format!("**approval request: `{}`**", normalize_card_markdown(kind_label))
            })
        },
        {
            serde_json::json!({
                "tag": "markdown",
                "content": content
            })
        },
    ];
    if !decisions.is_empty() {
        elements.push(serde_json::json!({
            "tag": "hr"
        }));
        elements.push(build_approval_button_row(decisions, request_key));
    }
    let mut card = build_markdown_card("", Some("approval pending"), Some(APPROVAL_CARD_TEMPLATE));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_resolved_approval_card(
    kind_label: &str,
    summary: &str,
    decision_label: &str,
    option_index: usize,
) -> serde_json::Value {
    let content = normalize_card_markdown(summary);
    let selected = normalize_card_markdown(decision_label.trim());
    let elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": format!("**approval request: `{}`**", normalize_card_markdown(kind_label))
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": content
        }),
        serde_json::json!({
            "tag": "hr"
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": format!("**selected /{}: {}**", option_index, selected)
        }),
    ];
    let mut card = build_markdown_card("", Some("approval resolved"), Some("green"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

fn build_approval_button_row(
    decisions: &[ApprovalDecisionOption],
    request_key: &str,
) -> serde_json::Value {
    let columns = decisions
        .iter()
        .enumerate()
        .map(|(index, decision)| {
            let option_index = index + 1;
            let primary = index == 0 && !decision_is_negative(&decision.decision);
            serde_json::json!({
                "tag": "column",
                "width": "auto",
                "padding": "0px 0px 0px 0px",
                "vertical_spacing": "0px",
                "elements": [
                    {
                        "tag": "button",
                        "text": {
                            "tag": "plain_text",
                            "content": decision.label.trim()
                        },
                        "type": if primary { "primary_filled" } else { "default" },
                        "width": "default",
                        "behaviors": [
                            {
                                "type": "callback",
                                "value": {
                                    "kind": "codex_approval_decision",
                                    "option": option_index,
                                    "requestKey": request_key
                                }
                            }
                        ]
                    }
                ]
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "tag": "column_set",
        "flex_mode": "flow",
        "horizontal_spacing": "8px",
        "horizontal_align": "left",
        "columns": columns
    })
}

fn decision_is_negative(decision: &JsonValue) -> bool {
    decision
        .as_str()
        .is_some_and(|value| matches!(value, "decline" | "cancel" | "denied"))
        || decision
            .get("applyNetworkPolicyAmendment")
            .and_then(|value| value.get("network_policy_amendment"))
            .and_then(|value| value.get("action"))
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == "deny")
}

pub fn build_oauth_device_card(
    verification_uri: &str,
    verification_uri_complete: &str,
    user_code: &str,
    expires_in: u64,
    scope_lines: &[String],
) -> serde_json::Value {
    let expires_minutes = ((expires_in as f64) / 60.0).ceil() as u64;
    let permission_content = if scope_lines.is_empty() {
        "当前这一步需要补充用户授权。".to_string()
    } else {
        format!(
            "所需权限：\n{}",
            scope_lines
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let auth_url = if !verification_uri_complete.trim().is_empty() {
        verification_uri_complete.trim()
    } else {
        verification_uri.trim()
    };
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": "授权后，应用将能够以你的身份继续执行当前操作。"
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": permission_content
        }),
        serde_json::json!({
            "tag": "column_set",
            "flex_mode": "none",
            "horizontal_align": "right",
            "columns": [
                {
                    "tag": "column",
                    "width": "auto",
                    "elements": [
                        {
                            "tag": "button",
                            "text": {
                                "tag": "plain_text",
                                "content": "前往授权"
                            },
                            "type": "primary",
                            "size": "medium",
                            "multi_url": {
                                "url": auth_url,
                                "pc_url": auth_url,
                                "android_url": auth_url,
                                "ios_url": auth_url
                            }
                        }
                    ]
                }
            ]
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": format!("<font color='grey'>授权链接将在 {} 分钟后失效，届时需要重新发起。</font>", expires_minutes.max(1))
        }),
    ];
    if !user_code.trim().is_empty() {
        elements.insert(
            1,
            serde_json::json!({
                "tag": "markdown",
                "content": format!("用户码：`{}`", user_code.trim())
            }),
        );
    }
    let mut card = build_markdown_card("", Some("请授权以继续当前操作"), Some("blue"));
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_oauth_device_explanation(operation_label: &str) -> String {
    format!(
        "这一步需要你先授权一下，我才能继续完成“{}”。\n\n我已经发了一张授权卡，点击里面的按钮完成授权后，回来继续发消息就行。",
        operation_label
    )
}

pub fn build_permission_required_explanation(operation_label: &str) -> String {
    format!(
        "当前这一步要完成“{}”，但飞书应用侧的权限还没开通。\n\n这个不是你操作错了，需要应用管理员先补权限，之后再重试。",
        operation_label
    )
}

pub fn build_permission_required_card(operation_label: &str) -> serde_json::Value {
    let content = format!(
        "当前飞书应用缺少完成“{}”所需的应用权限。\n\n请应用管理员先在飞书开放平台完成权限开通，随后再重试当前操作。",
        operation_label
    );
    let mut card = build_markdown_card(&content, Some("需要补充应用权限"), Some("orange"));
    card["body"]["elements"] = serde_json::json!([
        {
            "tag": "markdown",
            "content": normalize_card_markdown(&content)
        },
        {
            "tag": "hr"
        },
        {
            "tag": "markdown",
            "content": "_管理员完成开通后，重新发起当前操作即可。_"
        }
    ]);
    card
}

pub fn build_request_user_input_card(
    request_id: &str,
    question: &FeishuUserInputQuestion,
    index: usize,
    total: usize,
    text_entry_mode: bool,
    current_answers: &[String],
    resolved: bool,
) -> serde_json::Value {
    let title = if question.header.trim().is_empty() {
        format!("问题 {}", index + 1)
    } else {
        normalize_card_markdown(&question.header)
    };
    let prompt = normalize_card_markdown(&question.question);
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": format!("<font color='grey'>第 {}/{} 题</font>", index + 1, total.max(1))
        }),
        serde_json::json!({
            "tag": "div",
            "text": {
                "tag": "plain_text",
                "content": title,
                "text_size": "heading"
            }
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": prompt
        }),
    ];

    let has_options = question
        .options
        .as_ref()
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    if has_options && !text_entry_mode {
        for option in question.options.as_ref().into_iter().flatten() {
            let label = normalize_card_markdown(&option.label);
            let desc = normalize_card_markdown(&option.description);
            let selected = current_answers
                .iter()
                .any(|answer| answer.trim() == option.label.trim());
            let mut inner_elements = vec![serde_json::json!({
                "tag": "markdown",
                "content": label
            })];
            if !desc.is_empty() {
                inner_elements.push(serde_json::json!({
                    "tag": "markdown",
                    "content": format!("<font color='grey'>{}</font>", desc)
                }));
            }
            let mut container = serde_json::json!({
                "tag": "interactive_container",
                "width": "fill",
                "height": "auto",
                "horizontal_align": "left",
                "background_style": if resolved && selected { "wathet" } else { "default" },
                "has_border": true,
                "border_color": if resolved && selected { "blue" } else { "grey" },
                "corner_radius": "8px",
                "padding": "8px 12px 8px 12px",
                "elements": inner_elements
            });
            if !resolved {
                container["behaviors"] = serde_json::json!([
                    {
                        "type": "callback",
                        "value": {
                            "kind": "request_user_input_option",
                            "requestId": request_id,
                            "questionId": question.id,
                            "answer": option.label
                        }
                    }
                ]);
            }
            elements.push(container);
        }
        if question.is_other && !resolved {
            elements.push(serde_json::json!({
                "tag": "interactive_container",
                "width": "fill",
                "height": "auto",
                "horizontal_align": "left",
                "background_style": "default",
                "has_border": true,
                "border_color": "grey",
                "corner_radius": "8px",
                "padding": "8px 12px 8px 12px",
                "behaviors": [
                    {
                        "type": "callback",
                        "value": {
                            "kind": "request_user_input_other",
                            "requestId": request_id,
                            "questionId": question.id
                        }
                    }
                ],
                "elements": [
                    {
                        "tag": "markdown",
                        "content": "其他"
                    },
                    {
                        "tag": "markdown",
                        "content": "<font color='grey'>手动填写你自己的答案</font>"
                    }
                ]
            }));
        }
    }

    if (text_entry_mode || !has_options) && !resolved {
        elements.push(serde_json::json!({
            "tag": "form",
            "name": "request_user_input_form",
            "element_id": "request_input_form",
            "direction": "vertical",
            "vertical_spacing": "8px",
            "elements": [
                {
                    "tag": "input",
                    "name": "free_text",
                    "required": false
                },
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "auto",
                            "elements": [
                                {
                                    "tag": "button",
                                    "type": "primary",
                                    "text": {
                                        "tag": "plain_text",
                                        "content": "提交"
                                    },
                                    "name": "submit_request_user_input",
                                    "form_action_type": "submit",
                                    "behaviors": [
                                        {
                                            "type": "callback",
                                            "value": {
                                                "kind": "request_user_input_text",
                                                "requestId": request_id,
                                                "questionId": question.id
                                            }
                                        }
                                    ]
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
    }

    if !resolved {
        elements.push(serde_json::json!({
            "tag": "interactive_container",
            "width": "fill",
            "height": "auto",
            "horizontal_align": "left",
            "background_style": "default",
            "has_border": true,
            "border_color": "grey",
            "corner_radius": "8px",
            "padding": "8px 12px 8px 12px",
            "behaviors": [
                {
                    "type": "callback",
                    "value": {
                        "kind": "request_user_input_skip",
                        "requestId": request_id,
                        "questionId": question.id
                    }
                }
            ],
            "elements": [
                {
                    "tag": "markdown",
                    "content": "跳过"
                },
                {
                    "tag": "markdown",
                    "content": "<font color='grey'>暂时不回答这一题</font>"
                }
            ]
        }));
    }

    let mut card = build_markdown_card("", Some("需要你的输入"), Some("indigo"));
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_status_card(text: &str) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    build_markdown_card(&content, None, None)
}

pub fn build_image_generation_result_card(
    status: &str,
    revised_prompt: Option<&str>,
    saved_path: Option<&str>,
    image_key: &str,
) -> serde_json::Value {
    let mut elements = vec![image_element(image_key, "生成图片")];
    let mut details = vec![format!(
        "**状态**：`{}`",
        normalize_card_markdown(status).trim()
    )];
    if let Some(revised_prompt) = revised_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(format!(
            "**修订提示词**\n{}",
            normalize_card_markdown(revised_prompt)
        ));
    }
    if let Some(saved_path) = saved_path.map(str::trim).filter(|value| !value.is_empty()) {
        details.push(format!(
            "**保存路径**：`{}`",
            normalize_card_markdown(saved_path)
        ));
    }
    elements.push(serde_json::json!({
        "tag": "markdown",
        "content": details.join("\n\n")
    }));

    let mut card = build_markdown_card("", Some("图片生成"), Some("orange"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_image_view_result_card(path: &str, image_key: &str) -> serde_json::Value {
    let elements = vec![
        image_element(image_key, "图片预览"),
        serde_json::json!({
            "tag": "markdown",
            "content": format!("**路径**：`{}`", normalize_card_markdown(path))
        }),
    ];
    let mut card = build_markdown_card("", Some("图片"), Some("carmine"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_thread_routing_choice_card(
    title: &str,
    body: &str,
    actions: &[FeishuThreadRoutingAction],
) -> serde_json::Value {
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": normalize_card_markdown(body)
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": "<font color='orange'>提示：回复 `/q` 可退出当前会话，回复 `/s` 可中断当前任务。</font>"
        }),
    ];
    for action in actions {
        elements.push(build_interactive_choice_block(
            &action.label,
            &action.description,
            action.value.clone(),
            action.primary,
            action.selected,
            action.resolved,
        ));
    }

    let mut card = build_markdown_card("", Some(title), Some("indigo"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_thread_create_settings_card(
    request_id: &str,
    defaults: &ThreadCreateDefaults,
) -> serde_json::Value {
    let cwd_options = thread_cwd_options(defaults);
    let model_options = thread_model_options(defaults);
    let effort_options = thread_effort_options(defaults);
    let permission_options = thread_permission_options();
    let default_line = |label: &str, value: Option<&String>| {
        format!(
            "{}：{}",
            label,
            value
                .map(|value| normalize_card_markdown(value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "使用 Codex App 默认值".to_string())
        )
    };
    let remote_line = format!(
        "远端：{}",
        defaults
            .remote_name
            .as_ref()
            .map(|value| normalize_card_markdown(value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "未连接".to_string())
    );
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": "选择这次新会话的属性。Provider 固定使用 Codex App 当前配置。"
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": format!(
                "<font color='grey'>{}\n{}\n{}\n{}\n{}\n{}</font>",
                remote_line,
                default_line("目录", defaults.cwd.as_ref()),
                default_line("Provider", defaults.model_provider.as_ref()),
                default_line("模型", defaults.model.as_ref()),
                default_line("推理强度", defaults.effort.as_ref()),
                default_line("权限", defaults.permission.as_ref())
            )
        }),
        serde_json::json!({
            "tag": "form",
            "name": "thread_create_form",
            "element_id": "thread_create_form",
            "direction": "vertical",
            "vertical_spacing": "8px",
            "elements": [
                {
                    "tag": "markdown",
                    "content": "**项目目录**"
                },
                select_static_element(
                    "cwd_choice",
                    "选择已有项目目录",
                    cwd_options
                ),
                {
                    "tag": "input",
                    "name": "cwd_custom",
                    "required": false,
                    "placeholder": {
                        "tag": "plain_text",
                        "content": "可选：填绝对路径；不存在会自动创建"
                    }
                },
                {
                    "tag": "markdown",
                    "content": "**模型**"
                },
                select_static_element(
                    "model",
                    "选择模型",
                    model_options
                ),
                {
                    "tag": "markdown",
                    "content": "**推理强度**"
                },
                select_static_element(
                    "effort",
                    "选择推理强度",
                    effort_options
                ),
                {
                    "tag": "markdown",
                    "content": "**权限**"
                },
                select_static_element(
                    "permission",
                    "选择权限",
                    permission_options
                ),
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "auto",
                            "elements": [
                                {
                                    "tag": "button",
                                    "type": "primary",
                                    "text": {
                                        "tag": "plain_text",
                                        "content": "确认创建"
                                    },
                                    "name": "submit_thread_create",
                                    "form_action_type": "submit",
                                    "behaviors": [
                                        {
                                            "type": "callback",
                                            "value": {
                                                "kind": "thread_route_create_submit",
                                                "requestId": request_id
                                            }
                                        }
                                    ]
                                }
                            ]
                        }
                    ]
                }
            ]
        }),
    ];
    elements.push(build_interactive_choice_block(
        "使用默认配置创建",
        "使用当前 provider，不指定目录、模型和推理强度。",
        serde_json::json!({
            "kind": "thread_route_create_default",
            "requestId": request_id
        }),
        false,
        false,
        false,
    ));
    elements.push(build_interactive_choice_block(
        "返回",
        "回到新建/恢复会话选择。",
        serde_json::json!({
            "kind": "thread_route_choice",
            "requestId": request_id,
            "action": "back"
        }),
        false,
        false,
        false,
    ));

    let mut card = build_markdown_card("", Some("新建会话设置"), Some("indigo"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

fn select_static_element(
    name: &str,
    placeholder: &str,
    options: Vec<(String, String)>,
) -> serde_json::Value {
    serde_json::json!({
        "tag": "select_static",
        "name": name,
        "required": false,
        "placeholder": {
            "tag": "plain_text",
            "content": placeholder
        },
        "options": options
            .into_iter()
            .map(|(label, value)| {
                serde_json::json!({
                    "text": {
                        "tag": "plain_text",
                        "content": label
                    },
                    "value": value
                })
            })
            .collect::<Vec<_>>()
    })
}

fn thread_cwd_options(defaults: &ThreadCreateDefaults) -> Vec<(String, String)> {
    let mut options = vec![(
        "使用 Codex App 默认目录".to_string(),
        "__default__".to_string(),
    )];
    for project in defaults
        .projects
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .take(20)
    {
        options.push((project_option_label(project), project.to_string()));
    }
    options.push(("自定义或新建目录".to_string(), "__custom__".to_string()));
    dedupe_options(options)
}

fn thread_model_options(defaults: &ThreadCreateDefaults) -> Vec<(String, String)> {
    let mut options = vec![("使用当前模型".to_string(), "__default__".to_string())];
    for model in defaults
        .models
        .iter()
        .filter(|value| !value.value.trim().is_empty())
    {
        options.push((model.label.clone(), model.value.clone()));
    }
    dedupe_options(options)
}

fn thread_effort_options(defaults: &ThreadCreateDefaults) -> Vec<(String, String)> {
    let mut options = vec![(
        "使用模型默认推理强度".to_string(),
        "__default__".to_string(),
    )];
    if let Some(effort) = defaults.effort.as_deref().map(str::trim)
        && !effort.is_empty()
    {
        options.push((reasoning_effort_label(effort), effort.to_string()));
    }
    for effort in defaults
        .efforts
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        options.push((reasoning_effort_label(effort), effort.to_string()));
    }
    dedupe_options(options)
}

fn thread_permission_options() -> Vec<(String, String)> {
    vec![
        ("默认权限".to_string(), "workspace_user".to_string()),
        ("自动审查".to_string(), "auto_review".to_string()),
        ("完全访问权限".to_string(), "full_access".to_string()),
    ]
}

fn reasoning_effort_label(effort: &str) -> String {
    match effort {
        "none" => "无".to_string(),
        "minimal" => "极低".to_string(),
        "low" => "低".to_string(),
        "medium" => "中".to_string(),
        "high" => "高".to_string(),
        "xhigh" => "超高".to_string(),
        other => other.to_string(),
    }
}

fn dedupe_options(options: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut output = Vec::new();
    for (label, value) in options {
        if !output
            .iter()
            .any(|(_, existing_value): &(String, String)| existing_value == &value)
        {
            output.push((label, value));
        }
    }
    output
}

fn project_option_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

pub fn build_thread_list_card(
    request_id: &str,
    title: &str,
    body: &str,
    entries: &[FeishuThreadListEntry],
    page: usize,
    has_prev: bool,
    has_next: bool,
) -> serde_json::Value {
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": normalize_card_markdown(body)
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": format!("<font color='grey'>第 {} 页</font>", page.max(1))
        }),
    ];

    if entries.is_empty() {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": "<font color='grey'>当前工作区下没有可恢复的历史会话。</font>"
        }));
    } else {
        let mut current_cwd: Option<&str> = None;
        for entry in entries.iter() {
            let cwd = entry
                .cwd
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if current_cwd != cwd {
                elements.push(thread_project_header(cwd));
                current_cwd = cwd;
            }
            elements.push(build_thread_list_row(request_id, entry));
        }
    }

    let mut nav_columns = Vec::new();
    if has_prev {
        nav_columns.push(serde_json::json!({
            "tag": "column",
            "width": "auto",
            "elements": [
                {
                    "tag": "button",
                    "type": "default",
                    "text": {
                        "tag": "plain_text",
                        "content": "上一页"
                    },
                    "behaviors": [
                        {
                            "type": "callback",
                            "value": {
                                "kind": "thread_route_list_page",
                                "requestId": request_id,
                                "direction": "prev"
                            }
                        }
                    ]
                }
            ]
        }));
    }
    if has_next {
        nav_columns.push(serde_json::json!({
            "tag": "column",
            "width": "auto",
            "elements": [
                {
                    "tag": "button",
                    "type": "default",
                    "text": {
                        "tag": "plain_text",
                        "content": "下一页"
                    },
                    "behaviors": [
                        {
                            "type": "callback",
                            "value": {
                                "kind": "thread_route_list_page",
                                "requestId": request_id,
                                "direction": "next"
                            }
                        }
                    ]
                }
            ]
        }));
    }
    if !nav_columns.is_empty() {
        elements.push(serde_json::json!({
            "tag": "column_set",
            "flex_mode": "none",
            "horizontal_spacing": "8px",
            "columns": nav_columns
        }));
    }

    let mut card = build_markdown_card("", Some(title), Some("grey"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_thread_list_loading_card(title: &str, page: usize) -> serde_json::Value {
    let body = format!(
        "正在加载历史会话...\n\n<font color='grey'>第 {} 页</font>",
        page.max(1)
    );
    let mut card = build_markdown_card(&body, Some(title), Some("grey"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card
}

pub fn build_thread_routing_result_card(title: &str, body: &str) -> serde_json::Value {
    let mut card = build_markdown_card(
        normalize_card_markdown(body).as_str(),
        Some(title),
        Some("blue"),
    );
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card
}

pub fn build_history_summary_card(text: &str) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    let mut card = build_markdown_card(&content, Some("历史摘要"), Some("grey"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card
}

pub fn build_plan_implement_prompt_card() -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": "计划已完成"
            },
            "template": "wathet"
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": "要开始按这份计划继续实现吗？"
                },
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "8px",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "elements": [
                                {
                                    "tag": "interactive_container",
                                    "width": "fill",
                                    "height": "auto",
                                    "horizontal_align": "center",
                                    "background_style": "default",
                                    "has_border": true,
                                    "border_color": "grey",
                                    "corner_radius": "8px",
                                    "padding": "8px 12px 8px 12px",
                                    "behaviors": [
                                        {
                                            "type": "callback",
                                            "value": {
                                                "kind": "plan_implement_prompt",
                                                "action": "dismiss"
                                            }
                                        }
                                    ],
                                    "elements": [
                                        {
                                            "tag": "markdown",
                                            "content": "先不继续"
                                        }
                                    ]
                                }
                            ]
                        },
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "elements": [
                                {
                                    "tag": "interactive_container",
                                    "width": "fill",
                                    "height": "auto",
                                    "horizontal_align": "center",
                                    "background_style": "wathet",
                                    "has_border": true,
                                    "border_color": "blue",
                                    "corner_radius": "8px",
                                    "padding": "8px 12px 8px 12px",
                                    "behaviors": [
                                        {
                                            "type": "callback",
                                            "value": {
                                                "kind": "plan_implement_prompt",
                                                "action": "implement"
                                            }
                                        }
                                    ],
                                    "elements": [
                                        {
                                            "tag": "markdown",
                                            "content": "开始实现"
                                        }
                                    ]
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}

pub fn build_turn_completed_card(reply_text: &str) -> serde_json::Value {
    let content = normalize_card_markdown(reply_text);
    build_streaming_reply_card(&content, true)
}

fn build_agent_message_header_card(content: &str, show_generating: bool) -> serde_json::Value {
    let body_elements = if show_generating {
        serde_json::json!([
            {
                "tag": "column_set",
                "flex_mode": "none",
                "horizontal_spacing": "0px",
                "background_style": "indigo-50",
                "columns": [
                    {
                        "tag": "column",
                        "width": "weighted",
                        "weight": 1,
                        "padding": "0px",
                        "margin": "0px",
                        "vertical_spacing": "4px",
                        "elements": [
                            {
                                "tag": "markdown",
                                "content": content
                            },
                            {
                                "tag": "markdown",
                                "content": "<font color='grey'>生成中</font>"
                            }
                        ]
                    }
                ]
            }
        ])
    } else {
        serde_json::json!([
            {
                "tag": "column_set",
                "flex_mode": "none",
                "horizontal_spacing": "0px",
                "background_style": "indigo-50",
                "columns": [
                    {
                        "tag": "column",
                        "width": "weighted",
                        "weight": 1,
                        "padding": "0px",
                        "margin": "0px",
                        "vertical_spacing": "0px",
                        "elements": [
                            {
                                "tag": "markdown",
                                "content": content
                            }
                        ]
                    }
                ]
            }
        ])
    };
    let card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": " "
            },
            "template": "default",
            "icon": {
                "tag": "standard_icon",
                "token": "robot_outlined",
                "color": "blue"
            },
            "padding": "8px 8px 4px 8px"
        },
        "body": {
            "padding": "0px",
            "elements": body_elements
        }
    });
    card
}

pub fn build_streaming_reply_card(text: &str, is_completed: bool) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    build_agent_message_header_card(&content, !is_completed)
}

pub fn build_turn_terminal_mark_card(state_text: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "padding": "0px 8px 0px 8px",
            "elements": [
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "0px",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "vertical_align": "center",
                            "padding": "6px 0px 6px 0px",
                            "elements": [
                                {
                                    "tag": "markdown",
                                    "content": state_text
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}

pub fn build_streaming_command_card(text: &str, is_completed: bool) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    let (command_text, output_text, meta_text) =
        if let Some((_, rest)) = content.split_once("__COMMAND__\n") {
            if let Some((command, output_and_meta)) = rest.split_once("\n__OUTPUT__\n") {
                if let Some((output, meta)) = output_and_meta.split_once("\n__META__\n") {
                    (
                        command.trim().to_string(),
                        output.trim().to_string(),
                        meta.trim().to_string(),
                    )
                } else {
                    (
                        command.trim().to_string(),
                        output_and_meta.trim().to_string(),
                        String::new(),
                    )
                }
            } else {
                (rest.trim().to_string(), String::new(), String::new())
            }
        } else {
            let mut lines = content.lines();
            let command_line = lines
                .next()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with("```"))
                .unwrap_or("command");
            (
                command_line.to_string(),
                lines.collect::<Vec<_>>().join("\n").trim().to_string(),
                String::new(),
            )
        };

    let command_line = if command_text.is_empty() {
        "command".to_string()
    } else {
        command_text
    };
    let title = summarize_command_header(&command_line);
    let command_line = truncate_text(&command_line, FEISHU_STREAMING_COMMAND_COMMAND_CHARS);
    let output = truncate_text(&output_text, FEISHU_STREAMING_COMMAND_OUTPUT_CHARS);
    let status_text = if !meta_text.is_empty() {
        meta_text
    } else if is_completed {
        "Status: completed".to_string()
    } else {
        "Status: in_progress".to_string()
    };
    let status_text = truncate_text(&status_text, FEISHU_STREAMING_COMMAND_META_CHARS);
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "padding": "0px 8px 0px 8px",
            "elements": [
                {
                    "tag": "collapsible_panel",
                    "element_id": "command_execution_panel",
                    "direction": "vertical",
                    "vertical_spacing": "6px",
                    "padding": "0px 0px 0px 0px",
                    "margin": "0px 0px 0px 0px",
                    "background_color": tool_header_background("commandExecution"),
                    "expanded": false,
                    "header": build_light_collapsible_header(serde_json::json!({
                        "tag": "markdown",
                        "content": title
                    })),
                    "elements": [
                        {
                            "tag": "markdown",
                            "content": format!("```text\n{}\n```", normalize_card_markdown(&command_line))
                        },
                        {
                            "tag": "hr"
                        },
                        {
                            "tag": "markdown",
                            "content": if output.is_empty() {
                                "_Waiting for output..._".to_string()
                            } else {
                                format!("```text\n{}\n```", normalize_card_markdown(&output))
                            }
                        },
                        {
                            "tag": "hr"
                        },
                        {
                            "tag": "markdown",
                            "content": status_text
                        }
                    ]
                }
            ]
        }
    })
}

pub fn build_streaming_file_change_card(text: &str, is_completed: bool) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    let (title, body) = summarize_file_change_card(&content);
    let title_text = title.unwrap_or_else(|| "文件变更".to_string());
    let title_markdown = colorize_change_title(&title_text);
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "elements": [
                {
                    "tag": "collapsible_panel",
                    "element_id": "file_change_panel",
                    "direction": "vertical",
                    "vertical_spacing": "8px",
                    "padding": "0px 0px 0px 0px",
                    "margin": "0px 0px 0px 0px",
                    "expanded": false,
                    "header": build_light_collapsible_header(serde_json::json!({
                        "tag": "markdown",
                        "content": title_markdown
                    })),
                    "border": {
                        "color": "grey",
                        "corner_radius": "8px"
                    },
                    "elements": [
                        {
                            "tag": "markdown",
                            "content": if body.is_empty() {
                                if is_completed { "_已完成_" } else { "_处理中..._" }
                            } else {
                                &body
                            }
                        }
                    ]
                }
            ]
        }
    })
}

pub fn build_streaming_file_summary_card(text: &str, is_completed: bool) -> serde_json::Value {
    let parsed = parse_turn_diff_summary(text);
    let (total_additions, total_deletions, files) = parsed.unwrap_or_else(|| (0, 0, Vec::new()));
    let title_markdown = if files.len() == 1 {
        format!(
            "1 file changed&nbsp;&nbsp;<font color='green'>+{}</font> <font color='red'>-{}</font>",
            total_additions, total_deletions
        )
    } else {
        format!(
            "{} files changed&nbsp;&nbsp;<font color='green'>+{}</font> <font color='red'>-{}</font>",
            files.len(),
            total_additions,
            total_deletions
        )
    };
    let file_elements = if files.is_empty() {
        vec![serde_json::json!({
            "tag": "markdown",
            "content": if is_completed { "_已完成_" } else { "_整理中..._" }
        })]
    } else {
        files.iter()
            .enumerate()
            .flat_map(|(index, file)| {
                let diff_preview = truncate_lines(&file.diff, 12, 100).unwrap_or_else(|| normalize_card_markdown(&file.diff));
                let file_title = format!(
                    "• `{}`&nbsp;&nbsp;<font color='green'>+{}</font> <font color='red'>-{}</font>",
                    normalize_card_markdown(&file.path),
                    file.additions,
                    file.deletions
                );
                let mut elements = vec![
                    serde_json::json!({
                        "tag": "collapsible_panel",
                        "element_id": format!("file_summary_entry_{}", index),
                        "direction": "vertical",
                        "vertical_spacing": "8px",
                        "padding": "0px 0px 0px 0px",
                        "margin": "0px 0px 0px 0px",
                        "expanded": false,
                        "header": {
                            "title": {
                                "tag": "markdown",
                                "content": file_title
                            },
                            "padding": "4px 0px 4px 0px",
                            "icon": {
                                "tag": "standard_icon",
                                "token": "down_outlined",
                                "color": "grey",
                                "size": "14px 14px"
                            },
                            "icon_position": "right",
                            "icon_expanded_angle": 180
                        },
                        "elements": [
                            {
                                "tag": "markdown",
                                "content": format!("```diff\n{}\n```", normalize_card_markdown(&diff_preview))
                            }
                        ]
                    })
                ];
                if index + 1 < files.len() {
                    elements.push(serde_json::json!({
                        "tag": "hr"
                    }));
                }
                elements
            })
            .collect::<Vec<_>>()
    };
    let mut card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "elements": [
                {
                    "tag": "interactive_container",
                    "width": "fill",
                    "height": "auto",
                    "horizontal_align": "left",
                    "background_style": "default",
                    "has_border": true,
                    "border_color": "grey",
                    "corner_radius": "8px",
                    "padding": "10px 12px 10px 12px",
                    "elements": [
                        {
                            "tag": "markdown",
                            "content": title_markdown
                        }
                    ]
                }
            ]
        }
    });
    if let Some(elements) = card
        .get_mut("body")
        .and_then(|body| body.get_mut("elements"))
        .and_then(|value| value.as_array_mut())
    {
        elements.extend(file_elements);
    }
    card
}

pub fn build_streaming_mcp_tool_card(text: &str, is_completed: bool) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    let title = extract_mcp_tool_title(&content).unwrap_or_else(|| "MCP".to_string());
    let status_text = if is_completed {
        "状态：已完成"
    } else {
        "状态：调用中"
    };
    let mut elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": if content.is_empty() { "_等待输出..._" } else { &content }
    })];
    elements.push(serde_json::json!({
        "tag": "hr"
    }));
    elements.push(serde_json::json!({
        "tag": "markdown",
        "content": format!("_{status_text}_")
    }));

    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "padding": "0px 8px 0px 8px",
            "elements": [
                {
                    "tag": "collapsible_panel",
                    "element_id": "mcp_tool_panel",
                    "direction": "vertical",
                    "vertical_spacing": "6px",
                    "padding": "0px 0px 0px 0px",
                    "margin": "0px 0px 0px 0px",
                    "background_color": tool_header_background("mcpToolCall"),
                    "expanded": false,
                    "header": build_light_collapsible_header(serde_json::json!({
                        "tag": "markdown",
                        "content": title
                    })),
                    "elements": elements
                }
            ]
        }
    })
}

fn extract_mcp_tool_title(content: &str) -> Option<String> {
    let mut server = None;
    let mut tool = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if server.is_none() {
            if let Some(value) = trimmed.strip_prefix("**服务**:") {
                server = Some(value.trim().trim_matches('`').to_string());
                continue;
            }
        }
        if tool.is_none() {
            if let Some(value) = trimmed.strip_prefix("**工具**:") {
                tool = Some(value.trim().trim_matches('`').to_string());
                continue;
            }
        }
    }
    match (
        server.filter(|value| !value.is_empty()),
        tool.filter(|value| !value.is_empty()),
    ) {
        (Some(server), Some(tool)) => Some(format!(
            "{}.{}",
            normalize_card_markdown(&server),
            normalize_card_markdown(&tool)
        )),
        (None, Some(tool)) => Some(normalize_card_markdown(&tool)),
        _ => None,
    }
}

pub fn build_cardkit_streaming_reply_card() -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true,
            "streaming_mode": true,
            "summary": {
                "content": "Generating..."
            }
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": "",
                    "element_id": FEISHU_CARDKIT_STREAMING_ELEMENT_ID
                },
                {
                    "tag": "hr"
                },
                {
                    "tag": "markdown",
                    "content": "_状态：生成中_"
                }
            ]
        }
    })
}

pub fn build_cardkit_streaming_agent_message_card() -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true,
            "streaming_mode": true,
            "summary": {
                "content": "生成中"
            }
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": " "
            },
            "template": "default",
            "icon": {
                "tag": "standard_icon",
                "token": "robot_outlined",
                "color": "blue"
            },
            "padding": "8px 8px 4px 8px"
        },
        "body": {
            "padding": "0px",
            "elements": [
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "0px",
                    "background_style": "indigo-50",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "padding": "0px",
                            "margin": "0px",
                            "vertical_spacing": "0px",
                            "elements": [
                                {
                                    "tag": "markdown",
                                    "content": "",
                                    "element_id": FEISHU_CARDKIT_STREAMING_ELEMENT_ID
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}

pub fn build_cardkit_streaming_tool_card(
    title: &str,
    template: &str,
    status_text: &str,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true,
            "streaming_mode": true,
            "summary": {
                "content": "Generating..."
            }
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": title
            },
            "template": template
        },
        "body": {
            "padding": "0px 8px 0px 8px",
            "elements": [
                {
                    "tag": "markdown",
                    "content": "",
                    "element_id": FEISHU_CARDKIT_STREAMING_ELEMENT_ID
                },
                {
                    "tag": "hr"
                },
                {
                    "tag": "markdown",
                    "content": format!("_{}_",
                        status_text
                    )
                }
            ]
        }
    })
}

pub fn build_streaming_reasoning_card(text: &str, is_completed: bool) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    let mut card = build_markdown_card(&content, None, None);
    card["body"]["elements"] = serde_json::json!([
        {
            "tag": "column_set",
            "flex_mode": "none",
            "background_style": "default",
            "horizontal_spacing": "8px",
            "columns": [
                {
                    "tag": "column",
                    "width": "auto",
                    "elements": [
                        {
                            "tag": "div",
                            "text": {
                                "tag": "plain_text",
                                "content": if is_completed { "◌" } else { "◔" },
                                "text_color": "grey"
                            }
                        }
                    ]
                },
                {
                    "tag": "column",
                    "width": "weighted",
                    "weight": 1,
                    "elements": [
                        {
                            "tag": "markdown",
                            "content": content
                        }
                    ]
                }
            ]
        }
    ]);
    card
}

pub fn build_streaming_plan_card(text: &str, is_completed: bool) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    let mut card = build_markdown_card(&content, Some("计划"), Some("indigo"));
    let status_text = if is_completed {
        "状态：已结束"
    } else {
        "状态：规划中"
    };
    card["body"]["elements"] = serde_json::json!([
        {
            "tag": "markdown",
            "content": content
        },
        {
            "tag": "hr"
        },
        {
            "tag": "markdown",
            "content": format!("_{status_text}_")
        }
    ]);
    card
}

fn build_plan_item_card(item: &JsonValue) -> Option<serde_json::Value> {
    let text = item
        .get("text")
        .and_then(|v| v.as_str())
        .map(normalize_card_markdown)?;
    if text.is_empty() {
        return None;
    }
    let (title, steps) = extract_plan_title_and_steps(&text);
    let mut elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": "当前阶段规划如下："
    })];
    if let Some(title) = title.filter(|value| !value.is_empty()) {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": format!("**计划概览**\n{}", normalize_card_markdown(&title))
        }));
    }
    if !steps.is_empty() {
        let step_lines = steps
            .iter()
            .enumerate()
            .map(|(idx, step)| format!("{}. {}", idx + 1, normalize_card_markdown(step)))
            .collect::<Vec<_>>()
            .join("\n");
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": format!("**步骤**\n{}", step_lines)
        }));
    } else {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": text
        }));
    }
    elements.push(serde_json::json!({
        "tag": "hr"
    }));
    elements.push(serde_json::json!({
        "tag": "markdown",
        "content": "_状态：计划已更新_"
    }));
    let mut card = build_markdown_card(&text, Some("计划"), Some("indigo"));
    card["body"]["elements"] = serde_json::Value::Array(elements);
    Some(card)
}

fn build_web_search_card(item: &JsonValue) -> Option<serde_json::Value> {
    let query = item
        .get("query")
        .and_then(|v| v.as_str())
        .map(normalize_card_markdown)
        .filter(|text| !text.is_empty())?;
    Some(serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": "webSearch"
            },
            "icon": {
                "tag": "standard_icon",
                "token": "search_outlined",
                "color": "purple"
            },
            "padding": "8px 8px 4px 8px"
        },
        "body": {
            "padding": "0px 8px 12px 8px",
            "elements": [
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "0px",
                    "background_style": "purple-50",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "padding": "6px 8px 6px 8px",
                            "margin": "0px",
                            "elements": [
                                {
                                    "tag": "markdown",
                                    "content": format!("`{}`", query)
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    }))
}

fn build_tool_result_card(
    title: &str,
    _template: &str,
    tool_name: &str,
    header: &str,
    arguments: Option<String>,
    result: Option<String>,
    status_line: Option<String>,
) -> serde_json::Value {
    let header_text = if header.trim().is_empty() {
        if tool_name.trim().is_empty() {
            normalize_card_markdown(title)
        } else {
            normalize_card_markdown(tool_name)
        }
    } else {
        normalize_card_markdown(header)
    };

    let mut elements = Vec::new();
    if let Some(arguments) = arguments.filter(|value| !value.trim().is_empty()) {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": markdown_code_block("json", &arguments)
        }));
    }
    if let Some(result) = result.filter(|value| !value.trim().is_empty()) {
        if !elements.is_empty() {
            elements.push(serde_json::json!({
                "tag": "hr"
            }));
        }
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": result
        }));
    }
    if let Some(status_line) = status_line.filter(|value| !value.trim().is_empty()) {
        if !elements.is_empty() {
            elements.push(serde_json::json!({
                "tag": "hr"
            }));
        }
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": status_line
        }));
    }
    if elements.is_empty() {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": "_暂无内容_"
        }));
    }

    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "padding": "0px 8px 0px 8px",
            "elements": [
                {
                    "tag": "collapsible_panel",
                    "element_id": "tool_result_panel",
                    "direction": "vertical",
                    "vertical_spacing": "6px",
                    "padding": "0px 0px 0px 0px",
                    "margin": "0px 0px 0px 0px",
                    "background_color": tool_header_background(tool_name),
                    "expanded": false,
                    "header": build_light_collapsible_header(serde_json::json!({
                        "tag": "markdown",
                        "content": header_text
                    })),
                    "elements": elements
                }
            ]
        }
    })
}

fn parse_tool_arguments(args: Option<&str>) -> Option<JsonValue> {
    let raw = args?.trim();
    if raw.is_empty() {
        return None;
    }
    serde_json::from_str(raw).ok()
}

fn extract_range_from_args(args: Option<&JsonValue>) -> Option<(usize, usize)> {
    let args = args?;
    let offset = args
        .get("offset")
        .and_then(|v| v.as_u64())
        .or_else(|| args.get("from").and_then(|v| v.as_u64()))
        .or_else(|| args.get("start_line").and_then(|v| v.as_u64()))
        .or_else(|| args.get("startLine").and_then(|v| v.as_u64()))
        .map(|v| v.max(1) as usize);
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let end = args
        .get("to")
        .and_then(|v| v.as_u64())
        .or_else(|| args.get("end_line").and_then(|v| v.as_u64()))
        .or_else(|| args.get("endLine").and_then(|v| v.as_u64()))
        .map(|v| v.max(1) as usize);

    match (offset, end, limit) {
        (Some(start), Some(end), _) => Some((start, start.max(end))),
        (Some(start), None, Some(limit)) if limit > 0 => {
            Some((start, start + limit.saturating_sub(1)))
        }
        (Some(start), None, None) => Some((start, start)),
        _ => None,
    }
}

fn extract_range_from_output(output: &str) -> Option<(usize, usize)> {
    let mut start = None;
    let mut end = None;
    for line in output.lines() {
        let Some(rest) = line.strip_prefix('L') else {
            continue;
        };
        let Some((number, _)) = rest.split_once(':') else {
            continue;
        };
        let Ok(line_number) = number.parse::<usize>() else {
            continue;
        };
        start = Some(start.map_or(line_number, |current: usize| current.min(line_number)));
        end = Some(end.map_or(line_number, |current: usize| current.max(line_number)));
    }
    match (start, end) {
        (Some(start), Some(end)) => Some((start, end)),
        _ => None,
    }
}

fn tool_call_summary(tool_name: &str, output: &str, args: Option<&str>) -> String {
    let parsed_args = parse_tool_arguments(args);
    match tool_name {
        "grep_files" => {
            let lines = output
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            let pattern = parsed_args
                .as_ref()
                .and_then(|args| args.get("pattern").and_then(|v| v.as_str()))
                .or_else(|| {
                    parsed_args
                        .as_ref()
                        .and_then(|args| args.get("query").and_then(|v| v.as_str()))
                })
                .unwrap_or("(unknown)");
            format!(
                "{} \"{}\" -> {} file(s)",
                semantic_prefix("Search", "blue"),
                normalize_card_markdown(pattern),
                lines
            )
        }
        "request_user_input" => {
            let question_count = parsed_args
                .as_ref()
                .and_then(|args| args.get("questions").and_then(|v| v.as_array()))
                .map(|questions| questions.len());
            match question_count {
                Some(count) => format!("{} ({count})", semantic_prefix("Request input", "indigo")),
                None => semantic_prefix("Request input", "indigo"),
            }
        }
        "read_file" => {
            let file_path = parsed_args
                .as_ref()
                .and_then(|args| args.get("file_path").and_then(|v| v.as_str()))
                .or_else(|| {
                    parsed_args
                        .as_ref()
                        .and_then(|args| args.get("path").and_then(|v| v.as_str()))
                })
                .or_else(|| {
                    parsed_args
                        .as_ref()
                        .and_then(|args| args.get("file").and_then(|v| v.as_str()))
                });
            let file_name = file_path.map(basename).unwrap_or("(unknown)");
            match extract_range_from_args(parsed_args.as_ref())
                .or_else(|| extract_range_from_output(output))
            {
                Some((start, end)) if start != end => {
                    format!(
                        "{} {} L{}-{}",
                        semantic_prefix("Read", "turquoise"),
                        normalize_card_markdown(file_name),
                        start,
                        end
                    )
                }
                Some((start, _)) => format!(
                    "{} {} L{}",
                    semantic_prefix("Read", "turquoise"),
                    normalize_card_markdown(file_name),
                    start
                ),
                None => format!(
                    "{} {}",
                    semantic_prefix("Read", "turquoise"),
                    normalize_card_markdown(file_name)
                ),
            }
        }
        "list_dir" => {
            let dir_path = parsed_args
                .as_ref()
                .and_then(|args| args.get("path").and_then(|v| v.as_str()))
                .or_else(|| {
                    parsed_args
                        .as_ref()
                        .and_then(|args| args.get("directory").and_then(|v| v.as_str()))
                })
                .or_else(|| {
                    output
                        .lines()
                        .find_map(|line| line.strip_prefix("Absolute path:").map(str::trim))
                });
            let dir_name = dir_path.map(basename).unwrap_or("(unknown)");
            format!(
                "{} {}",
                semantic_prefix("List", "lime"),
                normalize_card_markdown(dir_name)
            )
        }
        _ => format!(
            "{}: {}",
            semantic_prefix("Tool", "grey"),
            normalize_card_markdown(tool_name)
        ),
    }
}

fn build_read_file_tool_card(
    arguments: Option<String>,
    output: Option<String>,
) -> serde_json::Value {
    let header = tool_call_summary(
        "read_file",
        output.as_deref().unwrap_or_default(),
        arguments.as_deref(),
    );
    build_tool_result_card(
        "工具结果",
        "turquoise",
        "read_file",
        &header,
        arguments,
        output,
        None,
    )
}

fn build_function_tool_call_card(item: &JsonValue) -> serde_json::Value {
    let tool_name = item
        .get("toolName")
        .and_then(|v| v.as_str())
        .unwrap_or("tool");
    let arguments = item
        .get("arguments")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .or_else(|| item.get("arguments").map(pretty_json));
    let output_value = item.get("output").cloned().unwrap_or(JsonValue::Null);
    let output_text = output_value.as_str().unwrap_or_default();
    let header = tool_call_summary(tool_name, output_text, arguments.as_deref());
    let result = match output_value {
        JsonValue::Null => None,
        JsonValue::String(text) => {
            if text.trim().is_empty() {
                None
            } else if tool_name == "read_file" {
                Some(text)
            } else {
                Some(markdown_code_block("text", &text))
            }
        }
        value => Some(markdown_code_block("json", &pretty_json(&value))),
    };
    if tool_name == "read_file" {
        return build_read_file_tool_card(arguments, result);
    }
    build_tool_result_card(
        "工具结果",
        "turquoise",
        tool_name,
        &header,
        arguments,
        result,
        None,
    )
}

fn build_mcp_tool_call_card(item: &JsonValue) -> serde_json::Value {
    let server = item
        .get("server")
        .and_then(|v| v.as_str())
        .unwrap_or("server");
    let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
    let status = item
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let arguments = item.get("arguments").map(pretty_json);
    let result = item
        .get("result")
        .filter(|value| !value.is_null())
        .map(|value| markdown_code_block("json", &pretty_json(value)))
        .or_else(|| {
            item.get("error")
                .filter(|value| !value.is_null())
                .map(|value| markdown_code_block("json", &pretty_json(value)))
        });
    build_tool_result_card(
        "MCP 工具",
        "turquoise",
        &format!("{server}/{tool}"),
        &format!("{server}.{tool}"),
        arguments,
        result,
        Some(format!("**状态**: `{}`", normalize_card_markdown(status))),
    )
}

fn build_dynamic_tool_call_card(item: &JsonValue) -> serde_json::Value {
    let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
    let status = item
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let arguments = item.get("arguments").map(pretty_json);
    let result = item
        .get("contentItems")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|entry| match entry.get("type").and_then(|v| v.as_str()) {
                    Some("inputText") => entry
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(|text| markdown_code_block("text", text)),
                    Some("inputImage") => entry
                        .get("imageUrl")
                        .and_then(|v| v.as_str())
                        .map(|url| format!("图片输入：`{}`", normalize_card_markdown(url))),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .filter(|text| !text.trim().is_empty());
    build_tool_result_card(
        "动态工具",
        "turquoise",
        tool,
        tool,
        arguments,
        result,
        Some(format!("**状态**: `{}`", normalize_card_markdown(status))),
    )
}

fn card_title_for_item_type(item_type: &str) -> (&'static str, &'static str) {
    match item_type {
        "userMessage" => ("消息", "grey"),
        "agentMessage" => ("回复", "blue"),
        "reasoning" => ("思考", "wathet"),
        "plan" => ("计划", "indigo"),
        "functionToolCall" => ("工具调用", "turquoise"),
        "commandExecution" => ("命令执行", "lime"),
        "fileChange" => ("文件变更", "green"),
        "mcpToolCall" => ("MCP 工具", "turquoise"),
        "dynamicToolCall" => ("动态工具", "turquoise"),
        "collabAgentToolCall" => ("协作代理", "carmine"),
        "webSearch" => ("网页搜索", "purple"),
        "imageGeneration" => ("图片生成", "orange"),
        "todoList" => ("任务列表", "purple"),
        "imageView" => ("图片", "carmine"),
        _ => ("Arthas", DEFAULT_CARD_TEMPLATE),
    }
}

pub fn item_markdown_summary(item: &JsonValue) -> Option<String> {
    let item_type = item.get("type").and_then(|v| v.as_str())?;
    match item_type {
        "userMessage" => {
            let content = item.get("content").and_then(|v| v.as_array())?;
            let parts = content
                .iter()
                .filter_map(|entry| match entry.get("type").and_then(|v| v.as_str()) {
                    Some("text") => entry
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(normalize_card_markdown)
                        .filter(|text| !text.is_empty()),
                    Some("image") => entry
                        .get("url")
                        .and_then(|v| v.as_str())
                        .map(|url| format!("图片输入：`{}`", normalize_card_markdown(url))),
                    Some("localImage") => entry
                        .get("path")
                        .and_then(|v| v.as_str())
                        .map(|path| format!("本地图片：`{}`", normalize_card_markdown(path))),
                    Some("mention") => entry
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|name| format!("@{}", normalize_card_markdown(name))),
                    Some("skill") => entry
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|name| format!("技能：`{}`", normalize_card_markdown(name))),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        "agentMessage" => item
            .get("text")
            .and_then(|v| v.as_str())
            .map(normalize_card_markdown)
            .filter(|text| !text.is_empty()),
        "reasoning" => {
            let mut parts = Vec::new();
            if let Some(summary) = item.get("summary").and_then(|v| v.as_array()) {
                for entry in summary {
                    if let Some(text) = entry.get("text").and_then(|v| v.as_str()) {
                        let normalized = normalize_card_markdown(text);
                        if !normalized.is_empty() {
                            parts.push(normalized);
                        }
                    }
                }
            }
            if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                for entry in content {
                    if let Some(text) = entry
                        .get("text")
                        .and_then(|v| v.as_str())
                        .or_else(|| entry.as_str())
                    {
                        let normalized = normalize_card_markdown(text);
                        if !normalized.is_empty() {
                            parts.push(normalized);
                        }
                    }
                }
            }
            parts.dedup();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        "plan" => item
            .get("text")
            .and_then(|v| v.as_str())
            .map(normalize_card_markdown)
            .filter(|text| !text.is_empty()),
        "functionToolCall" => {
            let tool_name = item
                .get("toolName")
                .and_then(|v| v.as_str())
                .unwrap_or("tool");
            let args = item
                .get("arguments")
                .and_then(|v| v.as_str())
                .map(normalize_card_markdown)
                .filter(|text| !text.is_empty());
            let output = item
                .get("output")
                .and_then(|v| v.as_str())
                .map(normalize_card_markdown)
                .filter(|text| !text.is_empty());
            let mut body = format!("**工具**: `{tool_name}`");
            if let Some(args) = args {
                body.push_str(&format!("\n\n**参数**\n```json\n{args}\n```"));
            }
            if let Some(output) = output {
                body.push_str(&format!("\n\n**结果**\n```text\n{output}\n```"));
            }
            Some(body)
        }
        "commandExecution" => {
            let command = item
                .get("commandActions")
                .and_then(|v| v.as_array())
                .and_then(|actions| actions.first())
                .and_then(|action| action.get("command"))
                .and_then(|v| v.as_str())
                .or_else(|| item.get("command").and_then(|v| v.as_str()))
                .map(normalize_card_markdown)?;
            Some(format!("```bash\n{command}\n```"))
        }
        "fileChange" => {
            let changes = item.get("changes").and_then(|v| v.as_array())?;
            if changes.is_empty() {
                return Some("有文件被修改。".to_string());
            }
            let sections = changes
                .iter()
                .filter_map(|change| {
                    let path = change.get("path").and_then(|v| v.as_str())?;
                    let diff = change
                        .get("diff")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let kind_label = file_change_kind_label(change);
                    let (additions, deletions) = file_change_stats(change);
                    let mut section = format!(
                        "**{}** `{}`  `+{} -{}`",
                        kind_label, path, additions, deletions
                    );
                    if let Some(preview) = truncate_lines(diff, 8, 88) {
                        section.push_str(&format!(
                            "\n```diff\n{}\n```",
                            normalize_card_markdown(&preview)
                        ));
                    }
                    Some(section)
                })
                .collect::<Vec<_>>();
            Some(sections.join("\n\n"))
        }
        "mcpToolCall" => {
            let server = item
                .get("server")
                .and_then(|v| v.as_str())
                .unwrap_or("server");
            let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
            let status = item
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let mut body =
                format!("**服务**: `{server}`\n\n**工具**: `{tool}`\n\n**状态**: `{status}`");
            if let Some(arguments) = item.get("arguments") {
                let args = serde_json::to_string_pretty(arguments)
                    .unwrap_or_else(|_| arguments.to_string());
                body.push_str(&format!(
                    "\n\n**参数**\n```json\n{}\n```",
                    normalize_card_markdown(&args)
                ));
            }
            if let Some(result) = item.get("result").filter(|v| !v.is_null()) {
                let result_text =
                    serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string());
                body.push_str(&format!(
                    "\n\n**结果**\n```json\n{}\n```",
                    normalize_card_markdown(&result_text)
                ));
            }
            if let Some(error) = item.get("error").filter(|v| !v.is_null()) {
                let error_text =
                    serde_json::to_string_pretty(error).unwrap_or_else(|_| error.to_string());
                body.push_str(&format!(
                    "\n\n**错误**\n```json\n{}\n```",
                    normalize_card_markdown(&error_text)
                ));
            }
            Some(body)
        }
        "dynamicToolCall" => {
            let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
            let status = item
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let success = item.get("success").and_then(|v| v.as_bool());
            let mut body = format!("**工具**: `{tool}`\n\n**状态**: `{status}`");
            if let Some(arguments) = item.get("arguments") {
                let args = serde_json::to_string_pretty(arguments)
                    .unwrap_or_else(|_| arguments.to_string());
                body.push_str(&format!(
                    "\n\n**参数**\n```json\n{}\n```",
                    normalize_card_markdown(&args)
                ));
            }
            if let Some(content_items) = item.get("contentItems").and_then(|v| v.as_array()) {
                let mut parts = Vec::new();
                for entry in content_items {
                    match entry.get("type").and_then(|v| v.as_str()) {
                        Some("inputText") => {
                            if let Some(text) = entry.get("text").and_then(|v| v.as_str()) {
                                let text = normalize_card_markdown(text);
                                if !text.is_empty() {
                                    parts.push(format!("```text\n{text}\n```"));
                                }
                            }
                        }
                        Some("inputImage") => {
                            if let Some(image_url) = entry.get("imageUrl").and_then(|v| v.as_str())
                            {
                                let image_url = normalize_card_markdown(image_url);
                                if !image_url.is_empty() {
                                    parts.push(format!("图片输入: `{image_url}`"));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    body.push_str(&format!("\n\n**内容**\n{}", parts.join("\n\n")));
                }
            }
            if let Some(success) = success {
                body.push_str(&format!("\n\n**成功**: `{success}`"));
            }
            Some(body)
        }
        "collabAgentToolCall" => {
            let tool = item.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
            let sender_thread_id = item
                .get("senderThreadId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let receiver_ids = item
                .get("receiverThreadIds")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|entry| entry.as_str())
                        .map(|value| format!("- `{value}`"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let prompt = item
                .get("prompt")
                .and_then(|v| v.as_str())
                .map(normalize_card_markdown);
            let mut body = format!("**工具**: `{tool}`");
            if !sender_thread_id.trim().is_empty() {
                body.push_str(&format!("\n\n**发送线程**: `{sender_thread_id}`"));
            }
            if !receiver_ids.is_empty() {
                body.push_str(&format!("\n\n**接收线程**\n{}", receiver_ids.join("\n")));
            }
            if let Some(prompt) = prompt.filter(|text| !text.is_empty()) {
                body.push_str(&format!("\n\n**提示词**\n```text\n{prompt}\n```"));
            }
            Some(body)
        }
        "webSearch" => {
            let query = item
                .get("query")
                .and_then(|v| v.as_str())
                .map(normalize_card_markdown)?;
            let action = item
                .get("action")
                .map(|v| normalize_card_markdown(&v.to_string()));
            let mut body = format!("**查询**: {query}");
            if let Some(action) = action.filter(|text| !text.is_empty() && text != "null") {
                body.push_str(&format!("\n\n**动作**\n```json\n{action}\n```"));
            }
            Some(body)
        }
        "imageGeneration" => {
            let status = item
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let result = item
                .get("result")
                .and_then(|v| v.as_str())
                .filter(|v| !looks_like_image_data_url(v))
                .filter(|v| !looks_like_image_base64(v))
                .map(|v| truncate_text(v, 800))
                .map(|v| normalize_card_markdown(&v))
                .filter(|text| !text.is_empty());
            let saved_path = item
                .get("savedPath")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("saved_path").and_then(|v| v.as_str()))
                .map(|v| truncate_text(v, 260))
                .map(|v| normalize_card_markdown(&v))
                .filter(|text| !text.is_empty());
            let revised_prompt = item
                .get("revisedPrompt")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("revised_prompt").and_then(|v| v.as_str()))
                .map(|v| truncate_text(v, 1000))
                .map(|v| normalize_card_markdown(&v))
                .filter(|text| !text.is_empty());
            let mut body = format!("**状态**: `{status}`");
            if let Some(revised_prompt) = revised_prompt {
                body.push_str(&format!(
                    "\n\n**修订提示词**\n```text\n{revised_prompt}\n```"
                ));
            }
            if let Some(result) = result {
                body.push_str(&format!("\n\n**结果**\n```text\n{result}\n```"));
            }
            if let Some(saved_path) = saved_path {
                body.push_str(&format!("\n\n**保存路径**: `{saved_path}`"));
            }
            Some(body)
        }
        "todoList" => {
            let entries = item.get("items").and_then(|v| v.as_array())?;
            let lines = entries
                .iter()
                .filter_map(|entry| {
                    let text = entry.get("text").and_then(|v| v.as_str())?;
                    let status = entry
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("pending");
                    let marker = if status == "completed" { "x" } else { " " };
                    Some(format!("- [{marker}] {}", normalize_card_markdown(text)))
                })
                .collect::<Vec<_>>();
            (!lines.is_empty()).then(|| lines.join("\n"))
        }
        "imageView" => item
            .get("path")
            .and_then(|v| v.as_str())
            .map(|path| format!("已查看图片：`{}`", normalize_card_markdown(path)))
            .filter(|text| !text.is_empty()),
        _ => None,
    }
}

pub fn build_item_card(item: &JsonValue) -> Option<serde_json::Value> {
    let item_type = item.get("type").and_then(|v| v.as_str())?;
    match item_type {
        "plan" => return build_plan_item_card(item),
        "webSearch" => return build_web_search_card(item),
        "functionToolCall" => return Some(build_function_tool_call_card(item)),
        "mcpToolCall" => return Some(build_mcp_tool_call_card(item)),
        "dynamicToolCall" => return Some(build_dynamic_tool_call_card(item)),
        "collabAgentToolCall" => return Some(build_collab_agent_tool_call_card(item)),
        _ => {}
    }
    let content = item_markdown_summary(item)?;
    let (title, template) = card_title_for_item_type(item_type);
    Some(build_markdown_card(&content, Some(title), Some(template)))
}

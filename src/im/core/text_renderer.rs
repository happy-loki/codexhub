use std::path::PathBuf;

use std::collections::HashMap;

use serde_json::Value;

use crate::codex::extract_agent_message_text;

const SUMMARY_CHAR_LIMIT: usize = 2400;
const JSON_CHAR_LIMIT: usize = 1800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkdownImageRef {
    pub alt: String,
    pub target: String,
    pub path: PathBuf,
}

pub(crate) fn render_agent_message_text(text: &str) -> String {
    let text = replace_local_markdown_images_with_text(text);
    format!("{}\n\n{}", type_header("agentMessage"), text.trim())
}

pub(crate) fn local_markdown_image_refs(text: &str) -> Vec<MarkdownImageRef> {
    let mut refs = Vec::new();
    visit_markdown_images(text, |alt, target, _| {
        let Some(path) = local_existing_path_from_markdown_target(target) else {
            return;
        };
        refs.push(MarkdownImageRef {
            alt: alt.trim().to_string(),
            target: target.trim().to_string(),
            path,
        });
    });
    refs
}

pub(crate) fn replace_local_markdown_images_with_text(text: &str) -> String {
    rewrite_markdown_images(text, |alt, target, _original| {
        local_existing_path_from_markdown_target(target)
            .map(|_| markdown_image_text_replacement(alt, target))
    })
}

pub(crate) fn replace_markdown_image_targets(
    text: &str,
    replacements: &HashMap<String, String>,
) -> String {
    rewrite_markdown_images(text, |alt, target, _original| {
        replacements
            .get(target.trim())
            .map(|replacement| format!("![{}]({})", alt.trim(), replacement))
    })
}

pub(crate) fn render_item_text(item: &Value) -> Option<String> {
    let item_type = item.get("type").and_then(|v| v.as_str())?;
    match item_type {
        "agentMessage" => {
            extract_agent_message_text(item).map(|text| render_agent_message_text(&text))
        }
        "userMessage" => render_user_message(item),
        "todoList" => render_todo_list(item),
        "imageGeneration" => render_image_generation(item),
        "imageView" => render_image_view(item),
        "reasoning" => render_reasoning(item),
        "plan" => render_plain_text_item(item, "plan"),
        "commandExecution" => render_command_execution(item),
        "fileChange" => render_file_change(item),
        "contextCompaction" => Some(render_context_compaction()),
        "mcpToolCall" => render_mcp_tool_call(item),
        "dynamicToolCall" => render_dynamic_tool_call(item),
        "functionToolCall" => render_function_tool_call(item),
        "collabAgentToolCall" => render_collab_agent_tool_call(item),
        "webSearch" => render_web_search(item),
        _ => render_unknown_item(item, item_type),
    }
}

fn local_existing_path_from_markdown_target(target: &str) -> Option<PathBuf> {
    let target = target.trim().trim_matches('<').trim_matches('>');
    let target = target.strip_prefix("file:///").unwrap_or(target);
    let target = target.strip_prefix("file://").unwrap_or(target);
    let path = PathBuf::from(target);
    path.is_file().then_some(path)
}

fn visit_markdown_images(mut text: &str, mut visitor: impl FnMut(&str, &str, &str)) {
    loop {
        let Some(start) = text.find("![") else {
            break;
        };
        let candidate = &text[start..];
        let Some(alt_end) = candidate.find("](") else {
            break;
        };
        let target_start = alt_end + 2;
        let Some(target_end) = candidate[target_start..].find(')') else {
            break;
        };

        let full_end = target_start + target_end + 1;
        let alt = &candidate[2..alt_end];
        let target = &candidate[target_start..target_start + target_end];
        visitor(alt, target, &candidate[..full_end]);
        text = &candidate[full_end..];
    }
}

fn rewrite_markdown_images(
    mut text: &str,
    mut replacer: impl FnMut(&str, &str, &str) -> Option<String>,
) -> String {
    let mut output = String::new();
    loop {
        let Some(start) = text.find("![") else {
            output.push_str(text);
            break;
        };
        output.push_str(&text[..start]);

        let candidate = &text[start..];
        let Some(alt_end) = candidate.find("](") else {
            output.push_str(candidate);
            break;
        };
        let target_start = alt_end + 2;
        let Some(target_end) = candidate[target_start..].find(')') else {
            output.push_str(candidate);
            break;
        };

        let full_end = target_start + target_end + 1;
        let alt = &candidate[2..alt_end];
        let target = &candidate[target_start..target_start + target_end];
        let original = &candidate[..full_end];
        if let Some(replacement) = replacer(alt, target, original) {
            output.push_str(&replacement);
        } else {
            output.push_str(original);
        }
        text = &candidate[full_end..];
    }
    output
}

fn markdown_image_text_replacement(alt: &str, target: &str) -> String {
    let alt = alt.trim();
    let target = target.trim().replace('`', "'");
    match (alt.is_empty(), target.is_empty()) {
        (true, true) => "图片".to_string(),
        (true, false) => format!("图片：`{target}`"),
        (false, true) => format!("图片：{alt}"),
        (false, false) => format!("图片：{alt}（`{target}`）"),
    }
}

pub(crate) fn image_item_path(item: &Value) -> Option<PathBuf> {
    item.get("savedPath")
        .and_then(|v| v.as_str())
        .or_else(|| item.get("saved_path").and_then(|v| v.as_str()))
        .or_else(|| item.get("path").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn image_item_caption(item: &Value) -> String {
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("image");
    let mut lines = vec![type_header(item_type)];
    if let Some(status) = string_field(item, "status") {
        lines.push(format!("status: `{status}`"));
    }
    if item_type == "imageGeneration"
        && let Some(prompt) =
            string_field(item, "revisedPrompt").or_else(|| string_field(item, "revised_prompt"))
    {
        lines.push(format!(
            "revisedPrompt:\n```text\n{}\n```",
            truncate_summary(&prompt)
        ));
    }
    lines.join("\n")
}

fn render_todo_list(item: &Value) -> Option<String> {
    let entries = item.get("items").and_then(|v| v.as_array())?;
    let lines = entries
        .iter()
        .filter_map(|entry| {
            let text = entry.get("text").and_then(|v| v.as_str())?.trim();
            if text.is_empty() {
                return None;
            }
            let status = entry
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending");
            let marker = match status {
                "completed" | "done" => "x",
                "in_progress" | "running" => ">",
                _ => " ",
            };
            Some(format!("- [{marker}] {text}"))
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| format!("{}\n\n{}", type_header("todoList"), lines.join("\n")))
}

fn render_user_message(item: &Value) -> Option<String> {
    let content = item.get("content").and_then(|v| v.as_array())?;
    let parts = content
        .iter()
        .filter_map(|entry| match entry.get("type").and_then(|v| v.as_str()) {
            Some("text") => entry
                .get("text")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string),
            Some("image") => entry
                .get("url")
                .and_then(|v| v.as_str())
                .map(|url| format!("image: `{}`", truncate_middle(url, 240))),
            Some("localImage") => entry
                .get("path")
                .and_then(|v| v.as_str())
                .map(|path| format!("localImage: `{}`", truncate_middle(path, 240))),
            Some("mention") => entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(|name| format!("mention: @{name}")),
            Some("skill") => entry
                .get("name")
                .and_then(|v| v.as_str())
                .map(|name| format!("skill: `{name}`")),
            _ => None,
        })
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| format!("{}\n\n{}", type_header("userMessage"), parts.join("\n\n")))
}

fn render_image_generation(item: &Value) -> Option<String> {
    let mut sections = Vec::new();
    if let Some(status) = string_field(item, "status") {
        sections.push(format!("status: `{status}`"));
    }
    if let Some(prompt) =
        string_field(item, "revisedPrompt").or_else(|| string_field(item, "revised_prompt"))
    {
        sections.push(format!(
            "revisedPrompt:\n```text\n{}\n```",
            truncate_summary(&prompt)
        ));
    }
    if let Some(result) =
        string_field(item, "result").filter(|value| !looks_like_image_payload(value))
    {
        sections.push(format!(
            "result:\n```text\n{}\n```",
            truncate_summary(&result)
        ));
    }
    if sections.is_empty() {
        None
    } else {
        Some(format!(
            "{}\n\n{}",
            type_header("imageGeneration"),
            sections.join("\n\n")
        ))
    }
}

fn render_image_view(_item: &Value) -> Option<String> {
    Some(type_header("imageView"))
}

fn render_reasoning(item: &Value) -> Option<String> {
    let text = collect_reasoning_text(item)?;
    Some(format!(
        "{}\n\n{}",
        type_header("reasoning"),
        truncate_summary(&text)
    ))
}

fn render_plain_text_item(item: &Value, tag: &str) -> Option<String> {
    let text = item
        .get("text")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    Some(format!("{}\n\n{text}", type_header(tag)))
}

fn render_command_execution(item: &Value) -> Option<String> {
    let command = command_text(item)?;
    let status_icon = command_status_icon(item)
        .map(|icon| format!(" {icon}"))
        .unwrap_or_default();
    Some(format!(
        "💻 Ran: `{}`{}",
        truncate_middle(&single_line_text(&command), 180),
        status_icon
    ))
}

fn render_context_compaction() -> String {
    "🧠 上下文已压缩\n\n任务将继续执行。".to_string()
}

fn render_file_change(item: &Value) -> Option<String> {
    let changes = item.get("changes").and_then(|v| v.as_array())?;
    if changes.is_empty() {
        return Some("📝 文件修改\n\n没有文件变化。".to_string());
    }

    let mut additions = 0usize;
    let mut deletions = 0usize;
    let paths = changes
        .iter()
        .take(8)
        .filter_map(|change| {
            let path = change.get("path").and_then(|v| v.as_str())?.trim();
            if path.is_empty() {
                return None;
            }
            let kind = change
                .get("kind")
                .or_else(|| change.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("change");
            if let Some(diff) = change.get("diff").and_then(|v| v.as_str()) {
                let (change_additions, change_deletions) = diff_line_stats(diff);
                additions = additions.saturating_add(change_additions);
                deletions = deletions.saturating_add(change_deletions);
            }
            let icon = match kind.trim().to_ascii_lowercase().as_str() {
                "add" | "added" | "create" | "created" => "➕",
                "delete" | "deleted" | "remove" | "removed" => "🗑",
                _ => "✏️",
            };
            Some(format!("{icon} `{}`", truncate_middle(path, 240)))
        })
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return None;
    }

    let total = changes.len();
    let mut title = format!("📝 文件修改 · {total} 个文件");
    if additions > 0 || deletions > 0 {
        title.push_str(&format!(" · +{additions} -{deletions}"));
    }
    let mut sections = vec![title, paths.join("\n")];
    if total > paths.len() {
        sections.push(format!("… 另外 {} 个文件", total - paths.len()));
    }
    Some(sections.join("\n\n"))
}

fn diff_line_stats(diff: &str) -> (usize, usize) {
    diff.lines()
        .fold((0usize, 0usize), |(additions, deletions), line| {
            if line.starts_with("+++") || line.starts_with("---") {
                (additions, deletions)
            } else if line.starts_with('+') {
                (additions.saturating_add(1), deletions)
            } else if line.starts_with('-') {
                (additions, deletions.saturating_add(1))
            } else {
                (additions, deletions)
            }
        })
}

fn render_mcp_tool_call(item: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(server) = string_field(item, "server") {
        lines.push(format!("server: `{}`", truncate_middle(&server, 120)));
    }
    if let Some(tool) = string_field(item, "tool") {
        lines.push(format!("tool: `{}`", truncate_middle(&tool, 120)));
    }
    if let Some(title) = mcp_tool_title(item) {
        lines.push(format!(
            "title: {}",
            truncate_text(&single_line_text(&title), 240)
        ));
    }
    if let Some(status) = string_field(item, "status") {
        let status_icon = mcp_status_icon(&status)
            .map(|icon| format!("{icon} "))
            .unwrap_or_default();
        lines.push(format!("status: `{status_icon}{status}`"));
    }
    if let Some(summary) = mcp_result_summary(item) {
        lines.push(summary);
    }
    if let Some(error) = item.get("error").filter(|v| !v.is_null()) {
        let text = error.as_str().map(str::to_string).unwrap_or_else(|| {
            serde_json::to_string_pretty(error).unwrap_or_else(|_| error.to_string())
        });
        lines.push(format!(
            "error: {}",
            truncate_text(&single_line_text(&text), 600)
        ));
    }
    (!lines.is_empty()).then(|| format!("{}\n\n{}", type_header("mcpToolCall"), lines.join("\n\n")))
}

fn mcp_tool_title(item: &Value) -> Option<String> {
    item.get("arguments")
        .and_then(|arguments| arguments.get("title"))
        .and_then(|v| v.as_str())
        .or_else(|| item.get("title").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn mcp_result_summary(item: &Value) -> Option<String> {
    let result = item.get("result")?;
    let content = result.get("content").and_then(|v| v.as_array())?;
    let image_count = content
        .iter()
        .filter(|entry| entry.get("type").and_then(|v| v.as_str()) == Some("image"))
        .count();
    let text_count = content
        .iter()
        .filter(|entry| entry.get("type").and_then(|v| v.as_str()) == Some("text"))
        .count();
    match (image_count, text_count) {
        (0, 0) => None,
        (images, 0) => Some(format!("result: 已获取 {images} 张图片。")),
        (0, texts) => Some(format!("result: 已获取 {texts} 段文本结果。")),
        (images, texts) => Some(format!(
            "result: 已获取 {texts} 段文本结果和 {images} 张图片。"
        )),
    }
}

fn mcp_status_icon(status: &str) -> Option<&'static str> {
    match status {
        "completed" | "succeeded" | "success" => Some("✅"),
        "failed" | "error" | "canceled" | "cancelled" | "timed_out" | "timedout" => Some("❌"),
        "running" | "in_progress" | "inProgress" => Some("⏳"),
        _ => None,
    }
}

fn render_dynamic_tool_call(item: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(tool) = string_field(item, "tool") {
        lines.push(format!("tool: `{tool}`"));
    }
    if let Some(status) = string_field(item, "status") {
        lines.push(format!("status: `{status}`"));
    }
    if let Some(success) = item.get("success").and_then(|v| v.as_bool()) {
        lines.push(format!("success: `{success}`"));
    }
    push_json_section(
        &mut lines,
        "arguments",
        item.get("arguments"),
        JSON_CHAR_LIMIT,
    );
    push_json_section(
        &mut lines,
        "contentItems",
        item.get("contentItems"),
        JSON_CHAR_LIMIT,
    );
    (!lines.is_empty()).then(|| {
        format!(
            "{}\n\n{}",
            type_header("dynamicToolCall"),
            lines.join("\n\n")
        )
    })
}

fn render_function_tool_call(item: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(tool) = string_field(item, "toolName").or_else(|| string_field(item, "tool")) {
        lines.push(format!("tool: `{tool}`"));
    }
    if let Some(args) = string_field(item, "arguments") {
        lines.push(format!(
            "arguments:\n```json\n{}\n```",
            truncate_text(&args, JSON_CHAR_LIMIT)
        ));
    }
    if let Some(output) = string_field(item, "output") {
        lines.push(format!(
            "output:\n```text\n{}\n```",
            truncate_summary(&output)
        ));
    }
    (!lines.is_empty()).then(|| {
        format!(
            "{}\n\n{}",
            type_header("functionToolCall"),
            lines.join("\n\n")
        )
    })
}

fn render_collab_agent_tool_call(item: &Value) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(tool) = string_field(item, "tool") {
        lines.push(format!("tool: `{tool}`"));
    }
    if let Some(sender) = string_field(item, "senderThreadId") {
        lines.push(format!(
            "senderThreadId: `{}`",
            truncate_middle(&sender, 120)
        ));
    }
    push_json_section(
        &mut lines,
        "receiverThreadIds",
        item.get("receiverThreadIds"),
        JSON_CHAR_LIMIT,
    );
    if let Some(prompt) = string_field(item, "prompt") {
        lines.push(format!(
            "prompt:\n```text\n{}\n```",
            truncate_summary(&prompt)
        ));
    }
    (!lines.is_empty()).then(|| {
        format!(
            "{}\n\n{}",
            type_header("collabAgentToolCall"),
            lines.join("\n\n")
        )
    })
}

fn render_web_search(item: &Value) -> Option<String> {
    let query = string_field(item, "query")?;
    let mut lines = vec![format!("query: {}", truncate_text(&query, 500))];
    push_json_section(&mut lines, "action", item.get("action"), JSON_CHAR_LIMIT);
    push_json_section(&mut lines, "results", item.get("results"), JSON_CHAR_LIMIT);
    Some(format!(
        "{}\n\n{}",
        type_header("webSearch"),
        lines.join("\n\n")
    ))
}

fn render_unknown_item(item: &Value, item_type: &str) -> Option<String> {
    let summary = serde_json::to_string_pretty(item).ok()?;
    Some(format!(
        "{}\n\n```json\n{}\n```",
        type_header(item_type),
        truncate_text(&summary, SUMMARY_CHAR_LIMIT)
    ))
}

fn type_header(item_type: &str) -> String {
    match item_type {
        "agentMessage" => "🤖 Codex".to_string(),
        "userMessage" => "👤 用户".to_string(),
        "todoList" => "✅ 待办".to_string(),
        "imageGeneration" | "imageView" => "🖼 图片".to_string(),
        "reasoning" => "🧠 思考".to_string(),
        "plan" => "📋 计划".to_string(),
        "commandExecution" => "💻 命令".to_string(),
        "fileChange" => "📝 文件修改".to_string(),
        "contextCompaction" => "🧠 上下文已压缩".to_string(),
        "mcpToolCall" => "🧩 MCP 工具".to_string(),
        "dynamicToolCall" | "functionToolCall" => "🛠 工具".to_string(),
        "collabAgentToolCall" => "🤝 协作".to_string(),
        "webSearch" => "🔎 搜索".to_string(),
        other => format!("• {other}"),
    }
}

fn command_text(item: &Value) -> Option<String> {
    item.get("commandActions")
        .and_then(|v| v.as_array())
        .and_then(|actions| actions.first())
        .and_then(|action| action.get("command"))
        .and_then(command_value_text)
        .or_else(|| item.get("command").and_then(command_value_text))
        .filter(|v| !v.is_empty())
}

fn command_value_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.trim().to_string());
    }
    value.as_array().map(|parts| {
        parts
            .iter()
            .filter_map(|part| part.as_str().map(str::trim))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn command_status_icon(item: &Value) -> Option<&'static str> {
    if let Some(exit_code) = item.get("exitCode").and_then(|v| v.as_i64()) {
        return Some(if exit_code == 0 { "✅" } else { "❌" });
    }
    match item
        .get("status")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some("completed" | "succeeded" | "success") => Some("✅"),
        Some("failed" | "error" | "canceled" | "cancelled" | "timed_out" | "timedout") => {
            Some("❌")
        }
        Some("running" | "in_progress" | "inProgress") => Some("⏳"),
        _ => None,
    }
}

fn single_line_text(text: &str) -> String {
    text.replace("\r\n", " ")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_reasoning_text(item: &Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(summary) = item.get("summary").and_then(|v| v.as_array()) {
        for entry in summary {
            if let Some(text) = entry.get("text").and_then(|v| v.as_str()) {
                push_nonempty(&mut parts, text);
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
                push_nonempty(&mut parts, text);
            }
        }
    }
    parts.dedup();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn push_nonempty(parts: &mut Vec<String>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }
}

fn push_json_section(lines: &mut Vec<String>, label: &str, value: Option<&Value>, limit: usize) {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return;
    };
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    if text.trim().is_empty() || text == "null" {
        return;
    }
    lines.push(format!(
        "{label}:\n```json\n{}\n```",
        truncate_text(&text, limit)
    ));
}

fn string_field(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn truncate_summary(text: &str) -> String {
    truncate_text(text, SUMMARY_CHAR_LIMIT)
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(32);
    let head_len = keep / 2;
    let tail_len = keep.saturating_sub(head_len);
    let head = text.chars().take(head_len).collect::<String>();
    let tail = text
        .chars()
        .rev()
        .take(tail_len)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head}\n...[truncated]...\n{tail}")
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(16);
    let head_len = keep / 2;
    let tail_len = keep.saturating_sub(head_len);
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

fn looks_like_image_payload(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("data:image/")
        || (trimmed.len() > 256
            && trimmed.chars().all(|ch| {
                ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '\r' | '\n')
            }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use std::collections::HashMap;

    use super::{
        local_markdown_image_refs, render_agent_message_text, render_item_text,
        replace_markdown_image_targets,
    };

    #[test]
    fn agent_message_is_tagged_and_not_truncated() {
        let text = "hello\n".repeat(3000);
        let rendered = render_agent_message_text(&text);

        assert!(rendered.starts_with("🤖 Codex"));
        assert!(!rendered.contains("agentMessage"));
        assert!(rendered.contains(&"hello\n".repeat(100)));
        assert!(!rendered.contains("[truncated]"));
    }

    #[test]
    fn agent_message_local_markdown_image_is_rendered_as_text_and_extracted() {
        let path =
            std::env::temp_dir().join(format!("codexhub-md-image-{}.png", std::process::id()));
        std::fs::write(&path, b"png").expect("write temp image");
        let text = format!("看图：![preview]({})", path.display());

        let refs = local_markdown_image_refs(&text);
        let rendered = render_agent_message_text(&text);

        let _ = std::fs::remove_file(&path);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].alt, "preview");
        assert_eq!(refs[0].path, path);
        assert!(!rendered.contains("![preview]("));
        assert!(rendered.contains("图片：preview"));
    }

    #[test]
    fn markdown_image_target_can_be_replaced_with_uploaded_key() {
        let path = std::env::temp_dir().join(format!(
            "codexhub-md-image-replace-{}.png",
            std::process::id()
        ));
        std::fs::write(&path, b"png").expect("write temp image");
        let target = path.display().to_string();
        let text = format!("看图：![preview]({target})");
        let mut replacements = HashMap::new();
        replacements.insert(target, "img_v3_uploaded".to_string());

        let replaced = replace_markdown_image_targets(&text, &replacements);

        let _ = std::fs::remove_file(&path);
        assert_eq!(replaced, "看图：![preview](img_v3_uploaded)");
    }

    #[test]
    fn todo_list_is_rendered_fully() {
        let item = json!({
            "type": "todoList",
            "items": [
                {"text": "one", "status": "completed"},
                {"text": "two", "status": "pending"}
            ]
        });
        let rendered = render_item_text(&item).expect("todo list");

        assert!(rendered.starts_with("✅ 待办"));
        assert!(!rendered.contains("todoList"));
        assert!(rendered.contains("- [x] one"));
        assert!(rendered.contains("- [ ] two"));
    }

    #[test]
    fn command_execution_omits_output() {
        let output = (0..80)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let item = json!({
            "type": "commandExecution",
            "status": "completed",
            "command": "cargo test",
            "aggregatedOutput": output
        });
        let rendered = render_item_text(&item).expect("command item");

        assert_eq!(rendered, "💻 Ran: `cargo test` ✅");
        assert!(!rendered.contains("line 0"));
        assert!(!rendered.contains("output:"));
    }

    #[test]
    fn command_execution_uses_command_actions_and_failure_icon() {
        let item = json!({
            "type": "commandExecution",
            "exitCode": 2,
            "commandActions": [
                {"command": ["git", "diff", "--stat"]}
            ],
            "aggregatedOutput": "large output"
        });
        let rendered = render_item_text(&item).expect("command item");

        assert_eq!(rendered, "💻 Ran: `git diff --stat` ❌");
        assert!(!rendered.contains("large output"));
    }

    #[test]
    fn context_compaction_is_rendered_without_internal_json() {
        let item = json!({
            "id": "019fdf21-e9ff-7112-a9e0-b7e05cdc069e",
            "type": "contextCompaction"
        });

        let rendered = render_item_text(&item).expect("context compaction");

        assert_eq!(rendered, "🧠 上下文已压缩\n\n任务将继续执行。");
        assert!(!rendered.contains("019fdf21"));
        assert!(!rendered.contains("json"));
        assert!(!rendered.contains("contextCompaction"));
    }

    #[test]
    fn file_change_computes_stats_and_omits_raw_diff() {
        let item = json!({
            "id": "change-1",
            "type": "fileChange",
            "status": "completed",
            "changes": [{
                "kind": "change",
                "path": "/Users/miaopasi/codexhub/src/im/telegram/collab_progress.rs",
                "diff": "--- a/src/im/telegram/collab_progress.rs\n+++ b/src/im/telegram/collab_progress.rs\n@@ -1,2 +1,3 @@\n-old\n+new\n+another\n context"
            }]
        });

        let rendered = render_item_text(&item).expect("file change");

        assert!(rendered.starts_with("📝 文件修改 · 1 个文件 · +2 -1"));
        assert!(rendered.contains("collab_progress.rs`"));
        assert!(!rendered.contains("kind:"));
        assert!(!rendered.contains("diff:"));
        assert!(!rendered.contains("@@ -1,2"));
        assert!(!rendered.contains("+0 -0"));
    }

    #[test]
    fn mcp_tool_call_renders_summary_without_raw_payloads() {
        let item = json!({
            "type": "mcpToolCall",
            "server": "node_repl",
            "tool": "js",
            "status": "completed",
            "arguments": {
                "code": "nodeRepl.write(JSON.stringify(largeWindowTree))",
                "title": "重新读取微信内容"
            },
            "result": {
                "content": [
                    {"type": "text", "text": "{\"window\":{\"app\":\"D:\\\\Weixin\\\\Weixin.exe\"}}"},
                    {
                        "_meta": {"codex/imageDetail": "original"},
                        "type": "image",
                        "mimeType": "image/jpeg",
                        "data": "/9j/4AAQSkZJRgABAQAAAQABAAD"
                    }
                ]
            }
        });
        let rendered = render_item_text(&item).expect("mcp item");

        assert!(rendered.starts_with("🧩 MCP 工具"));
        assert!(rendered.contains("server: `node_repl`"));
        assert!(rendered.contains("tool: `js`"));
        assert!(rendered.contains("title: 重新读取微信内容"));
        assert!(rendered.contains("status: `✅ completed`"));
        assert!(rendered.contains("已获取 1 段文本结果和 1 张图片"));
        assert!(!rendered.contains("arguments:"));
        assert!(!rendered.contains("nodeRepl.write"));
        assert!(!rendered.contains("/9j/4AAQ"));
        assert!(!rendered.contains("Weixin.exe"));
    }
}

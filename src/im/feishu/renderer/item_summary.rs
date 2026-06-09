use serde_json::Value as JsonValue;

use crate::codex::extract_agent_message_text;

use super::DEFAULT_CARD_TEMPLATE;
use super::common::{
    file_change_kind_label, file_change_stats, looks_like_image_base64, looks_like_image_data_url,
    truncate_lines, truncate_text,
};
use super::markdown::normalize_card_markdown;
pub(super) fn card_title_for_item_type(item_type: &str) -> (&'static str, &'static str) {
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
        "agentMessage" => extract_agent_message_text(item)
            .map(|text| normalize_card_markdown(&text))
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

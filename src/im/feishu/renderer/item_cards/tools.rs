use serde_json::Value as JsonValue;

use super::super::common::{
    basename, build_light_collapsible_header, markdown_code_block, pretty_json, semantic_prefix,
    tool_header_background,
};
use super::super::markdown::normalize_card_markdown;
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

pub(in crate::im::feishu::renderer) fn build_function_tool_call_card(
    item: &JsonValue,
) -> serde_json::Value {
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

pub(in crate::im::feishu::renderer) fn build_mcp_tool_call_card(
    item: &JsonValue,
) -> serde_json::Value {
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

pub(in crate::im::feishu::renderer) fn build_dynamic_tool_call_card(
    item: &JsonValue,
) -> serde_json::Value {
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

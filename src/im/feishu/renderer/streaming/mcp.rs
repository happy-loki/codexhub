use super::super::common::{build_light_collapsible_header, tool_header_background};
use super::super::markdown::normalize_card_markdown;
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

use serde_json::Value as JsonValue;

use super::super::common::{build_light_collapsible_header, truncate_single_line};
use super::super::markdown::normalize_card_markdown;
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

pub(in crate::im::feishu::renderer) fn build_collab_agent_tool_call_card(
    item: &JsonValue,
) -> serde_json::Value {
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

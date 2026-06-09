use super::super::common::{
    build_light_collapsible_header, summarize_command_header, tool_header_background, truncate_text,
};
use super::super::markdown::normalize_card_markdown;
use super::super::{
    FEISHU_STREAMING_COMMAND_COMMAND_CHARS, FEISHU_STREAMING_COMMAND_META_CHARS,
    FEISHU_STREAMING_COMMAND_OUTPUT_CHARS,
};
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

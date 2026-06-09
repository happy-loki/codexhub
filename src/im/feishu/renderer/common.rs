use serde_json::Value as JsonValue;

use super::DEFAULT_CARD_TEMPLATE;
use super::markdown::normalize_card_markdown;

pub(super) fn pretty_json(value: &JsonValue) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub(super) fn markdown_code_block(lang: &str, text: &str) -> String {
    format!("```{lang}\n{}\n```", normalize_card_markdown(text))
}

pub(super) fn parse_unified_diff_stats(diff: &str) -> (usize, usize) {
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

pub(super) fn file_change_stats(change: &JsonValue) -> (usize, usize) {
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

pub(super) fn truncate_lines(
    text: &str,
    max_lines: usize,
    max_chars_per_line: usize,
) -> Option<String> {
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

pub(super) fn truncate_single_line(text: &str, max_chars: usize) -> String {
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

pub(super) fn truncate_text(text: &str, max_chars: usize) -> String {
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

pub(super) fn looks_like_image_data_url(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("data:image/") && trimmed.contains(";base64,")
}

pub(super) fn looks_like_image_base64(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("iVBORw0KGgo")
        || trimmed.starts_with("/9j/")
        || trimmed.starts_with("R0lGOD")
        || trimmed.starts_with("UklGR")
}

pub(super) fn image_element(image_key: &str, alt: &str) -> serde_json::Value {
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

pub(super) fn summarize_command_header(text: &str) -> String {
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

pub(super) fn tool_header_background(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "commandExecution" => None,
        "read_file" => None,
        "list_dir" => None,
        "mcpToolCall" => None,
        _ => None,
    }
}

pub(super) fn semantic_prefix(label: &str, color: &str) -> String {
    format!(
        "<font color='{color}'>{}</font>",
        normalize_card_markdown(label)
    )
}

pub(super) fn build_light_collapsible_header(title: serde_json::Value) -> serde_json::Value {
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

pub(super) fn file_change_kind_label(change: &JsonValue) -> &'static str {
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

pub(super) fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[allow(dead_code)]
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

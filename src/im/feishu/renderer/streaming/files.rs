use super::super::common::{
    basename, build_light_collapsible_header, parse_unified_diff_stats, truncate_lines,
};
use super::super::markdown::normalize_card_markdown;
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

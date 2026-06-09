use crate::im::core::i18n::ImText;

use super::super::common::build_markdown_card;
use super::super::markdown::normalize_card_markdown;
use super::common::FeishuThreadListEntry;
fn build_thread_list_row(
    request_id: &str,
    entry: &FeishuThreadListEntry,
    text: ImText,
) -> serde_json::Value {
    let primary_text = if entry.title.trim().is_empty() {
        normalize_card_markdown(&text.thread_title_fallback(&entry.thread_id))
    } else {
        normalize_card_markdown(&entry.title)
    };
    let mut details = Vec::new();
    if let Some(state) = thread_state_suffix(&entry.state, text) {
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

fn thread_project_header(cwd: Option<&str>, text: ImText) -> serde_json::Value {
    let content = match cwd {
        Some(cwd) => {
            let name = project_name(cwd);
            format!(
                "**{}**\n<font color='grey'>{}</font>",
                normalize_card_markdown(&text.project_header(&name)),
                normalize_card_markdown(cwd)
            )
        }
        None => format!(
            "**{}**",
            normalize_card_markdown(text.unknown_project_header())
        ),
    };
    serde_json::json!({
        "tag": "markdown",
        "content": content
    })
}

fn thread_state_suffix(state: &str, text: ImText) -> Option<&'static str> {
    if state.contains("当前会话") || state.contains("Current session") {
        Some(text.current_short())
    } else if state.contains("已加载") || state.contains("Loaded") {
        Some(text.loaded_short())
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

pub fn build_thread_list_card(
    request_id: &str,
    title: &str,
    body: &str,
    entries: &[FeishuThreadListEntry],
    page: usize,
    has_prev: bool,
    has_next: bool,
    text: ImText,
) -> serde_json::Value {
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": normalize_card_markdown(body)
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": format!(
                "<font color='grey'>{}</font>",
                text.page_label(page)
            )
        }),
    ];

    if entries.is_empty() {
        elements.push(serde_json::json!({
            "tag": "markdown",
            "content": format!(
                "<font color='grey'>{}</font>",
                text.no_restorable_history_workspace()
            )
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
                elements.push(thread_project_header(cwd, text));
                current_cwd = cwd;
            }
            elements.push(build_thread_list_row(request_id, entry, text));
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
                        "content": text.previous_page_button()
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
                        "content": text.next_page_button()
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

#[allow(dead_code)]
pub fn build_thread_list_loading_card(title: &str, page: usize) -> serde_json::Value {
    let body = format!(
        "正在加载历史会话...\n\n<font color='grey'>第 {} 页</font>",
        page.max(1)
    );
    let mut card = build_markdown_card(&body, Some(title), Some("grey"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card
}

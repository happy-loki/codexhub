use super::super::common::build_markdown_card;
use super::super::markdown::normalize_card_markdown;
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

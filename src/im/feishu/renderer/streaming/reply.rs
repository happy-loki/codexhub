use super::super::markdown::normalize_card_markdown;
pub fn build_turn_completed_card(reply_text: &str) -> serde_json::Value {
    let content = normalize_card_markdown(reply_text);
    build_streaming_reply_card(&content, true)
}

fn build_agent_message_header_card(content: &str, show_generating: bool) -> serde_json::Value {
    let body_elements = if show_generating {
        serde_json::json!([
            {
                "tag": "column_set",
                "flex_mode": "none",
                "horizontal_spacing": "0px",
                "background_style": "indigo-50",
                "columns": [
                    {
                        "tag": "column",
                        "width": "weighted",
                        "weight": 1,
                        "padding": "0px",
                        "margin": "0px",
                        "vertical_spacing": "4px",
                        "elements": [
                            {
                                "tag": "markdown",
                                "content": content
                            },
                            {
                                "tag": "markdown",
                                "content": "<font color='grey'>生成中</font>"
                            }
                        ]
                    }
                ]
            }
        ])
    } else {
        serde_json::json!([
            {
                "tag": "column_set",
                "flex_mode": "none",
                "horizontal_spacing": "0px",
                "background_style": "indigo-50",
                "columns": [
                    {
                        "tag": "column",
                        "width": "weighted",
                        "weight": 1,
                        "padding": "0px",
                        "margin": "0px",
                        "vertical_spacing": "0px",
                        "elements": [
                            {
                                "tag": "markdown",
                                "content": content
                            }
                        ]
                    }
                ]
            }
        ])
    };
    let card = serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": " "
            },
            "template": "default",
            "icon": {
                "tag": "standard_icon",
                "token": "robot_outlined",
                "color": "blue"
            },
            "padding": "8px 8px 4px 8px"
        },
        "body": {
            "padding": "0px",
            "elements": body_elements
        }
    });
    card
}

pub fn build_streaming_reply_card(text: &str, is_completed: bool) -> serde_json::Value {
    let content = normalize_card_markdown(text);
    build_agent_message_header_card(&content, !is_completed)
}

pub fn build_turn_terminal_mark_card(state_text: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "body": {
            "padding": "0px 8px 0px 8px",
            "elements": [
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "0px",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "vertical_align": "center",
                            "padding": "6px 0px 6px 0px",
                            "elements": [
                                {
                                    "tag": "markdown",
                                    "content": state_text
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}

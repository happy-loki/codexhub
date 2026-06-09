use super::super::FEISHU_CARDKIT_STREAMING_ELEMENT_ID;
pub fn build_cardkit_streaming_reply_card() -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true,
            "streaming_mode": true,
            "summary": {
                "content": "Generating..."
            }
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": "",
                    "element_id": FEISHU_CARDKIT_STREAMING_ELEMENT_ID
                },
                {
                    "tag": "hr"
                },
                {
                    "tag": "markdown",
                    "content": "_状态：生成中_"
                }
            ]
        }
    })
}

pub fn build_cardkit_streaming_agent_message_card() -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true,
            "streaming_mode": true,
            "summary": {
                "content": "生成中"
            }
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
            "elements": [
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
                                    "content": "",
                                    "element_id": FEISHU_CARDKIT_STREAMING_ELEMENT_ID
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}

pub fn build_cardkit_streaming_tool_card(
    title: &str,
    template: &str,
    status_text: &str,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true,
            "streaming_mode": true,
            "summary": {
                "content": "Generating..."
            }
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": title
            },
            "template": template
        },
        "body": {
            "padding": "0px 8px 0px 8px",
            "elements": [
                {
                    "tag": "markdown",
                    "content": "",
                    "element_id": FEISHU_CARDKIT_STREAMING_ELEMENT_ID
                },
                {
                    "tag": "hr"
                },
                {
                    "tag": "markdown",
                    "content": format!("_{}_",
                        status_text
                    )
                }
            ]
        }
    })
}

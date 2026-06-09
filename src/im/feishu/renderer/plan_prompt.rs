#[allow(dead_code)]
pub fn build_plan_implement_prompt_card() -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "header": {
            "title": {
                "tag": "plain_text",
                "content": "计划已完成"
            },
            "template": "wathet"
        },
        "body": {
            "elements": [
                {
                    "tag": "markdown",
                    "content": "要开始按这份计划继续实现吗？"
                },
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "8px",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "elements": [
                                {
                                    "tag": "interactive_container",
                                    "width": "fill",
                                    "height": "auto",
                                    "horizontal_align": "center",
                                    "background_style": "default",
                                    "has_border": true,
                                    "border_color": "grey",
                                    "corner_radius": "8px",
                                    "padding": "8px 12px 8px 12px",
                                    "behaviors": [
                                        {
                                            "type": "callback",
                                            "value": {
                                                "kind": "plan_implement_prompt",
                                                "action": "dismiss"
                                            }
                                        }
                                    ],
                                    "elements": [
                                        {
                                            "tag": "markdown",
                                            "content": "先不继续"
                                        }
                                    ]
                                }
                            ]
                        },
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "elements": [
                                {
                                    "tag": "interactive_container",
                                    "width": "fill",
                                    "height": "auto",
                                    "horizontal_align": "center",
                                    "background_style": "wathet",
                                    "has_border": true,
                                    "border_color": "blue",
                                    "corner_radius": "8px",
                                    "padding": "8px 12px 8px 12px",
                                    "behaviors": [
                                        {
                                            "type": "callback",
                                            "value": {
                                                "kind": "plan_implement_prompt",
                                                "action": "implement"
                                            }
                                        }
                                    ],
                                    "elements": [
                                        {
                                            "tag": "markdown",
                                            "content": "开始实现"
                                        }
                                    ]
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    })
}

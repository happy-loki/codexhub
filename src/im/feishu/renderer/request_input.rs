use crate::im::feishu::types::FeishuUserInputQuestion;

use super::common::build_markdown_card;
use super::markdown::normalize_card_markdown;

#[allow(dead_code)]
fn request_input_field_name(index: usize) -> String {
    format!("q_{}", index + 1)
}

#[allow(dead_code)]
fn request_input_other_field_name(index: usize) -> String {
    format!("q_{}_other", index + 1)
}

#[allow(dead_code)]
pub fn build_request_user_input_card(
    request_id: &str,
    question: &FeishuUserInputQuestion,
    index: usize,
    total: usize,
    text_entry_mode: bool,
    current_answers: &[String],
    resolved: bool,
) -> serde_json::Value {
    let title = if question.header.trim().is_empty() {
        format!("问题 {}", index + 1)
    } else {
        normalize_card_markdown(&question.header)
    };
    let prompt = normalize_card_markdown(&question.question);
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": format!("<font color='grey'>第 {}/{} 题</font>", index + 1, total.max(1))
        }),
        serde_json::json!({
            "tag": "div",
            "text": {
                "tag": "plain_text",
                "content": title,
                "text_size": "heading"
            }
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": prompt
        }),
    ];

    let has_options = question
        .options
        .as_ref()
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    if has_options && !text_entry_mode {
        for option in question.options.as_ref().into_iter().flatten() {
            let label = normalize_card_markdown(&option.label);
            let desc = normalize_card_markdown(&option.description);
            let selected = current_answers
                .iter()
                .any(|answer| answer.trim() == option.label.trim());
            let mut inner_elements = vec![serde_json::json!({
                "tag": "markdown",
                "content": label
            })];
            if !desc.is_empty() {
                inner_elements.push(serde_json::json!({
                    "tag": "markdown",
                    "content": format!("<font color='grey'>{}</font>", desc)
                }));
            }
            let mut container = serde_json::json!({
                "tag": "interactive_container",
                "width": "fill",
                "height": "auto",
                "horizontal_align": "left",
                "background_style": if resolved && selected { "wathet" } else { "default" },
                "has_border": true,
                "border_color": if resolved && selected { "blue" } else { "grey" },
                "corner_radius": "8px",
                "padding": "8px 12px 8px 12px",
                "elements": inner_elements
            });
            if !resolved {
                container["behaviors"] = serde_json::json!([
                    {
                        "type": "callback",
                        "value": {
                            "kind": "request_user_input_option",
                            "requestId": request_id,
                            "questionId": question.id,
                            "answer": option.label
                        }
                    }
                ]);
            }
            elements.push(container);
        }
        if question.is_other && !resolved {
            elements.push(serde_json::json!({
                "tag": "interactive_container",
                "width": "fill",
                "height": "auto",
                "horizontal_align": "left",
                "background_style": "default",
                "has_border": true,
                "border_color": "grey",
                "corner_radius": "8px",
                "padding": "8px 12px 8px 12px",
                "behaviors": [
                    {
                        "type": "callback",
                        "value": {
                            "kind": "request_user_input_other",
                            "requestId": request_id,
                            "questionId": question.id
                        }
                    }
                ],
                "elements": [
                    {
                        "tag": "markdown",
                        "content": "其他"
                    },
                    {
                        "tag": "markdown",
                        "content": "<font color='grey'>手动填写你自己的答案</font>"
                    }
                ]
            }));
        }
    }

    if (text_entry_mode || !has_options) && !resolved {
        elements.push(serde_json::json!({
            "tag": "form",
            "name": "request_user_input_form",
            "element_id": "request_input_form",
            "direction": "vertical",
            "vertical_spacing": "8px",
            "elements": [
                {
                    "tag": "input",
                    "name": "free_text",
                    "required": false
                },
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "auto",
                            "elements": [
                                {
                                    "tag": "button",
                                    "type": "primary",
                                    "text": {
                                        "tag": "plain_text",
                                        "content": "提交"
                                    },
                                    "name": "submit_request_user_input",
                                    "form_action_type": "submit",
                                    "behaviors": [
                                        {
                                            "type": "callback",
                                            "value": {
                                                "kind": "request_user_input_text",
                                                "requestId": request_id,
                                                "questionId": question.id
                                            }
                                        }
                                    ]
                                }
                            ]
                        }
                    ]
                }
            ]
        }));
    }

    if !resolved {
        elements.push(serde_json::json!({
            "tag": "interactive_container",
            "width": "fill",
            "height": "auto",
            "horizontal_align": "left",
            "background_style": "default",
            "has_border": true,
            "border_color": "grey",
            "corner_radius": "8px",
            "padding": "8px 12px 8px 12px",
            "behaviors": [
                {
                    "type": "callback",
                    "value": {
                        "kind": "request_user_input_skip",
                        "requestId": request_id,
                        "questionId": question.id
                    }
                }
            ],
            "elements": [
                {
                    "tag": "markdown",
                    "content": "跳过"
                },
                {
                    "tag": "markdown",
                    "content": "<font color='grey'>暂时不回答这一题</font>"
                }
            ]
        }));
    }

    let mut card = build_markdown_card("", Some("需要你的输入"), Some("indigo"));
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

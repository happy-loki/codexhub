use serde_json::Value as JsonValue;

use crate::im::core::i18n::ImText;
use crate::im_runtime::ApprovalDecisionOption;

use super::APPROVAL_CARD_TEMPLATE;
use super::common::build_markdown_card;
use super::markdown::normalize_card_markdown;

pub fn build_approval_card(
    kind_label: &str,
    summary: &str,
    decisions: &[ApprovalDecisionOption],
    request_key: &str,
    text: ImText,
) -> serde_json::Value {
    let content = normalize_card_markdown(summary);
    let mut elements = vec![
        {
            serde_json::json!({
                "tag": "markdown",
                "content": format!(
                    "**{}: `{}`**",
                    normalize_card_markdown(text.approval_request_heading()),
                    normalize_card_markdown(kind_label)
                )
            })
        },
        {
            serde_json::json!({
                "tag": "markdown",
                "content": content
            })
        },
    ];
    if !decisions.is_empty() {
        elements.push(serde_json::json!({
            "tag": "hr"
        }));
        elements.push(build_approval_button_row(decisions, request_key));
    }
    let mut card = build_markdown_card(
        "",
        Some(text.approval_pending_title()),
        Some(APPROVAL_CARD_TEMPLATE),
    );
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_resolved_approval_card(
    kind_label: &str,
    summary: &str,
    decision_label: &str,
    option_index: usize,
    text: ImText,
) -> serde_json::Value {
    let content = normalize_card_markdown(summary);
    let selected = normalize_card_markdown(decision_label.trim());
    let elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": format!(
                "**{}: `{}`**",
                normalize_card_markdown(text.approval_request_heading()),
                normalize_card_markdown(kind_label)
            )
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": content
        }),
        serde_json::json!({
            "tag": "hr"
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": format!(
                "**{}**",
                normalize_card_markdown(&text.approval_selected_label(option_index, &selected))
            )
        }),
    ];
    let mut card = build_markdown_card("", Some(text.approval_resolved_title()), Some("green"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

fn build_approval_button_row(
    decisions: &[ApprovalDecisionOption],
    request_key: &str,
) -> serde_json::Value {
    let columns = decisions
        .iter()
        .enumerate()
        .map(|(index, decision)| {
            let option_index = index + 1;
            let primary = index == 0 && !decision_is_negative(&decision.decision);
            serde_json::json!({
                "tag": "column",
                "width": "auto",
                "padding": "0px 0px 0px 0px",
                "vertical_spacing": "0px",
                "elements": [
                    {
                        "tag": "button",
                        "text": {
                            "tag": "plain_text",
                            "content": decision.label.trim()
                        },
                        "type": if primary { "primary_filled" } else { "default" },
                        "width": "default",
                        "behaviors": [
                            {
                                "type": "callback",
                                "value": {
                                    "kind": "codex_approval_decision",
                                    "option": option_index,
                                    "requestKey": request_key
                                }
                            }
                        ]
                    }
                ]
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "tag": "column_set",
        "flex_mode": "flow",
        "horizontal_spacing": "8px",
        "horizontal_align": "left",
        "columns": columns
    })
}

fn decision_is_negative(decision: &JsonValue) -> bool {
    decision
        .as_str()
        .is_some_and(|value| matches!(value, "decline" | "cancel" | "denied"))
        || decision
            .get("applyNetworkPolicyAmendment")
            .and_then(|value| value.get("network_policy_amendment"))
            .and_then(|value| value.get("action"))
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == "deny")
}

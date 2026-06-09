use crate::im::core::i18n::ImText;

use super::super::common::build_markdown_card;
use super::super::markdown::normalize_card_markdown;
use super::common::{FeishuThreadRoutingAction, build_interactive_choice_block};
pub fn build_thread_routing_choice_card(
    title: &str,
    body: &str,
    actions: &[FeishuThreadRoutingAction],
    text: ImText,
) -> serde_json::Value {
    let mut elements = vec![
        serde_json::json!({
            "tag": "markdown",
            "content": normalize_card_markdown(body)
        }),
        serde_json::json!({
            "tag": "markdown",
            "content": format!("<font color='orange'>{}</font>", normalize_card_markdown(text.create_choice_tip_feishu()))
        }),
    ];
    for action in actions {
        elements.push(build_interactive_choice_block(
            &action.label,
            &action.description,
            action.value.clone(),
            action.primary,
            action.selected,
            action.resolved,
            text,
        ));
    }

    let mut card = build_markdown_card("", Some(title), Some("indigo"));
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card["body"]["vertical_spacing"] = serde_json::json!("8px");
    card["body"]["elements"] = serde_json::Value::Array(elements);
    card
}

pub fn build_thread_routing_result_card(title: &str, body: &str) -> serde_json::Value {
    let mut card = build_markdown_card(
        normalize_card_markdown(body).as_str(),
        Some(title),
        Some("blue"),
    );
    card["body"]["padding"] = serde_json::json!("8px 8px 8px 8px");
    card
}

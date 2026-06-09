use crate::im::core::i18n::ImText;

use super::super::markdown::normalize_card_markdown;
#[derive(Debug, Clone)]
pub struct FeishuThreadRoutingAction {
    pub label: String,
    pub description: String,
    pub value: serde_json::Value,
    pub primary: bool,
    pub selected: bool,
    pub resolved: bool,
}

#[derive(Debug, Clone)]
pub struct FeishuThreadListEntry {
    pub thread_id: String,
    pub title: String,
    pub state: String,
    pub cwd: Option<String>,
}

pub(super) fn build_interactive_choice_block(
    label: &str,
    description: &str,
    value: serde_json::Value,
    _primary: bool,
    selected: bool,
    resolved: bool,
    text: ImText,
) -> serde_json::Value {
    let title_content = if selected {
        format!(
            "**<font color='blue'>{}</font>**",
            normalize_card_markdown(&text.selected_prefix_feishu(label))
        )
    } else {
        normalize_card_markdown(label)
    };
    let mut container = serde_json::json!({
        "tag": "interactive_container",
        "width": "fill",
        "height": "auto",
        "horizontal_align": "left",
        "background_style": "default",
        "has_border": true,
        "border_color": if selected { "blue" } else { "grey" },
        "corner_radius": "8px",
        "padding": "10px 12px 10px 12px",
        "elements": [
            {
                "tag": "markdown",
                "content": title_content
            },
            {
                "tag": "markdown",
                "content": format!("<font color='grey'>{}</font>", normalize_card_markdown(description))
            }
        ],
        "behaviors": if resolved { serde_json::json!([]) } else { serde_json::json!([
            {
                "type": "callback",
                "value": value
            }
        ]) }
    });
    if description.trim().is_empty() {
        container["elements"] = serde_json::json!([
            {
                "tag": "markdown",
                "content": title_content
            }
        ]);
    }
    container
}

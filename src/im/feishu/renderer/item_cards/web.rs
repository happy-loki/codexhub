use serde_json::Value as JsonValue;

use super::super::markdown::normalize_card_markdown;
pub(in crate::im::feishu::renderer) fn build_web_search_card(
    item: &JsonValue,
) -> Option<serde_json::Value> {
    let query = item
        .get("query")
        .and_then(|v| v.as_str())
        .map(normalize_card_markdown)
        .filter(|text| !text.is_empty())?;
    Some(serde_json::json!({
        "schema": "2.0",
        "config": {
            "wide_screen_mode": true
        },
        "header": {
            "title": {
                "tag": "plain_text",
                "content": "webSearch"
            },
            "icon": {
                "tag": "standard_icon",
                "token": "search_outlined",
                "color": "purple"
            },
            "padding": "8px 8px 4px 8px"
        },
        "body": {
            "padding": "0px 8px 12px 8px",
            "elements": [
                {
                    "tag": "column_set",
                    "flex_mode": "none",
                    "horizontal_spacing": "0px",
                    "background_style": "purple-50",
                    "columns": [
                        {
                            "tag": "column",
                            "width": "weighted",
                            "weight": 1,
                            "padding": "6px 8px 6px 8px",
                            "margin": "0px",
                            "elements": [
                                {
                                    "tag": "markdown",
                                    "content": format!("`{}`", query)
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    }))
}

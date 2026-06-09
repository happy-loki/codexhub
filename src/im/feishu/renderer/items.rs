use serde_json::Value as JsonValue;

use super::common::build_markdown_card;
use super::item_cards::{
    build_collab_agent_tool_call_card, build_dynamic_tool_call_card, build_function_tool_call_card,
    build_mcp_tool_call_card, build_plan_item_card, build_web_search_card,
};
use super::item_summary::card_title_for_item_type;

pub use super::item_summary::item_markdown_summary;

pub fn build_item_card(item: &JsonValue) -> Option<serde_json::Value> {
    let item_type = item.get("type").and_then(|v| v.as_str())?;
    match item_type {
        "plan" => return build_plan_item_card(item),
        "webSearch" => return build_web_search_card(item),
        "functionToolCall" => return Some(build_function_tool_call_card(item)),
        "mcpToolCall" => return Some(build_mcp_tool_call_card(item)),
        "dynamicToolCall" => return Some(build_dynamic_tool_call_card(item)),
        "collabAgentToolCall" => return Some(build_collab_agent_tool_call_card(item)),
        _ => {}
    }
    let content = item_markdown_summary(item)?;
    let (title, template) = card_title_for_item_type(item_type);
    Some(build_markdown_card(&content, Some(title), Some(template)))
}

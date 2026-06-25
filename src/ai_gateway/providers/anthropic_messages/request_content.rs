use serde_json::{Value, json};

use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::{ContentPart, ItemContent, ItemType, JsonString, ResponseItem};
use crate::ai_gateway::tool_names::ToolNameMap;

pub(super) fn build_anthropic_messages(
    input: &[ResponseItem],
    tool_name_map: &mut ToolNameMap,
) -> Result<Vec<Value>, GatewayError> {
    let mut messages = Vec::new();
    for item in input {
        match item.item_type {
            ItemType::Message
            | ItemType::InputText
            | ItemType::InputImage
            | ItemType::OutputText => {
                let role = anthropic_role(item.role.as_deref(), &item.item_type);
                let content = anthropic_content_blocks(item);
                if !content.is_empty() {
                    messages.push(json!({
                        "role": role,
                        "content": content,
                    }));
                }
            }
            ItemType::FunctionCallOutput
            | ItemType::ToolSearchOutput
            | ItemType::CustomToolCallOutput => {
                let call_id = item.call_id.as_deref().unwrap_or("");
                if call_id.is_empty() {
                    return Err(GatewayError::bad_request(
                        "anthropic_messages requires tool output call_id",
                    ));
                }
                let content = match item.item_type {
                    ItemType::ToolSearchOutput => tool_search_output_to_anthropic_content(item),
                    _ => item
                        .output
                        .as_ref()
                        .map(|output| output.to_chat_tool_content())
                        .unwrap_or_default(),
                };
                messages.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": content,
                    }],
                }));
            }
            ItemType::FunctionCall | ItemType::ToolSearchCall | ItemType::CustomToolCall => {
                if let Some(block) = response_tool_call_to_anthropic(item, tool_name_map) {
                    messages.push(json!({
                        "role": "assistant",
                        "content": [block],
                    }));
                }
            }
            _ => {}
        }
    }

    if messages.is_empty() {
        return Err(GatewayError::bad_request(
            "anthropic_messages requires at least one user or assistant message",
        ));
    }
    Ok(messages)
}

fn tool_search_output_to_anthropic_content(item: &ResponseItem) -> String {
    serde_json::to_string(&json!({
        "status": item.status.as_deref().unwrap_or("completed"),
        "execution": item.execution.as_deref().unwrap_or("client"),
        "tools": item.tools.clone().unwrap_or_default(),
    }))
    .unwrap_or_else(|_| "{\"tools\":[]}".to_string())
}

fn response_tool_call_to_anthropic(
    item: &ResponseItem,
    tool_name_map: &mut ToolNameMap,
) -> Option<Value> {
    let (name, input) = match item.item_type {
        ItemType::FunctionCall => {
            let name = tool_name_map.encode_function(
                item.namespace.as_deref(),
                item.name.as_deref().unwrap_or(""),
            );
            let input = item
                .arguments
                .as_ref()
                .map(JsonString::to_value)
                .unwrap_or_else(|| json!({}));
            (name, input)
        }
        ItemType::ToolSearchCall => {
            let name = tool_name_map.encode_tool_search();
            let input = item
                .arguments
                .as_ref()
                .map(JsonString::to_value)
                .unwrap_or_else(|| json!({}));
            (name, input)
        }
        ItemType::CustomToolCall => {
            let name = tool_name_map.encode_custom(item.name.as_deref().unwrap_or(""));
            let input = json!({ "input": item.input.clone().unwrap_or_default() });
            (name, input)
        }
        _ => return None,
    };
    if name.trim().is_empty() {
        return None;
    }
    Some(json!({
        "type": "tool_use",
        "id": item.call_id.as_deref().or(item.id.as_deref()).unwrap_or(""),
        "name": name,
        "input": input,
    }))
}

fn anthropic_role(role: Option<&str>, item_type: &ItemType) -> &'static str {
    match (role, item_type) {
        (Some("assistant"), _) | (None, ItemType::OutputText) => "assistant",
        _ => "user",
    }
}

fn anthropic_content_blocks(item: &ResponseItem) -> Vec<Value> {
    match &item.content {
        Some(ItemContent::Text(text)) => text_block(text).into_iter().collect(),
        Some(ItemContent::Parts(parts)) => {
            parts.iter().filter_map(content_part_to_anthropic).collect()
        }
        None => {
            if let Some(text) = &item.text {
                text_block(text).into_iter().collect()
            } else if let Some(image_url) = &item.image_url {
                image_block(image_url, item.detail.as_deref())
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            }
        }
    }
}

fn content_part_to_anthropic(part: &ContentPart) -> Option<Value> {
    match part.part_type.as_str() {
        "input_text" | "output_text" | "text" => text_block(part.text.as_deref().unwrap_or("")),
        "input_image" | "image_url" => image_block(
            part.image_url.as_deref().unwrap_or(""),
            part.detail.as_deref(),
        ),
        _ => None,
    }
}

fn text_block(text: &str) -> Option<Value> {
    if text.is_empty() {
        None
    } else {
        Some(json!({"type": "text", "text": text}))
    }
}

fn image_block(image_url: &str, _detail: Option<&str>) -> Option<Value> {
    let data_url = image_url.strip_prefix("data:")?;
    let (media_type, data) = data_url.split_once(";base64,")?;
    Some(json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": data,
        }
    }))
}

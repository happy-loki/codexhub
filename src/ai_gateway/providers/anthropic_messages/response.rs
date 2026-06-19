use serde_json::{Value, json};

use crate::ai_gateway::model::{
    ContentPart, InputTokensDetails, ItemContent, ItemType, JsonString, OutputTokensDetails,
    ResponseItem, ResponseObject, SummaryPart, Usage, generate_item_id, generate_response_id,
};
use crate::ai_gateway::tool_names::{ToolCallKind, ToolNameMap};
pub(super) fn convert_anthropic_response(
    response: &Value,
    request_model: &str,
    tool_name_map: &ToolNameMap,
) -> ResponseObject {
    let output = response
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| anthropic_content_to_response_item(item, tool_name_map))
                .collect()
        })
        .unwrap_or_default();
    let usage = response.get("usage").map(convert_usage_value);
    let status = match response.get("stop_reason").and_then(Value::as_str) {
        Some("max_tokens") => "incomplete",
        _ => "completed",
    };

    ResponseObject {
        id: response
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(generate_response_id),
        object_type: "response".to_string(),
        model: response
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(request_model)
            .to_string(),
        created_at: chrono_timestamp(),
        status: status.to_string(),
        output,
        usage,
        error: None,
    }
}

fn anthropic_content_to_response_item(
    item: &Value,
    tool_name_map: &ToolNameMap,
) -> Option<ResponseItem> {
    match item.get("type").and_then(Value::as_str)? {
        "text" => {
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                return None;
            }
            Some(ResponseItem {
                item_type: ItemType::Message,
                id: Some(generate_item_id()),
                role: Some("assistant".to_string()),
                content: Some(ItemContent::Parts(vec![ContentPart::output_text(text)])),
                text: None,
                name: None,
                namespace: None,
                call_id: None,
                arguments: None,
                input: None,
                output: None,
                status: Some("completed".to_string()),
                execution: None,
                tools: None,
                image_url: None,
                detail: None,
                action: None,
                summary: None,
                encrypted_content: None,
            })
        }
        "thinking" => {
            let text = item.get("thinking").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                return None;
            }
            Some(ResponseItem {
                item_type: ItemType::Reasoning,
                id: Some(generate_item_id()),
                role: None,
                content: None,
                text: None,
                name: None,
                namespace: None,
                call_id: None,
                arguments: None,
                input: None,
                output: None,
                status: Some("completed".to_string()),
                execution: None,
                tools: None,
                image_url: None,
                detail: None,
                action: None,
                summary: Some(vec![SummaryPart {
                    part_type: "summary_text".to_string(),
                    text: text.to_string(),
                }]),
                encrypted_content: item
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        }
        "redacted_thinking" => Some(ResponseItem {
            item_type: ItemType::Reasoning,
            id: Some(generate_item_id()),
            role: None,
            content: None,
            text: None,
            name: None,
            namespace: None,
            call_id: None,
            arguments: None,
            input: None,
            output: None,
            status: Some("completed".to_string()),
            execution: None,
            tools: None,
            image_url: None,
            detail: None,
            action: None,
            summary: None,
            encrypted_content: item
                .get("data")
                .or_else(|| item.get("signature"))
                .and_then(Value::as_str)
                .map(str::to_string),
        }),
        "tool_use" => {
            let raw_name = item.get("name").and_then(Value::as_str).unwrap_or("");
            let target = tool_name_map.decode(raw_name);
            let input = item.get("input").cloned().unwrap_or_else(|| json!({}));
            let (item_type, name, namespace, arguments, custom_input, execution) = match target.kind
            {
                ToolCallKind::ToolSearch => (
                    ItemType::ToolSearchCall,
                    None,
                    None,
                    Some(JsonString::Value(input)),
                    None,
                    Some("client".to_string()),
                ),
                ToolCallKind::Custom => (
                    ItemType::CustomToolCall,
                    Some(target.name),
                    None,
                    None,
                    Some(extract_custom_tool_input(&input)),
                    None,
                ),
                ToolCallKind::Function => (
                    ItemType::FunctionCall,
                    Some(target.name),
                    target.namespace,
                    Some(JsonString::String(
                        serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string()),
                    )),
                    None,
                    None,
                ),
            };
            Some(ResponseItem {
                item_type,
                id: Some(generate_item_id()),
                role: None,
                content: None,
                text: None,
                name,
                namespace,
                call_id: item.get("id").and_then(Value::as_str).map(str::to_string),
                arguments,
                input: custom_input,
                output: None,
                status: Some("completed".to_string()),
                execution,
                tools: None,
                image_url: None,
                detail: None,
                action: None,
                summary: None,
                encrypted_content: None,
            })
        }
        "server_tool_use" => {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            if name != "web_search" {
                return None;
            }
            Some(ResponseItem {
                item_type: ItemType::WebSearchCall,
                id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| Some(generate_item_id())),
                role: None,
                content: None,
                text: None,
                name: None,
                namespace: None,
                call_id: item.get("id").and_then(Value::as_str).map(str::to_string),
                arguments: None,
                input: None,
                output: None,
                status: Some("completed".to_string()),
                execution: None,
                tools: None,
                image_url: None,
                detail: None,
                action: Some(server_tool_action(item)),
                summary: None,
                encrypted_content: None,
            })
        }
        "web_search_tool_result" => Some(ResponseItem {
            item_type: ItemType::WebSearchCall,
            id: item
                .get("tool_use_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| Some(generate_item_id())),
            role: None,
            content: None,
            text: None,
            name: None,
            namespace: None,
            call_id: item
                .get("tool_use_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            arguments: None,
            input: None,
            output: None,
            status: Some(
                if item
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "failed"
                } else {
                    "completed"
                }
                .to_string(),
            ),
            execution: None,
            tools: None,
            image_url: None,
            detail: None,
            action: Some(json!({
                "type": "web_search_tool_result",
                "tool_use_id": item.get("tool_use_id").cloned().unwrap_or(Value::Null),
                "content": item.get("content").cloned().unwrap_or(Value::Null),
                "is_error": item.get("is_error").cloned().unwrap_or(Value::Bool(false)),
            })),
            summary: None,
            encrypted_content: None,
        }),
        _ => None,
    }
}

fn server_tool_action(item: &Value) -> Value {
    json!({
        "type": "web_search",
        "input": item.get("input").cloned().unwrap_or_else(|| json!({})),
    })
}

fn extract_custom_tool_input(input: &Value) -> String {
    input
        .get("input")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if input.is_string() {
                input.as_str().unwrap_or_default().to_string()
            } else {
                serde_json::to_string(input).unwrap_or_default()
            }
        })
}

fn convert_usage_value(usage: &Value) -> Usage {
    let uncached_input = usage
        .get("input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cache_creation = anthropic_cache_creation_input_tokens(usage);
    let input = uncached_input + cached + cache_creation;
    Usage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        input_tokens_details: Some(InputTokensDetails {
            cached_tokens: cached,
            cache_creation_tokens: cache_creation,
        }),
        output_tokens_details: Some(OutputTokensDetails {
            reasoning_tokens: 0,
        }),
    }
}

fn anthropic_cache_creation_input_tokens(usage: &Value) -> i64 {
    usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .or_else(|| {
            usage.get("cache_creation").and_then(|cache_creation| {
                cache_creation
                    .as_object()
                    .map(|fields| fields.values().filter_map(Value::as_i64).sum::<i64>())
            })
        })
        .unwrap_or(0)
}

fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

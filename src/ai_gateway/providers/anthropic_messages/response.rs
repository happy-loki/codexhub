use serde_json::{Value, json};

use crate::ai_gateway::model::{
    ContentPart, InputTokensDetails, ItemContent, ItemType, JsonString, OutputTokensDetails,
    ResponseItem, ResponseObject, SummaryPart, Usage, generate_item_id, generate_response_id,
};
use crate::ai_gateway::tool_names::{ToolCallKind, ToolNameMap};

use super::options::AnthropicProviderProfile;
use super::{citations, glm_compat};

pub(super) fn convert_anthropic_response(
    response: &Value,
    request_model: &str,
    tool_name_map: &ToolNameMap,
    profile: AnthropicProviderProfile,
) -> ResponseObject {
    let output = response
        .get("content")
        .and_then(Value::as_array)
        .map(|items| convert_anthropic_content(items, tool_name_map, profile))
        .unwrap_or_default();
    let usage = response.get("usage").map(convert_usage_value);
    let stop_reason = response.get("stop_reason").and_then(Value::as_str);
    let status = match stop_reason {
        Some("max_tokens") => "incomplete",
        _ => "completed",
    };
    let incomplete_details = incomplete_details_for_stop_reason(stop_reason);

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
        incomplete_details,
    }
}

/// Map an Anthropic `stop_reason` to a Responses `incomplete_details` object.
///
/// Codex reads `incomplete_details.reason` from `response.incomplete`; when it is
/// missing it falls back to `unknown`. Only `max_tokens` maps to an incomplete
/// response today, which Codex expects as `max_output_tokens`.
pub(super) fn incomplete_details_for_stop_reason(stop_reason: Option<&str>) -> Option<Value> {
    match stop_reason {
        Some("max_tokens") => Some(json!({ "reason": "max_output_tokens" })),
        _ => None,
    }
}

fn convert_anthropic_content(
    items: &[Value],
    tool_name_map: &ToolNameMap,
    profile: AnthropicProviderProfile,
) -> Vec<ResponseItem> {
    let mut output = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("web_search_tool_result") => {
                attach_web_search_result(&mut output, item, true);
                continue;
            }
            Some("tool_result") if matches!(profile, AnthropicProviderProfile::GlmAnthropic) => {
                if attach_web_search_result(&mut output, item, false) {
                    continue;
                }
            }
            _ => {}
        }
        if let Some(item) = anthropic_content_to_response_item(item, tool_name_map, profile) {
            output.push(item);
        }
    }
    output
}

fn anthropic_content_to_response_item(
    item: &Value,
    tool_name_map: &ToolNameMap,
    profile: AnthropicProviderProfile,
) -> Option<ResponseItem> {
    match item.get("type").and_then(Value::as_str)? {
        "text" => {
            let mut text = item
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if matches!(profile, AnthropicProviderProfile::GlmAnthropic) {
                text = glm_compat::clean_private_web_search_text(&text)?;
            }
            if text.is_empty() {
                return None;
            }
            let annotations = item
                .get("citations")
                .and_then(Value::as_array)
                .map(|items| citations::convert_anthropic_citations(items, 0, text.chars().count()))
                .unwrap_or_default();
            let mut content_part = ContentPart::output_text(text);
            content_part.annotations = Some(annotations);
            Some(ResponseItem {
                item_type: ItemType::Message,
                id: Some(generate_item_id()),
                role: Some("assistant".to_string()),
                content: Some(ItemContent::Parts(vec![content_part])),
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
            let encrypted_content = item
                .get("signature")
                .and_then(Value::as_str)
                .map(str::to_string);
            if text.is_empty() && encrypted_content.is_none() {
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
                summary: Some(if text.is_empty() {
                    Vec::new()
                } else {
                    vec![SummaryPart {
                        part_type: "summary_text".to_string(),
                        text: text.to_string(),
                    }]
                }),
                encrypted_content,
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
            if raw_name == "WebSearch" {
                return Some(web_search_response_item(item));
            }
            if is_unmapped_web_search_tool_use(raw_name, tool_name_map, profile) {
                return Some(web_search_response_item(item));
            }
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
            if !profile.is_web_search_server_tool(name) {
                return None;
            }
            if web_search_query(item).is_empty() {
                return None;
            }
            Some(web_search_response_item(item))
        }
        "web_search_tool_result" | "tool_result" => None,
        _ => None,
    }
}

fn is_unmapped_web_search_tool_use(
    raw_name: &str,
    tool_name_map: &ToolNameMap,
    profile: AnthropicProviderProfile,
) -> bool {
    profile.is_web_search_server_tool(raw_name) && !tool_name_map.has_encoded(raw_name)
}

fn web_search_response_item(item: &Value) -> ResponseItem {
    ResponseItem {
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
    }
}

fn attach_web_search_result(
    output: &mut Vec<ResponseItem>,
    item: &Value,
    allow_orphan: bool,
) -> bool {
    let tool_use_id = item.get("tool_use_id").and_then(Value::as_str);
    let failed = item
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(existing) = tool_use_id.and_then(|tool_use_id| {
        output
            .iter_mut()
            .find(|candidate| candidate.call_id.as_deref() == Some(tool_use_id))
    }) {
        existing.status = Some(if failed { "failed" } else { "completed" }.to_string());
        return true;
    }

    if !allow_orphan || tool_use_id.is_none() {
        return false;
    }

    output.push(ResponseItem {
        item_type: ItemType::WebSearchCall,
        id: tool_use_id
            .map(str::to_string)
            .or_else(|| Some(generate_item_id())),
        role: None,
        content: None,
        text: None,
        name: None,
        namespace: None,
        call_id: tool_use_id.map(str::to_string),
        arguments: None,
        input: None,
        output: None,
        status: Some(if failed { "failed" } else { "completed" }.to_string()),
        execution: None,
        tools: None,
        image_url: None,
        detail: None,
        action: Some(json!({
            "type": "search",
            "query": "",
        })),
        summary: None,
        encrypted_content: None,
    });
    true
}

fn server_tool_action(item: &Value) -> Value {
    let query = web_search_query(item);
    json!({
        "type": "search",
        "query": query,
        "queries": [query],
    })
}

fn web_search_query(item: &Value) -> &str {
    item.get("input")
        .and_then(|input| input.get("query").or_else(|| input.get("search_query")))
        .and_then(Value::as_str)
        .unwrap_or_default()
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
    let (cache_creation_5m, cache_creation_1h) = anthropic_cache_creation_split(usage);
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("thinking_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let input = uncached_input + cached + cache_creation;
    Usage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        input_tokens_details: Some(InputTokensDetails {
            cached_tokens: cached,
            cache_creation_tokens: cache_creation,
            cache_creation_5m_tokens: cache_creation_5m,
            cache_creation_1h_tokens: cache_creation_1h,
        }),
        output_tokens_details: Some(OutputTokensDetails { reasoning_tokens }),
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

/// Extracts Anthropic's `(5m, 1h)` cache-write TTL split when present.
fn anthropic_cache_creation_split(usage: &Value) -> (i64, i64) {
    let Some(cache_creation) = usage.get("cache_creation") else {
        return (0, 0);
    };
    let five = cache_creation
        .get("ephemeral_5m_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let one = cache_creation
        .get("ephemeral_1h_input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    (five, one)
}

fn chrono_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

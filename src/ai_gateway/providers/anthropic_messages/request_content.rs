use serde_json::{Value, json};

use crate::ai_gateway::encrypted_content::{AnthropicEncryptedContentKind, EncryptedContentScope};
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::{
    ContentPart, FunctionCallOutput, FunctionCallOutputContentItem, ItemContent, ItemType,
    JsonString, ResponseItem,
};
use crate::ai_gateway::tool_names::ToolNameMap;

pub(super) fn build_anthropic_messages(
    input: &[ResponseItem],
    tool_name_map: &mut ToolNameMap,
    encrypted_content_scope: Option<&EncryptedContentScope>,
) -> Result<Vec<Value>, GatewayError> {
    let mut messages = Vec::new();
    for (item_index, item) in input.iter().enumerate() {
        match item.item_type {
            ItemType::Message
            | ItemType::InputText
            | ItemType::InputImage
            | ItemType::OutputText => {
                let role = anthropic_role(item.role.as_deref(), &item.item_type);
                let content = anthropic_content_blocks(item);
                if !content.is_empty() {
                    let merge_mode = if role == "assistant" {
                        MergeMode::AssistantContent
                    } else {
                        MergeMode::None
                    };
                    push_anthropic_message(&mut messages, role, content, merge_mode);
                }
            }
            ItemType::Reasoning => {
                if let Some(block) = anthropic_reasoning_block(item, encrypted_content_scope) {
                    push_anthropic_message(
                        &mut messages,
                        "assistant",
                        vec![block],
                        MergeMode::AssistantContent,
                    );
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
                validate_tool_use_id(call_id)?;
                let content = match item.item_type {
                    ItemType::ToolSearchOutput => {
                        Value::String(tool_search_output_to_anthropic_content(item))
                    }
                    _ => item
                        .output
                        .as_ref()
                        .map(function_call_output_to_anthropic_content)
                        .unwrap_or_else(|| Value::String(String::new())),
                };
                push_anthropic_message(
                    &mut messages,
                    "user",
                    vec![tool_result_block(call_id, content)],
                    MergeMode::ToolResult,
                );
            }
            ItemType::FunctionCall | ItemType::ToolSearchCall | ItemType::CustomToolCall => {
                if let Some(block) = response_tool_call_to_anthropic(item, tool_name_map)? {
                    push_anthropic_message(
                        &mut messages,
                        "assistant",
                        vec![block],
                        MergeMode::ToolUse,
                    );
                }
            }
            // Responses web_search_call is a completed built-in/server tool event.
            // Replay it in Claude Code's client-tool shape, but always include
            // the matching tool_result immediately so Anthropic history stays
            // well-formed.
            ItemType::WebSearchCall => {
                if let Some((assistant_content, tool_results)) =
                    web_search_call_history_to_anthropic(item, item_index)
                {
                    push_anthropic_message(
                        &mut messages,
                        "assistant",
                        assistant_content,
                        MergeMode::AssistantContent,
                    );
                    push_anthropic_message(&mut messages, "user", tool_results, MergeMode::None);
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

#[derive(Clone, Copy)]
enum MergeMode {
    None,
    AssistantContent,
    ToolUse,
    ToolResult,
}

fn push_anthropic_message(
    messages: &mut Vec<Value>,
    role: &str,
    content: Vec<Value>,
    merge_mode: MergeMode,
) {
    if content.is_empty() {
        return;
    }
    if should_merge_with_last_message(messages, role, merge_mode)
        && let Some(last_content) = messages
            .last_mut()
            .and_then(|message| message.get_mut("content"))
            .and_then(Value::as_array_mut)
    {
        append_content_blocks(last_content, content, merge_mode);
        return;
    }
    messages.push(json!({
        "role": role,
        "content": content,
    }));
}

fn should_merge_with_last_message(messages: &[Value], role: &str, merge_mode: MergeMode) -> bool {
    let Some(last) = messages.last() else {
        return false;
    };
    if last.get("role").and_then(Value::as_str) != Some(role) {
        return false;
    }
    let Some(content) = last.get("content").and_then(Value::as_array) else {
        return false;
    };
    match merge_mode {
        MergeMode::None => false,
        MergeMode::AssistantContent => role == "assistant",
        MergeMode::ToolUse => role == "assistant",
        MergeMode::ToolResult => content
            .iter()
            .all(|block| block.get("type").and_then(Value::as_str) == Some("tool_result")),
    }
}

fn anthropic_reasoning_block(
    item: &ResponseItem,
    encrypted_content_scope: Option<&EncryptedContentScope>,
) -> Option<Value> {
    let scope = encrypted_content_scope?;
    let encrypted_content = item.encrypted_content.as_deref()?;
    let (kind, raw_content) = scope.decode_anthropic(encrypted_content)?;
    if raw_content.is_empty() {
        return None;
    }

    match kind {
        AnthropicEncryptedContentKind::Thinking => {
            let thinking = item
                .summary
                .as_deref()?
                .iter()
                .map(|part| part.text.as_str())
                .collect::<String>();
            Some(json!({
                "type": "thinking",
                "thinking": thinking,
                "signature": raw_content,
            }))
        }
        AnthropicEncryptedContentKind::RedactedThinking => Some(json!({
            "type": "redacted_thinking",
            "data": raw_content,
        })),
    }
}

fn append_content_blocks(target: &mut Vec<Value>, content: Vec<Value>, merge_mode: MergeMode) {
    match merge_mode {
        MergeMode::ToolResult => {
            for block in content {
                append_tool_result_block(target, block);
            }
        }
        _ => target.extend(content),
    }
}

fn append_tool_result_block(target: &mut Vec<Value>, mut block: Value) {
    let Some(tool_use_id) = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        target.push(block);
        return;
    };
    let Some(existing) = target.iter_mut().find(|existing| {
        existing.get("type").and_then(Value::as_str) == Some("tool_result")
            && existing.get("tool_use_id").and_then(Value::as_str) == Some(tool_use_id.as_str())
    }) else {
        target.push(block);
        return;
    };
    let next_content = block.get_mut("content").map(Value::take);
    if let Some(next_content) = next_content {
        merge_tool_result_content(existing, next_content);
    }
}

fn tool_result_block(tool_use_id: &str, content: Value) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": tool_use_id,
        "content": content,
    })
}

fn merge_tool_result_content(existing: &mut Value, next: Value) {
    let current = existing.get_mut("content").map(Value::take);
    existing["content"] = match current {
        Some(current) => combine_tool_result_content(current, next),
        None => next,
    };
}

fn combine_tool_result_content(current: Value, next: Value) -> Value {
    match (current, next) {
        (Value::String(current), Value::String(next)) => {
            if current.is_empty() {
                Value::String(next)
            } else if next.is_empty() {
                Value::String(current)
            } else {
                Value::String(format!("{current}\n\n{next}"))
            }
        }
        (Value::Array(mut current), Value::Array(next)) => {
            current.extend(next);
            Value::Array(current)
        }
        (Value::Array(mut current), Value::String(next)) => {
            if !next.is_empty() {
                current.push(json!({"type": "text", "text": next}));
            }
            Value::Array(current)
        }
        (Value::String(current), Value::Array(mut next)) => {
            if current.is_empty() {
                Value::Array(next)
            } else {
                let mut content = vec![json!({"type": "text", "text": current})];
                content.append(&mut next);
                Value::Array(content)
            }
        }
        (current, next) => Value::String(format!("{current}\n\n{next}")),
    }
}

fn web_search_call_history_to_anthropic(
    item: &ResponseItem,
    item_index: usize,
) -> Option<(Vec<Value>, Vec<Value>)> {
    let queries = web_search_history_queries(item);
    if queries.is_empty() {
        return None;
    }

    let mut assistant_content = Vec::new();
    let mut tool_results = Vec::new();
    for (index, query) in queries.iter().enumerate() {
        let tool_use_id = web_search_history_tool_use_id(item, query, item_index, index);
        assistant_content.push(json!({
            "type": "tool_use",
            "id": tool_use_id,
            "name": "WebSearch",
            "input": { "query": query },
        }));
        tool_results.push(tool_result_block(
            &tool_use_id,
            Value::String(web_search_history_tool_result_content(item, query)),
        ));
    }

    Some((assistant_content, tool_results))
}

fn web_search_history_queries(item: &ResponseItem) -> Vec<String> {
    let Some(action) = item.action.as_ref() else {
        return Vec::new();
    };

    let mut queries = Vec::new();
    for key in ["query", "search_query"] {
        if let Some(query) = action.get(key).and_then(Value::as_str) {
            push_unique_non_empty(&mut queries, query);
        }
    }
    if let Some(values) = action.get("queries").and_then(Value::as_array) {
        for value in values {
            if let Some(query) = value.as_str() {
                push_unique_non_empty(&mut queries, query);
            }
        }
    }
    queries
}

fn web_search_history_tool_use_id(
    item: &ResponseItem,
    query: &str,
    item_index: usize,
    query_index: usize,
) -> String {
    let base = item.call_id.as_deref().or(item.id.as_deref()).unwrap_or("");
    if base.starts_with("tooluse_") && is_valid_tool_use_id(base) {
        return if query_index == 0 {
            base.to_string()
        } else {
            format!("{base}_{query_index}")
        };
    }

    format!(
        "tooluse_ws_{item_index}_{query_index}_{:016x}",
        stable_web_search_history_hash(query)
    )
}

fn stable_web_search_history_hash(input: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn web_search_history_tool_result_content(item: &ResponseItem, query: &str) -> String {
    let status = item.status.as_deref().unwrap_or("completed");
    let mut lines = vec![
        format!("Web search history item status: {status}."),
        format!("Query: {query}"),
    ];
    if let Some(result) = item.action.as_ref().and_then(|action| action.get("result")) {
        lines.push(format!(
            "Result: {}",
            serde_json::to_string(result).unwrap_or_else(|_| result.to_string())
        ));
    } else {
        lines.push(
            "Detailed search result blocks were not included in the Responses web_search_call item."
                .to_string(),
        );
    }
    lines.join("\n")
}

fn push_unique_non_empty(values: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() || values.iter().any(|existing| existing == value) {
        return;
    }
    values.push(value.to_string());
}

fn tool_search_output_to_anthropic_content(item: &ResponseItem) -> String {
    serde_json::to_string(&json!({
        "status": item.status.as_deref().unwrap_or("completed"),
        "execution": item.execution.as_deref().unwrap_or("client"),
        "tools": item.tools.clone().unwrap_or_default(),
    }))
    .unwrap_or_else(|_| "{\"tools\":[]}".to_string())
}

fn function_call_output_to_anthropic_content(output: &FunctionCallOutput) -> Value {
    match output {
        FunctionCallOutput::Text(text) => Value::String(text.clone()),
        FunctionCallOutput::ContentItems(items) if tool_output_items_are_text_only(items) => {
            Value::String(output.to_chat_tool_content())
        }
        FunctionCallOutput::ContentItems(items) => {
            let blocks = items
                .iter()
                .filter_map(tool_output_content_item_to_anthropic)
                .collect::<Vec<_>>();
            if blocks.is_empty() {
                Value::String(output.to_chat_tool_content())
            } else {
                Value::Array(blocks)
            }
        }
    }
}

fn tool_output_items_are_text_only(items: &[FunctionCallOutputContentItem]) -> bool {
    items.iter().all(|item| {
        matches!(
            item.item_type.as_str(),
            "input_text" | "output_text" | "text"
        )
    })
}

fn tool_output_content_item_to_anthropic(item: &FunctionCallOutputContentItem) -> Option<Value> {
    match item.item_type.as_str() {
        "input_text" | "output_text" | "text" => text_block(item.text.as_deref().unwrap_or("")),
        "input_image" | "image_url" => image_block(item.image_url.as_deref().unwrap_or(""), None),
        _ => None,
    }
}

fn response_tool_call_to_anthropic(
    item: &ResponseItem,
    tool_name_map: &mut ToolNameMap,
) -> Result<Option<Value>, GatewayError> {
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
        _ => return Ok(None),
    };
    if name.trim().is_empty() {
        return Ok(None);
    }
    let id = item.call_id.as_deref().or(item.id.as_deref()).unwrap_or("");
    if id.is_empty() {
        return Err(GatewayError::bad_request(
            "anthropic_messages requires tool call_id",
        ));
    }
    validate_tool_use_id(id)?;
    Ok(Some(json!({
        "type": "tool_use",
        "id": id,
        "name": name,
        "input": input,
    })))
}

fn validate_tool_use_id(id: &str) -> Result<(), GatewayError> {
    if is_valid_tool_use_id(id) {
        return Ok(());
    }
    Err(GatewayError::bad_request(
        "anthropic_messages tool_use id must match ^[a-zA-Z0-9_-]+$",
    ))
}

fn is_valid_tool_use_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
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

use serde_json::{Value, json};

use crate::ai_gateway::tool_names::{ToolCallKind, ToolCallTarget};

use super::stream_tools::AnthropicContentBlockState;
pub(super) struct ToolDeltaEvent {
    pub(super) event_type: &'static str,
    pub(super) item_id: String,
    pub(super) output_index: usize,
    pub(super) delta: String,
}

pub(super) fn tool_delta_event(
    state: &mut AnthropicContentBlockState,
    raw_delta: &str,
) -> Option<ToolDeltaEvent> {
    match state.target.kind {
        ToolCallKind::Custom => {
            let full_input = match partial_custom_tool_input(&state.arguments) {
                Some(input) => input,
                None if !state.arguments.trim_start().starts_with('{') => state.arguments.clone(),
                None => return None,
            };
            let delta = full_input
                .strip_prefix(&state.custom_emitted_input)
                .unwrap_or(&full_input)
                .to_string();
            if delta.is_empty() {
                return None;
            }
            state.custom_emitted_input = full_input;
            Some(ToolDeltaEvent {
                event_type: "response.custom_tool_call_input.delta",
                item_id: state.item_id.clone(),
                output_index: state.output_index,
                delta,
            })
        }
        ToolCallKind::Function => Some(ToolDeltaEvent {
            event_type: "response.function_call_arguments.delta",
            item_id: state.item_id.clone(),
            output_index: state.output_index,
            delta: raw_delta.to_string(),
        }),
        ToolCallKind::ToolSearch => None,
    }
}

pub(super) fn in_progress_tool_item(
    item_id: &str,
    call_id: &str,
    target: &ToolCallTarget,
) -> Value {
    match target.kind {
        ToolCallKind::ToolSearch => json!({
            "type": "tool_search_call",
            "id": item_id,
            "call_id": call_id,
            "execution": "client",
            "arguments": {},
            "status": "in_progress",
        }),
        ToolCallKind::Custom => json!({
            "type": "custom_tool_call",
            "id": item_id,
            "call_id": call_id,
            "name": target.name,
            "input": "",
        }),
        ToolCallKind::Function => {
            let mut item = json!({
                "type": "function_call",
                "id": item_id,
                "call_id": call_id,
                "name": target.name,
                "arguments": "",
                "status": "in_progress",
            });
            if let Some(namespace) = &target.namespace {
                item["namespace"] = json!(namespace);
            }
            item
        }
    }
}

pub(super) fn completed_tool_item(
    item_id: &str,
    call_id: &str,
    target: &ToolCallTarget,
    arguments: &str,
) -> Value {
    match target.kind {
        ToolCallKind::ToolSearch => json!({
            "type": "tool_search_call",
            "id": item_id,
            "call_id": call_id,
            "execution": "client",
            "arguments": serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({})),
            "status": "completed",
        }),
        ToolCallKind::Custom => json!({
            "type": "custom_tool_call",
            "id": item_id,
            "call_id": call_id,
            "name": target.name,
            "input": parse_custom_tool_input(arguments).unwrap_or_else(|| arguments.to_string()),
        }),
        ToolCallKind::Function => {
            let mut item = json!({
                "type": "function_call",
                "id": item_id,
                "call_id": call_id,
                "name": target.name,
                "arguments": arguments,
                "status": "completed",
            });
            if let Some(namespace) = &target.namespace {
                item["namespace"] = json!(namespace);
            }
            item
        }
    }
}

pub(super) fn web_search_item(
    item_id: &str,
    call_id: &str,
    status: &str,
    input: Value,
    result: Option<Value>,
) -> Value {
    let mut action = json!({
        "type": "web_search",
        "input": input,
    });
    if let Some(result) = result {
        action["result"] = result;
    }
    json!({
        "type": "web_search_call",
        "id": item_id,
        "call_id": call_id,
        "status": status,
        "action": action,
    })
}

fn partial_custom_tool_input(arguments: &str) -> Option<String> {
    parse_custom_tool_input(arguments).or_else(|| partial_wrapped_input_prefix(arguments))
}

fn parse_custom_tool_input(arguments: &str) -> Option<String> {
    serde_json::from_str::<Value>(arguments)
        .ok()?
        .get("input")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn partial_wrapped_input_prefix(arguments: &str) -> Option<String> {
    let mut rest = arguments.trim_start();
    rest = rest.strip_prefix('{')?.trim_start();
    let (key, after_key) = parse_json_string_prefix(rest)?;
    if key != "input" {
        return None;
    }
    rest = after_key.trim_start();
    rest = rest.strip_prefix(':')?.trim_start();
    parse_json_string_prefix(rest).map(|(value, _)| value)
}

fn parse_json_string_prefix(input: &str) -> Option<(String, &str)> {
    if !input.starts_with('"') {
        return None;
    }

    let mut output = String::new();
    let mut pos = 1;
    while pos < input.len() {
        let ch = input[pos..].chars().next()?;
        match ch {
            '"' => {
                let next = pos + ch.len_utf8();
                return Some((output, &input[next..]));
            }
            '\\' => {
                pos += ch.len_utf8();
                let escaped = input[pos..].chars().next()?;
                match escaped {
                    '"' => output.push('"'),
                    '\\' => output.push('\\'),
                    '/' => output.push('/'),
                    'b' => output.push('\u{0008}'),
                    'f' => output.push('\u{000c}'),
                    'n' => output.push('\n'),
                    'r' => output.push('\r'),
                    't' => output.push('\t'),
                    'u' => {
                        let after_u = pos + escaped.len_utf8();
                        let decoded = decode_json_unicode_escape(input, after_u)?;
                        output.push(decoded.0);
                        pos = decoded.1;
                        continue;
                    }
                    _ => output.push(escaped),
                }
                pos += escaped.len_utf8();
            }
            _ => {
                output.push(ch);
                pos += ch.len_utf8();
            }
        }
    }

    Some((output, ""))
}

fn decode_json_unicode_escape(input: &str, offset: usize) -> Option<(char, usize)> {
    let first = read_hex_u16(input, offset)?;
    let first_end = offset + 4;
    if (0xD800..=0xDBFF).contains(&first) {
        let low_offset = first_end + 2;
        if input.get(first_end..low_offset) != Some("\\u") {
            return None;
        }
        let second = read_hex_u16(input, low_offset)?;
        if !(0xDC00..=0xDFFF).contains(&second) {
            return None;
        }
        let codepoint = 0x10000 + (((first as u32 - 0xD800) << 10) | (second as u32 - 0xDC00));
        char::from_u32(codepoint).map(|ch| (ch, low_offset + 4))
    } else {
        char::from_u32(first as u32).map(|ch| (ch, first_end))
    }
}

fn read_hex_u16(input: &str, offset: usize) -> Option<u16> {
    let hex = input.get(offset..offset + 4)?;
    u16::from_str_radix(hex, 16).ok()
}

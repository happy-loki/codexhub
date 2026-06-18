//! Chat Completions response → Responses API response 转换。
//! 参考 AxonHub `responses/outbound_convert.go` 的 `convertToResponsesAPIResponse`。

use serde_json::Value;

use crate::ai_gateway::model::{
    ContentPart, InputTokensDetails, ItemContent, ItemType, OutputTokensDetails, ResponseItem,
    ResponseObject, SummaryPart, Usage, generate_item_id, generate_response_id,
};

/// 将 Chat Completions 非流式响应转为 Responses API ResponseObject。
pub fn convert_chat_response(
    chat_resp: &Value,
    request_model: &str,
) -> Result<ResponseObject, String> {
    let resp_id = generate_response_id();
    let model = chat_resp
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or(request_model);

    let mut output = Vec::new();

    let choice = chat_resp
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .ok_or("no choices in chat response")?;

    let message = choice.get("message").ok_or("no message in choice")?;

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("stop");

    // 1. reasoning_content → reasoning item
    if let Some(reasoning_content) = message.get("reasoning_content").and_then(|v| v.as_str()) {
        if !reasoning_content.is_empty() {
            output.push(ResponseItem {
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
                status: Some("completed".into()),
                image_url: None,
                detail: None,
                action: None,
                summary: Some(vec![SummaryPart {
                    part_type: "summary_text".into(),
                    text: reasoning_content.to_string(),
                }]),
                encrypted_content: message
                    .get("reasoning_signature")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });
        }
    }

    // 2. content → message item (role=assistant)
    if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
        if !content.is_empty() {
            output.push(ResponseItem {
                item_type: ItemType::Message,
                id: Some(generate_item_id()),
                role: Some("assistant".into()),
                content: Some(ItemContent::Parts(vec![ContentPart::output_text(content)])),
                text: None,
                name: None,
                namespace: None,
                call_id: None,
                arguments: None,
                input: None,
                output: None,
                status: Some("completed".into()),
                image_url: None,
                detail: None,
                action: None,
                summary: None,
                encrypted_content: None,
            });
        }
    }

    // 3. tool_calls → function_call items
    if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tool_calls {
            let func = tc.get("function").unwrap_or(&Value::Null);
            let raw_name = func.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let (namespace, name) = decode_responses_function_call_name(raw_name);
            output.push(ResponseItem {
                item_type: ItemType::FunctionCall,
                id: Some(generate_item_id()),
                role: None,
                content: None,
                text: None,
                name: if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                },
                namespace: namespace.map(str::to_string),
                call_id: tc.get("id").and_then(|v| v.as_str()).map(|s| s.into()),
                arguments: func
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .map(|s| s.into()),
                input: None,
                output: None,
                status: Some("completed".into()),
                image_url: None,
                detail: None,
                action: None,
                summary: None,
                encrypted_content: None,
            });
        }
    }

    // 4. usage
    let usage = convert_usage(chat_resp.get("usage"));

    // 5. status
    let status = match finish_reason {
        "length" => "incomplete",
        _ => "completed",
    };

    Ok(ResponseObject {
        id: resp_id,
        object_type: "response".into(),
        model: model.to_string(),
        created_at: chat_resp
            .get("created")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            }),
        status: status.into(),
        output,
        usage,
        error: None,
    })
}

const RESPONSES_MCP_NAMESPACE_UNIT_MARKER: &str = "responses_unit__";

fn decode_responses_function_call_name(name: &str) -> (Option<&str>, &str) {
    let Some(idx) = name.find(RESPONSES_MCP_NAMESPACE_UNIT_MARKER) else {
        return (None, name);
    };

    let namespace = &name[..idx];
    let tool_name = &name[idx + RESPONSES_MCP_NAMESPACE_UNIT_MARKER.len()..];
    if namespace.is_empty() || tool_name.is_empty() || !namespace.starts_with("mcp__") {
        return (None, name);
    }

    (Some(namespace), tool_name)
}

fn convert_usage(usage_val: Option<&Value>) -> Option<Usage> {
    let u = usage_val?;
    u.as_object()?;
    if !has_usage_token_fields(u) {
        return None;
    }

    let input_tokens = first_i64(u, &["prompt_tokens"]).unwrap_or(0);
    let output_tokens = first_i64(u, &["completion_tokens"]).unwrap_or(0);
    let total_tokens = first_i64(u, &["total_tokens"]).unwrap_or(input_tokens + output_tokens);

    let cached = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_i64())
        .or_else(|| first_i64(u, &["cached_tokens", "prompt_cache_hit_tokens"]))
        .unwrap_or(0);
    let reasoning = u
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Some(Usage {
        input_tokens,
        output_tokens,
        total_tokens,
        input_tokens_details: Some(InputTokensDetails {
            cached_tokens: cached,
        }),
        output_tokens_details: Some(OutputTokensDetails {
            reasoning_tokens: reasoning,
        }),
    })
}

fn has_usage_token_fields(usage: &Value) -> bool {
    first_i64(
        usage,
        &["prompt_tokens", "completion_tokens", "total_tokens"],
    )
    .is_some()
}

fn first_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_i64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_simple_content_response() {
        let chat_resp = json!({
            "id": "chatcmpl-123",
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello world!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        assert!(resp.id.starts_with("gwresp_"));
        assert_eq!(resp.status, "completed");
        assert_eq!(resp.output.len(), 1);
        assert_eq!(resp.output[0].item_type, ItemType::Message);
        assert_eq!(resp.output[0].role.as_deref(), Some("assistant"));
        assert!(resp.usage.is_some());
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    #[test]
    fn test_reasoning_content_response() {
        let chat_resp = json!({
            "model": "deepseek-v4-pro",
            "created": 1700000000,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "The answer is 42.",
                    "reasoning_content": "Let me think step by step..."
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-pro").unwrap();
        assert_eq!(resp.output.len(), 2);
        // 第一个是 reasoning
        assert_eq!(resp.output[0].item_type, ItemType::Reasoning);
        let summary = resp.output[0].summary.as_ref().unwrap();
        assert_eq!(summary[0].text, "Let me think step by step...");
        // 第二个是 message
        assert_eq!(resp.output[1].item_type, ItemType::Message);
    }

    #[test]
    fn test_tool_calls_response() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"NYC\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18}
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        assert_eq!(resp.status, "completed"); // tool_calls → completed
        assert_eq!(resp.output.len(), 1);
        assert_eq!(resp.output[0].item_type, ItemType::FunctionCall);
        assert_eq!(resp.output[0].name.as_deref(), Some("get_weather"));
        assert_eq!(resp.output[0].call_id.as_deref(), Some("call_abc"));
    }

    #[test]
    fn test_namespaced_tool_call_response_restores_responses_namespace() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "mcp__arthas__responses_unit__excelPeek",
                            "arguments": "{\"path\":\"a.xlsx\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();

        assert_eq!(resp.output.len(), 1);
        assert_eq!(resp.output[0].item_type, ItemType::FunctionCall);
        assert_eq!(resp.output[0].namespace.as_deref(), Some("mcp__arthas__"));
        assert_eq!(resp.output[0].name.as_deref(), Some("excelPeek"));
    }

    #[test]
    fn test_length_finish_reason_incomplete() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {"role": "assistant", "content": "truncated..."},
                "finish_reason": "length"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 4096, "total_tokens": 4106}
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        assert_eq!(resp.status, "incomplete");
    }

    #[test]
    fn test_no_choices_errors() {
        let chat_resp = json!({"model": "x", "choices": []});
        assert!(convert_chat_response(&chat_resp, "x").is_err());
    }

    #[test]
    fn test_usage_with_details() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {"role": "assistant", "content": "ok"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "total_tokens": 150,
                "prompt_tokens_details": {"cached_tokens": 80},
                "completion_tokens_details": {"reasoning_tokens": 30}
            }
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens_details.unwrap().cached_tokens, 80);
        assert_eq!(usage.output_tokens_details.unwrap().reasoning_tokens, 30);
    }

    // ─── 多个 tool_calls 响应 ──────────────────────────────────

    #[test]
    fn test_multiple_tool_calls_response() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "get_weather", "arguments": "{\"city\":\"NYC\"}"}
                        },
                        {
                            "id": "call_2",
                            "type": "function",
                            "function": {"name": "get_time", "arguments": "{\"tz\":\"EST\"}"}
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 15, "total_tokens": 35}
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        assert_eq!(resp.status, "completed");
        // 应该有 2 个 function_call items，没有 message item（content 是 null）
        assert_eq!(resp.output.len(), 2);
        assert_eq!(resp.output[0].item_type, ItemType::FunctionCall);
        assert_eq!(resp.output[0].name.as_deref(), Some("get_weather"));
        assert_eq!(resp.output[0].call_id.as_deref(), Some("call_1"));
        assert_eq!(resp.output[1].item_type, ItemType::FunctionCall);
        assert_eq!(resp.output[1].name.as_deref(), Some("get_time"));
        assert_eq!(resp.output[1].call_id.as_deref(), Some("call_2"));
    }

    // ─── reasoning + tool_calls 同时存在 ──────────────────────

    #[test]
    fn test_reasoning_plus_tool_calls_response() {
        let chat_resp = json!({
            "model": "deepseek-v4-pro",
            "created": 1700000000,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "I need to search for this.",
                    "tool_calls": [{
                        "id": "call_s",
                        "type": "function",
                        "function": {"name": "search", "arguments": "{\"q\":\"rust\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-pro").unwrap();
        // reasoning item + function_call item = 2
        assert_eq!(resp.output.len(), 2);
        assert_eq!(resp.output[0].item_type, ItemType::Reasoning);
        assert_eq!(
            resp.output[0].summary.as_ref().unwrap()[0].text,
            "I need to search for this."
        );
        assert_eq!(resp.output[1].item_type, ItemType::FunctionCall);
        assert_eq!(resp.output[1].name.as_deref(), Some("search"));
    }

    // ─── 空 reasoning_content 被跳过 ──────────────────────────

    #[test]
    fn test_empty_reasoning_content_skipped() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "hello",
                    "reasoning_content": ""
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 1, "total_tokens": 6}
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        // 空 reasoning_content 不应生成 reasoning item
        assert_eq!(resp.output.len(), 1);
        assert_eq!(resp.output[0].item_type, ItemType::Message);
    }

    // ─── content 为空不生成 message item ───────────────────────

    #[test]
    fn test_empty_content_no_message_item() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "foo", "arguments": "{}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 5, "total_tokens": 10}
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        // 只有 function_call，没有 message（content 为空）
        assert_eq!(resp.output.len(), 1);
        assert_eq!(resp.output[0].item_type, ItemType::FunctionCall);
    }

    // ─── 无 usage 字段 ────────────────────────────────────────

    #[test]
    fn test_no_usage_field() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }]
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_null_usage_is_ignored() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }],
            "usage": null
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_empty_usage_object_is_ignored() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }],
            "usage": {}
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        assert!(resp.usage.is_none());
    }

    #[test]
    fn test_deepseek_prompt_cache_hit_tokens_are_mapped() {
        let chat_resp = json!({
            "model": "deepseek-v4-pro",
            "created": 1700000000,
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 16,
                "completion_tokens": 645,
                "total_tokens": 661,
                "prompt_cache_hit_tokens": 4
            }
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-pro").unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens_details.unwrap().cached_tokens, 4);
    }
}

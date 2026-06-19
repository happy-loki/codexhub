//! Chat Completions response → Responses API response 转换。
//! 参考 AxonHub `responses/outbound_convert.go` 的 `convertToResponsesAPIResponse`。

use serde_json::{Value, json};

use crate::ai_gateway::model::{
    ContentPart, InputTokensDetails, ItemContent, ItemType, JsonString, OutputTokensDetails,
    ResponseItem, ResponseObject, SummaryPart, Usage, generate_item_id, generate_response_id,
};
use crate::ai_gateway::tool_names::{ToolCallKind, ToolNameMap};

/// 将 Chat Completions 非流式响应转为 Responses API ResponseObject。
#[cfg(test)]
pub fn convert_chat_response(
    chat_resp: &Value,
    request_model: &str,
) -> Result<ResponseObject, String> {
    convert_chat_response_with_tool_names(chat_resp, request_model, &ToolNameMap::default())
}

pub fn convert_chat_response_with_tool_names(
    chat_resp: &Value,
    request_model: &str,
    tool_name_map: &ToolNameMap,
) -> Result<ResponseObject, String> {
    let resp_id = chat_resp
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(generate_response_id);
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
                execution: None,
                tools: None,
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
                execution: None,
                tools: None,
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
            let target = tool_name_map.decode(raw_name);
            let arguments_text = func
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let (item_type, name, namespace, arguments, input, execution) = match target.kind {
                ToolCallKind::ToolSearch => (
                    ItemType::ToolSearchCall,
                    None,
                    None,
                    Some(JsonString::Value(parse_json_or_empty_object(
                        arguments_text,
                    ))),
                    None,
                    Some("client".to_string()),
                ),
                ToolCallKind::Custom => (
                    ItemType::CustomToolCall,
                    Some(target.name),
                    None,
                    None,
                    Some(extract_custom_tool_input(arguments_text)),
                    None,
                ),
                ToolCallKind::Function => (
                    ItemType::FunctionCall,
                    Some(target.name),
                    target.namespace,
                    Some(JsonString::String(arguments_text.to_string())),
                    None,
                    None,
                ),
            };
            output.push(ResponseItem {
                item_type,
                id: Some(generate_item_id()),
                role: None,
                content: None,
                text: None,
                name,
                namespace,
                call_id: tc.get("id").and_then(|v| v.as_str()).map(|s| s.into()),
                arguments,
                input,
                output: None,
                status: Some("completed".into()),
                execution,
                tools: None,
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

fn parse_json_or_empty_object(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!({}))
}

fn extract_custom_tool_input(arguments_text: &str) -> String {
    serde_json::from_str::<Value>(arguments_text)
        .ok()
        .and_then(|value| {
            value
                .get("input")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| arguments_text.to_string())
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
            cache_creation_tokens: 0,
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
    use crate::ai_gateway::tool_names::{ToolCallTarget, ToolNameMap};
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
        assert_eq!(resp.id, "chatcmpl-123");
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
    fn test_missing_chat_response_id_generates_response_id() {
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello world!"
                },
                "finish_reason": "stop"
            }]
        });

        let resp = convert_chat_response(&chat_resp, "deepseek-v4-flash").unwrap();
        assert!(resp.id.starts_with("gwresp_"));
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
    fn test_codex_app_namespace_restored_from_tool_name_map() {
        let mut tool_name_map = ToolNameMap::default();
        tool_name_map.insert(
            "codex_app__codexns__read_thread_terminal",
            ToolCallTarget::function(Some("codex_app"), "read_thread_terminal"),
        );
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_app",
                        "type": "function",
                        "function": {
                            "name": "codex_app__codexns__read_thread_terminal",
                            "arguments": "{\"limit\":20}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let resp =
            convert_chat_response_with_tool_names(&chat_resp, "deepseek-v4-flash", &tool_name_map)
                .unwrap();

        assert_eq!(resp.output[0].item_type, ItemType::FunctionCall);
        assert_eq!(resp.output[0].namespace.as_deref(), Some("codex_app"));
        assert_eq!(resp.output[0].name.as_deref(), Some("read_thread_terminal"));
    }

    #[test]
    fn test_tool_search_tool_call_response_restores_tool_search_call() {
        let mut tool_name_map = ToolNameMap::default();
        tool_name_map.insert("tool_search", ToolCallTarget::tool_search());
        let chat_resp = json!({
            "model": "deepseek-v4-flash",
            "created": 1700000000,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "search_1",
                        "type": "function",
                        "function": {
                            "name": "tool_search",
                            "arguments": "{\"query\":\"chrome browser\",\"limit\":2}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let resp =
            convert_chat_response_with_tool_names(&chat_resp, "deepseek-v4-flash", &tool_name_map)
                .unwrap();

        assert_eq!(resp.output[0].item_type, ItemType::ToolSearchCall);
        assert_eq!(resp.output[0].call_id.as_deref(), Some("search_1"));
        assert_eq!(resp.output[0].execution.as_deref(), Some("client"));
        assert_eq!(
            resp.output[0].arguments.as_ref().unwrap().to_value(),
            json!({"query": "chrome browser", "limit": 2})
        );
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

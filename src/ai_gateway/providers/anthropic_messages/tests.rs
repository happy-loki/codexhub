use super::options::AnthropicProviderProfile;
use super::request::build_anthropic_request;
use super::response::convert_anthropic_response;
use super::stream::AnthropicSseToResponsesSse;
use super::types::ANTHROPIC_WEB_SEARCH_TYPE;
use crate::ai_gateway::model::{
    ContentPart, FunctionCallOutput, FunctionCallOutputContentItem, GatewayRequest, ItemContent,
    ItemType, Reasoning, ResponseItem,
};
use crate::ai_gateway::tool_names::ToolNameMap;
use axum::body::Bytes;
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};

fn convert_response(response: &Value) -> crate::ai_gateway::model::ResponseObject {
    convert_anthropic_response(
        response,
        "fallback-model",
        &ToolNameMap::default(),
        AnthropicProviderProfile::Anthropic,
    )
}

fn convert_glm_response(response: &Value) -> crate::ai_gateway::model::ResponseObject {
    convert_anthropic_response(
        response,
        "fallback-model",
        &ToolNameMap::default(),
        AnthropicProviderProfile::GlmAnthropic,
    )
}

fn response_stream<S>(input: S, model: &str, map: ToolNameMap) -> AnthropicSseToResponsesSse<S> {
    AnthropicSseToResponsesSse::new(
        input,
        model.to_string(),
        map,
        AnthropicProviderProfile::Anthropic,
    )
}

fn glm_response_stream<S>(
    input: S,
    model: &str,
    map: ToolNameMap,
) -> AnthropicSseToResponsesSse<S> {
    AnthropicSseToResponsesSse::new(
        input,
        model.to_string(),
        map,
        AnthropicProviderProfile::GlmAnthropic,
    )
}

fn request(input: Vec<ResponseItem>) -> GatewayRequest {
    GatewayRequest {
        model: "claude-sonnet-4-6".to_string(),
        instructions: Some("Be precise.".to_string()),
        input,
        tools: Vec::new(),
        tool_choice: None,
        reasoning: None,
        text: None,
        stream: false,
        max_output_tokens: Some(1234),
        temperature: Some(0.2),
        top_p: None,
        prompt_cache_key: None,
        prompt_cache_retention: None,
        previous_response_id: None,
    }
}

fn message(role: &str, text: &str) -> ResponseItem {
    ResponseItem {
        item_type: ItemType::Message,
        id: None,
        role: Some(role.to_string()),
        content: Some(ItemContent::Parts(vec![ContentPart {
            part_type: if role == "assistant" {
                "output_text".to_string()
            } else {
                "input_text".to_string()
            },
            text: Some(text.to_string()),
            image_url: None,
            detail: None,
            annotations: None,
        }])),
        text: None,
        name: None,
        namespace: None,
        call_id: None,
        arguments: None,
        input: None,
        output: None,
        status: None,
        execution: None,
        tools: None,
        image_url: None,
        detail: None,
        action: None,
        summary: None,
        encrypted_content: None,
    }
}

fn parse_events_from_bytes(chunks: &[Bytes]) -> Vec<(String, Value)> {
    let mut events = Vec::new();
    for chunk in chunks {
        let text = String::from_utf8_lossy(chunk);
        let mut event_type = String::new();
        let mut data = String::new();
        for line in text.lines() {
            if let Some(event) = line.strip_prefix("event: ") {
                event_type = event.to_string();
            } else if let Some(value) = line.strip_prefix("data: ") {
                data = value.to_string();
            }
        }
        if !event_type.is_empty() && !data.is_empty() {
            events.push((event_type, serde_json::from_str(&data).unwrap()));
        }
    }
    events
}

#[test]
fn builds_anthropic_text_request() {
    let (body, _) = build_anthropic_request(
        &request(vec![
            message("user", "hello"),
            message("assistant", "hi"),
            message("user", "continue"),
        ]),
        AnthropicProviderProfile::Anthropic,
        None,
    )
    .unwrap();

    assert_eq!(body["model"], "claude-sonnet-4-6");
    assert_eq!(body["max_tokens"], 1234);
    assert_eq!(body["system"], "Be precise.");
    assert_eq!(body["temperature"], 0.2);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
    assert_eq!(body["messages"][1]["role"], "assistant");
    assert_eq!(body["messages"][1]["content"][0]["text"], "hi");
    assert_eq!(body["cache_control"]["type"], "ephemeral");
    assert!(body["cache_control"].get("ttl").is_none());
}

#[test]
fn builds_anthropic_request_with_one_hour_prompt_cache_ttl() {
    let (body, _) = build_anthropic_request(
        &request(vec![message("user", "hello")]),
        AnthropicProviderProfile::Anthropic,
        Some("1h"),
    )
    .unwrap();

    assert_eq!(body["cache_control"]["type"], "ephemeral");
    assert_eq!(body["cache_control"]["ttl"], "1h");
}

#[test]
fn builds_anthropic_tool_result_message() {
    let mut output = message("user", "ignored");
    output.item_type = ItemType::FunctionCallOutput;
    output.content = None;
    output.call_id = Some("toolu_123".to_string());
    output.output = Some(FunctionCallOutput::ContentItems(vec![
        FunctionCallOutputContentItem {
            item_type: "output_text".to_string(),
            text: Some("done".to_string()),
            image_url: None,
            encrypted_content: None,
            detail: None,
        },
    ]));

    let (body, _) = build_anthropic_request(
        &request(vec![message("user", "run"), output]),
        AnthropicProviderProfile::Anthropic,
        None,
    )
    .unwrap();
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
    assert_eq!(
        body["messages"][1]["content"][0]["tool_use_id"],
        "toolu_123"
    );
    assert_eq!(body["messages"][1]["content"][0]["content"], "done");
}

#[test]
fn builds_anthropic_tool_result_with_image_content_blocks() {
    let image_url = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQAAAQABAAD";
    let output = image_tool_result("toolu_image", image_url);

    let (body, _) = build_anthropic_request(
        &request(vec![message("user", "run"), output]),
        AnthropicProviderProfile::Anthropic,
        None,
    )
    .unwrap();

    assert_anthropic_tool_result_image_content(&body);
}

#[test]
fn builds_glm_tool_result_with_image_content_blocks() {
    let image_url = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQAAAQABAAD";
    let output = image_tool_result("toolu_image", image_url);

    let (body, _) = build_anthropic_request(
        &request(vec![message("user", "run"), output]),
        AnthropicProviderProfile::GlmAnthropic,
        None,
    )
    .unwrap();

    assert_anthropic_tool_result_image_content(&body);
}

fn image_tool_result(call_id: &str, image_url: &str) -> ResponseItem {
    let mut output = message("user", "ignored");
    output.item_type = ItemType::FunctionCallOutput;
    output.content = None;
    output.call_id = Some(call_id.to_string());
    output.output = Some(FunctionCallOutput::ContentItems(vec![
        FunctionCallOutputContentItem {
            item_type: "input_text".to_string(),
            text: Some("Wall time: 0.0060 seconds\nOutput:".to_string()),
            image_url: None,
            encrypted_content: None,
            detail: None,
        },
        FunctionCallOutputContentItem {
            item_type: "input_text".to_string(),
            text: Some("有的！这是B站搜索结果的截图：".to_string()),
            image_url: None,
            encrypted_content: None,
            detail: None,
        },
        FunctionCallOutputContentItem {
            item_type: "input_image".to_string(),
            text: None,
            image_url: Some(image_url.to_string()),
            encrypted_content: None,
            detail: Some(json!("original")),
        },
    ]));
    output
}

fn assert_anthropic_tool_result_image_content(body: &Value) {
    let content = &body["messages"][1]["content"][0]["content"];
    assert!(content.is_array());
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "Wall time: 0.0060 seconds\nOutput:");
    assert_eq!(content[1]["type"], "text");
    assert_eq!(content[1]["text"], "有的！这是B站搜索结果的截图：");
    assert_eq!(content[2]["type"], "image");
    assert_eq!(content[2]["source"]["type"], "base64");
    assert_eq!(content[2]["source"]["media_type"], "image/jpeg");
    assert_eq!(content[2]["source"]["data"], "/9j/4AAQSkZJRgABAQAAAQABAAD");
}

#[test]
fn builds_anthropic_tool_search_result_message_and_loaded_tools() {
    let mut search = message("assistant", "ignored");
    search.item_type = ItemType::ToolSearchCall;
    search.content = None;
    search.call_id = Some("tooluse_search_1".to_string());
    search.execution = Some("client".to_string());
    search.arguments = Some(crate::ai_gateway::model::JsonString::Value(json!({
        "query": "web search browse internet news sports"
    })));

    let mut output = message("user", "ignored");
    output.item_type = ItemType::ToolSearchOutput;
    output.content = None;
    output.call_id = Some("tooluse_search_1".to_string());
    output.execution = Some("client".to_string());
    output.status = Some("completed".to_string());
    output.tools = Some(vec![json!({
        "description": "Tools provided by the Codex app.",
        "name": "codex_app",
        "tools": [{
            "defer_loading": true,
            "description": "List recent Codex threads across the local host and connected remote hosts.",
            "name": "list_threads",
            "parameters": {
                "additionalProperties": false,
                "properties": {
                    "limit": {"type": "number"},
                    "query": {"type": "string"}
                },
                "type": "object"
            },
            "strict": false,
            "type": "function"
        }],
        "type": "namespace"
    })]);

    let (body, map) = build_anthropic_request(
        &request(vec![
            message("user", "today world cup results"),
            search,
            output,
        ]),
        AnthropicProviderProfile::Anthropic,
        None,
    )
    .unwrap();

    assert_eq!(body["messages"][1]["role"], "assistant");
    assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
    assert_eq!(body["messages"][1]["content"][0]["id"], "tooluse_search_1");
    assert_eq!(body["messages"][1]["content"][0]["name"], "tool_search");
    assert_eq!(
        body["messages"][1]["content"][0]["input"]["query"],
        "web search browse internet news sports"
    );
    assert_eq!(body["messages"][2]["role"], "user");
    assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(
        body["messages"][2]["content"][0]["tool_use_id"],
        "tooluse_search_1"
    );
    let result: Value = serde_json::from_str(
        body["messages"][2]["content"][0]["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["status"], "completed");
    assert_eq!(result["execution"], "client");
    assert_eq!(result["tools"][0]["name"], "codex_app");

    assert_eq!(body["tools"][0]["name"], "codex_app__codexns__list_threads");
    let decoded = map.decode("codex_app__codexns__list_threads");
    assert_eq!(decoded.namespace.as_deref(), Some("codex_app"));
    assert_eq!(decoded.name, "list_threads");
}

#[test]
fn builds_anthropic_tools_and_tool_choice() {
    let mut req = request(vec![message("user", "search docs")]);
    req.tools = vec![json!({
        "type": "namespace",
        "name": "browser",
        "tools": [{
            "type": "function",
            "name": "open page",
            "description": "Open a URL",
            "parameters": {
                "type": "object",
                "properties": {"url": {"type": "string"}},
                "required": ["url"]
            }
        }]
    })];
    req.tool_choice = Some(json!({
        "type": "function",
        "namespace": "browser",
        "name": "open page"
    }));

    let (body, map) =
        build_anthropic_request(&req, AnthropicProviderProfile::Anthropic, None).unwrap();
    assert_eq!(body["tools"][0]["name"], "browser__codexns__open_page");
    assert_eq!(body["tools"][0]["description"], "Open a URL");
    assert_eq!(body["tools"][0]["input_schema"]["required"][0], "url");
    assert_eq!(body["tool_choice"]["type"], "tool");
    assert_eq!(body["tool_choice"]["name"], "browser__codexns__open_page");

    let target = map.decode("browser__codexns__open_page");
    assert_eq!(target.namespace.as_deref(), Some("browser"));
    assert_eq!(target.name, "open page");
}

#[test]
fn builds_anthropic_apply_patch_custom_tool() {
    let mut req = request(vec![message("user", "edit a file")]);
    req.tools = vec![json!({
        "type": "custom",
        "name": "apply_patch",
        "description": "Use the `apply_patch` tool to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.",
        "format": {
            "type": "grammar",
            "syntax": "lark",
            "definition": "start: begin_patch hunk+ end_patch\nbegin_patch: \"*** Begin Patch\" LF\nend_patch: \"*** End Patch\" LF?"
        }
    })];
    req.tool_choice = Some(json!({
        "type": "custom",
        "name": "apply_patch"
    }));

    let (body, map) =
        build_anthropic_request(&req, AnthropicProviderProfile::Anthropic, None).unwrap();
    assert_eq!(body["tools"][0]["name"], "apply_patch");
    assert_eq!(body["tools"][0]["input_schema"]["required"][0], "input");
    assert_eq!(
        body["tools"][0]["input_schema"]["properties"]["input"]["description"],
        "The entire apply_patch patch body."
    );
    assert!(
        body["tools"][0]["description"]
            .as_str()
            .unwrap()
            .contains("Call this tool with JSON arguments matching")
    );
    assert!(
        body["tools"][0]["description"]
            .as_str()
            .unwrap()
            .contains("final non-whitespace line must be exactly `*** End Patch`")
    );
    let description = body["tools"][0]["description"].as_str().unwrap();
    assert!(description.contains("Few-shot examples:"));
    assert!(description.contains("*** Add File: notes.md\n+# Notes\n+"));
    assert!(description.contains("*** Update File: src/example.txt"));
    assert!(description.contains("*** Delete File: old.txt"));
    assert_eq!(body["tool_choice"]["type"], "tool");
    assert_eq!(body["tool_choice"]["name"], "apply_patch");

    let target = map.decode("apply_patch");
    assert_eq!(
        target.kind,
        crate::ai_gateway::tool_names::ToolCallKind::Custom
    );
    assert_eq!(target.name, "apply_patch");
}

#[test]
fn builds_anthropic_web_search_server_tool() {
    let mut req = request(vec![message("user", "latest rust news")]);
    req.tools = vec![json!({
        "type": "web_search_preview",
        "web_search": {
            "max_uses": 3,
            "allowed_domains": ["www.rust-lang.org"]
        }
    })];

    let (body, _) =
        build_anthropic_request(&req, AnthropicProviderProfile::Anthropic, None).unwrap();
    assert_eq!(body["tools"][0]["type"], ANTHROPIC_WEB_SEARCH_TYPE);
    assert_eq!(body["tools"][0]["name"], "web_search");
    assert_eq!(body["tools"][0]["max_uses"], 3);
    assert_eq!(body["tools"][0]["allowed_domains"][0], "www.rust-lang.org");
}

#[test]
fn preserves_native_anthropic_web_search_server_tool() {
    let mut req = request(vec![message("user", "latest rust news")]);
    req.tools = vec![json!({
        "type": ANTHROPIC_WEB_SEARCH_TYPE,
        "name": "web_search",
        "max_uses": 3,
        "allowed_domains": ["www.rust-lang.org"]
    })];

    let (body, _) =
        build_anthropic_request(&req, AnthropicProviderProfile::Anthropic, None).unwrap();
    assert_eq!(body["tools"][0]["type"], ANTHROPIC_WEB_SEARCH_TYPE);
    assert_eq!(body["tools"][0]["name"], "web_search");
    assert_eq!(body["tools"][0]["max_uses"], 3);
    assert_eq!(body["tools"][0]["allowed_domains"][0], "www.rust-lang.org");
}

#[test]
fn maps_responses_web_search_filters_to_anthropic_allowed_domains() {
    let mut req = request(vec![message("user", "latest rust news")]);
    req.tools = vec![json!({
        "type": "web_search",
        "external_web_access": true,
        "search_content_types": ["text", "image"],
        "filters": {
            "allowed_domains": ["www.rust-lang.org"]
        }
    })];

    let (body, _) =
        build_anthropic_request(&req, AnthropicProviderProfile::Anthropic, None).unwrap();
    assert_eq!(body["tools"][0]["type"], ANTHROPIC_WEB_SEARCH_TYPE);
    assert_eq!(body["tools"][0]["name"], "web_search");
    assert_eq!(body["tools"][0]["allowed_domains"][0], "www.rust-lang.org");
    assert!(body["tools"][0].get("external_web_access").is_none());
    assert!(body["tools"][0].get("search_content_types").is_none());
}

#[test]
fn builds_anthropic_adaptive_thinking_from_reasoning_effort() {
    let mut req = request(vec![message("user", "think carefully")]);
    req.reasoning = Some(Reasoning {
        effort: Some("max".to_string()),
        budget_tokens: None,
        generate_summary: None,
    });

    let (body, _) =
        build_anthropic_request(&req, AnthropicProviderProfile::Anthropic, None).unwrap();
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "max");
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn builds_anthropic_budget_thinking_from_explicit_budget() {
    let mut req = request(vec![message("user", "think carefully")]);
    req.reasoning = Some(Reasoning {
        effort: Some("high".to_string()),
        budget_tokens: Some(2_048),
        generate_summary: None,
    });

    let (body, _) =
        build_anthropic_request(&req, AnthropicProviderProfile::Anthropic, None).unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 2_048);
    assert!(body.get("output_config").is_none());
}

#[test]
fn builds_glm_reasoning_effort_from_reasoning() {
    let mut req = request(vec![message("user", "think carefully")]);
    req.reasoning = Some(Reasoning {
        effort: Some("max".to_string()),
        budget_tokens: None,
        generate_summary: None,
    });

    let (body, _) =
        build_anthropic_request(&req, AnthropicProviderProfile::GlmAnthropic, None).unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["reasoning_effort"], "max");
    assert!(body.get("output_config").is_none());
}

#[test]
fn maps_glm_medium_reasoning_to_high() {
    let mut req = request(vec![message("user", "think carefully")]);
    req.reasoning = Some(Reasoning {
        effort: Some("medium".to_string()),
        budget_tokens: None,
        generate_summary: None,
    });

    let (body, _) =
        build_anthropic_request(&req, AnthropicProviderProfile::GlmAnthropic, None).unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["reasoning_effort"], "high");
}

#[test]
fn maps_glm_none_reasoning_to_disabled_thinking() {
    let mut req = request(vec![message("user", "think carefully")]);
    req.reasoning = Some(Reasoning {
        effort: Some("none".to_string()),
        budget_tokens: None,
        generate_summary: None,
    });

    let (body, _) =
        build_anthropic_request(&req, AnthropicProviderProfile::GlmAnthropic, None).unwrap();
    assert_eq!(body["thinking"]["type"], "disabled");
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn converts_anthropic_text_response() {
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-6",
        "content": [{"type": "text", "text": "hello back"}],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 10,
            "output_tokens": 3,
            "cache_read_input_tokens": 4,
            "cache_creation_input_tokens": 6
        }
    });

    let converted = convert_response(&response);
    assert_eq!(converted.id, "msg_123");
    assert_eq!(converted.model, "claude-sonnet-4-6");
    assert_eq!(converted.status, "completed");
    assert_eq!(converted.output.len(), 1);
    let Some(ItemContent::Parts(parts)) = converted.output[0].content.as_ref() else {
        panic!("expected output_text content part");
    };
    assert_eq!(parts[0].part_type, "output_text");
    assert_eq!(parts[0].text.as_deref(), Some("hello back"));
    let usage = converted.usage.unwrap();
    assert_eq!(usage.input_tokens, 20);
    assert_eq!(usage.output_tokens, 3);
    assert_eq!(usage.total_tokens, 23);
    let details = usage.input_tokens_details.unwrap();
    assert_eq!(details.cached_tokens, 4);
    assert_eq!(details.cache_creation_tokens, 6);
}

#[test]
fn converts_anthropic_text_citations_to_responses_annotations() {
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-6",
        "content": [{
            "type": "text",
            "text": "Rust is maintained by the Rust Foundation.",
            "citations": [{
                "type": "web_search_result_location",
                "url": "https://www.rust-lang.org/",
                "title": "Rust",
                "cited_text": "Rust language homepage"
            }]
        }],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 10,
            "output_tokens": 3
        }
    });

    let converted = convert_response(&response);
    let Some(ItemContent::Parts(parts)) = converted.output[0].content.as_ref() else {
        panic!("expected output_text content part");
    };
    let annotations = parts[0].annotations.as_ref().expect("expected annotations");
    assert_eq!(annotations.len(), 1);
    assert_eq!(annotations[0]["type"], "url_citation");
    assert_eq!(annotations[0]["url"], "https://www.rust-lang.org/");
    assert_eq!(annotations[0]["title"], "Rust");
}

#[test]
fn converts_anthropic_cache_creation_breakdown_usage() {
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-6",
        "content": [{"type": "text", "text": "hello back"}],
        "stop_reason": "end_turn",
        "usage": {
            "input_tokens": 9,
            "output_tokens": 73,
            "cache_read_input_tokens": 5699,
            "cache_creation": {
                "ephemeral_5m_input_tokens": 100,
                "ephemeral_1h_input_tokens": 25
            }
        }
    });

    let converted = convert_response(&response);
    let usage = converted.usage.unwrap();
    assert_eq!(usage.input_tokens, 5833);
    assert_eq!(usage.output_tokens, 73);
    assert_eq!(usage.total_tokens, 5906);
    let details = usage.input_tokens_details.unwrap();
    assert_eq!(details.cached_tokens, 5699);
    assert_eq!(details.cache_creation_tokens, 125);
}

#[test]
fn converts_anthropic_thinking_response() {
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-6",
        "content": [
            {"type": "thinking", "thinking": "I should reason first.", "signature": "sig_123"},
            {"type": "redacted_thinking", "data": "encrypted_456"},
            {"type": "text", "text": "final"}
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_response(&response);
    assert_eq!(converted.output.len(), 3);
    assert_eq!(converted.output[0].item_type, ItemType::Reasoning);
    assert_eq!(
        converted.output[0].summary.as_ref().unwrap()[0].text,
        "I should reason first."
    );
    assert_eq!(
        converted.output[0].encrypted_content.as_deref(),
        Some("sig_123")
    );
    assert_eq!(converted.output[1].item_type, ItemType::Reasoning);
    assert_eq!(
        converted.output[1].encrypted_content.as_deref(),
        Some("encrypted_456")
    );
    assert_eq!(converted.output[2].item_type, ItemType::Message);
}

#[test]
fn converts_anthropic_tool_use_response() {
    let mut map = ToolNameMap::default();
    let encoded = map.encode_function(Some("browser"), "open page");
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-6",
        "content": [{
            "type": "tool_use",
            "id": "toolu_123",
            "name": encoded,
            "input": {"url": "https://example.com"}
        }],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_anthropic_response(
        &response,
        "fallback-model",
        &map,
        AnthropicProviderProfile::Anthropic,
    );
    assert_eq!(converted.status, "completed");
    assert_eq!(converted.output.len(), 1);
    let item = &converted.output[0];
    assert_eq!(item.item_type, ItemType::FunctionCall);
    assert_eq!(item.namespace.as_deref(), Some("browser"));
    assert_eq!(item.name.as_deref(), Some("open page"));
    assert_eq!(item.call_id.as_deref(), Some("toolu_123"));
    assert_eq!(
        item.arguments.as_ref().unwrap().to_value()["url"],
        "https://example.com"
    );
}

#[test]
fn converts_anthropic_apply_patch_tool_use_to_custom_tool_call() {
    let mut map = ToolNameMap::default();
    map.encode_custom("apply_patch");
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-4-8",
        "content": [{
            "type": "tool_use",
            "id": "toolu_patch",
            "name": "apply_patch",
            "input": {
                "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n"
            }
        }],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_anthropic_response(
        &response,
        "fallback-model",
        &map,
        AnthropicProviderProfile::Anthropic,
    );
    assert_eq!(converted.output.len(), 1);
    let item = &converted.output[0];
    assert_eq!(item.item_type, ItemType::CustomToolCall);
    assert_eq!(item.name.as_deref(), Some("apply_patch"));
    assert_eq!(item.call_id.as_deref(), Some("toolu_patch"));
    assert_eq!(
        item.input.as_deref(),
        Some("*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n")
    );
    assert!(item.arguments.is_none());
}

#[test]
fn passes_through_anthropic_apply_patch_tool_use_input() {
    let mut map = ToolNameMap::default();
    map.encode_custom("apply_patch");
    let raw_input = "Here is the patch:\n```text\n*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n```\n";
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-4-8",
        "content": [{
            "type": "tool_use",
            "id": "toolu_patch",
            "name": "apply_patch",
            "input": {
                "input": raw_input
            }
        }],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_anthropic_response(
        &response,
        "fallback-model",
        &map,
        AnthropicProviderProfile::Anthropic,
    );

    assert_eq!(converted.output[0].input.as_deref(), Some(raw_input));
}

#[test]
fn converts_anthropic_server_web_search_response() {
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-6",
        "content": [
            {
                "type": "server_tool_use",
                "id": "srvtoolu_123",
                "name": "web_search",
                "input": {"query": "rust 2026"}
            },
            {
                "type": "web_search_tool_result",
                "tool_use_id": "srvtoolu_123",
                "content": [{"type": "web_search_result", "title": "Rust", "url": "https://www.rust-lang.org"}]
            }
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_response(&response);
    assert_eq!(converted.output.len(), 1);
    assert_eq!(converted.output[0].item_type, ItemType::WebSearchCall);
    assert_eq!(converted.output[0].call_id.as_deref(), Some("srvtoolu_123"));
    assert_eq!(
        converted.output[0].action.as_ref().unwrap()["query"],
        "rust 2026"
    );
    assert_eq!(
        converted.output[0].action.as_ref().unwrap().get("result"),
        None
    );
}

#[test]
fn converts_anthropic_tool_use_web_search_response() {
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-4-8",
        "content": [
            {
                "type": "tool_use",
                "id": "tooluse_5gdkvmCM90l5foLBnddBYO",
                "name": "web_search",
                "input": {"query": "Portugal Uzbekistan World Cup 2026 result Ronaldo goal"}
            }
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_response(&response);
    assert_eq!(converted.output.len(), 1);
    assert_eq!(converted.output[0].item_type, ItemType::WebSearchCall);
    assert_eq!(
        converted.output[0].call_id.as_deref(),
        Some("tooluse_5gdkvmCM90l5foLBnddBYO")
    );
    assert_eq!(
        converted.output[0].action.as_ref().unwrap()["query"],
        "Portugal Uzbekistan World Cup 2026 result Ronaldo goal"
    );
}

#[test]
fn skips_empty_anthropic_internal_web_search_response() {
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-4-8",
        "content": [
            {
                "type": "tool_use",
                "id": "tooluse_search",
                "name": "web_search",
                "input": {"query": "Portugal World Cup 2026 result"}
            },
            {
                "type": "server_tool_use",
                "id": "srvtoolu_internal",
                "name": "web_search"
            },
            {
                "type": "web_search_tool_result",
                "content": [
                    {
                        "type": "web_search_result",
                        "title": "Portugal wins",
                        "url": "https://example.com"
                    }
                ]
            }
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_response(&response);
    assert_eq!(converted.output.len(), 1);
    assert_eq!(converted.output[0].item_type, ItemType::WebSearchCall);
    assert_eq!(
        converted.output[0].call_id.as_deref(),
        Some("tooluse_search")
    );
    assert_eq!(
        converted.output[0].action.as_ref().unwrap()["query"],
        "Portugal World Cup 2026 result"
    );
}

#[test]
fn preserves_mapped_function_named_web_search_response() {
    let mut map = ToolNameMap::default();
    map.encode_function(None, "web_search");
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "claude-opus-4-8",
        "content": [
            {
                "type": "tool_use",
                "id": "tooluse_function_search",
                "name": "web_search",
                "input": {"query": "local function call"}
            }
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_anthropic_response(
        &response,
        "fallback-model",
        &map,
        AnthropicProviderProfile::Anthropic,
    );

    assert_eq!(converted.output.len(), 1);
    assert_eq!(converted.output[0].item_type, ItemType::FunctionCall);
    assert_eq!(converted.output[0].name.as_deref(), Some("web_search"));
    assert_eq!(
        converted.output[0]
            .arguments
            .as_ref()
            .map(|arguments| arguments.to_chat_arguments())
            .as_deref(),
        Some("{\"query\":\"local function call\"}")
    );
}

#[test]
fn converts_glm_web_search_prime_response() {
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "glm-5.2",
        "content": [
            {
                "type": "server_tool_use",
                "id": "call_search_1",
                "name": "web_search_prime",
                "input": {"search_query": "OpenAI June 2026", "location": "us"}
            },
            {
                "type": "tool_result",
                "tool_use_id": "call_search_1",
                "content": "[{'text': [{'title': 'OpenAI News', 'link': 'https://openai.com/news/', 'content': 'Latest ...'}], 'type': 'text'}]"
            },
            {"type": "text", "text": "Done"}
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_glm_response(&response);
    assert_eq!(converted.output.len(), 2);
    assert_eq!(converted.output[0].item_type, ItemType::WebSearchCall);
    assert_eq!(
        converted.output[0].call_id.as_deref(),
        Some("call_search_1")
    );
    assert_eq!(
        converted.output[0].action.as_ref().unwrap()["query"],
        "OpenAI June 2026"
    );
    assert_eq!(
        converted.output[0].action.as_ref().unwrap().get("result"),
        None
    );
    assert_eq!(converted.output[1].item_type, ItemType::Message);
}

#[test]
fn filters_glm_private_web_search_text_from_response() {
    let response = json!({
        "id": "msg_123",
        "type": "message",
        "role": "assistant",
        "model": "glm-5.2",
        "content": [
            {
                "type": "text",
                "text": "**\u{1f310} Z.ai Built-in Tool: web_search_prime**\n\n**Input:**\n```json\n{\"search_query\":\"OpenAI June 2026\"}\n```\n*Executing on server...*\n"
            },
            {
                "type": "server_tool_use",
                "id": "call_search_1",
                "name": "web_search_prime",
                "input": {"search_query": "OpenAI June 2026"}
            },
            {
                "type": "tool_result",
                "tool_use_id": "call_search_1",
                "content": "[{\"text\":[{\"title\":\"OpenAI News\"}],\"type\":\"text\"}]"
            },
            {
                "type": "text",
                "text": "**Output:**\n**web_search_prime_result_summary:** [{\"text\":[{\"title\":\"OpenAI News\"}]}]\n                                                Final answer"
            }
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 3}
    });

    let converted = convert_glm_response(&response);
    assert_eq!(converted.output.len(), 2);
    assert_eq!(converted.output[0].item_type, ItemType::WebSearchCall);
    assert_eq!(
        converted.output[0].action.as_ref().unwrap().get("result"),
        None
    );
    assert_eq!(converted.output[1].item_type, ItemType::Message);
    let Some(ItemContent::Parts(parts)) = converted.output[1].content.as_ref() else {
        panic!("expected message parts");
    };
    assert_eq!(parts[0].text.as_deref(), Some("Final answer"));
    let encoded = serde_json::to_string(&converted).unwrap();
    assert!(!encoded.contains("web_search_prime"));
    assert!(!encoded.contains("web_search_prime_result_summary"));
}

#[tokio::test]
async fn streams_anthropic_thinking_as_responses_reasoning_sse() {
    let input = stream::iter(vec![
        Ok::<_, std::io::Error>(Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"I should\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\" think\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig_123\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"final\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )),
    ]);

    let chunks = response_stream(input, "fallback-model", ToolNameMap::default())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let events = parse_events_from_bytes(&chunks);

    assert!(events.iter().any(
        |(event, data)| event == "response.reasoning_summary_text.delta"
            && data["delta"] == "I should"
    ));
    let reasoning_done = events
        .iter()
        .find(|(event, data)| {
            event == "response.output_item.done" && data["item"]["type"] == "reasoning"
        })
        .unwrap();
    assert_eq!(
        reasoning_done.1["item"]["summary"][0]["text"],
        "I should think"
    );
    assert_eq!(reasoning_done.1["item"]["encrypted_content"], "sig_123");

    let reasoning_done_pos = events
        .iter()
        .position(|(event, data)| {
            event == "response.output_item.done" && data["item"]["type"] == "reasoning"
        })
        .unwrap();
    let text_delta_pos = events
        .iter()
        .position(|(event, data)| event == "response.output_text.delta" && data["delta"] == "final")
        .unwrap();
    assert!(reasoning_done_pos < text_delta_pos);
}

#[tokio::test]
async fn streams_anthropic_text_as_responses_sse() {
    let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0,\"cache_read_input_tokens\":3,\"cache_creation_input_tokens\":4}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

    let chunks = response_stream(input, "fallback-model", ToolNameMap::default())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let events = parse_events_from_bytes(&chunks);

    assert!(events.iter().any(|(event, _)| event == "response.created"));
    assert!(
        events
            .iter()
            .any(|(event, data)| event == "response.output_text.delta" && data["delta"] == "hello")
    );
    let completed = events
        .iter()
        .find(|(event, _)| event == "response.completed")
        .unwrap();
    assert_eq!(completed.1["response"]["id"], "msg_1");
    assert_eq!(completed.1["response"]["usage"]["input_tokens"], 9);
    assert_eq!(completed.1["response"]["usage"]["output_tokens"], 1);
    assert_eq!(
        completed.1["response"]["usage"]["input_tokens_details"]["cached_tokens"],
        3
    );
    assert_eq!(
        completed.1["response"]["usage"]["input_tokens_details"]["cache_creation_tokens"],
        4
    );
}

#[tokio::test]
async fn streams_anthropic_tool_use_as_responses_sse() {
    let mut map = ToolNameMap::default();
    let encoded = map.encode_function(Some("browser"), "open page");
    let start = format!(
        "event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"{encoded}\",\"input\":{{}}}}}}\n\n"
    );
    let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from(start)),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"url\\\":\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"https://example.com\\\"}\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

    let chunks = response_stream(input, "fallback-model", map)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let events = parse_events_from_bytes(&chunks);

    assert!(events.iter().any(
        |(event, data)| event == "response.function_call_arguments.delta"
            && data["delta"] == "{\"url\":"
    ));
    let done = events
        .iter()
        .find(|(event, data)| {
            event == "response.output_item.done" && data["item"]["type"] == "function_call"
        })
        .unwrap();
    assert_eq!(done.1["item"]["namespace"], "browser");
    assert_eq!(done.1["item"]["name"], "open page");
    assert_eq!(
        done.1["item"]["arguments"],
        "{\"url\":\"https://example.com\"}"
    );
}

#[tokio::test]
async fn streams_anthropic_apply_patch_tool_use_as_custom_tool_call() {
    let mut map = ToolNameMap::default();
    map.encode_custom("apply_patch");
    let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_patch\",\"name\":\"apply_patch\",\"input\":{}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"input\\\":\\\"Here is the patch:\\\\n*** Begin Patch\\\\n\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"*** Add File: hello.txt\\\\n+hello\\\\n*** End Patch\\\\n\\\"}\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

    let chunks = response_stream(input, "fallback-model", map)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let events = parse_events_from_bytes(&chunks);

    assert!(
        events
            .iter()
            .any(|(event, _)| event == "response.custom_tool_call_input.done")
    );
    let done = events
        .iter()
        .find(|(event, data)| {
            event == "response.output_item.done" && data["item"]["type"] == "custom_tool_call"
        })
        .unwrap();
    assert_eq!(done.1["item"]["name"], "apply_patch");
    assert_eq!(
        done.1["item"]["input"],
        "Here is the patch:\n*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n"
    );
}

#[tokio::test]
async fn streams_anthropic_web_search_as_responses_sse() {
    let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvtoolu_1\",\"name\":\"web_search\",\"input\":{}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\": \\\"rust 2026\\\"}\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvtoolu_1\",\"content\":[{\"type\":\"web_search_result\",\"title\":\"Rust\",\"url\":\"https://www.rust-lang.org\"}]}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

    let chunks = response_stream(input, "fallback-model", ToolNameMap::default())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let events = parse_events_from_bytes(&chunks);

    assert!(
        events
            .iter()
            .any(|(event, data)| event == "response.output_item.added"
                && data["item"]["type"] == "web_search_call"
                && data["item"]["status"] == "in_progress")
    );
    let done = events
        .iter()
        .find(|(event, data)| {
            event == "response.output_item.done" && data["item"]["type"] == "web_search_call"
        })
        .unwrap();
    assert_eq!(done.1["item"]["call_id"], "srvtoolu_1");
    assert_eq!(done.1["item"]["action"]["type"], "search");
    assert_eq!(done.1["item"]["action"]["query"], "rust 2026");
    assert!(done.1["item"]["action"].get("result").is_none());
}

#[tokio::test]
async fn streams_anthropic_citations_as_responses_annotations() {
    let input = stream::iter(vec![
        Ok::<_, std::io::Error>(Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"citations_delta\",\"citation\":{\"type\":\"web_search_result_location\",\"url\":\"https://www.rust-lang.org/\",\"title\":\"Rust\",\"cited_text\":\"Rust language homepage\"}}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Rust has a homepage.\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )),
    ]);

    let chunks = response_stream(input, "fallback-model", ToolNameMap::default())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let events = parse_events_from_bytes(&chunks);

    assert!(events.iter().any(
        |(event, data)| event == "response.output_text.annotation.added"
            && data["annotation"]["type"] == "url_citation"
            && data["annotation"]["url"] == "https://www.rust-lang.org/"
    ));
    let done = events
        .iter()
        .find(|(event, data)| {
            event == "response.output_item.done" && data["item"]["type"] == "message"
        })
        .unwrap();
    let annotation = &done.1["item"]["content"][0]["annotations"][0];
    assert_eq!(annotation["type"], "url_citation");
    assert_eq!(annotation["url"], "https://www.rust-lang.org/");
    assert_eq!(annotation["title"], "Rust");
    assert_eq!(annotation["start_index"], 0);
    assert_eq!(annotation["end_index"], 20);
}

#[tokio::test]
async fn streams_anthropic_tool_use_web_search_as_responses_sse() {
    let input = stream::iter(vec![
        Ok::<_, std::io::Error>(Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tooluse_5gdkvmCM90l5foLBnddBYO\",\"name\":\"web_search\",\"input\":{\"query\":\"Portugal Uzbekistan World Cup 2026 result Ronaldo goal\"}}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )),
    ]);

    let chunks = response_stream(input, "fallback-model", ToolNameMap::default())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let events = parse_events_from_bytes(&chunks);

    assert!(
        events
            .iter()
            .any(|(event, data)| event == "response.output_item.added"
                && data["item"]["type"] == "web_search_call"
                && data["item"]["status"] == "in_progress")
    );
    let done = events
        .iter()
        .find(|(event, data)| {
            event == "response.output_item.done" && data["item"]["type"] == "web_search_call"
        })
        .unwrap();
    assert_eq!(done.1["item"]["call_id"], "tooluse_5gdkvmCM90l5foLBnddBYO");
    assert_eq!(
        done.1["item"]["action"]["query"],
        "Portugal Uzbekistan World Cup 2026 result Ronaldo goal"
    );
}

#[tokio::test]
async fn streams_skip_empty_anthropic_internal_web_search_sse() {
    let input = stream::iter(vec![
        Ok::<_, std::io::Error>(Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-opus-4-8\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tooluse_search\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\": \\\"Portugal World Cup 2026 result\\\"}\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvtoolu_internal\",\"name\":\"web_search\"}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"web_search_tool_result\",\"content\":[{\"type\":\"web_search_result\",\"title\":\"Portugal wins\",\"url\":\"https://example.com\"}]}}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
        )),
        Ok(Bytes::from_static(
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        )),
    ]);

    let chunks = response_stream(input, "fallback-model", ToolNameMap::default())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let events = parse_events_from_bytes(&chunks);
    let done_searches = events
        .iter()
        .filter(|(event, data)| {
            event == "response.output_item.done" && data["item"]["type"] == "web_search_call"
        })
        .collect::<Vec<_>>();

    assert_eq!(done_searches.len(), 1);
    assert_eq!(done_searches[0].1["item"]["call_id"], "tooluse_search");
    assert_eq!(
        done_searches[0].1["item"]["action"]["query"],
        "Portugal World Cup 2026 result"
    );
}

#[tokio::test]
async fn streams_glm_web_search_prime_as_responses_sse() {
    let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"glm-5.2\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"call_search_1\",\"name\":\"web_search_prime\",\"input\":{\"search_query\":\"OpenAI June 2026\"}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_result\",\"tool_use_id\":\"call_search_1\",\"content\":\"[{\\\"text\\\":[{\\\"title\\\":\\\"OpenAI News\\\",\\\"link\\\":\\\"https://openai.com/news/\\\"}],\\\"type\\\":\\\"text\\\"}]\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

    let chunks = glm_response_stream(input, "fallback-model", ToolNameMap::default())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let events = parse_events_from_bytes(&chunks);

    assert!(
        events
            .iter()
            .any(|(event, data)| event == "response.output_item.added"
                && data["item"]["type"] == "web_search_call"
                && data["item"]["action"]["query"] == "OpenAI June 2026")
    );
    let done = events
        .iter()
        .find(|(event, data)| {
            event == "response.output_item.done" && data["item"]["type"] == "web_search_call"
        })
        .unwrap();
    assert_eq!(done.1["item"]["call_id"], "call_search_1");
    assert_eq!(done.1["item"]["action"]["query"], "OpenAI June 2026");
    assert!(done.1["item"]["action"].get("result").is_none());
}

#[tokio::test]
async fn streams_glm_private_web_search_text_is_filtered() {
    let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"glm-5.2\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"**\u{1f310} Z.ai Built-in Tool: web_search_prime**\\n\\n**Input:**\\n```json\\n{\\\"search_query\\\":\\\"OpenAI June 2026\\\"}\\n```\\n*Executing on server...*\\n\"}}\n\n".as_bytes(),
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"call_search_1\",\"name\":\"web_search_prime\",\"input\":{\"search_query\":\"OpenAI June 2026\"}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_result\",\"tool_use_id\":\"call_search_1\",\"content\":\"[{\\\"text\\\":[{\\\"title\\\":\\\"OpenAI News\\\"}],\\\"type\\\":\\\"text\\\"}]\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":3,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":3,\"delta\":{\"type\":\"text_delta\",\"text\":\"**Output:**\\n**web_search_prime_result_summary:** [{\\\"text\\\":[{\\\"title\\\":\\\"OpenAI News\\\"}]}]\\n                                                Final answer\"}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":3}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

    let chunks = glm_response_stream(input, "fallback-model", ToolNameMap::default())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let raw = String::from_utf8_lossy(&chunks.concat()).into_owned();
    assert!(!raw.contains("web_search_prime"));
    assert!(!raw.contains("web_search_prime_result_summary"));

    let events = parse_events_from_bytes(&chunks);
    assert!(
        events
            .iter()
            .any(|(event, data)| event == "response.output_text.delta"
                && data["delta"] == "Final answer")
    );
    assert!(
        events
            .iter()
            .any(|(event, data)| event == "response.output_item.done"
                && data["item"]["type"] == "web_search_call"
                && data["item"]["action"].get("result").is_none())
    );
}

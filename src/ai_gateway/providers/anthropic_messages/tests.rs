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
    let (body, _) = build_anthropic_request(&request(vec![
        message("user", "hello"),
        message("assistant", "hi"),
        message("user", "continue"),
    ]))
    .unwrap();

    assert_eq!(body["model"], "claude-sonnet-4-6");
    assert_eq!(body["max_tokens"], 1234);
    assert_eq!(body["system"], "Be precise.");
    assert_eq!(body["temperature"], 0.2);
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
    assert_eq!(body["messages"][1]["role"], "assistant");
    assert_eq!(body["messages"][1]["content"][0]["text"], "hi");
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

    let (body, _) =
        build_anthropic_request(&request(vec![message("user", "run"), output])).unwrap();
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
    assert_eq!(
        body["messages"][1]["content"][0]["tool_use_id"],
        "toolu_123"
    );
    assert_eq!(body["messages"][1]["content"][0]["content"], "done");
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

    let (body, map) = build_anthropic_request(&req).unwrap();
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
fn builds_anthropic_web_search_server_tool() {
    let mut req = request(vec![message("user", "latest rust news")]);
    req.tools = vec![json!({
        "type": "web_search_preview",
        "web_search": {
            "max_uses": 3,
            "allowed_domains": ["www.rust-lang.org"]
        }
    })];

    let (body, _) = build_anthropic_request(&req).unwrap();
    assert_eq!(body["tools"][0]["type"], ANTHROPIC_WEB_SEARCH_TYPE);
    assert_eq!(body["tools"][0]["name"], "web_search");
    assert_eq!(body["tools"][0]["max_uses"], 3);
    assert_eq!(body["tools"][0]["allowed_domains"][0], "www.rust-lang.org");
}

#[test]
fn builds_anthropic_thinking_from_reasoning() {
    let mut req = request(vec![message("user", "think carefully")]);
    req.reasoning = Some(Reasoning {
        effort: Some("high".to_string()),
        budget_tokens: Some(2_048),
        generate_summary: None,
    });

    let (body, _) = build_anthropic_request(&req).unwrap();
    assert_eq!(body["thinking"]["type"], "enabled");
    assert_eq!(body["thinking"]["budget_tokens"], 2_048);
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

    let converted =
        convert_anthropic_response(&response, "fallback-model", &ToolNameMap::default());
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

    let converted =
        convert_anthropic_response(&response, "fallback-model", &ToolNameMap::default());
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

    let converted =
        convert_anthropic_response(&response, "fallback-model", &ToolNameMap::default());
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

    let converted = convert_anthropic_response(&response, "fallback-model", &map);
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

    let converted =
        convert_anthropic_response(&response, "fallback-model", &ToolNameMap::default());
    assert_eq!(converted.output.len(), 2);
    assert_eq!(converted.output[0].item_type, ItemType::WebSearchCall);
    assert_eq!(converted.output[0].call_id.as_deref(), Some("srvtoolu_123"));
    assert_eq!(
        converted.output[0].action.as_ref().unwrap()["input"]["query"],
        "rust 2026"
    );
    assert_eq!(converted.output[1].item_type, ItemType::WebSearchCall);
    assert_eq!(
        converted.output[1].action.as_ref().unwrap()["content"][0]["url"],
        "https://www.rust-lang.org"
    );
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

    let chunks = AnthropicSseToResponsesSse::new(
        input,
        "fallback-model".to_string(),
        ToolNameMap::default(),
    )
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

    let chunks = AnthropicSseToResponsesSse::new(
        input,
        "fallback-model".to_string(),
        ToolNameMap::default(),
    )
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

    let chunks = AnthropicSseToResponsesSse::new(input, "fallback-model".to_string(), map)
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
async fn streams_anthropic_web_search_as_responses_sse() {
    let input = stream::iter(vec![
            Ok::<_, std::io::Error>(Bytes::from_static(
                b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvtoolu_1\",\"name\":\"web_search\",\"input\":{\"query\":\"rust 2026\"}}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvtoolu_1\",\"content\":[{\"type\":\"web_search_result\",\"title\":\"Rust\",\"url\":\"https://www.rust-lang.org\"}]}}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            )),
        ]);

    let chunks = AnthropicSseToResponsesSse::new(
        input,
        "fallback-model".to_string(),
        ToolNameMap::default(),
    )
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
    assert_eq!(done.1["item"]["action"]["input"]["query"], "rust 2026");
    assert_eq!(
        done.1["item"]["action"]["result"]["content"][0]["url"],
        "https://www.rust-lang.org"
    );
}

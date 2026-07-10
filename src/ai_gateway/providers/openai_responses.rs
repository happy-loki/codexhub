use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};
use tracing::{debug, error};

use crate::ai_gateway::config::{ProviderConfig, ProviderType, provider_api_root};
use crate::ai_gateway::context::{GatewayContext, apply_upstream_headers};
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::request_log::{
    self, RequestLogContext, RequestLogUpdate, ResponsesSseLogStream, UpstreamSseCaptureStream,
};

use super::{
    apply_total_request_timeout, ensure_success_response, execute_stream_start,
    execute_upstream_request,
};

/// OpenAI Responses API 透传：补齐 cache 字段后代理到上游。
pub async fn passthrough(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    mut raw_body: serde_json::Value,
    upstream_model: &str,
    provider: &ProviderConfig,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    // 1. 补齐 prompt_cache_key
    let existing_key = raw_body
        .get("prompt_cache_key")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if existing_key.is_empty() {
        raw_body["prompt_cache_key"] = json!(ctx.prompt_cache_key);
    }

    // 2. 补齐 prompt_cache_retention
    if let Some(retention) = &provider.prompt_cache_retention {
        if raw_body.get("prompt_cache_retention").is_none() {
            raw_body["prompt_cache_retention"] = json!(retention);
        }
    }
    raw_body["model"] = json!(upstream_model);
    normalize_grok_reasoning_replay(&mut raw_body, provider);

    let is_stream = raw_body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 3. 构建上游请求
    let url = format!("{}/v1/responses", provider_api_root(&provider.base_url));

    let req_builder = client
        .post(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", provider.api_key));
    let req_builder =
        apply_total_request_timeout(req_builder, provider.timeout_secs, is_stream).json(&raw_body);

    let req_builder = apply_upstream_headers(req_builder, &ctx.upstream_headers);
    let upstream_req = req_builder.build().map_err(|e| {
        error!(error = %e, "build upstream request failed");
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("build upstream request: {e}"),
        )
    })?;

    if let Some(log_context) = &log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: log_context
                .details_enabled
                .then(|| request_log::headers_to_json(upstream_req.headers()))
                .flatten(),
            upstream_request_body_bytes: request_log::json_body_size_bytes(&raw_body),
            upstream_request_json: log_context
                .details_enabled
                .then(|| serde_json::to_string(&raw_body).ok())
                .flatten(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(err);
        }
    }

    debug!(url = %url, stream = is_stream, "proxying to openai responses");

    let upstream_resp = if is_stream {
        execute_stream_start(
            client,
            upstream_req,
            provider.timeout_secs,
            "upstream request failed",
        )
        .await?
    } else {
        execute_upstream_request(
            client,
            upstream_req,
            provider.timeout_secs,
            "upstream request failed",
        )
        .await?
    };

    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;

    // 6. 流式：透传 SSE 流
    if is_stream {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            HeaderName::from_static("cache-control"),
            HeaderValue::from_static("no-cache"),
        );
        headers.insert(
            HeaderName::from_static("connection"),
            HeaderValue::from_static("keep-alive"),
        );

        let byte_stream = upstream_resp.bytes_stream().map(|result| {
            result.map_err(|e| {
                error!(error = %e, "upstream SSE stream error");
                std::io::Error::new(std::io::ErrorKind::Other, e)
            })
        });
        let body = if let Some(log_context) = log_context {
            let captured_upstream = UpstreamSseCaptureStream::new(byte_stream, log_context.clone());
            let compat_stream = ResponsesPassthroughCompatStream::new(Box::pin(captured_upstream));
            Body::from_stream(ResponsesSseLogStream::new(
                Box::pin(compat_stream),
                log_context,
            ))
        } else {
            Body::from_stream(ResponsesPassthroughCompatStream::new(Box::pin(byte_stream)))
        };
        let mut response = Response::new(body);
        *response.status_mut() = StatusCode::OK;
        *response.headers_mut() = headers;
        return Ok(response);
    }

    // 7. 非流式：透传 JSON 响应
    let body_bytes = upstream_resp.bytes().await.map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("read upstream body: {e}"))
    })?;
    let (body_bytes, response_json) = normalize_responses_passthrough_body(body_bytes);
    if let Some(log_context) = &log_context {
        let (status, usage, response_text) = response_json
            .as_ref()
            .map(|value| {
                (
                    request_log::status_from_response_value(value),
                    request_log::usage_from_response_value(value),
                    serde_json::to_string(value).ok(),
                )
            })
            .unwrap_or_else(|| ("completed".to_string(), Default::default(), None));
        let update = RequestLogUpdate {
            status: Some(status),
            usage: Some(usage),
            latency_ms: Some(request_log::elapsed_ms(log_context.started_at)),
            response_json: log_context
                .details_enabled
                .then_some(response_text)
                .flatten(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(err);
        }
    }

    let mut response = Response::new(Body::from(body_bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

fn normalize_grok_reasoning_replay(raw_body: &mut serde_json::Value, provider: &ProviderConfig) {
    if provider.provider_type != ProviderType::GrokResponses {
        return;
    }

    let Some(input) = raw_body
        .get_mut("input")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|item_type| item_type != "reasoning")
        {
            continue;
        }

        if item.get("content").is_some_and(serde_json::Value::is_null) {
            item.remove("content");
        }

        let has_encrypted_content = item
            .get("encrypted_content")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if !has_encrypted_content {
            continue;
        }

        let has_item_id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if has_item_id {
            item.entry("status".to_string())
                .or_insert_with(|| json!("completed"));
        } else {
            item.remove("encrypted_content");
        }
    }
}

/// Some OpenAI-compatible Responses providers emit Codex custom tools as
/// `name: "exec", namespace: "exec"`. Current Codex clients dispatch custom
/// tools by combining namespace and name, which turns this into `execexec`.
/// Function tools keep their namespace; this compatibility pass only removes a
/// duplicate namespace from custom tool-call items.
struct ResponsesPassthroughCompatStream<S> {
    inner: S,
    line_buf: String,
    output_queue: VecDeque<Result<Bytes, std::io::Error>>,
    ended: bool,
}

impl<S> ResponsesPassthroughCompatStream<S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            line_buf: String::new(),
            output_queue: VecDeque::new(),
            ended: false,
        }
    }
}

impl<S> Stream for ResponsesPassthroughCompatStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(item) = this.output_queue.pop_front() {
                return Poll::Ready(Some(item));
            }

            if this.ended {
                return Poll::Ready(None);
            }

            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.push_rewritten_chunk(&chunk);
                }
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err))),
                Poll::Ready(None) => {
                    this.ended = true;
                    if !this.line_buf.is_empty() {
                        let line = std::mem::take(&mut this.line_buf);
                        this.push_rewritten_line(&line);
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S> ResponsesPassthroughCompatStream<S> {
    fn push_rewritten_chunk(&mut self, chunk: &Bytes) {
        let text = String::from_utf8_lossy(chunk);
        self.line_buf.push_str(&text);
        while let Some(pos) = self.line_buf.find('\n') {
            let line = self.line_buf[..pos].trim_end_matches('\r').to_string();
            self.line_buf = self.line_buf[pos + 1..].to_string();
            self.push_rewritten_line(&line);
        }
    }

    fn push_rewritten_line(&mut self, line: &str) {
        let rewritten = rewrite_responses_sse_line(line);
        self.output_queue
            .push_back(Ok(Bytes::from(format!("{rewritten}\n"))));
    }
}

fn rewrite_responses_sse_line(line: &str) -> String {
    let Some(data) = sse_data_value(line) else {
        return line.to_string();
    };
    if data.trim() == "[DONE]" {
        return line.to_string();
    }
    let Ok(mut event) = serde_json::from_str::<Value>(data) else {
        return line.to_string();
    };
    if !normalize_duplicate_custom_tool_namespace(&mut event) {
        return line.to_string();
    }
    format!(
        "data: {}",
        serde_json::to_string(&event).unwrap_or_else(|_| data.to_string())
    )
}

fn sse_data_value(line: &str) -> Option<&str> {
    let data = line.strip_prefix("data:")?;
    Some(data.strip_prefix(' ').unwrap_or(data))
}

fn normalize_responses_passthrough_body(body_bytes: Bytes) -> (Bytes, Option<Value>) {
    let mut response_json = serde_json::from_slice::<Value>(&body_bytes).ok();
    let Some(value) = response_json.as_mut() else {
        return (body_bytes, response_json);
    };
    if normalize_duplicate_custom_tool_namespace(value) {
        let rewritten = serde_json::to_vec(value)
            .map(Bytes::from)
            .unwrap_or_else(|_| body_bytes.clone());
        (rewritten, response_json)
    } else {
        (body_bytes, response_json)
    }
}

fn normalize_duplicate_custom_tool_namespace(value: &mut Value) -> bool {
    match value {
        Value::Object(object) => {
            let mut changed = false;
            let is_duplicate_custom_tool_namespace = object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|item_type| item_type == "custom_tool_call")
                && object
                    .get("name")
                    .and_then(Value::as_str)
                    .zip(object.get("namespace").and_then(Value::as_str))
                    .is_some_and(|(name, namespace)| {
                        let name = name.trim();
                        !name.is_empty() && name == namespace.trim()
                    });

            if is_duplicate_custom_tool_namespace {
                object.remove("namespace");
                changed = true;
            }

            for child in object.values_mut() {
                changed |= normalize_duplicate_custom_tool_namespace(child);
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= normalize_duplicate_custom_tool_namespace(item);
            }
            changed
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Bytes;
    use futures_util::{StreamExt, stream};
    use serde_json::json;

    use crate::ai_gateway::config::{ProviderConfig, ProviderType};

    use super::{
        ResponsesPassthroughCompatStream, normalize_duplicate_custom_tool_namespace,
        normalize_grok_reasoning_replay, normalize_responses_passthrough_body,
    };

    fn grok_provider() -> ProviderConfig {
        ProviderConfig {
            name: "grok".to_string(),
            provider_type: ProviderType::GrokResponses,
            base_url: "https://api.x.ai/v1".to_string(),
            ..ProviderConfig::default()
        }
    }

    fn openai_responses_provider() -> ProviderConfig {
        ProviderConfig {
            name: "openai-compatible".to_string(),
            provider_type: ProviderType::OpenAiResponses,
            base_url: "https://api.x.ai/v1".to_string(),
            ..ProviderConfig::default()
        }
    }

    #[test]
    fn xai_reasoning_replay_without_item_id_drops_encrypted_content() {
        let mut body = json!({
            "model": "grok-4.5",
            "input": [
                {
                    "type": "reasoning",
                    "content": null,
                    "summary": [{"type": "summary_text", "text": "thinking"}],
                    "encrypted_content": "opaque-blob"
                }
            ]
        });

        normalize_grok_reasoning_replay(&mut body, &grok_provider());

        let reasoning = &body["input"][0];
        assert!(reasoning.get("encrypted_content").is_none());
        assert!(reasoning.get("content").is_none());
        assert!(reasoning.get("status").is_none());
        assert_eq!(reasoning["summary"][0]["text"], "thinking");
    }

    #[test]
    fn xai_reasoning_replay_with_item_id_keeps_blob_and_adds_status() {
        let mut body = json!({
            "model": "grok-4.5",
            "input": [
                {
                    "type": "reasoning",
                    "id": "rs_123",
                    "content": null,
                    "summary": [{"type": "summary_text", "text": "thinking"}],
                    "encrypted_content": "opaque-blob"
                }
            ]
        });

        normalize_grok_reasoning_replay(&mut body, &grok_provider());

        let reasoning = &body["input"][0];
        assert_eq!(reasoning["encrypted_content"], "opaque-blob");
        assert_eq!(reasoning["status"], "completed");
        assert!(reasoning.get("content").is_none());
    }

    #[test]
    fn openai_responses_provider_does_not_apply_grok_reasoning_replay_compatibility() {
        let mut body = json!({
            "model": "grok-4.5",
            "input": [
                {
                    "type": "reasoning",
                    "content": null,
                    "summary": [{"type": "summary_text", "text": "thinking"}],
                    "encrypted_content": "opaque-blob"
                }
            ]
        });

        normalize_grok_reasoning_replay(&mut body, &openai_responses_provider());

        let reasoning = &body["input"][0];
        assert_eq!(reasoning["encrypted_content"], "opaque-blob");
        assert!(
            reasoning
                .get("content")
                .is_some_and(|value| value.is_null())
        );
        assert!(reasoning.get("status").is_none());
    }

    #[test]
    fn duplicate_namespace_is_removed_only_for_custom_tool_calls() {
        let mut event = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "custom_tool_call",
                "name": "exec",
                "namespace": "exec",
                "call_id": "call_1",
                "input": "text('ok')"
            },
            "response": {
                "output": [
                    {
                        "type": "function_call",
                        "name": "read_file",
                        "namespace": "fs"
                    }
                ]
            }
        });

        assert!(normalize_duplicate_custom_tool_namespace(&mut event));

        assert!(event["item"].get("namespace").is_none());
        assert_eq!(event["item"]["name"], "exec");
        assert_eq!(event["response"]["output"][0]["namespace"], "fs");
    }

    #[test]
    fn non_streaming_body_normalizes_duplicate_custom_tool_namespace() {
        let body = Bytes::from(
            json!({
                "status": "completed",
                "output": [
                    {
                        "type": "custom_tool_call",
                        "name": "exec",
                        "namespace": "exec",
                        "call_id": "call_1"
                    }
                ]
            })
            .to_string(),
        );

        let (body, parsed) = normalize_responses_passthrough_body(body);
        let parsed = parsed.expect("parsed response");

        assert!(parsed["output"][0].get("namespace").is_none());
        assert!(
            !String::from_utf8(body.to_vec())
                .expect("utf8")
                .contains("\"namespace\"")
        );
    }

    #[tokio::test]
    async fn stream_normalizes_duplicate_custom_tool_namespace() {
        let chunks = stream::iter(vec![
            Ok(Bytes::from(
                "event: response.output_item.done\n\
                 data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"custom_tool_call\",\"name\":\"exec\",\"namespace\":\"exec\",\"call_id\":\"call_1\"}}\n\n",
            )),
            Ok(Bytes::from(
                "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"name\":\"read_file\",\"namespace\":\"fs\"}}\n\n",
            )),
        ]);
        let output = ResponsesPassthroughCompatStream::new(Box::pin(chunks))
            .collect::<Vec<Result<Bytes, std::io::Error>>>()
            .await;
        let text = output
            .into_iter()
            .map(|item| String::from_utf8(item.expect("chunk").to_vec()).expect("utf8"))
            .collect::<String>();

        assert!(text.contains("\"type\":\"custom_tool_call\""));
        assert!(text.contains("\"name\":\"exec\""));
        assert!(!text.contains("\"namespace\":\"exec\""));
        assert!(text.contains("\"type\":\"function_call\""));
        assert!(text.contains("\"name\":\"read_file\""));
        assert!(text.contains("\"namespace\":\"fs\""));
    }
}

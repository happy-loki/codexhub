use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use serde_json::json;
use tracing::{debug, error, warn};

use crate::ai_gateway::apply_patch_tool::APPLY_PATCH_TOOL_NAME;
use crate::ai_gateway::config::{ProviderConfig, ProviderType, provider_api_root};
use crate::ai_gateway::context::{GatewayContext, apply_upstream_headers};
use crate::ai_gateway::encrypted_content::{
    EncryptedContentScope, prepare_responses_request, remove_all_responses_encrypted_content,
};
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::request_log::{
    self, RequestLogContext, RequestLogUpdate, ResponsesSseLogStream, UpstreamSseCaptureStream,
};
use crate::ai_gateway::responses_compat::{
    ResponsesCompatSseStream, normalize_json_body_with_scope_and_tool_names,
};
use crate::ai_gateway::tool_names::ToolNameMap;

use super::{
    apply_total_request_timeout, ensure_success_response, execute_stream_start,
    execute_upstream_request, upstream_transport_retry_delay,
};

const UPSTREAM_REQUEST_BODY_READ_MAX_RETRIES: usize = 2;

#[derive(Clone, Copy)]
enum ResponsesEndpoint {
    Responses,
    Compact,
}

impl ResponsesEndpoint {
    fn path(self) -> &'static str {
        match self {
            Self::Responses => "/v1/responses",
            Self::Compact => "/v1/responses/compact",
        }
    }

    fn is_compact(self) -> bool {
        matches!(self, Self::Compact)
    }
}

/// OpenAI Responses API 透传：补齐 cache 字段后代理到上游。
pub async fn passthrough(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    raw_body: serde_json::Value,
    upstream_model: &str,
    provider: &ProviderConfig,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    passthrough_with_tool_names(
        client,
        ctx,
        raw_body,
        upstream_model,
        provider,
        None,
        log_context,
    )
    .await
}

pub async fn passthrough_with_tool_names(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    raw_body: serde_json::Value,
    upstream_model: &str,
    provider: &ProviderConfig,
    grok_tool_names: Option<ToolNameMap>,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    passthrough_to_endpoint(
        client,
        ctx,
        raw_body,
        upstream_model,
        provider,
        grok_tool_names,
        log_context,
        ResponsesEndpoint::Responses,
    )
    .await
}

/// OpenAI Responses Compact API 透传。该接口始终返回 unary JSON。
pub async fn passthrough_compact(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    raw_body: serde_json::Value,
    upstream_model: &str,
    provider: &ProviderConfig,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    passthrough_to_endpoint(
        client,
        ctx,
        raw_body,
        upstream_model,
        provider,
        None,
        log_context,
        ResponsesEndpoint::Compact,
    )
    .await
}

async fn passthrough_to_endpoint(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    mut raw_body: serde_json::Value,
    upstream_model: &str,
    provider: &ProviderConfig,
    mut grok_tool_names: Option<ToolNameMap>,
    log_context: Option<RequestLogContext>,
    endpoint: ResponsesEndpoint,
) -> Result<Response<Body>, GatewayError> {
    if endpoint.is_compact()
        && raw_body
            .get("stream")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    {
        return Err(GatewayError::bad_request(
            "responses compact does not support streaming",
        ));
    }

    if !endpoint.is_compact()
        && provider.provider_type == ProviderType::GrokResponses
        && grok_tool_names.is_none()
    {
        grok_tool_names = Some(ToolNameMap::default());
    }

    // 1. 补齐 prompt_cache_key
    let existing_key = raw_body
        .get("prompt_cache_key")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if existing_key.is_empty() {
        raw_body["prompt_cache_key"] = json!(ctx.prompt_cache_key);
    }

    // 2. 补齐 prompt_cache_retention
    if !endpoint.is_compact()
        && let Some(retention) = &provider.prompt_cache_retention
    {
        if raw_body.get("prompt_cache_retention").is_none() {
            raw_body["prompt_cache_retention"] = json!(retention);
        }
    }
    raw_body["model"] = json!(upstream_model);
    let encrypted_content_scope = EncryptedContentScope::for_provider(provider);
    let encrypted_content_stats =
        prepare_responses_request(&mut raw_body, &encrypted_content_scope);
    if encrypted_content_stats.filtered > 0 || encrypted_content_stats.decoded > 0 {
        debug!(
            provider = %provider.name,
            decoded = encrypted_content_stats.decoded,
            filtered = encrypted_content_stats.filtered,
            dropped_items = encrypted_content_stats.dropped_items,
            "prepared scoped encrypted reasoning content for upstream"
        );
    }
    let grok_compatibility = if endpoint.is_compact() {
        GrokModelInputStats::default()
    } else {
        normalize_grok_reasoning_replay(&mut raw_body, provider);
        normalize_grok_model_input_with_tool_names(
            &mut raw_body,
            provider,
            grok_tool_names.as_mut(),
        )
    };
    if grok_compatibility.changed() {
        debug!(
            provider = %provider.name,
            custom_calls = grok_compatibility.custom_calls,
            custom_outputs = grok_compatibility.custom_outputs,
            structured_outputs = grok_compatibility.structured_outputs,
            removed_phase_fields = grok_compatibility.removed_phase_fields,
            namespaced_calls = grok_compatibility.namespaced_calls,
            "normalized Codex tool history for Grok ModelInput"
        );
    }

    let is_stream = !endpoint.is_compact()
        && raw_body
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

    let url = format!(
        "{}{}",
        provider_api_root(&provider.base_url),
        endpoint.path()
    );
    let mut invalid_encrypted_content_retry = false;
    let mut request_body_read_retry_count = 0usize;
    let upstream_resp = loop {
        // 3. 构建上游请求。密文恢复重试会使用清理后的 body 重建请求。
        let req_builder = client
            .post(&url)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", provider.api_key));
        let req_builder =
            apply_total_request_timeout(req_builder, provider.timeout_secs, is_stream)
                .json(&raw_body);
        let req_builder = apply_upstream_headers(req_builder, &ctx.upstream_headers);
        let mut upstream_req = req_builder.build().map_err(|e| {
            error!(error = %e, "build upstream request failed");
            GatewayError::upstream(
                StatusCode::BAD_GATEWAY,
                format!("build upstream request: {e}"),
            )
        })?;
        if endpoint.is_compact() {
            upstream_req.headers_mut().insert(
                HeaderName::from_static("accept"),
                HeaderValue::from_static("application/json"),
            );
        }

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

        debug!(
            url = %url,
            stream = is_stream,
            encrypted_content_retry = invalid_encrypted_content_retry,
            request_body_read_retry_count,
            "proxying to openai responses endpoint"
        );

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

        if upstream_resp.status() == StatusCode::BAD_REQUEST {
            let body_text = upstream_resp.text().await.unwrap_or_default();
            if is_failed_to_read_request_body_error(StatusCode::BAD_REQUEST, &body_text)
                && request_body_read_retry_count < UPSTREAM_REQUEST_BODY_READ_MAX_RETRIES
            {
                request_body_read_retry_count += 1;
                warn!(
                    provider = %provider.name,
                    request_body_bytes = request_log::json_body_size_bytes(&raw_body),
                    retry_count = request_body_read_retry_count,
                    max_retries = UPSTREAM_REQUEST_BODY_READ_MAX_RETRIES,
                    "upstream could not finish reading request body; retrying safely"
                );
                tokio::time::sleep(upstream_transport_retry_delay(
                    request_body_read_retry_count,
                ))
                .await;
                continue;
            }
            if !invalid_encrypted_content_retry
                && is_invalid_encrypted_content_error(StatusCode::BAD_REQUEST, &body_text)
            {
                let cleanup = remove_all_responses_encrypted_content(&mut raw_body);
                if cleanup.filtered > 0 {
                    invalid_encrypted_content_retry = true;
                    warn!(
                        provider = %provider.name,
                        filtered = cleanup.filtered,
                        dropped_items = cleanup.dropped_items,
                        "retrying once after upstream rejected legacy or stale encrypted content"
                    );
                    continue;
                }
            }
            return Err(GatewayError::from_upstream_body(
                StatusCode::BAD_REQUEST,
                &provider.name,
                &body_text,
            ));
        }

        break upstream_resp;
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
        copy_upstream_response_headers(upstream_resp.headers(), &mut headers);

        let byte_stream = upstream_resp.bytes_stream().map(|result| {
            result.map_err(|e| {
                error!(error = %e, "upstream SSE stream error");
                std::io::Error::new(std::io::ErrorKind::Other, e)
            })
        });
        let body = if let Some(log_context) = log_context {
            let captured_upstream = UpstreamSseCaptureStream::new(byte_stream, log_context.clone());
            let compat_stream = ResponsesCompatSseStream::with_compatibility(
                Box::pin(captured_upstream),
                encrypted_content_scope.clone(),
                grok_tool_names.clone(),
            );
            Body::from_stream(ResponsesSseLogStream::new(
                Box::pin(compat_stream),
                log_context,
            ))
        } else {
            Body::from_stream(ResponsesCompatSseStream::with_compatibility(
                Box::pin(byte_stream),
                encrypted_content_scope.clone(),
                grok_tool_names.clone(),
            ))
        };
        let mut response = Response::new(body);
        *response.status_mut() = StatusCode::OK;
        *response.headers_mut() = headers;
        return Ok(response);
    }

    // 7. 非流式：透传 JSON 响应
    let upstream_headers = upstream_resp.headers().clone();
    let body_bytes = upstream_resp.bytes().await.map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("read upstream body: {e}"))
    })?;
    let (body_bytes, response_json) = normalize_json_body_with_scope_and_tool_names(
        body_bytes,
        Some(&encrypted_content_scope),
        grok_tool_names.as_ref(),
    );
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
    copy_upstream_response_headers(&upstream_headers, response.headers_mut());
    Ok(response)
}

fn copy_upstream_response_headers(source: &HeaderMap, target: &mut HeaderMap) {
    for name in ["x-codex-turn-state", "x-request-id", "openai-model"] {
        let name = HeaderName::from_static(name);
        if let Some(value) = source.get(&name) {
            target.insert(name, value.clone());
        }
    }
}

fn is_invalid_encrypted_content_error(status: StatusCode, body: &str) -> bool {
    if status != StatusCode::BAD_REQUEST {
        return false;
    }
    let normalized = body.to_ascii_lowercase();
    normalized.contains("invalid_encrypted_content")
        || (normalized.contains("encrypted content")
            && (normalized.contains("could not be verified")
                || normalized.contains("could not be decrypted")
                || normalized.contains("could not be parsed")))
}

fn is_failed_to_read_request_body_error(status: StatusCode, body: &str) -> bool {
    if status != StatusCode::BAD_REQUEST {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    value
        .get("error")
        .unwrap_or(&value)
        .get("message")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|message| {
            message
                .trim()
                .eq_ignore_ascii_case("Failed to read request body")
        })
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct GrokModelInputStats {
    custom_calls: usize,
    custom_outputs: usize,
    structured_outputs: usize,
    removed_phase_fields: usize,
    namespaced_calls: usize,
}

impl GrokModelInputStats {
    fn changed(self) -> bool {
        self.custom_calls > 0
            || self.custom_outputs > 0
            || self.structured_outputs > 0
            || self.removed_phase_fields > 0
            || self.namespaced_calls > 0
    }
}

#[cfg(test)]
fn normalize_grok_model_input(
    raw_body: &mut serde_json::Value,
    provider: &ProviderConfig,
) -> GrokModelInputStats {
    normalize_grok_model_input_with_tool_names(raw_body, provider, None)
}

fn normalize_grok_model_input_with_tool_names(
    raw_body: &mut serde_json::Value,
    provider: &ProviderConfig,
    mut tool_names: Option<&mut ToolNameMap>,
) -> GrokModelInputStats {
    let mut stats = GrokModelInputStats::default();
    if provider.provider_type != ProviderType::GrokResponses {
        return stats;
    }

    let Some(input) = raw_body
        .get_mut("input")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return stats;
    };

    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        let item_type = item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        match item_type.as_str() {
            "message" => {
                if item.remove("phase").is_some() {
                    stats.removed_phase_fields += 1;
                }
            }
            "custom_tool_call" => {
                let name = item
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                if let (Some(name), Some(tool_names)) = (name.as_deref(), tool_names.as_deref_mut())
                {
                    item.insert("name".to_string(), json!(tool_names.encode_custom(name)));
                }
                let input = item
                    .remove("input")
                    .map(grok_custom_tool_input_text)
                    .unwrap_or_default();
                let argument_name = if name.as_deref() == Some(APPLY_PATCH_TOOL_NAME) {
                    "patch"
                } else {
                    "input"
                };
                let arguments = serde_json::to_string(&json!({ (argument_name): input }))
                    .unwrap_or_else(|_| format!("{{\"{argument_name}\":\"\"}}"));
                item.insert("type".to_string(), json!("function_call"));
                item.insert("arguments".to_string(), json!(arguments));
                item.remove("status");
                stats.custom_calls += 1;
            }
            "function_call" => {
                let namespace = item
                    .get("namespace")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let name = item
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                if let (Some(namespace), Some(name), Some(tool_names)) =
                    (namespace, name, tool_names.as_deref_mut())
                {
                    item.insert(
                        "name".to_string(),
                        json!(tool_names.encode_function(Some(&namespace), &name)),
                    );
                    item.remove("namespace");
                    stats.namespaced_calls += 1;
                }
            }
            "custom_tool_call_output" => {
                item.insert("type".to_string(), json!("function_call_output"));
                stats.custom_outputs += 1;
                if normalize_grok_tool_output(item) {
                    stats.structured_outputs += 1;
                }
            }
            "function_call_output" => {
                if normalize_grok_tool_output(item) {
                    stats.structured_outputs += 1;
                }
            }
            _ => {}
        }
    }

    stats
}

fn grok_custom_tool_input_text(input: serde_json::Value) -> String {
    match input {
        serde_json::Value::String(input) => input,
        other => serde_json::to_string(&other).unwrap_or_default(),
    }
}

fn normalize_grok_tool_output(item: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    let Some(output) = item.get_mut("output") else {
        return false;
    };
    if output.is_string() {
        return false;
    }
    let normalized = grok_tool_output_text(output);
    *output = serde_json::Value::String(normalized);
    true
}

fn grok_tool_output_text(output: &serde_json::Value) -> String {
    let serde_json::Value::Array(items) = output else {
        return serde_json::to_string(output).unwrap_or_default();
    };
    if items.is_empty() {
        return String::new();
    }

    let mut text = Vec::with_capacity(items.len());
    for item in items {
        let Some(item) = item.as_object() else {
            return serde_json::to_string(output).unwrap_or_default();
        };
        let is_text = item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|item_type| matches!(item_type, "input_text" | "output_text" | "text"));
        if !is_text {
            return serde_json::to_string(output).unwrap_or_default();
        }
        if let Some(value) = item.get("text").and_then(serde_json::Value::as_str) {
            text.push(value);
        }
    }
    text.join("\n")
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{
        Json, Router,
        body::to_bytes,
        extract::State,
        http::{HeaderMap, HeaderValue, StatusCode},
        response::{IntoResponse, Response},
        routing::post,
    };
    use serde_json::json;
    use tokio::sync::mpsc;

    use crate::ai_gateway::config::{ProviderConfig, ProviderType};
    use crate::ai_gateway::context::GatewayContext;
    use crate::ai_gateway::tool_names::ToolNameMap;

    use super::{
        is_failed_to_read_request_body_error, is_invalid_encrypted_content_error,
        normalize_grok_model_input, normalize_grok_model_input_with_tool_names,
        normalize_grok_reasoning_replay, passthrough, passthrough_compact,
    };

    struct RetryServerState {
        attempts: AtomicUsize,
        requests: mpsc::UnboundedSender<serde_json::Value>,
    }

    async fn invalid_encrypted_content_then_success(
        State(state): State<Arc<RetryServerState>>,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        state
            .requests
            .send(body)
            .expect("request capture receiver should stay open");
        if state.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "code": "invalid_encrypted_content",
                        "type": "invalid_request_error",
                        "message": "The encrypted content could not be verified."
                    }
                })),
            )
                .into_response();
        }
        Json(json!({
            "id": "resp_recovered",
            "object": "response",
            "output": [{
                "type": "reasoning",
                "summary": [],
                "encrypted_content": "fresh-openai-content"
            }]
        }))
        .into_response()
    }

    async fn retry_server() -> (
        String,
        mpsc::UnboundedReceiver<serde_json::Value>,
        tokio::task::JoinHandle<()>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let state = Arc::new(RetryServerState {
            attempts: AtomicUsize::new(0),
            requests: sender,
        });
        let app = Router::new()
            .route(
                "/v1/responses",
                post(invalid_encrypted_content_then_success),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind retry server");
        let address = listener.local_addr().expect("retry server address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve retry endpoint");
        });
        (format!("http://{address}/v1"), receiver, task)
    }

    async fn failed_request_body_then_success(
        State(state): State<Arc<RetryServerState>>,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        state
            .requests
            .send(body)
            .expect("request capture receiver should stay open");
        if state.attempts.fetch_add(1, Ordering::SeqCst) < 2 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "message": "Failed to read request body",
                        "type": "invalid_request_error"
                    }
                })),
            )
                .into_response();
        }
        Json(json!({
            "id": "resp_upload_recovered",
            "object": "response",
            "status": "completed",
            "output": []
        }))
        .into_response()
    }

    async fn request_body_retry_server() -> (
        String,
        mpsc::UnboundedReceiver<serde_json::Value>,
        tokio::task::JoinHandle<()>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let state = Arc::new(RetryServerState {
            attempts: AtomicUsize::new(0),
            requests: sender,
        });
        let app = Router::new()
            .route("/v1/responses", post(failed_request_body_then_success))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind request body retry server");
        let address = listener.local_addr().expect("request body retry address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve request body retry endpoint");
        });
        (format!("http://{address}/v1"), receiver, task)
    }

    async fn capture_success(
        State(requests): State<mpsc::UnboundedSender<serde_json::Value>>,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        requests
            .send(body)
            .expect("request capture receiver should stay open");
        let mut response = Json(json!({
            "id": "resp_ok",
            "object": "response",
            "status": "completed",
            "output": []
        }))
        .into_response();
        response.headers_mut().insert(
            "x-codex-turn-state",
            HeaderValue::from_static("response-state"),
        );
        response
    }

    async fn capture_server() -> (
        String,
        mpsc::UnboundedReceiver<serde_json::Value>,
        tokio::task::JoinHandle<()>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/v1/responses", post(capture_success))
            .with_state(sender);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind capture server");
        let address = listener.local_addr().expect("capture server address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve capture endpoint");
        });
        (format!("http://{address}/v1"), receiver, task)
    }

    async fn capture_compact_success(
        State(requests): State<mpsc::UnboundedSender<(HeaderMap, serde_json::Value)>>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        requests
            .send((headers, body))
            .expect("request capture receiver should stay open");
        let mut response = Json(json!({
            "id": "cmp_ok",
            "object": "response.compaction",
            "created_at": 1_700_000_000,
            "output": [{
                "type": "compaction",
                "encrypted_content": "opaque-compact"
            }],
            "usage": {
                "input_tokens": 100,
                "output_tokens": 10,
                "total_tokens": 110
            }
        }))
        .into_response();
        response.headers_mut().insert(
            "x-codex-turn-state",
            HeaderValue::from_static("compact-state"),
        );
        response
    }

    async fn compact_capture_server() -> (
        String,
        mpsc::UnboundedReceiver<(HeaderMap, serde_json::Value)>,
        tokio::task::JoinHandle<()>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/v1/responses/compact", post(capture_compact_success))
            .with_state(sender);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind compact capture server");
        let address = listener
            .local_addr()
            .expect("compact capture server address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve compact capture endpoint");
        });
        (format!("http://{address}/v1"), receiver, task)
    }

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
    fn grok_model_input_normalizes_gpt_5_6_custom_tool_history() {
        let mut body = json!({
            "model": "grok-4.5",
            "input": [
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "commentary",
                    "content": [{"type": "output_text", "text": "running"}]
                },
                {
                    "type": "custom_tool_call",
                    "call_id": "call_exec",
                    "name": "exec",
                    "status": "completed",
                    "input": "Get-ChildItem"
                },
                {
                    "type": "custom_tool_call_output",
                    "call_id": "call_exec",
                    "output": [
                        {"type": "input_text", "text": "Wall time: 0.1 seconds"},
                        {"type": "input_text", "text": "file.txt"}
                    ]
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_wait",
                    "output": [
                        {"type": "output_text", "text": "done"}
                    ]
                }
            ]
        });

        let stats = normalize_grok_model_input(&mut body, &grok_provider());

        assert_eq!(stats.custom_calls, 1);
        assert_eq!(stats.custom_outputs, 1);
        assert_eq!(stats.structured_outputs, 2);
        assert_eq!(stats.removed_phase_fields, 1);
        assert!(body["input"][0].get("phase").is_none());

        let call = &body["input"][1];
        assert_eq!(call["type"], "function_call");
        assert_eq!(call["call_id"], "call_exec");
        assert_eq!(call["name"], "exec");
        assert!(call.get("input").is_none());
        assert!(call.get("status").is_none());
        let arguments: serde_json::Value =
            serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(arguments, json!({"input": "Get-ChildItem"}));

        let custom_output = &body["input"][2];
        assert_eq!(custom_output["type"], "function_call_output");
        assert_eq!(custom_output["output"], "Wall time: 0.1 seconds\nfile.txt");
        assert_eq!(body["input"][3]["output"], "done");
    }

    #[test]
    fn grok_model_input_serializes_non_text_tool_output_as_string() {
        let mut body = json!({
            "input": [{
                "type": "function_call_output",
                "call_id": "call_image",
                "output": [{
                    "type": "input_image",
                    "image_url": "data:image/png;base64,AAAA"
                }]
            }]
        });

        let stats = normalize_grok_model_input(&mut body, &grok_provider());

        assert_eq!(stats.structured_outputs, 1);
        let output = body["input"][0]["output"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(output).unwrap(),
            json!([{
                "type": "input_image",
                "image_url": "data:image/png;base64,AAAA"
            }])
        );
    }

    #[test]
    fn grok_model_input_reencodes_custom_and_namespace_history() {
        let mut body = json!({
            "input": [
                {
                    "type": "custom_tool_call",
                    "call_id": "call_exec",
                    "name": "exec",
                    "input": "pwd"
                },
                {
                    "type": "function_call",
                    "call_id": "call_open",
                    "namespace": "browser",
                    "name": "open",
                    "arguments": "{\"url\":\"https://example.com\"}"
                }
            ]
        });
        let mut tool_names = ToolNameMap::default();
        tool_names.encode_custom("exec");
        let browser_name = tool_names.encode_function(Some("browser"), "open");

        let stats = normalize_grok_model_input_with_tool_names(
            &mut body,
            &grok_provider(),
            Some(&mut tool_names),
        );

        assert_eq!(stats.custom_calls, 1);
        assert_eq!(stats.namespaced_calls, 1);
        assert_eq!(body["input"][0]["type"], "function_call");
        assert_eq!(body["input"][0]["name"], "exec");
        assert_eq!(body["input"][1]["name"], browser_name);
        assert!(body["input"][1].get("namespace").is_none());
    }

    #[test]
    fn grok_model_input_uses_grok_build_patch_argument_for_apply_patch_history() {
        let patch = "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch";
        let mut body = json!({
            "input": [{
                "type": "custom_tool_call",
                "call_id": "call_patch",
                "name": "apply_patch",
                "input": patch
            }]
        });
        let mut tool_names = ToolNameMap::default();
        tool_names.encode_custom("apply_patch");

        let stats = normalize_grok_model_input_with_tool_names(
            &mut body,
            &grok_provider(),
            Some(&mut tool_names),
        );

        assert_eq!(stats.custom_calls, 1);
        assert_eq!(body["input"][0]["type"], "function_call");
        assert_eq!(body["input"][0]["name"], "apply_patch");
        let arguments: serde_json::Value =
            serde_json::from_str(body["input"][0]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(arguments, json!({"patch": patch}));
    }

    #[test]
    fn openai_responses_provider_keeps_custom_tool_history_unchanged() {
        let mut body = json!({
            "input": [{
                "type": "custom_tool_call",
                "call_id": "call_exec",
                "name": "exec",
                "status": "completed",
                "input": "pwd"
            }]
        });
        let original = body.clone();

        let stats = normalize_grok_model_input(&mut body, &openai_responses_provider());

        assert!(!stats.changed());
        assert_eq!(body, original);
    }

    #[tokio::test]
    async fn grok_passthrough_sends_normalized_model_input() {
        let (base_url, mut requests, server) = capture_server().await;
        let provider = ProviderConfig {
            name: "grok".to_string(),
            provider_type: ProviderType::GrokResponses,
            base_url,
            api_key: "secret".to_string(),
            timeout_secs: 10,
            ..ProviderConfig::default()
        };
        let client = reqwest::Client::new();
        let context = GatewayContext::extract(&HeaderMap::new(), Some("grok-history-session"));
        let request = json!({
            "model": "grok-4.5",
            "stream": false,
            "input": [
                {
                    "type": "message",
                    "role": "assistant",
                    "phase": "commentary",
                    "content": [{"type": "output_text", "text": "running"}]
                },
                {
                    "type": "custom_tool_call",
                    "call_id": "call_exec",
                    "name": "exec",
                    "status": "completed",
                    "input": "Get-ChildItem"
                },
                {
                    "type": "custom_tool_call_output",
                    "call_id": "call_exec",
                    "output": [
                        {"type": "input_text", "text": "Wall time: 0.1 seconds"},
                        {"type": "input_text", "text": "file.txt"}
                    ]
                }
            ]
        });

        let response = passthrough(&client, &context, request, "grok-4.5", &provider, None)
            .await
            .expect("Grok history normalization should reach upstream");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-codex-turn-state").unwrap(),
            "response-state"
        );

        let upstream = requests.recv().await.expect("captured upstream request");
        assert!(upstream["input"][0].get("phase").is_none());
        assert_eq!(upstream["input"][1]["type"], "function_call");
        assert_eq!(upstream["input"][2]["type"], "function_call_output");
        assert_eq!(
            upstream["input"][2]["output"],
            "Wall time: 0.1 seconds\nfile.txt"
        );

        server.abort();
    }

    #[tokio::test]
    async fn compact_passthrough_uses_unary_compact_endpoint() {
        let (base_url, mut requests, server) = compact_capture_server().await;
        let provider = ProviderConfig {
            name: "openai".to_string(),
            provider_type: ProviderType::OpenAiResponses,
            base_url,
            api_key: "secret".to_string(),
            prompt_cache_retention: Some("24h".to_string()),
            timeout_secs: 10,
            ..ProviderConfig::default()
        };
        let client = reqwest::Client::new();
        let mut client_headers = HeaderMap::new();
        client_headers.insert("accept", HeaderValue::from_static("text/event-stream"));
        let context = GatewayContext::extract(&client_headers, Some("compact-cache-key"));
        let request = json!({
            "model": "gpt-client",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "compact this"}]
            }],
            "tools": [{"type": "function", "name": "lookup", "parameters": {}}],
            "parallel_tool_calls": true,
            "reasoning": {"effort": "high", "summary": "auto"},
            "service_tier": "priority",
            "text": {"verbosity": "low"},
            "future_field": {"preserved": true}
        });

        let response =
            passthrough_compact(&client, &context, request, "gpt-upstream", &provider, None)
                .await
                .expect("compact request should reach upstream");

        let (headers, upstream) = requests.recv().await.expect("captured compact request");
        assert_eq!(headers.get("accept").unwrap(), "application/json");
        assert_eq!(headers.get("authorization").unwrap(), "Bearer secret");
        assert_eq!(upstream["model"], "gpt-upstream");
        assert_eq!(upstream["prompt_cache_key"], "compact-cache-key");
        assert!(upstream.get("prompt_cache_retention").is_none());
        assert_eq!(upstream["tools"][0]["name"], "lookup");
        assert_eq!(upstream["parallel_tool_calls"], true);
        assert_eq!(upstream["service_tier"], "priority");
        assert_eq!(upstream["future_field"]["preserved"], true);

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-codex-turn-state").unwrap(),
            "compact-state"
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read compact response");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("compact response JSON");
        assert_eq!(body["object"], "response.compaction");
        assert_eq!(body["output"][0]["encrypted_content"], "opaque-compact");

        server.abort();
    }

    #[tokio::test]
    async fn compact_passthrough_rejects_streaming() {
        let provider = openai_responses_provider();
        let client = reqwest::Client::new();
        let context = GatewayContext::extract(&HeaderMap::new(), Some("compact-cache-key"));
        let result = passthrough_compact(
            &client,
            &context,
            json!({"model": "gpt-5", "stream": true, "input": []}),
            "gpt-5",
            &provider,
            None,
        )
        .await;

        let error = match result {
            Ok(_) => panic!("streaming compact request should fail"),
            Err(error) => error,
        };
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("does not support streaming"));
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
    fn recognizes_direct_and_wrapped_invalid_encrypted_content_errors() {
        assert!(is_invalid_encrypted_content_error(
            axum::http::StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"invalid_encrypted_content","message":"bad blob"}}"#,
        ));
        assert!(is_invalid_encrypted_content_error(
            axum::http::StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"upstream_error","message":"The encrypted content p3HD could not be verified. Reason: Encrypted content could not be decrypted or parsed."}}"#,
        ));
        assert!(!is_invalid_encrypted_content_error(
            axum::http::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Unknown parameter"}}"#,
        ));
        assert!(!is_invalid_encrypted_content_error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"error":{"code":"invalid_encrypted_content"}}"#,
        ));
    }

    #[test]
    fn recognizes_only_explicit_failed_request_body_errors() {
        assert!(is_failed_to_read_request_body_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Failed to read request body","type":"invalid_request_error"}}"#,
        ));
        assert!(is_failed_to_read_request_body_error(
            StatusCode::BAD_REQUEST,
            r#"{"message":"failed to read request body"}"#,
        ));
        assert!(!is_failed_to_read_request_body_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Failed to parse request body"}}"#,
        ));
        assert!(!is_failed_to_read_request_body_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"error":{"message":"Failed to read request body"}}"#,
        ));
    }

    #[tokio::test]
    async fn retries_failed_request_body_response_without_changing_payload() {
        let (base_url, mut requests, server) = request_body_retry_server().await;
        let provider = ProviderConfig {
            name: "sub2api".to_string(),
            provider_type: ProviderType::OpenAiResponses,
            base_url,
            api_key: "secret".to_string(),
            timeout_secs: 10,
            ..ProviderConfig::default()
        };
        let client = reqwest::Client::new();
        let context = GatewayContext::extract(&HeaderMap::new(), Some("upload-retry-session"));
        let request = json!({
            "model": "gpt-5.6-sol",
            "stream": false,
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "continue"}]
            }]
        });

        let response = passthrough(
            &client,
            &context,
            request.clone(),
            "gpt-5.6-sol",
            &provider,
            None,
        )
        .await
        .expect("request body retry should recover");

        let first = requests.recv().await.expect("first request");
        let second = requests.recv().await.expect("second request");
        let third = requests.recv().await.expect("third request");
        assert_eq!(first, second);
        assert_eq!(second, third);
        assert_eq!(third["input"], request["input"]);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read recovered response");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("response JSON");
        assert_eq!(body["id"], "resp_upload_recovered");

        server.abort();
    }

    #[tokio::test]
    async fn retries_once_without_stale_encrypted_content_and_preserves_response() {
        let (base_url, mut requests, server) = retry_server().await;
        let provider = ProviderConfig {
            name: "openai".to_string(),
            provider_type: ProviderType::OpenAiResponses,
            base_url,
            api_key: "secret".to_string(),
            timeout_secs: 10,
            ..ProviderConfig::default()
        };
        let client = reqwest::Client::new();
        let context = GatewayContext::extract(&HeaderMap::new(), Some("retry-session"));
        let request = json!({
            "model": "gpt-5.6-sol",
            "stream": false,
            "input": [
                {
                    "type": "reasoning",
                    "encrypted_content": "legacy-grok-content"
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "continue"}]
                }
            ]
        });

        let response = passthrough(&client, &context, request, "gpt-5.6-sol", &provider, None)
            .await
            .expect("retry should recover");

        let first = requests.recv().await.expect("first request");
        let second = requests.recv().await.expect("retry request");
        assert_eq!(
            first["input"][0]["encrypted_content"],
            "legacy-grok-content"
        );
        assert_eq!(second["input"].as_array().unwrap().len(), 1);
        assert_eq!(second["input"][0]["type"], "message");

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read recovered response");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("response JSON");
        assert_eq!(
            body["output"][0]["encrypted_content"],
            "fresh-openai-content"
        );

        server.abort();
    }
}

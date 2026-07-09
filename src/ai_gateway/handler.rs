use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, header::ETAG},
    response::IntoResponse,
};
use serde::{Deserialize, de::IntoDeserializer};
use serde_json::json;
use tracing::info;

use crate::app_state::SharedState;

use super::catalog::{configured_models_etag, configured_models_response_with_etag};
use super::config::{ProviderConfig, ProviderType};
use super::context::GatewayContext;
use super::error::GatewayError;
use super::model::GatewayRequest;
use super::providers::{anthropic_messages, deepseek_chat, openai_responses};
use super::request_log::{
    self, RequestLogContext, RequestLogRecord, RequestLogStore, RequestLogUpdate,
};
use super::router::resolve_provider_with_state;

static AI_GATEWAY_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// POST /ai-gateway/v1/responses
pub async fn handle_responses(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let started_at = Instant::now();
    let created_at_ms = request_log::now_ms();
    let config = state.config.lock().await;
    let gw_config = config.ai_gateway.clone();
    let filter_image_generation_tool = gw_config.filter_image_generation_tool;
    let request_logging_enabled = gw_config.request_logging_enabled;
    let request_log_details_enabled = gw_config.request_log_details_enabled;
    let models_etag = configured_models_etag(&gw_config);
    drop(config);
    let in_flight = AI_GATEWAY_IN_FLIGHT.fetch_add(1, Ordering::AcqRel) + 1;
    let _in_flight_guard = AiGatewayInFlightGuard;

    // 1. 解析请求 body
    let mut raw_body: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return GatewayError::bad_request(format!("invalid JSON: {e}")).into_response();
        }
    };
    filter_image_generation_tools(&mut raw_body, filter_image_generation_tool);
    let envelope: GatewayRequestEnvelope = match serde_json::from_value(raw_body.clone()) {
        Ok(envelope) => envelope,
        Err(e) => {
            return GatewayError::bad_request(format!("invalid request envelope: {e}"))
                .into_response();
        }
    };

    // 2. 提取上下文
    let body_cache_key = envelope.prompt_cache_key.as_deref();
    let ctx = GatewayContext::extract(&headers, body_cache_key);

    // 3. 路由到 provider（状态感知：熔断健康 + 会话粘性 + 权重优先级）
    let routing_now = Instant::now();
    let route_started = Instant::now();
    let (provider, route_id) = {
        let mut routing = state.ai_gateway_routing.lock().await;
        routing.evict_stale(routing_now);
        match resolve_provider_with_state(
            &envelope.model,
            ctx.session_id.as_deref(),
            &gw_config,
            &mut routing,
            routing_now,
        ) {
            Ok((provider, route_id)) => (provider, route_id),
            Err(e) => {
                drop(routing);
                let log_context = request_logging_enabled
                    .then(|| {
                        insert_initial_log(
                            &state.ai_gateway_request_logs,
                            &ctx,
                            &headers,
                            &envelope,
                            None,
                            &raw_body,
                            started_at,
                            created_at_ms,
                            request_log_details_enabled,
                        )
                    })
                    .flatten();
                update_failed_log(&log_context, &e.message);
                return e.into_response();
            }
        }
    };
    let route_ms = request_log::elapsed_ms(route_started);

    let log_context = request_logging_enabled
        .then(|| {
            insert_initial_log(
                &state.ai_gateway_request_logs,
                &ctx,
                &headers,
                &envelope,
                Some(provider),
                &raw_body,
                started_at,
                created_at_ms,
                request_log_details_enabled,
            )
        })
        .flatten();

    info!(
        model = %envelope.model,
        provider = %provider.name,
        provider_type = ?provider.provider_type,
        session_id = ?ctx.session_id,
        prompt_cache_key = %ctx.prompt_cache_key,
        stream = envelope.stream,
        in_flight,
        route_ms,
        details = request_log_details_enabled,
        "ai-gateway request routed"
    );

    // 4. 按 provider_type 分发
    let upstream_model = provider
        .resolve_upstream_model(&envelope.model)
        .unwrap_or(envelope.model.as_str())
        .to_string();
    match provider.provider_type {
        ProviderType::OpenAiResponses | ProviderType::GrokResponses => {
            let result = openai_responses::passthrough(
                &state.ai_gateway_http_client,
                &ctx,
                raw_body,
                &upstream_model,
                provider,
                log_context.clone(),
            )
            .await;
            let outcome = classify_outcome(&result);
            record_routing_outcome(&state, &route_id, outcome).await;
            match result {
                Ok(mut resp) => {
                    set_models_etag_header(&mut resp, &models_etag);
                    resp.into_response()
                }
                Err(e) => {
                    update_failed_log(&log_context, &e.message);
                    e.into_response()
                }
            }
        }
        ProviderType::ChatCompletions => {
            let request = match deserialize_gateway_request(raw_body.clone()) {
                Ok(request) => request,
                Err(e) => {
                    update_failed_log(&log_context, &format!("invalid request: {e}"));
                    return GatewayError::bad_request(format!("invalid request: {e}"))
                        .into_response();
                }
            };
            let mut upstream_request = request.clone();
            upstream_request.model = upstream_model;
            let result = deepseek_chat::handle(
                &state.ai_gateway_http_client,
                &ctx,
                &upstream_request,
                &request.model,
                provider,
                log_context.clone(),
            )
            .await;
            let outcome = classify_outcome(&result);
            record_routing_outcome(&state, &route_id, outcome).await;
            match result {
                Ok(mut resp) => {
                    set_models_etag_header(&mut resp, &models_etag);
                    resp.into_response()
                }
                Err(e) => {
                    update_failed_log(&log_context, &e.message);
                    e.into_response()
                }
            }
        }
        ProviderType::AnthropicMessages => {
            let request = match deserialize_gateway_request(raw_body.clone()) {
                Ok(request) => request,
                Err(e) => {
                    update_failed_log(&log_context, &format!("invalid request: {e}"));
                    return GatewayError::bad_request(format!("invalid request: {e}"))
                        .into_response();
                }
            };
            let mut upstream_request = request.clone();
            upstream_request.model = upstream_model;
            let result = anthropic_messages::handle(
                &state.ai_gateway_http_client,
                &ctx,
                &upstream_request,
                &request.model,
                provider,
                log_context.clone(),
            )
            .await;
            let outcome = classify_outcome(&result);
            record_routing_outcome(&state, &route_id, outcome).await;
            match result {
                Ok(mut resp) => {
                    set_models_etag_header(&mut resp, &models_etag);
                    resp.into_response()
                }
                Err(e) => {
                    update_failed_log(&log_context, &e.message);
                    e.into_response()
                }
            }
        }
    }
}

struct AiGatewayInFlightGuard;

impl Drop for AiGatewayInFlightGuard {
    fn drop(&mut self) {
        AI_GATEWAY_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 上游调用结果对熔断的分类。从 `Result<Response, _>` 提炼成只含 Send 信息的值，
/// 避免把 `!Sync` 的 axum Response body 跨 await 持有。
#[derive(Clone, Copy)]
enum RoutingOutcome {
    /// 成功：清零失败计数、解除拉黑。
    Success,
    /// 上游故障（5xx / 429 / 408 / 504）：计入熔断。
    UpstreamFailure,
    /// 客户端错误等：不影响熔断健康。
    Ignore,
}

fn classify_outcome<T>(result: &Result<T, GatewayError>) -> RoutingOutcome {
    match result {
        Ok(_) => RoutingOutcome::Success,
        Err(e) if is_circuit_breaker_failure(e.status) => RoutingOutcome::UpstreamFailure,
        Err(_) => RoutingOutcome::Ignore,
    }
}

/// 根据上游调用结果更新渠道熔断健康：成功清零、可熔断失败累加。
async fn record_routing_outcome(state: &SharedState, route_id: &str, outcome: RoutingOutcome) {
    let mut routing = state.ai_gateway_routing.lock().await;
    match outcome {
        RoutingOutcome::Success => routing.record_success(route_id),
        RoutingOutcome::UpstreamFailure => routing.record_failure(route_id, Instant::now()),
        RoutingOutcome::Ignore => {}
    }
}

/// 该 HTTP 状态是否属于「上游渠道故障」，应计入熔断。
fn is_circuit_breaker_failure(status: axum::http::StatusCode) -> bool {
    status.is_server_error()
        || status == axum::http::StatusCode::TOO_MANY_REQUESTS
        || status == axum::http::StatusCode::REQUEST_TIMEOUT
}

fn deserialize_gateway_request(raw_body: serde_json::Value) -> Result<GatewayRequest, String> {
    let deserializer = raw_body.into_deserializer();
    serde_path_to_error::deserialize(deserializer).map_err(|err| err.to_string())
}

fn filter_image_generation_tools(raw_body: &mut serde_json::Value, filter_enabled: bool) {
    if !filter_enabled {
        return;
    }

    let Some(tools) = raw_body
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    tools.retain(|tool| !is_image_generation_tool(tool));
}

fn is_image_generation_tool(tool: &serde_json::Value) -> bool {
    tool.get("type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|tool_type| tool_type == "image_generation")
}

#[derive(Debug, Clone, Deserialize)]
struct GatewayRequestEnvelope {
    model: String,
    #[serde(default)]
    stream: bool,
    #[serde(default)]
    prompt_cache_key: Option<String>,
}

/// GET /ai-gateway/v1/models
pub async fn handle_models(State(state): State<SharedState>) -> impl IntoResponse {
    let config = state.config.lock().await;
    let gw_config = config.ai_gateway.clone();
    drop(config);

    let (models, models_etag) = configured_models_response_with_etag(&gw_config);
    let mut response = Json(models).into_response();
    set_etag_header(&mut response, &models_etag);
    response
}

#[derive(Debug, Deserialize)]
pub struct RequestLogsQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ClearOldRequestLogsQuery {
    days: Option<u64>,
}

/// GET /ai-gateway/request-logs
pub async fn handle_request_logs(
    State(state): State<SharedState>,
    Query(query): Query<RequestLogsQuery>,
) -> impl IntoResponse {
    match state
        .ai_gateway_request_logs
        .list_recent(query.limit.unwrap_or(200))
    {
        Ok(logs) => Json(json!({ "logs": logs })).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE /ai-gateway/request-logs
pub async fn handle_clear_request_logs(State(state): State<SharedState>) -> impl IntoResponse {
    match state.ai_gateway_request_logs.delete_all() {
        Ok(deleted) => Json(json!({ "deleted": deleted })).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE /ai-gateway/request-logs/old?days=3
pub async fn handle_clear_old_request_logs(
    State(state): State<SharedState>,
    Query(query): Query<ClearOldRequestLogsQuery>,
) -> impl IntoResponse {
    let days = query.days.unwrap_or(3).clamp(1, 3650);
    let cutoff_ms = request_log::now_ms().saturating_sub((days as i64) * 24 * 60 * 60 * 1000);
    match state.ai_gateway_request_logs.delete_older_than(cutoff_ms) {
        Ok(deleted) => Json(json!({ "deleted": deleted })).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

/// GET /ai-gateway/request-logs/{id}
pub async fn handle_request_log_detail(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.ai_gateway_request_logs.get_detail(id) {
        Ok(Some(log)) => Json(json!({ "log": log })).into_response(),
        Ok(None) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({ "error": "request log not found" })),
        )
            .into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

fn insert_initial_log(
    store: &RequestLogStore,
    ctx: &GatewayContext,
    headers: &HeaderMap,
    request: &GatewayRequestEnvelope,
    provider: Option<&ProviderConfig>,
    raw_body: &serde_json::Value,
    started_at: Instant,
    created_at_ms: i64,
    details_enabled: bool,
) -> Option<RequestLogContext> {
    let record = RequestLogRecord {
        request_id: ctx.request_id.clone(),
        model_id: request.model.clone(),
        stream: request.stream,
        channel: provider
            .map(|provider| provider.name.clone())
            .unwrap_or_else(|| "-".to_string()),
        provider_type: provider
            .map(|provider| provider_type_key(&provider.provider_type).to_string())
            .unwrap_or_else(|| "-".to_string()),
        status: "running".to_string(),
        usage: Default::default(),
        cost_usd: None,
        latency_ms: None,
        ttft_ms: None,
        created_at_ms,
        error_message: None,
        request_headers_json: details_enabled
            .then(|| request_log::headers_to_json(headers))
            .flatten(),
        request_json: details_enabled
            .then(|| serde_json::to_string(raw_body).ok())
            .flatten(),
        upstream_request_body_bytes: None,
        upstream_request_headers_json: None,
        upstream_request_json: None,
        upstream_response_sse: None,
        response_json: None,
    };
    match store.insert_record(&record) {
        Ok(log_id) => Some(RequestLogContext {
            store: store.clone(),
            log_id,
            started_at,
            details_enabled,
        }),
        Err(err) => {
            request_log::log_insert_error(err);
            None
        }
    }
}

fn update_failed_log(log_context: &Option<RequestLogContext>, message: &str) {
    let Some(log_context) = log_context else {
        return;
    };
    let update = RequestLogUpdate {
        status: Some("failed".to_string()),
        latency_ms: Some(request_log::elapsed_ms(log_context.started_at)),
        error_message: Some(message.to_string()),
        ..RequestLogUpdate::default()
    };
    if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
        request_log::log_update_error(err);
    }
}

fn provider_type_key(provider_type: &ProviderType) -> &'static str {
    match provider_type {
        ProviderType::OpenAiResponses => "responses",
        ProviderType::GrokResponses => "grok_responses",
        ProviderType::ChatCompletions => "chat_completions",
        ProviderType::AnthropicMessages => "anthropic_messages",
    }
}

fn set_models_etag_header(response: &mut axum::response::Response, etag: &str) {
    response.headers_mut().insert(
        HeaderName::from_static("x-models-etag"),
        HeaderValue::from_str(etag).expect("models etag should be a valid header value"),
    );
}

fn set_etag_header(response: &mut axum::response::Response, etag: &str) {
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(etag).expect("models etag should be a valid header value"),
    );
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        GatewayRequestEnvelope, deserialize_gateway_request, filter_image_generation_tools,
    };

    #[test]
    fn filter_image_generation_tools_removes_only_builtin_image_tool() {
        let mut body = json!({
            "model": "gpt-5.5",
            "tools": [
                {"type": "image_generation", "output_format": "png"},
                {"type": "web_search"},
                {"type": "function", "name": "apply_patch"}
            ]
        });

        filter_image_generation_tools(&mut body, true);

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["type"], "web_search");
        assert_eq!(tools[1]["type"], "function");
    }

    #[test]
    fn filter_image_generation_tools_keeps_tools_when_filter_disabled() {
        let mut body = json!({
            "model": "gpt-5.5",
            "tools": [{"type": "image_generation", "output_format": "png"}]
        });

        filter_image_generation_tools(&mut body, false);

        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn responses_passthrough_envelope_accepts_future_payload_shapes() {
        let raw = json!({
            "model": "gpt-5.4",
            "stream": true,
            "prompt_cache_key": "thread-1",
            "previous_response_id": {
                "id": "resp_123"
            },
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": "preview this image"
                        },
                        {
                            "type": "input_image",
                            "image_url": {
                                "url": "data:image/png;base64,abc",
                                "detail": "high"
                            }
                        }
                    ]
                }
            ],
            "unknown_future_field": {
                "nested": {
                    "shape": "must not block Responses passthrough"
                }
            }
        });

        let envelope: GatewayRequestEnvelope =
            serde_json::from_value(raw.clone()).expect("envelope should parse");
        assert_eq!(envelope.model, "gpt-5.4");
        assert!(envelope.stream);
        assert_eq!(envelope.prompt_cache_key.as_deref(), Some("thread-1"));

        let full_parse = deserialize_gateway_request(raw).expect_err(
            "full GatewayRequest parsing should still reject incompatible Responses-only fields",
        );
        assert!(
            full_parse.contains("previous_response_id"),
            "expected field path in error, got: {full_parse}"
        );
    }
}

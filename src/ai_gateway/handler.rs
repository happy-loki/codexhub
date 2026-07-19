use std::{
    io::{Cursor, Read},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, RawQuery, State},
    http::{
        HeaderMap, HeaderName, HeaderValue,
        header::{CONTENT_ENCODING, ETAG},
    },
    response::{IntoResponse, Response},
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
use super::providers::{
    anthropic_messages, deepseek_chat, openai_alpha_search, openai_images, openai_responses,
};
use super::request_log::{
    self, RequestLogContext, RequestLogRecord, RequestLogStore, RequestLogUpdate,
};
use super::responses_lite_tools::prepare_for_provider;
use super::router::{resolve_provider_with_state, resolve_provider_with_state_for_type};

static AI_GATEWAY_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
const MAX_DECOMPRESSED_REQUEST_BODY_BYTES: usize = 512 * 1024 * 1024;

/// POST /ai-gateway/v1/alpha/search
pub async fn handle_alpha_search(
    State(state): State<SharedState>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let started_at = Instant::now();
    let created_at_ms = request_log::now_ms();
    let body = match decode_request_body(&headers, body) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let config = state.config.lock().await;
    let gateway_config = config.ai_gateway.clone();
    let request_logging_enabled = gateway_config.request_logging_enabled;
    let request_log_details_enabled = gateway_config.request_log_details_enabled;
    drop(config);

    let inspection = match openai_alpha_search::inspect_request(&body) {
        Ok(inspection) => inspection,
        Err(error) => return error.into_response(),
    };
    let request_model = inspection.model.clone();
    let context = GatewayContext::extract(&headers, None);
    let routing_session_id = context
        .session_id
        .as_deref()
        .or(inspection.session_id.as_deref());
    let routing_now = Instant::now();
    let (provider, route_id) = {
        let mut routing = state.ai_gateway_routing.lock().await;
        routing.evict_stale(routing_now);
        match resolve_provider_with_state_for_type(
            &request_model,
            routing_session_id,
            &gateway_config,
            &mut routing,
            routing_now,
            &ProviderType::OpenAiResponses,
        ) {
            Ok(result) => result,
            Err(error) => {
                drop(routing);
                let log_context = request_logging_enabled
                    .then(|| {
                        insert_initial_alpha_search_log(
                            &state.ai_gateway_request_logs,
                            &context,
                            &headers,
                            &request_model,
                            None,
                            &inspection.body,
                            started_at,
                            created_at_ms,
                            request_log_details_enabled,
                        )
                    })
                    .flatten();
                update_failed_log(&log_context, &error.message);
                return error.into_response();
            }
        }
    };
    let upstream_model = provider
        .resolve_upstream_model(&request_model)
        .unwrap_or(request_model.as_str())
        .to_string();

    info!(
        model = %request_model,
        upstream_model = %upstream_model,
        provider = %provider.name,
        "ai-gateway alpha search request routed"
    );

    let log_context = request_logging_enabled
        .then(|| {
            insert_initial_alpha_search_log(
                &state.ai_gateway_request_logs,
                &context,
                &headers,
                &request_model,
                Some(provider),
                &inspection.body,
                started_at,
                created_at_ms,
                request_log_details_enabled,
            )
        })
        .flatten();
    let client = crate::outbound_http::get();
    let result = openai_alpha_search::passthrough(
        &client,
        &context,
        inspection.body,
        &upstream_model,
        provider,
        raw_query.as_deref(),
        log_context.clone(),
    )
    .await;
    let outcome = match &result {
        Ok(response) => classify_response_status(response.status()),
        Err(error) if is_circuit_breaker_failure(error.status) => RoutingOutcome::UpstreamFailure,
        Err(_) => RoutingOutcome::Ignore,
    };
    record_routing_outcome(&state, &route_id, outcome).await;
    match result {
        Ok(response) => response,
        Err(error) => {
            update_failed_log(&log_context, &error.message);
            error.into_response()
        }
    }
}

/// POST /ai-gateway/v1/images/generations
pub async fn handle_image_generations(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle_image_request(
        state,
        headers,
        body,
        openai_images::ImageEndpoint::Generations,
    )
    .await
}

/// POST /ai-gateway/v1/images/edits
pub async fn handle_image_edits(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    handle_image_request(state, headers, body, openai_images::ImageEndpoint::Edits).await
}

async fn handle_image_request(
    state: SharedState,
    headers: HeaderMap,
    body: Bytes,
    endpoint: openai_images::ImageEndpoint,
) -> Response {
    let started_at = Instant::now();
    let created_at_ms = request_log::now_ms();
    let config = state.config.lock().await;
    let gateway_config = config.ai_gateway.clone();
    let request_logging_enabled = gateway_config.request_logging_enabled;
    let request_log_details_enabled = gateway_config.request_log_details_enabled;
    drop(config);

    let inspection = match openai_images::inspect_request(&body, endpoint) {
        Ok(inspection) => inspection,
        Err(error) => return error.into_response(),
    };
    let request_model = inspection.model.clone();

    let context = GatewayContext::extract(&headers, None);
    let routing_now = Instant::now();
    let (provider, route_id) = {
        let mut routing = state.ai_gateway_routing.lock().await;
        routing.evict_stale(routing_now);
        match resolve_provider_with_state(
            &request_model,
            context.session_id.as_deref(),
            &gateway_config,
            &mut routing,
            routing_now,
        ) {
            Ok(result) => result,
            Err(error) => {
                drop(routing);
                let log_context = request_logging_enabled
                    .then(|| {
                        insert_initial_image_log(
                            &state.ai_gateway_request_logs,
                            &context,
                            &headers,
                            &request_model,
                            None,
                            endpoint,
                            &inspection.sanitized_log,
                            started_at,
                            created_at_ms,
                            request_log_details_enabled,
                        )
                    })
                    .flatten();
                update_failed_log(&log_context, &error.message);
                return error.into_response();
            }
        }
    };
    let upstream_model = provider
        .resolve_upstream_model(&request_model)
        .unwrap_or(request_model.as_str())
        .to_string();

    info!(
        model = %request_model,
        upstream_model = %upstream_model,
        provider = %provider.name,
        endpoint = endpoint.path(),
        "ai-gateway image request routed"
    );

    let log_context = request_logging_enabled
        .then(|| {
            insert_initial_image_log(
                &state.ai_gateway_request_logs,
                &context,
                &headers,
                &request_model,
                Some(provider),
                endpoint,
                &inspection.sanitized_log,
                started_at,
                created_at_ms,
                request_log_details_enabled,
            )
        })
        .flatten();
    let upstream_request_log = request_log_details_enabled.then(|| {
        let mut value = inspection.sanitized_log.clone();
        value["model"] = json!(upstream_model);
        value
    });

    let client = crate::outbound_http::get();
    let result = openai_images::passthrough(
        &client,
        &context,
        body,
        &request_model,
        &upstream_model,
        provider,
        endpoint,
        log_context.clone(),
        upstream_request_log,
    )
    .await;
    let outcome = classify_outcome(&result);
    record_routing_outcome(&state, &route_id, outcome).await;
    match result {
        Ok(response) => response,
        Err(error) => {
            update_failed_log(&log_context, &error.message);
            error.into_response()
        }
    }
}

/// POST /ai-gateway/v1/responses/compact
pub async fn handle_responses_compact(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let started_at = Instant::now();
    let created_at_ms = request_log::now_ms();
    let config = state.config.lock().await;
    let gw_config = config.ai_gateway.clone();
    let request_logging_enabled = gw_config.request_logging_enabled;
    let request_log_details_enabled = gw_config.request_log_details_enabled;
    let models_etag = configured_models_etag(&gw_config);
    drop(config);
    let in_flight = AI_GATEWAY_IN_FLIGHT.fetch_add(1, Ordering::AcqRel) + 1;
    let _in_flight_guard = AiGatewayInFlightGuard;

    let body = match decode_request_body(&headers, body) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let raw_body: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return GatewayError::bad_request(format!("invalid JSON: {error}")).into_response();
        }
    };
    let envelope: GatewayRequestEnvelope = match serde_json::from_value(raw_body.clone()) {
        Ok(envelope) => envelope,
        Err(error) => {
            return GatewayError::bad_request(format!("invalid compact request envelope: {error}"))
                .into_response();
        }
    };
    if envelope.stream {
        return GatewayError::bad_request("responses compact does not support streaming")
            .into_response();
    }

    let ctx = GatewayContext::extract(&headers, envelope.prompt_cache_key.as_deref());
    let routing_now = Instant::now();
    let route_started = Instant::now();
    let (provider, route_id) = {
        let mut routing = state.ai_gateway_routing.lock().await;
        routing.evict_stale(routing_now);
        match resolve_provider_with_state_for_type(
            &envelope.model,
            ctx.session_id.as_deref(),
            &gw_config,
            &mut routing,
            routing_now,
            &ProviderType::OpenAiResponses,
        ) {
            Ok(result) => result,
            Err(error) => {
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
                update_failed_log(&log_context, &error.message);
                return error.into_response();
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
    let upstream_model = provider
        .resolve_upstream_model(&envelope.model)
        .unwrap_or(envelope.model.as_str())
        .to_string();

    info!(
        model = %envelope.model,
        upstream_model = %upstream_model,
        provider = %provider.name,
        session_id = ?ctx.session_id,
        prompt_cache_key = %ctx.prompt_cache_key,
        in_flight,
        route_ms,
        details = request_log_details_enabled,
        "ai-gateway compact request routed"
    );

    let http_client = crate::outbound_http::get();
    let result = openai_responses::passthrough_compact(
        &http_client,
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
        Ok(mut response) => {
            set_models_etag_header(&mut response, &models_etag);
            response
        }
        Err(error) => {
            update_failed_log(&log_context, &error.message);
            error.into_response()
        }
    }
}

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
    let body = match decode_request_body(&headers, body) {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let mut raw_body: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return GatewayError::bad_request(format!("invalid JSON: {e}")).into_response();
        }
    };
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

    // Preserve request_json as the Codex input; mutations below belong in upstream_request_json.
    filter_image_generation_tools(&mut raw_body, filter_image_generation_tool);

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
    // Responses Lite rejects hosted tools such as web_search:
    // "only supports function tools, custom tools, and client-executed tool search."
    // Never inject hosted web_search into Lite requests; strip if present.
    let stripped_hosted_web_search =
        strip_hosted_web_search_from_lite_request_tools(&mut raw_body, &provider.provider_type);
    if stripped_hosted_web_search > 0 {
        info!(
            model = %envelope.model,
            provider = %provider.name,
            stripped = stripped_hosted_web_search,
            "stripped hosted web_search from Responses Lite request tools"
        );
    }
    let tool_preparation = match prepare_for_provider(&mut raw_body, &provider.provider_type) {
        Ok(preparation) => preparation,
        Err(error) => {
            update_failed_log(&log_context, &format!("invalid Responses tools: {error}"));
            return GatewayError::bad_request(format!("invalid Responses tools: {error}"))
                .into_response();
        }
    };
    if tool_preparation.changed() {
        info!(
            model = %envelope.model,
            provider = %provider.name,
            carriers_removed = tool_preparation.carriers_removed,
            tools_added = tool_preparation.tools_added,
            duplicates_removed = tool_preparation.duplicates_removed,
            grok_tools_converted = tool_preparation.grok_tools_converted,
            grok_hosted_tools_normalized = tool_preparation.grok_hosted_tools_normalized,
            "prepared Responses tools for upstream provider"
        );
    }
    let grok_tool_names = tool_preparation.grok_tool_names;
    let http_client = crate::outbound_http::get();
    match provider.provider_type {
        ProviderType::OpenAiResponses | ProviderType::GrokResponses => {
            let result = openai_responses::passthrough_with_tool_names(
                &http_client,
                &ctx,
                raw_body,
                &upstream_model,
                provider,
                grok_tool_names,
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
                &http_client,
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
                &http_client,
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

fn decode_request_body(headers: &HeaderMap, body: Bytes) -> Result<Bytes, GatewayError> {
    let mut encodings = Vec::new();
    for value in headers.get_all(CONTENT_ENCODING) {
        let value = value
            .to_str()
            .map_err(|_| GatewayError::bad_request("invalid Content-Encoding header"))?;
        encodings.extend(
            value.split(',').map(str::trim).filter(|encoding| {
                !encoding.is_empty() && !encoding.eq_ignore_ascii_case("identity")
            }),
        );
    }

    if encodings.is_empty() {
        return Ok(body);
    }
    if encodings.len() != 1 || !encodings[0].eq_ignore_ascii_case("zstd") {
        return Err(GatewayError::bad_request(format!(
            "unsupported Content-Encoding: {}",
            encodings.join(", ")
        )));
    }

    let decoder =
        zstd::stream::read::Decoder::new(Cursor::new(body.as_ref())).map_err(|error| {
            GatewayError::bad_request(format!("invalid zstd request body: {error}"))
        })?;
    let mut decoded = Vec::new();
    decoder
        .take((MAX_DECOMPRESSED_REQUEST_BODY_BYTES as u64) + 1)
        .read_to_end(&mut decoded)
        .map_err(|error| {
            GatewayError::bad_request(format!("invalid zstd request body: {error}"))
        })?;
    if decoded.len() > MAX_DECOMPRESSED_REQUEST_BODY_BYTES {
        return Err(GatewayError::bad_request(format!(
            "decompressed request body exceeds {} bytes",
            MAX_DECOMPRESSED_REQUEST_BODY_BYTES
        )));
    }
    Ok(Bytes::from(decoded))
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

fn classify_response_status(status: axum::http::StatusCode) -> RoutingOutcome {
    if status.is_success() {
        RoutingOutcome::Success
    } else if is_circuit_breaker_failure(status) {
        RoutingOutcome::UpstreamFailure
    } else {
        RoutingOutcome::Ignore
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

    if let Some(tools) = raw_body
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
    {
        filter_image_generation_tool_array(tools);
    }

    let Some(input) = raw_body
        .get_mut("input")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

    for item in input {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("additional_tools") {
            continue;
        }
        if let Some(tools) = item
            .get_mut("tools")
            .and_then(serde_json::Value::as_array_mut)
        {
            filter_image_generation_tool_array(tools);
        }
    }
}

fn filter_image_generation_tool_array(tools: &mut Vec<serde_json::Value>) {
    for tool in tools.iter_mut() {
        strip_image_generation_from_code_mode_exec(tool);
    }
    tools.retain(|tool| !is_image_generation_tool(tool));
}

fn is_image_generation_tool(tool: &serde_json::Value) -> bool {
    let tool_type = tool.get("type").and_then(serde_json::Value::as_str);
    let name = tool.get("name").and_then(serde_json::Value::as_str);
    let namespace = tool.get("namespace").and_then(serde_json::Value::as_str);

    tool_type == Some("image_generation")
        || (tool_type == Some("namespace") && name == Some("image_gen"))
        || matches!(name, Some("image_gen__imagegen" | "image_gen.imagegen"))
        || (namespace == Some("image_gen") && name == Some("imagegen"))
}

fn strip_image_generation_from_code_mode_exec(tool: &mut serde_json::Value) {
    if tool.get("type").and_then(serde_json::Value::as_str) != Some("custom")
        || tool.get("name").and_then(serde_json::Value::as_str) != Some("exec")
    {
        return;
    }

    let Some(description) = tool.get_mut("description") else {
        return;
    };
    let Some(filtered) = description
        .as_str()
        .and_then(|description| remove_markdown_h2_section(description, "image_gen"))
    else {
        return;
    };
    *description = serde_json::Value::String(filtered);
}

fn remove_markdown_h2_section(text: &str, heading: &str) -> Option<String> {
    let target = format!("## {heading}");
    let mut section_start = None;
    let mut offset = 0;

    for segment in text.split_inclusive('\n') {
        let line_start = offset;
        offset += segment.len();
        let line = segment.trim_end_matches(&['\r', '\n'][..]);

        if let Some(start) = section_start {
            if line.starts_with("## ") {
                let mut filtered = String::with_capacity(text.len() - (line_start - start));
                filtered.push_str(&text[..start]);
                filtered.push_str(&text[line_start..]);
                return Some(filtered);
            }
        } else if line == target {
            section_start = Some(line_start);
        }
    }

    section_start.map(|start| {
        let prefix = &text[..start];
        let start = if prefix.ends_with("\r\n\r\n") {
            start - 4
        } else if prefix.ends_with("\n\n") {
            start - 2
        } else {
            start
        };
        text[..start].to_string()
    })
}

/// Responses Lite upstream rejects hosted tools (`web_search`, etc.) with:
/// `X-OpenAI-Internal-Codex-Responses-Lite only supports function tools,
/// custom tools, and client-executed tool search.`
///
/// Strip any hosted web_search entries from both top-level `tools` and Lite
/// `input[].additional_tools.tools`.
/// Do not inject hosted web_search as a compatibility path for Lite.
fn strip_hosted_web_search_from_lite_request_tools(
    raw_body: &mut serde_json::Value,
    provider_type: &ProviderType,
) -> usize {
    if provider_type != &ProviderType::OpenAiResponses || !is_responses_lite_request(raw_body) {
        return 0;
    }

    let mut stripped = raw_body
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
        .map(strip_hosted_web_search_tools)
        .unwrap_or(0);

    if let Some(input) = raw_body
        .get_mut("input")
        .and_then(serde_json::Value::as_array_mut)
    {
        for item in input {
            if item.get("type").and_then(serde_json::Value::as_str) != Some("additional_tools") {
                continue;
            }
            stripped += item
                .get_mut("tools")
                .and_then(serde_json::Value::as_array_mut)
                .map(strip_hosted_web_search_tools)
                .unwrap_or(0);
        }
    }

    stripped
}

fn strip_hosted_web_search_tools(tools: &mut Vec<serde_json::Value>) -> usize {
    let before = tools.len();
    tools.retain(|tool| !is_hosted_web_search_tool(tool));
    before.saturating_sub(tools.len())
}

fn is_responses_lite_request(raw_body: &serde_json::Value) -> bool {
    raw_body
        .get("input")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|input| {
            input.iter().any(|item| {
                item.get("type").and_then(serde_json::Value::as_str) == Some("additional_tools")
                    && item.get("tools").is_some_and(serde_json::Value::is_array)
            })
        })
}

fn is_hosted_web_search_tool(tool: &serde_json::Value) -> bool {
    matches!(
        tool.get("type").and_then(serde_json::Value::as_str),
        Some("web_search") | Some("web_search_preview")
    )
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
    let store = state.ai_gateway_request_logs.clone();
    match run_request_log_cleanup(move || store.delete_all()).await {
        Ok(deleted) => Json(json!({ "deleted": deleted })).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err })),
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
    let store = state.ai_gateway_request_logs.clone();
    match run_request_log_cleanup(move || store.delete_older_than(cutoff_ms)).await {
        Ok(deleted) => Json(json!({ "deleted": deleted })).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err })),
        )
            .into_response(),
    }
}

async fn run_request_log_cleanup(
    operation: impl FnOnce() -> rusqlite::Result<usize> + Send + 'static,
) -> Result<usize, String> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|err| format!("request log cleanup task failed: {err}"))?
        .map_err(|err| err.to_string())
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

fn insert_initial_image_log(
    store: &RequestLogStore,
    ctx: &GatewayContext,
    headers: &HeaderMap,
    model: &str,
    provider: Option<&ProviderConfig>,
    endpoint: openai_images::ImageEndpoint,
    sanitized_body: &serde_json::Value,
    started_at: Instant,
    created_at_ms: i64,
    details_enabled: bool,
) -> Option<RequestLogContext> {
    let record = RequestLogRecord {
        request_id: ctx.request_id.clone(),
        model_id: model.to_string(),
        stream: false,
        channel: provider
            .map(|provider| provider.name.clone())
            .unwrap_or_else(|| "-".to_string()),
        provider_type: endpoint.log_type().to_string(),
        status: "running".to_string(),
        usage: Default::default(),
        cost_usd: None,
        latency_ms: None,
        ttft_ms: None,
        created_at_ms,
        error_message: None,
        request_headers_json: details_enabled
            .then(|| request_log::headers_to_redacted_json(headers))
            .flatten(),
        request_json: details_enabled
            .then(|| serde_json::to_string(sanitized_body).ok())
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
        Err(error) => {
            request_log::log_insert_error(error);
            None
        }
    }
}

fn insert_initial_alpha_search_log(
    store: &RequestLogStore,
    ctx: &GatewayContext,
    headers: &HeaderMap,
    model: &str,
    provider: Option<&ProviderConfig>,
    raw_body: &serde_json::Value,
    started_at: Instant,
    created_at_ms: i64,
    details_enabled: bool,
) -> Option<RequestLogContext> {
    let record = RequestLogRecord {
        request_id: ctx.request_id.clone(),
        model_id: model.to_string(),
        stream: false,
        channel: provider
            .map(|provider| provider.name.clone())
            .unwrap_or_else(|| "-".to_string()),
        provider_type: "alpha_search".to_string(),
        status: "running".to_string(),
        usage: Default::default(),
        cost_usd: None,
        latency_ms: None,
        ttft_ms: None,
        created_at_ms,
        error_message: None,
        request_headers_json: details_enabled
            .then(|| request_log::headers_to_redacted_json(headers))
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
        Err(error) => {
            request_log::log_insert_error(error);
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
    use std::io::Cursor;
    use std::time::Instant;

    use axum::{
        body::Bytes,
        http::{HeaderMap, HeaderValue, header::CONTENT_ENCODING},
    };
    use serde_json::json;

    use crate::ai_gateway::config::{ProviderConfig, ProviderType};
    use crate::ai_gateway::context::GatewayContext;
    use crate::ai_gateway::providers::openai_images;
    use crate::ai_gateway::request_log::RequestLogStore;

    use super::{
        GatewayRequestEnvelope, decode_request_body, deserialize_gateway_request,
        filter_image_generation_tools, insert_initial_image_log,
        strip_hosted_web_search_from_lite_request_tools,
    };

    fn lite_request(model: &str) -> serde_json::Value {
        json!({
            "model": model,
            "input": [
                {
                    "type": "additional_tools",
                    "role": "developer",
                    "tools": [
                        {"type": "custom", "name": "exec"},
                        {"type": "function", "name": "request_user_input"}
                    ]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "latest Rust news"}]
                }
            ]
        })
    }

    #[test]
    fn decode_request_body_accepts_codex_zstd_payload() {
        let original = br#"{"model":"gpt-5.6-sol","stream":true,"input":"hello"}"#;
        let compressed =
            zstd::stream::encode_all(Cursor::new(original), 3).expect("compress request body");
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("zstd"));

        let decoded = decode_request_body(&headers, Bytes::from(compressed))
            .expect("decode zstd request body");

        assert_eq!(decoded.as_ref(), original);
        serde_json::from_slice::<serde_json::Value>(&decoded).expect("decoded JSON");
    }

    #[test]
    fn decode_request_body_rejects_unknown_content_encoding() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));

        let error = decode_request_body(&headers, Bytes::from_static(b"payload"))
            .expect_err("unsupported encoding should fail");

        assert_eq!(error.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(error.message.contains("unsupported Content-Encoding"));
    }

    #[test]
    fn initial_image_log_is_listed_with_redacted_headers() {
        let db_path = std::env::temp_dir().join(format!(
            "codexhub-image-handler-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let store = RequestLogStore::new(db_path.clone());
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer local-codex-token"),
        );
        let context = GatewayContext::extract(&headers, None);
        let provider = ProviderConfig {
            name: "image-provider".to_string(),
            ..ProviderConfig::default()
        };
        let sanitized = json!({
            "model": "gpt-image-2",
            "images": [{"payloadRedacted": true, "base64Chars": 16}]
        });

        let log_context = insert_initial_image_log(
            &store,
            &context,
            &headers,
            "gpt-image-2",
            Some(&provider),
            openai_images::ImageEndpoint::Edits,
            &sanitized,
            Instant::now(),
            1,
            true,
        )
        .expect("insert initial image log");

        let entries = store.list_recent(10).expect("list image logs");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].model_id, "gpt-image-2");
        assert_eq!(entries[0].channel, "image-provider");
        assert_eq!(entries[0].provider_type, "image_edit");
        let detail = store
            .get_detail(log_context.log_id)
            .expect("read image log")
            .expect("image log exists");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                detail
                    .request_headers_json
                    .as_deref()
                    .expect("request headers")
            )
            .expect("request headers JSON")["authorization"],
            "<redacted>"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                detail.request_json.as_deref().expect("request JSON")
            )
            .expect("request log JSON")["images"][0]["payloadRedacted"],
            true
        );

        drop(log_context);
        drop(store);
        let _ = std::fs::remove_file(db_path);
    }

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
            "tools": [
                {"type": "image_generation", "output_format": "png"},
                {
                    "type": "namespace",
                    "name": "image_gen",
                    "tools": [{"type": "function", "name": "imagegen"}]
                }
            ]
        });

        filter_image_generation_tools(&mut body, false);

        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn filter_image_generation_tools_removes_standalone_namespace_tools() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "tools": [
                {
                    "type": "namespace",
                    "name": "image_gen",
                    "tools": [{"type": "function", "name": "imagegen"}]
                },
                {
                    "type": "namespace",
                    "name": "web",
                    "tools": [{"type": "function", "name": "run"}]
                }
            ],
            "input": [{
                "type": "additional_tools",
                "role": "developer",
                "tools": [
                    {"type": "function", "name": "image_gen__imagegen"},
                    {"type": "function", "name": "request_user_input"}
                ]
            }]
        });

        filter_image_generation_tools(&mut body, true);

        let top_level_tools = body["tools"].as_array().unwrap();
        assert_eq!(top_level_tools.len(), 1);
        assert_eq!(top_level_tools[0]["name"], "web");
        let additional_tools = body["input"][0]["tools"].as_array().unwrap();
        assert_eq!(additional_tools.len(), 1);
        assert_eq!(additional_tools[0]["name"], "request_user_input");
    }

    #[test]
    fn filter_image_generation_tools_removes_code_mode_exec_section() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "input": [{
                "type": "additional_tools",
                "role": "developer",
                "tools": [{
                    "type": "custom",
                    "name": "exec",
                    "description": concat!(
                        "Run JavaScript.\r\n\r\n",
                        "## shell\r\nShell tools.\r\n\r\n",
                        "## image_gen\r\nTools in the image_gen namespace.\r\n\r\n",
                        "### `image_gen__imagegen`\r\nGenerate images.\r\n\r\n",
                        "## codex_app\r\nCodex App tools.\r\n"
                    )
                }]
            }]
        });

        filter_image_generation_tools(&mut body, true);

        let description = body["input"][0]["tools"][0]["description"]
            .as_str()
            .unwrap();
        assert!(description.contains("## shell"));
        assert!(description.contains("## codex_app"));
        assert!(!description.contains("## image_gen"));
        assert!(!description.contains("image_gen__imagegen"));
    }

    #[test]
    fn filter_image_generation_tools_removes_final_code_mode_exec_section() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "input": [
                {
                    "type": "additional_tools",
                    "role": "developer",
                    "tools": [{
                        "type": "custom",
                        "name": "exec",
                        "description": concat!(
                            "Run JavaScript.\n",
                            "- `generatedImage(result)`: Appends a generated image.\n",
                            "- `ALL_TOOLS`: metadata for enabled nested tools.\n\n",
                            "## image_gen\nTools in the image_gen namespace.\n\n",
                            "### `image_gen__imagegen`\nGenerate images."
                        )
                    }]
                },
                {
                    "type": "message",
                    "role": "developer",
                    "content": [{
                        "type": "input_text",
                        "text": concat!(
                            "<skills_instructions>\n",
                            "### Available skills\n",
                            "- imagegen: Generate or edit raster images.\n",
                            "</skills_instructions>"
                        )
                    }]
                }
            ]
        });

        filter_image_generation_tools(&mut body, true);

        assert_eq!(
            body["input"][0]["tools"][0]["description"],
            concat!(
                "Run JavaScript.\n",
                "- `generatedImage(result)`: Appends a generated image.\n",
                "- `ALL_TOOLS`: metadata for enabled nested tools."
            )
        );
        assert!(
            body["input"][1]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("- imagegen: Generate or edit raster images.")
        );
    }

    #[test]
    fn does_not_add_hosted_web_search_to_lite_request_tools() {
        let mut body = lite_request("gpt-5.6-sol");

        assert_eq!(
            strip_hosted_web_search_from_lite_request_tools(
                &mut body,
                &ProviderType::OpenAiResponses,
            ),
            0
        );
        assert!(body.get("tools").is_none());
        assert_eq!(body["input"][0]["tools"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn strips_hosted_web_search_from_lite_top_level_tools() {
        let mut body = lite_request("gpt-5.6-terra");
        body["tools"] = json!([
            {
                "type": "function",
                "name": "existing_tool",
                "parameters": {"type": "object", "properties": {}}
            },
            {
                "type": "web_search",
                "external_web_access": true,
                "search_content_types": ["text", "image"]
            },
            {"type": "web_search_preview"}
        ]);

        assert_eq!(
            strip_hosted_web_search_from_lite_request_tools(
                &mut body,
                &ProviderType::OpenAiResponses,
            ),
            2
        );

        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "existing_tool");
        assert_eq!(
            strip_hosted_web_search_from_lite_request_tools(
                &mut body,
                &ProviderType::OpenAiResponses,
            ),
            0
        );
    }

    #[test]
    fn strips_hosted_web_search_from_lite_additional_tools() {
        let mut body = lite_request("gpt-5.6-sol");
        body["input"][0]["tools"] = json!([
            {
                "type": "function",
                "name": "existing_tool",
                "parameters": {"type": "object", "properties": {}}
            },
            {
                "type": "custom",
                "name": "exec",
                "description": "Run JavaScript."
            },
            {
                "type": "tool_search",
                "execution": "client",
                "description": "Search tools",
                "parameters": {"type": "object", "properties": {}}
            },
            {"type": "web_search"},
            {"type": "web_search_preview"}
        ]);

        assert_eq!(
            strip_hosted_web_search_from_lite_request_tools(
                &mut body,
                &ProviderType::OpenAiResponses,
            ),
            2
        );

        let tools = body["input"][0]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[1]["type"], "custom");
        assert_eq!(tools[2]["type"], "tool_search");
        assert_eq!(tools[2]["execution"], "client");
    }

    #[test]
    fn hosted_web_search_lite_strip_is_scoped_to_openai_lite() {
        let mut non_lite = json!({
            "model": "gpt-5.4",
            "input": [{"type": "message", "role": "user", "content": []}],
            "tools": [{"type": "web_search"}]
        });
        let mut grok_lite = lite_request("gpt-5.6-luna");
        grok_lite["tools"] = json!([{"type": "web_search"}]);

        assert_eq!(
            strip_hosted_web_search_from_lite_request_tools(
                &mut non_lite,
                &ProviderType::OpenAiResponses,
            ),
            0
        );
        assert_eq!(non_lite["tools"].as_array().unwrap().len(), 1);
        assert_eq!(
            strip_hosted_web_search_from_lite_request_tools(
                &mut grok_lite,
                &ProviderType::GrokResponses,
            ),
            0
        );
        assert_eq!(grok_lite["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn hosted_web_search_lite_strip_requires_additional_tools() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "input": [{"type": "message", "role": "user", "content": []}],
            "tools": [
                {"type": "function", "name": "exec"},
                {"type": "web_search"}
            ]
        });

        assert_eq!(
            strip_hosted_web_search_from_lite_request_tools(
                &mut body,
                &ProviderType::OpenAiResponses,
            ),
            0
        );
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
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

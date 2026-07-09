//! DeepSeek Chat Completions 出站 provider。
//! 参考 AxonHub `deepseek/outbound.go`。

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use tracing::{debug, error};

use crate::ai_gateway::config::{ProviderConfig, provider_api_root};
use crate::ai_gateway::context::{GatewayContext, apply_upstream_headers};
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::GatewayRequest;
use crate::ai_gateway::request_log::{
    self, RequestLogContext, RequestLogUpdate, ResponsesSseLogStream, UpstreamSseCaptureStream,
};
use crate::ai_gateway::tool_names::ToolNameMap;
use crate::ai_gateway::transform::chat_to_responses::convert_chat_response_with_tool_names;
use crate::ai_gateway::transform::responses_stream::ChatSseToResponsesSse;
use crate::ai_gateway::transform::responses_to_chat::build_chat_request_with_tool_names;

use super::{
    apply_total_request_timeout, ensure_success_response, execute_stream_start,
    execute_upstream_request,
};

/// DeepSeek Chat Completions 出站处理。
pub async fn handle(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    response_model: &str,
    provider: &ProviderConfig,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    // 1. Responses → Chat Completions 请求转换
    let (chat_body, tool_name_map) = build_chat_request_with_tool_names(request, true)
        .map_err(|e| GatewayError::bad_request(format!("transform error: {e}")))?;

    let url = format!(
        "{}/v1/chat/completions",
        provider_api_root(&provider.base_url)
    );

    debug!(url = %url, stream = request.stream, "proxying to deepseek chat");

    // 2. 发送上游请求
    let req_builder = client
        .post(&url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", provider.api_key));
    let req_builder =
        apply_total_request_timeout(req_builder, provider.timeout_secs, request.stream)
            .json(&chat_body);
    let upstream_req = apply_upstream_headers(req_builder, &ctx.upstream_headers)
        .build()
        .map_err(|e| {
            error!(error = %e, "build deepseek upstream request failed");
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
            upstream_request_body_bytes: request_log::json_body_size_bytes(&chat_body),
            upstream_request_json: log_context
                .details_enabled
                .then(|| serde_json::to_string(&chat_body).ok())
                .flatten(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(err);
        }
    }

    let upstream_resp = if request.stream {
        execute_stream_start(
            client,
            upstream_req,
            provider.timeout_secs,
            "deepseek upstream request failed",
        )
        .await?
    } else {
        execute_upstream_request(
            client,
            upstream_req,
            provider.timeout_secs,
            "deepseek upstream request failed",
        )
        .await?
    };

    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;

    // 3. 流式 vs 非流式
    if request.stream {
        handle_stream(upstream_resp, response_model, tool_name_map, log_context).await
    } else {
        handle_non_stream(upstream_resp, response_model, tool_name_map, log_context).await
    }
}

/// 非流式：Chat JSON → Responses JSON。
async fn handle_non_stream(
    resp: reqwest::Response,
    model: &str,
    tool_name_map: ToolNameMap,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let chat_resp: serde_json::Value = resp.json().await.map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("parse upstream json: {e}"))
    })?;

    let response_obj = convert_chat_response_with_tool_names(&chat_resp, model, &tool_name_map)
        .map_err(|e| {
            GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("transform error: {e}"))
        })?;

    let body_bytes = serde_json::to_vec(&response_obj).unwrap_or_default();
    if let Some(log_context) = &log_context {
        let response_value = serde_json::to_value(&response_obj).unwrap_or_default();
        let update = RequestLogUpdate {
            status: Some(response_obj.status.clone()),
            usage: Some(request_log::usage_from_response_value(&response_value)),
            latency_ms: Some(request_log::elapsed_ms(log_context.started_at)),
            response_json: log_context
                .details_enabled
                .then(|| serde_json::to_string(&response_value).ok())
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

/// 流式：Chat SSE → Responses SSE（通过状态机转换）。
async fn handle_stream(
    resp: reqwest::Response,
    model: &str,
    tool_name_map: ToolNameMap,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let model = model.to_string();
    let upstream_bytes = resp.bytes_stream();
    let body = if let Some(log_context) = log_context {
        let captured_upstream = UpstreamSseCaptureStream::new(upstream_bytes, log_context.clone());
        let sse_stream =
            ChatSseToResponsesSse::new_with_tool_names(captured_upstream, model, tool_name_map);
        Body::from_stream(ResponsesSseLogStream::new(sse_stream, log_context))
    } else {
        let sse_stream =
            ChatSseToResponsesSse::new_with_tool_names(upstream_bytes, model, tool_name_map);
        Body::from_stream(sse_stream)
    };
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("text/event-stream"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-cache"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("connection"),
        HeaderValue::from_static("keep-alive"),
    );
    Ok(response)
}

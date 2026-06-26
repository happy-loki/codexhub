use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use serde_json::json;
use tracing::{debug, error};

use crate::ai_gateway::config::{ProviderConfig, provider_api_root};
use crate::ai_gateway::context::{GatewayContext, apply_upstream_headers};
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::request_log::{
    self, RequestLogContext, RequestLogUpdate, ResponsesSseLogStream,
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
            upstream_request_headers_json: request_log::headers_to_json(upstream_req.headers()),
            upstream_request_body_bytes: request_log::json_body_size_bytes(&raw_body),
            upstream_request_json: serde_json::to_string(&raw_body).ok(),
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
            Body::from_stream(ResponsesSseLogStream::new(
                Box::pin(byte_stream),
                log_context,
            ))
        } else {
            Body::from_stream(byte_stream)
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
    if let Some(log_context) = &log_context {
        let response_json = serde_json::from_slice::<serde_json::Value>(&body_bytes).ok();
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
            response_json: response_text,
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

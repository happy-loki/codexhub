//! Anthropic Messages 出站 provider。

use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use serde_json::Value;
use tracing::{debug, error};

use crate::ai_gateway::config::ProviderConfig;
use crate::ai_gateway::context::{GatewayContext, apply_upstream_headers};
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::GatewayRequest;
use crate::ai_gateway::request_log::{
    self, RequestLogContext, RequestLogUpdate, ResponsesSseLogStream, UpstreamSseCaptureStream,
};
use crate::ai_gateway::tool_names::ToolNameMap;

use super::{
    apply_total_request_timeout, ensure_success_response, execute_stream_start,
    execute_upstream_request,
};

mod citations;
mod custom_tools;
mod glm_compat;
mod options;
mod request;
mod request_content;
mod request_reasoning;
mod request_tools;
mod response;
mod stream;
mod stream_events;
mod stream_items;
mod stream_message;
mod stream_reasoning;
mod stream_response;
mod stream_state;
mod stream_tools;
mod types;

#[cfg(test)]
mod tests;

use options::{
    AnthropicAuthStyle, AnthropicProviderOptions, AnthropicProviderProfile, AnthropicVersionHeader,
};
use request::build_anthropic_request;
use response::convert_anthropic_response;
use stream::AnthropicSseToResponsesSse;

pub async fn handle(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    response_model: &str,
    provider: &ProviderConfig,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let options = AnthropicProviderOptions::from_provider(provider)?;
    let prompt_cache_retention = request
        .prompt_cache_retention
        .as_deref()
        .or(provider.prompt_cache_retention.as_deref());
    let (anthropic_body, tool_name_map) =
        build_anthropic_request(request, options.profile, prompt_cache_retention)?;
    let url = options.messages_url(provider);
    debug!(url = %url, stream = false, "proxying to anthropic messages");

    let req_builder = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&anthropic_body);
    let req_builder = apply_anthropic_auth(req_builder, provider, &options);
    let req_builder = apply_anthropic_version_header(req_builder, &options);
    let upstream_req = apply_upstream_headers(
        apply_total_request_timeout(req_builder, provider.timeout_secs, request.stream),
        &ctx.upstream_headers,
    )
    .build()
    .map_err(|e| {
        error!(error = %e, "build anthropic upstream request failed");
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("build upstream request: {e}"),
        )
    })?;

    if let Some(log_context) = &log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: request_log::headers_to_json(upstream_req.headers()),
            upstream_request_body_bytes: request_log::json_body_size_bytes(&anthropic_body),
            upstream_request_json: serde_json::to_string(&anthropic_body).ok(),
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
            "anthropic upstream request failed",
        )
        .await?
    } else {
        execute_upstream_request(
            client,
            upstream_req,
            provider.timeout_secs,
            "anthropic upstream request failed",
        )
        .await?
    };
    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;

    if request.stream {
        return handle_stream(
            upstream_resp,
            response_model,
            tool_name_map,
            options.profile,
            log_context,
        )
        .await;
    }

    let anthropic_resp: Value = upstream_resp.json().await.map_err(|e| {
        GatewayError::upstream(StatusCode::BAD_GATEWAY, format!("parse upstream json: {e}"))
    })?;
    let response_obj = convert_anthropic_response(
        &anthropic_resp,
        response_model,
        &tool_name_map,
        options.profile,
    );
    let body_bytes = serde_json::to_vec(&response_obj).unwrap_or_default();

    if let Some(log_context) = &log_context {
        let response_value = serde_json::to_value(&response_obj).unwrap_or_default();
        let update = RequestLogUpdate {
            status: Some(response_obj.status.clone()),
            usage: Some(request_log::usage_from_response_value(&response_value)),
            latency_ms: Some(request_log::elapsed_ms(log_context.started_at)),
            response_json: serde_json::to_string(&response_value).ok(),
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

fn apply_anthropic_auth(
    request: reqwest::RequestBuilder,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> reqwest::RequestBuilder {
    match options.auth {
        AnthropicAuthStyle::XApiKey => request.header("x-api-key", provider.api_key.clone()),
    }
}

fn apply_anthropic_version_header(
    request: reqwest::RequestBuilder,
    options: &AnthropicProviderOptions,
) -> reqwest::RequestBuilder {
    match options.version_header {
        AnthropicVersionHeader::Required(version) => request.header("anthropic-version", version),
    }
}

async fn handle_stream(
    resp: reqwest::Response,
    model: &str,
    tool_name_map: ToolNameMap,
    profile: AnthropicProviderProfile,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let upstream_bytes = resp.bytes_stream();
    let body = if let Some(log_context) = log_context {
        let captured_upstream = UpstreamSseCaptureStream::new(upstream_bytes, log_context.clone());
        let sse_stream = AnthropicSseToResponsesSse::new(
            captured_upstream,
            model.to_string(),
            tool_name_map,
            profile,
        );
        Body::from_stream(ResponsesSseLogStream::new(sse_stream, log_context))
    } else {
        let sse_stream = AnthropicSseToResponsesSse::new(
            upstream_bytes,
            model.to_string(),
            tool_name_map,
            profile,
        );
        Body::from_stream(sse_stream)
    };

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

    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    *response.headers_mut() = headers;
    Ok(response)
}

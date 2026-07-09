//! Anthropic Messages 出站 provider。

use std::{collections::HashSet, io};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::Response,
};
use futures_util::{StreamExt, stream as futures_stream};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::{debug, error};

use crate::ai_gateway::config::ProviderConfig;
use crate::ai_gateway::context::GatewayContext;
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::model::{GatewayRequest, generate_response_id};
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
mod stream_internal;
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
    AnthropicAuthStyle, AnthropicHeaderStyle, AnthropicProviderOptions, AnthropicProviderProfile,
    AnthropicVersionHeader,
};
use request::build_anthropic_request;
use response::convert_anthropic_response;
use stream::AnthropicSseToResponsesSse;
use stream_events::unix_timestamp;
use stream_internal::InternalSseEnvelope;
use types::{
    ANTHROPIC_CLAUDE_CODE_BETA, ANTHROPIC_WEB_SEARCH_TYPE, CLAUDE_CODE_STAINLESS_PACKAGE_VERSION,
    CLAUDE_CODE_STAINLESS_RUNTIME_VERSION, CLAUDE_CODE_USER_AGENT,
};

const WEB_SEARCH_TOOL_NAME: &str = "WebSearch";

pub async fn handle(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    response_model: &str,
    provider: &ProviderConfig,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    let options = AnthropicProviderOptions::from_provider(provider)?;
    let (mut anthropic_body, tool_name_map) = build_anthropic_request(request, options.profile)?;
    insert_metadata_user_id(&mut anthropic_body, ctx);
    let has_internal_web_search = has_web_search_client_tool(&anthropic_body);
    let url = options.messages_url(provider);
    debug!(
        url = %url,
        stream = request.stream,
        internal_web_search = has_internal_web_search,
        "proxying to anthropic messages"
    );

    if has_internal_web_search {
        if let Some(response) = handle_with_internal_web_search(
            client,
            ctx,
            request,
            response_model,
            provider,
            &options,
            log_context.clone(),
            anthropic_body.clone(),
            tool_name_map.clone(),
        )
        .await?
        {
            return Ok(response);
        }
        anthropic_body = force_anthropic_stream(anthropic_body, request.stream);
    }

    let upstream_req = build_anthropic_upstream_request(
        client,
        ctx,
        request,
        &anthropic_body,
        provider,
        &options,
    )?;

    if let Some(log_context) = &log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: log_context
                .details_enabled
                .then(|| request_log::headers_to_json(upstream_req.headers()))
                .flatten(),
            upstream_request_body_bytes: request_log::json_body_size_bytes(&anthropic_body),
            upstream_request_json: log_context
                .details_enabled
                .then(|| serde_json::to_string(&anthropic_body).ok())
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

#[allow(clippy::too_many_arguments)]
async fn handle_with_internal_web_search(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    response_model: &str,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
    log_context: Option<RequestLogContext>,
    mut anthropic_body: Value,
    tool_name_map: ToolNameMap,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request.stream {
        return Ok(Some(stream_internal_web_search_response(
            client,
            ctx,
            request,
            response_model,
            provider,
            options,
            log_context,
            anthropic_body,
            tool_name_map,
        )));
    }

    let mut internal_web_search_items = Vec::new();
    loop {
        let step_body = force_anthropic_stream(anthropic_body.clone(), true);
        let step_resp = execute_anthropic_stream_message(
            client,
            ctx,
            request,
            provider,
            options,
            &step_body,
            &log_context,
        )
        .await?;
        let tool_uses = find_web_search_tool_uses(&step_resp);
        if tool_uses.is_empty() {
            let response_obj = convert_anthropic_response(
                &step_resp,
                response_model,
                &tool_name_map,
                options.profile,
            );
            return Ok(Some(response_from_response_object(
                response_obj,
                request.stream,
                &log_context,
                &internal_web_search_items,
            )?));
        }

        let mut tool_results = Vec::new();
        for tool_use in tool_uses {
            internal_web_search_items.push(web_search_response_item(&tool_use));
            let search_text = execute_internal_web_search(
                client,
                ctx,
                request,
                provider,
                options,
                tool_use.query.as_str(),
                &log_context,
            )
            .await?;
            tool_results.push((tool_use.id, search_text));
        }
        append_tool_results(&mut anthropic_body, &step_resp, tool_results);
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_internal_web_search_response(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    response_model: &str,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
    log_context: Option<RequestLogContext>,
    anthropic_body: Value,
    tool_name_map: ToolNameMap,
) -> Response<Body> {
    let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(32);
    let task_log_context = log_context.clone();
    let client = client.clone();
    let ctx = ctx.clone();
    let request = request.clone();
    let response_model = response_model.to_string();
    let provider = provider.clone();
    let options = options.clone();

    tokio::spawn(async move {
        if let Err(err) = run_internal_web_search_stream(
            client,
            ctx,
            request,
            response_model,
            provider,
            options,
            task_log_context,
            anthropic_body,
            tool_name_map,
            tx.clone(),
        )
        .await
        {
            let _ = tx
                .send(Err(io::Error::new(io::ErrorKind::Other, err.message)))
                .await;
        }
    });

    let sse_stream = futures_stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let body = if let Some(log_context) = log_context {
        Body::from_stream(ResponsesSseLogStream::new(
            Box::pin(sse_stream),
            log_context,
        ))
    } else {
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
    response
}

#[allow(clippy::too_many_arguments)]
async fn run_internal_web_search_stream(
    client: reqwest::Client,
    ctx: GatewayContext,
    request: GatewayRequest,
    response_model: String,
    provider: ProviderConfig,
    options: AnthropicProviderOptions,
    log_context: Option<RequestLogContext>,
    mut anthropic_body: Value,
    tool_name_map: ToolNameMap,
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
) -> Result<(), GatewayError> {
    let mut envelope = InternalSseEnvelope::new(
        generate_response_id(),
        response_model.clone(),
        unix_timestamp(),
    );
    // TTFT is captured from the first content token of the first round so the
    // request log reflects the true time-to-first-token.
    let mut ttft_recorded = false;

    loop {
        let step_body = force_anthropic_stream(anthropic_body.clone(), true);
        // Stream this round through the converter so text/reasoning deltas reach
        // the client token-by-token, while capturing the raw Anthropic SSE to
        // detect whether the model asked for a web search.
        let raw_sse = stream_anthropic_round(
            &client,
            &ctx,
            &request,
            &provider,
            &options,
            &step_body,
            &log_context,
            &tool_name_map,
            &mut envelope,
            &tx,
            &mut ttft_recorded,
        )
        .await?;
        let step_resp = anthropic_message_from_sse(&raw_sse)?;
        let tool_uses = find_web_search_tool_uses(&step_resp);
        if tool_uses.is_empty() {
            envelope.finish(&tx).await?;
            return Ok(());
        }

        let mut tool_results = Vec::new();
        for tool_use in tool_uses {
            emit_injected_web_search_call(&mut envelope, &tx, &tool_use).await?;
            let search_text = execute_internal_web_search(
                &client,
                &ctx,
                &request,
                &provider,
                &options,
                tool_use.query.as_str(),
                &log_context,
            )
            .await?;
            tool_results.push((tool_use.id, search_text));
        }
        append_tool_results(&mut anthropic_body, &step_resp, tool_results);
    }
}

/// Streams one upstream Anthropic round through the Responses converter and the
/// shared envelope, forwarding converted content events to the client while
/// accumulating the raw Anthropic SSE. Returns the raw SSE so the caller can
/// detect web-search tool uses and, if needed, run another round.
#[allow(clippy::too_many_arguments)]
async fn stream_anthropic_round(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
    anthropic_body: &Value,
    log_context: &Option<RequestLogContext>,
    tool_name_map: &ToolNameMap,
    envelope: &mut InternalSseEnvelope,
    tx: &mpsc::Sender<Result<Bytes, io::Error>>,
    ttft_recorded: &mut bool,
) -> Result<String, GatewayError> {
    let mut step_request = request.clone();
    step_request.stream = true;
    let upstream_req = build_anthropic_upstream_request(
        client,
        ctx,
        &step_request,
        anthropic_body,
        provider,
        options,
    )?;
    update_upstream_log(log_context, upstream_req.headers(), anthropic_body);
    let upstream_resp = execute_stream_start(
        client,
        upstream_req,
        provider.timeout_secs,
        "anthropic upstream request failed",
    )
    .await?;
    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;

    envelope.ensure_started(tx).await?;
    envelope.begin_round();

    // Accumulate the raw Anthropic SSE for post-round inspection.
    let raw = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let raw_for_stream = raw.clone();
    let byte_stream = upstream_resp
        .bytes_stream()
        .map(move |result| match result {
            Ok(chunk) => {
                if let Ok(mut guard) = raw_for_stream.lock() {
                    guard.push_str(&String::from_utf8_lossy(&chunk));
                }
                Ok(chunk)
            }
            Err(e) => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            )),
        });

    let mut converted = AnthropicSseToResponsesSse::new(
        byte_stream,
        request.model.clone(),
        tool_name_map.clone(),
        options.profile,
    );

    while let Some(frame) = converted.next().await {
        let frame = frame.map_err(|e| {
            GatewayError::upstream(
                StatusCode::BAD_GATEWAY,
                format!("anthropic stream conversion failed: {e}"),
            )
        })?;
        for (event_type, data) in parse_converted_frames(&frame) {
            if !*ttft_recorded && is_first_token_event(&event_type) {
                if let Some(log_context) = log_context {
                    let ttft_ms = request_log::elapsed_ms(log_context.started_at);
                    if let Err(err) = log_context
                        .store
                        .record_ttft_once(log_context.log_id, ttft_ms)
                    {
                        request_log::log_update_error(err);
                    }
                }
                *ttft_recorded = true;
            }
            envelope.forward_converted(tx, &event_type, data).await?;
        }
    }

    save_internal_upstream_sse(
        log_context,
        &raw.lock().map(|g| g.clone()).unwrap_or_default(),
    );
    let raw_sse = raw.lock().map(|g| g.clone()).unwrap_or_default();
    Ok(raw_sse)
}

/// Emits the injected web-search call that represents the gateway running the
/// search on the model's behalf. Unlike answer text, a web-search call has no
/// incremental content, so it ships as a single `output_item.added` +
/// `output_item.done` pair rather than streamed progress. The Codex client only
/// consumes these two `output_item` events for `web_search_call`; the
/// intermediate `response.web_search_call.*` progress events are not read, so we
/// omit them.
async fn emit_injected_web_search_call(
    envelope: &mut InternalSseEnvelope,
    tx: &mpsc::Sender<Result<Bytes, io::Error>>,
    tool_use: &WebSearchToolUse,
) -> Result<(), GatewayError> {
    let item_id = generate_web_search_item_id();
    let output_index = envelope.reserve_output_index();
    envelope
        .emit_owned(
            tx,
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": {
                    "type": "web_search_call",
                    "id": item_id,
                    "status": "in_progress",
                },
            }),
        )
        .await?;
    let done_item = json!({
        "type": "web_search_call",
        "id": item_id,
        "status": "completed",
        "action": {
            "type": "search",
            "query": tool_use.query.clone(),
            "queries": [tool_use.query.clone()],
        },
    });
    envelope.push_completed_output(done_item.clone());
    envelope
        .emit_owned(
            tx,
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": done_item,
            }),
        )
        .await
}

/// Splits a converter output chunk (which may batch several `event:`/`data:`
/// frames) into `(event_type, data)` pairs.
fn parse_converted_frames(frame: &Bytes) -> Vec<(String, Value)> {
    let text = String::from_utf8_lossy(frame);
    let mut out = Vec::new();
    let mut event_type: Option<String> = None;
    let mut data_lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if let (Some(event), false) = (event_type.as_ref(), data_lines.is_empty()) {
                let data = data_lines.join("\n");
                if let Ok(value) = serde_json::from_str::<Value>(&data) {
                    out.push((event.clone(), value));
                }
            }
            event_type = None;
            data_lines.clear();
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event_type = Some(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        }
    }
    if let (Some(event), false) = (event_type.as_ref(), data_lines.is_empty()) {
        let data = data_lines.join("\n");
        if let Ok(value) = serde_json::from_str::<Value>(&data) {
            out.push((event.clone(), value));
        }
    }
    out
}

fn is_first_token_event(event_type: &str) -> bool {
    event_type.starts_with("response.") && event_type.ends_with(".delta")
}

fn generate_web_search_item_id() -> String {
    format!("ws_{}", uuid::Uuid::new_v4().as_simple())
}

async fn execute_anthropic_stream_message(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
    anthropic_body: &Value,
    log_context: &Option<RequestLogContext>,
) -> Result<Value, GatewayError> {
    let mut step_request = request.clone();
    step_request.stream = true;
    let upstream_req = build_anthropic_upstream_request(
        client,
        ctx,
        &step_request,
        anthropic_body,
        provider,
        options,
    )?;
    update_upstream_log(log_context, upstream_req.headers(), anthropic_body);
    let upstream_resp = execute_stream_start(
        client,
        upstream_req,
        provider.timeout_secs,
        "anthropic upstream request failed",
    )
    .await?;
    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;
    // This buffered path serves the main answer stream, so the first upstream
    // content token here is what TTFT should measure. The internal web-search
    // sub-requests reuse read_sse_to_string but must not clobber TTFT.
    let raw_sse = read_sse_to_string(upstream_resp, log_context, true).await?;
    anthropic_message_from_sse(&raw_sse)
}

async fn execute_internal_web_search(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
    query: &str,
    log_context: &Option<RequestLogContext>,
) -> Result<String, GatewayError> {
    let body = internal_web_search_body(ctx, &request.model, query);
    let mut search_request = request.clone();
    search_request.stream = true;
    let upstream_req =
        build_anthropic_upstream_request(client, ctx, &search_request, &body, provider, options)?;
    let upstream_resp = execute_stream_start(
        client,
        upstream_req,
        provider.timeout_secs,
        "anthropic internal web search request failed",
    )
    .await?;
    let upstream_resp = ensure_success_response(&provider.name, upstream_resp).await?;
    let raw = read_sse_to_string(upstream_resp, log_context, false).await?;
    Ok(search_results_to_tool_text(query, &raw))
}

fn internal_web_search_body(ctx: &GatewayContext, model: &str, query: &str) -> Value {
    json!({
        "model": model,
        "tools": [{
            "name": "web_search",
            "type": ANTHROPIC_WEB_SEARCH_TYPE,
            "max_uses": 8
        }],
        "stream": true,
        "system": [
            {
                "text": "You are Claude Code, Anthropic's official CLI for Claude.",
                "type": "text"
            },
            {
                "text": "You are an assistant for performing a web search tool use",
                "type": "text"
            }
        ],
        "messages": [{
            "role": "user",
            "content": [{
                "text": format!("Perform a web search for the query: {query}"),
                "type": "text"
            }]
        }],
        "metadata": {
            "user_id": claude_code_metadata_user_id(ctx)
        },
        "thinking": {"type": "disabled"},
        "max_tokens": 64000,
        "tool_choice": {
            "name": "web_search",
            "type": "tool"
        },
        "output_config": {"effort": "high"}
    })
}

fn claude_code_metadata_user_id(ctx: &GatewayContext) -> String {
    let session_id = ctx.session_id.as_deref().unwrap_or(&ctx.prompt_cache_key);
    json!({
        "device_id": claude_code_device_id(session_id),
        "account_uuid": "",
        "session_id": session_id
    })
    .to_string()
}

/// Attaches a stable `metadata.user_id` to the outbound request, mirroring
/// Claude Code. The value is derived from the session id (falling back to the
/// prompt cache key), so it stays identical across every turn of a conversation.
/// An existing user_id is left untouched.
fn insert_metadata_user_id(body: &mut Value, ctx: &GatewayContext) {
    let Some(body) = body.as_object_mut() else {
        return;
    };
    let metadata = body
        .entry("metadata".to_string())
        .or_insert_with(|| json!({}));
    let Some(metadata) = metadata.as_object_mut() else {
        return;
    };
    metadata
        .entry("user_id".to_string())
        .or_insert_with(|| json!(claude_code_metadata_user_id(ctx)));
}

fn claude_code_device_id(session_id: &str) -> String {
    let digest = Sha256::digest(format!("codexhub-claude-code-device:{session_id}").as_bytes());
    hex::encode(digest)
}

async fn read_sse_to_string(
    response: reqwest::Response,
    log_context: &Option<RequestLogContext>,
    record_ttft: bool,
) -> Result<String, GatewayError> {
    let mut stream = response.bytes_stream();
    let mut raw = String::new();
    let mut ttft_recorded = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            GatewayError::upstream(
                StatusCode::BAD_GATEWAY,
                format!("read upstream stream: {e}"),
            )
        })?;
        raw.push_str(&String::from_utf8_lossy(&chunk));
        // The gateway buffers this internal Anthropic stream before replaying a
        // synthetic Responses SSE, so ResponsesSseLogStream never sees a real
        // delta event and cannot compute TTFT. Capture it here from the first
        // upstream content token instead.
        if record_ttft && !ttft_recorded && raw_sse_has_first_content_token(&raw) {
            if let Some(log_context) = log_context {
                let ttft_ms = request_log::elapsed_ms(log_context.started_at);
                if let Err(err) = log_context
                    .store
                    .record_ttft_once(log_context.log_id, ttft_ms)
                {
                    request_log::log_update_error(err);
                }
            }
            ttft_recorded = true;
        }
    }
    save_internal_upstream_sse(log_context, &raw);
    Ok(raw)
}

/// Returns true once the accumulated raw Anthropic SSE contains the first token
/// of model output. Anthropic streams `message_start` and `ping` frames before
/// any real content, so those do not count. The first `content_block_delta`
/// (text, thinking, or tool input) marks time-to-first-token.
fn raw_sse_has_first_content_token(raw: &str) -> bool {
    raw.contains("\"type\":\"content_block_delta\"")
        || raw.contains("\"type\": \"content_block_delta\"")
}

fn response_from_response_object(
    response_obj: crate::ai_gateway::model::ResponseObject,
    stream: bool,
    log_context: &Option<RequestLogContext>,
    prefix_items: &[Value],
) -> Result<Response<Body>, GatewayError> {
    if let Some(log_context) = log_context {
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
    if stream {
        let body_bytes = response_object_to_sse(&response_obj, prefix_items);
        let mut response = Response::new(Body::from(body_bytes));
        *response.status_mut() = StatusCode::OK;
        response.headers_mut().insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static("text/event-stream"),
        );
        return Ok(response);
    }
    let body_bytes = serde_json::to_vec(&response_obj).unwrap_or_default();
    let mut response = Response::new(Body::from(body_bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        HeaderName::from_static("content-type"),
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

fn response_object_to_sse(
    response_obj: &crate::ai_gateway::model::ResponseObject,
    prefix_items: &[Value],
) -> Vec<u8> {
    let mut created = serde_json::to_value(response_obj).unwrap_or_default();
    created["status"] = json!("in_progress");
    let mut completed = serde_json::to_value(response_obj).unwrap_or_default();
    let mut output_items = prefix_items.to_vec();
    output_items.extend(
        response_obj
            .output
            .iter()
            .filter_map(|item| serde_json::to_value(item).ok()),
    );
    completed["output"] = Value::Array(output_items.clone());
    let mut events = Vec::new();
    events.push((
        "response.created",
        json!({
            "type": "response.created",
            "sequence_number": 0,
            "response": created,
        }),
    ));
    events.push((
        "response.in_progress",
        json!({
            "type": "response.in_progress",
            "sequence_number": 1,
            "response": created,
        }),
    ));
    for (index, item) in output_items.iter().enumerate() {
        events.push((
            "response.output_item.added",
            json!({
                "type": "response.output_item.added",
                "sequence_number": index * 2 + 2,
                "output_index": index,
                "item": item,
            }),
        ));
        events.push((
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "sequence_number": index * 2 + 3,
                "output_index": index,
                "item": item,
            }),
        ));
    }
    events.push((
        if response_obj.status == "incomplete" {
            "response.incomplete"
        } else {
            "response.completed"
        },
        json!({
            "type": if response_obj.status == "incomplete" {
                "response.incomplete"
            } else {
                "response.completed"
            },
            "sequence_number": (prefix_items.len() + response_obj.output.len()) * 2 + 2,
            "response": completed,
        }),
    ));
    events
        .into_iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
        .collect::<String>()
        .into_bytes()
}

fn web_search_response_item(tool_use: &WebSearchToolUse) -> Value {
    json!({
        "type": "web_search_call",
        "id": tool_use.id,
        "call_id": tool_use.id,
        "status": "completed",
        "action": {
            "type": "search",
            "query": tool_use.query.clone(),
            "queries": [tool_use.query.clone()],
        },
    })
}

#[derive(Debug)]
struct WebSearchToolUse {
    id: String,
    query: String,
}

fn find_web_search_tool_uses(response: &Value) -> Vec<WebSearchToolUse> {
    let mut seen_ids = HashSet::new();
    let mut tool_uses = Vec::new();
    let Some(content) = response.get("content").and_then(Value::as_array) else {
        return tool_uses;
    };
    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("tool_use")
            || block.get("name").and_then(Value::as_str) != Some(WEB_SEARCH_TOOL_NAME)
        {
            continue;
        }
        let Some(id) = block.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let query = block
            .get("input")
            .and_then(|input| input.get("query").or_else(|| input.get("search_query")))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if query.is_empty() || !seen_ids.insert(id.to_string()) {
            continue;
        }
        tool_uses.push(WebSearchToolUse {
            id: id.to_string(),
            query,
        });
    }
    tool_uses
}

fn append_tool_results(
    anthropic_body: &mut Value,
    assistant_response: &Value,
    tool_results: Vec<(String, String)>,
) {
    if tool_results.is_empty() {
        return;
    }
    let Some(messages) = anthropic_body
        .get_mut("messages")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let content = grouped_tool_result_blocks(tool_results);
    if content.is_empty() {
        return;
    }
    if let Some(content) = assistant_response.get("content").cloned() {
        messages.push(json!({
            "role": "assistant",
            "content": content
        }));
    }
    messages.push(json!({
        "role": "user",
        "content": content
    }));
}

fn grouped_tool_result_blocks(tool_results: Vec<(String, String)>) -> Vec<Value> {
    let mut content: Vec<Value> = Vec::new();
    for (tool_use_id, search_text) in tool_results {
        if tool_use_id.is_empty() {
            continue;
        }
        if let Some(existing) = content.iter_mut().find(|block| {
            block.get("tool_use_id").and_then(Value::as_str) == Some(tool_use_id.as_str())
        }) {
            if !search_text.is_empty() {
                let current = existing
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                existing["content"] = if current.is_empty() {
                    json!(search_text)
                } else {
                    json!(format!("{current}\n\n{search_text}"))
                };
            }
            continue;
        }
        content.push(json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": search_text
        }));
    }
    content
}

fn search_results_to_tool_text(query: &str, raw_sse: &str) -> String {
    let mut results = Vec::new();
    for event in parse_sse_json_events(raw_sse) {
        let Some(block) = event.get("content_block").filter(|block| {
            block.get("type").and_then(Value::as_str) == Some("web_search_tool_result")
        }) else {
            continue;
        };
        if let Some(content) = block.get("content").and_then(Value::as_array) {
            for result in content {
                if result.get("type").and_then(Value::as_str) != Some("web_search_result") {
                    continue;
                }
                let title = result.get("title").and_then(Value::as_str).unwrap_or("");
                let url = result.get("url").and_then(Value::as_str).unwrap_or("");
                let snippet = result
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                results.push(format!("{title}\nURL: {url}\nSnippet: {snippet}"));
            }
        }
    }
    if results.is_empty() {
        return format!("No web search results were returned for query: {query}");
    }
    format!(
        "Web search results for query: {query}\n\n{}",
        results.join("\n\n")
    )
}

fn anthropic_message_from_sse(raw_sse: &str) -> Result<Value, GatewayError> {
    let mut message = Map::new();
    let mut content = Vec::new();
    let mut stop_reason: Option<String> = None;
    let mut usage = Map::new();
    for event in parse_sse_json_events(raw_sse) {
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if let Some(start_message) = event.get("message").and_then(Value::as_object) {
                    for (key, value) in start_message {
                        if key == "content" {
                            continue;
                        }
                        if key == "usage"
                            && let Some(start_usage) = value.as_object()
                        {
                            for (usage_key, usage_value) in start_usage {
                                usage.insert(usage_key.clone(), usage_value.clone());
                            }
                            continue;
                        }
                        message.insert(key.clone(), value.clone());
                    }
                }
            }
            Some("content_block_start") => {
                if let Some(block) = event.get("content_block").cloned() {
                    let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    ensure_content_index(&mut content, index);
                    content[index] = block;
                }
            }
            Some("content_block_delta") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                ensure_content_index(&mut content, index);
                if let Some(delta) = event.get("delta") {
                    apply_content_delta(&mut content[index], delta);
                }
            }
            Some("message_delta") => {
                if let Some(reason) = event
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    stop_reason = Some(reason.to_string());
                }
                if let Some(delta_usage) = event.get("usage").and_then(Value::as_object) {
                    for (usage_key, usage_value) in delta_usage {
                        usage.insert(usage_key.clone(), usage_value.clone());
                    }
                }
            }
            _ => {}
        }
    }
    if message.is_empty() && content.is_empty() {
        return Err(GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            "anthropic stream did not contain a message",
        ));
    }
    message
        .entry("type".to_string())
        .or_insert_with(|| json!("message"));
    message
        .entry("role".to_string())
        .or_insert_with(|| json!("assistant"));
    message
        .entry("content".to_string())
        .or_insert_with(|| Value::Array(content));
    message.insert(
        "stop_reason".to_string(),
        stop_reason.map(Value::String).unwrap_or(Value::Null),
    );
    if !usage.is_empty() {
        message.insert("usage".to_string(), Value::Object(usage));
    }
    Ok(Value::Object(message))
}

fn ensure_content_index(content: &mut Vec<Value>, index: usize) {
    while content.len() <= index {
        content.push(Value::Null);
    }
}

fn apply_content_delta(block: &mut Value, delta: &Value) {
    match delta.get("type").and_then(Value::as_str) {
        Some("text_delta") => {
            let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
            append_string_field(block, "text", text);
        }
        Some("input_json_delta") => {
            let partial = delta
                .get("partial_json")
                .and_then(Value::as_str)
                .unwrap_or("");
            append_string_field(block, "__partial_input_json", partial);
            if let Some(input) = block
                .get("__partial_input_json")
                .and_then(Value::as_str)
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                && let Some(object) = block.as_object_mut()
            {
                object.insert("input".to_string(), input);
                object.remove("__partial_input_json");
            }
        }
        Some("thinking_delta") => {
            let thinking = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
            append_string_field(block, "thinking", thinking);
        }
        _ => {}
    }
}

fn append_string_field(block: &mut Value, field: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    if let Some(object) = block.as_object_mut() {
        let entry = object
            .entry(field.to_string())
            .or_insert_with(|| Value::String(String::new()));
        if let Some(text) = entry.as_str() {
            *entry = Value::String(format!("{text}{value}"));
        }
    }
}

fn parse_sse_json_events(raw_sse: &str) -> Vec<Value> {
    let mut events = Vec::new();
    let mut data_lines = Vec::new();
    for line in raw_sse.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if !data_lines.is_empty() {
                let data = data_lines.join("\n");
                if data.trim() != "[DONE]"
                    && let Ok(value) = serde_json::from_str::<Value>(&data)
                {
                    events.push(value);
                }
                data_lines.clear();
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
    }
    if !data_lines.is_empty() {
        let data = data_lines.join("\n");
        if data.trim() != "[DONE]"
            && let Ok(value) = serde_json::from_str::<Value>(&data)
        {
            events.push(value);
        }
    }
    events
}

fn has_web_search_client_tool(body: &Value) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools.iter().any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some(WEB_SEARCH_TOOL_NAME)
                    && tool.get("type").is_none()
            })
        })
        .unwrap_or(false)
}

fn force_anthropic_stream(mut body: Value, stream: bool) -> Value {
    if stream {
        body["stream"] = json!(true);
    } else if let Some(object) = body.as_object_mut() {
        object.remove("stream");
    }
    body
}

fn update_upstream_log(
    log_context: &Option<RequestLogContext>,
    headers: &HeaderMap,
    anthropic_body: &Value,
) {
    if let Some(log_context) = log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: log_context
                .details_enabled
                .then(|| request_log::headers_to_json(headers))
                .flatten(),
            upstream_request_body_bytes: request_log::json_body_size_bytes(anthropic_body),
            upstream_request_json: log_context
                .details_enabled
                .then(|| serde_json::to_string(anthropic_body).ok())
                .flatten(),
            ..RequestLogUpdate::default()
        };
        if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(err);
        }
    }
}

fn save_internal_upstream_sse(log_context: &Option<RequestLogContext>, raw_sse: &str) {
    let Some(log_context) = log_context else {
        return;
    };
    if !log_context.details_enabled {
        return;
    }
    if raw_sse.is_empty() {
        return;
    }
    let existing = log_context
        .store
        .get_detail(log_context.log_id)
        .ok()
        .flatten()
        .and_then(|detail| detail.upstream_response_sse)
        .unwrap_or_default();
    let mut text = if existing.is_empty() {
        raw_sse.to_string()
    } else {
        format!("{existing}\n\n: [codexhub] internal anthropic SSE segment\n\n{raw_sse}")
    };
    const INTERNAL_UPSTREAM_SSE_LOG_LIMIT_BYTES: usize = 512 * 1024;
    if text.len() > INTERNAL_UPSTREAM_SSE_LOG_LIMIT_BYTES {
        text.truncate(INTERNAL_UPSTREAM_SSE_LOG_LIMIT_BYTES);
        text.push_str("\n\n: [codexhub] upstream SSE log truncated\n");
    }
    let update = RequestLogUpdate {
        upstream_response_sse: Some(text),
        ..RequestLogUpdate::default()
    };
    if let Err(err) = log_context.store.update_record(log_context.log_id, &update) {
        request_log::log_update_error(err);
    }
}

fn build_anthropic_upstream_request(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    anthropic_body: &Value,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<reqwest::Request, GatewayError> {
    let req_builder = client
        .post(options.messages_url(provider))
        .json(anthropic_body);
    let req_builder =
        apply_total_request_timeout(req_builder, provider.timeout_secs, request.stream);
    let mut upstream_req = req_builder.build().map_err(|e| {
        error!(error = %e, "build anthropic upstream request failed");
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("build upstream request: {e}"),
        )
    })?;

    *upstream_req.version_mut() = reqwest::Version::HTTP_11;
    apply_anthropic_managed_headers(upstream_req.headers_mut(), ctx, provider, options)?;
    Ok(upstream_req)
}

fn apply_anthropic_managed_headers(
    headers: &mut HeaderMap,
    ctx: &GatewayContext,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<(), GatewayError> {
    headers.clear();
    insert_static_header(headers, "content-type", "application/json");
    apply_anthropic_auth(headers, provider, options)?;
    apply_anthropic_version_header(headers, options);
    apply_anthropic_client_headers(headers, ctx, provider, options)?;
    Ok(())
}

fn apply_anthropic_auth(
    headers: &mut HeaderMap,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<(), GatewayError> {
    match options.auth {
        AnthropicAuthStyle::XApiKey => {
            insert_dynamic_header(headers, "x-api-key", provider.api_key.trim())?;
        }
        AnthropicAuthStyle::Bearer => {
            insert_dynamic_header(
                headers,
                "authorization",
                bearer_authorization(&provider.api_key),
            )?;
        }
    }
    Ok(())
}

fn apply_anthropic_version_header(headers: &mut HeaderMap, options: &AnthropicProviderOptions) {
    match options.version_header {
        AnthropicVersionHeader::Required(version) => {
            insert_static_header(headers, "anthropic-version", version);
        }
    }
}

fn apply_anthropic_client_headers(
    headers: &mut HeaderMap,
    ctx: &GatewayContext,
    provider: &ProviderConfig,
    options: &AnthropicProviderOptions,
) -> Result<(), GatewayError> {
    match options.headers {
        AnthropicHeaderStyle::Plain => {}
        AnthropicHeaderStyle::ClaudeCode => {
            let existing_beta = headers
                .get("anthropic-beta")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            let merged_beta = merge_anthropic_betas(ANTHROPIC_CLAUDE_CODE_BETA, existing_beta);
            insert_dynamic_header(headers, "anthropic-beta", &merged_beta)?;
            insert_static_header(headers, "anthropic-dangerous-direct-browser-access", "true");
            insert_static_header(headers, "x-app", "cli");
            insert_static_header(headers, "accept", "application/json");
            insert_static_header(headers, "accept-encoding", "gzip, deflate, br, zstd");
            insert_static_header(headers, "connection", "keep-alive");
            insert_static_header(headers, "user-agent", CLAUDE_CODE_USER_AGENT);
            insert_dynamic_header(
                headers,
                "x-stainless-timeout",
                provider.timeout_secs.to_string(),
            )?;
            insert_static_header(headers, "x-stainless-retry-count", "0");
            insert_static_header(headers, "x-stainless-runtime", "node");
            insert_static_header(headers, "x-stainless-lang", "js");
            insert_static_header(headers, "x-stainless-arch", stainless_arch());
            insert_static_header(headers, "x-stainless-os", stainless_os());
            insert_static_header(
                headers,
                "x-stainless-package-version",
                CLAUDE_CODE_STAINLESS_PACKAGE_VERSION,
            );
            insert_static_header(
                headers,
                "x-stainless-runtime-version",
                CLAUDE_CODE_STAINLESS_RUNTIME_VERSION,
            );
            let session_id = ctx.session_id.as_deref().unwrap_or(&ctx.prompt_cache_key);
            insert_dynamic_header(headers, "x-claude-code-session-id", session_id)?;
        }
    }
    Ok(())
}

fn insert_static_header(headers: &mut HeaderMap, name: &'static str, value: &'static str) {
    headers.insert(
        HeaderName::from_static(name),
        HeaderValue::from_static(value),
    );
}

fn insert_dynamic_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: impl AsRef<str>,
) -> Result<(), GatewayError> {
    let value = HeaderValue::from_str(value.as_ref()).map_err(|e| {
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("invalid Anthropic header '{name}': {e}"),
        )
    })?;
    headers.insert(HeaderName::from_static(name), value);
    Ok(())
}

fn bearer_authorization(api_key: &str) -> String {
    let api_key = api_key.trim();
    if let Some(token) = strip_bearer_prefix(api_key) {
        format!("Bearer {}", token.trim())
    } else {
        format!("Bearer {api_key}")
    }
}

fn strip_bearer_prefix(value: &str) -> Option<&str> {
    let prefix_len = "bearer ".len();
    if value
        .get(..prefix_len)
        .map(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .unwrap_or(false)
    {
        Some(&value[prefix_len..])
    } else {
        None
    }
}

fn merge_anthropic_betas(required: &str, inbound: &str) -> String {
    let mut merged = Vec::new();
    for beta in required.split(',').chain(inbound.split(',')) {
        let beta = beta.trim();
        if !beta.is_empty() && !merged.contains(&beta) {
            merged.push(beta);
        }
    }
    merged.join(",")
}

fn stainless_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
    #[cfg(all(not(target_arch = "x86_64"), not(target_arch = "aarch64")))]
    {
        std::env::consts::ARCH
    }
}

fn stainless_os() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Windows"
    }
    #[cfg(target_os = "macos")]
    {
        "MacOS"
    }
    #[cfg(target_os = "linux")]
    {
        "Linux"
    }
    #[cfg(all(
        not(target_os = "windows"),
        not(target_os = "macos"),
        not(target_os = "linux")
    ))]
    {
        std::env::consts::OS
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

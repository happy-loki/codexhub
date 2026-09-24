use axum::http::{HeaderMap, HeaderValue, StatusCode, Version};
use serde_json::{Value, json};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{handshake::derive_accept_key, protocol::Role},
};

use super::{MAX_MESSAGE_BYTES, upstream_error};
use crate::ai_gateway::{
    chatgpt_auth::{self, AuthManager},
    config::{ProviderConfig, ProviderType},
    context::GatewayContext,
    encrypted_content::{EncryptedContentScope, prepare_responses_request},
    error::GatewayError,
    handler,
    providers::execute_openai_request_with_auth,
};

pub(in crate::ai_gateway) struct NativeConnection {
    pub socket: WebSocketStream<reqwest::Upgraded>,
    pub request_headers: HeaderMap,
    pub response_headers: HeaderMap,
    pub metadata_sent: bool,
}

pub(super) async fn connect(
    context: &GatewayContext,
    provider: &ProviderConfig,
) -> Result<NativeConnection, GatewayError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(provider.timeout_secs.clamp(1, 30)),
        connect_with_auth(
            &crate::outbound_http::get(),
            context,
            provider,
            &chatgpt_auth::endpoint(provider, "/v1/responses"),
            chatgpt_auth::manager(),
        ),
    )
    .await
    .map_err(|_| GatewayError::upstream_timeout())?
}

pub(in crate::ai_gateway) async fn connect_with_auth(
    client: &reqwest::Client,
    context: &GatewayContext,
    provider: &ProviderConfig,
    endpoint: &str,
    auth: &AuthManager,
) -> Result<NativeConnection, GatewayError> {
    let key = tokio_tungstenite::tungstenite::handshake::client::generate_key();
    let mut request = client
        .get(endpoint)
        .version(Version::HTTP_11)
        .headers(context.upstream_headers.clone())
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", &key)
        .build()
        .map_err(upstream_error)?;
    if provider.provider_type == ProviderType::OpenAiResponses {
        let mut bearer = HeaderValue::from_str(&format!("Bearer {}", provider.api_key))
            .map_err(|_| GatewayError::bad_request("invalid upstream API key"))?;
        bearer.set_sensitive(true);
        request.headers_mut().insert("authorization", bearer);
    }
    // Keep other beta flags while enabling the same transport version as Codex.
    let beta = request
        .headers()
        .get("openai-beta")
        .and_then(|v| v.to_str().ok());
    if !beta.is_some_and(|v| v.contains("responses_websockets=")) {
        let beta = match beta {
            Some(value) => format!("{value},responses_websockets=2026-02-06"),
            None => "responses_websockets=2026-02-06".to_string(),
        };
        request.headers_mut().insert(
            "openai-beta",
            HeaderValue::from_str(&beta).map_err(upstream_error)?,
        );
    }
    chatgpt_auth::authorize_with_manager(auth, client, &mut request, provider, None).await?;
    let request_headers = request.headers().clone();
    let response = execute_openai_request_with_auth(
        client,
        request,
        provider,
        "Responses WebSocket handshake failed",
        auth,
    )
    .await?;
    let status = response.status();
    if status != StatusCode::SWITCHING_PROTOCOLS {
        let body = response.text().await.map_err(upstream_error)?;
        return Err(GatewayError::from_upstream_body(
            status,
            &provider.name,
            &body,
        ));
    }
    let headers = response.headers();
    let has_token = |name: &str, token: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case(token))
            })
    };
    if !has_token("upgrade", "websocket")
        || !has_token("connection", "upgrade")
        || headers
            .get("sec-websocket-accept")
            .and_then(|v| v.to_str().ok())
            != Some(derive_accept_key(key.as_bytes()).as_str())
        || headers.contains_key("sec-websocket-extensions")
        || headers.contains_key("sec-websocket-protocol")
    {
        return Err(upstream_error("invalid upstream WebSocket handshake"));
    }
    let response_headers = headers.clone();
    let upgraded = response.upgrade().await.map_err(upstream_error)?;
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE_BYTES),
        max_frame_size: Some(MAX_MESSAGE_BYTES),
        ..Default::default()
    };
    Ok(NativeConnection {
        socket: WebSocketStream::from_raw_socket(upgraded, Role::Client, Some(config)).await,
        request_headers,
        response_headers,
        metadata_sent: false,
    })
}

pub(super) fn prepare(
    raw: &mut Value,
    provider: &ProviderConfig,
    context: &GatewayContext,
    filter_images: bool,
) {
    handler::filter_image_generation_tools(raw, filter_images);
    handler::strip_hosted_web_search_from_lite_request_tools(raw, &provider.provider_type);
    if let Some(model) = raw["model"]
        .as_str()
        .and_then(|m| provider.resolve_upstream_model(m))
    {
        raw["model"] = json!(model);
    }
    if raw
        .get("prompt_cache_key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .is_empty()
    {
        raw["prompt_cache_key"] = json!(context.prompt_cache_key);
    }
    if let Some(retention) = &provider.prompt_cache_retention
        && raw.get("prompt_cache_retention").is_none()
    {
        raw["prompt_cache_retention"] = json!(retention);
    }
    let had_input_array = raw.get("input").is_some_and(Value::is_array);
    prepare_responses_request(raw, &EncryptedContentScope::for_provider(provider));
    // HTTP cleanup removes empty input. WS warmup and incremental creates must
    // retain an explicitly empty input array.
    if had_input_array && raw.get("input").is_none() {
        raw["input"] = json!([]);
    }
    if let Some(object) = raw.as_object_mut() {
        object.remove("stream");
        object.remove("background");
    }
}

pub(super) fn add_response_headers(event: &mut Value, headers: &HeaderMap) {
    for name in ["x-codex-turn-state", "x-reasoning-included", "openai-model"] {
        if let Some(value) = headers.get(name).and_then(|v| v.to_str().ok()) {
            if event.get("headers").is_none() {
                event["headers"] = json!({});
            }
            if let Some(target) = event["headers"].as_object_mut() {
                target.entry(name).or_insert_with(|| json!(value));
            }
        }
    }
}

use std::borrow::Cow;

use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::Response,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tracing::{debug, error};

use crate::ai_gateway::context::{GatewayContext, apply_upstream_headers};
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::request_log::{self, RequestLogContext, RequestLogUpdate};
use crate::ai_gateway::{chatgpt_auth, config::ProviderConfig};

use super::{apply_total_request_timeout, ensure_success_response};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageEndpoint {
    Generations,
    Edits,
}

impl ImageEndpoint {
    pub fn path(self) -> &'static str {
        match self {
            Self::Generations => "images/generations",
            Self::Edits => "images/edits",
        }
    }

    pub fn log_type(self) -> &'static str {
        match self {
            Self::Generations => "image_generation",
            Self::Edits => "image_edit",
        }
    }
}

pub struct ImageRequestInspection {
    pub model: String,
    pub sanitized_log: Value,
}

#[derive(Deserialize)]
struct ImageRequestBody<'a> {
    #[serde(borrow)]
    model: Cow<'a, str>,
    #[serde(default, borrow)]
    prompt: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    background: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    quality: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    size: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    output_format: Option<Cow<'a, str>>,
    #[serde(default)]
    n: Option<u64>,
    #[serde(default, borrow)]
    images: Vec<ImageRequestInput<'a>>,
}

#[derive(Deserialize)]
struct ImageRequestInput<'a> {
    #[serde(default, borrow)]
    image_url: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct ImageResponseBody<'a> {
    #[serde(default)]
    created: Option<u64>,
    #[serde(default, borrow)]
    background: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    output_format: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    quality: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    size: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    data: Vec<ImageResponseData<'a>>,
    #[serde(default)]
    usage: Option<Value>,
}

#[derive(Deserialize)]
struct ImageResponseData<'a> {
    #[serde(default, borrow)]
    b64_json: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    revised_prompt: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    url: Option<Cow<'a, str>>,
}

pub fn inspect_request(
    raw_body: &Bytes,
    endpoint: ImageEndpoint,
) -> Result<ImageRequestInspection, GatewayError> {
    let request: ImageRequestBody<'_> = serde_json::from_slice(raw_body)
        .map_err(|error| GatewayError::bad_request(format!("invalid image request: {error}")))?;
    let model = request.model.trim();
    if model.is_empty() {
        return Err(GatewayError::bad_request(
            "image request model must not be empty",
        ));
    }

    let mut sanitized = Map::new();
    sanitized.insert("model".to_string(), Value::String(model.to_string()));
    insert_optional_string(&mut sanitized, "prompt", request.prompt.as_deref());
    insert_optional_string(&mut sanitized, "background", request.background.as_deref());
    insert_optional_string(&mut sanitized, "quality", request.quality.as_deref());
    insert_optional_string(&mut sanitized, "size", request.size.as_deref());
    insert_optional_string(
        &mut sanitized,
        "output_format",
        request.output_format.as_deref(),
    );
    if let Some(n) = request.n {
        sanitized.insert("n".to_string(), Value::Number(n.into()));
    }
    if !request.images.is_empty() {
        sanitized.insert(
            "images".to_string(),
            Value::Array(
                request
                    .images
                    .iter()
                    .map(|image| summarize_image_reference(image.image_url.as_deref()))
                    .collect(),
            ),
        );
    }
    sanitized.insert(
        "_codexhub".to_string(),
        json!({
            "endpoint": endpoint.path(),
            "requestBodyBytes": byte_len(raw_body.len()),
            "imagePayloadsRedacted": !request.images.is_empty()
        }),
    );

    Ok(ImageRequestInspection {
        model: model.to_string(),
        sanitized_log: Value::Object(sanitized),
    })
}

pub async fn passthrough(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    raw_body: Bytes,
    request_model: &str,
    upstream_model: &str,
    provider: &ProviderConfig,
    endpoint: ImageEndpoint,
    log_context: Option<RequestLogContext>,
    upstream_request_log: Option<Value>,
) -> Result<Response<Body>, GatewayError> {
    let request_body = rewrite_model_if_needed(raw_body, request_model, upstream_model)?;
    let request_body_bytes = byte_len(request_body.len());

    let url = chatgpt_auth::endpoint(provider, &format!("/v1/{}", endpoint.path()));
    let mut request = apply_upstream_headers(
        apply_total_request_timeout(
            client
                .post(&url)
                .header(CONTENT_TYPE, "application/json")
                .header("authorization", format!("Bearer {}", provider.api_key)),
            provider.timeout_secs,
            false,
        )
        .body(request_body),
        &ctx.upstream_headers,
    )
    .build()
    .map_err(|error| {
        error!(error = %error, "build upstream image request failed");
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("build upstream image request: {error}"),
        )
    })?;

    chatgpt_auth::authorize(client, &mut request, provider, None).await?;

    if let Some(log_context) = &log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: log_context
                .details_enabled
                .then(|| request_log::headers_to_redacted_json(request.headers()))
                .flatten(),
            upstream_request_body_bytes: Some(request_body_bytes),
            upstream_request_json: log_context
                .details_enabled
                .then(|| {
                    upstream_request_log
                        .as_ref()
                        .and_then(|value| serde_json::to_string(value).ok())
                })
                .flatten(),
            ..RequestLogUpdate::default()
        };
        if let Err(error) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(error);
        }
    }

    debug!(
        url = %url,
        provider = %provider.name,
        model = %upstream_model,
        endpoint = endpoint.path(),
        "proxying image request"
    );

    let upstream_response =
        super::execute_openai_request(client, request, provider, "upstream image request failed")
            .await?;
    request_log::record_upstream_response_headers(
        log_context.as_ref(),
        upstream_response.headers(),
    );
    let upstream_response = ensure_success_response(&provider.name, upstream_response).await?;
    let status = upstream_response.status();
    let content_type = upstream_response.headers().get(CONTENT_TYPE).cloned();
    let response_body = upstream_response.bytes().await.map_err(|error| {
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("read upstream image response: {error}"),
        )
    })?;

    if let Some(log_context) = &log_context {
        let response_log = sanitize_response(&response_body, endpoint);
        let update = RequestLogUpdate {
            status: Some(request_log::status_from_response_value(&response_log)),
            usage: Some(request_log::usage_from_response_value(&response_log)),
            latency_ms: Some(request_log::elapsed_ms(log_context.started_at)),
            response_json: log_context
                .details_enabled
                .then(|| serde_json::to_string(&response_log).ok())
                .flatten(),
            ..RequestLogUpdate::default()
        };
        if let Err(error) = log_context.store.update_record(log_context.log_id, &update) {
            request_log::log_update_error(error);
        }
    }

    let mut response = Response::new(Body::from(response_body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        content_type.unwrap_or_else(|| HeaderValue::from_static("application/json")),
    );
    Ok(response)
}

fn insert_optional_string(target: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        target.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn summarize_image_reference(image_url: Option<&str>) -> Value {
    let Some(image_url) = image_url else {
        return json!({"kind": "missing"});
    };
    let Some((metadata, payload)) = image_url.split_once(',') else {
        return json!({
            "kind": "reference",
            "referenceChars": byte_len(image_url.len()),
            "payloadRedacted": true
        });
    };
    if !metadata.starts_with("data:") {
        return json!({
            "kind": "reference",
            "referenceChars": byte_len(image_url.len()),
            "payloadRedacted": true
        });
    }

    let mime_type = metadata
        .strip_prefix("data:")
        .and_then(|value| value.split(';').next())
        .filter(|value| !value.is_empty());
    json!({
        "kind": "data_url",
        "mimeType": mime_type,
        "base64Chars": byte_len(payload.len()),
        "estimatedDecodedBytes": estimated_base64_bytes(payload),
        "payloadRedacted": true
    })
}

fn sanitize_response(raw_body: &Bytes, endpoint: ImageEndpoint) -> Value {
    let response: ImageResponseBody<'_> = match serde_json::from_slice(raw_body) {
        Ok(response) => response,
        Err(error) => {
            return json!({
                "status": "completed",
                "_codexhub": {
                    "endpoint": endpoint.path(),
                    "responseBodyBytes": byte_len(raw_body.len()),
                    "imagePayloadsRedacted": true,
                    "parseError": error.to_string()
                }
            });
        }
    };

    let mut sanitized = Map::new();
    sanitized.insert("status".to_string(), Value::String("completed".to_string()));
    if let Some(created) = response.created {
        sanitized.insert("created".to_string(), Value::Number(created.into()));
    }
    insert_optional_string(&mut sanitized, "background", response.background.as_deref());
    insert_optional_string(
        &mut sanitized,
        "output_format",
        response.output_format.as_deref(),
    );
    insert_optional_string(&mut sanitized, "quality", response.quality.as_deref());
    insert_optional_string(&mut sanitized, "size", response.size.as_deref());
    sanitized.insert(
        "data".to_string(),
        Value::Array(
            response
                .data
                .iter()
                .map(|image| {
                    let mut item = Map::new();
                    if let Some(base64) = image.b64_json.as_deref() {
                        item.insert(
                            "b64_json".to_string(),
                            Value::String("<redacted>".to_string()),
                        );
                        item.insert(
                            "base64Chars".to_string(),
                            Value::Number(byte_len(base64.len()).into()),
                        );
                        item.insert(
                            "estimatedDecodedBytes".to_string(),
                            Value::Number(estimated_base64_bytes(base64).into()),
                        );
                    }
                    if let Some(prompt) = image.revised_prompt.as_deref() {
                        item.insert(
                            "revised_prompt".to_string(),
                            Value::String(prompt.to_string()),
                        );
                    }
                    if let Some(url) = image.url.as_deref() {
                        item.insert("url".to_string(), Value::String("<redacted>".to_string()));
                        item.insert(
                            "urlChars".to_string(),
                            Value::Number(byte_len(url.len()).into()),
                        );
                    }
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    if let Some(usage) = response.usage {
        sanitized.insert("usage".to_string(), usage);
    }
    sanitized.insert(
        "_codexhub".to_string(),
        json!({
            "endpoint": endpoint.path(),
            "responseBodyBytes": byte_len(raw_body.len()),
            "imagePayloadsRedacted": true
        }),
    );
    Value::Object(sanitized)
}

fn estimated_base64_bytes(value: &str) -> i64 {
    let padding = value
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'=')
        .count();
    byte_len(value.len().saturating_mul(3) / 4).saturating_sub(byte_len(padding))
}

fn byte_len(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn rewrite_model_if_needed(
    raw_body: Bytes,
    request_model: &str,
    upstream_model: &str,
) -> Result<Bytes, GatewayError> {
    if request_model == upstream_model {
        return Ok(raw_body);
    }

    let mut body: Value = serde_json::from_slice(&raw_body)
        .map_err(|error| GatewayError::bad_request(format!("invalid JSON: {error}")))?;
    let object = body
        .as_object_mut()
        .ok_or_else(|| GatewayError::bad_request("image request body must be a JSON object"))?;
    object.insert(
        "model".to_string(),
        Value::String(upstream_model.to_string()),
    );
    serde_json::to_vec(&body)
        .map(Bytes::from)
        .map_err(|error| GatewayError::bad_request(format!("encode image request body: {error}")))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use axum::{
        Json, Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, Uri},
        routing::post,
    };
    use serde_json::{Value, json};
    use tokio::sync::mpsc;

    use crate::ai_gateway::config::ProviderConfig;
    use crate::ai_gateway::context::GatewayContext;
    use crate::ai_gateway::request_log::{
        LogUsage, RequestLogContext, RequestLogRecord, RequestLogStore,
    };

    use super::{
        ImageEndpoint, inspect_request, passthrough, rewrite_model_if_needed, sanitize_response,
    };

    struct CapturedRequest {
        path: String,
        headers: HeaderMap,
        body: Value,
    }

    async fn capture_request(
        State(sender): State<mpsc::UnboundedSender<CapturedRequest>>,
        uri: Uri,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        sender
            .send(CapturedRequest {
                path: uri.path().to_string(),
                headers,
                body,
            })
            .expect("capture receiver should stay open");
        Json(json!({
            "created": 1,
            "data": [{"b64_json": "aW1hZ2U="}],
            "usage": {
                "input_tokens": 7,
                "output_tokens": 11,
                "total_tokens": 18
            }
        }))
    }

    async fn mock_image_server() -> (
        String,
        mpsc::UnboundedReceiver<CapturedRequest>,
        tokio::task::JoinHandle<()>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/v1/images/generations", post(capture_request))
            .route("/v1/images/edits", post(capture_request))
            .with_state(sender);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock image server");
        let address = listener.local_addr().expect("mock server address");
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock image endpoint");
        });
        (format!("http://{address}/v1"), receiver, task)
    }

    fn provider(base_url: String) -> ProviderConfig {
        ProviderConfig {
            name: "image-provider".to_string(),
            base_url,
            api_key: "image-secret".to_string(),
            models: vec!["gpt-image-2".to_string()],
            ..ProviderConfig::default()
        }
    }

    #[test]
    fn keeps_original_request_bytes_when_model_does_not_change() {
        let original =
            Bytes::from_static(br#"{ "model": "gpt-image-2", "prompt": "paint a city" }"#);

        let rewritten = rewrite_model_if_needed(original.clone(), "gpt-image-2", "gpt-image-2")
            .expect("unchanged model should preserve request body");

        assert_eq!(rewritten, original);
    }

    #[test]
    fn rewrites_only_the_model_for_provider_aliases() {
        let original = Bytes::from_static(
            br#"{"model":"gpt-image-2","prompt":"paint a city","quality":"auto"}"#,
        );

        let rewritten = rewrite_model_if_needed(original, "gpt-image-2", "upstream-image-model")
            .expect("provider alias should rewrite model");
        let body: Value = serde_json::from_slice(&rewritten).expect("rewritten JSON");

        assert_eq!(body["model"], "upstream-image-model");
        assert_eq!(body["prompt"], "paint a city");
        assert_eq!(body["quality"], "auto");
    }

    #[test]
    fn image_request_log_redacts_data_urls() {
        let body = Bytes::from_static(
            br#"{"model":"gpt-image-2","prompt":"edit","images":[{"image_url":"data:image/png;base64,U0VDUkVUX0lNQUdF"}]}"#,
        );

        let inspection = inspect_request(&body, ImageEndpoint::Edits).expect("inspect request");
        let logged = serde_json::to_string(&inspection.sanitized_log).expect("serialize log");

        assert_eq!(inspection.model, "gpt-image-2");
        assert!(!logged.contains("U0VDUkVUX0lNQUdF"));
        assert_eq!(
            inspection.sanitized_log["images"][0]["mimeType"],
            "image/png"
        );
        assert_eq!(inspection.sanitized_log["images"][0]["base64Chars"], 16);
        assert_eq!(
            inspection.sanitized_log["images"][0]["payloadRedacted"],
            true
        );
    }

    #[test]
    fn image_response_log_redacts_base64_and_keeps_usage() {
        let body = Bytes::from_static(
            br#"{"created":1,"data":[{"b64_json":"U0VDUkVUX0lNQUdF"}],"usage":{"input_tokens":7,"output_tokens":11,"total_tokens":18}}"#,
        );

        let sanitized = sanitize_response(&body, ImageEndpoint::Generations);
        let logged = serde_json::to_string(&sanitized).expect("serialize response log");

        assert!(!logged.contains("U0VDUkVUX0lNQUdF"));
        assert_eq!(sanitized["data"][0]["b64_json"], "<redacted>");
        assert_eq!(sanitized["data"][0]["base64Chars"], 16);
        assert_eq!(sanitized["usage"]["total_tokens"], 18);
    }

    #[tokio::test]
    async fn proxies_generation_and_edit_json_to_openai_images_paths() {
        let (base_url, mut requests, server) = mock_image_server().await;
        let provider = provider(base_url);
        let client = reqwest::Client::new();
        let context = GatewayContext::extract(&HeaderMap::new(), None);

        for (endpoint, expected_path, body) in [
            (
                ImageEndpoint::Generations,
                "/v1/images/generations",
                json!({"model": "gpt-image-2", "prompt": "paint a city"}),
            ),
            (
                ImageEndpoint::Edits,
                "/v1/images/edits",
                json!({
                    "model": "gpt-image-2",
                    "prompt": "add rain",
                    "images": [{"image_url": "data:image/png;base64,aW1hZ2U="}]
                }),
            ),
        ] {
            let response = passthrough(
                &client,
                &context,
                Bytes::from(serde_json::to_vec(&body).expect("encode image request")),
                "gpt-image-2",
                "upstream-image-model",
                &provider,
                endpoint,
                None,
                None,
            )
            .await
            .expect("image proxy should succeed");
            assert_eq!(response.status(), axum::http::StatusCode::OK);

            let captured = requests.recv().await.expect("captured image request");
            assert_eq!(captured.path, expected_path);
            assert_eq!(captured.body["model"], "upstream-image-model");
            assert_eq!(
                captured.headers.get("authorization").unwrap(),
                "Bearer image-secret"
            );
            assert_eq!(
                captured.headers.get("content-type").unwrap(),
                "application/json"
            );
        }

        server.abort();
    }

    #[tokio::test]
    async fn records_completed_sanitized_image_log() {
        let (base_url, _requests, server) = mock_image_server().await;
        let provider = provider(base_url);
        let client = reqwest::Client::new();
        let context = GatewayContext::extract(&HeaderMap::new(), None);
        let db_path = std::env::temp_dir().join(format!(
            "codexhub-image-request-log-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let store = RequestLogStore::new(db_path.clone());
        let record = RequestLogRecord {
            request_id: "image-log-test".to_string(),
            model_id: "gpt-image-2".to_string(),
            stream: false,
            channel: "image-provider".to_string(),
            provider_type: "image_generation".to_string(),
            status: "running".to_string(),
            usage: LogUsage::default(),
            cost_usd: None,
            latency_ms: None,
            ttft_ms: None,
            created_at_ms: 1,
            error_message: None,
            request_headers_json: None,
            request_json: None,
            upstream_request_body_bytes: None,
            upstream_request_headers_json: None,
            upstream_request_json: None,
            upstream_response_sse: None,
            response_json: None,
        };
        let log_id = store.insert_record(&record).expect("insert image log");
        let log_context = RequestLogContext {
            store: store.clone(),
            log_id,
            started_at: Instant::now(),
            details_enabled: true,
        };
        let body = Bytes::from_static(br#"{"model":"gpt-image-2","prompt":"paint a city"}"#);
        let inspection =
            inspect_request(&body, ImageEndpoint::Generations).expect("inspect request");
        let mut upstream_log = inspection.sanitized_log;
        upstream_log["model"] = json!("upstream-image-model");

        passthrough(
            &client,
            &context,
            body,
            "gpt-image-2",
            "upstream-image-model",
            &provider,
            ImageEndpoint::Generations,
            Some(log_context.clone()),
            Some(upstream_log),
        )
        .await
        .expect("image request should succeed");

        let detail = store
            .get_detail(log_id)
            .expect("read image log")
            .expect("image log exists");
        assert_eq!(detail.summary.status, "completed");
        assert_eq!(detail.summary.total_tokens, Some(18));
        assert!(detail.summary.latency_ms.is_some());
        assert!(detail.summary.upstream_request_body_bytes.is_some());
        assert_eq!(
            serde_json::from_str::<Value>(
                detail
                    .upstream_request_headers_json
                    .as_deref()
                    .expect("upstream headers")
            )
            .expect("upstream headers JSON")["authorization"],
            "<redacted>"
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                detail
                    .upstream_request_json
                    .as_deref()
                    .expect("upstream request")
            )
            .expect("upstream request JSON")["model"],
            "upstream-image-model"
        );
        let response_log = detail.response_json.as_deref().expect("response log");
        assert!(!response_log.contains("aW1hZ2U="));
        assert!(response_log.contains("<redacted>"));

        drop(log_context);
        drop(store);
        server.abort();
        let _ = std::fs::remove_file(db_path);
    }
}

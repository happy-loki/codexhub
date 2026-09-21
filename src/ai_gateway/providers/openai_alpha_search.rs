use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::Response,
};
use serde_json::Value;
use tracing::{debug, error};

use crate::ai_gateway::context::{GatewayContext, apply_upstream_headers};
use crate::ai_gateway::error::GatewayError;
use crate::ai_gateway::request_log::{self, RequestLogContext, RequestLogUpdate};
use crate::ai_gateway::{chatgpt_auth, config::ProviderConfig};

use super::{apply_total_request_timeout, execute_openai_request};

#[derive(Debug)]
pub struct AlphaSearchRequestInspection {
    pub model: String,
    pub session_id: Option<String>,
    pub body: Value,
}

pub fn inspect_request(raw_body: &Bytes) -> Result<AlphaSearchRequestInspection, GatewayError> {
    let body: Value = serde_json::from_slice(raw_body).map_err(|error| {
        GatewayError::bad_request(format!("invalid alpha search request: {error}"))
    })?;
    if !body.is_object() {
        return Err(GatewayError::bad_request(
            "alpha search request must be a JSON object",
        ));
    }

    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| GatewayError::bad_request("alpha search request model is required"))?
        .to_string();
    let session_id = body
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);

    Ok(AlphaSearchRequestInspection {
        model,
        session_id,
        body,
    })
}

pub async fn passthrough(
    client: &reqwest::Client,
    ctx: &GatewayContext,
    mut raw_body: Value,
    upstream_model: &str,
    provider: &ProviderConfig,
    raw_query: Option<&str>,
    log_context: Option<RequestLogContext>,
) -> Result<Response<Body>, GatewayError> {
    raw_body["model"] = Value::String(upstream_model.to_string());
    let request_body = serde_json::to_vec(&raw_body).map_err(|error| {
        GatewayError::bad_request(format!("serialize alpha search request: {error}"))
    })?;

    let mut url = reqwest::Url::parse(&chatgpt_auth::endpoint(provider, "/v1/alpha/search"))
        .map_err(|error| {
            GatewayError::upstream(
                StatusCode::BAD_GATEWAY,
                format!("invalid alpha search upstream URL: {error}"),
            )
        })?;
    if let Some(query) = raw_query.map(str::trim).filter(|query| !query.is_empty()) {
        url.set_query(Some(query));
    }

    let mut request_builder = client
        .post(url.clone())
        .header(CONTENT_TYPE, "application/json")
        .header("authorization", format!("Bearer {}", provider.api_key));
    if !ctx.upstream_headers.contains_key("accept") {
        request_builder = request_builder.header("accept", "application/json");
    }
    let mut request = apply_upstream_headers(
        apply_total_request_timeout(request_builder, provider.timeout_secs, false)
            .body(request_body.clone()),
        &ctx.upstream_headers,
    )
    .build()
    .map_err(|error| {
        error!(error = %error, "build upstream alpha search request failed");
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("build upstream alpha search request: {error}"),
        )
    })?;

    chatgpt_auth::authorize(client, &mut request, provider, None).await?;

    if let Some(log_context) = &log_context {
        let update = RequestLogUpdate {
            upstream_request_headers_json: log_context
                .details_enabled
                .then(|| request_log::headers_to_redacted_json(request.headers()))
                .flatten(),
            upstream_request_body_bytes: i64::try_from(request_body.len()).ok(),
            upstream_request_json: log_context
                .details_enabled
                .then(|| serde_json::to_string(&raw_body).ok())
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
        "proxying alpha search request"
    );

    let upstream_response = execute_openai_request(
        client,
        request,
        provider,
        "upstream alpha search request failed",
    )
    .await?;
    request_log::record_upstream_response_headers(
        log_context.as_ref(),
        upstream_response.headers(),
    );
    let status = StatusCode::from_u16(upstream_response.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream_response.headers().get(CONTENT_TYPE).cloned();
    let response_body = upstream_response.bytes().await.map_err(|error| {
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("read upstream alpha search response: {error}"),
        )
    })?;

    if let Some(log_context) = &log_context {
        let response_value = serde_json::from_slice::<Value>(&response_body).ok();
        let response_text = String::from_utf8_lossy(&response_body).into_owned();
        let update = RequestLogUpdate {
            status: Some(if status.is_success() {
                response_value
                    .as_ref()
                    .map(request_log::status_from_response_value)
                    .unwrap_or_else(|| "completed".to_string())
            } else {
                "failed".to_string()
            }),
            usage: Some(
                response_value
                    .as_ref()
                    .map(request_log::usage_from_response_value)
                    .unwrap_or_default(),
            ),
            latency_ms: Some(request_log::elapsed_ms(log_context.started_at)),
            error_message: (!status.is_success()).then(|| {
                alpha_search_error_message(status, response_value.as_ref(), &response_text)
            }),
            response_json: log_context.details_enabled.then_some(response_text),
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

fn alpha_search_error_message(
    status: StatusCode,
    response: Option<&Value>,
    response_text: &str,
) -> String {
    response
        .and_then(|value| value.get("error").unwrap_or(value).get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let message = response_text.trim();
            (!message.is_empty()).then(|| message.to_string())
        })
        .unwrap_or_else(|| format!("upstream alpha search returned HTTP {}", status.as_u16()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_request_preserves_unknown_fields() {
        let request = Bytes::from_static(
            br#"{"id":"search-1","model":"gpt-5.6-sol","commands":{"future_command":{"x":1}},"future":true}"#,
        );

        let inspection = inspect_request(&request).unwrap();

        assert_eq!(inspection.model, "gpt-5.6-sol");
        assert_eq!(inspection.session_id.as_deref(), Some("search-1"));
        assert_eq!(inspection.body["commands"]["future_command"]["x"], 1);
        assert_eq!(inspection.body["future"], true);
    }

    #[test]
    fn inspect_request_requires_model() {
        let error = inspect_request(&Bytes::from_static(br#"{"commands":{}}"#)).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("model"));
    }
}

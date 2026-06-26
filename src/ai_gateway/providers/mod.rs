pub mod anthropic_messages;
pub mod deepseek_chat;
pub mod openai_responses;

use std::{error::Error as _, time::Duration};

use axum::http::StatusCode;
use tracing::{error, warn};

use crate::ai_gateway::error::GatewayError;

const UPSTREAM_TRANSPORT_MAX_RETRIES: usize = 2;

pub(super) fn apply_total_request_timeout(
    builder: reqwest::RequestBuilder,
    timeout_secs: u64,
    stream: bool,
) -> reqwest::RequestBuilder {
    if stream {
        builder
    } else {
        builder.timeout(provider_timeout(timeout_secs))
    }
}

pub(super) async fn execute_stream_start(
    client: &reqwest::Client,
    request: reqwest::Request,
    timeout_secs: u64,
    error_log: &'static str,
) -> Result<reqwest::Response, GatewayError> {
    execute_upstream_request(client, request, timeout_secs, error_log).await
}

pub(super) async fn execute_upstream_request(
    client: &reqwest::Client,
    request: reqwest::Request,
    timeout_secs: u64,
    error_log: &'static str,
) -> Result<reqwest::Response, GatewayError> {
    let retry_template = request.try_clone();
    let mut next_request = Some(request);
    let mut retry_count = 0usize;

    loop {
        let Some(request) = next_request.take() else {
            return Err(GatewayError::upstream(
                StatusCode::BAD_GATEWAY,
                "upstream request could not be retried",
            ));
        };

        let response =
            tokio::time::timeout(provider_timeout(timeout_secs), client.execute(request))
                .await
                .map_err(|_| GatewayError::upstream_timeout())?;

        match response {
            Ok(response) => return Ok(response),
            Err(err) => {
                if should_retry_transport_error(&err)
                    && retry_count < UPSTREAM_TRANSPORT_MAX_RETRIES
                    && let Some(template) = retry_template.as_ref()
                    && let Some(retry_request) = template.try_clone()
                {
                    retry_count += 1;
                    warn!(
                        error = %reqwest_error_summary(&err),
                        retry_count,
                        max_retries = UPSTREAM_TRANSPORT_MAX_RETRIES,
                        "{error_log}; retrying upstream transport error"
                    );
                    tokio::time::sleep(upstream_transport_retry_delay(retry_count)).await;
                    next_request = Some(retry_request);
                    continue;
                }

                return Err(map_reqwest_error(err, error_log));
            }
        }
    }
}

fn map_reqwest_error(err: reqwest::Error, error_log: &'static str) -> GatewayError {
    if err.is_timeout() {
        GatewayError::upstream_timeout()
    } else {
        error!(error = %err, "{error_log}");
        GatewayError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("upstream error: {}", reqwest_error_summary(&err)),
        )
    }
}

fn should_retry_transport_error(err: &reqwest::Error) -> bool {
    err.status().is_none()
        && !err.is_timeout()
        && !err.is_decode()
        && (err.is_request() || err.is_connect() || err.is_body())
}

fn upstream_transport_retry_delay(retry_count: usize) -> Duration {
    match retry_count {
        0 | 1 => Duration::from_millis(200),
        2 => Duration::from_millis(500),
        _ => Duration::from_millis(1000),
    }
}

pub(super) fn reqwest_error_summary(err: &reqwest::Error) -> String {
    let mut parts = vec![err.to_string()];
    if err.is_connect() {
        parts.push("kind=connect".to_string());
    }
    if err.is_timeout() {
        parts.push("kind=timeout".to_string());
    }
    if err.is_body() {
        parts.push("kind=body".to_string());
    }
    if err.is_decode() {
        parts.push("kind=decode".to_string());
    }
    if err.is_request() {
        parts.push("kind=request".to_string());
    }
    if let Some(status) = err.status() {
        parts.push(format!("status={}", status.as_u16()));
    }

    let mut source = err.source();
    while let Some(err) = source {
        parts.push(format!("caused by: {err}"));
        source = err.source();
    }
    parts.join("; ")
}

pub(super) async fn ensure_success_response(
    provider_name: &str,
    response: reqwest::Response,
) -> Result<reqwest::Response, GatewayError> {
    let upstream_status = response.status();
    if upstream_status.is_success() {
        return Ok(response);
    }

    let status = StatusCode::from_u16(upstream_status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body_text = response.text().await.unwrap_or_default();
    Err(GatewayError::from_upstream_body(
        status,
        provider_name,
        &body_text,
    ))
}

fn provider_timeout(timeout_secs: u64) -> Duration {
    Duration::from_secs(timeout_secs.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_request_timeout_is_not_applied_to_streaming_requests() {
        let client = reqwest::Client::new();

        let streaming_request =
            apply_total_request_timeout(client.get("https://example.com/stream"), 7, true)
                .build()
                .unwrap();
        assert!(streaming_request.timeout().is_none());

        let unary_request =
            apply_total_request_timeout(client.get("https://example.com/json"), 7, false)
                .build()
                .unwrap();
        assert_eq!(unary_request.timeout(), Some(&Duration::from_secs(7)));
    }
}

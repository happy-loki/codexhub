use std::time::Instant;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::ai_gateway::{
    config::{AiGatewayConfig, ProviderConfig},
    context::GatewayContext,
    handler,
    request_log::{self, RequestLogContext, RequestLogStore, RequestLogUpdate},
};

pub(super) struct TurnLog {
    context: Option<RequestLogContext>,
    events: String,
    complete: bool,
    first_token: bool,
}

impl TurnLog {
    pub fn new(
        store: &RequestLogStore,
        config: &AiGatewayConfig,
        context: &GatewayContext,
        headers: &HeaderMap,
        provider: &ProviderConfig,
        request: &Value,
    ) -> Self {
        let context = config
            .request_logging_enabled
            .then(|| {
                let envelope = handler::GatewayRequestEnvelope {
                    model: request["model"].as_str()?.to_owned(),
                    stream: true,
                    prompt_cache_key: request["prompt_cache_key"].as_str().map(str::to_owned),
                };
                handler::insert_initial_log(
                    store,
                    context,
                    headers,
                    &envelope,
                    Some(provider),
                    request,
                    Instant::now(),
                    request_log::now_ms(),
                    config.request_log_details_enabled,
                )
            })
            .flatten();
        Self {
            context,
            events: String::new(),
            complete: false,
            first_token: false,
        }
    }

    pub fn request(&self, request: &Value, headers: &HeaderMap, response_headers: &HeaderMap) {
        let Some(context) = &self.context else {
            return;
        };
        self.update(RequestLogUpdate {
            upstream_request_body_bytes: request_log::json_body_size_bytes(request),
            upstream_request_json: context.details_enabled.then(|| request.to_string()),
            upstream_request_headers_json: context
                .details_enabled
                .then(|| request_log::headers_to_redacted_json(headers))
                .flatten(),
            ..Default::default()
        });
        request_log::record_upstream_response_headers(Some(context), response_headers);
    }

    pub fn event(&mut self, event: &Value) {
        let Some(context) = &self.context else {
            return;
        };
        let kind = event["type"].as_str().unwrap_or("");
        if context.details_enabled && self.events.len() < 512 * 1024 {
            let text = format!("data: {event}\n\n");
            let remaining = (512 * 1024 - self.events.len()).min(text.len());
            let mut end = remaining;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.events.push_str(&text[..end]);
        }
        if kind.starts_with("response.") && kind.ends_with(".delta") && !self.first_token {
            self.first_token = true;
            self.update(RequestLogUpdate {
                ttft_ms: Some(request_log::elapsed_ms(context.started_at)),
                ..Default::default()
            });
        }
        if super::is_terminal(event) {
            let response = event.get("response").unwrap_or(event);
            self.update(RequestLogUpdate {
                status: Some(if kind == "error" {
                    "failed".into()
                } else {
                    request_log::status_from_response_value(response)
                }),
                usage: Some(request_log::usage_from_response_value(response)),
                latency_ms: Some(request_log::elapsed_ms(context.started_at)),
                error_message: response
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                response_json: context.details_enabled.then(|| response.to_string()),
                ..Default::default()
            });
            self.complete = true;
        }
    }

    pub fn fail(&mut self, message: &str) {
        if !self.complete {
            self.update(RequestLogUpdate {
                status: Some("failed".into()),
                latency_ms: self
                    .context
                    .as_ref()
                    .map(|c| request_log::elapsed_ms(c.started_at)),
                error_message: Some(message.into()),
                ..Default::default()
            });
            self.complete = true;
        }
    }

    fn update(&self, update: RequestLogUpdate) {
        if let Some(context) = &self.context
            && let Err(error) = context.store.update_record(context.log_id, &update)
        {
            request_log::log_update_error(error);
        }
    }
}

impl Drop for TurnLog {
    fn drop(&mut self) {
        self.fail("WebSocket disconnected before response completed");
        if !self.events.is_empty() {
            let events = std::mem::take(&mut self.events);
            self.update(RequestLogUpdate {
                upstream_response_sse: Some(events),
                ..Default::default()
            });
        }
    }
}

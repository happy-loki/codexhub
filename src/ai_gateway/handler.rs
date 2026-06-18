use std::time::Instant;

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, header::ETAG},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::json;
use tracing::info;

use crate::app_state::SharedState;

use super::catalog::{configured_models_etag, configured_models_response_with_etag};
use super::config::{ProviderConfig, ProviderType};
use super::context::GatewayContext;
use super::error::GatewayError;
use super::model::GatewayRequest;
use super::providers::{deepseek_chat, openai_responses};
use super::request_log::{self, RequestLogContext, RequestLogRecord, RequestLogUpdate};
use super::router::resolve_provider;

/// POST /ai-gateway/v1/responses
pub async fn handle_responses(
    State(state): State<SharedState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let started_at = Instant::now();
    let created_at_ms = request_log::now_ms();
    let config = state.config.lock().await;
    let log_db_path = request_log::database_path(&config);
    let gw_config = config.ai_gateway.clone();
    let models_etag = configured_models_etag(&gw_config);
    drop(config);

    // 1. 解析请求 body
    let raw_body: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return GatewayError::bad_request(format!("invalid JSON: {e}")).into_response();
        }
    };
    let request: GatewayRequest = match serde_json::from_value(raw_body.clone()) {
        Ok(r) => r,
        Err(e) => {
            return GatewayError::bad_request(format!("invalid request: {e}")).into_response();
        }
    };

    // 2. 提取上下文
    let body_cache_key = request.prompt_cache_key.as_deref();
    let ctx = GatewayContext::extract(&headers, body_cache_key);

    // 3. 路由到 provider
    let provider = match resolve_provider(&request.model, &gw_config) {
        Ok(p) => p,
        Err(e) => {
            let log_context = insert_initial_log(
                &log_db_path,
                &ctx,
                &request,
                None,
                &raw_body,
                started_at,
                created_at_ms,
            );
            update_failed_log(&log_context, &e.message);
            return e.into_response();
        }
    };
    let log_context = insert_initial_log(
        &log_db_path,
        &ctx,
        &request,
        Some(provider),
        &raw_body,
        started_at,
        created_at_ms,
    );

    info!(
        model = %request.model,
        provider = %provider.name,
        provider_type = ?provider.provider_type,
        session_id = ?ctx.session_id,
        prompt_cache_key = %ctx.prompt_cache_key,
        stream = request.stream,
        "ai-gateway request routed"
    );

    // 4. 按 provider_type 分发
    match provider.provider_type {
        ProviderType::OpenAiResponses => {
            match openai_responses::passthrough(&ctx, raw_body, provider, log_context.clone()).await
            {
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
            match deepseek_chat::handle(&ctx, &request, provider, log_context.clone()).await {
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
    let config = state.config.lock().await;
    let db_path = request_log::database_path(&config);
    drop(config);

    match request_log::list_recent(&db_path, query.limit.unwrap_or(200)) {
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
    let config = state.config.lock().await;
    let db_path = request_log::database_path(&config);
    drop(config);

    match request_log::delete_all(&db_path) {
        Ok(deleted) => Json(json!({ "deleted": deleted })).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE /ai-gateway/request-logs/old?days=3
pub async fn handle_clear_old_request_logs(
    State(state): State<SharedState>,
    Query(query): Query<ClearOldRequestLogsQuery>,
) -> impl IntoResponse {
    let config = state.config.lock().await;
    let db_path = request_log::database_path(&config);
    drop(config);

    let days = query.days.unwrap_or(3).clamp(1, 3650);
    let cutoff_ms = request_log::now_ms().saturating_sub((days as i64) * 24 * 60 * 60 * 1000);
    match request_log::delete_older_than(&db_path, cutoff_ms) {
        Ok(deleted) => Json(json!({ "deleted": deleted })).into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

/// GET /ai-gateway/request-logs/{id}
pub async fn handle_request_log_detail(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let config = state.config.lock().await;
    let db_path = request_log::database_path(&config);
    drop(config);

    match request_log::get_detail(&db_path, id) {
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
    db_path: &std::path::Path,
    ctx: &GatewayContext,
    request: &GatewayRequest,
    provider: Option<&ProviderConfig>,
    raw_body: &serde_json::Value,
    started_at: Instant,
    created_at_ms: i64,
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
        request_json: serde_json::to_string(raw_body).ok(),
        upstream_request_json: None,
        response_json: None,
    };
    match request_log::insert_record(db_path, &record) {
        Ok(log_id) => Some(RequestLogContext {
            db_path: db_path.to_path_buf(),
            log_id,
            started_at,
        }),
        Err(err) => {
            request_log::log_insert_error(err);
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
    if let Err(err) = request_log::update_record(&log_context.db_path, log_context.log_id, &update)
    {
        request_log::log_update_error(err);
    }
}

fn provider_type_key(provider_type: &ProviderType) -> &'static str {
    match provider_type {
        ProviderType::OpenAiResponses => "responses",
        ProviderType::ChatCompletions => "chat_completions",
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

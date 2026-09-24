pub mod apply_patch_tool;
pub mod catalog;
pub mod chatgpt_auth;
#[allow(dead_code)]
pub mod codec;
pub mod config;
pub mod context;
pub mod encrypted_content;
pub mod error;
pub mod handler;
#[allow(dead_code)]
pub mod ir;
pub mod model;
pub mod providers;
pub mod request_log;
pub mod responses_compat;
pub mod responses_lite_tools;
pub mod router;
pub mod routing_state;
pub mod tool_names;
pub mod transform;
mod websocket;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, post},
};

use crate::app_state::SharedState;

/// 构建 AI Gateway 子路由（state 由父 Router 提供）。
pub fn router() -> Router<SharedState> {
    Router::new()
        .route(
            "/v1/responses",
            post(handler::handle_responses).get(websocket::handle_upgrade),
        )
        .route(
            "/v1/responses/compact",
            post(handler::handle_responses_compact),
        )
        .route("/v1/alpha/search", post(handler::handle_alpha_search))
        .route(
            "/v1/images/generations",
            post(handler::handle_image_generations),
        )
        .route("/v1/images/edits", post(handler::handle_image_edits))
        .route("/v1/models", get(handler::handle_models))
        .route(
            "/request-logs",
            get(handler::handle_request_logs).delete(handler::handle_clear_request_logs),
        )
        .route(
            "/request-logs/old",
            delete(handler::handle_clear_old_request_logs),
        )
        .route(
            "/request-logs/{id}",
            get(handler::handle_request_log_detail),
        )
        .layer(DefaultBodyLimit::disable())
}

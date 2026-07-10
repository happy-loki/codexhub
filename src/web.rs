use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{
        Request, StatusCode,
        header::{CACHE_CONTROL, EXPIRES, HeaderValue, PRAGMA},
    },
    middleware::{self, Next},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Serialize;
use serde_json::json;

use crate::{
    app_state::{FeishuWsState, ImAccountRuntimeState, SharedState, TelegramState, WechatState},
    chain_log, codex_app_config,
    config::AppConfig,
    remote_control_backend,
};

mod codex_app;
mod im_api;
mod oauth;
mod onboarding;
pub(crate) mod plugins;

pub async fn start_bridge_if_ready(state: &SharedState, event_message: &'static str) -> bool {
    im_api::start_bridge_task(state, im_api::BridgeStartMode::KeepExisting, event_message).await
}

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/oauth/authorize", get(oauth::oauth_authorize))
        .route("/oauth/token", post(oauth::oauth_token))
        .route("/api/status", get(status))
        .route("/api/gui/dashboard", get(gui_dashboard))
        .route("/api/shutdown", post(shutdown))
        .route("/api/config", get(get_config).post(save_config))
        .route(
            "/api/codex-app/configure",
            post(codex_app::configure_codex_app),
        )
        .route(
            "/api/codex-app/provider/websocket",
            post(codex_app::set_codex_app_provider_websocket),
        )
        .route(
            "/api/codex-app/provider/delete",
            post(codex_app::delete_codex_app_provider),
        )
        .route(
            "/api/codex-app/repair-gui-environment",
            post(codex_app::repair_codex_app_gui_environment),
        )
        .route(
            "/api/codex-app/uninstall",
            post(codex_app::uninstall_codex_app),
        )
        .route("/api/codex-app/status", get(codex_app::codex_app_status))
        .route(
            "/api/codex-app/fast-startup",
            post(codex_app::set_codex_app_fast_startup),
        )
        .route(
            "/api/codex-app/models/refresh",
            post(codex_app::refresh_codex_app_models),
        )
        .route(
            "/api/codex-app/sessions",
            get(codex_app::codex_app_sessions),
        )
        .route(
            "/api/codex-app/session/provider",
            post(codex_app::move_codex_app_session_provider),
        )
        .route("/api/bridge/start", post(im_api::start_bridge))
        .route("/api/bridge/stop", post(im_api::stop_bridge))
        .route(
            "/api/im-channel/enabled",
            post(im_api::set_im_channel_enabled),
        )
        .route("/api/im/accounts", get(im_api::im_accounts))
        .route(
            "/api/im/account/enabled",
            post(im_api::set_im_account_enabled),
        )
        .route("/api/im/account/delete", post(im_api::delete_im_account))
        .route(
            "/api/remote-control/backend-status",
            get(remote_control_backend_status),
        )
        .route(
            "/api/feishu/onboard/start",
            post(onboarding::feishu_onboard_start),
        )
        .route(
            "/api/feishu/onboard/poll",
            post(onboarding::feishu_onboard_poll),
        )
        .route("/api/feishu/bot", get(im_api::feishu_bot_status))
        .route("/api/telegram/bot", get(im_api::telegram_bot_status))
        .route(
            "/api/telegram/configure",
            post(im_api::configure_telegram_bot),
        )
        .route(
            "/api/wechat/onboard/start",
            post(onboarding::wechat_onboard_start),
        )
        .route(
            "/api/wechat/onboard/poll",
            post(onboarding::wechat_onboard_poll),
        )
        .route("/api/wechat/bot", get(im_api::wechat_bot_status))
        .route("/api/events", get(events))
        .merge(plugins::router())
        .merge(remote_control_backend::router())
        .nest("/ai-gateway", crate::ai_gateway::router())
        .layer(middleware::from_fn(access_log))
        .with_state(state)
}

async fn access_log(request: Request<Body>, next: Next) -> impl IntoResponse {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started = std::time::Instant::now();
    let mut response = next.run(request).await;
    let status = response.status();
    let elapsed_ms = started.elapsed().as_millis();
    if path.starts_with("/backend-api/") || path.starts_with("/api/") {
        let headers = response.headers_mut();
        headers.insert(
            CACHE_CONTROL,
            HeaderValue::from_static("no-store, no-cache, max-age=0, must-revalidate"),
        );
        headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
        headers.insert(EXPIRES, HeaderValue::from_static("0"));
    }
    chain_log::write_line(format!(
        "[http] method={} path={} status={} elapsed_ms={}",
        method,
        path,
        status.as_u16(),
        elapsed_ms
    ));
    tracing::info!(
        target: "codexhub::http",
        method = %method,
        path,
        status = status.as_u16(),
        elapsed_ms,
        "http request"
    );
    response
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    service: String,
    pid: u32,
    instance_id: String,
    started_at_ms: u64,
    running: bool,
    bind: String,
    local_connection_mode: crate::config::LocalConnectionMode,
    outbound_proxy_mode: crate::config::OutboundProxyMode,
    codex_app_fast_startup: bool,
    state_path: String,
    feishu_ws: FeishuWsState,
    telegram: TelegramState,
    wechat: WechatState,
    im_accounts: Vec<ImAccountRuntimeState>,
}

async fn status(State(state): State<SharedState>) -> Json<StatusResponse> {
    Json(status_snapshot(&state).await)
}

async fn status_snapshot(state: &SharedState) -> StatusResponse {
    let running = state
        .bridge_task
        .lock()
        .await
        .as_ref()
        .map(|handle| !handle.is_finished())
        .unwrap_or(false);
    let config = state.config.lock().await;
    let feishu_ws = state.feishu_ws.lock().await.clone();
    let telegram = state.telegram.lock().await.clone();
    let wechat = state.wechat.lock().await.clone();
    let im_accounts = state
        .im_accounts
        .lock()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    StatusResponse {
        service: state.daemon_identity.service.clone(),
        pid: state.daemon_identity.pid,
        instance_id: state.daemon_identity.instance_id.clone(),
        started_at_ms: state.daemon_identity.started_at_ms,
        running,
        bind: config.bind.clone(),
        local_connection_mode: config.local_connection_mode,
        outbound_proxy_mode: config.outbound_proxy.mode,
        codex_app_fast_startup: config.codex_app_fast_startup,
        state_path: config.state_path.to_string_lossy().to_string(),
        feishu_ws,
        telegram,
        wechat,
        im_accounts,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GuiDashboardResponse {
    status: StatusResponse,
    remote: remote_control_backend::RemoteControlStatusResponse,
    codex_app: codex_app_config::CodexAppConfigStatus,
    im_accounts: im_api::ImAccountsResponse,
    ai_gateway: crate::ai_gateway::config::AiGatewayConfig,
}

async fn gui_dashboard(State(state): State<SharedState>) -> Json<GuiDashboardResponse> {
    let status = status_snapshot(&state).await;
    let remote = remote_control_backend::status_snapshot(&state).await;
    let codex_app = codex_app::codex_app_status_snapshot(&state).await;
    let im_accounts = im_api::im_accounts_snapshot(&state).await;
    let ai_gateway = state.config.lock().await.ai_gateway.clone();
    Json(GuiDashboardResponse {
        status,
        remote,
        codex_app,
        im_accounts,
        ai_gateway,
    })
}

async fn shutdown(State(state): State<SharedState>) -> impl IntoResponse {
    state
        .push_event("warn", "shutdown_requested", "daemon shutdown requested")
        .await;
    im_api::stop_bridge_task(&state).await;
    let accepted = state.request_shutdown().await;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "accepted": accepted })),
    )
}

async fn get_config(State(state): State<SharedState>) -> Json<AppConfig> {
    Json(state.config.lock().await.clone())
}

async fn save_config(
    State(state): State<SharedState>,
    Json(config): Json<AppConfig>,
) -> impl IntoResponse {
    if let Err(err) = crate::outbound_http::validate_for_local_port(
        &config.outbound_proxy,
        config.local_listen_port(),
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": err.to_string() })),
        );
    }
    if let Err(err) = config.save(&state.config_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        );
    }
    if let Err(err) = crate::outbound_http::init(&config.outbound_proxy, config.local_listen_port())
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        );
    }
    *state.config.lock().await = config;
    state
        .push_event("info", "config_saved", "configuration saved")
        .await;
    (StatusCode::OK, Json(json!({ "ok": true })))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteControlBackendStatusResponse {
    available: bool,
    enabled: bool,
    remote_control_base_url: String,
    remote_control_connected: bool,
    remote_control_initialized: bool,
    server_name: Option<String>,
    environment_id: Option<String>,
    installation_id: Option<String>,
    current_thread_id: Option<String>,
    feishu_configured: bool,
    telegram_configured: bool,
    wechat_configured: bool,
    reason: Option<String>,
}

async fn remote_control_backend_status(
    State(state): State<SharedState>,
) -> Json<RemoteControlBackendStatusResponse> {
    let config = state.config.lock().await.clone();
    let remote = remote_control_backend::status_snapshot(&state).await;
    let feishu_configured = im_api::feishu_configured(&config);
    let telegram_configured = im_api::telegram_configured(&config);
    let wechat_configured = im_api::wechat_configured(&config);
    let im_configured = im_api::im_bridge_configured(&config);
    let reason = if !config.bridge.enabled {
        Some("bridge disabled".to_string())
    } else if !im_configured {
        Some("No enabled IM channel is configured".to_string())
    } else {
        None
    };
    Json(RemoteControlBackendStatusResponse {
        available: config.bridge.enabled && im_configured,
        enabled: config.bridge.enabled,
        remote_control_base_url: config.remote_control_base_url(),
        remote_control_connected: remote.connected,
        remote_control_initialized: remote.initialized,
        server_name: remote.server_name,
        environment_id: remote.environment_id,
        installation_id: remote.installation_id,
        current_thread_id: remote.current_thread_id,
        feishu_configured,
        telegram_configured,
        wechat_configured,
        reason,
    })
}

async fn events(State(state): State<SharedState>) -> impl IntoResponse {
    let events = state.events.lock().await.clone();
    Json(events)
}

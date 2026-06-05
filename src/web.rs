use anyhow::Context;
use axum::{
    Json, Router,
    body::Body,
    extract::{Form, Path as AxumPath, Query, State},
    http::{
        Request, StatusCode,
        header::{CACHE_CONTROL, EXPIRES, HeaderValue, PRAGMA},
    },
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
};
use base64::Engine;
use qrcode::{QrCode, render::svg};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    app_state::{
        FeishuWsState, ImAccountRuntimeState, SharedState, TelegramState, WechatOnboardSession,
        WechatState, im_account_key,
    },
    bridge, chain_log,
    codex_app_config::{self, ConfigureCodexAppOptions},
    config::AppConfig,
    im::feishu::{FeishuApi, FeishuSettings},
    im::telegram::{api::TelegramApi, types::TelegramSettings},
    im::wechat::{
        api::WechatApi,
        store as wechat_store,
        types::{DEFAULT_WECHAT_API_BASE, WechatSettings},
    },
    remote_control_backend,
    types::ImPlatformKind,
};

pub async fn start_bridge_if_ready(state: &SharedState, event_message: &'static str) -> bool {
    start_bridge_task(state, BridgeStartMode::KeepExisting, event_message).await
}

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/oauth/authorize", get(oauth_authorize))
        .route("/oauth/token", post(oauth_token))
        .route("/api/status", get(status))
        .route("/api/shutdown", post(shutdown))
        .route("/api/config", get(get_config).post(save_config))
        .route("/api/codex-app/configure", post(configure_codex_app))
        .route(
            "/api/codex-app/provider/delete",
            post(delete_codex_app_provider),
        )
        .route(
            "/api/codex-app/repair-gui-environment",
            post(repair_codex_app_gui_environment),
        )
        .route("/api/codex-app/uninstall", post(uninstall_codex_app))
        .route("/api/codex-app/status", get(codex_app_status))
        .route("/api/bridge/start", post(start_bridge))
        .route("/api/bridge/stop", post(stop_bridge))
        .route("/api/im-channel/enabled", post(set_im_channel_enabled))
        .route("/api/im/accounts", get(im_accounts))
        .route("/api/im/account/enabled", post(set_im_account_enabled))
        .route("/api/im/account/delete", post(delete_im_account))
        .route(
            "/api/remote-control/backend-status",
            get(remote_control_backend_status),
        )
        .route("/api/feishu/onboard/start", post(feishu_onboard_start))
        .route("/api/feishu/onboard/poll", post(feishu_onboard_poll))
        .route("/api/feishu/bot", get(feishu_bot_status))
        .route("/api/telegram/bot", get(telegram_bot_status))
        .route("/api/telegram/configure", post(configure_telegram_bot))
        .route("/api/wechat/onboard/start", post(wechat_onboard_start))
        .route("/api/wechat/onboard/poll", post(wechat_onboard_poll))
        .route("/api/wechat/bot", get(wechat_bot_status))
        .route("/backend-api/plugins/list", get(plugin_legacy_list))
        .route("/backend-api/plugins/featured", get(plugin_legacy_featured))
        .route(
            "/backend-api/plugins/{plugin_id}/enable",
            post(plugin_legacy_enable),
        )
        .route("/backend-api/ps/plugins/list", get(plugin_catalog_page))
        .route(
            "/backend-api/ps/plugins/workspace/shared",
            get(plugin_empty_page),
        )
        .route(
            "/backend-api/ps/plugins/installed",
            get(plugin_installed_page),
        )
        .route("/backend-api/ps/plugins/{plugin_id}", get(plugin_detail))
        .route(
            "/backend-api/ps/plugins/{plugin_id}/install",
            post(plugin_install),
        )
        .route("/api/events", get(events))
        .merge(remote_control_backend::router())
        .layer(middleware::from_fn(access_log))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("web/index.html"))
}

#[derive(Deserialize)]
struct OAuthAuthorizeQuery {
    redirect_uri: String,
    state: Option<String>,
    current_workspace_id: Option<String>,
    allowed_workspace_id: Option<String>,
}

#[derive(Deserialize)]
struct OAuthTokenRequest {
    code: String,
}

async fn oauth_authorize(Query(query): Query<OAuthAuthorizeQuery>) -> impl IntoResponse {
    let account_id = query
        .current_workspace_id
        .or(query.allowed_workspace_id)
        .unwrap_or_else(|| "acct_codex_remote_local".to_string());
    let code = local_step_up_code(&account_id);
    let mut redirect_uri = match reqwest::Url::parse(&query.redirect_uri) {
        Ok(url) => url,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid redirect_uri").into_response(),
    };
    {
        let mut pairs = redirect_uri.query_pairs_mut();
        pairs.append_pair("code", &code);
        if let Some(state) = query.state {
            pairs.append_pair("state", &state);
        }
    }
    Redirect::temporary(redirect_uri.as_str()).into_response()
}

async fn oauth_token(Form(request): Form<OAuthTokenRequest>) -> impl IntoResponse {
    let account_id = account_id_from_step_up_code(&request.code)
        .unwrap_or_else(|| "acct_codex_remote_local".to_string());
    let user_id = "user_codex_remote_local";
    let account_user_id = format!("{user_id}__{account_id}");
    let now = unix_now();
    let token = jwt_none(&serde_json::json!({
        "iss": "codex-remote-local",
        "aud": ["https://api.openai.com/v1"],
        "iat": now,
        "nbf": now,
        "exp": now + 5 * 60,
        "pwd_auth_time": now * 1000,
        "scope": "codex.remote_control.enroll",
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "account_id": account_id,
            "chatgpt_account_user_id": account_user_id,
            "account_user_id": account_user_id,
            "user_id": user_id,
        },
    }));
    Json(serde_json::json!({ "access_token": token })).into_response()
}

fn local_step_up_code(account_id: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "account_id": account_id,
            "iat": unix_now(),
        }))
        .unwrap_or_default(),
    )
}

fn account_id_from_step_up_code(code: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(code)
        .ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    value
        .get("account_id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn jwt_none(payload: &serde_json::Value) -> String {
    format!(
        "{}.{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({ "alg": "none", "typ": "JWT" }))
                .unwrap_or_default()
        ),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(payload).unwrap_or_default()),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
    )
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
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
        target: "codex_remote::http",
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
    running: bool,
    bind: String,
    state_path: String,
    feishu_ws: FeishuWsState,
    telegram: TelegramState,
    wechat: WechatState,
    im_accounts: Vec<ImAccountRuntimeState>,
}

async fn status(State(state): State<SharedState>) -> Json<StatusResponse> {
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
    Json(StatusResponse {
        running,
        bind: config.bind.clone(),
        state_path: config.state_path.to_string_lossy().to_string(),
        feishu_ws,
        telegram,
        wechat,
        im_accounts,
    })
}

async fn shutdown(State(state): State<SharedState>) -> impl IntoResponse {
    state
        .push_event("warn", "shutdown_requested", "daemon shutdown requested")
        .await;
    stop_bridge_task(&state).await;
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
    if let Err(err) = config.save(&state.config_path) {
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureCodexAppRequest {
    codex_home: Option<String>,
    provider_name: Option<String>,
    provider_base_url: Option<String>,
    provider_key: Option<String>,
    model: Option<String>,
    activate: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteCodexAppProviderRequest {
    provider_name: String,
}

async fn configure_codex_app(
    State(state): State<SharedState>,
    payload: Option<Json<ConfigureCodexAppRequest>>,
) -> impl IntoResponse {
    let request = payload.map(|Json(value)| value);
    let config = state.config.lock().await.clone();
    let codex_home = request
        .as_ref()
        .and_then(|value| value.codex_home.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from);
    let provider_base_url = request
        .as_ref()
        .and_then(|value| value.provider_base_url.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let provider_name = request
        .as_ref()
        .and_then(|value| value.provider_name.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let provider_key = request
        .as_ref()
        .and_then(|value| value.provider_key.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let model = request
        .as_ref()
        .and_then(|value| value.model.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let activate_provider = request
        .as_ref()
        .and_then(|value| value.activate)
        .unwrap_or(true);

    let backend_url = config.remote_control_base_url();
    state
        .push_event(
            "info",
            "codex_app_configure_start",
            format!(
                "provider={} activate_provider={}",
                provider_name.as_deref().unwrap_or_default(),
                activate_provider
            ),
        )
        .await;
    match codex_app_config::configure_codex_app(ConfigureCodexAppOptions {
        codex_home,
        backend_url: backend_url.clone(),
        account_id: "acct_codex_remote_local".to_string(),
        user_id: "user_codex_remote_local".to_string(),
        email: "codex-remote-local@example.local".to_string(),
        plan_type: "pro".to_string(),
        provider_name,
        provider_base_url,
        provider_key,
        model,
        activate_provider,
    }) {
        Ok(report) => {
            let gui_api_base = codex_app_config::inspect_gui_api_base_url(&backend_url);
            let remote_control_switch = report.remote_control_switch.clone();
            state
                .push_event(
                    "info",
                    "codex_app_configured",
                    format!(
                        "codex_home={} config={} auth={} gui_api_base={} remote_control_switch={}",
                        report.codex_home.display(),
                        report.config_path.display(),
                        report.auth_path.display(),
                        gui_api_base.value.as_deref().unwrap_or_default(),
                        remote_control_switch.configured
                    ),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "codexHome": report.codex_home.to_string_lossy().to_string(),
                    "configPath": report.config_path.to_string_lossy().to_string(),
                    "authPath": report.auth_path.to_string_lossy().to_string(),
                    "backendUrl": report.backend_url,
                    "guiApiBase": gui_api_base,
                    "remoteControlSwitch": remote_control_switch,
                })),
            )
        }
        Err(err) => {
            state
                .push_event("error", "codex_app_configure_failed", err.to_string())
                .await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            )
        }
    }
}

async fn delete_codex_app_provider(
    State(state): State<SharedState>,
    Json(request): Json<DeleteCodexAppProviderRequest>,
) -> impl IntoResponse {
    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    match codex_app_config::delete_codex_app_provider(None, request.provider_name.trim()) {
        Ok(config_path) => {
            let status = codex_app_config::inspect_codex_app_config(None, &backend_url);
            state
                .push_event(
                    "info",
                    "codex_app_provider_deleted",
                    format!(
                        "config={} provider={}",
                        config_path.display(),
                        request.provider_name.trim()
                    ),
                )
                .await;
            (
                StatusCode::OK,
                Json(
                    json!({ "ok": true, "configPath": config_path.to_string_lossy().to_string(), "status": status }),
                ),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

async fn uninstall_codex_app(State(state): State<SharedState>) -> impl IntoResponse {
    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    match codex_app_config::uninstall_codex_app(None, &backend_url) {
        Ok(report) => {
            state
                .push_event(
                    "info",
                    "codex_app_uninstalled",
                    format!(
                        "codex_home={} config={} auth={} removed_chatgpt_base_url={} removed_model_provider={} removed_auth={} gui_api_base={}",
                        report.codex_home.display(),
                        report.config_path.display(),
                        report.auth_path.display(),
                        report.removed_chatgpt_base_url,
                        report.removed_model_provider,
                        report.removed_auth,
                        report.gui_api_base.value.as_deref().unwrap_or_default()
                    ),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({ "ok": true, "report": report })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

async fn repair_codex_app_gui_environment(State(state): State<SharedState>) -> impl IntoResponse {
    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    let status = codex_app_config::inspect_codex_app_config(None, &backend_url);
    if !status.config_ok || !status.auth_ok {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "Codex App local config is not ready; write config first",
                "status": status,
            })),
        );
    }

    let remote_control_switch = match codex_app_config::enable_codex_app_remote_control_switch(None)
    {
        Ok(status) => status,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "ok": false,
                    "error": err.to_string(),
                    "status": status,
                })),
            );
        }
    };
    let gui_api_base = codex_app_config::configure_gui_environment(&backend_url);
    state
        .push_event(
            "info",
            "codex_app_gui_environment_repaired",
            format!(
                "gui_api_base={} login_issuer={} remote_control_switch={}",
                gui_api_base.value.as_deref().unwrap_or_default(),
                gui_api_base
                    .login_issuer_value
                    .as_deref()
                    .unwrap_or_default(),
                remote_control_switch.configured
            ),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "guiApiBase": gui_api_base,
            "remoteControlSwitch": remote_control_switch,
        })),
    )
}

async fn codex_app_status(
    State(state): State<SharedState>,
) -> Json<codex_app_config::CodexAppConfigStatus> {
    let config = state.config.lock().await.clone();
    Json(codex_app_config::inspect_codex_app_config(
        None,
        &config.remote_control_base_url(),
    ))
}

async fn start_bridge(State(state): State<SharedState>) -> impl IntoResponse {
    {
        let mut config = state.config.lock().await;
        config.bridge.enabled = true;
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    }
    let running = start_bridge_task(
        &state,
        BridgeStartMode::KeepExisting,
        "bridge start requested",
    )
    .await;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "running": running })),
    )
}

async fn stop_bridge(State(state): State<SharedState>) -> impl IntoResponse {
    {
        let mut config = state.config.lock().await;
        config.bridge.enabled = false;
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    }
    stop_bridge_task(&state).await;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "running": false })),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetImChannelEnabledRequest {
    channel: String,
    enabled: bool,
}

async fn set_im_channel_enabled(
    State(state): State<SharedState>,
    Json(request): Json<SetImChannelEnabledRequest>,
) -> impl IntoResponse {
    let channel = request.channel.trim().to_ascii_lowercase();
    let should_run = {
        let mut config = state.config.lock().await;
        match channel.as_str() {
            "feishu" => {
                if request.enabled && !feishu_configured(&config) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "ok": false, "error": "Feishu is not configured" })),
                    );
                }
                for account in &mut config.feishu_accounts {
                    account.enabled = request.enabled;
                }
                config.feishu.enabled = request.enabled;
            }
            "telegram" => {
                if request.enabled && !telegram_configured(&config) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "ok": false, "error": "Telegram is not configured" })),
                    );
                }
                for account in &mut config.telegram_accounts {
                    account.enabled = request.enabled;
                }
                config.telegram.enabled = request.enabled;
            }
            "wechat" => {
                if request.enabled && !wechat_configured(&config) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "ok": false, "error": "WeChat is not configured" })),
                    );
                }
                for account in &mut config.wechat_accounts {
                    account.enabled = request.enabled;
                }
                config.wechat.enabled = request.enabled;
            }
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "ok": false, "error": "unknown IM channel" })),
                );
            }
        }
        config.bridge.enabled = im_bridge_configured(&config);
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
        config.bridge.enabled
    };

    if should_run {
        start_bridge_task(
            &state,
            BridgeStartMode::Restart,
            "bridge restarted after IM channel toggle",
        )
        .await;
    } else {
        stop_bridge_task(&state).await;
    }

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "channel": channel,
            "enabled": request.enabled,
            "running": should_run,
        })),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImAccountItem {
    platform: String,
    account_id: String,
    display_name: Option<String>,
    enabled: bool,
    configured: bool,
    secret_set: bool,
    connecting: bool,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
    last_event_at_ms: Option<u128>,
    last_inbound_at_ms: Option<u128>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImAccountsResponse {
    accounts: Vec<ImAccountItem>,
}

async fn im_accounts(State(state): State<SharedState>) -> Json<ImAccountsResponse> {
    let config = state.config.lock().await.clone();
    let runtime = state.im_accounts.lock().await.clone();
    Json(ImAccountsResponse {
        accounts: im_account_items(&config, &runtime),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetImAccountEnabledRequest {
    platform: String,
    account_id: String,
    enabled: bool,
}

async fn set_im_account_enabled(
    State(state): State<SharedState>,
    Json(request): Json<SetImAccountEnabledRequest>,
) -> impl IntoResponse {
    let platform = request.platform.trim().to_ascii_lowercase();
    let account_id = request.account_id.trim().to_string();
    if account_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing accountId" })),
        );
    }
    let should_run = {
        let mut config = state.config.lock().await;
        config.migrate_legacy_im_accounts();
        if !config.set_im_account_enabled(&platform, &account_id, request.enabled) {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": "IM account not found" })),
            );
        }
        set_legacy_im_account_enabled(&mut config, &platform, &account_id, request.enabled);
        config.bridge.enabled = im_bridge_configured(&config);
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
        config.bridge.enabled
    };
    if should_run {
        start_bridge_task(
            &state,
            BridgeStartMode::Restart,
            "bridge restarted after IM account toggle",
        )
        .await;
    } else {
        stop_bridge_task(&state).await;
    }
    (
        StatusCode::OK,
        Json(
            json!({ "ok": true, "platform": platform, "accountId": account_id, "enabled": request.enabled }),
        ),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteImAccountRequest {
    platform: String,
    account_id: String,
}

async fn delete_im_account(
    State(state): State<SharedState>,
    Json(request): Json<DeleteImAccountRequest>,
) -> impl IntoResponse {
    let platform = request.platform.trim().to_ascii_lowercase();
    let account_id = request.account_id.trim().to_string();
    if account_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing accountId" })),
        );
    }
    let should_run = {
        let mut config = state.config.lock().await;
        config.migrate_legacy_im_accounts();
        let removed = config.remove_im_account(&platform, &account_id);
        clear_legacy_im_account(&mut config, &platform, &account_id);
        if !removed {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": "IM account not found" })),
            );
        }
        config.bridge.enabled = im_bridge_configured(&config);
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
        config.bridge.enabled
    };
    clear_im_account_bindings(&state, &platform, &account_id).await;
    if should_run {
        start_bridge_task(
            &state,
            BridgeStartMode::Restart,
            "bridge restarted after IM account deletion",
        )
        .await;
    } else {
        stop_bridge_task(&state).await;
    }
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "platform": platform, "accountId": account_id })),
    )
}

async fn stop_bridge_task(state: &SharedState) {
    let mut task = state.bridge_task.lock().await;
    if let Some(handle) = task.take() {
        handle.abort();
    }
    state.runtime.lock().await.invalidate_bridge_generation();
    {
        let mut ws = state.feishu_ws.lock().await;
        ws.connecting = false;
        ws.connected = false;
    }
    {
        let mut wechat = state.wechat.lock().await;
        wechat.polling = false;
        wechat.connected = false;
    }
    {
        let mut telegram = state.telegram.lock().await;
        telegram.polling = false;
        telegram.connected = false;
    }
    {
        let mut accounts = state.im_accounts.lock().await;
        for account in accounts.values_mut() {
            account.connecting = false;
            account.polling = false;
            account.connected = false;
        }
    }
    state
        .push_event("warn", "bridge_stopped", "bridge task aborted")
        .await;
}

#[derive(Clone, Copy)]
enum BridgeStartMode {
    KeepExisting,
    Restart,
}

async fn start_bridge_task(
    state: &SharedState,
    mode: BridgeStartMode,
    event_message: &'static str,
) -> bool {
    let config = state.config.lock().await.clone();
    if !config.bridge.enabled {
        state
            .push_event("warn", "bridge_disabled", "bridge disabled by config")
            .await;
        return false;
    }
    if !im_bridge_configured(&config) {
        state
            .push_event(
                "warn",
                "bridge_waiting_for_im_config",
                "bridge is waiting for Feishu, Telegram, or WeChat configuration",
            )
            .await;
        return false;
    }

    let restart = matches!(mode, BridgeStartMode::Restart);
    let mut aborted_existing = false;
    {
        let mut task = state.bridge_task.lock().await;
        let running = task
            .as_ref()
            .map(|handle| !handle.is_finished())
            .unwrap_or(false);
        if running && !restart {
            return true;
        }
        if let Some(handle) = task.take()
            && !handle.is_finished()
        {
            handle.abort();
            aborted_existing = true;
        }
        let bridge_state = state.clone();
        *task = Some(tokio::spawn(async move {
            bridge::start_bridge(bridge_state).await;
        }));
    }

    if restart || aborted_existing {
        state.runtime.lock().await.invalidate_bridge_generation();
        let mut ws = state.feishu_ws.lock().await;
        ws.connecting = false;
        ws.connected = false;
        ws.last_error = None;
        let mut wechat = state.wechat.lock().await;
        wechat.polling = false;
        wechat.connected = false;
        wechat.last_error = None;
        let mut telegram = state.telegram.lock().await;
        telegram.polling = false;
        telegram.connected = false;
        telegram.last_error = None;
        let mut accounts = state.im_accounts.lock().await;
        for account in accounts.values_mut() {
            account.connecting = false;
            account.polling = false;
            account.connected = false;
            account.last_error = None;
        }
    }
    state
        .push_event("info", "bridge_start_requested", event_message)
        .await;
    true
}

fn feishu_configured(config: &AppConfig) -> bool {
    config
        .effective_feishu_accounts()
        .iter()
        .any(|account| account.is_configured())
}

fn telegram_configured(config: &AppConfig) -> bool {
    config
        .effective_telegram_accounts()
        .iter()
        .any(|account| account.is_configured())
}

fn wechat_configured(config: &AppConfig) -> bool {
    config
        .effective_wechat_accounts()
        .iter()
        .any(|account| account.is_configured())
}

fn feishu_active(config: &AppConfig) -> bool {
    config
        .effective_feishu_accounts()
        .iter()
        .any(|account| account.is_active())
}

fn telegram_active(config: &AppConfig) -> bool {
    config
        .effective_telegram_accounts()
        .iter()
        .any(|account| account.is_active())
}

fn wechat_active(config: &AppConfig) -> bool {
    config
        .effective_wechat_accounts()
        .iter()
        .any(|account| account.is_active())
}

fn im_bridge_configured(config: &AppConfig) -> bool {
    feishu_active(config) || telegram_active(config) || wechat_active(config)
}

fn im_account_items(
    config: &AppConfig,
    runtime: &HashMap<String, ImAccountRuntimeState>,
) -> Vec<ImAccountItem> {
    let mut accounts = Vec::new();
    for account in config.effective_feishu_accounts() {
        accounts.push(im_account_item(
            ImPlatformKind::Feishu,
            &account.account_id,
            non_empty_string(&account.display_name)
                .or_else(|| non_empty_string(&account.app_id))
                .or_else(|| Some("飞书机器人".to_string())),
            account.enabled,
            account.is_configured(),
            account.is_configured(),
            runtime,
        ));
    }
    for account in config.effective_telegram_accounts() {
        accounts.push(im_account_item(
            ImPlatformKind::Telegram,
            &account.account_id,
            non_empty_string(&account.display_name).or_else(|| Some("Telegram 机器人".to_string())),
            account.enabled,
            account.is_configured(),
            !account.bot_token.trim().is_empty(),
            runtime,
        ));
    }
    for account in config.effective_wechat_accounts() {
        accounts.push(im_account_item(
            ImPlatformKind::Wechat,
            &account.account_id,
            non_empty_string(&account.display_name).or_else(|| Some("微信机器人".to_string())),
            account.enabled,
            account.is_configured(),
            !account.bot_token.trim().is_empty(),
            runtime,
        ));
    }
    accounts
}

fn im_account_item(
    platform: ImPlatformKind,
    account_id: &str,
    display_name: Option<String>,
    enabled: bool,
    configured: bool,
    secret_set: bool,
    runtime: &HashMap<String, ImAccountRuntimeState>,
) -> ImAccountItem {
    let runtime = runtime.get(&im_account_key(platform, account_id));
    ImAccountItem {
        platform: platform.key().to_string(),
        account_id: account_id.to_string(),
        display_name,
        enabled,
        configured,
        secret_set,
        connecting: runtime.is_some_and(|state| state.connecting),
        polling: runtime.is_some_and(|state| state.polling),
        connected: runtime.is_some_and(|state| state.connected),
        last_error: runtime.and_then(|state| state.last_error.clone()),
        last_event_at_ms: runtime.and_then(|state| state.last_event_at_ms),
        last_inbound_at_ms: runtime.and_then(|state| state.last_inbound_at_ms),
    }
}

fn set_legacy_im_account_enabled(
    config: &mut AppConfig,
    platform: &str,
    account_id: &str,
    enabled: bool,
) {
    match platform {
        "feishu"
            if config.feishu.account_id.trim() == account_id
                || (config.feishu.account_id.trim().is_empty()
                    && config.bridge.account_id.trim() == account_id) =>
        {
            config.feishu.enabled = enabled
        }
        "telegram" if config.telegram.account_id.trim() == account_id => {
            config.telegram.enabled = enabled
        }
        "wechat" if config.wechat.account_id.trim() == account_id => {
            config.wechat.enabled = enabled
        }
        _ => {}
    }
}

fn clear_legacy_im_account(config: &mut AppConfig, platform: &str, account_id: &str) {
    match platform {
        "feishu"
            if config.feishu.account_id.trim() == account_id
                || (config.feishu.account_id.trim().is_empty()
                    && (config.feishu.app_id.trim() == account_id
                        || config.bridge.account_id.trim() == account_id)) =>
        {
            config.feishu = Default::default();
        }
        "telegram"
            if config.telegram.account_id.trim() == account_id
                || (config.telegram.account_id.trim().is_empty() && account_id == "telegram") =>
        {
            config.telegram = Default::default();
        }
        "wechat" if config.wechat.account_id.trim() == account_id => {
            config.wechat = Default::default();
        }
        _ => {}
    }
}

async fn clear_im_account_bindings(state: &SharedState, platform: &str, account_id: &str) {
    {
        let mut runtime = state.runtime.lock().await;
        let removed = runtime
            .route_by_thread
            .iter()
            .filter_map(|(thread_id, route)| {
                (route.platform.key() == platform && route.account_id == account_id)
                    .then(|| (thread_id.clone(), route.clone()))
            })
            .collect::<Vec<_>>();
        runtime.route_by_thread.retain(|_, route| {
            !(route.platform.key() == platform && route.account_id == account_id)
        });
        for (thread_id, route) in removed {
            chain_log::write_line(format!(
                "[im_route] level=warn event=unbind_account reason=clear_im_account_bindings thread={} platform={} account={} chat={} conversation={}",
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            ));
        }
    }
    if let Some(kind) = im_platform_from_key(platform) {
        state
            .im_accounts
            .lock()
            .await
            .remove(&im_account_key(kind, account_id));
    }
}

fn im_platform_from_key(platform: &str) -> Option<ImPlatformKind> {
    match platform {
        "feishu" => Some(ImPlatformKind::Feishu),
        "telegram" => Some(ImPlatformKind::Telegram),
        "wechat" => Some(ImPlatformKind::Wechat),
        _ => None,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeishuBotStatus {
    configured: bool,
    enabled: bool,
    app_id: Option<String>,
    display_name: Option<String>,
    allowed_open_ids: usize,
    error: Option<String>,
}

async fn feishu_bot_status(State(state): State<SharedState>) -> Json<FeishuBotStatus> {
    let config = state.config.lock().await.clone();
    let account = config.effective_feishu_accounts().into_iter().next();
    let app_id = account
        .as_ref()
        .and_then(|account| non_empty_string(&account.app_id));
    let mut display_name = account
        .as_ref()
        .and_then(|account| non_empty_string(&account.display_name));
    let configured = account
        .as_ref()
        .is_some_and(|account| account.is_configured());
    let mut error = None;

    if let Some(account) = account.as_ref()
        && configured
        && display_name.is_none()
    {
        let api = FeishuApi::new(FeishuSettings::from_app_config(account));
        match api
            .get_application_display_name(app_id.as_deref().unwrap_or_default())
            .await
        {
            Ok(Some(name)) => {
                display_name = Some(name.clone());
                let mut config = state.config.lock().await;
                if let Some(mut account) = config.feishu_account(&account.account_id)
                    && account.display_name.trim().is_empty()
                {
                    account.display_name = name;
                    config.upsert_feishu_account(account);
                    if let Err(err) = config.save(&state.config_path) {
                        error = Some(err.to_string());
                    }
                }
            }
            Ok(None) => {}
            Err(err) => error = Some(err.to_string()),
        }
    }

    Json(FeishuBotStatus {
        configured,
        enabled: account.as_ref().is_some_and(|account| account.enabled),
        app_id,
        display_name,
        allowed_open_ids: account
            .as_ref()
            .map(|account| account.allowed_open_ids.len())
            .unwrap_or_default(),
        error,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TelegramBotStatus {
    configured: bool,
    enabled: bool,
    token_set: bool,
    display_name: Option<String>,
    username: Option<String>,
    mention_only: bool,
    allowed_chat_ids: usize,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
    error: Option<String>,
}

async fn telegram_bot_status(State(state): State<SharedState>) -> Json<TelegramBotStatus> {
    let config = state.config.lock().await.clone();
    let telegram = state.telegram.lock().await.clone();
    let account = config.effective_telegram_accounts().into_iter().next();
    let configured = account
        .as_ref()
        .is_some_and(|account| account.is_configured());
    let mut display_name = account
        .as_ref()
        .and_then(|account| non_empty_string(&account.display_name));
    let mut username = None;
    let mut error = None;

    if let Some(account) = account.as_ref()
        && configured
        && display_name.is_none()
    {
        let api = TelegramApi::new(TelegramSettings::from_app_config(account));
        match tokio::time::timeout(std::time::Duration::from_secs(3), api.get_me()).await {
            Ok(Ok(user)) => {
                username = user
                    .username
                    .as_deref()
                    .map(|value| value.trim_start_matches('@').to_string())
                    .filter(|value| !value.is_empty());
                display_name = telegram_user_display_name(&user);
                if let Some(name) = display_name.clone() {
                    let mut config = state.config.lock().await;
                    if let Some(mut account) = config.telegram_account(&account.account_id)
                        && account.display_name.trim().is_empty()
                    {
                        account.display_name = name;
                        config.upsert_telegram_account(account);
                        if let Err(err) = config.save(&state.config_path) {
                            error = Some(err.to_string());
                        }
                    }
                }
            }
            Ok(Err(err)) => error = Some(err.to_string()),
            Err(_) => error = Some("telegram getMe timeout".to_string()),
        }
    }

    Json(TelegramBotStatus {
        configured,
        enabled: account.as_ref().is_some_and(|account| account.enabled),
        token_set: account
            .as_ref()
            .is_some_and(|account| !account.bot_token.trim().is_empty()),
        display_name,
        username,
        mention_only: account.as_ref().is_some_and(|account| account.mention_only),
        allowed_chat_ids: account
            .as_ref()
            .map(|account| account.allowed_chat_ids.len())
            .unwrap_or_default(),
        polling: telegram.polling,
        connected: telegram.connected,
        last_error: telegram.last_error,
        error,
    })
}

fn telegram_user_display_name(user: &crate::im::telegram::api::TelegramUser) -> Option<String> {
    let username = user
        .username
        .as_deref()
        .map(|value| value.trim_start_matches('@'))
        .filter(|value| !value.is_empty());
    let name = [user.first_name.as_deref(), user.last_name.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    match (name.is_empty(), username) {
        (false, Some(username)) => Some(format!("{name} (@{username})")),
        (false, None) => Some(name),
        (true, Some(username)) => Some(format!("@{username}")),
        (true, None) => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureTelegramBotRequest {
    bot_token: Option<String>,
    mention_only: Option<bool>,
}

async fn configure_telegram_bot(
    State(state): State<SharedState>,
    Json(request): Json<ConfigureTelegramBotRequest>,
) -> impl IntoResponse {
    let Some(token) = request
        .bot_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !is_masked_secret(value))
        .map(str::to_string)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "missing botToken" })),
        );
    };
    let mention_only = request.mention_only.unwrap_or(false);
    let mut telegram_config = crate::config::TelegramConfig {
        enabled: true,
        account_id: String::new(),
        bot_token: token,
        display_name: String::new(),
        mention_only,
        allowed_chat_ids: Vec::new(),
    };
    let api = TelegramApi::new(TelegramSettings::from_app_config(&telegram_config));
    let user = match tokio::time::timeout(std::time::Duration::from_secs(5), api.get_me()).await {
        Ok(Ok(user)) => user,
        Ok(Err(err)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
        Err(_) => {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(json!({ "ok": false, "error": "telegram getMe timeout" })),
            );
        }
    };
    telegram_config.account_id = format!("tg_{}", user.id);
    telegram_config.display_name = telegram_user_display_name(&user).unwrap_or_else(|| {
        user.username
            .as_deref()
            .map(|value| format!("@{}", value.trim_start_matches('@')))
            .unwrap_or_else(|| format!("Telegram {}", user.id))
    });
    {
        let mut config = state.config.lock().await;
        config.migrate_legacy_im_accounts();
        let token = telegram_config.bot_token.trim().to_string();
        config.telegram_accounts.retain(|account| {
            account.account_id.trim() == telegram_config.account_id
                || account.bot_token.trim() != token
        });
        config.upsert_telegram_account(telegram_config.clone());
        if !config.telegram.is_configured()
            || config.telegram.account_id.trim() == telegram_config.account_id
        {
            config.telegram = telegram_config.clone();
        }
        config.bridge.enabled = true;
        if let Err(err) = config.save(&state.config_path) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    }
    start_bridge_task(
        &state,
        BridgeStartMode::Restart,
        "bridge restarted after Telegram configuration",
    )
    .await;
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "configured": true, "accountId": telegram_config.account_id })),
    )
}

fn is_masked_secret(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '*')
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WechatBotStatus {
    configured: bool,
    enabled: bool,
    display_name: Option<String>,
    account_id: Option<String>,
    base_url: Option<String>,
    user_id: Option<String>,
    allowed_user_ids: usize,
    polling: bool,
    connected: bool,
    last_error: Option<String>,
    last_event_at_ms: Option<u128>,
    last_inbound_at_ms: Option<u128>,
}

async fn wechat_bot_status(State(state): State<SharedState>) -> Json<WechatBotStatus> {
    let config = state.config.lock().await.clone();
    let wechat = state.wechat.lock().await.clone();
    let account = config.effective_wechat_accounts().into_iter().next();
    Json(WechatBotStatus {
        configured: account
            .as_ref()
            .is_some_and(|account| account.is_configured()),
        enabled: account.as_ref().is_some_and(|account| account.enabled),
        display_name: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.display_name))
            .or_else(|| account.is_some().then(|| "微信机器人".to_string())),
        account_id: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.account_id)),
        base_url: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.base_url)),
        user_id: account
            .as_ref()
            .and_then(|account| non_empty_string(&account.user_id)),
        allowed_user_ids: account
            .as_ref()
            .map(|account| account.allowed_user_ids.len())
            .unwrap_or_default(),
        polling: wechat.polling,
        connected: wechat.connected,
        last_error: wechat.last_error,
        last_event_at_ms: wechat.last_event_at_ms,
        last_inbound_at_ms: wechat.last_inbound_at_ms,
    })
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
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
    let feishu_configured = feishu_configured(&config);
    let telegram_configured = telegram_configured(&config);
    let wechat_configured = wechat_configured(&config);
    let im_configured = im_bridge_configured(&config);
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

async fn plugin_legacy_list() -> Json<Vec<serde_json::Value>> {
    let installed = installed_plugin_ids();
    Json(
        local_plugin_catalog()
            .into_iter()
            .map(|plugin| {
                let id = plugin.config_id();
                json!({
                    "name": plugin.name,
                    "marketplace_name": plugin.marketplace,
                    "enabled": installed.contains(&id),
                })
            })
            .collect(),
    )
}

async fn plugin_legacy_featured() -> Json<Vec<String>> {
    Json(
        local_plugin_catalog()
            .into_iter()
            .map(|plugin| plugin.config_id())
            .collect(),
    )
}

async fn plugin_legacy_enable(AxumPath(plugin_id): AxumPath<String>) -> impl IntoResponse {
    match install_local_plugin(&plugin_id) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "id": plugin_id, "enabled": true })),
        ),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

#[derive(Deserialize)]
struct PluginPageQuery {
    scope: Option<String>,
}

async fn plugin_catalog_page(Query(query): Query<PluginPageQuery>) -> Json<serde_json::Value> {
    if query
        .scope
        .as_deref()
        .is_some_and(|scope| scope.eq_ignore_ascii_case("WORKSPACE"))
    {
        return Json(empty_plugin_page_json());
    }
    Json(plugin_page_json(None))
}

async fn plugin_empty_page() -> Json<serde_json::Value> {
    Json(empty_plugin_page_json())
}

async fn plugin_installed_page() -> Json<serde_json::Value> {
    let installed = installed_plugin_ids();
    Json(plugin_page_json(Some(&installed)))
}

async fn plugin_detail(AxumPath(plugin_id): AxumPath<String>) -> impl IntoResponse {
    let Some(plugin) = find_local_plugin_by_remote_id(&plugin_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "local plugin not found" })),
        )
            .into_response();
    };
    Json(plugin.to_remote_directory_item(installed_plugin_ids().contains(&plugin.config_id())))
        .into_response()
}

async fn plugin_install(AxumPath(plugin_id): AxumPath<String>) -> impl IntoResponse {
    match install_local_plugin(&plugin_id) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "id": plugin_id, "enabled": true })),
        )
            .into_response(),
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

fn plugin_page_json(installed_filter: Option<&BTreeSet<String>>) -> serde_json::Value {
    let installed = installed_plugin_ids();
    let plugins = local_plugin_catalog()
        .into_iter()
        .filter(|plugin| {
            installed_filter
                .map(|ids| ids.contains(&plugin.config_id()))
                .unwrap_or(true)
        })
        .map(|plugin| {
            let enabled = installed.contains(&plugin.config_id());
            if installed_filter.is_some() {
                plugin.to_remote_installed_item(enabled)
            } else {
                plugin.to_remote_directory_item(enabled)
            }
        })
        .collect::<Vec<_>>();
    json!({
        "plugins": plugins,
        "pagination": {
            "next_page_token": null
        }
    })
}

fn empty_plugin_page_json() -> serde_json::Value {
    json!({
        "plugins": [],
        "pagination": {
            "next_page_token": null
        }
    })
}

#[derive(Debug, Clone)]
struct LocalPluginCatalogEntry {
    name: String,
    marketplace: String,
    version: Option<String>,
    root: PathBuf,
    description: Option<String>,
    keywords: Vec<String>,
    interface: serde_json::Value,
    skills: Vec<LocalPluginSkill>,
}

#[derive(Debug, Clone)]
struct LocalPluginSkill {
    name: String,
    description: String,
}

impl LocalPluginCatalogEntry {
    fn config_id(&self) -> String {
        format!("{}@{}", self.name, self.marketplace)
    }

    fn remote_id(&self) -> String {
        format!("local~{}~{}", self.marketplace, self.name)
    }

    fn to_remote_directory_item(&self, enabled: bool) -> serde_json::Value {
        let release = self.release_json();
        json!({
            "id": self.remote_id(),
            "name": self.name,
            "scope": "GLOBAL",
            "installation_policy": if enabled { "INSTALLED_BY_DEFAULT" } else { "AVAILABLE" },
            "authentication_policy": "ON_USE",
            "status": "ENABLED",
            "release": release,
        })
    }

    fn to_remote_installed_item(&self, enabled: bool) -> serde_json::Value {
        let mut value = self.to_remote_directory_item(enabled);
        if let Some(object) = value.as_object_mut() {
            object.insert("enabled".to_string(), json!(enabled));
            object.insert(
                "disabled_skill_names".to_string(),
                serde_json::Value::Array(Vec::new()),
            );
        }
        value
    }

    fn release_json(&self) -> serde_json::Value {
        json!({
            "version": self.version,
            "display_name": interface_string(&self.interface, "displayName")
                .unwrap_or_else(|| self.name.clone()),
            "description": self.description.clone().unwrap_or_default(),
            "app_ids": [],
            "keywords": self.keywords,
            "interface": remote_release_interface(&self.interface),
            "skills": self.skills.iter().map(|skill| {
                json!({
                    "name": skill.name,
                    "description": skill.description,
                    "interface": {
                        "display_name": skill.name,
                        "short_description": skill.description,
                        "brand_color": interface_string(&self.interface, "brandColor"),
                        "default_prompt": null,
                        "icon_small_url": null,
                        "icon_large_url": null,
                    }
                })
            }).collect::<Vec<_>>(),
        })
    }
}

fn local_plugin_catalog() -> Vec<LocalPluginCatalogEntry> {
    let mut by_id = BTreeMap::<String, LocalPluginCatalogEntry>::new();
    for root in plugin_cache_roots() {
        let Ok(marketplaces) = fs::read_dir(&root) else {
            continue;
        };
        for marketplace in marketplaces.flatten() {
            let marketplace_path = marketplace.path();
            if !marketplace_path.is_dir() {
                continue;
            }
            let marketplace_name = marketplace.file_name().to_string_lossy().to_string();
            let Ok(plugin_dirs) = fs::read_dir(&marketplace_path) else {
                continue;
            };
            for plugin_dir in plugin_dirs.flatten() {
                let plugin_path = plugin_dir.path();
                if !plugin_path.is_dir() {
                    continue;
                }
                if let Some(entry) = newest_plugin_version_entry(&marketplace_name, &plugin_path) {
                    by_id.entry(entry.config_id()).or_insert(entry);
                }
            }
        }
    }
    by_id.into_values().collect()
}

fn newest_plugin_version_entry(
    marketplace_name: &str,
    plugin_path: &Path,
) -> Option<LocalPluginCatalogEntry> {
    if plugin_path
        .join(".codex-plugin")
        .join("plugin.json")
        .is_file()
    {
        return local_plugin_entry_from_root(marketplace_name, plugin_path.to_path_buf());
    }

    let mut versions = fs::read_dir(plugin_path)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join(".codex-plugin").join("plugin.json").is_file())
        .collect::<Vec<_>>();
    versions.sort();
    versions.reverse();
    versions
        .into_iter()
        .find_map(|root| local_plugin_entry_from_root(marketplace_name, root))
}

fn local_plugin_entry_from_root(
    marketplace_name: &str,
    root: PathBuf,
) -> Option<LocalPluginCatalogEntry> {
    let manifest_path = root.join(".codex-plugin").join("plugin.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(manifest_path).ok()?).ok()?;
    let name = manifest.get("name")?.as_str()?.trim().to_string();
    if name.is_empty() || manifest.get("apps").is_some() {
        return None;
    }
    let skills = load_local_plugin_skills(&root, &manifest);
    if skills.is_empty() {
        return None;
    }
    Some(LocalPluginCatalogEntry {
        name,
        marketplace: marketplace_name.to_string(),
        version: manifest
            .get("version")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        root,
        description: manifest
            .get("description")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        keywords: manifest
            .get("keywords")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        interface: manifest
            .get("interface")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
        skills,
    })
}

fn load_local_plugin_skills(root: &Path, manifest: &serde_json::Value) -> Vec<LocalPluginSkill> {
    let skills_path = manifest
        .get("skills")
        .and_then(|value| value.as_str())
        .map(|value| root.join(value.trim_start_matches("./")))
        .unwrap_or_else(|| root.join("skills"));
    let Ok(skill_dirs) = fs::read_dir(skills_path) else {
        return Vec::new();
    };
    let mut skills = skill_dirs
        .flatten()
        .filter_map(|entry| {
            let path = entry.path().join("SKILL.md");
            let contents = fs::read_to_string(path).ok()?;
            let name = yaml_front_matter_value(&contents, "name")
                .unwrap_or_else(|| entry.file_name().to_string_lossy().to_string());
            let description =
                yaml_front_matter_value(&contents, "description").unwrap_or_else(|| name.clone());
            Some(LocalPluginSkill { name, description })
        })
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    skills
}

fn yaml_front_matter_value(contents: &str, key: &str) -> Option<String> {
    let mut lines = contents.lines();
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line == "---" {
            break;
        }
        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        if left.trim() == key {
            return Some(
                right
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            )
            .filter(|value| !value.is_empty());
        }
    }
    None
}

fn remote_release_interface(interface: &serde_json::Value) -> serde_json::Value {
    json!({
        "short_description": interface_string(interface, "shortDescription"),
        "long_description": interface_string(interface, "longDescription"),
        "developer_name": interface_string(interface, "developerName"),
        "category": interface_string(interface, "category"),
        "capabilities": interface
            .get("capabilities")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        "website_url": interface_string(interface, "websiteURL")
            .or_else(|| interface_string(interface, "websiteUrl")),
        "privacy_policy_url": interface_string(interface, "privacyPolicyURL")
            .or_else(|| interface_string(interface, "privacyPolicyUrl")),
        "terms_of_service_url": interface_string(interface, "termsOfServiceURL")
            .or_else(|| interface_string(interface, "termsOfServiceUrl")),
        "brand_color": interface_string(interface, "brandColor"),
        "default_prompt": interface.get("defaultPrompt").and_then(|value| {
            value.as_array().map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        }),
        "composer_icon_url": null,
        "logo_url": null,
        "screenshot_urls": [],
    })
}

fn interface_string(interface: &serde_json::Value, key: &str) -> Option<String> {
    interface
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn find_local_plugin_by_remote_id(remote_id: &str) -> Option<LocalPluginCatalogEntry> {
    local_plugin_catalog()
        .into_iter()
        .find(|plugin| plugin.remote_id() == remote_id || plugin.config_id() == remote_id)
}

fn install_local_plugin(plugin_id: &str) -> anyhow::Result<()> {
    let plugin = find_local_plugin_by_remote_id(plugin_id)
        .ok_or_else(|| anyhow::anyhow!("local plugin not found"))?;
    let codex_home = default_codex_home_for_web();
    let target = codex_home
        .join("plugins")
        .join("cache")
        .join(&plugin.marketplace)
        .join(&plugin.name)
        .join(plugin.version.as_deref().unwrap_or("local"));
    if plugin.root != target {
        if target.exists() {
            fs::remove_dir_all(&target)
                .with_context(|| format!("failed to replace plugin cache {}", target.display()))?;
        }
        copy_dir_all(&plugin.root, &target)?;
    }
    write_enabled_plugin_config(&codex_home.join("config.toml"), &plugin.config_id())?;
    Ok(())
}

fn installed_plugin_ids() -> BTreeSet<String> {
    let path = default_codex_home_for_web().join("config.toml");
    let Ok(raw) = fs::read_to_string(path) else {
        return BTreeSet::new();
    };
    let Ok(doc) = raw.parse::<toml_edit::DocumentMut>() else {
        return BTreeSet::new();
    };
    doc.get("plugins")
        .and_then(|item| item.as_table())
        .map(|plugins| {
            plugins
                .iter()
                .filter_map(|(id, item)| {
                    item.as_table()
                        .and_then(|table| table.get("enabled"))
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
                        .then(|| id.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_enabled_plugin_config(config_path: &Path, plugin_id: &str) -> anyhow::Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut doc = if config_path.is_file() {
        fs::read_to_string(config_path)?
            .parse::<toml_edit::DocumentMut>()
            .unwrap_or_else(|_| toml_edit::DocumentMut::new())
    } else {
        toml_edit::DocumentMut::new()
    };
    if !doc.contains_key("plugins") {
        doc["plugins"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let plugins = doc["plugins"]
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config plugins entry is not a table"))?;
    let mut plugin_table = toml_edit::Table::new();
    plugin_table["enabled"] = toml_edit::value(true);
    plugins[plugin_id] = toml_edit::Item::Table(plugin_table);
    fs::write(config_path, doc.to_string())?;
    Ok(())
}

fn plugin_cache_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        roots.push(
            Path::new(&home)
                .join(".codex")
                .join("plugins")
                .join("cache"),
        );
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        roots.push(
            Path::new(&local_app_data)
                .join("com.lokizhou.arthas")
                .join("codex")
                .join("plugins")
                .join("cache"),
        );
    }
    if let Some(root) = find_openai_bundled_plugins_root() {
        roots.push(root);
    }
    roots
}

fn default_codex_home_for_web() -> PathBuf {
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        Path::new(&home).join(".codex")
    } else {
        PathBuf::from(".codex")
    }
}

fn find_openai_bundled_plugins_root() -> Option<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from)?;
    let windows_apps = program_files.join("WindowsApps");
    let entries = fs::read_dir(windows_apps).ok()?;
    let mut roots = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("OpenAI.Codex_") {
                return None;
            }
            let root = entry.path().join("app").join("resources").join("plugins");
            root.join("openai-bundled")
                .join(".agents")
                .join("plugins")
                .join("marketplace.json")
                .is_file()
                .then_some(root)
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots.pop()
}

fn copy_dir_all(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeishuOnboardStartResponse {
    verification_uri: String,
    verification_uri_complete: String,
    device_code: String,
    expires_in: u64,
    interval: u64,
    qr_svg: String,
}

async fn feishu_onboard_start(State(state): State<SharedState>) -> impl IntoResponse {
    let settings = {
        let config = state.config.lock().await;
        FeishuSettings::from_app_config(&config.feishu)
    };
    let api = FeishuApi::new(settings);
    match api.start_app_registration().await {
        Ok(payload) => {
            let verification_uri = payload
                .get("verification_uri")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let verification_uri_complete = payload
                .get("verification_uri_complete")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("verification_uri").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            let device_code = payload
                .get("device_code")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let expires_in = payload
                .get("expire_in")
                .or_else(|| payload.get("expires_in"))
                .and_then(|v| v.as_u64())
                .unwrap_or(600);
            let interval = payload
                .get("interval")
                .and_then(|v| v.as_u64())
                .unwrap_or(5);
            let qr_svg = build_qr_svg(&verification_uri_complete).unwrap_or_default();
            state
                .push_event("info", "feishu_onboard_started", "scan flow started")
                .await;
            (
                StatusCode::OK,
                Json(json!(FeishuOnboardStartResponse {
                    verification_uri,
                    verification_uri_complete,
                    device_code,
                    expires_in,
                    interval,
                    qr_svg,
                })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

async fn feishu_onboard_poll(
    State(state): State<SharedState>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(device_code) = payload.get("deviceCode").and_then(|v| v.as_str()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing deviceCode" })),
        );
    };
    let settings = {
        let config = state.config.lock().await;
        FeishuSettings::from_app_config(&config.feishu)
    };
    let api = FeishuApi::new(settings);
    match api.poll_app_registration(device_code).await {
        Ok(result) => {
            let app_id = result
                .get("client_id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let app_secret = result
                .get("client_secret")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let open_id = result
                .get("user_info")
                .and_then(|v| v.get("open_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let done = app_id.is_some() && app_secret.is_some();
            let mut display_name = None;
            if let (Some(app_id), Some(app_secret)) = (app_id.clone(), app_secret.clone()) {
                let feishu_config = {
                    let mut config = state.config.lock().await;
                    config.migrate_legacy_im_accounts();
                    config.feishu_accounts.retain(|account| {
                        account.account_id.trim() == app_id || account.app_id.trim() != app_id
                    });
                    let mut account = config.feishu_account(&app_id).unwrap_or_default();
                    account.enabled = true;
                    account.account_id = app_id.clone();
                    account.app_id = app_id.clone();
                    account.app_secret = app_secret;
                    if let Some(open_id) = open_id.clone()
                        && !account.allowed_open_ids.contains(&open_id)
                    {
                        account.allowed_open_ids.push(open_id);
                    }
                    config.upsert_feishu_account(account.clone());
                    let saved_account = account.clone();
                    if !config.feishu.is_configured() || config.feishu.app_id == app_id {
                        config.feishu = account.clone();
                    }
                    config.bridge.enabled = true;
                    if let Err(err) = config.save(&state.config_path) {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": err.to_string() })),
                        );
                    }
                    saved_account
                };
                let api = FeishuApi::new(FeishuSettings::from_app_config(&feishu_config));
                display_name = api
                    .get_application_display_name(&app_id)
                    .await
                    .ok()
                    .flatten();
                if let Some(name) = display_name.clone() {
                    let mut config = state.config.lock().await;
                    if let Some(mut account) = config.feishu_account(&app_id) {
                        account.display_name = name.clone();
                        config.upsert_feishu_account(account.clone());
                        if config.feishu.app_id == app_id {
                            config.feishu = account;
                        }
                        let _ = config.save(&state.config_path);
                    }
                }
                state
                    .push_event(
                        "info",
                        "feishu_onboard_completed",
                        format!(
                            "app_id={app_id} open_id={}",
                            open_id.clone().unwrap_or_default()
                        ),
                    )
                    .await;
                start_bridge_task(
                    &state,
                    BridgeStartMode::Restart,
                    "bridge restarted after Feishu onboarding",
                )
                .await;
            }
            (
                StatusCode::OK,
                Json(json!({
                    "done": done,
                    "appId": app_id,
                    "openId": open_id,
                    "displayName": display_name,
                    "error": result.get("error").cloned(),
                    "errorDescription": result.get("error_description").cloned(),
                    "raw": result,
                })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

const WECHAT_ONBOARD_TTL_MS: u128 = 5 * 60_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WechatOnboardStartResponse {
    session_key: String,
    qrcode_url: String,
    qr_svg: String,
    expires_in: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WechatOnboardPollRequest {
    session_key: String,
    verify_code: Option<String>,
}

async fn wechat_onboard_start(State(state): State<SharedState>) -> impl IntoResponse {
    let config = state.config.lock().await.clone();
    let api = WechatApi::new(WechatSettings::from_app_config(&config.wechat));
    let local_tokens = wechat_store::local_bot_tokens(&state).await;
    match api.start_qr_login(&local_tokens).await {
        Ok(payload) => {
            let session_key = format!("wechat-onboard-{}", unix_now_millis());
            let qr_svg = build_qr_svg(&payload.qrcode_img_content).unwrap_or_default();
            let session = WechatOnboardSession {
                session_key: session_key.clone(),
                qrcode: payload.qrcode,
                started_at_ms: unix_now_millis(),
                current_api_base_url: DEFAULT_WECHAT_API_BASE.to_string(),
            };
            *state.wechat_onboard.lock().await = Some(session);
            state
                .push_event("info", "wechat_onboard_started", "scan flow started")
                .await;
            (
                StatusCode::OK,
                Json(json!(WechatOnboardStartResponse {
                    session_key,
                    qrcode_url: payload.qrcode_img_content,
                    qr_svg,
                    expires_in: (WECHAT_ONBOARD_TTL_MS / 1000) as u64,
                })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

async fn wechat_onboard_poll(
    State(state): State<SharedState>,
    Json(request): Json<WechatOnboardPollRequest>,
) -> impl IntoResponse {
    let session = {
        let onboard = state.wechat_onboard.lock().await;
        onboard.clone()
    };
    let Some(mut session) = session else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "done": false, "error": "missing_session" })),
        );
    };
    if session.session_key != request.session_key {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "done": false, "error": "invalid_session" })),
        );
    }
    if unix_now_millis().saturating_sub(session.started_at_ms) > WECHAT_ONBOARD_TTL_MS {
        *state.wechat_onboard.lock().await = None;
        return (
            StatusCode::OK,
            Json(json!({ "done": false, "status": "expired", "error": "expired" })),
        );
    }

    let config = state.config.lock().await.clone();
    let api = WechatApi::new(WechatSettings::from_app_config(&config.wechat));
    let result = match api
        .poll_qr_status(
            &session.current_api_base_url,
            &session.qrcode,
            request.verify_code.as_deref(),
        )
        .await
    {
        Ok(result) => result,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "done": false, "error": err.to_string() })),
            );
        }
    };

    if result.status == "scaned_but_redirect" {
        if let Some(redirect_host) = result
            .redirect_host
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            session.current_api_base_url = normalize_wechat_base_url(redirect_host);
            *state.wechat_onboard.lock().await = Some(session);
        }
        return (
            StatusCode::OK,
            Json(json!({ "done": false, "status": result.status })),
        );
    }

    if result.status == "confirmed" {
        let Some(bot_token) = result
            .bot_token
            .clone()
            .filter(|value| !value.trim().is_empty())
        else {
            return (
                StatusCode::OK,
                Json(
                    json!({ "done": false, "status": result.status, "error": "missing_bot_token" }),
                ),
            );
        };
        let account_id = result
            .ilink_bot_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                if config.wechat.account_id.trim().is_empty() {
                    "wechat".to_string()
                } else {
                    config.wechat.account_id.clone()
                }
            });
        let base_url = result
            .baseurl
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| session.current_api_base_url.clone());
        let user_id = result.ilink_user_id.clone().unwrap_or_default();
        {
            let mut config = state.config.lock().await;
            config.migrate_legacy_im_accounts();
            let token = bot_token.trim().to_string();
            let resolved_account_id = if account_id.trim().is_empty() {
                "wechat".to_string()
            } else {
                account_id.clone()
            };
            config.wechat_accounts.retain(|account| {
                account.account_id.trim() == resolved_account_id
                    || account.bot_token.trim() != token
            });
            let mut account = config
                .wechat_account(&resolved_account_id)
                .unwrap_or_default();
            account.enabled = true;
            account.account_id = resolved_account_id.clone();
            account.bot_token = bot_token;
            if account.display_name.trim().is_empty() {
                account.display_name = "微信机器人".to_string();
            }
            account.base_url = normalize_wechat_base_url(&base_url);
            account.user_id = user_id.clone();
            if !user_id.trim().is_empty() && !account.allowed_user_ids.contains(&user_id) {
                account.allowed_user_ids.push(user_id.clone());
            }
            config.upsert_wechat_account(account.clone());
            if !config.wechat.is_configured() || config.wechat.account_id == resolved_account_id {
                config.wechat = account;
            }
            config.bridge.enabled = true;
            if let Err(err) = config.save(&state.config_path) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "done": false, "error": err.to_string() })),
                );
            }
        }
        *state.wechat_onboard.lock().await = None;
        state
            .push_event(
                "info",
                "wechat_onboard_completed",
                format!("account={} user={}", account_id, user_id),
            )
            .await;
        start_bridge_task(
            &state,
            BridgeStartMode::Restart,
            "bridge restarted after WeChat onboarding",
        )
        .await;
        return (
            StatusCode::OK,
            Json(json!({
                "done": true,
                "status": result.status,
                "accountId": account_id,
                "userId": user_id,
            })),
        );
    }

    if result.status == "binded_redirect" {
        *state.wechat_onboard.lock().await = None;
        state
            .push_event(
                "info",
                "wechat_onboard_already_connected",
                "already connected",
            )
            .await;
        return (
            StatusCode::OK,
            Json(json!({
                "done": true,
                "alreadyConnected": true,
                "status": result.status,
            })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "done": false,
            "status": result.status,
            "needVerifyCode": result.status == "need_verifycode",
            "error": match result.status.as_str() {
                "expired" => Some("expired"),
                "verify_code_blocked" => Some("verify_code_blocked"),
                _ => None,
            },
        })),
    )
}

fn build_qr_svg(content: &str) -> anyhow::Result<String> {
    let code = QrCode::new(content.as_bytes())?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#20242a"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

fn unix_now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn normalize_wechat_base_url(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return DEFAULT_WECHAT_API_BASE.to_string();
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        value.trim_end_matches('/').to_string()
    } else {
        format!("https://{}", value.trim_end_matches('/'))
    }
}

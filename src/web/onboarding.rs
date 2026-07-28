use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use qrcode::{QrCode, render::svg};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    app_state::{SharedState, WechatOnboardSession, WecomOnboardSession},
    im::feishu::{FeishuApi, FeishuSettings},
    im::wechat::{
        api::WechatApi,
        store as wechat_store,
        types::{DEFAULT_WECHAT_API_BASE, WechatSettings},
    },
    im::wecom::onboarding::{self as wecom_onboarding, QrPoll},
};

use super::im_api;

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

pub(super) async fn feishu_onboard_start(State(state): State<SharedState>) -> impl IntoResponse {
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

pub(super) async fn feishu_onboard_poll(
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
                im_api::start_bridge_task(
                    &state,
                    im_api::BridgeStartMode::Restart,
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
const WECOM_ONBOARD_TTL_MS: u128 = 5 * 60_000;

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
pub(super) struct WechatOnboardPollRequest {
    session_key: String,
    verify_code: Option<String>,
}

pub(super) async fn wechat_onboard_start(State(state): State<SharedState>) -> impl IntoResponse {
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

pub(super) async fn wechat_onboard_poll(
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
        state
            .push_event(
                "warn",
                "wechat_onboard_expired",
                "local onboarding session expired",
            )
            .await;
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
        im_api::start_bridge_task(
            &state,
            im_api::BridgeStartMode::Restart,
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

    if result.status == "expired" {
        state
            .push_event(
                "warn",
                "wechat_onboard_expired",
                "upstream QR status expired",
            )
            .await;
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WecomOnboardStartResponse {
    session_key: String,
    qrcode_url: String,
    qr_svg: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WecomOnboardPollRequest {
    session_key: String,
}

pub(super) async fn wecom_onboard_start(State(state): State<SharedState>) -> impl IntoResponse {
    let http_client = crate::outbound_http::get();
    match wecom_onboarding::start(&http_client).await {
        Ok(qr) => {
            let session_key = format!("wecom-onboard-{}", unix_now_millis());
            let qr_svg = build_qr_svg(&qr.auth_url).unwrap_or_default();
            *state.wecom_onboard.lock().await = Some(WecomOnboardSession {
                session_key: session_key.clone(),
                scode: qr.scode,
                started_at_ms: unix_now_millis(),
            });
            state
                .push_event("info", "wecom_onboard_started", "scan flow started")
                .await;
            (
                StatusCode::OK,
                Json(json!(WecomOnboardStartResponse {
                    session_key,
                    qrcode_url: qr.auth_url,
                    qr_svg,
                    expires_in: (WECOM_ONBOARD_TTL_MS / 1000) as u64,
                    interval: 3,
                })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        ),
    }
}

pub(super) async fn wecom_onboard_poll(
    State(state): State<SharedState>,
    Json(request): Json<WecomOnboardPollRequest>,
) -> impl IntoResponse {
    let Some(session) = state.wecom_onboard.lock().await.clone() else {
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
    if unix_now_millis().saturating_sub(session.started_at_ms) > WECOM_ONBOARD_TTL_MS {
        *state.wecom_onboard.lock().await = None;
        return (
            StatusCode::OK,
            Json(json!({ "done": false, "status": "expired", "error": "expired" })),
        );
    }

    let http_client = crate::outbound_http::get();
    match wecom_onboarding::poll(&http_client, &session.scode).await {
        Ok(QrPoll::Pending(status)) => (
            StatusCode::OK,
            Json(json!({ "done": false, "status": status })),
        ),
        Ok(QrPoll::Success { bot_id, secret }) => {
            let account_id = bot_id.clone();
            {
                let mut config = state.config.lock().await;
                config.migrate_legacy_im_accounts();
                let mut account = config.wecom_account(&account_id).unwrap_or_default();
                account.enabled = true;
                account.account_id = account_id.clone();
                account.bot_id = bot_id;
                account.secret = secret;
                if account.display_name.trim().is_empty() {
                    account.display_name = "企业微信机器人".to_string();
                }
                config.upsert_wecom_account(account.clone());
                if !config.wecom.is_configured() || config.wecom.account_id == account_id {
                    config.wecom = account;
                }
                config.bridge.enabled = true;
                if let Err(err) = config.save(&state.config_path) {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "done": false, "error": err.to_string() })),
                    );
                }
            }
            *state.wecom_onboard.lock().await = None;
            state
                .push_event(
                    "info",
                    "wecom_onboard_completed",
                    format!("account={account_id}"),
                )
                .await;
            im_api::start_bridge_task(
                &state,
                im_api::BridgeStartMode::Restart,
                "bridge restarted after WeCom onboarding",
            )
            .await;
            (
                StatusCode::OK,
                Json(json!({ "done": true, "status": "success", "accountId": account_id })),
            )
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "done": false, "error": err.to_string() })),
        ),
    }
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

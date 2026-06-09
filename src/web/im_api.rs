use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

use crate::{
    app_state::{ImAccountRuntimeState, SharedState, im_account_key},
    bridge, chain_log,
    config::AppConfig,
    im::feishu::{FeishuApi, FeishuSettings},
    im::telegram::{api::TelegramApi, types::TelegramSettings},
    types::ImPlatformKind,
};

pub(super) async fn start_bridge(State(state): State<SharedState>) -> impl IntoResponse {
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

pub(super) async fn stop_bridge(State(state): State<SharedState>) -> impl IntoResponse {
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
pub(super) struct SetImChannelEnabledRequest {
    channel: String,
    enabled: bool,
}

pub(super) async fn set_im_channel_enabled(
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
pub(super) struct ImAccountsResponse {
    accounts: Vec<ImAccountItem>,
}

pub(super) async fn im_accounts(State(state): State<SharedState>) -> Json<ImAccountsResponse> {
    let config = state.config.lock().await.clone();
    let runtime = state.im_accounts.lock().await.clone();
    Json(ImAccountsResponse {
        accounts: im_account_items(&config, &runtime),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetImAccountEnabledRequest {
    platform: String,
    account_id: String,
    enabled: bool,
}

pub(super) async fn set_im_account_enabled(
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
pub(super) struct DeleteImAccountRequest {
    platform: String,
    account_id: String,
}

pub(super) async fn delete_im_account(
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

pub(super) async fn stop_bridge_task(state: &SharedState) {
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
pub(super) enum BridgeStartMode {
    KeepExisting,
    Restart,
}

pub(super) async fn start_bridge_task(
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

pub(super) fn feishu_configured(config: &AppConfig) -> bool {
    config
        .effective_feishu_accounts()
        .iter()
        .any(|account| account.is_configured())
}

pub(super) fn telegram_configured(config: &AppConfig) -> bool {
    config
        .effective_telegram_accounts()
        .iter()
        .any(|account| account.is_configured())
}

pub(super) fn wechat_configured(config: &AppConfig) -> bool {
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

pub(super) fn im_bridge_configured(config: &AppConfig) -> bool {
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
pub(super) struct FeishuBotStatus {
    configured: bool,
    enabled: bool,
    app_id: Option<String>,
    display_name: Option<String>,
    allowed_open_ids: usize,
    error: Option<String>,
}

pub(super) async fn feishu_bot_status(State(state): State<SharedState>) -> Json<FeishuBotStatus> {
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
pub(super) struct TelegramBotStatus {
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

pub(super) async fn telegram_bot_status(
    State(state): State<SharedState>,
) -> Json<TelegramBotStatus> {
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
pub(super) struct ConfigureTelegramBotRequest {
    bot_token: Option<String>,
    mention_only: Option<bool>,
}

pub(super) async fn configure_telegram_bot(
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
pub(super) struct WechatBotStatus {
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

pub(super) async fn wechat_bot_status(State(state): State<SharedState>) -> Json<WechatBotStatus> {
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

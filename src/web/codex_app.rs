use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    ai_gateway::catalog::configured_models_response_with_etag,
    app_state::SharedState,
    codex_app_config::{self, ConfigureCodexAppOptions},
    codex_app_enhanced, codex_session_history,
    config::LocalConnectionMode,
    remote_control_backend,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConfigureCodexAppRequest {
    codex_home: Option<String>,
    connection_mode: Option<LocalConnectionMode>,
    provider_name: Option<String>,
    provider_base_url: Option<String>,
    provider_key: Option<String>,
    activate: Option<bool>,
    #[allow(dead_code)]
    image_generation_enabled: Option<bool>,
    supports_websockets: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeleteCodexAppProviderRequest {
    provider_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetCodexAppProviderWebSocketRequest {
    provider_name: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MoveCodexAppSessionProviderRequest {
    thread_id: String,
    rollout_path: Option<String>,
    target_provider: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexAppSessionsResponse {
    ok: bool,
    threads: Vec<serde_json::Value>,
    providers: Vec<String>,
    total: usize,
}

pub(super) async fn configure_codex_app(
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
    let activate_provider = request
        .as_ref()
        .and_then(|value| value.activate)
        .unwrap_or(true);
    let provider_supports_websockets = request.as_ref().and_then(|value| value.supports_websockets);
    let connection_mode = request
        .as_ref()
        .and_then(|value| value.connection_mode)
        .unwrap_or(config.local_connection_mode);

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
        connection_mode,
        account_id: "acct_codexhub_local".to_string(),
        user_id: "user_codexhub_local".to_string(),
        email: "codexhub-local@example.local".to_string(),
        plan_type: "pro".to_string(),
        provider_name,
        provider_base_url,
        provider_key,
        activate_provider,
        image_generation_enabled: None,
        provider_supports_websockets,
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

pub(super) async fn set_codex_app_provider_websocket(
    State(state): State<SharedState>,
    Json(request): Json<SetCodexAppProviderWebSocketRequest>,
) -> impl IntoResponse {
    let provider_name = request.provider_name.trim();
    if provider_name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "provider_name is required" })),
        );
    }

    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    match codex_app_config::set_codex_app_provider_websocket(None, provider_name, request.enabled) {
        Ok(config_path) => {
            let status =
                codex_app_config::inspect_codex_app_config_for_mode(None, &backend_url, true);
            state
                .push_event(
                    "info",
                    "codex_app_provider_websocket_set",
                    format!(
                        "config={} provider={} supports_websockets={}",
                        config_path.display(),
                        provider_name,
                        request.enabled
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

pub(super) async fn delete_codex_app_provider(
    State(state): State<SharedState>,
    Json(request): Json<DeleteCodexAppProviderRequest>,
) -> impl IntoResponse {
    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    match codex_app_config::delete_codex_app_provider(None, request.provider_name.trim()) {
        Ok(config_path) => {
            let status =
                codex_app_config::inspect_codex_app_config_for_mode(None, &backend_url, true);
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

pub(super) async fn uninstall_codex_app(State(state): State<SharedState>) -> impl IntoResponse {
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

pub(super) async fn refresh_codex_app_models(
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let cache_removed = match codex_app_config::clear_codex_models_cache(None) {
        Ok(removed) => removed,
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "codex_app_models_cache_clear_failed",
                    err.to_string(),
                )
                .await;
            false
        }
    };

    let model_list_result = remote_control_backend::model_list_for_client(
        &state,
        remote_control_backend::default_remote_client_key(),
        true,
        Some(200),
    )
    .await;

    match model_list_result {
        Ok(value) => {
            let count = value
                .get("data")
                .and_then(|value| value.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            state
                .push_event(
                    "info",
                    "codex_app_models_refreshed",
                    format!("cache_removed={cache_removed} count={count}"),
                )
                .await;
            Json(
                json!({ "ok": true, "cacheRemoved": cache_removed, "modelListRefreshed": true, "count": count }),
            )
        }
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "codex_app_models_refresh_skipped",
                    format!("cache_removed={cache_removed} err={err}"),
                )
                .await;
            Json(
                json!({ "ok": true, "cacheRemoved": cache_removed, "modelListRefreshed": false, "error": err.to_string() }),
            )
        }
    }
}

pub(super) async fn launch_codex_app_enhanced(
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let (models, backend_url) = {
        let config = state.config.lock().await;
        let (response, _) = configured_models_response_with_etag(&config.ai_gateway);
        let models = response["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>();
        (models, config.remote_control_base_url())
    };
    state
        .push_event(
            "info",
            "codex_app_enhanced_launch_start",
            format!("models={}", models.len()),
        )
        .await;
    match codex_app_enhanced::launch_and_inject(models, &backend_url).await {
        Ok(report) => {
            state
                .push_event(
                    "info",
                    "codex_app_enhanced_launch_ready",
                    format!(
                        "launched={} port={} models={} gates={}",
                        report.launched,
                        report.port,
                        report.available_models.len(),
                        report.key_gates_enabled
                    ),
                )
                .await;
            (
                StatusCode::OK,
                Json(json!({ "ok": true, "report": report })),
            )
        }
        Err(err) => {
            state
                .push_event("error", "codex_app_enhanced_launch_failed", err.to_string())
                .await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": err.to_string() })),
            )
        }
    }
}

pub(super) async fn codex_app_enhanced_preflight() -> impl IntoResponse {
    match codex_app_enhanced::preflight().await {
        Ok(status) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "status": status })),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

pub(super) async fn codex_app_sessions(State(state): State<SharedState>) -> impl IntoResponse {
    const PAGE_LIMIT: u32 = 100;
    const MAX_PAGES: usize = 20;
    let threads = match remote_control_backend::session_history_threads(
        &state,
        remote_control_backend::default_remote_client_key(),
        PAGE_LIMIT,
        MAX_PAGES,
        false,
    )
    .await
    {
        Ok(threads) => threads,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "ok": false, "error": err.to_string() })),
            );
        }
    };

    let mut providers = threads
        .iter()
        .filter_map(|thread| thread.get("modelProvider").and_then(|value| value.as_str()))
        .map(str::to_string)
        .collect::<Vec<_>>();
    providers.sort();
    providers.dedup();

    (
        StatusCode::OK,
        Json(json!(CodexAppSessionsResponse {
            ok: true,
            total: threads.len(),
            threads,
            providers,
        })),
    )
}

pub(super) async fn move_codex_app_session_provider(
    Json(request): Json<MoveCodexAppSessionProviderRequest>,
) -> impl IntoResponse {
    let rollout_path = request
        .rollout_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from);
    let target_provider = request
        .target_provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let result = match target_provider {
        Some(provider) => codex_session_history::move_thread_to_provider(
            None,
            request.thread_id.as_str(),
            rollout_path,
            provider,
        ),
        None => codex_session_history::move_thread_to_ai_gateway(
            None,
            request.thread_id.as_str(),
            rollout_path,
        ),
    };
    match result {
        Ok(report) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "report": report })),
        ),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": err.to_string() })),
        ),
    }
}

pub(super) async fn repair_codex_app_gui_environment(
    State(state): State<SharedState>,
) -> impl IntoResponse {
    let config = state.config.lock().await.clone();
    let backend_url = config.remote_control_base_url();
    let status = codex_app_config::inspect_codex_app_config_for_mode(None, &backend_url, true);
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

    let remote_control_switch =
        match codex_app_config::enable_codex_app_remote_control_switch_for_backend(
            None,
            &backend_url,
        ) {
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
    let gui_api_base = codex_app_config::configure_gui_environment(&backend_url, true);
    state
        .push_event(
            "info",
            "codex_app_gui_environment_configured",
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

pub(super) async fn codex_app_status(
    State(state): State<SharedState>,
) -> Json<codex_app_config::CodexAppConfigStatus> {
    Json(codex_app_status_snapshot(&state).await)
}

pub(super) async fn codex_app_status_snapshot(
    state: &SharedState,
) -> codex_app_config::CodexAppConfigStatus {
    let config = state.config.lock().await.clone();
    codex_app_config::inspect_codex_app_config_for_mode(
        None,
        &config.remote_control_base_url(),
        true,
    )
}

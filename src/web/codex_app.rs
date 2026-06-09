use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use serde_json::json;

use crate::{
    app_state::SharedState,
    codex_app_config::{self, ConfigureCodexAppOptions},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConfigureCodexAppRequest {
    codex_home: Option<String>,
    provider_name: Option<String>,
    provider_base_url: Option<String>,
    provider_key: Option<String>,
    model: Option<String>,
    activate: Option<bool>,
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
    let model = request
        .as_ref()
        .and_then(|value| value.model.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let activate_provider = request
        .as_ref()
        .and_then(|value| value.activate)
        .unwrap_or(true);
    let image_generation_enabled = request
        .as_ref()
        .and_then(|value| value.image_generation_enabled);
    let provider_supports_websockets = request.as_ref().and_then(|value| value.supports_websockets);

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
        image_generation_enabled,
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
            let status = codex_app_config::inspect_codex_app_config(None, &backend_url);
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

pub(super) async fn repair_codex_app_gui_environment(
    State(state): State<SharedState>,
) -> impl IntoResponse {
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

pub(super) async fn codex_app_status(
    State(state): State<SharedState>,
) -> Json<codex_app_config::CodexAppConfigStatus> {
    let config = state.config.lock().await.clone();
    Json(codex_app_config::inspect_codex_app_config(
        None,
        &config.remote_control_base_url(),
    ))
}

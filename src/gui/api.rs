use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::ai_gateway::config::AiGatewayConfig;
use crate::config::{AppConfig, LocalConnectionMode};
use crate::daemon_process::DaemonIdentity;

use super::text::GuiText;
use super::{GUI_ACTION_TIMEOUT, GUI_CONFIG_TIMEOUT, GUI_CONNECT_TIMEOUT, GUI_STATUS_TIMEOUT};

const GUI_SESSION_HISTORY_TIMEOUT: Duration = Duration::from_secs(30);
const GUI_WECHAT_ONBOARD_POLL_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone)]
pub(super) struct ApiClient {
    pub(super) base_url: String,
    pub(super) http: Client,
    text: GuiText,
}

impl ApiClient {
    pub(super) fn new(base_url: String, text: GuiText) -> Self {
        let http = Client::builder()
            .connect_timeout(GUI_CONNECT_TIMEOUT)
            .timeout(GUI_ACTION_TIMEOUT)
            // GUI API calls only target the local daemon; system proxies can break loopback.
            .no_proxy()
            .build()
            .expect("build HTTP client");
        Self {
            base_url,
            http,
            text,
        }
    }

    pub(super) fn get_quick<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let body = self.request_text(self.http.get(self.url(path)).timeout(GUI_STATUS_TIMEOUT))?;
        self.parse_response(path, &body)
    }

    pub(super) fn get_with_timeout<T: DeserializeOwned>(
        &self,
        path: &str,
        timeout: Duration,
    ) -> Result<T, String> {
        let body = self.request_text(self.http.get(self.url(path)).timeout(timeout))?;
        self.parse_response(path, &body)
    }

    pub(super) fn is_online(&self) -> bool {
        self.daemon_identity().is_ok()
    }

    pub(super) fn daemon_identity(&self) -> Result<DaemonIdentity, String> {
        let identity = self.get_quick::<DaemonIdentity>("/api/status")?;
        identity
            .is_codexhub()
            .then_some(identity)
            .ok_or_else(|| "local service identity does not match CodexHub".to_string())
    }

    pub(super) fn get_quick_json(&self, path: &str) -> Result<Value, String> {
        self.get_quick(path)
    }

    pub(super) fn post_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.post_empty_with_timeout(path, GUI_ACTION_TIMEOUT)
    }

    pub(super) fn post_empty_with_timeout<T: DeserializeOwned>(
        &self,
        path: &str,
        timeout: Duration,
    ) -> Result<T, String> {
        let body = self.request_text(self.http.post(self.url(path)).timeout(timeout))?;
        self.parse_response(path, &body)
    }

    pub(super) fn post_json<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, String> {
        self.post_json_with_timeout(path, body, GUI_ACTION_TIMEOUT)
    }

    pub(super) fn post_json_with_timeout<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<T, String> {
        let body = self.request_text(self.http.post(self.url(path)).json(body).timeout(timeout))?;
        self.parse_response(path, &body)
    }

    pub(super) fn delete_with_timeout<T: DeserializeOwned>(
        &self,
        path: &str,
        timeout: Duration,
    ) -> Result<T, String> {
        let body = self.request_text(self.http.delete(self.url(path)).timeout(timeout))?;
        self.parse_response(path, &body)
    }

    pub(super) fn request_text(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<String, String> {
        let response = request.send().map_err(|err| {
            let err_text = err.to_string();
            if err.is_timeout() {
                self.text.api_timeout(&self.base_url, &err_text)
            } else if err.is_connect() {
                self.text.api_connect_failed(&self.base_url, &err_text)
            } else {
                self.text.api_request_failed(&self.base_url, &err_text)
            }
        })?;
        let status = response.status();
        let text = response.text().map_err(|err| err.to_string())?;
        if status.is_success() {
            Ok(text)
        } else {
            Err(format!("HTTP {status}: {text}"))
        }
    }

    pub(super) fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    fn parse_response<T: DeserializeOwned>(&self, path: &str, body: &str) -> Result<T, String> {
        serde_json::from_str(body)
            .map_err(|err| self.text.api_response_parse_failed(path, &err.to_string()))
    }

    pub(super) fn local_port(&self) -> Option<u16> {
        let url = reqwest::Url::parse(&self.base_url).ok()?;
        let host = url.host_str()?;
        matches!(host, "127.0.0.1" | "localhost" | "::1").then_some(url.port_or_known_default()?)
    }

    pub(super) fn dashboard(&self) -> DashboardSnapshot {
        let dashboard = match self.get_quick::<GuiDashboardResponse>("/api/gui/dashboard") {
            Ok(dashboard) => dashboard,
            Err(_err) => {
                return self.dashboard_fallback();
            }
        };
        DashboardSnapshot::from_dashboard_response(dashboard)
    }

    fn dashboard_fallback(&self) -> DashboardSnapshot {
        let local_connection_mode = self.connection_mode();
        match self.get_quick::<ServerStatus>("/api/status") {
            Ok(status) => {
                let remote = self
                    .get_quick::<RemoteControlStatus>("/api/remote-control/status")
                    .ok();
                let codex_app = self
                    .get_quick::<CodexAppStatus>("/api/codex-app/status")
                    .ok();
                DashboardSnapshot::from_status_fallback(status, remote, codex_app)
            }
            Err(_err) => {
                let compatible_connection_available = local_connection_mode
                    == LocalConnectionMode::Standard
                    && self.probe_base_url("http://localhost:3847");
                DashboardSnapshot {
                    local_connection_mode,
                    compatible_connection_available,
                    ..DashboardSnapshot::default()
                }
            }
        }
    }

    pub(super) fn connection_mode(&self) -> LocalConnectionMode {
        if self
            .base_url
            .to_ascii_lowercase()
            .starts_with("http://localhost:")
        {
            LocalConnectionMode::VpnCompatible
        } else {
            LocalConnectionMode::Standard
        }
    }

    fn probe_base_url(&self, base_url: &str) -> bool {
        let url = format!("{}/api/status", base_url.trim_end_matches('/'));
        let Ok(response) = self.http.get(url).timeout(GUI_STATUS_TIMEOUT).send() else {
            return false;
        };
        response.status().is_success()
            && response
                .json::<DaemonIdentity>()
                .is_ok_and(|identity| identity.is_codexhub())
    }

    pub(super) fn configure_codex_app(
        &self,
        request: &ConfigureRequest,
    ) -> Result<serde_json::Value, String> {
        self.post_json_with_timeout("/api/codex-app/configure", request, GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn uninstall_codex_app(&self) -> Result<serde_json::Value, String> {
        self.post_empty_with_timeout("/api/codex-app/uninstall", GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn codex_app_sessions(&self) -> Result<CodexAppSessionsResponse, String> {
        self.get_with_timeout("/api/codex-app/sessions", GUI_SESSION_HISTORY_TIMEOUT)
    }

    pub(super) fn codex_app_status(&self) -> Result<CodexAppStatus, String> {
        self.get_with_timeout("/api/codex-app/status", GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn move_codex_app_session_provider(
        &self,
        request: &MoveCodexAppSessionProviderRequest,
    ) -> Result<serde_json::Value, String> {
        self.post_json_with_timeout(
            "/api/codex-app/session/provider",
            request,
            GUI_CONFIG_TIMEOUT,
        )
    }

    pub(super) fn repair_codex_app_gui_environment(&self) -> Result<serde_json::Value, String> {
        self.post_empty_with_timeout("/api/codex-app/repair-gui-environment", GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn set_codex_app_fast_startup(
        &self,
        request: &SetCodexAppFastStartupRequest,
    ) -> Result<serde_json::Value, String> {
        self.post_json_with_timeout("/api/codex-app/fast-startup", request, GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn refresh_codex_app_models(&self) -> Result<serde_json::Value, String> {
        self.post_empty_with_timeout("/api/codex-app/models/refresh", GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn set_im_account_enabled(
        &self,
        request: &SetImAccountEnabledRequest,
    ) -> Result<serde_json::Value, String> {
        self.post_json_with_timeout("/api/im/account/enabled", request, GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn delete_im_account(
        &self,
        request: &DeleteImAccountRequest,
    ) -> Result<serde_json::Value, String> {
        self.post_json_with_timeout("/api/im/account/delete", request, GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn shutdown(&self) -> Result<serde_json::Value, String> {
        self.post_empty("/api/shutdown")
    }

    pub(super) fn start_feishu_onboard(&self) -> Result<FeishuOnboardStart, String> {
        self.post_empty("/api/feishu/onboard/start")
    }

    pub(super) fn poll_feishu_onboard(
        &self,
        device_code: &str,
    ) -> Result<FeishuOnboardPoll, String> {
        self.post_json(
            "/api/feishu/onboard/poll",
            &serde_json::json!({ "deviceCode": device_code }),
        )
    }

    pub(super) fn configure_telegram_bot(
        &self,
        request: &ConfigureTelegramBotRequest,
    ) -> Result<serde_json::Value, String> {
        self.post_json_with_timeout("/api/telegram/configure", request, GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn start_wechat_onboard(&self) -> Result<WechatOnboardStart, String> {
        self.post_empty_with_timeout("/api/wechat/onboard/start", GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn poll_wechat_onboard(
        &self,
        session_key: &str,
        verify_code: Option<&str>,
    ) -> Result<WechatOnboardPoll, String> {
        self.post_json_with_timeout(
            "/api/wechat/onboard/poll",
            &serde_json::json!({
                "sessionKey": session_key,
                "verifyCode": verify_code,
            }),
            GUI_WECHAT_ONBOARD_POLL_TIMEOUT,
        )
    }

    pub(super) fn get_app_config(&self) -> Result<AppConfig, String> {
        self.get_with_timeout("/api/config", GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn save_app_config(&self, config: &AppConfig) -> Result<serde_json::Value, String> {
        self.post_json_with_timeout("/api/config", config, GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn ai_gateway_request_logs(&self) -> Result<RequestLogsResponse, String> {
        self.get_with_timeout("/ai-gateway/request-logs?limit=200", GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn ai_gateway_request_log_detail(
        &self,
        id: i64,
    ) -> Result<RequestLogDetailResponse, String> {
        self.get_with_timeout(
            &format!("/ai-gateway/request-logs/{id}"),
            GUI_CONFIG_TIMEOUT,
        )
    }

    pub(super) fn ai_gateway_clear_old_request_logs(
        &self,
    ) -> Result<ClearRequestLogsResponse, String> {
        self.delete_with_timeout("/ai-gateway/request-logs/old?days=3", GUI_CONFIG_TIMEOUT)
    }

    pub(super) fn ai_gateway_clear_all_request_logs(
        &self,
    ) -> Result<ClearRequestLogsResponse, String> {
        self.delete_with_timeout("/ai-gateway/request-logs", GUI_CONFIG_TIMEOUT)
    }
}

#[derive(Clone, Default)]
pub(super) struct DashboardSnapshot {
    pub(super) service_online: bool,
    pub(super) local_connection_mode: LocalConnectionMode,
    pub(super) compatible_connection_available: bool,
    pub(super) status: Option<ServerStatus>,
    pub(super) remote: Option<RemoteControlStatus>,
    pub(super) codex_app: Option<CodexAppStatus>,
    pub(super) im_accounts: Option<ImAccountsResponse>,
    pub(super) ai_gateway: Option<AiGatewayConfig>,
}

impl DashboardSnapshot {
    fn from_dashboard_response(dashboard: GuiDashboardResponse) -> Self {
        let local_connection_mode = dashboard.status.local_connection_mode;
        Self {
            service_online: true,
            local_connection_mode,
            compatible_connection_available: false,
            remote: Some(dashboard.remote),
            codex_app: Some(dashboard.codex_app),
            im_accounts: Some(dashboard.im_accounts),
            status: Some(dashboard.status),
            ai_gateway: Some(dashboard.ai_gateway),
        }
    }

    fn from_status_fallback(
        status: ServerStatus,
        remote: Option<RemoteControlStatus>,
        codex_app: Option<CodexAppStatus>,
    ) -> Self {
        let local_connection_mode = status.local_connection_mode;
        Self {
            service_online: true,
            local_connection_mode,
            compatible_connection_available: false,
            remote,
            codex_app,
            status: Some(status),
            ..Self::default()
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GuiDashboardResponse {
    status: ServerStatus,
    remote: RemoteControlStatus,
    codex_app: CodexAppStatus,
    im_accounts: ImAccountsResponse,
    ai_gateway: AiGatewayConfig,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ServerStatus {
    pub(super) bind: String,
    #[serde(default)]
    pub(super) local_connection_mode: LocalConnectionMode,
    #[serde(default)]
    pub(super) codex_app_fast_startup: bool,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImAccountsResponse {
    pub(super) accounts: Vec<ImAccountItem>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImAccountItem {
    pub(super) platform: String,
    pub(super) account_id: String,
    pub(super) display_name: Option<String>,
    pub(super) enabled: bool,
    pub(super) configured: bool,
    pub(super) secret_set: bool,
    pub(super) connecting: bool,
    pub(super) polling: bool,
    pub(super) connected: bool,
    pub(super) last_error: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteControlStatus {
    pub(super) connected: bool,
    #[allow(dead_code)]
    pub(super) initialized: bool,
    pub(super) active_source_kind: Option<String>,
    #[serde(default)]
    pub(super) connections: Vec<RemoteControlConnectionStatus>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteControlConnectionStatus {
    pub(super) connected: bool,
    #[allow(dead_code)]
    pub(super) initialized: bool,
    pub(super) source_kind: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CodexAppStatus {
    pub(super) configured: bool,
    pub(super) provider: Option<CodexAppProviderStatus>,
    #[serde(default)]
    pub(super) providers: Vec<CodexAppProviderStatus>,
    #[serde(default = "default_true")]
    #[allow(dead_code)]
    pub(super) image_generation_enabled: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CodexAppProviderStatus {
    pub(super) name: String,
    pub(super) base_url: Option<String>,
    pub(super) key: Option<String>,
    #[serde(default)]
    pub(super) supports_websockets: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CodexAppSessionsResponse {
    #[allow(dead_code)]
    pub(super) ok: bool,
    #[serde(default)]
    pub(super) threads: Vec<CodexAppThread>,
    #[serde(default)]
    pub(super) providers: Vec<String>,
    #[serde(default)]
    pub(super) total: usize,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct CodexAppThread {
    pub(super) id: String,
    #[serde(default)]
    pub(super) preview: String,
    pub(super) model_provider: String,
    #[serde(default)]
    pub(super) updated_at: i64,
    #[serde(default)]
    pub(super) path: Option<String>,
    #[serde(default)]
    pub(super) cwd: Option<Value>,
    #[serde(default)]
    pub(super) name: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MoveCodexAppSessionProviderRequest {
    pub(super) thread_id: String,
    pub(super) rollout_path: Option<String>,
    pub(super) target_provider: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetCodexAppFastStartupRequest {
    pub(super) enabled: bool,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_fallback_keeps_service_online() {
        let snapshot = DashboardSnapshot::from_status_fallback(
            ServerStatus {
                bind: "127.0.0.1:3847".to_string(),
                local_connection_mode: LocalConnectionMode::Standard,
                codex_app_fast_startup: true,
            },
            None,
            None,
        );

        assert!(snapshot.service_online);
        assert_eq!(
            snapshot.local_connection_mode,
            LocalConnectionMode::Standard
        );
        assert!(snapshot.status.is_some());
        assert!(snapshot.remote.is_none());
        assert!(snapshot.codex_app.is_none());
        assert!(snapshot.im_accounts.is_none());
        assert!(snapshot.ai_gateway.is_none());
    }

    #[test]
    fn status_fallback_keeps_terminal_status_when_available() {
        let remote = RemoteControlStatus {
            connected: true,
            initialized: true,
            active_source_kind: Some("vscode".to_string()),
            connections: vec![RemoteControlConnectionStatus {
                connected: true,
                initialized: true,
                source_kind: "vscode".to_string(),
            }],
        };
        let codex_app = CodexAppStatus {
            configured: true,
            provider: None,
            providers: Vec::new(),
            image_generation_enabled: true,
        };
        let snapshot = DashboardSnapshot::from_status_fallback(
            ServerStatus {
                bind: "127.0.0.1:3847".to_string(),
                local_connection_mode: LocalConnectionMode::Standard,
                codex_app_fast_startup: true,
            },
            Some(remote),
            Some(codex_app),
        );

        assert!(snapshot.service_online);
        assert!(snapshot.remote.is_some());
        assert!(snapshot.codex_app.is_some());
        assert!(snapshot.im_accounts.is_none());
        assert!(snapshot.ai_gateway.is_none());
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestLogsResponse {
    #[serde(default)]
    pub(super) logs: Vec<RequestLogItem>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClearRequestLogsResponse {
    pub(super) deleted: usize,
}

#[derive(Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestLogItem {
    pub(super) id: i64,
    pub(super) request_id: String,
    pub(super) model_id: String,
    pub(super) stream: bool,
    pub(super) channel: String,
    pub(super) provider_type: String,
    pub(super) status: String,
    pub(super) input_tokens: Option<i64>,
    pub(super) output_tokens: Option<i64>,
    pub(super) total_tokens: Option<i64>,
    pub(super) read_cache_tokens: Option<i64>,
    pub(super) read_cache_hit_rate: Option<f64>,
    pub(super) write_cache_tokens: Option<i64>,
    #[serde(default)]
    pub(super) write_cache_5m_tokens: Option<i64>,
    #[serde(default)]
    pub(super) write_cache_1h_tokens: Option<i64>,
    pub(super) cost_usd: Option<f64>,
    pub(super) latency_ms: Option<i64>,
    pub(super) ttft_ms: Option<i64>,
    pub(super) created_at: String,
    pub(super) error_message: Option<String>,
    pub(super) upstream_request_body_bytes: Option<i64>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestLogDetailResponse {
    pub(super) log: RequestLogDetail,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RequestLogDetail {
    #[serde(flatten)]
    pub(super) summary: RequestLogItem,
    pub(super) request_headers_json: Option<String>,
    pub(super) request_json: Option<String>,
    pub(super) upstream_request_headers_json: Option<String>,
    pub(super) upstream_request_json: Option<String>,
    pub(super) upstream_response_sse: Option<String>,
    pub(super) response_json: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConfigureRequest {
    pub(super) connection_mode: LocalConnectionMode,
    pub(super) provider_name: Option<String>,
    pub(super) provider_base_url: Option<String>,
    pub(super) provider_key: Option<String>,
    pub(super) activate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) image_generation_enabled: Option<bool>,
    pub(super) supports_websockets: bool,
    pub(super) fast_startup: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConfigureTelegramBotRequest {
    pub(super) bot_token: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetImAccountEnabledRequest {
    pub(super) platform: String,
    pub(super) account_id: String,
    pub(super) enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeleteImAccountRequest {
    pub(super) platform: String,
    pub(super) account_id: String,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct FeishuOnboardStart {
    pub(super) verification_uri_complete: String,
    pub(super) device_code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FeishuOnboardPoll {
    pub(super) done: bool,
    pub(super) error: Option<serde_json::Value>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct WechatOnboardStart {
    pub(super) session_key: String,
    pub(super) qrcode_url: String,
    pub(super) expires_in: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WechatOnboardPoll {
    pub(super) done: bool,
    pub(super) status: Option<String>,
    pub(super) error: Option<serde_json::Value>,
    pub(super) need_verify_code: Option<bool>,
    pub(super) already_connected: Option<bool>,
}

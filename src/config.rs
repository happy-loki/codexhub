use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

const DEFAULT_BIND: &str = "127.0.0.1:3847";
const LEGACY_DEFAULT_BIND: &str = "127.0.0.1:8000";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    pub bind: String,
    pub local_connection_mode: LocalConnectionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default)]
    pub codex_app_fast_startup: bool,
    pub state_path: PathBuf,
    pub logging: LoggingConfig,
    pub feishu: FeishuConfig,
    pub telegram: TelegramConfig,
    pub wechat: WechatConfig,
    pub feishu_accounts: Vec<FeishuConfig>,
    pub telegram_accounts: Vec<TelegramConfig>,
    pub wechat_accounts: Vec<WechatConfig>,
    pub bridge: BridgeConfig,
    pub ai_gateway: crate::ai_gateway::config::AiGatewayConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalConnectionMode {
    #[default]
    Standard,
    VpnCompatible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FeishuConfig {
    pub enabled: bool,
    pub account_id: String,
    pub app_id: String,
    pub app_secret: String,
    pub display_name: String,
    pub mention_only: bool,
    pub allowed_open_ids: Vec<String>,
    pub allowed_chat_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelegramConfig {
    pub enabled: bool,
    pub account_id: String,
    #[serde(alias = "bot_token")]
    pub bot_token: String,
    pub display_name: String,
    #[serde(alias = "mention_only")]
    pub mention_only: bool,
    #[serde(alias = "allowed_chat_ids")]
    pub allowed_chat_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WechatConfig {
    pub enabled: bool,
    pub account_id: String,
    pub bot_token: String,
    pub display_name: String,
    pub base_url: String,
    pub user_id: String,
    pub bot_type: String,
    pub allowed_user_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BridgeConfig {
    pub enabled: bool,
    pub account_id: String,
    pub send_streaming: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LoggingConfig {
    pub diagnostic: bool,
    pub max_mb: u64,
    pub retention_days: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_BIND.to_string(),
            local_connection_mode: LocalConnectionMode::default(),
            language: None,
            theme: None,
            codex_app_fast_startup: false,
            state_path: PathBuf::from("codex-remote-gateway-state.json"),
            logging: LoggingConfig::default(),
            feishu: FeishuConfig::default(),
            telegram: TelegramConfig::default(),
            wechat: WechatConfig::default(),
            feishu_accounts: Vec::new(),
            telegram_accounts: Vec::new(),
            wechat_accounts: Vec::new(),
            bridge: BridgeConfig::default(),
            ai_gateway: crate::ai_gateway::config::AiGatewayConfig::default(),
        }
    }
}

impl Default for FeishuConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            account_id: String::new(),
            app_id: String::new(),
            app_secret: String::new(),
            display_name: String::new(),
            mention_only: true,
            allowed_open_ids: Vec::new(),
            allowed_chat_ids: Vec::new(),
        }
    }
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            account_id: String::new(),
            bot_token: String::new(),
            display_name: String::new(),
            mention_only: false,
            allowed_chat_ids: Vec::new(),
        }
    }
}

impl Default for WechatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            account_id: "wechat".to_string(),
            bot_token: String::new(),
            display_name: String::new(),
            base_url: String::new(),
            user_id: String::new(),
            bot_type: "3".to_string(),
            allowed_user_ids: Vec::new(),
        }
    }
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            account_id: "default".to_string(),
            send_streaming: true,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            diagnostic: cfg!(debug_assertions),
            max_mb: 20,
            retention_days: 7,
        }
    }
}

impl AppConfig {
    pub fn apply_platform_defaults(&mut self) -> bool {
        let mut changed = false;
        if self.bind == LEGACY_DEFAULT_BIND {
            self.bind = DEFAULT_BIND.to_string();
            changed = true;
        }

        if self.migrate_legacy_im_accounts() {
            changed = true;
        }

        changed
    }

    pub fn remote_control_base_url(&self) -> String {
        self.remote_control_base_url_for_mode(self.local_connection_mode)
    }

    pub fn remote_control_base_url_for_mode(&self, mode: LocalConnectionMode) -> String {
        let host_port = self
            .bind
            .parse::<SocketAddr>()
            .ok()
            .map(|addr| {
                let host = if addr.ip().is_loopback() || addr.ip().is_unspecified() {
                    match mode {
                        LocalConnectionMode::Standard => "127.0.0.1".to_string(),
                        LocalConnectionMode::VpnCompatible => "localhost".to_string(),
                    }
                } else {
                    let host = addr.ip().to_string();
                    if host.contains(':') {
                        format!("[{host}]")
                    } else {
                        host
                    }
                };
                format!("{host}:{}", addr.port())
            })
            .unwrap_or_else(|| self.bind.clone());
        format!("http://{host_port}/backend-api")
    }

    pub fn load_or_default(path: &PathBuf) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("failed to parse config {}", path.display()))
    }

    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        let raw = toml::to_string_pretty(self)?;
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }
        std::fs::write(path, raw)
            .with_context(|| format!("failed to write config {}", path.display()))
    }

    pub fn migrate_legacy_im_accounts(&mut self) -> bool {
        let mut changed = false;
        if self.feishu_accounts.is_empty() && self.feishu.is_configured() {
            let mut account = self.feishu.clone();
            if account.account_id.trim().is_empty() {
                account.account_id = non_empty(&self.bridge.account_id)
                    .or_else(|| non_empty(&account.app_id))
                    .unwrap_or_else(|| "default".to_string());
            }
            self.feishu_accounts.push(account);
            changed = true;
        }
        if self.telegram_accounts.is_empty() && self.telegram.is_configured() {
            let mut account = self.telegram.clone();
            if account.account_id.trim().is_empty() {
                account.account_id = "telegram".to_string();
            }
            self.telegram_accounts.push(account);
            changed = true;
        }
        if self.wechat_accounts.is_empty() && self.wechat.is_configured() {
            let mut account = self.wechat.clone();
            if account.account_id.trim().is_empty() {
                account.account_id = "wechat".to_string();
            }
            self.wechat_accounts.push(account);
            changed = true;
        }
        changed
    }

    pub fn effective_feishu_accounts(&self) -> Vec<FeishuConfig> {
        effective_accounts(&self.feishu_accounts, &self.feishu, |account| {
            account.is_configured()
        })
        .into_iter()
        .map(|mut account| {
            if account.account_id.trim().is_empty() {
                account.account_id =
                    non_empty(&account.app_id).unwrap_or_else(|| "default".to_string());
            }
            account
        })
        .collect()
    }

    pub fn feishu_account(&self, account_id: &str) -> Option<FeishuConfig> {
        find_account(&self.effective_feishu_accounts(), account_id, |account| {
            account.account_id.as_str()
        })
    }

    pub fn effective_telegram_accounts(&self) -> Vec<TelegramConfig> {
        effective_accounts(&self.telegram_accounts, &self.telegram, |account| {
            account.is_configured()
        })
        .into_iter()
        .map(|mut account| {
            if account.account_id.trim().is_empty() {
                account.account_id = "telegram".to_string();
            }
            account
        })
        .collect()
    }

    pub fn telegram_account(&self, account_id: &str) -> Option<TelegramConfig> {
        find_account(&self.effective_telegram_accounts(), account_id, |account| {
            account.account_id.as_str()
        })
    }

    pub fn effective_wechat_accounts(&self) -> Vec<WechatConfig> {
        effective_accounts(&self.wechat_accounts, &self.wechat, |account| {
            account.is_configured()
        })
        .into_iter()
        .map(|mut account| {
            if account.account_id.trim().is_empty() {
                account.account_id = "wechat".to_string();
            }
            account
        })
        .collect()
    }

    pub fn wechat_account(&self, account_id: &str) -> Option<WechatConfig> {
        find_account(&self.effective_wechat_accounts(), account_id, |account| {
            account.account_id.as_str()
        })
    }

    pub fn upsert_feishu_account(&mut self, account: FeishuConfig) {
        upsert_account(&mut self.feishu_accounts, account, |account| {
            account.account_id.as_str()
        });
    }

    pub fn upsert_telegram_account(&mut self, account: TelegramConfig) {
        upsert_account(&mut self.telegram_accounts, account, |account| {
            account.account_id.as_str()
        });
    }

    pub fn upsert_wechat_account(&mut self, account: WechatConfig) {
        upsert_account(&mut self.wechat_accounts, account, |account| {
            account.account_id.as_str()
        });
    }

    pub fn remove_im_account(&mut self, platform: &str, account_id: &str) -> bool {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return false;
        }
        match platform.trim().to_ascii_lowercase().as_str() {
            "feishu" => remove_account(&mut self.feishu_accounts, account_id, |account| {
                account.account_id.as_str()
            }),
            "telegram" => remove_account(&mut self.telegram_accounts, account_id, |account| {
                account.account_id.as_str()
            }),
            "wechat" => remove_account(&mut self.wechat_accounts, account_id, |account| {
                account.account_id.as_str()
            }),
            _ => false,
        }
    }

    pub fn set_im_account_enabled(
        &mut self,
        platform: &str,
        account_id: &str,
        enabled: bool,
    ) -> bool {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return false;
        }
        match platform.trim().to_ascii_lowercase().as_str() {
            "feishu" => set_account_enabled(
                &mut self.feishu_accounts,
                account_id,
                enabled,
                |account| account.account_id.as_str(),
                |account| &mut account.enabled,
            ),
            "telegram" => set_account_enabled(
                &mut self.telegram_accounts,
                account_id,
                enabled,
                |account| account.account_id.as_str(),
                |account| &mut account.enabled,
            ),
            "wechat" => set_account_enabled(
                &mut self.wechat_accounts,
                account_id,
                enabled,
                |account| account.account_id.as_str(),
                |account| &mut account.enabled,
            ),
            _ => false,
        }
    }

    pub fn ensure_telegram_allowed_chat_id(
        &mut self,
        account_id: &str,
        chat_id: &str,
    ) -> TelegramChatAllowResult {
        let account_id = account_id.trim();
        let chat_id = chat_id.trim();
        if account_id.is_empty() || chat_id.is_empty() {
            return TelegramChatAllowResult::AccountNotFound;
        }
        self.migrate_legacy_im_accounts();
        let Some(account) = self.telegram_accounts.iter_mut().find(|account| {
            account.account_id.trim() == account_id
                || (account.account_id.trim().is_empty() && account_id == "telegram")
        }) else {
            if self.telegram.account_id.trim() == account_id
                || (self.telegram.account_id.trim().is_empty() && account_id == "telegram")
            {
                return ensure_telegram_chat_id_on_account(&mut self.telegram, chat_id);
            }
            return TelegramChatAllowResult::AccountNotFound;
        };
        let result = ensure_telegram_chat_id_on_account(account, chat_id);
        if self.telegram.account_id.trim() == account_id
            || (self.telegram.account_id.trim().is_empty() && account_id == "telegram")
        {
            self.telegram.allowed_chat_ids = account.allowed_chat_ids.clone();
        }
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelegramChatAllowResult {
    Allowed,
    Bound,
    Denied,
    AccountNotFound,
}

impl TelegramChatAllowResult {
    pub fn should_save(self) -> bool {
        matches!(self, Self::Bound)
    }
}

fn ensure_telegram_chat_id_on_account(
    account: &mut TelegramConfig,
    chat_id: &str,
) -> TelegramChatAllowResult {
    if account
        .allowed_chat_ids
        .iter()
        .any(|allowed| allowed.trim() == chat_id)
    {
        return TelegramChatAllowResult::Allowed;
    }
    if !account.allowed_chat_ids.is_empty() {
        return TelegramChatAllowResult::Denied;
    }
    account.allowed_chat_ids.push(chat_id.to_string());
    TelegramChatAllowResult::Bound
}

impl FeishuConfig {
    pub fn is_configured(&self) -> bool {
        !self.app_id.trim().is_empty() && !self.app_secret.trim().is_empty()
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.is_configured()
    }
}

impl TelegramConfig {
    pub fn is_configured(&self) -> bool {
        !self.bot_token.trim().is_empty()
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.is_configured()
    }
}

impl WechatConfig {
    pub fn is_configured(&self) -> bool {
        !self.bot_token.trim().is_empty()
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.is_configured()
    }
}

fn effective_accounts<T: Clone>(
    accounts: &[T],
    legacy: &T,
    configured: impl Fn(&T) -> bool,
) -> Vec<T> {
    if !accounts.is_empty() {
        accounts.to_vec()
    } else if configured(legacy) {
        vec![legacy.clone()]
    } else {
        Vec::new()
    }
}

fn upsert_account<T>(accounts: &mut Vec<T>, account: T, account_id: impl Fn(&T) -> &str) {
    let id = account_id(&account).trim().to_string();
    if let Some(existing) = accounts
        .iter_mut()
        .find(|existing| account_id(existing).trim() == id)
    {
        *existing = account;
    } else {
        accounts.push(account);
    }
}

fn remove_account<T>(
    accounts: &mut Vec<T>,
    account_id: &str,
    get_account_id: impl Fn(&T) -> &str,
) -> bool {
    let before = accounts.len();
    accounts.retain(|account| get_account_id(account).trim() != account_id);
    accounts.len() != before
}

fn set_account_enabled<T>(
    accounts: &mut [T],
    account_id: &str,
    enabled: bool,
    get_account_id: impl Fn(&T) -> &str,
    get_enabled: impl Fn(&mut T) -> &mut bool,
) -> bool {
    let Some(account) = accounts
        .iter_mut()
        .find(|account| get_account_id(account).trim() == account_id)
    else {
        return false;
    };
    *get_enabled(account) = enabled;
    true
}

fn find_account<T: Clone>(
    accounts: &[T],
    account_id: &str,
    get_account_id: impl Fn(&T) -> &str,
) -> Option<T> {
    let account_id = account_id.trim();
    accounts
        .iter()
        .find(|account| get_account_id(account).trim() == account_id)
        .cloned()
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, TelegramChatAllowResult, TelegramConfig};

    #[test]
    fn telegram_empty_allowlist_binds_first_private_chat() {
        let mut config = AppConfig::default();
        config.telegram_accounts.push(TelegramConfig {
            account_id: "tg_1".to_string(),
            bot_token: "token".to_string(),
            ..TelegramConfig::default()
        });

        assert_eq!(
            config.ensure_telegram_allowed_chat_id("tg_1", "123"),
            TelegramChatAllowResult::Bound
        );
        assert_eq!(
            config.telegram_accounts[0].allowed_chat_ids,
            vec!["123".to_string()]
        );
        assert_eq!(
            config.ensure_telegram_allowed_chat_id("tg_1", "123"),
            TelegramChatAllowResult::Allowed
        );
        assert_eq!(
            config.ensure_telegram_allowed_chat_id("tg_1", "456"),
            TelegramChatAllowResult::Denied
        );
    }

    #[test]
    fn telegram_legacy_default_account_can_bind_first_chat() {
        let mut config = AppConfig::default();
        config.telegram.bot_token = "token".to_string();

        assert_eq!(
            config.ensure_telegram_allowed_chat_id("telegram", "123"),
            TelegramChatAllowResult::Bound
        );
        assert_eq!(config.telegram.allowed_chat_ids, vec!["123".to_string()]);
        assert_eq!(
            config.telegram_accounts[0].allowed_chat_ids,
            vec!["123".to_string()]
        );
    }
}

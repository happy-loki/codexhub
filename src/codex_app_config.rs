#[cfg(target_os = "macos")]
use std::process::Command;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
#[cfg(target_os = "windows")]
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

use crate::{chain_log, config::LocalConnectionMode};

const DEFAULT_PROVIDER_NAME: &str = "ai-codex";
const AI_GATEWAY_PROVIDER_NAME: &str = "ai-gateway";
const CODEX_API_BASE_URL_ENV: &str = "CODEX_API_BASE_URL";
const CODEX_APP_SERVER_LOGIN_ISSUER_ENV: &str = "CODEX_APP_SERVER_LOGIN_ISSUER";
const CODEX_APP_SQLITE_DIR: &str = "sqlite";
const CODEX_APP_PRIMARY_DB: &str = "codex.db";
const CODEX_APP_DEV_DB: &str = "codex-dev.db";
const CODEX_APP_REMOTE_CONTROL_FEATURE: &str = "remote_control";
const CODEX_MODELS_CACHE_FILE: &str = "models_cache.json";
const SQLITE_WRITE_BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const SQLITE_INSPECT_BUSY_TIMEOUT: Duration = Duration::from_millis(150);
const CODEXHUB_HOME_ENV: &str = "CODEXHUB_HOME";
const OPENAI_BUNDLED_MARKETPLACE_NAME: &str = "openai-bundled";
const OPENAI_CURATED_MARKETPLACE_NAME: &str = "openai-curated";
const CODEXHUB_BUNDLED_REMOTE_ID_PREFIX: &str = "plugins~codexhub-bundled-";
const REMOTE_PLUGIN_INSTALL_METADATA_FILE: &str = ".codex-remote-plugin-install.json";
const PLUGIN_BLOCKING_FEATURE_FLAGS: &[&str] = &[
    "apps",
    "plugins",
    "computer_use",
    "browser_use",
    "in_app_browser",
];
const REQUIRED_OPENAI_BUNDLED_PLUGIN_IDS: &[&str] = &[
    "browser@openai-bundled",
    "chrome@openai-bundled",
    "computer-use@openai-bundled",
];

const LOCAL_AUTH_MODE: &str = "chatgptAuthTokens";
const LEGACY_LOCAL_AUTH_MODE: &str = "chatgpt";
const LEGACY_BAD_LOCAL_AUTH_API_KEY: &str = "codexhub-dummy-key";

const MANAGED_BACKUP_VERSION: u32 = 1;
const MANAGED_BACKUP_MANIFEST: &str = "manifest.json";
const MANAGED_BACKUP_AUTH: &str = "auth.json";

#[derive(Debug, Clone)]
pub struct ConfigureCodexAppOptions {
    pub codex_home: Option<PathBuf>,
    pub backend_url: String,
    pub connection_mode: LocalConnectionMode,
    pub account_id: String,
    pub user_id: String,
    pub email: String,
    pub plan_type: String,
    pub provider_name: Option<String>,
    pub provider_base_url: Option<String>,
    pub provider_key: Option<String>,
    pub activate_provider: bool,
    #[allow(dead_code)]
    pub image_generation_enabled: Option<bool>,
    pub provider_supports_websockets: Option<bool>,
}

impl ConfigureCodexAppOptions {
    fn codex_backend_url(&self) -> String {
        codex_connection_url(&self.backend_url, self.connection_mode)
    }

    fn ai_gateway_base_url(&self) -> String {
        let backend_url = self.codex_backend_url();
        let backend = backend_url.trim_end_matches('/');
        if let Some(base) = backend.strip_suffix("/backend-api") {
            format!("{base}/ai-gateway/v1")
        } else {
            format!("{backend}/ai-gateway/v1")
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigureCodexAppReport {
    pub codex_home: PathBuf,
    pub config_path: PathBuf,
    pub auth_path: PathBuf,
    pub backend_url: String,
    pub remote_control_switch: CodexAppRemoteControlSwitchStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallCodexAppReport {
    pub codex_home: PathBuf,
    pub config_path: PathBuf,
    pub auth_path: PathBuf,
    pub removed_chatgpt_base_url: bool,
    pub removed_model_provider: bool,
    pub removed_auth: bool,
    pub gui_api_base: CodexAppGuiApiBaseStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppGuiApiBaseStatus {
    pub supported: bool,
    pub configured: bool,
    pub expected: String,
    pub value: Option<String>,
    pub login_issuer_configured: bool,
    pub login_issuer_expected: String,
    pub login_issuer_value: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppConfigStatus {
    pub codex_home: PathBuf,
    pub config_path: PathBuf,
    pub auth_path: PathBuf,
    pub configured: bool,
    pub config_ok: bool,
    pub auth_ok: bool,
    pub provider_ok: bool,
    pub config_error: Option<String>,
    pub auth_error: Option<String>,
    pub gui_api_base: CodexAppGuiApiBaseStatus,
    pub remote_control_switch: CodexAppRemoteControlSwitchStatus,
    pub provider: Option<CodexAppProviderStatus>,
    pub providers: Vec<CodexAppProviderStatus>,
    pub image_generation_enabled: bool,
    pub connection_mode: LocalConnectionMode,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppRemoteControlSwitchStatus {
    pub supported: bool,
    pub configured: bool,
    pub feature_name: String,
    pub databases: Vec<CodexAppRemoteControlSwitchDatabaseStatus>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppRemoteControlSwitchDatabaseStatus {
    pub path: PathBuf,
    pub exists: bool,
    pub enabled: Option<bool>,
    pub updated_at: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAppProviderStatus {
    pub name: String,
    pub base_url: Option<String>,
    pub key: Option<String>,
    pub supports_websockets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManagedCodexAppBackupManifest {
    version: u32,
    created_at_ms: i64,
    codex_home: PathBuf,
    config_existed: bool,
    auth_existed: bool,
}

pub fn configure_codex_app(options: ConfigureCodexAppOptions) -> Result<ConfigureCodexAppReport> {
    let codex_home = options
        .codex_home
        .clone()
        .unwrap_or_else(default_codex_home);
    chain_log::write_line(format!(
        "[codex_app_config] event=configure_start codex_home={} provider={} activate_provider={}",
        codex_home.display(),
        options.provider_name.as_deref().unwrap_or_default(),
        options.activate_provider
    ));
    std::fs::create_dir_all(&codex_home)
        .with_context(|| format!("failed to create Codex home {}", codex_home.display()))?;

    let config_path = codex_home.join("config.toml");
    let auth_path = codex_home.join("auth.json");
    ensure_managed_backup(&codex_home, &config_path, &auth_path)?;
    chain_log::write_line(format!(
        "[codex_app_config] event=write_config_start path={}",
        config_path.display()
    ));
    write_config_toml(&config_path, &options)?;
    chain_log::write_line(format!(
        "[codex_app_config] event=write_config_done path={}",
        config_path.display()
    ));

    chain_log::write_line(format!(
        "[codex_app_config] event=write_auth_start path={}",
        auth_path.display()
    ));
    write_auth_json(&auth_path, &options)?;
    chain_log::write_line(format!(
        "[codex_app_config] event=write_auth_done path={}",
        auth_path.display()
    ));

    chain_log::write_line(format!(
        "[codex_app_config] event=clear_models_cache_start codex_home={}",
        codex_home.display()
    ));
    let removed_models_cache = clear_codex_models_cache(Some(codex_home.clone()))?;
    chain_log::write_line(format!(
        "[codex_app_config] event=clear_models_cache_done removed={removed_models_cache}"
    ));

    chain_log::write_line("[codex_app_config] event=clear_legacy_bundled_plugin_state_start");
    let legacy_plugin_cleanup = clear_legacy_codexhub_bundled_plugin_state(&codex_home)?;
    chain_log::write_line(format!(
        "[codex_app_config] event=clear_legacy_bundled_plugin_state_done removed_identity_files={} removed_catalog_files={}",
        legacy_plugin_cleanup.removed_remote_identity_files,
        legacy_plugin_cleanup.removed_remote_catalog_files
    ));

    chain_log::write_line("[codex_app_config] event=remote_control_switch_start");
    let remote_control_switch = enable_remote_control_switch_in_home(&codex_home)?;
    chain_log::write_line(format!(
        "[codex_app_config] event=remote_control_switch_done configured={}",
        remote_control_switch.configured
    ));

    #[cfg(not(test))]
    chain_log::write_line("[codex_app_config] event=gui_environment_cleanup_start");
    #[cfg(not(test))]
    let _ = cleanup_gui_environment(&options.backend_url);
    #[cfg(not(test))]
    chain_log::write_line("[codex_app_config] event=gui_environment_cleanup_done");

    Ok(ConfigureCodexAppReport {
        codex_home,
        config_path,
        auth_path,
        backend_url: options.backend_url,
        remote_control_switch,
    })
}

pub fn uninstall_codex_app(
    codex_home: Option<PathBuf>,
    backend_url: &str,
) -> Result<UninstallCodexAppReport> {
    let codex_home = codex_home.unwrap_or_else(default_codex_home);
    let config_path = codex_home.join("config.toml");
    let auth_path = codex_home.join("auth.json");

    let backup = managed_backup_paths(&codex_home);
    let (removed_chatgpt_base_url, removed_model_provider, removed_auth) =
        if backup.manifest_path.exists() {
            uninstall_with_managed_state(&backup, &config_path, &auth_path, backend_url)?
        } else {
            let (removed_chatgpt_base_url, removed_model_provider) =
                uninstall_config_toml(&config_path, backend_url)?;
            let removed_auth = uninstall_auth_json(&auth_path)?;
            (
                removed_chatgpt_base_url,
                removed_model_provider,
                removed_auth,
            )
        };
    #[cfg(not(test))]
    let gui_api_base = cleanup_gui_environment(backend_url);
    #[cfg(test)]
    let gui_api_base = inspect_gui_api_base_url(backend_url);

    Ok(UninstallCodexAppReport {
        codex_home,
        config_path,
        auth_path,
        removed_chatgpt_base_url,
        removed_model_provider,
        removed_auth,
        gui_api_base,
    })
}

pub fn inspect_codex_app_config(
    codex_home: Option<PathBuf>,
    backend_url: &str,
) -> CodexAppConfigStatus {
    let codex_home = codex_home.unwrap_or_else(default_codex_home);
    let config_path = codex_home.join("config.toml");
    let auth_path = codex_home.join("auth.json");

    let (config_ok, config_error) = inspect_config_toml(&config_path, backend_url);
    let (auth_ok, auth_error) = inspect_auth_json(&auth_path);
    let (provider, providers, image_generation_enabled) = inspect_provider_catalog(&config_path);
    let provider_ok = inspect_managed_ai_gateway_provider(&config_path, backend_url);
    let connection_mode = inspect_connection_mode(&config_path, backend_url);

    let gui_api_base = inspect_gui_api_base_url(backend_url);
    let gui_ok = gui_api_base.configured && gui_api_base.login_issuer_configured;
    let remote_control_switch = inspect_remote_control_switch_in_home(&codex_home);
    let remote_control_ok = remote_control_switch.configured;

    CodexAppConfigStatus {
        codex_home,
        config_path,
        auth_path,
        configured: config_ok && auth_ok && provider_ok && gui_ok && remote_control_ok,
        config_ok,
        auth_ok,
        provider_ok,
        config_error,
        auth_error,
        gui_api_base,
        remote_control_switch,
        provider,
        providers,
        image_generation_enabled,
        connection_mode,
    }
}

pub fn enable_codex_app_remote_control_switch(
    codex_home: Option<PathBuf>,
) -> Result<CodexAppRemoteControlSwitchStatus> {
    let codex_home = codex_home.unwrap_or_else(default_codex_home);
    enable_remote_control_switch_in_home(&codex_home)
}

pub fn delete_codex_app_provider(
    codex_home: Option<PathBuf>,
    provider_name: &str,
) -> Result<PathBuf> {
    let codex_home = codex_home.unwrap_or_else(default_codex_home);
    let config_path = codex_home.join("config.toml");
    delete_provider_from_config_toml(&config_path, provider_name)?;
    Ok(config_path)
}

pub fn set_codex_app_provider_websocket(
    codex_home: Option<PathBuf>,
    provider_name_value: &str,
    enabled: bool,
) -> Result<PathBuf> {
    let provider_name = provider_name(Some(provider_name_value))?;
    let codex_home = codex_home.unwrap_or_else(default_codex_home);
    let config_path = codex_home.join("config.toml");
    let mut doc = if config_path.exists() {
        parse_existing_config_toml(&config_path)?
    } else {
        toml_edit::DocumentMut::new()
    };

    let provider = provider_table_mut(&mut doc, &provider_name);
    provider["name"] = toml_edit::value(provider_name.as_str());
    provider["supports_websockets"] = toml_edit::value(enabled);

    let raw = normalize_config_toml_order(&doc.to_string());
    backup_existing(&config_path)?;
    std::fs::write(&config_path, raw)
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    Ok(config_path)
}

fn enable_remote_control_switch_in_home(
    codex_home: &Path,
) -> Result<CodexAppRemoteControlSwitchStatus> {
    let sqlite_dir = codex_home.join(CODEX_APP_SQLITE_DIR);
    std::fs::create_dir_all(&sqlite_dir).with_context(|| {
        format!(
            "failed to create Codex App sqlite directory {}",
            sqlite_dir.display()
        )
    })?;

    for db_path in codex_app_feature_db_paths(codex_home) {
        upsert_remote_control_feature(&db_path)?;
    }

    let status = inspect_remote_control_switch_in_home(codex_home);
    if status.configured {
        Ok(status)
    } else {
        Err(anyhow!(
            "{}",
            status.error.unwrap_or_else(|| {
                "remote_control switch was written but could not be verified".to_string()
            })
        ))
    }
}

fn inspect_remote_control_switch_in_home(codex_home: &Path) -> CodexAppRemoteControlSwitchStatus {
    let databases = codex_app_feature_db_paths(codex_home)
        .into_iter()
        .map(|path| inspect_remote_control_switch_db(&path))
        .collect::<Vec<_>>();
    let error = databases.iter().find_map(|db| db.error.clone());
    let configured = !databases.is_empty()
        && error.is_none()
        && databases
            .iter()
            .all(|db| db.exists && db.enabled == Some(true));

    CodexAppRemoteControlSwitchStatus {
        supported: true,
        configured,
        feature_name: CODEX_APP_REMOTE_CONTROL_FEATURE.to_string(),
        databases,
        error,
    }
}

fn codex_app_feature_db_paths(codex_home: &Path) -> Vec<PathBuf> {
    let sqlite_dir = codex_home.join(CODEX_APP_SQLITE_DIR);
    let primary = sqlite_dir.join(CODEX_APP_PRIMARY_DB);
    let dev = sqlite_dir.join(CODEX_APP_DEV_DB);
    if dev.exists() && dev != primary {
        vec![primary, dev]
    } else {
        vec![primary]
    }
}

fn upsert_remote_control_feature(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let connection = Connection::open(path)
        .with_context(|| format!("failed to open Codex App sqlite DB {}", path.display()))?;
    connection
        .busy_timeout(SQLITE_WRITE_BUSY_TIMEOUT)
        .with_context(|| {
            format!(
                "failed to configure sqlite busy timeout for {}",
                path.display()
            )
        })?;
    connection
        .execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS local_app_server_feature_enablement (
                feature_name TEXT PRIMARY KEY,
                enabled INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )
        .with_context(|| {
            format!(
                "failed to ensure local_app_server_feature_enablement in {}",
                path.display()
            )
        })?;
    let updated_at = unix_now_millis()?;
    connection
        .execute(
            r#"
            INSERT INTO local_app_server_feature_enablement (feature_name, enabled, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(feature_name) DO UPDATE SET
                enabled = excluded.enabled,
                updated_at = excluded.updated_at
            "#,
            params![CODEX_APP_REMOTE_CONTROL_FEATURE, 1_i64, updated_at],
        )
        .with_context(|| {
            format!(
                "failed to upsert remote_control feature enablement in {}",
                path.display()
            )
        })?;
    Ok(())
}

fn inspect_remote_control_switch_db(path: &Path) -> CodexAppRemoteControlSwitchDatabaseStatus {
    if !path.exists() {
        return CodexAppRemoteControlSwitchDatabaseStatus {
            path: path.to_path_buf(),
            exists: false,
            enabled: None,
            updated_at: None,
            error: None,
        };
    }

    match read_remote_control_switch_db(path) {
        Ok((enabled, updated_at)) => CodexAppRemoteControlSwitchDatabaseStatus {
            path: path.to_path_buf(),
            exists: true,
            enabled,
            updated_at,
            error: None,
        },
        Err(err) => CodexAppRemoteControlSwitchDatabaseStatus {
            path: path.to_path_buf(),
            exists: true,
            enabled: None,
            updated_at: None,
            error: Some(err.to_string()),
        },
    }
}

fn read_remote_control_switch_db(path: &Path) -> Result<(Option<bool>, Option<i64>)> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open Codex App sqlite DB {}", path.display()))?;
    connection
        .busy_timeout(SQLITE_INSPECT_BUSY_TIMEOUT)
        .with_context(|| {
            format!(
                "failed to configure sqlite busy timeout for {}",
                path.display()
            )
        })?;
    let table_exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
            params!["local_app_server_feature_enablement"],
            |_| Ok(()),
        )
        .optional()
        .with_context(|| {
            format!(
                "failed to inspect local_app_server_feature_enablement in {}",
                path.display()
            )
        })?
        .is_some();
    if !table_exists {
        return Ok((None, None));
    }

    let row = connection
        .query_row(
            "SELECT enabled, updated_at FROM local_app_server_feature_enablement WHERE feature_name = ?1",
            params![CODEX_APP_REMOTE_CONTROL_FEATURE],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .with_context(|| {
            format!(
                "failed to read remote_control feature enablement from {}",
                path.display()
            )
        })?;

    Ok(row
        .map(|(enabled, updated_at)| (Some(enabled != 0), Some(updated_at)))
        .unwrap_or((None, None)))
}

pub fn inspect_gui_api_base_url(backend_url: &str) -> CodexAppGuiApiBaseStatus {
    let _ = backend_url;
    let login_issuer_expected = String::new();
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let api_base = match gui_getenv(CODEX_API_BASE_URL_ENV) {
            Ok(value) => value,
            Err(err) => {
                return CodexAppGuiApiBaseStatus {
                    supported: true,
                    configured: false,
                    expected: String::new(),
                    value: None,
                    login_issuer_configured: false,
                    login_issuer_expected,
                    login_issuer_value: None,
                    error: Some(err),
                };
            }
        };
        let login_issuer = match gui_getenv(CODEX_APP_SERVER_LOGIN_ISSUER_ENV) {
            Ok(value) => value,
            Err(err) => {
                return CodexAppGuiApiBaseStatus {
                    supported: true,
                    configured: false,
                    expected: String::new(),
                    value: api_base,
                    login_issuer_configured: false,
                    login_issuer_expected,
                    login_issuer_value: None,
                    error: Some(err),
                };
            }
        };
        CodexAppGuiApiBaseStatus {
            supported: true,
            configured: api_base.is_none(),
            expected: String::new(),
            value: api_base,
            login_issuer_configured: login_issuer.is_none(),
            login_issuer_expected,
            login_issuer_value: login_issuer,
            error: None,
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        CodexAppGuiApiBaseStatus {
            supported: false,
            configured: true,
            expected: String::new(),
            value: None,
            login_issuer_configured: true,
            login_issuer_expected,
            login_issuer_value: None,
            error: None,
        }
    }
}

pub fn cleanup_gui_environment(backend_url: &str) -> CodexAppGuiApiBaseStatus {
    let _ = backend_url;
    #[cfg(target_os = "windows")]
    {
        let cleanup_result =
            gui_unsetenv_many(&[CODEX_API_BASE_URL_ENV, CODEX_APP_SERVER_LOGIN_ISSUER_ENV]);
        let mut status = inspect_gui_api_base_url(backend_url);
        status.error = cleanup_result.err();
        status
    }

    #[cfg(target_os = "macos")]
    {
        let api_result = gui_unsetenv(CODEX_API_BASE_URL_ENV);
        let issuer_result = gui_unsetenv(CODEX_APP_SERVER_LOGIN_ISSUER_ENV);
        let mut status = inspect_gui_api_base_url(backend_url);
        status.error = api_result.err().or_else(|| issuer_result.err());
        status
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        inspect_gui_api_base_url(backend_url)
    }
}

#[cfg(target_os = "macos")]
fn gui_getenv(name: &str) -> Result<Option<String>, String> {
    launchctl_getenv(name)
}

#[cfg(target_os = "macos")]
fn gui_unsetenv(name: &str) -> Result<(), String> {
    launchctl_unsetenv(name)
}

#[cfg(target_os = "windows")]
fn gui_getenv(name: &str) -> Result<Option<String>, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = match hkcu.open_subkey("Environment") {
        Ok(env) => env,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    match env.get_value::<String, _>(name) {
        Ok(value) => Ok((!value.trim().is_empty()).then_some(value)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(target_os = "windows")]
fn gui_unsetenv_many(names: &[&str]) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (env, _) = hkcu
        .create_subkey("Environment")
        .map_err(|err| err.to_string())?;
    for name in names {
        match env.delete_value(name) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.to_string()),
        }
    }
    broadcast_windows_environment_change();
    Ok(())
}

#[cfg(target_os = "windows")]
fn broadcast_windows_environment_change() {
    let message: Vec<u16> = "Environment".encode_utf16().chain(Some(0)).collect();
    let mut result = 0usize;
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            message.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            1000,
            &mut result,
        );
    }
}

#[cfg(target_os = "macos")]
fn launchctl_getenv(name: &str) -> Result<Option<String>, String> {
    match Command::new("/bin/launchctl")
        .arg("getenv")
        .arg(name)
        .output()
    {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok((!value.is_empty()).then_some(value))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                output.status.to_string()
            } else {
                stderr
            })
        }
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(target_os = "macos")]
fn launchctl_unsetenv(name: &str) -> Result<(), String> {
    match Command::new("/bin/launchctl")
        .arg("unsetenv")
        .arg(name)
        .output()
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                output.status.to_string()
            } else {
                stderr
            })
        }
        Err(err) => Err(err.to_string()),
    }
}

fn inspect_config_toml(path: &Path, backend_url: &str) -> (bool, Option<String>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (false, Some("config.toml not found".to_string()));
        }
        Err(err) => return (false, Some(err.to_string())),
    };
    let doc = match raw.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(err) => return (false, Some(err.to_string())),
    };
    let actual = doc
        .get("chatgpt_base_url")
        .and_then(|item| item.as_str())
        .map(str::trim);
    if actual
        .map(|actual| backend_urls_equivalent(actual, backend_url))
        .unwrap_or(false)
    {
        (true, None)
    } else {
        (
            false,
            Some(format!(
                "chatgpt_base_url is {}, expected {backend_url}",
                actual.unwrap_or("<missing>")
            )),
        )
    }
}

fn inspect_managed_ai_gateway_provider(path: &Path, backend_url: &str) -> bool {
    let Ok(doc) = parse_existing_config_toml(path) else {
        return false;
    };
    let Some(active_provider) = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    managed_provider_names_in_config(&doc, backend_url).contains(active_provider)
}

fn inspect_auth_json(path: &Path) -> (bool, Option<String>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (false, Some("auth.json not found".to_string()));
        }
        Err(err) => return (false, Some(err.to_string())),
    };
    let auth = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(auth) => auth,
        Err(err) => return (false, Some(err.to_string())),
    };
    if is_codexhub_auth_json(&auth) {
        (true, None)
    } else {
        (
            false,
            Some("auth.json is not codexhub local auth".to_string()),
        )
    }
}

fn inspect_provider_catalog(
    path: &Path,
) -> (
    Option<CodexAppProviderStatus>,
    Vec<CodexAppProviderStatus>,
    bool,
) {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return (None, Vec::new(), true),
    };
    let doc = match raw.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => doc,
        Err(_) => return (None, Vec::new(), true),
    };
    let image_generation_enabled = doc
        .get("features")
        .and_then(|item| item.as_table())
        .and_then(|features| features.get("image_generation"))
        .and_then(|item| item.as_bool())
        .unwrap_or(true);

    let mut providers = Vec::new();
    if let Some(table) = doc.get("model_providers").and_then(|item| item.as_table()) {
        for (name, item) in table.iter() {
            if let Some(provider) = item.as_table() {
                providers.push(provider_status_from_table(name, Some(provider)));
            }
        }
    }

    let provider = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|name| {
            providers
                .iter()
                .find(|provider| provider.name == name)
                .cloned()
                .unwrap_or_else(|| provider_status_from_table(name, None))
        });

    (provider, providers, image_generation_enabled)
}

fn provider_status_from_table(
    name: &str,
    provider: Option<&toml_edit::Table>,
) -> CodexAppProviderStatus {
    let base_url = provider
        .and_then(|table| table.get("base_url"))
        .and_then(|item| item.as_str())
        .and_then(config_value);
    let key = provider
        .and_then(|table| table.get("experimental_bearer_token"))
        .and_then(|item| item.as_str())
        .and_then(config_value);
    let supports_websockets = provider
        .and_then(|table| table.get("supports_websockets"))
        .and_then(|item| item.as_bool())
        .unwrap_or(false);

    CodexAppProviderStatus {
        name: name.to_string(),
        base_url,
        key,
        supports_websockets,
    }
}

fn config_value(value: &str) -> Option<String> {
    let value = value.trim_matches('\0').trim();
    if value.is_empty() || value.contains("未配") {
        None
    } else {
        Some(value.to_string())
    }
}

fn write_config_toml(path: &Path, options: &ConfigureCodexAppOptions) -> Result<()> {
    let mut doc = if path.exists() {
        parse_existing_config_toml(path)?
    } else {
        toml_edit::DocumentMut::new()
    };
    let codex_home = path.parent().unwrap_or_else(|| Path::new("."));

    let codex_backend_url = options.codex_backend_url();
    doc["chatgpt_base_url"] = toml_edit::value(&codex_backend_url);

    let explicit_provider_name = non_empty(options.provider_name.as_deref());
    let explicit_provider_base_url = options.provider_base_url.as_deref().and_then(config_value);
    let provider_key = options.provider_key.as_deref().and_then(config_value);
    let inject_default_ai_gateway = explicit_provider_name.is_none()
        && explicit_provider_base_url.is_none()
        && provider_key.is_none();
    let default_ai_gateway_base_url =
        inject_default_ai_gateway.then(|| options.ai_gateway_base_url());
    let provider_config_requested = if inject_default_ai_gateway {
        true
    } else {
        explicit_provider_name.is_some()
            || explicit_provider_base_url.is_some()
            || provider_key.is_some()
    };

    if provider_config_requested {
        let provider_name = if inject_default_ai_gateway {
            provider_name(Some(AI_GATEWAY_PROVIDER_NAME))?
        } else {
            provider_name(options.provider_name.as_deref())?
        };

        if options.activate_provider {
            doc["model_provider"] = toml_edit::value(provider_name.as_str());
        }

        let provider = provider_table_mut(&mut doc, provider_name.as_str());
        provider["name"] = toml_edit::value(provider_name.as_str());
        provider["wire_api"] = toml_edit::value("responses");
        provider["requires_openai_auth"] = toml_edit::value(true);

        if inject_default_ai_gateway {
            provider["base_url"] =
                toml_edit::value(default_ai_gateway_base_url.as_deref().unwrap_or_default());
            provider.remove("env_key");
            provider.remove("env_key_instructions");
            provider["experimental_bearer_token"] = toml_edit::value("dummy-token");
            provider.remove("auth");
        } else if let Some(provider_base_url) = explicit_provider_base_url {
            provider["base_url"] = toml_edit::value(provider_base_url);
        }
        if let Some(provider_key) = provider_key {
            provider["experimental_bearer_token"] = toml_edit::value(provider_key);
        }
        if let Some(supports_websockets) = options
            .provider_supports_websockets
            .or(inject_default_ai_gateway.then_some(false))
        {
            provider["supports_websockets"] = toml_edit::value(supports_websockets);
        }
    }

    remove_disabled_plugin_feature_flags(&mut doc);
    remove_legacy_openai_bundled_marketplace(&mut doc);
    upsert_enabled_plugins(&mut doc, REQUIRED_OPENAI_BUNDLED_PLUGIN_IDS);
    if let Some(openai_curated) = find_openai_curated_marketplace_root(&codex_home) {
        upsert_local_marketplace(&mut doc, OPENAI_CURATED_MARKETPLACE_NAME, &openai_curated);
    }
    disable_default_otel_telemetry(&mut doc);

    let raw = normalize_config_toml_order(&doc.to_string());
    backup_existing(path)?;
    std::fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))
}

fn remove_disabled_plugin_feature_flags(doc: &mut toml_edit::DocumentMut) {
    let remove_features =
        if let Some(features) = doc.get_mut("features").and_then(|item| item.as_table_mut()) {
            for key in PLUGIN_BLOCKING_FEATURE_FLAGS {
                if features
                    .get(key)
                    .and_then(|item| item.as_bool())
                    .is_some_and(|enabled| !enabled)
                {
                    features.remove(key);
                }
            }
            features.is_empty()
        } else {
            false
        };
    if remove_features {
        doc.remove("features");
    }
}

fn remove_legacy_openai_bundled_marketplace(doc: &mut toml_edit::DocumentMut) {
    let marketplaces_empty = if let Some(marketplaces) = doc
        .get_mut("marketplaces")
        .and_then(|item| item.as_table_mut())
    {
        marketplaces.remove(OPENAI_BUNDLED_MARKETPLACE_NAME);
        marketplaces.is_empty()
    } else {
        false
    };
    if marketplaces_empty {
        doc.remove("marketplaces");
    }
}

fn is_codexhub_local_marketplace(
    marketplace: Option<&toml_edit::Table>,
    source_marker: &str,
) -> bool {
    marketplace.is_some_and(|marketplace| {
        marketplace
            .get("source_type")
            .and_then(|item| item.as_str())
            .map(str::trim)
            == Some("local")
            && marketplace
                .get("source")
                .and_then(|item| item.as_str())
                .map(normalize_config_path)
                .is_some_and(|source| source.contains(source_marker))
    })
}

fn normalize_config_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn upsert_local_marketplace(
    doc: &mut toml_edit::DocumentMut,
    marketplace_name: &str,
    marketplace_root: &Path,
) {
    let marketplaces = ensure_config_table(doc, "marketplaces");
    let marketplace = ensure_config_table_item(&mut marketplaces[marketplace_name]);
    marketplace.clear();
    marketplace["source_type"] = toml_edit::value("local");
    marketplace["source"] = toml_edit::value(marketplace_root.to_string_lossy().to_string());
    marketplace["last_updated"] = toml_edit::value(rfc3339_now());
}

fn upsert_enabled_plugins(doc: &mut toml_edit::DocumentMut, plugin_ids: &[&str]) {
    let plugins = ensure_config_table(doc, "plugins");
    for plugin_id in plugin_ids {
        let plugin = ensure_config_table_item(&mut plugins[*plugin_id]);
        plugin["enabled"] = toml_edit::value(true);
    }
}

// The Codex app-server ships an OpenTelemetry stack that, by default, pushes
// metrics to `https://ab.chatgpt.com/otlp/v1/metrics` (the `Statsig` metrics
// exporter). On networks that cannot reach that host (e.g. mainland China
// without a VPN), each export attempt blocks on a ~10s connect timeout and
// retries, which makes Codex startup and restart feel extremely slow.
//
// The metrics push is what actually hangs: in Codex's `otel_init`, the metrics
// exporter defaults to `Statsig` and is gated by `analytics.enabled`, while the
// log/trace exporters already default to `None`. codexhub runs the app fully
// against a local backend, so none of this telemetry is useful here. We turn
// the analytics gate off and pin every exporter to `none`, but only fill in
// fields the user has not set so any deliberate override is preserved.
fn disable_default_otel_telemetry(doc: &mut toml_edit::DocumentMut) {
    let analytics = ensure_config_table(doc, "analytics");
    if !analytics.contains_key("enabled") {
        analytics["enabled"] = toml_edit::value(false);
    }

    let otel = ensure_config_table(doc, "otel");
    for key in ["exporter", "trace_exporter", "metrics_exporter"] {
        if !otel.contains_key(key) {
            otel[key] = toml_edit::value("none");
        }
    }
}

fn ensure_config_table<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    key: &str,
) -> &'a mut toml_edit::Table {
    let root = doc.as_table_mut();
    if !root.contains_key(key) || !root.get(key).is_some_and(|item| item.is_table()) {
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        root.insert(key, toml_edit::Item::Table(table));
    }
    root.get_mut(key)
        .and_then(|item| item.as_table_mut())
        .expect("config table should exist")
}

fn ensure_config_table_item(item: &mut toml_edit::Item) -> &mut toml_edit::Table {
    match item {
        toml_edit::Item::Table(_) => {}
        toml_edit::Item::Value(value) => {
            let table = value.as_inline_table().map_or_else(
                || {
                    let mut table = toml_edit::Table::new();
                    table.set_implicit(true);
                    table
                },
                |inline| {
                    let mut table = toml_edit::Table::new();
                    table.set_implicit(true);
                    for (key, value) in inline.iter() {
                        table.insert(key, toml_edit::Item::Value(value.clone()));
                    }
                    table
                },
            );
            *item = toml_edit::Item::Table(table);
        }
        toml_edit::Item::None => {
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            *item = toml_edit::Item::Table(table);
        }
        _ => {
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            *item = toml_edit::Item::Table(table);
        }
    }
    item.as_table_mut().expect("config item should be a table")
}

fn find_openai_curated_marketplace_root(codex_home: &Path) -> Option<PathBuf> {
    let curated = codex_home.join(".tmp").join("plugins");
    openai_curated_marketplace_exists(&curated).then_some(curated)
}

fn openai_curated_marketplace_exists(path: &Path) -> bool {
    path.join(".agents")
        .join("plugins")
        .join("marketplace.json")
        .is_file()
}

fn normalize_config_toml_order(raw: &str) -> String {
    const PRIORITY_KEYS: &[&str] = &["chatgpt_base_url", "model_provider", "model"];

    let mut root_lines = Vec::new();
    let mut table_lines = Vec::new();
    let mut in_tables = false;
    for line in raw.lines() {
        if line.trim_start().starts_with('[') {
            in_tables = true;
        }
        if in_tables {
            table_lines.push(line.to_string());
        } else {
            root_lines.push(line.to_string());
        }
    }

    let mut prioritized = Vec::new();
    let mut remaining = root_lines;
    for key in PRIORITY_KEYS {
        if let Some(index) = remaining
            .iter()
            .position(|line| assignment_key(line.trim()) == Some(*key))
        {
            prioritized.push(remaining.remove(index));
        }
    }

    let mut output = Vec::new();
    output.extend(prioritized);
    for line in remaining {
        if line.trim().is_empty()
            && output
                .last()
                .is_none_or(|prev: &String| prev.trim().is_empty())
        {
            continue;
        }
        output.push(line);
    }
    if !output.is_empty() && !table_lines.is_empty() {
        output.push(String::new());
    }
    output.extend(table_lines);
    format!("{}\n", output.join("\n").trim_end())
}

fn uninstall_config_toml(path: &Path, backend_url: &str) -> Result<(bool, bool)> {
    cleanup_codexhub_config(path, backend_url, true)
}

fn managed_provider_names_in_config(
    doc: &toml_edit::DocumentMut,
    backend_url: &str,
) -> HashSet<String> {
    let mut names = HashSet::new();
    let ai_gateway_base_url = ai_gateway_base_url_from_backend_url(backend_url);
    let Some(providers) = doc.get("model_providers").and_then(|item| item.as_table()) else {
        return names;
    };

    for provider_name in [AI_GATEWAY_PROVIDER_NAME, DEFAULT_PROVIDER_NAME] {
        let Some(provider) = providers
            .get(provider_name)
            .and_then(|item| item.as_table())
        else {
            continue;
        };
        let local_gateway_provider = provider
            .get("base_url")
            .and_then(|item| item.as_str())
            .map(str::trim)
            .map(|value| backend_urls_equivalent(value, &ai_gateway_base_url))
            .unwrap_or(false);
        if local_gateway_provider && provider_table_has_codexhub_shape(provider, provider_name) {
            names.insert(provider_name.to_string());
        }
    }

    names
}

fn ai_gateway_base_url_from_backend_url(backend_url: &str) -> String {
    let backend = backend_url.trim_end_matches('/');
    if let Some(base) = backend.strip_suffix("/backend-api") {
        format!("{base}/ai-gateway/v1")
    } else {
        format!("{backend}/ai-gateway/v1")
    }
}

fn codex_connection_url(url: &str, mode: LocalConnectionMode) -> String {
    if mode == LocalConnectionMode::Standard {
        return url.to_string();
    }

    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    let Some(host) = parsed.host_str() else {
        return url.to_string();
    };
    if matches!(host, "127.0.0.1" | "::1" | "localhost") {
        if parsed.set_host(Some("localhost")).is_ok() {
            return parsed.to_string();
        }
    }
    url.to_string()
}

fn inspect_connection_mode(path: &Path, backend_url: &str) -> LocalConnectionMode {
    let Ok(doc) = parse_existing_config_toml(path) else {
        return LocalConnectionMode::Standard;
    };
    let Some(value) = doc
        .get("chatgpt_base_url")
        .and_then(|item| item.as_str())
        .map(str::trim)
    else {
        return LocalConnectionMode::Standard;
    };
    let Ok(actual_url) = url::Url::parse(value) else {
        return LocalConnectionMode::Standard;
    };
    let Ok(expected_url) = url::Url::parse(&codex_connection_url(
        backend_url,
        LocalConnectionMode::VpnCompatible,
    )) else {
        return LocalConnectionMode::Standard;
    };
    if actual_url.host_str() == Some("localhost")
        && actual_url.scheme() == expected_url.scheme()
        && actual_url.path().trim_end_matches('/') == expected_url.path().trim_end_matches('/')
        && actual_url.query() == expected_url.query()
        && actual_url.port_or_known_default() == expected_url.port_or_known_default()
    {
        LocalConnectionMode::VpnCompatible
    } else {
        LocalConnectionMode::Standard
    }
}

fn provider_table_has_codexhub_shape(provider: &toml_edit::Table, provider_name: &str) -> bool {
    let name_matches = provider
        .get("name")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .is_none_or(|name| name == provider_name);
    let responses_wire_api = provider
        .get("wire_api")
        .and_then(|item| item.as_str())
        .map(str::trim)
        == Some("responses");
    let requires_openai_auth = provider
        .get("requires_openai_auth")
        .and_then(|item| item.as_bool())
        == Some(true);
    name_matches && responses_wire_api && requires_openai_auth
}

fn backend_urls_equivalent(actual: &str, expected: &str) -> bool {
    if actual == expected {
        return true;
    }

    let Ok(actual_url) = url::Url::parse(actual) else {
        return false;
    };
    let Ok(expected_url) = url::Url::parse(expected) else {
        return false;
    };

    if actual_url.scheme() != expected_url.scheme()
        || actual_url.path().trim_end_matches('/') != expected_url.path().trim_end_matches('/')
        || actual_url.query() != expected_url.query()
    {
        return false;
    }

    let actual_host = actual_url.host_str().unwrap_or_default();
    let expected_host = expected_url.host_str().unwrap_or_default();
    let loopback_hosts = |host: &str| matches!(host, "127.0.0.1" | "localhost" | "::1");
    let hosts_match = actual_host == expected_host
        || loopback_hosts(actual_host) && loopback_hosts(expected_host);
    hosts_match && actual_url.port_or_known_default() == expected_url.port_or_known_default()
}

fn remove_provider_table(doc: &mut toml_edit::DocumentMut, provider_name: &str) {
    let providers_empty = if let Some(providers) = doc
        .get_mut("model_providers")
        .and_then(|item| item.as_table_mut())
    {
        providers.remove(provider_name);
        providers.is_empty()
    } else {
        false
    };
    if providers_empty {
        doc.remove("model_providers");
    }
}

fn delete_provider_from_config_toml(path: &Path, requested_provider_name: &str) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("config.toml not found"));
    }
    let provider_name = provider_name(Some(requested_provider_name))?;
    let mut doc = parse_existing_config_toml(path)?;
    let active_provider = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .is_some_and(|active| active == provider_name);
    if active_provider {
        doc.remove("model_provider");
    }
    remove_provider_table(&mut doc, &provider_name);
    backup_existing(path)?;
    std::fs::write(path, normalize_config_toml_order(&doc.to_string()))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn uninstall_auth_json(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let auth = serde_json::from_str::<serde_json::Value>(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if !is_codexhub_auth_json(&auth) {
        return Ok(false);
    }

    backup_existing(path)?;
    std::fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))?;
    Ok(true)
}

fn parse_existing_config_toml(path: &Path) -> Result<toml_edit::DocumentMut> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    match raw.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => Ok(doc),
        Err(err) => {
            let repaired = dedupe_duplicate_key_lines(&raw);
            if repaired == raw {
                return Err(err).with_context(|| format!("failed to parse {}", path.display()));
            }
            repaired.parse::<toml_edit::DocumentMut>().with_context(|| {
                format!(
                    "failed to parse {} after duplicate-key repair; original error: {err}",
                    path.display()
                )
            })
        }
    }
}

fn dedupe_duplicate_key_lines(raw: &str) -> String {
    let mut seen = HashMap::<String, HashSet<String>>::new();
    let mut current_table = String::new();
    let mut repaired = String::with_capacity(raw.len());

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_table = trimmed.to_string();
            repaired.push_str(line);
            repaired.push('\n');
            continue;
        }

        if let Some(key) = assignment_key(trimmed) {
            let table_seen = seen.entry(current_table.clone()).or_default();
            if !table_seen.insert(key.to_string()) {
                continue;
            }
        }

        repaired.push_str(line);
        repaired.push('\n');
    }

    repaired.trim_end_matches('\n').to_string()
}

fn assignment_key(trimmed: &str) -> Option<&str> {
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || key.starts_with('[') || key.contains(char::is_whitespace) {
        None
    } else {
        Some(key)
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn provider_table_mut<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    provider_name: &str,
) -> &'a mut toml_edit::Table {
    if doc
        .get("model_providers")
        .is_none_or(|item| !item.is_table())
    {
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        doc["model_providers"] = toml_edit::Item::Table(table);
    }
    let providers = doc["model_providers"]
        .as_table_mut()
        .expect("model_providers should be a table");
    if providers.is_empty() {
        providers.set_implicit(true);
    }
    if providers
        .get(provider_name)
        .is_none_or(|item| !item.is_table())
    {
        providers.insert(
            provider_name,
            toml_edit::Item::Table(toml_edit::Table::new()),
        );
    }
    providers
        .get_mut(provider_name)
        .expect("provider table should exist")
        .as_table_mut()
        .expect("provider should be a table")
}

fn provider_name(value: Option<&str>) -> Result<String> {
    let provider_name = non_empty(value).unwrap_or(DEFAULT_PROVIDER_NAME);
    if provider_name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        Ok(provider_name.to_string())
    } else {
        Err(anyhow!(
            "provider name `{provider_name}` can only contain ASCII letters, numbers, `_`, and `-`"
        ))
    }
}

#[derive(Debug, Clone)]
struct LocalAuthIdentity {
    account_id: String,
    user_id: String,
    email: String,
    plan_type: String,
    account_is_fedramp: bool,
}

impl LocalAuthIdentity {
    fn from_options(options: &ConfigureCodexAppOptions) -> Self {
        Self {
            account_id: options.account_id.clone(),
            user_id: options.user_id.clone(),
            email: options.email.clone(),
            plan_type: options.plan_type.clone(),
            account_is_fedramp: false,
        }
    }
}

fn write_auth_json(path: &Path, options: &ConfigureCodexAppOptions) -> Result<()> {
    let identity = derive_local_auth_identity(path, options);
    let jwt = local_chatgpt_jwt(&identity)?;
    let auth = json!({
        "auth_mode": LOCAL_AUTH_MODE,
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": jwt,
            "access_token": jwt,
            "refresh_token": "",
            "account_id": identity.account_id,
        },
        "last_refresh": rfc3339_now(),
    });
    let raw = serde_json::to_string_pretty(&auth)?;
    backup_existing(path)?;
    std::fs::write(path, format!("{raw}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn derive_local_auth_identity(
    _path: &Path,
    options: &ConfigureCodexAppOptions,
) -> LocalAuthIdentity {
    LocalAuthIdentity::from_options(options)
}

fn is_codexhub_auth_json(auth: &serde_json::Value) -> bool {
    let auth_mode = auth.get("auth_mode").and_then(|value| value.as_str());
    let api_key = auth.get("OPENAI_API_KEY").and_then(|value| value.as_str());
    if auth_mode.is_none() && api_key == Some(LEGACY_BAD_LOCAL_AUTH_API_KEY) {
        return true;
    }

    if !matches!(
        auth_mode,
        Some(LOCAL_AUTH_MODE) | Some(LEGACY_LOCAL_AUTH_MODE)
    ) {
        return false;
    }

    auth.pointer("/tokens/access_token")
        .and_then(|value| value.as_str())
        .or_else(|| {
            auth.pointer("/tokens/id_token")
                .and_then(|value| value.as_str())
        })
        .is_some_and(is_codexhub_local_jwt)
}

fn is_codexhub_local_jwt(token: &str) -> bool {
    let Some(payload) = decode_jwt_payload_value(token) else {
        return false;
    };
    let local_subject = payload
        .get("sub")
        .and_then(|value| value.as_str())
        .is_some_and(|value| value.starts_with("local|"));
    let local_auth = payload
        .get("https://api.openai.com/auth")
        .and_then(|value| value.get("localhost"))
        .and_then(|value| value.as_bool())
        == Some(true);
    local_subject && local_auth
}

fn decode_jwt_payload_value(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice::<serde_json::Value>(&payload).ok()
}

#[derive(Debug, Clone)]
struct ManagedBackupPaths {
    dir: PathBuf,
    manifest_path: PathBuf,
    auth_path: PathBuf,
}

fn ensure_managed_backup(codex_home: &Path, config_path: &Path, auth_path: &Path) -> Result<()> {
    let backup = managed_backup_paths(codex_home);
    if backup.manifest_path.exists() {
        chain_log::write_line(format!(
            "[codex_app_config] event=managed_backup_exists path={}",
            backup.manifest_path.display()
        ));
        return Ok(());
    }

    std::fs::create_dir_all(&backup.dir).with_context(|| {
        format!(
            "failed to create managed backup dir {}",
            backup.dir.display()
        )
    })?;
    let manifest = ManagedCodexAppBackupManifest {
        version: MANAGED_BACKUP_VERSION,
        created_at_ms: unix_now_millis()?,
        codex_home: codex_home.to_path_buf(),
        config_existed: config_path.exists(),
        auth_existed: auth_path.exists(),
    };
    if manifest.auth_existed {
        std::fs::copy(auth_path, &backup.auth_path).with_context(|| {
            format!(
                "failed to backup existing {} to {}",
                auth_path.display(),
                backup.auth_path.display()
            )
        })?;
    }

    let raw = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(&backup.manifest_path, format!("{raw}\n")).with_context(|| {
        format!(
            "failed to write managed backup manifest {}",
            backup.manifest_path.display()
        )
    })?;
    chain_log::write_line(format!(
        "[codex_app_config] event=managed_backup_created path={} config_existed={} auth_existed={}",
        backup.manifest_path.display(),
        manifest.config_existed,
        manifest.auth_existed
    ));
    Ok(())
}

fn uninstall_with_managed_state(
    backup: &ManagedBackupPaths,
    config_path: &Path,
    auth_path: &Path,
    backend_url: &str,
) -> Result<(bool, bool, bool)> {
    let manifest = read_managed_backup_manifest(&backup.manifest_path)?;
    let (removed_chatgpt_base_url, removed_model_provider) =
        cleanup_codexhub_config(config_path, backend_url, manifest.config_existed)?;
    let removed_auth = restore_or_remove_managed_file(
        auth_path,
        &backup.auth_path,
        manifest.auth_existed,
        Some(is_codexhub_auth_file),
    )?;
    chain_log::write_line(format!(
        "[codex_app_config] event=managed_backup_restored path={}",
        backup.manifest_path.display()
    ));
    if let Err(err) = std::fs::remove_dir_all(&backup.dir) {
        chain_log::write_line(format!(
            "[codex_app_config] event=managed_backup_cleanup_failed path={} error={}",
            backup.dir.display(),
            err
        ));
    }
    Ok((
        removed_chatgpt_base_url,
        removed_model_provider,
        removed_auth,
    ))
}

fn cleanup_codexhub_config(
    path: &Path,
    backend_url: &str,
    config_existed_before_first_write: bool,
) -> Result<(bool, bool)> {
    if !path.exists() {
        return Ok((false, false));
    }
    let mut doc = parse_existing_config_toml(path)?;
    let managed_provider_names = managed_provider_names_in_config(&doc, backend_url);

    let removed_chatgpt_base_url = doc
        .get("chatgpt_base_url")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .map(|value| backend_urls_equivalent(value, backend_url))
        .unwrap_or(false);
    if removed_chatgpt_base_url {
        doc.remove("chatgpt_base_url");
    }

    let removed_model_provider = doc
        .get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .is_some_and(|active| managed_provider_names.contains(active));
    if removed_model_provider {
        doc.remove("model_provider");
    }
    for provider_name in managed_provider_names {
        remove_provider_table(&mut doc, &provider_name);
    }
    if !config_existed_before_first_write {
        remove_created_feature_defaults(&mut doc);
        remove_created_plugin_defaults(&mut doc);
        remove_created_local_marketplaces(&mut doc);
    }

    if doc.iter().next().is_none() {
        std::fs::remove_file(path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
    } else {
        backup_existing(path)?;
        std::fs::write(path, normalize_config_toml_order(&doc.to_string()))
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok((removed_chatgpt_base_url, removed_model_provider))
}

fn remove_created_feature_defaults(doc: &mut toml_edit::DocumentMut) {
    let features_empty =
        if let Some(features) = doc.get_mut("features").and_then(|item| item.as_table_mut()) {
            if features
                .get("apps")
                .and_then(|item| item.as_bool())
                .is_some_and(|enabled| !enabled)
            {
                features.remove("apps");
            }
            features.remove("image_generation");
            features.is_empty()
        } else {
            false
        };
    if features_empty {
        doc.remove("features");
    }
}

fn remove_created_plugin_defaults(doc: &mut toml_edit::DocumentMut) {
    let plugins_empty =
        if let Some(plugins) = doc.get_mut("plugins").and_then(|item| item.as_table_mut()) {
            for plugin_id in REQUIRED_OPENAI_BUNDLED_PLUGIN_IDS {
                let remove_plugin = plugins
                    .get(*plugin_id)
                    .and_then(|item| item.as_table())
                    .is_some_and(is_created_plugin_default);
                if remove_plugin {
                    plugins.remove(*plugin_id);
                }
            }
            plugins.is_empty()
        } else {
            false
        };
    if plugins_empty {
        doc.remove("plugins");
    }
}

fn is_created_plugin_default(plugin: &toml_edit::Table) -> bool {
    plugin.len() == 1 && plugin.get("enabled").and_then(|item| item.as_bool()) == Some(true)
}

fn remove_created_local_marketplaces(doc: &mut toml_edit::DocumentMut) {
    let marketplaces_empty = if let Some(marketplaces) = doc
        .get_mut("marketplaces")
        .and_then(|item| item.as_table_mut())
    {
        for (name, source_marker) in [
            (OPENAI_BUNDLED_MARKETPLACE_NAME, "openai-bundled"),
            (OPENAI_CURATED_MARKETPLACE_NAME, ".tmp/plugins"),
        ] {
            let remove_marketplace = marketplaces
                .get(name)
                .and_then(|item| item.as_table())
                .is_some_and(|marketplace| {
                    is_codexhub_local_marketplace(Some(marketplace), source_marker)
                });
            if remove_marketplace {
                marketplaces.remove(name);
            }
        }
        marketplaces.is_empty()
    } else {
        false
    };
    if marketplaces_empty {
        doc.remove("marketplaces");
    }
}

fn restore_or_remove_managed_file(
    target_path: &Path,
    backup_path: &Path,
    originally_existed: bool,
    remove_guard: Option<fn(&Path) -> Result<bool>>,
) -> Result<bool> {
    if originally_existed {
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::copy(backup_path, target_path).with_context(|| {
            format!(
                "failed to restore managed backup {} to {}",
                backup_path.display(),
                target_path.display()
            )
        })?;
        return Ok(true);
    }

    if !target_path.exists() {
        return Ok(false);
    }
    if let Some(remove_guard) = remove_guard {
        if !remove_guard(target_path)? {
            return Ok(false);
        }
    }
    std::fs::remove_file(target_path)
        .with_context(|| format!("failed to remove {}", target_path.display()))?;
    Ok(true)
}

fn read_managed_backup_manifest(path: &Path) -> Result<ManagedCodexAppBackupManifest> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read managed backup manifest {}", path.display()))?;
    let manifest: ManagedCodexAppBackupManifest = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse managed backup manifest {}", path.display()))?;
    if manifest.version != MANAGED_BACKUP_VERSION {
        return Err(anyhow!(
            "unsupported managed backup manifest version {} in {}",
            manifest.version,
            path.display()
        ));
    }
    Ok(manifest)
}

fn managed_backup_paths(codex_home: &Path) -> ManagedBackupPaths {
    let dir = codexhub_app_support_dir()
        .join("backups")
        .join("codex-app")
        .join(codex_home_backup_id(codex_home));
    ManagedBackupPaths {
        manifest_path: dir.join(MANAGED_BACKUP_MANIFEST),
        auth_path: dir.join(MANAGED_BACKUP_AUTH),
        dir,
    }
}

fn codex_home_backup_id(codex_home: &Path) -> String {
    let normalized = codex_home
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    hex::encode(&digest[..16])
}

fn codexhub_app_support_dir() -> PathBuf {
    if let Some(base) = std::env::var_os(CODEXHUB_HOME_ENV).map(PathBuf::from) {
        return base;
    }
    platform_codexhub_app_support_dir()
}

#[cfg(test)]
fn platform_codexhub_app_support_dir() -> PathBuf {
    std::env::temp_dir().join("codexhub-managed-backups-tests")
}

#[cfg(all(target_os = "windows", not(test)))]
fn platform_codexhub_app_support_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CodexHub")
}

#[cfg(all(not(target_os = "windows"), not(test)))]
fn platform_codexhub_app_support_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/CodexHub"))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn is_codexhub_auth_file(path: &Path) -> Result<bool> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let auth = serde_json::from_str::<serde_json::Value>(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(is_codexhub_auth_json(&auth))
}

fn backup_existing(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("invalid backup path {}", path.display()))?
        .to_string_lossy();
    let backup = path.with_file_name(format!("{file_name}.bak"));
    std::fs::copy(path, &backup).with_context(|| {
        format!(
            "failed to backup existing {} to {}",
            path.display(),
            backup.display()
        )
    })?;
    Ok(())
}

fn local_chatgpt_jwt(identity: &LocalAuthIdentity) -> Result<String> {
    let now = unix_now()?;
    let exp = now + 10 * 365 * 24 * 60 * 60;
    let payload = json!({
        "iss": "https://auth.openai.com",
        "aud": ["https://api.openai.com/v1"],
        "iat": now,
        "nbf": now,
        "exp": exp,
        "sub": format!("local|{}", identity.user_id),
        "email": identity.email,
        "email_verified": true,
        "https://api.openai.com/profile": {
            "email": identity.email,
            "email_verified": true,
        },
        "https://api.openai.com/auth": {
            "chatgpt_account_id": identity.account_id,
            "account_id": identity.account_id,
            "chatgpt_account_user_id": format!("{}__{}", identity.user_id, identity.account_id),
            "account_user_id": format!("{}__{}", identity.user_id, identity.account_id),
            "chatgpt_plan_type": identity.plan_type,
            "chatgpt_user_id": identity.user_id,
            "user_id": identity.user_id,
            "chatgpt_account_is_fedramp": identity.account_is_fedramp,
            "localhost": true,
            "groups": [],
            "organizations": [{
                "id": identity.account_id,
                "is_default": true,
                "role": "owner",
                "title": "CodexHub Local",
            }]
        },
        "scp": [
            "openid",
            "profile",
            "email",
            "offline_access",
            "api.connectors.read",
            "api.connectors.invoke"
        ],
    });

    Ok(format!(
        "{}.{}.{}",
        b64url_json(&json!({ "alg": "none", "typ": "JWT" }))?,
        b64url_json(&payload)?,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
    ))
}

fn b64url_json(value: &serde_json::Value) -> Result<String> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(value)?))
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow!("system time is before UNIX epoch: {err}"))?
        .as_secs())
}

fn unix_now_millis() -> Result<i64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| anyhow!("system time is before UNIX epoch: {err}"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| anyhow!("system time is too large for sqlite timestamp"))
}

fn rfc3339_now() -> String {
    match unix_now() {
        Ok(now) => format_rfc3339_utc(now),
        Err(_) => "1970-01-01T00:00:00Z".to_string(),
    }
}

fn format_rfc3339_utc(timestamp: u64) -> String {
    // Valid for normal contemporary timestamps. This avoids adding a time crate
    // just to stamp the local auth file.
    let days = (timestamp / 86_400) as i64;
    let seconds_of_day = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as i32, m as u32, d as u32)
}

pub(crate) fn default_codex_home() -> PathBuf {
    // This helper configures the separately launched Codex App, not the
    // CODEX_HOME of the process that happens to run codexhub.
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".codex"))
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

pub(crate) fn clear_codex_models_cache(codex_home: Option<PathBuf>) -> Result<bool> {
    let cache_path = codex_home
        .unwrap_or_else(default_codex_home)
        .join(CODEX_MODELS_CACHE_FILE);
    match std::fs::remove_file(&cache_path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err).with_context(|| format!("failed to remove {}", cache_path.display())),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct LegacyBundledPluginStateCleanup {
    removed_remote_identity_files: usize,
    removed_remote_catalog_files: usize,
}

fn clear_legacy_codexhub_bundled_plugin_state(
    codex_home: &Path,
) -> Result<LegacyBundledPluginStateCleanup> {
    Ok(LegacyBundledPluginStateCleanup {
        removed_remote_identity_files: clear_legacy_codexhub_bundled_remote_identity_files(
            codex_home,
        )?,
        removed_remote_catalog_files: clear_legacy_codexhub_bundled_remote_catalog_files(
            codex_home,
        )?,
    })
}

fn clear_legacy_codexhub_bundled_remote_identity_files(codex_home: &Path) -> Result<usize> {
    let bundled_cache_root = codex_home
        .join("plugins")
        .join("cache")
        .join(OPENAI_BUNDLED_MARKETPLACE_NAME);
    let Ok(plugin_dirs) = std::fs::read_dir(&bundled_cache_root) else {
        return Ok(0);
    };

    let mut removed = 0;
    for plugin_dir in plugin_dirs.flatten() {
        let plugin_path = plugin_dir.path();
        if !plugin_path.is_dir() {
            continue;
        }
        let metadata_path = plugin_path.join(REMOTE_PLUGIN_INSTALL_METADATA_FILE);
        let Ok(contents) = std::fs::read_to_string(&metadata_path) else {
            continue;
        };
        if !contents.contains(CODEXHUB_BUNDLED_REMOTE_ID_PREFIX) {
            continue;
        }
        std::fs::remove_file(&metadata_path)
            .with_context(|| format!("failed to remove {}", metadata_path.display()))?;
        removed += 1;
    }
    Ok(removed)
}

fn clear_legacy_codexhub_bundled_remote_catalog_files(codex_home: &Path) -> Result<usize> {
    let catalog_cache_root = codex_home.join("cache").join("remote_plugin_catalog");
    let Ok(entries) = std::fs::read_dir(&catalog_cache_root) else {
        return Ok(0);
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !contents.contains(CODEXHUB_BUNDLED_REMOTE_ID_PREFIX) {
            continue;
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn configure_codex_app_writes_provider_and_local_auth() {
        let codex_home = unique_temp_dir();
        let report = configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::Standard,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: None,
            provider_base_url: Some("https://api.example.invalid".to_string()),
            provider_key: Some("test-provider-key".to_string()),
            activate_provider: true,
            image_generation_enabled: None,
            provider_supports_websockets: Some(true),
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(report.config_path).expect("read config");
        assert!(config.starts_with("chatgpt_base_url = \"http://127.0.0.1:3847/backend-api\"\n"));
        assert!(config.contains("chatgpt_base_url = \"http://127.0.0.1:3847/backend-api\""));
        assert!(config.contains("model_provider = \"ai-codex\""));
        assert!(!config.contains("model = \"gpt-5.5\""));
        assert!(!config.contains("review_model"));
        assert!(!config.contains("model_reasoning_effort"));
        assert!(!config.contains("disable_response_storage"));
        assert!(!config.contains("network_access"));
        assert!(!config.contains("windows_wsl_setup_acknowledged"));
        assert!(!config.contains("[features]"));
        assert!(!config.contains("apps = false"));
        assert!(!config.contains("image_generation = false"));
        assert!(config.contains("[model_providers.ai-codex]"));
        assert!(config.contains("base_url = \"https://api.example.invalid\""));
        assert!(config.contains("wire_api = \"responses\""));
        assert!(config.contains("requires_openai_auth = true"));
        assert!(config.contains("supports_websockets = true"));
        assert!(config.contains("experimental_bearer_token = \"test-provider-key\""));

        let auth = std::fs::read_to_string(report.auth_path).expect("read auth");
        assert!(auth.contains(&format!("\"auth_mode\": \"{LOCAL_AUTH_MODE}\"")));
        assert!(auth.contains("\"OPENAI_API_KEY\": null"));
        assert!(auth.contains("\"account_id\": \"acct_test\""));
        assert!(report.remote_control_switch.configured);

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_defaults_to_ai_gateway_provider() {
        let codex_home = unique_temp_dir();
        let report = configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::Standard,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: None,
            provider_base_url: None,
            provider_key: None,
            activate_provider: true,
            image_generation_enabled: None,
            provider_supports_websockets: None,
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(report.config_path).expect("read config");
        assert!(config.contains("chatgpt_base_url = \"http://127.0.0.1:3847/backend-api\""));
        assert!(config.contains("model_provider = \"ai-gateway\""));
        assert!(config.contains("[model_providers.ai-gateway]"));
        assert!(config.contains("name = \"ai-gateway\""));
        assert!(config.contains("base_url = \"http://127.0.0.1:3847/ai-gateway/v1\""));
        assert!(config.contains("wire_api = \"responses\""));
        assert!(config.contains("requires_openai_auth = true"));
        assert!(config.contains("supports_websockets = false"));
        assert!(config.contains("experimental_bearer_token = \"dummy-token\""));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn inspect_managed_ai_gateway_provider_requires_active_gateway_provider() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
"#,
        )
        .expect("write partial config");

        assert!(!inspect_managed_ai_gateway_provider(
            &config_path,
            "http://127.0.0.1:3847/backend-api",
        ));

        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "ai-gateway"

[model_providers.ai-gateway]
name = "ai-gateway"
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "dummy-token"
"#,
        )
        .expect("write gateway config");

        assert!(inspect_managed_ai_gateway_provider(
            &config_path,
            "http://127.0.0.1:3847/backend-api",
        ));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_compatible_connection_uses_localhost() {
        let codex_home = unique_temp_dir();
        configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::VpnCompatible,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "test@example.com".to_string(),
            plan_type: "pro".to_string(),
            provider_name: None,
            provider_base_url: None,
            provider_key: None,
            activate_provider: true,
            image_generation_enabled: None,
            provider_supports_websockets: Some(false),
        })
        .expect("configure");

        let config_path = codex_home.join("config.toml");
        let config = std::fs::read_to_string(&config_path).expect("read config");
        assert!(config.contains("chatgpt_base_url = \"http://localhost:3847/backend-api\""));
        assert!(config.contains("base_url = \"http://localhost:3847/ai-gateway/v1\""));

        let status = inspect_codex_app_config(
            Some(codex_home.clone()),
            "http://127.0.0.1:3847/backend-api",
        );
        assert_eq!(status.connection_mode, LocalConnectionMode::VpnCompatible);
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_replaces_existing_chatgpt_auth_with_local_tokens() {
        let codex_home = unique_temp_dir();
        let auth_path = codex_home.join("auth.json");
        std::fs::write(
            &auth_path,
            official_chatgpt_auth_json(
                "acct_official",
                "user_official",
                "official@example.test",
                "plus",
                true,
            ),
        )
        .expect("write existing auth");

        let report =
            configure_codex_app(test_configure_options(codex_home.clone())).expect("configure");
        let auth = std::fs::read_to_string(report.auth_path).expect("read auth");
        let auth = serde_json::from_str::<serde_json::Value>(&auth).expect("parse auth");
        assert_eq!(
            auth.get("auth_mode").and_then(|value| value.as_str()),
            Some(LOCAL_AUTH_MODE)
        );
        assert!(
            auth.get("OPENAI_API_KEY")
                .is_some_and(serde_json::Value::is_null)
        );
        assert_eq!(
            auth.pointer("/tokens/account_id")
                .and_then(|value| value.as_str()),
            Some("acct_test")
        );
        let payload = auth
            .pointer("/tokens/id_token")
            .and_then(|value| value.as_str())
            .and_then(decode_jwt_payload_value)
            .expect("local jwt payload");
        let claim_auth = payload
            .get("https://api.openai.com/auth")
            .expect("auth claims");
        assert_eq!(
            claim_auth
                .get("chatgpt_account_id")
                .and_then(|value| value.as_str()),
            Some("acct_test")
        );
        assert_eq!(
            claim_auth
                .get("chatgpt_user_id")
                .and_then(|value| value.as_str()),
            Some("user_test")
        );
        assert_eq!(
            claim_auth
                .get("chatgpt_plan_type")
                .and_then(|value| value.as_str()),
            Some("pro")
        );
        assert_eq!(
            claim_auth
                .get("chatgpt_account_is_fedramp")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
        assert_eq!(
            payload.get("email").and_then(|value| value.as_str()),
            Some("local@example.test")
        );

        let _ = std::fs::remove_dir_all(managed_backup_paths(&codex_home).dir);
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_replaces_existing_api_key_auth_with_local_tokens() {
        let codex_home = unique_temp_dir();
        let auth_path = codex_home.join("auth.json");
        std::fs::write(
            &auth_path,
            r#"{
  "auth_mode": "apiKey",
  "OPENAI_API_KEY": "sk-test"
}
"#,
        )
        .expect("write api key auth");

        let report =
            configure_codex_app(test_configure_options(codex_home.clone())).expect("configure");
        let auth = std::fs::read_to_string(report.auth_path).expect("read auth");
        let auth = serde_json::from_str::<serde_json::Value>(&auth).expect("parse auth");
        assert_eq!(
            auth.get("auth_mode").and_then(|value| value.as_str()),
            Some(LOCAL_AUTH_MODE)
        );
        assert!(
            auth.get("OPENAI_API_KEY")
                .is_some_and(serde_json::Value::is_null)
        );
        assert_eq!(
            auth.pointer("/tokens/account_id")
                .and_then(|value| value.as_str()),
            Some("acct_test")
        );
        let payload = auth
            .pointer("/tokens/id_token")
            .and_then(|value| value.as_str())
            .and_then(decode_jwt_payload_value)
            .expect("local jwt payload");
        assert_eq!(
            payload.get("email").and_then(|value| value.as_str()),
            Some("local@example.test")
        );

        let _ = std::fs::remove_dir_all(managed_backup_paths(&codex_home).dir);
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_default_ai_gateway_repairs_existing_provider_base_url() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        std::fs::write(
            &config_path,
            r#"model_provider = "custom-provider"
model = "custom-model"

[model_providers.ai-gateway]
name = "ai-gateway"
base_url = "https://old.example.invalid/v1"
env_key = "AI_GATEWAY_API_KEY"
experimental_bearer_token = "old-token"

[model_providers.ai-gateway.auth]
command = "print-token"
"#,
        )
        .expect("write config");

        configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::Standard,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: None,
            provider_base_url: None,
            provider_key: None,
            activate_provider: true,
            image_generation_enabled: None,
            provider_supports_websockets: None,
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(config_path).expect("read config");
        assert!(config.contains("model_provider = \"ai-gateway\""));
        assert!(config.contains("model = \"custom-model\""));
        assert!(config.contains("base_url = \"http://127.0.0.1:3847/ai-gateway/v1\""));
        assert!(config.contains("requires_openai_auth = true"));
        assert!(!config.contains("env_key"));
        assert!(config.contains("experimental_bearer_token = \"dummy-token\""));
        assert!(!config.contains("[model_providers.ai-gateway.auth]"));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_default_ai_gateway_completes_matching_existing_provider() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        std::fs::write(
            &config_path,
            r#"model_provider = "custom-provider"
model = "custom-model"

[model_providers.ai-gateway]
name = "ai-gateway"
base_url = "http://localhost:3847/ai-gateway/v1"
env_key = "AI_GATEWAY_API_KEY"
"#,
        )
        .expect("write config");

        configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::Standard,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: None,
            provider_base_url: None,
            provider_key: None,
            activate_provider: true,
            image_generation_enabled: None,
            provider_supports_websockets: None,
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(config_path).expect("read config");
        assert!(config.contains("model_provider = \"ai-gateway\""));
        assert!(config.contains("model = \"custom-model\""));
        assert!(config.contains("base_url = \"http://127.0.0.1:3847/ai-gateway/v1\""));
        assert!(config.contains("requires_openai_auth = true"));
        assert!(!config.contains("env_key"));
        assert!(config.contains("experimental_bearer_token = \"dummy-token\""));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_preserves_explicit_apps_feature() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"[features]
apps = true
image_generation = true

[model_providers.old-provider]
name = "old-provider"
"#,
        )
        .expect("write config");

        let report = configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::Standard,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: None,
            provider_base_url: None,
            provider_key: None,
            activate_provider: true,
            image_generation_enabled: Some(true),
            provider_supports_websockets: None,
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(report.config_path).expect("read config");
        assert!(config.contains("[features]"));
        assert!(config.contains("apps = true"));
        assert!(config.contains("image_generation = true"));
        assert!(!config.contains("apps = false"));
        assert!(!config.contains("image_generation = false"));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_removes_plugin_blocking_feature_flags() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"[features]
apps = false
plugins = false
computer_use = false
browser_use = false
in_app_browser = false
image_generation = true
keep_me = false
"#,
        )
        .expect("write config");

        configure_codex_app(test_configure_options(codex_home.clone())).expect("configure");

        let config = std::fs::read_to_string(config_path).expect("read config");
        assert!(!config.contains("apps = false"));
        assert!(!config.contains("plugins = false"));
        assert!(!config.contains("computer_use = false"));
        assert!(!config.contains("browser_use = false"));
        assert!(!config.contains("in_app_browser = false"));
        assert!(config.contains("image_generation = true"));
        assert!(config.contains("keep_me = false"));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_registers_local_curated_marketplace_and_removes_legacy_bundled() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        let curated_marketplace_root = codex_home.join(".tmp").join("plugins");
        std::fs::create_dir_all(curated_marketplace_root.join(".agents").join("plugins"))
            .expect("create curated marketplace");
        std::fs::write(
            curated_marketplace_root
                .join(".agents")
                .join("plugins")
                .join("marketplace.json"),
            r#"{"name":"openai-curated","plugins":[]}"#,
        )
        .expect("write curated marketplace");
        std::fs::write(
            &config_path,
            r#"[marketplaces.openai-bundled]
source_type = "local"
source = 'C:\Users\test\.codex\.tmp\bundled-marketplaces\openai-bundled'
"#,
        )
        .expect("write config");

        configure_codex_app(test_configure_options(codex_home.clone())).expect("configure");

        let config = std::fs::read_to_string(config_path).expect("read config");
        let doc = config
            .parse::<toml_edit::DocumentMut>()
            .expect("parse config");
        let marketplaces = doc
            .get("marketplaces")
            .and_then(|item| item.as_table())
            .expect("marketplaces");
        assert!(marketplaces.get(OPENAI_BUNDLED_MARKETPLACE_NAME).is_none());
        let curated_marketplace = marketplaces
            .get(OPENAI_CURATED_MARKETPLACE_NAME)
            .and_then(|item| item.as_table())
            .expect("openai-curated marketplace");
        assert_eq!(
            curated_marketplace
                .get("source_type")
                .and_then(|item| item.as_str()),
            Some("local")
        );
        assert_eq!(
            curated_marketplace
                .get("source")
                .and_then(|item| item.as_str()),
            Some(curated_marketplace_root.to_string_lossy().as_ref())
        );

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_enables_required_bundled_plugins() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"[plugins."computer-use@openai-bundled"]
enabled = false

[plugins."latex@openai-bundled"]
enabled = false
"#,
        )
        .expect("write config");

        configure_codex_app(test_configure_options(codex_home.clone())).expect("configure");

        let config = std::fs::read_to_string(config_path).expect("read config");
        let doc = config
            .parse::<toml_edit::DocumentMut>()
            .expect("parse config");
        for plugin_id in REQUIRED_OPENAI_BUNDLED_PLUGIN_IDS {
            assert_eq!(
                doc.get("plugins")
                    .and_then(|item| item.as_table())
                    .and_then(|plugins| plugins.get(*plugin_id))
                    .and_then(|item| item.as_table())
                    .and_then(|plugin| plugin.get("enabled"))
                    .and_then(|item| item.as_bool()),
                Some(true),
                "{plugin_id} should be enabled"
            );
        }
        assert_eq!(
            doc.get("plugins")
                .and_then(|item| item.as_table())
                .and_then(|plugins| plugins.get("latex@openai-bundled"))
                .and_then(|item| item.as_table())
                .and_then(|plugin| plugin.get("enabled"))
                .and_then(|item| item.as_bool()),
            Some(false)
        );

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_disables_default_otel_telemetry() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");

        configure_codex_app(test_configure_options(codex_home.clone())).expect("configure");

        let config = std::fs::read_to_string(&config_path).expect("read config");
        let doc = config
            .parse::<toml_edit::DocumentMut>()
            .expect("parse config");
        let otel = doc
            .get("otel")
            .and_then(|item| item.as_table())
            .expect("otel table present");
        // The metrics exporter is the one that pushes to ab.chatgpt.com and
        // hangs without a VPN; all three must be pinned to none.
        for key in ["exporter", "trace_exporter", "metrics_exporter"] {
            assert_eq!(
                otel.get(key).and_then(|item| item.as_str()),
                Some("none"),
                "otel {key} should default to none to avoid blocking on ab.chatgpt.com"
            );
        }
        assert_eq!(
            doc.get("analytics")
                .and_then(|item| item.as_table())
                .and_then(|analytics| analytics.get("enabled"))
                .and_then(|item| item.as_bool()),
            Some(false),
            "analytics gate should be disabled so metrics are never exported"
        );

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_preserves_explicit_otel_exporter() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"[otel]
exporter = "otlp-http"
environment = "prod"
"#,
        )
        .expect("write config");

        configure_codex_app(test_configure_options(codex_home.clone())).expect("configure");

        let config = std::fs::read_to_string(&config_path).expect("read config");
        let doc = config
            .parse::<toml_edit::DocumentMut>()
            .expect("parse config");
        let otel = doc
            .get("otel")
            .and_then(|item| item.as_table())
            .expect("otel table present");
        assert_eq!(
            otel.get("exporter").and_then(|item| item.as_str()),
            Some("otlp-http"),
            "an explicit user exporter must be preserved"
        );
        assert_eq!(
            otel.get("environment").and_then(|item| item.as_str()),
            Some("prod"),
            "other user otel settings must be preserved"
        );
        // Fields the user did not set are still filled with safe defaults.
        assert_eq!(
            otel.get("metrics_exporter").and_then(|item| item.as_str()),
            Some("none"),
            "metrics exporter should still be pinned to none"
        );

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn set_provider_websocket_does_not_change_image_generation_feature() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"[features]
image_generation = true

[model_providers.qwen]
name = "qwen"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
"#,
        )
        .expect("write config");

        set_codex_app_provider_websocket(Some(codex_home.clone()), "qwen", true)
            .expect("set websocket");

        let config = std::fs::read_to_string(config_path).expect("read config");
        assert!(config.contains("image_generation = true"));
        assert!(!config.contains("image_generation = false"));
        assert!(config.contains("supports_websockets = true"));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_switches_existing_provider_by_name_only() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "old-provider"
model = "gpt-5.5"

[model_providers.old-provider]
name = "old-provider"
base_url = "https://old.example.invalid"

[model_providers.qwen]
name = "qwen"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
experimental_bearer_token = "existing-qwen-key"
"#,
        )
        .expect("write config");

        let report = configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::Standard,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: Some("qwen".to_string()),
            provider_base_url: None,
            provider_key: None,
            activate_provider: true,
            image_generation_enabled: None,
            provider_supports_websockets: None,
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(report.config_path).expect("read config");
        assert!(config.contains("model_provider = \"qwen\""));
        assert!(config.contains("[model_providers.qwen]"));
        assert!(
            config.contains("base_url = \"https://dashscope.aliyuncs.com/compatible-mode/v1\"")
        );
        assert!(config.contains("experimental_bearer_token = \"existing-qwen-key\""));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_can_save_provider_without_activating() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "old-provider"
model = "gpt-5.5"

[model_providers.old-provider]
name = "old-provider"
base_url = "https://old.example.invalid"
"#,
        )
        .expect("write config");

        let report = configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::Standard,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: Some("qwen".to_string()),
            provider_base_url: Some(
                "https://dashscope.aliyuncs.com/compatible-mode/v1".to_string(),
            ),
            provider_key: Some("existing-qwen-key".to_string()),
            activate_provider: false,
            image_generation_enabled: None,
            provider_supports_websockets: Some(false),
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(report.config_path).expect("read config");
        assert!(config.contains("model_provider = \"old-provider\""));
        assert!(config.contains("[model_providers.qwen]"));
        assert!(
            config.contains("base_url = \"https://dashscope.aliyuncs.com/compatible-mode/v1\"")
        );
        assert!(config.contains("experimental_bearer_token = \"existing-qwen-key\""));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn delete_codex_app_provider_removes_provider_and_active_selection() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "qwen"
model = "gpt-5.5"

[model_providers.qwen]
name = "qwen"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
"#,
        )
        .expect("write config");

        delete_codex_app_provider(Some(codex_home.clone()), "qwen").expect("delete provider");

        let config = std::fs::read_to_string(config_path).expect("read config");
        assert!(!config.contains("model_provider = \"qwen\""));
        assert!(!config.contains("[model_providers.qwen]"));
        assert!(config.contains("model = \"gpt-5.5\""));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn enable_remote_control_switch_updates_existing_disabled_row() {
        let codex_home = unique_temp_dir();
        let db_path = codex_home
            .join(CODEX_APP_SQLITE_DIR)
            .join(CODEX_APP_PRIMARY_DB);
        std::fs::create_dir_all(db_path.parent().expect("db parent")).expect("create sqlite dir");
        let connection = Connection::open(&db_path).expect("open sqlite db");
        connection
            .execute_batch(
                r#"
                CREATE TABLE local_app_server_feature_enablement (
                    feature_name TEXT PRIMARY KEY,
                    enabled INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                "#,
            )
            .expect("create feature table");
        connection
            .execute(
                "INSERT INTO local_app_server_feature_enablement (feature_name, enabled, updated_at) VALUES (?1, ?2, ?3)",
                params![CODEX_APP_REMOTE_CONTROL_FEATURE, 0_i64, 1_i64],
            )
            .expect("insert disabled row");
        drop(connection);

        let status = enable_codex_app_remote_control_switch(Some(codex_home.clone()))
            .expect("enable remote_control switch");
        assert!(status.configured);
        let db_status = status
            .databases
            .iter()
            .find(|db| db.path == db_path)
            .expect("codex db status");
        assert_eq!(db_status.enabled, Some(true));
        assert!(db_status.updated_at.unwrap_or_default() > 1);

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn rejects_invalid_provider_name() {
        let err = provider_name(Some("bad.provider")).expect_err("invalid provider name");
        assert!(err.to_string().contains("can only contain ASCII"));
    }

    #[test]
    fn configure_codex_app_repairs_duplicate_provider_key() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"model_provider = "ai-codex"

[model_providers.ai-codex]
name = "ai-codex"
wire_api = "responses"
requires_openai_auth = true
base_url = "https://old.example"
requires_openai_auth = true
"#,
        )
        .expect("write invalid config");

        configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::Standard,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: None,
            provider_base_url: Some("https://api.example.invalid".to_string()),
            provider_key: Some("test-provider-key".to_string()),
            activate_provider: true,
            image_generation_enabled: None,
            provider_supports_websockets: None,
        })
        .expect("configure codex app");

        let config = std::fs::read_to_string(config_path).expect("read config");
        assert_eq!(config.matches("requires_openai_auth = true").count(), 1);
        assert!(config.contains("base_url = \"https://api.example.invalid\""));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn inspect_codex_app_config_lists_existing_providers() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "llmx"

[model_providers.llmx]
name = "llmx"
base_url = "https://llmx.example"
experimental_bearer_token = "llmx-key"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#,
        )
        .expect("write config");

        let status = inspect_codex_app_config(
            Some(codex_home.clone()),
            "http://127.0.0.1:3847/backend-api",
        );

        let active = status.provider.expect("active provider");
        assert_eq!(active.name, "llmx");
        assert_eq!(active.base_url.as_deref(), Some("https://llmx.example"));
        assert_eq!(status.providers.len(), 2);
        assert!(
            status
                .providers
                .iter()
                .any(|provider| provider.name == "llmx")
        );
        assert!(
            status
                .providers
                .iter()
                .any(|provider| provider.name == "openai")
        );

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn inspect_provider_catalog_ignores_placeholder_values() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "ai-code"

[model_providers.ai-code]
name = "ai-code"
base_url = "未配置，写入时新�"
experimental_bearer_token = "****未配�"
"#,
        )
        .expect("write config");

        let status = inspect_codex_app_config(
            Some(codex_home.clone()),
            "http://127.0.0.1:3847/backend-api",
        );

        let active = status.provider.expect("active provider");
        assert_eq!(active.name, "ai-code");
        assert_eq!(active.base_url, None);
        assert_eq!(active.key, None);

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_creates_managed_state_and_auth_backup() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        let auth_path = codex_home.join("auth.json");
        let original_config = "model = \"gpt-5.5\"\n";
        let original_auth = "{\n  \"auth_mode\": \"chatgpt\"\n}\n";
        std::fs::write(&config_path, original_config).expect("write original config");
        std::fs::write(&auth_path, original_auth).expect("write original auth");

        configure_codex_app(test_configure_options(codex_home.clone()))
            .expect("configure codex app");

        let backup = managed_backup_paths(&codex_home);
        let manifest = read_managed_backup_manifest(&backup.manifest_path).expect("manifest");
        assert_eq!(manifest.version, MANAGED_BACKUP_VERSION);
        assert_eq!(manifest.codex_home, codex_home);
        assert!(manifest.config_existed);
        assert!(manifest.auth_existed);
        assert!(!backup.dir.join("config.toml").exists());
        assert_eq!(
            std::fs::read_to_string(&backup.auth_path).expect("read backup auth"),
            original_auth
        );

        let _ = std::fs::remove_dir_all(managed_backup_paths(&codex_home).dir);
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn configure_codex_app_does_not_overwrite_first_auth_backup() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        let auth_path = codex_home.join("auth.json");
        let original_auth = "{\n  \"auth_mode\": \"first\"\n}\n";
        std::fs::write(&config_path, "model = \"first\"\n").expect("write original config");
        std::fs::write(&auth_path, original_auth).expect("write original auth");

        configure_codex_app(test_configure_options(codex_home.clone())).expect("first configure");
        std::fs::write(&config_path, "model = \"second\"\n").expect("mutate config");
        std::fs::write(&auth_path, "{\n  \"auth_mode\": \"second\"\n}\n").expect("mutate auth");
        configure_codex_app(test_configure_options(codex_home.clone())).expect("second configure");

        let backup = managed_backup_paths(&codex_home);
        assert!(!backup.dir.join("config.toml").exists());
        assert_eq!(
            std::fs::read_to_string(&backup.auth_path).expect("read backup auth"),
            original_auth
        );

        let _ = std::fs::remove_dir_all(backup.dir);
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn uninstall_codex_app_cleans_config_without_reverting_codex_app_writes() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        let auth_path = codex_home.join("auth.json");
        let original_config = r#"model_provider = "openai"
model = "gpt-5.5"

[features]
image_generation = true

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#;
        let original_auth = "{\n  \"auth_mode\": \"chatgpt\"\n}\n";
        std::fs::write(&config_path, original_config).expect("write original config");
        std::fs::write(&auth_path, original_auth).expect("write original auth");
        configure_codex_app(test_configure_options(codex_home.clone()))
            .expect("configure codex app");
        let codex_app_updated_config = r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "ai-gateway"
model = "codex-app-later-model"

[features]
image_generation = true
new_codex_app_flag = true

[model_providers.ai-gateway]
name = "ai-gateway"
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "dummy-token"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#;
        std::fs::write(&config_path, codex_app_updated_config).expect("simulate codex app write");

        let report = uninstall_codex_app(
            Some(codex_home.clone()),
            "http://127.0.0.1:3847/backend-api",
        )
        .expect("uninstall config");

        assert!(report.removed_chatgpt_base_url);
        assert!(report.removed_model_provider);
        assert!(report.removed_auth);
        let config = std::fs::read_to_string(&config_path).expect("read cleaned config");
        assert!(!config.contains("chatgpt_base_url"));
        assert!(!config.contains("model_provider = \"ai-gateway\""));
        assert!(!config.contains("[model_providers.ai-gateway]"));
        assert!(config.contains("model = \"codex-app-later-model\""));
        assert!(config.contains("new_codex_app_flag = true"));
        assert!(config.contains("[model_providers.openai]"));
        assert_eq!(
            std::fs::read_to_string(&auth_path).expect("read restored auth"),
            original_auth
        );

        let _ = std::fs::remove_dir_all(managed_backup_paths(&codex_home).dir);
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn uninstall_codex_app_cleans_files_absent_before_first_write() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        let auth_path = codex_home.join("auth.json");

        configure_codex_app(test_configure_options(codex_home.clone()))
            .expect("configure codex app");
        let report = uninstall_codex_app(
            Some(codex_home.clone()),
            "http://127.0.0.1:3847/backend-api",
        )
        .expect("uninstall config");

        assert!(report.removed_chatgpt_base_url);
        assert!(report.removed_model_provider);
        assert!(report.removed_auth);
        assert!(!config_path.exists());
        assert!(!auth_path.exists());

        let _ = std::fs::remove_dir_all(managed_backup_paths(&codex_home).dir);
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn uninstall_codex_app_without_backup_falls_back_to_safe_cleanup() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        let auth_path = codex_home.join("auth.json");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "ai-gateway"
model = "gpt-5.5"

[model_providers.ai-gateway]
name = "ai-gateway"
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "dummy-token"

[model_providers.keep]
name = "keep"
base_url = "https://api.example.invalid"
"#,
        )
        .expect("write config");
        write_auth_json(&auth_path, &test_configure_options(codex_home.clone()))
            .expect("write auth");

        let report = uninstall_codex_app(
            Some(codex_home.clone()),
            "http://127.0.0.1:3847/backend-api",
        )
        .expect("uninstall config");

        assert!(report.removed_chatgpt_base_url);
        assert!(report.removed_model_provider);
        assert!(report.removed_auth);

        let config = std::fs::read_to_string(&config_path).expect("read config");
        assert!(!config.contains("chatgpt_base_url"));
        assert!(!config.contains("model_provider = \"ai-gateway\""));
        assert!(!config.contains("[model_providers.ai-gateway]"));
        assert!(config.contains("model = \"gpt-5.5\""));
        assert!(config.contains("[model_providers.keep]"));
        assert!(config.contains("base_url = \"https://api.example.invalid\""));
        assert!(!auth_path.exists());

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn uninstall_auth_json_preserves_non_local_chatgpt_auth() {
        let codex_home = unique_temp_dir();
        let auth_path = codex_home.join("auth.json");
        std::fs::write(
            &auth_path,
            r#"{
  "auth_mode": "chatgpt",
  "tokens": {
    "access_token": "official-token",
    "account_id": "acct_test"
  }
}
"#,
        )
        .expect("write auth");

        let removed_auth = uninstall_auth_json(&auth_path).expect("uninstall auth");

        assert!(!removed_auth);
        assert!(auth_path.exists());

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn uninstall_codex_app_without_backup_preserves_user_active_provider() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "keep"

[model_providers.keep]
name = "keep"
base_url = "https://api.example.invalid"
"#,
        )
        .expect("write config");

        let report = uninstall_codex_app(
            Some(codex_home.clone()),
            "http://127.0.0.1:3847/backend-api",
        )
        .expect("uninstall config");

        assert!(report.removed_chatgpt_base_url);
        assert!(!report.removed_model_provider);
        let config = std::fs::read_to_string(&config_path).expect("read config");
        assert!(!config.contains("chatgpt_base_url"));
        assert!(config.contains("model_provider = \"keep\""));
        assert!(config.contains("[model_providers.keep]"));

        let _ = std::fs::remove_dir_all(codex_home);
    }

    fn test_configure_options(codex_home: PathBuf) -> ConfigureCodexAppOptions {
        ConfigureCodexAppOptions {
            codex_home: Some(codex_home),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
            connection_mode: LocalConnectionMode::Standard,
            account_id: "acct_test".to_string(),
            user_id: "user_test".to_string(),
            email: "local@example.test".to_string(),
            plan_type: "pro".to_string(),
            provider_name: None,
            provider_base_url: None,
            provider_key: None,
            activate_provider: true,
            image_generation_enabled: None,
            provider_supports_websockets: None,
        }
    }

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "codexhub-test-{}-{}-{}",
            std::process::id(),
            nanos,
            sequence
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn official_chatgpt_auth_json(
        account_id: &str,
        user_id: &str,
        email: &str,
        plan_type: &str,
        fedramp: bool,
    ) -> String {
        let jwt = test_jwt(&json!({
            "sub": user_id,
            "email": email,
            "https://api.openai.com/profile": {
                "email": email,
            },
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "account_id": account_id,
                "chatgpt_user_id": user_id,
                "user_id": user_id,
                "chatgpt_plan_type": plan_type,
                "chatgpt_account_is_fedramp": fedramp,
            }
        }));
        serde_json::to_string_pretty(&json!({
            "auth_mode": LEGACY_LOCAL_AUTH_MODE,
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": jwt,
                "access_token": jwt,
                "refresh_token": "refresh",
                "account_id": account_id,
            },
            "last_refresh": "2026-01-01T00:00:00Z",
        }))
        .expect("serialize auth")
    }

    fn test_jwt(payload: &serde_json::Value) -> String {
        format!(
            "{}.{}.{}",
            b64url_json(&json!({ "alg": "none", "typ": "JWT" })).expect("jwt header"),
            b64url_json(payload).expect("jwt payload"),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
        )
    }

    #[test]
    fn default_codex_home_ignores_process_codex_home() {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .expect("HOME or USERPROFILE should exist for this test");
        let expected = PathBuf::from(home).join(".codex");

        let old_codex_home = std::env::var_os("CODEX_HOME");
        unsafe {
            std::env::set_var("CODEX_HOME", "/tmp/not-the-codex-app-home");
        }
        let actual = default_codex_home();
        match old_codex_home {
            Some(value) => unsafe {
                std::env::set_var("CODEX_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("CODEX_HOME");
            },
        }

        assert_eq!(actual, expected);
    }

    #[test]
    fn clear_codex_models_cache_removes_cache_file() {
        let codex_home = unique_temp_dir();
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        let cache_path = codex_home.join(CODEX_MODELS_CACHE_FILE);
        std::fs::write(&cache_path, "{}").expect("write cache");

        let removed = clear_codex_models_cache(Some(codex_home.clone())).expect("clear cache");

        assert!(removed);
        assert!(!cache_path.exists());
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn clear_codex_models_cache_ignores_missing_cache_file() {
        let codex_home = unique_temp_dir();
        std::fs::create_dir_all(&codex_home).expect("create codex home");

        let removed = clear_codex_models_cache(Some(codex_home.clone())).expect("clear cache");

        assert!(!removed);
        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn clear_legacy_codexhub_bundled_plugin_state_removes_only_old_remote_identity() {
        let codex_home = unique_temp_dir();
        let computer_use_root = codex_home
            .join("plugins")
            .join("cache")
            .join(OPENAI_BUNDLED_MARKETPLACE_NAME)
            .join("computer-use");
        let browser_root = codex_home
            .join("plugins")
            .join("cache")
            .join(OPENAI_BUNDLED_MARKETPLACE_NAME)
            .join("browser");
        std::fs::create_dir_all(computer_use_root.join("26.623.42026"))
            .expect("create computer-use cache");
        std::fs::create_dir_all(browser_root.join("26.623.42026")).expect("create browser cache");
        std::fs::write(
            computer_use_root.join(REMOTE_PLUGIN_INSTALL_METADATA_FILE),
            r#"{"schema_version":1,"remote_plugin_id":"plugins~codexhub-bundled-computer-use"}"#,
        )
        .expect("write legacy identity");
        std::fs::write(
            browser_root.join(REMOTE_PLUGIN_INSTALL_METADATA_FILE),
            r#"{"schema_version":1,"remote_plugin_id":"plugins~Plugin_browser"}"#,
        )
        .expect("write non-codexhub identity");

        let result =
            clear_legacy_codexhub_bundled_plugin_state(&codex_home).expect("clear legacy state");

        assert_eq!(result.removed_remote_identity_files, 1);
        assert!(
            !computer_use_root
                .join(REMOTE_PLUGIN_INSTALL_METADATA_FILE)
                .exists()
        );
        assert!(computer_use_root.join("26.623.42026").is_dir());
        assert!(
            browser_root
                .join(REMOTE_PLUGIN_INSTALL_METADATA_FILE)
                .is_file()
        );

        let _ = std::fs::remove_dir_all(codex_home);
    }

    #[test]
    fn clear_legacy_codexhub_bundled_plugin_state_removes_old_catalog_cache_only() {
        let codex_home = unique_temp_dir();
        let catalog_root = codex_home.join("cache").join("remote_plugin_catalog");
        std::fs::create_dir_all(&catalog_root).expect("create catalog cache");
        let legacy_catalog = catalog_root.join("legacy.json");
        let curated_catalog = catalog_root.join("curated.json");
        std::fs::write(
            &legacy_catalog,
            r#"{"plugins":[{"id":"plugins~codexhub-bundled-browser"}]}"#,
        )
        .expect("write legacy catalog");
        std::fs::write(
            &curated_catalog,
            r#"{"plugins":[{"id":"plugins~codexhub-local-github"}]}"#,
        )
        .expect("write curated catalog");

        let result =
            clear_legacy_codexhub_bundled_plugin_state(&codex_home).expect("clear legacy state");

        assert_eq!(result.removed_remote_catalog_files, 1);
        assert!(!legacy_catalog.exists());
        assert!(curated_catalog.is_file());

        let _ = std::fs::remove_dir_all(codex_home);
    }
}

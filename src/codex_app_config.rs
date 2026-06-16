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
use serde::Serialize;
use serde_json::json;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
#[cfg(target_os = "windows")]
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

use crate::chain_log;

const DEFAULT_PROVIDER_NAME: &str = "ai-codex";
const AI_GATEWAY_PROVIDER_NAME: &str = "ai-gateway";
const CODEX_API_BASE_URL_ENV: &str = "CODEX_API_BASE_URL";
const CODEX_APP_SERVER_LOGIN_ISSUER_ENV: &str = "CODEX_APP_SERVER_LOGIN_ISSUER";
const CODEX_APP_SQLITE_DIR: &str = "sqlite";
const CODEX_APP_PRIMARY_DB: &str = "codex.db";
const CODEX_APP_DEV_DB: &str = "codex-dev.db";
const CODEX_APP_REMOTE_CONTROL_FEATURE: &str = "remote_control";
const SQLITE_WRITE_BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const SQLITE_INSPECT_BUSY_TIMEOUT: Duration = Duration::from_millis(150);

const LOCAL_AUTH_MODE: &str = "chatgptAuthTokens";
const LEGACY_LOCAL_AUTH_MODE: &str = "chatgpt";

#[derive(Debug, Clone)]
pub struct ConfigureCodexAppOptions {
    pub codex_home: Option<PathBuf>,
    pub backend_url: String,
    pub account_id: String,
    pub user_id: String,
    pub email: String,
    pub plan_type: String,
    pub provider_name: Option<String>,
    pub provider_base_url: Option<String>,
    pub provider_key: Option<String>,
    pub activate_provider: bool,
    pub image_generation_enabled: Option<bool>,
    pub provider_supports_websockets: Option<bool>,
}

impl ConfigureCodexAppOptions {
    fn ai_gateway_base_url(&self) -> String {
        let backend = self.backend_url.trim_end_matches('/');
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
    pub config_error: Option<String>,
    pub auth_error: Option<String>,
    pub gui_api_base: CodexAppGuiApiBaseStatus,
    pub remote_control_switch: CodexAppRemoteControlSwitchStatus,
    pub provider: Option<CodexAppProviderStatus>,
    pub providers: Vec<CodexAppProviderStatus>,
    pub image_generation_enabled: bool,
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
    chain_log::write_line(format!(
        "[codex_app_config] event=write_config_start path={}",
        config_path.display()
    ));
    write_config_toml(&config_path, &options)?;
    chain_log::write_line(format!(
        "[codex_app_config] event=write_config_done path={}",
        config_path.display()
    ));

    let auth_path = codex_home.join("auth.json");
    chain_log::write_line(format!(
        "[codex_app_config] event=write_auth_start path={}",
        auth_path.display()
    ));
    write_auth_json(&auth_path, &options)?;
    chain_log::write_line(format!(
        "[codex_app_config] event=write_auth_done path={}",
        auth_path.display()
    ));

    chain_log::write_line("[codex_app_config] event=remote_control_switch_start");
    let remote_control_switch = enable_remote_control_switch_in_home(&codex_home)?;
    chain_log::write_line(format!(
        "[codex_app_config] event=remote_control_switch_done configured={}",
        remote_control_switch.configured
    ));

    #[cfg(not(test))]
    chain_log::write_line("[codex_app_config] event=gui_environment_start");
    #[cfg(not(test))]
    let _ = configure_gui_environment(&options.backend_url);
    #[cfg(not(test))]
    chain_log::write_line("[codex_app_config] event=gui_environment_done");

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

    let (removed_chatgpt_base_url, removed_model_provider) =
        uninstall_config_toml(&config_path, backend_url)?;
    let removed_auth = false;
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

    let gui_api_base = inspect_gui_api_base_url(backend_url);
    let gui_ok = gui_api_base.configured && gui_api_base.login_issuer_configured;
    let remote_control_switch = inspect_remote_control_switch_in_home(&codex_home);
    let remote_control_ok = remote_control_switch.configured;

    CodexAppConfigStatus {
        codex_home,
        config_path,
        auth_path,
        configured: config_ok && auth_ok && gui_ok && remote_control_ok,
        config_ok,
        auth_ok,
        config_error,
        auth_error,
        gui_api_base,
        remote_control_switch,
        provider,
        providers,
        image_generation_enabled,
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
    let login_issuer_expected = oauth_issuer_url(backend_url);
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let api_base = match gui_getenv(CODEX_API_BASE_URL_ENV) {
            Ok(value) => value,
            Err(err) => {
                return CodexAppGuiApiBaseStatus {
                    supported: true,
                    configured: false,
                    expected: backend_url.to_string(),
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
                    expected: backend_url.to_string(),
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
            configured: api_base
                .as_deref()
                .map(|value| backend_urls_equivalent(value, backend_url))
                .unwrap_or(false),
            expected: backend_url.to_string(),
            value: api_base,
            login_issuer_configured: login_issuer
                .as_deref()
                .map(|value| backend_urls_equivalent(value, &login_issuer_expected))
                .unwrap_or(false),
            login_issuer_expected,
            login_issuer_value: login_issuer,
            error: None,
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        CodexAppGuiApiBaseStatus {
            supported: false,
            configured: false,
            expected: backend_url.to_string(),
            value: None,
            login_issuer_configured: false,
            login_issuer_expected,
            login_issuer_value: None,
            error: Some(
                "CODEX_API_BASE_URL one-click setup is only implemented for macOS and Windows"
                    .to_string(),
            ),
        }
    }
}

pub fn configure_gui_environment(backend_url: &str) -> CodexAppGuiApiBaseStatus {
    #[cfg(target_os = "windows")]
    {
        let login_issuer = oauth_issuer_url(backend_url);
        let env_result = gui_setenv_many(&[
            (CODEX_API_BASE_URL_ENV, backend_url),
            (CODEX_APP_SERVER_LOGIN_ISSUER_ENV, &login_issuer),
        ]);
        let mut status = inspect_gui_api_base_url(backend_url);
        status.error = env_result.err();
        status
    }

    #[cfg(target_os = "macos")]
    {
        let login_issuer = oauth_issuer_url(backend_url);
        let api_result = gui_setenv(CODEX_API_BASE_URL_ENV, backend_url);
        let issuer_result = gui_setenv(CODEX_APP_SERVER_LOGIN_ISSUER_ENV, &login_issuer);
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
fn gui_setenv(name: &str, value: &str) -> Result<(), String> {
    launchctl_setenv(name, value)
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
fn gui_setenv_many(values: &[(&str, &str)]) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (env, _) = hkcu
        .create_subkey("Environment")
        .map_err(|err| err.to_string())?;
    for (name, value) in values {
        env.set_value(name, value).map_err(|err| err.to_string())?;
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
fn launchctl_setenv(name: &str, value: &str) -> Result<(), String> {
    match Command::new("/bin/launchctl")
        .arg("setenv")
        .arg(name)
        .arg(value)
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

pub fn oauth_issuer_url(backend_url: &str) -> String {
    backend_url
        .trim_end_matches('/')
        .strip_suffix("/backend-api")
        .unwrap_or_else(|| backend_url.trim_end_matches('/'))
        .to_string()
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
    if is_codex_remote_auth_json(&auth) {
        (true, None)
    } else {
        (
            false,
            Some("auth.json is not codex-remote local auth".to_string()),
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

    doc["chatgpt_base_url"] = toml_edit::value(&options.backend_url);
    disable_codex_apps_feature_if_unset(&mut doc);
    set_hosted_image_generation_feature(&mut doc, options.image_generation_enabled);

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

    write_bundled_plugin_marketplace(&mut doc);

    let raw = normalize_config_toml_order(&doc.to_string());
    backup_existing(path)?;
    std::fs::write(path, raw).with_context(|| format!("failed to write {}", path.display()))
}

fn disable_codex_apps_feature_if_unset(doc: &mut toml_edit::DocumentMut) {
    if !doc.contains_key("features") {
        doc["features"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let Some(features) = doc["features"].as_table_mut() else {
        return;
    };
    if features.get("apps").is_none() && features.get("connectors").is_none() {
        features["apps"] = toml_edit::value(false);
    }
}

fn set_hosted_image_generation_feature(doc: &mut toml_edit::DocumentMut, enabled: Option<bool>) {
    if !doc.contains_key("features") {
        doc["features"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    let Some(features) = doc["features"].as_table_mut() else {
        return;
    };
    match enabled {
        Some(enabled) => features["image_generation"] = toml_edit::value(enabled),
        None if features.get("image_generation").is_none() => {
            features["image_generation"] = toml_edit::value(false);
        }
        None => {}
    }
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
    if !path.exists() {
        return Ok((false, false));
    }
    let mut doc = parse_existing_config_toml(path)?;

    let removed_chatgpt_base_url = doc
        .get("chatgpt_base_url")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .map(|value| backend_urls_equivalent(value, backend_url))
        .unwrap_or(false);
    if removed_chatgpt_base_url {
        doc.remove("chatgpt_base_url");
    }

    let removed_model_provider = doc.get("model_provider").is_some();
    if removed_model_provider {
        doc.remove("model_provider");
    }

    if removed_chatgpt_base_url || removed_model_provider {
        backup_existing(path)?;
        std::fs::write(path, normalize_config_toml_order(&doc.to_string()))
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok((removed_chatgpt_base_url, removed_model_provider))
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

#[cfg(test)]
fn uninstall_auth_json(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let auth = serde_json::from_str::<serde_json::Value>(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if !is_codex_remote_auth_json(&auth) {
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

fn write_bundled_plugin_marketplace(doc: &mut toml_edit::DocumentMut) {
    let Some(root) = find_openai_bundled_marketplace_root() else {
        return;
    };

    if !doc.contains_key("marketplaces") {
        doc["marketplaces"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let Some(marketplaces) = doc["marketplaces"].as_table_mut() else {
        return;
    };

    let mut marketplace = toml_edit::Table::new();
    marketplace["source_type"] = toml_edit::value("local");
    marketplace["source"] = toml_edit::value(root.to_string_lossy().to_string());
    marketplaces["openai-bundled"] = toml_edit::Item::Table(marketplace);
}

fn find_openai_bundled_marketplace_root() -> Option<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from)?;
    let windows_apps = program_files.join("WindowsApps");
    let entries = std::fs::read_dir(windows_apps).ok()?;
    let mut roots = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("OpenAI.Codex_") {
                return None;
            }
            let root = entry
                .path()
                .join("app")
                .join("resources")
                .join("plugins")
                .join("openai-bundled");
            root.join(".agents")
                .join("plugins")
                .join("marketplace.json")
                .is_file()
                .then_some(root)
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots.pop()
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

fn write_auth_json(path: &Path, options: &ConfigureCodexAppOptions) -> Result<()> {
    let jwt = local_chatgpt_jwt(options)?;
    let auth = json!({
        "auth_mode": LOCAL_AUTH_MODE,
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": jwt,
            "access_token": jwt,
            "refresh_token": "",
            "account_id": options.account_id,
        },
        "last_refresh": rfc3339_now(),
    });
    let raw = serde_json::to_string_pretty(&auth)?;
    backup_existing(path)?;
    std::fs::write(path, format!("{raw}\n"))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn is_codex_remote_auth_json(auth: &serde_json::Value) -> bool {
    let auth_mode = auth.get("auth_mode").and_then(|value| value.as_str());
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
        .is_some_and(is_codex_remote_local_jwt)
}

fn is_codex_remote_local_jwt(token: &str) -> bool {
    let Some(payload) = token.split('.').nth(1) else {
        return false;
    };
    let Ok(payload) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) else {
        return false;
    };
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&payload) else {
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

fn local_chatgpt_jwt(options: &ConfigureCodexAppOptions) -> Result<String> {
    let now = unix_now()?;
    let exp = now + 10 * 365 * 24 * 60 * 60;
    let payload = json!({
        "iss": "https://auth.openai.com",
        "aud": ["https://api.openai.com/v1"],
        "iat": now,
        "nbf": now,
        "exp": exp,
        "sub": format!("local|{}", options.user_id),
        "email": options.email,
        "email_verified": true,
        "https://api.openai.com/profile": {
            "email": options.email,
            "email_verified": true,
        },
        "https://api.openai.com/auth": {
            "chatgpt_account_id": options.account_id,
            "account_id": options.account_id,
            "chatgpt_account_user_id": format!("{}__{}", options.user_id, options.account_id),
            "account_user_id": format!("{}__{}", options.user_id, options.account_id),
            "chatgpt_plan_type": options.plan_type,
            "chatgpt_user_id": options.user_id,
            "user_id": options.user_id,
            "chatgpt_account_is_fedramp": false,
            "localhost": true,
            "groups": [],
            "organizations": [{
                "id": options.account_id,
                "is_default": true,
                "role": "owner",
                "title": "Codex Remote Local",
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
    // CODEX_HOME of the process that happens to run codex-remote.
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".codex"))
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_codex_app_writes_provider_and_local_auth() {
        let codex_home = unique_temp_dir();
        let report = configure_codex_app(ConfigureCodexAppOptions {
            codex_home: Some(codex_home.clone()),
            backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
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
        assert!(config.contains("[features]"));
        assert!(config.contains("apps = false"));
        assert!(config.contains("image_generation = false"));
        assert!(config.contains("[model_providers.ai-codex]"));
        assert!(config.contains("base_url = \"https://api.example.invalid\""));
        assert!(config.contains("wire_api = \"responses\""));
        assert!(config.contains("requires_openai_auth = true"));
        assert!(config.contains("supports_websockets = true"));
        assert!(config.contains("experimental_bearer_token = \"test-provider-key\""));

        let auth = std::fs::read_to_string(report.auth_path).expect("read auth");
        assert!(auth.contains(&format!("\"auth_mode\": \"{LOCAL_AUTH_MODE}\"")));
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
    fn uninstall_codex_app_removes_root_routing_only() {
        let codex_home = unique_temp_dir();
        let config_path = codex_home.join("config.toml");
        let auth_path = codex_home.join("auth.json");
        std::fs::write(
            &config_path,
            r#"chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
model_provider = "codex"
model = "gpt-5.5"

[model_providers.codex]
name = "codex"
base_url = "https://api.example.invalid"
"#,
        )
        .expect("write config");
        write_auth_json(
            &auth_path,
            &ConfigureCodexAppOptions {
                codex_home: Some(codex_home.clone()),
                backend_url: "http://127.0.0.1:3847/backend-api".to_string(),
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
            },
        )
        .expect("write auth");

        let report = uninstall_codex_app(
            Some(codex_home.clone()),
            "http://127.0.0.1:3847/backend-api",
        )
        .expect("uninstall config");

        assert!(report.removed_chatgpt_base_url);
        assert!(report.removed_model_provider);
        assert!(!report.removed_auth);

        let config = std::fs::read_to_string(&config_path).expect("read config");
        assert!(!config.contains("chatgpt_base_url"));
        assert!(!config.contains("model_provider = \"codex\""));
        assert!(config.contains("model = \"gpt-5.5\""));
        assert!(config.contains("[model_providers.codex]"));
        assert!(config.contains("base_url = \"https://api.example.invalid\""));
        assert!(auth_path.exists());

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

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codex-remote-test-{}-{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
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
}

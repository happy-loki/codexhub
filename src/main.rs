#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

mod ai_gateway;
mod app_state;
mod bridge;
mod chain_log;
mod cli;
mod codex;
mod codex_app_config;
mod codex_app_enhanced;
mod codex_session_history;
mod config;
mod daemon_process;
mod diagnostics_export;
#[cfg(feature = "gui")]
mod gui;
mod im;
mod im_runtime;
mod outbound_http;
mod remote_control_backend;
mod store;
mod types;
mod vscode_extension_patch;
mod web;

use std::{
    env,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use axum::Router;
use serde_json::Value;
use tokio::{net::TcpListener, sync::watch};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::{
    app_state::AppState,
    cli::{Cli, Command},
    config::AppConfig,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse()?;
    if matches!(cli.command, Command::Gui) {
        return run_gui_command();
    }

    let config_path = config_path_from_cli(cli.config_path.clone());
    let mut config = AppConfig::load_or_default(&config_path)?;
    let should_save_config = !config_path.exists() || config.apply_platform_defaults();
    normalize_config_paths(&mut config, &config_path);
    let log_path = init_logging(&config)?;
    tracing::info!(
        target: "codexhub::logging",
        path = %log_path.display(),
        "codexhub chain log initialized"
    );
    if should_save_config {
        config.save(&config_path)?;
    }

    match cli.command {
        Command::Daemon => run_daemon(config_path, config).await,
        Command::On => set_bridge_enabled(&config_path, true).await,
        Command::Off => set_bridge_enabled(&config_path, false).await,
        Command::Status => print_status(&config).await,
        Command::ConfigureCodexApp {
            codex_home,
            provider_name,
            provider_base_url,
            provider_key,
            model: _,
        } => {
            let backend_url = config.remote_control_base_url();
            let report = codex_app_config::configure_codex_app(
                codex_app_config::ConfigureCodexAppOptions {
                    codex_home,
                    backend_url: backend_url.clone(),
                    connection_mode: config.local_connection_mode,
                    account_id: "acct_codexhub_local".to_string(),
                    user_id: "user_codexhub_local".to_string(),
                    email: "codexhub-local@example.local".to_string(),
                    plan_type: "pro".to_string(),
                    provider_name,
                    provider_base_url,
                    provider_key,
                    activate_provider: true,
                    image_generation_enabled: None,
                    provider_supports_websockets: None,
                },
            )?;
            println!("Codex App configured:");
            println!("  codex home: {}", report.codex_home.display());
            println!("  config: {}", report.config_path.display());
            println!("  auth: {}", report.auth_path.display());
            println!("  chatgpt_base_url: {}", report.backend_url);
            println!(
                "  remote_control switch: {}",
                if report.remote_control_switch.configured {
                    "enabled"
                } else {
                    "not enabled"
                }
            );
            Ok(())
        }
        Command::UninstallCodexApp { codex_home } => {
            let backend_url = config.remote_control_base_url();
            let report = codex_app_config::uninstall_codex_app(codex_home, &backend_url)?;
            println!("Codex App local remote-control config removed:");
            println!("  codex home: {}", report.codex_home.display());
            println!("  config: {}", report.config_path.display());
            println!("  auth: {}", report.auth_path.display());
            println!(
                "  removed chatgpt_base_url: {}",
                report.removed_chatgpt_base_url
            );
            println!(
                "  removed model_provider: {}",
                report.removed_model_provider
            );
            println!("  removed local auth: {}", report.removed_auth);
            println!(
                "  Codex App GUI backend: {}",
                report.gui_api_base.value.as_deref().unwrap_or("<unset>")
            );
            Ok(())
        }
        Command::Gui => unreachable!("GUI command is handled before config loading"),
    }
}

fn run_gui_command() -> anyhow::Result<()> {
    #[cfg(feature = "gui")]
    {
        gui::run();
        Ok(())
    }

    #[cfg(not(feature = "gui"))]
    {
        anyhow::bail!("this codexhub build does not include GUI support")
    }
}

async fn run_daemon(config_path: PathBuf, config: AppConfig) -> anyhow::Result<()> {
    let daemon_identity = daemon_process::DaemonIdentity::new();
    let _daemon_lock = daemon_process::DaemonInstanceLock::acquire(&config_path, &daemon_identity)?;
    let bind = config.bind.clone();
    outbound_http::init(&config.outbound_proxy, config.local_listen_port())?;
    let chain_log_path = chain_log_path(&config);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let (server_shutdown_tx, server_shutdown_rx) = watch::channel(false);
    let state = AppState::new(
        config_path,
        config,
        Some(shutdown_tx),
        Some(daemon_identity),
    );
    {
        let config = state.config.lock().await;
        state
            .push_event(
                "info",
                "config_loaded",
                format!(
                    "config={} state={}",
                    state.config_path.display(),
                    config.state_path.display()
                ),
            )
            .await;
    }
    state
        .push_event(
            "info",
            "chain_log_ready",
            format!("path={}", chain_log_path.display()),
        )
        .await;
    {
        let config = state.config.lock().await;
        let backend_url = config.remote_control_base_url();
        let gui_api_base = codex_app_config::configure_gui_environment(&backend_url, true);
        let proxy_cleanup = codex_app_config::cleanup_legacy_app_server_proxy_environment();
        state
            .push_event(
                "info",
                "codex_app_direct_api_environment_checked",
                format!(
                    "configured={} value={} error={}",
                    gui_api_base.configured,
                    gui_api_base.value.as_deref().unwrap_or_default(),
                    gui_api_base.error.as_deref().unwrap_or_default()
                ),
            )
            .await;
        state
            .push_event(
                if proxy_cleanup.is_ok() {
                    "info"
                } else {
                    "warn"
                },
                "codex_app_server_proxy_environment_cleanup_checked",
                match proxy_cleanup {
                    Ok(()) => "cleaned=true".to_string(),
                    Err(error) => format!("cleaned=false error={error}"),
                },
            )
            .await;
    }
    tokio::spawn(run_daemon_startup_tasks(state.clone()));
    let app = web::router(state).layer(TraceLayer::new_for_http());
    let addr: SocketAddr = bind
        .parse()
        .with_context(|| format!("invalid bind address `{bind}`"))?;
    let listener = TcpListener::bind(addr).await?;
    println!("codexhub web: http://{addr}");

    let companion = compatible_loopback_addr(addr);
    let mut companion_tasks = Vec::new();
    if let Some(companion_addr) = companion {
        match TcpListener::bind(companion_addr).await {
            Ok(companion_listener) => {
                println!("codexhub web: http://{companion_addr}");
                companion_tasks.push(tokio::spawn(serve_http(
                    companion_listener,
                    app.clone(),
                    server_shutdown_rx.clone(),
                )));
            }
            Err(err) => {
                tracing::warn!(
                    target: "codexhub::server",
                    addr = %companion_addr,
                    error = %err,
                    "compatible loopback listener unavailable"
                );
            }
        }
    }
    let shutdown_task_tx = server_shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        let _ = shutdown_task_tx.send(true);
    });

    let primary_result = serve_http(listener, app, server_shutdown_rx).await;
    let _ = server_shutdown_tx.send(true);
    for task in companion_tasks {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(
                    target: "codexhub::server",
                    error = %err,
                    "compatible loopback server stopped with error"
                );
            }
            Err(err) => {
                tracing::warn!(
                    target: "codexhub::server",
                    error = %err,
                    "compatible loopback server task failed"
                );
            }
        }
    }
    primary_result?;
    match vscode_extension_patch::restore_remote_control() {
        Ok(report) => {
            tracing::info!(
                target: "codexhub::vscode_extension_patch",
                action = %report.action,
                extension_js = %report.extension_js.as_ref().map(|path| path.display().to_string()).unwrap_or_default(),
                message = %report.message,
                "VS Code Codex extension restore finished"
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "codexhub::vscode_extension_patch",
                error = %err,
                "VS Code Codex extension restore failed"
            );
        }
    }
    Ok(())
}

fn compatible_loopback_addr(addr: SocketAddr) -> Option<SocketAddr> {
    let port = addr.port();
    match addr.ip() {
        IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST => {
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port))
        }
        IpAddr::V6(ip) if ip == Ipv6Addr::LOCALHOST => {
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
        }
        _ => None,
    }
}

async fn serve_http(
    listener: TcpListener,
    app: Router,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            if *shutdown_rx.borrow() {
                return;
            }
            let _ = shutdown_rx.changed().await;
        })
        .await?;
    Ok(())
}

async fn run_daemon_startup_tasks(state: crate::app_state::SharedState) {
    match vscode_extension_patch::enable_remote_control() {
        Ok(report) => {
            state
                .push_event(
                    "info",
                    "vscode_codex_extension_patch",
                    format!(
                        "action={} extension_js={} message={}",
                        report.action,
                        report
                            .extension_js
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_default(),
                        report.message
                    ),
                )
                .await;
        }
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "vscode_codex_extension_patch_failed",
                    err.to_string(),
                )
                .await;
        }
    }
    if state.config.lock().await.bridge.enabled {
        web::start_bridge_if_ready(&state, "bridge start requested during daemon startup").await;
    } else {
        state
            .push_event("warn", "bridge_disabled", "bridge disabled by config")
            .await;
    }
}

fn config_path_from_cli(path: Option<PathBuf>) -> PathBuf {
    if let Some(path) = path {
        return absolutize(path);
    }

    if env::var_os("CODEXHUB_HOME").is_some() {
        return app_support_config_path();
    }

    if let Some(path) = adjacent_config_from_current_exe() {
        return path;
    }

    if env::var_os("CODEXHUB_USE_REPO_CONFIG").is_none() {
        return app_support_config_path();
    }

    inferred_repo_config_from_target_exe()
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|cwd| cwd.join("config.toml"))
                .filter(|path| path.exists())
        })
        .unwrap_or_else(|| absolutize(PathBuf::from("config.toml")))
}

fn app_support_config_path() -> PathBuf {
    if let Some(base) = env::var_os("CODEXHUB_HOME").map(PathBuf::from) {
        return base.join("config.toml");
    }
    platform_app_support_config_path()
}

#[cfg(target_os = "windows")]
fn platform_app_support_config_path() -> PathBuf {
    let legacy = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/CodexHub/config.toml"));
    if let Some(path) = legacy.filter(|path| path.exists()) {
        return path;
    }
    let base = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("CodexHub").join("config.toml")
}

#[cfg(not(target_os = "windows"))]
fn platform_app_support_config_path() -> PathBuf {
    let base = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/CodexHub"))
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("config.toml")
}

fn inferred_repo_config_from_target_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let profile_dir = exe.parent()?;
    let target_dir = profile_dir.parent()?;
    if target_dir.file_name().and_then(|value| value.to_str()) != Some("target") {
        return None;
    }
    let profile = profile_dir.file_name().and_then(|value| value.to_str())?;
    if profile != "debug" && profile != "release" {
        return None;
    }
    let config = target_dir.parent()?.join("config.toml");
    config.exists().then_some(config)
}

fn adjacent_config_from_current_exe() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("config.toml")))
        .filter(|path| path.exists())
        .filter(|path| {
            // Only use the exe-adjacent config when its directory is actually
            // writable. Installed builds under protected locations such as
            // `C:\\Program Files\\CodexHub` ship a default `config.toml` next to
            // the exe, but the directory is read-only for normal-privilege
            // processes, so saving config there fails. In that case fall through
            // to the per-user app-support path instead.
            path.parent()
                .map(config_directory_is_writable)
                .unwrap_or(false)
        })
}

/// Returns true when a config file can be created/replaced inside `dir`.
fn config_directory_is_writable(dir: &Path) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let probe = dir.join(format!(".codexhub-write-probe-{nanos}"));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn normalize_config_paths(config: &mut AppConfig, config_path: &Path) {
    let base = config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    if config.state_path.is_relative() {
        config.state_path = base.join(&config.state_path);
    }
    if let Some(log_dir) = config.logging.log_dir.as_mut()
        && log_dir.is_relative()
    {
        *log_dir = base.join(&log_dir);
    }
}

fn init_logging(config: &AppConfig) -> anyhow::Result<PathBuf> {
    let path = chain_log_path(config);
    crate::chain_log::init(
        &path,
        effective_chain_log_diagnostic(config),
        config.logging.max_mb.saturating_mul(1024 * 1024),
        config.logging.retention_days,
    )?;

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("codexhub=info".parse()?))
        .with_ansi(false)
        .init();
    Ok(path)
}

fn effective_chain_log_diagnostic(config: &AppConfig) -> bool {
    config.logging.diagnostic
}

fn chain_log_path(config: &AppConfig) -> PathBuf {
    log_dir_from_config(config).join("codexhub-chain.log")
}

fn log_dir_from_config(config: &AppConfig) -> PathBuf {
    if let Some(log_dir) = &config.logging.log_dir
        && !log_dir.as_os_str().is_empty()
    {
        return log_dir.clone();
    }
    config
        .state_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
}

async fn set_bridge_enabled(config_path: &Path, enabled: bool) -> anyhow::Result<()> {
    let mut config = AppConfig::load_or_default(&config_path.to_path_buf())?;
    config.bridge.enabled = enabled;
    config.save(&config_path.to_path_buf())?;
    let _ = notify_daemon_bridge(&config, enabled).await;
    println!(
        "codexhub Feishu bridge {}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

async fn print_status(config: &AppConfig) -> anyhow::Result<()> {
    println!(
        "Feishu bridge: {}",
        if config.bridge.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!(
        "remote-control backend: {}",
        config.remote_control_base_url()
    );
    let status = query_daemon_backend_status(config).await;
    match status {
        Ok(status) => {
            let reason = status
                .get("reason")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    status
                        .get("remoteControlBaseUrl")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                })
                .unwrap_or("ok");
            let available = status
                .get("available")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            println!(
                "daemon: {} ({reason})",
                if available {
                    "available"
                } else {
                    "unavailable"
                }
            );
        }
        Err(err) => println!("daemon: unavailable ({err})"),
    }
    Ok(())
}

async fn notify_daemon_bridge(config: &AppConfig, enabled: bool) -> anyhow::Result<()> {
    let action = if enabled { "start" } else { "stop" };
    let url = format!("http://{}/api/bridge/{action}", config.bind);
    local_daemon_http_client()?
        .post(url)
        .timeout(Duration::from_millis(700))
        .send()
        .await?;
    Ok(())
}

async fn query_daemon_backend_status(config: &AppConfig) -> anyhow::Result<Value> {
    let url = format!("http://{}/api/remote-control/backend-status", config.bind);
    let response = local_daemon_http_client()?
        .get(url)
        .timeout(Duration::from_millis(700))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("daemon returned {}", response.status());
    }
    response.json::<Value>().await.map_err(Into::into)
}

fn local_daemon_http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .context("failed to build local daemon HTTP client")
}

fn absolutize(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or_else(|_| path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatible_loopback_addr_pairs_ipv4_and_ipv6_localhost() {
        let ipv4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3847);
        let ipv6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 3847);

        assert_eq!(compatible_loopback_addr(ipv4), Some(ipv6));
        assert_eq!(compatible_loopback_addr(ipv6), Some(ipv4));
    }

    #[test]
    fn compatible_loopback_addr_ignores_non_loopback() {
        let public_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 3847);

        assert_eq!(compatible_loopback_addr(public_addr), None);
    }
}

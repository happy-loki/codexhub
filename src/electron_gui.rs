use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};

use crate::config::AppConfig;

pub fn run(config_path: PathBuf) -> Result<()> {
    let electron_dir = electron_ui_dir()?;
    let package_json = electron_dir.join("package.json");
    if !package_json.exists() {
        anyhow::bail!("Electron UI is not available at {}", package_json.display());
    }
    ensure_electron_dependencies(&electron_dir)?;
    ensure_electron_dist(&electron_dir)?;

    let current_exe = env::current_exe().context("failed to locate current executable")?;
    let base_url = AppConfig::load_or_default(&config_path)
        .map(|config| daemon_base_url(&config.bind))
        .unwrap_or_else(|_| "http://127.0.0.1:3847".to_string());

    let mut command = Command::new(npm_command());
    command
        .arg("--prefix")
        .arg(&electron_dir)
        .arg("run")
        .arg("start")
        .env("CODEX_REMOTE_GATEWAY_BIN", current_exe)
        .env("CODEX_REMOTE_GATEWAY_CONFIG", &config_path)
        .env("CODEX_REMOTE_GATEWAY_BASE_URL", base_url)
        .env("CODEX_REMOTE_GATEWAY_ELECTRON_MODE", "desktop");

    let status = command
        .status()
        .with_context(|| format!("failed to launch Electron UI in {}", electron_dir.display()))?;
    if !status.success() {
        anyhow::bail!("Electron UI exited with {status}");
    }
    Ok(())
}

fn ensure_electron_dependencies(electron_dir: &Path) -> Result<()> {
    if electron_dir.join("node_modules").join("electron").exists() {
        return Ok(());
    }
    let install_command = if electron_dir.join("package-lock.json").exists() {
        "ci"
    } else {
        "install"
    };
    let status = Command::new(npm_command())
        .arg("--prefix")
        .arg(electron_dir)
        .arg(install_command)
        .status()
        .with_context(|| {
            format!(
                "failed to install Electron UI dependencies in {}",
                electron_dir.display()
            )
        })?;
    if !status.success() {
        anyhow::bail!("Electron UI dependency install exited with {status}");
    }
    Ok(())
}

fn ensure_electron_dist(electron_dir: &Path) -> Result<()> {
    if electron_dir.join("dist").join("index.html").exists() {
        return Ok(());
    }
    let status = Command::new(npm_command())
        .arg("--prefix")
        .arg(electron_dir)
        .arg("run")
        .arg("build")
        .status()
        .with_context(|| {
            format!(
                "failed to build Electron UI assets in {}",
                electron_dir.display()
            )
        })?;
    if !status.success() {
        anyhow::bail!("Electron UI build exited with {status}");
    }
    Ok(())
}

fn electron_ui_dir() -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("CODEX_REMOTE_GATEWAY_ELECTRON_DIR") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("electron-ui"));
    if let Ok(exe) = env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        candidates.push(exe_dir.join("electron-ui"));
        candidates.push(exe_dir.join("resources").join("electron-ui"));
        candidates.push(exe_dir.join("..").join("Resources").join("electron-ui"));
    }

    candidates
        .into_iter()
        .find(|path| path.join("package.json").exists())
        .ok_or_else(|| anyhow::anyhow!("could not find electron-ui/package.json"))
}

fn daemon_base_url(bind: &str) -> String {
    if bind.starts_with("http://") || bind.starts_with("https://") {
        return bind.trim_end_matches('/').to_string();
    }
    format!("http://{}", bind.trim_end_matches('/'))
}

fn npm_command() -> &'static Path {
    #[cfg(windows)]
    {
        Path::new("npm.cmd")
    }
    #[cfg(not(windows))]
    {
        Path::new("npm")
    }
}

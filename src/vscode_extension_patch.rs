use std::{
    env, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ORIGINAL_ARGS: &str = r#"["--analytics-default-enabled"]"#;
const PATCHED_ARGS: &str = r#"["--analytics-default-enabled","--remote-control"]"#;
const BACKUP_SUFFIX: &str = ".bak-codexhub";
const STATE_SUFFIX: &str = ".codexhub-state.json";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VsCodeExtensionPatchReport {
    pub extension_dir: Option<PathBuf>,
    pub extension_js: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
    pub action: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchState {
    extension_js: PathBuf,
    backup_path: PathBuf,
    original_sha256: String,
    patched_sha256: String,
    patched_at_unix_secs: u64,
}

#[derive(Debug)]
struct ExtensionInstall {
    dir: PathBuf,
    extension_js: PathBuf,
    version_key: Vec<u64>,
    modified: SystemTime,
}

pub fn enable_remote_control() -> Result<VsCodeExtensionPatchReport> {
    let Some(install) = find_latest_codex_extension()? else {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: None,
            extension_js: None,
            backup_path: None,
            action: "not_found".to_string(),
            message: "没有找到 OpenAI Codex VS Code 插件安装目录。".to_string(),
        });
    };

    let extension_js = install.extension_js;
    let backup_path = backup_path(&extension_js);
    let state_path = state_path(&extension_js);
    let source = fs::read_to_string(&extension_js)
        .with_context(|| format!("failed to read {}", extension_js.display()))?;

    if source.contains(PATCHED_ARGS) {
        ensure_state_for_existing_patch(&extension_js, &backup_path, &state_path, &source)?;
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: Some(backup_path),
            action: "already_patched".to_string(),
            message: "VS Code Codex 插件已经带有 --remote-control。".to_string(),
        });
    }

    if source.contains("--remote-control") {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: None,
            action: "already_supported".to_string(),
            message: "VS Code Codex 插件已包含 --remote-control，未创建还原备份。".to_string(),
        });
    }

    if !source.contains(ORIGINAL_ARGS) {
        return Err(anyhow!(
            "无法识别 VS Code Codex 插件启动参数位置: {}",
            extension_js.display()
        ));
    }

    if !backup_path.exists() {
        fs::copy(&extension_js, &backup_path).with_context(|| {
            format!(
                "failed to backup {} to {}",
                extension_js.display(),
                backup_path.display()
            )
        })?;
    }

    let patched = source.replacen(ORIGINAL_ARGS, PATCHED_ARGS, 1);
    fs::write(&extension_js, &patched)
        .with_context(|| format!("failed to write {}", extension_js.display()))?;
    write_patch_state(&extension_js, &backup_path, &source, &patched, &state_path)?;

    Ok(VsCodeExtensionPatchReport {
        extension_dir: Some(install.dir),
        extension_js: Some(extension_js),
        backup_path: Some(backup_path),
        action: "patched".to_string(),
        message: "已为 VS Code Codex 插件启动参数加入 --remote-control。".to_string(),
    })
}

pub fn restore_remote_control() -> Result<VsCodeExtensionPatchReport> {
    let Some(install) = find_latest_codex_extension()? else {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: None,
            extension_js: None,
            backup_path: None,
            action: "not_found".to_string(),
            message: "没有找到 OpenAI Codex VS Code 插件安装目录。".to_string(),
        });
    };

    let extension_js = install.extension_js;
    let backup_path = backup_path(&extension_js);
    let state_path = state_path(&extension_js);
    if !backup_path.exists() {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: Some(backup_path),
            action: "no_backup".to_string(),
            message: "没有找到 Codex Remote Gateway 创建的插件备份，未还原。".to_string(),
        });
    }

    let current = fs::read_to_string(&extension_js)
        .with_context(|| format!("failed to read {}", extension_js.display()))?;
    let state = read_patch_state(&state_path).ok();
    if let Some(state) = state.as_ref()
        && state.patched_sha256 != sha256_hex(current.as_bytes())
    {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: Some(backup_path),
            action: "skipped_modified".to_string(),
            message: "VS Code 插件文件已被用户或插件更新修改，未自动还原。".to_string(),
        });
    }

    if state.is_none() && !current.contains(PATCHED_ARGS) {
        return Ok(VsCodeExtensionPatchReport {
            extension_dir: Some(install.dir),
            extension_js: Some(extension_js),
            backup_path: Some(backup_path),
            action: "skipped_unmanaged".to_string(),
            message: "当前插件文件不像 Codex Remote Gateway 写入的版本，未自动还原。".to_string(),
        });
    }

    fs::copy(&backup_path, &extension_js).with_context(|| {
        format!(
            "failed to restore {} from {}",
            extension_js.display(),
            backup_path.display()
        )
    })?;
    let _ = fs::remove_file(&state_path);

    Ok(VsCodeExtensionPatchReport {
        extension_dir: Some(install.dir),
        extension_js: Some(extension_js),
        backup_path: Some(backup_path),
        action: "restored".to_string(),
        message: "已还原 VS Code Codex 插件原始启动方式。".to_string(),
    })
}

fn find_latest_codex_extension() -> Result<Option<ExtensionInstall>> {
    let mut installs = Vec::new();
    for root in extension_roots() {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(install) = inspect_extension_dir(&dir)? else {
                continue;
            };
            installs.push(install);
        }
    }
    installs.sort_by(|a, b| {
        a.version_key
            .cmp(&b.version_key)
            .then_with(|| a.modified.cmp(&b.modified))
    });
    Ok(installs.pop())
}

fn inspect_extension_dir(dir: &Path) -> Result<Option<ExtensionInstall>> {
    let package_path = dir.join("package.json");
    if !package_path.exists() {
        return Ok(None);
    }
    let package_text = fs::read_to_string(&package_path)
        .with_context(|| format!("failed to read {}", package_path.display()))?;
    let package: serde_json::Value = serde_json::from_str(&package_text)
        .with_context(|| format!("failed to parse {}", package_path.display()))?;
    if package.get("publisher").and_then(|value| value.as_str()) != Some("openai")
        || package.get("name").and_then(|value| value.as_str()) != Some("chatgpt")
    {
        return Ok(None);
    }

    let main = package
        .get("main")
        .and_then(|value| value.as_str())
        .unwrap_or("./out/extension.js")
        .trim_start_matches("./");
    let extension_js = dir.join(main);
    if !extension_js.exists() {
        return Ok(None);
    }
    let version = package
        .get("version")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let modified = fs::metadata(&extension_js)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH);
    Ok(Some(ExtensionInstall {
        dir: dir.to_path_buf(),
        extension_js,
        version_key: version_key(version),
        modified,
    }))
}

fn extension_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(value) = env::var_os("VSCODE_EXTENSIONS") {
        roots.push(PathBuf::from(value));
    }
    if let Some(home) = env::var_os("USERPROFILE").map(PathBuf::from) {
        roots.push(home.join(".vscode").join("extensions"));
        roots.push(home.join(".vscode-insiders").join("extensions"));
        roots.push(home.join(".vscodium").join("extensions"));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

fn backup_path(extension_js: &Path) -> PathBuf {
    extension_js.with_file_name(format!(
        "{}{}",
        extension_js
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("extension.js"),
        BACKUP_SUFFIX
    ))
}

fn state_path(extension_js: &Path) -> PathBuf {
    extension_js.with_file_name(format!(
        "{}{}",
        extension_js
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("extension.js"),
        STATE_SUFFIX
    ))
}

fn ensure_state_for_existing_patch(
    extension_js: &Path,
    backup_path: &Path,
    state_path: &Path,
    current: &str,
) -> Result<()> {
    if state_path.exists() || !backup_path.exists() {
        return Ok(());
    }
    let original = fs::read_to_string(backup_path)
        .with_context(|| format!("failed to read {}", backup_path.display()))?;
    write_patch_state(extension_js, backup_path, &original, current, state_path)
}

fn write_patch_state(
    extension_js: &Path,
    backup_path: &Path,
    original: &str,
    patched: &str,
    state_path: &Path,
) -> Result<()> {
    let state = PatchState {
        extension_js: extension_js.to_path_buf(),
        backup_path: backup_path.to_path_buf(),
        original_sha256: sha256_hex(original.as_bytes()),
        patched_sha256: sha256_hex(patched.as_bytes()),
        patched_at_unix_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    let text = serde_json::to_string_pretty(&state)?;
    fs::write(state_path, text).with_context(|| format!("failed to write {}", state_path.display()))
}

fn read_patch_state(path: &Path) -> io::Result<PatchState> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(io::Error::other)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

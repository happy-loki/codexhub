use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::now_ms;

pub const DAEMON_INSTANCE_ENV: &str = "CODEXHUB_DAEMON_INSTANCE_ID";
pub const DAEMON_SERVICE_NAME: &str = "codexhub";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonIdentity {
    pub service: String,
    pub pid: u32,
    pub instance_id: String,
    pub started_at_ms: u64,
}

impl DaemonIdentity {
    pub fn new() -> Self {
        let instance_id = std::env::var(DAEMON_INSTANCE_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        Self {
            service: DAEMON_SERVICE_NAME.to_string(),
            pid: std::process::id(),
            instance_id,
            started_at_ms: now_ms().min(u64::MAX as u128) as u64,
        }
    }

    pub fn is_codexhub(&self) -> bool {
        self.service == DAEMON_SERVICE_NAME && self.pid > 0 && !self.instance_id.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonMetadata {
    #[serde(flatten)]
    pub identity: DaemonIdentity,
    pub executable: String,
    pub config_path: String,
}

pub struct DaemonInstanceLock {
    file: File,
    metadata_path: PathBuf,
}

impl DaemonInstanceLock {
    pub fn acquire(config_path: &Path, identity: &DaemonIdentity) -> Result<Self> {
        let path = daemon_lock_path(config_path);
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create daemon lock directory `{}`",
                    parent.display()
                )
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open daemon lock `{}`", path.display()))?;
        if let Err(err) = FileExt::try_lock_exclusive(&file) {
            let owner = read_daemon_metadata(config_path)
                .map(|metadata| {
                    format!(
                        "pid={} instance_id={}",
                        metadata.identity.pid, metadata.identity.instance_id
                    )
                })
                .unwrap_or_else(|| "owner=unknown".to_string());
            return Err(anyhow!(
                "another CodexHub daemon holds `{}` ({owner}): {err}",
                path.display()
            ));
        }

        let metadata = DaemonMetadata {
            identity: identity.clone(),
            executable: std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            config_path: config_path.display().to_string(),
        };
        let metadata_path = daemon_metadata_path(config_path);
        let mut metadata_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&metadata_path)
            .with_context(|| {
                format!(
                    "failed to open daemon metadata `{}`",
                    metadata_path.display()
                )
            })?;
        serde_json::to_writer(&mut metadata_file, &metadata)?;
        metadata_file.write_all(b"\n")?;
        metadata_file.sync_data()?;
        Ok(Self {
            file,
            metadata_path,
        })
    }
}

impl Drop for DaemonInstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.metadata_path);
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn daemon_lock_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("codexhub-daemon.lock")
}

pub fn daemon_metadata_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("codexhub-daemon.json")
}

pub fn read_daemon_metadata(config_path: &Path) -> Option<DaemonMetadata> {
    let bytes = std::fs::read(daemon_metadata_path(config_path))
        .or_else(|_| std::fs::read(daemon_lock_path(config_path)))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn read_active_daemon_metadata(config_path: &Path) -> Option<DaemonMetadata> {
    let metadata = read_daemon_metadata(config_path)?;
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(daemon_lock_path(config_path))
        .ok()?;
    match FileExt::try_lock_exclusive(&lock_file) {
        Ok(()) => {
            let _ = FileExt::unlock(&lock_file);
            None
        }
        Err(_) => Some(metadata),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_lock_path_follows_config_directory() {
        let config = PathBuf::from("root").join("config.toml");
        assert_eq!(
            daemon_lock_path(&config),
            PathBuf::from("root").join("codexhub-daemon.lock")
        );
        assert_eq!(
            daemon_metadata_path(&config),
            PathBuf::from("root").join("codexhub-daemon.json")
        );
    }

    #[test]
    fn daemon_identity_requires_codexhub_service_and_instance() {
        let mut identity = DaemonIdentity::new();
        assert!(identity.is_codexhub());
        identity.service = "other".to_string();
        assert!(!identity.is_codexhub());
    }

    #[test]
    fn daemon_metadata_is_readable_while_lock_is_held() {
        let root = std::env::temp_dir().join(format!("codexhub-lock-test-{}", Uuid::new_v4()));
        let config_path = root.join("config.toml");
        std::fs::create_dir_all(&root).expect("create temp directory");
        let identity = DaemonIdentity::new();

        let daemon_lock =
            DaemonInstanceLock::acquire(&config_path, &identity).expect("acquire daemon lock");
        let metadata_bytes =
            std::fs::read(daemon_metadata_path(&config_path)).expect("read daemon metadata file");
        let metadata: DaemonMetadata =
            serde_json::from_slice(&metadata_bytes).expect("parse daemon metadata");
        assert_eq!(metadata.identity.pid, identity.pid);
        assert_eq!(metadata.identity.instance_id, identity.instance_id);
        let active_metadata =
            read_active_daemon_metadata(&config_path).expect("read active daemon metadata");
        assert_eq!(active_metadata.identity.instance_id, identity.instance_id);

        let second_identity = DaemonIdentity::new();
        let error = DaemonInstanceLock::acquire(&config_path, &second_identity)
            .err()
            .expect("second lock should fail")
            .to_string();
        assert!(error.contains(&format!("pid={}", identity.pid)));
        assert!(error.contains(&format!("instance_id={}", identity.instance_id)));

        drop(daemon_lock);
        assert!(!daemon_metadata_path(&config_path).exists());

        std::fs::write(daemon_metadata_path(&config_path), metadata_bytes)
            .expect("write stale daemon metadata");
        assert!(read_daemon_metadata(&config_path).is_some());
        assert!(read_active_daemon_metadata(&config_path).is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}

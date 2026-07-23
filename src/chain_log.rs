use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use once_cell::sync::OnceCell;

static CHAIN_LOG: OnceCell<ChainLog> = OnceCell::new();

struct ChainLog {
    inner: Mutex<ChainLogInner>,
    diagnostic: bool,
    max_bytes: u64,
}

struct ChainLogInner {
    file: Option<File>,
    path: PathBuf,
    written_bytes: u64,
}

pub fn init(
    path: &Path,
    diagnostic: bool,
    max_bytes: u64,
    retention_days: u64,
) -> anyhow::Result<()> {
    let log_dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid log path {}", path.display()))?;
    std::fs::create_dir_all(log_dir)
        .with_context(|| format!("failed to create log directory {}", log_dir.display()))?;
    cleanup_old_logs(log_dir, path, retention_days)?;
    rotate_if_large(path, max_bytes)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open chain log {}", path.display()))?;
    let _ = writeln!(
        file,
        "\n===== codexhub start ts_ms={} =====",
        timestamp_ms()
    );
    let written_bytes = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let _ = CHAIN_LOG.set(ChainLog {
        inner: Mutex::new(ChainLogInner {
            file: Some(file),
            path: path.to_path_buf(),
            written_bytes,
        }),
        diagnostic,
        max_bytes,
    });
    Ok(())
}

pub fn write_line(line: impl AsRef<str>) {
    let line = line.as_ref();
    if !should_write_default(line) {
        return;
    }
    write_line_inner(line, should_flush(line));
}

pub fn write_diagnostic_lazy(build: impl FnOnce() -> String) {
    if !diagnostic_enabled() {
        return;
    }
    let line = build();
    write_line_inner(&line, false);
}

pub fn diagnostic_enabled() -> bool {
    CHAIN_LOG.get().is_some_and(|log| log.diagnostic)
}

pub fn active_path() -> Option<PathBuf> {
    let log = CHAIN_LOG.get()?;
    let inner = log.inner.lock().ok()?;
    Some(inner.path.clone())
}

pub fn clear_logs() -> anyhow::Result<usize> {
    let Some(log) = CHAIN_LOG.get() else {
        return Ok(0);
    };
    let mut inner = log
        .inner
        .lock()
        .map_err(|_| anyhow::anyhow!("chain log lock is poisoned"))?;
    if let Some(mut file) = inner.file.take() {
        let _ = file.flush();
    }
    let active_path = inner.path.clone();
    let log_dir = active_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("invalid log path {}", active_path.display()))?
        .to_path_buf();
    let mut deleted = 0usize;
    if let Ok(entries) = std::fs::read_dir(&log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !is_codexhub_log_path(&path) {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                deleted = deleted.saturating_add(1);
            }
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&active_path)
        .with_context(|| format!("failed to reopen chain log {}", active_path.display()))?;
    let _ = writeln!(
        file,
        "\n===== codexhub log cleared ts_ms={} =====",
        timestamp_ms()
    );
    inner.file = Some(file);
    inner.written_bytes = 0;
    Ok(deleted)
}

fn write_line_inner(line: &str, flush: bool) {
    let Some(log) = CHAIN_LOG.get() else {
        return;
    };
    let Ok(mut inner) = log.inner.lock() else {
        return;
    };
    if log.max_bytes > 0 && inner.written_bytes >= log.max_bytes {
        rotate_open_log(&mut inner);
    }
    let wrote = if let Some(file) = inner.file.as_mut() {
        let _ = writeln!(file, "[ts_ms={}] {line}", timestamp_ms());
        if flush {
            let _ = file.flush();
        }
        true
    } else {
        false
    };
    if wrote {
        inner.written_bytes = inner
            .written_bytes
            .saturating_add(line.len() as u64)
            .saturating_add(24);
    }
}

fn rotate_open_log(inner: &mut ChainLogInner) {
    if let Some(mut file) = inner.file.take() {
        let _ = file.flush();
    }
    let rotated = rotated_path(&inner.path);
    let _ = std::fs::remove_file(&rotated);
    if inner.path.exists() {
        let _ = std::fs::rename(&inner.path, &rotated);
    }
    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&inner.path)
    {
        Ok(file) => {
            inner.file = Some(file);
            inner.written_bytes = 0;
        }
        Err(_) => {
            inner.file = None;
            inner.written_bytes = 0;
        }
    }
}

fn should_write_default(line: &str) -> bool {
    if CHAIN_LOG.get().is_some_and(|log| log.diagnostic) {
        return true;
    }
    let lower = line.to_ascii_lowercase();
    lower.contains("level=error")
        || lower.contains("level=warn")
        || lower.contains(" error")
        || lower.contains("err=")
        || lower.contains("failed")
        || lower.contains("timeout")
}

fn should_flush(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("level=error")
        || lower.contains("level=warn")
        || lower.contains("err=")
        || lower.contains("failed")
        || lower.contains("timeout")
}

fn rotate_if_large(path: &Path, max_bytes: u64) -> anyhow::Result<()> {
    if max_bytes == 0 || !path.exists() {
        return Ok(());
    }
    let len = std::fs::metadata(path)
        .with_context(|| format!("failed to stat chain log {}", path.display()))?
        .len();
    if len < max_bytes {
        return Ok(());
    }
    let rotated = rotated_path(path);
    let _ = std::fs::remove_file(&rotated);
    std::fs::rename(path, &rotated).with_context(|| {
        format!(
            "failed to rotate chain log {} to {}",
            path.display(),
            rotated.display()
        )
    })?;
    Ok(())
}

fn cleanup_old_logs(log_dir: &Path, active_path: &Path, retention_days: u64) -> anyhow::Result<()> {
    if retention_days == 0 {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return Ok(());
    };
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(
            retention_days.saturating_mul(24 * 60 * 60),
        ))
        .unwrap_or(UNIX_EPOCH);
    for entry in entries.flatten() {
        let path = entry.path();
        if path == active_path || !is_codexhub_log_path(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let modified = metadata
            .modified()
            .or_else(|_| metadata.created())
            .unwrap_or(SystemTime::now());
        if modified < cutoff {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

fn is_codexhub_log_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.starts_with("codexhub") && name.contains(".log"))
}

fn rotated_path(path: &Path) -> PathBuf {
    let mut rotated = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("codexhub-chain.log");
    rotated.set_file_name(format!("{file_name}.1"));
    rotated
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_open_log_replaces_active_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("codexhub-chain-log-test-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("codexhub-chain.log");
        std::fs::write(&path, "old\n").unwrap();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        let mut inner = ChainLogInner {
            file: Some(file),
            path: path.clone(),
            written_bytes: 4,
        };

        rotate_open_log(&mut inner);
        writeln!(inner.file.as_mut().unwrap(), "new").unwrap();
        drop(inner);

        assert!(rotated_path(&path).exists());
        assert_eq!(
            std::fs::read_to_string(rotated_path(&path)).unwrap(),
            "old\n"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        let _ = std::fs::remove_dir_all(dir);
    }
}

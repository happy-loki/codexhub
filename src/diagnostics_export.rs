use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use zip::{ZipWriter, write::SimpleFileOptions};

const MAX_LOG_FILES: usize = 4;
const MAX_LOG_BYTES_PER_FILE: u64 = 256 * 1024;

#[derive(Debug, Clone)]
pub struct ConnectionDiagnosticsInput {
    pub app_version: String,
    pub base_url: String,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
    pub remote_status: Option<Value>,
    pub codex_app_status: Option<Value>,
    pub service_status: Option<Value>,
    pub dashboard: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ConnectionDiagnosticsExport {
    pub path: PathBuf,
}

pub fn export_connection_diagnostics(
    input: &ConnectionDiagnosticsInput,
    output_dir: &Path,
) -> Result<ConnectionDiagnosticsExport> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let path = output_dir.join(format!(
        "codexhub-connection-diagnostics-{}.zip",
        timestamp_for_filename()
    ));
    export_connection_diagnostics_to_path(input, &path)
}

pub fn export_connection_diagnostics_to_path(
    input: &ConnectionDiagnosticsInput,
    path: &Path,
) -> Result<ConnectionDiagnosticsExport> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let file =
        File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    add_json(
        &mut zip,
        options,
        "summary.json",
        &diagnostics_summary(input),
    )?;
    let service_status = input
        .service_status
        .as_ref()
        .map(strip_im_runtime_status_fields);
    add_optional_json(
        &mut zip,
        options,
        "remote-control-status.json",
        input.remote_status.as_ref(),
    )?;
    add_optional_json(
        &mut zip,
        options,
        "codex-app-status.json",
        input.codex_app_status.as_ref(),
    )?;
    add_optional_json(
        &mut zip,
        options,
        "service-status.json",
        service_status.as_ref(),
    )?;
    add_optional_json(
        &mut zip,
        options,
        "gui-dashboard.json",
        input.dashboard.as_ref(),
    )?;
    add_logs(
        &mut zip,
        options,
        &log_dir_for_state_path(&input.state_path),
    )?;
    zip.finish().context("failed to finish diagnostics zip")?;

    Ok(ConnectionDiagnosticsExport {
        path: path.to_path_buf(),
    })
}

pub fn connection_status_snapshot(path: &str, result: Result<Value, String>) -> Value {
    match result {
        Ok(response) => json!({
            "path": path,
            "ok": true,
            "response": response,
        }),
        Err(error) => json!({
            "path": path,
            "ok": false,
            "error": error,
        }),
    }
}

fn diagnostics_summary(input: &ConnectionDiagnosticsInput) -> Value {
    json!({
        "kind": "codexhub_connection_diagnostics",
        "exportedAtMs": timestamp_ms(),
        "appVersion": input.app_version,
        "baseUrl": input.base_url,
        "configPath": input.config_path.to_string_lossy(),
        "statePath": input.state_path.to_string_lossy(),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "included": {
            "remoteStatus": input.remote_status.is_some(),
            "codexAppStatus": input.codex_app_status.is_some(),
            "serviceStatus": input.service_status.is_some(),
            "dashboard": input.dashboard.is_some(),
            "aiGatewayRequestLogs": false
        },
        "logLimits": {
            "maxFiles": MAX_LOG_FILES,
            "maxBytesPerFile": MAX_LOG_BYTES_PER_FILE
        },
        "privacy": {
            "secretFieldsRedacted": true,
            "requestLogsIncluded": false,
            "authFilesIncluded": false,
            "imRuntimeStateIncluded": false
        }
    })
}

fn add_optional_json(
    zip: &mut ZipWriter<File>,
    options: SimpleFileOptions,
    name: &str,
    value: Option<&Value>,
) -> Result<()> {
    if let Some(value) = value {
        add_json(zip, options, name, value)?;
    }
    Ok(())
}

fn add_json(
    zip: &mut ZipWriter<File>,
    options: SimpleFileOptions,
    name: &str,
    value: &Value,
) -> Result<()> {
    let redacted = redact_value(value);
    let bytes = serde_json::to_vec_pretty(&redacted).context("failed to serialize diagnostics")?;
    zip.start_file(name, options)
        .with_context(|| format!("failed to add {name}"))?;
    zip.write_all(&bytes)
        .with_context(|| format!("failed to write {name}"))?;
    zip.write_all(b"\n")
        .with_context(|| format!("failed to finish {name}"))?;
    Ok(())
}

fn add_logs(zip: &mut ZipWriter<File>, options: SimpleFileOptions, log_dir: &Path) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return Ok(());
    };
    let mut logs = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !is_codexhub_connection_log(&path) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            let modified = metadata
                .modified()
                .or_else(|_| metadata.created())
                .unwrap_or(UNIX_EPOCH);
            Some(LogCandidate {
                path,
                modified,
                len: metadata.len(),
            })
        })
        .collect::<Vec<_>>();
    logs.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| log_file_name(&a.path).cmp(&log_file_name(&b.path)))
    });
    logs.truncate(MAX_LOG_FILES);

    for log in logs {
        let Some(file_name) = log.path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let contents = sanitize_log_text(&read_log_tail(&log.path, log.len)?);
        if contents.trim().is_empty() {
            continue;
        }
        zip.start_file(format!("logs/{file_name}"), options)
            .with_context(|| format!("failed to add log {file_name}"))?;
        zip.write_all(contents.as_bytes())
            .with_context(|| format!("failed to write log {file_name}"))?;
    }
    Ok(())
}

struct LogCandidate {
    path: PathBuf,
    modified: SystemTime,
    len: u64,
}

fn read_log_tail(path: &Path, len: u64) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let start = len.saturating_sub(MAX_LOG_BYTES_PER_FILE);
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .with_context(|| format!("failed to seek {}", path.display()))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let contents = String::from_utf8_lossy(&bytes);
    if start > 0 {
        Ok(format!(
            "[codexhub diagnostics] log truncated; showing last {} bytes of {} bytes\n{}",
            bytes.len(),
            len,
            contents
        ))
    } else {
        Ok(contents.into_owned())
    }
}

fn log_dir_for_state_path(state_path: &Path) -> PathBuf {
    state_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("logs")
}

fn is_codexhub_connection_log(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| {
            let lower = name.to_ascii_lowercase();
            lower.starts_with("codexhub")
                && lower.contains(".log")
                && !lower.contains("ai-gateway")
                && !lower.contains("ai_gateway")
                && !lower.contains("request")
        })
}

fn log_file_name(path: &Path) -> String {
    path.file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn strip_im_runtime_status_fields(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let stripped = map
                .iter()
                .filter_map(|(key, value)| {
                    if is_im_runtime_status_key(key) {
                        None
                    } else {
                        Some((key.clone(), strip_im_runtime_status_fields(value)))
                    }
                })
                .collect();
            Value::Object(stripped)
        }
        Value::Array(values) => {
            Value::Array(values.iter().map(strip_im_runtime_status_fields).collect())
        }
        _ => value.clone(),
    }
}

fn is_im_runtime_status_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(
        key.as_str(),
        "imaccounts"
            | "im_accounts"
            | "feishu"
            | "feishuws"
            | "feishu_ws"
            | "telegram"
            | "wechat"
            | "wechatrecovery"
            | "wechat_recovery"
    ) || key.starts_with("im_")
}

fn redact_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let redacted = map
                .iter()
                .map(|(key, value)| {
                    if is_sensitive_key(key) {
                        (key.clone(), Value::String("<redacted>".to_string()))
                    } else {
                        (key.clone(), redact_value(value))
                    }
                })
                .collect();
            Value::Object(redacted)
        }
        Value::Array(values) => Value::Array(values.iter().map(redact_value).collect()),
        Value::String(value) => Value::String(redact_sensitive_string(value)),
        _ => value.clone(),
    }
}

fn sanitize_log_text(value: &str) -> String {
    value
        .lines()
        .filter(|line| should_include_diagnostics_log_line(line))
        .map(|line| {
            let redacted = redact_log_identifiers(line);
            // Remote-control diagnostic lines legitimately carry `client_key=`, whose
            // `key=` fragment trips the coarse full-line redactor. For those lines we
            // check for genuine secret material only, so structured metadata survives
            // while real credentials are still scrubbed.
            let sensitive = if is_remote_control_diagnostic_line(&redacted) {
                line_contains_secret_material(&redacted)
            } else {
                line_contains_sensitive_key(&redacted)
            };
            if sensitive {
                redact_sensitive_line(&redacted)
            } else {
                redacted
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn should_include_diagnostics_log_line(line: &str) -> bool {
    // Remote-control diagnostic lines only carry protocol metadata (method, id,
    // thread, timers, health). They never carry conversation text, which lives on
    // `[im_trace]` lines that stay filtered below. Keep them even when they mention
    // an IM platform via a `client_key`, since that identity is redacted separately.
    if is_remote_control_diagnostic_line(line) {
        return true;
    }
    !is_im_diagnostics_log_line(line)
}

fn is_remote_control_diagnostic_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("[remote_control]")
        || lower.contains("kind=remote_control")
        || lower.contains("event=remote_control")
}

fn is_im_diagnostics_log_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase().replace("codexhub-feishu", "");
    [
        "@im.bot",
        "[feishu_",
        "[im_",
        "[telegram_",
        "[wechat_",
        "api.telegram",
        "feishu",
        "feishu=",
        "feishu_",
        "ilinkai.weixin",
        "im_route",
        "im_trace",
        "platform=feishu",
        "platform=telegram",
        "platform=wechat",
        "telegram",
        "telegram=",
        "telegram_",
        "wechat",
        "wechat=",
        "wechat_",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn line_contains_sensitive_key(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "authorization",
        "auth_token",
        "bot_token",
        "client_secret",
        "key:",
        "key=",
        "password",
        "secret",
        "session_key",
        "token",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Genuine secret material, excluding the bare `key:`/`key=` fragments that also
/// match harmless identifiers such as `client_key=`. Used for remote-control
/// diagnostic lines so their metadata is not truncated by the coarse redactor.
fn line_contains_secret_material(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "authorization",
        "auth_token",
        "bot_token",
        "client_secret",
        "password",
        "secret",
        "session_key",
        "token",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn redact_sensitive_line(line: &str) -> String {
    let prefix = line
        .find(['=', ':'])
        .map(|index| &line[..=index])
        .unwrap_or(line);
    format!("{prefix}<redacted>")
}

fn redact_log_identifiers(line: &str) -> String {
    [
        "account_id",
        "client_id",
        "connection_id",
        "environment_id",
        "installation_id",
        "server_id",
        "server_name",
        "stream_id",
    ]
    .iter()
    .fold(
        redact_im_client_keys(&line.replace("codexhub-feishu", "<redacted>")),
        |line, field| redact_key_value_field(&line, field),
    )
}

fn redact_im_client_keys(line: &str) -> String {
    // IM `client_key`s look like `im:feishu:<hash>`. The hash is derived from the
    // account and chat, so it identifies a conversation. Keep the platform for
    // diagnostics, but strip the identifying suffix.
    ["feishu", "telegram", "wechat"]
        .iter()
        .fold(line.to_string(), |line, platform| {
            let needle = format!("im:{platform}:");
            let mut result = String::new();
            let mut rest = line.as_str();
            while let Some(index) = rest.find(&needle) {
                let value_start = index + needle.len();
                result.push_str(&rest[..value_start]);
                result.push_str("<redacted>");
                let value_end = rest[value_start..]
                    .find(|ch: char| !ch.is_ascii_alphanumeric())
                    .map(|offset| value_start + offset)
                    .unwrap_or(rest.len());
                rest = &rest[value_end..];
            }
            result.push_str(rest);
            result
        })
}

fn redact_key_value_field(line: &str, field: &str) -> String {
    let needle = format!("{field}=");
    let mut result = String::new();
    let mut rest = line;
    while let Some(index) = rest.find(&needle) {
        let value_start = index + needle.len();
        result.push_str(&rest[..value_start]);
        result.push_str("<redacted>");
        let value_end = rest[value_start..]
            .find(char::is_whitespace)
            .map(|offset| value_start + offset)
            .unwrap_or(rest.len());
        rest = &rest[value_end..];
    }
    result.push_str(rest);
    result
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    if matches!(
        key.as_str(),
        "authfilesincluded"
            | "imgatewayrequestlogs"
            | "imruntimestateincluded"
            | "requestlogsincluded"
            | "secretfieldsredacted"
    ) {
        return false;
    }
    key.contains("api_key")
        || key.contains("apikey")
        || key == "accountid"
        || key == "account_id"
        || key.contains("authorization")
        || key.contains("auth")
        || key.contains("bot_token")
        || key == "clientid"
        || key == "client_id"
        || key.contains("client_secret")
        || key == "environmentid"
        || key == "environment_id"
        || key == "id"
        || key == "installationid"
        || key == "installation_id"
        || key == "key"
        || key.ends_with("_key")
        || key.contains("password")
        || key == "serverid"
        || key == "server_id"
        || key == "servername"
        || key == "server_name"
        || key.contains("secret")
        || key.contains("session_key")
        || key == "streamid"
        || key == "stream_id"
        || key.contains("token")
        || key == "userid"
        || key == "user_id"
        || key == "useragent"
        || key == "user_agent"
}

fn redact_sensitive_string(value: &str) -> String {
    if value.contains("@im.bot") || value == "codexhub-feishu" {
        "<redacted>".to_string()
    } else {
        value.to_string()
    }
}

fn timestamp_for_filename() -> String {
    timestamp_ms().to_string()
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use serde_json::json;

    use super::*;

    fn zip_entry_to_string(archive: &mut zip::ZipArchive<File>, name: &str) -> String {
        let mut contents = String::new();
        archive
            .by_name(name)
            .unwrap_or_else(|_| panic!("missing zip entry {name}"))
            .read_to_string(&mut contents)
            .unwrap_or_else(|_| panic!("read zip entry {name}"));
        contents
    }

    #[test]
    fn connection_diagnostics_redacts_status_and_includes_only_connection_logs() {
        let unique = format!(
            "codexhub-diagnostics-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let logs = root.join("logs");
        std::fs::create_dir_all(&logs).expect("create logs dir");
        std::fs::write(
            logs.join("codexhub-chain.log"),
            "connected\napi_key=sk-secret\nAuthorization: Bearer abc\n",
        )
        .expect("write chain log");
        std::fs::write(
            logs.join("codexhub-daemon-startup.log"),
            "daemon failed before chain log\napi_key=sk-secret\n",
        )
        .expect("write daemon startup log");
        std::fs::write(logs.join("ai-gateway-request-log.sqlite"), "do not include")
            .expect("write request log db");
        std::fs::write(logs.join("codexhub-ai-gateway.log"), "do not include")
            .expect("write ai gateway log");

        let input = ConnectionDiagnosticsInput {
            app_version: "v0.test".to_string(),
            base_url: "http://127.0.0.1:3847".to_string(),
            config_path: root.join("config.toml"),
            state_path: root.join("codexhub-state.json"),
            remote_status: Some(json!({
                "connected": true,
                "authToken": "secret",
                "connections": [
                    {"sourceKind": "vscode", "connected": true, "token": "secret"}
                ]
            })),
            codex_app_status: Some(json!({
                "configured": true,
                "authPath": "/Users/alice/.codex/auth.json",
                "provider": {"name": "CodexHub", "key": "sk-secret"}
            })),
            service_status: Some(json!({"bind": "127.0.0.1:3847"})),
            dashboard: None,
        };

        let export =
            export_connection_diagnostics(&input, &root.join("exports")).expect("export zip");
        let file = File::open(export.path).expect("open zip");
        let mut archive = zip::ZipArchive::new(file).expect("read zip");
        let names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect::<Vec<_>>();

        assert!(names.contains(&"summary.json".to_string()));
        assert!(names.contains(&"remote-control-status.json".to_string()));
        assert!(names.contains(&"codex-app-status.json".to_string()));
        assert!(names.contains(&"logs/codexhub-chain.log".to_string()));
        assert!(names.contains(&"logs/codexhub-daemon-startup.log".to_string()));
        assert!(!names.iter().any(|name| name.contains("ai-gateway")));

        let mut remote = String::new();
        archive
            .by_name("remote-control-status.json")
            .unwrap()
            .read_to_string(&mut remote)
            .unwrap();
        assert!(remote.contains("\"authToken\": \"<redacted>\""));
        assert!(remote.contains("\"token\": \"<redacted>\""));
        assert!(!remote.contains("secret"));

        let mut log = String::new();
        archive
            .by_name("logs/codexhub-chain.log")
            .unwrap()
            .read_to_string(&mut log)
            .unwrap();
        assert!(log.contains("connected"));
        assert!(log.contains("api_key=<redacted>"));
        assert!(log.contains("Authorization:<redacted>"));
        assert!(!log.contains("sk-secret"));
        assert!(!log.contains("Bearer abc"));

        let mut startup_log = String::new();
        archive
            .by_name("logs/codexhub-daemon-startup.log")
            .unwrap()
            .read_to_string(&mut startup_log)
            .unwrap();
        assert!(startup_log.contains("daemon failed before chain log"));
        assert!(startup_log.contains("api_key=<redacted>"));
        assert!(!startup_log.contains("sk-secret"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn connection_diagnostics_excludes_im_runtime_status_and_logs() {
        let unique = format!(
            "codexhub-diagnostics-im-filter-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let logs = root.join("logs");
        std::fs::create_dir_all(&logs).expect("create logs dir");
        std::fs::write(
            logs.join("codexhub-chain.log"),
            [
                "[remote_control] event=ws_open client_id=codexhub-feishu stream_id=stream-secret installation_id=install-secret source_kind=codex_app",
                "[remote_control] event=request_send client_key=im:feishu:bf166bbba9dc50bf method=turn/start thread=thread-abc123",
                "[event] level=warn kind=remote_control_request_timeout message=client_key=im:feishu:bf166bbba9dc50bf method=turn/start id=200069 timeout_secs=45",
                "[wechat_api] event=request url=https://ilinkai.weixin.qq.com/ilink/bot/getupdates account=wechat-account",
                "[event] level=info kind=bridge_running message=bridge running feishu=0 telegram=0 wechat=1",
                "[im_route] event=bind platform=wechat account=wechat-account chat=chat-secret conversation=wechat:secret",
                "[codex_app_config] event=remote_control_switch_done configured=true",
            ]
            .join("\n"),
        )
        .expect("write chain log");

        let input = ConnectionDiagnosticsInput {
            app_version: "v0.test".to_string(),
            base_url: "http://127.0.0.1:3847".to_string(),
            config_path: root.join("config.toml"),
            state_path: root.join("codexhub-state.json"),
            remote_status: Some(json!({
                "path": "/api/remote-control/status",
                "ok": true,
                "response": {
                    "connected": true,
                    "clientId": "codexhub-feishu",
                    "activeConnectionId": "conn-secret",
                    "activeSourceKind": "codex_app",
                    "connections": [
                        {
                            "id": "conn-secret",
                            "sourceKind": "codex_app",
                            "connected": true,
                            "initialized": true,
                            "installationId": "install-secret",
                            "accountId": "acct-secret"
                        }
                    ]
                }
            })),
            codex_app_status: None,
            service_status: Some(json!({
                "path": "/api/status",
                "ok": true,
                "response": {
                    "bind": "127.0.0.1:3847",
                    "running": true,
                    "localConnectionMode": "standard",
                    "wechat": {"connected": true},
                    "telegram": {"connected": false},
                    "feishuWs": {"connected": false},
                    "imAccounts": [
                        {
                            "platform": "wechat",
                            "accountId": "wechat-account@im.bot",
                            "connected": true
                        }
                    ]
                }
            })),
            dashboard: None,
        };

        let export =
            export_connection_diagnostics(&input, &root.join("exports")).expect("export zip");
        let file = File::open(export.path).expect("open zip");
        let mut archive = zip::ZipArchive::new(file).expect("read zip");
        let summary = zip_entry_to_string(&mut archive, "summary.json");
        let remote = zip_entry_to_string(&mut archive, "remote-control-status.json");
        let service = zip_entry_to_string(&mut archive, "service-status.json");
        let log = zip_entry_to_string(&mut archive, "logs/codexhub-chain.log");
        let combined = format!("{summary}\n{remote}\n{service}\n{log}").to_ascii_lowercase();

        assert!(summary.contains("\"imRuntimeStateIncluded\": false"));
        assert!(service.contains("\"bind\": \"127.0.0.1:3847\""));
        assert!(!service.contains("imAccounts"));
        assert!(!service.contains("wechat"));
        assert!(!service.contains("telegram"));
        assert!(!service.contains("feishuWs"));
        assert!(log.contains("[remote_control] event=ws_open"));
        assert!(log.contains("[codex_app_config] event=remote_control_switch_done"));
        assert!(log.contains("client_id=<redacted>"));
        assert!(log.contains("stream_id=<redacted>"));
        assert!(log.contains("installation_id=<redacted>"));
        // Remote-control diagnostic lines survive IM filtering so timeouts can be
        // triaged from an upload, but the conversation-identifying client_key hash
        // is redacted while the platform stays visible.
        assert!(log.contains("[remote_control] event=request_send"));
        assert!(log.contains("kind=remote_control_request_timeout"));
        assert!(log.contains("method=turn/start"));
        assert!(log.contains("timeout_secs=45"));
        assert!(log.contains("im:feishu:<redacted>"));
        assert!(!log.contains("bf166bbba9dc50bf"));
        assert!(!combined.contains("wechat-account"));
        assert!(!combined.contains("@im.bot"));
        assert!(!combined.contains("ilinkai.weixin"));
        assert!(!combined.contains("telegram"));
        assert!(!combined.contains("im_route"));
        assert!(!combined.contains("codexhub-feishu"));
        assert!(!combined.contains("stream-secret"));
        assert!(!combined.contains("install-secret"));
        assert!(!combined.contains("acct-secret"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn connection_status_snapshot_records_api_errors() {
        let snapshot = connection_status_snapshot(
            "/api/remote-control/status",
            Err("request timed out".to_string()),
        );

        assert_eq!(
            snapshot,
            json!({
                "path": "/api/remote-control/status",
                "ok": false,
                "error": "request timed out",
            })
        );
    }

    #[test]
    fn connection_diagnostics_can_export_to_user_selected_zip_path() {
        let unique = format!(
            "codexhub-diagnostics-selected-path-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let input = ConnectionDiagnosticsInput {
            app_version: "v0.test".to_string(),
            base_url: "http://127.0.0.1:3847".to_string(),
            config_path: root.join("config.toml"),
            state_path: root.join("codexhub-state.json"),
            remote_status: Some(json!({"ok": true})),
            codex_app_status: None,
            service_status: None,
            dashboard: None,
        };
        let target = root.join("chosen").join("support.zip");

        let export =
            export_connection_diagnostics_to_path(&input, &target).expect("export selected zip");

        assert_eq!(export.path, target);
        assert!(export.path.exists());
        let file = File::open(&export.path).expect("open zip");
        let mut archive = zip::ZipArchive::new(file).expect("read zip");
        assert!(archive.by_name("summary.json").is_ok());
        assert!(archive.by_name("remote-control-status.json").is_ok());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn connection_diagnostics_limits_log_count_and_exports_log_tail() {
        let unique = format!(
            "codexhub-diagnostics-log-limit-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let logs = root.join("logs");
        std::fs::create_dir_all(&logs).expect("create logs dir");
        for index in 0..(MAX_LOG_FILES + 2) {
            std::fs::write(logs.join(format!("codexhub-{index}.log")), "small log\n")
                .expect("write log");
        }
        let large_log = logs.join("codexhub-chain.log");
        let large_contents = format!(
            "unique-head-marker\n{}tail-marker\nkey=secret\n",
            "padding\n".repeat((MAX_LOG_BYTES_PER_FILE as usize / 8) + 32)
        );
        std::fs::write(&large_log, large_contents).expect("write large log");

        let input = ConnectionDiagnosticsInput {
            app_version: "v0.test".to_string(),
            base_url: "http://127.0.0.1:3847".to_string(),
            config_path: root.join("config.toml"),
            state_path: root.join("codexhub-state.json"),
            remote_status: None,
            codex_app_status: None,
            service_status: None,
            dashboard: None,
        };

        let export =
            export_connection_diagnostics(&input, &root.join("exports")).expect("export zip");
        let file = File::open(export.path).expect("open zip");
        let mut archive = zip::ZipArchive::new(file).expect("read zip");
        let log_names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .filter(|name| name.starts_with("logs/"))
            .collect::<Vec<_>>();
        assert!(log_names.len() <= MAX_LOG_FILES);

        let mut log = String::new();
        archive
            .by_name("logs/codexhub-chain.log")
            .unwrap()
            .read_to_string(&mut log)
            .unwrap();
        assert!(log.contains("log truncated"));
        assert!(log.contains("tail-marker"));
        assert!(log.contains("key=<redacted>"));
        assert!(!log.contains("unique-head-marker"));
        assert!(!log.contains("secret"));

        let _ = std::fs::remove_dir_all(root);
    }
}

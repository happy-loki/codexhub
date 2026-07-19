use std::{
    collections::HashSet,
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use url::Url;

use crate::chain_log;

const DEFAULT_CDP_PORT: u16 = 9335;
const CODEX_APP_READY_TIMEOUT: Duration = Duration::from_secs(30);
const ENHANCED_INJECTION_TIMEOUT: Duration = Duration::from_secs(45);
// The injected script retries internally for about 24 seconds. Re-run it only
// after that window so a cold renderer does not accumulate retry timers.
const ENHANCED_SCRIPT_RETRY_INTERVAL: Duration = Duration::from_secs(26);
const ENHANCED_STATUS_POLL_INTERVAL: Duration = Duration::from_millis(250);
const ENHANCED_SCRIPT_VERSION: u64 = 8;
type CdpSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
static ENHANCED_CDP_SESSION: OnceLock<Mutex<Option<tokio::task::JoinHandle<()>>>> = OnceLock::new();
const SUPPORTED_FEATURE_GATES: &[&str] = &[
    "1042620455",
    "4114442250",
    "824038554",
    "410065390",
    "2296472986",
    "3446105535",
];
const LEGACY_CODEXHUB_FEATURE_GATES: &[&str] = &[
    "1834314516",
    "1714131075",
    "72045066",
    "2982604767",
    "2177625257",
    "3657624089",
    "3245360288",
    "3646210497",
    "1186680773",
    "1042620455",
    "4114442250",
    "824038554",
    "410065390",
    "2296472986",
];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhancedLaunchReport {
    pub port: u16,
    pub launched: bool,
    pub target_id: String,
    pub available_models: Vec<String>,
    pub use_hidden_models: bool,
    pub key_gates_enabled: usize,
    pub i18n_enabled: bool,
    pub fast_initialize_applied: bool,
    pub fast_initialize_source: Option<String>,
    pub bootstrap_intercepted: bool,
    pub bootstrap_source: Option<String>,
    pub routes_mounted: bool,
    pub renderer_ready_ms: Option<u64>,
    pub startup_elapsed_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhancedLaunchPreflight {
    pub running: bool,
}

pub async fn preflight() -> Result<EnhancedLaunchPreflight> {
    let running = tokio::task::spawn_blocking(codex_app_is_running)
        .await
        .context("检测 Codex App 进程失败")??;
    Ok(EnhancedLaunchPreflight { running })
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpTarget {
    id: String,
    #[serde(rename = "type")]
    target_type: String,
    url: String,
    web_socket_debugger_url: Option<String>,
}

pub async fn launch_and_inject(
    models: Vec<String>,
    backend_url: &str,
) -> Result<EnhancedLaunchReport> {
    let started = Instant::now();
    let models = normalized_models(models);
    if models.is_empty() {
        bail!("Codex 可见模型列表为空，请先在 Codex 接入页面保存模型");
    }
    chain_log::write_line(format!(
        "[codex_app_enhanced] event=launch_start model_count={}",
        models.len()
    ));
    crate::codex_app_config::prepare_codex_app_config_recovery_snapshot(None)
        .context("准备 Codex 配置恢复快照失败")?;

    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()?;
    let mut launched = false;
    let app_running = tokio::task::spawn_blocking(codex_app_is_running)
        .await
        .context("检测 Codex App 进程失败")??;
    let target = if app_running {
        match find_app_target(&client, DEFAULT_CDP_PORT).await? {
            Some(target) => {
                crate::codex_app_config::configure_gui_direct_api_base(backend_url)
                    .map_err(|err| anyhow!("配置 CODEX_API_BASE_URL 失败: {err}"))?;
                target
            }
            None => bail!("Codex App 正在运行。请先完全退出，再使用增强模式启动 Codex"),
        }
    } else {
        crate::codex_app_config::configure_gui_direct_api_base(backend_url)
            .map_err(|err| anyhow!("配置 CODEX_API_BASE_URL 失败: {err}"))?;
        launch_codex_app(DEFAULT_CDP_PORT).await?;
        launched = true;
        wait_for_app_target(&client, DEFAULT_CDP_PORT).await?
    };
    chain_log::write_line(format!(
        "[codex_app_enhanced] event=target_ready elapsed_ms={} launched={} target_id={}",
        started.elapsed().as_millis(),
        launched,
        target.id
    ));

    let (target, status) = inject_until_ready(&client, DEFAULT_CDP_PORT, target, &models).await?;
    chain_log::write_line(format!(
        "[codex_app_enhanced] event=injection_applied elapsed_ms={} target_id={}",
        started.elapsed().as_millis(),
        target.id
    ));
    let startup_elapsed_ms = started.elapsed().as_millis() as u64;
    chain_log::write_line(format!(
        "[codex_app_enhanced] event=config_ready elapsed_ms={startup_elapsed_ms} i18n_enabled={} fast_initialize_applied={} fast_initialize_source={} routes_mounted={} renderer_ready_ms={} bootstrap_intercepted={} bootstrap_source={}",
        status.i18n_enabled,
        status.fast_initialize_applied,
        status.fast_initialize_source.as_deref().unwrap_or("none"),
        status.routes_mounted,
        status
            .renderer_ready_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        status.bootstrap_intercepted,
        status.bootstrap_source.as_deref().unwrap_or("none")
    ));

    Ok(EnhancedLaunchReport {
        port: DEFAULT_CDP_PORT,
        launched,
        target_id: target.id,
        available_models: status.available_models,
        use_hidden_models: status.use_hidden_models,
        key_gates_enabled: status.key_gates_enabled,
        i18n_enabled: status.i18n_enabled,
        fast_initialize_applied: status.fast_initialize_applied,
        fast_initialize_source: status.fast_initialize_source,
        bootstrap_intercepted: status.bootstrap_intercepted,
        bootstrap_source: status.bootstrap_source,
        routes_mounted: status.routes_mounted,
        renderer_ready_ms: status.renderer_ready_ms,
        startup_elapsed_ms,
    })
}

fn normalized_models(models: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty() && seen.insert(model.clone()))
        .collect()
}

async fn find_app_target(client: &reqwest::Client, port: u16) -> Result<Option<CdpTarget>> {
    let response = match client
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(_) | Err(_) => return Ok(None),
    };
    let targets = response.json::<Vec<CdpTarget>>().await?;
    Ok(targets.into_iter().find(|target| {
        target.target_type == "page"
            && target.url.starts_with("app://")
            && target.web_socket_debugger_url.is_some()
    }))
}

async fn wait_for_app_target(client: &reqwest::Client, port: u16) -> Result<CdpTarget> {
    let deadline = tokio::time::Instant::now() + CODEX_APP_READY_TIMEOUT;
    loop {
        if let Some(target) = find_app_target(client, port).await? {
            return Ok(target);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Codex App 未在 30 秒内开放本地 CDP 端口 {port}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn validated_websocket_url(target: &CdpTarget, expected_port: u16) -> Result<String> {
    let raw = target
        .web_socket_debugger_url
        .as_deref()
        .context("Codex CDP target 缺少 WebSocket 地址")?;
    let url = Url::parse(raw)?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if url.scheme() != "ws" || !loopback || url.port() != Some(expected_port) {
        bail!("拒绝连接非本机 Codex CDP 地址: {raw}");
    }
    Ok(raw.to_string())
}

fn same_cdp_target(left: &CdpTarget, right: &CdpTarget) -> bool {
    left.id == right.id && left.web_socket_debugger_url == right.web_socket_debugger_url
}

struct ActiveInjection {
    target: CdpTarget,
    socket: CdpSocket,
    command_id: u64,
    installed_at: Instant,
}

async fn connect_and_install(
    target: &CdpTarget,
    port: u16,
    source: &str,
) -> Result<ActiveInjection> {
    let websocket_url = validated_websocket_url(target, port)?;
    let (mut socket, _) = connect_async(websocket_url)
        .await
        .context("连接 Codex App CDP 失败")?;
    let mut command_id = 1_u64;
    for (method, params) in enhanced_script_install_commands(source) {
        let result = cdp_command(&mut socket, &mut command_id, method, params).await?;
        if method == "Runtime.evaluate" {
            ensure_runtime_evaluation_succeeded(&result)?;
        }
    }
    chain_log::write_line(format!(
        "[codex_app_enhanced] event=script_installed reload=false target_id={}",
        target.id
    ));
    Ok(ActiveInjection {
        target: target.clone(),
        socket,
        command_id,
        installed_at: Instant::now(),
    })
}

async fn reapply_script(active: &mut ActiveInjection, source: &str) -> Result<()> {
    let result = cdp_command(
        &mut active.socket,
        &mut active.command_id,
        "Runtime.evaluate",
        json!({
            "expression": source,
            "returnByValue": true,
        }),
    )
    .await?;
    ensure_runtime_evaluation_succeeded(&result)?;
    active.installed_at = Instant::now();
    chain_log::write_line(format!(
        "[codex_app_enhanced] event=script_retry target_id={}",
        active.target.id
    ));
    Ok(())
}

fn enhanced_script_install_commands(source: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "Runtime.evaluate",
            json!({
                "expression": source,
                "returnByValue": true,
            }),
        ),
        (
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source }),
        ),
    ]
}

fn ensure_runtime_evaluation_succeeded(result: &Value) -> Result<()> {
    let Some(exception) = result.get("exceptionDetails") else {
        return Ok(());
    };
    let message = exception
        .pointer("/exception/description")
        .and_then(Value::as_str)
        .or_else(|| exception.get("text").and_then(Value::as_str))
        .unwrap_or("unknown JavaScript error");
    bail!("Codex App 增强脚本执行失败: {message}")
}

fn retain_cdp_session(mut socket: CdpSocket) {
    let handle = tokio::spawn(async move {
        while let Some(message) = socket.next().await {
            match message {
                Ok(Message::Ping(payload)) => {
                    if socket.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    });
    let sessions = ENHANCED_CDP_SESSION.get_or_init(|| Mutex::new(None));
    let mut current = sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(previous) = current.replace(handle) {
        previous.abort();
    }
}

async fn cdp_command<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    command_id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let id = *command_id;
    *command_id += 1;
    socket
        .send(Message::Text(
            json!({ "id": id, "method": method, "params": params })
                .to_string()
                .into(),
        ))
        .await?;
    while let Some(message) = socket.next().await {
        let message = message?;
        let Message::Text(text) = message else {
            continue;
        };
        let response: Value = serde_json::from_str(&text)?;
        if response.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = response.get("error") {
            bail!("CDP {method} 失败: {error}");
        }
        return Ok(response.get("result").cloned().unwrap_or(Value::Null));
    }
    bail!("Codex App 在响应 CDP {method} 前关闭了连接")
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct InjectedStatus {
    script_installed: bool,
    script_applied: bool,
    script_version: Option<u64>,
    ready: bool,
    available_models: Vec<String>,
    use_hidden_models: bool,
    key_gates_enabled: usize,
    i18n_enabled: bool,
    fast_initialize_applied: bool,
    fast_initialize_source: Option<String>,
    bootstrap_intercepted: bool,
    bootstrap_source: Option<String>,
    routes_mounted: bool,
    renderer_ready_ms: Option<u64>,
}

async fn inject_until_ready(
    client: &reqwest::Client,
    port: u16,
    initial_target: CdpTarget,
    expected_models: &[String],
) -> Result<(CdpTarget, InjectedStatus)> {
    let source = enhanced_statsig_script(expected_models)?;
    let deadline = tokio::time::Instant::now() + ENHANCED_INJECTION_TIMEOUT;
    let mut initial_target = Some(initial_target);
    let mut active: Option<ActiveInjection> = None;
    let mut last_error = None::<String>;
    loop {
        let target = match initial_target.take() {
            Some(target) => Some(target),
            None => match find_app_target(client, port).await {
                Ok(target) => target,
                Err(err) => {
                    last_error = Some(err.to_string());
                    None
                }
            },
        };
        if let Some(target) = target {
            let replace_session = active
                .as_ref()
                .map(|current| !same_cdp_target(&current.target, &target))
                .unwrap_or(true);
            if replace_session {
                if let Some(current) = active.as_ref() {
                    chain_log::write_line(format!(
                        "[codex_app_enhanced] event=target_changed old_target_id={} new_target_id={}",
                        current.target.id, target.id
                    ));
                }
                active = None;
                match connect_and_install(&target, port, &source).await {
                    Ok(connection) => active = Some(connection),
                    Err(err) => last_error = Some(err.to_string()),
                }
            }

            if let Some(connection) = active.as_mut() {
                match inspect_injected_status_on_socket(connection).await {
                    Ok(status) if injected_status_is_ready(&status, expected_models) => {
                        let connection = active
                            .take()
                            .expect("active injection exists after status check");
                        retain_cdp_session(connection.socket);
                        return Ok((connection.target, status));
                    }
                    Ok(status)
                        if should_reapply_script(&status, connection.installed_at.elapsed()) =>
                    {
                        if let Err(err) = reapply_script(connection, &source).await {
                            last_error = Some(err.to_string());
                            active = None;
                        }
                    }
                    Ok(_) => {}
                    Err(err) => {
                        last_error = Some(err.to_string());
                        active = None;
                    }
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let detail = last_error
                .map(|error| format!("（最近一次 CDP 状态：{error}）"))
                .unwrap_or_default();
            bail!("Codex App 已启动，但增强配置（模型列表/语言）未在 45 秒内生效{detail}");
        }
        tokio::time::sleep(ENHANCED_STATUS_POLL_INTERVAL).await;
    }
}

fn should_reapply_script(status: &InjectedStatus, installed_for: Duration) -> bool {
    (!status.script_installed && installed_for >= Duration::from_secs(2))
        || installed_for >= ENHANCED_SCRIPT_RETRY_INTERVAL
}

fn injected_status_is_ready(status: &InjectedStatus, expected_models: &[String]) -> bool {
    // Renderer routes may mount well after Statsig is updated; the retained script
    // will keep the enhanced configuration active while the UI finishes loading.
    status.script_installed
        && status.script_applied
        && status.script_version == Some(ENHANCED_SCRIPT_VERSION)
        && status.ready
        && status.available_models == expected_models
        && !status.use_hidden_models
        && status.key_gates_enabled == SUPPORTED_FEATURE_GATES.len()
        && status.i18n_enabled
}

async fn inspect_injected_status_on_socket(active: &mut ActiveInjection) -> Result<InjectedStatus> {
    let gates = serde_json::to_string(SUPPORTED_FEATURE_GATES)?;
    let expression = format!(
        r#"(() => {{
           const client = window.__STATSIG__?.firstInstance;
           const enhanced = window.__CODEXHUB_ENHANCED_MODE__;
           const config = client?.getDynamicConfig?.("107580212")?.value;
          const i18n = client?.getLayer?.("72216192");
           const gates = {gates};
           return {{
             scriptInstalled: Boolean(enhanced?.installed),
             scriptApplied: Boolean(enhanced?.applied),
             scriptVersion: enhanced?.version ?? null,
             ready: Boolean(Array.isArray(config?.available_models)),
            availableModels: config?.available_models ?? [],
            useHiddenModels: config?.use_hidden_models ?? true,
            keyGatesEnabled: gates.filter((gate) => client?.checkGate?.(gate) === true).length,
            i18nEnabled: i18n?.get?.("enable_i18n", false) === true,
            fastInitializeApplied: Boolean(window.__CODEXHUB_ENHANCED_MODE__?.fastInitializeApplied),
            fastInitializeSource: window.__CODEXHUB_ENHANCED_MODE__?.fastInitializeSource ?? null,
            bootstrapIntercepted: Boolean(window.__CODEXHUB_ENHANCED_MODE__?.bootstrapIntercepted),
            bootstrapSource: window.__CODEXHUB_ENHANCED_MODE__?.bootstrapSource ?? null,
            routesMounted: Boolean(window.__CODEXHUB_ENHANCED_MODE__?.routesMounted),
            rendererReadyMs: window.__CODEXHUB_ENHANCED_MODE__?.routesMountedAtMs ?? null,
          }};
        }})()"#
    );
    let result = cdp_command(
        &mut active.socket,
        &mut active.command_id,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true }),
    )
    .await?;
    let value = result
        .pointer("/result/value")
        .cloned()
        .context("Codex CDP 状态响应缺少 value")?;
    Ok(serde_json::from_value(value)?)
}

fn enhanced_statsig_script(models: &[String]) -> Result<String> {
    let models = serde_json::to_string(models)?;
    let gates = serde_json::to_string(SUPPORTED_FEATURE_GATES)?;
    let legacy_gates = serde_json::to_string(LEGACY_CODEXHUB_FEATURE_GATES)?;
    Ok(format!(
        r#"(() => {{
  const MARKER = "__CODEXHUB_ENHANCED_MODE__";
  const SCRIPT_VERSION = 8;
  const MODELS = {models};
  const SUPPORTED_GATES = {gates};
  const LEGACY_CODEXHUB_GATES = {legacy_gates};
  const existing = window[MARKER];
  if (existing?.installed && existing.version === SCRIPT_VERSION) {{
      existing.update?.(MODELS);
      return;
  }}
  if (existing?.installed) existing.applying = true;
  const CONFIG_ID = "107580212";
  const state = {{
    installed: true,
    version: SCRIPT_VERSION,
    applied: false,
    attempts: 0,
    client: null,
    models: MODELS,
    bootstrapIntercepted: false,
    bootstrapInterceptedAtMs: null,
    bootstrapSource: null,
    fastInitializeApplied: false,
    fastInitializeAppliedAtMs: null,
    fastInitializeSource: null,
    fastInitializeError: null,
    i18nReactCacheInvalidated: 0,
    routesMounted: false,
    routesMountedAtMs: null,
  }};
  window[MARKER] = state;

  const patchValues = (values) => {{
    if (!values || typeof values !== "object") values = {{}};
    values.has_updates = true;
    values.time = Math.max(Number(values.time ?? 0), Date.now());
    values.feature_gates ??= {{}};
    values.dynamic_configs ??= {{}};
    values.layer_configs ??= {{}};
    values.values ??= {{}};
    values.param_stores ??= {{}};
    values.sdkParams ??= {{}};
    values.sdk_flags ??= {{}};
    const currentI18nLayer = values.layer_configs["72216192"];
    const currentI18nValue = currentI18nLayer?.value && typeof currentI18nLayer.value === "object"
      ? currentI18nLayer.value
      : currentI18nLayer?.v && values.values[currentI18nLayer.v]
        && typeof values.values[currentI18nLayer.v] === "object"
        ? values.values[currentI18nLayer.v]
        : values.values.codexhub_i18n_layer_config
        && typeof values.values.codexhub_i18n_layer_config === "object"
        ? values.values.codexhub_i18n_layer_config
        : {{}};
    const modelEntry = values.dynamic_configs[CONFIG_ID];
    const modelIsV2 = typeof modelEntry?.v === "string"
      && Object.prototype.hasOwnProperty.call(values.values, modelEntry.v)
      && !modelEntry?.value;
    const i18nIsV2 = typeof currentI18nLayer?.v === "string"
      && Object.prototype.hasOwnProperty.call(values.values, currentI18nLayer.v)
      && !currentI18nLayer?.value;
    const allEntries = [
      ...Object.values(values.feature_gates),
      ...Object.values(values.dynamic_configs),
      ...Object.values(values.layer_configs),
    ];
    const hasV1Entries = allEntries.some((entry) => entry
      && typeof entry === "object"
      && Object.prototype.hasOwnProperty.call(entry, "value"));
    const isV2 = !hasV1Entries && (values.response_format === "init-v2"
      || modelIsV2 && i18nIsV2);
    if (isV2) values.response_format = "init-v2";
    else if (values.response_format === "init-v2") delete values.response_format;
    for (const gate of LEGACY_CODEXHUB_GATES) {{
      const current = values.feature_gates[gate];
      if (current?.rule_id === "codexhub-local" || current?.r === "codexhub-local") {{
        delete values.feature_gates[gate];
      }}
    }}
    for (const gate of SUPPORTED_GATES) {{
      const current = values.feature_gates[gate];
      const next = {{
        ...(current && typeof current === "object" ? current : {{}}),
        name: gate,
        rule_id: "codexhub-local",
        r: "codexhub-local",
        secondary_exposures: current?.secondary_exposures ?? current?.s ?? [],
        s: current?.s ?? current?.secondary_exposures ?? [],
        version: current?.version ?? 1,
        id_type: current?.id_type ?? current?.i ?? "userID",
        i: current?.i ?? current?.id_type ?? "userID",
        value: true,
        v: true,
      }};
      if (isV2) delete next.value;
      else delete next.v;
      values.feature_gates[gate] = next;
    }}
    const current = values.dynamic_configs[CONFIG_ID];
    const modelValue = {{
      ...(current?.value && typeof current.value === "object" ? current.value : {{}}),
      ...(current?.v && values.values[current.v] && typeof values.values[current.v] === "object"
        ? values.values[current.v]
        : {{}}),
      available_models: state.models,
      default_model: state.models[0] ?? "gpt-5.6-sol",
      use_hidden_models: false,
    }};
    const nextConfig = {{
      ...(current && typeof current === "object" ? current : {{}}),
      name: CONFIG_ID,
      rule_id: "codexhub-local",
      r: "codexhub-local",
      secondary_exposures: current?.secondary_exposures ?? [],
      s: current?.s ?? current?.secondary_exposures ?? [],
      version: current?.version ?? 1,
      id_type: current?.id_type ?? current?.i ?? "userID",
      i: current?.i ?? current?.id_type ?? "userID",
      is_device_based: current?.is_device_based ?? false,
      passed: true,
    }};
    if (isV2) {{
      const modelKey = typeof current?.v === "string" ? current.v : "codexhub_model_list_config";
      nextConfig.v = modelKey;
      delete nextConfig.value;
      values.values[modelKey] = modelValue;
    }} else {{
      nextConfig.value = modelValue;
      delete nextConfig.v;
    }}
    values.dynamic_configs[CONFIG_ID] = nextConfig;
    const i18nValue = {{
      ...currentI18nValue,
      enable_i18n: true,
      locale_source: currentI18nValue.locale_source ?? "FIRST_AVAILABLE",
    }};
    const nextI18nLayer = {{
      ...(currentI18nLayer && typeof currentI18nLayer === "object" ? currentI18nLayer : {{}}),
      name: "72216192",
      rule_id: "codexhub-local",
      r: "codexhub-local",
      secondary_exposures: currentI18nLayer?.secondary_exposures ?? currentI18nLayer?.s ?? [],
      s: currentI18nLayer?.s ?? currentI18nLayer?.secondary_exposures ?? [],
      id_type: currentI18nLayer?.id_type ?? currentI18nLayer?.i ?? "userID",
      i: currentI18nLayer?.i ?? currentI18nLayer?.id_type ?? "userID",
      passed: true,
    }};
    if (isV2) {{
      const layerKey = typeof currentI18nLayer?.v === "string"
        ? currentI18nLayer.v
        : "codexhub_i18n_layer_config";
      nextI18nLayer.v = layerKey;
      delete nextI18nLayer.value;
      values.values[layerKey] = i18nValue;
    }} else {{
      nextI18nLayer.value = i18nValue;
      delete nextI18nLayer.v;
    }}
    values.layer_configs["72216192"] = nextI18nLayer;
    return values;
  }};

  const isObject = (value) => value !== null
    && typeof value === "object"
    && !Array.isArray(value);
  const isCodexHubLocalEntry = (entry) => isObject(entry)
    && (entry.rule_id === "codexhub-local" || entry.r === "codexhub-local");
  const statsigEntryValue = (values, entry) => {{
    if (!isObject(entry)) return null;
    if (isObject(entry.value)) return entry.value;
    if (typeof entry.v === "string" && isObject(values.values?.[entry.v])) {{
      return values.values[entry.v];
    }}
    return null;
  }};
  // A local compatibility payload is not a substitute for the official evaluation.
  // Only a structurally complete evaluation with at least one official entry may be
  // used as the fast-start base.
  const isCompleteOfficialValues = (values) => {{
    if (!isObject(values)
      || !isObject(values.feature_gates)
      || !isObject(values.dynamic_configs)
      || !isObject(values.layer_configs)) return false;
    const entries = [
      ...Object.values(values.feature_gates),
      ...Object.values(values.dynamic_configs),
      ...Object.values(values.layer_configs),
    ];
    const hasOfficialEntry = entries.some((entry) => isObject(entry)
      && !isCodexHubLocalEntry(entry));
    if (!hasOfficialEntry) return false;
    const valueCount = isObject(values.values) ? Object.keys(values.values).length : 0;
    const sectionCount = Object.keys(values.feature_gates).length
      + Object.keys(values.dynamic_configs).length
      + Object.keys(values.layer_configs).length;
    const runtimeValue = statsigEntryValue(values, values.layer_configs["2096615506"]);
    const hasRuntimeConfig = isObject(runtimeValue) && Object.keys(runtimeValue).length > 0;
    const hasEvaluationMetadata = isObject(values.evaluated_keys)
      || isObject(values.sdkParams)
      || isObject(values.sdk_flags);
    return hasRuntimeConfig || hasEvaluationMetadata || valueCount > 0 || sectionCount >= 4;
  }};

  const STORE_PATCH_MARKER = "__CODEXHUB_ENHANCED_SET_VALUES_PATCH__";
  const installStorePatch = (client) => {{
    const store = client?._store;
    if (!store || typeof store.setValues !== "function") return false;
    const installed = store[STORE_PATCH_MARKER];
    if (installed) {{
      installed.patch = patchValues;
      return true;
    }}
    const control = {{
      original: store.setValues.bind(store),
      patch: patchValues,
    }};
    store.setValues = (packet, user) => {{
      if (!packet || typeof packet.data !== "string") return control.original(packet, user);
      try {{
        const values = control.patch(JSON.parse(packet.data));
        packet = {{ ...packet, data: JSON.stringify(values) }};
      }} catch {{}}
      return control.original(packet, user);
    }};
    store[STORE_PATCH_MARKER] = control;
    return true;
  }};

  const FAST_INITIALIZE_PATCH_MARKER = "__CODEXHUB_ENHANCED_FAST_INITIALIZE_PATCH__";
  const installFastInitializePatch = (client) => {{
    if (!client
      || typeof client.initializeAsync !== "function"
      || typeof client.initializeSync !== "function"
      || typeof client.dataAdapter?.setData !== "function") return false;
    if (client[FAST_INITIALIZE_PATCH_MARKER]) return true;
    const control = {{
      original: client.initializeAsync.bind(client),
      promise: null,
    }};
    client.initializeAsync = (options) => {{
      if (control.promise) return control.promise;
      try {{
        const bootstrapPayload = buildBootstrapPayload();
        if (!bootstrapPayload) {{
          state.fastInitializeSource = "official-network";
          try {{
            const details = control.original(options);
            control.promise = Promise.resolve(details).then((result) => {{
              try {{ applyClient(client); }} catch (error) {{
                state.fastInitializeError = String(error?.stack ?? error);
              }}
              return result;
            }});
          }} catch (error) {{
            state.fastInitializeError = String(error?.stack ?? error);
            control.promise = Promise.reject(error);
          }}
          return control.promise;
        }}
        const values = JSON.parse(bootstrapPayload);
        const currentUser = client.getContext?.().user ?? client._user;
        if (currentUser && typeof currentUser === "object") {{
          values.user = structuredClone(currentUser);
        }}
        client.dataAdapter.setData(JSON.stringify(values));
        const details = client.initializeSync({{ disableBackgroundCacheRefresh: true }});
        if (client.loadingStatus !== "Ready") {{
          throw new Error(`local Statsig initialization ended in ${{client.loadingStatus}}`);
        }}
        state.fastInitializeApplied = true;
        state.fastInitializeAppliedAtMs = Math.round(performance.now());
        state.fastInitializeSource = state.bootstrapSource ?? "statsig-cache-official";
        control.promise = Promise.resolve(details);
      }} catch (error) {{
        state.fastInitializeError = String(error?.stack ?? error);
        control.promise = control.original(options);
      }}
      return control.promise;
    }};
    client[FAST_INITIALIZE_PATCH_MARKER] = control;
    return true;
  }};

  const installClientPatches = (client) => {{
    installStorePatch(client);
    installFastInitializePatch(client);
  }};

  const installStatsigClientPatches = (statsig) => {{
    installClientPatches(statsig?.firstInstance);
    for (const client of Object.values(statsig?.instances ?? {{}})) {{
      installClientPatches(client);
    }}
  }};

  const hookFirstInstance = (statsig) => {{
    if (!statsig || typeof statsig !== "object") return;
    const marker = "__CODEXHUB_ENHANCED_FIRST_INSTANCE_HOOK__";
    if (statsig[marker]) {{
      installStatsigClientPatches(statsig);
      return;
    }}
    const descriptor = Object.getOwnPropertyDescriptor(statsig, "firstInstance");
    if (descriptor?.configurable === false) {{
      installStatsigClientPatches(statsig);
      return;
    }}
    let current = statsig.firstInstance;
    Object.defineProperty(statsig, "firstInstance", {{
      configurable: true,
      enumerable: true,
      get: () => current,
      set: (client) => {{
        current = client;
        installClientPatches(client);
      }},
    }});
    statsig[marker] = true;
    installStatsigClientPatches(statsig);
  }};

  const hookStatsigGlobal = () => {{
    const marker = "__CODEXHUB_ENHANCED_GLOBAL_HOOK__";
    const descriptor = Object.getOwnPropertyDescriptor(window, "__STATSIG__");
    if (descriptor?.get?.[marker]) {{
      hookFirstInstance(window.__STATSIG__);
      return;
    }}
    if (descriptor?.configurable === false) {{
      hookFirstInstance(window.__STATSIG__);
      return;
    }}
    let current = window.__STATSIG__;
    const getStatsig = () => current;
    getStatsig[marker] = true;
    Object.defineProperty(window, "__STATSIG__", {{
      configurable: true,
      enumerable: descriptor?.enumerable ?? true,
      get: getStatsig,
      set: (statsig) => {{
        current = statsig;
        hookFirstInstance(statsig);
      }},
    }});
    hookFirstInstance(current);
  }};

  const patchSerialized = (raw) => {{
    if (typeof raw !== "string") return raw;
    try {{
      const parsed = JSON.parse(raw);
      const visit = (value, depth = 0) => {{
        if (depth > 10 || value == null) return value;
        if (typeof value === "string") {{
          const trimmed = value.trim();
          if (!trimmed.startsWith("{{") && !trimmed.startsWith("[")) return value;
          try {{ return JSON.stringify(visit(JSON.parse(value), depth + 1)); }} catch {{ return value; }}
        }}
        if (Array.isArray(value)) return value.map((item) => visit(item, depth + 1));
        if (typeof value !== "object") return value;
        if (isCompleteOfficialValues(value)
          || value.dynamic_configs?.[CONFIG_ID]
          || value.layer_configs?.["72216192"]) {{
          patchValues(value);
        }}
        for (const key of Object.keys(value)) value[key] = visit(value[key], depth + 1);
        return value;
      }};
      return JSON.stringify(visit(parsed));
    }} catch {{ return raw; }}
  }};

  const originalGetItem = Storage.prototype.getItem;

  const localUser = (values, envelope) => {{
    const evaluatedCustomIDs = values?.evaluated_keys?.customIDs;
    const stableIDKey = Object.keys(localStorage)
      .find((key) => key.startsWith("statsig.stable_id."));
    const stableID = envelope?.stableID
      ?? evaluatedCustomIDs?.stableID
      ?? (stableIDKey ? originalGetItem.call(localStorage, stableIDKey) : undefined)
      ?? undefined;
    return {{
      userID: "user_codexhub_local",
      email: "codexhub-local@example.local",
      locale: navigator.language || "en",
      customIDs: {{
        ...(evaluatedCustomIDs && typeof evaluatedCustomIDs === "object" ? evaluatedCustomIDs : {{}}),
        ...(stableID ? {{ stableID, source_surface_stable_id: stableID }} : {{}}),
        account_id: "acct_codexhub_local",
      }},
      custom: {{
        auth_status: "logged_in",
        auth_method: "chatgpt",
        account_id: "acct_codexhub_local",
        plan_type: "pro",
        brand_name: "codex",
      }},
    }};
  }};

  const selectOfficialBootstrap = () => {{
    let selected = null;
    for (const key of Object.keys(localStorage)) {{
      if (!key.startsWith("statsig.cached.evaluations.")) continue;
      try {{
        const envelope = JSON.parse(originalGetItem.call(localStorage, key));
        const values = JSON.parse(envelope?.data);
        if (!isCompleteOfficialValues(values)) continue;
        const receivedAt = Number(envelope.receivedAt ?? values.time ?? 0);
        const evaluatedUserID = values?.evaluated_keys?.userID;
        const priority = evaluatedUserID == null
          ? 2
          : evaluatedUserID === "user_codexhub_local" ? 1 : 0;
        if (!selected
          || priority > selected.priority
          || (priority === selected.priority && receivedAt > selected.receivedAt)) {{
          selected = {{ envelope, values, receivedAt, priority }};
        }}
      }} catch {{}}
    }}
    return selected;
  }};

  const buildBootstrapPayload = () => {{
    const selected = selectOfficialBootstrap();
    if (!selected) {{
      state.bootstrapSource = "official-network";
      return null;
    }}
    const values = patchValues(structuredClone(selected.values));
    values.user = localUser(values, selected?.envelope);
    state.bootstrapSource = selected.priority === 2
      ? "statsig-cache-prelogin"
      : selected.priority === 1 ? "statsig-cache-local" : "statsig-cache-other";
    return JSON.stringify(values);
  }};

  const dispatchFetchResponse = (request, body) => {{
    window.dispatchEvent(new MessageEvent("message", {{
      source: window,
      origin: window.location.origin,
      data: {{
        type: "fetch-response",
        requestId: request.requestId,
        responseType: "success",
        status: 200,
        headers: {{}},
        bodyJsonString: JSON.stringify(body),
      }},
    }}));
  }};

  window.addEventListener("codex-message-from-view", (event) => {{
    const message = event?.detail;
    if (message?.type === "ready") {{
      state.routesMounted = true;
      state.routesMountedAtMs = Math.round(performance.now());
      return;
    }}
    if (message?.type !== "fetch"
      || String(message.method).toUpperCase() !== "POST"
      || message.url !== "/wham/statsig/bootstrap") return;
    try {{
      const payload = buildBootstrapPayload();
      if (!payload) return;
      dispatchFetchResponse(message, {{ statsigPayload: payload }});
      state.bootstrapIntercepted = true;
      state.bootstrapInterceptedAtMs = Math.round(performance.now());
    }} catch (error) {{
      state.bootstrapSource = "intercept-error";
      state.bootstrapError = String(error?.stack ?? error);
    }}
  }});

  Storage.prototype.getItem = function(key) {{
    const raw = originalGetItem.call(this, key);
    return typeof key === "string" && key.startsWith("statsig.cached.evaluations.")
      ? patchSerialized(raw)
      : raw;
  }};

  const patchCachedI18nLayer = (layer, value) => {{
    if (!layer || typeof layer !== "object") return;
    const marker = "__CODEXHUB_I18N_VALUE__";
    let control = layer[marker];
    if (!control) {{
      control = {{ fallback: typeof layer.get === "function" ? layer.get.bind(layer) : null, value }};
      layer[marker] = control;
      layer.get = (key, fallback) => Object.prototype.hasOwnProperty.call(control.value, key)
        ? control.value[key]
        : control.fallback?.(key, fallback) ?? fallback;
    }}
    control.value = value;
    layer.__value = value;
  }};

  const invalidateI18nReactCache = (i18nValue) => {{
    const rootElement = document.getElementById("root");
    const containerKey = Object.getOwnPropertyNames(rootElement ?? {{}})
      .find((key) => key.startsWith("__reactContainer"));
    const container = containerKey ? rootElement?.[containerKey] : null;
    const rootFiber = container?.stateNode?.current ?? container;
    if (!rootFiber) return 0;

    const stack = [rootFiber];
    const seenFibers = new Set();
    const seenRows = new Set();
    let invalidated = 0;
    while (stack.length > 0) {{
      const fiber = stack.pop();
      if (!fiber || seenFibers.has(fiber)) continue;
      seenFibers.add(fiber);
      for (const candidate of [fiber, fiber.alternate]) {{
        const rows = candidate?.updateQueue?.memoCache?.data;
        if (!Array.isArray(rows)) continue;
        for (const row of rows) {{
          if (!Array.isArray(row) || seenRows.has(row)) continue;
          seenRows.add(row);
          for (let index = 0; index < row.length - 1; index += 1) {{
            const layer = row[index];
            if (layer?.name !== "72216192" || typeof layer.get !== "function") continue;
            patchCachedI18nLayer(layer, i18nValue);
            if (typeof row[index + 1] === "boolean" && row[index + 1] !== true) {{
              row[index] = null;
              invalidated += 1;
            }}
          }}
        }}
      }}
      if (fiber.sibling) stack.push(fiber.sibling);
      if (fiber.child) stack.push(fiber.child);
    }}
    state.i18nReactCacheInvalidated =
      (state.i18nReactCacheInvalidated ?? 0) + invalidated;
    return invalidated;
  }};

  const applyClient = (client) => {{
    if (!client?._store?.setValues || typeof client._finalizeUpdate !== "function") return false;
    installStorePatch(client);
    const stored = client._store.getValues?.();
    if (!isCompleteOfficialValues(stored)) {{
      state.bootstrapSource ??= "official-network";
      return false;
    }}
    const current = stored;
    current.feature_gates ??= {{}};
    current.dynamic_configs ??= {{}};
    current.layer_configs ??= {{}};
    current.values ??= {{}};
    const currentConfigEntry = current.dynamic_configs[CONFIG_ID];
    const currentConfig = currentConfigEntry?.v
      && current.values?.[currentConfigEntry.v]
      && typeof current.values[currentConfigEntry.v] === "object"
      ? current.values[currentConfigEntry.v]
      : currentConfigEntry?.value;
    const currentI18nEntry = current.layer_configs["72216192"];
    const currentI18n = currentI18nEntry?.v
      && current.values?.[currentI18nEntry.v]
      && typeof current.values[currentI18nEntry.v] === "object"
      ? current.values[currentI18nEntry.v]
      : currentI18nEntry?.value;
    const gateEnabled = (gate) => current.feature_gates?.[gate]?.v === true
      || current.feature_gates?.[gate]?.value === true;
    const alreadyApplied = currentConfig?.use_hidden_models === false
      && JSON.stringify(currentConfig.available_models) === JSON.stringify(state.models)
      && currentI18n?.enable_i18n === true
      && SUPPORTED_GATES.every(gateEnabled);
    const cachedI18nLayer = client.getLayer?.("72216192");
    const publicI18nEnabled = cachedI18nLayer?.get?.("enable_i18n", false) === true;
    const next = patchValues(structuredClone(current));
    const nextI18nEntry = next.layer_configs["72216192"];
    const nextI18nValue = nextI18nEntry?.value
      ?? next.values?.[nextI18nEntry?.v]
      ?? {{ enable_i18n: true, locale_source: "FIRST_AVAILABLE" }};
    patchCachedI18nLayer(cachedI18nLayer, nextI18nValue);
    const invalidatedI18nCache = state.i18nReactCacheInvalidated > 0
      ? 0
      : invalidateI18nReactCache(nextI18nValue);
    if (!alreadyApplied || !publicI18nEnabled || invalidatedI18nCache > 0) {{
      const packet = {{
        data: JSON.stringify(next),
        source: ["Loading", "NoValues", "Uninitialized"].includes(client._store.getSource?.())
          ? "Bootstrap"
          : client._store.getSource?.() ?? "Bootstrap",
        receivedAt: Date.now(),
      }};
      if (!client._store.setValues(packet, client._user)) return false;
      client._finalizeUpdate(packet);
    }}
    const resolvedConfig = client.getDynamicConfig?.(CONFIG_ID)?.value;
    const resolvedI18n = client.getLayer?.("72216192");
    state.applied = resolvedConfig?.use_hidden_models === false
      && JSON.stringify(resolvedConfig.available_models) === JSON.stringify(state.models)
      && resolvedI18n?.get?.("enable_i18n", false) === true
      && SUPPORTED_GATES.every((gate) => client.checkGate?.(gate) === true);
    state.client = client;
    return state.applied;
  }};

  const attach = () => {{
    state.attempts += 1;
    const client = window.__STATSIG__?.firstInstance;
    if (!client) {{
      if (state.attempts < 300) setTimeout(attach, state.attempts < 80 ? 25 : 100);
      return;
    }}
    installStorePatch(client);
    if (client.__CODEXHUB_ENHANCED_LISTENER_VERSION__ !== SCRIPT_VERSION) {{
      const listener = () => {{
        if (state.applying) return;
        state.applying = true;
        try {{
          if (!applyClient(client)) queueMicrotask(attach);
        }} finally {{ state.applying = false; }}
      }};
      state.listener = listener;
      client.__CODEXHUB_ENHANCED_LISTENER_VERSION__ = SCRIPT_VERSION;
      client.on?.("values_updated", listener);
    }}
    if (!applyClient(client) && state.attempts < 300) {{
      setTimeout(attach, state.attempts < 80 ? 25 : 100);
    }}
  }};
  state.update = (models) => {{
    state.models = models;
    state.applied = false;
    state.attempts = 0;
    queueMicrotask(attach);
  }};
  hookStatsigGlobal();
  queueMicrotask(attach);
}})();"#
    ))
}

async fn launch_codex_app(port: u16) -> Result<()> {
    tokio::task::spawn_blocking(move || launch_codex_app_blocking(port))
        .await
        .context("启动 Codex App 任务失败")??;
    Ok(())
}

#[cfg(target_os = "windows")]
const CODEX_APP_USER_MODEL_ID: &str = "OpenAI.Codex_2p2nqsd0c76g0!App";

#[cfg(target_os = "windows")]
const IID_IAPPLICATION_ACTIVATION_MANAGER: windows_sys::core::GUID =
    windows_sys::core::GUID::from_u128(0x2e941141_7f97_4756_ba1d_9decde894a3d);

#[cfg(target_os = "windows")]
#[repr(C)]
struct ApplicationActivationManagerInterface {
    vtable: *const ApplicationActivationManagerVtable,
}

#[cfg(target_os = "windows")]
#[allow(dead_code)]
#[repr(C)]
struct ApplicationActivationManagerVtable {
    query_interface: unsafe extern "system" fn(
        *mut ApplicationActivationManagerInterface,
        *const windows_sys::core::GUID,
        *mut *mut std::ffi::c_void,
    ) -> windows_sys::core::HRESULT,
    add_ref: unsafe extern "system" fn(*mut ApplicationActivationManagerInterface) -> u32,
    release: unsafe extern "system" fn(*mut ApplicationActivationManagerInterface) -> u32,
    activate_application: unsafe extern "system" fn(
        *mut ApplicationActivationManagerInterface,
        *const u16,
        *const u16,
        u32,
        *mut u32,
    ) -> windows_sys::core::HRESULT,
}

#[cfg(target_os = "windows")]
struct ComApartmentGuard {
    should_uninitialize: bool,
}

#[cfg(target_os = "windows")]
impl Drop for ComApartmentGuard {
    fn drop(&mut self) {
        if self.should_uninitialize {
            unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
        }
    }
}

#[cfg(target_os = "windows")]
struct ApplicationActivationManagerHandle(*mut ApplicationActivationManagerInterface);

#[cfg(target_os = "windows")]
impl Drop for ApplicationActivationManagerHandle {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        let vtable = unsafe { (*self.0).vtable };
        if !vtable.is_null() {
            unsafe { ((*vtable).release)(self.0) };
        }
    }
}

#[cfg(target_os = "windows")]
fn initialize_com_apartment() -> Result<ComApartmentGuard> {
    use windows_sys::Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
    };

    let result = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
    if result < 0 && result != RPC_E_CHANGED_MODE {
        bail!("Windows COM 初始化失败（HRESULT 0x{:08X}）", result as u32);
    }
    Ok(ComApartmentGuard {
        should_uninitialize: result >= 0,
    })
}

#[cfg(target_os = "windows")]
fn create_application_activation_manager() -> Result<ApplicationActivationManagerHandle> {
    use windows_sys::Win32::{
        System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance},
        UI::Shell::ApplicationActivationManager,
    };

    let mut raw = std::ptr::null_mut();
    let result = unsafe {
        CoCreateInstance(
            &ApplicationActivationManager,
            std::ptr::null_mut(),
            CLSCTX_LOCAL_SERVER,
            &IID_IAPPLICATION_ACTIVATION_MANAGER,
            &mut raw,
        )
    };
    if result < 0 || raw.is_null() {
        bail!(
            "Windows 应用激活服务不可用（HRESULT 0x{:08X}）",
            result as u32
        );
    }
    Ok(ApplicationActivationManagerHandle(raw.cast()))
}

#[cfg(target_os = "windows")]
fn wide_null_terminated(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn codex_app_activation_arguments(port: u16) -> String {
    format!("--remote-debugging-address=127.0.0.1 --remote-debugging-port={port}")
}

#[cfg(target_os = "windows")]
fn launch_codex_app_blocking(port: u16) -> Result<()> {
    let _apartment = initialize_com_apartment()?;
    let manager = create_application_activation_manager()?;
    let app_id = wide_null_terminated(CODEX_APP_USER_MODEL_ID);
    let arguments = wide_null_terminated(&codex_app_activation_arguments(port));
    let mut process_id = 0_u32;
    let vtable = unsafe { (*manager.0).vtable };
    if vtable.is_null() {
        bail!("Windows 应用激活服务返回了无效接口");
    }
    let result = unsafe {
        ((*vtable).activate_application)(
            manager.0,
            app_id.as_ptr(),
            arguments.as_ptr(),
            0,
            &mut process_id,
        )
    };
    if result < 0 {
        bail!(
            "Windows 无法激活 Codex App 商店包 {CODEX_APP_USER_MODEL_ID}（HRESULT 0x{:08X}）",
            result as u32
        );
    }
    chain_log::write_line(format!(
        "[codex_app_enhanced] event=windows_native_activation process_id={process_id} port={port}"
    ));
    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_codex_app_blocking(port: u16) -> Result<()> {
    let arguments = format!("--remote-debugging-address=127.0.0.1 --remote-debugging-port={port}");
    for app_name in ["Codex", "ChatGPT"] {
        let status = Command::new("open")
            .args(["-na", app_name, "--args"])
            .args(arguments.split_whitespace())
            .status()?;
        if status.success() {
            return Ok(());
        }
    }
    bail!("macOS 无法定位 Codex App")
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn launch_codex_app_blocking(_port: u16) -> Result<()> {
    bail!("增强模式启动当前仅支持 Windows 和 macOS")
}

#[cfg(target_os = "windows")]
fn codex_app_is_running() -> Result<bool> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let output = Command::new("tasklist.exe")
        .args(["/FI", "IMAGENAME eq ChatGPT.exe", "/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).contains("ChatGPT.exe"))
}

#[cfg(target_os = "macos")]
fn codex_app_is_running() -> Result<bool> {
    for name in ["Codex", "ChatGPT"] {
        if Command::new("pgrep").args(["-x", name]).status()?.success() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn codex_app_is_running() -> Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enhanced_script_embeds_models_and_only_patches_selected_statsig_sections() {
        let script =
            enhanced_statsig_script(&["gpt-5.6-sol".into(), "grok-4.5".into()]).expect("script");
        assert!(script.contains("gpt-5.6-sol"));
        assert!(script.contains("grok-4.5"));
        assert!(script.contains("107580212"));
        assert!(script.contains("3446105535"));
        assert!(script.contains("72216192"));
        assert!(script.contains("enable_i18n: true"));
        assert!(script.contains("response_format = \"init-v2\""));
        assert!(script.contains("const modelIsV2"));
        assert!(script.contains("installStorePatch"));
        assert!(script.contains("patchCachedI18nLayer"));
        assert!(script.contains("SCRIPT_VERSION = 8"));
        assert!(script.contains("isCompleteOfficialValues"));
        assert!(script.contains("selectOfficialBootstrap"));
        assert!(script.contains("state.fastInitializeSource = \"official-network\""));
        assert!(script.contains("installFastInitializePatch"));
        assert!(script.contains("client.initializeSync({ disableBackgroundCacheRefresh: true })"));
        assert!(script.contains("fastInitializeAppliedAtMs"));
        assert!(script.contains("isCompleteOfficialValues(stored)"));
        assert!(script.contains("const current = stored"));
        assert!(!script.contains("JSON.parse(buildBootstrapPayload())"));
        assert!(!script.contains("codexhub-minimal-store"));
        assert!(script.contains("invalidateI18nReactCache"));
        assert!(script.contains("__reactContainer"));
        assert!(script.contains("memoCache?.data"));
        assert!(script.contains("invalidatedI18nCache > 0"));
        assert!(script.contains("const hasV1Entries"));
        assert!(script.contains("delete values.response_format"));
        assert!(script.contains("delete nextConfig.value"));
        assert!(script.contains("delete nextConfig.v"));
        assert!(script.contains("LEGACY_CODEXHUB_GATES"));
        assert!(script.contains("current?.rule_id === \"codexhub-local\""));
        assert!(script.contains("delete values.feature_gates[gate]"));
        assert!(!script.contains("state.gates.every"));
        assert!(script.contains("/wham/statsig/bootstrap"));
        assert!(script.contains("codex-message-from-view"));
        assert!(script.contains("routesMountedAtMs"));
        assert!(script.contains("values.layer_configs[\"72216192\"]"));
    }

    #[test]
    fn enhanced_install_executes_in_place_without_reloading_codex_app() {
        let commands = enhanced_script_install_commands("window.testEnhanced = true;");
        let methods = commands
            .iter()
            .map(|(method, _)| *method)
            .collect::<Vec<_>>();

        assert_eq!(
            methods,
            vec!["Runtime.evaluate", "Page.addScriptToEvaluateOnNewDocument"]
        );
        assert!(!methods.contains(&"Page.reload"));
        assert_eq!(commands[0].1["expression"], "window.testEnhanced = true;");
    }

    #[test]
    fn enhanced_install_reports_runtime_script_exceptions() {
        let result = json!({
            "exceptionDetails": {
                "text": "Uncaught",
                "exception": { "description": "TypeError: failed" }
            }
        });

        let error = ensure_runtime_evaluation_succeeded(&result).unwrap_err();
        assert!(error.to_string().contains("TypeError: failed"));
    }

    #[test]
    fn normalized_models_trims_and_deduplicates() {
        assert_eq!(
            normalized_models(vec![" grok-4.5 ".into(), "".into(), "grok-4.5".into()]),
            vec!["grok-4.5"]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_launcher_builds_native_activation_inputs() {
        assert_eq!(CODEX_APP_USER_MODEL_ID, "OpenAI.Codex_2p2nqsd0c76g0!App");
        assert_eq!(
            codex_app_activation_arguments(9335),
            "--remote-debugging-address=127.0.0.1 --remote-debugging-port=9335"
        );
        let encoded = wide_null_terminated("Codex");
        assert_eq!(encoded.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&encoded[..encoded.len() - 1]).unwrap(),
            "Codex"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires an interactive Windows shell COM service"]
    fn windows_native_activation_manager_is_available() {
        let _apartment = initialize_com_apartment().expect("initialize COM apartment");
        let manager = create_application_activation_manager().expect("create activation manager");
        assert!(!manager.0.is_null());
    }

    #[test]
    fn enhanced_config_ready_does_not_require_renderer_routes_or_bootstrap_interception() {
        let expected_models = vec!["gpt-5.6-sol".to_string()];
        let mut status = InjectedStatus {
            script_installed: true,
            script_applied: true,
            script_version: Some(ENHANCED_SCRIPT_VERSION),
            ready: true,
            available_models: expected_models.clone(),
            use_hidden_models: false,
            key_gates_enabled: SUPPORTED_FEATURE_GATES.len(),
            i18n_enabled: true,
            fast_initialize_applied: false,
            fast_initialize_source: None,
            bootstrap_intercepted: false,
            bootstrap_source: None,
            routes_mounted: false,
            renderer_ready_ms: None,
        };

        assert!(injected_status_is_ready(&status, &expected_models));
        status.i18n_enabled = false;
        assert!(!injected_status_is_ready(&status, &expected_models));
    }

    #[test]
    fn cold_start_retries_after_the_script_attempt_window() {
        let status = InjectedStatus {
            script_installed: true,
            script_applied: false,
            script_version: Some(ENHANCED_SCRIPT_VERSION),
            ready: false,
            available_models: Vec::new(),
            use_hidden_models: true,
            key_gates_enabled: 0,
            i18n_enabled: false,
            fast_initialize_applied: false,
            fast_initialize_source: None,
            bootstrap_intercepted: false,
            bootstrap_source: None,
            routes_mounted: false,
            renderer_ready_ms: None,
        };

        assert!(!should_reapply_script(&status, Duration::from_secs(2)));
        assert!(should_reapply_script(
            &status,
            ENHANCED_SCRIPT_RETRY_INTERVAL
        ));
    }

    #[test]
    fn target_replacement_is_detected_even_when_id_is_reused() {
        let first = CdpTarget {
            id: "renderer".into(),
            target_type: "page".into(),
            url: "app://-/index.html".into(),
            web_socket_debugger_url: Some("ws://127.0.0.1:9335/devtools/page/first".into()),
        };
        let replacement = CdpTarget {
            web_socket_debugger_url: Some("ws://127.0.0.1:9335/devtools/page/replacement".into()),
            ..first.clone()
        };

        assert!(same_cdp_target(&first, &first));
        assert!(!same_cdp_target(&first, &replacement));
    }

    #[tokio::test]
    #[ignore = "requires a Codex App renderer already listening on CDP port 9335"]
    async fn live_injects_models_into_codex_app() {
        let preflight = preflight().await.expect("live preflight");
        assert!(preflight.running);
        let models = [
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.4-mini",
            "grok-4.5",
            "deepseek-v4-pro",
            "deepseek-v4-flash",
            "GLM-5.2",
            "Opus-4.8",
            "Sonnet-4.6",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let report = launch_and_inject(models, "http://127.0.0.1:3847/backend-api")
            .await
            .expect("live enhanced injection");
        assert_eq!(report.available_models.len(), 12);
        assert!(!report.use_hidden_models);
        assert_eq!(report.key_gates_enabled, SUPPORTED_FEATURE_GATES.len());
        assert!(report.i18n_enabled);
        let repeated = launch_and_inject(
            report.available_models.clone(),
            "http://127.0.0.1:3847/backend-api",
        )
        .await
        .expect("repeat live enhanced injection");
        assert_eq!(repeated.available_models, report.available_models);
        assert_eq!(repeated.key_gates_enabled, SUPPORTED_FEATURE_GATES.len());
        assert!(repeated.i18n_enabled);
    }
}

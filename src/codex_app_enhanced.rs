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
type CdpSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
static ENHANCED_CDP_SESSION: OnceLock<Mutex<Option<tokio::task::JoinHandle<()>>>> = OnceLock::new();
const KEY_FEATURE_GATES: &[&str] = &[
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

#[derive(Debug, serde::Deserialize)]
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
            None => bail!("Codex App 正在运行。请先完全退出，再使用增强模式启动"),
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

    inject_target(&target, DEFAULT_CDP_PORT, &models).await?;
    chain_log::write_line(format!(
        "[codex_app_enhanced] event=injection_applied elapsed_ms={} target_id={}",
        started.elapsed().as_millis(),
        target.id
    ));
    let status = wait_for_injected_status(&client, DEFAULT_CDP_PORT, &target.id, &models).await?;
    let startup_elapsed_ms = started.elapsed().as_millis() as u64;
    chain_log::write_line(format!(
        "[codex_app_enhanced] event=config_ready elapsed_ms={startup_elapsed_ms} routes_mounted={} renderer_ready_ms={} bootstrap_intercepted={} bootstrap_source={}",
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
        tokio::time::sleep(Duration::from_millis(300)).await;
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

async fn inject_target(target: &CdpTarget, port: u16, models: &[String]) -> Result<()> {
    let websocket_url = validated_websocket_url(target, port)?;
    let (mut socket, _) = connect_async(websocket_url)
        .await
        .context("连接 Codex App CDP 失败")?;
    let source = enhanced_statsig_script(models)?;
    let mut command_id = 1_u64;
    cdp_command(&mut socket, &mut command_id, "Page.enable", json!({})).await?;
    cdp_command(
        &mut socket,
        &mut command_id,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": source }),
    )
    .await?;
    cdp_command(
        &mut socket,
        &mut command_id,
        "Page.reload",
        json!({ "ignoreCache": false }),
    )
    .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let applied = cdp_command(
            &mut socket,
            &mut command_id,
            "Runtime.evaluate",
            json!({
                "expression": "Boolean(window.__CODEXHUB_ENHANCED_MODE__?.applied)",
                "returnByValue": true
            }),
        )
        .await
        .ok()
        .and_then(|result| result.pointer("/result/value").and_then(Value::as_bool))
        .unwrap_or(false);
        if applied {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Codex App renderer 已刷新，但增强模型配置未能生效");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    retain_cdp_session(socket);
    Ok(())
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
    ready: bool,
    available_models: Vec<String>,
    use_hidden_models: bool,
    key_gates_enabled: usize,
    bootstrap_intercepted: bool,
    bootstrap_source: Option<String>,
    routes_mounted: bool,
    renderer_ready_ms: Option<u64>,
}

async fn wait_for_injected_status(
    client: &reqwest::Client,
    port: u16,
    target_id: &str,
    expected_models: &[String],
) -> Result<InjectedStatus> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(target) = find_app_target(client, port).await?
            && target.id == target_id
            && let Ok(status) = inspect_injected_status(&target, port).await
            && injected_status_is_ready(&status, expected_models)
        {
            return Ok(status);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Codex App renderer 已启动，但增强模型配置未在 20 秒内生效");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn injected_status_is_ready(status: &InjectedStatus, expected_models: &[String]) -> bool {
    // Renderer routes may mount well after Statsig is updated; the retained script
    // will keep the enhanced configuration active while the UI finishes loading.
    status.ready
        && status.available_models == expected_models
        && !status.use_hidden_models
        && status.key_gates_enabled == KEY_FEATURE_GATES.len()
}

async fn inspect_injected_status(target: &CdpTarget, port: u16) -> Result<InjectedStatus> {
    let websocket_url = validated_websocket_url(target, port)?;
    let (mut socket, _) = connect_async(websocket_url).await?;
    let gates = serde_json::to_string(KEY_FEATURE_GATES)?;
    let expression = format!(
        r#"(() => {{
          const client = window.__STATSIG__?.firstInstance;
          const config = client?.getDynamicConfig?.("107580212")?.value;
          const gates = {gates};
          return {{
            ready: Boolean(Array.isArray(config?.available_models)),
            availableModels: config?.available_models ?? [],
            useHiddenModels: config?.use_hidden_models ?? true,
            keyGatesEnabled: gates.filter((gate) => client?.checkGate?.(gate) === true).length,
            bootstrapIntercepted: Boolean(window.__CODEXHUB_ENHANCED_MODE__?.bootstrapIntercepted),
            bootstrapSource: window.__CODEXHUB_ENHANCED_MODE__?.bootstrapSource ?? null,
            routesMounted: Boolean(window.__CODEXHUB_ENHANCED_MODE__?.routesMounted),
            rendererReadyMs: window.__CODEXHUB_ENHANCED_MODE__?.routesMountedAtMs ?? null,
          }};
        }})()"#
    );
    let mut command_id = 1;
    let result = cdp_command(
        &mut socket,
        &mut command_id,
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
    let gates = serde_json::to_string(KEY_FEATURE_GATES)?;
    Ok(format!(
        r#"(() => {{
  const MARKER = "__CODEXHUB_ENHANCED_MODE__";
  const MODELS = {models};
  const GATES = {gates};
  const existing = window[MARKER];
  if (existing?.installed) {{
    existing.update?.(MODELS, GATES);
    return;
  }}
  const CONFIG_ID = "107580212";
  const state = {{
    installed: true,
    applied: false,
    attempts: 0,
    client: null,
    models: MODELS,
    gates: GATES,
    bootstrapIntercepted: false,
    bootstrapInterceptedAtMs: null,
    bootstrapSource: null,
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
    values.param_stores ??= {{}};
    values.sdkParams ??= {{}};
    values.sdk_flags ??= {{}};
    for (const gate of state.gates) {{
      const current = values.feature_gates[gate];
      values.feature_gates[gate] = {{
        ...(current && typeof current === "object" ? current : {{}}),
        name: gate,
        rule_id: "codexhub-local",
        secondary_exposures: current?.secondary_exposures ?? [],
        version: current?.version ?? 1,
        id_type: current?.id_type ?? "userID",
        value: true,
      }};
    }}
    const current = values.dynamic_configs[CONFIG_ID];
    values.dynamic_configs[CONFIG_ID] = {{
      ...(current && typeof current === "object" ? current : {{}}),
      name: CONFIG_ID,
      rule_id: "codexhub-local",
      secondary_exposures: current?.secondary_exposures ?? [],
      version: current?.version ?? 1,
      id_type: current?.id_type ?? "userID",
      value: {{
        ...(current?.value && typeof current.value === "object" ? current.value : {{}}),
        available_models: state.models,
        default_model: state.models[0] ?? "gpt-5.6-sol",
        use_hidden_models: false,
      }},
      is_device_based: current?.is_device_based ?? false,
      passed: true,
    }};
    return values;
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
        if (value.dynamic_configs?.[CONFIG_ID]) patchValues(value);
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

  const minimalBootstrapValues = () => ({{
    response_format: "init-v2",
    feature_gates: {{}},
    dynamic_configs: {{}},
    layer_configs: {{
      "2096615506": {{
        v: "codexhub_primary_runtime_config",
        r: "codexhub-local",
        s: [],
        i: "userID",
        ue: false,
        p: true,
      }},
      "72216192": {{
        v: "codexhub_i18n_layer_config",
        r: "codexhub-local",
        s: [],
        i: "userID",
        ue: false,
        p: true,
      }},
    }},
    param_stores: {{}},
    values: {{
      codexhub_model_list_config: {{}},
      codexhub_primary_runtime_config: {{}},
      codexhub_i18n_layer_config: {{
        enable_i18n: true,
        locale_source: "FIRST_AVAILABLE",
      }},
    }},
    exposures: {{}},
    sdkParams: {{}},
    sdk_flags: {{}},
    has_updates: true,
    time: Date.now(),
  }});

  const buildBootstrapPayload = () => {{
    let selected = null;
    for (const key of Object.keys(localStorage)) {{
      if (!key.startsWith("statsig.cached.evaluations.")) continue;
      try {{
        const envelope = JSON.parse(originalGetItem.call(localStorage, key));
        const values = JSON.parse(envelope?.data);
        if (!values || typeof values !== "object") continue;
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

    const values = patchValues(structuredClone(selected?.values ?? minimalBootstrapValues()));
    values.response_format ??= "init-v2";
    values.user = localUser(values, selected?.envelope);
    state.bootstrapSource = selected
      ? selected.priority === 2
        ? "statsig-cache-prelogin"
        : selected.priority === 1 ? "statsig-cache-local" : "statsig-cache-other"
      : "codexhub-minimal";
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
      dispatchFetchResponse(message, {{ statsigPayload: buildBootstrapPayload() }});
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

  const applyClient = (client) => {{
    if (!client?._store?.setValues || typeof client._finalizeUpdate !== "function") return false;
    const current = client._store.getValues?.() ?? {{}};
    const currentConfig = current.dynamic_configs?.[CONFIG_ID]?.value;
    const alreadyApplied = currentConfig?.use_hidden_models === false
      && JSON.stringify(currentConfig.available_models) === JSON.stringify(state.models)
      && state.gates.every((gate) => current.feature_gates?.[gate]?.value === true);
    if (!alreadyApplied) {{
      const next = patchValues(structuredClone(current));
      const packet = {{
        data: JSON.stringify(next),
        source: client._store.getSource?.() ?? "Bootstrap",
        receivedAt: Date.now(),
      }};
      if (!client._store.setValues(packet, client._user)) return false;
      client._finalizeUpdate(packet);
    }}
    state.applied = true;
    state.client = client;
    return true;
  }};

  const attach = () => {{
    state.attempts += 1;
    const client = window.__STATSIG__?.firstInstance;
    if (!applyClient(client)) {{
      if (state.attempts < 300) setTimeout(attach, state.attempts < 80 ? 25 : 100);
      return;
    }}
    if (!client.__CODEXHUB_ENHANCED_LISTENER__) {{
      client.__CODEXHUB_ENHANCED_LISTENER__ = true;
      client.on?.("values_updated", () => {{
        if (state.applying) return;
        state.applying = true;
        try {{ applyClient(client); }} finally {{ state.applying = false; }}
      }});
    }}
  }};
  state.update = (models, gates) => {{
    state.models = models;
    state.gates = gates;
    state.applied = false;
    state.attempts = 0;
    queueMicrotask(attach);
  }};
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
fn launch_codex_app_blocking(port: u16) -> Result<()> {
    use std::os::windows::process::CommandExt;

    use base64::{Engine as _, engine::general_purpose::STANDARD};

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let script = format!(
        r#"$ErrorActionPreference='Stop'
$package=Get-AppxPackage OpenAI.Codex|Sort-Object Version -Descending|Select-Object -First 1
if(-not $package){{throw 'OpenAI.Codex Store package is not installed'}}
$source=@'
using System;
using System.Runtime.InteropServices;
[ComImport,Guid("2e941141-7f97-4756-ba1d-9decde894a3d"),InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
interface IApplicationActivationManager {{
 int ActivateApplication([MarshalAs(UnmanagedType.LPWStr)] string appUserModelId,[MarshalAs(UnmanagedType.LPWStr)] string arguments,uint options,out uint processId);
 int ActivateForFile(IntPtr appUserModelId,IntPtr itemArray,IntPtr verb,out uint processId);
 int ActivateForProtocol(IntPtr appUserModelId,IntPtr itemArray,out uint processId);
}}
[ComImport,Guid("45BA127D-10A8-46EA-8AB7-56EA9078943C")]
class ApplicationActivationManager {{}}
public static class CodexHubEnhancedLauncher {{
 public static uint Launch(string appId,string arguments) {{
  var manager=(IApplicationActivationManager)new ApplicationActivationManager();
  uint processId; int result=manager.ActivateApplication(appId,arguments,0,out processId);
  Marshal.ThrowExceptionForHR(result); return processId;
 }}
}}
'@
Add-Type -TypeDefinition $source
$appId="$($package.PackageFamilyName)!App"
[CodexHubEnhancedLauncher]::Launch($appId,'--remote-debugging-address=127.0.0.1 --remote-debugging-port={port}')|Out-Null"#
    );
    let encoded: Vec<u8> = script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &STANDARD.encode(encoded),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if !status.success() {
        bail!("Windows 无法以增强模式激活 Codex App");
    }
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
        assert!(script.contains("1042620455"));
        assert!(script.contains("/wham/statsig/bootstrap"));
        assert!(script.contains("codex-message-from-view"));
        assert!(script.contains("routesMountedAtMs"));
        assert!(!script.contains("values.layer_configs["));
    }

    #[test]
    fn normalized_models_trims_and_deduplicates() {
        assert_eq!(
            normalized_models(vec![" grok-4.5 ".into(), "".into(), "grok-4.5".into()]),
            vec!["grok-4.5"]
        );
    }

    #[test]
    fn enhanced_config_ready_does_not_require_renderer_routes_or_bootstrap_interception() {
        let expected_models = vec!["gpt-5.6-sol".to_string()];
        let status = InjectedStatus {
            ready: true,
            available_models: expected_models.clone(),
            use_hidden_models: false,
            key_gates_enabled: KEY_FEATURE_GATES.len(),
            bootstrap_intercepted: false,
            bootstrap_source: None,
            routes_mounted: false,
            renderer_ready_ms: None,
        };

        assert!(injected_status_is_ready(&status, &expected_models));
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
        assert_eq!(report.key_gates_enabled, KEY_FEATURE_GATES.len());
        let repeated = launch_and_inject(
            report.available_models.clone(),
            "http://127.0.0.1:3847/backend-api",
        )
        .await
        .expect("repeat live enhanced injection");
        assert_eq!(repeated.available_models, report.available_models);
        assert_eq!(repeated.key_gates_enabled, KEY_FEATURE_GATES.len());
    }
}

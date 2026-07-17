const LOOPBACK_HOSTS = new Set(["127.0.0.1", "localhost", "[::1]"]);

function parseArgs(argv) {
  const options = { port: 9335, mode: "status", openPicker: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--port") options.port = Number(argv[++index]);
    else if (arg === "--status") options.mode = "status";
    else if (arg === "--apply") options.mode = "apply";
    else if (arg === "--restore") options.mode = "restore";
    else if (arg === "--offline-statsig-reload") options.mode = "offlineStatsigReload";
    else if (arg === "--local-statsig-reload") options.mode = "localStatsigReload";
    else if (arg === "--trace-statsig-reload") options.mode = "traceStatsigReload";
    else if (arg === "--open-picker") options.openPicker = true;
    else throw new Error(`Unknown argument: ${arg}`);
  }
  if (!Number.isInteger(options.port) || options.port < 1024 || options.port > 65535) {
    throw new Error(`Invalid port: ${options.port}`);
  }
  return options;
}

function validatedWebSocketUrl(target, port) {
  const url = new URL(target.webSocketDebuggerUrl);
  if (url.protocol !== "ws:" || !LOOPBACK_HOSTS.has(url.hostname) || Number(url.port) !== port) {
    throw new Error(`Rejected non-loopback CDP WebSocket URL: ${url.href}`);
  }
  return url.href;
}

class CdpSession {
  constructor(target, port) {
    this.socket = new WebSocket(validatedWebSocketUrl(target, port));
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Map();
    this.closed = false;
  }

  async open() {
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error("CDP WebSocket open timed out")), 5000);
      this.socket.addEventListener("open", () => { clearTimeout(timeout); resolve(); }, { once: true });
      this.socket.addEventListener("error", () => { clearTimeout(timeout); reject(new Error("CDP WebSocket open failed")); }, { once: true });
    });
    this.socket.addEventListener("message", (event) => this.onMessage(event));
    this.socket.addEventListener("close", () => {
      this.closed = true;
      for (const waiter of this.pending.values()) {
        clearTimeout(waiter.timeout);
        waiter.reject(new Error("CDP WebSocket closed"));
      }
      this.pending.clear();
    });
    await this.send("Runtime.enable");
    return this;
  }

  onMessage(event) {
    const message = JSON.parse(String(event.data));
    if (!message.id) {
      for (const listener of this.listeners.get(message.method) ?? []) listener(message.params ?? {});
      return;
    }
    const waiter = this.pending.get(message.id);
    if (!waiter) return;
    clearTimeout(waiter.timeout);
    this.pending.delete(message.id);
    if (message.error) waiter.reject(new Error(`${message.error.message} (${message.error.code})`));
    else waiter.resolve(message.result);
  }

  send(method, params = {}) {
    if (this.closed) return Promise.reject(new Error("CDP session is closed"));
    return new Promise((resolve, reject) => {
      const id = this.nextId++;
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP command timed out: ${method}`));
      }, 10000);
      this.pending.set(id, { resolve, reject, timeout });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  on(method, listener) {
    const listeners = this.listeners.get(method) ?? [];
    listeners.push(listener);
    this.listeners.set(method, listeners);
  }

  async evaluate(expression) {
    const result = await this.send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
      userGesture: false,
    });
    if (result.exceptionDetails) {
      const detail = result.exceptionDetails.exception?.description ?? result.exceptionDetails.text;
      throw new Error(`Renderer evaluation failed: ${detail}`);
    }
    return result.result?.value;
  }

  close() {
    if (!this.closed) this.socket.close();
    this.closed = true;
  }
}

async function listCodexTargets(port) {
  const response = await fetch(`http://127.0.0.1:${port}/json/list`);
  if (!response.ok) throw new Error(`CDP target list returned HTTP ${response.status}`);
  const targets = await response.json();
  return targets.filter((target) => {
    if (target.type !== "page" || !target.url?.startsWith("app://") || !target.webSocketDebuggerUrl) return false;
    try {
      validatedWebSocketUrl(target, port);
      return true;
    } catch {
      return false;
    }
  });
}

const experimentExpression = (mode) => `(async () => {
  const STATE_KEY = "__CODEXHUB_MODEL_VISIBILITY_EXPERIMENT__";
  const CONFIG_ID = "107580212";
  const client = window.__STATSIG__?.firstInstance;
  if (!client) throw new Error("Statsig client is unavailable");

  const installValues = (values) => {
    const next = structuredClone(values);
    const currentTime = Number(client._store.getValues()?.time ?? 0);
    next.has_updates = true;
    next.time = Math.max(currentTime, Number(next.time ?? 0)) + 1;
    const packet = {
      data: JSON.stringify(next),
      source: client._store.getSource(),
      receivedAt: Date.now(),
    };
    if (!client._store.setValues(packet, client._user)) throw new Error("Statsig store rejected the in-memory values");
    client._finalizeUpdate(packet);
  };

  const findQueryClient = () => {
    for (const element of document.querySelectorAll("button,[role=main],main")) {
      const reactKey = Object.getOwnPropertyNames(element).find((name) =>
        name.startsWith("__reactFiber$") || name.startsWith("__reactInternalInstance$"));
      if (!reactKey) continue;
      let fiber = element[reactKey];
      for (let level = 0; fiber && level < 200; level += 1, fiber = fiber.return) {
        const candidate = fiber.memoizedProps?.client;
        if (candidate && typeof candidate.invalidateQueries === "function") return candidate;
      }
    }
    return null;
  };

  const refreshModelQuery = async (allowAllModels) => {
    const queryClient = findQueryClient();
    if (!queryClient) return { found: false, refreshed: false };
    const nativeSetHas = Set.prototype.has;
    if (allowAllModels) {
      Set.prototype.has = function(value) {
        const isModelAllowlist = this.size === 8 &&
          nativeSetHas.call(this, "gpt-5.6-sol") &&
          nativeSetHas.call(this, "gpt-5.4") &&
          nativeSetHas.call(this, "gpt-5.2");
        return isModelAllowlist ? true : nativeSetHas.call(this, value);
      };
    }
    try {
      await queryClient.refetchQueries({ queryKey: ["models", "list"], type: "active" });
    } finally {
      Set.prototype.has = nativeSetHas;
    }
    return { found: true, refreshed: true, transientAllowlistBypass: allowAllModels };
  };

  let queryRefresh = null;

  if (${JSON.stringify(mode)} === "apply") {
    if (!window[STATE_KEY]) {
      const originalValues = structuredClone(client._store.getValues());
      const patchedValues = structuredClone(originalValues);
      const config = patchedValues?.dynamic_configs?.[CONFIG_ID]?.value;
      if (!config || typeof config !== "object") throw new Error("Statsig dynamic config 107580212 was not found");
      window[STATE_KEY] = {
        originalValues,
        appliedAt: new Date().toISOString(),
      };
      config.use_hidden_models = false;
      installValues(patchedValues);
      queryRefresh = await refreshModelQuery(true);
    }
  } else if (${JSON.stringify(mode)} === "restore") {
    const state = window[STATE_KEY];
    if (state) {
      installValues(state.originalValues);
      delete window[STATE_KEY];
      queryRefresh = await refreshModelQuery(false);
    }
  }

  const state = window[STATE_KEY] ?? null;
  const dynamicConfig = client.getDynamicConfig(CONFIG_ID);
  const listeners = client._listeners instanceof Map
    ? [...client._listeners.entries()].map(([name, values]) => ({ name, count: values?.length ?? values?.size ?? null }))
    : Object.entries(client._listeners ?? {}).map(([name, values]) => ({ name, count: values?.length ?? values?.size ?? null }));
  const instances = Object.entries(window.__STATSIG__?.instances ?? {}).map(([sdkKey, instance]) => {
    const config = instance.getDynamicConfig(CONFIG_ID);
    return {
      sdkKey,
      firstInstance: instance === client,
      valuesUpdatedListeners: instance._listeners?.values_updated?.length ?? null,
      useHiddenModels: config.value?.use_hidden_models ?? null,
      availableModelCount: config.value?.available_models?.length ?? null,
    };
  });
  const queryClient = findQueryClient();
  const modelQueries = queryClient?.getQueriesData({ queryKey: ["models", "list"] }).map(([queryKey, data]) => ({
    queryKey,
    models: (data?.data ?? data?.models ?? []).map((model) => ({
      model: model.model,
      displayName: model.displayName ?? model.display_name ?? null,
      hidden: model.hidden ?? null,
      isDefault: model.isDefault ?? model.is_default ?? null,
    })),
  })) ?? [];
  return {
    mode: ${JSON.stringify(mode)},
    applied: Boolean(state),
    appliedAt: state?.appliedAt ?? null,
    patchedSources: state ? ["store.dynamic_configs.107580212.value.use_hidden_models"] : [],
    listeners,
    instances,
    modelQueries,
    queryRefresh,
    enhancedMode: window.__CODEXHUB_ENHANCED_MODE__ ?? null,
    statsigNetwork: {
      sdkKey: client._sdkKey ?? null,
      options: {
        api: client._options?.networkConfig?.api ?? null,
        initializeUrl: client._options?.networkConfig?.initializeUrl ?? null,
        hasNetworkOverride: typeof client._options?.networkConfig?.networkOverrideFunc === "function",
      },
      initializeUrlConfig: {
        customUrl: client._network?._initializeUrlConfig?.customUrl ?? null,
        defaultUrl: client._network?._initializeUrlConfig?.defaultUrl ?? null,
        resolvedUrl: client._network?._initializeUrlConfig?.getUrl?.() ?? null,
      },
    },
    statsigStoreShape: {
      fields: Object.keys(client._store.getValues?.() ?? {}).sort(),
      featureGate: client._store.getValues?.()?.feature_gates?.["1042620455"] ?? null,
      dynamicConfig: client._store.getValues?.()?.dynamic_configs?.["107580212"] ?? null,
    },
    dynamicConfig: {
      availableModels: dynamicConfig.value?.available_models ?? [],
      useHiddenModels: dynamicConfig.value?.use_hidden_models ?? null,
      defaultModel: dynamicConfig.value?.default_model ?? null,
    },
  };
})()`;

const inspectModelButtonExpression = `(() => {
  const buttons = [...document.querySelectorAll("button")];
  for (const button of buttons) {
    const reactKey = Object.getOwnPropertyNames(button).find((name) =>
      name.startsWith("__reactFiber$") || name.startsWith("__reactInternalInstance$"));
    if (!reactKey) continue;
    let fiber = button[reactKey];
    for (let level = 0; fiber && level < 15; level += 1, fiber = fiber.return) {
      const props = fiber.memoizedProps;
      if (!Array.isArray(props?.labelCandidates)) continue;
      return {
        buttonText: (button.innerText || button.textContent || "").trim(),
        currentModel: props.model ?? null,
        reasoningEffort: props.reasoningEffort ?? null,
        candidateCount: props.labelCandidates.length,
        candidateModels: [...new Set(props.labelCandidates.map((item) => item?.model).filter(Boolean))],
      };
    }
  }
  return null;
})()`;

const locateModelPickerExpression = `(() => {
  for (const button of document.querySelectorAll("button")) {
    const reactKey = Object.getOwnPropertyNames(button).find((name) =>
      name.startsWith("__reactFiber$") || name.startsWith("__reactInternalInstance$"));
    if (!reactKey) continue;
    let fiber = button[reactKey];
    for (let level = 0; fiber && level < 15; level += 1, fiber = fiber.return) {
      if (!Array.isArray(fiber.memoizedProps?.labelCandidates)) continue;
      const rect = button.getBoundingClientRect();
      return {
        found: true,
        buttonText: (button.innerText || button.textContent || "").trim(),
        x: Math.round(rect.left + rect.width / 2),
        y: Math.round(rect.top + rect.height / 2),
      };
    }
  }
  return { found: false, buttonText: null, x: null, y: null };
})()`;

const inspectOpenMenuExpression = `(() => ({
  activeElement: document.activeElement ? {
    tag: document.activeElement.tagName.toLowerCase(),
    text: (document.activeElement.innerText || document.activeElement.textContent || "").trim().slice(0, 500),
  } : null,
  openElements: [...document.querySelectorAll('[data-state="open"],[role="menu"],[role="menuitem"],[role="listbox"],[role="option"],[data-radix-menu-content]')]
    .map((element) => ({
      tag: element.tagName.toLowerCase(),
      role: element.getAttribute("role"),
      text: (element.innerText || element.textContent || "").trim().slice(0, 2000),
    }))
    .filter((item) => item.text),
}))()`;

const statsigCacheBootstrapExpression = (forceCacheMiss) => `(() => {
  const marker = "__CODEXHUB_STATSIG_CACHE_INTERCEPT__";
  if (window[marker]) return;
  const originalGetItem = Storage.prototype.getItem;
  const patchNode = (value, depth = 0) => {
    if (depth > 12 || value == null) return value;
    if (typeof value === "string") {
      const trimmed = value.trim();
      if (!trimmed.startsWith("{") && !trimmed.startsWith("[")) return value;
      try {
        const parsed = JSON.parse(value);
        const patched = patchNode(parsed, depth + 1);
        return JSON.stringify(patched);
      } catch {
        return value;
      }
    }
    if (Array.isArray(value)) return value.map((item) => patchNode(item, depth + 1));
    if (typeof value !== "object") return value;
    const config = value.dynamic_configs?.["107580212"]?.value;
    if (config && typeof config === "object") config.use_hidden_models = false;
    for (const key of Object.keys(value)) value[key] = patchNode(value[key], depth + 1);
    return value;
  };
  Storage.prototype.getItem = function(key) {
    window[marker]?.keys?.push(String(key));
    const raw = originalGetItem.call(this, key);
    if (typeof key !== "string" || !key.startsWith("statsig.cached.evaluations.")) return raw;
    if (${JSON.stringify(forceCacheMiss)}) return null;
    return raw != null ? patchNode(raw) : raw;
  };
  window[marker] = { installed: true, keys: [] };
})()`;

const networkTraceBootstrapExpression = `(() => {
  const marker = "__CODEXHUB_NETWORK_TRACE__";
  if (window[marker]) return;
  const entries = [];
  const record = (kind, value) => {
    try {
      entries.push({ kind, url: String(value), at: Date.now() });
    } catch {}
  };
  const originalFetch = window.fetch;
  window.fetch = function(input, init) {
    record("fetch", typeof input === "string" ? input : input?.url);
    return originalFetch.call(this, input, init);
  };
  const originalOpen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function(method, url, ...rest) {
    record("xhr:" + String(method).toUpperCase(), url);
    return originalOpen.call(this, method, url, ...rest);
  };
  window[marker] = { entries };
})()`;

function patchStatsigValues(values) {
  const patched = structuredClone(values);
  const config = patched?.dynamic_configs?.["107580212"]?.value;
  if (!config || typeof config !== "object") throw new Error("Statsig dynamic config 107580212 is unavailable");
  config.use_hidden_models = false;
  patched.has_updates = true;
  patched.time = Math.max(Number(patched.time ?? 0), Date.now());
  return patched;
}

async function runOfflineStatsigReload(session, useLocalPayload) {
  let responseBody;
  if (useLocalPayload) {
    const response = await fetch("http://127.0.0.1:3847/backend-api/wham/statsig/bootstrap", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
    });
    if (!response.ok) throw new Error(`CodexHub Statsig bootstrap returned HTTP ${response.status}`);
    const envelope = await response.json();
    if (typeof envelope.statsigPayload !== "string") throw new Error("CodexHub Statsig bootstrap payload is missing");
    responseBody = envelope.statsigPayload;
  } else {
    const values = await session.evaluate("window.__STATSIG__?.firstInstance?._store?.getValues?.() ?? null");
    if (!values) throw new Error("Current Statsig evaluation values are unavailable");
    responseBody = JSON.stringify(patchStatsigValues(values));
  }
  const bodyBase64 = Buffer.from(responseBody, "utf8").toString("base64");
  let intercepted = 0;
  let interceptionError = null;

  session.on("Fetch.requestPaused", (event) => {
    if (!event.request?.url?.startsWith("https://ab.chatgpt.com/v1/initialize")) {
      session.send("Fetch.continueRequest", { requestId: event.requestId }).catch(() => {});
      return;
    }
    intercepted += 1;
    session.send("Fetch.fulfillRequest", {
      requestId: event.requestId,
      responseCode: 200,
      responseHeaders: [
        { name: "content-type", value: "application/json" },
        { name: "access-control-allow-origin", value: "*" },
        { name: "cache-control", value: "no-store" },
      ],
      body: bodyBase64,
    }).catch((error) => { interceptionError = error.message; });
  });

  await session.send("Page.enable");
  const preload = await session.send("Page.addScriptToEvaluateOnNewDocument", {
    source: statsigCacheBootstrapExpression(useLocalPayload),
  });
  await session.send("Fetch.enable", {
    patterns: [{ urlPattern: "https://ab.chatgpt.com/v1/initialize*", requestStage: "Request" }],
  });
  await session.send("Page.reload", { ignoreCache: false });

  const deadline = Date.now() + 20000;
  let status = null;
  while (Date.now() < deadline) {
    try {
      status = await session.evaluate(`(() => {
        const client = window.__STATSIG__?.firstInstance;
        const config = client?.getDynamicConfig?.("107580212")?.value;
        return {
          ready: document.readyState === "complete" && Boolean(client) &&
            Array.isArray(config?.available_models) && config.available_models.length > 0,
          readyState: document.readyState,
          cacheInterceptInstalled: Boolean(window.__CODEXHUB_STATSIG_CACHE_INTERCEPT__),
          cacheKeys: [...new Set(window.__CODEXHUB_STATSIG_CACHE_INTERCEPT__?.keys ?? [])],
          storeSource: client?._store?.getSource?.() ?? null,
          useHiddenModels: config?.use_hidden_models ?? null,
          availableModels: config?.available_models ?? [],
        };
      })()`);
      if (status?.ready) break;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  await session.send("Fetch.disable");
  if (preload.identifier) {
    await session.send("Page.removeScriptToEvaluateOnNewDocument", { identifier: preload.identifier });
  }
  return { intercepted, interceptionError, status };
}

async function runStatsigNetworkTrace(session) {
  const networkRequests = [];
  session.on("Network.requestWillBeSent", (event) => {
    const url = event.request?.url;
    if (typeof url === "string") {
      networkRequests.push({
        type: event.type ?? null,
        method: event.request.method ?? null,
        url,
      });
    }
  });

  await session.send("Page.enable");
  await session.send("Network.enable");
  const cachePreload = await session.send("Page.addScriptToEvaluateOnNewDocument", {
    source: statsigCacheBootstrapExpression(true),
  });
  const tracePreload = await session.send("Page.addScriptToEvaluateOnNewDocument", {
    source: networkTraceBootstrapExpression,
  });
  await session.send("Page.reload", { ignoreCache: true });

  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    try {
      const ready = await session.evaluate("document.readyState === 'complete' && Boolean(window.__STATSIG__?.firstInstance)");
      if (ready) {
        await new Promise((resolve) => setTimeout(resolve, 3000));
        break;
      }
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  const rendererTrace = await session.evaluate(`(() => ({
    entries: window.__CODEXHUB_NETWORK_TRACE__?.entries ?? [],
    cacheKeys: [...new Set(window.__CODEXHUB_STATSIG_CACHE_INTERCEPT__?.keys ?? [])],
    storeSource: window.__STATSIG__?.firstInstance?._store?.getSource?.() ?? null,
  }))()`);
  await session.send("Network.disable");
  if (cachePreload.identifier) {
    await session.send("Page.removeScriptToEvaluateOnNewDocument", { identifier: cachePreload.identifier });
  }
  if (tracePreload.identifier) {
    await session.send("Page.removeScriptToEvaluateOnNewDocument", { identifier: tracePreload.identifier });
  }
  const relevant = (entry) => /chatgpt\.com|openai\.com|statsig|127\.0\.0\.1|localhost/i.test(entry.url);
  return {
    renderer: (rendererTrace?.entries ?? []).filter(relevant),
    cdpNetwork: networkRequests.filter(relevant),
    cacheKeys: rendererTrace?.cacheKeys ?? [],
    storeSource: rendererTrace?.storeSource ?? null,
  };
}

const locateModelSubmenuExpression = `(() => {
  const items = [...document.querySelectorAll('[role="menuitem"]')];
  const item = items.find((element) => /(^|\\n)(模型|model)(\\n|$)/i.test((element.innerText || element.textContent || "").trim()));
  if (!item) return { found: false, text: null, x: null, y: null };
  item.focus();
  const rect = item.getBoundingClientRect();
  return {
    found: true,
    text: (item.innerText || item.textContent || "").trim(),
    attributes: Object.fromEntries([...item.attributes].map((attribute) => [attribute.name, attribute.value])),
    x: Math.round(rect.left + rect.width / 2),
    y: Math.round(rect.top + rect.height / 2),
  };
})()`;

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const targets = await listCodexTargets(options.port);
  if (!targets.length) throw new Error(`No app:// Codex renderer found on CDP port ${options.port}`);
  const reports = [];
  for (const target of targets) {
    const session = await new CdpSession(target, options.port).open();
    try {
      const statsigReloadMode = options.mode === "offlineStatsigReload" || options.mode === "localStatsigReload";
      const networkTrace = options.mode === "traceStatsigReload"
        ? await runStatsigNetworkTrace(session)
        : null;
      const offlineStatsig = statsigReloadMode
        ? await runOfflineStatsigReload(session, options.mode === "localStatsigReload")
        : null;
      const effectiveMode = statsigReloadMode || options.mode === "traceStatsigReload"
        ? "status"
        : options.mode;
      const experiment = await session.evaluate(experimentExpression(effectiveMode));
      if (options.mode !== "status") await new Promise((resolve) => setTimeout(resolve, 1500));
      const modelButton = await session.evaluate(inspectModelButtonExpression);
      let picker = null;
      if (options.openPicker) {
        await session.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
        await session.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
        const trigger = await session.evaluate(locateModelPickerExpression);
        if (trigger.found) {
          await session.send("Input.dispatchMouseEvent", { type: "mousePressed", x: trigger.x, y: trigger.y, button: "left", clickCount: 1 });
          await session.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: trigger.x, y: trigger.y, button: "left", clickCount: 1 });
        }
        await new Promise((resolve) => setTimeout(resolve, 500));
        const submenu = await session.evaluate(locateModelSubmenuExpression);
        if (submenu.found) {
          await session.send("Input.dispatchKeyEvent", { type: "keyDown", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39 });
          await session.send("Input.dispatchKeyEvent", { type: "keyUp", key: "ArrowRight", code: "ArrowRight", windowsVirtualKeyCode: 39 });
          await new Promise((resolve) => setTimeout(resolve, 800));
        }
        picker = { trigger, submenu, ...(await session.evaluate(inspectOpenMenuExpression)) };
      }
      reports.push({ target: { id: target.id, title: target.title }, networkTrace, offlineStatsig, experiment, modelButton, picker });
    } finally {
      session.close();
    }
  }
  console.log(JSON.stringify({ port: options.port, reports }, null, 2));
}

main().catch((error) => {
  console.error(`codex-app-cdp-model-visibility-experiment: ${error.message}`);
  process.exitCode = 1;
});

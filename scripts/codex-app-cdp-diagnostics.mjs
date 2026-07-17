import fs from "node:fs/promises";
import path from "node:path";

const LOOPBACK_HOSTS = new Set(["127.0.0.1", "localhost", "[::1]"]);

function parseArgs(argv) {
  const options = {
    port: 9335,
    output: path.resolve("outputs", "codex-app-cdp-model-diagnostics.json"),
    watchMs: 15000,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--port") options.port = Number(argv[++index]);
    else if (arg === "--output") options.output = path.resolve(argv[++index]);
    else if (arg === "--watch-ms") options.watchMs = Number(argv[++index]);
    else throw new Error(`Unknown argument: ${arg}`);
  }
  if (!Number.isInteger(options.port) || options.port < 1024 || options.port > 65535) {
    throw new Error(`Invalid port: ${options.port}`);
  }
  if (!Number.isInteger(options.watchMs) || options.watchMs < 0 || options.watchMs > 120000) {
    throw new Error(`Invalid watch duration: ${options.watchMs}`);
  }
  return options;
}

function validateWebSocketUrl(rawUrl, port) {
  const url = new URL(rawUrl);
  if (url.protocol !== "ws:" || !LOOPBACK_HOSTS.has(url.hostname) || Number(url.port) !== port) {
    throw new Error(`Rejected non-loopback CDP WebSocket URL: ${url.href}`);
  }
  return url.href;
}

class CdpSession {
  constructor(target, port) {
    this.socket = new WebSocket(validateWebSocketUrl(target.webSocketDebuggerUrl, port));
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
    await this.send("Network.enable", { maxTotalBufferSize: 0, maxResourceBufferSize: 0 });
    return this;
  }

  onMessage(event) {
    const message = JSON.parse(String(event.data));
    if (message.id) {
      const waiter = this.pending.get(message.id);
      if (!waiter) return;
      clearTimeout(waiter.timeout);
      this.pending.delete(message.id);
      if (message.error) waiter.reject(new Error(`${message.error.message} (${message.error.code})`));
      else waiter.resolve(message.result);
      return;
    }
    for (const listener of this.listeners.get(message.method) ?? []) listener(message.params ?? {});
  }

  on(method, listener) {
    const listeners = this.listeners.get(method) ?? [];
    listeners.push(listener);
    this.listeners.set(method, listeners);
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
      validateWebSocketUrl(target.webSocketDebuggerUrl, port);
      return true;
    } catch {
      return false;
    }
  });
}

const SNAPSHOT_EXPRESSION = String.raw`(() => {
  const KEYWORDS = /model|available_models|statsig|provider|authmethod|hidden_models/i;
  const MODEL_TEXT = /gpt|codex|claude|opus|sonnet|grok|deepseek|glm/i;
  const SECRET = /token|secret|password|authorization|cookie|api[-_]?key/i;
  const MAX_DEPTH = 5;
  const MAX_KEYS = 80;
  const MAX_ARRAY = 80;
  const MAX_STRING = 1000;

  const cleanUrl = (raw) => {
    try {
      const url = new URL(raw, location.href);
      url.search = url.search ? "?<redacted>" : "";
      url.hash = "";
      return url.href;
    } catch {
      return String(raw).slice(0, MAX_STRING);
    }
  };

  const sanitize = (value, depth = 0, key = "", seen = new WeakSet()) => {
    if (SECRET.test(key)) return "<redacted>";
    if (value == null || typeof value === "boolean" || typeof value === "number") return value;
    if (typeof value === "string") return value.length > MAX_STRING ? value.slice(0, MAX_STRING) + "<truncated>" : value;
    if (typeof value === "function") return "<function " + (value.name || "anonymous") + ">";
    if (typeof value !== "object") return String(value);
    if (depth >= MAX_DEPTH) return "<" + (value.constructor?.name || "object") + ":max-depth>";
    if (seen.has(value)) return "<cycle>";
    seen.add(value);
    if (Array.isArray(value)) return value.slice(0, MAX_ARRAY).map((item) => sanitize(item, depth + 1, "", seen));
    const output = {};
    let descriptors;
    try { descriptors = Object.getOwnPropertyDescriptors(value); } catch { return "<unreadable>"; }
    for (const [name, descriptor] of Object.entries(descriptors).slice(0, MAX_KEYS)) {
      if (!("value" in descriptor)) continue;
      output[name] = sanitize(descriptor.value, depth + 1, name, seen);
    }
    return output;
  };

  const elementSummary = (element) => ({
    tag: element.tagName?.toLowerCase() ?? null,
    role: element.getAttribute?.("role"),
    testId: element.getAttribute?.("data-testid"),
    ariaLabel: element.getAttribute?.("aria-label"),
    text: (element.innerText || element.textContent || "").trim().slice(0, 500),
    className: typeof element.className === "string" ? element.className.slice(0, 500) : null,
  });

  const candidates = [...document.querySelectorAll("button,[role],input,[data-testid]")]
    .filter((element) => {
      const role = element.getAttribute("role");
      const inComposer = Boolean(element.closest(".composer-surface-chrome,[aria-label*=\"撰写\"],[aria-label*=\"composer\" i]"));
      const material = [
      element.textContent,
      element.getAttribute("aria-label"),
      element.getAttribute("data-testid"),
      element.getAttribute("name"),
      ].filter(Boolean).join(" ");
      return inComposer || ["option", "menuitem", "listbox", "dialog"].includes(role) || KEYWORDS.test(material) || MODEL_TEXT.test(material);
    })
    .slice(0, 100);

  const reactEntries = [];
  for (const element of candidates) {
    const reactKey = Object.getOwnPropertyNames(element).find((name) =>
      name.startsWith("__reactFiber$") || name.startsWith("__reactInternalInstance$") || name.startsWith("__reactProps$"));
    if (!reactKey) continue;
    let fiber = element[reactKey];
    const chain = [];
    for (let level = 0; fiber && level < 12; level += 1, fiber = fiber.return) {
      const displayName = fiber.elementType?.displayName || fiber.elementType?.name || fiber.type?.displayName || fiber.type?.name || fiber.type;
      const material = {
        level,
        displayName: typeof displayName === "string" ? displayName : null,
        key: fiber.key ?? null,
        memoizedProps: fiber.memoizedProps,
        memoizedState: fiber.memoizedState,
      };
      const cleanMaterial = sanitize(material);
      let searchable = "";
      try { searchable = JSON.stringify(cleanMaterial); } catch {}
      if (KEYWORDS.test(searchable) || MODEL_TEXT.test(searchable)) chain.push(cleanMaterial);
    }
    reactEntries.push({ element: elementSummary(element), reactKeyType: reactKey.split("$")[0], chain });
  }

  const matchingGlobals = {};
  for (const name of Object.getOwnPropertyNames(window).filter((item) => KEYWORDS.test(item)).slice(0, 100)) {
    const descriptor = Object.getOwnPropertyDescriptor(window, name);
    if (descriptor && "value" in descriptor) {
      matchingGlobals[name] = descriptor.value == null
        ? descriptor.value
        : { type: typeof descriptor.value, constructor: descriptor.value?.constructor?.name ?? null };
    }
  }

  const findNamedValues = (root, wantedNames) => {
    if (!root || typeof root !== "object") return [];
    const results = [];
    const visited = new WeakSet();
    const queue = [{ value: root, path: "window.__STATSIG__" }];
    let visitedCount = 0;
    while (queue.length && visitedCount < 20000 && results.length < 100) {
      const current = queue.shift();
      if (!current.value || typeof current.value !== "object" || visited.has(current.value)) continue;
      visited.add(current.value);
      visitedCount += 1;
      let descriptors;
      try { descriptors = Object.getOwnPropertyDescriptors(current.value); } catch { continue; }
      for (const [name, descriptor] of Object.entries(descriptors).slice(0, 250)) {
        if (!("value" in descriptor)) continue;
        const childPath = current.path + "." + name;
        if (wantedNames.has(name)) results.push({ path: childPath, value: sanitize(descriptor.value, 0, name) });
        if (descriptor.value && typeof descriptor.value === "object") queue.push({ value: descriptor.value, path: childPath });
      }
    }
    return results;
  };

  const statsigDescriptor = Object.getOwnPropertyDescriptor(window, "__STATSIG__");
  const statsigClient = statsigDescriptor && "value" in statsigDescriptor
    ? statsigDescriptor.value?.firstInstance
    : null;
  const statsigClientInfo = statsigClient ? (() => {
    const prototypeMethods = [];
    let prototype = Object.getPrototypeOf(statsigClient);
    for (let level = 0; prototype && level < 4; level += 1, prototype = Object.getPrototypeOf(prototype)) {
      prototypeMethods.push({
        level,
        constructor: prototype.constructor?.name ?? null,
        methods: Object.getOwnPropertyNames(prototype).filter((name) =>
          typeof Object.getOwnPropertyDescriptor(prototype, name)?.value === "function"),
      });
    }
    const override = statsigClient.overrideAdapter;
    return {
      ownKeys: Object.getOwnPropertyNames(statsigClient),
      prototypeMethods,
      overrideAdapter: override ? {
        constructor: override.constructor?.name ?? null,
        ownKeys: Object.getOwnPropertyNames(override),
        prototypeMethods: Object.getOwnPropertyNames(Object.getPrototypeOf(override) ?? {}).filter((name) =>
          typeof Object.getOwnPropertyDescriptor(Object.getPrototypeOf(override), name)?.value === "function"),
      } : null,
    };
  })() : null;
  const statsigValues = statsigDescriptor && "value" in statsigDescriptor
    ? findNamedValues(statsigDescriptor.value, new Set(["available_models", "use_hidden_models", "default_model"]))
    : [];

  const storageKeys = (storage) => {
    const result = [];
    for (let index = 0; index < storage.length; index += 1) {
      const key = storage.key(index);
      if (key && KEYWORDS.test(key)) result.push(key);
    }
    return result;
  };

  return {
    capturedAt: new Date().toISOString(),
    page: { title: document.title, href: location.origin + location.pathname, readyState: document.readyState },
    shellMarkers: {
      mainSurface: Boolean(document.querySelector("main.main-surface")),
      leftPanel: Boolean(document.querySelector("aside.app-shell-left-panel")),
      composer: Boolean(document.querySelector(".composer-surface-chrome")),
    },
    candidates: candidates.map(elementSummary),
    reactEntries,
    matchingGlobals,
    statsigClientInfo,
    statsigValues,
    storageKeys: { local: storageKeys(localStorage), session: storageKeys(sessionStorage) },
    performanceResources: performance.getEntriesByType("resource").map((entry) => ({
      name: cleanUrl(entry.name),
      initiatorType: entry.initiatorType,
      duration: Math.round(entry.duration),
      transferSize: entry.transferSize,
    })),
  };
})()`;

function cleanNetworkUrl(rawUrl) {
  try {
    const url = new URL(rawUrl);
    url.search = url.search ? "?<redacted>" : "";
    url.hash = "";
    return url.href;
  } catch {
    return String(rawUrl).slice(0, 1000);
  }
}

async function captureTarget(target, options) {
  const session = await new CdpSession(target, options.port).open();
  const network = [];
  const requests = new Map();
  session.on("Network.requestWillBeSent", (event) => {
    requests.set(event.requestId, {
      requestId: event.requestId,
      timestamp: event.wallTime ? new Date(event.wallTime * 1000).toISOString() : null,
      method: event.request?.method,
      url: cleanNetworkUrl(event.request?.url),
      resourceType: event.type,
    });
  });
  session.on("Network.responseReceived", (event) => {
    const request = requests.get(event.requestId) ?? { requestId: event.requestId };
    network.push({
      ...request,
      url: cleanNetworkUrl(event.response?.url ?? request.url),
      status: event.response?.status,
      mimeType: event.response?.mimeType,
      fromDiskCache: event.response?.fromDiskCache ?? false,
      fromServiceWorker: event.response?.fromServiceWorker ?? false,
    });
  });
  try {
    const before = await session.evaluate(SNAPSHOT_EXPRESSION);
    if (options.watchMs > 0) await new Promise((resolve) => setTimeout(resolve, options.watchMs));
    const after = await session.evaluate(SNAPSHOT_EXPRESSION);
    return {
      target: { id: target.id, title: target.title, url: target.url },
      before,
      after,
      observedNetwork: network,
    };
  } finally {
    session.close();
  }
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  let targets;
  try {
    targets = await listCodexTargets(options.port);
  } catch (error) {
    throw new Error(
      `Cannot reach Codex CDP on 127.0.0.1:${options.port}. ` +
      `Codex must already have been launched with --remote-debugging-address=127.0.0.1 ` +
      `--remote-debugging-port=${options.port}. The diagnostic does not restart Codex. (${error.message})`,
    );
  }
  if (!targets.length) throw new Error(`No app:// Codex renderer found on CDP port ${options.port}`);

  console.error(`Connected to ${targets.length} Codex renderer(s). Open the model selector now; observing for ${options.watchMs} ms...`);
  const reports = [];
  for (const target of targets) reports.push(await captureTarget(target, options));
  const report = {
    schemaVersion: 1,
    readOnly: true,
    port: options.port,
    watchMs: options.watchMs,
    generatedAt: new Date().toISOString(),
    reports,
  };
  await fs.mkdir(path.dirname(options.output), { recursive: true });
  await fs.writeFile(options.output, JSON.stringify(report, null, 2), "utf8");
  console.log(options.output);
}

main().catch((error) => {
  console.error(`codex-app-cdp-diagnostics: ${error.message}`);
  process.exitCode = 1;
});

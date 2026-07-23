import {
  Activity,
  Bot,
  BrainCircuit,
  Cable,
  CheckCircle2,
  ChevronDown,
  ClipboardCopy,
  Clock3,
  Code2,
  Cog,
  DatabaseZap,
  FileClock,
  Gauge,
  GitBranch,
  History,
  KeyRound,
  Layers3,
  ListFilter,
  MessageSquare,
  Monitor,
  Network,
  Pencil,
  PlayCircle,
  Plus,
  RefreshCw,
  RotateCcw,
  Router,
  Save,
  Search,
  Send,
  Server,
  Settings,
  ShieldCheck,
  SlidersHorizontal,
  TerminalSquare,
  Trash2,
  Wifi,
  Wrench,
  XCircle,
  Zap
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { GatewayApi, truncateMiddle } from "./api";
import type { AppConfig, DashboardResponse, EventItem, LoggingStatus, ProviderConfig, RequestLogItem, Tone } from "./types";

type PageKey = "overview" | "gateway" | "codex" | "chat" | "logs" | "settings";

const navItems: Array<{ key: PageKey; label: string; icon: typeof Monitor }> = [
  { key: "overview", label: "概览", icon: Monitor },
  { key: "gateway", label: "AI Gateway", icon: Router },
  { key: "codex", label: "Codex 接入", icon: Cable },
  { key: "chat", label: "聊天工具", icon: MessageSquare },
  { key: "logs", label: "请求日志", icon: FileClock },
  { key: "settings", label: "设置", icon: Settings }
];

const providerTemplates: ProviderConfig[] = [
  {
    name: "OpenAI Responses",
    enabled: true,
    providerType: "openai_responses",
    baseUrl: "https://api.openai.com/v1",
    apiKey: "",
    models: ["gpt-5.4-mini", "o4-mini"],
    modelAliases: {},
    weight: 100,
    timeoutSecs: 600
  },
  {
    name: "DeepSeek",
    enabled: true,
    providerType: "chat_completions",
    baseUrl: "https://api.deepseek.com/v1",
    apiKey: "",
    models: ["deepseek-v4-flash"],
    modelAliases: { "gpt-5.4-mini": "deepseek-v4-flash" },
    weight: 95,
    timeoutSecs: 600
  },
  {
    name: "Anthropic Claude",
    enabled: true,
    providerType: "anthropic_messages",
    baseUrl: "https://api.anthropic.com/v1",
    apiKey: "",
    models: ["claude-sonnet"],
    modelAliases: {},
    weight: 90,
    timeoutSecs: 600
  },
  {
    name: "Zhipu GLM",
    enabled: false,
    providerType: "anthropic_messages",
    compatibility: "glm",
    baseUrl: "https://open.bigmodel.cn/api/anthropic/v1",
    apiKey: "",
    models: ["GLM-5.2"],
    modelAliases: { "glm-5.2": "GLM-5.2" },
    weight: 80,
    timeoutSecs: 600
  }
];

export default function App() {
  const [activePage, setActivePage] = useState<PageKey>("overview");
  const [api, setApi] = useState<GatewayApi | null>(null);
  const [desktop, setDesktop] = useState({ baseUrl: browserBaseUrl(), version: "0.3.22", managedDaemon: false });
  const [dashboard, setDashboard] = useState<DashboardResponse | null>(null);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [events, setEvents] = useState<EventItem[]>([]);
  const [loggingStatus, setLoggingStatus] = useState<LoggingStatus | null>(null);
  const [logs, setLogs] = useState<RequestLogItem[]>([]);
  const [selectedLogId, setSelectedLogId] = useState<number | null>(null);
  const [lastSync, setLastSync] = useState<Date | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function boot() {
      const info = window.gateway
        ? await window.gateway.getBackendInfo()
        : { baseUrl: browserBaseUrl(), version: "0.3.22", managedDaemon: false };
      if (cancelled) return;
      setDesktop(info);
      setApi(new GatewayApi(info.baseUrl));
    }
    void boot();
    return () => {
      cancelled = true;
    };
  }, []);

  const refresh = useCallback(async () => {
    if (!api) return;
    try {
      const [dashboardRes, configRes, eventsRes, loggingRes] = await Promise.all([
        api.dashboard(),
        api.config(),
        api.events(),
        api.loggingStatus()
      ]);
      setDashboard(dashboardRes);
      setConfig(configRes);
      setEvents(eventsRes.slice().reverse().slice(0, 8));
      setLoggingStatus(loggingRes);
      try {
        const logRes = await api.requestLogs();
        setLogs(logRes.logs ?? []);
      } catch {
        setLogs([]);
      }
      setLastSync(new Date());
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [api]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const providers = useMemo(
    () => config?.aiGateway.providers ?? dashboard?.aiGateway.providers ?? [],
    [config, dashboard]
  );
  const visibleModels = useMemo(
    () => config?.aiGateway.codexVisibleModels ?? dashboard?.aiGateway.codexVisibleModels ?? [],
    [config, dashboard]
  );
  async function runAction(label: string, action: () => Promise<unknown>) {
    if (!api) return;
    setBusy(true);
    setNotice(null);
    setError(null);
    try {
      await action();
      setNotice(`${label} 已完成`);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function updateConfig(mutator: (draft: AppConfig) => void, label = "配置保存") {
    if (!api || !config) return;
    const draft = structuredClone(config) as AppConfig;
    mutator(draft);
    await runAction(label, async () => {
      await api.saveConfig(draft);
    });
  }

  const context = {
    api,
    dashboard,
    config,
    events,
    loggingStatus,
    providers,
    visibleModels,
    logs,
    selectedLogId,
    setSelectedLogId,
    runAction,
    updateConfig,
    busy,
    baseUrl: desktop.baseUrl
  };

  return (
    <div className="app-shell">
      <Sidebar activePage={activePage} setActivePage={setActivePage} running={!error} />
      <main className="main">
        <Topbar activePage={activePage} baseUrl={desktop.baseUrl} refresh={refresh} />
        {error && <Banner tone="bad" text={error} />}
        {notice && <Banner tone="good" text={notice} />}
        {activePage === "overview" && <OverviewPage {...context} />}
        {activePage === "gateway" && <GatewayPage {...context} />}
        {activePage === "codex" && <CodexPage {...context} />}
        {activePage === "chat" && <ChatPage {...context} />}
        {activePage === "logs" && <LogsPage {...context} />}
        {activePage === "settings" && <SettingsPage {...context} version={desktop.version} />}
        <footer className="statusbar">
          <span>v{desktop.version}</span>
          <span>请求日志 {config?.aiGateway.requestLoggingEnabled ? "开启" : "关闭"}</span>
          <span>配置 {truncateMiddle(config?.statePath ?? "未读取", 50)}</span>
          <span>同步 {lastSync ? lastSync.toLocaleTimeString() : "等待中"}</span>
        </footer>
      </main>
    </div>
  );
}

function browserBaseUrl() {
  return window.location.origin && window.location.origin !== "null"
    ? window.location.origin
    : "http://127.0.0.1:3847";
}

function Sidebar({
  activePage,
  setActivePage,
  running
}: {
  activePage: PageKey;
  setActivePage: (page: PageKey) => void;
  running: boolean;
}) {
  return (
    <aside className="sidebar">
      <div className="brand">
        <div className="brand-mark">
          <Router size={18} />
        </div>
        <div>
          <strong>Codex Remote Gateway</strong>
          <span className={`pill ${running ? "good" : "bad"}`}>{running ? "本地服务运行中" : "本地服务离线"}</span>
        </div>
      </div>
      <nav className="nav">
        {navItems.map((item) => {
          const Icon = item.icon;
          return (
            <button
              key={item.key}
              className={`nav-item ${activePage === item.key ? "active" : ""}`}
              onClick={() => setActivePage(item.key)}
            >
              <Icon size={17} />
              <span>{item.label}</span>
            </button>
          );
        })}
      </nav>
      <div className="sidebar-footer">
        <div className="mini-label">核心模式</div>
        <div className="mini-value">Rust daemon + Electron UI</div>
      </div>
    </aside>
  );
}

function Topbar({
  activePage,
  baseUrl,
  refresh
}: {
  activePage: PageKey;
  baseUrl: string;
  refresh: () => Promise<void>;
}) {
  const title = {
    overview: ["控制台概览", "本地 AI Gateway + Codex remote-control bridge"],
    gateway: ["AI Gateway", "统一管理 Codex 可见模型、上游渠道和路由策略"],
    codex: ["Codex 接入", "把 Codex App、VS Code 插件和 CLI 连接到本地网关"],
    chat: ["聊天工具接入", "通过飞书、Telegram、微信远程管理 Codex 会话"],
    logs: ["请求日志", "观察 AI Gateway 请求、首帧、延迟和上游错误"],
    settings: ["设置", "管理本地服务、桌面行为、更新和安全选项"]
  }[activePage];
  return (
    <header className="topbar">
      <div>
        <h1>{title[0]}</h1>
        <p>{title[1]}</p>
      </div>
      <div className="top-actions">
        <span className="address-pill">{baseUrl.replace(/^https?:\/\//, "")}</span>
        <button className="icon-button" title="刷新" onClick={() => void refresh()}>
          <RefreshCw size={16} />
        </button>
        <button className="icon-button" title="设置">
          <Cog size={16} />
        </button>
      </div>
    </header>
  );
}

function OverviewPage(props: PageProps) {
  const { dashboard, providers, events, runAction, updateConfig, api, baseUrl } = props;
  const healthyProviders = providers.filter((provider) => provider.enabled).length;
  const imAccounts = imRows(dashboard);
  return (
    <div className="page-grid overview-grid">
      <HealthCard icon={Server} label="本地服务" value={dashboard?.status.running ? "运行中" : "等待连接"} detail={baseUrl} tone={dashboard ? "good" : "warn"} />
      <HealthCard icon={BrainCircuit} label="Codex App" value={dashboard?.remote.connected ? "已连接" : "未连接"} detail={String(dashboard?.remote.serverName ?? "remote-control")} tone={dashboard?.remote.connected ? "good" : "warn"} />
      <HealthCard icon={Code2} label="VS Code 插件" value={dashboard?.codexApp.configured ? "可接入" : "待写入"} detail={dashboard?.codexApp.configPath ? truncateMiddle(dashboard.codexApp.configPath, 36) : "~/.codex/config.toml"} tone={dashboard?.codexApp.configured ? "good" : "warn"} />
      <HealthCard icon={MessageSquare} label="IM Bridge" value={`${imAccounts.filter((item) => item.connected).length}/${Math.max(imAccounts.length, 3)} 已接入`} detail="Feishu / Telegram / WeChat" tone={imAccounts.some((item) => item.connected) ? "good" : "warn"} />

      <Panel className="topology-panel" title="连接拓扑" action={<Badge tone="good">实时</Badge>}>
        <Topology dashboard={dashboard} baseUrl={baseUrl} />
      </Panel>

      <Panel title="AI Gateway 渠道" className="provider-panel" action={<Badge tone={healthyProviders ? "good" : "warn"}>{healthyProviders} 个可用</Badge>}>
        <ProviderTable providers={providers} compact />
      </Panel>

      <Panel title="快速操作" className="quick-panel">
        <div className="quick-actions">
          <ActionButton icon={Save} label="写入 Codex 配置" onClick={() => runAction("写入 Codex 配置", () => api?.configureCodexApp({}) ?? Promise.resolve())} />
          <ActionButton icon={Plus} label="添加 Provider" onClick={() => addProviderFromTemplate(updateConfig)} />
          <ActionButton icon={Bot} label="扫码接入微信" onClick={() => runAction("微信扫码接入", () => api?.startWechatOnboard() ?? Promise.resolve())} />
          <ActionButton icon={History} label="查看会话" onClick={() => Promise.resolve()} />
        </div>
        <EventFeed events={events} />
      </Panel>
    </div>
  );
}

function GatewayPage(props: PageProps) {
  const { providers, config, updateConfig, runAction, api } = props;
  const enabledCount = providers.filter((provider) => provider.enabled).length;
  const avgLatency = providers.length ? Math.round(providers.reduce((sum, provider) => sum + providerLatency(provider), 0) / providers.length) : 0;
  return (
    <div className="page-stack">
      <div className="metric-row">
        <MetricCard label="可用渠道" value={String(providers.length)} sub="provider configs" tone="info" />
        <MetricCard label="健康渠道" value={String(enabledCount)} sub="enabled routes" tone="good" />
        <MetricCard label="平均首帧" value={`${avgLatency || 0}ms`} sub="estimated TTFT" tone="warn" />
        <ToggleMetric
          label="请求日志"
          checked={Boolean(config?.aiGateway.requestLoggingEnabled)}
          onChange={() =>
            updateConfig(
              (draft) => {
                draft.aiGateway.requestLoggingEnabled = !draft.aiGateway.requestLoggingEnabled;
              },
              "请求日志开关"
            )
          }
        />
      </div>
      <div className="content-split wide-left">
        <Panel title="渠道列表" action={<ActionButton icon={Plus} label="添加渠道" onClick={() => addProviderFromTemplate(updateConfig)} small />}>
          <ProviderTable
            providers={providers}
            onToggle={(name) =>
              updateConfig((draft) => {
                draft.aiGateway.providers = draft.aiGateway.providers.map((provider) =>
                  provider.name === name ? { ...provider, enabled: !provider.enabled } : provider
                );
              }, "渠道状态")
            }
          />
        </Panel>
        <Panel title="路由策略">
          <Segmented values={["优先级", "权重", "会话粘性"]} active="优先级" />
          <div className="settings-list">
            {providers.slice(0, 4).map((provider) => (
              <div className="setting-row" key={provider.name}>
                <span>{provider.name}</span>
                <input
                  type="range"
                  min={1}
                  max={120}
                  value={provider.weight || 100}
                  onChange={(event) =>
                    updateConfig((draft) => {
                      draft.aiGateway.providers = draft.aiGateway.providers.map((item) =>
                        item.name === provider.name ? { ...item, weight: Number(event.target.value) } : item
                      );
                    }, "路由权重")
                  }
                />
                <strong>{provider.weight || 100}</strong>
              </div>
            ))}
            <label className="switch-row">
              <input type="checkbox" checked readOnly />
              <span>同会话粘性绑定</span>
            </label>
          </div>
        </Panel>
      </div>
      <div className="content-split">
        <Panel title="模型映射">
          <AliasChips providers={providers} />
          <button className="secondary-button">
            <Plus size={15} /> 添加映射
          </button>
        </Panel>
        <Panel title="渠道测试">
          <div className="test-box">
            <SelectLike label="Provider" value={providers[0]?.name ?? "未配置"} />
            <SelectLike label="Model" value={providers[0]?.models?.[0] ?? "gpt-5.4-mini"} />
            <button className="primary-button" onClick={() => runAction("发送测试", async () => undefined)}>
              <Send size={15} /> 发送测试
            </button>
            <div className="result-grid">
              <Badge tone="good">200 OK</Badge>
              <span>TTFT 612ms</span>
              <span>总耗时 2.1s</span>
            </div>
          </div>
        </Panel>
      </div>
    </div>
  );
}

function CodexPage(props: PageProps) {
  const { dashboard, visibleModels, api, runAction, updateConfig, config } = props;
  return (
    <div className="page-stack">
      <div className="metric-row">
        <HealthCard icon={BrainCircuit} label="Codex App" value={dashboard?.remote.connected ? "已连接" : "未连接"} detail={String(dashboard?.remote.serverName ?? "等待 remote-control")} tone={dashboard?.remote.connected ? "good" : "warn"} />
        <HealthCard icon={Code2} label="VS Code 插件" value={dashboard?.codexApp.configured ? "可接入" : "待写入"} detail="读取 ~/.codex/config.toml" tone={dashboard?.codexApp.configured ? "good" : "warn"} />
        <HealthCard icon={TerminalSquare} label="Codex CLI" value="未连接" detail="app-server 3849" tone="neutral" />
      </div>
      <div className="content-split wide-left">
        <Panel title="接入配置">
          <InfoRows
            rows={[
              ["Codex Home", String(dashboard?.codexApp.codexHome ?? "~/.codex")],
              ["Backend URL", `${props.baseUrl}/backend-api`],
              ["Active Provider", dashboard?.codexApp.provider?.name ?? "AI Gateway"],
              ["可见模型", `${visibleModels.length || modelSet(props.providers).length} models`]
            ]}
          />
          <div className="button-row">
            <button className="primary-button" onClick={() => runAction("写入 Codex 配置", () => api?.configureCodexApp({}) ?? Promise.resolve())}>
              <Save size={15} /> 写入 Codex 配置
            </button>
            <button className="secondary-button" onClick={() => runAction("恢复原有配置", () => api?.uninstallCodexApp() ?? Promise.resolve())}>
              <RotateCcw size={15} /> 恢复原有配置
            </button>
          </div>
        </Panel>
        <Panel title="连接模式">
          <Segmented values={["标准模式", "VPN 兼容模式"]} active={config?.localConnectionMode === "vpnCompatible" ? "VPN 兼容模式" : "标准模式"} />
          <label className="switch-row">
            <input
              type="checkbox"
              checked={Boolean(config?.codexAppFastStartup)}
              onChange={() =>
                updateConfig((draft) => {
                  draft.codexAppFastStartup = !draft.codexAppFastStartup;
                }, "快速启动")
              }
            />
            <span>快速启动监听 localhost:8000</span>
          </label>
          <StatusLine tone={config?.codexAppFastStartup ? "good" : "neutral"} text={config?.codexAppFastStartup ? "快速启动监听：已启用" : "快速启动监听：未启用"} />
        </Panel>
      </div>
      <div className="content-split wide-left">
        <Panel title="可见模型" action={<button className="ghost-button" onClick={() => runAction("刷新模型列表", () => api?.refreshModels() ?? Promise.resolve())}><RefreshCw size={14} />刷新</button>}>
          <div className="model-chip-grid">
            {(visibleModels.length ? visibleModels : modelSet(props.providers)).slice(0, 18).map((model) => (
              <label className="check-chip" key={model}>
                <input type="checkbox" defaultChecked />
                <span>{model}</span>
              </label>
            ))}
          </div>
        </Panel>
        <Panel title="接入步骤">
          <StepList
            steps={[
              ["本地服务运行", true],
              ["AI Gateway 已配置", props.providers.length > 0],
              ["Codex 配置已写入", Boolean(dashboard?.codexApp.configured)],
              ["Codex App 已连接", Boolean(dashboard?.remote.connected)]
            ]}
          />
        </Panel>
      </div>
      <Panel title="会话归属">
        <div className="session-columns">
          <MiniTable title="其他 Provider 会话" rows={[["openai", "codex-remote", "12 min"], ["anthropic", "writer", "1 h"]]} />
          <MiniTable title="AI Gateway 会话" rows={[["ai-gateway", "cursor2api", "active"], ["ai-gateway", "skills-hub", "23 min"]]} />
        </div>
      </Panel>
    </div>
  );
}

function ChatPage(props: PageProps) {
  const rows = imRows(props.dashboard);
  return (
    <div className="page-stack">
      <div className="metric-row">
        {["飞书", "Telegram", "微信"].map((name) => {
          const row = rows.find((item) => item.platform === name);
          return (
            <ChannelCard
              key={name}
              name={name}
              connected={Boolean(row?.connected)}
              detail={row?.detail ?? (name === "Telegram" ? "添加 Bot Token" : name === "微信" ? "扫码添加机器人" : "扫码使用新机器人")}
              action={name === "Telegram" ? "添加 Bot Token" : name === "微信" ? "扫码接入" : "管理机器人"}
            />
          );
        })}
      </div>
      <div className="content-split wide-left">
        <Panel title="机器人池">
          <table className="data-table">
            <thead>
              <tr>
                <th>启用</th>
                <th>名称</th>
                <th>平台</th>
                <th>账号 / Chat ID</th>
                <th>状态</th>
                <th>当前会话</th>
                <th>最近活跃</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.platform}>
                  <td><Toggle checked={row.enabled} /></td>
                  <td>{row.name}</td>
                  <td>{row.platform}</td>
                  <td>{row.account}</td>
                  <td><Badge tone={row.connected ? "good" : "warn"}>{row.connected ? "已接入" : "未接入"}</Badge></td>
                  <td>{row.session}</td>
                  <td>{row.lastActive}</td>
                  <td><RowActions /></td>
                </tr>
              ))}
            </tbody>
          </table>
        </Panel>
        <Panel title="新增接入">
          <Segmented values={["飞书", "Telegram", "微信"]} active="Telegram" />
          <div className="form-stack">
            <Field label="Bot Token" value="••••••••••••••••" />
            <Field label="Allowed Chat ID" value="default" />
            <button className="primary-button" onClick={() => props.runAction("保存 Telegram", () => props.api?.configureTelegram({}) ?? Promise.resolve())}>
              <Save size={15} /> 保存并接入
            </button>
            <StatusLine tone="warn" text="等待 Token 校验" />
          </div>
        </Panel>
      </div>
      <div className="content-split">
        <Panel title="授权范围">
          <PermissionGrid items={["创建会话", "恢复会话", "审批命令", "接收图片", "会话列表"]} />
        </Panel>
        <Panel title="扫码状态">
          <QrPlaceholder />
        </Panel>
      </div>
      <Panel title="会话分配策略">
        <div className="assignment-grid">
          {["飞书 1 -> Codex App", "Telegram -> VS Code", "微信 -> CLI fallback"].map((item) => (
            <div className="assignment" key={item}>
              <GitBranch size={16} />
              <span>{item}</span>
              <Pencil size={14} />
            </div>
          ))}
        </div>
      </Panel>
    </div>
  );
}

function LogsPage(props: PageProps) {
  const [query, setQuery] = useState("");
  const filtered = props.logs.filter((log) => {
    const blob = `${log.requestId ?? ""} ${log.modelId ?? ""} ${log.providerName ?? ""} ${log.status ?? ""}`.toLowerCase();
    return blob.includes(query.toLowerCase());
  });
  const selected = filtered.find((log) => log.id === props.selectedLogId) ?? filtered[0];
  return (
    <div className="page-stack">
      <div className="metric-row">
        <MetricCard label="今日请求" value={String(props.logs.length || 0)} sub="recent log rows" tone="info" />
        <MetricCard label="成功率" value={`${successRate(props.logs)}%`} sub="2xx / completed" tone="good" />
        <MetricCard label="平均 TTFT" value={`${average(props.logs.map((log) => log.ttftMs ?? 0))}ms`} sub="time to first token" tone="warn" />
        <MetricCard label="错误" value={String(props.logs.filter((log) => isErrorLog(log)).length)} sub="4xx / 5xx / failed" tone="bad" />
      </div>
      <div className="filter-row">
        <div className="search-box">
          <Search size={16} />
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索 request id / model / provider" />
        </div>
        <SelectLike label="Provider" value="All" />
        <SelectLike label="Model" value="All" />
        <Segmented values={["All", "2xx", "4xx", "5xx"]} active="All" compact />
        <button className="ghost-button" onClick={() => props.runAction("清空日志", () => props.api?.clearRequestLogs() ?? Promise.resolve())}>
          <Trash2 size={14} />清空
        </button>
      </div>
      <div className="content-split log-layout">
        <Panel title="请求列表">
          <table className="data-table log-table">
            <thead>
              <tr>
                <th>时间</th>
                <th>Provider</th>
                <th>模型</th>
                <th>Stream</th>
                <th>状态</th>
                <th>TTFT</th>
                <th>总耗时</th>
                <th>Tokens</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((log) => (
                <tr key={log.id} className={selected?.id === log.id ? "selected" : ""} onClick={() => props.setSelectedLogId(log.id)}>
                  <td>{formatTime(log.createdAtMs)}</td>
                  <td>{log.providerName ?? log.channel ?? "unknown"}</td>
                  <td>{log.modelId ?? "-"}</td>
                  <td>{log.stream ? "Yes" : "No"}</td>
                  <td><Badge tone={isErrorLog(log) ? "bad" : "good"}>{log.httpStatus ?? log.status ?? "-"}</Badge></td>
                  <td>{log.ttftMs ?? 0}ms</td>
                  <td>{log.latencyMs ?? 0}ms</td>
                  <td>{log.totalTokens ?? "-"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </Panel>
        <Panel title="请求详情">
          <RequestDetail log={selected} />
        </Panel>
      </div>
      <Panel title="延迟趋势">
        <TrendBars logs={props.logs} />
      </Panel>
    </div>
  );
}

function SettingsPage(props: PageProps & { version: string }) {
  const { api, config, loggingStatus, runAction, updateConfig, version } = props;
  const [logDirDraft, setLogDirDraft] = useState("");
  const canOpenLogDir = Boolean(window.gateway?.openPath && loggingStatus?.logDir);

  useEffect(() => {
    setLogDirDraft(String(config?.logging.logDir ?? loggingStatus?.logDir ?? ""));
  }, [config?.logging.logDir, loggingStatus?.logDir]);

  return (
    <div className="settings-grid">
      <Panel title="本地服务">
        <div className="form-stack">
          <Field label="监听地址" value={config?.bind?.split(":")[0] ?? "127.0.0.1"} />
          <Field label="主端口" value={config?.bind?.split(":")[1] ?? "3847"} />
          <Field label="Codex app-server" value="3849" />
          <Field label="快速启动端口" value="8000" />
          <label className="switch-row">
            <input type="checkbox" defaultChecked />
            <span>启动应用时自动启动本地服务</span>
          </label>
          <label className="switch-row">
            <input type="checkbox" defaultChecked />
            <span>退出时停止本地服务</span>
          </label>
        </div>
      </Panel>
      <Panel title="更新">
        <InfoRows rows={[["当前版本", `v${version}`], ["最新版本", "v0.3.22"], ["上次检查", "刚刚"]]} />
        <Segmented values={["stable", "beta"]} active="stable" />
        <button className="secondary-button">
          <RefreshCw size={15} /> 检查更新
        </button>
      </Panel>
      <Panel title="桌面行为">
        <Segmented values={["中文", "English"]} active="中文" />
        <Segmented values={["跟随系统", "浅色", "深色"]} active="浅色" />
        <label className="switch-row">
          <input type="checkbox" defaultChecked />
          <span>关闭窗口时隐藏到托盘</span>
        </label>
        <label className="switch-row">
          <input type="checkbox" />
          <span>启动时最小化</span>
        </label>
      </Panel>
      <Panel title="日志与诊断">
        <label className="switch-row">
          <input
            type="checkbox"
            checked={Boolean(config?.aiGateway.requestLoggingEnabled)}
            onChange={() =>
              updateConfig((draft) => {
                draft.aiGateway.requestLoggingEnabled = !draft.aiGateway.requestLoggingEnabled;
              }, "请求日志")
            }
          />
          <span>请求日志</span>
        </label>
        <label className="switch-row">
          <input
            type="checkbox"
            checked={Boolean(config?.logging.diagnostic)}
            onChange={() =>
              updateConfig((draft) => {
                draft.logging.diagnostic = !draft.logging.diagnostic;
              }, "诊断日志")
            }
          />
          <span>诊断链路日志</span>
        </label>
        <InfoRows rows={[
          ["当前目录", loggingStatus?.logDir ?? "-"],
          ["当前文件", loggingStatus?.activeLogPath ?? "-"],
          ["保留天数", `${config?.logging.retentionDays ?? 7} days`],
          ["最大日志", `${config?.logging.maxMb ?? 20} MB`]
        ]} />
        <Field
          label="自定义日志目录"
          value={logDirDraft}
          onChange={setLogDirDraft}
          action={
            <button
              className="secondary-button small"
              onClick={() =>
                updateConfig((draft) => {
                  draft.logging.logDir = logDirDraft.trim() ? logDirDraft.trim() : null;
                }, "日志目录")
              }
            >
              保存目录
            </button>
          }
        />
        <StatusLine tone="neutral" text="日志目录保存后，下次重启本地服务生效" />
        <div className="button-row">
          <button
            className="secondary-button"
            onClick={() => runAction("清理日志", () => api?.clearLogging() ?? Promise.resolve())}
          >
            <Trash2 size={15} /> 清理日志
          </button>
          <button
            className="secondary-button"
            disabled={!canOpenLogDir}
            onClick={() => {
              if (loggingStatus?.logDir) {
                void window.gateway?.openPath(loggingStatus.logDir);
              }
            }}
          >
            <FileClock size={15} /> 打开日志目录
          </button>
          <button className="secondary-button"><DatabaseZap size={15} /> 导出诊断包</button>
        </div>
      </Panel>
      <Panel title="安全与权限" className="settings-wide">
        <div className="security-grid">
          <StatusLine tone="good" text="仅允许本机 Electron / Vite 页面访问管理 API" />
          <StatusLine tone="neutral" text="Allowed hosts: 127.0.0.1, localhost, file origin" />
          <label className="switch-row">
            <input type="checkbox" />
            <span>API protection token</span>
          </label>
          <div className="button-row">
            <button className="secondary-button"><RotateCcw size={15} /> 重置 Codex 配置备份</button>
            <button className="secondary-button"><Wrench size={15} /> 清理旧插件状态</button>
            <button className="danger-button"><XCircle size={15} /> 恢复默认设置</button>
          </div>
        </div>
      </Panel>
    </div>
  );
}

interface PageProps {
  api: GatewayApi | null;
  dashboard: DashboardResponse | null;
  config: AppConfig | null;
  events: EventItem[];
  loggingStatus: LoggingStatus | null;
  providers: ProviderConfig[];
  visibleModels: string[];
  logs: RequestLogItem[];
  selectedLogId: number | null;
  setSelectedLogId: (id: number) => void;
  runAction: (label: string, action: () => Promise<unknown>) => Promise<void>;
  updateConfig: (mutator: (draft: AppConfig) => void, label?: string) => Promise<void>;
  busy: boolean;
  baseUrl: string;
}

function Panel({ title, action, children, className = "" }: { title: string; action?: React.ReactNode; children: React.ReactNode; className?: string }) {
  return (
    <section className={`panel ${className}`}>
      <div className="panel-head">
        <h2>{title}</h2>
        {action}
      </div>
      {children}
    </section>
  );
}

function HealthCard({ icon: Icon, label, value, detail, tone }: { icon: typeof Monitor; label: string; value: string; detail: string; tone: Tone }) {
  return (
    <div className="health-card">
      <div className={`icon-tile ${tone}`}>
        <Icon size={19} />
      </div>
      <div>
        <span className="label">{label}</span>
        <strong>{value}</strong>
        <small>{detail}</small>
      </div>
    </div>
  );
}

function MetricCard({ label, value, sub, tone }: { label: string; value: string; sub: string; tone: Tone }) {
  return (
    <div className="metric-card">
      <span className="label">{label}</span>
      <strong>{value}</strong>
      <small>{sub}</small>
      <div className={`spark ${tone}`} />
    </div>
  );
}

function ToggleMetric({ label, checked, onChange }: { label: string; checked: boolean; onChange: () => void }) {
  return (
    <button className="metric-card toggle-metric" onClick={onChange}>
      <span className="label">{label}</span>
      <strong>{checked ? "开启" : "关闭"}</strong>
      <Toggle checked={checked} />
    </button>
  );
}

function Topology({ dashboard, baseUrl }: { dashboard: DashboardResponse | null; baseUrl: string }) {
  const left = [
    ["Codex App", dashboard?.remote.connected ? "已连接" : "未连接", BrainCircuit],
    ["VS Code", dashboard?.codexApp.configured ? "可接入" : "待配置", Code2],
    ["CLI", "等待 app-server", TerminalSquare]
  ] as const;
  const right = [
    ["飞书", channelConnected(dashboard, "feishu") ? "已接入" : "未接入", MessageSquare],
    ["Telegram", channelConnected(dashboard, "telegram") ? "已接入" : "未接入", Send],
    ["微信", channelConnected(dashboard, "wechat") ? "已接入" : "未接入", Bot]
  ] as const;
  return (
    <div className="topology">
      <div className="node-stack">
        {left.map(([label, state, Icon]) => <Node key={label} icon={Icon} label={label} state={state} good={state !== "未连接"} />)}
      </div>
      <div className="connector left-connector" />
      <div className="daemon-node">
        <div className="daemon-orb"><Server size={24} /></div>
        <strong>Rust Daemon / AI Gateway</strong>
        <span>监听 {baseUrl.replace(/^https?:\/\//, "")}</span>
      </div>
      <div className="connector right-connector" />
      <div className="node-stack">
        {right.map(([label, state, Icon]) => <Node key={label} icon={Icon} label={label} state={state} good={state === "已接入"} />)}
      </div>
    </div>
  );
}

function Node({ icon: Icon, label, state, good }: { icon: typeof Monitor; label: string; state: string; good: boolean }) {
  return (
    <div className="topology-node">
      <Icon size={18} />
      <div>
        <strong>{label}</strong>
        <span className={good ? "text-good" : "text-warn"}>{state}</span>
      </div>
    </div>
  );
}

function ProviderTable({ providers, compact = false, onToggle }: { providers: ProviderConfig[]; compact?: boolean; onToggle?: (name: string) => void }) {
  return (
    <table className={`data-table ${compact ? "compact" : ""}`}>
      <thead>
        <tr>
          <th>启用</th>
          <th>服务商</th>
          <th>Base URL</th>
          <th>协议</th>
          <th>模型</th>
          <th>权重</th>
          <th>健康</th>
          <th>延迟</th>
          {!compact && <th>操作</th>}
        </tr>
      </thead>
      <tbody>
        {providers.length === 0 && (
          <tr>
            <td colSpan={compact ? 8 : 9} className="table-empty">
              暂无渠道，点击添加 Provider 创建第一个模型渠道
            </td>
          </tr>
        )}
        {providers.map((provider) => (
          <tr key={provider.name}>
            <td><Toggle checked={provider.enabled} onClick={() => onToggle?.(provider.name)} /></td>
            <td><ProviderName provider={provider} /></td>
            <td>{truncateMiddle(provider.baseUrl || "-", compact ? 20 : 34)}</td>
            <td>{providerTypeLabel(provider)}</td>
            <td>{provider.models?.length ?? 0}</td>
            <td>{provider.weight || 100}</td>
            <td><Badge tone={provider.enabled ? "good" : "neutral"}>{provider.enabled ? "Healthy" : "Disabled"}</Badge></td>
            <td>{provider.enabled ? `${providerLatency(provider)}ms` : "-"}</td>
            {!compact && <td><RowActions /></td>}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function ProviderName({ provider }: { provider: ProviderConfig }) {
  const icon = provider.providerType === "anthropic_messages" ? BrainCircuit : provider.providerType === "chat_completions" ? Zap : Router;
  const Icon = icon;
  return (
    <div className="provider-name">
      <Icon size={16} />
      <span>{provider.name || "unnamed"}</span>
    </div>
  );
}

function QuickIcon({ icon: Icon }: { icon: typeof Monitor }) {
  return <Icon size={15} />;
}

function ActionButton({ icon, label, onClick, small = false }: { icon: typeof Monitor; label: string; onClick: () => void | Promise<void>; small?: boolean }) {
  return (
    <button className={small ? "secondary-button small" : "action-button"} onClick={() => void onClick()}>
      <QuickIcon icon={icon} />
      <span>{label}</span>
    </button>
  );
}

function EventFeed({ events }: { events: EventItem[] }) {
  const display = events.length
    ? events
    : [
        { kind: "daemon_ready", message: "本地服务已启动" },
        { kind: "gateway_idle", message: "AI Gateway 等待请求" },
        { kind: "codex_status", message: "Codex remote-control 等待连接" }
      ];
  return (
    <div className="event-feed">
      <h3>最近事件</h3>
      {display.slice(0, 4).map((event, index) => (
        <div className="event-item" key={`${event.kind ?? "event"}-${index}`}>
          <span className={`dot ${event.level === "error" ? "bad" : event.level === "warn" ? "warn" : "good"}`} />
          <div>
            <strong>{event.kind ?? "event"}</strong>
            <small>{event.message ?? "状态已更新"}</small>
          </div>
        </div>
      ))}
    </div>
  );
}

function AliasChips({ providers }: { providers: ProviderConfig[] }) {
  const aliases = providers.flatMap((provider) =>
    Object.entries(provider.modelAliases ?? {}).map(([from, to]) => ({ from, to, provider: provider.name }))
  );
  const display = aliases.length
    ? aliases
    : [
        { from: "gpt-5.4-mini", to: "deepseek-v4-flash", provider: "DeepSeek" },
        { from: "claude-sonnet", to: "claude-code-sonnet", provider: "Anthropic" },
        { from: "glm-5.2", to: "GLM-5.2", provider: "Zhipu" }
      ];
  return (
    <div className="alias-grid">
      {display.map((alias) => (
        <div className="alias-chip" key={`${alias.provider}-${alias.from}`}>
          <span>{alias.from}</span>
          <ChevronDown size={14} />
          <strong>{alias.to}</strong>
        </div>
      ))}
    </div>
  );
}

function Badge({ tone, children }: { tone: Tone; children: React.ReactNode }) {
  return <span className={`badge ${tone}`}>{children}</span>;
}

function Toggle({ checked, onClick }: { checked: boolean; onClick?: () => void }) {
  return (
    <button className={`toggle ${checked ? "on" : ""}`} onClick={onClick} aria-label="toggle">
      <span />
    </button>
  );
}

function Segmented({ values, active, compact = false }: { values: string[]; active: string; compact?: boolean }) {
  return (
    <div className={`segmented ${compact ? "compact" : ""}`}>
      {values.map((value) => (
        <button className={value === active ? "active" : ""} key={value}>{value}</button>
      ))}
    </div>
  );
}

function SelectLike({ label, value }: { label: string; value: string }) {
  return (
    <div className="select-like">
      <span>{label}</span>
      <strong>{value}</strong>
      <ChevronDown size={14} />
    </div>
  );
}

function Field({
  label,
  value,
  onChange,
  action
}: {
  label: string;
  value: string;
  onChange?: (value: string) => void;
  action?: React.ReactNode;
}) {
  return (
    <label className="field">
      <span>{label}</span>
      <input value={value} readOnly={!onChange} onChange={(event) => onChange?.(event.target.value)} />
      {action ?? (!onChange && <ClipboardCopy size={15} />)}
    </label>
  );
}

function InfoRows({ rows }: { rows: Array<[string, string]> }) {
  return (
    <div className="info-rows">
      {rows.map(([label, value]) => (
        <div className="info-row" key={label}>
          <span>{label}</span>
          <strong>{truncateMiddle(value, 58)}</strong>
        </div>
      ))}
    </div>
  );
}

function StatusLine({ tone, text }: { tone: Tone; text: string }) {
  return (
    <div className={`status-line ${tone}`}>
      <span className={`dot ${tone}`} />
      <span>{text}</span>
    </div>
  );
}

function StepList({ steps }: { steps: Array<[string, boolean]> }) {
  return (
    <div className="step-list">
      {steps.map(([label, done], index) => (
        <div className="step" key={label}>
          <span className={done ? "done" : ""}>{done ? <CheckCircle2 size={15} /> : index + 1}</span>
          <strong>{label}</strong>
        </div>
      ))}
    </div>
  );
}

function MiniTable({ title, rows }: { title: string; rows: string[][] }) {
  return (
    <div className="mini-table">
      <h3>{title}</h3>
      {rows.map((row) => (
        <div className="mini-row" key={row.join("-")}>
          {row.map((cell) => <span key={cell}>{cell}</span>)}
        </div>
      ))}
    </div>
  );
}

function ChannelCard({ name, connected, detail, action }: { name: string; connected: boolean; detail: string; action: string }) {
  const Icon = name === "飞书" ? MessageSquare : name === "Telegram" ? Send : Bot;
  return (
    <div className="channel-card">
      <div className={`icon-tile ${connected ? "good" : "warn"}`}>
        <Icon size={19} />
      </div>
      <div>
        <strong>{name}</strong>
        <Badge tone={connected ? "good" : "warn"}>{connected ? "已接入" : "未接入"}</Badge>
        <small>{detail}</small>
      </div>
      <button className="secondary-button small">{action}</button>
    </div>
  );
}

function PermissionGrid({ items }: { items: string[] }) {
  return (
    <div className="permission-grid">
      {items.map((item) => (
        <div className="permission" key={item}>
          <CheckCircle2 size={15} />
          <span>{item}</span>
        </div>
      ))}
    </div>
  );
}

function QrPlaceholder() {
  return (
    <div className="qr-box">
      <div className="qr-grid" />
      <strong>剩余 02:14</strong>
      <span>等待移动端扫码确认</span>
    </div>
  );
}

function RequestDetail({ log }: { log?: RequestLogItem }) {
  if (!log) return <div className="empty-state">暂无请求日志</div>;
  return (
    <div className="request-detail">
      <div className="request-id">{log.requestId ?? `#${log.id}`}</div>
      <InfoRows
        rows={[
          ["Provider", String(log.providerName ?? log.channel ?? "-")],
          ["Model", String(log.modelId ?? "-")],
          ["Status", String(log.httpStatus ?? log.status ?? "-")],
          ["Created", formatTime(log.createdAtMs)]
        ]}
      />
      <div className="timing-bars">
        <Timing label="TTFT" value={log.ttftMs ?? 0} max={2400} />
        <Timing label="Total" value={log.latencyMs ?? 0} max={5000} />
        <Timing label="Retry" value={isErrorLog(log) ? 700 : 80} max={2000} />
      </div>
      <div className="button-row">
        <button className="secondary-button"><ClipboardCopy size={15} /> 复制 cURL</button>
        <button className="secondary-button"><ClipboardCopy size={15} /> 复制响应</button>
        <button className="primary-button"><PlayCircle size={15} /> 重新发送</button>
      </div>
    </div>
  );
}

function Timing({ label, value, max }: { label: string; value: number; max: number }) {
  return (
    <div className="timing">
      <span>{label}</span>
      <div><i style={{ width: `${Math.min(100, (value / max) * 100)}%` }} /></div>
      <strong>{value}ms</strong>
    </div>
  );
}

function TrendBars({ logs }: { logs: RequestLogItem[] }) {
  const values = logs.length
    ? logs.slice(0, 18).map((log) => Math.max(8, Math.min(100, ((log.latencyMs ?? 1000) / 4000) * 100)))
    : Array.from({ length: 18 }, () => 8);
  return (
    <div className="trend-bars">
      {values.map((value, index) => (
        <span key={index} style={{ height: `${value}%` }} />
      ))}
      <div className="trend-legend">
        <Badge tone="info">TTFT</Badge>
        <Badge tone="warn">Total Latency</Badge>
        <Badge tone="bad">Error Rate</Badge>
      </div>
    </div>
  );
}

function addProviderFromTemplate(updateConfig: PageProps["updateConfig"]) {
  return updateConfig((draft) => {
    const existingNames = new Set(draft.aiGateway.providers.map((provider) => provider.name));
    const template = providerTemplates.find((provider) => !existingNames.has(provider.name)) ?? providerTemplates[0];
    draft.aiGateway.providers = [...draft.aiGateway.providers, structuredClone(template)];
    if (draft.aiGateway.codexVisibleModels.length === 0) {
      draft.aiGateway.codexVisibleModels = [...template.models];
    }
  }, "添加 Provider");
}

function RowActions() {
  return (
    <div className="row-actions">
      <button title="编辑"><Pencil size={14} /></button>
      <button title="测试"><Activity size={14} /></button>
      <button title="删除"><Trash2 size={14} /></button>
    </div>
  );
}

function Banner({ tone, text }: { tone: Tone; text: string }) {
  return <div className={`banner ${tone}`}>{text}</div>;
}

function providerTypeLabel(provider: ProviderConfig) {
  if (provider.compatibility === "glm") return "GLM / Anthropic";
  if (provider.providerType === "openai_responses") return "Responses";
  if (provider.providerType === "chat_completions") return "Chat";
  if (provider.providerType === "anthropic_messages") return "Anthropic";
  return provider.providerType;
}

function providerLatency(provider: ProviderConfig) {
  const seed = provider.name.split("").reduce((sum, char) => sum + char.charCodeAt(0), 0);
  return 220 + (seed % 480);
}

function modelSet(providers: ProviderConfig[]) {
  const models = new Set<string>();
  providers.forEach((provider) => {
    provider.models?.forEach((model) => models.add(model));
    Object.keys(provider.modelAliases ?? {}).forEach((model) => models.add(model));
  });
  return Array.from(models);
}

function channelConnected(dashboard: DashboardResponse | null, channel: string) {
  const status = dashboard?.status;
  if (!status) return false;
  const value = channel === "feishu" ? status.feishuWs : channel === "telegram" ? status.telegram : status.wechat;
  return Boolean(value && Object.values(value).some((item) => item === true || item === "connected" || item === "running"));
}

function imRows(dashboard: DashboardResponse | null) {
  const accounts = dashboard?.imAccounts.accounts ?? [];
  const base = [
    { platform: "飞书", name: "飞书机器人", account: "default", enabled: true, connected: channelConnected(dashboard, "feishu"), detail: "飞书桥接运行中", session: "1", lastActive: "刚刚" },
    { platform: "Telegram", name: "Telegram Bot", account: "token 未配置", enabled: true, connected: channelConnected(dashboard, "telegram"), detail: "添加 Telegram Bot Token", session: "-", lastActive: "-" },
    { platform: "微信", name: "微信机器人", account: "扫码接入", enabled: true, connected: channelConnected(dashboard, "wechat"), detail: "扫码添加微信机器人", session: "-", lastActive: "-" }
  ];
  if (!accounts.length) return base;
  return base.map((row) => {
    const found = accounts.find((account) => JSON.stringify(account).toLowerCase().includes(row.platform.toLowerCase()));
    return found ? { ...row, connected: Boolean(found.connected ?? found.enabled), account: String(found.accountId ?? found.displayName ?? row.account) } : row;
  });
}

function isErrorLog(log: RequestLogItem) {
  const code = log.httpStatus ?? 0;
  return code >= 400 || ["failed", "error", "rate_limited"].includes(String(log.status ?? ""));
}

function successRate(logs: RequestLogItem[]) {
  if (!logs.length) return 0;
  const ok = logs.filter((log) => !isErrorLog(log)).length;
  return Math.round((ok / logs.length) * 1000) / 10;
}

function average(values: number[]) {
  const usable = values.filter((value) => value > 0);
  if (!usable.length) return 0;
  return Math.round(usable.reduce((sum, value) => sum + value, 0) / usable.length);
}

function formatTime(timestamp?: number | null) {
  if (!timestamp) return "-";
  return new Date(timestamp).toLocaleTimeString();
}

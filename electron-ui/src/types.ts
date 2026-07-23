export type Tone = "good" | "warn" | "bad" | "neutral" | "info";

export interface ProviderConfig {
  name: string;
  enabled: boolean;
  providerType: "openai_responses" | "chat_completions" | "anthropic_messages" | string;
  compatibility?: string | null;
  baseUrl: string;
  modelsUrl?: string | null;
  apiKey: string;
  models: string[];
  modelAliases?: Record<string, string>;
  promptCacheRetention?: string | null;
  weight: number;
  timeoutSecs: number;
}

export interface AiGatewayConfig {
  enabled: boolean;
  promptCacheRetention?: string | null;
  providers: ProviderConfig[];
  codexVisibleModels: string[];
  filterImageGenerationTool: boolean;
  requestLoggingEnabled: boolean;
}

export interface AppConfig {
  bind: string;
  localConnectionMode: "standard" | "vpnCompatible" | string;
  language?: string | null;
  theme?: string | null;
  codexAppFastStartup: boolean;
  statePath: string;
  logging: {
    diagnostic: boolean;
    maxMb: number;
    retentionDays: number;
    logDir?: string | null;
  };
  bridge: {
    enabled: boolean;
    accountId: string;
    sendStreaming: boolean;
  };
  telegram: Record<string, unknown>;
  wechat: Record<string, unknown>;
  feishu: Record<string, unknown>;
  aiGateway: AiGatewayConfig;
  [key: string]: unknown;
}

export interface DashboardResponse {
  status: {
    running: boolean;
    bind: string;
    localConnectionMode: string;
    codexAppFastStartup: boolean;
    statePath: string;
    feishuWs?: Record<string, unknown>;
    telegram?: Record<string, unknown>;
    wechat?: Record<string, unknown>;
    imAccounts?: unknown[];
  };
  remote: {
    connected?: boolean;
    initialized?: boolean;
    serverName?: string | null;
    currentThreadId?: string | null;
    [key: string]: unknown;
  };
  codexApp: {
    configured?: boolean;
    configPath?: string | null;
    codexHome?: string | null;
    provider?: {
      name?: string;
      baseUrl?: string | null;
      key?: string | null;
      supportsWebsockets?: boolean;
    } | null;
    providers?: Array<{ name: string; baseUrl?: string | null; key?: string | null }>;
    [key: string]: unknown;
  };
  imAccounts: {
    accounts?: ImAccount[];
    [key: string]: unknown;
  };
  aiGateway: AiGatewayConfig;
}

export interface ImAccount {
  accountId?: string;
  displayName?: string;
  platform?: string;
  kind?: string;
  enabled?: boolean;
  connected?: boolean;
  status?: string;
  [key: string]: unknown;
}

export interface EventItem {
  id?: number | string;
  level?: string;
  kind?: string;
  message?: string;
  createdAtMs?: number;
  timestampMs?: number;
  [key: string]: unknown;
}

export interface RequestLogItem {
  id: number;
  requestId?: string | null;
  createdAtMs?: number;
  modelId?: string | null;
  providerName?: string | null;
  channel?: string | null;
  providerType?: string | null;
  stream?: boolean | null;
  status?: string | null;
  httpStatus?: number | null;
  ttftMs?: number | null;
  latencyMs?: number | null;
  totalTokens?: number | null;
  [key: string]: unknown;
}

export interface RequestLogDetail {
  log?: RequestLogItem;
  detail?: RequestLogItem;
  [key: string]: unknown;
}

export interface LoggingStatus {
  logDir: string;
  activeLogPath: string;
  diagnostic: boolean;
  maxMb: number;
  retentionDays: number;
}

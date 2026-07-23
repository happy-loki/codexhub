import type { AppConfig, DashboardResponse, EventItem, LoggingStatus, RequestLogDetail, RequestLogItem } from "./types";

export class GatewayApi {
  constructor(public readonly baseUrl: string) {}

  async dashboard(): Promise<DashboardResponse> {
    return this.get("/api/gui/dashboard");
  }

  async config(): Promise<AppConfig> {
    return this.get("/api/config");
  }

  async saveConfig(config: AppConfig): Promise<{ ok: boolean }> {
    return this.post("/api/config", config);
  }

  async events(): Promise<EventItem[]> {
    return this.get("/api/events");
  }

  async loggingStatus(): Promise<LoggingStatus> {
    return this.get("/api/logging/status");
  }

  async clearLogging(): Promise<{ ok: boolean; chainLogsDeleted?: number; requestLogsDeleted?: number }> {
    return this.post("/api/logging/clear", {});
  }

  async requestLogs(): Promise<{ logs: RequestLogItem[] }> {
    return this.get("/ai-gateway/request-logs");
  }

  async requestLogDetail(id: number): Promise<RequestLogDetail> {
    return this.get(`/ai-gateway/request-logs/${id}`);
  }

  async clearRequestLogs(): Promise<{ ok: boolean; deleted?: number }> {
    return this.delete("/ai-gateway/request-logs");
  }

  async configureCodexApp(body: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
    return this.post("/api/codex-app/configure", body);
  }

  async uninstallCodexApp(): Promise<Record<string, unknown>> {
    return this.post("/api/codex-app/uninstall", {});
  }

  async refreshModels(): Promise<Record<string, unknown>> {
    return this.post("/api/codex-app/models/refresh", {});
  }

  async startFeishuOnboard(): Promise<Record<string, unknown>> {
    return this.post("/api/feishu/onboard/start", {});
  }

  async startWechatOnboard(): Promise<Record<string, unknown>> {
    return this.post("/api/wechat/onboard/start", {});
  }

  async configureTelegram(body: Record<string, unknown>): Promise<Record<string, unknown>> {
    return this.post("/api/telegram/configure", body);
  }

  private async get<T>(path: string): Promise<T> {
    return this.request<T>(path, { method: "GET" });
  }

  private async post<T>(path: string, body: unknown): Promise<T> {
    return this.request<T>(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body)
    });
  }

  private async delete<T>(path: string): Promise<T> {
    return this.request<T>(path, { method: "DELETE" });
  }

  private async request<T>(path: string, init: RequestInit): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, init);
    const text = await response.text();
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${text}`);
    }
    return text ? (JSON.parse(text) as T) : ({} as T);
  }
}

export function truncateMiddle(value: string, max = 42): string {
  if (value.length <= max) return value;
  const left = Math.ceil((max - 1) / 2);
  const right = Math.floor((max - 1) / 2);
  return `${value.slice(0, left)}...${value.slice(value.length - right)}`;
}

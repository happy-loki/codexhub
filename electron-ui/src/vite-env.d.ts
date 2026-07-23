/// <reference types="vite/client" />

interface GatewayDesktopApi {
  getBackendInfo(): Promise<{
    baseUrl: string;
    version: string;
    platform: string;
    managedDaemon: boolean;
  }>;
  openExternal(url: string): Promise<void>;
  openPath(targetPath: string): Promise<void>;
  quit(): Promise<void>;
}

interface Window {
  gateway?: GatewayDesktopApi;
}

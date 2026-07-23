const { app, BrowserWindow, ipcMain, shell, Tray, Menu, nativeImage } = require("electron");
const path = require("node:path");
const fs = require("node:fs");
const { spawn } = require("node:child_process");

let mainWindow = null;
let daemon = null;
let tray = null;

const baseUrl = process.env.CODEX_REMOTE_GATEWAY_BASE_URL || "http://127.0.0.1:3847";
const managedDaemon = process.env.CODEX_REMOTE_GATEWAY_MANAGED_DAEMON !== "0";

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1440,
    height: 900,
    minWidth: 1120,
    minHeight: 720,
    title: "Codex Remote Gateway",
    backgroundColor: "#f6f7f9",
    titleBarStyle: process.platform === "darwin" ? "hiddenInset" : "default",
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false
    }
  });

  mainWindow.on("close", (event) => {
    if (!app.isQuitting) {
      event.preventDefault();
      mainWindow.hide();
    }
  });

  if (process.env.VITE_DEV_SERVER_URL) {
    mainWindow.loadURL(process.env.VITE_DEV_SERVER_URL);
  } else {
    mainWindow.loadFile(path.join(__dirname, "..", "dist", "index.html"));
  }
}

function createTray() {
  const iconPath = path.join(__dirname, "..", "..", "packaging", "icons", "dolphin-rounded-48.png");
  const image = fs.existsSync(iconPath) ? nativeImage.createFromPath(iconPath) : nativeImage.createEmpty();
  tray = new Tray(image);
  tray.setToolTip("Codex Remote Gateway");
  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: "Open Codex Remote Gateway", click: () => showMainWindow() },
      { type: "separator" },
      { label: "Quit", click: () => quitApp() }
    ])
  );
  tray.on("click", () => showMainWindow());
}

function showMainWindow() {
  if (!mainWindow) return;
  mainWindow.show();
  mainWindow.focus();
}

function startDaemon() {
  if (!managedDaemon) return;
  const bin = resolveDaemonBinary();
  if (!bin) return;

  const args = [];
  if (process.env.CODEX_REMOTE_GATEWAY_CONFIG) {
    args.push("--config", process.env.CODEX_REMOTE_GATEWAY_CONFIG);
  }
  args.push("daemon");

  daemon = spawn(bin, args, {
    stdio: "ignore",
    windowsHide: true,
    env: {
      ...process.env,
      CODEX_REMOTE_GATEWAY_ELECTRON_CHILD: "1"
    }
  });
  daemon.on("exit", () => {
    daemon = null;
  });
}

function resolveDaemonBinary() {
  const explicit = process.env.CODEX_REMOTE_GATEWAY_BIN;
  if (explicit && fs.existsSync(explicit)) return explicit;

  const exe = process.platform === "win32" ? "codex-remote-gateway.exe" : "codex-remote-gateway";
  const candidates = [
    path.join(process.resourcesPath || "", exe),
    path.join(__dirname, "..", "..", "target", "release", exe),
    path.join(__dirname, "..", "..", "target", "debug", exe),
    path.join(__dirname, "..", "..", exe)
  ];
  return candidates.find((candidate) => candidate && fs.existsSync(candidate));
}

async function quitApp() {
  app.isQuitting = true;
  if (daemon) {
    try {
      await fetch(`${baseUrl}/api/shutdown`, { method: "POST" });
    } catch (_) {
      daemon.kill();
    }
  }
  app.quit();
}

ipcMain.handle("gateway:getBackendInfo", () => ({
  baseUrl,
  version: app.getVersion(),
  platform: process.platform,
  managedDaemon
}));

ipcMain.handle("gateway:openExternal", async (_event, url) => {
  await shell.openExternal(url);
});

ipcMain.handle("gateway:openPath", async (_event, targetPath) => {
  if (!targetPath || typeof targetPath !== "string") return;
  const error = await shell.openPath(targetPath);
  if (error) throw new Error(error);
});

ipcMain.handle("gateway:quit", async () => {
  await quitApp();
});

app.whenReady().then(() => {
  startDaemon();
  createWindow();
  createTray();

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow();
    } else {
      showMainWindow();
    }
  });
});

app.on("before-quit", () => {
  app.isQuitting = true;
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") {
    app.quit();
  }
});

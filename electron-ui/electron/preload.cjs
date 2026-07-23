const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("gateway", {
  getBackendInfo: () => ipcRenderer.invoke("gateway:getBackendInfo"),
  openExternal: (url) => ipcRenderer.invoke("gateway:openExternal", url),
  openPath: (targetPath) => ipcRenderer.invoke("gateway:openPath", targetPath),
  quit: () => ipcRenderer.invoke("gateway:quit")
});

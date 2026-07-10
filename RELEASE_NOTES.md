CodexHub v0.3.34

本版本集中修复本地连接、代理环境和 daemon 生命周期问题：

- 新增系统代理、直连和自定义 HTTP/SOCKS5 出站代理模式。本地 GUI、Codex App、VS Code 与 CodexHub 之间的回环通信始终绕过代理，改善 VPN 和全局代理环境下的连接稳定性。
- 重构 remote-control 多连接状态管理，分别识别 Codex App、VS Code、CLI 和 IM 虚拟客户端，修复功能正常但面板显示未连接、重连后旧连接状态污染等问题。
- 加固 Windows 和 macOS daemon 进程管理：增加单实例锁和实例身份校验，可识别并清理占用端口但 HTTP 已失效的旧进程，等待端口释放后再启动，并在连续健康检查失败时自动恢复。
- 端口被非 CodexHub 进程占用时停止自动启动并明确报错，避免误杀其他应用。

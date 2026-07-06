CodexHub v0.3.25

这是一个针对 v0.3.24 升级问题的紧急修复版本，重点处理部分 Windows 用户升级后仍运行旧界面、多端连接状态被未初始化连接抢占、以及面板状态偶发跳变的问题。

## Windows 更新

- **修复自动更新覆盖失败风险**：Windows 自动更新下载 MSI 后，现在会先让 CodexHub 完整退出，再由后台 helper 启动安装器，避免安装器启动时旧进程仍占用 `CodexHub.exe`，导致用户升级后实际还在运行旧版本。
- **更新提示更准确**：下载完成后的提示改为说明“退出后会自动启动安装器”，避免用户误以为安装已经完成。

## Remote Control / 状态

- **优先保留已初始化连接**：当 Codex App / VSCode / CLI 同时或反复重连时，已初始化的连接优先于新建但未初始化的高优先级连接，避免状态在“成功/失败”之间跳动。
- **明确隔离 unknown 连接**：对已连接且已初始化但来源为 `unknown` 的旧连接，不再归到 Codex App / VSCode / CLI 任一明确终端，避免某一端误显示“已连接”。
- **修复 dashboard fallback 状态丢失**：当 GUI 聚合 dashboard 接口偶发超时或失败、但本地服务仍在线时，面板会继续单独读取 remote-control 和 Codex App 状态，不再把三端状态错误降级为“读取中”。

## 验证

- `cargo test --features gui`
- `rustfmt --edition 2024 --check src/gui.rs src/gui/api.rs src/gui/text.rs src/gui/update.rs src/remote_control_backend/client_state.rs src/remote_control_backend/tests.rs`
- `git diff --check -- src/gui.rs src/gui/api.rs src/gui/text.rs src/gui/update.rs src/remote_control_backend/client_state.rs src/remote_control_backend/tests.rs`

---

CodexHub v0.3.24

这是一个紧急修复版本，重点修复桌面面板状态显示混乱、本地服务误判超时，以及远程连接历史在内存和状态接口里无限累积的问题。

## GUI / 状态栏

- **统一连接状态文案**：Codex App、VSCode 插件、CLI 三类终端统一为更清晰的状态集合，包括 `未初始化配置`、`已连接`、`未连接`、`初始化中`、`读取中`，减少普通用户看到的中间状态和心智负担。
- **修复本地服务误判离线**：GUI dashboard 请求失败或超时时，会回退到轻量 `/api/status` 判断本地服务是否在线，不再把面板子状态接口的异常直接显示成“服务未运行”。
- **移除误导状态**：不再显示 `未注入`、`未打开控制`、`可接入` 等容易误解的状态。

## Remote Control / 内存

- **远程连接只保留当前活跃连接**：`remote.connections` 不再保存断开的历史连接，断开后立即从内存状态移除。
- **状态接口自动清理历史连接**：生成 `/api/remote-control/status` 快照前会清理非活跃连接，避免历史连接把响应撑到几十 MB 甚至上百 MB。
- **连接选择更稳定**：新连接在初始化完成前也可以被选为活跃连接，同时优先选择已初始化连接，避免 Codex App / VSCode 插件明明可用但状态显示不一致。

## 验证

- `cargo test --features gui`
- `rustfmt --edition 2024 --check src/gui.rs src/gui/api.rs src/gui/text.rs src/remote_control_backend/client_state.rs src/remote_control_backend/status.rs src/remote_control_backend/tests.rs src/remote_control_backend/websocket.rs`
- `git diff --check`

---

CodexHub v0.3.23

本次为聚焦修复版本，解决插件目录里出现"能看见但装不了"的插件问题，让 API 登录环境下的插件列表回归可用状态。

## 插件 — 移除重复且无法安装的远程目录

- **停止提供重复的 curated-remote 目录**：Codex 桌面端会合并两个独立的插件来源——本地磁盘的 `openai-curated` marketplace（显示为"Codex official"），以及 codexhub 通过 HTTP 暴露的"远程"目录 `/backend-api/ps/plugins/*`。此前 codexhub 把同一批 curated 插件又用远程目录暴露了一遍，导致桌面端多出一个"OpenAI Curated Remote"标签页，而该标签页里的插件安装会走远程分支、因缺少 HTTPS `bundle_download_url` 而报 `MissingBundleDownloadUrl`（即"插件安装失败"）。现在 `ps/plugins/list` 返回空目录，去掉这个装不了的重复标签页，本地"Codex official"标签页原样保留、安装照常成功。
- **精选插件指向本地目录**：featured 插件 id 改为指向本地 `openai-curated` marketplace，让高亮命中真正存在、可安装的条目。
- **过滤依赖远程后端的插件**：配置初始化时，从磁盘 marketplace 清单中过滤掉需要远程 OpenAI 后端的插件（`.app.json` 连接器，以及 HTTP/SSE 传输的 `.mcp.json`），只保留本地可用的技能类与本地 stdio MCP 插件。
- **强制关闭 host 托管的 codex_apps 并清理连接器缓存**：将 `features.apps` 固定为 `false`，并在配置流程中清除过期的连接器目录缓存，避免 Gmail / Google Drive / GitHub 等远程连接器泄漏进插件目录。

---

CodexHub v0.3.22

这是一个大版本累积更新，涵盖了 AI Gateway 路由与流式、GUI 性能与桌面体验、macOS 构建体系、Anthropic/Claude Code 对齐等多个方向的改进。

## macOS — 真正的 Universal Binary

- **macOS Universal Binary**：在 macos-15 (Apple Silicon) runner 上通过交叉编译，产出同时支持 arm64 和 x86_64 的通用二进制文件。一个 DMG 搞定两种芯片，不再需要单独的 Intel 构建。
- **wxWidgets 构建增强**：wxdragon-sys build.rs 新增 `WX_OSX_ARCHITECTURES` 环境变量支持，允许传入 `"arm64;x86_64"` 让 CMake 直接构建 fat library，为实现跨架构 Universal Binary 铺平了道路。
- **macOS CI 重构**：移除了对已弃用的 macos-13 runner 的依赖（该 runner 在 2025-2026 年已基本不可用，导致 Intel 构建长期失败）。改为单 macos-15 job，构建 + 双 target 交叉编译 + lipo 合并的流水线，产物命名统一为 `macos-universal`。

## AI Gateway — 优先级路由与会话粘性

- **优先级路由**：支持基于权重的多 provider 优先级路由，同一模型的多个 provider 按权重高低择优，权重相同的组内通过 HRW Hash 按 session 稳定分流。
- **会话粘性绑定**：同一 session 始终路由到同一 provider/endpoint，充分利用 Anthropic prompt cache，避免缓存失效。
- **熔断与自动恢复**：上游 provider 连续失败达到阈值后自动拉黑（circuit breaker），冷却时间过后自动恢复；全 provider 被拉黑时仍有优先级兜底，避免 500。

## AI Gateway — 流式传输改进

- **GLM 流式去缓冲**：GLM 明文输出改为 token-by-token 实时推送，不再累积后批量输出，交互延迟大幅降低。
- **Anthropic 内部 web-search 流式**：内部 web-search 路径改为 token-by-token 流式，注入的 web-search call 作为单条非流式 item 正确分发。
- **响应流式日志**：上游 OpenAI Responses SSE 日志完整捕获，TTFT 计时修复。

## Anthropic / Claude Code 对齐

- **缓存断点对齐**：cache_control 断点策略回归 Claude Code 形态——system 最后一个 text block、messages 尾部消息的最后一个 text block 各打一个断点，tools 不打断点。
- **双滚动断点**：消息历史采用 dual rolling cache_control breakpoints，更充分利用上下文窗口。
- **Headers 指纹**：headers / anthropic-beta / auth 全面对齐 Claude Code，包含 `context-1m-2025-08-07` 等必要头部。
- **Web Search 历史映射**：Anthropic web search 的并行 tool call results 和 Responses streaming 映射对齐。

## GUI — 性能优化

- **轮询改为空闲驱动**：GUI dashboard 从周期性定时器轮询改为 idle-driven 事件驱动，CPU 占用和功耗显著降低。
- **请求日志计时器优化**：无活动时自动停止 request log 计时器。
- **wxDragon 升级至 0.9.17**：GUI 框架同步上游最新版。

## 桌面体验

- **系统托盘 / 菜单栏**：Windows 系统托盘 + macOS 菜单栏状态项，关闭窗口默认隐藏而非退出，菜单提供退出入口。
- **自动更新准备**：各平台更新元数据（latest-*.json / appcast-*.xml）已接入 CI 骨架。
- **下载进度对话框**：更新下载时显示实时进度条。
- **Codex App 快速启动**：修复与 Codex App 的兼容性，支持快速启动路径。

## Windows

- **WiX v7 MSI 打包**：接受 WiX v7 EULA，支持桌面和开始菜单快捷方式。
- **代码签名支持**：CI 可选 Windows 代码签名，有证书后自动签名 exe 和 msi。

## 通用改进

- **OTLP 导出器默认关闭**：修复无 VPN 环境下 Codex App 启动卡顿的问题（OpenAI OTLP exporter 在网络不通时长时间超时）。
- **请求日志默认关闭**：可通过配置启用，减少日志 I/O 开销。
- **上游请求重试**：transport 级别错误（connect/body/request）自动重试最多 2 次，配合指数退避。
- **AI Gateway provider UX**：session 操作改用 AI Gateway 标签，UI 文案和 headers 体验优化。

## 验证

- `cargo fmt`
- `cargo test`（全部通过）

有问题可以提 GitHub issue，也可以关注 README 里的公众号后直接发消息给我。

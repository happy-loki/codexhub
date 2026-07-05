Codex Remote Gateway v0.3.22

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

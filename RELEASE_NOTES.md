CodexHub v0.4.9

本次版本重点修复 Codex App 增强模式下插件市场无法显示的问题，并改进增强启动的早期注入稳定性。

## 插件市场

- 兼容新版 Codex App 的 `plugin/list` MCP 消息协议。
- 修复插件桥接已安装但 `pluginCatalogResponsesAdapted=0`、本地 curated 插件被前端隐藏的问题。
- 增强模式下，本地 `openai-curated` 会在 renderer 中显示为 `Codex official`，内部插件 ID、安装状态、绝对路径和配置身份保持不变。
- 本地 curated 目录可正常展示约 25 个插件；`openai-curated-remote` 仍保持隐藏，避免出现无法安装的远程插件条目。
- 保留旧版 `vscode://codex/list-plugins` 协议兼容，降低 Codex App 版本变化带来的影响。

## 增强模式启动

- 通过 browser CDP 的 `Target.setAutoAttach` 在 renderer 释放前注入脚本，减少启动阶段错过首个消息的情况。
- 增加 browser/page CDP 会话生命周期管理和早期注入诊断日志。
- 增强脚本升级到 v14；插件桥接状态和实际适配次数会写入启动诊断。
- 不修改全局 `authMethod`，不改变普通启动、Codex CLI 或 VS Code 插件行为。

## 请求可靠性

- 上游请求体传输被网络中断后，可以重试上传，而不是直接结束本次请求。
- 重试复用原始请求体，不改变请求语义。

## 验证

- 默认测试：535 passed，0 failed，2 ignored。
- GUI 测试：562 passed，0 failed，2 ignored。
- `cargo fmt --all -- --check` 通过。
- `cargo build --features gui --bin codexhub` 通过。
- 实机 CDP 热回放确认本地 curated 目录响应成功适配，并加载 25 个插件图标。

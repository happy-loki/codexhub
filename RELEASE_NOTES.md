CodexHub v0.4.14

本次版本修复 Anthropic 流式响应的 Token 统计，并适配新版 Codex App 的增强模式启动流程。

## Anthropic 流式 Token 统计

- 按 Anthropic SSE 的实际分帧语义分别合并未缓存输入、缓存读取和缓存写入 Token。
- 保留 `message_start` 首帧报告的非零输入 Token，避免被 `message_delta` 末帧中的零值覆盖。
- 当末帧提供非零输入 Token 时，以末帧最新快照为准，不与首帧重复相加。
- 修复缓存数据在末帧返回时，输入 Token 总数只包含缓存量或错误显示为零的问题。
- 补充基于真实请求日志结构的回归测试，覆盖首帧输入与末帧缓存分离上报的场景。

## Codex App 增强模式

- 适配新版 Codex renderer 的 JavaScript realm 变化，修复 `MutationObserver.observe` 参数不是 `Node` 导致的 HTTP 500 启动错误。
- 使用页面自身的 `MutationObserver` 观察插件页面，避免注入环境与页面 DOM 跨 realm 不兼容。
- 将插件目录快捷入口改为非关键增强：即使该入口安装失败，也不会中断模型列表、中文、Statsig 和插件目录数据桥接。
- 增强脚本版本升级至 20，确保升级后重新注入兼容脚本。

## 验证

- `cargo fmt --all -- --check` 通过。
- `cargo test --bin codexhub` 通过：569 passed，2 ignored。
- Anthropic provider 专项测试通过：84 passed。
- Codex App 增强模式专项测试通过：16 passed，2 ignored。

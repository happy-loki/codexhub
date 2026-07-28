CodexHub v0.4.13

本次版本重点新增企业微信接入，完善第三方模型工具协议与 Anthropic 缓存，并修复新版 Codex App 中本地精选插件目录无法进入的问题。

## 企业微信接入

- 新增企业微信 AI Bot WebSocket 接入和扫码配置流程。
- 支持私聊与群聊文本消息、流式回复、最终回复以及图片和文件收发。
- 支持新建会话、恢复历史会话、切换模型和工作目录等交互卡片。
- 支持 Codex 审批模板卡片，可直接在企业微信内处理审批请求。
- 个人微信与企业微信在 GUI 中并列展示，并使用独立连接状态和企业微信官方图标。

## IM 稳定性与体验

- 统一飞书、Telegram、微信和企业微信的文本适配与出站消息处理。
- 完善 IM 路由、会话列表和运行状态管理，避免不同平台消息状态互相干扰。
- 优化微信长回复、菜单指令、图片处理和流式消息合并。
- 补充企业微信用户与群聊白名单配置，并纳入诊断导出。

## 第三方模型工具协议

- Anthropic 请求在 tools 尾部增加缓存断点，提升稳定工具定义的缓存命中率。
- 规范化 Grok 工具搜索结果，并兼容 apply_patch 等自定义工具的历史回放。
- DeepSeek 请求自动去重同名工具，修复跨模型会话中 `Tool names must be unique` 错误。
- OpenAI Responses 和 Responses Lite 继续保持原生透传，不改写未来字段。

## Codex App 插件目录

- 适配 Codex App 26.721.4979 的新版插件页面和 React 内存路由。
- 保留本地 `openai-curated` 精选插件目录，并排除不需要的远程市场目录。
- 在已安装插件管理页增加“浏览插件”入口，可进入 Codex 官方目录页查看和安装完整精选插件。
- 延长冷启动时插件缓存刷新窗口，避免 renderer 挂载较慢时只显示少量已安装插件。

## 验证

- `cargo fmt --all -- --check` 通过。
- `cargo test --bin codexhub` 通过：568 passed，2 ignored。
- `cargo check --features gui --bin codexhub` 通过。
- 已在 Windows Codex App 26.721.4979 中验证插件目录入口和完整精选插件展示。

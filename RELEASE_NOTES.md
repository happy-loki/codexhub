CodexHub v0.3.32

紧急修复连接状态和新模型工具调用问题：

- 修复 Codex App remote-control 已可用但状态仍显示未连接/初始化中的问题。
- 适配新版 VS Code Codex 插件启动参数，确保自动注入 remote-control 仍然生效。
- 修复部分 Responses 新模型返回 `custom_tool_call namespace=exec` 时 Codex 执行成 `execexec` 的问题。

CodexHub v0.4.15

本次版本重点修复 DeepSeek 工具调用兼容性，并恢复 Codex App 已有分页历史会话的访问能力。

## DeepSeek 工具兼容

- Responses 转 Chat Completions 时，自动为缺少根类型的函数参数补充 `type: "object"`。
- 保留 `$defs`、`oneOf` 等完整 JSON Schema 内容，不再因为规范化而丢失工具约束。
- 修复 Codex App 自动化工具等 schema 根类型为空时，DeepSeek 返回 `Invalid schema for function` 的问题。
- 增加缺失参数、已有对象类型以及复杂 schema 的回归测试。

## Codex App 会话恢复

- 恢复分页历史 gate，使此前由增强模式创建的分页会话可以正常打开。
- 修复关闭该 gate 后，点击历史会话但 renderer 不发送 `thread/read`、`thread/resume` 或 `thread/turns/list` 的问题。
- 移除当前 renderer 已无引用的旧 gate，减少不必要的本地门控覆盖。
- 同步更新 Statsig bootstrap、增强模式注入与兼容性文档。

## 已知限制

- 当前 Codex App renderer 仍明确禁止分页会话执行 fork、消息编辑和 rollback；最新版 app-server 已具备部分后端能力，但官方前端尚未开放。
- CodexHub 本次优先保证已有会话可访问，不修改 ASAR，也不拦截 renderer 资源；相关功能等待 Codex App 官方完善。

## 验证

- `cargo fmt --check` 通过。
- `cargo check --features gui --bin codexhub` 通过。
- `cargo test --release --features gui` 通过：598 passed，2 ignored。

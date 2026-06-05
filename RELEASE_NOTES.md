# Codex Remote v0.2.11

本次版本调整 IM 会话绑定语义，并改善飞书 CardKit 流式回复的结束体验。

## 更新内容

- 移除 `persisted.sessions` 持久化绑定，重启 `codex-remote` 后不会自动把 IM 会话接回旧 Codex thread。
- 飞书、Telegram、微信现在统一只认当前进程内的活跃 IM route；重启后的第一条普通 IM 消息会回到新建/恢复 thread 的选择流程。
- Codex 输出不再通过历史本地 state 反向恢复 IM 路由，避免 Codex 收到消息但 IM 侧无法收到回复的半失效状态。
- 飞书 CardKit 流式节流恢复为 100ms，并在完成时改回直接更新最终卡片后关闭 streaming mode，减少长回复结束阶段的卡顿感。

## 兼容性说明

- 旧 `codex-remote-state.json` 中如果仍包含 `sessions` 字段，会被忽略，不需要手动清理。
- 历史 thread 恢复仍通过 IM 端的新建/恢复选择流程和 Codex thread list 完成，不再依赖本地持久化绑定。

## 验证

- `cargo fmt`
- `cargo test`
- `cargo build --release --features gui --bin codex-remote`

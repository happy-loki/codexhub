# Codex Remote v0.2.14

本次版本重点修复 IM 多通道同时连接 Codex 时的 thread/stream 隔离，补齐图片输出链路，并降低 remote-control 广播噪音。

## 更新内容

- IM 会话改为基于 platform/account/chat 生成确定性 `im:<platform>:<hash>` remote client key，飞书、微信、Telegram 多机器人或多群同时使用时不会共用 `default` stream。
- 创建、恢复、发送 turn、审批回调、线程列表等 IM API 全部使用 route 绑定的确定性 remote client key，不再回退到模糊的 current thread。
- remote-control 恢复逻辑按 route key 重新订阅已绑定 thread，断链恢复后仍回到原 IM 会话对应的 thread。
- 过滤 Codex app-server 广播到非 owner stream 的 thread 通知：保留 ack 和低频 skip 日志，但不再进入 IM 投递链。
- 支持 agent message / tool output 中的本地图片路径上传到飞书、Telegram、微信对应 IM。
- 拆分 remote-control backend、web bridge、Feishu renderer 相关大文件，降低维护成本。
- 更新 GUI/onboarding/web 代码结构和移动端预览文档。

## 兼容性说明

- IM route 现在以确定性 remote client key 为准；旧运行时内存里绑定到 `default` 的 route 不做兼容迁移，升级后建议重启服务并重新绑定需要继续订阅的 thread。
- 日志里仍可能看到 app-server 将 `thread/started`、`thread/status/changed` 广播到多个 stream；非 owner 广播会被记录为 `notification_broadcast_skipped`，不会投递到错误 IM。

## 验证

- `cargo fmt`
- `cargo check --features gui --bin codex-remote`
- `cargo test --features gui --bin codex-remote remote_control_backend::tests`
- `cargo build --release --features gui --bin codex-remote`

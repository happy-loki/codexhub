CodexHub v0.4.18

本次版本修复 Windows 本地服务启动卡死问题，并增强启动阶段诊断能力。

## Windows 启动修复

- 让 CodexHub daemon 先监听 `127.0.0.1:3847`，再同步 Codex App 环境变量。
- Windows 环境变量广播不再阻塞本地 API 服务启动。
- 环境变量没有变化时不再重复写注册表或广播系统消息。
- 避免因 Clash、Windows 安全中心或其他桌面程序响应缓慢，导致本地服务启动超时并反复重启。

## 启动诊断

- 增加端口绑定、监听成功、环境同步和 Windows 环境广播耗时日志。
- 即使环境同步异常，CodexHub 本地服务仍可先启动并响应状态接口。

## 验证

- `cargo fmt -- --check` 通过。
- `cargo check --features gui --bin codexhub` 通过。
- GitHub Actions 将在 Windows、macOS 和 Linux 上构建并上传安装包。

CodexHub v0.4.17

本次版本重点修复飞书图片交互与重复回复问题，并修复 macOS GUI 启动异常。

## 飞书图片交互

- 飞书发送纯图片时，不再向 Codex 创建正文为空的用户消息。
- 收到纯图片后会提示用户补充说明；下一条文字会自动与图片合并，再交给 Codex 处理。
- 支持连续发送多张图片，最多暂存最近 8 张，超过 10 分钟未补充说明会自动失效。
- 不同飞书会话的待处理图片相互隔离，服务重连后会清理失效状态。
- 图片附带文字时仍按原流程立即处理，不增加额外操作。

## 飞书回复修复

- 修复流式回复完成后，相同正文又被静态卡片重复发送一次的问题。
- 保留原有流式展示和“已完成”状态提示。
- 增加重复回复跳过日志，方便后续定位消息投递问题。

## macOS 稳定性

- 修复 macOS GUI 启动时 Tokio runtime 初始化顺序不正确导致的 panic。
- 保持命令行模式与其他平台启动行为不变。

## 验证

- `cargo test` 通过：578 passed，2 ignored。
- `cargo check --features gui --bin codexhub` 通过。
- `git diff --check` 通过。
- GitHub Actions 将在 Windows、macOS 和 Linux 上构建并上传安装包。

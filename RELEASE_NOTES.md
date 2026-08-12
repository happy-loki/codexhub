CodexHub v0.4.22

本次版本同步最新模型配置，并收敛大模型厂商配置界面。

## 模型更新

- Grok 模型统一更新为旗舰模型 `grok-4.6`，移除 `grok-4.5` 的目录、默认配置和界面引用。
- DeepSeek Pro 默认使用原生 Responses 接口。
- DeepSeek Responses 同时支持 `deepseek-v4-pro` 和 `deepseek-v4-flash`，默认选择 Pro。

## 厂商配置界面

- 隐藏“Chat Completions（其他厂商）”入口，避免用户误将 DeepSeek Pro 配置到旧 Chat 协议。
- 底层 Chat Completions 类型和转换代码继续保留，方便后续接入其他仅支持 Chat 协议的厂商。
- 已有旧 Chat 配置仍可读取和编辑，不会被自动删除。

## 验证

- `cargo fmt --check` 通过。
- `cargo check --features gui --bin codexhub` 通过。
- 完整测试通过：679 passed，2 ignored。
- GitHub Actions 将构建 Windows、macOS 和 Linux 安装包。

CodexHub v0.4.21

本次版本重点完善 Telegram 远程任务体验，并修复 DeepSeek Responses 会话中工具调用历史不完整导致的请求失败。

## Telegram 任务体验

- 聚合展示命令、MCP 工具、推理、计划、文件变更和子任务进度，减少消息刷屏。
- 支持流式草稿更新和最终状态收口，任务失败时也能明确结束，不再长时间停留在执行中。
- 支持 Telegram 图片、文件、音频和语音附件，并增加大小、数量和过期限制。
- 增强轮询冲突、网络超时和 Telegram API 限流的退避处理，降低高频重试风险。
- MCP 工具返回图片时单独发送图片，同一工具完成事件只发送一次。

## DeepSeek Responses

- 修复会话历史中工具调用与工具结果不成对时，上游返回 `No tool output found` 的问题。
- 缺少结果的孤儿工具调用会被移除；缺少调用的工具结果会降级为普通上下文，尽量保留有效信息。
- 修复仅作用于 DeepSeek Responses，OpenAI Responses 和 Grok 原生透传保持不变。

## 验证

- `cargo fmt --check` 通过。
- 完整测试通过：677 passed，2 ignored。
- GitHub Actions 将构建 Windows、macOS 和 Linux 安装包。

CodexHub v0.4.20

本次版本调整 DeepSeek 模型的上下文窗口，避免 1M 上下文声明带来的超长会话性能和稳定性问题。

## DeepSeek 上下文

- `deepseek-v4-pro` 的上下文窗口和最大上下文窗口调整为 372K。
- `deepseek-v4-flash` 的上下文窗口和最大上下文窗口调整为 372K。
- 继续保留 95% 的有效上下文安全比例，约在 353K 时进入压缩边界。
- DeepSeek 的搜索、工具调用、推理等级和协议能力保持不变。

## 验证

- 内置模型目录 JSON 解析通过。
- DeepSeek 模型能力测试通过。

CodexHub v0.4.19

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

## 发布修复

- 修复 macOS notarization 重试参数在 Bash 严格模式下触发 `unbound variable`，确保 macOS 安装包可以正常发布。

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

CodexHub v0.4.27

本次更新同步 GPT 模型目录，并修复从实验版本切回正式版本时的配置兼容问题。

## GPT 模型目录

- 新增 `gpt-6-astra`，完整同步 Codex 最新目录中的 GPT-5.5、GPT-5.6-Sol/Terra/Luna 和 GPT-6-Astra 条目。
- 同步工具能力、模型指令、思考等级和上下文设置。GPT-5.6 系列与 GPT-6-Astra 默认上下文为 272,000，上限为 872,000。
- 移除内置 GPT-5.4 和 GPT-5.4-mini 条目，第三方模型配置保持不变。

## 配置兼容

- 配置文件中存在当前版本不支持的渠道类型时，跳过该渠道并记录提示，其他渠道和本地服务可正常启动。
- 保存到原配置文件时，保留未识别渠道的原始字段和 API Key，避免从实验版本回退后丢失配置。
- 配置语法损坏或已支持渠道的字段类型错误仍会提示，不会静默忽略。
- Gemini 功能仍处于独立实验分支，本次正式版本不包含 Gemini 接入。

## 验证

- 完整测试通过：691 passed，2 ignored。
- 未知渠道加载、重复保存保留、非法配置报错测试通过。
- GPT 模型目录测试已同步更新。

CodexHub v0.4.26

本次版本修复 Grok 无法稳定使用 Codex 图片查看工具的问题。

## Grok 图片查看

- 发往 Grok 时，将 Codex 的 `view_image(path)` 自动适配为 Grok 更熟悉的 `read_file(target_file)`。
- Grok 返回工具调用后，再还原为 Codex 原生的 `view_image(path)`，支持流式和非流式响应。
- 多轮会话中的历史工具调用会同步转换，避免后续请求因工具名称或参数不一致而失败。
- 当会话中同时存在真正的 `read_file` 工具时，会自动分配无冲突名称并保持双向还原。
- 适配仅作用于 Grok Responses，不改变 OpenAI、DeepSeek 和 Anthropic 的工具协议。

## 范围说明

- 保留 Codex 原始 `view_image` 工具说明，不额外修改提示词。
- 未加入 `ReasoningOnly` 或空响应自动重试，避免网关擅自发起额外模型请求。

## 验证

- 完整测试通过：689 passed，2 ignored。
- Grok 工具声明、历史回放、JSON/SSE 返回和名称冲突测试通过。
- `git diff --check` 通过。

CodexHub v0.4.25

本次版本同步 GLM 5.3 模型目录，并修复恢复 Codex 原有配置后历史会话无法继续打开的问题。

## 模型目录

- 新增 `GLM-5.3` 和 `GLM-5.3-Flash` 模型条目。
- 同步 GLM 模型的图片输入、搜索、推理和上下文能力配置。
- 保留 `availability_nux` 等 Codex 模型目录字段，确保模型列表显示一致。

## Codex App 配置恢复

- 点击“恢复 Codex 原有配置”时，仍会恢复用户原来的默认 provider。
- 不再删除 `[model_providers.ai-gateway]` 配置段。
- 旧的 ai-gateway 历史会话可以继续找到对应 provider，避免点击会话时报“未找到模型提供者 ai-gateway”。
- 不修改 rollout 历史文件和 Codex state SQLite 数据库。

## 验证

- `cargo fmt --all -- --check` 通过。
- 完整测试通过：682 passed，2 ignored。
- `git diff --check` 通过。

CodexHub v0.4.24

本次版本新增 DeepSeek 多模态识图支持，并保持原有模型选择方式不变。

- `deepseek-v4-flash` 现在支持图片理解。
- 图片请求会自动使用 DeepSeek 官方视觉模型处理。
- `deepseek-v4-pro` 保持文本和工具调用能力不变。

CodexHub v0.4.23

本次版本修复自动更新链路，避免残缺 Release、GitHub API 限流和过长更新说明影响用户升级。

## 自动更新

- 应用优先读取各平台的静态更新清单，不再回退到容易触发共享 IP 限流的 GitHub Releases API。
- 应用内更新说明与开发者 Release Note 分离，并限制为最多 4 行，避免 macOS 更新按钮被长文本挤出窗口。
- 更新检查失败时显示简洁提示，不再直接向普通用户展示多段 403/404 技术错误。
- 新增 Linux `latest-linux.json`，统一 Windows、macOS 和 Linux 的更新清单机制。

## 发布可靠性

- macOS 创建 DMG 前释放双架构 Rust 和 wxWidgets 构建中间文件，修复 GitHub Runner 磁盘不足导致的发布失败。
- Windows 和 Linux 只上传资产；macOS 在确认三个平台清单齐全后，才将 Release 晋升为 Latest。
- 发布失败时继续保留上一个完整版本为 Latest，避免旧客户端进入缺少平台清单的半成品 Release。

## 验证

- `cargo fmt --check` 通过。
- `cargo check --features gui --bin codexhub` 通过。
- 更新清单与发布流程防回归检查通过。

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

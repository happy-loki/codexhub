CodexHub v0.4.5

这是一次以 Codex App 体验和协议稳定性为核心的重要更新：新增增强模式启动，支持把 CodexHub 模型列表和中文能力同步到 Codex App；补齐 Grok 的 `apply_patch` / `web_search` 工具桥接，并改善模型目录、超长历史会话与请求日志维护。

## Codex App 增强模式

- 在 Codex 接入页提供“增强模式启动 Codex”，把 CodexHub 可见模型同步到 Codex App 前端；普通启动、Codex CLI 和 VS Code 插件不受影响。
- 不修改 Codex App 的 ASAR、LevelDB、快捷方式，也不再占用 `localhost:8000`；增强状态仅存在于本次 Codex App 进程。
- 启动时优先复用完整的官方 Statsig 缓存，只增量覆盖模型列表、中文能力和已确认的关键 gate，保留插件、runtime 与其他官方配置。
- 修复冷启动时 CDP 已连接但增强配置偶发不生效的问题：启动器会保持会话、跟踪 Codex 自身的 renderer 替换，并在 Statsig 就绪后继续完成注入。
- 不再通过 `Page.reload` 重试，避免与 Electron 首次加载竞争；同一 renderer 只在内部重试窗口结束后重新执行脚本，减少重复 timer。
- 有完整官方缓存时会在 React 路由挂载前复用，降低无 VPN 或官方 Statsig 网络不稳定造成的启动等待；缓存不可用时保留官方初始化路径，不伪造精简配置。
- 修复增强模式下切换中文不生效的问题，并同步处理 Statsig store、i18n layer 与 React memo cache。
- Windows 改用原生 COM 激活商店版 Codex App，不再启动隐藏 PowerShell，减少 Windows 安全中心误报和启动闪窗。
- Codex 配置未初始化时禁用增强启动；配置文件内容未变化时不再重复替换，继续保留原子写入、备份和全零文件恢复保护。

## 模型目录

- 同步 Codex 最新目录中的 GPT-5.6 Sol、Terra、Luna，以及 GPT-5.5、GPT-5.4、GPT-5.4 Mini 属性。
- 对齐上下文窗口、模型优先级、推理摘要参数、技能使用说明和模型消息等能力声明。
- 按 CodexHub 的兼容策略继续显示 GPT-5.4 与 GPT-5.4 Mini，方便现有 Provider 和历史会话继续选择。

## Grok 工具桥接

- 将 Codex 的 custom `apply_patch` 工具转换为 Grok Responses function schema，并在响应时恢复为 Codex 可执行的 `custom_tool_call`。
- 完整支持 `apply_patch` 的历史回放、工具结果、非流式响应和 SSE 流式事件；实际文件修改仍由 Codex 本地执行。
- 对 Grok 偶发生成的 Markdown 对称星号控制行做严格、局部修复，不猜测或改写普通补丁内容。
- 将 `web_search_preview` 规范化为 Grok hosted `web_search`，兼容图片搜索和域名过滤字段；上游搜索事件原样返回 Codex。
- Grok 兼容逻辑严格限定在 Grok Provider，OpenAI Responses / Responses Lite 原生字段继续透传。

## 历史会话与启动稳定性

- 启用 Codex App 当前版本的按需历史恢复 gate，恢复会话时先读取最近 5 个 turn，更早内容在向上滚动时继续分页加载。
- 避免超长历史会话在启动时被一次性排空，降低启动卡顿、内存峰值和 renderer 崩溃风险；不会删除或截断历史内容。
- 增强启动增加模型、中文、gate、renderer 和本地 Statsig 初始化诊断字段，便于 Codex App 更新后快速核对兼容性。

## 请求日志清理

- 删除旧日志或全部日志后执行 SQLite `VACUUM`，实际回收数据库占用的磁盘空间。
- 清理和磁盘回收在后台执行；维护期间新请求日志进入队列，AI Gateway 请求转发不会被数据库维护阻塞。
- GUI 增加醒目的动态“正在清理...”状态、完成/失败结果，并延长大型数据库清理的等待时间。
- 修复刚进入日志页面就错误显示“正在清理”的问题；状态只在清理任务实际开始后出现。
- 清理过程中暂停列表刷新，避免并发查询干扰维护任务。

## 验证

- 全量测试：`555 passed, 0 failed, 2 ignored`。
- `cargo check --release --features gui --bin codexhub` 通过。
- Windows release GUI 构建通过；当前 Codex App renderer 热注入验证通过。

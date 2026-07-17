# Codex App 模型显示与 `web.run` Provider 取舍

日期：2026-07-15

状态：方案 1 的 Codex App 模型显示已通过可选增强启动解决；普通启动模型显示和账号态相关市场仍有边界。

本文记录 CodexHub 接入 Codex App 时，自定义模型显示、原生 `web.run` 和本地压缩之间的条件冲突。该结论基于 `references/codex-main` 最新源码和 Codex App `26.707.9981` 的实际 renderer bundle 分析。

## 1. 当前决策

目前没有一个只靠公开配置的组合，可以同时满足以下全部目标：

1. Provider 名称保持 `ai-gateway`。
2. Codex App 显示 CodexHub 的完整自定义模型列表。
3. GPT-5.6 Responses Lite 注册原生 `web.run`。
4. 保持本地压缩，不进入 OpenAI Remote Compact V2。
5. 不修改 Codex App 本体，不代理或替换 Codex App 自己的 app-server。

现有两套方案如下：

| 方案 | Provider 配置 | 已解决 | 待解决 |
| --- | --- | --- | --- |
| 方案 1 | `ai-gateway + requires_openai_auth=false + Actor Authorization` | `web.run`、本地压缩、增强模式下的 Codex App 模型显示 | 普通启动下的模型显示；账号态相关的 curated 市场 |
| 方案 2 | `ai-gateway + requires_openai_auth=true` | 模型显示、账号态、本地压缩 | `web.run` 注册 |

CodexHub 不修改 ASAR，也不接管默认官方入口。需要完整模型列表时，用户从 CodexHub 主动使用增强模式启动。

## 2. 方案 1：Actor Authorization

配置形态：

```toml
model_provider = "ai-gateway"
web_search = "live"

[model_providers.ai-gateway]
name = "ai-gateway"
wire_api = "responses"
requires_openai_auth = false
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
experimental_bearer_token = "dummy-token"
http_headers = { x-openai-actor-authorization = "codexhub-local" }
```

### 2.1 已解决的能力

Codex 最新源码中，Web Search Extension 的可用条件为：

```rust
(config.model_provider.is_openai()
    || config.model_provider.uses_openai_actor_authorization())
    && web_search_mode != WebSearchMode::Disabled
```

Actor Authorization 的判断为：

```rust
!self.requires_openai_auth
    && http_headers 中存在非空 x-openai-actor-authorization
```

因此该配置可以创建 `web.run` executor。Responses Lite 会把 `web.run` 放进 `input[].additional_tools`，工具执行时再请求 Provider 的 `/alpha/search`。

Provider 名称仍为 `ai-gateway`，不满足 `provider.is_openai()`，也不满足 Azure Responses Provider 判断，因此不会启用 OpenAI Remote Compact V2，继续使用 Codex 本地文本压缩。

### 2.2 未解决的模型显示

自定义 Provider 的 Core、CLI、app-server 和 Remote Control 都能从 Provider 的 `/models` 拉取完整模型目录。问题不在 AI Gateway 的 `/models`，而在 Codex App renderer 的二次过滤。

`requires_openai_auth=false` 时，Core 的账户响应为：

```json
{
  "account": null,
  "requiresOpenaiAuth": false
}
```

Codex App renderer 随后得到 `authMethod=null`，进入 pre-login Statsig 路径。该路径访问 renderer 中硬编码的：

```text
https://ab.chatgpt.com/v1
```

它不会调用 CodexHub 的 `/wham/statsig/bootstrap`。官方 Statsig dynamic config `107580212` 中的 `available_models` 和 `use_hidden_models` 会再次过滤 app-server 的 `model/list`，最终只显示官方白名单模型。

所以即使以下链路都正常，Codex App 下拉框仍可能看不到 DeepSeek、Grok、GLM、Opus 和 Sonnet：

```text
CodexHub /models
  -> Codex Core ModelsManager
  -> app-server model/list
  -> Remote Control 可见
  -> Codex App renderer 再次过滤
```

### 2.3 可选增强启动

CodexHub 可以用 loopback-only CDP 启动 Codex App，在 renderer 第一帧增量合并 Statsig `107580212` 和关键 gate。该模式已实机验证完整显示 DeepSeek、Grok、GLM、Opus 和 Sonnet，同时保留官方 primary runtime 和插件配置。

增强模式不修改 ASAR、LevelDB 或快捷方式，只影响本次由 CodexHub 启动的 Codex App。VS Code 插件和用户从官方入口普通启动的 Codex App 不受影响。

### 2.4 插件市场与账号态副作用

`requires_openai_auth=false` 不只影响模型白名单。Codex app-server 会基于当前 Provider 明确返回等价状态：`account/read` 中 `account=null`、`requiresOpenaiAuth=false`，`getAuthStatus` 中 `authMethod=null`、`requiresOpenaiAuth=false`。

```json
{
  "account": null,
  "authMethod": null,
  "requiresOpenaiAuth": false
}
```

这个结果由当前 Provider 决定。即使 `~/.codex/auth.json` 仍保存 `chatgptAuthTokens`，renderer 看到的 `authMethod` 依然是 `null`。

Codex App `26.707.91948` 的插件 renderer 还有一层独立过滤：当 `authMethod` 不是 `chatgpt`、`apikey` 或 `amazonBedrock` 时，会从 `plugin/list` 结果中移除以下两个 marketplace：

```text
openai-curated
openai-curated-remote
```

2026-07-16 实机读取 renderer 的 React 查询缓存确认：

1. `plugins=true`、`remote_plugin=true`，Statsig gate `4218407052=true`；
2. 左侧“插件”入口仍存在；
3. `openai-bundled` 和 `openai-primary-runtime` 共 10 个本地插件正常显示，包括 Computer Use、Chrome、Documents、PDF、Spreadsheets、Presentations、LaTeX 和 Visualize；
4. 本地 `openai-curated` manifest 仍有 25 个经过 CodexHub 过滤、理论上可本地使用的插件，但查询键被标记为 `openai-curated-marketplaces-hidden`，renderer 不渲染它们；
5. `/backend-api/ps/plugins/list` 仍被请求，因此问题不是 CodexHub 路由中断，也不是增强模式漏补 Statsig gate。

增强模式目前只修补模型 dynamic config 和已经确认的功能 gate，不修改 React Auth Context。它解决模型显示，但不会把 `authMethod=null` 伪造成 ChatGPT 登录态。

### 2.5 Apps/Connectors 是另一条链路

CodexHub 当前还会写入：

```toml
[features]
apps = false
```

该开关关闭的是 `codex_apps` MCP 以及依赖 ChatGPT 官方后端的 Apps/Connectors，例如 Gmail、Google Drive 等；它不关闭本地 plugin、skill、Computer Use 或 Chrome。CodexHub 尚未实现官方 `.../backend-api/wham/apps` streamable HTTP 后端，因此不能为了恢复入口而直接改成 `apps=true`，否则只会展示无法工作的功能并产生 MCP 启动错误。

### 2.6 后续可选修复

若要在方案 1 下恢复本地可用的 curated 插件，优先评估以下窄范围方案：

1. 将 CodexHub 已过滤过的本地目录改用 `codexhub-curated` marketplace 身份；
2. 同步迁移 `<plugin>@openai-curated` 的安装/启用状态和 featured plugin id；
3. 保持官方 Apps/Connectors 关闭，不把远端依赖伪装成本地可用；
4. 验证插件安装、详情、卸载和 Codex 更新后 marketplace 重建流程。

不采用 CDP 修改全局 `authMethod`。该值还控制账号区域、套餐、共享市场和其他 ChatGPT 行为，伪造后影响面远大于插件列表。也不把 `requires_openai_auth` 改回 `true` 作为局部修复，因为这会重新关闭 Actor Authorization 路径下的原生 `web.run`。

## 3. 方案 2：保留 OpenAI 账号要求

配置形态：

```toml
model_provider = "ai-gateway"
web_search = "live"

[model_providers.ai-gateway]
name = "ai-gateway"
wire_api = "responses"
requires_openai_auth = true
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
```

### 3.1 已解决的能力

该配置保留 Codex App 的 ChatGPT 账号态。renderer 不会进入 `authMethod=null` 的 pre-login 分支，因此模型显示、账号区域以及依赖账号态的前端行为保持正常。

Provider 名称仍为 `ai-gateway`，所以 `provider.is_openai()` 为 false，继续使用本地压缩，不触发 OpenAI Remote Compact V2。

### 3.2 未解决的 `web.run`

该配置既不满足：

```text
provider.is_openai()
```

也不满足：

```text
uses_openai_actor_authorization()
```

后者明确要求 `requires_openai_auth=false`。因此 Web Search Extension 不会创建 `web.run` executor。

仅在 AI Gateway 转发请求时注入 `web.run` 描述没有作用。模型即使返回 `web.run` 调用，Codex 本地 Tool Registry 也没有对应 executor，不能完成 `/alpha/search` 调用和工具结果回填。

## 4. 为什么不把 Provider 改名为 `OpenAI`

下面的组合可以越过 `web.run` 的 `provider.is_openai()` 条件：

```toml
name = "OpenAI"
requires_openai_auth = true
```

但 `is_openai()` 不只控制 Web Search。它还会影响 OpenAI 私有行为，包括 Remote Compact V2、请求编码、认证和其他 Provider 能力判断。

CodexHub 的上游可能是 Grok、DeepSeek、GLM 或 Anthropic。为了开启搜索而把整个 Gateway 伪装成 OpenAI，会扩大协议影响范围，并重新引入跨 Provider 密文、压缩结果和会话迁移问题。因此不采用。

## 5. 已评估但暂不采用的方案

| 方案 | 不采用原因 |
| --- | --- |
| Gateway 注入 `web.run` 工具描述 | Codex 本地没有 executor，工具调用无法执行 |
| Responses Lite 注入 hosted `web_search` | 当前协议明确拒绝顶层 hosted tools |
| Gateway 模拟 Remote Compact V2 | 需要处理 SSE、opaque compaction、切换模型和历史迁移，风险过大 |
| 修改 `account/read` 或替换 app-server 启动链路 | 会介入 Codex App、CLI 和 VS Code 的进程及账号行为 |
| 修改 LevelDB/Statsig 本地缓存 | 数据结构和生命周期不稳定，更新后容易失效或损坏状态 |
| 修改 Codex App `app.asar` | 技术上可做最小 renderer 补丁，但 Windows MSIX、macOS 签名、多架构和频繁更新带来持续维护成本 |
| 伪造完整官方登录状态 | 需要持续模拟更多官方后端接口，影响面超过模型显示问题 |

## 6. ASAR 调研结论

Codex App `26.707.9981` 的 renderer 模型过滤逻辑位于独立的 `model-list-filter-*.js` bundle。逻辑会在 `use_hidden_models=true` 时，只保留 Statsig `available_models` 白名单中的模型。

已验证可以通过等长字节替换让 renderer 始终信任 app-server 返回的非隐藏模型，而且不改变 ASAR 文件大小和目录偏移。但该方法仍存在以下发布问题：

1. Windows Store 版本位于 `WindowsApps`，由 `TrustedInstaller` 管理，并受 MSIX block map 保护。
2. macOS 需要单独处理 App Bundle 签名和可能的 ASAR Integrity。
3. Windows x64、Windows ARM64、macOS Intel 和 Apple Silicon 都需要独立验证。
4. 每次 Codex App 更新都可能改变 bundle 文件名和压缩代码形态。

因此 ASAR 补丁只保留为调研结论，不进入当前产品实现。

## 7. 等待 Codex 更新时的复查清单

每次更新 `references/codex-main` 后，优先检查：

1. `codex-rs/ext/web-search/src/extension.rs`
   - `web.run` 是否仍要求 `is_openai()` 或 Actor Authorization。
   - 是否新增独立的 Web Search capability 配置。
2. `codex-rs/model-provider-info/src/lib.rs`
   - `uses_openai_actor_authorization()` 是否仍强制 `requires_openai_auth=false`。
   - `supports_remote_compaction()` 是否提供显式开关。
3. `codex-rs/app-server/src/request_processors/account_processor.rs`
   - 自定义 Provider 是否可以保留账号态，同时报告自己的 Provider 能力。
4. `codex-rs/model-provider/src/provider.rs`
   - 自定义 Provider 的 `/models` manager 是否继续正常工作。
5. Codex App renderer
   - 是否仍用 Statsig `107580212` 二次过滤 `model/list`。
   - 是否开始直接信任 app-server 的模型目录。
   - `authMethod=null` 时是否仍过滤 `openai-curated` / `openai-curated-remote`。
   - 插件目录是否提供了与 OpenAI 账号态解耦的公开能力开关。

满足下面任意一项，就值得重新启动适配：

1. `web.run` 增加与 `requires_openai_auth` 无关的显式 capability 开关。
2. Actor Authorization 可以与 `requires_openai_auth=true` 共存。
3. Codex App renderer 不再用 pre-login Statsig 白名单过滤 app-server 模型。
4. Codex App 提供正式的自定义模型目录配置或扩展接口。
5. Codex App 允许无 OpenAI Auth Provider 使用本地 curated marketplace。

## 8. 维护原则

1. Provider 名称保持 `ai-gateway`，不为单一能力伪装成 `OpenAI`。
2. 不实现 Gateway Remote Compact V2 模拟。
3. 不通过请求注入伪造 Codex 本地没有注册的工具。
4. 不修改 Codex App 安装文件作为默认产品能力。
5. 普通启动接受官方白名单限制；需要完整模型列表时由用户主动选择增强模式。

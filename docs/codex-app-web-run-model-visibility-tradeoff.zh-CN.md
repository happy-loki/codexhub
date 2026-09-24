# Codex App 模型显示与 `web.run` Provider 取舍

首次记录：2026-07-15；源码复核：2026-09-22。

状态（2026-09-24）：新版源码已增加独立搜索能力配置，旧版“`requires_openai_auth=true` 与自定义 Provider 的 `web.run` 无法兼得”限制已解除。但本机 `true + chatgptAuthTokens` 实测触发 `account/read` 的 `workspace routing discovery missing backend origin`，App 显示登录页；默认配置已恢复 `false + Actor Authorization`，保留独立搜索能力和模型目录发现配置。

本文区分本地最新 `references/codex-main` 的源码结论与 Codex App 的实机结果。源码更新不代表用户安装的 Codex App 已包含相同实现；恢复后的全部工具功能仍需客户端联调。

## 2026-09-22：新版接入方案

### 搜索与本地压缩可以兼得

新版 `codex-rs/ext/web-search/src/extension.rs` 的 provider 条件为：

```rust
(provider.is_openai()
    || provider.uses_openai_actor_authorization()
    || provider.supports_standalone_web_search)
    && web_search_mode != WebSearchMode::Disabled
```

因此可以保留 `name = "ai-gateway"`，设置 `requires_openai_auth = true`，再显式声明 `supports_standalone_web_search = true`。Actor Authorization 本身仍要求 `requires_openai_auth=false`，但已不再是自定义 Provider 开启 `web.run` 的唯一入口。

这是扩展可用条件。最终注册还要求 `namespace_tools`、`web_search` provider capabilities，以及 `use_responses_lite=true` 或 `[features].standalone_web_search=true`。app-server 的 `extensions.rs` 安装了搜索扩展；仅由 Gateway 注入工具描述仍不能代替客户端 executor。

远程压缩独立判断：`model-provider/src/provider.rs` 中，普通配置 Provider 只有 `is_openai()` 或 Azure Responses 身份才获得 `RemoteCompactionSupport::V2`。`ai-gateway` 和本地 base URL 不满足这些条件，`requires_openai_auth=true` 与搜索能力开关不会改变它，仍走本地摘要压缩。“本地压缩”仍会调用模型生成摘要，并非完全离线运行。

### 模型目录还有一个新增条件

不能只修改 `requires_openai_auth`：

- `account_state()` 在 `true` 时读取现有登录信息；没有凭证时不会自动产生 ChatGPT 账号态。CodexHub 管理的本地 `chatgptAuthTokens` 认证与上游 ChatGPT OAuth 渠道凭证是两回事。新版 workspace routing discovery 仍可能使 `account/read` 失败，不能把凭证存在等同于账号读取成功。
- 新版 `models-manager/src/manager.rs` 把 provider 上的 `env_key` 或 `experimental_bearer_token` 也视为 API Key 模式。默认的 `dummy-token` 同样命中该判断，即使 AuthManager 里另有 ChatGPT 账号态。
- 自定义 `base_url` 下，这条目录发现路径需要 provider 的 `model_catalog_url` 和 `[features].api_key_model_discovery=true`；否则可能在读取远程目录、甚至使用其缓存前直接返回。
- `model_catalog_url` 必须返回 Codex 模型元数据目录，CodexHub 的 `/ai-gateway/v1/models` 可提供该结构。这是网络目录 URL，不是本地 JSON 文件路径。

这些条件解决 Core/app-server 的账号响应和目录获取，不足以证明最新 App renderer 一定展示全部模型。历史 Statsig 白名单、模型可见性和账号过滤必须另做实机验证。

### 已实现的配置写入

以下组合已由 `src/codex_app_config.rs` 的默认初始化流程写入，尚未发布。手动合并时不能重复创建 `[features]` 或同名 provider 表。

```toml
model_provider = "ai-gateway"
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
web_search = "live"

[features]
standalone_web_search = true
api_key_model_discovery = true

[model_providers.ai-gateway]
name = "ai-gateway"
wire_api = "responses"
requires_openai_auth = false
http_headers = { x-openai-actor-authorization = "codexhub-local" }
supports_standalone_web_search = true
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
model_catalog_url = "http://127.0.0.1:3847/ai-gateway/v1/models"
experimental_bearer_token = "dummy-token"
```

说明：

1. `standalone_web_search` 对 Lite 模型不是必需项；显式开启可让非 Lite 模型也走 `web.run`。真正注册 standalone executor 后，Core 不再同时声明 hosted `web_search`。该 feature 当前仍标为 `UnderDevelopment`。
2. `api_key_model_discovery` 当前也标为 `UnderDevelopment`，需要客户端确实包含该功能。保留 `dummy-token` 是为了保持 Gateway 请求认证独立；不要为了拉目录直接删除它、意外改变凭证发送路径。
3. 更新时恢复本地 Actor header，保留用户其他 headers。`false + Actor` 满足搜索和生图的认证分支；`true` 下 Actor 判断不生效。
4. `supports_websockets` 是独立的传输配置，沿用用户现有选择，不作为本方案的前提。
5. `chatgpt_base_url` 保持本地后端配置。这个方案不意味着我们已经支持全部 ChatGPT 官方账号功能，也不代表可以撤掉所有增强启动适配。
6. 不覆盖用户的 `features.image_generation`。最新 Codex 默认开启生图；用户已有 `false` 时保留，Gateway 生图过滤也保持原设置。

已初始化用户在新版 CodexHub 点击“更新 Codex 配置”，即可写入新配置，不必先恢复配置。随后自行重新打开 Codex 客户端以加载新配置。仅升级 CodexHub 可执行文件不会自动改写正在使用的配置。

初始化会单独备份原来的 `standalone_web_search`、`api_key_model_discovery` 值，重复初始化不覆盖首份备份。恢复时还原这些值；用户后来关闭或删除的开关不强行改回。旧版备份缺少这部分信息时，在首次写入新开关前补记。恢复仍保留 `ai-gateway` provider 表，便于打开历史会话。

### 生图不能只看 Provider 名称

当前 Core 的 `image_generation_available()` 还要求：

- 有效运行时 `Feature::ImageGeneration=true`，目前默认开启。
- 缓存账号套餐不是明确的 `Free`。
- provider 支持 `image_generation` 和 `namespace_tools`。
- 当前模型 `input_modalities` 包含 `image`。
- Actor Authorization 生效，或 `requires_openai_auth=true` 且 AuthManager 的认证使用 Codex backend。普通 API Key 登录不满足后一项；`chatgpt`、`chatgptAuthTokens` 等 backend 认证可满足。

因此当前默认恢复 `false + Actor`，生图走 Actor 认证分支，`auth.json` 保留本地 `chatgptAuthTokens` 供远程控制使用。`true` 实验未解决新版账号路由错误和 Chrome/Computer Use 的认证兼容；不能把 `experimental_bearer_token` 本身当成生图资格。这些本地兼容信息不代表上游真实授权，上游搜索、生图仍需其各自有效凭证与接口支持。详情见 [认证说明](auth-notes.zh-CN.md)。

新版 Core 确实会在有效 feature 为 false 时跳过 `image_gen.imagegen`。旧文档关于 App 忽略配置的观察不能推广成所有新版的结论；App 是否覆盖 feature、是否加载了新配置仍要验证。

### 对比与验证边界

| 组合 | 原生 `web.run` | 本地压缩 | 账号与生图 |
| --- | --- | --- | --- |
| `false + Actor`，当前默认 | provider 条件满足 | 保持 | provider account 仍为空；生图走 Actor 分支，仍有 feature、模型和套餐限制 |
| `true`，不加新能力配置 | 自定义名称下仍不满足 | 保持 | 读取已有账号；生图需 backend 认证 |
| `true + supports_standalone_web_search=true`，已撤回的实验 | provider 条件满足 | 保持 | 本机账号读取被 workspace routing discovery 阻断；生图需 backend 认证 |

当前使用第一种组合，并保留新增的搜索和目录发现配置。增强启动的模型显示适配仍然需要，不能据此宣称普通启动的模型显示已经修复。客户端验证顺序：

1. 确认实际运行的 app-server 包含这些字段和 feature。
2. 确认 `account/read` 的实际账号类型，以及本地 `/models` 请求和 `model/list`。
3. 检查 App 下拉框是否展示自定义模型，再决定是否仍需增强模式。
4. 检查模型请求确实声明 `web.run`，工具调用到本地 `/alpha/search`，且上游完成搜索。
5. 检查 `image_gen` 注册及独立 Images API 请求；确认 Gateway 的生图过滤未开启。
6. 确认压缩走摘要请求，没有生成 Remote Compact V2 请求。

2026-09-24 已同步恢复配置生成和更新按钮判断，并备份、修改本机实时配置；没有重启客户端或编译 Codex。按模型家族选择远程/本地压缩的后续研究记录在 [todolist.md](../todolist.md)，本次不实现。主要源码依据：

- `codex-rs/model-provider-info/src/lib.rs`：新 capability、Actor 判断与 Provider 身份。
- `codex-rs/ext/web-search/src/extension.rs`、`app-server/src/extensions.rs`：扩展条件和安装。
- `codex-rs/core/src/tools/spec_plan.rs`：搜索选择、生图完整条件和 hosted/standalone 互斥。
- `codex-rs/model-provider/src/provider.rs`：账号状态和远程压缩能力。
- `codex-rs/model-provider/src/models_endpoint.rs`、`models-manager/src/manager.rs`：目录发现条件。
- `codex-rs/features/src/lib.rs`：feature 名称、默认值和开发状态。
- `codex-rs/app-server/tests/suite/v2/web_search.rs`：已有自定义 Provider + standalone capability 的搜索回合测试；该测试使用 `requires_openai_auth=false`，不等于本候选组合已实测。

2026-09-24 实现验证：`cargo test --locked --features gui --bin codexhub --quiet` 为 779 项通过、2 项忽略、0 项失败；`cargo fmt --all -- --check` 和 `git diff --check` 通过。回归覆盖重复初始化后的开关恢复、用户后续改动保留、旧备份补记、内联 TOML 配置保留，以及从 `true` 恢复 Actor 配置后保留其他 headers、生图和 WebSocket 偏好。真实客户端工具注册、上游搜索及生图结果仍需联调。

## 历史记录范围

以下第 1 至 8 节保留 2026 年 7 月的取舍和当时 App 实机记录，其中“当前”“最新”均指当时版本。涉及“只能二选一”“true 无法注册 web.run”“一定能显示模型”的说法，不再作为新版结论；以以上复核为准。

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

增强模式不修改 React Auth Context，也不会把 `authMethod=null` 伪造成 ChatGPT 登录态。2026-07-21 起，它除模型、语言和已确认的功能 gate 外，还会对本地 curated 插件目录做窄范围展示适配，详见 2.6 节。

### 2.5 Apps/Connectors 是另一条链路

CodexHub 当前还会写入：

```toml
[features]
apps = false
```

该开关关闭的是 `codex_apps` MCP 以及依赖 ChatGPT 官方后端的 Apps/Connectors，例如 Gmail、Google Drive 等；它不关闭本地 plugin、skill、Computer Use 或 Chrome。CodexHub 尚未实现官方 `.../backend-api/wham/apps` streamable HTTP 后端，因此不能为了恢复入口而直接改成 `apps=true`，否则只会展示无法工作的功能并产生 MCP 启动错误。

### 2.6 本地 curated 插件展示适配

Codex App `26.715.8383` 的 renderer 仍按 2.4 节所述过滤市场，同时把 `codex-official` 识别为内置市场。增强模式因此在 renderer 第一帧安装了一个仅面向插件目录的响应适配器：

1. 监听 `codex-message-from-view`，只记录 `vscode://codex/list-plugins` 的 request id；
2. 捕获对应的 `fetch-response`，只把顶层本地市场名 `openai-curated` 临时映射为 renderer 已接受的 `codex-official`；
3. 保持 `marketplace.path`、插件 ID（如 `game-studio@openai-curated`）、插件名、安装/启用状态和配置身份原样不变；
4. 完全不修改 `openai-curated-remote`、`openai-bundled`、`openai-primary-runtime` 或其他用户市场；
5. 本地插件的安装、读取和详情继续使用绝对 `marketplacePath`，不会把展示别名写回 app-server 或 `config.toml`。

实机 renderer 消息回放验证了上述约束：本地路径、原始插件 ID、禁用状态和 featured ID 均保持不变，只有本地市场的展示名发生变化。启动诊断新增 `pluginCatalogBridgeInstalled` 和 `pluginCatalogResponsesAdapted`，前者也纳入增强模式成功条件。

当前限制是 renderer 仍会按原始 `@openai-curated` 后缀过滤 featured 推荐位，因此完整插件目录可以显示，但 curated 插件不一定进入首页推荐区。这里不改写插件 ID，避免为了推荐位破坏安装、卸载和历史配置身份。

不采用 CDP 修改全局 `authMethod`。该值还控制账号区域、套餐、共享市场和其他 ChatGPT 行为，伪造后影响面远大于插件列表。也不把 `requires_openai_auth` 改回 `true` 作为局部修复，因为这会重新关闭 Actor Authorization 路径下的原生 `web.run`。普通启动、CLI 和 VS Code 插件不经过该适配。

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

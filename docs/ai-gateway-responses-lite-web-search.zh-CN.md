# Codex Responses Lite 协议与 Web Search 对接避坑说明

日期：2026-07-14

状态：`/alpha/search` 透明代理已落地；**已撤销** Lite 顶层 hosted `web_search` 注入（上游现明确拒绝）。更新后的 Codex 已正式支持 actor-authorized 自定义 Provider 注册 `web.run`，默认 provider 改为 `name = "ai-gateway" + requires_openai_auth = false + x-openai-actor-authorization`。模型目录继续从 CodexHub `/models` 拉取。

本文记录 Codex 新版 Responses Lite 请求形态、工具注册与执行边界、当前 CodexHub/Sub2API 的兼容策略，以及原生 `web.run` 的剩余验证工作。本文以本仓库 2026-07-14 更新后的 `references/codex-main`、`references/sub2api-main` 和既有真实请求验证为准。

相关文档：

- [`codex-app-web-run-model-visibility-tradeoff.zh-CN.md`](codex-app-web-run-model-visibility-tradeoff.zh-CN.md)：Codex App 模型显示、原生 `web.run` 与本地压缩之间的当前取舍和暂缓决策。
- [`ai-gateway-web-search-protocol.zh-CN.md`](ai-gateway-web-search-protocol.zh-CN.md)：标准 Responses 托管 `web_search` 以及 Anthropic/GLM 转换规则。
- [`ai-gateway-architecture.zh-CN.md`](ai-gateway-architecture.zh-CN.md)：AI Gateway 总体架构。
- [`codex-app-fast-startup-statsig.zh-CN.md`](codex-app-fast-startup-statsig.zh-CN.md)：Codex App 本地后端与 feature gate 兼容边界。

## 1. 当前结论

先给出当前版本必须遵守的结论：

1. GPT-5.6 Sol、Terra、Luna 的模型元数据都启用了 `use_responses_lite = true`。
2. Responses Lite 把客户端执行工具放进 `input[].additional_tools`，而不是顶层 `tools`。
3. Codex 原生 Lite 搜索工具是客户端工具 `web.run`，不是托管工具 `web_search`。
4. `web.run` 执行时会独立请求 provider 的 `/alpha/search`，不会在当前 `/responses` 请求中完成搜索。
5. CodexHub 默认 provider 使用 `name="ai-gateway"`、`requires_openai_auth=false` 和本地 Actor Authorization header；满足独立工具 gate，但不满足 `provider.is_openai()`，因此不会触发 OpenAI 远程压缩。
6. CodexHub 已提供 `/ai-gateway/v1/alpha/search`，当前 Sub2API `0.1.152` 也已提供 `/v1/alpha/search`；搜索请求可以按 OpenAI Responses Provider 路由并透明转发。
7. 把托管 `{"type":"web_search"}` 塞进 `additional_tools.tools` 会被当前上游静默忽略。这不是 Codex 原生 Lite 结构。
8. **不要**再向 Responses Lite 请求顶层 `tools` 注入 hosted `web_search`。上游会返回 `unsupported_value`：`X-OpenAI-Internal-Codex-Responses-Lite only supports function tools, custom tools, and client-executed tool search.`
9. 若 Lite 请求顶层 `tools` 或 `input[].additional_tools.tools` 已带有 hosted `web_search` / `web_search_preview`，CodexHub 会剥离它们再转发；合法的客户端 `tool_search`（`execution=client`）保留，标准（非 Lite）Responses 仍可使用 hosted `web_search`。
10. GPT-5.6 Lite 的原生搜索路径是客户端 `web.run` + `/alpha/search`；当前默认配置会注册该工具，不再用 hosted 工具伪装。
11. 历史“Lite transport + 顶层 hosted web_search”混合形态曾短暂可用，现已被上游明确拒绝，不再作为兼容策略。
12. 旧版 Codex App 曾在 `provider account = None` 时触发前端模型白名单。更新后的 Codex 为自定义 Provider 创建 OpenAI-compatible models manager，并明确测试 actor-authorized Responses Lite 工具；当前版本重新启用该方案，但仍需实机回归模型下拉框、账户区和 Remote Control。
13. Codex App 前端还会读取 Statsig dynamic config `107580212.value.available_models` 二次过滤 `model/list`。CodexHub bootstrap 必须把 `v` 直接返回为包含 `available_models`、`use_hidden_models` 和 `default_model` 的对象；只返回配置名称字符串会被解析成空白名单，即使 `models_cache.json` 已缓存全部模型，界面仍然看不到模型。

## 2. `use_responses_lite` 不是普通能力开关

最容易误判的一点，是把模型元数据里的：

```json
"use_responses_lite": true
```

理解成一个只影响后端路由的小开关。实际上它会改变整个请求的编码方式。

当前 GPT-5.6 模型：

| 模型 | `tool_mode` | `use_responses_lite` | `web_search_tool_type` |
| --- | --- | --- | --- |
| `gpt-5.6-sol` | `code_mode_only` | `true` | `text_and_image` |
| `gpt-5.6-terra` | `code_mode_only` | `true` | `text_and_image` |
| `gpt-5.6-luna` | `code_mode_only` | `true` | `text_and_image` |

来源：`src/ai_gateway/models.json`。

### 2.1 标准 Responses 请求

标准模式大致是：

```json
{
  "model": "gpt-5.4",
  "instructions": "You are Codex...",
  "input": [
    {
      "type": "message",
      "role": "user",
      "content": [{"type": "input_text", "text": "hello"}]
    }
  ],
  "tools": [
    {"type": "custom", "name": "exec"},
    {"type": "web_search", "external_web_access": true}
  ],
  "parallel_tool_calls": true,
  "stream": true
}
```

### 2.2 Responses Lite 请求

Lite 模式大致是：

```json
{
  "model": "gpt-5.6-sol",
  "input": [
    {
      "type": "additional_tools",
      "role": "developer",
      "tools": [
        {"type": "custom", "name": "exec"},
        {"type": "function", "name": "wait"},
        {"type": "function", "name": "request_user_input"}
      ]
    },
    {
      "type": "message",
      "role": "developer",
      "content": [
        {"type": "input_text", "text": "You are Codex..."}
      ]
    },
    {
      "type": "message",
      "role": "user",
      "content": [{"type": "input_text", "text": "hello"}]
    }
  ],
  "reasoning": {
    "context": "all_turns"
  },
  "parallel_tool_calls": false,
  "stream": true
}
```

请求头还会增加：

```http
x-openai-internal-codex-responses-lite: true
```

### 2.3 当前源码中已确认的差异

| 行为 | 标准 Responses | Responses Lite |
| --- | --- | --- |
| 基础指令 | 顶层 `instructions` | `input` 前缀中的 `role=developer` message |
| 工具定义 | 顶层 `tools` | `input[].type=additional_tools` |
| 托管工具 | 可以生成 | Codex 当前主动排除 |
| `parallel_tool_calls` | 跟随模型能力 | 强制为 `false` |
| reasoning context | 省略，使用服务端默认 | 显式 `all_turns` |
| 图片 detail | 保留 | Codex 会剥离 detail |
| Lite 请求头 | 无 | `x-openai-internal-codex-responses-lite: true` |

关键源码：

- `references/codex-main/codex-rs/core/src/client.rs`
  - `build_responses_request`
  - `build_reasoning`
  - `add_responses_lite_header`
- `references/codex-main/codex-rs/core/src/client_common.rs`
  - `get_formatted_input_for_request`
  - `strip_image_details`

因此，修改 `use_responses_lite` 会同时改变 instructions、tools、reasoning、并行工具调用和图片输入，不应当只为了恢复一个工具就随意切换。

## 3. `additional_tools` 的真实语义

`additional_tools` 是一个 Responses Lite input item：

```json
{
  "type": "additional_tools",
  "role": "developer",
  "tools": [...]
}
```

它承载的是客户端执行工具规格，例如：

- `exec`
- `wait`
- `request_user_input`
- `namespace` 工具
- MCP/插件提供的客户端工具
- 原生 Lite 搜索工具 `web.run`

### 3.1 不要把 `additional_tools` 当普通 developer message

它虽然带有 `role=developer`，但不是一条文本指令。转换器必须首先根据 `type=additional_tools` 识别它，不能：

- 转成普通 system/developer 文本。
- 合并进用户消息。
- 删除 `tools` 内部的 `namespace`。
- 改变它在 Lite input 前缀中的顺序。
- 把它持久化成会话中的自然语言消息。

Lite 请求中频繁出现 developer role，并不代表用户或模型反复插入了高优先级系统指令。前两个 item 往往只是工具声明和基础 instructions。

### 3.2 工具规格不等于工具执行器

模型看见工具定义，不代表 Codex App 一定有对应执行器。

Codex 工具计划分为两部分：

```text
model_visible_specs：发给模型看的规格
ToolRegistry：客户端真正可以执行的 runtime
```

客户端 `function_call` / `custom_tool_call` 会进入 `ToolRegistry` 查找执行器。若只在请求中伪造一个函数：

```json
{
  "type": "function",
  "name": "web_search"
}
```

模型可能调用它，但 Codex 本地没有同名 runtime，最终会得到 unsupported/unknown tool 错误。

关键源码：

- `references/codex-main/codex-rs/core/src/tools/spec_plan.rs`
- `references/codex-main/codex-rs/core/src/tools/registry.rs`
- `references/codex-main/codex-rs/core/src/tools/router.rs`

### 3.3 托管 `web_search` 不需要进入 ToolRegistry

标准 Responses 托管工具：

```json
{"type":"web_search"}
```

由上游执行。上游返回的是：

```json
{
  "type": "web_search_call",
  "id": "ws_...",
  "status": "completed",
  "action": {
    "type": "search",
    "query": "..."
  }
}
```

`WebSearchCall` 不进入本地工具分发，而是直接映射成 Codex UI/历史中的 WebSearch item。

关键源码：

- `references/codex-main/codex-rs/core/src/event_mapping.rs`
- `references/codex-main/codex-rs/core/src/stream_events_utils.rs`
- `references/codex-main/codex-rs/protocol/src/models.rs`

## 4. 两种完全不同的 Web Search

不要因为名字相近，就把 `web_search` 和 `web.run` 当成同一个协议。

### 4.1 标准 Responses 托管 `web_search`

请求：

```http
POST /v1/responses
```

```json
{
  "tools": [
    {
      "type": "web_search",
      "external_web_access": true,
      "search_content_types": ["text", "image"]
    }
  ]
}
```

执行者：Responses 上游。

返回：Responses SSE 中的 `web_search_call` 过程事件、最终 message 和 citations。

特点：

- 模型自己决定 query。
- 搜索和模型推理在同一个 Responses 请求中完成。
- Codex 只消费上游事件，不执行本地搜索函数。
- 当前 Sub2API/LLMX 路径已经支持。

### 4.2 Responses Lite 原生 `web.run`

工具声明：

```json
{
  "type": "namespace",
  "name": "web",
  "tools": [
    {
      "type": "function",
      "name": "run",
      "parameters": {...}
    }
  ]
}
```

它位于 `input[].additional_tools.tools` 中。

典型调用链：

```text
Responses Lite 模型
    -> 返回 web.run 客户端工具调用
Codex App WebSearchTool runtime
    -> POST provider_base_url/alpha/search
搜索后端
    -> { encrypted_output, output }
Codex App
    -> 把工具结果加入下一轮 /responses
模型
    -> 继续生成最终回答
```

执行者：Codex App 中的 Web Search extension，加上 provider 提供的 `/alpha/search` 服务。

关键源码：

- `references/codex-main/codex-rs/ext/web-search/src/extension.rs`
- `references/codex-main/codex-rs/ext/web-search/src/tool.rs`
- `references/codex-main/codex-rs/codex-api/src/endpoint/search.rs`
- `references/codex-main/codex-rs/codex-api/src/search.rs`

## 5. `/alpha/search` 不是 `/responses + web_search` 的别名

`/alpha/search` 是独立协议。

请求：

```json
{
  "id": "session-id",
  "model": "gpt-5.6-sol",
  "input": [...],
  "commands": {
    "search_query": [
      {
        "q": "2026 世界杯四强",
        "recency": 7,
        "domains": ["fifa.com"]
      }
    ],
    "response_length": "short"
  },
  "settings": {
    "allowed_callers": ["direct"],
    "external_web_access": true
  },
  "max_output_tokens": 10000
}
```

响应：

```json
{
  "encrypted_output": "optional opaque value",
  "output": "Search result text"
}
```

`commands` 还支持：

- `search_query`
- `image_query`
- `open`
- `click`
- `find`
- `screenshot`
- `finance`
- `weather`
- `sports`
- `time`

因此不能把 `/alpha/search` body 原样转发到 `/v1/responses`。未来如果 CodexHub 用托管 `web_search` 模拟 `/alpha/search`，必须显式完成命令转换和返回转换，而且不一定能完整覆盖 open/click/find/screenshot/finance/weather/sports/time 的语义。

## 6. Provider gate 与账户语义

Codex Web Search extension 的可用条件是：

```text
(provider.is_openai() OR provider.uses_openai_actor_authorization())
AND web_search_mode != disabled
```

CodexHub 默认写入的本地 provider：

```toml
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
web_search = "live"

[model_providers.ai-gateway]
name = "ai-gateway"
wire_api = "responses"
requires_openai_auth = false
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
experimental_bearer_token = "dummy-token"
http_headers = { x-openai-actor-authorization = "codexhub-local" }
```

表键和身份字段都保持 `ai-gateway`，因此 `provider.is_openai()` 为假，不会仅因本地 Gateway 身份而启用 OpenAI 私有协议行为。Actor header 只作为 Codex 本地 capability gate；CodexHub 将其列为敏感 header，不会转发给 Sub2API。

### 6.1 Actor Authorization 的判定

`uses_openai_actor_authorization()` 要求：

1. `requires_openai_auth = false`
2. provider 静态 `http_headers` 中存在非空 `x-openai-actor-authorization`

只满足其中一个没有用。

`requires_openai_auth=false` 会让 provider account 变成 `None`。在旧 Codex App 26.707.8479 的实机验证中，前端曾随后采用官方 Statsig dynamic config `107580212` 的 `available_models` 白名单，导致自定义模型隐藏。

更新后的 Codex 已有两项关键变化：

1. `DefaultModelProvider::models_manager()` 对自定义 Provider 也创建 `OpenAiModelsManager`，通过 provider base URL 请求 `/models`。
2. Responses Lite 集成测试明确覆盖 `name="local" + requires_openai_auth=false + x-openai-actor-authorization`，并断言 `web.run` 与 `image_gen` 同时可见。

Codex App 26.707.9981 前端仍会使用 Statsig `107580212` 的 `available_models` 二次过滤。CodexHub 虽然会从当前 `ai_gateway.codex_visible_models` 和内置目录动态生成本地 bootstrap 白名单，但 `requires_openai_auth=false` 会让 renderer 进入 `authMethod=null` 的 pre-login 路径，并直接访问硬编码的官方 Statsig 地址；此时本地 bootstrap 不会被调用。普通启动仍受该限制；用户主动使用 CodexHub 的增强模式启动后，CodexHub 会在 renderer 第一帧增量同步模型白名单，不修改官方 ASAR 或 runtime layer。

### 6.2 为什么不再使用 `name = "OpenAI"`

`is_openai()` 不只控制 Web Search。直接改名会同时影响或开启其他 OpenAI 私有行为，例如：

- OpenAI 请求压缩策略。
- 私有 metadata passthrough。
- remote compaction。
- model-switch compaction。
- reasoning summary 并发/流式行为。

这些行为与开启搜索无关，却扩大了所有渠道的协议面，尤其会把 Grok、Anthropic 和 DeepSeek 会话也带入 OpenAI remote compaction 判断。因此默认配置既不伪装 OpenAI，也不使用 Actor Authorization，而是保留 hosted `web_search` 兼容链路。

其中一个立即可见的差异是请求压缩。Codex 的 `enable_request_compression` 默认开启；当认证使用 Codex backend 且 `provider.is_openai()` 为真时，流式 `/responses` body 会使用 zstd，并携带 `Content-Encoding: zstd`。默认 `ai-gateway` 身份不再触发该分支，但 CodexHub 仍保留 zstd 解压支持，以兼容旧配置和用户显式配置的 OpenAI provider。

### 6.3 Image generation 与 Web Search 使用不同 gate

Codex 当前对 image generation 和 web search 的 runtime gate 并不完全相同。Image generation 接受 OpenAI provider、`requires_openai_auth=true` 或 Actor Authorization；Web Search extension 接受 OpenAI provider或 Actor Authorization。

当前默认配置通过 Actor Authorization 同时满足 image generation 和 Web Search extension gate。两者仍使用不同执行路由：`image_gen` 请求 Images API，`web.run` 请求 `/alpha/search`。

### 6.4 不要依赖 `[features].image_generation` 控制 Codex App

历史版本中，`[features].image_generation = false` 对 Codex CLI 有效，但不能作为 Codex App 的可靠控制面。Codex App 使用 app-server、runtime feature enablement 和自身工具注册流程；不同版本还可能存在配置覆盖或不立即 reload 的差异。

CodexHub 因此不再通过修改该字段实现“过滤生图工具”，而是在 AI Gateway 转发前按实际请求结构处理：

- 删除旧版顶层 hosted tool：`tools[].type = image_generation`。
- 删除新版 standalone namespace：`name = image_gen`。
- 删除 Responses Lite `input[].type = additional_tools` 中的 `image_gen` 工具。
- Code mode 只向上游暴露一个 `exec` custom tool 时，从 `exec.description` 中移除完整的 `## image_gen` 工具注册段。

最后一项移除的是模型可见的常规工具声明，并不会卸载 Codex App 进程内已经注册的 extension，也不会改写 code mode 运行时的 `ALL_TOOLS`。因此它能让模型在正常工具选择中不再看到 `image_gen`，但不是安全隔离边界；若未来需要绝对禁用执行能力，仍需 Codex App 提供可靠的 feature/capability 控制。当前方案的目标是稳定过滤转发请求，同时避免修改 Codex App 本体或依赖不稳定的 App 配置开关。

对齐 Codex 源码时不要扩大过滤范围：`Feature::ImageGeneration=false` 只会让 Tool Plan 跳过 `image_gen.imagegen` executor。系统 `imagegen` skill 由独立的 bundled-skills 机制加载，code mode 模板中的 `generatedImage` 与 `ALL_TOOLS` 也是通用固定文案，因此 Codex 原生关闭 image generation 后仍可能保留这些内容。Gateway 应保留它们，只删除 Codex 在 feature 关闭时本来就不会序列化的工具声明。

## 7. 当前 Sub2API 能力边界

对 `references/sub2api-main` 的代码审查结论：

### 7.1 已支持

- `/v1/responses` 请求。
- GPT-5.6 模型映射。
- 普通 Responses Lite `additional_tools` 透传与保留。
- `namespace`/custom/function 等客户端工具声明。
- 顶层标准 Responses `web_search`。
- `web_search_call` 响应和协议转换。
- 部分协议下的 Web Search emulation。

这解释了为什么 Lite 下的 `exec`、浏览器插件等客户端工具可以正常工作。

### 7.2 新增支持

当前 `references/sub2api-main` 版本为 `0.1.152`，已经新增：

- `/v1/alpha/search`。
- `/alpha/search`。
- `/backend-api/codex/alpha/search`。
- API Key 上游转发到 provider `/v1/alpha/search`。
- OAuth 上游转发到 ChatGPT Codex `/backend-api/codex/alpha/search`。
- model mapping、query string、未知请求/响应字段和上游错误原样保留。

CodexHub 对应链路为：

```text
Codex App
  -> http://127.0.0.1:3847/ai-gateway/v1/alpha/search
CodexHub
  -> Sub2API /v1/alpha/search
Sub2API
  -> OpenAI-compatible provider /v1/alpha/search
```

### 7.3 正确归因

不能简单说“Sub2API 不支持 Responses Lite”。更准确的说法是：

- Sub2API 支持 Lite 请求和客户端工具声明的转发。
- Sub2API 支持标准托管 `web_search`。
- Sub2API 已实现原生 Lite `web.run` 所依赖的 `/alpha/search` 搜索服务。
- CodexHub 已实现 OpenAI Responses-only 的透明代理、模型映射、路由粘性、熔断反馈和请求日志。
- `web.run` 是否出现仍由 Codex App provider gate 决定，不能把“后端路由可用”和“前端工具已注册”混为一谈。

## 8. 2026-07-12 真实验证记录

### 8.1 失败验证：托管工具放进 `additional_tools`

CodexHub 曾尝试把：

```json
{
  "type": "web_search",
  "external_web_access": true,
  "search_content_types": ["text", "image"]
}
```

追加到 `input[].additional_tools.tools`。

请求日志 `8623` 到 `8627` 显示：

- Codex 原始请求没有 `web_search`。
- CodexHub 上游请求已成功追加 `web_search`。
- 上游均返回 HTTP 成功，没有 400。
- 所有 SSE 都没有 `web_search_call`。
- 所有 SSE 都没有 `function/custom name=web_search`。
- 模型明确回答“当前环境没有提供 web_search 工具”。

结论：当前上游把 `additional_tools` 中的 hosted `web_search` 静默忽略。没有报错不等于工具生效。

### 8.2 成功验证：Lite 请求中使用顶层托管工具

真实探针 request id：

```text
codexhub-web-search-probe-1783795233976
```

探针保持：

- `x-openai-internal-codex-responses-lite: true`
- `input[].additional_tools`
- GPT-5.6 Sol

只把托管工具改到顶层：

```json
{
  "tools": [
    {
      "type": "web_search",
      "external_web_access": true,
      "search_content_types": ["text"]
    }
  ]
}
```

结果：

- HTTP 200。
- SSE 正常 completed。
- SSE 出现真实 `web_search_call`。

结论：该探针发出时（2026-07-12），LLMX/Sub2API/上游组合接受“Lite transport + 顶层 hosted web_search”的混合形态。**此结论已在 2026-07-14 失效**，见下节。

## 9. CodexHub 当前兼容实现

> **2026-07-14 更新**：上游 Responses Lite 已明确拒绝顶层 hosted `web_search`，返回 `unsupported_value` / `param=tools`：
>
> ```text
> X-OpenAI-Internal-Codex-Responses-Lite only supports function tools, custom tools, and client-executed tool search.
> ```
>
> 因此第 8.2 节验证过的“注入 hosted web_search”兼容方案已废弃。Gateway 不再注入，改为**剥离** Lite 请求顶层 `tools` 及 `input[].additional_tools.tools` 中的 hosted `web_search`，避免因历史配置或 Codex 工具构造把这类工具塞回请求而触发上游报错。

当前实现在：

```text
src/ai_gateway/handler.rs
strip_hosted_web_search_from_lite_request_tools
```

匹配条件：

1. provider type 为 `OpenAiResponses`。
2. `input` 中存在合法 `additional_tools.tools` 数组，用于确认这是 Lite 请求。

行为：

- 从顶层 `tools` 及 `input[].additional_tools.tools` 移除所有 `type = web_search` / `web_search_preview` 项。
- 保留其余工具的原始顺序。
- 保留 `type = tool_search` 且 `execution = client` 的客户端工具搜索；错误文本明确允许该形态。
- 非 Lite 的标准 Responses 请求不受影响，仍可使用 hosted `web_search`。
- Grok Responses 不处理（provider type 不匹配）。
- 实际剥离时记录日志：`stripped hosted web_search from Responses Lite request tools`，并带上被移除的数量。

### 9.1 原生 Lite 搜索仍走 `web.run`

Lite 的搜索能力由客户端工具 `web.run` + `/alpha/search` 提供，不是 hosted `web_search`。`web.run` 有两种注册形态：

1. 原生 `namespace=web, function=run`。
2. Code Mode `exec.description` 中用精确 Markdown 标题 ``### `web__run` `` 注册（2026-07-13 真实日志形态），模型随后通过 `exec` 调用 `tools.web__run(...)` 触发 `/alpha/search`。

如果异常配置导致 `web.run` 未注册，本轮就不暴露搜索工具，而不是用 hosted 工具伪装。

### 9.2 已知边界

1. GPT-5.6 Lite 若因版本或配置问题未注册原生 `web.run`，模型在该轮没有可用搜索工具。
2. 原生 `web.run` 的 provider gate 已默认开启，仍需实机观察模型列表和 Remote Control 是否有前端回归。
3. 未来 GPT-5.6 或 Sub2API 升级后，Lite 工具协议需重新回归验证。

## 10. 已验证失败或不推荐的方案

| 方案 | 结果/风险 | 结论 |
| --- | --- | --- |
| 把 hosted `web_search` 放进 `additional_tools` | 上游静默忽略，无 `web_search_call` | 不使用 |
| 在 `additional_tools` 伪造 function `web_search` | 模型可能调用，但 Codex 无本地 runtime | 不使用 |
| 只在请求中伪造 `web.run` | 没有 Codex 本地 runtime，模型调用后无法执行 | 不使用 |
| 默认注入 Actor Authorization（旧 Codex） | `account=None`，旧前端套用官方 Statsig 模型白名单 | 历史失败；新版重新验证 |
| 表键保持 `ai-gateway`，身份写为 `OpenAI` | 启用原生 `web.run`，同时开启 remote compaction、请求压缩等 OpenAI 私有行为 | 不使用 |
| CodexHub 将 `/alpha/search` 透明转发到当前 Sub2API OpenAI 渠道 | 保留 command 协议和 opaque response，不做错误的 `/responses` 转换 | 已采用 |
| 直接把 `use_responses_lite=false` | 会改变完整请求编码，GPT-5.6 兼容性未知 | 需单独验证 |
| 把 `/alpha/search` body 原样转成 `/responses` body | 两个协议字段和返回完全不同 | 无效 |

## 11. 原生 `web.run` 当前实现与剩余工作

原生 Lite Web Search 必须同时具备工具注册和搜索 backend。当前新版 Codex 工具 gate、CodexHub transport 与 Sub2API backend 三段均已具备；hosted `web_search` 兼容路径已在 2026-07-14 撤销。

### 11.1 Provider 能力与 gate

当前实现：

1. provider 表键和身份字段都保持 `ai-gateway`。
2. 默认本地 provider 写入 `requires_openai_auth=false`、顶层 `web_search="live"` 和本地 Actor Authorization header。
3. GPT-5.6 Responses Lite 自动选择 standalone search，并把 `web.run` 放入 `additional_tools`；标准 Responses 模型仍可继续使用 hosted `web_search`。
4. Gateway 的 `/alpha/search` transport 将请求限定路由到 OpenAI Responses Provider；Actor header 在 Gateway 入口被过滤。
5. `requires_openai_auth=true`、`name="OpenAI"` 和旧 `ai-codex` 仍作为升级/卸载兼容形态识别；重新配置会迁移到当前 actor-authorized `ai-gateway` 身份。

实机验收必须同时覆盖 `/alpha/search` 调用、工具结果回填、前端完整模型展示、账户区、手机入口和 Remote Control。任一前端能力回归都应保留日志并回滚默认配置，再评估是否增加用户开关。

### 11.2 Statsig 模型白名单

当前 Codex App 前端读取：

```text
dynamic_configs["107580212"].v.available_models
dynamic_configs["107580212"].v.use_hidden_models
dynamic_configs["107580212"].v.default_model
```

正确 bootstrap 形态：

```json
{
  "107580212": {
    "v": {
      "available_models": ["gpt-5.6-sol", "grok-4.5", "Opus-4.8"],
      "use_hidden_models": false,
      "default_model": "gpt-5.6-sol"
    }
  }
}
```

模型数组必须动态来自 CodexHub 实际 `/models` 目录。禁止硬编码固定模型，否则用户修改可见模型或后续增加模型时，前端和 app-server 会再次不一致。

### 11.3 桌面账户态与 Core Provider 分层

`requires_openai_auth=false` 时，Codex Core 的 `account/read` 原始响应为：

```json
{
  "account": null,
  "requiresOpenaiAuth": false
}
```

Codex App 前端据此得到 `authMethod=null`，不会调用 CodexHub 的 `/wham/statsig/bootstrap`，而是访问硬编码 Statsig 地址。无 VPN 时这会造成启动等待；成功读取官方缓存后，又可能用官方 `available_models` 白名单过滤 CodexHub 模型。

曾验证通过 `CODEX_CLI_PATH` 代理改写 `account/read` 可以让前端走 CodexHub Statsig，并显示完整模型列表。但该方案会修改用户级 GUI 环境并介入 Codex App 的 app-server 启动链路，当前发布版本不采用。

当前模型目录仍由 `ai-gateway /models` 提供。实机验证表明 app-server 和 Remote Control 可以读取完整目录，但 Codex App 前端仍可能使用官方 Statsig `available_models` 二次过滤。这个问题属于桌面前端展示层，不影响 Core 请求、模型路由和原生 `web.run` 执行，后续围绕 `account/read` 单独设计不依赖 `CODEX_CLI_PATH` 的方案。

### 11.4 CodexHub 路由

按当前 base URL，CodexHub 已实现：

```http
POST /ai-gateway/v1/alpha/search
```

该路由只选择 `ProviderType::OpenAiResponses`，并复用现有 model alias、权重优先级、会话粘性和熔断状态。请求会转发到：

```http
POST {provider_api_root}/v1/alpha/search
```

原始 query string、除 `model` 映射外的未知 JSON 字段、上游状态码、`content-type` 和响应 body 均保留。

### 11.5 SearchRequest 支持范围

CodexHub 不绑定不断演进的 SearchRequest schema，只要求顶层 `model` 是非空字符串。`id`、`input`、全部 `commands`、`settings`、`max_output_tokens` 以及未来新增字段均透明转发。因此命令支持范围由当前 Sub2API 和最终 OpenAI-compatible 上游决定，而不是由 CodexHub 白名单决定。

### 11.6 搜索 backend 选择

当前采用原生透明转发，不再把 `/alpha/search` 转换成 hosted `/responses + web_search`。后者仍有以下语义差距：

- `/alpha/search` 是明确 command 驱动。
- hosted `web_search` 由模型决定搜索过程。
- open/click/find/screenshot/finance/weather/sports/time 不一定能一一对应。
- hosted Responses 返回 SSE/response items，而 `/alpha/search` 只返回 `{encrypted_output, output}`。

因此 hosted `web_search` 只作为另一条兼容路径，不能冒充 `/alpha/search`。

### 11.7 SearchResponse

Codex 当前最低可解析响应：

```json
{
  "encrypted_output": null,
  "output": "search result"
}
```

注意：

- `output` 是 Codex 当前需要的结果字符串。
- `encrypted_output` 是可选 opaque 数据，CodexHub 不解析、不修改。
- CodexHub 不重组 SearchResponse；成功、错误和未来新增字段都按上游 wire body 返回。

### 11.8 UI 事件

原生 `web.run` runtime 会由 Codex extension 产生 WebSearch begin/end item。若使用原生链路，Gateway 不需要把 `/alpha/search` 本身伪装成 Responses `web_search_call`。

这和当前 hosted `web_search` 不同：当前 UI item 来自上游 SSE 中的 `web_search_call`。

## 12. Responses Lite 其他协议坑

### 12.1 严格反序列化容易被新字段击穿

Responses Lite 会引入：

- `additional_tools`
- `namespace`
- 新的 tool spec 形态
- 新 input item 类型

Responses 到 Responses 透传路径不应先强制反序列化成只覆盖旧协议的完整业务结构。CodexHub 当前使用宽松 `GatewayRequestEnvelope` 只读取路由所需字段，再保留原始 JSON 透传，这是正确方向。

典型错误：

```text
Unknown parameter: input[N].namespace
```

该错误可能来自上游不支持 Lite，也可能来自中间转换器把 namespace 放错层级。必须比较 Codex 原始请求和上游请求，不能只看最终 400。

### 12.2 不要随意改 developer role

Lite 基础 instructions 通过 developer message 注入。把所有 developer message 统一降级为 user 或提升为 system，会改变模型指令层级。

协议转换必须先区分：

- Lite 基础 instructions。
- `additional_tools` carrier。
- 用户配置插入的 developer message。
- 低信任记忆/上下文 developer message。

不要只按 role 一刀切。

### 12.3 不要修改加密上下文

Lite 请求和 compact/续链可能携带：

- `reasoning.encrypted_content`
- compaction blob
- `compaction_summary`
- item id/call id

这些字段可能是上游不透明数据。除已明确验证的兼容修复外，应原样保留。任何重新序列化、裁剪或协议转换都可能触发：

```text
Could not decode the compaction blob. Ensure it is unmodified from the compact response.
```

### 12.4 Lite header 不能被代理层误删

请求体是 Lite 形态时，应保留：

```http
x-openai-internal-codex-responses-lite: true
```

若代理保留 Lite body 却删除 header，上游可能按标准 Responses 解析；若只保留 header 却改写 body，也可能进入不一致状态。

当前 CodexHub header 转发规则会保留该头。升级 header allowlist/blocklist 时必须回归。

### 12.5 `supports_search_tool` 不等于 `web_search` 一定存在

模型元数据中的：

```json
"supports_search_tool": true
```

不代表当前 provider、认证和 runtime 一定提供 Web Search。最终可见工具还受：

- `use_responses_lite`
- provider capabilities
- provider identity
- Actor Authorization
- feature flags
- `web_search_mode`
- extension 是否安装
- `tool_mode=code_mode_only`

共同影响。

同理，`web_search_tool_type=text_and_image` 只决定 hosted tool 被创建时的序列化字段，不会强制创建该工具。

### 12.6 Code Mode Only 会改变工具可见方式

GPT-5.6 使用 `tool_mode=code_mode_only`。很多 namespace 工具不会作为普通顶层 function 直接暴露，而是通过 `exec` 内的工具桥调用，例如：

```javascript
const result = await tools.web__run({
  search_query: [{ q: "query" }]
});
```

因此只看顶层直出工具名称，可能误判工具是否存在。排查时需要同时检查：

- `additional_tools` 完整工具规格。
- `exec` 描述中的嵌套工具。
- Codex ToolRegistry runtime。
- 模型返回的是 custom/function/namespace 还是 hosted call。

Gateway 的 hosted 搜索互斥判断也必须遵守同一规则：直接 namespace `web.run` 和 `exec.description` 中精确的 ``### `web__run` `` 标题都代表原生搜索已注册，此时不得再注入顶层 `web_search`。

### 12.7 原始请求与上游请求必须分开记录

Gateway 可能执行：

- model alias 替换。
- prompt cache 字段补齐。
- Grok reasoning 修复。
- image generation 过滤，包括 hosted `image_generation`、standalone `image_gen` 和 code mode `exec` 描述。
- GPT-5.6 hosted web search 注入。

因此日志必须同时保留：

```text
request_json：Codex 原始请求
upstream_request_json：Gateway 实际发给上游的请求
upstream_response_sse：上游原始流
```

没有这三份数据，就无法判断工具是 Codex 没发、Gateway 删除、上游忽略，还是 Codex UI 没展示。

## 13. 故障定位速查表

| 现象 | 优先检查 | 常见原因 |
| --- | --- | --- |
| 模型说没有 `web_search` | 上游请求中是否有顶层 `tools[].type=web_search` | 工具没注入或放进了 additional_tools |
| `additional_tools` 有 `web_search`，但无搜索事件 | `upstream_response_sse` 是否有 `web_search_call` | hosted 工具放错层，被静默忽略 |
| 模型调用 `web_search` 后报 unknown tool | 返回 item 是 function/custom 还是 hosted | 伪造了客户端函数但没有 runtime |
| `web.run` 不出现在工具列表 | provider gate、Actor Authorization、web_search_mode | 默认配置未生效、Codex 版本过旧或搜索被关闭 |
| `web.run` 可见但执行 404 | 实际请求 URL、安装版本 | CodexHub/Sub2API 仍是旧版本，或 provider base URL 指向不支持该接口的上游 |
| Sub2API 返回 404 | 渠道平台和版本 | 非 OpenAI group，或运行版本早于 `0.1.152` |
| 上游 400 `input[N].namespace` | 对比原始与上游 JSON | 上游不支持 Lite，或 namespace 被错误改写 |
| 有搜索答案但 UI 没有搜索 item | SSE 是否有 `web_search_call` | 上游只生成文本，没有标准事件 |
| `web_search_call` 有但 query 为空 | added/done 事件顺序和 action | 过早发出 completed item |
| GPT-5.6 搜索生效但旧模型行为变化 | 注入 scope | 条件过宽，误伤非 Lite/非 GPT-5.6 |

## 14. 升级审计清单

每次同步 Codex 或 Sub2API 新版本后，至少检查以下内容。

### 14.1 Codex 模型元数据

- GPT-5.6 是否仍为 `use_responses_lite=true`。
- 是否新增 GPT-5.6 变体。
- `tool_mode` 是否变化。
- `web_search_tool_type` 是否变化。
- `supports_search_tool` 是否变化。

### 14.2 Codex 请求构造

- `core/src/client.rs::build_responses_request`。
- `core/src/tools/spec_plan.rs::hosted_model_tool_specs`。
- Lite 是否仍排除 hosted tools。
- `additional_tools` 是否新增字段。
- Lite header 名称是否变化。
- `reasoning.context` 和 `parallel_tool_calls` 是否变化。

### 14.3 Web Search extension

- `ext/web-search/src/extension.rs` 的 provider gate。
- `ext/web-search/src/tool.rs` 的 namespace/tool 名称。
- `codex-api/src/endpoint/search.rs` 的 endpoint path。
- `codex-api/src/search.rs` 的 SearchRequest/SearchResponse 字段。
- 是否新增 command、settings 或 allowed caller。

### 14.4 Sub2API

- 是否新增 `/alpha/search`。
- 是否新增 Responses Lite 专门解析器。
- 是否继续保留 `additional_tools`。
- 是否继续支持 Lite header + 顶层 `web_search` 混合形态。
- hosted `web_search` 是否仍返回标准 `web_search_call`。

### 14.5 CodexHub 回归

- GPT-5.6 Lite 原始请求不含 hosted `web_search`。
- 上游请求只在顶层新增一次 `web_search`。
- additional client tools 未被移动或删除。
- 原始请求已有直接 `web.run`，或 Code Mode `exec` 已注册 `web__run` 时不新增 hosted `web_search`。
- Grok/Anthropic/Chat Completions 不误注入。
- GPT-5.4/GPT-5.5 标准请求不误注入。
- 上游 SSE 出现 `web_search_call`。
- Codex App 显示搜索 item。
- 搜索结束后会话能继续，不产生孤儿 tool call。

## 15. 测试要求

当前兼容方案至少保留以下单元测试：

- GPT-5.6 Lite 请求创建顶层 `web_search`。
- additional client tools 保持不变。
- 注入幂等。
- 保留已有顶层工具。
- 非 GPT-5.6 不注入。
- 非 OpenAI Responses provider 不注入。
- 无 `additional_tools` 的标准请求不注入。
- `additional_tools` 已包含原生 `web.run` 时不注入。

原生 `web.run` transport 已增加：

- `/ai-gateway/v1/alpha/search` 路由。
- OpenAI Responses Provider 类型限定。
- model alias、会话粘性、权重和熔断复用。
- query、未知字段、状态码和响应 body 透明保留。
- `alpha_search` 请求日志。

仍需增加或做真实环境验证：

- `/alpha/search` 上游 2xx、4xx、5xx 和 query passthrough 集成测试。
- web.run 调用、搜索、工具结果回填、下一轮 Responses 的端到端测试。
- 404、超时、上游 4xx/5xx、空 output、取消请求测试。
- Codex App WebSearch begin/end UI item 测试。
- actor-authorized provider 下回归原生 `web.run`、自定义模型、账号区、手机入口和 Remote Control。

## 16. 当前决策记录

当前版本选择：

```text
保留 Responses Lite
+ provider 表键和身份均保持 ai-gateway
+ 默认 requires_openai_auth=false、Actor Authorization 且 web_search=live
+ GPT-5.6 Lite 使用 Codex 原生 web.run
+ 提供 /ai-gateway/v1/alpha/search 透明代理
+ 只允许 OpenAI Responses Provider 承接 alpha/search
+ 标准非 Lite Responses 继续允许 hosted web_search
- 不将本地 provider 伪装成 OpenAI
- 不启用 OpenAI remote compaction
- 不伪造或本地模拟 SearchResponse
```

理由：

1. Responses Lite 上游明确拒绝 hosted `web_search`，必须使用客户端执行工具。
2. 更新后的 Codex 明确支持 actor-authorized 自定义 Provider 注册 `web.run` 和拉取 `/models`。
3. 当前 Sub2API 已实现 alpha search，CodexHub 只做透明 transport，不猜测 command 语义。
4. `name="OpenAI"` 会额外开启 remote compaction、zstd 和私有 metadata，不适合作为单纯开启搜索的手段。
5. 原生 `web.run` 与 hosted `web_search` 互斥：Lite 使用前者，标准 Responses 可继续使用后者。
6. 旧版 Actor Authorization 隐藏自定义模型的问题必须重新做实机验证；若仍存在则回滚配置并保留日志。

发布前应以本文第 11、14、15 节为验收基线，确认客户端 provider gate、CodexHub endpoint、搜索 backend、账户模型和 Remote Control 在同一版本中同时可用。

## 17. 待探讨方案：Gateway 内部搜索工具循环

状态：**仅记录设计，当前未实现、未启用，也不进入当前发布范围。**

### 17.1 提出背景

Gateway 内部工具循环最初用于规避旧版原生 `web.run` 的两条限制：

1. 旧版使用 Actor Authorization 会让 provider account 变成 `None`，并曾触发官方 Statsig 模型白名单，导致 CodexHub 自定义模型消失；新版正在重新验证。
2. 仅在 Gateway 请求中注入 `web.run` 工具定义，只会让模型看见工具，不会在 Codex App 本地注册对应 executor。若工具调用被直接透传，Codex 仍可能报 unsupported tool。

因此可以考虑把搜索执行和后续模型调用全部留在 Gateway 内部，对 Codex 隐藏中间工具轮次。

### 17.2 Codex 原生流程不是一次请求

GPT-5.6 Code Mode 下，原生搜索通常不是模型直接返回 namespace `web.run`，而是：

```text
模型返回 custom_tool_call(name=exec)
  -> exec JavaScript 调用 tools.web__run(...)
  -> Codex 本地 Code Mode runtime 执行 JavaScript
  -> Web Search extension 请求 /v1/alpha/search
  -> Codex 生成 custom_tool_call_output
  -> Codex 再次请求模型
  -> 模型生成最终回答
```

对应源码与测试：

- `references/codex-main/codex-rs/core/tests/suite/code_mode.rs::assert_code_mode_standalone_web_search`
- `references/codex-main/codex-rs/ext/web-search/src/tool.rs::WebSearchTool::handle_call`
- `references/codex-main/codex-rs/ext/web-search/src/extension.rs::WebSearchExtensionConfig`

这意味着 Gateway 不能把 `/alpha/search` 的原始结果直接当作模型回答返回 Codex。搜索结果必须先作为 tool output 交回模型，再由模型生成用户可读的最终回答。

### 17.3 候选请求链路

```text
Codex App
  -> CodexHub /v1/responses
CodexHub
  -> 向上游请求临时注入 Gateway 私有搜索 function
上游模型
  -> 返回私有搜索 function_call
CodexHub
  -> 截流，不把该 function_call 发给 Codex App
  -> 调用现有 /v1/alpha/search
  -> 把 function_call + function_call_output 追加到内部请求
  -> 再次请求上游模型
上游模型
  -> 返回最终 assistant response
CodexHub
  -> 将最终响应流式返回 Codex App
```

该模式本质上是 Gateway 托管的 agent loop，不是 Codex 原生 `web.run` runtime。

### 17.4 不建议直接使用 `web.run` 名称

候选实现应优先注入只对上游可见的私有 function，例如：

```text
codexhub_web_run
```

它可以复用 `SearchCommands` 的参数 schema 和 `web.run` 的工具说明，但不应伪装成 Codex 本地已经注册的 namespace tool。原因包括：

- 避免与未来真正注册的 `web.run` 冲突。
- 避免 Codex 把中间调用当作需要本地执行的工具。
- Gateway 可以精确识别并只截流自己注入的 function。
- hosted `web_search`、原生 `web.run` 和 Gateway 私有搜索必须保持互斥。

### 17.5 关键可行性探针

实现完整内部循环前，必须先用真实 GPT-5.6 上游验证：

1. 在 Responses Lite 顶层注入普通 function。
2. 不修改 Codex 原有 `exec` description。
3. 让模型执行一次明确的搜索任务。
4. 检查模型是否直接返回该 function 的 `function_call`。

只有直接返回 `function_call`，Gateway 才能稳定截流。

如果模型仍返回：

```text
custom_tool_call(name=exec)
input = "const result = await tools.web__run(...);"
```

则 Gateway 需要执行模型生成的任意 JavaScript，或者解析 JavaScript 并模拟 Code Mode runtime。两者都会显著扩大安全面、依赖和维护成本。不得使用正则或字符串截取来模拟 JavaScript 语义；在没有可靠沙箱和完整工具注册表前，该分支应判定为不可落地。

### 17.6 候选 MVP 边界

若关键探针通过，第一版仍应严格限制：

1. 只针对 `OpenAiResponses` 和明确支持的 GPT-5.6 模型。
2. 私有搜索 function 只注入上游请求，不写入 Codex 会话历史。
3. 每轮只截流 Gateway 自己注入的搜索调用。
4. 同一响应混有未知工具调用时不进入内部循环，避免 Gateway 和 Codex 同时拥有工具执行权。
5. 最多执行 3 至 4 轮搜索，超过上限返回明确错误。
6. 搜索请求使用稳定的 thread/session id，保证同一内部循环中的 `open`、`click`、`find` 和 ref id 可继续使用。
7. 客户端取消、Gateway 超时和上游失败必须能终止整个内部循环。
8. 请求日志分别记录模型轮次和 `/alpha/search` 轮次，并统计隐藏轮次的 token usage。
9. 中间搜索轮次不透传；最终 assistant response 尽量保持正常 SSE 流式输出。
10. MVP 不伪造 Codex 原生 WebSearch begin/end item，Codex App 只显示最终回答；搜索过程先在 CodexHub 请求日志中展示。

### 17.7 仍需解决的问题

- 如何在不完整缓冲最终响应的前提下，判断当前响应是搜索调用还是最终文本。
- 多个并行搜索调用的执行顺序和 tool output 顺序。
- 第一轮模型产生 reasoning item 时，内部 follow-up 应保留哪些字段和私有状态。
- `store=false`、Responses Lite input 和 `previous_response_id` 的兼容策略。
- 隐藏搜索历史不进入 Codex 下一轮上下文后，跨用户轮次 ref id 是否仍能稳定复用。
- 多轮模型 usage、TTFT、总延迟和错误状态如何汇总到单条用户请求日志。
- 是否需要在后续版本中合成标准 `web_search_call` SSE 事件，以及 Codex App 对合成事件序列的容忍度。

### 17.8 当前决策

当前不实现 Gateway 内部搜索循环。保留本节作为后续设计输入，只有在“GPT-5.6 能直接调用 Gateway 私有 function”的真实上游探针通过后，才继续评估 MVP；若必须执行 `exec` JavaScript，则暂不采用该方案。

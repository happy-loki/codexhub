# AI Gateway Anthropic Messages 优先路线

更新时间：2026-06-20

状态：路线设计稿，用于指导后续实现。

本文记录 `codexhub` AI Gateway 后续演进方向：入口继续兼容 Codex 使用的 OpenAI Responses 协议，出站主线转向 Anthropic Messages 协议。Chat Completions 保留为历史兼容路径，不再作为新 provider 接入的主要抽象。

相关文档：

- [`ai-gateway-architecture.zh-CN.md`](ai-gateway-architecture.zh-CN.md)：当前 AI Gateway 总体架构和已落地能力。
- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)：当前 Anthropic Messages adapter 设计与实现记录。
- [`ai-gateway-glm-anthropic-integration.zh-CN.md`](ai-gateway-glm-anthropic-integration.zh-CN.md)：智谱 Anthropic Messages profile 对接说明，可作为后续新增厂商模板。
- [`ai-gateway-provider-adapter-design.zh-CN.md`](ai-gateway-provider-adapter-design.zh-CN.md)：更通用的 Provider Adapter / Gateway IR 设计草案。

## 1. 背景判断

Codex 侧当前稳定入口是 OpenAI Responses。这个事实短期不会变，AI Gateway 必须继续把 `/v1/responses` 作为对 Codex 的主入口。

出站 provider 侧则在变化：

- 越来越多厂家模型支持 Anthropic Messages 兼容协议。
- DeepSeek、Kimi、智谱等模型未来或已经可以通过 Messages 形态接入。
- Chat Completions 协议语义较薄，对 reasoning、tool_use、tool_result、cache_control、content block 等能力表达不如 Messages 自然。
- 如果继续围绕 Chat Completions 做 provider 泛化，会把未来主要精力投入到一条可能逐步退场的兼容路径上。

因此后续战略应调整为：

```text
Codex / OpenAI Responses
  -> codexhub AI Gateway
  -> Anthropic Messages-compatible providers
```

OpenAI Responses 与 Anthropic Messages 是未来的两个核心协议面：

- **OpenAI Responses**：Codex 入站协议，也是 OpenAI provider 的最佳直连协议。
- **Anthropic Messages**：非 OpenAI provider 的优先出站协议，用于接入 Claude、DeepSeek Messages、Kimi Messages、智谱 Messages 等兼容实现。

Chat Completions 只保留既有 DeepSeek 链路和必要 bugfix，不再作为新厂家接入优先路径。

## 2. 目标架构

目标链路：

```text
Codex Responses Wire
  -> Responses Inbound Decoder
  -> GatewayTurn IR
  -> Anthropic Messages Adapter
  -> Vendor Anthropic-compatible API
  -> Anthropic Stream/JSON Decoder
  -> Responses Outbound Encoder
  -> Codex Responses Wire
```

其中 OpenAI provider 可以继续短路：

```text
Codex Responses Wire
  -> OpenAI Responses passthrough
  -> OpenAI /v1/responses
```

目标不是把 Anthropic Messages 再转换成 Chat，也不是给每个厂家复制一套 provider，而是：

```text
anthropic_messages provider
  + ProviderProfile
  + ProviderCapabilities
  + ProviderQuirks
```

每家厂商的差异应集中在 profile/capabilities 层，而不是散落在 request、response、stream 转换代码里。

## 3. Provider 分类

后续 `ProviderType` 应表示协议族：

```rust
pub enum ProviderType {
    OpenAiResponses,
    AnthropicMessages,
    ChatCompletions,
}
```

含义：

- `openai_responses`：Responses 原生或兼容 provider，优先透传。
- `anthropic_messages`：Messages 原生或兼容 provider，未来新厂家接入优先使用。
- `chat_completions`：历史兼容 provider，主要保留既有 DeepSeek Chat 链路。

不要把 provider type 用作厂家名。厂家差异应放到 `compatibility` / `profile`。

建议配置形态：

```toml
[[aiGateway.providers]]
name = "claude"
enabled = true
providerType = "anthropic_messages"
compatibility = "anthropic"
baseUrl = "https://api.anthropic.com/v1"
apiKey = "..."
models = ["claude-sonnet-4-5"]

[[aiGateway.providers]]
name = "deepseek-messages"
enabled = true
providerType = "anthropic_messages"
compatibility = "deepseek_anthropic"
baseUrl = "https://api.deepseek.com/v1"
apiKey = "..."
models = ["deepseek-v4-pro"]

[[aiGateway.providers]]
name = "kimi-messages"
enabled = true
providerType = "anthropic_messages"
compatibility = "kimi_anthropic"
baseUrl = "https://api.moonshot.cn/anthropic/v1"
apiKey = "..."
models = ["kimi-k2"]

[[aiGateway.providers]]
name = "glm-messages"
enabled = true
providerType = "anthropic_messages"
compatibility = "glm_anthropic"
baseUrl = "https://open.bigmodel.cn/api/anthropic"
apiKey = "..."
models = ["glm-4.6"]
```

`compatibility` 是受支持厂商 profile 的白名单，不是任意 Anthropic-compatible 自由文本。未知厂商不兜底为通用兼容模式；只有经过登记和测试的 profile 才允许进入 GUI 和配置。字段命名需要兼容当前实现的 `provider_type` / `providerType` 解析习惯。

## 4. AnthropicProviderProfile

建议新增 Anthropic 专用 profile：

```rust
pub enum AnthropicProviderProfile {
    Anthropic,
    DeepSeekAnthropic,
    GlmAnthropic,
    KimiAnthropic,
    OpenRouterAnthropic,
}
```

profile 负责选择默认能力和兼容差异。它不应直接散落在转换函数里的 `if provider.name == ...`。

建议运行期合成 options：

```rust
pub struct AnthropicProviderOptions {
    pub profile: AnthropicProviderProfile,
    pub auth: AnthropicAuthStyle,
    pub version_header: AnthropicVersionHeader,
    pub endpoint: AnthropicEndpointStyle,
    pub capabilities: AnthropicCapabilities,
    pub quirks: AnthropicQuirks,
}
```

### 4.1 Auth 差异

不同兼容实现可能使用不同鉴权方式：

```rust
pub enum AnthropicAuthStyle {
    XApiKey,
    Bearer,
}
```

默认：

- `anthropic`：`Authorization: Bearer`，并按 Claude Code 参考请求发送托管 header
- `glm_anthropic`：`Authorization: Bearer`，与 Anthropic profile 保持 Claude Code header 形态
- `openrouter_anthropic`：通常 `Authorization: Bearer`
- 其它 vendor profile 按实测配置

### 4.2 Version Header 差异

Anthropic 原生要求 `anthropic-version`。部分兼容厂家可能：

- 必须传固定版本。
- 可传但忽略。
- 不接受该 header。

```rust
pub enum AnthropicVersionHeader {
    Required(&'static str),
    Optional(&'static str),
    Omit,
}
```

当前实现固定写 `anthropic-version`，后续应改为由 options 控制。

### 4.3 Endpoint 差异

大多数 Messages API 使用：

```text
POST /v1/messages
```

但兼容厂家可能把 Anthropic API 放到不同 base path 下。当前 `provider_api_root(base_url)` 会去掉末尾 `/v1` 再拼 `/v1/messages`，对所有兼容厂家不一定够用。后续应支持：

```rust
pub enum AnthropicEndpointStyle {
    V1Messages,
    MessagesFromBaseUrl,
}
```

或更简单地允许 provider 配置覆盖 path：

```toml
messagesPath = "/v1/messages"
```

第一阶段可以保持当前行为，只把 endpoint 差异放进 options 设计。

## 5. AnthropicCapabilities

能力应显式建模，避免静默丢弃。

建议结构：

```rust
pub struct AnthropicCapabilities {
    pub text: bool,
    pub images: bool,
    pub tools: bool,
    pub parallel_tool_use: bool,
    pub tool_choice: AnthropicToolChoiceSupport,
    pub thinking: AnthropicThinkingSupport,
    pub prompt_cache_control: bool,
    pub system_cache_control: bool,
    pub web_search_tool: bool,
    pub computer_use_tool: bool,
    pub max_tool_result_blocks: Option<usize>,
}
```

工具选择能力：

```rust
pub enum AnthropicToolChoiceSupport {
    None,
    AutoOnly,
    AutoAnyTool,
    Full,
}
```

Thinking 能力：

```rust
pub enum AnthropicThinkingSupport {
    None,
    AnthropicNative,
    VendorSpecific,
}
```

能力处理原则：

- provider 支持：原样映射。
- provider 不支持但可降级：明确降级，并记录到 request log。
- provider 不支持且无法降级：返回清晰错误，不静默丢弃。

本地工具能力是 Codex core 执行的，AI Gateway 只负责把 tool definition、tool_use、tool_result 保真传给模型侧。

## 6. AnthropicQuirks

兼容厂家差异不一定是“能力缺失”，也可能是格式细节。建议集中到 quirks：

```rust
pub struct AnthropicQuirks {
    pub require_user_message_first: bool,
    pub merge_consecutive_same_role_messages: bool,
    pub allow_empty_text_blocks: bool,
    pub require_tool_result_after_tool_use: bool,
    pub tool_name_regex: Option<&'static str>,
    pub usage_shape: AnthropicUsageShape,
    pub stream_event_shape: AnthropicStreamShape,
}
```

示例：

- 某些 provider 可能不接受连续 assistant messages，需要合并。
- 某些 provider 对 tool name 的正则更严格，需要继续复用 `ToolNameMap`。
- 某些 provider 的 stream usage 事件字段名不完全相同，需要在 response/stream decoder 层处理。

Quirks 应只影响 Anthropic adapter，不应污染 `GatewayTurn`。

## 7. Responses -> Messages 映射原则

Anthropic Messages 比 Chat 更接近 Responses，因此后续转换应以 Gateway IR 为中心。

### 7.1 输入映射

```text
Responses instructions
  -> Anthropic system

Responses message(role=user)
  -> Anthropic message(role=user, content=[text/image/tool_result...])

Responses message(role=assistant)
  -> Anthropic message(role=assistant, content=[text/tool_use/thinking...])

Responses function_call / custom_tool_call / tool_search_call
  -> Anthropic tool_use block

Responses function_call_output / custom_tool_call_output / tool_search_output
  -> Anthropic tool_result block

Responses reasoning
  -> Anthropic thinking block 或 vendor-specific thinking
```

原则：

- `call_id` 必须稳定映射到 Anthropic `tool_use.id`。
- `tool_result` 必须保留对应 `tool_use_id`。
- namespaced tool 使用 `ToolNameMap` 编码为 provider 可接受名称，再在返回时还原。
- Unknown item 保留 raw；不能表达时返回明确错误或降级说明。

### 7.2 工具定义映射

Responses 工具形态：

```text
function
namespace/tool_search
custom
builtin web_search / image_generation / computer_use
```

Messages 工具形态：

```text
tools: [{ name, description, input_schema }]
```

映射策略：

- 普通 function：直接转 Anthropic tool。
- namespace tool：flatten 成 Anthropic tool name，并用 `ToolNameMap` 保留 roundtrip。
- tool_search：优先作为一等动态工具语义进入 IR；provider 不支持动态工具时，按已加载工具列表展开。
- custom tool：如果 provider 不支持 custom tool，转为普通 tool 或返回不支持错误。
- builtin tool：按 provider capability 判断支持、降级或拒绝。

## 8. Messages -> Responses 映射原则

非流式返回：

```text
Anthropic text block
  -> response message output_text

Anthropic thinking block
  -> response reasoning item

Anthropic tool_use block
  -> response function_call / custom_tool_call / tool_search_call

Anthropic stop_reason
  -> response status / incomplete_details

Anthropic usage
  -> Responses usage
```

流式返回：

```text
message_start
content_block_start
content_block_delta
content_block_stop
message_delta
message_stop
```

应转换为 Codex 可消费的 Responses SSE：

```text
response.created
response.output_item.added
response.content_part.added
response.output_text.delta
response.function_call_arguments.delta
response.reasoning_summary_text.delta
response.output_item.done
response.completed
```

不同 vendor 的 stream event shape 应由 `AnthropicStreamShape` 处理，不应复制整套 stream 状态机。

## 9. 当前代码适配计划

当前实现已经有 Anthropic provider：

```text
src/ai_gateway/providers/anthropic_messages/
  mod.rs
  request.rs
  request_content.rs
  request_reasoning.rs
  request_tools.rs
  response.rs
  stream*.rs
```

后续建议按小步迁移：

### Phase A：Profile / Options 骨架

- 在配置中新增 `compatibility` 字段。
- 为 `ProviderType::AnthropicMessages` 解析 `AnthropicProviderProfile`。
- 新增 `AnthropicProviderOptions::from_provider(provider)`。
- 当前默认 profile 为 `anthropic`，行为保持不变。
- `mod.rs` 构造 request 前先生成 options，并传入 request/stream/response 模块。

验收：

- 现有 Anthropic tests 全部通过。
- 未配置 `compatibility` 时行为不变。

### Phase B：Auth / Version / Endpoint 由 options 控制

- `x-api-key` / `Authorization: Bearer` 由 `AnthropicAuthStyle` 控制。
- `anthropic-version` 是否发送由 `AnthropicVersionHeader` 控制。
- `/v1/messages` 路径由 endpoint options 或配置控制。

验收：

- `anthropic` profile 输出与当前一致。
- 新增一个 vendor profile 单测验证 Bearer auth / omit version / custom path。

### Phase C：Capabilities 驱动 request 构造

- `thinking`、`cache_control`、`tool_choice`、builtin tools 由 capabilities 决定。
- 不支持的能力必须显式降级或报错。
- 降级结果写入 request log 或 debug log。

验收：

- Anthropic 原生 capability 保持当前行为。
- 新增兼容 vendor profile 时，只改 profile/capabilities 单元测试。

### Phase D：GatewayTurn 逐步接入 Anthropic adapter

- `responses_inbound` 已经可以 decode `GatewayTurn`。
- 先让 Anthropic request builder 支持从 `GatewayTurn` 构造 messages/tools。
- 与当前 `GatewayRequest` 路径并行一段时间，用 golden tests 对比输出。
- 稳定后 Anthropic adapter 改以 `GatewayTurn` 为主输入。

验收：

- Messages/tools/tool_result/thinking golden tests 全部一致。
- Unknown / unsupported item 有明确错误路径。

### Phase E：Vendor Profile 扩展

按优先级接入：

```text
anthropic
glm_anthropic
deepseek_anthropic
kimi_anthropic
openrouter_anthropic
```

每新增一家：

- 增加 profile 默认 options。
- 增加 auth/version/endpoint/capabilities 单测。
- 用最小真实样本记录 request/response/stream golden case。
- 不复制 provider 主逻辑。
- 不为未知 profile 提供通用兜底。

## 10. Chat Completions 处理策略

Chat Completions 保留：

- 现有 DeepSeek Chat provider 继续可用。
- 当前 DeepSeek strict 兼容规则继续保留。
- 只修 bug 和必要兼容，不围绕 Chat 做新的 provider profile 架构。

新增厂家默认不走 Chat，除非该厂家完全没有 Anthropic Messages 兼容 API。

如果未来 Chat 协议逐步退出：

- `ProviderType::ChatCompletions` 可标记为 legacy。
- GUI 可降低 Chat provider 的推荐优先级。
- 文档将新 provider 接入指向 Anthropic Messages profile。

## 11. 开放问题

- 各厂家 Anthropic-compatible API 的真实 stream event 是否完全兼容 Anthropic 原生。
- DeepSeek/Kimi/智谱是否支持 Anthropic prompt cache control，字段是否一致。
- 各厂家是否支持 thinking block，还是只支持纯文本/工具。
- 对 web_search / computer_use 等 builtin tool 的支持边界。
- Codex 前端对 Responses SSE 中 reasoning/tool event 的容错范围。

这些问题需要用真实 provider 样本逐步验证，不能只靠文档假设。

## 12. 推荐下一步

下一轮实现建议只做 Phase A：

1. `ProviderConfig` 增加 `compatibility` 字段。
2. 新增 `AnthropicProviderProfile` / `AnthropicProviderOptions`。
3. Anthropic provider 在入口处生成 options，并传给 request builder。
4. 默认 `anthropic` profile 保持当前行为。
5. 不改 DeepSeek Chat provider。

这一步完成后，后续接 DeepSeek/Kimi/智谱 Messages 协议时，就有清晰落点。

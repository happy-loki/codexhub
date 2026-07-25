# AI Gateway Provider Adapter 完备转换设计

更新时间：2026-06-19

状态：设计草案，可修订。

本文档记录 `codexhub` AI Gateway 从“Responses 到 Chat Completions 的点对点转换”升级为“Codex Responses 入口协议 + Gateway IR + 多 Provider Adapter”的设计。它用于指导后续实现，但不是唯一真实来源；落地过程中如果发现 Codex、DeepSeek、Anthropic 或其它 provider 的协议细节与本文不一致，应优先修正实现并同步更新本文档。

相关文档：

- [`ai-gateway-architecture.zh-CN.md`](ai-gateway-architecture.zh-CN.md)：当前 AI Gateway 总体架构和 Phase 1 说明。
- [`ai-gateway-impl.zh-CN.md`](ai-gateway-impl.zh-CN.md)：当前实现计划和落地记录。
- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)：Anthropic Messages 独立 adapter 设计和验收计划。

## 1. 背景

Codex 侧按 OpenAI Responses 协议请求 AI Gateway。当前实现中：

- OpenAI Responses provider 基本 raw passthrough。
- DeepSeek 等 Chat Completions provider 会把 Responses 请求反序列化成 `GatewayRequest`，再转成 Chat 请求。
- `GatewayRequest` 当前偏向 Chat 兼容，不能完整表达 Codex Responses 的工具、动态工具和未知扩展语义。

这导致 Chat provider 链路存在语义损耗：

- `tool_search` 这类 Responses 原生 tool 可能被过滤。
- `tool_search_output` 等 input item 缺少完整模型表达。
- namespaced tool 被 flatten 后回解不完整。
- `custom`、`web_search`、`image_generation` 等工具能力缺少统一降级策略。
- 后续接 Anthropic Messages 时，如果继续在 Chat 中间态上叠补丁，会再次发生语义丢失。

目标不是把所有 provider 伪装成 OpenAI Chat，而是让 Gateway 有一个保真内部语义层，再由各 provider adapter 按能力做映射。

## 2. 设计目标

核心目标：

- Codex 入口协议仍然是 OpenAI Responses。
- Gateway 内部使用可保真的 IR，不绑定 Chat Completions 或 Anthropic Messages。
- Provider adapter 只负责自身协议映射和能力降级。
- OpenAI Responses 仍优先走 raw passthrough，避免无谓改写。
- Chat Completions、Anthropic Messages 等非 Responses provider 都通过同一套 IR 接入。
- 完整支持 Codex dynamic tools / deferred tools / `tool_search`，让 Chrome、Browser、Apps、MCP 等工具可在非 Responses provider 下工作。
- 对无法无损转换的能力给出显式策略：支持、降级、拒绝、或保留原始 raw 扩展，不静默丢弃。
- 为后续 ledger、WebSocket、跨 provider 切换预留状态模型。

非目标：

- Gateway 不执行 Codex 工具。工具执行仍由 Codex core/app-server 处理。
- Gateway 不替代 Codex approval、turn、thread、dynamic tool runtime。
- Gateway 不承诺不同 provider 之间共享真实 prompt cache。
- 第一轮实现不要求所有 provider 都达到同等能力，但 IR 必须能表达完整语义。

## 3. 总体架构

目标链路：

```text
Codex Responses Wire
  -> Responses Inbound Decoder
  -> GatewayTurn IR
  -> ProviderAdapter
  -> Provider Native Request
  -> Provider Native Response / Stream
  -> ProviderAdapter
  -> GatewayEvent / GatewayTurnDelta IR
  -> Responses Outbound Encoder
  -> Codex Responses Wire
```

Provider 类型：

```text
openai_responses
  - raw passthrough 优先
  - 可选解析请求/响应用于日志、ledger、调试

chat_completions
  - DeepSeek、OpenAI Chat-compatible、其它兼容厂商
  - tools 扁平化，tool calls 还原为 Gateway IR

anthropic_messages
  - Claude Messages API
  - content blocks、tool_use、tool_result、system 字段单独映射

future adapters
  - Gemini、Mistral、Moonshot、Qwen 等
  - 只新增 adapter，不改 Codex 入口协议
```

## 4. Gateway IR

IR 需要表达 Codex Responses 的语义，而不是 provider 请求 JSON。

建议新增模块：

```text
src/ai_gateway/ir.rs
src/ai_gateway/adapters/mod.rs
src/ai_gateway/adapters/openai_responses.rs
src/ai_gateway/adapters/chat_completions.rs
src/ai_gateway/adapters/anthropic_messages.rs
src/ai_gateway/codec/responses_inbound.rs
src/ai_gateway/codec/responses_outbound.rs
src/ai_gateway/codec/sse.rs
```

概念结构：

```rust
pub struct GatewayTurn {
    pub model: String,
    pub instructions: Option<String>,
    pub input: Vec<GatewayItem>,
    pub tools: Vec<GatewayTool>,
    pub tool_choice: Option<GatewayToolChoice>,
    pub reasoning: Option<GatewayReasoning>,
    pub text: Option<GatewayTextOptions>,
    pub stream: bool,
    pub max_output_tokens: Option<i64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub prompt_cache_key: Option<String>,
    pub prompt_cache_retention: Option<String>,
    pub previous_response_id: Option<String>,
    pub raw: serde_json::Value,
}

pub struct GatewayContext {
    pub request_id: String,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub prompt_cache_key: String,
    pub upstream_headers: HeaderMap,
}
```

### 4.1 GatewayItem

IR item 至少覆盖：

```rust
pub enum GatewayItem {
    Message(GatewayMessage),
    Reasoning(GatewayReasoningItem),
    FunctionCall(GatewayFunctionCall),
    FunctionCallOutput(GatewayToolOutput),
    ToolSearchCall(GatewayToolSearchCall),
    ToolSearchOutput(GatewayToolSearchOutput),
    CustomToolCall(GatewayCustomToolCall),
    CustomToolCallOutput(GatewayToolOutput),
    WebSearchCall(GatewayBuiltinCall),
    ImageGenerationCall(GatewayBuiltinCall),
    LocalShellCall(GatewayBuiltinCall),
    Unknown(GatewayUnknownItem),
}
```

原则：

- `Unknown` 必须保存 `type` 和完整 `raw`，不能直接丢弃。
- 所有 call/output item 必须保留 `call_id`。
- 所有 assistant tool call 必须能回到 Codex Responses item。
- `tool_search_call` / `tool_search_output` 是一等 item，不使用普通 function output 偷偷承载。

### 4.2 Content Block

```rust
pub enum GatewayContentBlock {
    Text {
        text: String,
        kind: TextKind,
        raw: Option<Value>,
    },
    Image {
        image_url: String,
        detail: Option<String>,
        raw: Option<Value>,
    },
    ToolUse {
        id: String,
        tool: ToolKey,
        input: Value,
        raw: Option<Value>,
    },
    ToolResult {
        call_id: String,
        content: Vec<GatewayContentBlock>,
        is_error: Option<bool>,
        raw: Option<Value>,
    },
    Unknown {
        block_type: String,
        raw: Value,
    },
}
```

`TextKind` 用于区分 input/output/summary 等来源，但 provider adapter 可以按自身协议折叠。

### 4.3 ToolKey 和 ToolNameMap

Responses 支持 namespace，Chat Completions / Anthropic Messages 的 tool name 通常是平面字符串。不能把 namespace 简单拼接后靠 `starts_with("mcp__")` 回解。

内部工具标识：

```rust
pub struct ToolKey {
    pub namespace: Option<String>,
    pub name: String,
    pub kind: ToolKind,
}

pub enum ToolKind {
    Function,
    ToolSearch,
    Custom,
    BuiltinWebSearch,
    BuiltinImageGeneration,
    Dynamic,
    Unknown,
}
```

每次出站 provider 请求构建一个 `ToolNameMap`：

```rust
pub struct ToolNameMap {
    pub encoded_to_key: HashMap<String, ToolKey>,
    pub key_to_encoded: HashMap<ToolKey, String>,
}
```

推荐编码：

```text
plain function:
  get_weather

namespaced function:
  <namespace>__codexns__<name>

tool_search:
  tool_search
```

约束：

- 编码结果必须满足 provider 的 tool name 正则限制。
- 如果冲突，追加短 hash，例如 `__h_<8hex>`。
- `ToolNameMap` 必须进入 request-scoped adapter state；非流式响应和流式响应都要用同一个 map 回解。
- 旧的 `responses_unit__` 可以作为兼容输入读取，但新实现不应继续依赖它。

## 5. Tool 语义映射

### 5.1 GatewayTool

```rust
pub enum GatewayTool {
    Function(GatewayFunctionTool),
    Namespace(GatewayNamespaceTool),
    ToolSearch(GatewayToolSearch),
    Custom(GatewayCustomTool),
    WebSearch(GatewayWebSearchTool),
    ImageGeneration(GatewayImageGenerationTool),
    Unknown(GatewayUnknownTool),
}
```

工具转换原则：

- IR 保留 Responses 原始 tool 结构。
- Provider adapter 根据 capability 决定如何暴露。
- 不支持的工具不能静默过滤，必须记录降级决策。

### 5.2 `tool_search`

`tool_search` 是 Codex dynamic/deferred tools 的关键路径，必须完整支持。

Responses 原生语义：

```json
{
  "type": "tool_search",
  "execution": "client",
  "description": "...",
  "parameters": {
    "type": "object",
    "properties": {
      "query": { "type": "string" },
      "limit": { "type": "number" }
    },
    "required": ["query"],
    "additionalProperties": false
  }
}
```

Chat Completions adapter：

```json
{
  "type": "function",
  "function": {
    "name": "tool_search",
    "description": "...",
    "parameters": { "...": "..." }
  }
}
```

Chat tool call 回解：

```text
assistant tool_call name=tool_search
  -> GatewayItem::ToolSearchCall {
       call_id,
       execution: "client",
       arguments
     }
  -> Responses item type=tool_search_call
```

Codex 执行后下一轮 input：

```text
Responses input item type=tool_search_output
  -> GatewayItem::ToolSearchOutput
  -> provider adapter 可把 output 转成 tool result，同时把 output.tools 注册为下一轮可见 tools
```

关键要求：

- `tool_search_output.tools` 里的 namespace/function 必须被解析进 IR。
- 下一轮发给 Chat/Anthropic 时，adapter 必须把这些 loadable tools 暴露给 provider。
- 如果 provider 支持并行工具调用，`tool_search` 可与普通 function 并行；如果不支持，则按 provider capability 降级。

### 5.3 Namespaced Tools

Responses tool：

```json
{
  "type": "namespace",
  "name": "mcp__node_repl",
  "tools": [
    { "type": "function", "name": "js", "parameters": { "...": "..." } }
  ]
}
```

Chat adapter 出站：

```json
{
  "type": "function",
  "function": {
    "name": "mcp__node_repl__codexns__js",
    "parameters": { "...": "..." }
  }
}
```

回解：

```text
mcp__node_repl__codexns__js
  -> namespace=mcp__node_repl
  -> name=js
  -> GatewayItem::FunctionCall
  -> Responses function_call { namespace, name, call_id, arguments }
```

这套逻辑必须适用于：

- `mcp__*`
- `codex_app`
- `multi_agent_v1`
- 未来所有合法 namespace

不能只允许 `mcp__`。

### 5.4 Custom Tool

Responses `custom` / `custom_tool_call` 是 freeform 工具语义。

Provider adapter 策略：

- 如果 provider 支持 freeform/custom tool，原生映射。
- 如果 provider 只支持 JSON function tool，则用包装 function：

```json
{
  "name": "<custom_tool_name>",
  "parameters": {
    "type": "object",
    "properties": {
      "input": {
        "type": "string",
        "description": "Freeform input for the custom tool."
      }
    },
    "required": ["input"],
    "additionalProperties": false
  }
}
```

回解时：

```text
function call { input: "..." }
  -> GatewayItem::CustomToolCall { input }
```

风险：

- 包装 custom tool 可能改变模型行为。
- 对要求严格 grammar 的 custom tool，Chat provider 不能保证无损。
- adapter 必须记录 `degraded_custom_tool=true`，方便日志定位。

### 5.5 Builtin Tools

包括：

- `web_search`
- `image_generation`
- 其它 Codex/OpenAI Responses builtin

Provider capability 分三类：

```rust
pub enum BuiltinToolSupport {
    Native,
    EmulatedAsFunction,
    Unsupported,
}
```

策略：

- `Native`：按 provider 原生能力发出。
- `EmulatedAsFunction`：包装成普通 function，但必须确保 Codex 能执行对应工具；如果 Codex 不会执行，就不能伪装。
- `Unsupported`：请求中若 tool_choice 强制使用该工具，返回明确错误；如果只是可选工具，可从 provider tools 中隐藏，并记录降级。

不再允许“静默过滤后模型不知道发生了什么”。

## 6. ProviderCapabilities

每个 adapter 必须声明能力：

```rust
pub struct ProviderCapabilities {
    pub protocol: ProviderProtocol,
    pub supports_stream: bool,
    pub supports_system: bool,
    pub supports_developer: bool,
    pub supports_tool_calls: bool,
    pub supports_parallel_tool_calls: bool,
    pub supports_tool_choice_required: bool,
    pub supports_namespaced_tools: bool,
    pub supports_custom_tools: CustomToolSupport,
    pub supports_tool_search: ToolSearchSupport,
    pub supports_images: bool,
    pub supports_reasoning_input: bool,
    pub supports_reasoning_output: bool,
    pub supports_json_schema_response_format: bool,
    pub requires_max_tokens: bool,
}
```

Adapter 只能根据 capability 做转换。任何降级都应该产生 `GatewayTransformNotice`：

```rust
pub struct GatewayTransformNotice {
    pub level: NoticeLevel,
    pub code: String,
    pub message: String,
    pub item_ref: Option<String>,
}
```

日志中要能看到：

- 哪些 tool 暴露给 provider。
- 哪些 tool 被降级或隐藏。
- tool name map。
- provider 不支持哪些请求能力。

## 7. Chat Completions Adapter

适用于 DeepSeek 和其它 OpenAI Chat-compatible provider。

### 7.1 请求映射

Responses / IR 到 Chat：

- `instructions` 和 developer message 合并为 `system` 或 provider 支持的角色。
- user/assistant message 转 `messages[]`。
- image block 转 Chat `content[]` 的 `image_url`。
- `function_call` 转 assistant `tool_calls[]`。
- `function_call_output` 转 `role=tool` message。
- `tool_search_call` 转 assistant `tool_calls[]`，name=`tool_search`。
- `tool_search_output` 转 `role=tool` message，并将其中 `tools` 注册到当前 turn 的 visible tools。
- reasoning 按 provider 能力映射到 `reasoning_content` 或丢入 transform notice。

### 7.2 DeepSeek 特化规则

DeepSeek adapter 可以是 Chat adapter 的 profile：

```rust
ChatCompletionsAdapter {
    profile: ChatProviderProfile::DeepSeek
}
```

保留当前规则：

- `developer` -> `system`
- `json_schema` response format -> `json_object`
- thinking 启用时移除 `temperature`、`top_p`、`presence_penalty`、`frequency_penalty`
- tool_calls assistant message 必须补 `reasoning_content`
- 删除 provider 不接受的空 assistant message

这些规则属于 DeepSeek profile，不应写死在通用 Chat adapter。

### 7.3 响应映射

Chat response：

- `message.content` -> `GatewayItem::Message(role=assistant)`
- `message.reasoning_content` -> `GatewayItem::Reasoning`
- `message.tool_calls[]`：
  - name=`tool_search` -> `GatewayItem::ToolSearchCall`
  - `ToolNameMap` 命中 -> `GatewayItem::FunctionCall` / `CustomToolCall`
  - 未命中 -> `GatewayItem::FunctionCall(namespace=None, name=raw_name)` 并记录 notice

### 7.4 流式映射

不要让 `responses_stream.rs` 只理解 Chat SSE。建议拆成：

```text
Provider native stream
  -> Adapter-specific GatewayEvent stream
  -> ResponsesSseEncoder
```

GatewayEvent：

```rust
pub enum GatewayEvent {
    ResponseStarted,
    OutputItemStarted { item },
    TextDelta { item_id, delta },
    ReasoningDelta { item_id, delta },
    FunctionArgumentsDelta { call_id, tool: ToolKey, delta },
    OutputItemCompleted { item },
    ResponseCompleted { usage },
    Error { error },
}
```

这样 Anthropic Messages streaming 也能复用 Responses SSE encoder。

## 8. Anthropic Messages Adapter

Anthropic Messages 不是 Chat Completions 的简单别名，必须独立 adapter。

### 8.1 请求映射

Anthropic Messages 形态要点：

- `system` 是顶层字段，不是 `messages[]` 中的 role。
- `messages[]` 通常只有 `user` / `assistant`。
- content 是 blocks。
- tool call 是 assistant content block `tool_use`。
- tool result 是 user content block `tool_result`。
- `max_tokens` 通常是必填。

IR 到 Anthropic：

```text
Gateway instructions / system / developer
  -> top-level system string or system blocks

Gateway message(role=user)
  -> messages[] user content blocks

Gateway message(role=assistant)
  -> messages[] assistant content blocks

Gateway FunctionCall / ToolSearchCall / CustomToolCall
  -> assistant content block tool_use

Gateway FunctionCallOutput / ToolSearchOutput / CustomToolCallOutput
  -> user content block tool_result
```

Tool schema：

```json
{
  "name": "mcp__node_repl__codexns__js",
  "description": "...",
  "input_schema": { "...": "..." }
}
```

`tool_search` 同样作为普通 Anthropic tool 暴露：

```json
{
  "name": "tool_search",
  "description": "...",
  "input_schema": {
    "type": "object",
    "properties": {
      "query": { "type": "string" },
      "limit": { "type": "number" }
    },
    "required": ["query"],
    "additionalProperties": false
  }
}
```

### 8.2 响应映射

Anthropic message content blocks：

- `text` -> assistant message text。
- `thinking` 或类似 reasoning block -> reasoning item（具体字段以落地时官方文档为准）。
- `tool_use`:
  - name=`tool_search` -> `GatewayItem::ToolSearchCall`
  - `ToolNameMap` 命中 -> `GatewayItem::FunctionCall` / `CustomToolCall`

### 8.3 流式映射

Anthropic streaming 先转 `GatewayEvent`，再交给 `ResponsesSseEncoder`。

需要覆盖：

- message start -> `ResponseStarted`
- content block start -> `OutputItemStarted`
- text delta -> `TextDelta`
- input_json delta for tool_use -> `FunctionArgumentsDelta`
- content block stop -> `OutputItemCompleted`
- message delta usage -> usage update
- message stop -> `ResponseCompleted`

具体事件名、字段名以实现时官方文档和实测为准。本文只规定 Gateway 内部边界。

## 9. Responses Codec

### 9.1 Inbound Decoder

`responses_inbound` 负责：

- 从 raw JSON 解析 `GatewayTurn`。
- 对未知字段保留 `raw`。
- 对 `input` 支持 string、array、null。
- 对 `arguments` / `input` 支持 string 或 object，object 序列化为 JSON string 或保留 Value。
- 对 image_url 支持 string 或 object。
- 对 `tool_search_call` / `tool_search_output` 完整解析。
- 对 `namespace` tools 完整解析。

OpenAI Responses provider 可以继续 raw passthrough，但为了日志、ledger 和跨 provider 切换，建议同时 best-effort decode。

### 9.2 Outbound Encoder

`responses_outbound` 负责：

- 把 `GatewayTurnDelta` / `GatewayEvent` 编回 Codex 期望的 Responses JSON/SSE。
- 确保 event 顺序稳定。
- 确保 output item index、content index、sequence number 正确。
- 非流式响应也从同一个 IR encoder 生成，避免非流式和流式行为分叉。

## 10. Ledger 和 Adapter State

完备转换需要两类状态。

### 10.1 Request-scoped state

```rust
pub struct AdapterRequestState {
    pub tool_name_map: ToolNameMap,
    pub visible_tools: Vec<GatewayTool>,
    pub transform_notices: Vec<GatewayTransformNotice>,
}
```

用于：

- tool call 回解。
- streaming tool delta 归属。
- 记录降级。

### 10.2 Session/turn ledger

```rust
pub struct GatewayResponseRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub requested_model: String,
    pub provider_name: String,
    pub provider_type: ProviderType,
    pub upstream_response_id: Option<String>,
    pub input_items: Vec<Value>,
    pub output_items: Vec<Value>,
    pub canonical_ir_hash: String,
    pub tool_name_map: Option<Value>,
}
```

用于：

- WebSocket / `previous_response_id` delta input 还原。
- 跨 provider 切换时避免泄漏上游 response id。
- 解释 cache prefix 是否稳定。
- 复盘 tool_search 和 dynamic tool 暴露链路。

第一步可以只实现 request-scoped state；ledger 可第二步落库。

## 11. 错误与降级策略

所有无法无损转换的情况分四类：

```rust
pub enum TransformDecision {
    Native,
    LosslessEmulation,
    LossyDegradation,
    Unsupported,
}
```

行为：

- `Native`：provider 原生支持。
- `LosslessEmulation`：语义可保真，例如 namespace flatten + ToolNameMap。
- `LossyDegradation`：语义可能变弱，例如 custom freeform 包装成 `{ input: string }`。
- `Unsupported`：不能安全执行，必须报错或隐藏可选能力。

错误策略：

- 强制 tool_choice 指向 unsupported tool：返回 400 Responses error。
- 可选 tool unsupported：隐藏该 tool，记录 notice。
- input history 包含无法转换的 required output：返回 400，避免模型上下文断裂。
- unknown item：默认保留 raw；adapter 不需要时可忽略，但必须记录。

## 12. 测试策略

### 12.1 IR decode/encode

- Responses request 包含所有已知 item，decode 不丢字段。
- 未知 item 进入 `Unknown(raw)`。
- object arguments、string arguments 都能解析。
- image_url string/object 都能解析。

### 12.2 ToolNameMap

- `mcp__node_repl/js` roundtrip。
- `codex_app/read_thread_terminal` roundtrip。
- `multi_agent_v1/spawn_agent` roundtrip。
- namespace/name 冲突时 hash 后仍能回解。
- 兼容旧 `responses_unit__` 输入。

### 12.3 Tool Search

完整链路测试：

```text
Responses tools include tool_search
  -> Chat tools include function tool_search
  -> provider returns tool_search call
  -> Gateway emits Responses tool_search_call
  -> next input contains tool_search_output with namespace tools
  -> next provider request exposes loaded tools
  -> provider calls loaded namespaced tool
  -> Gateway restores namespace/name
```

这条测试是 Chrome/Browser/Apps 插件可用性的核心验收。

### 12.4 Chat/DeepSeek

- DeepSeek thinking 参数清理。
- tool_calls assistant message 补 reasoning_content。
- function_call_output 转 tool message。
- tool_search_output 转 tool message + visible tools。
- stream tool call delta 组装多个并行 tool calls。

### 12.5 Anthropic

- system 顶层映射。
- text/image content block。
- function call -> tool_use。
- function output -> tool_result。
- tool_search -> tool_use -> Responses tool_search_call。
- streaming input_json delta -> function arguments delta。

### 12.6 Regression

- OpenAI Responses raw passthrough body 不被破坏。
- current request log 仍记录 request/upstream/response。
- 旧 DeepSeek 普通文本对话不回归。
- 旧 DeepSeek 普通 function tool call 不回归。

## 13. 建议实现顺序

### 当前落地状态（2026-06-19）

本轮已经落地：

- 新增 `src/ai_gateway/tool_names.rs`，`ToolNameMap` 成为 request-scoped 工具名映射。
- 工具名编码现在会做 provider-safe 处理：只保留 ASCII 字母、数字、`_`、`-`，限制 64 字符以内，冲突或超长时追加短 hash，同时保留反向映射。
- 新编码使用 `<namespace>__codexns__<name>`，继续兼容旧 `responses_unit__` 回解。
- DeepSeek / ChatCompletions 出站请求、非流式响应、流式响应共享同一个 `ToolNameMap`。
- Chat adapter 已支持：
  - 任意 Responses namespace function tool 回解，不再只认 `mcp__`。
  - `tool_search` tool / `tool_search_call` / `tool_search_output`。
  - `tool_search_output.tools` 暴露为下一轮 visible tools。
  - `custom` tool 包装为 `{input: string}` function tool。
  - 历史 `custom_tool_call` / `custom_tool_call_output` 回放到 Chat messages。
  - `tool_choice` 的 namespaced function / `tool_search` / `custom` 编码。
- 新增 `src/ai_gateway/ir.rs` 和 `src/ai_gateway/codec/responses_inbound.rs`。
- Handler 已对所有 `/v1/responses` 请求做 best-effort Responses inbound decode；OpenAI Responses provider 仍 raw passthrough，ChatCompletions 继续使用现有转换链路。
- `GatewayRequest` 解析支持 `function_call.arguments` / `tool_search_call.arguments` 为 string 或 object。
- Anthropic Messages 独立 adapter 设计已拆到专门文档，明确不复用 ChatCompletions adapter。

本轮验证：

- `cargo test ai_gateway --features gui`：138 passed。
- `cargo check --features gui`：通过。

### Step 1：新增 IR，不改路由行为

- 新增 `ir.rs`。
- 新增 Responses inbound decoder。
- 对当前日志请求做 best-effort decode，不影响 OpenAI passthrough。
- 补 decode 单测。

### Step 2：迁移 Chat adapter 到 IR

- 保留现有 DeepSeek 行为。
- 把 `GatewayRequest` -> `build_chat_request` 改成 `GatewayTurn` -> `ChatRequest`。
- 引入 `ToolNameMap`。
- 修复 non-`mcp__` namespace 回解。

### Step 3：补 `tool_search`

- IR 支持 `ToolSearchCall` / `ToolSearchOutput`。
- Chat adapter 暴露 `tool_search`。
- Chat response 回解 `tool_search_call`。
- `tool_search_output.tools` 注册为下一轮 visible tools。
- 完整链路单测。

### Step 4：统一 stream encoder

- 把 `responses_stream.rs` 拆成 Chat native stream parser + Responses SSE encoder。
- GatewayEvent 成为中间事件。
- 现有 Chat SSE 测试全部迁移。

### Step 5：Anthropic Messages adapter

- 配置新增 provider type。
- request/response/stream 三条路径实现。
- 先覆盖 text + tools + tool_search。
- reasoning 和高级 blocks 按官方文档实测逐步补齐。

当前落地状态：

- 已新增独立 `anthropic_messages` provider type 和 handler 分发。
- 已覆盖非流式 text、function/custom/tool_search 主链路、tool_choice、tool_result。
- 已覆盖基础 streaming text 和 `tool_use.input_json_delta` 到 Responses SSE。
- 暂未抽取统一 `GatewayEvent -> ResponsesSseEncoder`，Anthropic 目前有 provider-local stream adapter。

### Step 6：Ledger

- 写入 gateway-owned response record。
- 支持 previous_response_id 全量还原。
- 为 WebSocket 做前置。

## 14. 当前代码差距清单

当前已知差距：

- `src/ai_gateway/ir.rs` 已存在，但 Chat adapter 还没有完全迁移到 IR，当前 DeepSeek 仍主要走 `GatewayRequest` + transform 模块。
- `responses_stream.rs` 当前是 Chat SSE 到 Responses SSE 的直接状态机；Anthropic 已用 provider-local adapter 先落地，后续仍应抽公共 encoder。
- DeepSeek profile 规则写在 Chat 转换函数中，后续应拆成 profile。
- builtin tools 当前仍按策略过滤/隐藏，还没有完整 `GatewayTransformNotice`。
- `GatewayTransformNotice` / provider capability / adapter state 尚未落库或暴露到 request log。
- Ledger / `previous_response_id` 全量还原尚未实现。
- Anthropic thinking/cache/高级 content blocks 尚未实现。

已解决的旧差距：

- `tool_search_call` / `tool_search_output` 已进入 `ItemType` 和 Chat 转换链路。
- `tool_search_output.tools` 已解析并暴露到 Chat provider tools。
- namespace 回解不再只认 `mcp__`。
- `custom` tool 已能在 Chat provider 下包装降级，并能回放历史 call/output。
- provider 工具名已做 64 字符和字符集约束，支持冲突 hash。

## 15. 决策记录

当前决策：

- 2026-07-25：第三方 tool_search 采用小范围补洞，不推进完整统一 IR 重构。DeepSeek 重新开启
  supports_search_tool，复用已完成的 ChatCompletions 双向转换；Grok 在 ProviderType::GrokResponses
  内把 Codex 原生 tool_search 双向翻译为普通 function。OpenAI Responses 原生透传、Responses Lite
  新字段、hosted web_search 和客户端 web.run 都不纳入本次改动。详细方案见
  docs/ai-gateway-third-party-tool-search-plan.zh-CN.md。

- Codex Responses 是入口 wire protocol，不是 Gateway 内部唯一模型。
- Gateway IR 必须优先保真，adapter 再降级。
- `tool_search` 是一等语义，不作为普通 function 的特殊字符串临时处理。
- namespace flatten 必须由 `ToolNameMap` 管理，不能靠硬编码前缀。
- Anthropic Messages 必须独立 adapter，不复用 ChatCompletions adapter。
- OpenAI Responses raw passthrough 继续保留，避免无谓风险。
- Anthropic 工具名约束按官方文档实现：`^[a-zA-Z0-9_-]{1,64}$`，因此所有 provider native tool name 都必须走 `ToolNameMap`。
- Anthropic streaming 的 `tool_use.input` delta 是 `input_json_delta.partial_json`，最终 `tool_use.input` 是 object；实现时必须累积 partial JSON 到 `content_block_stop` 再解析。

待实现中复核：

- Anthropic Messages adapter 的完整字段：`system`、`messages[]`、`tools[]`、`tool_choice`、`max_tokens`、`thinking`、`tool_result`。
- Anthropic Messages request headers：`x-api-key`、`anthropic-version`、必要 beta headers。
- Anthropic thinking / signature delta 如何映射到 Responses `reasoning` item。
- DeepSeek 对 tool name 字符集和最大长度的实际限制；当前先按 Anthropic 严格约束收敛。
- Codex 当前 Responses SSE 对 `tool_search_call` 的精确事件序列要求。
- `tool_search_output.tools` 是否需要在同一 request 内影响可见工具，还是只影响下一轮 request；以 Codex core 实测为准。
- `custom` freeform tool 在 Anthropic 下是否可用 native/custom 语义，还是沿用 `{input: string}` 包装。

## 16. 本轮实现笔记

代码文件：

- `src/ai_gateway/tool_names.rs`
- `src/ai_gateway/ir.rs`
- `src/ai_gateway/codec/responses_inbound.rs`
- `src/ai_gateway/model.rs`
- `src/ai_gateway/handler.rs`
- `src/ai_gateway/providers/deepseek_chat.rs`
- `src/ai_gateway/transform/responses_to_chat.rs`
- `src/ai_gateway/transform/chat_to_responses.rs`
- `src/ai_gateway/transform/responses_stream.rs`

外部协议确认：

- Anthropic Define tools：client tools 放在顶层 `tools` 参数；每个 tool 至少包含 `name`、`description`、`input_schema`，其中 `name` 必须匹配 `^[a-zA-Z0-9_-]{1,64}$`。
- Anthropic Streaming：stream 事件序列为 `message_start`、若干 `content_block_start` / `content_block_delta` / `content_block_stop`、`message_delta`、`message_stop`；`tool_use` 参数流式 delta 为 `input_json_delta.partial_json`。

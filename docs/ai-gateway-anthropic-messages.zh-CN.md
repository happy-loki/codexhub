# AI Gateway Anthropic Messages Adapter 设计

更新时间：2026-06-19

状态：设计草案，可随实现和实测修订。

本文档定义 `codexhub` AI Gateway 对接 Anthropic Messages API 的独立 adapter 方案。Anthropic Messages 是一等 provider 协议，不作为 Chat Completions 的变体实现。

参考资料：

- Anthropic Messages API Reference: <https://docs.anthropic.com/en/api/messages>
- Anthropic Streaming Messages: <https://docs.anthropic.com/en/api/messages-streaming>
- Anthropic Tool Use: <https://docs.anthropic.com/en/docs/build-with-claude/tool-use/overview>
- Anthropic Define Tools: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/implement-tool-use>
- Anthropic Fine-grained Tool Streaming: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/fine-grained-tool-streaming>
- Anthropic Prompt Caching: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching>
- [`ai-gateway-web-search-protocol.zh-CN.md`](ai-gateway-web-search-protocol.zh-CN.md)：Codex Responses `web_search` 与 Anthropic web search server tool 的详细对接规则。
- [`ai-gateway-provider-private-state-conversion.zh-CN.md`](ai-gateway-provider-private-state-conversion.zh-CN.md)：thinking signature、redacted data、typed marker 和跨 Provider 回放规则。

## 1. 目标

目标链路：

```text
Codex Responses
  -> AI Gateway Responses inbound decode
  -> Gateway IR
  -> Anthropic Messages adapter
  -> POST /v1/messages
  -> Anthropic Message / SSE
  -> Gateway IR / GatewayEvent
  -> Responses JSON / SSE
  -> Codex
```

核心原则：

- `anthropic_messages` 是独立 `ProviderType`。
- 不复用 `chat_completions` provider，也不把 Anthropic 强行伪装成 OpenAI Chat。
- OpenAI Responses 入口协议不变，Codex 不需要知道后端是 Anthropic。
- 工具执行仍由 Codex 处理；Gateway 只做协议映射。
- 对无法无损转换的能力显式记录、降级或拒绝，不静默丢字段。

## 2. Provider 配置

新增 provider type：

```rust
pub enum ProviderType {
    OpenAiResponses,
    ChatCompletions,
    AnthropicMessages,
}
```

配置示例：

```toml
[[aiGateway.providers]]
name = "anthropic"
enabled = true
providerType = "anthropic_messages"
baseUrl = "https://api.anthropic.com/v1"
apiKey = "..."
models = ["claude-opus-4-8", "claude-sonnet-4-6", "claude-haiku-4-5"]
timeoutSecs = 600
```

请求头：

- `authorization: Bearer <provider.api_key>`
- `anthropic-version: 2023-06-01`
- `anthropic-beta: <Claude Code beta flags>`
- `x-app: cli`
- `user-agent: claude-cli/2.1.185 (external, cli)`
- `content-type: application/json`
- `accept: application/json`

Anthropic / GLM Anthropic-compatible profile 的上游 header 由 adapter 托管，按 Claude Code
参考请求生成，不透传 Codex 入站 header。

## 3. 请求映射

Anthropic Messages 请求关键字段：

```json
{
  "model": "claude-sonnet-4-6",
  "max_tokens": 4096,
  "system": "...",
  "messages": [
    {"role": "user", "content": [{"type": "text", "text": "..."}]}
  ],
  "tools": [],
  "tool_choice": {"type": "auto"},
  "stream": true
}
```

### 3.1 顶层字段

Responses 到 Anthropic：

| Responses / Gateway | Anthropic |
| --- | --- |
| `model` | `model` |
| `instructions` / system/developer 前缀 | top-level `system` |
| `max_output_tokens` | `max_tokens` |
| `stream` | `stream` |
| `temperature` | `temperature`，仅 provider 支持时透传 |
| `top_p` | `top_p`，仅 provider 支持时透传 |
| `metadata` | 暂不透传，后续按 Anthropic metadata 规范补 |

`max_tokens` 在 Anthropic Messages 中是常规必填项。若 Codex 请求没有给 `max_output_tokens`，adapter 需要使用模型配置默认值，例如 `4096`，并在 request log 记录默认值来源。

### 3.2 Messages

Gateway message 到 Anthropic content blocks：

| Gateway item | Anthropic message |
| --- | --- |
| `message(role=user)` + text | `role=user`, `content=[{type:"text", text}]` |
| `message(role=assistant)` + output_text | `role=assistant`, `content=[{type:"text", text}]` |
| `input_image` | Anthropic image block，按官方支持的 source 形态转换 |
| `reasoning` | 第一阶段不回放为 visible text；保留到后续 thinking/signature 实测 |

连续同 role 文本可合并，但必须保持 tool call / tool result 的相对顺序。

### 3.3 System

Anthropic 的 `system` 是顶层字段，不是 `messages[]` 中的 `role=system`。

合并顺序：

```text
GatewayRequest.instructions
  + input 中可能出现的 developer/system 语义文本
  -> Anthropic top-level system
```

Anthropic provider 会在出站适配层模拟 Claude Code 的 block-level prompt caching：把非空 `system`
文本转成 `system[]` text block，并在 system 段最后一个 text block 上附 `cache_control`。断点读写机制
详见 [`ai-gateway-anthropic-cache-control.zh-CN.md`](ai-gateway-anthropic-cache-control.zh-CN.md)。

## 4. Tool 映射

Anthropic client tool 形态：

```json
{
  "name": "mcp__node_repl__codexns__js",
  "description": "...",
  "input_schema": {
    "type": "object",
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "properties": {},
    "additionalProperties": false
  }
}
```

### 4.1 工具名

所有 Anthropic tool name 必须通过 `ToolNameMap`：

- 保证字符集和长度满足 provider 限制。
- 保留 namespace/name 回解。
- 处理冲突和超长 hash。

不能直接把 Responses namespace 拼成原始字符串发给 Anthropic。

### 4.2 Function Tool

Responses function tool：

```json
{
  "type": "function",
  "name": "read_file",
  "namespace": "codex_app",
  "parameters": {}
}
```

映射为 Anthropic tool：

```json
{
  "name": "<encoded>",
  "description": "...",
  "input_schema": {
    "type": "object",
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "properties": {},
    "additionalProperties": false
  }
}
```

Anthropic 返回 `tool_use`：

```json
{
  "type": "tool_use",
  "id": "toolu_...",
  "name": "<encoded>",
  "input": {}
}
```

映射回 Responses：

```json
{
  "type": "function_call",
  "id": "fc_...",
  "call_id": "toolu_...",
  "namespace": "codex_app",
  "name": "read_file",
  "arguments": "{\"...\":...}",
  "status": "completed"
}
```

### 4.3 Tool Result

Responses `function_call_output` / `custom_tool_call_output` / `tool_search_output` 在下一轮 Anthropic 请求中映射为 `role=user` 的 `tool_result` content block：

```json
{
  "role": "user",
  "content": [
    {
      "type": "tool_result",
      "tool_use_id": "toolu_...",
      "content": "..."
    }
  ]
}
```

若 output 是结构化 content items：

- 文本继续放在 `tool_result.content` 中。
- `view_image`、MCP 截图等 `input_image` 不嵌入 `tool_result.content`，而是提升为同一条 `role=user` 消息中位于所有 `tool_result` 后面的并列 `image` block。
- 纯图片结果在 `tool_result.content` 中使用短占位文本 `Image output attached below.`，继续保留 `tool_use_id` 配对。
- 多个并行工具结果合并为一条 user message，并保持“全部 tool_result 在前、全部 image 在后”的顺序。

提升后的结构仍是标准 Anthropic Messages。这样既保留工具调用配对，也兼容内部会再次转换为 Responses 的上游，避免部分节点把嵌套图片的 base64 当作普通文本估算 token。

### 4.4 Tool Search

`tool_search` 是 Codex/Gateway 的一等语义，在 Anthropic 下作为普通 client tool 暴露：

```json
{
  "name": "tool_search",
  "description": "Search available tools by query.",
  "input_schema": {
    "type": "object",
    "properties": {
      "query": {"type": "string"},
      "limit": {"type": "integer"}
    },
    "required": ["query"],
    "additionalProperties": false
  }
}
```

Anthropic `tool_use(name="tool_search")` 映射回 Responses `tool_search_call`，不映射成普通 `function_call`。

### 4.5 Custom Tool

Responses custom/freeform tool 与 Anthropic structured `tool_use.input` 存在语义差异。

第一阶段策略：

- 如果 Anthropic native 能力可安全表达 freeform custom tool，再走 native。
- 否则沿用当前 Chat 降级策略，把 custom input 包成 `{ "input": string }`。
- 映射回 Responses 时必须恢复为 `custom_tool_call.input` 裸字符串。
- request log 记录 `LossyDegradation(custom_tool_wrapped_input)`。

## 5. Tool Choice

映射建议：

| Responses tool_choice | Anthropic tool_choice |
| --- | --- |
| `"auto"` / missing | `{"type":"auto"}` 或省略 |
| `"none"` | `{"type":"none"}`，以官方支持为准 |
| required / any | `{"type":"any"}` |
| named function/custom/tool_search | `{"type":"tool","name":"<encoded>"}` |

如果指定工具无法映射，返回 400 Responses error，不要静默改成 auto。

## 6. 非流式响应映射

Anthropic Message response：

```json
{
  "id": "msg_...",
  "type": "message",
  "role": "assistant",
  "content": [
    {"type": "text", "text": "..."},
    {"type": "tool_use", "id": "toolu_...", "name": "...", "input": {}}
  ],
  "model": "...",
  "stop_reason": "tool_use",
  "usage": {
    "input_tokens": 10,
    "output_tokens": 20
  }
}
```

Responses response：

```json
{
  "id": "msg_...",
  "object": "response",
  "model": "...",
  "status": "completed",
  "output": [
    {"type": "message", "role": "assistant", "content": [...]},
    {"type": "function_call", "...": "..."}
  ],
  "usage": {
    "input_tokens": 10,
    "output_tokens": 20,
    "total_tokens": 30
  }
}
```

`stop_reason` 映射：

| Anthropic stop_reason | Responses status / meaning |
| --- | --- |
| `end_turn` | completed |
| `tool_use` | completed with tool call output items |
| `max_tokens` | incomplete |
| `stop_sequence` | completed |
| unknown | completed，记录 notice |

## 7. 流式响应映射

Anthropic streaming 是 SSE，但 event 不是 Responses 事件。必须由 Anthropic stream parser 转成 GatewayEvent，再由 ResponsesSseEncoder 生成 Responses SSE。

> GLM profile 的私有网搜文本过滤与逐 token 流式（`split_streamable` 增量切割）详见
> [`ai-gateway-anthropic-streaming-glm-buffering.zh-CN.md`](ai-gateway-anthropic-streaming-glm-buffering.zh-CN.md)。

Anthropic 事件序列：

```text
message_start
content_block_start
content_block_delta
content_block_stop
message_delta
message_stop
ping
error
```

### 7.1 Text

Anthropic：

```text
content_block_start: {"type":"text","text":""}
content_block_delta: {"type":"text_delta","text":"hello"}
content_block_stop
```

Responses：

```text
response.output_item.added(message)
response.content_part.added(output_text)
response.output_text.delta
response.output_text.done
response.content_part.done
response.output_item.done
```

### 7.2 Tool Use

Anthropic：

```text
content_block_start: {"type":"tool_use","id":"toolu_...","name":"...","input":{}}
content_block_delta: {"type":"input_json_delta","partial_json":"{\"path\""}
content_block_delta: {"type":"input_json_delta","partial_json":":\"Cargo.toml\"}"}
content_block_stop
```

处理规则：

- 按 content block index 维护 active tool state。
- 累积 `partial_json`。
- 对 function/tool_search，在 Responses SSE 中可发 `function_call_arguments.delta`，delta 是原始 partial JSON。
- 对 custom tool，如果采用 `{input:string}` 包装，需要和 DeepSeek 当前逻辑一样只输出裸 input delta。
- `content_block_stop` 时解析完整 JSON，生成 `function_call_arguments.done` 或 `custom_tool_call_input.done`，再发 `response.output_item.done`。

### 7.3 Usage

Anthropic usage 可能在 `message_start` 和 `message_delta` 中出现。adapter 需要累积：

```text
input_tokens
output_tokens
cache_creation_input_tokens
cache_read_input_tokens
```

Responses usage：

```json
{
  "input_tokens": 10,
  "output_tokens": 20,
  "total_tokens": 30,
  "input_tokens_details": {
    "cached_tokens": 5
  },
  "output_tokens_details": {
    "reasoning_tokens": 0
  }
}
```

### 7.4 Ping / Error

- `ping` 不转发给 Codex Responses，可用于保持连接活跃或忽略。
- Anthropic `error` 转成 Responses SSE `error` 或 `response.failed`，具体以 Codex 当前消费能力实测为准。
- 上游流异常但未收到 `message_stop` 时，请求日志标记 failed；客户端断开时标记 cancelled。

## 8. Thinking / Reasoning

Anthropic extended thinking 不能简单映射成普通 text。

第一阶段：

- 不主动开启 thinking。
- 若响应里出现 thinking block，保留为 Responses `reasoning` item。
- signature / encrypted thinking 字段先保留 raw，避免破坏后续多轮上下文。

第二阶段：

- 实测 `thinking_delta`、`signature_delta`。
- 映射到 Responses reasoning summary 或 encrypted_content。
- 加多轮工具调用回放测试。

## 9. Prompt Cache

OpenAI 的 `prompt_cache_key` 不能直接映射成 Anthropic cache；它只用于 OpenAI
Responses 侧的缓存分桶。

Anthropic Messages 使用 block-level `cache_control` 开启 prompt caching。Gateway 策略（读写机制与实证见
[`ai-gateway-anthropic-cache-control.zh-CN.md`](ai-gateway-anthropic-cache-control.zh-CN.md)）：

- Codex / Responses 入站不需要、也不应携带 Anthropic 专属 `cache_control`。
- Anthropic provider 在出站适配层先剥离工具自带的缓存标记，再给 `tools` 数组的**最后一个工具定义**附一个 `cache_control: {"type":"ephemeral"}`，缓存完整工具前缀。
- `system` 段只在**最后一个** text block 附一个同类断点，靠前缀回溯覆盖其余 system block。
- 多轮历史只给会话尾部最后一条非空 `role==user`/`assistant` 消息附一个同类断点；优先落在最后一个 text block，无 text 时落在最后一个 content block，因此 tool_result-only 与 tool_use-only 消息也能覆盖。
- 常规请求共使用 tools、system、messages 三个断点，保持在 Anthropic 的 4 个断点上限以内。
- 不生成顶层 `cache_control`，不生成 `ttl`，也不把 `prompt_cache_retention = "1h"` 映射为 Anthropic `ttl`。
- request log 记录 Anthropic cache read/create token。

## 10. 实现计划

### Phase A：骨架 ✅

- `ProviderType::AnthropicMessages`
- `providers/anthropic_messages.rs`
- handler 分发
- provider config 解析和 route key
- 单测：provider type 反序列化、路由选择

### Phase B：非流式 text ✅

- Responses message/system -> Anthropic messages/system
- Anthropic text response -> Responses message
- usage 映射
- 单测：普通问答闭环

### Phase C：非流式 tools ✅

- Responses tools -> Anthropic tools
- function_call/tool_use 双向映射
- function_call_output/tool_result 映射
- tool_search 映射
- web_search / web_search_preview -> Anthropic server tool `web_search_20250305`
- Anthropic `server_tool_use(name=web_search)` / `web_search_tool_result` -> Responses `web_search_call`
- custom tool 包装降级
- 单测：function/tool_choice/tool_result/tool_use/web_search 主链路

### Phase D：统一 Responses SSE encoder（待抽取）

- 拆 `responses_stream.rs`
- 新增 GatewayEvent
- 新增 ResponsesSseEncoder
- Chat/DeepSeek 迁移到 encoder，保持当前测试全绿

### Phase E：Anthropic streaming 基础链路 ✅

- Anthropic SSE parser
- text delta
- tool_use input_json_delta
- server_tool_use / web_search_tool_result
- usage delta
- 单测：text stream、tool stream、web_search stream

待补：

- 统一 GatewayEvent/ResponsesSseEncoder 后复用公共 encoder。
- error/ping 细节映射。
- 并行 content blocks 与 thinking 交织场景。

### Phase F：thinking/cache/高级 blocks

- thinking/signature
- image/PDF/source blocks
- prompt cache config ✅
- fine-grained tool streaming beta

## 11. 验收测试

必须覆盖：

- `Responses -> Anthropic` 普通文本请求。
- top-level `system` 映射。
- user/assistant 多轮 history 顺序。
- function tool call + tool_result 下一轮。
- `tool_search_call` 和 `tool_search_output.tools`。
- custom tool `{input:string}` 降级与恢复。
- `web_search` / `web_search_preview` 映射为 Anthropic web search server tool。
- Anthropic web search 回包映射为 Responses `web_search_call`。
- streaming text 输出事件序列。
- streaming `tool_use.input_json_delta` 到 Responses tool call events。
- streaming `server_tool_use` / `web_search_tool_result` 到 Responses `web_search_call` events。
- usage/cache token 映射。
- Anthropic 非 2xx 错误映射。
- 客户端 cancel 后 request log 标记 cancelled。

## 12. 开放问题

- Anthropic 当前最新模型的 `max_tokens` 上限需要从模型 catalog 或配置读取，不能硬编码一个全局值。
- `tool_choice none`、`any`、`tool` 的细节需要按当前官方文档和实测确认。
- fine-grained tool streaming 是否默认开启，还是只对 custom/freeform 工具开启。
- thinking block 是否应该默认暴露给 Codex UI，还是只作为 reasoning raw 保留。
- prompt caching 已按 Claude Code 样例使用 Anthropic block-level `cache_control`；OpenAI `prompt_cache_key` 不映射到 Anthropic。

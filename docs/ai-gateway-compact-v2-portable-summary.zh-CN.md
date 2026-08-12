# AI Gateway Compact V2 可移植摘要设计

日期：2026-07-14

状态：备选方案，暂不实施。本文档保留 Compact V2 截获、统一摘要和 portable marker 的完整设计，供未来跨 Provider 压缩状态丢失成为必须解决的问题时重新评估。

当前决定（2026-07-14）：

- 当前主路径继续使用 hosted `web_search`，暂不启用原生 `web.run`。
- Codex Provider 配置保持 `name = "ai-gateway"`，不伪装成 OpenAI Provider。
- 因为 `provider.is_openai() == false`，Codex 不启用 Remote Compact V2，而是使用本地文本摘要压缩。
- 本地摘要由旧模型生成，并以普通 `role=user` 摘要进入新历史，天然可供 OpenAI、Grok、DeepSeek 和 Anthropic 读取。
- 不实现 Compact V2 截获和 synthetic compaction SSE。
- 不增加 `codexhub:compact:v1:` portable marker。
- 不实现 Gateway 内部摘要子请求和后续 marker 展开。
- 不把本文方案作为当前 release 的前置工作。
- 内置可见模型已经按协议族补齐 `comp_hash`；在当前主路径下，hash 触发的是本地文本摘要，不需要迁移 V2 blob。
- 用户自定义模型映射不参与本次 hash 推导，继续使用被映射的 Codex 模型条目自身 metadata。
- 现有 Provider scope 规则继续处理 reasoning 私有状态和旧会话遗留的原生 compaction blob。

相关文档：

- [`ai-gateway-provider-private-state-conversion.zh-CN.md`](ai-gateway-provider-private-state-conversion.zh-CN.md)：Provider 私有 reasoning、签名和密文的作用域隔离。
- [`ai-gateway-encrypted-content-scope.zh-CN.md`](ai-gateway-encrypted-content-scope.zh-CN.md)：Responses 原生密文透传、Anthropic typed marker 和旧前缀迁移规则。
- [`ai-gateway-responses-lite-web-search.zh-CN.md`](ai-gateway-responses-lite-web-search.zh-CN.md)：Responses Lite、`web.run` 和 OpenAI Provider 身份约束。
- [OpenAI Compaction 指南](https://developers.openai.com/api/docs/guides/compaction)：OpenAI 官方 compaction item 和 opaque `encrypted_content` 说明。
- [OpenAI Compact API Reference](https://developers.openai.com/api/reference/resources/responses/methods/compact/)：standalone compact 请求与响应字段。

## 1. 背景

CodexHub 对 Codex 暴露一个 OpenAI Responses Provider，但内部可以把不同模型路由到：

- OpenAI Responses
- Grok Responses
- DeepSeek Chat Completions
- Anthropic Messages 兼容服务，包括 Claude、GLM 等

Codex 的 Compact V2 适合在 OpenAI 模型内部保存长会话状态，但它返回的 `compaction.encrypted_content` 是只能由兼容 OpenAI 模型继续使用的不透明内容。它不是“发生过压缩”的空标记，也没有可供其他 Provider 读取的明文摘要字段。

如果用户在同一会话中从 OpenAI 切换到 Grok、DeepSeek 或 Anthropic，直接过滤 OpenAI compaction blob 虽然能避免协议错误，但会同时丢失 blob 承载的旧 assistant 状态、工具进度和任务结论。

本文的目标是在保留 Codex Compact V2 生命周期和 `web.run` 能力的前提下，让所有压缩结果都能跨 Provider 使用。

## 2. 已核实事实

### 2.1 Compact V2 的压缩内容确实位于密文中

Codex 当前协议结构：

```text
ResponseItem::Compaction {
    id: Option<ResponseItemId>,
    encrypted_content: String,
}
```

它没有明文 `summary` 字段。OpenAI 官方也把该字段定义为 encrypted、opaque、不可由客户端解释的压缩状态。

准确描述不是“整个新窗口都是密文”，而是：

```text
压缩后的新窗口
= 最近保留的可读消息
+ 一个承载旧压缩状态的 opaque compaction blob
```

Codex V2 当前会从原请求中保留最近一部分消息，文本预算约为 64K tokens，再追加一个 `compaction` item。经过 Codex 自己的历史过滤和上下文重新注入后，真正被压缩掉的 assistant、工具和执行状态主要依赖 `encrypted_content`。

### 2.2 Gateway 在压缩时不知道目标模型

模型切换前，Codex 使用旧模型执行 pre-sampling compaction。Gateway 收到的请求包含：

- 请求体中的旧 `model`
- 旧历史
- `input` 末尾的 `{"type":"compaction_trigger"}`
- `x-codex-turn-metadata` 中的 `request_kind=compaction`
- `compaction.reason`

当前元数据只有：

```json
{
  "trigger": "auto",
  "reason": "comp_hash_changed",
  "implementation": "responses_compaction_v2",
  "phase": "pre_turn",
  "strategy": "memento"
}
```

它不包含：

- `target_model`
- `target_provider`
- `target_comp_hash`

Codex 还会从压缩请求中移除本次新用户消息和 `<model_switch>`。因此以下两种切换在 Gateway 看来可能完全一样：

```text
GPT-5.6 -> GPT-5.5
GPT-5.6 -> Grok-4.6
```

Gateway 只能看到旧模型 `gpt-5.6` 和 `reason=comp_hash_changed`，无法无状态地判断目标是否仍为 OpenAI。

### 2.3 若启用 `web.run`，需要 OpenAI Provider 身份

原生 `web.run` 是本文备选方案成立的前提。若未来启用它，CodexHub 的 Provider 配置键和 Provider 能力名称是两个概念：

```toml
model_provider = "ai-gateway"

[model_providers.ai-gateway]
name = "OpenAI"
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
wire_api = "responses"
requires_openai_auth = true
```

- `ai-gateway` 是 Codex 配置中的 Provider ID。
- `name = "OpenAI"` 用于通过 Codex 的 OpenAI capability gate。
- 把 `name` 改成 `ai-gateway` 会关闭原生 `web.run` 等依赖 OpenAI 身份的能力。

当前主路径不启用 `web.run`，所以继续使用 `name = "ai-gateway"` 和 hosted `web_search`，并直接获得 Codex 本地摘要压缩。只有未来重新启用 `web.run` 时，才需要面对 OpenAI Provider 身份与 Remote Compact V2 的耦合。

### 2.4 启用本备选方案时 Compact V2 必须保持开启

如果未来启用本文的 portable V2 备选方案，则保持：

```toml
[features]
remote_compaction_v2 = true
```

该功能当前默认开启，可以不显式写入配置。CodexHub 不使用旧 `/responses/compact` 作为主要实现，也不依赖 Compact V1。

## 3. 备选方案设计

### 3.1 所有 Compact V2 请求统一生成文本摘要

Gateway 不再尝试区分“同 Provider 压缩”和“跨 Provider 压缩”。所有检测到的 Compact V2 请求都执行同一流程：

```text
任何 compaction_trigger
  -> 使用请求中的旧模型生成文本交接摘要
  -> 将文本封装成 Codex 可接受的 V2 compaction item
  -> Codex 保存该 item
  -> 后续请求中 Gateway 将其展开成普通 user 摘要消息
```

这意味着实现完成后：

- OpenAI 普通上下文超限也不再使用原生 OpenAI compaction blob。
- OpenAI -> OpenAI、OpenAI -> Grok、Grok -> OpenAI 等路径行为完全一致。
- Gateway 不需要知道目标模型。
- V2 只承担 Codex 与 Gateway 之间的压缩请求和生命周期协议。
- 真正的压缩语义由 CodexHub 文本摘要实现。

### 3.2 不修改 Codex 本体

不 patch Codex App，不要求 Codex 增加 `target_model` 元数据，也不依赖特定 App 版本的前端行为。

未来如果 Codex 官方在 compaction metadata 中增加目标模型，可以重新评估是否恢复 OpenAI 同 Provider 原生 V2。目前不为该可能性增加双路径复杂度。

### 3.3 不解析 OpenAI 原生 blob

CodexHub 不解密、不反序列化、不猜测 OpenAI compaction blob 的内部结构。

处理会话时，原生 OpenAI blob 不带 CodexHub marker，Gateway 原样交还给旧模型。只有历史版本已经写入 marker 的会话才执行兼容解包。

## 4. 总体流程

```text
Codex
  POST /responses
  input = [...history, {type: compaction_trigger}]
  x-codex-turn-metadata.request_kind = compaction
        |
        v
CodexHub 检测 Compact V2
        |
        +-> 移除 compaction_trigger
        +-> 展开已有 portable summary
        +-> 恢复当前旧 Provider 可读取的私有状态
        +-> 添加交接摘要 prompt
        +-> 禁用工具调用
        |
        v
旧模型 / 旧 Provider 生成普通文本摘要
        |
        v
CodexHub 合成 Compact V2 SSE
  output item = compaction
  encrypted_content = codexhub:compact:v1:<payload>
        |
        v
Codex 安装 replacement history
        |
        v
下一次普通模型请求
        |
        v
CodexHub 将 portable compaction 替换成 role=user 摘要
        |
        v
目标 OpenAI / Grok / DeepSeek / Anthropic Provider
```

## 5. Compact V2 请求识别

### 5.1 主判断

请求 `input` 中存在：

```json
{"type":"compaction_trigger"}
```

Codex 当前把 trigger 放在输入末尾。实现不应只依赖最后一个数组元素，应该扫描顶层 `input`，确保协议小改动不会漏判。

### 5.2 辅助元数据

解析 `x-codex-turn-metadata`：

```json
{
  "request_kind": "compaction",
  "compaction": {
    "trigger": "auto",
    "reason": "comp_hash_changed",
    "implementation": "responses_compaction_v2",
    "phase": "pre_turn",
    "strategy": "memento"
  }
}
```

元数据用于日志、指标和兼容检查，不作为唯一判断条件。缺少或解析失败时，只要存在合法 `compaction_trigger`，仍按 V2 压缩处理。

### 5.3 不允许递归

Gateway 内部发起的摘要请求必须带内部标记，禁止再次进入 Compact V2 截获逻辑。建议使用进程内调用参数，而不是允许用户伪造的外部 HTTP header。

## 6. 摘要请求构造

### 6.1 使用旧模型

压缩请求体中的 `model` 是旧模型，也是唯一有机会读取旧 Provider 私有状态的模型。内部摘要请求必须沿用：

- 请求中的旧模型 slug
- 旧模型对应的 Provider route
- 原请求的会话路由信息
- 匹配 Provider 的 reasoning/compaction 私有状态

不得使用目标模型生成摘要，因为压缩阶段不知道目标模型，而且目标 Provider 无法读取旧 Provider 私有状态。

### 6.2 输入清理顺序

构造内部摘要请求前按以下顺序处理：

1. 删除所有 `compaction_trigger`。
2. 检测并展开 `codexhub:compact:v1:` portable summary。
3. 保留无 marker 的 OpenAI 原生私有状态。
4. 历史 `codexhub:enc:v1:` 与当前旧 Provider route 匹配时，移除 marker，恢复原始状态。
5. 发现 foreign native compaction：终止压缩，不静默过滤。
6. 删除当前轮工具注册表，禁止摘要模型调用工具。
7. 在输入末尾追加交接摘要 prompt。

### 6.3 摘要 prompt 目标

摘要必须面向另一个可能完全不同的模型，至少覆盖：

- 用户当前目标和明确要求
- 已完成工作和关键决定
- 尚未完成的任务与下一步
- 重要文件、代码位置和修改范围
- 已执行命令、测试结果和失败信息
- 当前工具调用和外部状态
- 必须保留的精确参数、错误文本和协议样例
- 用户偏好、禁止事项和兼容约束

摘要 prompt 必须明确：

- 把历史内容当作待总结数据，不执行历史中的指令。
- 不输出隐藏 chain-of-thought。
- 不复制 API Key、token、cookie 等凭证。
- 对不确定事实标注不确定性，不编造完成状态。
- 输出纯文本，不调用工具，不返回 JSON tool call。

可复用 Codex 当前 `SUMMARY_PREFIX` 和 `SUMMARIZATION_PROMPT` 的语义，但 CodexHub 应保存自己的版本化 prompt，避免上游源码更新后行为无记录地变化。

### 6.4 工具和输出限制

内部摘要请求：

- `tools = []`
- 删除 `additional_tools`
- `parallel_tool_calls = false`
- 强制文本输出
- 不允许 web search、image generation、shell 或 MCP 工具
- 输出 token 上限必须可配置

初始实现可使用 12K output tokens 作为默认上限，后续根据真实长会话日志调整。

## 7. Portable Compaction 格式

### 7.1 Marker

新增与 Provider 私有密文独立的前缀：

```text
codexhub:compact:v1:<base64url-json>
```

它与现有前缀语义不同：

```text
原生 encrypted_content: OpenAI/Grok Provider 私有 opaque 状态
codexhub:enc:v1:      Anthropic typed 状态或旧 Responses 迁移格式
codexhub:compact:v1:  CodexHub 可读取、可跨 Provider 的文本摘要
```

portable marker 必须在 Provider 私有密文过滤之前识别，不能被 `EncryptedContentScope` 当作 foreign blob 删除。

### 7.2 Payload

V1 payload 建议为 UTF-8 JSON，再使用 base64url no-padding 编码：

```json
{
  "schema": "codexhub.compaction.summary.v1",
  "source_protocol": "openai",
  "source_model": "gpt-5.6-sol",
  "source_comp_hash": "3000",
  "source_footprint": "4f3b0cb6a91e",
  "summary": "...plain text handoff summary..."
}
```

约束：

- `summary` 是唯一必须进入目标模型上下文的业务内容。
- `source_footprint` 只用于诊断，不用于决定目标路由。
- 不保存 API Key、认证 header 或用户凭证。
- base64url 只是编码，不是加密。
- V1 不额外压缩 payload，避免引入 zstd/gzip 版本和跨平台兼容问题。
- 新格式必须使用新的 marker 版本，不原地改变 V1 语义。

### 7.3 Token 估算约束

Codex 不会把 `compaction.encrypted_content` 当作零成本字段。当前源码会根据字符串长度，按近似 base64 payload 的方式估算 model-visible tokens。

因此 portable payload 必须满足：

1. 摘要输出有明确 token 上限。
2. metadata 保持精简，不能把原历史 JSON 一起塞入 payload。
3. 安装 portable compaction 后，估算 token 必须显著低于当前模型的 auto-compact 阈值。
4. 必须测试长摘要不会导致“压缩成功后立即再次压缩”的循环。
5. 若内部摘要超过上限，应在生成阶段截断或重试更短摘要，不能依赖 Codex 对 marker 字符串做截断。

12K 摘要只是初始默认值，不是固定协议常量。实现阶段应通过真实会话测量 portable envelope 的最终字符串长度和 Codex 重算后的 token usage。

## 8. 合成 Compact V2 响应

Codex V2 collector 要求得到一个 `ResponseItem::Compaction`，不能直接返回普通 assistant 消息。

合成 item：

```json
{
  "id": "cmp_codexhub_<uuid>",
  "type": "compaction",
  "encrypted_content": "codexhub:compact:v1:<base64url-json>"
}
```

虽然字段名仍是 `encrypted_content`，这里承载的是 CodexHub portable envelope，不应声称它是 OpenAI 原生密文。

SSE 至少保持以下生命周期一致：

```text
response.output_item.added
response.output_item.done
response.completed
```

要求：

1. `output_item.done.item` 必须是唯一的 `compaction` item。
2. `response.completed.output[]` 中的 item 与 done 事件完全一致。
3. 不能同时返回 assistant message 或 reasoning item。
4. `response.completed.usage` 应记录内部摘要请求的真实 token 用量，无法取得时才省略。
5. HTTP 状态、SSE content type 和结束标记必须符合现有 Responses stream 规范。

### 8.1 第一层：Codex -> CodexHub

Codex 发给 Gateway 的外层请求保持 Compact V2，不改变客户端协议：

```http
POST /ai-gateway/v1/responses
Content-Type: application/json
Accept: text/event-stream
X-Codex-Turn-Metadata: {"request_kind":"compaction","compaction":{"reason":"comp_hash_changed","implementation":"responses_compaction_v2","phase":"pre_turn","trigger":"auto","strategy":"memento"}}
```

```json
{
  "model": "gpt-5.6-sol",
  "instructions": "...current Codex instructions...",
  "input": [
    {"type": "message", "role": "user", "content": []},
    {"type": "compaction", "encrypted_content": "...optional old state..."},
    {"type": "compaction_trigger"}
  ],
  "tools": [],
  "parallel_tool_calls": true,
  "stream": true
}
```

这里的 `model` 是旧模型。Gateway 不把该请求原样发给 OpenAI，而是在 HTTP 响应尚未开始前完成内部摘要调用。

### 8.2 第二层：CodexHub -> 旧 OpenAI Provider

Gateway 构造普通 Responses 摘要请求：

```json
{
  "model": "gpt-5.6-sol",
  "instructions": "...preserved current Codex instructions...",
  "input": [
    {"type": "message", "role": "user", "content": []},
    {"type": "compaction", "encrypted_content": "...native OpenAI state if present..."},
    {
      "type": "message",
      "role": "user",
      "content": [
        {
          "type": "input_text",
          "text": "You are performing a context checkpoint compaction. Produce a concise handoff summary..."
        }
      ]
    }
  ],
  "tools": [],
  "parallel_tool_calls": false,
  "stream": false,
  "max_output_tokens": 12000
}
```

转换规则：

1. 保留原 `instructions`，在最后追加 user 摘要 prompt，行为与 Codex 本地 compaction 接近。
2. 删除 `compaction_trigger`。
3. 删除顶层 `tools` 和 `input[].additional_tools`。
4. 已有 portable compaction 先展开成 user 摘要。
5. 已有 OpenAI 原生 compaction blob 时原样发送；历史 marker 与当前 route 匹配时先兼容解包，让旧 OpenAI 模型读取自己的状态。
6. 内部请求可以使用 JSON 非流式响应；若 Provider adapter 只提供流式接口，也可以内部收集完整 SSE，但不能把该流直接转发给 Codex。

旧 OpenAI 正常返回普通 assistant 文本：

```json
{
  "id": "resp_internal_summary",
  "output": [
    {
      "type": "message",
      "role": "assistant",
      "content": [
        {
          "type": "output_text",
          "text": "Current objective...\nCompleted...\nNext steps..."
        }
      ]
    }
  ],
  "usage": {
    "input_tokens": 180000,
    "output_tokens": 3500,
    "total_tokens": 183500
  }
}
```

Gateway 只提取 assistant `output_text`。出现 tool call、空文本或不完整响应时，本次压缩失败。

### 8.3 第三层：CodexHub -> Codex

普通 assistant 摘要不能原样返回 Codex。Gateway 必须重新包装成 Compact V2 SSE。

假设生成：

```text
response id = resp_codexhub_compact_123
item id     = cmp_codexhub_123
payload     = codexhub:compact:v1:<base64url-json>
```

推荐返回：

```text
event: response.created
data: {"type":"response.created","response":{"id":"resp_codexhub_compact_123"}}

event: response.output_item.added
data: {"type":"response.output_item.added","output_index":0,"item":{"id":"cmp_codexhub_123","type":"compaction","encrypted_content":"codexhub:compact:v1:<base64url-json>"}}

event: response.output_item.done
data: {"type":"response.output_item.done","output_index":0,"item":{"id":"cmp_codexhub_123","type":"compaction","encrypted_content":"codexhub:compact:v1:<base64url-json>"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_codexhub_compact_123","status":"completed","end_turn":true,"output":[{"id":"cmp_codexhub_123","type":"compaction","encrypted_content":"codexhub:compact:v1:<base64url-json>"}],"usage":{"input_tokens":180000,"input_tokens_details":{"cached_tokens":0},"output_tokens":3500,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":183500}}}

```

HTTP 响应：

```http
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-cache
```

Codex 当前真正依赖的是：

1. 至少一个可解析的 `response.output_item.done`。
2. done item 中恰好有一个 `type=compaction`。
3. 流最终出现可解析的 `response.completed`。

`response.created` 和 `response.output_item.added` 当前不会决定压缩结果，但为了保持完整 Responses 生命周期仍应发送。`response.completed.response.id` 必须存在；usage 字段存在时必须包含完整的 input、output 和 total tokens。

内部摘要响应中的 assistant message、reasoning 和 delta 事件全部终止在 Gateway 内部，不能泄漏到外层 V2 流。

### 8.4 第四层：Codex 下一轮请求 -> 目标 Provider

Codex 安装完成后，下一轮会回放：

```json
{
  "type": "compaction",
  "id": "cmp_codexhub_123",
  "encrypted_content": "codexhub:compact:v1:<base64url-json>"
}
```

Gateway 在任何 Provider 私有密文处理之前，将其替换成：

```json
{
  "type": "message",
  "role": "user",
  "content": [
    {
      "type": "input_text",
      "text": "<Codex summary prefix>\nCurrent objective...\nCompleted...\nNext steps..."
    }
  ]
}
```

随后才执行 OpenAI、Grok、DeepSeek 或 Anthropic adapter。即使目标仍是 OpenAI，也使用同样的 user 摘要，不把 portable envelope 当作原生 OpenAI compaction blob。

### 8.5 与现有密文中间件的顺序

当前 `responses_compat` 不会给 OpenAI/Grok 的 `compaction.encrypted_content` 添加 Provider scope。若未来实现 portable item，仍必须与 Anthropic typed marker 和历史 Responses marker 使用不同解析器：

```text
portable 格式：
codexhub:compact:v1:...
```

请求预处理顺序必须是：

```text
解析 body
  -> 识别并展开 portable compaction
  -> Responses Lite 工具整理
  -> Provider 私有 encrypted_content scope 处理
  -> 目标 Provider adapter
```

响应处理有两个可选实现：

1. synthetic compaction SSE 使用专用返回路径，绕过普通 Provider response marker 包装。
2. 通用 `encode_response_object` 遇到 `codexhub:compact:v1:` 时明确保持原样。

无论采用哪种方式，都必须测试 portable marker 不会变成 OpenAI/Grok 私有 marker。

### 8.6 失败响应边界

Gateway 应先完成内部摘要，再开始向 Codex 写 SSE。这样摘要失败时可以直接返回普通 HTTP 错误，不会产生半截 Compact V2 流。

一旦已经发出 `response.created` 或 `output_item.added`，后续失败就必须使用合法 `response.failed` 结束；初始实现应避免进入这一复杂路径。

## 9. 后续请求展开

普通请求发往任意上游前，扫描 `input` 中的：

```json
{
  "type": "compaction",
  "encrypted_content": "codexhub:compact:v1:..."
}
```

将其原位替换为：

```json
{
  "type": "message",
  "role": "user",
  "content": [
    {
      "type": "input_text",
      "text": "<Codex summary prefix>\n<portable summary>"
    }
  ]
}
```

使用 `user` 而不是 `system` 或 `developer`，原因是：

- Codex 本地摘要本身使用 `role=user`。
- 摘要中可能包含原用户输入，不能无故提升为 system 权限。
- OpenAI、Grok、Chat Completions 和 Anthropic Messages 都能稳定表达 user 文本。

展开后再进入现有 Provider adapter：

- OpenAI/Grok Responses：保留 Responses user message。
- DeepSeek Chat Completions：转换成 `role=user` message。
- Anthropic Messages：转换成 Anthropic user content block。

portable marker 绝不能原样发给任何上游。

## 10. 重复压缩

第二次及后续 Compact V2 请求可能已经包含旧 portable compaction。

处理方式：

```text
旧 portable compaction
  -> 展开成 user 摘要
  -> 与最近真实消息一起交给旧模型
  -> 生成新的完整摘要
  -> 返回新的 portable compaction
```

不得把多个 portable marker 无限叠加，也不得仅总结最近消息而忽略旧摘要。

## 11. 旧原生 V2 会话迁移

实现上线前，用户历史中可能已经包含 OpenAI 或 Grok 原生 compaction blob。

迁移规则：

1. marker 与当前旧 Provider route 匹配：解包原始 blob，交给旧模型生成 portable summary。
2. 已是 `codexhub:compact:v1:`：直接展开后重新总结。
3. marker 属于其他 Provider route：中止压缩并报告上下文来源不匹配。
4. 无 marker 的 legacy blob：只允许向当前旧 Provider 原样尝试一次。
5. legacy blob 被上游拒绝：中止压缩，不执行“删除 blob 后继续摘要”的静默降级。

压缩迁移失败时，Codex 应保留原历史并终止当前轮。不能安装空摘要，也不能只保留最近消息后假装迁移成功。

## 12. `comp_hash` 策略

所有可选模型必须提供非空 `comp_hash`。它负责让 Codex 在模型兼容边界执行 pre-sampling compaction。

原则：

1. OpenAI 模型保留模型目录提供的原始 hash。
2. 非 OpenAI 模型使用 CodexHub 自己的稳定、版本化 hash。
3. 不同协议族的 hash 必须不同。
4. 同协议族只有在压缩和私有状态语义确实兼容时才能共享 hash。
5. 改变摘要格式、工具历史降级规则或 Provider 私有状态表示时，应升级对应 hash。

当前内置模型目录：

| 模型族 | `comp_hash` |
| --- | --- |
| GPT-5.6 Sol/Terra/Luna | 保留 `3000` |
| GPT-5.5/5.4 系列 | 保留 `2911` |
| Grok Responses | `codexhub-grok-summary-v1` |
| DeepSeek Chat Completions | `codexhub-deepseek-summary-v1` |
| Anthropic Messages/Claude/GLM | `codexhub-anthropic-summary-v1` |

不能给全部非 OpenAI 模型填写同一个 hash，否则 Grok -> Anthropic 等跨协议切换不会触发摘要。

## 13. 错误处理

### 13.1 必须终止当前压缩

以下情况返回明确错误，不安装新历史：

- 内部摘要请求失败或超时。
- 摘要模型返回工具调用而不是文本。
- 摘要文本为空。
- portable marker 无法解码或 schema 不支持。
- 发现 foreign native compaction。
- 合成 SSE 不满足单 compaction item 约束。
- 上游拒绝旧 Provider 自己的原生 compaction blob。

### 13.2 不允许的静默降级

- 删除 compaction blob 后继续。
- 只保留最近 64K 消息并宣称压缩成功。
- 把 foreign blob 作为普通文本发送给目标模型。
- 将 portable summary 提升成 system/developer 消息。
- 摘要失败后自动切换到其他模型重新总结。

## 14. 安全约束

1. portable summary 与普通会话历史具有相同敏感级别。
2. base64url 不是加密，日志和诊断包必须按敏感内容处理。
3. 内部摘要 prompt 必须防止历史 prompt injection。
4. 不记录 API Key、认证 header、cookie 或原始 token。
5. `codexhub:compact:v1:` 只能存在于 Codex 与 CodexHub 之间。
6. portable marker、Anthropic typed marker 和旧 Responses marker 必须使用不同解析器和测试矩阵。
7. 内部摘要请求不允许调用任何工具。

## 15. 可观测性

每次压缩至少记录：

- parent request id
- session/thread/window id
- compaction trigger、reason、phase、implementation
- source model、source Provider type、source route footprint
- 输入 token、摘要输出 token、缓存 token
- 摘要耗时、总压缩耗时
- 是否迁移了旧 native compaction
- 是否展开了旧 portable summary
- portable schema version
- 最终状态和错误分类

默认日志不写摘要正文。只有用户明确开启请求详情时才保存，并继续受诊断导出脱敏规则约束。

## 16. 测试矩阵

### 16.1 请求识别

- metadata 完整 + `compaction_trigger`
- metadata 缺失 + `compaction_trigger`
- metadata 非法 + `compaction_trigger`
- 普通请求不存在 trigger，不得误判
- trigger 不在最后一个 input 时仍可识别

### 16.2 Provider 切换

- OpenAI -> OpenAI
- OpenAI -> Grok
- Grok -> OpenAI
- OpenAI -> DeepSeek
- DeepSeek -> OpenAI
- OpenAI -> Anthropic
- Anthropic -> OpenAI
- Grok -> Anthropic
- Anthropic -> DeepSeek

所有路径最终送给目标 Provider 的都必须是普通 user 摘要，不得包含 portable marker 或 foreign native blob。

### 16.3 历史迁移

- 新 portable summary 正常展开
- 连续两次 portable compaction
- OpenAI 原生 compaction -> portable summary
- Grok 原生 compaction -> portable summary
- 匹配 route 的 native marker 正常解包
- foreign route marker 明确失败
- legacy 无 marker blob 成功迁移
- legacy blob 被拒绝后终止，不静默删除

### 16.4 SSE

- exactly one compaction item
- added/done/completed item 一致
- completed usage 正确
- portable payload 的 token 重算符合预期
- 安装后不会立即再次触发 context-limit compaction
- 流中断时不安装新历史
- 非重试错误直接结束
- retry 不得重复生成或安装两个 portable item

### 16.5 `comp_hash`

- 所有模型均有非空 hash
- 不同协议族 hash 不同
- GPT-5.6 -> GPT-5.5 触发 compaction
- GPT-5.6 -> Grok 触发 compaction
- Grok -> Anthropic 触发 compaction
- 同 hash 同模型普通 turn 不额外 compaction

### 16.6 能力回归

- Provider 配置 ID 仍为 `ai-gateway`
- Provider `name` 保持 `OpenAI`
- `web.run` 仍出现在 GPT-5.6 工具注册表
- `/alpha/search` 路由不受影响
- `remote_compaction_v2` 保持开启
- 不调用旧 `/responses/compact`
- 模型列表、Remote Control、image generation 不受影响

## 17. 若启用时的实施顺序

1. 给全部模型补齐并测试 `comp_hash`。
2. 增加 portable compaction codec 和独立 marker。
3. 在普通请求出站前实现 portable marker 展开。
4. 解析 `x-codex-turn-metadata` 的 compaction 字段。
5. 检测并截获 `compaction_trigger`。
6. 实现无工具的内部摘要调用。
7. 实现 Compact V2 SSE 合成器。
8. 增加旧 native compaction 迁移逻辑。
9. 完成跨 Provider、连续压缩和错误路径测试。
10. 更新请求日志详情、诊断文档和 release note。

每一步完成后保持普通 `/responses` 路径可测试，不在一个提交中同时重写全部 Provider adapter。

## 18. 代价与接受的取舍

统一文本摘要会失去 OpenAI 原生 Compact V2 的部分优势：

- 原生模型状态保真度可能更高。
- 文本摘要可能遗漏细节。
- 每次压缩需要一次完整摘要生成，增加延迟和费用。
- 多次摘要可能产生信息漂移。

如果未来启用本方案，项目需要接受这些代价，以换取：

- OpenAI、Grok、DeepSeek、Anthropic 行为统一。
- 不需要推断未知的目标 Provider。
- 压缩内容可检查、可测试、可迁移。
- 模型切换时不依赖 foreign opaque blob。
- 保留 `name = "OpenAI"` 和原生 `web.run`。
- 继续使用 Codex 当前 Compact V2 生命周期，不依赖旧 Compact V1。

## 19. 非目标

- 不破解或解析 OpenAI/Grok 原生 compaction blob。
- 不 patch Codex App 增加目标模型元数据。
- 不恢复同 Provider 原生 V2 与跨 Provider 文本摘要的双路径优化。
- 不使用服务器数据库保存 blob -> summary 映射作为主要状态。
- 不把 portable summary 当作长期记忆或用户画像系统。
- 不保证摘要能百分之百还原被压缩前的所有细节。

## 20. 升级审计

Codex 或 OpenAI 协议升级后至少检查：

1. `ResponseItem::Compaction` 是否仍只有 opaque `encrypted_content`。
2. `compaction_trigger` 是否仍存在，位置和字段是否变化。
3. V2 collector 是否仍要求 exactly one compaction item。
4. `x-codex-turn-metadata.compaction` 是否新增目标模型信息。
5. retained message token budget 是否仍为 64K。
6. `remote_compaction_v2` 是否仍默认开启。
7. Provider `name = "OpenAI"` 是否仍是 `web.run` gate 的组成部分。
8. Compact V2 SSE 事件是否新增必填字段。
9. 新模型目录中的 `comp_hash` 是否变化。
10. portable marker 是否仍会被 Codex 原样保存和回放。

如果未来官方 metadata 提供可靠的 `target_model` 和 `target_provider`，可以重新评估同 Provider 原生 V2 与跨 Provider 文本摘要的双路径。若真实用户日志证明当前过滤行为造成不可接受的上下文丢失，也可以重新启用本文的统一摘要方案；在此之前，本文只作为技术备选，不代表当前实现路线。

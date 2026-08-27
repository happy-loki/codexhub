# CodexHub 智谱 Anthropic 搜索协议说明

更新时间：2026-08-27

状态：已实现。本文同时作为后续维护和回归测试的协议基线。

## 1. What：我们要处理什么

本文件只讨论下面这条链路：

```text
Codex Responses 请求
    -> CodexHub AI Gateway
    -> 智谱 Anthropic Messages 接口
```

需要兼容的搜索形态有两种，它们名称相似，但协议语义不同。

### 1.1 Claude Code 的 `WebSearch`

Claude Code 把搜索声明成客户端执行的工具。典型流程是两次 Anthropic
Messages 请求：

```text
第 1 次 /messages
  assistant -> tool_use(name = "WebSearch", input = { query })

第 2 次 /messages
  user -> tool_result(tool_use_id, 搜索文本)
  assistant -> 最终回答
```

也就是说，模型先提出工具调用，客户端执行搜索，再把结果作为下一次请求的
`tool_result` 交回模型。

### 1.2 智谱的 `web_search_prime`

智谱 Anthropic 兼容接口可以把搜索作为服务端工具执行。观察到的 SSE
内容块类似：

```json
{
  "type": "server_tool_use",
  "id": "call_search_1",
  "name": "web_search_prime",
  "input": {"search_query": "OpenAI June 2026"}
}
```

随后结果可能出现在同一轮 SSE 的 `tool_result` 中，并且 `content` 可能是
嵌套 JSON 字符串：

```json
{
  "type": "tool_result",
  "tool_use_id": "call_search_1",
  "content": "[{\"text\":[{\"title\":\"OpenAI News\",\"link\":\"https://openai.com/news/\",\"content\":\"...\"}],\"type\":\"text\"}]"
}
```

这两个内容块属于同一次服务端搜索，不等价于 Claude Code 的两次 HTTP
请求。智谱还可能在文本块中输出类似 `Z.ai Built-in Tool:
web_search_prime` 的内部过程信息。

这里要区分两个层次：智谱原生服务端搜索通常在同一个 Anthropic SSE 轮次里
完成 `server_tool_use` 和结果回包；而 Codex 发给网关的是客户端可执行的
`WebSearch` 意图，网关为了执行并回填结果，仍会发起“模型请求 -> 搜索请求 ->
模型继续回答”的桥接流程。前者描述上游能力，后者描述 CodexHub 的适配方式，
不能把它们简单理解成同一套两段式协议。

### 1.3 CodexHub 的职责

CodexHub 需要完成三件事：

1. 把 Codex 的标准搜索工具意图转换成智谱能执行的 Anthropic 搜索请求。
2. 从智谱的多种结果包装中提取可读的标题、链接和摘要，作为下一轮
   Anthropic `tool_result`。
3. 对 Codex 只输出标准 Responses `web_search_call`，隐藏智谱私有工具名和
   私有过程文本。

## 2. Why：为什么不能直接透传

### 2.1 字段名称不一致

标准结果通常使用 `url`、`snippet` 或 `encrypted_content`；智谱结果可能使用
`link`、`content` 或 `description`。如果只读取标准字段，会把真实结果误判
为空。

### 2.2 结果可能被多层编码

智谱的 `tool_result.content` 可能是：

- 直接的对象或数组；
- JSON 数组包着 `{ "text": [...] }`；
- JSON 字符串再次包着上述数组；
- 多层数组或多个文本块。

因此不能用一次 `as_array()` 或只匹配一个字段解决，必须先递归解包，再做
字段归一化。

### 2.3 两套工具生命周期不能重复展示

Anthropic 转换器已经会把主对话中的 `WebSearch` 工具请求转换成一个
Responses `web_search_call`。内部搜索完成后，如果网关再额外发送一个合成
的 `web_search_call`，Codex 会看到两个搜索条目。

这不是智谱执行了两次搜索，而是网关把“模型提出搜索”和“网关内部执行搜索”
分别暴露了。一次真实搜索对 Codex 必须只有一个可见的
`web_search_call`。

### 2.4 空结果会诱发错误降级

日志 39688 的事实是：

- 渠道为 `glm`，模型为 `GLM-5.2`；
- 智谱实际上返回了标题、链接和正文；
- 旧解析器没有读出这些字段，生成了
  `No web search results were returned for query: ...`；
- 模型随后把搜索当作失败，改用 `curl.exe`。

这会造成额外请求、较差的回答和看起来像“搜索没有生效”的问题。

## 3. How：目标实现方式

### 3.1 请求流程

保留现有的内部桥接模型：

```text
1. Codex -> Gateway：携带标准搜索工具
2. Gateway -> 智谱：发送 Anthropic Messages 请求
3. 智谱 -> Gateway：返回 server_tool_use / tool_result
4. Gateway：归一化结果，组成 Anthropic tool_result
5. Gateway -> 智谱：追加 assistant 工具调用和 user 工具结果
6. 智谱 -> Gateway：返回最终答案
7. Gateway -> Codex：透传答案和一个标准 web_search_call
```

内部 `tool_result` 必须保留，因为它是模型真正读取搜索内容的载体；不能只
发一个空的 Responses 搜索事件就结束。

### 3.2 搜索结果归一化

实现一个仅负责解析的结构化归一化函数，输入原始 SSE，输出稳定的纯文本：

```text
[{ title, url, snippet }]
```

解析器至少支持：

| 来源形态 | 标题 | 链接 | 摘要 |
| --- | --- | --- | --- |
| 标准 `web_search_result` | `title` | `url` | `snippet` / `description` |
| 智谱文本结果 | `title` | `link` / `url` | `content` / `snippet` / `description` |
| 嵌套 `text` 数组 | 同上 | 同上 | 同上 |

规则：

- 递归解析 JSON 字符串、数组和对象包装；
- 只把包含可读标题、链接或摘要的对象作为结果；
- `encrypted_content` 只作为不可读的私有字段，不把它当作摘要；
- 去除重复结果，保持上游出现顺序；
- 只有确实没有任何可读结果时，才生成
  `No web search results were returned for query: ...`；
- 解析失败不能让整个主请求崩溃，但必须保留诊断日志。

### 3.3 Responses 事件规则

Codex 可见事件遵循以下不变量：

```text
一次真实搜索 = 一个 response.output_item.added(web_search_call)
             + 一个 response.output_item.done(web_search_call)
```

具体做法：

- 复用转换器已经产生的第一个 `web_search_call`；
- 流式内部搜索完成后，不再调用额外的
  `emit_injected_web_search_call()`；
- `tool_result` 继续写回下一轮 Anthropic 请求，但不单独变成第二个
  Responses 搜索条目；
- 非流式和流式路径使用相同的结果归一化规则；
- `web_search_call.action` 只携带 Codex 当前稳定消费的
  `type`、`query`、`queries`，不强行塞入智谱私有结果结构。

当前保守实现允许第一个搜索事件在内部搜索完成前显示为已完成。如果实际
Codex UI 对状态时序敏感，再单独评估“抑制转换器事件、内部搜索完成后只注入
一个事件”的复杂方案；本次不预先引入该风险。

### 3.4 私有过程文本

仅在 `glm_anthropic` profile 下过滤智谱的过程标记，例如：

- `Z.ai Built-in Tool: web_search_prime`；
- `web_search_prime_result_summary`。

普通回答中的同名自然语言不应被无条件删除。过滤只作用于已识别为私有
搜索过程的文本块，并保留正常最终回答。

## 4. What 不在本次范围内

本次不做以下工作：

- 不实现 Responses Lite 的 `web.run` 或 `/alpha/search`；
- 不要求智谱实现 Claude Code 的客户端两次 HTTP 请求；
- 不把智谱私有搜索结果完整映射成 OpenAI annotations；
- 不修改通用 Chat Completions provider；
- 不改变 Codex 的工具注册表；
- 不把不可读的密文伪装成可读搜索摘要。

## 5. 实现文件与测试

实际修改范围：

```text
src/ai_gateway/providers/anthropic_messages/mod.rs
src/ai_gateway/providers/anthropic_messages/stream_internal.rs
src/ai_gateway/providers/anthropic_messages/tests.rs
```

解析逻辑保留在 adapter 内，避免为了本次修复重构整个 Anthropic adapter。

已落地的行为：

- 流式内部搜索复用转换器产生的搜索事件，不再追加第二个网关合成事件；
- 结果解析支持标准 `web_search_tool_result` 和智谱 `tool_result`；
- 结果解析支持数组、双层数组、`text` 包装和 JSON 字符串；
- 结果字段支持 `url/link` 与 `snippet/content/description`；
- 结果按 URL 去重，并补齐同一结果后续出现的缺失字段；
- 原始密文不会被当作可读搜索正文。

必须覆盖：

1. 标准 `web_search_tool_result` 的解析；
2. 智谱 `tool_result` + 嵌套 JSON 字符串的解析；
3. `link` 到 `url`、`content` 到摘要的映射；
4. 多层数组和重复结果；
5. 只有空结果时才生成空结果提示；
6. 流式内部搜索只产生一个 `web_search_call`；
7. 智谱私有过程文本过滤后，最终回答仍然保留。

验证命令：

```powershell
cargo fmt --all -- --check
cargo test -q anthropic_messages
cargo test -q
git diff --check
```

## 6. 验收标准

- 真实智谱搜索结果不再被误判为空；
- Codex 能在后续回答中使用标题、链接和摘要；
- 同一次搜索在 Codex 侧只有一个 `web_search_call`；
- 智谱的 `web_search_prime` 和过程标记不会泄漏到最终用户界面；
- 普通文本、函数工具和非 GLM Anthropic profile 的行为不改变；
- 现有工作区未提交文件和运行时 SQLite 日志不被覆盖、删除或加入提交。

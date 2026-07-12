# AI Gateway Provider 私有密文作用域

## 背景

OpenAI Responses、Grok Responses 等上游会在 `reasoning.encrypted_content` 中返回只能由原渠道继续使用的不透明状态。Anthropic Messages 也有同类 Provider 私有状态：

- `thinking.signature`：与 thinking 内容绑定的签名。
- `redacted_thinking.data`：不可解析的 redacted thinking 数据。

CodexHub 会把 Anthropic 的这两个字段映射到 Responses `reasoning.encrypted_content`。Codex 会保存该字段，并在同一会话的后续请求中原样回放。

当用户在同一会话中切换模型时，Codex 只知道自己仍在访问 CodexHub 的 Responses 接口，并不知道 CodexHub 内部已经从 Grok 切换到 OpenAI。若直接把 Grok 密文发送给 OpenAI，上游会返回 `invalid_encrypted_content` 或“encrypted content could not be verified”。

CodexHub 参考 AxonHub 的 signature marking 方案，为 Provider 私有密文增加稳定的内部作用域标记。

## 标记格式

```text
codexhub:enc:v1:<protocol>:<footprint>:<raw encrypted content>
```

示例：

```text
codexhub:enc:v1:grok:4f3b0cb6a91e:p3HD...G1SY
```

Anthropic 额外保存原始 content block 类型：

```text
codexhub:enc:v1:anthropic:<footprint>:thinking:<raw signature>
codexhub:enc:v1:anthropic:<footprint>:redacted_thinking:<raw data>
```

- `codexhub:enc:v1:`：固定版本前缀。
- `protocol`：当前上游协议，现有值为 `openai`、`grok` 或 `anthropic`。
- `footprint`：Provider route 的 SHA-256 前 6 字节，编码为 12 位十六进制。
- `raw encrypted content`：上游返回的原始密文，不修改内容。
- Anthropic `kind`：明确区分 `thinking.signature` 和 `redacted_thinking.data`，不能根据 thinking 文本是否为空推断。

Provider route 由以下字段组成：

```text
provider name + provider type + base URL
```

API Key 不进入指纹，避免泄漏凭证，也允许同一渠道轮换 Key。修改 Provider 名称、类型或 Base URL 会生成新指纹，旧密文会被视为其他渠道的私有状态。

## 响应方向

在把上游响应返回给 Codex 前：

1. 检查 `reasoning`、`compaction`、`compaction_summary` 和 `context_compaction` item。
2. 若存在非空 `encrypted_content`，添加当前 Provider 的协议和渠道指纹。
3. JSON 响应与 SSE 的 `data:` 事件使用同一套递归处理。
4. 已带 CodexHub 标记的值不会重复包装。

Anthropic JSON 响应和 SSE 响应都会在返回 Codex 前完成包装。流式响应中的 `response.output_item.done` 和最终 `response.completed` 使用相同 marker，避免 Codex 在不同事件中保存到不一致的私有状态。

Codex 将标记后的字符串当作不透明内容保存，不需要理解前缀。

## 请求方向

在把 Codex 请求发送给上游前：

1. 标记的协议和指纹与当前 Provider 完全一致：移除 CodexHub 前缀，恢复原始密文并发送。
2. 协议或指纹不一致：删除 `encrypted_content`、Provider 私有 item `id` 和 `status`。
3. 删除密文后仍有可读 `summary` 或 `content`：保留该 reasoning item。
4. 删除密文后 item 已无有效内容：删除整个 item。

因此：

```text
Grok -> Grok   保留并恢复 Grok 密文
Grok -> OpenAI 删除 Grok 密文
OpenAI -> Grok 删除 OpenAI 密文
OpenAI -> OpenAI 保留并恢复 OpenAI 密文
Anthropic -> Anthropic（同渠道）恢复 thinking.signature 或 redacted_thinking.data
Anthropic -> Anthropic（不同渠道）删除 Anthropic 私有状态
OpenAI/Grok -> Anthropic 忽略非 Anthropic 私有状态
Anthropic -> OpenAI/Grok 删除 Anthropic 私有状态
```

不同 Base URL 或不同 Provider 配置之间也不会误用密文。

Anthropic 请求重建还有一条额外约束：reasoning、assistant 文本和紧随其后的 `tool_use` 必须合并到同一条 assistant message，保持原始内容顺序。具体恢复规则为：

1. marker 与当前 Anthropic Provider 匹配、类型是 `thinking` 且 reasoning 有非空 summary：构造 `thinking`，summary 作为 `thinking`，解包后的原始值作为 `signature`。
2. marker 类型是 `thinking` 且 summary 是空数组：仍构造 `thinking`，thinking 文本为空，原始值作为 `signature`。这是上游隐藏 thinking 展示内容时的合法形态，不能降级为 redacted。
3. marker 类型是 `redacted_thinking`：构造 `redacted_thinking`，解包后的原始值作为 `data`。
4. marker 的协议、类型或渠道指纹不匹配：忽略整个 Anthropic 私有 reasoning block。

## 旧会话迁移

旧版本保存的密文没有 CodexHub 前缀，无法可靠判断来源。迁移规则如下：

1. 请求中完全没有 CodexHub 标记时，首次仍保留旧密文，避免升级后破坏原有同渠道会话。
2. 上游若明确返回 `invalid_encrypted_content` 或密文无法校验，删除 Provider 私有密文并只重试一次。
3. 新的成功响应会带作用域标记。
4. 请求中一旦出现任意 CodexHub 标记，未标记的旧密文会被视为迁移残留并过滤，避免以后重复触发 400。

该迁移不依赖进程内映射或数据库，CodexHub 重启后行为保持一致。

上述兼容迁移只适用于 OpenAI/Grok Responses 透传路径。Anthropic 的旧无 marker `encrypted_content` 一律忽略，不尝试回放。原因是旧值已经被统一映射到 Responses 字段，无法可靠区分它原本是 `thinking.signature`、`redacted_thinking.data`，还是其他 Provider 的私有密文；贸然发送可能造成签名校验失败或跨渠道污染。新的 Anthropic 响应会自动带 marker，之后的同渠道回放不受影响。

## 约束

- CodexHub 标记只能存在于 Codex 与 CodexHub 之间，绝不能原样发送给上游。
- 匹配渠道解包后，原始密文字节必须保持不变。
- 不得仅按模型名称判断密文来源；模型 alias 和多渠道配置会导致误判。
- 不得把一个 Provider 的 item `id` 与另一个 Provider 的请求组合发送。
- 密文错误恢复最多重试一次，防止无效请求循环。
- 新增 Provider 私有签名协议时，应分配新的 `protocol` 标记，并在请求和响应两个方向同时接入。
- Anthropic block 类型必须显式保存在 marker 中；不得用 summary 文本是否为空区分 `thinking` 和 `redacted_thinking`。

## 升级审计

Codex 或上游协议更新后至少检查：

1. 新增的私有状态字段是否仍为 `encrypted_content`。
2. 新增的 item type 是否需要加入标记范围。
3. SSE 是否仍通过 `response.output_item.added`、`response.output_item.done` 或 `response.completed` 携带完整 item。
4. Codex 是否仍会在后续请求中回放标记后的字符串。
5. 同渠道工具调用续跑、Grok 切 OpenAI、OpenAI 切 Grok 是否通过集成测试。
6. Anthropic 的 `thinking.signature`、`redacted_thinking.data` 是否仍可在同渠道回放，并在切换 Provider 时被过滤。
7. Anthropic reasoning、assistant 文本和 `tool_use` 是否仍合并在同一条 assistant message 中。

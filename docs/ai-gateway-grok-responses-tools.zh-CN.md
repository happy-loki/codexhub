# Codex 到 Grok Responses 工具桥接

更新时间：2026-07-18

状态：已实现并通过测试。本文记录 CodexHub 对 Grok `apply_patch` 和 `web_search` 的当前
适配边界，供后续 Codex、xAI 或兼容上游升级时复核。

## 1. 结论

Grok Provider 的上游请求继续使用标准 OpenAI Responses：

```text
Codex -> CodexHub /ai-gateway/v1/responses -> Grok /v1/responses
```

这里没有把 Grok 声明成 Responses Lite，也不会向 Grok 上游附加 Responses Lite 私有请求头。
Codex 入站请求可能包含 Lite 使用的 `input[].additional_tools` 载体；CodexHub 只在网关内部把它
合并到标准 Responses 顶层 `tools[]`，然后再按 Grok 能力转换。

两个核心工具采用不同执行模型：

| 工具 | 发给 Grok 的形态 | 谁执行 |
| --- | --- | --- |
| `apply_patch` | `type=function`，参数为 `patch` | Grok 选择调用，Codex 客户端在本地执行 |
| `web_search` | `type=web_search` hosted tool | Grok/xAI 上游执行 |

因此，`apply_patch` 需要完整的双向协议桥接；`web_search` 只需要修正工具声明并透传上游搜索事件，
不需要 CodexHub 再发起第二轮搜索请求。

## 2. 总体数据流

### 2.1 apply_patch

```text
Codex custom apply_patch declaration
  -> CodexHub 转成 Grok function schema { patch: string }
  -> Grok 返回 function_call
  -> CodexHub 恢复成 custom_tool_call
  -> Codex 本地执行补丁
  -> Codex 下一轮发送 custom_tool_call_output
  -> CodexHub 转成 function_call_output 发给 Grok
```

`apply_patch` 不是 Grok/xAI 原生 hosted tool。模型只负责生成补丁内容，真正的文件读取、修改、
权限判断和执行结果仍由 Codex 本地工具完成。

### 2.2 web_search

```text
Codex web_search/web_search_preview declaration
  -> CodexHub 规范化为 Grok web_search declaration
  -> Grok 上游执行搜索
  -> 标准 Responses web_search_call 和最终回答
  -> CodexHub 透传给 Codex
```

这条路径没有本地函数调用、没有 `/v1/alpha/search` 回调，也没有网关代替模型执行搜索的第二轮
请求。上游模型或兼容服务必须真正支持 Responses hosted `web_search`。

## 3. apply_patch 请求映射

### 3.1 工具声明

Codex 使用自由文本 custom tool：

```json
{
  "type": "custom",
  "name": "apply_patch",
  "description": "...",
  "format": { "type": "grammar", "syntax": "lark", "definition": "..." }
}
```

Grok 标准 Responses 路径使用 JSON function tool。CodexHub 按 Grok Build 的参数约定转换为：

```json
{
  "type": "function",
  "name": "apply_patch",
  "description": "...",
  "parameters": {
    "type": "object",
    "properties": {
      "patch": {
        "type": "string",
        "description": "The complete patch body..."
      }
    },
    "required": ["patch"],
    "additionalProperties": false
  },
  "strict": false
}
```

关键差异是：

- Codex custom tool 的自由文本字段叫 `input`。
- Grok function 的 JSON 参数字段叫 `patch`。
- custom tool 的 grammar `format` 不发送给 Grok function。
- 工具说明中的示例和约束会同步改成 `patch`，避免模型仍生成 `{ "input": ... }`。
- Grok 专属说明把控制行定义为字面量，并给出正确/错误对照，明确禁止把
  `*** Begin Patch` 写成 Markdown 风格的 `*** Begin Patch ***`。
- `ToolNameMap` 保存“这个 function 原本是 custom tool”的请求级映射，响应时据此恢复原语义。

普通 custom tool 仍使用 `{ "input": string }`；只有 `apply_patch` 使用
`{ "patch": string }`。

### 3.2 历史回放

Codex 后续请求中的历史调用：

```json
{
  "type": "custom_tool_call",
  "call_id": "call_patch",
  "name": "apply_patch",
  "input": "*** Begin Patch\n...\n*** End Patch"
}
```

发送给 Grok 前转换为：

```json
{
  "type": "function_call",
  "call_id": "call_patch",
  "name": "apply_patch",
  "arguments": "{\"patch\":\"*** Begin Patch\\n...\\n*** End Patch\"}"
}
```

对应的 `custom_tool_call_output` 转为 `function_call_output`。`call_id` 保持不变，使调用和执行
结果继续配对。结构化工具结果按既有 Grok 兼容逻辑序列化为字符串。

## 4. apply_patch 响应映射

### 4.1 完整响应项

Grok 返回：

```json
{
  "type": "function_call",
  "call_id": "call_patch",
  "name": "apply_patch",
  "arguments": "{\"patch\":\"*** Begin Patch\\n...\\n*** End Patch\"}"
}
```

CodexHub 恢复为 Codex 能执行的形态：

```json
{
  "type": "custom_tool_call",
  "call_id": "call_patch",
  "name": "apply_patch",
  "input": "*** Begin Patch\n...\n*** End Patch"
}
```

响应解析同时兼容 `patch` 和旧的通用 `input` 参数，但新发给 Grok 的 `apply_patch` 声明和历史
统一使用 `patch`。

### 4.2 Grok 对称星号兼容

真实请求日志 `14704` 确认，Grok 在已经收到完整规则和 5 组 few-shot 的情况下，仍可能把所有
控制行误当成 Markdown 强调符号：

```text
*** Begin Patch ***
*** Add File: notes.md ***
+content
*** End Patch ***
```

Codex 本地执行器会正确拒绝它，因为标准语法要求控制行没有尾随 ` ***`。CodexHub 对 Grok
function 的 `patch` 参数增加了一个严格、局部的兼容修复：只移除已知控制行末尾的单个 ` ***`，
包括 Begin/End、Add/Delete/Update/Move 和 End of File。以 `+`、`-` 或空格开头的补丁内容不会
修改；普通 custom tool 的 `input` 也不会进入该规则。合法补丁保持字节语义不变。

这层修复不是通用“猜测补丁意图”。除此之外的非法 hunk、上下文不匹配、路径错误和内容错误仍由
Codex 原生 `apply_patch` 返回，模型必须根据真实错误重试。

### 4.3 SSE 流式事件

标准 function 流使用：

```text
response.output_item.added(function_call)
response.function_call_arguments.delta
response.function_call_arguments.done
response.output_item.done(function_call)
```

Codex custom tool 需要：

```text
response.output_item.added(custom_tool_call)
response.custom_tool_call_input.delta
response.custom_tool_call_input.done
response.output_item.done(custom_tool_call)
```

CodexHub 以当前请求的 `ToolNameMap` 和 `item_id`/`call_id` 识别哪些 function 其实来自 custom
tool，只改写这些事件，不会把普通 function call 错改成 custom tool。

function arguments 的 delta 可能是无法单独解析的 JSON 分片。处理规则是：

1. 若当前 delta 已是完整 `{ "patch": "..." }`，立即解包并发送 custom input delta。
2. 若只是 JSON 分片，暂时保留原事件，不猜测或拼接不完整 JSON。
3. 在 arguments done 收到完整参数后，补发一次完整的 custom input delta，再由最终 item 完成恢复。

这个兜底保证 Codex 最终一定拿到完整补丁；代价是遇到 JSON 分片时，补丁内容可能到 done 阶段才
一次性出现，而不是逐字符显示。

## 5. web_search 字段映射

Grok 上游只收到标准 Responses 顶层 hosted tool。当前规范化规则如下：

| Codex/OpenAI 字段 | Grok 字段或处理 |
| --- | --- |
| `type=web_search_preview` | `type=web_search` |
| `type=web_search` | 保持 `type=web_search` |
| `search_content_types` 包含 `image` | 若未显式设置，增加 `enable_image_search=true` |
| `filters.allowed_domains` | 原样保留 |
| `filters.blocked_domains` | 当没有 `excluded_domains` 时改名为 `excluded_domains` |
| `filters.excluded_domains` | 原样保留，优先于 `blocked_domains` |
| `external_web_access` | 移除 |
| `indexed_web_access` | 移除 |
| `search_context_size` | 移除 |
| `user_location` | 移除 |
| `search_content_types` | 完成图片搜索映射后移除 |

移除项是 Codex/OpenAI 的声明扩展，目前不属于这条 Grok hosted tool 请求的稳定字段。不能把它们
未经确认直接透传给 xAI 或兼容上游。

`web_search_call` 是上游执行过程的一部分，不会被恢复成 Codex 本地 function。已有真实请求中
Grok 已返回标准 `web_search_call`，因此当前实现不增加额外搜索执行器。

## 6. Provider 隔离

这次改动严格限定在 `ProviderType::GrokResponses`：

- `OpenAiResponses` 在工具准备入口直接返回，原生 Responses/Responses Lite 字段保持透传。
- Anthropic Messages 和 Chat Completions 继续使用现有 `additional_tools` 提取及各自转换逻辑。
- Grok 才执行 custom/namespace 到 function 的转换、工具名双向映射和 hosted search 字段规范化。
- OpenAI、Anthropic 原有 `apply_patch` 行为不变。

不要为了“统一”而让 OpenAI Responses 也经过 Grok function 转换，否则会破坏 Responses Lite
custom tool、新字段和原生 SSE 语义。

## 7. 已验证证据

本地真实请求日志曾观察到：

| 日志 ID | 现象 | 结论 |
| --- | --- | --- |
| `14061` | Grok 返回 `function_call(name=apply_patch)` | Grok 能选择并生成补丁 function call |
| `14062` | 下一轮请求已包含工具执行结果 | Codex 已在本地执行并回传结果 |
| `11462` | Grok 返回标准 `web_search_call` | Grok hosted search 可执行 |
| `14590` | Grok 返回标准 `web_search_call` | hosted search 行为可重复观察 |

日志 ID 属于当时的本地数据库，仅用于说明研发时的链路证据，不应作为长期测试 fixture。

自动化覆盖包括：

- `apply_patch` custom declaration 到 `{ patch: string }` function schema。
- Grok 专属字面量规则、正反例和参数说明完整进入 function declaration。
- 对称 Markdown 星号只在 Grok `patch` 参数的已知控制行上修复。
- 普通 custom tool 继续使用 `{ input: string }`。
- 历史 `custom_tool_call` 到 Grok `function_call` 的 `patch` 参数转换。
- 完整响应和非流式 JSON 恢复。
- 完整 JSON arguments delta 的 SSE 恢复。
- 无法单独解析的 arguments 分片在 done 阶段补发。
- `web_search_preview`、图片搜索和 domain filter 字段映射。
- OpenAI Responses 原生请求不被修改。

实现时全量结果为：`511 passed, 0 failed, 1 ignored`；GUI release check 和格式检查通过。

## 8. 维护检查清单

升级 Codex、xAI SDK、Grok Build 参考代码或兼容上游后，至少检查：

1. Codex 是否仍使用 `custom_tool_call.input` 和 `custom_tool_call_output` 承载 `apply_patch`。
2. Codex custom input SSE 的事件名和 `item_id`/`call_id` 关联方式是否变化。
3. Grok function 参数是否仍约定为 `patch`，以及 function schema 是否新增严格限制。
4. xAI `web_search` 是否新增、删除或重命名 filter、图片搜索字段。
5. 上游是否仍返回标准 `web_search_call`，而不是要求客户端二次执行。
6. `ToolNameMap` 是否仍能无碰撞恢复 custom、namespace 和普通 function 工具。
7. OpenAI Responses 早返回保护是否仍存在，避免 Grok 兼容逻辑污染原生透传。
8. 用完整 JSON delta 和拆分 JSON delta 两种 SSE fixture 同时回归。
9. 用真实 Grok 上游分别执行一次修改文件和一次联网搜索，确认不是只通过结构测试。

## 9. 当前限制

- `apply_patch` 的安全性、工作区权限和实际写盘仍由 Codex 决定，CodexHub 不执行补丁。
- hosted `web_search` 依赖目标 Grok 模型和上游账号能力；字段转换不能替代上游授权或能力开通。
- 对分片 JSON arguments 不做猜测性增量解析，可能在流末尾一次性显示完整补丁。
- `web_search` 没有本地或第二次请求 fallback；上游不支持时应保留真实错误，不能静默伪造结果。
- 本实现只解决工具协议桥接，不改变 Grok reasoning 密文、压缩或跨 Provider 会话策略。

## 10. 代码入口与参考

CodexHub：

- [`responses_lite_tools.rs`](../src/ai_gateway/responses_lite_tools.rs)：工具载体合并、Grok declaration 和 web search 映射。
- [`openai_responses.rs`](../src/ai_gateway/providers/openai_responses.rs)：Grok 历史调用和输出规范化。
- [`responses_compat.rs`](../src/ai_gateway/responses_compat.rs)：非流式及 SSE 响应恢复。
- [`apply_patch_tool.rs`](../src/ai_gateway/apply_patch_tool.rs)：共享工具说明和参数名适配。
- [`handler.rs`](../src/ai_gateway/handler.rs)：请求准备、日志统计和 Grok 工具名映射传递。

Grok Build 参考：

- `references/grok-build-main/crates/codegen/xai-grok-tools/src/implementations/codex/apply_patch/tool.rs`
- `references/grok-build-main/crates/codegen/xai-grok-tools/src/implementations/grok_build/web_search/mod.rs`
- `references/grok-build-main/crates/codegen/xai-grok-sampling-types/src/conversation.rs`

相关文档：

- [`ai-gateway-grok-build-protocol-conversion-reference.zh-CN.md`](ai-gateway-grok-build-protocol-conversion-reference.zh-CN.md)
- [ai-gateway-third-party-tool-search-plan.zh-CN.md](ai-gateway-third-party-tool-search-plan.zh-CN.md)
- [`ai-gateway-responses-lite-web-search.zh-CN.md`](ai-gateway-responses-lite-web-search.zh-CN.md)
- [xAI Web Search 官方文档](https://docs.x.ai/developers/tools/web-search)

## 11. 2026-07-25 补充：tool_search 决策

本文前面的“已实现”状态原本只覆盖 Grok apply_patch 和 hosted web_search。根据新的真实日志，
Grok 路径现已补齐 tool_search 双向翻译：请求侧把 Codex 原生 tool_search declaration、
tool_search_call、tool_search_output 降级为 Grok 普通 function 语义；回程侧把
function_call.name == tool_search 恢复为 Codex tool_search_call。

详细决策、实现入口和测试清单见：

- [ai-gateway-third-party-tool-search-plan.zh-CN.md](ai-gateway-third-party-tool-search-plan.zh-CN.md)

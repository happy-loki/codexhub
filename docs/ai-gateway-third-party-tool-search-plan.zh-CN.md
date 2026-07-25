# 第三方 Provider 的 tool_search 兼容计划

更新时间：2026-07-25

状态：已按本文 P0 范围落地。本文记录本轮已经确认并实现的方向；发版前仍建议用真实上游请求回归。

## 1. 背景

tool_search 容易和联网搜索混淆，但它们不是一回事：

| 名称 | 含义 | 执行方 |
| --- | --- | --- |
| tool_search | Codex 本地延迟工具发现器，用来查找 MCP、插件、Apps 等工具 schema | Codex 本地执行 |
| web_search | Responses hosted web search 工具 | 上游模型服务执行 |
| web.run | Codex App / Responses Lite 下的客户端 web search extension | Codex 本地 extension 执行，必要时访问 /alpha/search |

本轮只处理 tool_search。它的目标不是联网搜索，而是让第三方模型也能发现并调用 Codex 本地工具。

最新 Codex 暴露 tool_search 需要同时满足：

    model_info.supports_search_tool == true
    provider.capabilities().namespace_tools == true
    存在 ToolExposure::Deferred 的工具

ProviderCapabilities::namespace_tools 对普通配置 Provider 默认是 true；真正决定第三方模型是否看到
tool_search 的常见开关是模型目录里的 supports_search_tool。

## 2. 当前判断

### 2.1 DeepSeek

DeepSeek 走 Chat Completions 转换链路。当前转换器已经具备完整的 tool_search 降级能力：

- 顶层 tool_search declaration 转 Chat function tool。
- 历史 tool_search_call 转 assistant tool_calls。
- 历史 tool_search_output 转 tool message。
- tool_search_output.tools 转下一轮可见工具。
- Chat function call name=tool_search 转 Responses tool_search_call。

现在 DeepSeek 不能看到 tool_search 的主要原因不是转换器缺失，而是模型目录/输出目录里仍强制关闭
supports_search_tool。这个开关属于早期保守策略：当时转换链路还不完整，后来转换补齐后没有恢复。

### 2.2 Grok

Grok 走标准 OpenAI Responses，不是 Responses Lite。grok-4.5 目录里已经开启
supports_search_tool=true，所以 Codex 会把原生 tool_search 暴露出来。

当前问题是 Grok adapter 只完成了这些工具兼容：

- custom / namespace tool 转 Grok function tool。
- apply_patch custom tool 转 { patch: string } function。
- hosted web_search 字段规范化。
- Grok function call 转 Codex custom_tool_call 或 namespace function call。

还缺少 tool_search 的双向翻译。因此当会话第二轮出现 tool_search_call /
tool_search_output 历史时，Grok 上游可能把 Codex 原生 item 当作不认识的 ModelInput，触发
反序列化错误。

### 2.3 Claude / GLM

Anthropic Messages 路径已经独立适配 tool_search：

- tool_search 作为普通 Anthropic tool_use 暴露。
- tool_search_output.tools 会合并到下一轮工具列表。
- 非流式和流式回程都能恢复为 Codex 原生 tool_search_call。

本轮不改 Claude / GLM。

## 3. 决策

### 3.1 DeepSeek：重新开启 tool_search

DeepSeek 的目标改动：

1. 移除模型 catalog 输出阶段对 DeepSeek 的 supports_search_tool=false 强制覆盖。
2. 将 deepseek-v4-pro 和 deepseek-v4-flash 对 Codex 暴露为 supports_search_tool=true。
3. 保持 DeepSeek 其它严格约束不变：
   - developer 转 system。
   - thinking 参数清理。
   - json_schema 转 json_object。
   - assistant tool_calls 补 reasoning_content。
   - 文本-only / image support 限制按当前策略继续保守。

验收要求：

- 普通 DeepSeek 对话不回归。
- 普通 function tool call 不回归。
- tool_search -> tool_search_output -> 动态 namespace tool 完整链路可用。
- 真实 DeepSeek 上游至少跑一次含 MCP/插件延迟工具发现的任务。

### 3.2 Grok：做 tool_search 双向翻译

Grok 的目标不是让 xAI 原生理解 Codex 扩展，而是在 CodexHub 内部把 tool_search 映射为
Grok 能接受的普通 function。

请求侧：

| Codex 入站 | 发给 Grok |
| --- | --- |
| tools 中 type=tool_search | type=function, name=tool_search |
| input 中 type=tool_search_call | type=function_call, name=tool_search |
| input 中 type=tool_search_output | type=function_call_output，output 为工具发现结果 JSON 文本 |
| tool_search_output.tools | 解析后合并进当前请求顶层 tools，再按 Grok function 规则转换 |
| tool_choice 指向 tool_search | 转为 Grok function tool_choice |

回程侧：

| Grok 返回 | 返回给 Codex |
| --- | --- |
| function_call.name == tool_search | type=tool_search_call, execution=client |
| function arguments string/object | 恢复为 tool_search_call.arguments 对象 |
| SSE function call delta / done | 保持现有 SSE 兼容逻辑，并在 done 阶段恢复为 Codex 可执行的 tool_search_call |

关键约束：

- 只在 ProviderType::GrokResponses 下启用，不污染 OpenAI Responses 原生透传。
- 不把 Grok 声明为 Responses Lite。
- 不改变 hosted web_search 的语义；Grok web_search 仍由上游执行。
- 不把 tool_search 和 web.run 合并。web.run 是客户端 extension，不属于本轮。
- 继续使用 request-scoped ToolNameMap，避免 namespace 名称和普通 function 名称冲突。

## 4. 推荐实现入口

DeepSeek：

- src/ai_gateway/models.json
- src/ai_gateway/catalog.rs
- src/ai_gateway/transform/responses_to_chat.rs
- src/ai_gateway/transform/chat_to_responses.rs
- src/ai_gateway/transform/responses_stream.rs

Grok：

- src/ai_gateway/responses_lite_tools.rs
  - 增加 type=tool_search declaration 到 function declaration 的转换。
  - 从 tool_search_output.tools 提取动态工具并合并到顶层 tools。
- src/ai_gateway/providers/openai_responses.rs
  - 增加 Grok 请求历史中 tool_search_call / tool_search_output 的 ModelInput 兼容。
- src/ai_gateway/responses_compat.rs
  - 增加 Grok function call 回程到 Codex tool_search_call 的恢复。
  - 补 SSE 事件恢复覆盖。
- src/ai_gateway/tool_names.rs
  - 复用现有 ToolCallKind::ToolSearch 和 ToolNameMap::encode_tool_search()。

## 5. 测试清单

最少需要补这些测试：

- DeepSeek catalog 输出不再强制关闭 supports_search_tool。
- DeepSeek Chat request 中 tool_search declaration 转为 function。
- DeepSeek 非流式 function call tool_search 恢复为 Responses tool_search_call。
- DeepSeek streaming function call tool_search 恢复为 Responses SSE。
- Grok declaration：type=tool_search 转 type=function,name=tool_search。
- Grok 历史：tool_search_call 转 function_call。
- Grok 历史：tool_search_output 转 function_call_output。
- Grok 动态工具：tool_search_output.tools 合并后可被转换为 Grok function。
- Grok 回程：function_call.name=tool_search 转 tool_search_call。
- Grok SSE：arguments delta / done 不破坏现有 apply_patch 和普通 function。
- OpenAI Responses raw passthrough 不被改写。
- Anthropic/GLM 现有 tool_search 路径不回归。

## 6. 发布说明口径

面向用户可以这样描述：

> 改进第三方模型的 Codex 工具发现能力。DeepSeek 将支持 Codex 本地工具搜索；
> Grok 将通过 CodexHub 做 tool_search 双向协议翻译，从而更稳定地发现并调用 MCP、插件和
> Codex 本地工具。联网搜索 web_search / web.run 不在本次改动范围内。

## 7. 暂不做

- 不为 Grok 增加本地 web.run 执行器。
- 不把 hosted web_search 转成 tool_search。
- 不改 OpenAI Responses / Responses Lite 原生请求字段。
- 不推进完整统一 IR 重构；本轮只做小范围协议补洞。

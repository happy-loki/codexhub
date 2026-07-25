CodexHub v0.4.12

本次版本重点完善第三方模型对 Codex 本地工具发现 `tool_search` 的支持，修复 Grok 在工具发现后的后续轮次中可能出现协议不兼容的问题。

## DeepSeek 工具发现

- 为 `deepseek-v4-pro` 和 `deepseek-v4-flash` 开启 Codex 本地工具搜索能力。
- 复用现有 Responses 与 Chat Completions 双向转换，支持工具声明、工具发现结果、动态 namespace 工具以及流式工具调用。
- 保持 DeepSeek 现有 developer、thinking、JSON 输出和 reasoning 兼容策略不变。

## Grok 双向协议兼容

- 将 Codex 原生 `tool_search` 声明和 `tool_choice` 转换为 Grok 可接受的普通 function。
- 将 `tool_search_output.tools` 合并到下一轮工具列表，并继续处理 namespace 和自定义工具名称。
- 将历史 `tool_search_call` / `tool_search_output` 转换为 Grok Responses 可接受的 `function_call` / `function_call_output`。
- 将 Grok 返回的 `function_call(name=tool_search)` 在 JSON 和 SSE 回程中恢复为 Codex 原生 `tool_search_call`。
- 修复工具发现完成后第二轮请求可能因 Grok 不认识 Codex 原生 item 而失败的问题。

## 协议边界

- OpenAI Responses 和 Responses Lite 继续保持原生透传，不因第三方适配丢失新字段。
- `tool_search` 仅表示 Codex 本地延迟工具发现，不等同于 hosted `web_search` 或客户端 `web.run`。
- 本次版本不改变现有联网搜索和图片生成策略。

## 文档

- 新增第三方 Provider `tool_search` 兼容方案文档。
- 补充 Grok Responses 工具适配和 Provider Adapter 设计说明。

## 验证

- `cargo fmt` 通过。
- AI Gateway 全部 354 个相关测试通过。
- Grok 工具声明、历史回放、JSON 回程和 SSE 回程均已增加专项测试。

## 当前边界

- 本版完成网关级协议转换和自动化测试；不同第三方上游对 function schema 的实现仍可能存在差异，建议按实际渠道进行端到端验证。
- hosted `web_search`、`web.run` 与 `tool_search` 保持彼此独立，不进行语义混用。

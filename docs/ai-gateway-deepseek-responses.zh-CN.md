# DeepSeek Responses API 接入说明

本文记录 CodexHub 对 DeepSeek 原生 Responses API 的接入边界。协议依据为
[DeepSeek Responses API 官方指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)。

## DeepSeek Responses

DeepSeek Pro 已支持原生 Responses API。CodexHub 的 GUI 默认使用
`deepseek_responses`，不再把 DeepSeek Pro 配置到 Chat Completions 入口。

底层 `chat_completions` Provider 和 Responses -> Chat 转换代码暂时保留，主要用于
其它仍只提供 Chat Completions 的兼容厂商；已有旧版 DeepSeek Chat 配置也不会被自动删除，
但新配置应优先使用 DeepSeek Responses。

## 默认配置

```text
name: deepseek-responses
baseUrl: https://api.deepseek.com/v1
model: deepseek-v4-pro
```

官方模型目录同时包含 `deepseek-v4-flash` 和 `deepseek-v4-pro`。CodexHub 默认填入
`deepseek-v4-pro`；如果账号或上游渠道只提供 Flash，可以在模型列表中手动选择
`deepseek-v4-flash`。

当前官方目录中的两款模型均声明：

- 上下文窗口：`372,000`
- 有效上下文比例：`95%`
- `comp_hash`：`3000`
- `use_responses_lite`：`false`
- `prefer_websockets`：`false`
- 最低 Codex 客户端版本：`0.144.0`

CodexHub 仅保留一项本地产品覆盖：Flash 和 Pro 的 `availability_nux.message`
继续使用 CodexHub 原有的 DeepSeek 中文提示。其余模型能力字段跟随官方目录。

## 请求处理

CodexHub 将请求发送到 `POST /v1/responses`，保留原生 Responses 字段和 SSE 事件。
为了兼容 Codex Responses Lite 请求，仅执行以下必要处理：

1. 将 `input[].type = "additional_tools"` 中的工具提升到顶层 `tools`。
2. 移除已被提升的 `additional_tools` carrier，避免它作为模型输入发送。
3. 保持 hosted `web_search` 原始声明，不按 OpenAI Lite 规则删除。
4. 保持 `custom apply_patch` 原始声明，不转换成 Grok function 格式。
5. 保持普通 function 工具原始声明。
6. 使用 `deepseek_responses` 独立密文作用域，不与 OpenAI 或 Grok 密文混用。

DeepSeek 自动管理提示缓存，因此 CodexHub 不为该渠道注入：

- `prompt_cache_key`
- `prompt_cache_retention`

## 官方能力边界

当前支持：

- function 工具
- hosted `web_search` / `web_search_2025_08_26`
- custom `apply_patch`
- reasoning text SSE 事件
- output text SSE 事件

当前不支持：

- `previous_response_id`
- `conversation`
- `store`
- `background`
- `metadata`
- `include`
- 图片和文件输入
- `reasoning.encrypted_content`
- OpenAI Responses Compact V2

DeepSeek 对部分未知普通参数会静默忽略。CodexHub 第一版不主动删除未来字段，
避免在 Responses 原生透传路径上制造不必要的协议损失。

## 暂未适配的 Codex 工具

DeepSeek 当前没有声明支持 Codex 的 `namespace` 和 `tool_search` 类型。第一版不会套用
Grok 的全套工具翻译，因为这会改变 DeepSeek 已原生支持的 `apply_patch` 和
`web_search` 语义。

后续若要支持 DeepSeek tool search，需要单独实现并测试双向转换：

- 请求：`tool_search` 转 function
- 请求：namespace 工具名扁平化
- 响应：function call 恢复为 `tool_search_call`
- 响应：扁平工具名恢复 namespace

## 压缩策略

DeepSeek 官方当前不提供 OpenAI 的 `/responses/compact` 协议。CodexHub 不伪造
DeepSeek Compact 响应，也不会把 OpenAI 私有压缩密文发给 DeepSeek。需要压缩时，
应继续使用 Codex/CodexHub 已有的本地摘要路径。

## 升级核对清单

DeepSeek 文档或模型更新后，至少核对：

1. 新增可用模型及上下文窗口。
2. Flash 和 Pro 的账号开放范围是否发生变化。
3. tool search、namespace、图片和文件输入是否开放。
4. `previous_response_id`、conversation 和 store 是否开放。
5. SSE 是否新增或调整事件类型。
6. 是否新增 Compact 或密文 reasoning 协议。

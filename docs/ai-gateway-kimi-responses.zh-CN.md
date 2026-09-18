# Kimi Responses 接入

## 范围与原因

Kimi 原生提供 Responses API。CodexHub 只增加一个 Kimi 渠道，复用现有
Responses 转发链路，不经过 Chat Completions 或 Anthropic Messages 转换。

不区分按量 API、Coding Plan 或第三方服务，不通过域名识别套餐，不自动切换
地址或模型。用户填写 Base URL、API Key，并拉取或手动添加上游模型；模型名
不一致时使用现有模型映射功能。

## 配置

- 渠道类型：`kimi_responses`。
- 新建时默认 Base URL：`https://api.moonshot.cn/v1`，可自行修改。
- 默认上游模型及 Codex 可见模型：`kimi-k3`。
- 请求地址：Base URL 去掉末尾 `/v1` 后追加 `/v1/responses`。
- 认证：`Authorization: Bearer <API Key>`。
- 模型拉取：沿用现有 `/models` 拉取逻辑以及可选的独立模型列表 URL，
  不增加跨域官方地址回退。Kimi 渠道不提供 `k3-256k` / `kimi-k3-256k`
  选项，拉取时排除这两个名称（含厂商路径前缀），其它第三方模型名称保留。

官方地址示例，仅供配置参考，不参与程序分支判断：

| 服务 | Base URL | 上游模型示例 |
| --- | --- | --- |
| 按量 API | `https://api.moonshot.cn/v1` | `kimi-k3` |
| Kimi Code 国内 | `https://api.kimi.com/coding/v1` | `k3` |
| Kimi Code 海外 | `https://api.kimi.ai/coding/v1` | `k3` |

不同服务的 Key 和模型名不能假定通用。比如上游实际模型为 `k3`，可以将
Codex 可见的 `kimi-k3` 映射到 `k3`；网关不擅自改写模型 ID。

## 协议边界

1. 保留原生 function、namespace、apply_patch custom 工具及其历史、图片、
   reasoning、encrypted_content 和未知扩展字段；不套用 Grok 的工具降级逻辑。
2. Kimi 支持服务端 `web_search`，保留搜索事件和引用。仅删除该工具明确不支持的
   `search_context_size`，包括动态工具声明中的同名参数。
3. 不额外注入 OpenAI 的 `prompt_cache_key` / `prompt_cache_retention`。
   调用者显式传入的扩展字段仍保持透传。
4. K3 模型元数据不启用 Responses Lite、code mode 或 client tool_search；
   普通工具、原生 apply_patch 和 hosted web_search 走 Responses。
5. 原生密文沿用现有 Responses 策略，不增加前缀，不尝试解密；跨模型压缩和
   不兼容密文的处理不在本次接入中重新设计。
6. 不宣称支持 OpenAI 的远程压缩、生图和 `/alpha/search` 接口。

K3 模型配置参考用户提供的官方 `kimi-k3.json` 中 `k3` 条目，思考等级为
`low/high/max`，支持文本和图片。CodexHub 使用 `kimi-k3` 作为可见模型 ID，
并按产品要求将 `context_window` 和 `max_context_window` 都设为 `372000`，
不使用官方的 1M 默认值，也不额外增加 `k3-256k` 可见模型。

本次不支持 256K K3，用户应使用容量至少满足 372K 的上游。网关不判断
套餐，无法从单独一个 `k3` 名称推断实际额度或提升真实上下文上限。

## 基础指令的源码结论

用户提供的官方示例使用 `base_instructions: ""`，但空字符串不表示“自动使用
Codex 默认提示词”。以下依据本地最新 Codex 源码：

- `protocol/src/openai_models.rs` 的
  `deserialize_model_infos_with_legacy_base` 会把存在的旧字段（包括空字符串）
  提升为 `model_messages.instructions_template`。
- `ModelInfo::get_model_instructions` 原样返回模板，空字符串仍为空，
  不调用通用基础指令。两个指令字段都缺失时，模型目录解析会报错。
- `models-manager/src/manager.rs` 只有找不到模型元数据时才调用
  `model_info_from_slug`；后者使用 `models-manager/prompt.md` 作为通用指令。
- 会话级配置或已保存会话中的基础指令可能覆盖模型模板。

按用户确定的方案，K3 的 `base_instructions` 原样复制内置
`deepseek-v4-pro` 的同名字段；DeepSeek Pro 和 Flash 当前使用相同内容。
不改写提示词语义，不复制 DeepSeek 的其它模型参数。K3 未另设
`model_messages.instructions_template`，由 Codex 将该非空旧字段提升为模板。
回归测试检查 K3 基础指令非空，且与两个 DeepSeek 模型一致。

## 验证

回归测试覆盖配置读写、任意 Base URL 的路径拼接、Bearer 认证、模型映射、
工具及动态声明保留、搜索参数清理、原生密文和 JSON/SSE 响应。
现有 OpenAI 请求必须保持原样；不能为了 Kimi 修改 OpenAI Lite 的字段。

真实服务验证优先使用用户持有的按量 API Key。没有真实请求成功前，不将
单元测试或模拟上游测试描述为已通过官方服务验证；Coding Plan 和第三方
服务需要分别验证自己的 Key、模型权限和兼容能力。

## 官方资料

- [按量 API 的 Codex 指南](https://platform.kimi.com/docs/guide/codex-kimi)
- [Responses API](https://platform.kimi.com/docs/api/responses)
- [Kimi Code 的 Codex 指南](https://www.kimi.com/code/docs/en/third-party-tools/codex.html)

核对日期：2026-09-18。

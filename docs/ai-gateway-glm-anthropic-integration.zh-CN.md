# AI Gateway 智谱 Anthropic Messages 对接说明

更新时间：2026-08-27

状态：已落地首版。

本文记录 `codexhub` AI Gateway 如何通过 Anthropic Messages 协议接入智谱普通 API 和 Coding Plan。两种套餐共用一个智谱入口，服务类型由渠道里的 Radio 选择决定。

相关文档：

- [`ai-gateway-anthropic-first-roadmap.zh-CN.md`](ai-gateway-anthropic-first-roadmap.zh-CN.md)：Anthropic Messages 优先路线。
- [`ai-gateway-anthropic-messages.zh-CN.md`](ai-gateway-anthropic-messages.zh-CN.md)：Anthropic Messages adapter 设计。
- [`ai-gateway-web-search-protocol.zh-CN.md`](ai-gateway-web-search-protocol.zh-CN.md)：Codex Responses 与 Anthropic/GLM web search 的对接规则。
- [`provider-logo-assets.zh-CN.md`](provider-logo-assets.zh-CN.md)：provider logo 资源维护方式。

官方参考：

- 智谱 Claude API 兼容说明：<https://docs.bigmodel.cn/cn/guide/develop/claude/introduction>
- 智谱 Coding Plan 模型配置说明：<https://docs.bigmodel.cn/cn/guide/develop/codingplan/model>

## 1. 接入结论

智谱普通 API 和 Coding Plan 统一使用 Anthropic Messages。智谱不再提供单独的 Chat Completions 配置入口，也不需要为智谱维护 Responses 到 Chat 的兼容分支。

配置形态：

```toml
[[aiGateway.providers]]
name = "zai"
enabled = true
providerType = "anthropic_messages"
compatibility = "glm_anthropic"
zaiAccessMode = "api"
baseUrl = "https://open.bigmodel.cn/api/anthropic"
apiKey = "..."
models = ["GLM-5.3", "GLM-5.3-Flash"]
```

关键约束：

- `providerType` 表示协议族，智谱使用 `anthropic_messages`。
- `compatibility` 表示厂商 profile，智谱使用 `glm_anthropic`。
- `zaiAccessMode` 只能是 `api` 或 `coding_plan`，分别表示智谱普通 API 和 GLM Coding Plan。
- `baseUrl` 只表示 Anthropic 对话入口；`modelsUrl` 只表示模型列表入口。
- `modelAliases` 表示 Codex 侧模型名到上游模型名的映射。
- GUI 新建智谱渠道时只生成 `glm_anthropic`。
- `zhipu_anthropic` 作为历史配置别名保留，便于迁移到同一 Anthropic profile。

通用 `ChatCompletions` provider 仍可服务于其它只提供 Chat 接口的厂商，但它不是智谱入口，也不会被智谱配置使用。

## 2. 两个独立接口

### 2.1 Anthropic 对话接口

智谱 Anthropic 兼容 base URL 为：

```text
https://open.bigmodel.cn/api/anthropic
```

Gateway 会在该地址后拼接 `/v1/messages`：

```text
https://open.bigmodel.cn/api/anthropic/v1/messages
```

请求使用：

```http
Authorization: Bearer <apiKey>
anthropic-version: <ANTHROPIC_VERSION>
```

### 2.2 模型列表接口

模型列表不从 Anthropic 对话地址推导。GUI 使用 API Key，按 `zaiAccessMode` 独立请求对应的智谱目录：

| `zaiAccessMode` | 中国区目录 | 国际区目录 |
| --- | --- | --- |
| `api` | `https://open.bigmodel.cn/api/paas/v4/models` | `https://api.z.ai/api/paas/v4/models` |
| `coding_plan` | `https://open.bigmodel.cn/api/coding/paas/v4/models` | `https://api.z.ai/api/coding/paas/v4/models` |

```text
GET https://open.bigmodel.cn/api/paas/v4/models
```

国际站对应地址为：

```text
https://api.z.ai/api/paas/v4/models
```

选择 `api` 时只尝试普通 API 目录，选择 `coding_plan` 时只尝试 Coding Plan 目录；当前 `baseUrl` 所在区域优先，失败后再尝试另一区域。HTTP 错误、无效 JSON 或空模型列表会触发下一个区域候选地址；第一个非空列表成功后停止。

目录请求成功后，只把模型 ID 写入模型表，不会把目录地址写回对话 `baseUrl`。

## 3. Cline 参考与取舍

Cline 将普通 Z.AI 和 Coding Plan 建成两个 provider ID，并分别保存区域地址。CodexHub 采用一个“智谱 Anthropic（API / Coding Plan）”入口，再用二级 Radio 选择服务类型，因为两者的对话协议相同，差异主要在额度和模型目录。

因此：

- 对话始终走 Anthropic Messages。
- 模型列表只探测所选服务类型的目录。
- 不新增智谱 Chat Completions 入口。
- 通用 Chat 适配器只保留给其它厂商和历史配置。

## 4. GLM 响应差异

智谱与 Anthropic 共用鉴权、版本头、endpoint、usage 和 SSE 基础形态。需要 profile 单独处理的差异主要是 web search 回包：

- Codex / Responses 侧仍按标准 `web_search` 能力表达。
- Anthropic Messages 出站请求构造 server tool `web_search_20250305`。
- 智谱回包可能使用 `server_tool_use.name = "web_search_prime"`。
- 搜索结果可能使用 `tool_result`，而不是 `web_search_tool_result`。
- 可能出现 `Z.ai Built-in Tool: web_search_prime` 等私有过程文本。

Gateway 只在 `glm_anthropic` profile 内识别这些差异，最终向 Codex 输出标准 `web_search_call`，不泄漏智谱私有字段。流式场景会在合适的 block stop 或 response done 时清理私有过程文本。

## 5. 代码落点

```text
src/ai_gateway/config.rs
src/ai_gateway/providers/anthropic_messages/options.rs
src/ai_gateway/providers/anthropic_messages/request.rs
src/ai_gateway/providers/anthropic_messages/glm_compat.rs
src/gui.rs
src/gui/ai_gateway.rs
```

约定：

- `ProviderConfig::models_url` 不参与对话路由。
- `ProviderConfig::zai_access_mode` 只影响智谱模型目录选择，不改变 Anthropic 对话 URL。
- `AnthropicProviderProfile` 使用白名单；未知 profile 明确返回错误。
- 智谱目录探测只在 `anthropic_messages + glm_anthropic/zhipu_anthropic` 组合下启用。
- URL 只接受 `http` 和 `https`。
- API Key 只放在 `Authorization` 请求头，不拼进 URL。

## 6. 验证清单

```powershell
cargo fmt --all
cargo test ai_gateway
cargo check
cargo check --features gui
git diff --check
```

至少覆盖：

- 智谱配置反序列化为 `AnthropicMessages`。
- `glm_anthropic` 和 `zhipu_anthropic` 映射到同一 profile。
- 对话 URL 为 `/api/anthropic/v1/messages`。
- 中国和国际模型目录候选顺序正确。
- `api` 只使用普通 API 目录，`coding_plan` 只使用 Coding Plan 目录。
- 模型目录请求不会覆盖对话 `baseUrl`。
- GLM web search 非流式和流式回包可转换为 Responses `web_search_call`。

## 7. 后续新增 Anthropic 厂商

1. 确认官方 base URL、鉴权 header、版本 header、SSE 和 tool 格式。
2. 在 `AnthropicProviderProfile` 增加显式 profile。
3. 在 `from_compatibility()` 增加白名单字符串。
4. 只有实测存在协议差异时，才增加 response 或 stream 分支。
5. GUI 需要一键新增时，增加一个入口；不要为同一协议的套餐复制多个入口。
6. 用真实 API 做最小 smoke test，再决定是否打开更多能力。

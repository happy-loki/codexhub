# OpenAI / ChatGPT Responses WebSocket

## Why

OpenAI Responses API Key 渠道和 ChatGPT 账号渠道需要支持 Codex 的 Responses WebSocket，复用一条连接完成多轮
`response.create`，并保留 `previous_response_id` 和 Responses Lite 的原生字段。
只改 `supports_websockets` 不够：网关必须处理同一路径的 GET Upgrade，原有 POST
继续提供 HTTP/SSE。

Codex 建立连接时不一定携带模型名，因此不能在握手阶段直接选择上游渠道。
网关等第一条 `response.create` 再按原有路由规则选渠道。

## What

- ChatGPT 账号渠道：原生 WebSocket 转发，沿用 OAuth 凭证、账号头及一次 401 刷新。
- OpenAI Responses 渠道：对配置的 Base URL 的 `/v1/responses` 尝试原生 Upgrade，
  使用该渠道 API Key，不将 Codex 发来的本地 dummy-token 当成上游凭证。
- DeepSeek、Grok、Kimi、Anthropic、Chat Completions 专用渠道保留 HTTP/SSE，
  不实现 WS 转 HTTP 的兼容桥。即使都名为 Responses，也不假定其他厂商支持该传输。
- Codex 第三方 provider 显式配置 `supports_websockets = true` 才启用。
  不改变默认值，不后台改写用户配置；连接失败后的重试与 SSE 回退由 Codex 负责。
- 不改变远程压缩、工具注册、账号身份策略，不采集或筛选 turn-state。

使用时，在「Codex 接入 → Codex 初始化」勾选「优先使用 WebSocket」。
开关读写 Codex 已有的 `[model_providers.ai-gateway].supports_websockets`，
保存后重新打开 Codex 客户端。默认关闭、未初始化或本地服务不可用时置灰；
保存时禁用重复点击，接口返回后校验实际值，失败恢复原勾选并显示错误。
不需要更改 `base_url`、provider 名称或 `requires_openai_auth`。
重新初始化保留现有开关选择；首次无配置时仍为 `false`，显式指定值优先。

## How

1. 复用 CodexHub 出站 HTTP 客户端进行 HTTP/1.1 Upgrade，再接管升级后的流。
   因此直连、系统代理、自定义代理和 TLS 策略与 HTTP 请求一致。
2. 握手使用新的 WebSocket key，校验服务端 accept，不转发客户端握手 key 或扩展。
   ChatGPT 凭证仅发送到已验证的官方后端；OpenAI API Key 仅用于其配置的渠道地址。
3. 原生请求使用 JSON Value 保留 Lite 和未知字段；沿用图片工具过滤、模型映射、
   缓存键和历史密文处理。继续透传上游事件，绝不自动重发已发送的推理请求。
4. 按当前权重和会话粘性选定渠道后才尝试握手；不会为寻找 WS 绕过用户的路由
   优先级或同时向所有渠道探测。一条连接内的增量请求固定使用原渠道和凭证。
   切换渠道、账号或 API Key 时，要求完整上下文，不回放旧 `previous_response_id`。
5. 原样转发 `generate:false` 预热，不本地伪造响应或保存一份完整会话历史。
   单连接按 Codex 的顺序请求工作，不实现并发生成流。
6. 每次原生 `response.create` 单独记录请求、响应事件、首字延迟、用量和结束状态；
   断线不会留下永久“运行中”的日志。日志中的事件沿用 SSE 展示格式，网络仍为 WS。
7. 客户端提前完成本地握手，不能事后补写 HTTP 101 的头。官方握手中的 Codex
   响应元数据放入首条响应事件的 `headers`，已有事件头优先；不伪造元数据。
8. 单条 WebSocket 消息上限 64 MiB，握手总等待最长 30 秒（渠道超时更短则用更短值），
   写入超时 30 秒，生成期间按渠道配置控制
   上游事件等待超时；日志事件截取上限 512 KiB。保留日志终帧用量与结束状态。

## 回退边界

未配置启用的 OpenAI / ChatGPT 渠道时，GET 直接返回 HTTP 426，Codex 立即回退 HTTP。
混合渠道时握手不一定带模型名；如果升级后的首条请求实际选中了其他协议渠道，
网关关闭连接，不发起推理，由 Codex 走流失败重试和最终 HTTP 回退。这可能先显示
重连提示，不能把“升级后的 JSON 426”当成握手阶段的立即回退。

OpenAI / ChatGPT 上游握手拒绝、超时或校验失败时，保存真实失败原因后关闭下游
连接，让 Codex 决定重试和回退。不把上游握手 400/404/405 等包装成普通模型错误
帧，否则 Codex 可能直接结束任务。已发送生成帧后不自动换渠道，也不自行改用 SSE。
不支持 WS 的 4xx 不触发渠道故障熔断，以免影响随后正常的 HTTP 请求；429/5xx
仍沿用原有健康判断。无论开关如何，原 POST Responses 入口都可用。

## 验证

以本地模拟服务测试握手认证、连接复用、Lite 字段、增量工具历史、
断线取消和逐轮日志。真实官方账号和网络链路需另行验证，不以模拟测试替代。

2026-09-21：新增 9 项测试，覆盖 OAuth 握手 401 刷新及账号头覆盖、Lite 与
未知字段保留、响应元数据优先级、HTTP 426 与原 POST 路由、混合渠道拒绝、
切换账号后的增量请求拒绝、实际代理转发、错误握手校验、连续多轮及取消日志。
`cargo test --locked --features gui --bin codexhub --quiet`：767 项通过、
2 项忽略、0 项失败。未编译 Codex 源码，未修改运行中客户端的配置。

2026-09-22：扩展 OpenAI Responses 渠道并增加 GUI 开关。新增本地真实 Upgrade
测试：API Key 覆盖客户端令牌、自定义 Base URL 路径、模型映射、两轮连接复用；
400/401/404/405/426 拒绝握手时关闭下游、不自动 POST，随后客户端发起 HTTP
仍成功；初始化保留已开启选项且可以显式关闭。完整 GUI 功能测试为 770 项通过、
2 项忽略、0 项失败。真实上游能力和桌面窗口交互尚未人工验证。

参考：Codex `core/src/client.rs`、`core/src/responses_retry.rs`、
`codex-api/src/endpoint/responses_websocket.rs`，
[官方 WebSocket 模式文档](https://developers.openai.com/api/docs/guides/websocket-mode)。

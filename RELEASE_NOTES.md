CodexHub v0.3.30

本次版本重点优化 AI Gateway 请求日志，降低默认日志记录带来的 CPU、内存和日志详情页压力：

- 将 AI Gateway 请求日志拆成“摘要日志”和“详情日志”：默认只记录状态、耗时、TTFT、token 用量、上游请求体大小等轻量指标。
- 新增“记录详情 / Record details”开关；只有开启详情后才保存请求体、请求头、上游请求、上游 SSE 和响应 JSON。
- 关闭请求日志时会自动关闭详情日志，避免后台继续写入重 payload。
- Anthropic、DeepSeek 和 OpenAI Responses provider 均按详情开关控制上游请求和响应内容落库。
- 增加 metadata-only 性能观测：记录 AI Gateway in-flight 数量、路由耗时，以及请求日志 SQLite 锁等待和持有时间，不记录用户内容。
- 补充请求日志单元测试，覆盖详情关闭时仍保留摘要指标、但跳过 SSE/response payload 保存的场景。

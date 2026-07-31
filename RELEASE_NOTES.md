CodexHub v0.4.16

本次版本新增 DeepSeek 官方 Responses API 接入，同步官方模型能力，并优化 DeepSeek 两种协议渠道的模型获取体验。

## DeepSeek Responses 原生接入

- 新增独立的 `DeepSeek Responses` 渠道，原生转发至 DeepSeek `/v1/responses`。
- 支持官方 hosted web search、function tools、custom `apply_patch` 及对应 SSE 流式事件。
- 兼容 Codex Responses Lite 的 `additional_tools` 结构，将工具提升到 DeepSeek 支持的顶层 `tools`。
- 不向 DeepSeek 注入 OpenAI 专用的 `prompt_cache_key` 与 `prompt_cache_retention`。
- 使用独立的 `deepseek_responses` 密文作用域，避免与 OpenAI、Grok 的协议状态混用。

## DeepSeek 模型目录

- 根据 DeepSeek 官方目录同步 `deepseek-v4-pro` 与 `deepseek-v4-flash` 的上下文、推理档位、客户端版本和能力字段。
- 保留 CodexHub 原有的 DeepSeek `availability_nux` 中文提示。
- `DeepSeek Chat` 获取模型时只展示 `deepseek-v4-pro`。
- `DeepSeek Responses` 获取模型时只展示 `deepseek-v4-flash`。
- 上述筛选仅作用于 GUI 的“获取模型”结果；手工添加、配置文件和 AI Gateway 实际路由不受限制。

## 兼容性

- 现有 `DeepSeek Chat / Chat Completions` 渠道保持不变，不会自动迁移或改写。
- OpenAI Responses、Grok Responses 和 Anthropic Messages 渠道的模型获取结果不受影响。
- 增加 DeepSeek Responses 配置、原生请求处理、Lite 工具提升和模型列表筛选的回归测试。

## 验证

- `cargo fmt --check` 通过。
- `cargo check --features gui --bin codexhub` 通过。
- DeepSeek 专项测试通过：27 passed。
- Responses Lite 工具专项测试通过：8 passed。
- GitHub Actions 将在各平台执行干净环境构建并生成安装包。

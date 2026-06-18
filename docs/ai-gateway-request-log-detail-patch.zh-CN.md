# AI Gateway 请求日志详情补丁说明

更新时间：2026-06-18

本文记录请求日志详情弹窗和 JSON 查看器补丁的具体实现。这个补丁的目标是让 AI Gateway 调试链路能直接查看三类关键数据：Codex 原始请求、转换后的上游请求、上游/网关返回内容。

## 背景

请求日志列表已经能展示 `id`、`model id`、`stream`、`channel`、`status`、`tokens`、cache、cost、TTFT、latency、created at 等概要字段，但列表不适合排查协议转换问题。

排查 DeepSeek ChatCompletions 与 OpenAI Responses 相互切换问题时，最需要对比的是：

- Codex 发送给 `/ai-gateway/v1/responses` 的原始 Responses 请求。
- AI Gateway 转换后发给上游渠道的请求。
- 上游返回后，AI Gateway 记录到的最终响应或错误。

因此本补丁增加详情查询接口和 GUI 详情弹窗，并把详情内容做成可折叠的 JSON 查看器。

## 用户可见变化

请求日志列表新增详情入口。点击某条日志的详情后，GUI 会打开一个可调整大小的弹窗。

弹窗结构：

- 顶部显示当前日志概要：`id`、`model`、`channel`、`protocol`、`stream`、`status`、`tokens`、`ttft`、`latency`、`created at`。
- 主体使用 notebook tab 展示：
  - Codex 请求
  - 上游请求
  - 响应
  - 错误信息（仅失败日志有）
- JSON tab 使用 `StyledTextCtrl` 展示，不再保留左侧树形栏。
- JSON 查看器支持语法高亮、行号、折叠 margin、双击折叠行。
- 顶部提供“展开代码 / 折叠代码”按钮，用于批量展开或折叠所有 JSON 子元素。

## 后端落库

请求日志仍使用 SQLite，数据库文件名：

```text
ai-gateway-request-logs.sqlite
```

数据库路径由 `request_log::database_path(config)` 计算，默认放在 `state_path` 同级目录。

核心表：

```sql
CREATE TABLE IF NOT EXISTS ai_gateway_request_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL,
    model_id TEXT NOT NULL,
    stream INTEGER NOT NULL,
    channel TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    status TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    total_tokens INTEGER,
    read_cache_tokens INTEGER,
    read_cache_hit_rate REAL,
    write_cache_tokens INTEGER,
    cost_usd REAL,
    latency_ms INTEGER,
    ttft_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    error_message TEXT,
    request_json TEXT,
    upstream_request_json TEXT,
    response_json TEXT
);
```

本次补丁新增或强化的详情字段：

- `request_json`：Codex 原始请求体。
- `upstream_request_json`：转换后发给上游 provider 的请求体。
- `response_json`：最终响应或 completed response 对象。
- `ttft_ms`：首个 token 类 SSE 事件出现的耗时。
- `latency_ms`：请求完成或失败的总耗时。

兼容旧库时会尝试执行：

```sql
ALTER TABLE ai_gateway_request_logs ADD COLUMN upstream_request_json TEXT
```

如果列已经存在，会忽略 duplicate column 错误。

## 接口

列表接口：

```http
GET /ai-gateway/request-logs?limit=200
```

详情接口：

```http
GET /ai-gateway/request-logs/{id}
```

详情接口返回结构：

```json
{
  "log": {
    "id": 1,
    "requestId": "...",
    "modelId": "...",
    "stream": true,
    "channel": "...",
    "providerType": "chat_completions",
    "status": "completed",
    "requestJson": "{...}",
    "upstreamRequestJson": "{...}",
    "responseJson": "{...}"
  }
}
```

`requestJson`、`upstreamRequestJson`、`responseJson` 是字符串形式保存的 JSON。GUI 读取后会重新 parse 并 pretty print；如果 parse 失败，则按原始字符串展示。

## Provider 侧记录点

OpenAI Responses provider：

- 上游请求基本透传 Responses 结构。
- 发送前把上游请求体序列化写入 `upstream_request_json`。
- 非流式完成时写入 usage、latency 和 response。
- 流式时通过 SSE 观察器记录 TTFT 和最终 completed response。

DeepSeek ChatCompletions provider：

- 发送前先完成 Responses -> ChatCompletions 转换。
- 转换后的 chat body 写入 `upstream_request_json`。
- 上游响应再转换回 Responses 语义后写入日志。
- DeepSeek cache usage 会映射到请求日志的 read/write cache 字段。

## GUI 实现

详情弹窗实现文件：

```text
src/gui/request_log_detail.rs
```

列表和异步详情加载逻辑在：

```text
src/gui.rs
src/gui/request_logs.rs
src/gui/api.rs
src/gui/text.rs
```

GUI 详情加载流程：

1. 用户在请求日志列表点击详情。
2. GUI 后台线程调用 `ApiClient::ai_gateway_request_log_detail(id)`。
3. 主线程 timer 轮询结果。
4. 成功后调用 `request_log_detail::show(...)` 打开 modal dialog。
5. 弹窗内部为每个 JSON 内容创建一个 `StyledTextCtrl`。

此前曾尝试左侧树 + 右侧代码查看的布局，但左侧树没有实际调试价值，并且会挤压 JSON 内容区域。本补丁移除左侧栏，只保留一个更大的 STC 代码区。

## JSON 高亮和折叠

当前没有引入 WebView，也没有引入 JS/HTML JSON viewer。原因是请求日志详情属于本地调试工具，优先使用 wxDragon 原生控件，保持依赖轻量。

`StyledTextCtrl` 配置：

- 行号 margin：margin `0`。
- 折叠 symbol margin：margin `2`。
- fold marker：使用 STC folder marker `25..31`。
- fold level：按 JSON 的 `{`、`[`、`}`、`]` 逐行计算。
- 只读：内容写入、样式和 fold level 完成后设置 read-only。

折叠逻辑：

- 如果某行最后一个结构字符是 `{` 或 `[`，该行标记为 fold header。
- 如果某行以 `}` 或 `]` 开头，显示层级会先减一。
- 字符串内部的 `{`、`[`、`}`、`]` 会被忽略，避免误判 JSON 字符串内容。
- 点击 fold margin 或双击 header 行会调用 `toggle_fold(line)`。
- “展开代码 / 折叠代码”按钮遍历所有 fold header，并统一设置展开状态。

语法高亮是轻量实现，按字节扫描 JSON：

- object key：蓝色加粗。
- string：绿色。
- number：棕色。
- `true` / `false` / `null`：紫色。
- 标点：灰色。

这不是完整 JSON lexer，但足够覆盖 pretty JSON 的调试查看场景。

## wxDragon Vendor Patch

crates.io 上的 `wxdragon 0.9.16` 已有 STC 基础能力，但缺少本次折叠体验需要的几个 API：

- `SetMarginMask`
- `SetMarginSensitive`
- `SetFoldFlags`
- `SetAutomaticFold`
- `wxStyledTextEvent::GetPosition`
- `wxStyledTextEvent::GetMargin`

如果只依赖本机 `D:/rust_demo/wxDragon`，GitHub CI 无法复现。因此本补丁把 wxDragon 代码 vendored 到：

```text
vendor/wxdragon/
```

并在根 `Cargo.toml` 增加：

```toml
[patch.crates-io]
wxdragon = { path = "vendor/wxdragon/rust/wxdragon" }
wxdragon-sys = { path = "vendor/wxdragon/rust/wxdragon-sys" }
wxdragon-macros = { path = "vendor/wxdragon/rust/wxdragon-macros" }
```

同时 `gui` feature 显式启用 wxDragon 的 STC feature：

```toml
gui = ["dep:image", "dep:wxdragon", "wxdragon/stc", "reqwest/blocking"]
```

本次对 vendor 的实质修改点只在 STC 相关文件：

```text
vendor/wxdragon/rust/wxdragon-sys/cpp/include/widgets/wxd_styledtextctrl.h
vendor/wxdragon/rust/wxdragon-sys/cpp/src/styledtextctrl.cpp
vendor/wxdragon/rust/wxdragon/src/widgets/styledtextctrl.rs
```

其它 vendor 文件是为了让 CI 可以从仓库内完整构建 wxDragon。

## CI 和构建注意事项

由于使用 vendored wxDragon，release workflow 里增加了 wxWidgets / wxDragon CMake build cache 清理，避免 CI 复用旧 CMake cache 后仍指向旧源码路径。

Windows release 构建命令：

```powershell
cargo build --release --features gui --bin codex-remote
```

如果本地旧 exe 正在运行，Windows 会拒绝覆盖：

```text
failed to remove file target\release\codex-remote.exe
拒绝访问。 (os error 5)
```

测试阶段可以先结束 `codex-remote` 进程后重试。

## 验证记录

本补丁完成后已验证：

```powershell
cargo fmt --check
cargo test --bin codex-remote
cargo build --release --features gui --bin codex-remote
```

其中 `cargo test --bin codex-remote` 通过 183 个测试。

## 后续维护规则

- 请求日志详情只展示调试必要字段，不在弹窗里做复杂编辑功能。
- JSON viewer 继续使用原生 `StyledTextCtrl`，不要回退到 WebView。
- 新增 provider 时必须记录转换后的上游请求体，方便与 Codex 原始请求对比。
- 如果未来 wxDragon 发布版本补齐 STC API，可以考虑移除 vendor patch，回到 crates.io 依赖。
- 如果继续保留 vendor，升级 wxDragon 时要重新确认 STC patch 是否仍然存在。

# 认证说明

这份文档记录 CodexHub 当前和 Codex App 的 auth 边界。

## 当前决策

2026-09-24：默认 Provider 恢复 `requires_openai_auth=false`，并写入 `http_headers = { x-openai-actor-authorization = "codexhub-local" }`。`auth.json` 仍使用本地 ChatGPT-shaped token，认证类型保留 `chatgptAuthTokens`。优先保留本地远程控制、自定义模型、搜索和生图的接入路径，不切换到要求真实 ChatGPT 登录的方案；当前客户端上的完整联调仍待验证。

```json
{
  "auth_mode": "chatgptAuthTokens",
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "<本地 ChatGPT-shaped JWT>",
    "access_token": "<本地 ChatGPT-shaped JWT>",
    "refresh_token": "",
    "account_id": "acct_codexhub_local"
  },
  "last_refresh": "2026-06-29T00:00:00Z"
}
```

正常初始化不要切到纯 API key auth。新版本 Codex App 在纯 API key 模式下可以显示插件，但上游 remote-control 会在连接 CodexHub 之前拒绝 API key auth。

### 当前组合与已知限制

Codex Core 接受 `chatgptAuthTokens`，但用户实际调用 Chrome 工具时遇到 `unsupported Codex auth method: chatgptAuthTokens`。相同错误字符串存在于安装的 Codex App `26.915.4065.0` 的 `node_repl.exe` 和 Computer Use 运行时中。Core 的 `getAuthStatus` 在 `requires_openai_auth=true` 时报告实际认证类型，因此只改 Provider 配置不能覆盖这层插件兼容。

初始化和“更新 Codex 配置”现在写入 `chatgptAuthTokens`，仍保留本地账号、套餐字段和长期有效 JWT，不导入或混用上游 ChatGPT 渠道的真实 OAuth 凭证。上一轮写入的本地 `chatgpt` 继续被识别为受管理配置，更新时迁回 `chatgptAuthTokens`；重复更新不覆盖原始登录备份，恢复时按本地 JWT 标记判断，不能只凭认证类型删除用户真实登录。

本机使用 `true + chatgptAuthTokens` 后，Codex App 显示登录页，启动日志中的 `account/read` 报错 `workspace routing discovery missing backend origin`。`chatgptAuthTokens` 同样属于 ChatGPT 账号类型，不能绕过新版路由字段和 HTTPS 校验。因此撤回 `true` 实验，恢复 `false + Actor Authorization`；此时 provider account 为空，自定义模型显示继续使用增强启动适配。本次不声称修复 Chrome，保留本地 `chatgpt_base_url`，不增设 HTTPS 服务，也不修改官方插件或关闭其安全检查。

### `chatgpt_base_url` 不控制令牌刷新

`chatgpt_base_url = "http://127.0.0.1:3847/backend-api"` 只配置相关 Codex backend 请求，不会自动把 OAuth 刷新改到本机。

最新 `references/codex-main/codex-rs/login/src/auth/manager.rs` 中：

- `request_chatgpt_token_refresh()` 单独调用 `refresh_token_endpoint()`。
- 默认刷新地址为 `https://auth.openai.com/oauth/token`。
- `CODEX_REFRESH_TOKEN_URL_OVERRIDE` 可以覆盖这个地址，它是进程环境变量，不是 `chatgpt_base_url` 的派生值。
- `should_refresh_proactively()` 优先检查可解析的 access token `exp`。只有无法获取该过期时间时，才按 `last_refresh` 的 8 天间隔判断。
- `chatgpt` 的 401 恢复路径仍可能主动刷新，不受长期 `exp` 完全保护。

当前 `chatgptAuthTokens` 表示由外部管理 token，Codex 不按普通 `chatgpt` 的托管 OAuth 刷新流程维护它；这不等于免认证，也不代表所有接口都会接受本地凭证。CodexHub 保留现有 10 年有效期的本地 JWT，`refresh_token` 仍为空。上一轮本地 `chatgpt` 在迁移前仍有上述 401 刷新风险，不能声称刷新已被本地 backend 拦截。

本次不写入系统级刷新地址覆盖变量，也不改上游真实 OAuth 渠道的刷新方式。已有本地 `/oauth/token` 是 remote-control step-up 的授权码交换接口，接收表单 `code`；它不是 Codex 使用 JSON `refresh_token` grant 的 OAuth 刷新实现，不能直接拿来替代官方刷新端点。

升级后需点击“更新 Codex 配置”，并由用户重新打开客户端加载认证状态。Chrome、原生搜索、生图和 remote-control 的真实联调仍需进行。

按钮按本地配置状态显示：未初始化时可初始化，旧认证或旧搜索/模型发现配置可更新；配置符合当前写入要求后显示灰色“配置已更新”。该状态只表示本地配置已写入，不表示客户端已重新加载或插件联调成功。WebSocket、所选模型和生图开关等用户偏好不会单独触发更新提示。

## 配置注入

`codexhub configure-codex-app` 会写入：

- `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`，用于本地 backend fallback 接口。
- 默认 `ai-gateway` provider，地址是 `http://127.0.0.1:3847/ai-gateway/v1`。
- `requires_openai_auth=false` 和本地 Actor Authorization header；仍保留 `supports_standalone_web_search=true`、独立搜索 feature 和模型目录发现配置。更新时恢复 Actor header，保留其他 headers。
- `experimental_bearer_token = "dummy-token"`，所以模型请求仍然通过 provider 走 CodexHub。
- 如果本地存在 cached curated catalog，则写入本地 `openai-curated` marketplace。
- 清理历史插件阻断项，例如 `plugins = false`、`computer_use = false`；仍保留 `apps=false`，不启用尚未实现的官方 Apps/Connectors 后端。
- 清理旧版 CodexHub 生成的 bundled remote plugin 状态。

CodexHub 不通过 remote `list` 或 `installed` fallback 发布 `openai-bundled` 插件。包括 `computer-use` 在内的 bundled 插件必须来自 Codex App 自己的本地 `openai-bundled` marketplace。

## 历史兼容

某个未发布的中间版本曾经写过不带 `auth_mode` 的 `OPENAI_API_KEY = "codexhub-dummy-key"`。当前代码只把这种形态作为卸载/清理时的旧 CodexHub-managed auth 识别对象；它不是目标 auth 形态。

本地 `/backend-api/ps/plugins/*` fallback 继续保持窄范围：

- 服务 cached `openai-curated` remote catalog/detail。
- 对已经卡在 UI/cache 里的旧 bundled remote ID 提供只读 detail/skill fallback。
- 不允许把 bundled 插件重新放回 remote list/installed 响应。

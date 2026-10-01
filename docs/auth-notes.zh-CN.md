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

## 2026-10-01：新版 Chrome 插件授权复核

本次核查对象是本机安装的 Windows 包 `OpenAI.Codex_26.928.3736.0`，包内 Chrome 插件版本 `26.928.31416`，浏览器原生运行时为 `0.0.27/20260927214556-b77d38801cca`。同时对照 `references/codex-main` 当前源码。这里的“新版”只指该安装版本，不代表已经验证后续所有版本。

用户已确认增强启动和插件列表恢复。**插件可见性修复不等于 Chrome 身份认证修复。** 当前本地配置仍为 `chatgptAuthTokens`、CodexHub 本地账号、`requires_openai_auth=false`。核查只读取认证类型和凭证是否存在，没有导出 token。

### 当前链路

1. 桌面 App 安装本地 Chrome 插件，同时在 Chromium 浏览器里安装 ChatGPT 扩展。
2. 本地插件通过 `browser-client.mjs`、`browser-service.mjs` 和 `node_repl.exe` 工作；扩展通过 Native Messaging host `com.openai.codexextension` 与本机通信。浏览器内已登录的网站身份和 OpenAI 侧的调用者身份是不同概念。
3. 原生运行时含 `CODEX_CLI_PATH`、`getAuthStatus`、`includeToken`、`refreshToken`、`authMethod`、`authToken` 等认证路径标识。
4. 服务脚本会通过运行时 fetch 访问写死的 `https://chatgpt.com/backend-api/aura/identity`，并包含官方站点状态查询逻辑。这些地址不是从 CodexHub 的 `chatgpt_base_url` 拼接出来的。

### 仍然存在的限制

- `references/codex-main/codex-rs/app-server/src/request_processors/account_processor.rs::get_auth_status_response()` 在 `requires_openai_auth=false` 时直接返回 `auth_method=None`、`auth_token=None`。磁盘上存在 auth.json 不会改变这个分支。
- 同函数在 `true` 分支报告实际认证类型。`app-server/src/auth_mode.rs` 对 `ChatgptAuthTokens` 的映射仍是 `ChatgptAuthTokens`，不会自动报告成 `Chatgpt`。
- 本机新版 `cua_node/bin/node_repl.exe` 仍包含 `Codex auth method is unavailable`、`unsupported Codex auth method: ` 及相邻的 `chatgpt` 标识；没有找到 `chatgptAuthTokens` 字符串。这证明旧错误路径仍存在，但二进制字符串检查不能代替真实调用，不能据此声称已穷尽运行时的全部认证分支。
- `browser-service.mjs` 中身份初始化会读取上述 `/aura/identity`，检查成功状态、`object === "user"` 及非空用户 ID。`readControlledTabRequestHeaderEnabled()` 会等待该身份初始化；其调用者身份缺失错误为 `Browser request-header policy requires caller identity.`。因此不能把身份初始化一概当作可忽略的遥测。
- 站点状态查询的部分错误路径记录 `error_fail_open`，不能把所有 `site_status` 网络异常都解释为 Chrome 必然不可用。
- 本机缓存的 Chrome `26.928.21956` 与包内 `26.928.31416` 的 `browser-client.mjs` 完全一致；`browser-service.mjs` 在归一化版本字符串后完全一致。此次桌面改版没有在这两个脚本里解决旧的认证兼容问题。

### 处理边界与后续方向

本轮没有改写 auth.json/config.toml、官方插件或 App 二进制，没有改门控、关闭客户端，也没有伪造官方身份/站点许可接口。最近的新桌面日志尚为空，检查的近期非空日志没有出现上述 Chrome 认证错误；所以本轮结论是代码与包内容复核，不是当前 Chrome 操作已经成功或失败的实机验收。

后续若要正式接通，应先验证 Chrome 的调用者认证能否与模型 Provider 的认证解耦，并使用用户授权的真实 ChatGPT 凭证。当前检查的 `BrowserUseConfigToml` 只有历史访问与站点权限相关设置，没有独立浏览器登录字段；`CODEX_BROWSER_USE_PEER_AUTHORIZATION` 在安装包中是 macOS 本地进程校验路径，不是 Windows 的 ChatGPT 登录开关。

在找到可验证的接入点之前，不把 `requires_openai_auth=true`、修改本地 JWT 的认证类型或新增显示 gate 当作完整修复。即使让运行时接受某个类型，也不能据此认定本地生成的 token 获得了官方浏览器服务授权。

官方使用说明：<https://developers.openai.com/zh-Hans/docs/chrome-extension>。说明中的安装流程用于区分“扩展连接”和“调用者认证”，不用于证明 CodexHub 本地认证已被官方支持。

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

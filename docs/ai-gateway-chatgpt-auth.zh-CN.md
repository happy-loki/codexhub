# ChatGPT 登录渠道

## Why

ChatGPT 订阅登录使用 OAuth access token、refresh token 和 ChatGPT account ID。
它不是 OpenAI Platform API Key，出站地址也是 Codex 专用后端。因此增加独立
`chatgpt_responses` 渠道，不能只把 token 填进现有 API Key 文本框。

## What

- 在渠道编辑器提供 ChatGPT 登录入口，通过系统浏览器完成官方授权。
- 配置文件只保存凭证引用；真实凭证保存在 CodexHub 用户数据目录的
  `chatgpt-auth` 子目录，不读取或覆盖 `~/.codex/auth.json`。
- 模型列表使用登录账号的 `/backend-api/codex/models`，选定模型后沿用现有路由。
  `client_version` 使用内置官方 GPT 目录的最高最低版本要求，随目录同步更新，
  不把 CodexHub 自己的发布版本误当成 Codex 协议版本，也不启动 Codex 子进程。
- Responses、compact、alpha/search、images 请求使用同一账号认证和官方端点。
- 保留原生 Responses/Lite 内容、工具和密文，不新增协议转换。
- 不改变 Codex 客户端的 provider 名称、登录态、模型显示或压缩选择策略。
- 账号是否有权使用某个模型或工具，以官方返回为准。

## How

参考 `references/codex-main/codex-rs/login/src/server.rs`、
`login/src/auth/manager.rs`、`login/src/token_data.rs` 和
`model-provider-info/src/lib.rs`（2026-09-20）。

1. 使用 Codex 公共 OAuth client ID，生成独立随机 state 和 PKCE S256。
2. 在 loopback 监听登录回调，优先 1455，端口占用时使用随机端口；
   不结束已有 Codex 登录进程。state 不匹配的回调不能完成登录。
3. 浏览器访问 `https://auth.openai.com/oauth/authorize`，用户自行完成授权；
   回调 code 通过 `/oauth/token` 换取凭证，不记录 code/token 到日志。
4. 凭证文件以临时文件原子替换，Unix 使用用户独占权限；刷新前持有账号锁，
   重新读取落盘凭证，避免并发使用已经轮换的 refresh token。
5. 出站使用 `Authorization: Bearer <access_token>` 与 `ChatGPT-Account-ID`。
   官方路径固定为 `https://chatgpt.com/backend-api/codex`，不能通过
   Base URL 把 ChatGPT 凭证发给其它站点。客户端同名账号头不能覆盖渠道账号。
6. access token 即将过期时刷新；出站返回 401 时至多刷新并重发一次。
   refresh token 失效时提示重新登录，不循环刷新。网络错误允许后续重试。
7. 登录支持取消、失败和超时；界面网络操作在后台执行。
   请求日志中的 ChatGPT Authorization 必须脱敏，配置和诊断不包含 token。
8. “退出账号”删除 CodexHub 本地凭证，不注销其它设备。支持官方浏览器授权和
   导入 Codex `auth.json`；导入只保存副本，不修改来源文件。原程序与副本仍可能
   共用会轮换的 refresh token；若提示凭证失效，应使用官方登录创建独立授权。

## 使用入口

AI Gateway → 新增渠道 → ChatGPT（账号登录）→ 登录 ChatGPT。
在浏览器完成授权后返回 CodexHub，拉取并勾选模型，然后保存渠道。
支持为多个账号分别创建渠道，仍按现有优先级和会话粘性进行路由。

## 导入与账号用量

- 导入入口使用文件选择器，由 GUI 读取用户选中的 JSON，再交给本地服务校验。
  接受 Codex ChatGPT 登录的 `tokens`（access_token、id_token、refresh_token、
  可选 account_id）。没有 refresh_token 的有效凭证也可导入，界面提示到期后
  需要重新登录；已过期且不能刷新的凭证不接受。API Key 文件不属于账号登录
  凭证，不接受。不会导入其它
  配置、端点或文件路径；错误信息不回显凭证。
- 用量使用 Codex 相同的 `GET https://chatgpt.com/backend-api/wham/usage`，
  带对应账号的 Bearer 和 ChatGPT-Account-ID，沿用刷新与最多一次 401 重试。
  账号窗口打开、登录或导入成功时读取，也提供手动刷新；不循环轮询官方接口。
- 显示套餐、当前能否使用、额度已用百分比、重置时间及查询时间。按官方返回的
  `limit_window_seconds` 识别 5 小时（18000 秒）和 7 天（604800 秒），不把
  primary/secondary 强行当成固定周期；额外模型额度单独列出。
  时间显示明确标注 UTC；保存和取消按钮固定在窗口底部，详情可滚动查看。
- `rate_limit` 或窗口缺失代表官方未提供，不能显示为 0% 已用或 100% 可用。
  查询失败明确显示失败，不把缓存或本地凭证状态当作实时账号状态。
- 当前源码的用量返回和账号信息没有套餐到期字段，显示“官方未提供”。JWT 的
  `exp` 是 access token 到期时间；额度 `reset_at` 是重置时间；均不是套餐到期日。
  不猜测账单日期，也不拿令牌过期时间替代套餐到期日。

源码依据：`login/src/auth/storage.rs` 的 AuthDotJson、`login/src/token_data.rs`、
`backend-client/src/client/rate_limit_resets.rs` 及
`codex-backend-openapi-models/src/models/rate_limit_status_payload.rs`。

## 验证

### Windows 浏览器参数截断修复

官方页面报 `missing_required_parameter` 时，不能只验证生成的授权 URL。
本次核对 Codex `build_authorize_url`：client_id、redirect_uri、scope、PKCE、
state、originator 等参数均已具备，但 CodexHub 的 Windows 通用打开链接函数
使用 `cmd /C start "" <url>`，没有阻止命令解释器解析 URL 中的 `&`。
独立 Rust 复现（使用无凭证的测试 URL）确认浏览器入口之前只剩
`?response_type=code`，后面的 client_id 等参数被当成其它命令。

修复为现有 wxDragon/wxWidgets 的原生默认浏览器打开函数，完整传递 URL，
不经过 cmd；无需新增依赖。macOS/Linux 原有逐参数启动方式不变。
同步补齐全部授权参数和回调备用端口测试。模拟 OAuth 测试通过，不代表
浏览器启动到官方授权页面的完整链路已经验证，后续需分别检查这两个层次。

### 自动检查

使用本地模拟 OAuth/Responses 服务器验证 PKCE/state、凭证存储、并发刷新、
401 重试、账号头覆盖、模型列表、Lite 字段保留及配置序列化；补充官方及旧版
auth.json 导入、错误账号拒绝、无刷新凭证和过期导入、用量接口路径及 401 重试、
缺失窗口、反序窗口和重置时间显示验证。
真实登录和账号权限仍需用户在浏览器授权后验证，不以模拟测试替代官方验证。

2026-09-20：`cargo test --locked --features gui --bin codexhub --quiet`：
755 项通过、2 项忽略；未操作真实账号、未构建发布包。

2026-09-21 提交前复验：`cargo fmt --all -- --check` 通过；
`cargo test --locked --features gui --bin codexhub --quiet` 为 758 项通过、
2 项忽略、0 项失败。本次提交包含上游响应头记录与查看；不包含实验性的
turn-state 采集、筛选或注入策略，不修改用户的系统代理或 Codex 登录配置。

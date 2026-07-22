# Codex App 插件与 Remote-Control 收敛计划

日期: 2026-06-29

## 当前结论

旧的完整 `/backend-api/ps/plugins/*` 本地插件商店 mock 方案废弃。它太重，容易和新版本 Codex App 自己的 bundled plugin 机制打架，例如造成同一个工具出现两份。CodexHub 只保留基于本地 `openai-curated` 缓存的最小 remote catalog fallback。

## 2026-06-29 最新工作记录

这次只处理 Codex App 插件页里的 `computer-use` 详情问题，不处理 MCP runtime、不启动 Codex App、不做 GUI 自动验证。

## 2026-06-29 纠正后的主线

用户确认过一个关键事实:

```text
不使用 CodexHub，仅切到 Codex App 自己的 API key 模式时，插件可以正常显示和使用。
但昨天已经验证过: API key 模式不能满足 CodexHub remote-control 功能。
再切回 CodexHub 初始化后的配置后，computer-use 详情失败。
```

因此不能再把 API key 模式当作 CodexHub 默认初始化方案。API key 模式正常只能证明 Codex App 自己的本地 bundled/plugin 链路没有坏；CodexHub 仍必须维持 `chatgptAuthTokens`，否则 remote-control 会在连接到 CodexHub 之前被上游拒绝。

当前优先级:

1. 默认初始化恢复并保持 `chatgptAuthTokens`，不能改成 API key dummy。
2. 插件问题只从 bundled marketplace、本地缓存、旧 remote catalog item 和 feature flags 收敛，不再通过切换 auth 到 API key 规避。
3. 保持 bundled 插件由 Codex App 自己的 `openai-bundled` 本地 marketplace 处理；CodexHub 不再发布 bundled remote list/installed item。
4. 只保留旧 bundled remote id 的只读详情兼容，作为旧 UI/cache 已经点进来的兜底。
5. 不再用纯 API key 模式日志解释 CodexHub 模式的 `computer-use` 失败。
6. 初始化时显式确保 `browser@openai-bundled`、`chrome@openai-bundled`、`computer-use@openai-bundled` 为 enabled，修复历史 UI 卸载/禁用造成的状态偏移。

2026-06-29 14:xx 新日志补充:

```text
plugin/read -> errorCode=null
bundled marketplace 写入成功，包含 browser/chrome/computer-use/latex
computer-use native pipe startup ready
随后出现 plugin_uninstall_succeeded pluginId=computer-use@openai-bundled
```

这说明这一轮里 `computer-use` 详情读取不再是旧的 `plugin/read` 或 `plugin/skill/read` 404；至少有一次详情读取已经成功。后续不可见/失败要优先查本地安装状态是否被卸载或被 UI toggle 改写，而不是继续补 `/backend-api/ps/plugins/*` detail fallback。

下一步只处理本地状态收敛:

1. CodexHub 初始化后必须确保 `[plugins."computer-use@openai-bundled"] enabled = true`。
2. 如果 Codex App 历史状态里记录了 bundled 插件卸载/禁用，CodexHub 初始化需要清理或覆盖这类状态。
3. 继续不发布 `computer-use` remote list/installed item，避免再次制造重复项。

2026-06-29 14:xx 错误修正记录:

```json
{
  "OPENAI_API_KEY": "codexhub-dummy-key",
  "tokens": null,
  "last_refresh": null
}
```

这条方向已撤销。它能复用 Codex App API key 插件链路，但会破坏 CodexHub 必需的 remote-control。当前代码只把 `codexhub-dummy-key` 作为历史误写的 CodexHub-managed auth 识别对象，便于卸载/清理；目标状态仍是 `chatgptAuthTokens`。

当前修正的核心目的:

1. 保持 remote-control 可用，不能因为插件问题切换到 API key auth。
2. 避免 CodexHub 通过 remote list/installed 伪装 bundled 插件，减少 `computer-use` 被当作 remote catalog item 的机会。
3. 保持 bundled 插件走 Codex App 本地 marketplace，而不是 CodexHub remote catalog。

执行边界:

1. CodexHub 侧只负责逻辑分析、文档记录和代码修改。
2. 不再由 CodexHub 侧启动、重启、点击或验证 Codex App。
3. 不再通过额外运行态探测来确认用户当前 UI 状态。
4. 验证由用户手动完成；用户反馈新的日志或现象后，再根据反馈补最小代码修复。

已经做的改动:

1. 删除误加的 `/backend-api/ps/mcp` 逻辑。这个接口不是 `computer-use` 详情页接口，它属于 hosted plugin runtime / apps MCP。CodexHub 不能伪造空成功响应，否则可能让 Codex App 进入错误的 runtime 重连链路。
2. `src/web/plugins.rs` 保留最小插件 fallback 路由:

   ```text
   GET /backend-api/ps/plugins/list
   GET /backend-api/ps/plugins/installed
   GET /backend-api/ps/plugins/suggested
   GET /backend-api/ps/plugins/{plugin_id}
   POST /backend-api/ps/plugins/{plugin_id}/install
   GET /backend-api/ps/plugins/{plugin_id}/skills/{skill_name}
   GET /backend-api/plugins/featured
   ```

3. `/backend-api/ps/plugins/installed` 不再返回 `openai-bundled` 插件。`computer-use` 是 Codex App 自己的本地 bundled 插件，不能被 CodexHub 伪装成 remote installed plugin。
4. 插件 fallback 的 Codex home 统一使用 `codex_app_config::default_codex_home()`，也就是 `%USERPROFILE%\.codex`。不能使用当前 CodexHub 进程里的 `CODEX_HOME`，因为从 Arthas/Codex shell 启动时这个环境变量可能指向 `%LOCALAPPDATA%\com.lokizhou.arthas\codex`，不是 Codex App 的 home。

当前判断:

1. `computer-use` 正常路径应该由 Codex App 的 `openai-bundled` 本地 marketplace 读取，走上游 `plugin/read` 的 `marketplacePath` 分支。
2. 如果 CodexHub 在 installed fallback 里返回 `computer-use@openai-bundled`，UI 可能把它当作 remote plugin，详情页转去 `remoteMarketplaceName` 分支，进而出现“未找到插件 / 无法从插件市场详情页加载此插件”。
3. CodexHub 不再从 list/installed 返回 bundled remote 插件。只要继续在列表里返回 `plugins~codexhub-bundled-*`，UI 就可能把 `computer-use` 当作远端市场插件。当前策略是让新列表回到 Codex App 本地 `marketplacePath` 分支，同时对旧 UI/cache 已经点进来的 bundled remote ID 提供只读详情兼容。
4. 后续如果用户手动验证仍失败，只根据最新日志里的 `plugin/read` 参数继续补最小修复，不再扩大到 MCP、完整插件商店 mock 或 GUI 自动操作。

## 2026-06-29 最新日志结论

2026-06-29 13:xx 重新按正确目录核对:

```text
Codex App home: %USERPROFILE%\.codex
Codex App logs: %LOCALAPPDATA%\Packages\OpenAI.Codex_2p2nqsd0c76g0\LocalCache\Local\Codex\Logs\2026\06\29
```

不能用当前 CodexHub/Arthas shell 里的 `CODEX_HOME` 推断 Codex App home。当前 shell 的 `CODEX_HOME` 可能指向 `%LOCALAPPDATA%\com.lokizhou.arthas\codex`，但 Codex App 实际读写的是 `%USERPROFILE%\.codex`。

最新日志里 `openai-bundled` 本地 marketplace 和 `computer-use` 本体是正常写入/注册的:

```text
BundledPluginsMarketplace 写入 openai-bundled，包含 browser/chrome/computer-use/latex
plugin_marketplace_add_succeeded marketplaceName=openai-bundled
computer-use native pipe startup ready
bundled_plugin_install_skipped_current pluginName=computer-use
%USERPROFILE%\.codex\plugins\cache\openai-bundled\computer-use\26.623.61825 存在
%USERPROFILE%\.codex\.tmp\bundled-marketplaces\openai-bundled\plugins\computer-use 存在
```

新发现的直接错误不是 `plugin/read`，而是 `plugin/skill/read`:

```text
read remote plugin skill details:
GET http://127.0.0.1:3847/backend-api/ps/plugins/local~openai-bundled~computer-use/skills/computer-use
-> 404 Not Found
```

这说明 Codex App 在详情页打开 `computer-use` 后，会继续通过 remote skill detail 分支读取 `SKILL.md` 内容。`local~openai-bundled~computer-use` 是上游允许的 remote plugin id 形态，因为 remote id 允许 ASCII 字母、数字、`_`、`-`、`~`。因此 CodexHub 需要补的是这个窄范围的 skill detail fallback:

```text
GET /backend-api/ps/plugins/local~openai-bundled~{plugin}/skills/{skill}
```

它应读取:

```text
%USERPROFILE%\.codex\plugins\cache\openai-bundled\{plugin}\<version>\skills\{skill}\SKILL.md
```

如果 cache 没有，再读:

```text
%USERPROFILE%\.codex\.tmp\bundled-marketplaces\openai-bundled\plugins\{plugin}\skills\{skill}\SKILL.md
```

这个修复不能恢复旧的 bundled marketplace list/installed mock。否则又会让 UI 出现两个 `computer-use`。当前代码已按这个结论补 `src/web/plugins.rs` 的窄范围 fallback。

2026-06-29 13:5x 复查:

```text
127.0.0.1:3847 listener: D:\rust_demo\codexhub\target\release\codexhub.exe
GET /backend-api/ps/plugins/local~openai-bundled~computer-use/skills/computer-use -> 200
response.plugin_id = local~openai-bundled~computer-use
response.name = computer-use
response.skill_md_contents 存在，长度约 40 KB
```

所以旧的 `plugin/skill/read` 404 已被当前运行中的 CodexHub 解决。如果 UI 仍显示旧的 `computer-use` 详情错误，当前日志还不能证明是同一个 404 仍在发生；需要看重新打开详情页后是否出现新的 `plugin/skill/read` 或 `plugin/read`。最新非空 Codex App 日志只显示启动后 `stop_process`，没有新的 `computer-use` detail 请求。

同一轮又发现一个响应结构问题:

```text
GET /backend-api/ps/plugins/local~openai-bundled~computer-use -> 200
release.skills = []
```

这会让 `plugin/read` 返回的详情对象缺少技能摘要。即使后续 `/skills/computer-use` 能返回 `SKILL.md`，前一个 detail 对象仍是不完整的。源码已继续补窄范围逻辑: 对 bundled compat detail 读取本地 bundled `skills/*/SKILL.md` front matter，把 `release.skills` 填回去。这个改动仍然只影响 detail fallback，不恢复 bundled list/installed mock。

注意: 这次 `release.skills` 修复还没有进入当前监听 `127.0.0.1:3847` 的进程。当前监听进程是 `D:\rust_demo\codexhub\target\release\codexhub.exe`，启动时间早于这个源码修复。需要用户重新 build/restart CodexHub 后才会生效。

同一轮日志还显示:

```text
refresh_local_remote_control_client_id_failed:
Sign in to ChatGPT in Codex to check remote control authorization.
bundled_plugins_reconcile_skipped_features_unavailable reason=focus
```

这说明当前 Codex App 启动时认为某些 ChatGPT/feature 状态不可用。它不直接解释 `computer-use` skill detail 404，因为 skill detail endpoint 已经 200；但它可能解释为什么 UI 没有进入完整插件详情刷新链路。

2026-06-29 14:10 复查:

```text
127.0.0.1:3847 listener: D:\rust_demo\codexhub\target\release\codexhub.exe
GET /backend-api/ps/plugins/local~openai-bundled~computer-use -> 200
release.skills count = 1
GET /backend-api/ps/plugins/local~openai-bundled~computer-use/skills/computer-use -> 200
skill_md_contents 存在，长度约 40 KB
```

所以 CodexHub 侧旧的 `computer-use` detail/skill fallback 已经生效。继续“看不见”的新原因来自 Codex App 自己的 auth/config 状态:

```text
remoteControl/enable:
remote control requires ChatGPT authentication; API key auth is not supported

plugin/list:
list remote plugin catalog: chatgpt authentication required for remote plugin catalog; api key auth is not supported
```

当前 `%USERPROFILE%\.codex\auth.json` 顶层存在 `OPENAI_API_KEY` / `auth_mode`，并且没有 `tokens.chatgptAuthTokens`。这会让上游把当前身份判定为 API key auth，而不是 ChatGPT backend auth。之前“API key 能看插件”的观察只说明本地部分插件路径可用；但 remote-control 和 remote plugin catalog 都明确拒绝 API key auth。

同一批日志还显示 bundled reconcile 状态异常:

```text
bundled_plugins_runtime_marketplace_written pluginNames=["browser","chrome","computer-use","latex"]
plugin_marketplace_add_succeeded marketplaceName=openai-bundled
bundled_plugin_install_skipped_missing pluginName=computer-use
```

因此下一步不再继续补 `/backend-api/ps/plugins/...` detail fallback，而是修 CodexHub 写入的 `auth.json` 形态: 必须恢复 ChatGPT-shaped auth，让 Codex App 的 `auth_mode.uses_codex_backend()` 为 true，同时保留 dummy token 只用于本机 `chatgpt_base_url`。

较早一批日志里，用户截图对应的 `computer-use` 详情页显示“未找到插件 / 无法从插件市场详情页加载此插件”。那批日志对应的是 Codex App 在读取 remote plugin detail 时请求本机 CodexHub:

```text
plugin/list -> GET http://127.0.0.1:3847/backend-api/ps/plugins/installed?scope=USER
plugin/read -> GET http://127.0.0.1:3847/backend-api/ps/plugins/plugins~codexhub-bundled-browser
```

失败类型不是 404，也不是 JSON 字段解析失败，而是:

```text
failed to send remote plugin catalog request
error sending request for url
```

同一时间点 CodexHub 后端还没有在 `127.0.0.1:3847` 上可用；后面 CodexHub 启动后，旧版本代码下手动请求这些接口曾返回 200:

```text
GET /backend-api/ps/plugins/installed?scope=USER -> 200
GET /backend-api/ps/plugins/plugins~codexhub-bundled-browser -> 200
GET /backend-api/ps/plugins/plugins~codexhub-bundled-computer-use -> 200
GET /backend-api/ps/plugins/computer-use@openai-bundled -> 200
```

所以这批日志能证明的结论是: 当次详情页失败首先是 Codex App 访问本机 `chatgpt_base_url` 失败。要判断当前代码是否仍有 `computer-use` 逻辑错误，需要在 CodexHub 后端已运行的状态下重新点击一次详情页，再看新的 `plugin/read` 日志。

仍需注意一个残留现象: 日志里出现了 `plugins~codexhub-bundled-browser` 这种 CodexHub fallback ID。它可能来自之前旧 fallback 返回过 bundled 插件后的 UI/remote catalog 缓存或内存状态。当前代码已经不再从 `/backend-api/ps/plugins/installed` 返回 `openai-bundled` 插件；如果后续日志仍然出现这个 ID，就只查 Codex App 的 remote catalog cache / renderer state，不恢复完整插件 mock。

2026-06-29 后续修正: 当前代码不再通过 list/installed 发布 `plugins~codexhub-bundled-*` 或 `*@openai-bundled`。`computer-use` 的正确来源仍是 Codex App 自己注册的本地 `openai-bundled` marketplace；但详情接口会对旧 bundled remote ID 做只读兼容，避免旧缓存点击后直接 404。

2026-06-29 后续工作约束: 不继续做本机验证。即使后续修改代码，也不运行 Codex App、不点击插件页、不做 smoke test；只根据用户给出的现象和日志调整逻辑。

继续核对后端启动后的日志:

```text
2026-06-29T04:07:09Z 之后多次 plugin/read: errorCode=null
2026-06-29T04:07:xx 之后多次 plugin/list: errorCode=null
CodexHub chain log 中对应 /backend-api/ps/plugins/* 均为 200
```

这些成功请求里，CodexHub chain log 能看到的是 curated remote fallback，例如:

```text
GET /backend-api/ps/plugins/plugins~codexhub-local-amplitude -> 200
GET /backend-api/ps/plugins/plugins~codexhub-local-linear -> 200
```

没有看到 Codex App 在后端运行后请求:

```text
GET /backend-api/ps/plugins/plugins~codexhub-bundled-computer-use
GET /backend-api/ps/plugins/computer-use@openai-bundled
```

目前 `computer-use` 本地状态是正常的:

```text
bundled marketplace 写入成功，包含 browser/chrome/computer-use/latex
plugin_marketplace_add_succeeded marketplaceName=openai-bundled
computer-use native pipe startup ready
bundled_plugin_install_skipped_current pluginName=computer-use
~/.codex/plugins/cache/openai-bundled/computer-use/26.623.42026 存在
~/.codex/.tmp/bundled-marketplaces/openai-bundled/plugins/computer-use 存在
```

因此当前还不能得出“computer-use 本地 bundle 或 marketplace 仍坏”的结论。更准确的判断是:

1. 旧截图对应的直接错误已经由“本机后端不可达”解释。
2. 后端恢复后，Codex App 还没有在日志中留下新的 `computer-use` detail 失败请求。
3. 如果 UI 仍显示旧错误，优先怀疑 renderer detail 页状态或旧 remote catalog item 仍在内存中，而不是继续改 `/ps/mcp` 或恢复 bundled remote mock。

新的主线是:

1. `auth.json` 继续使用 ChatGPT-shaped `chatgptAuthTokens`，但只服务于 remote-control 启动检查。
2. 不再模拟完整远端插件目录、bundle 下载、卸载等重链路；只保留 `openai-curated` 缓存对应的最小 catalog/detail fallback。
3. Bundled 插件展示和使用交给 Codex App / app-server 自己处理。
4. CodexHub 只负责写入最小必要配置:

   ```toml
   chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
   model_provider = "ai-gateway"

   [model_providers.ai-gateway]
   name = "ai-gateway"
   base_url = "http://127.0.0.1:3847/ai-gateway/v1"
   wire_api = "responses"
   requires_openai_auth = true
   experimental_bearer_token = "dummy-token"

   [marketplaces.openai-curated]
   source_type = "local"
   source = "<Codex cached curated plugins path>"
   ```

5. 清理历史阻断项:

   ```toml
   [features]
   apps = false
   plugins = false
   computer_use = false
   browser_use = false
   in_app_browser = false
   ```

## 为什么不能只用 API Key

API key 模式可以让模型请求和部分插件路径更简单，但上游 remote-control 明确拒绝 API key:

```text
remote control requires ChatGPT authentication; API key auth is not supported
```

对应源码:

```text
references/codex-main/codex-rs/app-server-transport/src/transport/remote_control/auth.rs
```

所以 CodexHub 如果还要 IM 远程控制、会话列表、启动线程、审批响应，就必须让 Codex App 的 auth 形态看起来是 ChatGPT backend auth。

## 为什么不再做插件 Mock

上游插件逻辑已经有本地 marketplace / bundled plugin 路径。API key 模式下插件能显示，说明新版本 Codex App 不再依赖我们完整模拟远端插件商店。

ChatGPT-shaped auth 下会额外启用 Codex backend 相关路径，但我们可以通过几件事减少干扰:

1. 本地 auth 使用个人账号形态，不使用 workspace / enterprise plan，避免触发 workspace `enable_plugins` 策略分支。
2. 不注入 `openai-bundled`。新版 Codex App 会自己把 bundled marketplace 写到 `~/.codex/.tmp/bundled-marketplaces/openai-bundled` 并通过 `marketplace/add` 注册；CodexHub 预先写同名 marketplace 会造成 “already added from a different source”。
3. 注入本机已有的 `openai-curated` 缓存 marketplace，也就是 `~/.codex/.tmp/plugins`，让 GitHub/官方 curated 插件可以在本地 catalog 中展示。
4. 移除 `apps = false` 等历史禁用项，避免插件能力被旧配置关掉。

不要默认写 `features.remote_plugin = false`。该开关会关闭远端 global catalog，虽然能强制使用本地 curated fallback，但会让用户开 VPN 后也无法验证远端 catalog。当前策略是保留上游远端能力，同时显式注册本地 cached curated marketplace 作为 fallback。

## 2026-06-29 日志结论

用户配置里的 Windows 路径格式可以被 Codex App 读取，但 `openai-bundled` 不应由 CodexHub 写入:

```toml
[marketplaces.openai-primary-runtime]
source_type = "local"
source = '\\?\%USERPROFILE%\.cache\codex-runtimes\codex-primary-runtime\plugins\openai-primary-runtime'

[marketplaces.openai-curated]
source_type = "local"
source = '%USERPROFILE%\.codex\.tmp\plugins'
```

TOML 单引号字符串里的反斜杠不是转义字符。上游 `installed_marketplaces.rs` 对 `source_type = "local"` 的处理是 `toml::Value::as_str` 后直接 `PathBuf::from`，再检查 marketplace manifest。实测这些路径 `Path.exists()` 为 true，且对应 manifest 存在:

```text
~/.cache/codex-runtimes/codex-primary-runtime/plugins/openai-primary-runtime/.agents/plugins/marketplace.json
~/.codex/.tmp/plugins/.agents/plugins/marketplace.json
```

Codex App 的插件页每次进入会发 `plugin/list`。在 ChatGPT-shaped auth 且 `features.remote_plugin` 开启时，源码分支是:

```text
include_global_remote = !explicit_marketplace_kinds && remote_plugin enabled
use_remote_global_catalog = include_global_remote && auth_mode.uses_codex_backend
```

所以它会请求 remote global catalog。日志里看到 `plugin/list` 期间请求 `http://127.0.0.1:3847/`，并且被本机代理 `127.0.0.1:7897` 拦截，这说明当时走的是 remote catalog 路径，不是单纯读取本地 `openai-curated`。

Codex App 有 remote global catalog 磁盘缓存，路径是:

```text
~/.codex/cache/remote_plugin_catalog/*.json
```

但这个缓存只缓存 global directory plugins。命中缓存后，`fetch_remote_marketplaces` 仍会请求 installed plugins，并且如果已有缓存，`plugin/list` 还会安排后台刷新 remote global catalog。因此 UI 看起来仍然像“每次进插件页都在拉远端”。

另外，旧 GitHub curated 插件缓存也存在:

```text
~/.codex/.tmp/plugins
~/.codex/.tmp/plugins.sha
```

这部分由 `sync_openai_plugins_repo` 维护，优先 git，再 GitHub zipball，再 export archive。日志里曾出现:

```text
failed to sync curated plugins repo:
git ls-remote https://github.com/openai/plugins.git failed
GitHub HTTP zipball failed
export archive failed
```

只要本地已有 `~/.codex/.tmp/plugins/.agents/plugins/marketplace.json` 和 `plugins.sha`，这些同步失败不会删除本地快照；但失败会让上游把 curated sync 标记重置，后续进入插件页或刷新时可能再次尝试。

当前判断:

1. 路径语法不是主要问题。
2. “openai-curated 什么时候拉”答案是: app-server 的 curated repo sync 会在插件相关后台任务里启动，优先拉 GitHub `openai/plugins`；但 ChatGPT-shaped auth + remote_plugin 开启时，插件页主列表还会走 remote global catalog。
3. “为什么以前 GitHub 插件很多、现在只看到几个”更可能是 UI 当前展示了 remote global catalog 或 installed-only 过滤后的结果，而不是本地 `.tmp/plugins` 没了。
4. 若要强制无 VPN 时优先展示本地 GitHub curated，需要让 `plugin/list` 不走 remote global catalog，最直接是 `features.remote_plugin = false`；但这会牺牲 VPN 下验证远端插件的能力，所以不是默认策略。

## 2026-06-29 Remote Catalog Fallback

已添加轻量 fallback，不恢复旧的完整插件 mock。CodexHub 现在只处理 Codex App 插件页会请求的最小 remote catalog 接口:

```text
GET /backend-api/ps/plugins/list
GET /backend-api/ps/plugins/installed
GET /backend-api/ps/plugins/suggested
GET /backend-api/ps/plugins/{plugin_id}
POST /backend-api/ps/plugins/{plugin_id}/install
GET /backend-api/ps/plugins/{plugin_id}/skills/{skill_name}
GET /backend-api/plugins/featured
```

行为:

1. `/backend-api/ps/plugins/list?scope=GLOBAL` 读取 `~/.codex/.tmp/plugins/.agents/plugins/marketplace.json`，把本地 `openai-curated` 的 GitHub curated 插件转换成 `openai-curated-remote` 形态返回。
2. `/backend-api/ps/plugins/installed` 只返回真正需要 remote fallback 的 curated 插件。不要返回 `openai-bundled` 里的插件。
3. `/backend-api/plugins/featured` 返回一组稳定 featured id，例如 `github@openai-curated-remote`、`linear@openai-curated-remote`、`gmail@openai-curated-remote`。
4. `/backend-api/ps/plugins/suggested` 返回少量推荐插件，避免 app-server 记录推荐接口 404。
5. `/backend-api/ps/plugins/{plugin_id}` 返回同一份 converted directory item，供远程目录详情页读取。
6. `POST /backend-api/ps/plugins/{plugin_id}/install` 只返回轻量成功响应；当前不做 bundle 下载伪造。

本机验证结果:

```text
GET /backend-api/ps/plugins/list?scope=GLOBAL&limit=200
Count: 180
HasGithub: true
HasSlack: true
First10: linear, atlassian-rovo, google-calendar, gmail, slack, teams, sharepoint, outlook-email, outlook-calendar, canva
```

这个 fallback 的目的只是让插件页面在无 VPN / 官方 remote catalog 不可达时仍能看到 GitHub curated 插件列表和详情页。卸载、skill detail、bundle 下载仍未模拟；如果 UI 后续需要这些接口，再按实际日志补最小接口。

CodexHub 仍不应该恢复这些完整 fake 接口:

```text
GET  /backend-api/plugins/list
POST /backend-api/plugins/{plugin_id}/enable
GET  /backend-api/ps/plugins/workspace/shared
GET  /backend-api/ps/plugins/workspace/created
POST /backend-api/ps/plugins/{plugin_id}/uninstall
```

## 2026-06-29 computer-use 详情页故障结论

用户现象:

```text
插件列表可以看到 computer-use，但点击进去显示:
未找到插件 / 无法从插件市场详情页加载此插件
```

当前结论:

1. 这不是 `computer-use` 本地 bundle 缺失。Codex App 日志显示它已经成功把 bundled marketplace 写到:

   ```text
   %USERPROFILE%\.codex\.tmp\bundled-marketplaces\openai-bundled
   ```

   且包含:

   ```text
   ["browser", "chrome", "computer-use", "latex"]
   ```

2. 旧日志里真正的失败是 `openai-bundled` 被重复注册:

   ```text
   marketplace 'openai-bundled' is already added from a different source; remove it before adding this source
   ```

   这会导致 Codex App 自己的 bundled marketplace reconcile 失败。列表里仍可能显示 `computer-use`，但详情页走 `plugin/read` 时找不到正确 marketplace/detail，于是 UI 报“未找到插件”。

3. CodexHub 代码已经改成不再写 `[marketplaces.openai-bundled]`，并且 `configure-codex-app` 会清理历史残留。当前本机 `%USERPROFILE%\.codex\config.toml` 已确认:

   ```text
   没有 [marketplaces.openai-bundled]
   有 [marketplaces.openai-curated]
   有 [plugins."computer-use@openai-bundled"] enabled = true
   ```

4. 曾经验证过本地 CodexHub remote fallback 可以返回 bundled detail。恢复完整 bundled mock 的方向已经废弃，因为它会继续制造 `plugins~codexhub-bundled-*` 远端身份，和 Codex App 自己的 `openai-bundled` 本地 marketplace 冲突。当前只保留旧 remote id 的只读 detail 兼容，不在 list/installed 发布 bundled 插件。

5. 2026-06-29 进一步确认: 不能把 CodexHub 当前进程里的 `CODEX_HOME`
   当成 Codex App 的 home。Codex App 这条链路使用的是用户目录:

   ```text
   %USERPROFILE%\.codex
   ```

   但 CodexHub 如果从 Arthas/Codex shell 启动，进程环境里可能有:

   ```text
   CODEX_HOME=%LOCALAPPDATA%\com.lokizhou.arthas\codex
   ```

   这只影响 CodexHub 自己的 fallback 路径解析，不代表 Codex App 读错
   `CODEX_HOME`。已修正 `src/web/plugins.rs`，让插件 fallback 和 GUI 初始化
   配置统一使用 `codex_app_config::default_codex_home()`，也就是用户 `.codex`。
   当前保留这个修正是为了读取 `openai-curated` 缓存，不是为了读取 bundled 插件。Codex App 当前使用的相关目录包括:

   ```text
   %USERPROFILE%\.codex\.tmp\bundled-marketplaces\openai-bundled
   %USERPROFILE%\.codex\.tmp\plugins
   ```

   主路径仍然要求 `computer-use` 详情走 Codex App 本地 `marketplacePath`。只有旧 UI/cache 已经点到 `plugins~codexhub-bundled-*` 时，CodexHub 才读取 `openai-bundled` marketplace manifest，返回只读 detail 兜底。

6. 详情页还可能读取 remote skill detail。当前只允许 `openai-curated` fallback 走这个接口；bundled skill detail fallback 仍不提供:

   ```text
   GET /backend-api/ps/plugins/{plugin_id}/skills/{skill_name}
   ```

   以下旧接口不再作为目标:

   ```text
   /backend-api/ps/plugins/plugins~codexhub-bundled-computer-use/skills/computer-use
   /backend-api/ps/plugins/computer-use@openai-bundled/skills/computer-use
   ```

7. 旧日志时间是 `2026-06-29T02:12`，发生在配置修复之前。配置修复和 release build 之后，检查进程时 Codex App 没有运行，所以还没有新的点击日志能证明 UI 已恢复或仍失败。

已完成验证:

```powershell
Get-Process codexhub -ErrorAction SilentlyContinue | Stop-Process -Force
cargo build --release --features gui
.\target\release\codexhub.exe configure-codex-app
Start-Process -FilePath ".\target\release\codexhub.exe" -ArgumentList "daemon" -WorkingDirectory "." -WindowStyle Hidden
```

结果:

```text
release build 通过，仅有既有 dead-code warnings
codexhub daemon 正在运行
本地插件 list fallback 可返回约 180 个 curated 插件
computer-use bundled remote list/installed fallback 已删除；旧 bundled remote detail 只读兼容保留
```

下一步只做这件事，不继续扩展方案:

1. 完整重启 Codex App。
2. 进入插件页，点击 `computer-use` 一次。
3. 读取最新日志:

   ```powershell
   rg -n "plugin/read|computer-use|openai-bundled|plugin_marketplace_add_failed|bundled_plugins_reconcile_failed|not found|无法|未找到" "$env:LOCALAPPDATA\Packages\OpenAI.Codex_2p2nqsd0c76g0\LocalCache\Local\Codex\Logs\2026\06\29"
   ```

4. 如果没有新的 `openai-bundled is already added from a different source`，且详情页打开，问题结束。
5. 如果仍失败，只根据最新日志补一个最小修复:
   - 如果失败仍是 marketplace 重复，继续查 Codex App/CLI 的 marketplace install metadata/cache，而不是改 remote catalog。
   - 如果失败是 `plugin/read` 指向错误 `marketplacePath`，修 config/本地 marketplace 路径。
   - 如果失败是 remote `/ps/plugins/...` 404，才继续补 CodexHub 的最小 fallback endpoint。

## 2026-06-29 11:17 新复现结论

用户按 GUI 初始化配置并重启 Codex App 后，`computer-use` 详情页仍显示:

```text
未找到插件
无法从插件市场详情页加载此插件。
```

最新日志更新了结论:

1. 这次不再是 `openai-bundled` 重复注册。Codex App 日志显示:

   ```text
   plugin_marketplace_add_succeeded alreadyAdded=false marketplaceName=openai-bundled
   bundled_plugins_marketplace_added marketplacePluginNames=["browser","chrome","computer-use","latex"]
   ```

2. Codex App 只安装了 3 个 bundled 插件，`computer-use` 被跳过:

   ```text
   bundled_plugin_install_requested pluginName=browser reason=missing
   plugin_install_succeeded pluginName=browser
   bundled_plugin_install_requested pluginName=chrome reason=missing
   plugin_install_succeeded pluginName=chrome
   bundled_plugin_install_skipped_current pluginName=computer-use
   plugin_marketplace_sync_completed pluginsInstalledAfterWrite=3 pluginsInstalledBeforeWrite=0
   ```

3. `config.toml` 是 Codex App 自己重新写回了:

   ```toml
   [marketplaces.openai-bundled]
   source_type = "local"
   source = '\\?\%USERPROFILE%\.codex\.tmp\bundled-marketplaces\openai-bundled'
   ```

   这不是 CodexHub 注入的历史残留；这次是 Codex App bundled reconcile 成功后的正常持久化。

4. 当前待查主线不是 remote catalog fallback，而是为什么 `computer-use` 被认为是 `current` 并跳过安装后，详情页还走“插件市场详情页”并加载失败。下一步只查:

   ```text
   bundled_plugin_install_skipped_current
   plugin/read 请求参数
   installed plugin cache: ~/.codex/plugins/cache/openai-bundled/computer-use
   openai-bundled marketplace manifest 里的 computer-use source/path
   ```

## 2026-06-29 `computer-use` 详情页修正

`computer-use` 是 Codex App 自己注册的 `openai-bundled` 本地 marketplace 插件，不应该通过 CodexHub 的 `/backend-api/ps/plugins/installed` 伪装成 remote installed plugin。

上游 `plugin/read` 有两条路径:

```text
marketplacePath -> 本地 marketplace 读取
remoteMarketplaceName -> 远端 /backend-api/ps/plugins/{remote_plugin_id} 读取
```

`openai-bundled` 应走第一条本地 `marketplacePath`。如果 CodexHub 把 `computer-use@openai-bundled` 放进 remote installed 返回里，UI 可能把它当成远端插件，点击详情时走“插件市场详情页”，从而出现“未找到插件 / 无法从插件市场详情页加载此插件”。

因此当前修正是:

```text
/backend-api/ps/plugins/installed 不再返回 openai-bundled 插件
```

`computer-use` 的列表、安装状态和详情应由 Codex App 自己的 `openai-bundled` marketplace 处理；CodexHub 只保留 `openai-curated` 对应的最小 remote catalog/detail fallback。

补充结论:

1. 如果日志里仍出现 `plugins~codexhub-bundled-*`，这不是应该恢复的接口，而是旧 fallback 返回过的 remote item、旧二进制或 renderer/cache 状态残留。
2. 新逻辑下 CodexHub 不应该再制造任何 bundled remote plugin ID。
3. 如果 `computer-use` 详情仍失败，下一步只从“前端选中了错误的 plugin item / remote item 优先级高于本地 marketplace item”这个方向做代码或配置清理，不回到 bundled mock。
4. 后续不由 CodexHub 侧验证；用户验证后把新的 UI 现象或日志给回来，再补最小修改。

2026-06-29 静态源码补充:

上游 `PluginsManager` 有进程内 `remote_installed_plugins_cache`。`plugin/list` 会优先使用这个缓存构造 remote installed marketplace；缓存里如果曾经装入过旧 CodexHub 返回的 bundled remote item，就可能继续让 UI 拿到 `plugins~codexhub-bundled-*`，直到 app-server 重新从 `/backend-api/ps/plugins/installed` 刷新或进程重建。

这说明 `plugins~codexhub-bundled-*` 的来源不一定是磁盘 `config.toml`，也不一定是 `~/.codex/cache/remote_plugin_catalog`；它可以是 app-server 内存态。CodexHub 当前代码侧的原则仍不变:

```text
不要返回 bundled remote installed item
只允许 bundled remote detail 做旧 ID 只读兼容
不要恢复 bundled mock
```

2026-06-29 已实现代码修复:

在 CodexHub 初始化 Codex App 配置时，做一个只针对旧 CodexHub mock 的清理:

1. 删除 `openai-bundled` 插件 cache 下 `.codex-remote-plugin-install.json` 里由旧 CodexHub 写入的 remote id。
2. 删除 `~/.codex/cache/remote_plugin_catalog/*.json` 中包含 `plugins~codexhub-bundled-` 的旧目录缓存文件。
3. 不删除 `~/.codex/plugins/cache/openai-bundled/*` 插件本体。
4. 不删除 `[plugins."computer-use@openai-bundled"]`、`browser@openai-bundled`、`chrome@openai-bundled` 等 Codex App 自己的安装配置。
5. 不清理 `openai-curated` 缓存，因为它是当前 fallback 需要保留的本地 GitHub/curated 插件来源。

目的不是恢复 bundled mock，而是把旧 mock 产生的 remote 身份从本地状态里剥离，让 `computer-use` 回到 Codex App 本地 `marketplacePath` 详情链路。

对应代码在 `src/codex_app_config.rs`:

```text
configure_codex_app()
  -> clear_legacy_codexhub_bundled_plugin_state()
```

这次没有运行测试或启动 Codex App，验证交给用户手动完成。

2026-06-29 用户验证结果:

用户按新版本编译、通过 GUI 初始化 Codex 配置并重启 Codex App 后，点击 `computer-use` 详情页仍显示:

```text
未找到插件
无法从插件市场详情页加载此插件。
```

当前只能确认 UI 仍进入了“插件市场详情页加载失败”的状态；截图本身不能区分这次是继续使用旧 `plugins~codexhub-bundled-*` remote id，还是本地 `openai-bundled` marketplace detail 读取失败。下一步只做静态逻辑分析和代码修正，不由 CodexHub 侧运行验证。

2026-06-29 兼容修正结论:

前面“完全不给 bundled remote detail fallback”的策略过硬。实际 UI 仍可能命中旧的 bundled remote item；如果这时 CodexHub 返回 404，用户就只能看到“未找到插件”。新的边界改成:

1. `/backend-api/ps/plugins/list` 和 `/backend-api/ps/plugins/installed` 仍不返回 `openai-bundled` 插件，避免制造新的重复列表项。
2. `/backend-api/ps/plugins/{plugin_id}` 可以对旧 `plugins~codexhub-bundled-*` 和 `*@openai-bundled` 做只读详情兼容。
3. 这个兼容只用于“旧 UI/cache/source selection 已经点进来了”的场景，不把 bundled 插件重新发布到 remote 列表。
4. 详情内容从 Codex App 自己写出的本地 `~/.codex/.tmp/bundled-marketplaces/openai-bundled` marketplace 读取，仍然不模拟 bundle 下载或安装。

2026-06-29 静态修正:

1. 上游 `plugin/read` 在 remote 分支会先校验 `pluginName`，只允许 ASCII 字母、数字、`_`、`-`、`~`。所以 `computer-use@openai-bundled` 不可能是 Codex App remote read 正常传入的 pluginName；它只作为 CodexHub HTTP 兼容入口保留。
2. 真正需要兜底的是旧 remote id，例如 `plugins~codexhub-bundled-computer-use`。CodexHub 对这类请求返回 detail 时，响应里的 remote `id` 必须保持为请求进来的旧 id，避免 app-server 读到详情后把 remote identity 换成另一个值。
3. 这仍然不是恢复 bundled mock。`list` 和 `installed` 不发布 bundled 插件，只有 detail endpoint 对旧 id 返回只读 directory item。

## 2026-06-29 `/ps/mcp` 结论

`POST /backend-api/ps/mcp` 不是 `computer-use` 插件详情展示接口。上游源码确认它是 hosted plugin runtime / apps MCP 的 streamable HTTP 入口，由 `hosted_plugin_runtime_mcp_server_config` 从 `chatgpt_base_url` 推导出来。

不要在 CodexHub 里伪造 `/ps/mcp` 成功响应。伪造空 MCP runtime 会让 Codex App 进入错误的 app-server/MCP 启动链路，可能造成重连或重启。当前目标是修 `computer-use` 插件详情，范围只保留:

```text
/backend-api/ps/plugins/*
/backend-api/plugins/featured
```

## 实现计划

1. 更新文档，明确本地插件 mock 废弃。
2. `src/web/plugins.rs` 保持最小 fallback 路由，不恢复旧的完整 mock。
3. `src/web.rs` 继续注册 `plugins::router()`。
4. 保留 `src/codex_app_config.rs` 里的:
   - `chatgpt_base_url` 注入。
   - 默认 `ai-gateway` provider 注入。
   - 清理旧版 CodexHub 写入的 `openai-bundled` marketplace。
   - `openai-curated` cached marketplace 注入。
   - 插件相关 blocking feature flag 清理。
5. 确认 local auth 默认是非 workspace 个人形态，例如 `plan_type = "pro"`，不要改成 team/business/enterprise。
6. 更新测试:
   - 删除 `web::plugins` 相关测试。
   - 保留/补充 `codex_app_config` 测试，确认配置注入和 feature 清理仍然生效。

## 验证计划

聚焦验证:

```powershell
cargo fmt
cargo test codex_app_config --features gui
```

如果通过，再跑:

```powershell
cargo test --features gui
```

手工验证:

1. 运行 `codexhub configure-codex-app`。
2. 确认 `~/.codex/auth.json` 是 `chatgptAuthTokens`。
3. 确认 `~/.codex/config.toml` 有 `chatgpt_base_url`、`ai-gateway`，且没有旧版 CodexHub 写入的 `marketplaces.openai-bundled`。
4. 如果本机存在 `~/.codex/.tmp/plugins/.agents/plugins/marketplace.json`，确认 `config.toml` 有 `marketplaces.openai-curated`。
5. （历史验证项，当前已废弃）当时要求 `config.toml` 没有 `apps = false`、`plugins = false` 等阻断项。当前策略会主动写入 `apps = false`，只继续禁止 `plugins = false` 等本地插件阻断项；详见文末 2026-07-16 结论。
6. 重启 Codex App，确认 remote-control 能连接 CodexHub。
7. 在 Codex App 里确认 bundled plugins 不再出现重复项，且 cached curated 插件能显示。

## 历史记录

2026-06-28 曾实现过完整本地 `/backend-api/ps/plugins/*` mock，并通过单元测试和 smoke test。该方案现已废弃，原因是新版本 Codex App 自身的 bundled plugin 路径已经可用，继续 mock 远端插件目录会增加复杂度并带来重复插件风险。2026-06-29 保留的只是基于 `openai-curated` 缓存的最小 remote catalog fallback。

## 2026-06-29 最终结论（computer-use 收敛）

经过完整的日志和源码交叉验证，`computer-use` 问题正式收敛。结论如下，后续不再当作未决问题反复排查。

1. `computer-use` 功能本身一直正常。Codex App 日志里 `computer-use native pipe startup ready` 每次都成功，重启后实际调用（让模型操作电脑）可用。
2. 唯一遗留是插件页点进 `computer-use` 详情时显示「未找到插件 / 无法从插件市场详情页加载此插件」。这是 Codex App 前端在 ChatGPT-backend auth + `chatgpt_base_url` 指向 CodexHub 模式下，对 bundled 本地插件详情的展示行为问题，不影响功能使用。
3. CodexHub 侧已无可补接口。实测当前运行进程对 detail 和 skill 两个接口都返回 200：

   ```text
   GET /backend-api/ps/plugins/local~openai-bundled~computer-use -> 200 (release.skills count=1)
   GET /backend-api/ps/plugins/local~openai-bundled~computer-use/skills/computer-use -> 200 (skill_md ~40KB)
   ```

   重启后用户点击详情时，app-server 甚至没有再向 CodexHub 发出 detail 请求（chain log 无 `/ps/plugins/{id}`），说明失败发生在前端，CodexHub 层面已无可改。

4. 关键机制：CodexHub 为支持 remote-control 必须写入 ChatGPT-backend 形态 auth（`uses_codex_backend = true`）。上游 `plugin/list` 在该模式下走 remote global catalog（见 `references/codex-main/codex-rs/app-server/src/request_processors/plugins.rs` 的 `use_remote_global_catalog`），bundled 本地插件在前端详情归属判定上被错当远端插件，于是详情页失败。
5. `features.remote_plugin = false` 已验证不可取：它确实关闭了远端 catalog（chain log 不再出现 `/ps/plugins`），但会让「由 OpenAI 提供」标签页空转卡在「正在加载插件…」，且不修复 detail 展示。保持 `remote_plugin` 默认开启时，列表刷新快、computer-use 功能正常，仅详情页无法展示。因此**不把 `remote_plugin = false` 写入 `configure-codex-app`**。
6. 持续出现的 `sa_server_request_failed net::ERR_CONNECTION_CLOSED`/401（`/wham/tasks/list`、`/wham/usage`、`/wham/accounts/check`）是 App 直连 `chatgpt.com` 后端的固有噪音，不经过 CodexHub，与插件详情无关。

2026-06-29 当时的最终状态：保持 `chatgptAuthTokens` auth、`remote_plugin` 默认开启、bundled 走 Codex App 本地 marketplace，CodexHub 仅保留 `openai-curated` 最小 fallback 与 bundled 旧 ID 只读 detail 兼容。`computer-use` 详情页展示问题归类为 Codex App 前端行为，接受现状，不再继续改 CodexHub 接口。Provider 后续已切换为 Actor Authorization 方案，当前界面结论以紧随其后的 2026-07-16 小节为准。

## 2026-07-16 `requires_openai_auth=false` 下的市场可见性

本节是 Provider 切换到 Actor Authorization 方案后的新结论，优先级高于前文 2026-06-29 基于 `requires_openai_auth=true` 的界面判断。前面的接口兼容和 bundled 插件结论仍有效，但“cached curated 插件一定会显示”的假设已经不成立。

当前默认 Provider 为：

```toml
[model_providers.ai-gateway]
name = "ai-gateway"
requires_openai_auth = false
http_headers = { x-openai-actor-authorization = "codexhub-local" }
```

该配置用于同时获得原生 `web.run` 和本地压缩。Codex app-server 会因此向 renderer 报告 `authMethod=null`、`requiresAuth=false`，不会因为 `auth.json` 中仍有 `chatgptAuthTokens` 而报告 ChatGPT 登录态。

Codex App `26.707.91948` renderer 的插件过滤函数只把以下 Auth Method 视为可显示 OpenAI curated marketplace：

```text
chatgpt
apikey
amazonBedrock
```

其他值（包括 `null`）会过滤：

```text
openai-curated
openai-curated-remote
```

实机验证结果：

1. `plugins`、`remote_plugin` 和远端插件 Statsig gate 都处于开启状态；
2. 左侧插件入口存在，`openai-bundled` 与 `openai-primary-runtime` 的 10 个本地插件可见；
3. `~/.codex/.tmp/plugins/.agents/plugins/marketplace.json` 中仍有 25 个经过过滤的本地兼容插件；
4. React Query 把对应请求标记为 `openai-curated-marketplaces-hidden`，因此这 25 个插件没有进入页面；
5. CodexHub 仍收到 `/backend-api/ps/plugins/list`，不是接口 404、目录丢失或 `remote_plugin` gate 关闭。

`features.apps=false` 是另一项独立策略。它只关闭需要官方 ChatGPT `codex_apps` MCP 后端的 Apps/Connectors；Computer Use、Chrome、primary runtime plugin 和普通 skill 不依赖这个开关。CodexHub 未实现官方 Apps streamable HTTP 后端前，不应仅为恢复图标而开启它。

## 2026-07-21 增强模式本地 curated 显示修复

最终没有迁移磁盘目录或配置身份，也没有伪造全局 `authMethod`。Codex App `26.715.8383` 的 renderer 一方面会在 `authMethod=null` 时过滤 `openai-curated`，另一方面明确把 `codex-official` 识别为内置市场。增强模式利用这一点做窄范围响应适配：

1. 跟踪新版 `mcp-request(request.method=plugin/list)` 及对应 `mcp-response`，同时保留旧版 `vscode://codex/list-plugins` / `fetch-response` 兼容；
2. 只把返回中的顶层本地市场名 `openai-curated` 临时改为 `codex-official`；
3. 不改 `marketplacePath`、`<plugin>@openai-curated` ID、插件名、安装/启用状态或 `config.toml`；
4. 不处理 `openai-curated-remote`，继续保持远端 curated 市场不可见；
5. 安装和详情仍通过本地绝对路径进入 app-server，因此展示别名不会污染持久状态。

真实 renderer 消息回放确认：本地市场通过过滤，路径和插件身份保持不变，远端 curated 与 bundled 市场均未修改。完整目录可以显示；featured 推荐位仍可能被 renderer 按原始 `@openai-curated` 后缀过滤，这是刻意保留的限制，不能为推荐位改写插件 ID。

该修复只在“增强模式启动 Codex”中生效。普通启动、Codex CLI 和 VS Code 插件不经过 CDP 注入，不受影响。

### 2026-07-22 插件消息协议变更

新版 Codex App 已把插件目录从 renderer `fetch` 桥接迁移到 app-server MCP 消息：

```text
codex-message-from-view:
  type=mcp-request
  request.method=plugin/list
  request.id=<id>

window message:
  type=mcp-response
  message.id=<id>
  message.result.marketplaces=[...]
```

旧实现只匹配 `fetch + vscode://codex/list-plugins`，因此桥接虽然已安装，`pluginCatalogResponsesAdapted` 始终为 `0`。修复后用请求 ID 关联 `plugin/list` 响应，只改 `message.result.marketplaces[].name`。实机 CDP 热回放共命中 3 个目录响应，React Query 中本地 curated 市场显示为 `codex-official`、`pluginCount=25`，并正常进入插件图标加载流程。

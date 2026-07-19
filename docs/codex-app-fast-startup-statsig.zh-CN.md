# Codex App 快速启动与 Statsig 兼容说明

> 状态：`localhost:8000` 快速启动方案已于 2026-07-16 废弃。本文后续内容仅保留为历史协议记录。
>
> 当前 CodexHub 默认设置 `CODEX_API_BASE_URL=http://127.0.0.1:3847/api`，在 3847 主服务上复用旧 dev API 的轻量协议语义，不再提供 8000 listener，也不再显示快速启动开关。需要同步 Codex App 前端模型列表时，用户从 Codex 接入页主动选择“增强模式启动 Codex App”。

日期: 2026-07-02

这份文档记录 CodexHub 开启 Codex App 快速启动时的后端切换原理、Statsig feature gate 的构造规则，以及 Codex App 升级后如何快速复查。

2026-07-19 的增强启动中文失效与冷启动 500 排查过程，见
[Codex App 增强启动语言问题复盘](codex-app-enhanced-language-debugging-postmortem.zh-CN.md)。

## Windows 增强启动的安全边界

历史 Windows 实现曾启动隐藏 PowerShell，并组合使用 `-ExecutionPolicy Bypass`、`-EncodedCommand`
和 `Add-Type` 动态编译 `IApplicationActivationManager` COM 声明。代码本身只用于给商店版 Codex App
传入 CDP 参数，但这组行为特征与恶意脚本高度相似，可能触发 Windows 安全中心或第三方安全软件的
重复告警。

当前实现已经改为 Rust 直接调用 Windows 原生 COM：

```text
CoInitializeEx
CoCreateInstance(ApplicationActivationManager)
IApplicationActivationManager::ActivateApplication
```

激活目标固定为官方商店包 AUMID `OpenAI.Codex_2p2nqsd0c76g0!App`。CodexHub 不申请 Defender
排除项、不关闭实时保护，也不再为增强启动创建 PowerShell 进程。日志中的
`event=windows_native_activation` 表示走了原生激活路径。

Codex App 自身仍可能为了读取本机应用图标而启动 PowerShell；启动阶段还可能另外初始化 WSL/Hyper-V
网络。这些行为不是 CodexHub 增强启动器创建的。判断来源时应对照父进程和 PowerShell Operational
日志，而不是仅凭它出现在增强启动之后就归因于 CodexHub。

本机历史证据还需要区分两种不同事件：

- Defender `2050` 表示文件被上传以进一步分析，不等于已经检出恶意软件。`~/.codex/config.toml`
  的此类事件集中发生在 2026-06-07 和 2026-06-25，早于增强模式启动功能，因此不能归因于
  CDP。短时间内出现多个不同 SHA256，说明配置当时被连续改写，Defender 将每个版本视为新样本。
- Defender `1116/1117` 才是实际检出和处置。本机命中过的 `ClickFix`、`PowhidSubExec` 都指向
  PowerShell 命令行行为，不是 `config.toml` 内容本身被判定为木马。

CodexHub 现在会在原子写入 `config.toml`、`auth.json` 或其备份前比较完整字节；内容未变化时不再
替换文件，避免无意义地产生新文件版本、修改时间和安全扫描。Codex App 自己仍会在启动时通过
`config/batchWrite` 更新它负责的 Node REPL / 插件配置，并可能以 `-ExecutionPolicy Bypass`、
`-EncodedCommand`、`Add-Type` 组合解析本机应用图标；这部分属于官方 App 当前实现，CodexHub 不应
通过关闭 Defender、添加排除项或阻止配置写入来掩盖。

## 当前决策

快速启动只解决一个问题: 在网络不稳定或无 VPN 时，避免 Codex App 启动阶段长时间等待官方 `chatgpt.com/backend-api`。

当前 Codex App `26.715.4045` 的增强模式不再依赖 8000 端口。增强脚本只在找到结构完整的官方
Statsig evaluation 缓存时，才在首个 Statsig client 的异步初始化之前使用 SDK 自己的
`dataAdapter.setData()` 和 `initializeSync()` 快速启动；官方缓存中的 runtime、插件、浏览器和其他
dynamic config 原样保留。找不到完整官方缓存时，不再用本地极简 payload 冒充官方配置，而是让
Codex App 执行官方 `initializeAsync()`，初始化完成后再叠加 CodexHub 的局部覆盖。这样首次冷启动
可能恢复为官方网络速度，但不会为了提速静默丢失官方能力。该逻辑只作用于用户主动选择的增强模式，
不改变普通启动、CLI 或 VS Code 插件。

快速启动不是要完整模拟官方后端。CodexHub 只维护必要兼容层:

1. Codex App 能快速完成启动。
2. CodexHub 需要的 remote-control 入口保持可见。
3. bundled/local plugin 可见性不被错误 gate 关闭。
4. i18n/bootstrap 这类基础配置不阻塞 UI。
5. 恢复超长会话时按页加载历史，不在启动后台排空全部 turn。

以下能力当前不作为快速启动兼容目标:

1. profile / avatar 云端资料接口，例如 `/wham/profiles/me`、`/wham/profiles/me/photo`。
2. referral、rate-limit credits、workspace messages 等官方运营类接口。
3. Codex cloud environments 完整 CRUD。
4. 官方 cloud task / worktree snapshot 上传链路。
5. 手动 remote pair 接口 `/wham/remote/control/client/pair`。CodexHub 当前是本地默认接入 remote-control，不依赖这个官方配对流程。

## 后端切换原理

Codex App 打包前端里有两套 backend base URL:

```text
prodApiBaseUrl = https://chatgpt.com/backend-api
devApiBaseUrl  = http://localhost:8000/api
```

选择逻辑是:

1. 如果 `CODEX_API_BASE_URL` 有值，优先使用它。
2. 否则如果 `CODEX_API_ENDPOINT=localhost`，使用 `http://localhost:8000/api`。
3. 否则使用 `https://chatgpt.com/backend-api`。

CodexHub 快速启动开启时会设置:

```text
CODEX_API_ENDPOINT=localhost
CODEX_API_BASE_URL unset
```

快速启动关闭时会清理这些环境变量，让 Codex App 回到官方 backend。

因此两种模式的行为不同:

| 模式 | backend | 特点 |
| --- | --- | --- |
| 默认官方模式 | `https://chatgpt.com/backend-api` | 慢，但官方返回完整账号、Statsig、插件、远控配置 |
| 快速启动模式 | `http://localhost:8000/api` | 快，但 CodexHub 必须本地构造必要后端契约 |

这解释了一个历史现象: 不做快速启动时只是慢，但很多功能还能用；做快速启动后，任何缺失的本地 contract 都会让相关功能消失或降级。

## 为什么重点是 Statsig

Codex App 前端用 Statsig 控制大量功能显隐。典型路径:

```text
POST /wham/statsig/bootstrap
```

返回体中 `statsigPayload` 是一个 JSON 字符串，里面包含:

```json
{
  "response_format": "init-v2",
  "feature_gates": {},
  "dynamic_configs": {},
  "layer_configs": {},
  "values": {},
  "user": {}
}
```

前端读取 gate 时，如果某个 gate 缺失，通常等价于 false。结果就是:

1. 手机/远控入口可能不显示。
2. computer use / browser use 插件可能被判断为不可用。
3. 插件安装流程可能进入错误分支。
4. i18n layer 不完整时，语言相关 UI 可能降级。

但是不能把所有 gate 全部设成 true。原因:

1. 有些 gate 是官方灰度或付费能力。
2. 有些 gate 打开后会触发 CodexHub 没实现的 `/wham/*` 或 `/aip/*` 路由。
3. 有些 gate 是反向逻辑，true 反而关闭旧流程。
4. 全量 true 会让 UI 进入“看似可用、实际后端不存在”的状态，问题更难定位。

结论: 只打开 CodexHub 明确支持的 gate；未知 gate 默认 false 或不返回。

## Statsig Payload 构造规则

feature gate 推荐统一构造成:

```json
{
  "v": true,
  "r": "codexhub-local",
  "s": [],
  "i": "userID"
}
```

字段含义:

| 字段 | 含义 |
| --- | --- |
| `v` | gate 值，true/false |
| `r` | reason，标记为 CodexHub 本地构造 |
| `s` | secondary exposures，当前置空 |
| `i` | id type，保持前端兼容 |

dynamic config / layer config 推荐使用间接引用:

```json
{
  "dynamic_configs": {
    "107580212": {
      "v": "codexhub_model_list_config",
      "r": "codexhub-local",
      "s": [],
      "i": "userID",
      "ue": false,
      "p": true
    }
  },
  "values": {
    "codexhub_model_list_config": {}
  }
}
```

规则:

1. `dynamic_configs[id].v` 或 `layer_configs[id].v` 填一个本地 value key。
2. 真实配置放在 `values[value_key]`。
3. 未确认 schema 的 config 保持空对象或保守默认值。

## 当前必须维护的 Gate

这些 gate 和 CodexHub 当前快速启动目标有关。

| Gate ID | 建议值 | 作用 | 说明 |
| --- | --- | --- | --- |
| `1042620455` | true | remote-control / slingshot 入口 | 缺失会导致底部手机/远控入口消失 |
| `4114442250` | true | `features.remote_connections` fallback | remote connection visibility 使用 |
| `824038554` | true | remote/mobile/browser-use 相关 UI | 多处远控和浏览器设置使用 |
| `410065390` | true | browser/computer use 可见性 | `use-is-plugins-enabled` 里检查外部 browser/computer use 能力 |
| `2296472986` | true | 插件安装流程中的 remote-control / locked computer use 判断 | 影响 plugin install flow |
| `3446105535` | true | `suppressResumeHistoryDrain` | 启动只恢复最近 5 个 turn，旧历史在用户滚动时分页加载，避免超长会话启动时被完整排空 |
| `2055603567` | false 或不返回 | 官方 mobile setup / server pairing 流程 | 设为 true 会触发 `remoteControl/pairing/start`，在 CodexHub 当前本地 enrollment 模式下会报 `remote control pairing is unavailable until enrollment completes` |
| `3936985709` | false 或不返回 | remote pair 分支反向 gate | 代码里存在 `!gate(3936985709)`；当前不依赖官方 pairing 流程，不要用它解决本地兼容问题 |

### 历史 14 个 gate 的完整清单

旧版 CodexHub 曾把以下 14 项全部强制设为 true。Codex App `26.715.4045` 的 renderer bundle
已经可以确认前 13 项的用途；`824038554` 在当前安装包中已没有引用，只能保留旧版结论。

| Gate ID | 已确认语义 | 当前处理 |
| --- | --- | --- |
| `1834314516` | Owl 浏览器能力组合：`OwlAuth`、`OwlAutofillAndPasswords`、`OwlDownloads`、`OwlPermissions`、`OwlWebViewEnhancements`；也用于浏览器资料导入、外链图标、调试页等 UI | 保留官方值 |
| `1714131075` | `OwlWebViewEnhancements` | 保留官方值 |
| `72045066` | `OwlAuth`、`OwlAutofillAndPasswords` | 保留官方值 |
| `2982604767` | `OwlExtensions` | 保留官方值 |
| `2177625257` | `OwlHistory` | 保留官方值 |
| `3657624089` | `OwlPermissions` | 保留官方值 |
| `3245360288` | `OwlPrinting` | 保留官方值 |
| `3646210497` | `OwlOpenAIGoLinks` | 保留官方值 |
| `1186680773` | 模型列表和动态工具中的 Ultra reasoning effort | 当前不属于核心兼容目标，保留官方值 |
| `1042620455` | remote-control / slingshot 入口 | CodexHub 强制 true |
| `4114442250` | `features.remote_connections` fallback 和侧栏远程连接状态 | CodexHub 强制 true |
| `824038554` | 旧版远控、移动端或 browser-use 相关 UI；当前 `26.715.4045` 已无引用 | 暂时保留 true，升级时可移除验证 |
| `410065390` | external browser/computer-use、Chrome 扩展及移动端设置入口 | CodexHub 强制 true |
| `2296472986` | 插件安装时的 remote-control / locked computer-use 判断 | CodexHub 强制 true |

中文界面不属于这 14 个 feature gate。当前 renderer 读取的是 i18n layer `72216192`：

```json
{
  "enable_i18n": true,
  "locale_source": "FIRST_AVAILABLE"
}
```

`72216192` 只负责“是否启用 i18n”和“从哪一类 locale 取值”，不直接把页面切换成中文。
Codex App `26.715.4045` 的实际选择顺序是:

1. `desktop.localeOverride` 有值时，永远优先使用它；例如设置为 `en-US` 就会保持英文。
2. `locale_source=SYSTEM` 时使用系统 locale。
3. `locale_source=FIRST_AVAILABLE` 时依次尝试 IDE locale、系统 locale。
4. 默认 `IDE` 时使用 App 主进程 `locale-info` 返回的 `ideLocale`。

其中 `locale-info` 由 App 主进程提供，CodexHub 的 `[desktop]` 之外的 GUI `language` 字段不会自动修改它。
因此，CodexHub 自己显示中文，不代表 Codex App 一定显示中文；需要检查 Codex App 的
`desktop.localeOverride` 是否为空或为 `zh-CN`。

### Statsig V1/V2 格式边界

Statsig SDK 会根据 `response_format` 选择容器。两种格式不能混写:

```json
{
  "response_format": "init-v2",
  "layer_configs": {
    "72216192": { "v": "codexhub_i18n_layer_config" }
  },
  "values": {
    "codexhub_i18n_layer_config": {
      "enable_i18n": true,
      "locale_source": "FIRST_AVAILABLE"
    }
  }
}
```

V1 直接把配置放在 `layer_configs[id].value`；V2 必须使用 `layer_configs[id].v` 和
`values[v]`。增强模式会保留输入响应的格式，并同时修补模型 dynamic config、feature gate
和 i18n layer；不能只给 i18n layer 加 `v`，也不能在 V2 响应里继续使用 V1 的 `value`。
否则 Statsig 可能显示模型列表，但 `getLayer("72216192")` 返回空值，语言 provider 会退回英文。

### 为什么官方切换语言能立即生效

CDP 实机对比确认，官方切换语言依次执行：

1. i18n layer 在语言 provider 首次挂载前已经是 `enable_i18n=true`；
2. 设置页先乐观更新 React Query 中的 `localeOverride`；
3. 再请求 `vscode://codex/set-setting` 持久化，并通过 `get-settings` 失效重取；
4. 语言 provider 观察到 locale 变化后动态加载 `zh-CN-*.js`，不需要刷新页面。

旧增强脚本与它的差别不是“少发一次 set-setting”，而是修改时机和数据格式都不对：renderer
已经用 `enable_i18n=false` 挂载后，脚本才调用 Statsig 私有 store；同时还曾把无
`response_format` 的 V1 响应写成 `v + values[...]`。当前 renderer 的编译后 layer hook 会保留首次
取得的 Layer 对象，因此晚到的 store 修改即使触发 `values_updated`，也不能可靠地启动语言包加载。

增强脚本现在会包装 Statsig store 的 `setValues`，在 SDK 创建 V1/V2 容器之前修补模型、gate 和
i18n layer。已挂载页面还需要处理 React Compiler 的 memo cache：当前 renderer 会把首次读取结果
缓存为 `[Layer 对象, enable_i18n]`。如果 CDP 接入时该槽已经是 false，仅修改 Statsig store 或 Layer
对象不会重新执行语言包加载 effect。脚本会按 layer ID `72216192` 定位这一个缓存槽，使其失效，再
通过 Statsig 自己的 values update 驱动 React 重渲染；不会改写用户的 `localeOverride`，也不依赖
当前版本的混淆函数名。启动报告还必须满足 `i18nEnabled=true`，不能再仅凭模型列表和普通 gate
正常就报告成功。

冷启动时 renderer 可能在 CDP 脚本接入前已经错过 bootstrap，此时 Statsig store 的 source 会是
`NoValues`，并且 `getValues()` 没有任何配置容器。增强脚本现在不会用一份本地极简对象替换这个状态：
如果存在完整官方缓存，就以缓存为底座快速初始化；如果不存在，就保留官方 `initializeAsync()`，
等待官方 evaluation 进入 store 后再做局部 overlay。增强报告中的
`fast_initialize_source=official-network` 表示采用了后一条保守路径。这样 CodexHub 不会伪造一个
缺少插件、runtime 或浏览器配置的“完整” Statsig 响应。

增强启动升级后会清除这些 gate 中由旧版 CodexHub 写入且 `rule_id/r` 为
`codexhub-local` 的值，但不会删除或改写官方 Statsig 返回的值。这样可以撤销旧注入副作用，同时保留
官方灰度配置。

## 超长会话恢复与 `tail_history`

2026-07-18 对四份 `codex.exe` 转储和两次最新启动日志的联合排查确认：

1. 转储均为 app-server 原生进程异常，最新异常码为 `0xC0000409`，WER 将其归类为
   `FAST_FAIL_UNEXPECTED_HEAP_EXCEPTION`，故障模块为 `ntdll.dll`；
2. 两次最新崩溃都在恢复会话 `019f122d-3c78-7c82-88a6-34a483c36dbd` 时发生；
3. 对应 JSONL 已约 146 MB、超过 4.2 万行，启动阶段连续请求 `thread/turns/list`，来源为
   `tail_history`；
4. `skills cache cleared` 或插件同步只是进程退出前最后打印的日志，不是两次崩溃的共同触发器；
5. Codex App `26.715.4045` renderer 已提供 gate `3446105535`，其语义名称为
   `suppressResumeHistoryDrain`。

CodexHub 在本地 bootstrap 和增强模式启动的增量 Statsig 注入中都将该 gate 设为 true。
启用后，恢复会话只先读取最近 5 个 turn；更早历史仍保留在原 JSONL 中，用户向上滚动时再分页读取。
这项修复不删除、不截断、不归档，也不改写会话内容。

该 gate 针对的是已经确认的超长历史启动触发路径，不能承诺消除所有 Windows 原生堆异常。若开启后
仍在不加载该会话时崩溃，应按新的转储和时间线独立排查。

## 当前必须维护的 Layer / Dynamic Config

| ID | 类型 | value key | 建议值 | 作用 |
| --- | --- | --- | --- | --- |
| `107580212` | dynamic config | `codexhub_model_list_config` | `{}` | 模型列表配置，空对象走默认 |
| `2096615506` | layer config | `codexhub_primary_runtime_config` | `{}` | primary runtime 配置，空对象走默认 |
| `72216192` | layer config | `codexhub_i18n_layer_config` | `{ "enable_i18n": true, "locale_source": "FIRST_AVAILABLE" }` | 开启 i18n layer |

注意: i18n layer 只决定语言能力是否启用和 locale 来源。用户语言的最终值属于 Codex App
自己的应用设置。快速启动兼容层不应伪造 `localeOverride`，也不应覆盖用户明确选择的语言；
如果用户要使用中文，应在 Codex App 的语言设置中选择中文，或确保 `desktop.localeOverride = "zh-CN"`。

## 路由兼容边界

当前快速启动不是完整官方 backend，所以路由遵循最小原则。

保留必要路由:

1. `/api/wham/accounts/check`
2. `/api/wham/statsig/bootstrap`
3. `/api/wham/onboarding/context`
4. `/api/wham/remote/control/clients`
5. `/api/wham/remote/control/clients/{client_id}`
6. `/api/wham/remote/control/mfa_requirement`
7. `/api/wham/tasks/list`
8. `/api/wham/environments`
9. `/api/wham/apps`
10. `/api/wham/usage`

可考虑补但不是本次重点:

1. `/api/accounts/{account_id}/settings`: 如果 computer use 可见性仍异常，需要返回 `beta_settings.windows_computer_use=true`。
2. `/api/wham/settings/user` 和 `/api/wham/settings/configs/user-preferences`: 只有需要兼容官方云端偏好同步时才补；它们不是 Codex App 初始语言选择的唯一来源。
3. `/api/wham/analytics-events/events`: 可 no-op 204，减少 404 噪声。

明确不追求完整兼容:

1. `/api/wham/profiles/*`
2. `/api/wham/referrals/*`
3. `/api/wham/usage/*` 细分图表接口
4. `/api/wham/worktree_snapshots/*`
5. `/api/wham/environments/*` 完整云环境管理
6. `/api/aip/connectors/*` 官方 connector 连接流程

## Codex App 升级后的复查流程

每次 Codex App 更新后，按这个流程复查。

### 1. 确认 backend base URL 选择逻辑

检查新版本是否仍然使用:

```text
CODEX_API_BASE_URL
CODEX_API_ENDPOINT=localhost
http://localhost:8000/api
https://chatgpt.com/backend-api
```

如果官方改了 `localhost` 端口、路径或环境变量名，快速启动会直接失效。

### 2. 扫描前端请求路径

从安装包 JS bundle 中扫描:

```text
/wham/
/accounts/
/aip/
/connectors/
```

只新增必要路由，不要因为扫描到就全部实现。

### 3. 扫描 Statsig 使用点

重点搜索:

```text
checkGate
getDynamicConfig
getLayer
feature_gates
dynamic_configs
layer_configs
```

对每个新增数字 ID 判断:

1. 是否影响 remote-control 入口。
2. 是否影响 computer use / browser use 插件可见性。
3. 是否影响 bundled/local plugin 页面。
4. 是否是反向 gate。
5. 打开后是否会触发未实现的官方后端接口。

只有满足 CodexHub 支持范围的 gate 才设 true。

### 4. 更新测试

`src/remote_control_backend/compatibility.rs` 的测试需要至少断言:

1. `1042620455` 为 true。
2. `410065390` 为 true。
3. `2296472986` 为 true。
4. `3446105535` 为 true。
5. `2055603567` 不为 true。
6. `3936985709` 不为 true。
7. 本地 bootstrap 只包含 6 个已确认 gate，旧兼容集合不再被强制开启。
8. `72216192` layer 存在且 `enable_i18n=true`。
9. 增强启动报告中的 `i18nEnabled` 为 true。

### 5. 手动验证

快速启动开启后验证:

1. Codex App 能秒开。
2. 底部手机/远控入口存在。
3. computer-use 仍显示为可用。
4. 插件列表不会长时间卡远端刷新。
5. 日志里没有关键 `/wham/statsig/bootstrap` 或 `/wham/accounts/check` 失败。
6. 未实现的官方接口如果出现 404，应确认是否属于明确不兼容范围。
7. 恢复超长会话时只先请求最近一页历史，不再自动排空全部 `tail_history`。
8. 在 Codex App 设置中切换中文/英文时，界面无需刷新即可更新。

## 维护原则

1. Statsig 只补必要 gate，不做全量 true。
2. 路由只补 CodexHub 实际需要的 contract。
3. 不用 API key auth 规避问题，因为 API key 模式不支持 CodexHub 需要的 remote-control。
4. 历史版本曾允许关闭快速启动并回到官方 backend；当前版本已改为默认直连 CodexHub 3847 backend API，不再提供开关。
5. 每次 Codex App 更新后先扫描再改，不根据旧 ID 盲目迁移。

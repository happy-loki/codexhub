# Codex App 快速启动与 Statsig 兼容说明

> 状态：`localhost:8000` 快速启动方案已于 2026-07-16 废弃。本文后续内容仅保留为历史协议记录。
>
> 当前 CodexHub 默认设置 `CODEX_API_BASE_URL=http://127.0.0.1:3847/api`，在 3847 主服务上复用旧 dev API 的轻量协议语义，不再提供 8000 listener，也不再显示快速启动开关。需要同步 Codex App 前端模型列表时，用户从 Codex 接入页主动选择“增强模式启动 Codex App”。

日期: 2026-07-02

这份文档记录 CodexHub 开启 Codex App 快速启动时的后端切换原理、Statsig feature gate 的构造规则，以及 Codex App 升级后如何快速复查。

## 当前决策

快速启动只解决一个问题: 在网络不稳定或无 VPN 时，避免 Codex App 启动阶段长时间等待官方 `chatgpt.com/backend-api`。

快速启动不是要完整模拟官方后端。CodexHub 只维护必要兼容层:

1. Codex App 能快速完成启动。
2. CodexHub 需要的 remote-control 入口保持可见。
3. bundled/local plugin 可见性不被错误 gate 关闭。
4. i18n/bootstrap 这类基础配置不阻塞 UI。

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
| `2055603567` | false 或不返回 | 官方 mobile setup / server pairing 流程 | 设为 true 会触发 `remoteControl/pairing/start`，在 CodexHub 当前本地 enrollment 模式下会报 `remote control pairing is unavailable until enrollment completes` |
| `3936985709` | false 或不返回 | remote pair 分支反向 gate | 代码里存在 `!gate(3936985709)`；当前不依赖官方 pairing 流程，不要用它解决本地兼容问题 |

已有但需要继续观察的 gate:

| Gate ID | 当前建议 | 说明 |
| --- | --- | --- |
| `1186680773` | 可保持 true | ultra reasoning effort / model list 相关，不是快速启动核心 |
| `1834314516` | 可保持 true | debug / external link target 等 UI 使用，风险较低 |
| `1714131075`、`72045066`、`2982604767`、`2177625257`、`3657624089`、`3245360288`、`3646210497` | 保持现状，升级时复查 | 当前来源为历史兼容集合，未确认具体业务语义前不要扩散 |

## 当前必须维护的 Layer / Dynamic Config

| ID | 类型 | value key | 建议值 | 作用 |
| --- | --- | --- | --- | --- |
| `107580212` | dynamic config | `codexhub_model_list_config` | `{}` | 模型列表配置，空对象走默认 |
| `2096615506` | layer config | `codexhub_primary_runtime_config` | `{}` | primary runtime 配置，空对象走默认 |
| `72216192` | layer config | `codexhub_i18n_layer_config` | `{ "enable_i18n": true, "locale_source": "FIRST_AVAILABLE" }` | 开启 i18n layer |

注意: i18n layer 只决定语言能力是否启用和 locale 来源。用户修改语言是否持久化，还取决于用户设置接口。快速启动兼容层如果不实现 `/wham/settings/user`，语言修改可能只在前端局部生效或不生效。

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
2. `/api/wham/settings/user` 和 `/api/wham/settings/configs/user-preferences`: 如果要让语言设置在快速启动模式下持久化，需要补。
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
4. `2055603567` 不为 true。
5. `3936985709` 不为 true。
6. `72216192` layer 存在且 `enable_i18n=true`。

### 5. 手动验证

快速启动开启后验证:

1. Codex App 能秒开。
2. 底部手机/远控入口存在。
3. computer-use 仍显示为可用。
4. 插件列表不会长时间卡远端刷新。
5. 日志里没有关键 `/wham/statsig/bootstrap` 或 `/wham/accounts/check` 失败。
6. 未实现的官方接口如果出现 404，应确认是否属于明确不兼容范围。

## 维护原则

1. Statsig 只补必要 gate，不做全量 true。
2. 路由只补 CodexHub 实际需要的 contract。
3. 不用 API key auth 规避问题，因为 API key 模式不支持 CodexHub 需要的 remote-control。
4. 历史版本曾允许关闭快速启动并回到官方 backend；当前版本已改为默认直连 CodexHub 3847 backend API，不再提供开关。
5. 每次 Codex App 更新后先扫描再改，不根据旧 ID 盲目迁移。

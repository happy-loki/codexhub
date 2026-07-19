# Codex App 模型选择器 CDP 诊断

日期：2026-07-16

状态：调研工具，不进入 CodexHub 正式运行链路。

## 目标

只读观察 Codex App renderer，确认自定义模型到底在哪一层被过滤：

1. app-server `model/list` 之后的 React 数据；
2. Statsig `available_models` 等前端配置；
3. 模型选择器相关 DOM 和 React fiber/props；
4. 打开模型选择器期间发生的网络请求。

脚本不会点击界面、刷新页面、修改 DOM、写入 storage、读取响应正文或记录请求头。URL 查询参数和疑似认证字段会脱敏。

报告仍会包含当前界面的可见文本、本地项目路径和模型名称。它默认写入已被 Git 忽略的 `outputs/`；对外发送前必须人工检查，不要直接上传完整文件。

## 前置条件

CDP 端口必须在 Codex App 启动时开启。已经正常启动且未带调试参数的 Codex App，无法在运行中补开 CDP。

Windows 启动参数：

```text
--remote-debugging-address=127.0.0.1 --remote-debugging-port=9335
```

Windows Store 包不能从 `WindowsApps` 路径直接执行。仓库提供的辅助脚本通过 Windows 官方 MSIX `ApplicationActivationManager` 传入参数：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/start-codex-app-cdp-diagnostics.ps1 -Port 9335
```

如果 Codex App 已经运行，辅助脚本会直接报错，不会关闭或重启现有进程。

诊断脚本本身不会关闭或重启 Codex App，也不会接管 CodexHub 的启动流程。

CDP 是本机无认证的高权限调试接口。仅绑定 `127.0.0.1`，诊断结束后应正常退出这次 Codex App，让端口随进程关闭。

## 使用方法

确认 Codex App 已带上述参数启动后，在仓库根目录执行：

```powershell
node scripts/codex-app-cdp-diagnostics.mjs --port 9335 --watch-ms 20000
```

命令显示已连接后，在 20 秒内手动打开 Codex App 的模型选择器。结果默认写入：

```text
outputs/codex-app-cdp-model-diagnostics.json
```

指定输出位置：

```powershell
node scripts/codex-app-cdp-diagnostics.mjs --port 9335 --watch-ms 30000 --output outputs/model-picker.json
```

## 输出说明

- `before`：开始监听时的 renderer 快照；
- `after`：监听结束时的 renderer 快照，模型下拉框保持打开时价值最高；
- `candidates`：撰写器内全部控件，以及文本、ARIA 或 test id 命中模型关键词的元素；
- `reactEntries`：候选元素对应 React fiber 向上的 props/state 摘要；
- `matchingGlobals`：名称命中 model、Statsig、provider 等关键词的 window 属性类型，不展开整个对象；
- `statsigClientInfo`：Statsig client 的属性、原型方法和 override adapter 能力摘要；
- `statsigValues`：从 Statsig 状态中定点提取的 `available_models`、`use_hidden_models` 和 `default_model`；
- `performanceResources`：页面已有的资源加载记录；
- `observedNetwork`：监听窗口内新发生的请求与响应元数据，不包含 header 和 body。

## 判断方法

1. `reactEntries` 已包含 DeepSeek、Opus 等模型，但 DOM 不包含：过滤发生在模型选择器组件内部或渲染条件。
2. React 数据只含官方模型，同时 `model/list` 返回完整：过滤发生在 RPC 数据进入选择器之前，重点检查 Statsig 白名单合并逻辑。
3. renderer 数据中存在 `available_models` 且只有官方模型：可以确认 Statsig dynamic config 是直接过滤依据。
4. `model/list` 本身只含官方模型：回到 app-server、models cache 或 Provider `/models` 链路排查，不属于 renderer 白名单问题。

## 限制

React 生产构建会压缩组件名，内部结构也会随 Codex App 更新变化。该工具采用启发式扫描，目的是定位数据流，不承诺每个版本都能直接导出完整 store。若结果里只有 DOM 而没有有效 fiber，下一步应根据当期 bundle 的 React 根节点和数据缓存实现补充专用探针。

## 2026-07-16 实机结论

测试版本：Codex App `26.707.9981`。

诊断器定位到撰写器模型按钮对应的压缩 React 组件 `RO`。组件 props 包含：

```text
model = grok-4.5
reasoningEffort = xhigh
labelCandidates = 29 个模型/推理档位组合
```

`labelCandidates` 去重后只有以下 8 个模型：

```text
gpt-5.6-sol
gpt-5.6-terra
gpt-5.6-luna
gpt-5.5
gpt-5.4
gpt-5.4-mini
gpt-5.3-codex
gpt-5.2
```

同一 renderer 的 `window.__STATSIG__` 中，dynamic config `107580212` 为：

```json
{
  "available_models": [
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.3-codex",
    "gpt-5.2"
  ],
  "use_hidden_models": true,
  "default_model": "gpt-5.4"
}
```

两个列表完全一致。当前模型 `grok-4.5` 仍保留在按钮状态中，但没有进入 `labelCandidates`。因此可以确认：

1. CodexHub/Core 的模型数据已经进入 renderer，当前会话也能识别 `grok-4.5`；
2. 真正丢失发生在 renderer 生成模型候选列表时；
3. 直接过滤输入就是官方 Statsig `107580212.available_models`，并由 `use_hidden_models=true` 启用白名单行为；
4. 这不是 CodexHub `/models` 合成或 app-server `model/list` 丢弃造成的。

### 是否能转成不依赖 CDP 的稳定修复

当前不能从公开配置得到稳定修复。`requires_openai_auth=false` 的 pre-login renderer 会读取硬编码的 `https://ab.chatgpt.com/v1`，公开的 Provider 配置和 `chatgpt_base_url` 都不能替换这条前端 Statsig 链路。

现阶段只剩三类技术路径：

1. CDP 运行时修改 Statsig/React 状态：无需改安装包，但要求以调试参数启动，适合作为用户主动选择的增强模式；
2. ASAR renderer 补丁：不依赖 CDP，但需维护 Windows/macOS、多架构、签名和每次 App 更新后的 bundle 差异；
3. 等待 Codex App 提供正式的自定义模型或 Statsig 覆盖入口：最稳定，但当前版本没有。

因此 CDP 不作为 CodexHub 默认启动行为，但可以由用户从 CodexHub 主动选择“增强模式启动 Codex App”。普通官方入口保持不变。

## 临时因果实验

`codex-app-cdp-model-visibility-experiment.mjs` 可以只在当前 renderer 内存里把 `use_hidden_models` 临时设为 `false`，并通过 Statsig 自己的 `values_updated` 事件触发 React 重新计算：

```powershell
node scripts/codex-app-cdp-model-visibility-experiment.mjs --port 9335 --status
node scripts/codex-app-cdp-model-visibility-experiment.mjs --port 9335 --apply
node scripts/codex-app-cdp-model-visibility-experiment.mjs --port 9335 --offline-statsig-reload
node scripts/codex-app-cdp-model-visibility-experiment.mjs --port 9335 --local-statsig-reload
node scripts/codex-app-cdp-model-visibility-experiment.mjs --port 9335 --trace-statsig-reload
node scripts/codex-app-cdp-model-visibility-experiment.mjs --port 9335 --status --open-picker
node scripts/codex-app-cdp-model-visibility-experiment.mjs --port 9335 --restore
```

`--open-picker` 只点击已经定位到的模型按钮并读取菜单 DOM，不选择模型。

实验会克隆 Statsig 当前的 evaluation 响应，只在内存 `_store` 中把 dynamic config 的 `use_hidden_models` 改成 `false`，再调用 SDK 自己的 `setValues()` 和 `_finalizeUpdate()`。它不写 localStorage、LevelDB 或 ASAR。`apply` 会保留原始 evaluation，`restore` 会通过同一内存更新流程恢复；退出 Codex App 同样会清除实验状态。

### 实验结果

Codex App `26.707.9981` 的 renderer bundle 中，模型查询路径为：

```text
list-models-for-host(includeHidden=true)
  -> TanStack Query raw cache
  -> model-list-filter-C2SM1X_9.js
  -> React 模型菜单
```

过滤函数的等价逻辑为：

```javascript
const enforceAllowlist = useHiddenModels && authMethod !== "amazonBedrock";

for (const model of models) {
  if (enforceAllowlist ? availableModels.has(model.model) : !model.hidden) {
    visibleModels.push(model);
  }
}
```

TanStack Query 原始缓存中实际有 12 个模型，且以下自定义模型全部为 `hidden=false`：

```text
Grok-4.5
deepseek-v4-pro
deepseek-v4-flash
GLM-5.2
Opus-4.8
Sonnet-4.6
```

只更新 Statsig `_store` 并触发 `values_updated` 仍不足以立即改变菜单，因为已经挂载的 TanStack Query observer 持有旧 `availableModels` Set 的闭包。实验脚本在一次 `refetchQueries()` 期间，临时让“恰好匹配官方 8 模型白名单”的 Set 通过所有模型；Promise 完成后立即恢复原生 `Set.prototype.has`。

重新派生后，真实二级模型菜单显示完整 12 个模型。按钮也从“自定义”恢复为当前模型名 `Grok-4.5`。这证明：

1. CodexHub `/models`、app-server `model/list` 和 React Query 原始缓存都正确；
2. 自定义模型的 `hidden` 标志正确；
3. 唯一导致模型消失的是 renderer 的 Statsig allowlist 过滤；
4. renderer 内同时存在 Statsig store、Statsig memo cache、React hook 和 TanStack Query observer 四层状态，运行中修改必须让四层一起重新派生。

临时 `Set.prototype.has` 代理只用于确认旧闭包，不能作为正式产品实现。若以后采用 CDP 运行时方案，应在 React 查询首次创建前完成 Statsig override，避免已挂载 observer 持有旧闭包；同时仍需处理页面 reload、renderer 重建和 Codex App 更新后的结构变化。

## `CODEX_API_BASE_URL` 与 Statsig 重定向实验

Codex App `26.707.9981` 主进程读取 API 地址的优先级是：

```text
CODEX_API_BASE_URL
  > 遗留 CODEX_API_ENDPOINT=localhost 对应的 http://localhost:8000/api
  > https://chatgpt.com/backend-api
```

旧 8000 快速模式实际使用的是轻量 `/api` 协议路径，而不是端口本身带来加速。当前 CodexHub 默认把 `CODEX_API_BASE_URL` 设为 `http://127.0.0.1:3847/api`：继续由 3847 主服务承载，但保留旧 dev API 的启动语义，并清理遗留 `CODEX_API_ENDPOINT`。远程控制 server 与插件接口同时提供 `/api` 别名，因此不需要 8000 listener，也不牺牲现有功能。

但该环境变量不会修改 renderer Statsig 地址。Statsig client 的实际运行时配置仍为：

```text
api = https://ab.chatgpt.com/v1
initialize = https://ab.chatgpt.com/v1/initialize
```

`--trace-statsig-reload` 强制 Statsig evaluation 缓存 miss，并同时记录页面 `fetch`、XHR 和 CDP Network。三处都没有出现 `ab.chatgpt.com`，但 Statsig 仍能完成更新。原因是 Codex App 为 Statsig 注入了自定义 `networkOverrideFunc`，请求不经过 renderer 的普通 `window.fetch`，因此 `Fetch.enable` 无法直接拦截它。

CDP 运行时实验进一步确认，可以临时包装：

```text
window.__STATSIG__.firstInstance._network._netConfig.networkOverrideFunc
```

包装器成功捕获到带 SDK 参数的真实 `/v1/initialize` URL，也能把该调用改投 `127.0.0.1:3847`。所以 CDP 模式下可以修改这条链路，阻点不在网络拦截能力。

现有 `/backend-api/wham/statsig/bootstrap` 不能直接作为 `/v1/initialize` 响应：前者返回 ChatGPT bootstrap envelope，内部 `statsigPayload` 使用 `v/i/r/s` 等压缩字段；后者要求 Statsig JavaScript SDK 的 initialize 线协议，并且要与请求中的当前用户、ID type、hash mode 和增量字段一致。直接复用会让 dynamic config 和 feature gate 变成未识别状态。实验后已用官方传输冷刷新，恢复正常配置。

最终产品实现没有伪造完整 initialize 响应。官方 store 中包含体积很大的 primary runtime、插件和其他动态配置；替换完整响应会扩大影响面。CodexHub 改为在 renderer 第一帧修补 Statsig evaluation 缓存，并通过 SDK 自己的 `_store.setValues()` / `_finalizeUpdate()` 增量合并模型配置和已确认的关键 gate，官方其他配置原样保留。

## 启动期 33 至 36 秒阻塞的根因

空任务仍然启动缓慢，说明任务历史不是核心原因。Codex App `26.707.91948` 的 Electron 日志给出了完整时间线：主窗口和 app-server 约 2 秒内就绪，但 React 从 `root render requested` 到 `app routes mounted` 等待约 33 秒。

renderer 在 ChatGPT 登录态下会先调用：

```text
POST /wham/statsig/bootstrap
```

这条调用被放在 `CodexStatsigProvider.sync` 的 Suspense 边界前。单次请求超时为 5 秒；401 会最多重试 5 次，重试间隔 500ms。因此本地请求被拒绝时，最坏路径约为 `6 * 5s + 5 * 0.5s = 32.5s`，与实测的 33.2 秒一致。只有所有重试结束并进入 pre-login Statsig fallback 后，应用路由才会挂载。

CodexHub 的本地 `/api/wham/statsig/bootstrap` 实测约几十毫秒即可返回。延迟发生在请求到达 CodexHub 之前：Codex App 主进程的安全请求层拒绝给非 OpenAI URL 附加 ChatGPT 认证。旧的 CDP 实现只在 Statsig client 创建后修补 store，已经晚于这段 Suspense 等待。

Codex App `26.715.4045` 又确认了另一条同类路径。`requires_openai_auth=false` 时 renderer 进入
pre-login 分支，直接调用 Statsig React SDK 的 `useClientAsyncInit()`。SDK 的实际顺序是：

1. 从缓存读取 evaluation 并写入临时 store；
2. 仍然调用 `initializeAsync()` 等待 `https://ab.chatgpt.com/v1/initialize`；
3. 网络请求结束后才把 React 的 `isLoading` 改为 false。

因此“缓存已有正确模型”和“首屏不等待网络”是两回事。SDK `NetworkCore` 的默认单次超时为
10 秒，失败重试后会形成约 30 多秒的等待。本机实测中，窗口在 314ms ready-to-show，增强配置
在 4.126 秒完成，但 `app routes mounted` 直到 35.984 秒才出现；普通启动历史日志也曾等待
33.2 秒。阻塞位于 renderer 的 Statsig Provider，不在 CodexHub 启动器、app-server、插件同步或
会话历史读取。

增强模式现在从 renderer 第一行代码前监听 `codex-message-from-view`。当 Codex App 进入 post-login Statsig 路径时：

1. 只匹配 `POST /wham/statsig/bootstrap`，其他请求继续走 Codex App 原路径；
2. 优先读取最新的 pre-login 官方 Statsig evaluation 缓存，其次读取 CodexHub 本地用户缓存；
3. 保留完整官方 payload，只覆盖 CodexHub 模型目录和已确认的关键 gate；
4. 没有任何缓存时，使用包含模型、关键 gate、runtime 与 i18n layer 的最小本地 payload；
5. 同步发送对应的 `fetch-response`，原主进程请求随后返回的错误因 request ID 已完成而被忽略。

运行时探针通过 Codex App 自己的 `safePost` 验证了这条消息链路，bootstrap 响应在 1ms 内完成。该方案没有伪造 Statsig `/v1/initialize`，也没有恢复 8000 端口。

Codex App 也可能直接进入 pre-login Statsig 路径，此时不会发出 `/wham/statsig/bootstrap`。因此 `bootstrapIntercepted` 只用于诊断，不能作为增强启动的完成条件；完成条件是 renderer 已发出 `ready`，且模型目录和关键 gate 已生效。

增强脚本 v8 在 React root render 之前监听 `window.__STATSIG__.firstInstance`。Statsig client 构造器
同步注册实例时，脚本会在 React hook 调用 `initializeAsync()` 之前完成以下操作：

1. 只从完整的官方 evaluation 缓存生成启动底座；
2. 把 evaluation 的 user 替换为该 client 当前的真实 Statsig user；
3. 有缓存时调用 SDK 自己的 `dataAdapter.setData()`；
4. 有缓存时调用 `initializeSync({ disableBackgroundCacheRefresh: true })`，直接结束首屏 loading；
5. 没有完整缓存或本地初始化异常时，原样调用原生 `initializeAsync()`，完成后再做局部 overlay，
   不用 CodexHub 自己的极简 payload 冒充官方配置。

该做法没有伪造 `/v1/initialize` 响应，也没有修改 ASAR、LevelDB 或系统代理。启动日志中的
`fast_initialize_applied=true` 和 `fast_initialize_source` 用于确认是否命中；命中缓存时可把首屏
从约 36 秒降到数秒。无缓存时应重点确认官方 evaluation 最终进入 store，不能把“快”作为牺牲官方
配置完整性的条件。

## 增强模式启动

Codex 接入页提供“增强模式启动 Codex”按钮。按钮只在 Codex 配置已经初始化时可用；未初始化时
保持置灰，避免在缺少本地 Provider 和认证配置时进入不完整的启动链路：

1. 用户主动选择增强模式启动后，CodexHub 设置 `CODEX_API_BASE_URL=<本地 backend-api>`；
2. Windows 通过隐藏进程激活 Store/MSIX App，macOS 通过 `open --args` 启动；
3. CDP 只监听 `127.0.0.1:9335`；
4. 页面目标出现后，CodexHub 先在当前 renderer 立即执行脚本，再注册 `Page.addScriptToEvaluateOnNewDocument`；目标探测间隔为 50ms，且当前文档注入不等待 `Page.enable`，尽量保证 hook 早于 React root render；不主动刷新页面，后续若发生应用自身的自然导航，新文档仍会自动注入；
5. 注入脚本在 post-login 路径只对完整官方 Statsig 缓存响应启动期 `/wham/statsig/bootstrap`，并修补
   Statsig 缓存；pre-login 路径同样只使用完整官方缓存；没有完整缓存时保留官方初始化路径；
6. SDK 就绪后事件驱动地合并一次 store，官方后续触发 `values_updated` 时只重新应用模型配置和关键 gate，不覆盖 runtime layer；
7. daemon 事件驱动地持有 CDP WebSocket，使新文档注入注册在本次 renderer 生命周期内保持有效；再次增强启动会替换旧 session，不增加轮询 timer；
8. 启动接口以 Statsig 模型配置和关键 gate 已生效作为完成条件；`app routes mounted` 和 bootstrap 是否被拦截仅作为诊断信息，兼容 pre-login 路径。

按钮位于原“快速启动”复选框位置。Codex App 未运行时，点击按钮会直接使用增强模式启动，不要求用户额外确认。只有检测到 Codex App 正在运行时才显示关闭提示并禁用启动按钮；用户完全退出后，短生命周期检测 timer 会自动启用按钮。弹窗关闭后 timer 立即销毁，不形成后台轮询。用户界面不展示 CDP、端口或网络请求等实现细节。

普通启动、VS Code 插件和 CLI 不经过这条 CDP 链路。增强状态只在本次 Codex App 进程期间存在，完全退出后 9335 端口和内存注入同时消失，不修改 ASAR、LevelDB 或快捷方式。

实机回归结果：12 个模型全部进入 Statsig 和真实菜单，`use_hidden_models=false`，6 个已确认 gate 全部启用；`codex-primary-runtime` layer 的版本、下载地址和其他官方字段保持不变。

### Codex App 26.715.3651 启动刷新竞态

2026-07-18 的连续失败日志确认，旧实现取得 `app://-/index.html` target 后立即调用
`Page.reload`，会取消 Codex App 主进程尚未完成的首次 `loadURL`。Codex App 随后记录：

```text
ERR_ABORTED (-3) loading 'app://-/index.html'
Desktop bootstrap failed to start the main app
```

主进程把这次导航取消视为启动失败，最终显示 `ChatGPT failed to start` 和
`ERR_FAILED (-2) loading data:text/html...`。这不是模型接口、账号或上游网络错误，而是增强模式
主动刷新与 Electron 首次导航发生竞态。

修复后的增强模式不再调用 `Page.reload`：脚本先注册给未来文档，再通过 `Runtime.evaluate` 在当前
页面立即执行。若注入最终未生效，接口会报告增强配置失败，但不会为了重试而刷新或破坏已经启动的
Codex App，用户仍可按普通模式继续使用。

### Codex App 26.715.4045 超长历史恢复崩溃

2026-07-18 的 `ChatGPT stopped unexpectedly` 与上述首次导航竞态不同。四份
`CrashDumps\\codex.exe.*.dmp` 表明退出的是 Codex app-server；两次最新崩溃前都在恢复同一条约
146 MB、超过 4.2 万行的会话，并由 `tail_history` 连续发起 `thread/turns/list`，直到出现
`0xC0000409 / FAST_FAIL_UNEXPECTED_HEAP_EXCEPTION`。

当前 renderer bundle 将 Statsig gate `3446105535` 映射为
`suppressResumeHistoryDrain`。CodexHub 现在只维护 6 个已确认 gate，其中包含该项；开启后首次恢复
只加载最近 5 个 turn，旧历史随用户滚动分页获取。原始 JSONL 不会被删除、截断、归档或重写。

同时，增强脚本会撤销旧版 CodexHub 对另外 9 个历史 gate 的本地强制值，只保留官方值；本地
`/wham/statsig/bootstrap` 也从 14 项收敛为相同的 6 项，避免未知 gate 引入新的前端分支。

### 2026-07-18 Windows 蓝屏与配置全零事件

本次事件的证据时间线如下：

1. `12:12:04.026` 开始增强模式启动；
2. `12:12:08.639` 注入完成，12 个模型和旧实现当时维护的 14 个 gate 已生效；
3. Codex App 随后继续正常连接 remote control，并响应插件和模型接口；
4. `12:12:51.615` 是 CodexHub 最后一条正常业务日志；
5. `12:12:54`，`~/.codex/config.toml` 的 5696 字节全部变成 `0x00`；
6. Windows 在 `12:13:21` 记录从 bugcheck 重启，错误为 `0x0000001E
   (KMODE_EXCEPTION_NOT_HANDLED)`，转储文件为
   `C:\WINDOWS\Minidump\071826-12062-01.dmp`。

增强启动到蓝屏之间存在时间相关性，但当前不能证明 CDP 是内核崩溃根因。该时间段没有任何
CodexHub `write_config_start` 事件；增强启动路径本身只设置用户环境变量、通过 Windows App 激活
接口附加 CDP 参数，并执行 renderer 内存脚本。用户态 CDP 脚本不能直接产生内核 bugcheck，但启动
Electron renderer 可能间接触发已有的显卡、存储或其他内核驱动缺陷。责任模块必须使用管理员权限
分析上述 minidump 后才能确定。

配置全零与突然重启时间完全一致，属于未持久化文件内容在系统异常中断后的典型损坏形态。为降低
同类事件的损失，CodexHub 做以下防护：

- 管理 `config.toml` 和 `auth.json` 时，先写同目录临时文件并 `sync_all`，再原子替换目标文件；
- `.bak` 也采用相同的持久化原子替换，且全零源文件不能覆盖已有有效备份；
- 增强模式启动前刷新一份有效 `config.toml.bak`；
- 仅在明确执行配置更新或增强启动准备时，识别“非空且每个字节都是 `0x00`”的主文件并从有效
  `.bak` 恢复；普通 TOML 语法错误只报错，不自动覆盖用户内容；
- 普通状态检查保持只读，避免在另一个进程短暂写文件时参与竞争。

#### 第二次蓝屏的驱动结论

同日第二次重启生成 `C:\WINDOWS\Minidump\071826-11921-01.dmp`。离线 KD 分析结果为：

```text
Bugcheck: 0xD1 DRIVER_IRQL_NOT_LESS_OR_EQUAL
Memory referenced: 0x18
IRQL: 2
Operation: read
Faulting instruction: Netwtw14.sys+0x49687
```

`Netwtw14.sys` 是 Intel Wi-Fi 6E AX211 160MHz 驱动。当前安装包为 `24.50.0.4`
（`oem205.inf`），驱动文件版本为 `23.50.21.2`。系统在两次蓝屏前后还持续记录
`Netwtw14` 事件 `6062`，因此第二次蓝屏的直接责任模块已经可以确定为 Intel 无线网卡驱动，
而不是 CodexHub、CDP、Electron 或 WinDbg。

第一份 `0x1E` minidump 只保留有限寄存器和栈，表现为执行空地址，无法独立锁定责任模块；结合
随后可复现的 `0xD1`、同一版本的 `Netwtw14` 和连续的 `6062` 警告，Intel Wi-Fi 驱动问题是
两次异常最一致的解释，但第一份转储本身不足以做百分之百归因。

本机 DriverStore 已保留旧版 `oem207.inf`，包版本 `23.130.1.1`、驱动文件版本
`23.50.8.4`。在 Intel 或 ASUS 发布修复版之前，优先回滚到该版本；回滚会中断 Wi-Fi，应先切换
到有线网络或 USB 网络。系统同时加载了 Wintun `0.14.0`，但故障指令不在 Wintun 中，可在主驱动
回滚后再把 VPN/Wintun 更新作为第二层排查。

### 最终判断

本次实验没有发现不依赖 CDP 或 ASAR 的前端注入点。当前路径是：

1. CDP：已作为用户主动选择的增强启动实现，默认启动不接管；
2. ASAR：可以直接修改 `model-list-filter-*.js` 的过滤条件，但需要按 Codex App 版本维护 Windows/macOS 包和签名；
3. 官方能力：等待 Codex App 提供自定义 Provider 模型目录或关闭 Statsig allowlist 的公开配置。

因此，默认路径仍不接管 Codex App；需要前端自定义模型的用户可以明确选择增强模式。该边界同时保留了普通官方启动的可卸载性和增强模式的实际可用性。

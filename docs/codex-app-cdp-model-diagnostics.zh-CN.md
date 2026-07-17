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

## 启动期 33 秒阻塞的根因

空任务仍然启动缓慢，说明任务历史不是核心原因。Codex App `26.707.91948` 的 Electron 日志给出了完整时间线：主窗口和 app-server 约 2 秒内就绪，但 React 从 `root render requested` 到 `app routes mounted` 等待约 33 秒。

renderer 在 ChatGPT 登录态下会先调用：

```text
POST /wham/statsig/bootstrap
```

这条调用被放在 `CodexStatsigProvider.sync` 的 Suspense 边界前。单次请求超时为 5 秒；401 会最多重试 5 次，重试间隔 500ms。因此本地请求被拒绝时，最坏路径约为 `6 * 5s + 5 * 0.5s = 32.5s`，与实测的 33.2 秒一致。只有所有重试结束并进入 pre-login Statsig fallback 后，应用路由才会挂载。

CodexHub 的本地 `/api/wham/statsig/bootstrap` 实测约几十毫秒即可返回。延迟发生在请求到达 CodexHub 之前：Codex App 主进程的安全请求层拒绝给非 OpenAI URL 附加 ChatGPT 认证。旧的 CDP 实现只在 Statsig client 创建后修补 store，已经晚于这段 Suspense 等待。

增强模式现在从 renderer 第一行代码前监听 `codex-message-from-view`。当 Codex App 进入 post-login Statsig 路径时：

1. 只匹配 `POST /wham/statsig/bootstrap`，其他请求继续走 Codex App 原路径；
2. 优先读取最新的 pre-login 官方 Statsig evaluation 缓存，其次读取 CodexHub 本地用户缓存；
3. 保留完整官方 payload，只覆盖 CodexHub 模型目录和已确认的关键 gate；
4. 没有任何缓存时，使用包含模型、关键 gate、runtime 与 i18n layer 的最小本地 payload；
5. 同步发送对应的 `fetch-response`，原主进程请求随后返回的错误因 request ID 已完成而被忽略。

运行时探针通过 Codex App 自己的 `safePost` 验证了这条消息链路，bootstrap 响应在 1ms 内完成。该方案没有伪造 Statsig `/v1/initialize`，也没有恢复 8000 端口。

Codex App 也可能直接进入 pre-login Statsig 路径，此时不会发出 `/wham/statsig/bootstrap`。因此 `bootstrapIntercepted` 只用于诊断，不能作为增强启动的完成条件；完成条件是 renderer 已发出 `ready`，且模型目录和关键 gate 已生效。

## 增强模式产品实现

Codex 接入页提供“增强模式启动 Codex App”按钮：

1. 用户主动选择增强模式后，CodexHub 设置 `CODEX_API_BASE_URL=<本地 backend-api>`；
2. Windows 通过隐藏进程激活 Store/MSIX App，macOS 通过 `open --args` 启动；
3. CDP 只监听 `127.0.0.1:9335`；
4. 页面目标出现后，CodexHub 注册 `Page.addScriptToEvaluateOnNewDocument`，再刷新一次 renderer，确保脚本从新文档第一行代码前生效；
5. 首帧脚本在 post-login 路径响应启动期 `/wham/statsig/bootstrap`，并修补 Statsig 缓存；pre-login 路径直接使用缓存；
6. SDK 就绪后事件驱动地合并一次 store，官方后续触发 `values_updated` 时只重新应用模型配置和关键 gate，不覆盖 runtime layer；
7. daemon 事件驱动地持有 CDP WebSocket，使 preload 在 renderer reload 后仍然有效；再次增强启动会替换旧 session，不增加轮询 timer；
8. 启动接口等待 Codex App 自己发出 `ready`，即 `app routes mounted`，不再把“CDP 注入完成”误报为“界面就绪”。

按钮位于原“快速启动”复选框位置。Codex App 未运行时，点击按钮会直接以增强模式启动，不要求用户额外确认。只有检测到 Codex App 正在运行时才显示关闭提示并禁用启动按钮；用户完全退出后，短生命周期检测 timer 会自动启用按钮。弹窗关闭后 timer 立即销毁，不形成后台轮询。用户界面不展示 CDP、端口或网络请求等实现细节。

普通启动、VS Code 插件和 CLI 不经过这条 CDP 链路。增强状态只在本次 Codex App 进程期间存在，完全退出后 9335 端口和内存注入同时消失，不修改 ASAR、LevelDB 或快捷方式。

实机回归结果：12 个模型全部进入 Statsig 和真实菜单，`use_hidden_models=false`，14 个关键 gate 全部启用；`codex-primary-runtime` layer 的版本、下载地址和其他官方字段保持不变。

### 最终判断

本次实验没有发现不依赖 CDP 或 ASAR 的前端注入点。当前路径是：

1. CDP：已作为用户主动选择的增强启动实现，默认启动不接管；
2. ASAR：可以直接修改 `model-list-filter-*.js` 的过滤条件，但需要按 Codex App 版本维护 Windows/macOS 包和签名；
3. 官方能力：等待 Codex App 提供自定义 Provider 模型目录或关闭 Statsig allowlist 的公开配置。

因此，默认路径仍不接管 Codex App；需要前端自定义模型的用户可以明确选择增强模式。该边界同时保留了普通官方启动的可卸载性和增强模式的实际可用性。

CodexHub v0.4.10

本次版本重点提高 Codex App 增强模式在无 VPN、官方 Statsig 请求缓慢或本地没有完整官方缓存时的启动成功率。

## 本地 Statsig 初始化

- 新增 CodexHub 本地 Statsig `initialize` 端点，Codex App 可直接通过 `127.0.0.1:3847` 完成初始化，不需要远程中继。
- 响应使用 Statsig `init-v2` 原生结构，修正 dynamic config 的 `v -> values[key]` 引用方式。
- 本地响应提供 CodexHub 模型列表、中文界面配置和已确认的关键功能门控。
- 增加 `no-store`、CORS 和 Private Network 响应头，兼容 Electron renderer 的本地请求。

## 增强模式兼容

- 增强脚本升级到 v15，在 Statsig SDK 初始化前临时设置本地 initialize URL。
- 有完整官方缓存时继续优先使用官方配置，并恢复官方刷新地址，避免本地精简配置覆盖官方运行时数据。
- 没有官方配置时允许本地响应作为有效初始化基底，避免旧版本长期停留在 `NoValues` 并最终超时。
- 原始 SDK URL 使用 `WeakMap` 保存，不向 Statsig 私有对象写入额外标记。
- 不修改 Codex App 的 `app.asar`、LevelDB、账号状态或系统代理。

## 诊断能力

- 启动日志新增本地 initialize 安装状态、当前地址、错误信息和本地基底状态。
- 增强模式成功条件同时支持完整官方基底和合法的 CodexHub 本地基底。
- 本地端点提供三条兼容路由，降低 CodexHub API 前缀变化带来的影响。

## 验证

- 使用 Codex App 内置 Statsig JavaScript SDK 3.32.6 验证初始化成功。
- 验证 12 个 CodexHub 模型、中文界面和关键功能门控成功载入。
- 默认测试：536 passed，0 failed，2 ignored。
- `cargo fmt --check`、GUI 编译检查和 `git diff --check` 通过。

## 当前边界

- 本地端点响应约 1 ms，但 Codex App renderer 和 Statsig 客户端的创建仍可能需要十几秒；本版解决的是无官方数据时的启动失败，不承诺所有环境都能秒开。
- 仅使用本地精简 Statsig 配置时，依赖其他官方配置的插件市场内容可能不完整，后续版本会继续收敛。

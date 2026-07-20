CodexHub v0.4.7

本次更新重点提高 Codex App 增强模式对客户端升级的兼容能力。模型列表和中文能力不再只依赖固定 Statsig 数字 ID；遇到无法识别的新结构时，增强模式会停止覆盖并保留 Codex App 官方状态，避免升级后出现界面异常、中文消失或旧配置被强行写入。

## Codex App 升级兼容

- 模型配置改为根据 `available_models`、`use_hidden_models` 和 `default_model` 字段自动识别。
- 中文配置改为根据 `enable_i18n` 和 `locale_source` 字段自动识别。
- 旧版模型与中文数字 ID 只在官方数据中仍然存在时作为兜底，不再凭空创建过期配置。
- 同时支持 Statsig Store、公开 API 和快速初始化三条适配路径，减少对单个 renderer 私有字段的依赖。
- 保留完整的官方 Statsig evaluation，只增量覆盖 CodexHub 明确支持的模型、中文能力和关键 gate。
- 如果模型或中文结构无法识别，记录兼容失败原因并停止增强，不刷新页面、不修改 ASAR、LevelDB 或官方其他功能状态。

## 启动与诊断

- 增强启动报告新增实际配置 ID、识别来源、可用适配器、官方底座状态、Store 来源、脚本尝试次数和兼容失败原因。
- 启动超时错误会附带最后一次兼容状态，便于区分网络等待、Statsig 尚未就绪和客户端结构变化。
- GUI 等待时间调整为 60 秒，覆盖 daemon 的 45 秒增强检查窗口，避免前端提前显示超时。
- Codex App 自然导航或 renderer 更新后继续保持本次进程内的增强配置，不主动刷新或二次加载页面。

## 维护边界

- 纯布尔 feature gate 没有可供自动推断的语义，目前 6 个已确认 gate 仍使用显式清单。
- Codex App 更新后先运行能力探测；模型、中文和适配器验证通过时无需同步修改 CodexHub。
- 只有实际结构或 gate 语义变化导致兼容检查失败时，才需要针对新版客户端更新适配。

## 验证

- 全量测试：`528 passed, 0 failed, 2 ignored`。
- `cargo fmt --check`、`cargo check --release --features gui --bin codexhub` 通过。
- 当前 Codex App 实机热注入验证通过：12 个模型和中文能力生效，三条适配路径均被识别，未报告兼容失败。

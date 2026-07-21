CodexHub v0.4.8

本次更新修复部分 Windows 用户点击“增强模式启动 Codex”时，频繁出现“暂时无法检测 Codex App 状态”的问题。

## 增强模式启动

- Windows 改用系统原生接口检测 Codex App，避免每次检测都启动额外的系统命令，速度更快，也更不容易被系统负载或安全软件拖慢。
- 同时识别新版 `Codex.exe` 和旧版 `ChatGPT.exe` 进程名称。
- 检测等待时间由 650 毫秒调整为 3 秒，减少低配机器和繁忙环境下的误报。
- 原生检测不可用时仍保留兼容检测，确保特殊 Windows 环境可以继续使用。

## 错误提示与诊断

- 检测失败时不再只显示笼统提示，而是附带具体原因和可操作的处理建议。
- daemon 日志新增检测成功与失败记录，便于远程定位用户环境问题。
- 本次修改不会关闭、重启或修改正在运行的 Codex App。

## 验证

- 全量测试：`528 passed, 0 failed, 2 ignored`。
- `cargo fmt --check`、`cargo check --release --features gui --bin codexhub` 通过。

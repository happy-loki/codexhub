# codex-remote

[English](README.en.md)

## 产品预览

本地 GUI 负责启动 backend、接入飞书 / Telegram / 微信，并写入 Codex App 和 Codex VS Code 插件所需的本地配置。

<p align="center">
  <img src="docs/assets/product/codex-remote-gui.png" alt="Codex Remote GUI 状态和配置界面" width="900">
</p>

同一个 Codex thread 可以继续在 Codex App 里查看，IM 发起的会话和工具结果会同步回来。

<p align="center">
  <img src="docs/assets/product/codex-app-chat.png" alt="Codex App 会话同步和图片结果" width="900">
</p>

在飞书移动端也可以直接收到 Codex 的图片、命令和审批结果。

<p align="center">
  <img src="docs/assets/product/feishu-mobile-image.jpg" alt="飞书移动端展示 Codex 图片结果" width="520">
</p>

`codex-remote` 是一个本地 Codex remote-control backend，用来把 Codex App、Codex VS Code 插件和 Codex CLI app-server 的远程控制能力接到飞书 / Lark、Telegram Bot 和微信机器人。

它只做一件事：用户明确打开本地 GUI 后，让 Codex 客户端连接本机 backend，再把 remote-control 消息桥接到用户选择的 IM 通道。

当前支持 macOS / Windows 上的 Codex App、Codex VS Code 插件，以及 Codex CLI 的官方 app-server remote-control 启动方式。本项目只写入本地配置和必要的 GUI 环境入口，不替换 Codex App、Codex CLI 或 VS Code 插件的原始可执行文件。

## 快速使用

### 0. 前置条件

- macOS 或 Windows 设备
- Codex App、Codex VS Code 插件或 Codex CLI
- 不需要 ChatGPT 账号，也不需要“加速网络”
- 一个可以访问 GPT-5.x 兼容模型的第三方 key
- 至少一个 IM 通道：飞书、Telegram Bot 或微信机器人

### 1. 安装

从 GitHub Releases 下载 `Codex Remote.dmg`，拖到 Applications 后打开。

第一次打开时，如果 macOS 提示来自互联网，按系统提示确认即可。Windows 直接运行 release 包里的 `codex-remote.exe`。这个 App 不会安装开机启动项，也不会自动常驻后台。

后续可以在菜单 `Help -> Check for Updates` 手动检查 GitHub Releases 是否有新版本。当前 MVP 只引导打开下载页，不会静默替换本机程序。

### 2. 打开应用

打开 `Codex Remote`。GUI 会自动启动本地 backend，并在退出时关闭本次启动的 backend。

状态概览显示本地服务运行后继续下一步。

### 3. 接入 IM 通道

切到“聊天工具接入”页面，选择一个通道：

- 飞书：点击“扫码使用新机器人”，按二维码流程完成接入。
- Telegram：填写 BotFather 提供的 Bot Token，点击“保存并接入”。当前仅支持私聊机器人，群聊不会接入。
- 微信：点击“扫码连接微信”，使用微信扫码确认。

接入成功后，状态概览里的“IM 通道”会显示可用。之后正常使用不需要反复扫码或重新填 token；只有更换机器人时才需要重新接入。

### 4. 填写模型信息

切到 “Codex 接入” 页面，点击“新增”后填写你的模型服务信息：

- Provider 名称
- 第三方 Base URL
- API Key

Provider 名称可以留空。留空时，如果填写了 Base URL 或 API Key，默认会使用 `ai-codex`。

### 5. 启用 Provider

点击“保存”只保存当前 provider；点击“启用”会保存当前 provider，并让 Codex App 使用它。

启用时会先备份旧配置，然后让 Codex App 和 Codex VS Code 插件的 remote-control 连接到本机 `codex-remote`，同时写入本地认证信息和当前模型 provider。

在 macOS 上会通过 `launchctl` 写入本次用户会话的 `CODEX_API_BASE_URL` 和 `CODEX_APP_SERVER_LOGIN_ISSUER`。在 Windows 上会写入当前用户的同名环境变量。

### 6. 打开 Codex

正常启动 Codex App 或 Codex VS Code 插件，并打开 remote-control / 控制这台电脑。

连接成功后，`Codex Remote` 里会看到 Codex 控制通道变为已连接。

不需要在 Codex App 的“连接”设置页里看到远程连接设备列表。这个项目走的是本地 backend + IM bridge，只要 `Codex Remote` 的状态概览都正常，就可以直接在已接入的 IM 里使用。

### 7. 使用 Codex CLI

如果使用 Codex CLI，不需要替换 `codex` 命令，也不需要安装包装脚本。先确认上一步已经让 `~/.codex/config.toml` 指向本机 backend：

```toml
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
```

只需要在 IM 里远程使用时，启动一个无界面的 Codex app-server 即可。

macOS / Linux：

```bash
codex remote-control
```

Windows：

```powershell
codex app-server --listen off --remote-control
```

如果希望本地 TUI 和 IM 远程同时使用同一个 Codex app-server，先启动带 remote-control 的 app-server，再用 TUI 连接它。

终端 1：

```bash
codex app-server --listen ws://127.0.0.1:3849 --remote-control
```

终端 2：

```bash
codex --remote ws://127.0.0.1:3849 -C /path/to/project
```

Windows 也使用同样的 websocket 方式，把目录换成 Windows 路径：

```powershell
codex --remote ws://127.0.0.1:3849 -C D:\path\to\project
```

端口 `3849` 被占用时可以换成其它本机端口，但两个命令里的地址必须一致。连接成功后可以检查：

```text
GET http://127.0.0.1:3847/api/remote-control/status
```

其中 `connected=true` 且 `initialized=true` 表示 Codex CLI app-server 已经连到 `codex-remote`。

### 8. 在 IM 里开始使用

在飞书、Telegram 私聊或微信里给机器人发消息。

如果当前 IM 会话还没有绑定 Codex thread，机器人会先让你选择新建 thread 或恢复已有 thread。选择后，后续对话就会进入对应的 Codex thread。

## 交流与支持

推荐关注公众号，后续会更新技术干货、实践记录和项目进展。

<img src="docs/assets/wechat-public-account.jpg" alt="微信公众号" width="220">

微信群主要用于反馈问题、交流使用体验和提出功能建议。

<img src="docs/assets/wechat-group.jpg" alt="AI-Agent 技术交流群" width="260">

## IM 命令  一个/q 命令就够了. 其它按照提示操作

```text
/q         中断并清除当前绑定
```

审批卡片在选择后会高亮并标记为已处理，避免聊天里堆了很多卡片后分不清哪些已经操作过。

## 清除 Codex 接入

GUI 里点击“清除 Codex 接入”即可移除本项目写入 Codex 根配置里的：

- `chatgpt_base_url`
- `model_provider`

## 项目边界

`codex-remote` 只支持干净的 Codex remote-control 路径。

它不会：

- 安装 `codex` 包装命令
- 替换 Codex CLI
- 通过 shim 启动 Codex App
- 安装登录项或开机启动项
- 自动常驻后台
- 替换 Codex App、Codex CLI 或 VS Code 插件的原始可执行文件

本地 backend 只会在用户明确打开 GUI 或主动从开发工具启动时运行。

## 技术说明

主链路：

```text
Codex App / Codex VS Code 插件 / Codex CLI app-server
  |
  | chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
  | 用户打开 remote-control，或启动 codex app-server --remote-control
  v
官方 Codex app-server
  |
  | outbound remote-control websocket
  v
codex-remote 本地 backend
  |
  | 飞书 websocket 事件 / 消息卡片 API
  | Telegram long polling / Bot API
  | 微信 iLink long polling / sendmessage
  v
IM 通道
```

本项目实现官方 remote-control endpoint：

```text
POST /backend-api/wham/remote/control/server/enroll
GET  /backend-api/wham/remote/control/server
```

Codex remote-control 要求 ChatGPT 兼容的 auth mode。这个项目采用本地 `ChatgptAuthTokens` 形态，用来通过 Codex 客户端的 remote-control 账号检查。API-key-only auth 不能启动 remote-control。

Thread 绑定模型：

- Codex app-server 仍然维护 thread 生命周期和历史
- 一个 IM 会话同一时间只绑定一个 Codex thread
- 如果 IM 会话还没绑定 thread，bridge 会先给出新建或恢复 thread 的入口
- 从 IM 恢复某个 thread 后，会订阅这个 thread 后续的 remote-control 事件
- IM 发起的 turn 会按 turn id 记录来源，避免 userMessage 回显

## 命令

```text
codex-remote [--config PATH] daemon
codex-remote [--config PATH] status
codex-remote [--config PATH] on
codex-remote [--config PATH] off
codex-remote [--config PATH] configure-codex-app [--codex-home PATH] [--provider-name NAME] [--provider-base-url URL] [--provider-key TOKEN] [--model MODEL]
codex-remote [--config PATH] uninstall-codex-app [--codex-home PATH]
```

`on` / `off` 用来启用或暂停 IM bridge。

`configure-codex-app` 是 GUI“写入配置”的 CLI 等价形式。如果写入模型 provider 配置，默认 provider 是 `ai-codex`，默认模型是 `gpt-5.5`。

## 配置

`config.toml` 是 `codex-remote` 自己的配置：

```toml
bind = "127.0.0.1:3847"
statePath = "codex-remote-state.json"

[feishu]
appId = ""
appSecret = ""
mentionOnly = true
allowedOpenIds = []
allowedChatIds = []

[telegram]
botToken = ""
allowedChatIds = []

[wechat]
accountId = "wechat"
botToken = ""
baseUrl = ""
userId = ""
botType = "3"
allowedUserIds = []

[bridge]
enabled = true
accountId = "default"
sendStreaming = true
```

Telegram 面向的是“用 BotFather 创建一个自己的 bot，然后在 Telegram 里私聊这个 bot”。群聊暂不接入，避免群成员通过机器人操控宿主本机。`allowedChatIds = []` 表示等待首次私聊绑定；第一个私聊这个 bot 的聊天会自动写入白名单，后续其它私聊会被拒绝。也可以提前手写 `allowedChatIds = ["123456789"]` 锁定自己的 Telegram 私聊。

微信配置通常由 GUI 扫码写入。`botType = "3"` 对应当前 OpenClaw 微信机器人链路；手写配置时不要提交真实 `botToken`。

Codex 客户端配置是另一份文件，通常在 `~/.codex/config.toml`。

详见 [config.example.toml](config.example.toml) 和 [docs/configuration.md](docs/configuration.md)。

## 开发

```powershell
cargo fmt
cargo test
cargo build --release --features gui --bin codex-remote
```

daemon 运行时常用状态接口：

```text
GET http://127.0.0.1:3847/api/status
GET http://127.0.0.1:3847/api/remote-control/status
GET http://127.0.0.1:3847/api/remote-control/backend-status
GET http://127.0.0.1:3847/api/events
```

## 安全说明

- daemon 默认只绑定 `127.0.0.1`，不要直接暴露到公网
- `config.toml` 里保存飞书 `appId` / `appSecret`、Telegram `botToken` 和微信 `botToken`，不要提交
- Codex 的 `auth.json` 和第三方 provider key 都是本地 secret，不要提交
- 飞书附件会下载到本地状态目录旁边的 `.im/attachments/feishu/`
- 真正使用时建议配置 `allowedOpenIds` 和 / 或 `allowedChatIds`
- bridge 可以替 IM 用户向 Codex 提交审批决定，所以飞书 / Telegram / 微信访问权限应视为等价于本地 Codex 审批权限

## 更多文档

- [架构](docs/architecture.md)
- [配置](docs/configuration.md)
- [微信集成计划](docs/wechat-integration-plan.zh-CN.md)
- [认证说明](docs/auth-notes.zh-CN.md)
- [排障](docs/troubleshooting.md)

## License

Apache-2.0

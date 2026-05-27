# codex-remote

[中文说明](README.zh-CN.md)

`codex-remote` is a local Codex App remote-control backend with a Feishu/Lark bridge.

It has one job: after the user explicitly starts the local service, Codex App connects to the local backend, and remote-control messages are bridged to Feishu.

## Quick Start

### 1. Install

Download `Codex Remote.dmg` from GitHub Releases, drag it to Applications, then open it.

If macOS warns that the app was downloaded from the internet, confirm the system prompt. The app does not install startup items and does not run in the background automatically.

### 2. Start Local Service

Open `Codex Remote`, then click `Start Local Service`.

Continue when the local service status shows running.

### 3. Connect Feishu

On first use, click `Change Bot` and complete the QR onboarding flow.

After Feishu is connected, normal use does not require scanning again. Scan again only when switching bots.

### 4. Fill Model Info

Open the `Codex App` page and fill your model service settings:

- Provider name
- Third-party Base URL
- API Key

Provider name can be empty. If it is empty but Base URL or API Key is filled, the default provider name is `codex`.

### 5. Write Codex App Config

Click `Write Config`.

This button only edits Codex App's local config, with backups for existing files. It points Codex App remote control to local `codex-remote`, and writes local auth plus optional model provider settings.

### 6. Open Codex App

Open Codex App normally, then enable remote control in Codex App.

When connected, `Codex Remote` shows Codex App as connected.

### 7. Use Feishu

Send a message to the bot in Feishu.

If the Feishu chat is not bound to a Codex thread yet, the bot first sends a selection card so you can create a new thread or resume an existing one. After selection, the chat is bridged to that Codex thread.

## Community And Support

The WeChat public account is recommended for technical notes, implementation write-ups, and project updates.

<img src="docs/assets/wechat-public-account.jpg" alt="WeChat public account" width="220">

The WeChat group is for issue feedback, usage discussion, and feature suggestions.

<img src="docs/assets/wechat-group.jpg" alt="AI-Agent technical discussion group" width="260">

## Feishu Commands

```text
/new       bind the Feishu chat to a new Codex thread
/status    show current binding and runtime status
/s /stop   interrupt the active Codex turn
/q         interrupt and clear the current binding
/y /n      approve or reject the current approval request
/1 /2 /3   select an exact approval card option
```

Approval cards are updated after selection, so handled approvals are marked visually.

## Uninstall Injection

Click `Uninstall Injection` in the GUI to remove this project's Codex App injection:

- `chatgpt_base_url`
- `model_provider`
- local `ChatgptAuthTokens` auth file

## Project Boundary

`codex-remote` only supports the clean Codex App remote-control path.

It does not:

- install a `codex` wrapper
- replace Codex CLI
- launch Codex App through a shim
- install login items or startup agents
- run as a background service automatically
- change Codex model, sandbox, approval policy, cwd, or environment

The local backend starts only when the user clicks `Start Local Service` or explicitly starts it from development tooling.

## Technical Notes

Runtime path:

```text
Codex App
  |
  | chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
  | user enables remote control in the app
  v
official Codex app-server
  |
  | outbound remote-control websocket
  v
codex-remote local backend
  |
  | Feishu websocket events
  | Feishu message/card APIs
  v
Feishu IM
```

The project implements the official remote-control endpoints:

```text
POST /backend-api/wham/remote/control/server/enroll
GET  /backend-api/wham/remote/control/server
```

Codex remote-control requires a ChatGPT-compatible auth mode. This project writes local `ChatgptAuthTokens` to satisfy Codex App's remote-control account check. API-key-only auth does not start remote control.

Thread binding model:

- Codex app-server remains the source of truth for thread lifecycle and history.
- A Feishu chat binds to one Codex thread at a time.
- If Feishu has not bound a thread yet, the bridge sends a thread list card.
- Resuming a thread from Feishu subscribes to that thread's future remote-control events.
- Feishu-origin turns are tracked by turn id to avoid `userMessage` echo.

## Commands

```text
codex-remote [--config PATH] daemon
codex-remote [--config PATH] status
codex-remote [--config PATH] on
codex-remote [--config PATH] off
codex-remote [--config PATH] configure-codex-app [--codex-home PATH] [--provider-name NAME] [--provider-base-url URL] [--provider-key TOKEN] [--model MODEL]
codex-remote [--config PATH] uninstall-codex-app [--codex-home PATH]
```

`on` / `off` enable or pause the Feishu bridge.

`configure-codex-app` is the CLI equivalent of the GUI `Write Config` button. If model provider config is written, the default provider is `codex` and the default model is `gpt-5.5`.

## Configuration

`config.toml` is for `codex-remote` itself:

```toml
bind = "127.0.0.1:3847"
statePath = "codex-remote-state.json"

[feishu]
appId = ""
appSecret = ""
mentionOnly = true
allowedOpenIds = []
allowedChatIds = []

[bridge]
enabled = true
accountId = "default"
sendStreaming = true
```

Codex App config is separate and usually lives at `~/.codex/config.toml`.

See [config.example.toml](config.example.toml) and [docs/configuration.md](docs/configuration.md).

## Development

```powershell
cargo fmt
cargo test
cargo build
```

Useful status endpoints while the daemon is running:

```text
GET http://127.0.0.1:3847/api/status
GET http://127.0.0.1:3847/api/remote-control/status
GET http://127.0.0.1:3847/api/remote-control/backend-status
GET http://127.0.0.1:3847/api/events
```

## Security Notes

- The daemon binds to `127.0.0.1` by default. Do not expose it publicly.
- `config.toml` stores Feishu `appId` and `appSecret`; do not commit it.
- Codex App `auth.json` and third-party provider keys are local secrets; do not commit them.
- Attachments from Feishu are downloaded to a local state-adjacent `.im/attachments/feishu/` directory.
- Restrict access with `allowedOpenIds` and/or `allowedChatIds` for real usage.
- The bridge can send approval decisions to Codex. Treat Feishu access as equivalent to local Codex approval access.

## More Docs

- [Architecture](docs/architecture.md)
- [Configuration](docs/configuration.md)
- [Auth notes](docs/auth-notes.md)
- [Troubleshooting](docs/troubleshooting.md)

## License

Apache-2.0

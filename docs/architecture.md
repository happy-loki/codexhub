# Architecture

`codex-remote` bridges three systems:

- Codex App / official Codex app-server remote-control protocol
- A local ChatGPT backend-shaped base URL
- IM channel adapters: Feishu websocket/message APIs, Telegram Bot API, and WeChat iLink APIs

It is not a Codex client replacement. It implements the remote-control backend that official Codex app-server connects to, then adapts those JSON-RPC messages to IM channels.

The design target is strict:

- Codex owns threads, turns, cwd, approvals, tools, and execution semantics.
- `codex-remote` owns only bridge-local transport state.
- IM channels are remote interaction surfaces attached to selected Codex threads, not a second source of truth.

## Process Model

The primary path is Codex App direct connection:

```text
Codex App
  |
  | ~/.codex/config.toml:
  |   chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
  |
  | user enables remote control
  v
official Codex app-server
  |
  | GET /backend-api/wham/remote/control/server
  | outbound websocket
  v
codex-remote daemon
  |
  | Feishu websocket listener
  | Feishu message/card APIs
  | Telegram long polling / Bot API
  | WeChat iLink long polling / sendmessage
  v
IM channel
```

The daemon runs separately:

```text
codex-remote daemon
```

It owns:

- local backend API
- official remote-control backend endpoints
- local ChatGPT backend compatibility endpoints needed by the app
- IM channel listeners
- in-memory route/thread/approval/card state

## Remote-Control Backend

The backend exposes the official Codex remote-control paths under `bind`:

```text
POST /backend-api/wham/remote/control/server/enroll
GET  /backend-api/wham/remote/control/server
```

Official Codex app-server connects outbound to those endpoints when Codex App has:

```toml
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
```

and remote control is enabled.

Protocol notes:

- Codex sends `ServerEnvelope` values: `server_message`, `server_message_chunk`, `ack`, `pong`.
- `codex-remote` sends `ClientEnvelope` values: `client_message`, `client_message_chunk`, `ack`, `ping`.
- The first client message is JSON-RPC `initialize`; after the initialize response, `codex-remote` sends `initialized`.
- Server envelopes are acknowledged by `seq_id`; chunk acknowledgements include `segment_id`.
- Large outbound client JSON-RPC messages are segmented with the same 100 KiB target used by official Codex.

## Local Auth Shape

Remote-control startup is gated by Codex auth, before the websocket reaches `codex-remote`. API-key-only auth is rejected by official Codex app-server.

For this project, the local identity shape is `chatgpt`:

```json
{
  "auth_mode": "chatgpt",
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "<local ChatGPT-shaped JWT>",
    "access_token": "<local ChatGPT-shaped JWT>",
    "refresh_token": "",
    "account_id": "acct_codex_remote_local"
  },
  "last_refresh": "2026-05-26T00:00:00Z"
}
```

The JWT only needs the ChatGPT-shaped claims Codex reads locally, especially:

```json
{
  "email": "codex-remote-local@example.local",
  "https://api.openai.com/auth": {
    "chatgpt_account_id": "acct_codex_remote_local",
    "chatgpt_user_id": "user_codex_remote_local",
    "user_id": "user_codex_remote_local",
    "chatgpt_plan_type": "pro",
    "chatgpt_account_is_fedramp": false
  }
}
```

The third-party model key is separate. It belongs in the Codex model provider configuration and is used for model calls, not remote-control enrollment.

## IM Bridge

The bridge has platform-specific adapters under `src/im`. Feishu receives websocket events, Telegram and WeChat use long polling. Platform adapters convert inbound messages into a shared `InboundMessage` shape before the bridge touches Codex remote-control.

Feishu handles:

- `im.message.receive_v1`
- `card.action.trigger`

Normal text messages are mapped to Codex input items and sent to the selected Codex thread through `turn/start`. Attachments are downloaded locally and converted into `localImage` or text file-path references.

Outbound Codex events are rendered as Feishu messages/cards:

- thread selection cards
- assistant streaming output
- command/tool cards
- completion cards
- approval cards

Telegram and WeChat use text-first renderers and inline/text actions instead of Feishu CardKit.

The bridge only renders events for threads that are bound to an IM conversation.

`userMessage` handling is asymmetric by design:

- Codex-origin `userMessage` items may be rendered to IM for a bound thread.
- IM-origin turns are marked in bridge-local runtime state by `turnId`.
- When Codex later emits `item/completed` for that same `userMessage`, the bridge suppresses it instead of echoing the IM message back into the same chat.

The bridge keeps one route per Codex thread. Route keys are platform-prefixed:

```text
feishu:<accountId>:<chatId>
telegram:<accountId>:<chatId>
wechat:<accountId>:<userId>
```

## Thread Subscription Model

IM channels do not automatically subscribe to every Codex thread.

The bridge keeps a one-chat-to-one-thread binding and relies on official remote-control thread APIs:

- `thread/list` for historical thread discovery
- `thread/loaded/list` for currently loaded threads
- `thread/resume { excludeTurns: true }` to subscribe to future events of a chosen thread

This is an explicit subscription step, not hidden client logic. Without it, the remote-control backend does not receive future item/turn notifications for arbitrary old threads.

Behavior:

1. An IM user sends a message.
2. If that IM conversation is already bound to a live thread, the bridge calls `turn/start`.
3. If it is not bound, the bridge asks the user to create or resume a thread instead of guessing.
4. After the user selects a thread, `codex-remote` calls `thread/resume { excludeTurns: true }`.
5. Future notifications for that thread are then eligible for IM rendering.

This keeps the implementation aligned with the official remote-control model instead of inventing a parallel thread store.

## Codex App Runtime

`codex-remote` is intentionally scoped to Codex App remote-control. Codex App is launched normally by the user, reads `chatgpt_base_url = "http://127.0.0.1:3847/backend-api"`, and opens the remote-control websocket back to the local daemon. The project does not install a CLI wrapper or start Codex processes on the user's behalf.

## Approval Handling

Codex app-server sends approval requests as JSON-RPC server requests over remote-control. The bridge stores them as pending approvals.

Important rules:

- Request ids are preserved.
- Platform actions answer the original JSON-RPC request id.
- Decision payloads are built from the Codex app-server protocol.
- If `availableDecisions` exists, the bridge uses it.
- Otherwise compatibility decisions mirror Codex TUI behavior.
- The bridge only displays one current approval per conversation.
- Additional approvals remain queued and are sent only after the current approval is resolved.

When an approval action is selected:

1. The bridge sends `{ "decision": ... }` as the response to the original Codex server request.
2. The platform message is updated when the platform supports update semantics.
3. The selected option is shown in the platform-specific format.
4. The next queued approval prompt is sent, if present.

## Local API

The daemon serves the local API on `bind`, default `127.0.0.1:3847`.
The desktop GUI is the maintained user interface; the previous web console is no longer shipped.

## State Boundaries

`codex-remote` owns only bridge-local state:

- config path
- IM channel credentials
- IM conversation to Codex thread binding
- pending approvals
- platform card ids/message ids
- downloaded attachments

Codex-owned state stays in Codex:

- project cwd
- sandbox policy
- model
- approval policy
- thread data
- tool execution semantics
- MCP configuration
- model provider keys

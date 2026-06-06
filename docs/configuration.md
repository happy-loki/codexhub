# Configuration

There are two separate config surfaces:

- `codex-remote` config, usually this repository's `config.toml`
- Codex App config, usually `~/.codex/config.toml`

Do not mix them. `codex-remote` stores IM channel and bridge settings. Codex App stores model provider, auth, and `chatgpt_base_url`.

## `codex-remote` Config

Use an explicit config path for predictable behavior:

```powershell
codex-remote --config D:\path\to\config.toml daemon
```

Example:

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

Paths relative to the config file are normalized at startup.

### `bind`

HTTP bind address for the local backend API and remote-control websocket.

Default:

```toml
bind = "127.0.0.1:3847"
```

Keep this on localhost. Do not expose it directly to a network.

### `statePath`

Path to the persisted state JSON file.

This stores local bridge state such as IM conversation bindings. It should not be committed.

## Feishu

```toml
[feishu]
appId = ""
appSecret = ""
mentionOnly = true
allowedOpenIds = []
allowedChatIds = []
```

### `appId` / `appSecret`

Feishu app credentials. The desktop GUI onboarding flow can populate these automatically.

Do not commit real credentials.

### `mentionOnly`

When `true`, group messages are ignored unless the bot is mentioned. Direct messages are still accepted.

### `allowedOpenIds`

Optional allowlist of Feishu user `open_id` values.

Empty means no user-level allowlist.

### `allowedChatIds`

Optional allowlist of Feishu chat ids.

Empty means no chat-level allowlist.

## Telegram

```toml
[telegram]
botToken = ""
allowedChatIds = []
```

### `botToken`

Telegram Bot token from BotFather. `bot_token` is also accepted for hand-written config.

This is the private-chat bot flow: create your own bot with BotFather, then send messages to that bot from your Telegram account. It does not require Telegram `api_id`, `api_hash`, phone login, or an MTProto user session.

Group chats are intentionally ignored for now. This prevents other group members from controlling the host machine through the bot.

Existing configs may still contain `mentionOnly`; it is kept for compatibility but is not used while Telegram group chats are disabled.

### `allowedChatIds`

Allowlist of Telegram private chat ids as strings.

Empty means "bind the first private chat". After the first private Telegram message is accepted, `codex-remote` writes that chat id into `allowedChatIds` and rejects other private chats.

For stricter setup, prefill this list before starting the bridge:

```toml
allowedChatIds = ["123456789"]
```

## WeChat

```toml
[wechat]
accountId = "wechat"
botToken = ""
baseUrl = ""
userId = ""
botType = "3"
allowedUserIds = []
```

WeChat config is normally written by the GUI QR onboarding flow. The implementation follows the OpenClaw WeChat bot path: QR login through `https://ilinkai.weixin.qq.com`, bot type `3`, long polling through `ilink/bot/getupdates`, and text replies through `ilink/bot/sendmessage`.

### `accountId`

Local label for the WeChat bot account. It is used in route keys and persisted state.

### `botToken`

WeChat bot token returned by QR onboarding. Do not commit real tokens.

### `baseUrl`

WeChat iLink API base URL. Leave empty unless the QR flow returns a redirected host.

### `userId`

The WeChat user id returned by onboarding. It is stored for display and allowlist defaults.

### `botType`

Current bot type. The default is `3`.

### `allowedUserIds`

Optional allowlist of WeChat user ids.

Empty means no user-level allowlist.

## Bridge

```toml
[bridge]
enabled = true
accountId = "default"
sendStreaming = true
```

### `enabled`

Controls whether the IM bridge should run.

When disabled, Feishu websocket listening, Telegram polling, and WeChat polling stop, and IM messages are not forwarded to Codex.

### `accountId`

Local label used to build route keys:

```text
feishu:<accountId>:<chatId>
telegram:<accountId>:<chatId>
wechat:<accountId>:<userId>
```

### `sendStreaming`

Controls whether assistant deltas are streamed into Feishu cards.

## Codex App Config

Codex App must point ChatGPT backend traffic at the local daemon:

```toml
chatgpt_base_url = "http://127.0.0.1:3847/backend-api"
```

This belongs in the Codex App config home, usually:

```text
~/.codex/config.toml
```

Third-party model provider keys stay in the Codex model provider section. Example:

```toml
model_provider = "llmx"
model = "gpt-5.5"

chatgpt_base_url = "http://127.0.0.1:3847/backend-api"

[model_providers.llmx]
name = "llmx"
base_url = "https://ai.llmx.cloud"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "your-third-party-key"
```

`chatgpt_base_url` is not the model API base URL. It is the ChatGPT backend-shaped URL used by Codex App features such as remote-control enrollment.
`codex-remote` does not manage Codex App runtime settings such as `[features]`, `[windows]`, `[desktop]`, `[mcp_servers]`, or per-plugin `enabled` flags.

## Codex App Auth

Remote-control requires ChatGPT-compatible auth. API-key-only auth is rejected before the websocket connects.

For this local backend, use `chatgpt` in Codex App's `auth.json`:

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

The local JWT needs to parse as a JWT and include the ChatGPT-shaped auth metadata Codex reads:

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

This identity is local bridge identity only. The model provider key controls the actual model provider.

The desktop GUI provides Codex App configuration controls that write the local Codex App config for you.

The CLI equivalent is:

```powershell
codex-remote --config config.toml configure-codex-app
```

Optional provider fields:

```powershell
codex-remote --config config.toml configure-codex-app --provider-name llmx --provider-base-url https://ai.llmx.cloud --provider-key sk-... --model gpt-5.5
```

When provider fields are supplied without `--provider-name`, `llmx` is used as the provider name.

The daemon does not modify Codex App config on startup. It writes these files only when the desktop GUI or CLI command is used.

## Feishu App Requirements

For a manually created Feishu app, enable bot messaging and websocket event delivery. Subscribe to:

```text
im.message.receive_v1
card.action.trigger
```

Typical permissions:

```text
im:message
im:message:send_as_bot
im:resource
```

Depending on Feishu app type and tenant policy, additional scopes may be required for card updates or attachment downloads.

## Local Files To Keep Private

These should stay ignored:

```text
config.toml
codex-remote-state.json
*.log
.im/
target/
target-verify/
reference/
```

Do not commit Codex App `auth.json`, third-party provider keys, Feishu credentials, open ids, or chat ids.

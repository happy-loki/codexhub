# Configuration

There are two separate config surfaces:

- `codexhub` config, usually this repository's `config.toml`
- Codex App config, usually `~/.codex/config.toml`

Do not mix them. `codexhub` stores IM channel and bridge settings. Codex App stores model provider, auth, and `chatgpt_base_url`.

## `codexhub` Config

Use an explicit config path for predictable behavior:

```powershell
codexhub --config D:\path\to\config.toml daemon
```

Example:

```toml
bind = "127.0.0.1:3847"
statePath = "codexhub-state.json"

[outboundProxy]
mode = "system"
url = ""

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

### `outboundProxy`

Controls only requests that CodexHub sends to external services such as model providers,
WeChat, Telegram, Feishu HTTP APIs, and update endpoints. It does not change the operating
system proxy or the environment of other applications.

```toml
[outboundProxy]
mode = "system" # system | direct | custom
url = ""
```

- `system` follows the operating system proxy and proxy environment variables.
- `direct` disables proxy discovery for CodexHub HTTP requests.
- `custom` uses `url` as an explicit HTTP, HTTPS, SOCKS5, or SOCKS5H proxy.

Example for a local Clash mixed port:

```toml
[outboundProxy]
mode = "custom"
url = "http://127.0.0.1:7890"
```

The desktop GUI exposes the same setting under `Network` and applies it immediately while the
daemon is running. Local GUI-to-daemon requests always bypass proxies. A VPN implemented as a TUN or Network Extension may still route traffic below
the HTTP proxy layer; configure loopback exclusions in that VPN when necessary.

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

Empty means "bind the first private chat". After the first private Telegram message is accepted, `codexhub` writes that chat id into `allowedChatIds` and rejects other private chats.

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

## WeCom (Enterprise WeChat)

```toml
[wecom]
enabled = true
accountId = "wecom"
botId = ""
secret = ""
displayName = "企业微信机器人"
websocketUrl = "wss://openws.work.weixin.qq.com"
allowedUserIds = []
allowedChatIds = []
```

The GUI QR flow normally writes `botId` and `secret`. CodexHub then subscribes to the official WeCom AI Bot WebSocket and supports direct/group text, streaming and final replies, initial/history thread routing cards, image/file input and output, and interactive approval template cards. Empty allowlists accept all users and chats. Keep `secret` private.

## Bridge

```toml
[bridge]
enabled = true
accountId = "default"
sendStreaming = true
```

### `enabled`

Controls whether the IM bridge should run.

When disabled, Feishu and WeCom websocket listening, Telegram polling, and WeChat polling stop, and IM messages are not forwarded to Codex.

### `accountId`

Local label used to build route keys:

```text
feishu:<accountId>:<chatId>
telegram:<accountId>:<chatId>
wechat:<accountId>:<userId>
wecom:<accountId>:<userId-or-groupChatId>
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
`codexhub` does not manage Codex App runtime settings such as `[features]`, `[windows]`, `[desktop]`, `[mcp_servers]`, or per-plugin `enabled` flags.

When CodexHub injects its default local AI Gateway provider, it keeps ChatGPT-shaped authentication enabled so Codex App retains its account-backed model catalog and Remote Control state:

```toml
web_search = "live"

[model_providers.ai-gateway]
name = "ai-gateway"
base_url = "http://127.0.0.1:3847/ai-gateway/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "dummy-token"
```

The provider identity remains `ai-gateway`, so `provider.is_openai()` is false and OpenAI-only remote compaction, request compression, and private metadata behavior stay disabled. The managed provider intentionally does not use Actor Authorization by default.

Actor Authorization requires `requires_openai_auth = false`, which makes the provider account API return no account. In Codex App 26.707.8479 this also causes the frontend to apply the official Statsig `available_models` allowlist; custom CodexHub models then disappear even though `/ai-gateway/v1/models` returns them. For that reason the native `web.run` provider gate remains disabled in the default configuration, and GPT-5.6 uses CodexHub's hosted `web_search` compatibility path instead. The `/alpha/search` proxy remains available for future Codex versions or explicit experimental configurations.

## Codex App Auth

Remote-control requires ChatGPT-compatible auth. API-key-only auth is rejected before the websocket connects.

For this local backend, use `chatgptAuthTokens` in Codex App's `auth.json`:

```json
{
  "auth_mode": "chatgptAuthTokens",
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "<local ChatGPT-shaped JWT>",
    "access_token": "<local ChatGPT-shaped JWT>",
    "refresh_token": "",
    "account_id": "acct_codexhub_local"
  },
  "last_refresh": "2026-05-26T00:00:00Z"
}
```

The local JWT needs to parse as a JWT and include the ChatGPT-shaped auth metadata Codex reads:

```json
{
  "email": "codexhub-local@example.local",
  "https://api.openai.com/auth": {
    "chatgpt_account_id": "acct_codexhub_local",
    "chatgpt_user_id": "user_codexhub_local",
    "user_id": "user_codexhub_local",
    "chatgpt_plan_type": "pro",
    "chatgpt_account_is_fedramp": false
  }
}
```

This identity is local bridge identity only. The model provider key controls the actual model provider.

The desktop GUI provides Codex App configuration controls that write the local Codex App config for you.

The CLI equivalent is:

```powershell
codexhub --config config.toml configure-codex-app
```

Optional provider fields:

```powershell
codexhub --config config.toml configure-codex-app --provider-name llmx --provider-base-url https://ai.llmx.cloud --provider-key sk-... --model gpt-5.5
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
codexhub-state.json
*.log
.im/
target/
target-verify/
reference/
```

Do not commit Codex App `auth.json`, third-party provider keys, Feishu credentials, open ids, or chat ids.

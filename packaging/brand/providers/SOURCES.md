# Brand Assets

Brand SVGs are copied from `@lobehub/icons` version `5.8.0` when the package contains a matching icon. The package is distributed under the MIT license.

- Codex: `references/lobehub-icons/es/Codex/components/Color.js` -> `packaging/brand/codex.svg`
- OpenAI: `references/lobehub-icons/es/OpenAI/components/Mono.js`
- OpenAI badge: `packaging/brand/openai-badge.svg`; based on the OpenAI icon above, with a light badge background so status icons remain visible in dark theme.
- DeepSeek: `references/lobehub-icons/es/DeepSeek/components/Color.js`
- Anthropic: `references/lobehub-icons/es/Anthropic/components/Mono.js`
- Zhipu: `references/lobehub-icons/es/Zhipu/components/Color.js`

Lucide UI icons are copied from Lucide Icons and covered by `packaging/brand/LICENSE.lucide-icons`.

- Codex CLI terminal: Lucide `terminal` -> `packaging/brand/codex-cli.svg`
- Local service: Lucide `server` -> `packaging/brand/service-server.svg`

Fallback local assets:

- Telegram: `packaging/brand/telegram-logo.svg`; official SVG from https://telegram.org/img/t_logo.svg
- WeChat: `packaging/brand/wechat-logo.svg`; from Simple Icons (https://simpleicons.org), CC0 1.0 Universal
- Feishu: `packaging/brand/feishu-logo.png`; official GitHub avatar from https://avatars.githubusercontent.com/u/54944174?s=200&v=4
- VS Code: `packaging/brand/vscode-logo.svg`; custom SVG using official VS Code brand colors (#007ACC)
- App icon: `packaging/icons/dolphin-rounded-256.png`; this is the Codex Remote Gateway application icon.

The `references/` directory is intentionally ignored by git, so extracted and fallback assets live under `packaging/brand/` or `packaging/icons/` for compile-time embedding in the GUI.

## High-DPI Rendering

All logo SVGs are designed to render crisply at any scale, including Retina and high-DPI displays. The GUI uses `BitmapBundle::from_svg_data()` to create resolution-independent graphics that automatically scale to the display's pixel density.

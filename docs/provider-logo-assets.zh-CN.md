# 品牌图标资源接入说明

GUI 首页状态栏、IM 通道、AI Gateway 渠道列表和新增渠道弹窗都会展示品牌图标。当前项目不从运行时加载第三方包，所有实际使用的图标都要落地到仓库内，并通过 `include_bytes!` 编译进 GUI。

## 资源来源

优先使用 `@lobehub/icons`，通用 UI 图标使用 Lucide：

- 本地参考目录：`references/lobehub-icons`
- NPM 包名：`@lobehub/icons`
- 当前使用版本：`5.8.0`
- 授权：MIT License
- 已落地授权文件：`packaging/brand/providers/LICENSE.lobehub-icons`
- Lucide 授权：ISC License，已落地授权文件：`packaging/brand/LICENSE.lucide-icons`

`references/` 目录已被 `.gitignore` 忽略，只作为取材参考。真正参与编译和提交的资源必须复制到：

```text
packaging/brand/
packaging/brand/providers/
```

当前已接入：

- Codex：`packaging/brand/codex.svg`
- Telegram：`packaging/brand/telegram-logo.svg`
- 微信：`packaging/brand/wechat-logo.svg`
- 飞书：`packaging/brand/feishu-logo.png`，`@lobehub/icons` 当前没有对应组件，使用飞书 GitHub 官方头像资源
- VS Code 插件：`packaging/brand/openai-badge.svg`，基于 LobeHub OpenAI 图形增加浅色徽章背景，保证暗色主题可见
- Codex CLI：`packaging/brand/codex-cli.svg`，来自 Lucide `terminal`
- 本地服务：`packaging/brand/service-server.svg`，来自 Lucide `server`
- OpenAI：`packaging/brand/providers/openai.svg`
- Grok：`packaging/brand/providers/grok.svg`
- DeepSeek：`packaging/brand/providers/deepseek.svg`
- Kimi：`packaging/brand/providers/kimi.svg`
- Anthropic：`packaging/brand/providers/anthropic.svg`
- 智谱：`packaging/brand/providers/zhipu.svg`
- 来源记录：`packaging/brand/providers/SOURCES.md`

## 从 @lobehub/icons 提取

一般路径如下：

```text
references/lobehub-icons/es/<Provider>/components/
```

常见组件：

- `Color.js`：彩色图标，适合品牌识别。
- `Mono.js`：单色图标，适合跟随产品 UI 风格。
- `Text.js`：带文字的横向 logo，通常不适合小尺寸列表图标。

提取时只复制 SVG 需要的信息：

- `<svg ... viewBox="...">`
- `<title>ProviderName</title>`
- `<path ...>`、`<g ...>` 等图形节点
- 原始 fill / stroke 信息

不要把 React/JSX 组件代码放入项目资源目录。

## SVG 处理规则

小图标在 wxWidgets / wxDragon 下容易遇到裁切，尤其是原始 path 贴满 `viewBox` 边缘的 logo。处理原则：

1. 保留 SVG 作为源资产，不手绘 logo。
2. 如果图形贴边，在 SVG 内部加真实留白，例如：

```xml
<g transform="translate(2.4 2.4) scale(0.8)">
  ...
</g>
```

3. GUI 里不要直接把 SVG `BitmapBundle` 塞给 `StaticBitmap` 显示。当前项目使用固定尺寸 bitmap：

```rust
BitmapBundle::from_svg_data(bytes, Size::new(size, size))
    .and_then(|bundle| bundle.get_bitmap(Size::new(size, size)))
```

这样可以避免 `wxStaticBitmap` 按 SVG bundle 的 intrinsic size 在较小行高里裁切。

## 新增图标步骤

1. 在 `references/lobehub-icons/es/<Provider>/components/` 找合适组件，优先 `Color.js` 或 `Mono.js`。
2. 提取 SVG，保存到：

```text
packaging/brand/<name>.svg
packaging/brand/providers/<provider>.svg
```

3. 如果 `@lobehub/icons` 没有对应组件，优先从 Lucide 这类明确开源授权的图标集选择通用 UI 图标；仍然拿不到时才允许使用自备 SVG/PNG。
4. 更新来源记录：

```text
packaging/brand/providers/SOURCES.md
```

5. 在 `src/gui/widgets.rs` 增加枚举和 `include_bytes!`：

```rust
pub(super) enum ProviderLogoKind {
    OpenAi,
    DeepSeek,
    NewProvider,
}
```

并在 `provider_logo_bitmap`、`status_icon_bitmap` 或 `im_channel_icon_bitmap` 中加入对应资源。

6. 在 AI Gateway 渠道选择 UI 中引用这个 logo：

```rust
Some(ProviderLogoKind::NewProvider)
```

7. 运行验证：

```powershell
cargo build --release --features gui --bin codexhub
cargo test --bin codexhub ai_gateway
```

## 注意事项

- 不要提交 `references/`，它只是本地参考源。
- 不要从网络运行时加载 logo，GUI 应保持离线可用。
- 新增第三方资源时必须记录来源和授权。
- 如果 logo 在小尺寸下仍被裁切，先调整 SVG 内部留白，再确认 UI 里使用的是 `provider_logo_bitmap(..., 24)` 这种固定尺寸 bitmap。

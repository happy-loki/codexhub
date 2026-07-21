use crate::config::LocalConnectionMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GuiLocale {
    ZhCn,
    EnUs,
}

impl GuiLocale {
    pub(super) fn from_code(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "zh-cn" | "zh_cn" | "zh" | "cn" => Some(Self::ZhCn),
            "en-us" | "en_us" | "en" => Some(Self::EnUs),
            _ => None,
        }
    }

    pub(super) fn code(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::EnUs => "en-US",
        }
    }
}

impl Default for GuiLocale {
    fn default() -> Self {
        Self::ZhCn
    }
}

#[derive(Clone, Copy)]
pub(super) struct GuiText {
    pub(super) locale: GuiLocale,
}

impl GuiText {
    pub(super) fn new(locale: GuiLocale) -> Self {
        Self { locale }
    }

    pub(super) fn version(self) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("版本 {}", env!("CARGO_PKG_VERSION")),
            GuiLocale::EnUs => format!("Version {}", env!("CARGO_PKG_VERSION")),
        }
    }

    pub(super) fn file_menu(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "文件",
            GuiLocale::EnUs => "&File",
        }
    }

    pub(super) fn close_window(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "关闭窗口\tCtrl+W",
            GuiLocale::EnUs => "&Close Window\tCtrl+W",
        }
    }

    pub(super) fn close_window_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "隐藏窗口，CodexHub 会继续在托盘运行",
            GuiLocale::EnUs => "Hide this window and keep CodexHub running in the tray",
        }
    }

    pub(super) fn minimize(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "最小化\tCtrl+M",
            GuiLocale::EnUs => "Mi&nimize\tCtrl+M",
        }
    }

    pub(super) fn minimize_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "最小化窗口",
            GuiLocale::EnUs => "Minimize this window",
        }
    }

    pub(super) fn quit(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "退出 CodexHub\tCtrl+Q",
            GuiLocale::EnUs => "&Quit CodexHub\tCtrl+Q",
        }
    }

    pub(super) fn quit_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "退出 CodexHub 并停止本地服务",
            GuiLocale::EnUs => "Quit CodexHub and stop the local service",
        }
    }

    pub(super) fn tray_open(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "打开 CodexHub",
            GuiLocale::EnUs => "Open CodexHub",
        }
    }

    pub(super) fn tray_open_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "显示主窗口",
            GuiLocale::EnUs => "Show the main window",
        }
    }

    pub(super) fn tray_still_running_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "CodexHub 已隐藏到托盘，本地服务会继续运行。需要退出时请使用托盘菜单里的“退出 CodexHub”。"
            }
            GuiLocale::EnUs => {
                "CodexHub is hidden in the tray and the local service keeps running. Use Quit CodexHub from the tray menu to exit."
            }
        }
    }

    pub(super) fn language_menu(self) -> &'static str {
        "&Language / 语言"
    }

    pub(super) fn language_zh_cn(self) -> &'static str {
        "中文（简体）"
    }

    pub(super) fn language_en_us(self) -> &'static str {
        "English"
    }

    pub(super) fn language_restart_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "语言设置已保存，重启 CodexHub 后生效。",
            GuiLocale::EnUs => "Language saved. Restart CodexHub to apply it.",
        }
    }

    pub(super) fn language_save_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "语言设置保存失败",
            GuiLocale::EnUs => "Failed to save language setting",
        }
    }

    pub(super) fn theme_menu(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "主题",
            GuiLocale::EnUs => "&Theme",
        }
    }

    pub(super) fn theme_system(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "跟随系统",
            GuiLocale::EnUs => "Follow System",
        }
    }

    pub(super) fn theme_light(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "浅色",
            GuiLocale::EnUs => "Light",
        }
    }

    pub(super) fn theme_dark(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "深色",
            GuiLocale::EnUs => "Dark",
        }
    }

    pub(super) fn theme_restart_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "主题设置已保存，重启 CodexHub 后生效。",
            GuiLocale::EnUs => "Theme saved. Restart CodexHub to apply it.",
        }
    }

    pub(super) fn theme_save_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "主题设置保存失败",
            GuiLocale::EnUs => "Failed to save theme setting",
        }
    }

    pub(super) fn network_menu(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "网络",
            GuiLocale::EnUs => "&Network",
        }
    }

    pub(super) fn outbound_proxy_system(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "跟随系统代理",
            GuiLocale::EnUs => "Use System Proxy",
        }
    }

    pub(super) fn outbound_proxy_system_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "CodexHub 访问外部 API 时跟随系统代理设置",
            GuiLocale::EnUs => "Use the system proxy for CodexHub external API requests",
        }
    }

    pub(super) fn outbound_proxy_direct(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "强制直连",
            GuiLocale::EnUs => "Direct Connection",
        }
    }

    pub(super) fn outbound_proxy_direct_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "CodexHub 访问外部 API 时不使用任何代理",
            GuiLocale::EnUs => "Do not use a proxy for CodexHub external API requests",
        }
    }

    pub(super) fn outbound_proxy_custom(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "自定义 HTTP/SOCKS5 代理...",
            GuiLocale::EnUs => "Custom HTTP/SOCKS5 Proxy...",
        }
    }

    pub(super) fn outbound_proxy_custom_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "指定 CodexHub 自身使用的出站代理",
            GuiLocale::EnUs => "Set an explicit outbound proxy for CodexHub",
        }
    }

    pub(super) fn outbound_proxy_prompt(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "输入代理 URL，例如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080"
            }
            GuiLocale::EnUs => {
                "Enter a proxy URL, such as http://127.0.0.1:7890 or socks5://127.0.0.1:1080"
            }
        }
    }

    pub(super) fn outbound_proxy_restart_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "出站代理设置已保存，重启 CodexHub 后生效。",
            GuiLocale::EnUs => "Outbound proxy saved. Restart CodexHub to apply it.",
        }
    }

    pub(super) fn outbound_proxy_applied_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "出站代理设置已保存并立即生效。",
            GuiLocale::EnUs => "Outbound proxy saved and applied.",
        }
    }

    pub(super) fn outbound_proxy_save_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "出站代理设置保存失败",
            GuiLocale::EnUs => "Failed to save outbound proxy setting",
        }
    }

    pub(super) fn help_menu(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "帮助",
            GuiLocale::EnUs => "&Help",
        }
    }

    pub(super) fn check_updates(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "检查更新",
            GuiLocale::EnUs => "&Check for Updates",
        }
    }

    pub(super) fn check_updates_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "检查 GitHub Releases 是否有新版本",
            GuiLocale::EnUs => "Check GitHub Releases for a newer CodexHub version",
        }
    }

    pub(super) fn export_connection_diagnostics(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "导出连接诊断包",
            GuiLocale::EnUs => "Export Connection Diagnostics",
        }
    }

    pub(super) fn export_connection_diagnostics_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "导出 Codex / VSCode / CLI 连接状态和最近日志，不包含 AI Gateway 请求日志"
            }
            GuiLocale::EnUs => {
                "Export Codex / VS Code / CLI connection state and recent logs without AI Gateway request logs"
            }
        }
    }

    pub(super) fn about(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "关于 CodexHub",
            GuiLocale::EnUs => "&About CodexHub",
        }
    }

    pub(super) fn status_overview(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "状态概览",
            GuiLocale::EnUs => "Status",
        }
    }

    pub(super) fn codex_control_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex App 控制通道",
            GuiLocale::EnUs => "Codex App Control",
        }
    }

    pub(super) fn vscode_extension(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "VS Code 插件",
            GuiLocale::EnUs => "VS Code Extension",
        }
    }

    pub(super) fn codex_cli(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex CLI",
            GuiLocale::EnUs => "Codex CLI",
        }
    }

    pub(super) fn local_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地服务",
            GuiLocale::EnUs => "Local Service",
        }
    }

    pub(super) fn local_connection_standard(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "标准连接",
            GuiLocale::EnUs => "Standard connection",
        }
    }

    pub(super) fn local_connection_vpn_compatible(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "VPN 兼容连接",
            GuiLocale::EnUs => "VPN-compatible connection",
        }
    }

    pub(super) fn local_connection_label(self, mode: LocalConnectionMode) -> &'static str {
        match mode {
            LocalConnectionMode::Standard => self.local_connection_standard(),
            LocalConnectionMode::VpnCompatible => self.local_connection_vpn_compatible(),
        }
    }

    pub(super) fn local_connection_settings(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "连接设置",
            GuiLocale::EnUs => "Connection settings",
        }
    }

    pub(super) fn local_connection_settings_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "切换本地服务连接方式",
            GuiLocale::EnUs => "Switch how the local service is reached",
        }
    }

    pub(super) fn switch_to_vpn_compatible_connection(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "切换到 VPN 兼容连接",
            GuiLocale::EnUs => "Switch to VPN-compatible connection",
        }
    }

    pub(super) fn switch_to_standard_connection(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "切回标准连接",
            GuiLocale::EnUs => "Switch to standard connection",
        }
    }

    pub(super) fn local_connection_switch_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "切换后需要重启 CodexHub，之后再重新初始化 Codex 配置。",
            GuiLocale::EnUs => {
                "Restart CodexHub after switching, then initialize Codex config again."
            }
        }
    }

    pub(super) fn local_connection_restart_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "连接模式已更新。请重启 CodexHub，之后再重新初始化 Codex 配置。",
            GuiLocale::EnUs => {
                "Connection mode updated. Restart CodexHub, then initialize Codex config again."
            }
        }
    }

    pub(super) fn local_connection_save_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "连接模式保存失败",
            GuiLocale::EnUs => "Failed to save connection mode",
        }
    }

    pub(super) fn local_connection_detected_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地连接异常",
            GuiLocale::EnUs => "Local Connection Issue",
        }
    }

    pub(super) fn local_connection_detected_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "检测到当前网络环境可能影响本地连接，建议切换到 VPN 兼容连接。切换后需要重启 CodexHub。"
            }
            GuiLocale::EnUs => {
                "The current network environment may affect local connections. Switch to VPN-compatible connection, then restart CodexHub."
            }
        }
    }

    pub(super) fn detecting(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "检测中",
            GuiLocale::EnUs => "Checking",
        }
    }

    pub(super) fn unavailable(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "暂不可用",
            GuiLocale::EnUs => "Unavailable",
        }
    }

    pub(super) fn app_gui_unsupported(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "当前平台暂不支持 App GUI",
            GuiLocale::EnUs => "App GUI is not supported on this platform.",
        }
    }

    pub(super) fn provider_management(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Provider 管理",
            GuiLocale::EnUs => "Provider Management",
        }
    }

    pub(super) fn add(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "新增",
            GuiLocale::EnUs => "Add",
        }
    }

    pub(super) fn save(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存",
            GuiLocale::EnUs => "Save",
        }
    }

    pub(super) fn delete(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除",
            GuiLocale::EnUs => "Delete",
        }
    }

    pub(super) fn enable(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启用",
            GuiLocale::EnUs => "Enable",
        }
    }

    pub(super) fn new_provider_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清空表单，新增一个 provider",
            GuiLocale::EnUs => "Clear the form and add a provider",
        }
    }

    pub(super) fn save_provider_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存或更新当前表单里的 provider",
            GuiLocale::EnUs => "Save or update the provider in the form",
        }
    }

    pub(super) fn delete_provider_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除当前选中的 provider",
            GuiLocale::EnUs => "Delete the selected provider",
        }
    }

    pub(super) fn configure_provider_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存并使用这个模型服务",
            GuiLocale::EnUs => "Save and use this model provider",
        }
    }

    pub(super) fn provider_catalog_loading(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在匹配 ~/.codex/config.toml 里的 provider",
            GuiLocale::EnUs => "Reading providers from ~/.codex/config.toml",
        }
    }

    pub(super) fn provider_name(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Provider 名称",
            GuiLocale::EnUs => "Provider Name",
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "名称",
            GuiLocale::EnUs => "Name",
        }
    }

    pub(super) fn current(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "当前",
            GuiLocale::EnUs => "Current",
        }
    }

    pub(super) fn api_key_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "API Key 已保存时会用星号显示；需要更换时直接输入新 key。",
            GuiLocale::EnUs => "Saved API keys are masked. Enter a new key to replace it.",
        }
    }

    pub(super) fn image_generation_feature(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "过滤生图工具",
            GuiLocale::EnUs => "Filter image generation tool",
        }
    }

    pub(super) fn image_generation_feature_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "勾选后，AI Gateway 会从 Codex 请求中移除 image_generation 和 image_gen 生图工具。"
            }
            GuiLocale::EnUs => {
                "When checked, AI Gateway removes image_generation and image_gen tools from Codex requests."
            }
        }
    }

    pub(super) fn image_generation_feature_note(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "实时生效",
            GuiLocale::EnUs => "Applies immediately",
        }
    }

    pub(super) fn ai_gw_behavior(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "网关行为",
            GuiLocale::EnUs => "Gateway Behavior",
        }
    }

    pub(super) fn codex_local_config(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 初始化",
            GuiLocale::EnUs => "Codex Setup",
        }
    }

    pub(super) fn codex_local_config_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "初始化 CodexHub 所需配置，可随时恢复到初始化前状态，包括 ChatGPT 登录状态。"
            }
            GuiLocale::EnUs => {
                "Set up CodexHub integration. You can restore the pre-setup state anytime, including ChatGPT sign-in."
            }
        }
    }

    pub(super) fn codex_visible_models(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 可见模型",
            GuiLocale::EnUs => "Codex visible models",
        }
    }

    pub(super) fn codex_visible_models_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "选择 Codex 里要显示的模型。这里勾选只决定 Codex 能看到哪些模型名称。"
            }
            GuiLocale::EnUs => {
                "Choose which models are shown in Codex. This only controls the model names Codex can see."
            }
        }
    }

    pub(super) fn codex_visible_models_scope_warning(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "重要：普通方式启动 Codex App 时，模型列表仍只影响 CLI；请使用上方“增强模式启动 Codex”同步前端模型列表。"
            }
            GuiLocale::EnUs => {
                "Important: With a normally launched Codex App, this list still only affects the CLI. Use Launch Codex in Enhanced Mode above to sync the App model picker."
            }
        }
    }

    pub(super) fn codex_visible_models_warning(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "注意：如果 Codex 报 “not configured in any provider”，请在大模型接入里添加对应大模型厂商并配置模型；也可以设置模型映射，例如把 Codex 里的 gpt-5.4-mini 映射到实际可用的 deepseek-v4-flash。"
            }
            GuiLocale::EnUs => {
                "Note: If Codex reports \"not configured in any provider\", add the matching provider and model in AI Gateway, or map a Codex model name such as gpt-5.4-mini to an available upstream model such as deepseek-v4-flash."
            }
        }
    }

    pub(super) fn codex_session_history(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "会话历史管理",
            GuiLocale::EnUs => "Session History",
        }
    }

    pub(super) fn codex_session_history_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "把以前的 Codex 会话归到当前入口，之后可以在 Codex App、VS Code 插件或 CLI 里继续打开。"
            }
            GuiLocale::EnUs => {
                "Move existing Codex sessions into the current entry so they can be opened from Codex App, VS Code, or CLI."
            }
        }
    }

    pub(super) fn open_codex_session_history(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "会话历史修复管理",
            GuiLocale::EnUs => "Manage Session History",
        }
    }

    pub(super) fn session_history_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 会话管理",
            GuiLocale::EnUs => "Codex Session History",
        }
    }

    pub(super) fn other_provider_sessions(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "其它会话",
            GuiLocale::EnUs => "Other Sessions",
        }
    }

    pub(super) fn ai_gateway_sessions(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "AI Gateway 会话",
            GuiLocale::EnUs => "AI Gateway Sessions",
        }
    }

    pub(super) fn refresh(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "刷新",
            GuiLocale::EnUs => "Refresh",
        }
    }

    pub(super) fn move_to_ai_gateway(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "归到 AI Gateway",
            GuiLocale::EnUs => "Move to AI Gateway",
        }
    }

    pub(super) fn move_to_ai_gateway_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "把选中的会话移动到 AI Gateway",
            GuiLocale::EnUs => "Move selected sessions to AI Gateway",
        }
    }

    pub(super) fn move_back_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "移动到 Provider...",
            GuiLocale::EnUs => "Move to Provider...",
        }
    }

    pub(super) fn move_back_provider_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "把选中的会话移动到其它 Provider",
            GuiLocale::EnUs => "Move selected sessions to another provider",
        }
    }

    pub(super) fn session_history_selection_hint(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "可按 Ctrl 多选、Shift 连续选择，也可以右键操作。移动后如果 Codex 侧边栏仍看不到，请在 Codex 里重新添加对应 workspace。"
            }
            GuiLocale::EnUs => {
                "Use Ctrl for multiple selection, Shift for ranges, or right-click. If moved sessions still do not appear in Codex, add the workspace again in Codex."
            }
        }
    }

    pub(super) fn session_col_provider(self) -> &'static str {
        "Provider"
    }

    pub(super) fn session_col_preview(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "会话",
            GuiLocale::EnUs => "Session",
        }
    }

    pub(super) fn session_col_workspace(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Workspace",
            GuiLocale::EnUs => "Workspace",
        }
    }

    pub(super) fn session_select_left_first(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请先在左侧选择一个会话。",
            GuiLocale::EnUs => "Select a session on the left first.",
        }
    }

    pub(super) fn session_select_right_first(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请先在右侧选择一个会话。",
            GuiLocale::EnUs => "Select a session on the right first.",
        }
    }

    pub(super) fn session_select_provider_first(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请选择目标 Provider。",
            GuiLocale::EnUs => "Choose a target provider.",
        }
    }

    pub(super) fn session_target_provider_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "选择 Provider",
            GuiLocale::EnUs => "Choose Provider",
        }
    }

    pub(super) fn session_target_provider_prompt(self, count: usize) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                if count <= 1 {
                    "把选中的会话移动到：".to_string()
                } else {
                    format!("把选中的 {count} 个会话移动到：")
                }
            }
            GuiLocale::EnUs => {
                if count <= 1 {
                    "Move the selected session to:".to_string()
                } else {
                    format!("Move {count} selected sessions to:")
                }
            }
        }
    }

    pub(super) fn move_sessions_confirm(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "移动",
            GuiLocale::EnUs => "Move",
        }
    }

    pub(super) fn session_no_target_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "没有可选的 Provider。请先在 Codex 配置里添加其它 Provider。",
            GuiLocale::EnUs => {
                "No provider is available. Add another provider in Codex settings first."
            }
        }
    }

    pub(super) fn session_move_done(self, count: usize) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                if count <= 1 {
                    "会话归属已更新。".to_string()
                } else {
                    format!("{count} 个会话归属已更新。")
                }
            }
            GuiLocale::EnUs => {
                if count <= 1 {
                    "Session updated.".to_string()
                } else {
                    format!("{count} sessions updated.")
                }
            }
        }
    }

    pub(super) fn save_codex_models(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存模型列表",
            GuiLocale::EnUs => "Save model list",
        }
    }

    pub(super) fn saving_codex_models(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存中...",
            GuiLocale::EnUs => "Saving...",
        }
    }

    pub(super) fn codex_models_saved(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "Codex 可见模型列表已保存。\nCodex 端模型列表有缓存，通常会在 5 分钟内更新。"
            }
            GuiLocale::EnUs => {
                "Codex visible model list saved.\nCodex caches the model picker list and usually updates it within 5 minutes."
            }
        }
    }

    pub(super) fn codex_enhanced_launch(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "增强模式启动 Codex",
            GuiLocale::EnUs => "Enhanced Launch",
        }
    }

    pub(super) fn codex_enhanced_launching(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在增强模式启动 Codex...",
            GuiLocale::EnUs => "Launching Codex in enhanced mode...",
        }
    }

    pub(super) fn codex_enhanced_launch_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "使用 CodexHub 当前模型列表启动 Codex，并同步 Codex 前端模型选项。Codex App 正在运行时，需要先完全退出。"
            }
            GuiLocale::EnUs => {
                "Launch Codex with the current CodexHub model list and sync the model picker. Exit Codex App first if it is already running."
            }
        }
    }

    pub(super) fn codex_enhanced_launch_ready(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 已使用增强模式启动，前端模型列表已同步。",
            GuiLocale::EnUs => {
                "Codex is running in enhanced mode with the custom model list synchronized."
            }
        }
    }

    pub(super) fn codex_enhanced_launch_close_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请先关闭 Codex App",
            GuiLocale::EnUs => "Exit Codex App First",
        }
    }

    pub(super) fn codex_enhanced_launch_waiting_for_close(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在等待 Codex App 退出...",
            GuiLocale::EnUs => "Waiting for Codex App to exit...",
        }
    }

    pub(super) fn codex_enhanced_launch_confirm(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "增强模式启动",
            GuiLocale::EnUs => "Launch in Enhanced Mode",
        }
    }

    pub(super) fn codex_enhanced_launch_close_running(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "检测到 Codex App 正在运行。请完全退出 Codex App，退出后即可使用增强模式启动。"
            }
            GuiLocale::EnUs => {
                "Codex App is running. Exit it completely before launching in enhanced mode."
            }
        }
    }

    pub(super) fn codex_enhanced_launch_ready_to_start(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未检测到 Codex App 进程，可以使用增强模式启动 Codex。",
            GuiLocale::EnUs => "Codex App is not running. Enhanced mode is ready.",
        }
    }

    pub(super) fn codex_enhanced_launch_check_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "暂时无法检测 Codex App 状态，请稍后重试。",
            GuiLocale::EnUs => "Could not check Codex App status. Please try again.",
        }
    }

    pub(super) fn codex_enhanced_launch_check_failed_detail(self, detail: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!(
                "暂时无法检测 Codex App 状态。请稍后重试；如果反复出现，请完全退出并重新打开 CodexHub。\n检测详情：{detail}"
            ),
            GuiLocale::EnUs => format!(
                "Could not check Codex App status. Please try again. If it keeps happening, fully exit and reopen CodexHub.\nDetails: {detail}"
            ),
        }
    }

    pub(super) fn provider_websocket(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启用 WebSocket",
            GuiLocale::EnUs => "Enable WebSocket",
        }
    }

    pub(super) fn clear_codex_access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "恢复 Codex 原有配置",
            GuiLocale::EnUs => "Restore Codex Config",
        }
    }

    pub(super) fn clear_codex_access_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "恢复写入前的 Codex 连接",
            GuiLocale::EnUs => "Restore the Codex connection from before setup",
        }
    }

    pub(super) fn inject_codex_access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "初始化 Codex 配置",
            GuiLocale::EnUs => "Set Up Codex Config",
        }
    }

    pub(super) fn inject_codex_access_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "让 Codex 使用本地服务",
            GuiLocale::EnUs => "Use the local service from Codex",
        }
    }

    pub(super) fn injecting_codex_access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "初始化中...",
            GuiLocale::EnUs => "Setting up...",
        }
    }

    pub(super) fn clearing_codex_access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "恢复中...",
            GuiLocale::EnUs => "Restoring...",
        }
    }

    pub(super) fn codex_tab(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 接入",
            GuiLocale::EnUs => "Codex",
        }
    }

    pub(super) fn chat_tab(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "聊天工具接入",
            GuiLocale::EnUs => "Chat Integrations",
        }
    }

    pub(super) fn request_logs_tab(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请求日志",
            GuiLocale::EnUs => "Request Logs",
        }
    }

    pub(super) fn enable_request_logging(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启用请求日志",
            GuiLocale::EnUs => "Enable request logging",
        }
    }

    pub(super) fn enable_request_logging_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "开启后记录大模型接入请求摘要指标，便于调试和分析",
            GuiLocale::EnUs => {
                "When enabled, records AI Gateway request summary metrics for debugging and analysis"
            }
        }
    }

    pub(super) fn enable_request_log_details(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "记录详情",
            GuiLocale::EnUs => "Record details",
        }
    }

    pub(super) fn enable_request_log_details_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "开启后额外保存请求、上游请求、上游 SSE 和响应内容；可能占用更多资源"
            }
            GuiLocale::EnUs => {
                "When enabled, also stores request, upstream request, upstream SSE, and response payloads"
            }
        }
    }

    pub(super) fn request_logging_disabled_hint(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请求日志已关闭。如需查看日志，请在大模型接入管理中启用日志记录。",
            GuiLocale::EnUs => {
                "Request logging is disabled. To view logs, enable logging in AI Gateway management."
            }
        }
    }

    pub(super) fn request_logs(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请求日志",
            GuiLocale::EnUs => "Request Logs",
        }
    }

    pub(super) fn request_log_open_hint(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "双击查看请求详情",
            GuiLocale::EnUs => "Double-click a request to view details",
        }
    }

    pub(super) fn request_log_clear_old(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清理3天之外",
            GuiLocale::EnUs => "Clear Older Than 3 Days",
        }
    }

    pub(super) fn request_log_clear_all(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清理所有",
            GuiLocale::EnUs => "Clear All",
        }
    }

    pub(super) fn request_log_clearing(self, frame: usize) -> &'static str {
        match (self.locale, frame % 3) {
            (GuiLocale::ZhCn, 0) => "正在清理.",
            (GuiLocale::ZhCn, 1) => "正在清理..",
            (GuiLocale::ZhCn, _) => "正在清理...",
            (GuiLocale::EnUs, 0) => "Cleaning.",
            (GuiLocale::EnUs, 1) => "Cleaning..",
            (GuiLocale::EnUs, _) => "Cleaning...",
        }
    }

    pub(super) fn request_log_clear_old_confirm_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清理请求日志",
            GuiLocale::EnUs => "Clear Request Logs",
        }
    }

    pub(super) fn request_log_clear_old_confirm_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "确定删除3天之前的请求日志吗？此操作不可撤销。",
            GuiLocale::EnUs => {
                "Delete request logs older than 3 days? This action cannot be undone."
            }
        }
    }

    pub(super) fn request_log_clear_all_confirm_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清理请求日志",
            GuiLocale::EnUs => "Clear Request Logs",
        }
    }

    pub(super) fn request_log_clear_all_confirm_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "确定删除所有请求日志吗？此操作不可撤销。",
            GuiLocale::EnUs => "Delete all request logs? This action cannot be undone.",
        }
    }

    pub(super) fn request_log_clear_done(self, deleted: usize) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("清理完成，已删除 {deleted} 条请求日志。"),
            GuiLocale::EnUs => format!("Cleanup complete. Deleted {deleted} request logs."),
        }
    }

    pub(super) fn request_log_clear_done_status(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清理完成",
            GuiLocale::EnUs => "Cleanup complete",
        }
    }

    pub(super) fn request_log_clear_failed_status(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清理失败，请重试。",
            GuiLocale::EnUs => "Cleanup failed. Please try again.",
        }
    }

    pub(super) fn request_log_clear_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("清理请求日志失败：{err}"),
            GuiLocale::EnUs => format!("Failed to clear request logs: {err}"),
        }
    }

    pub(super) fn request_log_detail_title(self, id: i64) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("请求日志详情 #{id}"),
            GuiLocale::EnUs => format!("Request Log #{id}"),
        }
    }

    pub(super) fn request_log_detail_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("读取详情失败：{err}"),
            GuiLocale::EnUs => format!("Failed to load detail: {err}"),
        }
    }

    pub(super) fn request_log_detail_codex_request(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 请求",
            GuiLocale::EnUs => "Codex Request",
        }
    }

    pub(super) fn request_log_detail_upstream_request(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "上游请求",
            GuiLocale::EnUs => "Upstream Request",
        }
    }

    pub(super) fn request_log_detail_upstream_sse(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "上游 SSE",
            GuiLocale::EnUs => "Upstream SSE",
        }
    }

    pub(super) fn request_log_detail_response(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "响应",
            GuiLocale::EnUs => "Response",
        }
    }

    pub(super) fn request_log_detail_error(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "错误",
            GuiLocale::EnUs => "Error",
        }
    }

    pub(super) fn request_log_detail_empty(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "没有记录",
            GuiLocale::EnUs => "No data",
        }
    }

    pub(super) fn request_log_col_id(self) -> &'static str {
        "ID"
    }

    pub(super) fn request_log_col_model(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "MODEL ID",
            GuiLocale::EnUs => "MODEL ID",
        }
    }

    pub(super) fn request_log_col_stream(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "STREAM",
            GuiLocale::EnUs => "STREAM",
        }
    }

    pub(super) fn request_log_col_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "CHANNEL",
            GuiLocale::EnUs => "CHANNEL",
        }
    }

    pub(super) fn request_log_col_status(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "STATUS",
            GuiLocale::EnUs => "STATUS",
        }
    }

    pub(super) fn request_log_col_tokens(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "TOKENS",
            GuiLocale::EnUs => "TOKENS",
        }
    }

    pub(super) fn request_log_col_request_size(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "REQ SIZE",
            GuiLocale::EnUs => "REQ SIZE",
        }
    }

    pub(super) fn request_log_col_read_cache(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "READ CACHE",
            GuiLocale::EnUs => "READ CACHE",
        }
    }

    pub(super) fn request_log_col_write_cache(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "WRITE CACHE",
            GuiLocale::EnUs => "WRITE CACHE",
        }
    }

    pub(super) fn request_log_col_cost(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "COST",
            GuiLocale::EnUs => "COST",
        }
    }

    pub(super) fn request_log_col_latency(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "LATENCY",
            GuiLocale::EnUs => "LATENCY",
        }
    }

    pub(super) fn request_log_col_ttft(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "TTFT",
            GuiLocale::EnUs => "TTFT",
        }
    }

    pub(super) fn request_log_col_created_at(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "CREATED AT",
            GuiLocale::EnUs => "CREATED AT",
        }
    }

    pub(super) fn im_access_hint(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "多个机器人/agent 可以分别管理多个 Codex 会话；暂不支持多个机器人管理同一个会话。例如飞书 1 管理会话 1、飞书 2 管理会话 2、Telegram 1 管理会话 3；并行数量取决于本机能同时承载多少 Codex 任务。"
            }
            GuiLocale::EnUs => {
                "Multiple bots/agents can manage separate Codex sessions. Multiple bots managing the same session is not supported yet. Parallel capacity depends on how many Codex tasks this machine can run."
            }
        }
    }

    pub(super) fn bot_pool(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "聊天工具机器人池",
            GuiLocale::EnUs => "Bot Pool",
        }
    }

    pub(super) fn bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "机器人",
            GuiLocale::EnUs => "Bot",
        }
    }

    pub(super) fn platform(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "平台",
            GuiLocale::EnUs => "Platform",
        }
    }

    pub(super) fn state(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "状态",
            GuiLocale::EnUs => "State",
        }
    }

    pub(super) fn account(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "账号",
            GuiLocale::EnUs => "Account",
        }
    }

    pub(super) fn access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "接入",
            GuiLocale::EnUs => "Access",
        }
    }

    pub(super) fn delete_selected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除选中",
            GuiLocale::EnUs => "Delete Selected",
        }
    }

    pub(super) fn delete_im_account_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除当前选中的机器人接入配置",
            GuiLocale::EnUs => "Delete the selected bot integration",
        }
    }

    pub(super) fn add_bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "新增机器人",
            GuiLocale::EnUs => "Add Bot",
        }
    }

    pub(super) fn add_feishu_bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "添加飞书机器人",
            GuiLocale::EnUs => "Add Feishu Bot",
        }
    }

    pub(super) fn add_feishu_bot_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "扫码接入一个新的飞书机器人",
            GuiLocale::EnUs => "Scan to connect a new Feishu bot",
        }
    }

    pub(super) fn add_telegram_bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "添加 Telegram 机器人",
            GuiLocale::EnUs => "Add Telegram Bot",
        }
    }

    pub(super) fn add_telegram_bot_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "填写 Telegram Bot Token 并接入",
            GuiLocale::EnUs => "Enter a Telegram Bot Token and connect it",
        }
    }

    pub(super) fn add_wechat_bot(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "添加微信机器人",
            GuiLocale::EnUs => "Add WeChat Bot",
        }
    }

    pub(super) fn add_wechat_bot_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "使用微信扫码接入机器人",
            GuiLocale::EnUs => "Scan with WeChat to connect the bot",
        }
    }

    pub(super) fn wechat_context_token_warning(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "微信官方链路不成熟，长任务客户端可能主动断开；需要在手机端发送 ! 或者 ? 激活。长任务推荐使用飞书。"
            }
            GuiLocale::EnUs => {
                "WeChat's official link is unstable for long tasks; send ! or ? from the phone to reactivate it. Feishu is recommended for long tasks."
            }
        }
    }

    pub(super) fn new_provider_prompt(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "填写新 provider 名称、Base URL 和 API Key，然后点击启用。",
            GuiLocale::EnUs => "Enter a provider name, Base URL, and API key, then click Enable.",
        }
    }

    pub(super) fn saving_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在保存 provider，请稍候...",
            GuiLocale::EnUs => "Saving provider...",
        }
    }

    pub(super) fn deleting_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在删除 provider，请稍候...",
            GuiLocale::EnUs => "Deleting provider...",
        }
    }

    pub(super) fn enabling_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在启用，请稍候...",
            GuiLocale::EnUs => "Enabling provider...",
        }
    }

    pub(super) fn save_in_progress(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存中...",
            GuiLocale::EnUs => "Saving...",
        }
    }

    pub(super) fn delete_in_progress(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除中...",
            GuiLocale::EnUs => "Deleting...",
        }
    }

    pub(super) fn enable_in_progress(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启用中...",
            GuiLocale::EnUs => "Enabling...",
        }
    }

    pub(super) fn add_in_progress(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "添加中...",
            GuiLocale::EnUs => "Adding...",
        }
    }

    pub(super) fn starting(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启动中",
            GuiLocale::EnUs => "Starting",
        }
    }

    pub(super) fn waiting_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "等待服务",
            GuiLocale::EnUs => "Waiting",
        }
    }

    pub(super) fn service_reads_status(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "服务启动后读取状态",
            GuiLocale::EnUs => "Status loads after service startup.",
        }
    }

    pub(super) fn startup_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启动失败",
            GuiLocale::EnUs => "Startup Failed",
        }
    }

    pub(super) fn not_running(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未运行",
            GuiLocale::EnUs => "Not Running",
        }
    }

    pub(super) fn local_service_offline_detail(self, mode: LocalConnectionMode) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!(
                "{} · GUI 会自动启动本地服务；如果一直未运行，请重启 CodexHub。",
                self.local_connection_label(mode)
            ),
            GuiLocale::EnUs => format!(
                "{} · The GUI starts the local service automatically. Restart CodexHub if it stays offline.",
                self.local_connection_label(mode)
            ),
        }
    }

    pub(super) fn local_service_not_running(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地服务未运行",
            GuiLocale::EnUs => "Local service is not running.",
        }
    }

    pub(super) fn running(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "运行中",
            GuiLocale::EnUs => "Running",
        }
    }

    pub(super) fn local_service_detail(self, bind: &str, mode: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{mode} · 监听 {bind}"),
            GuiLocale::EnUs => format!("{mode} · {bind}"),
        }
    }

    pub(super) fn connected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已连接",
            GuiLocale::EnUs => "Connected",
        }
    }

    pub(super) fn uninitialized_config(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未初始化配置",
            GuiLocale::EnUs => "Not Initialized",
        }
    }

    pub(super) fn provider_waiting_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "等待本地服务",
            GuiLocale::EnUs => "Waiting for local service",
        }
    }

    pub(super) fn provider_read_after_start(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "启动后读取 ~/.codex/config.toml",
            GuiLocale::EnUs => "Reads ~/.codex/config.toml after startup",
        }
    }

    pub(super) fn not_configured(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未配置",
            GuiLocale::EnUs => "Not configured",
        }
    }

    pub(super) fn provider_create_on_write(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未配置，写入时新建",
            GuiLocale::EnUs => "Not configured; created when written",
        }
    }

    pub(super) fn in_use(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "使用中",
            GuiLocale::EnUs => "Active",
        }
    }

    pub(super) fn key_configured(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已配置",
            GuiLocale::EnUs => "Configured",
        }
    }

    pub(super) fn provider_catalog_after_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地服务运行后会读取 ~/.codex/config.toml 里的 provider。",
            GuiLocale::EnUs => {
                "Providers are read from ~/.codex/config.toml after the local service starts."
            }
        }
    }

    pub(super) fn no_provider(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "还没有 provider，填写后点击启用。",
            GuiLocale::EnUs => "No providers yet. Fill the form and click Enable.",
        }
    }

    pub(super) fn current_provider(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("当前 provider: {name}"),
            GuiLocale::EnUs => format!("Current provider: {name}"),
        }
    }

    pub(super) fn saved_providers(self, count: usize) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("已保存 {count} 个 provider，请选择一个使用。"),
            GuiLocale::EnUs => format!("{count} providers saved. Select one to use."),
        }
    }

    pub(super) fn im_waiting_service_row(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地服务启动后读取",
            GuiLocale::EnUs => "Loads after local service starts",
        }
    }

    pub(super) fn reading(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "读取中",
            GuiLocale::EnUs => "Loading",
        }
    }

    pub(super) fn reading_bot_list(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在读取机器人列表",
            GuiLocale::EnUs => "Loading bot list",
        }
    }

    pub(super) fn not_connected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未连接",
            GuiLocale::EnUs => "Not Connected",
        }
    }

    pub(super) fn scan_or_token(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请扫码或填写 Bot Token",
            GuiLocale::EnUs => "Scan or enter a Bot Token",
        }
    }

    pub(super) fn paused(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已暂停",
            GuiLocale::EnUs => "Paused",
        }
    }

    pub(super) fn im_connected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已接入",
            GuiLocale::EnUs => "Connected",
        }
    }

    pub(super) fn error(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "异常",
            GuiLocale::EnUs => "Error",
        }
    }

    pub(super) fn connecting(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "连接中",
            GuiLocale::EnUs => "Connecting",
        }
    }

    pub(super) fn waiting_connection(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "等待连接",
            GuiLocale::EnUs => "Waiting",
        }
    }

    pub(super) fn bot_saved(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "机器人已保存",
            GuiLocale::EnUs => "Bot saved",
        }
    }

    pub(super) fn name_saved(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{name} 已保存"),
            GuiLocale::EnUs => format!("{name} saved"),
        }
    }

    pub(super) fn waiting_bot_connection(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "等待机器人连接",
            GuiLocale::EnUs => "Waiting for bot connection",
        }
    }

    pub(super) fn im_empty_detail(self, platform: &str) -> String {
        match (self.locale, platform) {
            (GuiLocale::ZhCn, "feishu") => "扫码添加飞书机器人".to_string(),
            (GuiLocale::ZhCn, "telegram") => "添加 Telegram Bot Token".to_string(),
            (GuiLocale::ZhCn, "wechat") => "扫码添加微信机器人".to_string(),
            (GuiLocale::ZhCn, _) => "添加机器人".to_string(),
            (GuiLocale::EnUs, "feishu") => "Scan to add a Feishu bot".to_string(),
            (GuiLocale::EnUs, "telegram") => "Add a Telegram Bot Token".to_string(),
            (GuiLocale::EnUs, "wechat") => "Scan to add a WeChat bot".to_string(),
            (GuiLocale::EnUs, _) => "Add a bot".to_string(),
        }
    }

    pub(super) fn bot_fallback(self, platform: &str) -> &'static str {
        match (self.locale, platform) {
            (GuiLocale::ZhCn, "feishu") => "飞书机器人",
            (GuiLocale::ZhCn, "telegram") => "Telegram 机器人",
            (GuiLocale::ZhCn, "wechat") => "微信机器人",
            (GuiLocale::ZhCn, _) => "机器人",
            (GuiLocale::EnUs, "feishu") => "Feishu bot",
            (GuiLocale::EnUs, "telegram") => "Telegram bot",
            (GuiLocale::EnUs, "wechat") => "WeChat bot",
            (GuiLocale::EnUs, _) => "Bot",
        }
    }

    pub(super) fn bot_connecting(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{name} 正在连接"),
            GuiLocale::EnUs => format!("{name} connecting"),
        }
    }

    pub(super) fn bot_waiting(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{name} 等待连接"),
            GuiLocale::EnUs => format!("{name} waiting"),
        }
    }

    pub(super) fn bot_error(self, name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{name} 异常"),
            GuiLocale::EnUs => format!("{name} error"),
        }
    }

    pub(super) fn feishu_label(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "飞书",
            GuiLocale::EnUs => "Feishu",
        }
    }

    pub(super) fn wechat_label(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "微信",
            GuiLocale::EnUs => "WeChat",
        }
    }

    pub(super) fn close(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "关闭",
            GuiLocale::EnUs => "Close",
        }
    }

    pub(super) fn cancel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "取消",
            GuiLocale::EnUs => "Cancel",
        }
    }

    pub(super) fn save_and_connect(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存并接入",
            GuiLocale::EnUs => "Save and Connect",
        }
    }

    pub(super) fn select_provider_to_delete(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请先选择或填写要删除的 provider。",
            GuiLocale::EnUs => "Select or enter a provider to delete first.",
        }
    }

    pub(super) fn codex_app_config_uninstalled(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 原有配置已恢复。请重启 Codex App 生效。",
            GuiLocale::EnUs => "Codex configuration was restored. Restart Codex App to apply it.",
        }
    }

    pub(super) fn codex_app_config_injected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 本地配置已写入。请重启 Codex App 生效。",
            GuiLocale::EnUs => {
                "Local Codex settings were written. Restart Codex App to apply them."
            }
        }
    }

    pub(super) fn select_bot_first(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请先选择一个机器人。",
            GuiLocale::EnUs => "Select a bot first.",
        }
    }

    pub(super) fn service_starting_wait(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地服务正在启动，请稍后再试。",
            GuiLocale::EnUs => "The local service is starting. Try again shortly.",
        }
    }

    pub(super) fn service_not_ready_retry(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "本地服务还没有启动完成，请稍后再试。如果一直未运行，请重启 CodexHub。"
            }
            GuiLocale::EnUs => {
                "The local service is not ready yet. Try again shortly. Restart CodexHub if it stays offline."
            }
        }
    }

    pub(super) fn about_description(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地 remote-control backend + 聊天工具桥接。",
            GuiLocale::EnUs => "Local remote-control backend with chat integration bridges.",
        }
    }

    pub(super) fn diagnostics_export_save_dialog_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存连接诊断包",
            GuiLocale::EnUs => "Save Connection Diagnostics",
        }
    }

    pub(super) fn diagnostics_export_zip_wildcard(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "ZIP 压缩包 (*.zip)|*.zip",
            GuiLocale::EnUs => "ZIP archives (*.zip)|*.zip",
        }
    }

    pub(super) fn diagnostics_export_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("导出连接诊断包失败：{err}"),
            GuiLocale::EnUs => format!("Failed to export connection diagnostics: {err}"),
        }
    }

    pub(super) fn update_dialog_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "CodexHub 更新",
            GuiLocale::EnUs => "CodexHub Update",
        }
    }

    pub(super) fn checking_updates_busy(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在检查更新，请稍候。",
            GuiLocale::EnUs => "Checking for updates. Please wait.",
        }
    }

    pub(super) fn update_client_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("创建更新检查客户端失败：{err}"),
            GuiLocale::EnUs => format!("Failed to create the update check client: {err}"),
        }
    }

    pub(super) fn update_sources_failed(
        self,
        api_err: &str,
        platform_manifest_err: &str,
        legacy_manifest_err: &str,
    ) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!(
                    "无法读取 GitHub Release 更新信息：{api_err}\n平台更新清单检查结果：{platform_manifest_err}\n旧版 latest.json 检查结果：{legacy_manifest_err}"
                )
            }
            GuiLocale::EnUs => {
                format!(
                    "Failed to read GitHub Release update info: {api_err}\nPlatform manifest result: {platform_manifest_err}\nLegacy latest.json result: {legacy_manifest_err}"
                )
            }
        }
    }

    pub(super) fn update_manifest_parse_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("latest.json 无法解析：{err}"),
            GuiLocale::EnUs => format!("Failed to parse latest.json: {err}"),
        }
    }

    pub(super) fn github_release_parse_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("GitHub Release API 无法解析：{err}"),
            GuiLocale::EnUs => format!("Failed to parse GitHub Release API response: {err}"),
        }
    }

    pub(super) fn url_request_timeout(self, url: &str, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{url} 请求超时：{err}"),
            GuiLocale::EnUs => format!("{url} timed out: {err}"),
        }
    }

    pub(super) fn url_request_failed(self, url: &str, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{url} 请求失败：{err}"),
            GuiLocale::EnUs => format!("{url} request failed: {err}"),
        }
    }

    pub(super) fn url_http_failed(self, url: &str, status: &str, body: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{url} 返回 HTTP {status}: {body}"),
            GuiLocale::EnUs => format!("{url} returned HTTP {status}: {body}"),
        }
    }

    pub(super) fn release_missing_version(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "GitHub Release 没有版本号。",
            GuiLocale::EnUs => "The GitHub Release has no version.",
        }
    }

    pub(super) fn already_latest_version(self, current: &str, latest: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!("已是最新版本。\n当前版本：{current}\nGitHub 最新版本：{latest}")
            }
            GuiLocale::EnUs => format!(
                "Already up to date.\nCurrent version: {current}\nLatest GitHub version: {latest}"
            ),
        }
    }

    pub(super) fn new_version_message(
        self,
        current: &str,
        latest: &str,
        notes: &str,
        can_download_installer: bool,
    ) -> String {
        let action = if can_download_installer {
            match self.locale {
                GuiLocale::ZhCn => "是否下载更新包并启动安装器？",
                GuiLocale::EnUs => "Download and start the installer?",
            }
        } else {
            match self.locale {
                GuiLocale::ZhCn => "是否打开发布页手动下载并安装？",
                GuiLocale::EnUs => "Open the release page to download and install manually?",
            }
        };
        match self.locale {
            GuiLocale::ZhCn => {
                format!(
                    "发现新版本。\n当前版本：{current}\n最新版本：{latest}\n\n{notes}\n\n{action}"
                )
            }
            GuiLocale::EnUs => {
                format!(
                    "A new version is available.\nCurrent version: {current}\nLatest version: {latest}\n\n{notes}\n\n{action}"
                )
            }
        }
    }

    pub(super) fn update_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("检查更新失败：{err}"),
            GuiLocale::EnUs => format!("Update check failed: {err}"),
        }
    }

    pub(super) fn release_notes_default(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Release 页面包含安装包和更新说明。",
            GuiLocale::EnUs => "The Release page includes installers and release notes.",
        }
    }

    pub(super) fn release_notes(self, notes: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("更新说明：\n{notes}"),
            GuiLocale::EnUs => format!("Release notes:\n{notes}"),
        }
    }

    pub(super) fn version_not_comparable(self, version: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("版本号 {version} 无法比较。"),
            GuiLocale::EnUs => format!("Version {version} cannot be compared."),
        }
    }

    pub(super) fn empty_download_url(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "下载地址为空。",
            GuiLocale::EnUs => "The download URL is empty.",
        }
    }

    pub(super) fn update_download_started(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在下载更新包，下载完成后会打开安装包。",
            GuiLocale::EnUs => {
                "Downloading the update. The installer package will open when the download finishes."
            }
        }
    }

    pub(super) fn update_download_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "下载更新",
            GuiLocale::EnUs => "Downloading Update",
        }
    }

    pub(super) fn update_download_preparing(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "正在连接下载服务器…",
            GuiLocale::EnUs => "Connecting to the download server...",
        }
    }

    pub(super) fn update_download_progress(self, downloaded: &str, total: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("正在下载更新包… {downloaded} / {total}"),
            GuiLocale::EnUs => format!("Downloading the update... {downloaded} / {total}"),
        }
    }

    pub(super) fn update_download_progress_unknown(self, downloaded: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("正在下载更新包… 已下载 {downloaded}"),
            GuiLocale::EnUs => format!("Downloading the update... {downloaded} downloaded"),
        }
    }

    pub(super) fn update_download_verifying(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "下载完成，正在校验更新包…",
            GuiLocale::EnUs => "Download complete. Verifying the update...",
        }
    }

    pub(super) fn update_download_cancelled(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已取消更新下载。",
            GuiLocale::EnUs => "The update download was cancelled.",
        }
    }

    pub(super) fn update_download_failed(self, url: &str, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("下载更新包失败：{err}\n地址：{url}"),
            GuiLocale::EnUs => format!("Failed to download the update: {err}\nURL: {url}"),
        }
    }

    pub(super) fn update_download_http_failed(self, url: &str, status: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("下载更新包失败：HTTP {status}\n地址：{url}"),
            GuiLocale::EnUs => format!("Failed to download the update: HTTP {status}\nURL: {url}"),
        }
    }

    pub(super) fn update_checksum_mismatch(self, expected: &str, actual: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!("更新包校验失败。\n期望 SHA256：{expected}\n实际 SHA256：{actual}")
            }
            GuiLocale::EnUs => format!(
                "Update checksum verification failed.\nExpected SHA256: {expected}\nActual SHA256: {actual}"
            ),
        }
    }

    #[cfg(target_os = "windows")]
    pub(super) fn update_installer_started(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "更新包已下载。CodexHub 将退出以继续安装。",
            GuiLocale::EnUs => {
                "The update was downloaded. CodexHub will exit to continue installation."
            }
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn update_installer_started(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "更新包已下载并打开。请将 CodexHub 拖到 Applications 覆盖安装，然后重新打开。"
            }
            GuiLocale::EnUs => {
                "The update was downloaded and opened. Drag CodexHub to Applications to replace the old app, then reopen it."
            }
        }
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    pub(super) fn update_installer_started(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "更新包已下载并打开，请按系统提示完成安装。",
            GuiLocale::EnUs => {
                "The update was downloaded and opened. Follow the system prompts to finish installation."
            }
        }
    }

    pub(super) fn update_installer_launch_failed(self, url: &str, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("启动更新安装器失败：{err}\n下载地址：{url}"),
            GuiLocale::EnUs => {
                format!("Failed to start the update installer: {err}\nDownload URL: {url}")
            }
        }
    }

    pub(super) fn open_browser_failed(self, err: &str, url: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("无法打开浏览器：{err}\n下载地址：{url}"),
            GuiLocale::EnUs => format!("Failed to open the browser: {err}\nDownload URL: {url}"),
        }
    }

    pub(super) fn confirm_uninstall_codex_app_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "移除后，Codex 会恢复默认连接。本工具会保留 Codex 后续产生的其它设置，并尽量恢复原来的登录状态。确认继续？"
            }
            GuiLocale::EnUs => {
                "After removal, Codex will use its default connection again. Other settings added later are kept, and the previous sign-in state is restored when available. Continue?"
            }
        }
    }

    pub(super) fn confirm_uninstall_codex_app_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "恢复 Codex 原有配置",
            GuiLocale::EnUs => "Restore Codex Config",
        }
    }

    pub(super) fn confirm_delete_provider_message(self, provider_name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!(
                    "删除 provider `{provider_name}`？如果它正在使用中，也会取消当前 provider 设置。"
                )
            }
            GuiLocale::EnUs => {
                format!(
                    "Delete provider `{provider_name}`? If it is active, the current provider setting will also be cleared."
                )
            }
        }
    }

    pub(super) fn confirm_delete_provider_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除 Provider",
            GuiLocale::EnUs => "Delete Provider",
        }
    }

    pub(super) fn confirm_delete_im_account_message(self, account_name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("删除机器人 `{account_name}`？相关会话绑定也会一起清理。"),
            GuiLocale::EnUs => {
                format!(
                    "Delete bot `{account_name}`? Related conversation bindings will also be cleared."
                )
            }
        }
    }

    pub(super) fn confirm_delete_im_account_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除机器人接入",
            GuiLocale::EnUs => "Delete Bot Integration",
        }
    }

    pub(super) fn telegram_saved(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Telegram 已保存并接入。",
            GuiLocale::EnUs => "Telegram has been saved and connected.",
        }
    }

    pub(super) fn im_account_deleted(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "机器人接入已删除。",
            GuiLocale::EnUs => "Bot integration deleted.",
        }
    }

    pub(super) fn provider_verify_selected_failed(self, active: &str, expected: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!(
                "配置接口已返回成功，但当前 provider 仍是 {active}，期望是 {expected}。请刷新后再试一次。"
            ),
            GuiLocale::EnUs => format!(
                "The configure API returned success, but the current provider is still {active}; expected {expected}. Refresh and try again."
            ),
        }
    }

    pub(super) fn provider_name_empty(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Provider 名称不能为空。",
            GuiLocale::EnUs => "Provider name cannot be empty.",
        }
    }

    pub(super) fn provider_verify_saved_failed(self, provider_name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!(
                "保存接口已返回成功，但 provider {provider_name} 没有出现在配置列表里。请刷新后再试一次。"
            ),
            GuiLocale::EnUs => format!(
                "The save API returned success, but provider {provider_name} is not in the configured provider list. Refresh and try again."
            ),
        }
    }

    pub(super) fn provider_verify_websocket_failed(
        self,
        provider_name: &str,
        actual: &str,
        expected: bool,
    ) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!(
                "WebSocket 写入接口已返回成功，但 provider {provider_name} 的 supports_websockets 仍是 {actual}，期望是 {expected}。请刷新后再试一次。"
            ),
            GuiLocale::EnUs => format!(
                "The WebSocket API returned success, but provider {provider_name} still has supports_websockets={actual}; expected {expected}. Refresh and try again."
            ),
        }
    }

    pub(super) fn not_found(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "<未找到>",
            GuiLocale::EnUs => "<not found>",
        }
    }

    pub(super) fn unset(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "<未设置>",
            GuiLocale::EnUs => "<unset>",
        }
    }

    pub(super) fn provider_verify_deleted_failed(self, provider_name: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!(
                "删除接口已返回成功，但 provider {provider_name} 仍在配置列表里。请刷新后再试一次。"
            ),
            GuiLocale::EnUs => format!(
                "The delete API returned success, but provider {provider_name} is still in the configured provider list. Refresh and try again."
            ),
        }
    }

    pub(super) fn provider_saved_info(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Provider 已保存。需要使用它时再点击启用。",
            GuiLocale::EnUs => "Provider saved. Click Enable when you want to use it.",
        }
    }

    pub(super) fn provider_deleted_info(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Provider 已删除。",
            GuiLocale::EnUs => "Provider deleted.",
        }
    }

    pub(super) fn provider_enabled_info(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "已启用。请重启 Codex App，然后在 App 里打开 remote-control；VS Code 插件也可以接入。"
            }
            GuiLocale::EnUs => {
                "Enabled. Restart Codex App, then open remote-control in the App. The VS Code extension can also connect."
            }
        }
    }

    pub(super) fn telegram_dialog_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "添加 Telegram 机器人",
            GuiLocale::EnUs => "Add Telegram Bot",
        }
    }

    pub(super) fn telegram_token_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "填写 BotFather 提供的 Bot Token",
            GuiLocale::EnUs => "Enter the Bot Token from BotFather",
        }
    }

    pub(super) fn telegram_private_hint(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "仅支持与机器人私聊；群聊暂不接入。",
            GuiLocale::EnUs => {
                "Only private bot chats are supported. Group chats are not connected yet."
            }
        }
    }

    pub(super) fn telegram_token_required(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请输入 Telegram Bot Token。",
            GuiLocale::EnUs => "Enter a Telegram Bot Token.",
        }
    }

    pub(super) fn feishu_onboard_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "扫码使用新机器人",
            GuiLocale::EnUs => "Scan to Use a New Bot",
        }
    }

    pub(super) fn scan_feishu(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请使用飞书扫码",
            GuiLocale::EnUs => "Scan with Feishu",
        }
    }

    pub(super) fn qr_open_browser_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "二维码生成失败，请使用浏览器打开链接。",
            GuiLocale::EnUs => "QR code generation failed. Open the link in a browser.",
        }
    }

    pub(super) fn feishu_fallback_link(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "扫码失败？打开飞书确认链接",
            GuiLocale::EnUs => "Scan failed? Open the Feishu confirmation link",
        }
    }

    pub(super) fn scan_done_auto_close(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "扫码完成后会自动关闭。",
            GuiLocale::EnUs => "This will close automatically after scanning.",
        }
    }

    pub(super) fn onboard_failed_retry(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "接入失败，请关闭后重试。",
            GuiLocale::EnUs => "Connection failed. Close this dialog and try again.",
        }
    }

    pub(super) fn wechat_onboard_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "扫码连接微信",
            GuiLocale::EnUs => "Scan to Connect WeChat",
        }
    }

    pub(super) fn scan_wechat(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请使用微信扫码",
            GuiLocale::EnUs => "Scan with WeChat",
        }
    }

    pub(super) fn qr_retry_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "二维码生成失败，请关闭后重试。",
            GuiLocale::EnUs => "QR code generation failed. Close this dialog and try again.",
        }
    }

    pub(super) fn verify_code(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "验证码",
            GuiLocale::EnUs => "Verification Code",
        }
    }

    pub(super) fn wechat_expire_notice(self, seconds: u64) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("扫码完成后会自动关闭。二维码约 {seconds} 秒后过期。"),
            GuiLocale::EnUs => {
                format!(
                    "This will close automatically after scanning. The QR code expires in about {seconds} seconds."
                )
            }
        }
    }

    pub(super) fn wechat_need_verify(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "微信需要验证码，请输入后等待自动确认。",
            GuiLocale::EnUs => {
                "WeChat requires a verification code. Enter it and wait for confirmation."
            }
        }
    }

    pub(super) fn wechat_qr_expired(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "二维码已过期，请关闭后重新扫码。",
            GuiLocale::EnUs => "The QR code expired. Close this dialog and scan again.",
        }
    }

    pub(super) fn wechat_verify_blocked(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "验证码被限制，请稍后重试。",
            GuiLocale::EnUs => "Verification codes are temporarily restricted. Try again later.",
        }
    }

    pub(super) fn onboard_pending_error(self, error: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("接入暂未完成：{error}"),
            GuiLocale::EnUs => format!("Connection is not complete yet: {error}"),
        }
    }

    pub(super) fn wechat_wait(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "等待微信扫码。",
            GuiLocale::EnUs => "Waiting for WeChat scan.",
        }
    }

    pub(super) fn wechat_scanned(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已扫码，请在微信里确认。",
            GuiLocale::EnUs => "Scanned. Confirm in WeChat.",
        }
    }

    pub(super) fn wechat_redirect(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已扫码，正在切换微信登录入口。",
            GuiLocale::EnUs => "Scanned. Switching WeChat login entry.",
        }
    }

    pub(super) fn wechat_confirmed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已确认，正在保存配置。",
            GuiLocale::EnUs => "Confirmed. Saving settings.",
        }
    }

    pub(super) fn wechat_bound(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "该微信已完成绑定。",
            GuiLocale::EnUs => "This WeChat account is already connected.",
        }
    }

    pub(super) fn current_status(self, status: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("当前状态：{status}"),
            GuiLocale::EnUs => format!("Current status: {status}"),
        }
    }

    pub(super) fn api_response_parse_failed(self, path: &str, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("{path} 返回数据无法解析：{err}"),
            GuiLocale::EnUs => format!("Failed to parse the response from {path}: {err}"),
        }
    }

    pub(super) fn api_timeout(self, base_url: &str, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("本地服务 {base_url} 响应超时：{err}"),
            GuiLocale::EnUs => format!("Local service {base_url} timed out: {err}"),
        }
    }

    pub(super) fn api_connect_failed(self, base_url: &str, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("无法连接本地服务 {base_url}：{err}"),
            GuiLocale::EnUs => format!("Failed to connect to local service {base_url}: {err}"),
        }
    }

    pub(super) fn api_request_failed(self, base_url: &str, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("本地服务 {base_url} 请求失败：{err}"),
            GuiLocale::EnUs => format!("Local service {base_url} request failed: {err}"),
        }
    }

    pub(super) fn daemon_exited(self, status: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("本地服务启动后退出：{status}"),
            GuiLocale::EnUs => format!("The local service exited after startup: {status}"),
        }
    }

    pub(super) fn daemon_start_timeout(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "本地服务已启动，但 10 秒内没有响应。请检查 logs/codexhub-chain.log。"
            }
            GuiLocale::EnUs => {
                "The local service started, but did not respond within 10 seconds. Check logs/codexhub-chain.log."
            }
        }
    }

    pub(super) fn daemon_watchdog_timeout(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "本地服务启动超过 30 秒仍未完成。请检查旧进程占用或 logs/codexhub-chain.log。"
            }
            GuiLocale::EnUs => {
                "The local service has not finished starting after 30 seconds. Check for an old process or logs/codexhub-chain.log."
            }
        }
    }

    pub(super) fn daemon_spawn_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("无法启动本地服务：{err}"),
            GuiLocale::EnUs => format!("Failed to start the local service: {err}"),
        }
    }

    pub(super) fn daemon_port_unknown(self, base_url: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("无法识别本地服务地址的端口：{base_url}"),
            GuiLocale::EnUs => format!("Could not determine the local service port: {base_url}"),
        }
    }

    pub(super) fn daemon_port_conflict(self, port: u16, owner: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!("端口 {port} 被非 CodexHub 进程占用，已停止自动启动以避免误杀：{owner}")
            }
            GuiLocale::EnUs => format!(
                "Port {port} is owned by a non-CodexHub process. Automatic startup was stopped to avoid terminating it: {owner}"
            ),
        }
    }

    pub(super) fn daemon_stop_failed(self, port: u16, pids: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!("无法停止占用端口 {port} 的旧 CodexHub 进程（PID：{pids}）")
            }
            GuiLocale::EnUs => {
                format!("Failed to stop the old CodexHub process on port {port} (PID: {pids})")
            }
        }
    }

    pub(super) fn daemon_current_exe_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("无法定位当前程序：{err}"),
            GuiLocale::EnUs => format!("Failed to locate the current executable: {err}"),
        }
    }

    // --- AI Gateway Tab ---

    pub(super) fn ai_gateway_tab(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "大模型接入",
            GuiLocale::EnUs => "AI Gateway",
        }
    }

    pub(super) fn ai_gateway_management(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "大模型厂商管理",
            GuiLocale::EnUs => "AI Gateway Management",
        }
    }

    pub(super) fn ai_gw_channel_list(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "大模型厂商列表",
            GuiLocale::EnUs => "Channels",
        }
    }

    pub(super) fn ai_gw_channel_editor(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "新增 / 编辑大模型厂商",
            GuiLocale::EnUs => "Add / Edit Channel",
        }
    }

    pub(super) fn ai_gw_channel_editor_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "选择 OpenAI、Grok、DeepSeek、Anthropic 或智谱 GLM，并填写大模型厂商接入信息。"
            }
            GuiLocale::EnUs => {
                "Choose OpenAI, Grok, DeepSeek, Anthropic, or GLM and fill in the channel connection details."
            }
        }
    }

    pub(super) fn ai_gw_channel_settings(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "大模型厂商配置",
            GuiLocale::EnUs => "Channel Settings",
        }
    }

    pub(super) fn ai_gw_add_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "新增大模型厂商",
            GuiLocale::EnUs => "Add Channel",
        }
    }

    pub(super) fn ai_gw_edit_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "修改大模型厂商",
            GuiLocale::EnUs => "Edit Channel",
        }
    }

    pub(super) fn ai_gw_delete_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除大模型厂商",
            GuiLocale::EnUs => "Delete Channel",
        }
    }

    pub(super) fn ai_gw_save_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存大模型厂商",
            GuiLocale::EnUs => "Save Channel",
        }
    }

    pub(super) fn ai_gw_create_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "创建大模型厂商",
            GuiLocale::EnUs => "Create Channel",
        }
    }

    pub(super) fn ai_gw_col_name(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "名称",
            GuiLocale::EnUs => "Name",
        }
    }

    pub(super) fn ai_gw_col_type(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "类型",
            GuiLocale::EnUs => "Type",
        }
    }

    pub(super) fn ai_gw_provider_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "服务商",
            GuiLocale::EnUs => "Provider",
        }
    }

    pub(super) fn ai_gw_service_openai(self) -> &'static str {
        "OpenAI"
    }

    pub(super) fn ai_gw_service_grok(self) -> &'static str {
        "Grok"
    }

    pub(super) fn ai_gw_service_deepseek(self) -> &'static str {
        "DeepSeek"
    }

    pub(super) fn ai_gw_service_anthropic(self) -> &'static str {
        "Anthropic"
    }

    pub(super) fn ai_gw_service_glm(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "智谱 GLM",
            GuiLocale::EnUs => "GLM",
        }
    }

    pub(super) fn ai_gw_col_base_url(self) -> &'static str {
        "Base URL"
    }

    pub(super) fn ai_gw_models_url(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "模型列表接口（可选）",
            GuiLocale::EnUs => "Models URL (optional)",
        }
    }

    pub(super) fn ai_gw_models_url_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "留空会根据 Base URL 自动尝试常见模型列表接口；如果获取失败，再填写完整的 /models 地址。"
            }
            GuiLocale::EnUs => {
                "Leave blank to try common model-list endpoints from Base URL. Fill the full /models URL only if fetching fails."
            }
        }
    }

    pub(super) fn ai_gw_col_api_key(self) -> &'static str {
        "API Key"
    }

    pub(super) fn ai_gw_weight(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "权重",
            GuiLocale::EnUs => "Weight",
        }
    }

    pub(super) fn ai_gw_models(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "模型列表",
            GuiLocale::EnUs => "Models",
        }
    }

    pub(super) fn ai_gw_upstream_model(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "上游模型",
            GuiLocale::EnUs => "Upstream Model",
        }
    }

    pub(super) fn ai_gw_codex_model_aliases(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex 映射模型",
            GuiLocale::EnUs => "Codex Model Aliases",
        }
    }

    pub(super) fn ai_gw_fetch_models(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "获取远端模型列表",
            GuiLocale::EnUs => "Fetch Remote List",
        }
    }

    pub(super) fn ai_gw_fetching_models(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "获取中...",
            GuiLocale::EnUs => "Fetching...",
        }
    }

    pub(super) fn ai_gw_add_model(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "手工添加",
            GuiLocale::EnUs => "Add Manually",
        }
    }

    pub(super) fn ai_gw_delete_model(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除选中",
            GuiLocale::EnUs => "Delete Selected",
        }
    }

    pub(super) fn ai_gw_edit_model_aliases(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "编辑模型映射",
            GuiLocale::EnUs => "Edit Model Aliases",
        }
    }

    pub(super) fn ai_gw_model_id_prompt(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请输入模型 ID。多个模型可以用逗号或换行分隔。",
            GuiLocale::EnUs => "Enter model ID. Use commas or new lines for multiple models.",
        }
    }

    pub(super) fn ai_gw_model_id_empty(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请输入模型 ID。",
            GuiLocale::EnUs => "Please enter a model ID.",
        }
    }

    pub(super) fn ai_gw_model_alias_prompt(self, upstream_model: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!(
                    "上游模型：{upstream_model}\n请输入 Codex 侧模型 ID，多个用逗号或换行分隔。留空会清除映射。"
                )
            }
            GuiLocale::EnUs => {
                format!(
                    "Upstream model: {upstream_model}\nEnter Codex-side model IDs. Use commas or new lines for multiple aliases. Leave empty to clear aliases."
                )
            }
        }
    }

    pub(super) fn ai_gw_select_model(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请选择一个模型。",
            GuiLocale::EnUs => "Please select a model.",
        }
    }

    pub(super) fn ai_gw_base_url_empty(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请输入 Base URL。",
            GuiLocale::EnUs => "Please enter Base URL.",
        }
    }

    pub(super) fn ai_gw_models_empty(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "远端没有返回可用模型。",
            GuiLocale::EnUs => "The remote endpoint returned no models.",
        }
    }

    pub(super) fn ai_gw_models_fetched(self, count: usize) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("已获取 {count} 个模型。"),
            GuiLocale::EnUs => format!("Fetched {count} models."),
        }
    }

    pub(super) fn ai_gw_models_fetch_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!(
                    "获取模型列表失败：{err}\n如果自动查找失败，请手动填写完整的模型列表接口，例如 https://api.example.com/v1/models。"
                )
            }
            GuiLocale::EnUs => {
                format!(
                    "Failed to fetch model list: {err}\nIf automatic discovery fails, enter the full models URL, for example https://api.example.com/v1/models."
                )
            }
        }
    }

    pub(super) fn ai_gw_api_format(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "API 格式",
            GuiLocale::EnUs => "API Format",
        }
    }

    pub(super) fn ai_gw_api_protocol(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "API 协议",
            GuiLocale::EnUs => "API Protocol",
        }
    }

    pub(super) fn ai_gw_provider_type(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "类型",
            GuiLocale::EnUs => "Type",
        }
    }

    pub(super) fn provider_type_openai_responses(self) -> &'static str {
        "OpenAI Responses"
    }

    pub(super) fn provider_type_grok_responses(self) -> &'static str {
        "Grok Responses"
    }

    pub(super) fn provider_type_chat_completions(self) -> &'static str {
        "Chat Completions"
    }

    pub(super) fn provider_type_anthropic_messages(self) -> &'static str {
        "Anthropic Messages"
    }

    pub(super) fn provider_type_glm_anthropic_messages(self) -> &'static str {
        "GLM Anthropic Messages"
    }

    pub(super) fn ai_gw_saved(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "大模型厂商已保存。",
            GuiLocale::EnUs => "AI Gateway channel saved.",
        }
    }

    pub(super) fn ai_gw_deleted(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "大模型厂商已删除。",
            GuiLocale::EnUs => "AI Gateway channel deleted.",
        }
    }

    pub(super) fn ai_gw_save_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("保存大模型厂商配置失败：{err}"),
            GuiLocale::EnUs => format!("Failed to save AI Gateway config: {err}"),
        }
    }

    pub(super) fn ai_gw_provider_name_empty(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请输入大模型厂商名称。",
            GuiLocale::EnUs => "Please enter a channel name.",
        }
    }

    pub(super) fn ai_gw_select_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请选择一个大模型厂商。",
            GuiLocale::EnUs => "Please select a channel.",
        }
    }

    pub(super) fn ai_gw_provider_name(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "大模型厂商名称",
            GuiLocale::EnUs => "Channel Name",
        }
    }

    pub(super) fn ai_gw_deleting(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除中...",
            GuiLocale::EnUs => "Deleting...",
        }
    }
}

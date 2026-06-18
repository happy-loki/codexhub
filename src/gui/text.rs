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
            GuiLocale::ZhCn => "关闭这个窗口",
            GuiLocale::EnUs => "Close this window",
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
            GuiLocale::ZhCn => "退出 Codex Remote\tCtrl+Q",
            GuiLocale::EnUs => "&Quit Codex Remote\tCtrl+Q",
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
            GuiLocale::ZhCn => "语言设置已保存，重启 Codex Remote 后生效。",
            GuiLocale::EnUs => "Language saved. Restart Codex Remote to apply it.",
        }
    }

    pub(super) fn language_save_failed(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "语言设置保存失败",
            GuiLocale::EnUs => "Failed to save language setting",
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
            GuiLocale::EnUs => "Check GitHub Releases for a newer Codex Remote version",
        }
    }

    pub(super) fn about(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "关于 Codex Remote",
            GuiLocale::EnUs => "&About Codex Remote",
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
            GuiLocale::ZhCn => "启用生图工具",
            GuiLocale::EnUs => "Enable image generation",
        }
    }

    pub(super) fn image_generation_feature_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "写入 ~/.codex/config.toml 的 [features].image_generation；仅用于影响 Codex CLI 和 VS Code 插件。Codex App 本地会话可能使用自己的 feature gate，本开关不能保证干预。"
            }
            GuiLocale::EnUs => {
                "Writes [features].image_generation in ~/.codex/config.toml for Codex CLI and the VS Code extension. Codex App local sessions may use their own feature gates, so this switch cannot reliably control them."
            }
        }
    }

    pub(super) fn image_generation_feature_note(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "仅 VS Code 插件和 Codex CLI 有效",
            GuiLocale::EnUs => "Only affects VS Code extension and Codex CLI",
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
                "只控制 Codex App 模型选择器展示哪些 models.json 中可用的模型；不影响 AI Gateway 渠道路由。"
            }
            GuiLocale::EnUs => {
                "Controls which available models from models.json are shown in the Codex App picker; does not affect AI Gateway routing."
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
                "Codex 可见模型列表已保存。\n无需重启 Codex，Codex 端通常会在 5 分钟内更新列表。"
            }
            GuiLocale::EnUs => {
                "Codex visible model list saved.\nNo Codex restart is needed; Codex usually updates the list within 5 minutes."
            }
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
            GuiLocale::ZhCn => "清除 Codex 接入",
            GuiLocale::EnUs => "Clear Codex Access",
        }
    }

    pub(super) fn clear_codex_access_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "移除本工具写入的 Codex App 本地接入配置",
            GuiLocale::EnUs => "Remove local Codex App access settings written by this tool",
        }
    }

    pub(super) fn inject_codex_access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "注入 Codex 配置",
            GuiLocale::EnUs => "Inject Codex Config",
        }
    }

    pub(super) fn inject_codex_access_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "写入本工具管理的 Codex App 本地接入配置",
            GuiLocale::EnUs => "Write the local Codex App access settings managed by this tool",
        }
    }

    pub(super) fn injecting_codex_access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "注入中...",
            GuiLocale::EnUs => "Injecting...",
        }
    }

    pub(super) fn clearing_codex_access(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "清除中...",
            GuiLocale::EnUs => "Clearing...",
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
            GuiLocale::ZhCn => format!("已清理 {deleted} 条请求日志"),
            GuiLocale::EnUs => format!("Deleted {deleted} request logs"),
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

    pub(super) fn gui_auto_start_service(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "GUI 会自动启动本地服务；如果一直未运行，请重启 Codex Remote。",
            GuiLocale::EnUs => {
                "The GUI starts the local service automatically. Restart Codex Remote if it stays offline."
            }
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

    pub(super) fn listening(self, bind: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("监听 {bind}"),
            GuiLocale::EnUs => format!("Listening on {bind}"),
        }
    }

    pub(super) fn connected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "已连接",
            GuiLocale::EnUs => "Connected",
        }
    }

    pub(super) fn initializing(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "初始化中",
            GuiLocale::EnUs => "Initializing",
        }
    }

    pub(super) fn control_not_open(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未打开控制",
            GuiLocale::EnUs => "Control Closed",
        }
    }

    pub(super) fn not_injected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "未注入",
            GuiLocale::EnUs => "Not Injected",
        }
    }

    pub(super) fn can_connect(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "可接入",
            GuiLocale::EnUs => "Ready",
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
            GuiLocale::ZhCn => "未接入",
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
            GuiLocale::ZhCn => "Codex App 本地接入配置已卸载。请重启 Codex App 以恢复官方连接。",
            GuiLocale::EnUs => {
                "Local Codex App access settings were removed. Restart Codex App to restore the official connection."
            }
        }
    }

    pub(super) fn codex_app_config_injected(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex App 本地接入配置已注入。请重启 Codex App 生效。",
            GuiLocale::EnUs => {
                "Local Codex App access settings were injected. Restart Codex App to apply them."
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
                "本地服务还没有启动完成，请稍后再试。如果一直未运行，请重启 Codex Remote。"
            }
            GuiLocale::EnUs => {
                "The local service is not ready yet. Try again shortly. Restart Codex Remote if it stays offline."
            }
        }
    }

    pub(super) fn about_description(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "本地 remote-control backend + 聊天工具桥接。",
            GuiLocale::EnUs => "Local remote-control backend with chat integration bridges.",
        }
    }

    pub(super) fn update_dialog_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "Codex Remote 更新",
            GuiLocale::EnUs => "Codex Remote Update",
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

    pub(super) fn update_sources_failed(self, api_err: &str, manifest_err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!(
                    "无法读取 GitHub Release 更新信息：{api_err}\nlatest.json 检查结果：{manifest_err}"
                )
            }
            GuiLocale::EnUs => {
                format!(
                    "Failed to read GitHub Release update info: {api_err}\nlatest.json result: {manifest_err}"
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

    pub(super) fn new_version_message(self, current: &str, latest: &str, notes: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => {
                format!(
                    "发现新版本。\n当前版本：{current}\n最新版本：{latest}\n\n{notes}\n\n是否打开 GitHub Releases 下载？"
                )
            }
            GuiLocale::EnUs => {
                format!(
                    "A new version is available.\nCurrent version: {current}\nLatest version: {latest}\n\n{notes}\n\nOpen GitHub Releases to download it?"
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

    pub(super) fn open_browser_failed(self, err: &str, url: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("无法打开浏览器：{err}\n下载地址：{url}"),
            GuiLocale::EnUs => format!("Failed to open the browser: {err}\nDownload URL: {url}"),
        }
    }

    pub(super) fn confirm_uninstall_codex_app_message(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "卸载会移除本工具写入的 chatgpt_base_url、本地认证信息和 Codex App 环境变量。确认继续？"
            }
            GuiLocale::EnUs => {
                "Uninstalling removes chatgpt_base_url, local auth data, and Codex App environment variables written by this tool. Continue?"
            }
        }
    }

    pub(super) fn confirm_uninstall_codex_app_title(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "卸载 Codex App 配置",
            GuiLocale::EnUs => "Uninstall Codex App Settings",
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
                "本地服务已启动，但 10 秒内没有响应。请检查 logs/codex-remote-chain.log。"
            }
            GuiLocale::EnUs => {
                "The local service started, but did not respond within 10 seconds. Check logs/codex-remote-chain.log."
            }
        }
    }

    pub(super) fn daemon_watchdog_timeout(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => {
                "本地服务启动超过 30 秒仍未完成。请检查旧进程占用或 logs/codex-remote-chain.log。"
            }
            GuiLocale::EnUs => {
                "The local service has not finished starting after 30 seconds. Check for an old process or logs/codex-remote-chain.log."
            }
        }
    }

    pub(super) fn daemon_spawn_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("无法启动本地服务：{err}"),
            GuiLocale::EnUs => format!("Failed to start the local service: {err}"),
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
            GuiLocale::ZhCn => "AI Gateway",
            GuiLocale::EnUs => "AI Gateway",
        }
    }

    pub(super) fn ai_gateway_management(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "渠道管理",
            GuiLocale::EnUs => "AI Gateway Management",
        }
    }

    pub(super) fn ai_gw_channel_list(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "渠道列表",
            GuiLocale::EnUs => "Channels",
        }
    }

    pub(super) fn ai_gw_channel_editor(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "新增 / 编辑渠道",
            GuiLocale::EnUs => "Add / Edit Channel",
        }
    }

    pub(super) fn ai_gw_channel_editor_help(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "选择 OpenAI 或 DeepSeek，并填写渠道连接信息。",
            GuiLocale::EnUs => {
                "Choose OpenAI or DeepSeek and fill in the channel connection details."
            }
        }
    }

    pub(super) fn ai_gw_channel_settings(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "渠道配置",
            GuiLocale::EnUs => "Channel Settings",
        }
    }

    pub(super) fn ai_gw_add_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "新增渠道",
            GuiLocale::EnUs => "Add Channel",
        }
    }

    pub(super) fn ai_gw_edit_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "编辑渠道",
            GuiLocale::EnUs => "Edit Channel",
        }
    }

    pub(super) fn ai_gw_delete_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "删除渠道",
            GuiLocale::EnUs => "Delete Channel",
        }
    }

    pub(super) fn ai_gw_save_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "保存渠道",
            GuiLocale::EnUs => "Save Channel",
        }
    }

    pub(super) fn ai_gw_create_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "创建渠道",
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

    pub(super) fn ai_gw_service_deepseek(self) -> &'static str {
        "DeepSeek"
    }

    pub(super) fn ai_gw_col_base_url(self) -> &'static str {
        "Base URL"
    }

    pub(super) fn ai_gw_col_api_key(self) -> &'static str {
        "API Key"
    }

    pub(super) fn ai_gw_timeout(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "超时(秒)",
            GuiLocale::EnUs => "Timeout (s)",
        }
    }

    pub(super) fn ai_gw_models(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "模型列表",
            GuiLocale::EnUs => "Models",
        }
    }

    pub(super) fn ai_gw_fetch_models(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "获取远端列表",
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
            GuiLocale::ZhCn => format!("获取模型列表失败：{err}"),
            GuiLocale::EnUs => format!("Failed to fetch model list: {err}"),
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

    pub(super) fn provider_type_chat_completions(self) -> &'static str {
        "Chat Completions"
    }

    pub(super) fn ai_gw_saved(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "渠道已保存。",
            GuiLocale::EnUs => "AI Gateway channel saved.",
        }
    }

    pub(super) fn ai_gw_deleted(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "渠道已删除。",
            GuiLocale::EnUs => "AI Gateway channel deleted.",
        }
    }

    pub(super) fn ai_gw_save_failed(self, err: &str) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("保存渠道配置失败：{err}"),
            GuiLocale::EnUs => format!("Failed to save AI Gateway config: {err}"),
        }
    }

    pub(super) fn ai_gw_provider_name_empty(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请输入渠道名称。",
            GuiLocale::EnUs => "Please enter a channel name.",
        }
    }

    pub(super) fn ai_gw_select_channel(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "请选择一个渠道。",
            GuiLocale::EnUs => "Please select a channel.",
        }
    }

    pub(super) fn ai_gw_status_enabled(self, count: usize) -> String {
        match self.locale {
            GuiLocale::ZhCn => format!("AI Gateway: 已启用 ({count} 个渠道)"),
            GuiLocale::EnUs => format!("AI Gateway: Enabled ({count} channels)"),
        }
    }

    pub(super) fn ai_gw_provider_name(self) -> &'static str {
        match self.locale {
            GuiLocale::ZhCn => "渠道名称",
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

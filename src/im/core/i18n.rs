use serde::Deserialize;

use crate::{app_state::SharedState, im_runtime::PendingApproval};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImLocale {
    ZhCn,
    EnUs,
}

impl ImLocale {
    fn from_code(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh_cn" | "cn" => Some(Self::ZhCn),
            "en" | "en-us" | "en_us" | "us" => Some(Self::EnUs),
            _ => None,
        }
    }
}

impl Default for ImLocale {
    fn default() -> Self {
        Self::ZhCn
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ImText {
    locale: ImLocale,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ImConfigLanguage {
    language: Option<String>,
}

pub(crate) fn im_text_for_state(state: &SharedState) -> ImText {
    let locale = std::fs::read_to_string(&state.config_path)
        .ok()
        .and_then(|raw| toml::from_str::<ImConfigLanguage>(&raw).ok())
        .and_then(|config| config.language)
        .and_then(|language| ImLocale::from_code(&language))
        .unwrap_or_default();
    ImText { locale }
}

impl ImText {
    #[cfg(test)]
    pub(crate) fn zh_cn() -> Self {
        Self {
            locale: ImLocale::ZhCn,
        }
    }

    fn choose(self, zh_cn: &'static str, en_us: &'static str) -> &'static str {
        match self.locale {
            ImLocale::ZhCn => zh_cn,
            ImLocale::EnUs => en_us,
        }
    }

    pub(crate) fn field_line(self, label: &str, value: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("{label}：{value}"),
            ImLocale::EnUs => format!("{label}: {value}"),
        }
    }

    pub(crate) fn no_running_turn(self) -> &'static str {
        self.choose("当前没有运行中的 turn。", "There is no running turn.")
    }

    pub(crate) fn interrupted(self) -> &'static str {
        self.choose("已中断当前任务。", "Interrupted the current task.")
    }

    pub(crate) fn exited(self) -> &'static str {
        self.choose("已退出当前会话。", "Exited the current session.")
    }

    pub(crate) fn unsupported_command(self, command: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => {
                format!("不支持的命令：{command}。当前只支持 /s 中断当前任务、/q 退出当前会话。")
            }
            ImLocale::EnUs => {
                format!(
                    "Unsupported command: {command}. Only /s interrupt and /q exit are supported."
                )
            }
        }
    }

    pub(crate) fn turn_busy_notice(self) -> &'static str {
        self.choose(
            "任务还在进行中，打断 /s，退出会话 /q。",
            "A task is still running. Use /s to interrupt or /q to exit the session.",
        )
    }

    pub(crate) fn turn_completed_notice(self) -> &'static str {
        self.choose("✅ 已完成", "✅ Completed")
    }

    pub(crate) fn approval_request_heading(self) -> &'static str {
        self.choose("审批请求", "approval request")
    }

    pub(crate) fn approval_pending_title(self) -> &'static str {
        self.choose("审批待处理", "approval pending")
    }

    pub(crate) fn approval_resolved_title(self) -> &'static str {
        self.choose("审批已处理", "approval resolved")
    }

    pub(crate) fn available_decisions_label(self) -> &'static str {
        self.choose("可选决定", "availableDecisions")
    }

    pub(crate) fn approval_reply_hint(self, pending: &PendingApproval) -> String {
        let options = pending
            .decisions
            .iter()
            .enumerate()
            .map(|(index, _)| format!("/{}", index + 1))
            .collect::<Vec<_>>();
        if options.is_empty() {
            match self.locale {
                ImLocale::ZhCn => "`/y` 或 `/n`".to_string(),
                ImLocale::EnUs => "`/y` or `/n`".to_string(),
            }
        } else {
            options.join(self.choose("、", ", "))
        }
    }

    pub(crate) fn approval_reply_footer(self, hint: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("回复 {hint} 处理。"),
            ImLocale::EnUs => format!("Reply {hint} to choose."),
        }
    }

    pub(crate) fn approval_decision_submitted(self) -> &'static str {
        self.choose("审批决定已提交。", "Approval decision submitted.")
    }

    pub(crate) fn approval_decision_submitted_label(self, label: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已提交：{label}"),
            ImLocale::EnUs => format!("Submitted: {label}"),
        }
    }

    pub(crate) fn approval_selected_label(self, option_index: usize, label: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已选择 /{option_index}：{label}"),
            ImLocale::EnUs => format!("selected /{option_index}: {label}"),
        }
    }

    pub(crate) fn no_pending_approval(self) -> &'static str {
        self.choose("当前没有待处理审批。", "No pending approval.")
    }

    pub(crate) fn approval_not_current(self) -> &'static str {
        self.choose(
            "这个审批请求已经不是当前待处理项。",
            "This approval is no longer current.",
        )
    }

    pub(crate) fn invalid_approval_reply(self, hint: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("审批回复无效，请回复 {hint}。"),
            ImLocale::EnUs => format!("Invalid approval reply. Reply {hint}."),
        }
    }

    pub(crate) fn unsupported_approval_callback(self) -> &'static str {
        self.choose("不支持的审批回调。", "Unsupported approval callback.")
    }

    pub(crate) fn telegram_creation_action_only(self) -> &'static str {
        self.choose(
            "这个创建操作只支持 Telegram 按钮流程。",
            "This creation action is only supported in the Telegram button flow.",
        )
    }

    pub(crate) fn remote_not_connected(self) -> &'static str {
        self.choose(
            "Codex remote-control 还没有连接。请在项目目录运行 codex，确认它已经通过 remote-control 连接到 codex-remote-gateway。",
            "Codex remote-control is not connected yet. Run codex in the project directory and make sure it is connected to codex-remote-gateway through remote-control.",
        )
    }

    pub(crate) fn inbound_expired(self) -> &'static str {
        self.choose(
            "这条消息是在上一轮任务期间收到的，已跳过。请重新发送最新指令。",
            "This message arrived during the previous task and was skipped. Send the latest instruction again.",
        )
    }

    pub(crate) fn app_message_failed(self, error: &dyn std::fmt::Display) -> String {
        match self.locale {
            ImLocale::ZhCn => {
                format!(
                    "Codex 没有接收这条消息：{error}\n\n当前 IM 会话绑定的 Codex 端点可能已经退出或断开。请回复 /q 退出当前会话，然后重新新建会话或恢复历史会话。"
                )
            }
            ImLocale::EnUs => {
                format!(
                    "Codex did not accept this message: {error}\n\nThe Codex endpoint bound to this IM session may have exited or disconnected. Reply /q to exit the current session, then create a new session or resume a historical session."
                )
            }
        }
    }

    pub(crate) fn creating_new_thread(self) -> &'static str {
        self.choose(
            "正在创建新的 Codex 会话...",
            "Creating a new Codex session...",
        )
    }

    pub(crate) fn created_new_session_title(self) -> &'static str {
        self.choose("已创建新会话", "New session created")
    }

    pub(crate) fn creating_session_title(self) -> &'static str {
        self.choose("正在创建会话", "Creating Session")
    }

    pub(crate) fn created_new_session_body(self, thread_id: &str, summary: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => {
                format!("已接入新 thread `{thread_id}`。\n\n{summary}\n\n现在可以直接发送消息。")
            }
            ImLocale::EnUs => {
                format!(
                    "Attached to new thread `{thread_id}`.\n\n{summary}\n\nYou can now send messages directly."
                )
            }
        }
    }

    pub(crate) fn subscribing_thread(self, thread_id: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("正在订阅 thread `{thread_id}` 的后续事件..."),
            ImLocale::EnUs => format!("Subscribing to thread `{thread_id}` events..."),
        }
    }

    pub(crate) fn subscribed_session_title(self) -> &'static str {
        self.choose("已订阅会话", "Session attached")
    }

    pub(crate) fn subscribing_session_title(self) -> &'static str {
        self.choose("正在接入会话", "Attaching Session")
    }

    pub(crate) fn subscribed_session_body(
        self,
        thread_id: &str,
        title: &str,
        cwd: &str,
        status: &str,
    ) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已接入 thread `{thread_id}`。\n\n{title}\n{cwd}\n{status}"),
            ImLocale::EnUs => {
                format!("Attached to thread `{thread_id}`.\n\n{title}\n{cwd}\n{status}")
            }
        }
    }

    pub(crate) fn resumed_session_title(self) -> &'static str {
        self.choose("已接入历史会话", "History session attached")
    }

    pub(crate) fn resumed_session_body(self, title: &str, cwd: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("{title}\n{cwd}\n\n现在可以直接发送消息。"),
            ImLocale::EnUs => format!("{title}\n{cwd}\n\nYou can now send messages directly."),
        }
    }

    pub(crate) fn create_settings_menu_suffix(self) -> &'static str {
        self.choose(
            "\n\n1. 修改目录\n2. 修改模型\n3. 修改推理强度\n4. 修改权限\n5. 创建会话\n6. 恢复历史会话\n\n回复数字选择。也可以回复 y 创建，n 取消。",
            "\n\n1. Change directory\n2. Change model\n3. Change reasoning effort\n4. Change permissions\n5. Create session\n6. Restore history session\n\nReply with a number. You can also reply y to create or n to cancel.",
        )
    }

    pub(crate) fn invalid_create_settings_reply(self) -> &'static str {
        self.choose(
            "请回复 1~6，或回复 y 创建、n 取消。",
            "Reply 1~6, or reply y to create, n to cancel.",
        )
    }

    pub(crate) fn create_cancelled(self) -> &'static str {
        self.choose("已取消创建会话。", "Session creation cancelled.")
    }

    pub(crate) fn create_option_unavailable(self) -> &'static str {
        self.choose(
            "这个创建选项不可用，请重新打开创建设置。",
            "This creation option is unavailable. Open creation settings again.",
        )
    }

    pub(crate) fn create_option_expired(self) -> &'static str {
        self.choose(
            "这个创建选项已经失效，请重新打开创建设置。",
            "This creation option has expired. Open creation settings again.",
        )
    }

    pub(crate) fn invalid_option_index(self) -> &'static str {
        self.choose(
            "这个选项序号不可用，请按当前列表里的数字选择。",
            "This option number is unavailable. Choose a number from the current list.",
        )
    }

    pub(crate) fn invalid_create_form(self, error: &dyn std::fmt::Display) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("新建会话参数不正确：{error}"),
            ImLocale::EnUs => format!("Invalid new session settings: {error}"),
        }
    }

    pub(crate) fn custom_cwd_prompt_wechat(self) -> &'static str {
        self.choose(
            "请发送项目目录的绝对路径。目录不存在时，创建会话时会自动创建。\n\n回复 n 取消。",
            "Send the absolute project directory path. If it does not exist, it will be created when the session starts.\n\nReply n to cancel.",
        )
    }

    pub(crate) fn custom_cwd_prompt_telegram(self) -> &'static str {
        self.choose(
            "请发送项目目录的绝对路径。目录不存在时，创建 thread 时会自动创建。\n\n发送 /cancel 取消。",
            "Send the absolute project directory path. If it does not exist, it will be created when the thread starts.\n\nSend /cancel to cancel.",
        )
    }

    pub(crate) fn cwd_must_be_absolute_wechat(self) -> &'static str {
        self.choose(
            "项目目录需要是绝对路径。请重新发送绝对路径，或回复 n 取消。",
            "The project directory must be an absolute path. Send an absolute path again, or reply n to cancel.",
        )
    }

    pub(crate) fn cwd_must_be_absolute_telegram(self) -> &'static str {
        self.choose(
            "项目目录需要是绝对路径。请重新发送一个绝对路径，或发送 /cancel 取消。",
            "The project directory must be an absolute path. Send an absolute path again, or send /cancel to cancel.",
        )
    }

    pub(crate) fn custom_cwd_label(self) -> &'static str {
        self.choose("自定义或新建目录", "Custom or new directory")
    }

    pub(crate) fn custom_cwd_summary(self) -> &'static str {
        self.choose(
            "选择后发送绝对路径。目录不存在时会自动创建。",
            "Select this, then send an absolute path. Missing directories are created automatically.",
        )
    }

    pub(crate) fn create_choice_wechat(self) -> &'static str {
        self.choose(
            "当前微信会话还没有接入 Codex 会话。\n\n1. 新建会话\n2. 恢复历史会话或接入当前 Codex 活跃会话\n\n回复 1 或 2。",
            "This WeChat chat is not attached to a Codex session yet.\n\n1. Create new session\n2. Restore a history session or attach to the current active Codex session\n\nReply 1 or 2.",
        )
    }

    pub(crate) fn create_choice_telegram(self) -> &'static str {
        self.choose(
            "当前 Telegram 会话还没有接入 Codex thread。\n请选择创建新会话，或恢复历史会话或接入当前 Codex 活跃会话。",
            "This Telegram chat is not attached to a Codex thread yet.\nCreate a new session, or restore a history session or attach to the current active Codex session.",
        )
    }

    pub(crate) fn invalid_route_choice_wechat(self) -> &'static str {
        self.choose(
            "请回复 1 新建会话，或回复 2 恢复历史会话或接入当前 Codex 活跃会话。",
            "Reply 1 to create a session, or 2 to restore a history session or attach to the current active Codex session.",
        )
    }

    pub(crate) fn no_history_create_hint_wechat(self) -> &'static str {
        self.choose(
            "当前没有可恢复的历史会话。\n\n回复 1 创建新会话。",
            "There are no restorable history sessions.\n\nReply 1 to create a new session.",
        )
    }

    pub(crate) fn list_load_failed(self) -> &'static str {
        self.choose(
            "会话列表加载失败：Codex App 暂时没有响应，请稍后重试。",
            "Failed to load the session list: Codex App is not responding right now. Try again later.",
        )
    }

    pub(crate) fn list_load_failed_title(self) -> &'static str {
        self.choose("会话列表加载失败", "Failed to Load Sessions")
    }

    pub(crate) fn first_page(self) -> &'static str {
        self.choose("已经是第一页。", "Already on the first page.")
    }

    pub(crate) fn last_page(self) -> &'static str {
        self.choose("已经是最后一页。", "Already on the last page.")
    }

    pub(crate) fn invalid_thread_index(self) -> &'static str {
        self.choose(
            "这个序号不在当前会话列表里，请按列表里的数字选择。",
            "This number is not in the current session list. Choose a number from the list.",
        )
    }

    pub(crate) fn invalid_thread_index_restart(self) -> &'static str {
        self.choose(
            "这个会话序号不可用，请重新发送一条消息触发会话选择。",
            "This session number is unavailable. Send a new message to reopen session selection.",
        )
    }

    pub(crate) fn thread_operation_expired(self) -> &'static str {
        self.choose(
            "这个 thread 操作已经失效，请重新发送一条消息触发会话选择。",
            "This thread operation has expired. Send a new message to reopen session selection.",
        )
    }

    pub(crate) fn thread_choice_card_expired(self) -> &'static str {
        self.choose(
            "这张 thread 选择卡片已经失效，请重新发送消息。",
            "This thread selection card has expired. Send a new message.",
        )
    }

    pub(crate) fn thread_choice_not_current(self) -> &'static str {
        self.choose(
            "这个 thread 选择不属于当前会话。",
            "This thread selection does not belong to the current chat.",
        )
    }

    pub(crate) fn thread_list_not_current(self) -> &'static str {
        self.choose(
            "这个 thread 列表不属于当前会话。",
            "This thread list does not belong to the current chat.",
        )
    }

    pub(crate) fn thread_selection_expired(self) -> &'static str {
        self.choose(
            "这个 thread 选择已经失效，请重新打开列表。",
            "This thread selection has expired. Open the list again.",
        )
    }

    pub(crate) fn stale_thread_unbound(self) -> &'static str {
        self.choose(
            "当前绑定的 Codex thread 已失效，已解除绑定。",
            "The attached Codex thread is no longer valid and has been detached.",
        )
    }

    pub(crate) fn unsupported_thread_action(self, action: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("不支持的 thread 操作：{action}"),
            ImLocale::EnUs => format!("Unsupported thread action: {action}"),
        }
    }

    pub(crate) fn thread_list_title_wechat(self) -> &'static str {
        self.choose("恢复历史会话", "Restore History Session")
    }

    pub(crate) fn thread_list_title_telegram(self) -> &'static str {
        self.choose("恢复历史会话", "Restore history session")
    }

    pub(crate) fn thread_list_title_feishu(self) -> &'static str {
        self.choose("选择 Codex 会话", "Select Codex Session")
    }

    pub(crate) fn thread_list_body_telegram(self, provider: Option<&str>) -> String {
        let mut body = self
            .choose(
                "请选择一个会话接入后续事件。",
                "Choose a session to attach future events.",
            )
            .to_string();
        if let Some(provider) = provider {
            body.push_str(&match self.locale {
                ImLocale::ZhCn => format!("\n已按当前 Codex App provider `{provider}` 过滤。"),
                ImLocale::EnUs => {
                    format!("\nFiltered by current Codex App provider `{provider}`.")
                }
            });
        }
        body
    }

    pub(crate) fn thread_list_body_feishu(self, provider: Option<&str>) -> String {
        let mut body = self
            .choose(
                "当前飞书会话还没有订阅任何 Codex thread。请选择一个会话接入后续事件。",
                "This Feishu chat is not subscribed to any Codex thread yet. Choose a session to attach future events.",
            )
            .to_string();
        if let Some(provider) = provider {
            body.push_str(&match self.locale {
                ImLocale::ZhCn => {
                    format!("\n\n<font color='grey'>已按当前 Codex App provider `{provider}` 过滤。</font>")
                }
                ImLocale::EnUs => {
                    format!("\n\n<font color='grey'>Filtered by current Codex App provider `{provider}`.</font>")
                }
            });
        }
        body
    }

    pub(crate) fn provider_filter_line(self, provider: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已按当前 Codex App provider `{provider}` 过滤。"),
            ImLocale::EnUs => format!("Filtered by current Codex App provider `{provider}`."),
        }
    }

    pub(crate) fn reply_choose_session_range(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("回复 1~{count} 选择会话"),
            ImLocale::EnUs => format!("Reply 1~{count} to choose a session"),
        }
    }

    pub(crate) fn reply_choose_range(self, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("回复 1~{count} 选择"),
            ImLocale::EnUs => format!("Reply 1~{count} to choose"),
        }
    }

    pub(crate) fn page_hint(self, page: usize, actions: &[String]) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("第 {} 页 · {}", page.max(1), actions.join("，")),
            ImLocale::EnUs => format!("Page {} · {}", page.max(1), actions.join(", ")),
        }
    }

    pub(crate) fn page_click_hint(self, page: usize, count: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("第 {} 页 · 点击 /1 ~ /{} 选择", page.max(1), count),
            ImLocale::EnUs => format!("Page {} · Click /1 ~ /{} to choose", page.max(1), count),
        }
    }

    pub(crate) fn page_label(self, page: usize) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("第 {} 页", page.max(1)),
            ImLocale::EnUs => format!("Page {}", page.max(1)),
        }
    }

    pub(crate) fn no_options(self) -> &'static str {
        self.choose("当前没有可选项。", "No options are available.")
    }

    pub(crate) fn no_restorable_history(self) -> &'static str {
        self.choose(
            "当前没有可恢复的历史会话。",
            "There are no restorable history sessions.",
        )
    }

    pub(crate) fn no_restorable_history_workspace(self) -> &'static str {
        self.choose(
            "当前工作区下没有可恢复的历史会话。",
            "There are no restorable history sessions in this workspace.",
        )
    }

    pub(crate) fn prev_action_markdown(self) -> &'static str {
        self.choose("**p** 上一页", "**p** Previous")
    }

    pub(crate) fn next_action_markdown(self) -> &'static str {
        self.choose("**n** 下一页", "**n** Next")
    }

    pub(crate) fn back_create_settings_markdown(self) -> &'static str {
        self.choose("**0** 返回设置", "**0** Back to settings")
    }

    pub(crate) fn previous_page_button(self) -> &'static str {
        self.choose("上一页", "Previous")
    }

    pub(crate) fn next_page_button(self) -> &'static str {
        self.choose("下一页", "Next")
    }

    pub(crate) fn create_new_session_button(self) -> &'static str {
        self.choose("创建新会话", "Create new session")
    }

    pub(crate) fn restore_history_button(self) -> &'static str {
        self.choose("恢复历史会话", "Restore history session")
    }

    pub(crate) fn directory_button(self) -> &'static str {
        self.choose("目录", "Directory")
    }

    pub(crate) fn model_button(self) -> &'static str {
        self.choose("模型", "Model")
    }

    pub(crate) fn effort_button(self) -> &'static str {
        self.choose("推理强度", "Reasoning")
    }

    pub(crate) fn permission_button(self) -> &'static str {
        self.choose("权限", "Permissions")
    }

    pub(crate) fn create_button(self) -> &'static str {
        self.choose("创建", "Create")
    }

    pub(crate) fn back_to_create_settings_button(self) -> &'static str {
        self.choose("返回创建设置", "Back to settings")
    }

    pub(crate) fn create_choice_title_feishu(self) -> &'static str {
        self.choose("未绑定会话", "No Session Attached")
    }

    pub(crate) fn create_choice_body_feishu(self) -> &'static str {
        self.choose(
            "当前飞书会话还没有接入 Codex thread。请选择新建会话，或恢复历史会话或接入当前 Codex 活跃会话。",
            "This Feishu chat is not attached to a Codex thread yet. Create a new session, or restore a history session or attach to the current active Codex session.",
        )
    }

    pub(crate) fn create_choice_tip_feishu(self) -> &'static str {
        self.choose(
            "提示：回复 `/q` 可退出当前会话，回复 `/s` 可中断当前任务。",
            "Tip: reply `/q` to exit the current session, or `/s` to interrupt the current task.",
        )
    }

    pub(crate) fn create_new_description_feishu(self) -> &'static str {
        self.choose(
            "创建一个新的 Codex thread，并接入后续消息。",
            "Create a new Codex thread and attach future messages.",
        )
    }

    pub(crate) fn restore_history_description_feishu(self) -> &'static str {
        self.choose(
            "查看 Codex App 当前可恢复的历史 thread 列表。",
            "View restorable Codex App history threads.",
        )
    }

    pub(crate) fn selected_prefix_feishu(self, label: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已选择 · {label}"),
            ImLocale::EnUs => format!("Selected · {label}"),
        }
    }

    pub(crate) fn create_settings_card_intro(self) -> &'static str {
        self.choose(
            "选择这次新会话的属性。Provider 固定使用 Codex App 当前配置。",
            "Choose settings for this new session. Provider uses the current Codex App configuration.",
        )
    }

    pub(crate) fn create_settings_card_title(self) -> &'static str {
        self.choose("新建会话设置", "New Session Settings")
    }

    pub(crate) fn cwd_section(self) -> &'static str {
        self.choose("项目目录", "Project Directory")
    }

    pub(crate) fn cwd_select_placeholder(self) -> &'static str {
        self.choose("选择已有项目目录", "Select an existing project directory")
    }

    pub(crate) fn cwd_custom_placeholder(self) -> &'static str {
        self.choose(
            "可选：填绝对路径；不存在会自动创建",
            "Optional: absolute path; created automatically if missing",
        )
    }

    pub(crate) fn model_section(self) -> &'static str {
        self.choose("模型", "Model")
    }

    pub(crate) fn model_select_placeholder(self) -> &'static str {
        self.choose("选择模型", "Select model")
    }

    pub(crate) fn effort_section(self) -> &'static str {
        self.choose("推理强度", "Reasoning Effort")
    }

    pub(crate) fn effort_select_placeholder(self) -> &'static str {
        self.choose("选择推理强度", "Select reasoning effort")
    }

    pub(crate) fn permission_section(self) -> &'static str {
        self.choose("权限", "Permissions")
    }

    pub(crate) fn permission_select_placeholder(self) -> &'static str {
        self.choose("选择权限", "Select permissions")
    }

    pub(crate) fn confirm_create_button(self) -> &'static str {
        self.choose("确认创建", "Create")
    }

    pub(crate) fn create_default_button(self) -> &'static str {
        self.choose("使用默认配置创建", "Create with defaults")
    }

    pub(crate) fn create_default_description(self) -> &'static str {
        self.choose(
            "使用当前 provider，不指定目录、模型和推理强度。",
            "Use the current provider without overriding directory, model, or reasoning effort.",
        )
    }

    pub(crate) fn back_button(self) -> &'static str {
        self.choose("返回", "Back")
    }

    pub(crate) fn back_description(self) -> &'static str {
        self.choose(
            "回到新建/恢复会话选择。",
            "Return to create/restore session selection.",
        )
    }

    pub(crate) fn remote_label(self) -> &'static str {
        self.choose("远端", "Remote")
    }

    pub(crate) fn cwd_label(self) -> &'static str {
        self.choose("目录", "Directory")
    }

    pub(crate) fn provider_label(self) -> &'static str {
        "Provider"
    }

    pub(crate) fn model_label(self) -> &'static str {
        self.choose("模型", "Model")
    }

    pub(crate) fn effort_label(self) -> &'static str {
        self.choose("推理强度", "Reasoning effort")
    }

    pub(crate) fn permission_label_title(self) -> &'static str {
        self.choose("权限", "Permissions")
    }

    pub(crate) fn not_connected(self) -> &'static str {
        self.choose("未连接", "Not connected")
    }

    pub(crate) fn codex_app_default_value(self) -> &'static str {
        self.choose("使用 Codex App 默认值", "Use Codex App default")
    }

    pub(crate) fn create_thread_heading(self) -> &'static str {
        self.choose("创建新 Codex thread", "Create New Codex Thread")
    }

    pub(crate) fn current_settings_heading(self) -> &'static str {
        self.choose("当前设置：", "Current settings:")
    }

    pub(crate) fn create_help_footer(self) -> &'static str {
        self.choose(
            "请选择要修改的设置，确认后创建。",
            "Select settings to change, then create the session.",
        )
    }

    pub(crate) fn current_prefix(self, value: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("当前：{value}"),
            ImLocale::EnUs => format!("Current: {value}"),
        }
    }

    pub(crate) fn current_default_prefix(self, value: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("当前默认：{value}"),
            ImLocale::EnUs => format!("Current default: {value}"),
        }
    }

    pub(crate) fn selected_prefix(self, label: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("已选：{label}"),
            ImLocale::EnUs => format!("Selected: {label}"),
        }
    }

    pub(crate) fn selected(self) -> &'static str {
        self.choose("已选", "Selected")
    }

    pub(crate) fn use_current_provider(self) -> &'static str {
        self.choose(
            "使用 Codex App 当前 provider",
            "Use Codex App current provider",
        )
    }

    pub(crate) fn use_default_cwd(self) -> &'static str {
        self.choose("使用 Codex App 默认目录", "Use Codex App default directory")
    }

    pub(crate) fn waiting_custom_cwd(self) -> &'static str {
        self.choose("等待输入自定义目录", "Waiting for custom directory")
    }

    pub(crate) fn use_default_cwd_with_path(self, cwd: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("使用 Codex App 默认目录（{cwd}）"),
            ImLocale::EnUs => format!("Use Codex App default directory ({cwd})"),
        }
    }

    pub(crate) fn use_current_model(self) -> &'static str {
        self.choose("使用当前模型", "Use current model")
    }

    pub(crate) fn use_current_model_with_value(self, model: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("使用当前模型（{model}）"),
            ImLocale::EnUs => format!("Use current model ({model})"),
        }
    }

    pub(crate) fn do_not_override_model(self) -> &'static str {
        self.choose(
            "不覆盖模型，由 Codex App 决定",
            "Do not override model; Codex App decides",
        )
    }

    pub(crate) fn use_model_default_effort(self) -> &'static str {
        self.choose("使用模型默认推理强度", "Use model default reasoning effort")
    }

    pub(crate) fn use_default_effort_with_value(self, effort: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("使用默认推理强度（{effort}）"),
            ImLocale::EnUs => format!("Use default reasoning effort ({effort})"),
        }
    }

    pub(crate) fn do_not_override_effort(self) -> &'static str {
        self.choose(
            "不覆盖推理强度，由模型决定",
            "Do not override reasoning effort; the model decides",
        )
    }

    pub(crate) fn use_current_permission(self) -> &'static str {
        self.choose(
            "使用 Codex App 当前权限",
            "Use Codex App current permissions",
        )
    }

    pub(crate) fn use_current_permission_with_value(self, permission: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("使用 Codex App 当前权限（{permission}）"),
            ImLocale::EnUs => format!("Use Codex App current permissions ({permission})"),
        }
    }

    pub(crate) fn do_not_override_permission(self) -> &'static str {
        self.choose("不覆盖权限配置", "Do not override permission settings")
    }

    pub(crate) fn select_project_dir_title(self) -> &'static str {
        self.choose("选择项目目录", "Select Project Directory")
    }

    pub(crate) fn select_model_title(self) -> &'static str {
        self.choose("选择模型", "Select Model")
    }

    pub(crate) fn select_effort_title(self) -> &'static str {
        self.choose("选择推理强度", "Select Reasoning Effort")
    }

    pub(crate) fn select_permission_title(self) -> &'static str {
        self.choose("选择权限", "Select Permissions")
    }

    pub(crate) fn default_permission_label(self) -> &'static str {
        self.choose("默认权限", "Default permissions")
    }

    pub(crate) fn auto_review_label(self) -> &'static str {
        self.choose("自动审查", "Auto review")
    }

    pub(crate) fn full_access_label(self) -> &'static str {
        self.choose("完全访问权限", "Full access")
    }

    pub(crate) fn read_only_label(self) -> &'static str {
        self.choose("只读", "Read only")
    }

    pub(crate) fn custom_label(self) -> &'static str {
        self.choose("自定义", "Custom")
    }

    pub(crate) fn default_permission_summary(self) -> &'static str {
        self.choose(
            "适合常规项目，需要时由用户确认。",
            "Good for normal projects; asks the user when needed.",
        )
    }

    pub(crate) fn auto_review_summary(self) -> &'static str {
        self.choose(
            "需要审批时优先交给自动审查。",
            "Use automatic review first when approval is needed.",
        )
    }

    pub(crate) fn full_access_summary(self) -> &'static str {
        self.choose(
            "不再请求确认，允许完整本机访问。",
            "Do not ask for confirmation; allow full local access.",
        )
    }

    pub(crate) fn reasoning_effort_label(self, effort: &str) -> String {
        match (self.locale, effort.trim()) {
            (ImLocale::ZhCn, "none") => "无 (none)".to_string(),
            (ImLocale::ZhCn, "minimal") => "极低 (minimal)".to_string(),
            (ImLocale::ZhCn, "low") => "低 (low)".to_string(),
            (ImLocale::ZhCn, "medium") => "中 (medium)".to_string(),
            (ImLocale::ZhCn, "high") => "高 (high)".to_string(),
            (ImLocale::ZhCn, "xhigh") => "超高 (xhigh)".to_string(),
            (ImLocale::EnUs, "none") => "None (none)".to_string(),
            (ImLocale::EnUs, "minimal") => "Minimal (minimal)".to_string(),
            (ImLocale::EnUs, "low") => "Low (low)".to_string(),
            (ImLocale::EnUs, "medium") => "Medium (medium)".to_string(),
            (ImLocale::EnUs, "high") => "High (high)".to_string(),
            (ImLocale::EnUs, "xhigh") => "Extra high (xhigh)".to_string(),
            (_, other) => other.to_string(),
        }
    }

    pub(crate) fn permission_label(self, permission: &str) -> String {
        match permission.trim() {
            "workspace_user" | "default" | "default_permissions" | "auto" => {
                self.default_permission_label().to_string()
            }
            "auto_review" | "guardian-approvals" | "guardian_approvals" => {
                self.auto_review_label().to_string()
            }
            "full_access" | "full-access" => self.full_access_label().to_string(),
            "read_only" | "read-only" => self.read_only_label().to_string(),
            other => other.to_string(),
        }
    }

    pub(crate) fn thread_title_fallback(self, thread_id: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("会话 {thread_id}"),
            ImLocale::EnUs => format!("Session {thread_id}"),
        }
    }

    pub(crate) fn untitled_session(self) -> &'static str {
        self.choose("未命名会话", "Untitled session")
    }

    pub(crate) fn thread_cwd_summary(self, cwd: Option<String>) -> String {
        match (self.locale, cwd) {
            (ImLocale::ZhCn, Some(cwd)) => format!("目录：`{cwd}`"),
            (ImLocale::EnUs, Some(cwd)) => format!("Directory: `{cwd}`"),
            (ImLocale::ZhCn, None) => "目录未知".to_string(),
            (ImLocale::EnUs, None) => "Directory unknown".to_string(),
        }
    }

    pub(crate) fn thread_status(self, status: &str) -> String {
        match (self.locale, status) {
            (ImLocale::ZhCn, "active") => "运行中".to_string(),
            (ImLocale::ZhCn, "idle") => "空闲".to_string(),
            (ImLocale::ZhCn, "notLoaded") => "未加载".to_string(),
            (ImLocale::ZhCn, "systemError") => "系统错误".to_string(),
            (ImLocale::EnUs, "active") => "Running".to_string(),
            (ImLocale::EnUs, "idle") => "Idle".to_string(),
            (ImLocale::EnUs, "notLoaded") => "Not loaded".to_string(),
            (ImLocale::EnUs, "systemError") => "System error".to_string(),
            (_, other) => other.to_string(),
        }
    }

    pub(crate) fn route_state_current(self) -> &'static str {
        self.choose("当前会话", "Current session")
    }

    pub(crate) fn route_state_loaded(self) -> &'static str {
        self.choose("已加载，可接入", "Loaded, attachable")
    }

    pub(crate) fn route_state_history(self) -> &'static str {
        self.choose("历史会话，可接入", "History session, attachable")
    }

    pub(crate) fn current_short(self) -> &'static str {
        self.choose("当前", "Current")
    }

    pub(crate) fn loaded_short(self) -> &'static str {
        self.choose("已加载", "Loaded")
    }

    pub(crate) fn project_header(self, name: &str) -> String {
        match self.locale {
            ImLocale::ZhCn => format!("项目：{name}"),
            ImLocale::EnUs => format!("Project: {name}"),
        }
    }

    pub(crate) fn unknown_project_header(self) -> &'static str {
        self.choose("项目：未知目录", "Project: Unknown directory")
    }
}

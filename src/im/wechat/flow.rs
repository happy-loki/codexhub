use anyhow::Result;
use tracing::info;

use crate::{
    app_state::SharedState,
    im::{
        core::{
            approval::{ApprovalReplyOutcome, resolve_approval_reply, submit_approval_decision},
            routing::{active_turn_for_message, clear_thread_binding, route_for_message},
            session::{create_and_bind_thread, resume_and_bind_thread},
            thread::{
                ThreadCreateOption, apply_thread_create_draft_value, create_options_for_field,
                expand_home_prefix, is_approval_reply, load_thread_create_defaults_for_client,
                next_thread_routing_request_id, normalize_thread_create_field,
                summarize_thread_cwd, summarize_thread_start_options, summarize_thread_title,
                thread_create_form_from_draft, thread_create_help_text,
                thread_start_options_from_form_for_client,
            },
            thread_list::{empty_thread_routing_request, load_thread_routing_page},
            turn::{TurnStartOutcome, start_turn_for_route, turn_busy_notice},
        },
        wechat::{adapter::WechatAdapter, api::WechatApi, types::WechatSettings},
    },
    im_runtime::{RouteTarget, ThreadRoutingRequestState, ThreadRoutingStage, TurnOrigin},
    remote_control_backend,
    types::InboundMessage,
};

const WECHAT_CREATE_OPTION_PAGE_SIZE: usize = 8;

pub(crate) async fn handle_inbound(state: SharedState, message: InboundMessage) -> Result<()> {
    info!(
        "inbound wechat message chat={} sender={}",
        message.chat_id, message.sender_id
    );
    state
        .push_event(
            "info",
            "wechat_message",
            format!(
                "chat={} sender={} text_len={}",
                message.chat_id,
                message.sender_id,
                message.text.chars().count()
            ),
        )
        .await;

    let config = state.config.lock().await.clone();
    let wechat_config = config
        .wechat_account(&message.account_id)
        .unwrap_or_else(|| config.wechat.clone());
    let settings = WechatSettings::from_app_config(&wechat_config);
    let api = WechatApi::new(settings);
    let adapter = WechatAdapter::new(api);
    let account_id = message.account_id.clone();
    let route = route_for_message(&message);
    {
        let mut runtime = state.runtime.lock().await;
        runtime.last_route = Some(route.clone());
    }

    let trimmed = message.text.trim();
    let normalized = command(trimmed);
    let menu_command = menu_command(trimmed);

    if let Some(command) = menu_command.as_deref()
        && is_approval_reply(command)
        && state
            .runtime
            .lock()
            .await
            .has_pending_approvals(&message.conversation_key())
    {
        handle_wechat_approval_text_reply(&state, &adapter, &message, command).await?;
        return Ok(());
    }

    if handle_thread_create_custom_cwd_text_input(&state, &adapter, &message, trimmed).await? {
        return Ok(());
    }

    if handle_thread_create_option_text_reply(
        &state,
        &adapter,
        &message,
        trimmed,
        menu_command.as_deref(),
    )
    .await?
    {
        return Ok(());
    }

    if handle_thread_create_settings_text_reply(
        &state,
        &adapter,
        &message,
        trimmed,
        menu_command.as_deref(),
    )
    .await?
    {
        return Ok(());
    }

    if let Some(command) = menu_command.as_deref()
        && handle_thread_route_choice_text_reply(&state, &adapter, &message, command).await?
    {
        return Ok(());
    }

    if let Some(command) = menu_command.as_deref()
        && handle_thread_list_text_reply(&state, &adapter, &message, command).await?
    {
        return Ok(());
    }

    match normalized.as_deref() {
        Some("/s") => {
            let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await else {
                adapter
                    .send_text(
                        &state,
                        &account_id,
                        &message.chat_id,
                        "当前没有运行中的 turn。",
                    )
                    .await?;
                return Ok(());
            };
            remote_control_backend::interrupt_turn_for_client(
                &state,
                &route.conversation_key,
                &thread_id,
                &turn_id,
            )
            .await?;
            remote_control_backend::clear_turn_for_client(
                &state,
                &route.conversation_key,
                Some(&turn_id),
            )
            .await;
            state
                .runtime
                .lock()
                .await
                .mark_turn_completed(&thread_id, Some(&turn_id));
            adapter
                .send_text(&state, &account_id, &message.chat_id, "已中断当前任务。")
                .await?;
            return Ok(());
        }
        Some("/q") => {
            if let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await {
                let _ = remote_control_backend::interrupt_turn_for_client(
                    &state,
                    &route.conversation_key,
                    &thread_id,
                    &turn_id,
                )
                .await;
                remote_control_backend::clear_thread_for_client(
                    &state,
                    &route.conversation_key,
                    Some(&thread_id),
                )
                .await;
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(&thread_id, Some(&turn_id));
            }
            clear_thread_binding(&state, &route.conversation_key).await?;
            adapter
                .send_text(&state, &account_id, &message.chat_id, "已退出当前会话。")
                .await?;
            return Ok(());
        }
        Some(other) => {
            adapter
                .send_text(
                    &state,
                    &account_id,
                    &message.chat_id,
                    &format!(
                        "不支持的命令：{other}。当前只支持 /s 中断当前任务、/q 退出当前会话。"
                    ),
                )
                .await?;
            return Ok(());
        }
        None => {}
    }

    if let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await {
        adapter
            .send_text(
                &state,
                &account_id,
                &message.chat_id,
                turn_busy_notice(&thread_id, &turn_id),
            )
            .await?;
        return Ok(());
    }

    let remote_status = remote_control_backend::status_snapshot(&state).await;
    if !remote_status.connected {
        adapter
            .send_text(
                &state,
                &account_id,
                &message.chat_id,
                "Codex remote-control 还没有连接。请在项目目录运行 Codex，并打开 remote-control 后再发送消息。",
            )
            .await?;
        return Ok(());
    }

    match start_turn_for_route(
        &state,
        &route,
        trimmed,
        &message.attachments,
        message.received_at_ms,
        TurnOrigin::Wechat,
    )
    .await
    {
        TurnStartOutcome::Started { thread_id, turn_id } => {
            state
                .push_event(
                    "info",
                    "wechat_turn_started",
                    format!(
                        "chat={} thread={} turn={turn_id}",
                        message.chat_id, thread_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::Busy { thread_id, turn_id } => {
            adapter
                .send_text(
                    &state,
                    &account_id,
                    &message.chat_id,
                    turn_busy_notice(&thread_id, turn_id.as_deref().unwrap_or("")),
                )
                .await?;
            Ok(())
        }
        TurnStartOutcome::Expired { thread_id } => {
            adapter
                .send_text(
                    &state,
                    &account_id,
                    &message.chat_id,
                    "这条消息是在上一轮任务期间收到的，已跳过。请重新发送最新指令。",
                )
                .await?;
            state
                .push_event(
                    "warn",
                    "wechat_inbound_expired",
                    format!(
                        "chat={} thread={thread_id} message={}",
                        message.chat_id, message.message_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::NoThread => {
            send_thread_routing_choice(&state, &adapter, &message).await?;
            Ok(())
        }
        TurnStartOutcome::Stale { thread_id } => {
            state
                .push_event(
                    "warn",
                    "wechat_thread_route_stale",
                    format!("conversation={} thread={thread_id}", route.conversation_key),
                )
                .await;
            send_thread_routing_choice(&state, &adapter, &message).await
        }
        TurnStartOutcome::Failed { error } => {
            adapter
                .send_text(
                    &state,
                    &account_id,
                    &message.chat_id,
                    &format!("Codex App 没有接收这条消息：{error}\n\n请确认 Codex App 还打开着 remote-control。"),
                )
                .await?;
            Err(error)
        }
    }
}

async fn create_wechat_thread_for_route(
    state: &SharedState,
    adapter: &WechatAdapter,
    route: &RouteTarget,
    options: remote_control_backend::ThreadStartOptions,
    request_id: Option<&str>,
) -> Result<String> {
    adapter
        .send_text(
            state,
            &route.account_id,
            &route.chat_id,
            "正在创建新的 Codex 会话...",
        )
        .await?;
    let thread_id = create_and_bind_thread(state, route, options.clone(), request_id).await?;
    adapter
        .send_text(
            state,
            &route.account_id,
            &route.chat_id,
            &format!(
                "已创建新会话\n\n已接入新 thread `{thread_id}`。\n\n{}\n\n现在可以直接发送消息。",
                summarize_thread_start_options(&options)
            ),
        )
        .await?;
    state
        .push_event(
            "info",
            "wechat_thread_route_created",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(thread_id)
}

async fn send_thread_create_settings(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
) -> Result<()> {
    let route = route_for_message(message);
    let request_id = existing_request
        .as_ref()
        .map(|request| request.request_id.clone())
        .unwrap_or_else(next_thread_routing_request_id);
    let create_draft = existing_request
        .as_ref()
        .map(|request| request.create_draft.clone())
        .unwrap_or_default();
    let defaults = load_thread_create_defaults_for_client(state, &route.conversation_key).await;
    let mut text = thread_create_help_text(&defaults, &create_draft);
    text.push_str(
        "\n\n1. 修改目录\n2. 修改模型\n3. 修改推理强度\n4. 修改权限\n5. 创建会话\n6. 恢复历史会话\n\n回复数字选择。也可以回复 y 创建，n 取消。",
    );
    let message_id = adapter
        .send_text(state, &message.account_id, &message.chat_id, &text)
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(ThreadRoutingRequestState {
            request_id,
            conversation_key: route.conversation_key,
            account_id: route.account_id,
            chat_id: route.chat_id,
            message_id: Some(message_id),
            stage: ThreadRoutingStage::CreateSettings,
            page: 1,
            page_cursors: vec![None],
            thread_ids_by_page: vec![vec![]],
            create_draft,
            create_option_values_by_field_page: Default::default(),
            history_cursor: None,
            history_has_next: false,
        });
    Ok(())
}

async fn handle_thread_create_settings_text_reply(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    text: &str,
    command: Option<&str>,
) -> Result<bool> {
    let Some(request) =
        latest_thread_request_for_conversation(state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    if request.stage != ThreadRoutingStage::CreateSettings {
        return Ok(false);
    }

    let lowered = text.trim().to_ascii_lowercase();
    if matches!(
        lowered.as_str(),
        "y" | "yes" | "ok" | "确认" | "创建" | "开始" | "开始创建"
    ) {
        create_wechat_thread_from_request(state, adapter, message, request).await?;
        return Ok(true);
    }
    if matches!(lowered.as_str(), "n" | "no" | "取消" | "cancel") {
        state
            .runtime
            .lock()
            .await
            .clear_thread_routing_request(&request.request_id);
        adapter
            .send_text(
                state,
                &message.account_id,
                &message.chat_id,
                "已取消创建会话。",
            )
            .await?;
        return Ok(true);
    }

    match command.and_then(numeric_command_index) {
        Some(0) => {
            send_thread_create_options(state, adapter, message, request, "cwd", 1).await?;
            Ok(true)
        }
        Some(1) => {
            send_thread_create_options(state, adapter, message, request, "model", 1).await?;
            Ok(true)
        }
        Some(2) => {
            send_thread_create_options(state, adapter, message, request, "effort", 1).await?;
            Ok(true)
        }
        Some(3) => {
            send_thread_create_options(state, adapter, message, request, "perm", 1).await?;
            Ok(true)
        }
        Some(4) => {
            create_wechat_thread_from_request(state, adapter, message, request).await?;
            Ok(true)
        }
        Some(5) => {
            send_thread_routing_list(state, adapter, message, Some(request), None, 1).await?;
            Ok(true)
        }
        Some(_) => {
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    "请回复 1~6，或回复 y 创建、n 取消。",
                )
                .await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

async fn create_wechat_thread_from_request(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    request: ThreadRoutingRequestState,
) -> Result<()> {
    let options = match thread_start_options_from_form_for_client(
        state,
        &message.conversation_key(),
        thread_create_form_from_draft(&request.create_draft),
    )
    .await
    {
        Ok(options) => options,
        Err(err) => {
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    &format!("新建会话参数不正确：{err}"),
                )
                .await?;
            return Ok(());
        }
    };
    let route = route_for_message(message);
    create_wechat_thread_for_route(state, adapter, &route, options, Some(&request.request_id))
        .await?;
    Ok(())
}

async fn send_thread_create_options(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    mut request: ThreadRoutingRequestState,
    field: &str,
    page: usize,
) -> Result<()> {
    let Some(field) = normalize_thread_create_field(field) else {
        adapter
            .send_text(
                state,
                &message.account_id,
                &message.chat_id,
                "这个创建选项不可用，请重新打开创建设置。",
            )
            .await?;
        return Ok(());
    };
    let route = route_for_message(message);
    let defaults = load_thread_create_defaults_for_client(state, &route.conversation_key).await;
    let (title, body, mut options) =
        create_options_for_field(&defaults, &request.create_draft, field)?;
    if field == "cwd" {
        insert_custom_cwd_option(&mut options);
    }
    let total_pages = ((options.len() + WECHAT_CREATE_OPTION_PAGE_SIZE - 1)
        / WECHAT_CREATE_OPTION_PAGE_SIZE)
        .max(1);
    let page = page.clamp(1, total_pages);
    let start = (page - 1) * WECHAT_CREATE_OPTION_PAGE_SIZE;
    let end = (start + WECHAT_CREATE_OPTION_PAGE_SIZE).min(options.len());
    let page_options = options[start..end]
        .iter()
        .map(|(_, option)| option.clone())
        .collect::<Vec<_>>();
    let value_pages = options
        .chunks(WECHAT_CREATE_OPTION_PAGE_SIZE)
        .map(|chunk| {
            chunk
                .iter()
                .map(|(value, _)| value.clone())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    request.create_option_values_by_field_page.clear();
    request
        .create_option_values_by_field_page
        .insert(field.to_string(), value_pages);
    request.stage = ThreadRoutingStage::CreateOptions;
    request.page = page;

    let text = thread_create_options_text(
        &title,
        &body,
        &page_options,
        page,
        page > 1,
        page < total_pages,
    );
    let message_id = adapter
        .send_text(state, &message.account_id, &message.chat_id, &text)
        .await?;
    request.message_id = Some(message_id);
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request);
    Ok(())
}

async fn handle_thread_create_option_text_reply(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    text: &str,
    command: Option<&str>,
) -> Result<bool> {
    let Some(mut request) =
        latest_thread_request_for_conversation(state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    if request.stage != ThreadRoutingStage::CreateOptions {
        return Ok(false);
    }

    let Some(field) = request
        .create_option_values_by_field_page
        .keys()
        .next()
        .cloned()
    else {
        return Ok(false);
    };
    let lowered = text.trim().to_ascii_lowercase();
    if matches!(lowered.as_str(), "0" | "back" | "返回" | "返回设置") {
        send_thread_create_settings(state, adapter, message, Some(request)).await?;
        return Ok(true);
    }
    if matches!(lowered.as_str(), "取消" | "cancel") {
        state
            .runtime
            .lock()
            .await
            .clear_thread_routing_request(&request.request_id);
        adapter
            .send_text(
                state,
                &message.account_id,
                &message.chat_id,
                "已取消创建会话。",
            )
            .await?;
        return Ok(true);
    }
    if command == Some("/prev") {
        let page = request.page.saturating_sub(1).max(1);
        send_thread_create_options(state, adapter, message, request, &field, page).await?;
        return Ok(true);
    }
    if command == Some("/next") {
        let page = request.page.saturating_add(1);
        send_thread_create_options(state, adapter, message, request, &field, page).await?;
        return Ok(true);
    }

    let Some(index) = command.and_then(numeric_command_index) else {
        return Ok(false);
    };
    let page = request.page.max(1);
    let Some(value) = request
        .create_option_values_by_field_page
        .get(&field)
        .and_then(|pages| pages.get(page.saturating_sub(1)))
        .and_then(|values| values.get(index))
        .cloned()
    else {
        adapter
            .send_text(
                state,
                &message.account_id,
                &message.chat_id,
                "这个选项序号不可用，请按当前列表里的数字选择。",
            )
            .await?;
        return Ok(true);
    };

    apply_thread_create_draft_value(&mut request.create_draft, &field, &value)?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request.clone());
    if field == "cwd" && value == "__custom__" {
        send_thread_create_custom_cwd_prompt(state, adapter, message).await?;
        return Ok(true);
    }
    send_thread_create_settings(state, adapter, message, Some(request)).await?;
    Ok(true)
}

async fn handle_thread_create_custom_cwd_text_input(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    text: &str,
) -> Result<bool> {
    let Some(mut request) =
        pending_thread_create_custom_cwd_request(state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    let lowered = text.trim().to_ascii_lowercase();
    if matches!(lowered.as_str(), "n" | "no" | "取消" | "cancel") {
        request.create_draft.cwd_choice = None;
        request.create_draft.cwd_custom = None;
        state
            .runtime
            .lock()
            .await
            .remember_thread_routing_request(request.clone());
        send_thread_create_settings(state, adapter, message, Some(request)).await?;
        return Ok(true);
    }
    if command(text).is_some() {
        request.create_draft.cwd_choice = None;
        request.create_draft.cwd_custom = None;
        state
            .runtime
            .lock()
            .await
            .remember_thread_routing_request(request);
        return Ok(false);
    }

    let path = text.trim();
    if path.is_empty() {
        send_thread_create_custom_cwd_prompt(state, adapter, message).await?;
        return Ok(true);
    }
    if !expand_home_prefix(path).is_absolute() {
        adapter
            .send_text(
                state,
                &message.account_id,
                &message.chat_id,
                "项目目录需要是绝对路径。请重新发送绝对路径，或回复 n 取消。",
            )
            .await?;
        return Ok(true);
    }
    request.create_draft.cwd_choice = None;
    request.create_draft.cwd_custom = Some(path.to_string());
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request.clone());
    send_thread_create_settings(state, adapter, message, Some(request)).await?;
    Ok(true)
}

async fn send_thread_create_custom_cwd_prompt(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
) -> Result<()> {
    adapter
        .send_text(
            state,
            &message.account_id,
            &message.chat_id,
            "请发送项目目录的绝对路径。目录不存在时，创建会话时会自动创建。\n\n回复 n 取消。",
        )
        .await?;
    Ok(())
}

async fn pending_thread_create_custom_cwd_request(
    state: &SharedState,
    conversation_key: &str,
) -> Option<ThreadRoutingRequestState> {
    state
        .runtime
        .lock()
        .await
        .thread_routing_requests
        .values()
        .filter(|request| request.conversation_key == conversation_key)
        .filter(|request| request.stage == ThreadRoutingStage::CreateOptions)
        .filter(|request| {
            request.create_draft.cwd_choice.as_deref() == Some("__custom__")
                && request.create_draft.cwd_custom.is_none()
        })
        .max_by_key(|request| thread_routing_request_rank(&request.request_id))
        .cloned()
}

fn insert_custom_cwd_option(options: &mut Vec<(String, ThreadCreateOption)>) {
    if options.iter().any(|(value, _)| value == "__custom__") {
        return;
    }
    let custom = (
        "__custom__".to_string(),
        ThreadCreateOption {
            label: "自定义或新建目录".to_string(),
            summary: Some("选择后发送绝对路径。目录不存在时会自动创建。".to_string()),
        },
    );
    if options.is_empty() {
        options.push(custom);
    } else {
        options.insert(1, custom);
    }
}

fn thread_create_options_text(
    title: &str,
    body: &str,
    options: &[ThreadCreateOption],
    page: usize,
    has_prev: bool,
    has_next: bool,
) -> String {
    let mut lines = Vec::new();
    lines.push(format!("**{title}**"));
    lines.push(body.to_string());
    let mut hints = vec![format!("回复 1~{} 选择", options.len())];
    if has_prev {
        hints.push("**p** 上一页".to_string());
    }
    if has_next {
        hints.push("**n** 下一页".to_string());
    }
    hints.push("**0** 返回设置".to_string());
    let navigation_hint = format!("第 {} 页 · {}", page.max(1), hints.join("，"));
    lines.push("---".to_string());
    lines.push(String::new());
    for (index, option) in options.iter().enumerate() {
        lines.push(format!(
            "{}. **{}**",
            index + 1,
            truncate_line(option.label.trim(), 72)
        ));
        if let Some(summary) = option
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lines.push(format_option_summary(summary));
        }
        lines.push(String::new());
    }
    lines.push("---".to_string());
    lines.push(navigation_hint);
    lines.join("\n").trim_end().to_string()
}

fn format_option_summary(summary: &str) -> String {
    let summary = truncate_line(summary, 96);
    if looks_like_path(&summary) {
        format!("`{summary}`")
    } else {
        summary
    }
}

fn looks_like_path(value: &str) -> bool {
    let value = value.trim();
    value.contains('\\') || value.contains('/') || value.starts_with('~')
}

async fn send_thread_routing_choice(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
) -> Result<()> {
    let route = route_for_message(message);
    let request_id = next_thread_routing_request_id();
    let message_id = adapter
        .send_text(
            state,
            &message.account_id,
            &message.chat_id,
            "当前微信会话还没有接入 Codex 会话。\n\n1. 新建会话\n2. 恢复历史会话\n\n回复 1 或 2。",
        )
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(empty_thread_routing_request(
            &route, request_id, message_id,
        ));
    Ok(())
}

async fn handle_thread_route_choice_text_reply(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    let Some(request) =
        latest_thread_request_for_conversation(state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    if !is_thread_choice_request(&request) {
        return Ok(false);
    }

    match command {
        "/1" => {
            send_thread_create_settings(state, adapter, message, Some(request)).await?;
            Ok(true)
        }
        "/2" => {
            send_thread_routing_list(state, adapter, message, Some(request), None, 1).await?;
            Ok(true)
        }
        command if numeric_command_index(command).is_some() => {
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    "请回复 1 新建会话，或回复 2 恢复历史会话。",
                )
                .await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn send_thread_routing_list(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
    cursor: Option<&str>,
    page: usize,
) -> Result<()> {
    let route = route_for_message(message);
    let loaded_page = match load_thread_routing_page(
        state,
        &route,
        existing_request.as_ref(),
        cursor,
        page,
    )
    .await
    {
        Ok(page) => page,
        Err(err) => {
            state
                .push_event(
                    "error",
                    "wechat_thread_list_failed",
                    format!("conversation={} err={err}", route.conversation_key),
                )
                .await;
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    "会话列表加载失败：Codex App 暂时没有响应，请稍后重试。",
                )
                .await?;
            return Ok(());
        }
    };
    if loaded_page.entries.is_empty() {
        let message_id = adapter
            .send_text(
                state,
                &message.account_id,
                &message.chat_id,
                "当前没有可恢复的历史会话。\n\n回复 1 创建新会话。",
            )
            .await?;
        state
            .runtime
            .lock()
            .await
            .remember_thread_routing_request(empty_thread_routing_request(
                &route,
                loaded_page.request_id,
                message_id,
            ));
        return Ok(());
    }
    let text = thread_list_text(&loaded_page);
    let message_id = adapter
        .send_text(state, &message.account_id, &message.chat_id, &text)
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(loaded_page.into_request(
            &route,
            message_id,
            existing_request.as_ref(),
            cursor,
        ));
    Ok(())
}

async fn handle_thread_list_text_reply(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    let Some(request) =
        latest_thread_request_for_conversation(state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    if request.stage != ThreadRoutingStage::ResumeList {
        return Ok(false);
    }
    match command {
        "/next" => {
            if request.history_has_next {
                let next_page = request.page + 1;
                let cursor = request
                    .page_cursors
                    .get(request.page)
                    .and_then(|value| value.as_ref())
                    .cloned();
                send_thread_routing_list(
                    state,
                    adapter,
                    message,
                    Some(request),
                    cursor.as_deref(),
                    next_page,
                )
                .await?;
            } else {
                adapter
                    .send_text(
                        state,
                        &message.account_id,
                        &message.chat_id,
                        "已经是最后一页。",
                    )
                    .await?;
            }
            return Ok(true);
        }
        "/prev" => {
            if request.page > 1 {
                let previous_page = request.page - 1;
                let cursor = request
                    .page_cursors
                    .get(previous_page.saturating_sub(1))
                    .and_then(|value| value.as_ref())
                    .cloned();
                send_thread_routing_list(
                    state,
                    adapter,
                    message,
                    Some(request),
                    cursor.as_deref(),
                    previous_page,
                )
                .await?;
            } else {
                adapter
                    .send_text(
                        state,
                        &message.account_id,
                        &message.chat_id,
                        "已经是第一页。",
                    )
                    .await?;
            }
            return Ok(true);
        }
        _ => {}
    }
    let Some(index) = command
        .strip_prefix('/')
        .and_then(|value| value.parse::<usize>().ok())
    else {
        return Ok(false);
    };
    let Some(thread_id) = request
        .thread_ids_by_page
        .get(request.page.saturating_sub(1))
        .and_then(|threads| threads.get(index.saturating_sub(1)))
        .cloned()
    else {
        adapter
            .send_text(
                state,
                &message.account_id,
                &message.chat_id,
                "这个序号不在当前会话列表里，请按列表里的 1、2 选择。",
            )
            .await?;
        return Ok(true);
    };
    let route = route_for_message(message);
    let thread =
        resume_and_bind_thread(state, &route, &thread_id, Some(&request.request_id)).await?;
    adapter
        .send_text(
            state,
            &message.account_id,
            &message.chat_id,
            &format!(
                "已接入历史会话\n\n{}\n{}\n\n现在可以直接发送消息。",
                summarize_thread_title(&thread),
                summarize_thread_cwd(&thread)
            ),
        )
        .await?;
    state
        .push_event(
            "info",
            "wechat_thread_route_resumed",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(true)
}

async fn handle_wechat_approval_text_reply(
    state: &SharedState,
    adapter: &WechatAdapter,
    message: &InboundMessage,
    command: &str,
) -> Result<()> {
    match resolve_approval_reply(state, message, command).await {
        ApprovalReplyOutcome::Ready {
            pending, decision, ..
        } => {
            let next = submit_approval_decision(state, &pending, &decision).await?;
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    "审批决定已提交。",
                )
                .await?;
            if let Some((conversation_key, next_approval)) = next
                && let Some(route) =
                    crate::im_runtime::route_from_conversation_key(&conversation_key)
                && route.platform == crate::types::ImPlatformKind::Wechat
            {
                adapter
                    .send_approval(state, &route.account_id, &route.chat_id, &next_approval)
                    .await?;
            }
        }
        ApprovalReplyOutcome::NoPending => {
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    "当前没有待处理审批。",
                )
                .await?;
        }
        ApprovalReplyOutcome::NotCurrent => {
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    "这个审批请求已经不是当前待处理项。",
                )
                .await?;
        }
        ApprovalReplyOutcome::InvalidInput { hint } => {
            adapter
                .send_text(
                    state,
                    &message.account_id,
                    &message.chat_id,
                    &format!("审批回复无效，请回复 {hint}。"),
                )
                .await?;
        }
    }
    Ok(())
}

async fn latest_thread_request_for_conversation(
    state: &SharedState,
    conversation_key: &str,
) -> Option<ThreadRoutingRequestState> {
    state
        .runtime
        .lock()
        .await
        .thread_routing_requests
        .values()
        .filter(|request| request.conversation_key == conversation_key)
        .max_by_key(|request| thread_routing_request_rank(&request.request_id))
        .cloned()
}

fn is_thread_choice_request(request: &ThreadRoutingRequestState) -> bool {
    request.stage == ThreadRoutingStage::Choice
}

fn numeric_command_index(command: &str) -> Option<usize> {
    command
        .strip_prefix('/')
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(|value| value.checked_sub(1))
}

fn thread_routing_request_rank(request_id: &str) -> u64 {
    request_id
        .rsplit('-')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn thread_list_text(page: &crate::im::core::thread_list::ThreadRoutingPage) -> String {
    let mut lines = Vec::new();
    lines.push("**恢复历史会话**".to_string());
    if let Some(provider) = page.model_provider_filter.as_deref() {
        lines.push(format!("已按当前 Codex App provider `{provider}` 过滤。"));
    }
    lines.push("---".to_string());
    let mut actions = vec![format!("回复 1~{} 选择会话", page.entries.len())];
    if page.page > 1 {
        actions.push("**p** 上一页".to_string());
    }
    if page.next_cursor.is_some() {
        actions.push("**n** 下一页".to_string());
    }
    let navigation_hint = format!("第 {} 页 · {}", page.page.max(1), actions.join("，"));
    let mut current_cwd: Option<&str> = None;
    for (index, entry) in page.entries.iter().enumerate() {
        let cwd = entry
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if current_cwd != cwd {
            if !lines.last().is_some_and(|line| line.is_empty()) {
                lines.push(String::new());
            }
            lines.extend(thread_project_header_lines(cwd));
            current_cwd = cwd;
        }
        let state = thread_state_suffix(&entry.state);
        lines.push(format!(
            "{}. **{}**{}",
            index + 1,
            truncate_line(entry.title.trim(), 64),
            state.unwrap_or_default()
        ));
        lines.push(String::new());
    }
    if !lines.last().is_some_and(|line| line.is_empty()) {
        lines.push(String::new());
    }
    lines.push("---".to_string());
    lines.push(navigation_hint);
    lines.join("\n").trim_end().to_string()
}

fn thread_project_header_lines(cwd: Option<&str>) -> Vec<String> {
    match cwd {
        Some(cwd) => vec![
            format!("**项目：{}**", project_name(cwd)),
            format!("`{cwd}`"),
        ],
        None => vec!["**项目：未知目录**".to_string()],
    }
}

fn thread_state_suffix(state: &str) -> Option<&'static str> {
    if state.contains("当前会话") {
        Some(" · 当前")
    } else if state.contains("已加载") {
        Some(" · 已加载")
    } else {
        None
    }
}

fn project_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string())
}

fn command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?.trim();
    let lower = first.to_ascii_lowercase();
    lower.starts_with('/').then_some(lower)
}

fn menu_command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?.trim();
    if first
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .is_some()
    {
        return Some(format!("/{first}"));
    }
    let lower = first.to_ascii_lowercase();
    if lower.starts_with('/') {
        return Some(lower);
    }
    match lower.as_str() {
        "n" | "next" | "下一页" => Some("/next".to_string()),
        "p" | "prev" | "previous" | "上一页" => Some("/prev".to_string()),
        _ => None,
    }
}

fn truncate_line(text: &str, max_chars: usize) -> String {
    let text = text.replace('\r', " ").replace('\n', " ");
    if text.chars().count() <= max_chars {
        return text;
    }
    let mut output = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::{command, menu_command};

    #[test]
    fn command_keeps_bare_numbers_as_user_text() {
        assert_eq!(command("1"), None);
        assert_eq!(command("n"), None);
        assert_eq!(command("p"), None);
        assert_eq!(command("new"), None);
        assert_eq!(command("恢复历史"), None);
        assert_eq!(command("/n"), Some("/n".to_string()));
    }

    #[test]
    fn menu_command_accepts_wechat_text_menu_replies() {
        assert_eq!(menu_command("1"), Some("/1".to_string()));
        assert_eq!(menu_command(" 2 "), Some("/2".to_string()));
        assert_eq!(menu_command("n"), Some("/next".to_string()));
        assert_eq!(menu_command("p"), Some("/prev".to_string()));
        assert_eq!(menu_command("new"), None);
        assert_eq!(menu_command("恢复历史"), None);
    }
}

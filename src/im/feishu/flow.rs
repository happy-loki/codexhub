use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use tracing::info;

use crate::{
    app_state::SharedState,
    im::core::{
        approval::{ApprovalReplyOutcome, resolve_approval_reply, submit_approval_decision},
        i18n::im_text_for_state,
        routing::{active_turn_for_message, remote_client_key_for_thread, route_for_message},
        session::{create_and_bind_thread, resume_and_bind_thread},
        thread::{
            ThreadCreateForm, is_approval_reply, load_thread_create_defaults_for_client,
            next_thread_routing_request_id, summarize_thread_cwd, summarize_thread_start_options,
            summarize_thread_status, summarize_thread_title,
            thread_start_options_from_form_for_client, thread_start_options_with_current_provider,
        },
        thread_list::{empty_thread_routing_request, load_thread_routing_page},
        turn::{TurnStartOutcome, start_turn_for_route},
    },
    im::feishu::{FeishuAdapter, FeishuApi, renderer},
    im_runtime::{PendingApproval, RouteTarget, ThreadRoutingRequestState, TurnOrigin},
    remote_control_backend,
    types::{InboundAction, InboundMessage, ThreadRouteDirection},
};

pub(crate) async fn handle_inbound(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
) -> Result<()> {
    info!(
        "inbound feishu message chat={} sender={}",
        message.chat_id, message.sender_id
    );
    state
        .push_event(
            "info",
            "feishu_message",
            format!(
                "chat={} sender={} text_len={} attachments={}",
                message.chat_id,
                message.sender_id,
                message.text.chars().count(),
                message.attachments.len()
            ),
        )
        .await;

    let trimmed = message.text.trim();
    let route = route_for_message(&message);
    let text = im_text_for_state(&state);
    {
        let mut runtime = state.runtime.lock().await;
        runtime.last_route = Some(route.clone());
    }
    if let Some(action) = message.action.clone() {
        return handle_inbound_action(state, api, message, action).await;
    }
    if handle_control_message(&state, &api, &message, trimmed).await? {
        return Ok(());
    }
    if active_turn_for_message(&state, &message).await.is_some() {
        send_text_to_message(&api, &message, text.turn_busy_notice()).await?;
        return Ok(());
    }
    let remote_status = remote_control_backend::status_snapshot(&state).await;
    if !remote_status.connected {
        send_text_to_message(&api, &message, text.remote_not_connected()).await?;
        return Ok(());
    }

    match start_turn_for_route(
        &state,
        &route,
        trimmed,
        &message.attachments,
        message.received_at_ms,
        TurnOrigin::Feishu,
    )
    .await
    {
        TurnStartOutcome::Started { thread_id, turn_id } => {
            state
                .push_event(
                    "info",
                    "feishu_turn_started",
                    format!(
                        "chat={} thread={} turn={turn_id}",
                        message.chat_id, thread_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::Busy => {
            send_text_to_message(&api, &message, text.turn_busy_notice()).await?;
            Ok(())
        }
        TurnStartOutcome::Expired { thread_id } => {
            send_text_to_message(&api, &message, text.inbound_expired()).await?;
            state
                .push_event(
                    "warn",
                    "feishu_inbound_expired",
                    format!(
                        "chat={} thread={thread_id} message={}",
                        message.chat_id, message.message_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::NoThread => {
            send_thread_routing_choice_card(&state, &api, &message, None).await?;
            Ok(())
        }
        TurnStartOutcome::Stale { thread_id } => {
            state
                .push_event(
                    "warn",
                    "thread_route_stale",
                    format!(
                        "conversation={} thread={} during=turn/start",
                        route.conversation_key, thread_id
                    ),
                )
                .await;
            send_thread_routing_list(&state, &api, &message, None, None, 1).await
        }
        TurnStartOutcome::Failed { error } => {
            send_text_to_message(&api, &message, &text.app_message_failed(&error)).await?;
            Err(error)
        }
    }
}

async fn send_text_to_message(api: &FeishuApi, message: &InboundMessage, text: &str) -> Result<()> {
    FeishuAdapter::new(api.clone())
        .send_text(&message.chat_id, text)
        .await
}

async fn handle_control_message(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    let normalized = command.to_ascii_lowercase();
    let text = im_text_for_state(state);
    if is_approval_reply(&normalized) {
        return handle_approval_text_reply(state, api, message, command).await;
    }
    match normalized.as_str() {
        "/s" => {
            let Some((thread_id, turn_id)) = active_turn_for_message(state, message).await else {
                send_text_to_message(api, message, text.no_running_turn()).await?;
                return Ok(true);
            };
            let remote_client_key = remote_client_key_for_thread(state, &thread_id)
                .await
                .context("bound IM thread is missing remote client key")?;
            remote_control_backend::interrupt_turn_for_client(
                state,
                &remote_client_key,
                &thread_id,
                &turn_id,
            )
            .await?;
            remote_control_backend::clear_turn_for_client(
                state,
                &remote_client_key,
                Some(&turn_id),
            )
            .await;
            state
                .runtime
                .lock()
                .await
                .mark_turn_completed(&thread_id, Some(&turn_id));
            send_text_to_message(api, message, text.interrupted()).await?;
            return Ok(true);
        }
        "/q" => {
            if let Some((thread_id, turn_id)) = active_turn_for_message(state, message).await {
                let remote_client_key = remote_client_key_for_thread(state, &thread_id)
                    .await
                    .context("bound IM thread is missing remote client key")?;
                let _ = remote_control_backend::interrupt_turn_for_client(
                    state,
                    &remote_client_key,
                    &thread_id,
                    &turn_id,
                )
                .await;
                remote_control_backend::clear_thread_for_client(
                    state,
                    &remote_client_key,
                    Some(&thread_id),
                )
                .await;
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(&thread_id, Some(&turn_id));
            }
            {
                let mut runtime = state.runtime.lock().await;
                runtime.unbind_routes_for_conversation_with_reason(
                    &message.conversation_key(),
                    "feishu_quit_command",
                );
            }
            send_text_to_message(api, message, text.exited()).await?;
            return Ok(true);
        }
        other if other.starts_with('/') => {
            send_text_to_message(api, message, &text.unsupported_command(other)).await?;
            return Ok(true);
        }
        _ => {}
    }
    Ok(false)
}

pub(crate) async fn handle_inbound_action(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    action: InboundAction,
) -> Result<()> {
    match action {
        InboundAction::ApprovalDecision { .. } => {
            let text = im_text_for_state(&state);
            send_text_to_message(&api, &message, text.unsupported_approval_callback()).await?;
            Ok(())
        }
        InboundAction::ThreadRouteChoice { request_id, action } => {
            handle_thread_route_choice(state, api, message, &request_id, &action).await
        }
        InboundAction::ThreadRouteCreateSubmit {
            request_id,
            cwd_choice,
            cwd_custom,
            model,
            effort,
            permission,
        } => {
            handle_thread_route_create_submit(
                state,
                api,
                message,
                &request_id,
                ThreadCreateForm {
                    cwd_choice,
                    cwd_custom,
                    model,
                    effort,
                    permission,
                },
            )
            .await
        }
        InboundAction::ThreadRouteCreateDefault { request_id } => {
            handle_thread_route_create_default(state, api, message, &request_id).await
        }
        InboundAction::ThreadRouteCreateConfigured { .. }
        | InboundAction::ThreadRouteCreateEdit { .. }
        | InboundAction::ThreadRouteCreateSetIndex { .. }
        | InboundAction::ThreadRouteCreateSetValue { .. }
        | InboundAction::ThreadRouteCreateOptionsPage { .. } => {
            let text = im_text_for_state(&state);
            send_text_to_message(&api, &message, text.telegram_creation_action_only()).await?;
            Ok(())
        }
        InboundAction::ThreadRouteResumeSelected {
            request_id,
            thread_id,
        } => {
            handle_thread_route_resume_selected(state, api, message, &request_id, &thread_id).await
        }
        InboundAction::ThreadRouteResumeIndex {
            request_id,
            page,
            index,
        } => handle_thread_route_resume_index(state, api, message, &request_id, page, index).await,
        InboundAction::ThreadRouteListPage {
            request_id,
            direction,
        } => handle_thread_route_list_page(state, api, message, &request_id, direction).await,
    }
}

async fn handle_thread_route_choice(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    request_id: &str,
    action: &str,
) -> Result<()> {
    let request = {
        state
            .runtime
            .lock()
            .await
            .thread_routing_request(request_id)
    };
    let Some(request) = request else {
        let text = im_text_for_state(&state);
        send_text_to_message(&api, &message, text.thread_choice_card_expired()).await?;
        return Ok(());
    };
    if request.conversation_key != message.conversation_key() {
        let text = im_text_for_state(&state);
        send_text_to_message(&api, &message, text.thread_choice_not_current()).await?;
        return Ok(());
    }

    let card_message_id = request
        .message_id
        .clone()
        .or(message.card_message_id.clone());
    update_thread_routing_choice_card_selected(
        &state,
        &api,
        request_id,
        card_message_id.as_deref(),
        action,
    )
    .await;

    match action {
        "create_new" => send_thread_create_settings_card(&state, &api, &message, request).await,
        "resume_history" => {
            send_thread_routing_list(&state, &api, &message, Some(request), None, 1).await
        }
        "back" => send_thread_routing_choice_card(&state, &api, &message, Some(request)).await,
        other => {
            let text = im_text_for_state(&state);
            send_text_to_message(&api, &message, &text.unsupported_thread_action(other)).await?;
            Ok(())
        }
    }
}

async fn handle_thread_route_create_default(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    request_id: &str,
) -> Result<()> {
    let Some(request) = checked_thread_routing_request(&state, &api, &message, request_id).await?
    else {
        return Ok(());
    };
    create_new_thread_for_route(
        &state,
        &api,
        &message,
        request_id,
        request,
        thread_start_options_with_current_provider(
            remote_control_backend::ThreadStartOptions::default(),
        ),
    )
    .await
}

async fn handle_thread_route_create_submit(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    request_id: &str,
    form: ThreadCreateForm,
) -> Result<()> {
    let Some(request) = checked_thread_routing_request(&state, &api, &message, request_id).await?
    else {
        return Ok(());
    };
    let route = route_for_message(&message);
    let remote_client_key = route.remote_client_key.clone();
    let options =
        match thread_start_options_from_form_for_client(&state, &remote_client_key, form).await {
            Ok(options) => options,
            Err(err) => {
                let text = im_text_for_state(&state);
                send_text_to_message(&api, &message, &text.invalid_create_form(&err)).await?;
                return Ok(());
            }
        };
    create_new_thread_for_route(&state, &api, &message, request_id, request, options).await
}

async fn checked_thread_routing_request(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    request_id: &str,
) -> Result<Option<ThreadRoutingRequestState>> {
    let request = {
        state
            .runtime
            .lock()
            .await
            .thread_routing_request(request_id)
    };
    let Some(request) = request else {
        let text = im_text_for_state(state);
        send_text_to_message(api, message, text.thread_choice_card_expired()).await?;
        return Ok(None);
    };
    if request.conversation_key != message.conversation_key() {
        let text = im_text_for_state(state);
        send_text_to_message(api, message, text.thread_choice_not_current()).await?;
        return Ok(None);
    }
    Ok(Some(request))
}

async fn send_thread_create_settings_card(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    request: ThreadRoutingRequestState,
) -> Result<()> {
    let route = route_for_message(message);
    let remote_client_key = route.remote_client_key.clone();
    let defaults = load_thread_create_defaults_for_client(state, &remote_client_key).await;
    let text = im_text_for_state(state);
    let adapter = FeishuAdapter::new(api.clone());
    if let Some(message_id) = request
        .message_id
        .clone()
        .or_else(|| message.card_message_id.clone())
    {
        adapter
            .send_thread_create_settings(
                &message.chat_id,
                &request.request_id,
                &defaults,
                Some(&message_id),
                text,
            )
            .await?;
    } else {
        let message_id = adapter
            .send_thread_create_settings(
                &message.chat_id,
                &request.request_id,
                &defaults,
                None,
                text,
            )
            .await?;
        state
            .runtime
            .lock()
            .await
            .update_thread_routing_request_message_id(&request.request_id, message_id);
    }
    state
        .push_event(
            "info",
            "thread_create_settings_sent",
            format!("conversation={}", request.conversation_key),
        )
        .await;
    Ok(())
}

async fn create_new_thread_for_route(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    request_id: &str,
    request: ThreadRoutingRequestState,
    options: remote_control_backend::ThreadStartOptions,
) -> Result<()> {
    let card_message_id = request
        .message_id
        .clone()
        .or_else(|| message.card_message_id.clone());
    let adapter = FeishuAdapter::new(api.clone());
    let text = im_text_for_state(state);
    if let Some(message_id) = card_message_id.as_deref() {
        let _ = adapter
            .send_thread_routing_result(
                &message.chat_id,
                text.creating_session_title(),
                text.creating_new_thread(),
                Some(message_id),
            )
            .await;
    }

    let route = route_for_message(message);
    let thread_id =
        create_and_bind_thread(state, &route, options.clone(), Some(request_id)).await?;
    let body =
        text.created_new_session_body(&thread_id, &summarize_thread_start_options(&options, text));
    let _ = adapter
        .send_thread_routing_result(
            &route.chat_id,
            text.created_new_session_title(),
            &body,
            card_message_id.as_deref(),
        )
        .await?;
    state
        .push_event(
            "info",
            "thread_route_created",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(())
}

async fn handle_approval_text_reply(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    match resolve_approval_reply(state, message, command).await {
        ApprovalReplyOutcome::Ready {
            conversation_key,
            pending,
            option_index,
            decision,
        } => {
            respond_to_pending_approval(
                state,
                api,
                &message.chat_id,
                &conversation_key,
                pending,
                option_index,
                decision,
            )
            .await?;
        }
        ApprovalReplyOutcome::NoPending => {
            let text = im_text_for_state(state);
            send_text_to_message(api, message, text.no_pending_approval()).await?;
        }
        ApprovalReplyOutcome::NotCurrent => {
            let text = im_text_for_state(state);
            send_text_to_message(api, message, text.approval_not_current()).await?;
        }
        ApprovalReplyOutcome::InvalidInput { hint } => {
            let text = im_text_for_state(state);
            send_text_to_message(api, message, &text.invalid_approval_reply(&hint)).await?;
        }
    }
    Ok(true)
}

async fn handle_thread_route_resume_selected(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    request_id: &str,
    thread_id: &str,
) -> Result<()> {
    let request = {
        state
            .runtime
            .lock()
            .await
            .thread_routing_request(request_id)
    };
    let Some(request) = request else {
        let text = im_text_for_state(&state);
        send_text_to_message(&api, &message, text.thread_choice_card_expired()).await?;
        return Ok(());
    };
    if request.conversation_key != message.conversation_key() {
        let text = im_text_for_state(&state);
        send_text_to_message(&api, &message, text.thread_choice_not_current()).await?;
        return Ok(());
    }

    let card_message_id = request
        .message_id
        .clone()
        .or(message.card_message_id.clone());
    let adapter = FeishuAdapter::new(api.clone());
    let text = im_text_for_state(&state);
    if let Some(message_id) = card_message_id.as_deref() {
        let _ = adapter
            .send_thread_routing_result(
                &message.chat_id,
                text.subscribing_session_title(),
                &text.subscribing_thread(thread_id),
                Some(message_id),
            )
            .await;
    }

    let route = route_for_message(&message);
    let thread = resume_and_bind_thread(&state, &route, thread_id, Some(request_id)).await?;
    let body = text.subscribed_session_body(
        thread_id,
        &summarize_thread_title(&thread, text),
        &summarize_thread_cwd(&thread, text),
        &summarize_thread_status(&thread, text),
    );
    let _ = adapter
        .send_thread_routing_result(
            &route.chat_id,
            text.subscribed_session_title(),
            &body,
            card_message_id.as_deref(),
        )
        .await?;
    state
        .push_event(
            "info",
            "thread_route_resumed",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(())
}

async fn handle_thread_route_resume_index(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    request_id: &str,
    page: usize,
    index: usize,
) -> Result<()> {
    let request = {
        state
            .runtime
            .lock()
            .await
            .thread_routing_request(request_id)
    };
    let Some(request) = request else {
        let text = im_text_for_state(&state);
        send_text_to_message(&api, &message, text.thread_choice_card_expired()).await?;
        return Ok(());
    };
    if request.conversation_key != message.conversation_key() {
        let text = im_text_for_state(&state);
        send_text_to_message(&api, &message, text.thread_choice_not_current()).await?;
        return Ok(());
    }
    let Some(thread_id) = request
        .thread_ids_by_page
        .get(page.saturating_sub(1))
        .and_then(|thread_ids| thread_ids.get(index))
        .cloned()
    else {
        let text = im_text_for_state(&state);
        send_text_to_message(&api, &message, text.thread_selection_expired()).await?;
        return Ok(());
    };
    handle_thread_route_resume_selected(state, api, message, request_id, &thread_id).await
}

async fn handle_thread_route_list_page(
    state: SharedState,
    api: FeishuApi,
    message: InboundMessage,
    request_id: &str,
    direction: ThreadRouteDirection,
) -> Result<()> {
    let request = {
        state
            .runtime
            .lock()
            .await
            .thread_routing_request(request_id)
    };
    let Some(request) = request else {
        let text = im_text_for_state(&state);
        send_text_to_message(&api, &message, text.thread_choice_card_expired()).await?;
        return Ok(());
    };
    if request.conversation_key != message.conversation_key() {
        let text = im_text_for_state(&state);
        send_text_to_message(&api, &message, text.thread_list_not_current()).await?;
        return Ok(());
    }

    let target_page = match direction {
        ThreadRouteDirection::Prev => request.page.saturating_sub(1).max(1),
        ThreadRouteDirection::Next => request.page.saturating_add(1),
    };
    let cursor = request
        .page_cursors
        .get(target_page.saturating_sub(1))
        .cloned()
        .flatten();
    send_thread_routing_list(
        &state,
        &api,
        &message,
        Some(request),
        cursor.as_deref(),
        target_page,
    )
    .await
}

async fn respond_to_pending_approval(
    state: &SharedState,
    api: &FeishuApi,
    chat_id: &str,
    conversation_key: &str,
    pending: PendingApproval,
    option_index: usize,
    decision: crate::im_runtime::ApprovalDecisionOption,
) -> Result<()> {
    let next = submit_approval_decision(state, &pending, &decision).await?;
    update_resolved_approval_card(state, api, &pending, option_index, &decision.label).await;
    let _ = chat_id;
    state
        .push_event(
            "info",
            "approval_decision_sent",
            format!(
                "conversation={} request_id={} option={} label={}",
                conversation_key, pending.request_id, option_index, decision.label
            ),
        )
        .await;
    if let Some((conversation_key, next_approval)) = next {
        send_next_approval_card(state, api, &conversation_key, &next_approval).await?;
    }
    Ok(())
}

async fn send_thread_routing_list(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
    cursor: Option<&str>,
    page: usize,
) -> Result<()> {
    let route = route_for_message(message);
    let existing_message_id = existing_request
        .as_ref()
        .and_then(|request| request.message_id.as_deref());
    let adapter = FeishuAdapter::new(api.clone());
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
                    "thread_route_list_failed",
                    format!("conversation={} err={err}", route.conversation_key),
                )
                .await;
            let text = im_text_for_state(state);
            let _ = adapter
                .send_thread_routing_result(
                    &route.chat_id,
                    text.list_load_failed_title(),
                    text.list_load_failed(),
                    existing_message_id,
                )
                .await;
            return Ok(());
        }
    };
    let feishu_entries = loaded_page
        .entries
        .iter()
        .map(|entry| renderer::FeishuThreadListEntry {
            thread_id: entry.thread_id.clone(),
            title: entry.title.clone(),
            state: entry.state.clone(),
            cwd: entry.cwd.clone(),
        })
        .collect::<Vec<_>>();

    let text = im_text_for_state(state);
    let body = text.thread_list_body_feishu(loaded_page.model_provider_filter.as_deref());
    let message_id = adapter
        .send_thread_list(
            &route.chat_id,
            &loaded_page.request_id,
            text.thread_list_title_feishu(),
            &body,
            &feishu_entries,
            loaded_page.page,
            loaded_page.page > 1,
            loaded_page.next_cursor.is_some(),
            existing_message_id,
            text,
        )
        .await?;
    {
        let mut runtime = state.runtime.lock().await;
        runtime.remember_thread_routing_request(loaded_page.clone().into_request(
            &route,
            message_id.clone(),
            existing_request.as_ref(),
            cursor,
        ));
    }
    state
        .push_event(
            "info",
            "thread_route_list_sent",
            format!(
                "conversation={} page={page} entries={} provider={}",
                route.conversation_key,
                loaded_page.entries.len(),
                loaded_page.model_provider_filter.as_deref().unwrap_or("")
            ),
        )
        .await;
    Ok(())
}

async fn send_thread_routing_choice_card(
    state: &SharedState,
    api: &FeishuApi,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
) -> Result<()> {
    let route = route_for_message(message);
    let request_id = existing_request
        .as_ref()
        .map(|request| request.request_id.clone())
        .unwrap_or_else(next_thread_routing_request_id);
    let existing_message_id = existing_request
        .as_ref()
        .and_then(|request| request.message_id.as_deref());
    let adapter = FeishuAdapter::new(api.clone());
    let text = im_text_for_state(state);
    let message_id = adapter
        .send_thread_routing_choice(&route.chat_id, &request_id, existing_message_id, text)
        .await?;
    {
        let mut runtime = state.runtime.lock().await;
        runtime.remember_thread_routing_request(empty_thread_routing_request(
            &route, request_id, message_id,
        ));
    }
    state
        .push_event(
            "info",
            "thread_route_choice_sent",
            format!("conversation={}", route.conversation_key),
        )
        .await;
    Ok(())
}

async fn update_thread_routing_choice_card_selected(
    state: &SharedState,
    api: &FeishuApi,
    request_id: &str,
    message_id: Option<&str>,
    selected_action: &str,
) {
    let adapter = FeishuAdapter::new(api.clone());
    let _ = adapter
        .update_thread_routing_choice_selected(
            request_id,
            message_id,
            selected_action,
            im_text_for_state(state),
        )
        .await;
}

#[derive(Debug, Clone)]
struct UploadedImageForFeishu {
    image_key: String,
    local_path: PathBuf,
}

#[derive(Debug, Clone)]
struct DecodedImage {
    bytes: Vec<u8>,
    extension: &'static str,
}

pub(crate) async fn update_resolved_approval_card(
    state: &SharedState,
    api: &FeishuApi,
    pending: &PendingApproval,
    option_index: usize,
    decision_label: &str,
) {
    let adapter = FeishuAdapter::new(api.clone());
    let _ = adapter
        .update_resolved_approval(
            pending,
            option_index,
            decision_label,
            im_text_for_state(state),
        )
        .await;
}

pub(crate) async fn send_next_approval_card(
    state: &SharedState,
    api: &FeishuApi,
    conversation_key: &str,
    approval: &PendingApproval,
) -> Result<()> {
    let Some(route) = crate::im_runtime::route_from_conversation_key(conversation_key) else {
        state
            .push_event(
                "warn",
                "approval_next_route_missing",
                format!("conversation={conversation_key}"),
            )
            .await;
        return Ok(());
    };
    send_approval_card(state, api, &route, approval).await
}

pub(crate) async fn send_approval_card(
    state: &SharedState,
    api: &FeishuApi,
    route: &RouteTarget,
    approval: &PendingApproval,
) -> Result<()> {
    let adapter = FeishuAdapter::new(api.clone());
    let message_id = adapter
        .send_approval(&route.chat_id, approval, im_text_for_state(state))
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_approval_message_id(&approval.request_id, message_id.clone());
    state
        .push_event(
            "info",
            "approval_card_sent",
            format!(
                "conversation={} request_id={} message={}",
                route.conversation_key, approval.request_id, message_id
            ),
        )
        .await;
    Ok(())
}

pub(crate) async fn send_image_item_card(
    state: &SharedState,
    api: &FeishuApi,
    route: &RouteTarget,
    item_type: &str,
    item: &serde_json::Value,
    thread_id: &str,
    item_id: &str,
) -> bool {
    let result = async {
        let uploaded = image_for_feishu(state, api, item_type, item, item_id).await?;
        let card = match item_type {
            "imageGeneration" => renderer::build_image_generation_result_card(
                item.get("status")
                    .and_then(|value| value.as_str())
                    .unwrap_or("completed"),
                item.get("revisedPrompt")
                    .and_then(|value| value.as_str())
                    .or_else(|| item.get("revised_prompt").and_then(|value| value.as_str())),
                item.get("savedPath")
                    .and_then(|value| value.as_str())
                    .or_else(|| item.get("saved_path").and_then(|value| value.as_str()))
                    .or_else(|| uploaded.local_path.to_str()),
                &uploaded.image_key,
            ),
            "imageView" => {
                let path = item
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or_else(|| uploaded.local_path.to_str().unwrap_or(""));
                renderer::build_image_view_result_card(path, &uploaded.image_key)
            }
            _ => return Ok(false),
        };
        FeishuAdapter::new(api.clone())
            .send_interactive(&route.chat_id, &card)
            .await?;
        Ok::<bool, anyhow::Error>(true)
    }
    .await;

    match result {
        Ok(sent) => sent,
        Err(err) => {
            state
                .push_event(
                    "warn",
                    "feishu_image_item_failed",
                    format!(
                        "thread={thread_id} item={item_id} type={item_type} chat={} err={err}",
                        route.chat_id
                    ),
                )
                .await;
            false
        }
    }
}

async fn image_for_feishu(
    state: &SharedState,
    api: &FeishuApi,
    item_type: &str,
    item: &serde_json::Value,
    item_id: &str,
) -> Result<UploadedImageForFeishu> {
    let local_path = match item_type {
        "imageGeneration" => image_generation_local_path(state, item, item_id).await?,
        "imageView" => item
            .get("path")
            .and_then(|value| value.as_str())
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("imageView item missing path"))?,
        _ => return Err(anyhow!("unsupported image item type: {item_type}")),
    };
    if !local_path.is_file() {
        return Err(anyhow!("image file not found: {}", local_path.display()));
    }
    let image_key = api.upload_image(&local_path.to_string_lossy()).await?;
    Ok(UploadedImageForFeishu {
        image_key,
        local_path,
    })
}

async fn image_generation_local_path(
    state: &SharedState,
    item: &serde_json::Value,
    item_id: &str,
) -> Result<PathBuf> {
    if let Some(result) = item.get("result").and_then(|value| value.as_str())
        && let Some(decoded) = decode_image_string(result)
    {
        return write_decoded_image(state, item_id, decoded).await;
    }
    let saved_path = item
        .get("savedPath")
        .and_then(|value| value.as_str())
        .or_else(|| item.get("saved_path").and_then(|value| value.as_str()))
        .ok_or_else(|| anyhow!("imageGeneration item has no image result or savedPath"))?;
    Ok(PathBuf::from(saved_path))
}

async fn write_decoded_image(
    state: &SharedState,
    item_id: &str,
    decoded: DecodedImage,
) -> Result<PathBuf> {
    let state_path = state.config.lock().await.state_path.clone();
    let root = state_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".im")
        .join("images");
    std::fs::create_dir_all(&root)
        .with_context(|| format!("failed to create image cache {}", root.display()))?;
    let path = root.join(format!(
        "{}-{}.{}",
        crate::types::now_ms(),
        sanitize_file_stem(item_id),
        decoded.extension
    ));
    std::fs::write(&path, decoded.bytes)
        .with_context(|| format!("failed to write image cache {}", path.display()))?;
    Ok(path)
}

fn decode_image_string(value: &str) -> Option<DecodedImage> {
    let trimmed = value.trim();
    if let Some((mime, payload)) = parse_image_data_url(trimmed) {
        let bytes = decode_base64_payload(payload)?;
        let extension =
            image_extension_from_mime(mime).or_else(|| image_extension_from_bytes(&bytes))?;
        return Some(DecodedImage { bytes, extension });
    }
    if !looks_like_inline_image_base64(trimmed) {
        return None;
    }
    let bytes = decode_base64_payload(trimmed)?;
    let extension = image_extension_from_bytes(&bytes)?;
    Some(DecodedImage { bytes, extension })
}

fn parse_image_data_url(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix("data:")?;
    let (metadata, payload) = rest.split_once(',')?;
    let mut parts = metadata.split(';');
    let mime = parts.next()?.trim();
    if !mime.starts_with("image/") || !parts.any(|part| part == "base64") {
        return None;
    }
    Some((mime, payload))
}

fn decode_base64_payload(value: &str) -> Option<Vec<u8>> {
    let compact = value.split_whitespace().collect::<String>();
    base64::engine::general_purpose::STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(compact.as_bytes()))
        .ok()
}

fn looks_like_inline_image_base64(value: &str) -> bool {
    let compact = value.trim_start();
    compact.starts_with("iVBORw0KGgo")
        || compact.starts_with("/9j/")
        || compact.starts_with("R0lGOD")
        || compact.starts_with("UklGR")
}

fn image_extension_from_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn image_extension_from_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn sanitize_file_stem(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        }
        if out.len() >= 48 {
            break;
        }
    }
    if out.is_empty() {
        "image".to_string()
    } else {
        out
    }
}

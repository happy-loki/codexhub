use anyhow::{Context, Result};
use tracing::info;

use crate::{
    app_state::SharedState,
    im::core::{
        approval::{
            ApprovalReplyOutcome, resolve_approval_button_reply, resolve_approval_reply,
            submit_approval_decision,
        },
        i18n::im_text_for_state,
        outbound::ImOutboundSender,
        routing::{
            active_turn_for_message, clear_thread_binding, remote_client_key_for_thread,
            route_for_message,
        },
        session::{create_and_bind_thread, resume_and_bind_thread},
        thread::{
            ThreadCreateForm, apply_thread_create_draft_value, create_options_for_field,
            expand_home_prefix, is_approval_reply, load_thread_create_defaults_for_client,
            next_thread_routing_request_id, normalize_thread_create_field, summarize_thread_cwd,
            summarize_thread_start_options, summarize_thread_status, summarize_thread_title,
            thread_create_form_from_draft, thread_create_help_text,
            thread_start_options_from_form_for_client, thread_start_options_with_current_provider,
        },
        thread_list::{empty_thread_routing_request, load_thread_routing_page},
        turn::{TurnStartOutcome, start_turn_for_route},
    },
    im::events,
    im::telegram::{
        adapter::{TelegramAdapter, TelegramThreadListEntry},
        api::TelegramApi,
        types::TelegramSettings,
    },
    im_runtime::{ThreadRoutingRequestState, ThreadRoutingStage, TurnOrigin},
    remote_control_backend,
    types::{InboundAction, InboundMessage, ThreadRouteDirection},
};

const TELEGRAM_CREATE_OPTION_PAGE_SIZE: usize = 8;

#[derive(Debug, PartialEq, Eq)]
enum TelegramThreadRoutingResultDelivery {
    Updated,
    SentAsNew {
        message_id: String,
        update_error: String,
    },
    Undelivered {
        update_error: String,
        fallback_error: String,
    },
}

#[async_trait::async_trait]
trait TelegramThreadRoutingResultSender: Sync {
    async fn send_thread_routing_result(
        &self,
        target: &str,
        title: &str,
        body: &str,
        message_id: Option<&str>,
    ) -> Result<String>;
}

#[async_trait::async_trait]
impl TelegramThreadRoutingResultSender for TelegramAdapter {
    async fn send_thread_routing_result(
        &self,
        target: &str,
        title: &str,
        body: &str,
        message_id: Option<&str>,
    ) -> Result<String> {
        TelegramAdapter::send_thread_routing_result(self, target, title, body, message_id).await
    }
}

async fn deliver_telegram_thread_routing_result(
    sender: &(impl TelegramThreadRoutingResultSender + ?Sized),
    target: &str,
    title: &str,
    body: &str,
    progress_message_id: &str,
) -> TelegramThreadRoutingResultDelivery {
    let update_error = match sender
        .send_thread_routing_result(target, title, body, Some(progress_message_id))
        .await
    {
        Ok(_) => return TelegramThreadRoutingResultDelivery::Updated,
        Err(error) => error.to_string(),
    };

    match sender
        .send_thread_routing_result(target, title, body, None)
        .await
    {
        Ok(message_id) => TelegramThreadRoutingResultDelivery::SentAsNew {
            message_id,
            update_error,
        },
        Err(error) => TelegramThreadRoutingResultDelivery::Undelivered {
            update_error,
            fallback_error: error.to_string(),
        },
    }
}

async fn publish_telegram_thread_routing_result(
    state: &SharedState,
    adapter: &TelegramAdapter,
    chat_id: &str,
    conversation_key: &str,
    thread_id: &str,
    title: &str,
    body: &str,
    progress_message_id: &str,
    success_event_kind: &str,
) {
    let delivery =
        deliver_telegram_thread_routing_result(adapter, chat_id, title, body, progress_message_id)
            .await;
    match delivery {
        TelegramThreadRoutingResultDelivery::Updated => {}
        TelegramThreadRoutingResultDelivery::SentAsNew {
            message_id,
            update_error,
        } => {
            let event_kind = format!("{success_event_kind}_status_update_failed");
            state
                .push_event(
                    "warn",
                    &event_kind,
                    format!(
                        "conversation={conversation_key} thread={thread_id} message={progress_message_id} fallback=sent_as_new fallback_message={message_id} update_err={update_error}"
                    ),
                )
                .await;
        }
        TelegramThreadRoutingResultDelivery::Undelivered {
            update_error,
            fallback_error,
        } => {
            let update_event_kind = format!("{success_event_kind}_status_update_failed");
            state
                .push_event(
                    "warn",
                    &update_event_kind,
                    format!(
                        "conversation={conversation_key} thread={thread_id} message={progress_message_id} fallback=send_new_failed update_err={update_error}"
                    ),
                )
                .await;
            let fallback_event_kind = format!("{success_event_kind}_status_fallback_failed");
            state
                .push_event(
                    "warn",
                    &fallback_event_kind,
                    format!(
                        "conversation={conversation_key} thread={thread_id} fallback_err={fallback_error}"
                    ),
                )
                .await;
        }
    }
}

pub(crate) async fn handle_inbound(
    state: SharedState,
    outbound_tx: ImOutboundSender,
    message: InboundMessage,
) -> Result<()> {
    info!(
        "inbound telegram message chat={} sender={}",
        message.chat_id, message.sender_id
    );
    state
        .push_event(
            "info",
            "telegram_message",
            format!(
                "chat={} sender={} text_len={}",
                message.chat_id,
                message.sender_id,
                message.text.chars().count()
            ),
        )
        .await;

    let config = state.config.lock().await.clone();
    let telegram_config = config
        .telegram_account(&message.account_id)
        .unwrap_or_else(|| config.telegram.clone());
    let api = TelegramApi::new(TelegramSettings::from_app_config(&telegram_config));
    let adapter = TelegramAdapter::new(api.clone());
    let trimmed = message.text.trim();
    let route = route_for_message(&message);
    let text = im_text_for_state(&state);
    {
        let mut runtime = state.runtime.lock().await;
        runtime.last_route = Some(route.clone());
    }
    if let Some(action) = message.action.clone() {
        return handle_inbound_action(state, outbound_tx, adapter, message, action).await;
    }

    if handle_telegram_thread_create_text_input(&state, &adapter, &message, trimmed).await? {
        return Ok(());
    }

    let command = command(trimmed);
    if let Some(command) = command.as_deref()
        && handle_telegram_thread_create_option_text_reply(
            state.clone(),
            adapter.clone(),
            message.clone(),
            command,
        )
        .await?
    {
        return Ok(());
    }
    if let Some(command) = command.as_deref()
        && is_approval_reply(command)
        && state
            .runtime
            .lock()
            .await
            .has_pending_approvals(&message.conversation_key())
    {
        handle_telegram_approval_text_reply(&state, &outbound_tx, &adapter, &message, command)
            .await?;
        return Ok(());
    }
    if let Some(command) = command.as_deref()
        && handle_telegram_thread_list_text_reply(
            state.clone(),
            adapter.clone(),
            message.clone(),
            command,
        )
        .await?
    {
        return Ok(());
    }
    if let Some(command) = command.as_deref()
        && is_approval_reply(command)
    {
        handle_telegram_approval_text_reply(&state, &outbound_tx, &adapter, &message, command)
            .await?;
        return Ok(());
    }

    match command.as_deref() {
        Some("/s") => {
            let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await else {
                adapter
                    .send_text(&message.chat_id, text.no_running_turn())
                    .await?;
                return Ok(());
            };
            let remote_client_key = remote_client_key_for_thread(&state, &thread_id)
                .await
                .context("bound IM thread is missing remote client key")?;
            let claimed_terminal_notice = state
                .runtime
                .lock()
                .await
                .claim_terminal_notice(&turn_id, true);
            if let Err(err) = remote_control_backend::interrupt_turn_for_client(
                &state,
                &remote_client_key,
                &thread_id,
                &turn_id,
            )
            .await
            {
                if claimed_terminal_notice {
                    state.runtime.lock().await.release_terminal_notice(&turn_id);
                }
                return Err(err);
            }
            remote_control_backend::clear_turn_for_client(
                &state,
                &remote_client_key,
                Some(&turn_id),
            )
            .await;
            events::finish_telegram_command_progress_with_api(
                &state,
                api.clone(),
                &thread_id,
                &route,
                &turn_id,
            )
            .await;
            events::finish_telegram_collab_progress_with_api(
                &state,
                api.clone(),
                &thread_id,
                &route,
                &turn_id,
            )
            .await;
            state
                .runtime
                .lock()
                .await
                .mark_turn_completed(&thread_id, Some(&turn_id));
            adapter
                .send_text(&message.chat_id, text.interrupted())
                .await?;
            return Ok(());
        }
        Some("/q") => {
            if let Some((thread_id, turn_id)) = active_turn_for_message(&state, &message).await {
                state
                    .runtime
                    .lock()
                    .await
                    .claim_terminal_notice(&turn_id, true);
                let remote_client_key = remote_client_key_for_thread(&state, &thread_id)
                    .await
                    .context("bound IM thread is missing remote client key")?;
                let _ = remote_control_backend::interrupt_turn_for_client(
                    &state,
                    &remote_client_key,
                    &thread_id,
                    &turn_id,
                )
                .await;
                remote_control_backend::clear_thread_for_client(
                    &state,
                    &remote_client_key,
                    Some(&thread_id),
                )
                .await;
                events::finish_telegram_command_progress_with_api(
                    &state,
                    api.clone(),
                    &thread_id,
                    &route,
                    &turn_id,
                )
                .await;
                events::finish_telegram_collab_progress_with_api(
                    &state,
                    api.clone(),
                    &thread_id,
                    &route,
                    &turn_id,
                )
                .await;
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(&thread_id, Some(&turn_id));
            }
            clear_thread_binding(&state, &route.conversation_key).await?;
            adapter.send_text(&message.chat_id, text.exited()).await?;
            return Ok(());
        }
        Some(other) => {
            adapter
                .send_text(&message.chat_id, &text.unsupported_command(other))
                .await?;
            return Ok(());
        }
        None => {}
    }

    if active_turn_for_message(&state, &message).await.is_some() {
        if !message.attachments.is_empty() {
            let attachment_count = state.runtime.lock().await.hold_pending_attachments(
                &route.conversation_key,
                message.attachments.clone(),
                message.received_at_ms,
            );
            adapter
                .send_text(
                    &message.chat_id,
                    &text.turn_busy_attachments_held(attachment_count),
                )
                .await?;
            return Ok(());
        }
        adapter
            .send_text(&message.chat_id, text.turn_busy_notice())
            .await?;
        return Ok(());
    }

    let remote_status = remote_control_backend::status_snapshot(&state).await;
    if !remote_status.connected {
        adapter
            .send_text(&message.chat_id, text.remote_not_connected())
            .await?;
        return Ok(());
    }

    let only_images = trimmed.is_empty()
        && !message.attachments.is_empty()
        && message
            .attachments
            .iter()
            .all(|attachment| attachment.kind == "image");
    if only_images {
        let attachment_count = state.runtime.lock().await.hold_pending_attachments(
            &route.conversation_key,
            message.attachments.clone(),
            message.received_at_ms,
        );
        adapter
            .send_text(&message.chat_id, text.image_description_needed())
            .await?;
        state
            .push_event(
                "info",
                "telegram_image_waiting_for_description",
                format!(
                    "chat={} pending_attachments={attachment_count}",
                    message.chat_id
                ),
            )
            .await;
        return Ok(());
    }

    let pending_attachments = if trimmed.is_empty() {
        Vec::new()
    } else {
        state
            .runtime
            .lock()
            .await
            .take_pending_attachments(&route.conversation_key, message.received_at_ms)
    };
    let mut attachments = message.attachments.clone();
    attachments.extend(pending_attachments.clone());

    let outcome = start_turn_for_route(
        &state,
        &route,
        trimmed,
        &attachments,
        message.received_at_ms,
        TurnOrigin::Telegram,
    )
    .await;
    let attachments_to_restore = if matches!(&outcome, TurnStartOutcome::Started { .. }) {
        Vec::new()
    } else if matches!(&outcome, TurnStartOutcome::Expired { .. }) {
        pending_attachments
    } else {
        attachments
    };
    if !attachments_to_restore.is_empty() {
        state.runtime.lock().await.hold_pending_attachments(
            &route.conversation_key,
            attachments_to_restore,
            message.received_at_ms,
        );
    }

    match outcome {
        TurnStartOutcome::Started { thread_id, turn_id } => {
            state
                .push_event(
                    "info",
                    "telegram_turn_started",
                    format!(
                        "chat={} thread={} turn={turn_id}",
                        message.chat_id, thread_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::Busy => {
            adapter
                .send_text(&message.chat_id, text.turn_busy_notice())
                .await?;
            Ok(())
        }
        TurnStartOutcome::Expired { thread_id } => {
            adapter
                .send_text(&message.chat_id, text.inbound_expired())
                .await?;
            state
                .push_event(
                    "warn",
                    "telegram_inbound_expired",
                    format!(
                        "chat={} thread={thread_id} message={}",
                        message.chat_id, message.message_id
                    ),
                )
                .await;
            Ok(())
        }
        TurnStartOutcome::NoThread => {
            send_telegram_thread_routing_choice(&state, &adapter, &message, None).await?;
            Ok(())
        }
        TurnStartOutcome::Stale { thread_id } => {
            state
                .push_event(
                    "warn",
                    "telegram_thread_route_stale",
                    format!(
                        "conversation={} thread={} during=turn/start",
                        route.conversation_key, thread_id
                    ),
                )
                .await;
            adapter
                .send_text(&message.chat_id, text.stale_thread_unbound())
                .await?;
            send_telegram_thread_routing_choice(&state, &adapter, &message, None).await
        }
        TurnStartOutcome::Failed { error } => {
            adapter
                .send_text(&message.chat_id, &text.app_message_failed(&error))
                .await?;
            Err(error)
        }
    }
}

async fn create_telegram_thread_for_route(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    options: remote_control_backend::ThreadStartOptions,
    request: ThreadRoutingRequestState,
) -> Result<String> {
    let text = im_text_for_state(state);
    let route = route_for_message(message);
    let progress_message_id = adapter
        .send_or_update_text(
            &route.chat_id,
            request.message_id.as_deref(),
            text.creating_new_thread(),
        )
        .await?;
    let mut retry_request = request;
    retry_request.message_id = Some(progress_message_id.clone());
    let thread_id = match create_and_bind_thread(
        state,
        &route,
        options.clone(),
        Some(&retry_request.request_id),
    )
    .await
    {
        Ok(thread_id) => thread_id,
        Err(error) => {
            if retry_request.stage == ThreadRoutingStage::Choice {
                send_telegram_thread_routing_choice(state, adapter, message, Some(retry_request))
                    .await?;
            } else {
                send_telegram_thread_create_settings(state, adapter, message, Some(retry_request))
                    .await?;
            }
            adapter
                .send_text(&route.chat_id, &text.app_message_failed(&error))
                .await?;
            return Err(error);
        }
    };
    publish_telegram_thread_routing_result(
        state,
        adapter,
        &route.chat_id,
        &route.conversation_key,
        &thread_id,
        text.created_new_session_title(),
        &text.created_new_session_body(&thread_id, &summarize_thread_start_options(&options, text)),
        &progress_message_id,
        "telegram_thread_route_created",
    )
    .await;
    state
        .push_event(
            "info",
            "telegram_thread_route_created",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(thread_id)
}

pub(crate) async fn handle_inbound_action(
    state: SharedState,
    outbound_tx: ImOutboundSender,
    adapter: TelegramAdapter,
    message: InboundMessage,
    action: InboundAction,
) -> Result<()> {
    match action {
        InboundAction::ThreadRouteOpen => Ok(()),
        InboundAction::ApprovalDecision {
            request_fingerprint,
            option_index,
        } => {
            handle_telegram_approval_button_reply(
                &state,
                &outbound_tx,
                &adapter,
                &message,
                &request_fingerprint,
                option_index,
            )
            .await?;
            Ok(())
        }
        InboundAction::ThreadRouteChoice { request_id, action } => {
            handle_telegram_thread_route_choice(state, adapter, message, &request_id, &action).await
        }
        InboundAction::ThreadRouteCreateSubmit {
            request_id,
            cwd_choice,
            cwd_custom,
            model,
            effort,
            permission,
        } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let route = route_for_message(&message);
            let remote_client_key = route.remote_client_key.clone();
            let options = match thread_start_options_from_form_for_client(
                &state,
                &remote_client_key,
                ThreadCreateForm {
                    cwd_choice,
                    cwd_custom,
                    model,
                    effort,
                    permission,
                },
            )
            .await
            {
                Ok(options) => options,
                Err(err) => {
                    let text = im_text_for_state(&state);
                    adapter
                        .send_text(&message.chat_id, &text.invalid_create_form(&err))
                        .await?;
                    return Ok(());
                }
            };
            create_telegram_thread_for_route(&state, &adapter, &message, options, request).await?;
            Ok(())
        }
        InboundAction::ThreadRouteCreateDefault { request_id } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let options = thread_start_options_with_current_provider(
                remote_control_backend::ThreadStartOptions::default(),
            );
            create_telegram_thread_for_route(&state, &adapter, &message, options, request).await?;
            Ok(())
        }
        InboundAction::ThreadRouteCreateConfigured { request_id } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let route = route_for_message(&message);
            let remote_client_key = route.remote_client_key.clone();
            let options = match thread_start_options_from_form_for_client(
                &state,
                &remote_client_key,
                thread_create_form_from_draft(&request.create_draft),
            )
            .await
            {
                Ok(options) => options,
                Err(err) => {
                    let text = im_text_for_state(&state);
                    adapter
                        .send_text(&message.chat_id, &text.invalid_create_form(&err))
                        .await?;
                    return Ok(());
                }
            };
            create_telegram_thread_for_route(&state, &adapter, &message, options, request).await?;
            Ok(())
        }
        InboundAction::ThreadRouteCreateEdit { request_id, field } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            send_telegram_thread_create_options(&state, &adapter, &message, request, &field, 1)
                .await
        }
        InboundAction::ThreadRouteCreateSetIndex {
            request_id,
            field,
            page,
            index,
        } => {
            let Some(mut request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let Some(field) = normalize_thread_create_field(&field) else {
                let text = im_text_for_state(&state);
                adapter
                    .send_text(&message.chat_id, text.create_option_unavailable())
                    .await?;
                return Ok(());
            };
            let Some(value) = request
                .create_option_values_by_field_page
                .get(field)
                .and_then(|pages| pages.get(page.saturating_sub(1)))
                .and_then(|values| values.get(index))
                .cloned()
            else {
                let text = im_text_for_state(&state);
                adapter
                    .send_text(&message.chat_id, text.create_option_expired())
                    .await?;
                return Ok(());
            };
            apply_thread_create_draft_value(&mut request.create_draft, field, &value)?;
            state
                .runtime
                .lock()
                .await
                .remember_thread_routing_request(request.clone());
            if field == "cwd" && value == "__custom__" {
                send_telegram_thread_create_custom_cwd_prompt(&state, &adapter, &message).await?;
                return Ok(());
            }
            send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await
        }
        InboundAction::ThreadRouteCreateSetValue {
            request_id,
            field,
            value,
        } => {
            let Some(mut request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let Some(field) = normalize_thread_create_field(&field) else {
                let text = im_text_for_state(&state);
                adapter
                    .send_text(&message.chat_id, text.create_option_unavailable())
                    .await?;
                return Ok(());
            };
            apply_thread_create_draft_value(&mut request.create_draft, field, &value)?;
            state
                .runtime
                .lock()
                .await
                .remember_thread_routing_request(request.clone());
            if field == "cwd" && value == "__custom__" {
                send_telegram_thread_create_custom_cwd_prompt(&state, &adapter, &message).await?;
                return Ok(());
            }
            send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await
        }
        InboundAction::ThreadRouteCreateOptionsPage {
            request_id,
            field,
            direction,
        } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let current_page = request.page.max(1);
            let target_page = match direction {
                ThreadRouteDirection::Prev => current_page.saturating_sub(1).max(1),
                ThreadRouteDirection::Next => current_page.saturating_add(1),
            };
            send_telegram_thread_create_options(
                &state,
                &adapter,
                &message,
                request,
                &field,
                target_page,
            )
            .await
        }
        InboundAction::ThreadRouteResumeSelected {
            request_id,
            thread_id,
        } => {
            handle_telegram_thread_route_resume_selected(
                state,
                adapter,
                message,
                &request_id,
                &thread_id,
            )
            .await
        }
        InboundAction::ThreadRouteResumeIndex {
            request_id,
            page,
            index,
        } => {
            let Some(request) =
                checked_telegram_thread_routing_request(&state, &adapter, &message, &request_id)
                    .await?
            else {
                return Ok(());
            };
            let Some(thread_id) = request
                .thread_ids_by_page
                .get(page.saturating_sub(1))
                .and_then(|thread_ids| thread_ids.get(index))
                .cloned()
            else {
                let text = im_text_for_state(&state);
                adapter
                    .send_text(&message.chat_id, text.thread_selection_expired())
                    .await?;
                return Ok(());
            };
            handle_telegram_thread_route_resume_selected(
                state,
                adapter,
                message,
                &request_id,
                &thread_id,
            )
            .await
        }
        InboundAction::ThreadRouteListPage {
            request_id,
            direction,
        } => {
            handle_telegram_thread_route_list_page(state, adapter, message, &request_id, direction)
                .await
        }
    }
}

async fn handle_telegram_thread_route_choice(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    action: &str,
) -> Result<()> {
    let request =
        checked_telegram_thread_routing_request(&state, &adapter, &message, request_id).await?;
    let Some(request) = request else {
        return Ok(());
    };

    match action {
        "create_new" => {
            send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await
        }
        "resume_history" => {
            send_telegram_thread_routing_list(&state, &adapter, &message, Some(request), None, 1)
                .await
        }
        "back" => {
            send_telegram_thread_routing_choice(&state, &adapter, &message, Some(request)).await
        }
        other => {
            let text = im_text_for_state(&state);
            adapter
                .send_text(&message.chat_id, &text.unsupported_thread_action(other))
                .await?;
            Ok(())
        }
    }
}

async fn checked_telegram_thread_routing_request(
    state: &SharedState,
    adapter: &TelegramAdapter,
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
        adapter
            .send_text(&message.chat_id, text.thread_operation_expired())
            .await?;
        return Ok(None);
    };
    if request.conversation_key != message.conversation_key() {
        let text = im_text_for_state(state);
        adapter
            .send_text(&message.chat_id, text.thread_choice_not_current())
            .await?;
        return Ok(None);
    }
    if !callback_targets_current_message(
        request.message_id.as_deref(),
        message.card_message_id.as_deref(),
    ) {
        if let Some(callback_message_id) = message.card_message_id.as_deref() {
            let _ = adapter
                .clear_reply_markup(&message.chat_id, Some(callback_message_id))
                .await;
        }
        let text = im_text_for_state(state);
        adapter
            .send_text(&message.chat_id, text.thread_choice_not_current())
            .await?;
        return Ok(None);
    }
    Ok(Some(request))
}

fn callback_targets_current_message(
    expected_message_id: Option<&str>,
    callback_message_id: Option<&str>,
) -> bool {
    !matches!(
        (expected_message_id, callback_message_id),
        (Some(expected), Some(actual)) if expected != actual
    )
}

async fn handle_telegram_thread_list_text_reply(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    command: &str,
) -> Result<bool> {
    let Some(index) = numeric_command_index(command) else {
        return Ok(false);
    };
    let Some(request) =
        pending_telegram_thread_list_request(&state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    let page = request.page.max(1);
    let Some(thread_id) = request
        .thread_ids_by_page
        .get(page.saturating_sub(1))
        .and_then(|thread_ids| thread_ids.get(index))
        .cloned()
    else {
        let text = im_text_for_state(&state);
        adapter
            .send_text(&message.chat_id, text.invalid_thread_index_restart())
            .await?;
        return Ok(true);
    };
    handle_telegram_thread_route_resume_selected(
        state,
        adapter,
        message,
        &request.request_id,
        &thread_id,
    )
    .await?;
    Ok(true)
}

async fn handle_telegram_thread_create_option_text_reply(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    command: &str,
) -> Result<bool> {
    let Some(index) = numeric_command_index(command) else {
        return Ok(false);
    };
    let Some(mut request) =
        pending_telegram_thread_create_options_request(&state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    let page = request.page.max(1);
    let Some((field, value)) =
        request
            .create_option_values_by_field_page
            .iter()
            .find_map(|(field, pages)| {
                pages
                    .get(page.saturating_sub(1))
                    .and_then(|values| values.get(index))
                    .cloned()
                    .map(|value| (field.clone(), value))
            })
    else {
        let text = im_text_for_state(&state);
        adapter
            .send_text(&message.chat_id, text.create_option_unavailable())
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
        send_telegram_thread_create_custom_cwd_prompt(&state, &adapter, &message).await?;
        return Ok(true);
    }
    send_telegram_thread_create_settings(&state, &adapter, &message, Some(request)).await?;
    Ok(true)
}

async fn pending_telegram_thread_create_options_request(
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
        .filter(|request| !request.create_option_values_by_field_page.is_empty())
        .max_by_key(|request| thread_routing_request_rank(&request.request_id))
        .cloned()
}

async fn pending_telegram_thread_list_request(
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
        .filter(|request| {
            request
                .thread_ids_by_page
                .get(request.page.saturating_sub(1))
                .is_some_and(|thread_ids| !thread_ids.is_empty())
        })
        .max_by_key(|request| thread_routing_request_rank(&request.request_id))
        .cloned()
}

fn thread_routing_request_rank(request_id: &str) -> u64 {
    request_id
        .strip_prefix("thread-route-")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or_default()
}

async fn handle_telegram_thread_create_text_input(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    text: &str,
) -> Result<bool> {
    let Some(mut request) =
        pending_telegram_thread_create_custom_cwd_request(state, &message.conversation_key()).await
    else {
        return Ok(false);
    };
    if text.eq_ignore_ascii_case("/cancel") {
        request.create_draft.cwd_choice = None;
        request.create_draft.cwd_custom = None;
        state
            .runtime
            .lock()
            .await
            .remember_thread_routing_request(request.clone());
        send_telegram_thread_create_settings(state, adapter, message, Some(request)).await?;
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
        send_telegram_thread_create_custom_cwd_prompt(state, adapter, message).await?;
        return Ok(true);
    }
    if !expand_home_prefix(path).is_absolute() {
        let text = im_text_for_state(state);
        adapter
            .send_text(&message.chat_id, text.cwd_must_be_absolute_telegram())
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
    send_telegram_thread_create_settings(state, adapter, message, Some(request)).await?;
    Ok(true)
}

async fn pending_telegram_thread_create_custom_cwd_request(
    state: &SharedState,
    conversation_key: &str,
) -> Option<ThreadRoutingRequestState> {
    state
        .runtime
        .lock()
        .await
        .thread_routing_requests
        .values()
        .find(|request| {
            request.conversation_key == conversation_key
                && request.create_draft.cwd_choice.as_deref() == Some("__custom__")
                && request.create_draft.cwd_custom.is_none()
        })
        .cloned()
}

async fn send_telegram_thread_create_custom_cwd_prompt(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
) -> Result<()> {
    let text = im_text_for_state(state);
    let pending =
        pending_telegram_thread_create_custom_cwd_request(state, &message.conversation_key()).await;
    let prompt_message_id = adapter
        .send_or_update_text(
            &message.chat_id,
            pending
                .as_ref()
                .and_then(|request| request.message_id.as_deref()),
            text.custom_cwd_prompt_telegram(),
        )
        .await?;
    if let Some(request) = pending {
        state
            .runtime
            .lock()
            .await
            .update_thread_routing_request_message_id(&request.request_id, prompt_message_id);
    }
    Ok(())
}

async fn send_telegram_thread_routing_choice(
    state: &SharedState,
    adapter: &TelegramAdapter,
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
    let text = im_text_for_state(state);
    let message_id = adapter
        .send_thread_routing_choice(&route.chat_id, &request_id, existing_message_id, text)
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

async fn send_telegram_thread_create_settings(
    state: &SharedState,
    adapter: &TelegramAdapter,
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
    let remote_client_key = route.remote_client_key.clone();
    let defaults = load_thread_create_defaults_for_client(state, &remote_client_key).await;
    let im_text = im_text_for_state(state);
    let text = thread_create_help_text(&defaults, &create_draft, im_text);
    let existing_message_id = existing_request
        .as_ref()
        .and_then(|request| request.message_id.as_deref());
    let message_id = adapter
        .send_thread_create_settings(
            &route.chat_id,
            &request_id,
            &text,
            existing_message_id,
            im_text,
        )
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(ThreadRoutingRequestState {
            request_id: request_id.clone(),
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

async fn send_telegram_thread_create_options(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    mut request: ThreadRoutingRequestState,
    field: &str,
    page: usize,
) -> Result<()> {
    let Some(field) = normalize_thread_create_field(field) else {
        let text = im_text_for_state(state);
        adapter
            .send_text(&message.chat_id, text.create_option_unavailable())
            .await?;
        return Ok(());
    };
    let remote_client_key = route_for_message(message).remote_client_key;
    let defaults = load_thread_create_defaults_for_client(state, &remote_client_key).await;
    let text = im_text_for_state(state);
    let (title, body, options) =
        create_options_for_field(&defaults, &request.create_draft, field, text)?;
    let total_pages = ((options.len() + TELEGRAM_CREATE_OPTION_PAGE_SIZE - 1)
        / TELEGRAM_CREATE_OPTION_PAGE_SIZE)
        .max(1);
    let page = page.clamp(1, total_pages);
    let start = (page - 1) * TELEGRAM_CREATE_OPTION_PAGE_SIZE;
    let end = (start + TELEGRAM_CREATE_OPTION_PAGE_SIZE).min(options.len());
    let page_options = options[start..end]
        .iter()
        .map(|(_, option)| option.clone())
        .collect::<Vec<_>>();
    let value_pages = options
        .chunks(TELEGRAM_CREATE_OPTION_PAGE_SIZE)
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
    let message_id = adapter
        .send_thread_create_options(
            &request.chat_id,
            &request.request_id,
            field,
            &title,
            &body,
            &page_options,
            page,
            page > 1,
            page < total_pages,
            request.message_id.as_deref(),
            text,
        )
        .await?;
    request.message_id = Some(message_id);
    state
        .runtime
        .lock()
        .await
        .remember_thread_routing_request(request.clone());
    state
        .push_event(
            "info",
            "telegram_thread_create_options_sent",
            format!(
                "conversation={} field={} page={page} options={}",
                request.conversation_key,
                field,
                options.len()
            ),
        )
        .await;
    Ok(())
}

async fn send_telegram_thread_routing_list(
    state: &SharedState,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    existing_request: Option<ThreadRoutingRequestState>,
    cursor: Option<&str>,
    page: usize,
) -> Result<()> {
    let route = route_for_message(message);
    let existing_message_id = existing_request
        .as_ref()
        .and_then(|request| request.message_id.as_deref());
    let loaded_page =
        match load_thread_routing_page(state, &route, existing_request.as_ref(), cursor, page, 8)
            .await
        {
            Ok(page) => page,
            Err(err) => {
                state
                    .push_event(
                        "error",
                        "telegram_thread_list_failed",
                        format!("conversation={} err={err}", route.conversation_key),
                    )
                    .await;
                adapter
                    .send_or_update_text(
                        &route.chat_id,
                        existing_message_id,
                        im_text_for_state(state).list_load_failed(),
                    )
                    .await?;
                return Ok(());
            }
        };
    let telegram_entries = loaded_page
        .entries
        .iter()
        .map(|entry| TelegramThreadListEntry {
            title: entry.title.clone(),
            state: entry.state.clone(),
            cwd: entry.cwd.clone(),
        })
        .collect::<Vec<_>>();

    let text = im_text_for_state(state);
    let body = text.thread_list_body_telegram(loaded_page.model_provider_filter.as_deref());
    let message_id = adapter
        .send_thread_list(
            &route.chat_id,
            &loaded_page.request_id,
            text.thread_list_title_telegram(),
            &body,
            &telegram_entries,
            loaded_page.page,
            loaded_page.page > 1,
            loaded_page.next_cursor.is_some(),
            existing_message_id,
            text,
        )
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

async fn handle_telegram_thread_route_list_page(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    direction: ThreadRouteDirection,
) -> Result<()> {
    let Some(request) =
        checked_telegram_thread_routing_request(&state, &adapter, &message, request_id).await?
    else {
        return Ok(());
    };
    let target_page = match direction {
        ThreadRouteDirection::Prev => request.page.saturating_sub(1).max(1),
        ThreadRouteDirection::Next => request.page.saturating_add(1),
    };
    let cursor = request
        .page_cursors
        .get(target_page.saturating_sub(1))
        .cloned()
        .flatten();
    send_telegram_thread_routing_list(
        &state,
        &adapter,
        &message,
        Some(request),
        cursor.as_deref(),
        target_page,
    )
    .await
}

async fn handle_telegram_thread_route_resume_selected(
    state: SharedState,
    adapter: TelegramAdapter,
    message: InboundMessage,
    request_id: &str,
    thread_id: &str,
) -> Result<()> {
    let Some(request) =
        checked_telegram_thread_routing_request(&state, &adapter, &message, request_id).await?
    else {
        return Ok(());
    };
    let text = im_text_for_state(&state);
    let progress_message_id = adapter
        .send_or_update_text(
            &message.chat_id,
            request.message_id.as_deref(),
            &text.subscribing_thread(thread_id),
        )
        .await?;
    let route = route_for_message(&message);
    let thread = match resume_and_bind_thread(&state, &route, thread_id, Some(request_id)).await {
        Ok(thread) => thread,
        Err(error) => {
            let mut retry_request = request;
            retry_request.message_id = Some(progress_message_id);
            send_telegram_thread_routing_choice(&state, &adapter, &message, Some(retry_request))
                .await?;
            adapter
                .send_text(&message.chat_id, &text.app_message_failed(&error))
                .await?;
            return Err(error);
        }
    };
    let body = text.subscribed_session_body(
        thread_id,
        &summarize_thread_title(&thread, text),
        &summarize_thread_cwd(&thread, text),
        &summarize_thread_status(&thread, text),
    );
    publish_telegram_thread_routing_result(
        &state,
        &adapter,
        &route.chat_id,
        &route.conversation_key,
        thread_id,
        text.subscribed_session_title(),
        &body,
        &progress_message_id,
        "telegram_thread_route_resumed",
    )
    .await;
    state
        .push_event(
            "info",
            "telegram_thread_route_resumed",
            format!("conversation={} thread={thread_id}", route.conversation_key),
        )
        .await;
    Ok(())
}

async fn handle_telegram_approval_text_reply(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    command: &str,
) -> Result<bool> {
    handle_telegram_approval_outcome(
        state,
        outbound_tx,
        adapter,
        message,
        resolve_approval_reply(state, message, command).await,
    )
    .await
}

async fn handle_telegram_approval_button_reply(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    request_fingerprint: &str,
    option_index: usize,
) -> Result<bool> {
    handle_telegram_approval_outcome(
        state,
        outbound_tx,
        adapter,
        message,
        resolve_approval_button_reply(state, message, request_fingerprint, option_index).await,
    )
    .await
}

async fn handle_telegram_approval_outcome(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    adapter: &TelegramAdapter,
    message: &InboundMessage,
    outcome: ApprovalReplyOutcome,
) -> Result<bool> {
    match outcome {
        ApprovalReplyOutcome::Ready {
            conversation_key,
            pending,
            option_index,
            decision,
        } => {
            let approval_message_id = pending
                .message_id
                .clone()
                .or_else(|| message.card_message_id.clone());
            let next = submit_approval_decision(state, &pending, &decision).await?;
            let resolved = adapter
                .update_resolved_approval(
                    &message.chat_id,
                    approval_message_id.as_deref(),
                    &pending,
                    option_index,
                    &decision.label,
                    im_text_for_state(state),
                )
                .await;
            let update_succeeded = match resolved {
                Ok(updated) => updated,
                Err(err) => {
                    state
                        .push_event(
                            "warn",
                            "telegram_approval_update_failed",
                            format!(
                                "conversation={} request_id={} message={} err={err}",
                                conversation_key,
                                pending.request_id,
                                approval_message_id.as_deref().unwrap_or("")
                            ),
                        )
                        .await;
                    false
                }
            };
            if !update_succeeded {
                adapter
                    .send_text(
                        &message.chat_id,
                        &im_text_for_state(state)
                            .approval_decision_submitted_label(&decision.label),
                    )
                    .await?;
            }
            state
                .push_event(
                    "info",
                    "telegram_approval_decision_sent",
                    format!(
                        "conversation={} request_id={} option={} label={}",
                        conversation_key, pending.request_id, option_index, decision.label
                    ),
                )
                .await;
            if let Some((conversation_key, next_approval)) = next {
                events::send_next_approval(state, outbound_tx, &conversation_key, &next_approval)
                    .await?;
            }
        }
        ApprovalReplyOutcome::NoPending => {
            let text = im_text_for_state(state);
            let _ = adapter
                .clear_reply_markup(&message.chat_id, message.card_message_id.as_deref())
                .await;
            adapter
                .send_text(&message.chat_id, text.no_pending_approval())
                .await?;
        }
        ApprovalReplyOutcome::NotCurrent => {
            let text = im_text_for_state(state);
            let _ = adapter
                .clear_reply_markup(&message.chat_id, message.card_message_id.as_deref())
                .await;
            adapter
                .send_text(&message.chat_id, text.approval_not_current())
                .await?;
        }
        ApprovalReplyOutcome::InvalidInput { hint } => {
            let text = im_text_for_state(state);
            adapter
                .send_text(&message.chat_id, &text.invalid_approval_reply(&hint))
                .await?;
        }
    }
    Ok(true)
}

pub(crate) fn command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?.trim();
    if !first.starts_with('/') {
        return None;
    }
    let command = first
        .split_once('@')
        .map(|(command, _)| command)
        .unwrap_or(first)
        .to_ascii_lowercase();
    Some(command)
}

pub(crate) fn numeric_command_index(command: &str) -> Option<usize> {
    let number = command.strip_prefix('/')?.parse::<usize>().ok()?;
    number.checked_sub(1)
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use anyhow::{Result, anyhow};

    use super::{
        TelegramThreadRoutingResultDelivery, TelegramThreadRoutingResultSender,
        callback_targets_current_message, deliver_telegram_thread_routing_result,
    };

    struct ScriptedThreadRoutingResultSender {
        results: Mutex<VecDeque<Result<String>>>,
        message_ids: Mutex<Vec<Option<String>>>,
    }

    impl ScriptedThreadRoutingResultSender {
        fn new(results: Vec<Result<String>>) -> Self {
            Self {
                results: Mutex::new(results.into()),
                message_ids: Mutex::new(Vec::new()),
            }
        }

        fn message_ids(&self) -> Vec<Option<String>> {
            self.message_ids.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl TelegramThreadRoutingResultSender for ScriptedThreadRoutingResultSender {
        async fn send_thread_routing_result(
            &self,
            _target: &str,
            _title: &str,
            _body: &str,
            message_id: Option<&str>,
        ) -> Result<String> {
            self.message_ids
                .lock()
                .unwrap()
                .push(message_id.map(str::to_string));
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .expect("missing scripted Telegram result")
        }
    }

    #[test]
    fn rejects_callback_from_replaced_routing_message() {
        assert!(!callback_targets_current_message(Some("10"), Some("9")));
    }

    #[test]
    fn accepts_current_callback_and_text_replies() {
        assert!(callback_targets_current_message(Some("10"), Some("10")));
        assert!(callback_targets_current_message(Some("10"), None));
    }

    #[tokio::test]
    async fn final_routing_result_updates_progress_message_without_fallback() {
        let sender = ScriptedThreadRoutingResultSender::new(vec![Ok("10".to_string())]);

        let delivery =
            deliver_telegram_thread_routing_result(&sender, "chat", "title", "body", "10").await;

        assert_eq!(delivery, TelegramThreadRoutingResultDelivery::Updated);
        assert_eq!(sender.message_ids(), vec![Some("10".to_string())]);
    }

    #[tokio::test]
    async fn final_routing_result_falls_back_to_new_message_after_update_failure() {
        let sender = ScriptedThreadRoutingResultSender::new(vec![
            Err(anyhow!("edit failed")),
            Ok("11".to_string()),
        ]);

        let delivery =
            deliver_telegram_thread_routing_result(&sender, "chat", "title", "body", "10").await;

        assert_eq!(
            delivery,
            TelegramThreadRoutingResultDelivery::SentAsNew {
                message_id: "11".to_string(),
                update_error: "edit failed".to_string(),
            }
        );
        assert_eq!(sender.message_ids(), vec![Some("10".to_string()), None]);
    }

    #[tokio::test]
    async fn final_routing_result_swallows_both_delivery_failures() {
        let sender = ScriptedThreadRoutingResultSender::new(vec![
            Err(anyhow!("edit failed")),
            Err(anyhow!("send failed")),
        ]);

        let delivery =
            deliver_telegram_thread_routing_result(&sender, "chat", "title", "body", "10").await;

        assert_eq!(
            delivery,
            TelegramThreadRoutingResultDelivery::Undelivered {
                update_error: "edit failed".to_string(),
                fallback_error: "send failed".to_string(),
            }
        );
        assert_eq!(sender.message_ids(), vec![Some("10".to_string()), None]);
    }
}

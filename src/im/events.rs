use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose};

use crate::{
    app_state::SharedState,
    chain_log,
    codex::{extract_agent_message_text, extract_turn_reply_text},
    im::{
        core::{
            accounts::ImApiRegistry,
            outbound::{ImOutboundKind, ImOutboundMessage, ImOutboundPayload, ImOutboundSender},
            text_renderer,
            turn::turn_completed_notice,
        },
        feishu::{
            FeishuAdapter, FeishuApi, flow as feishu_flow, renderer,
            runtime::{
                complete_existing_item_card, ensure_started_streaming_card_state,
                upsert_streaming_card_state,
            },
        },
        telegram::adapter::TelegramAdapter,
        wechat::adapter::WechatAdapter,
    },
    im_runtime::{PendingApproval, RouteTarget, TurnOrigin},
    types::ImPlatformKind,
};

const COMMAND_OUTPUT_PREVIEW_CHARS: usize = 2400;
pub(crate) async fn send_next_approval(
    state: &SharedState,
    api_registry: &ImApiRegistry,
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
    send_approval(state, api_registry, &route, approval).await
}

pub(crate) async fn send_approval(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    route: &RouteTarget,
    approval: &PendingApproval,
) -> Result<()> {
    match route.platform {
        ImPlatformKind::Feishu => {
            let Some(api) = api_registry.feishu_for_route(route) else {
                log_missing_api(state, route, "approval").await;
                return Ok(());
            };
            send_feishu_approval(state, &api, route, approval).await
        }
        ImPlatformKind::Telegram => {
            let Some(api) = api_registry.telegram_for_route(route) else {
                log_missing_api(state, route, "approval").await;
                return Ok(());
            };
            let adapter = TelegramAdapter::new(api);
            send_telegram_approval(state, &adapter, route, approval).await
        }
        ImPlatformKind::Wechat => {
            let Some(api) = api_registry.wechat_for_route(route) else {
                log_missing_api(state, route, "approval").await;
                return Ok(());
            };
            let adapter = WechatAdapter::new(api);
            send_wechat_approval(state, &adapter, route, approval).await
        }
    }
}

pub(crate) async fn send_next_telegram_approval(
    state: &SharedState,
    adapter: &TelegramAdapter,
    conversation_key: &str,
    approval: &PendingApproval,
) -> Result<()> {
    let Some(route) = crate::im_runtime::route_from_conversation_key(conversation_key) else {
        state
            .push_event(
                "warn",
                "telegram_approval_next_route_missing",
                format!("conversation={conversation_key}"),
            )
            .await;
        return Ok(());
    };
    if route.platform != ImPlatformKind::Telegram {
        return Ok(());
    }
    send_telegram_approval(state, adapter, &route, approval).await
}

pub(crate) async fn send_turn_reply(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    outbound_tx: Option<&ImOutboundSender>,
    thread_id: &str,
    route: &RouteTarget,
    text: &str,
) {
    log_remote_to_im_enqueue(
        "turn_reply_input",
        thread_id,
        route,
        "",
        "agentMessage",
        text,
    );
    let should_send = {
        let mut runtime = state.runtime.lock().await;
        let key = format!("{}:turn-reply", route.conversation_key);
        if runtime.should_skip_duplicate_text(&key, text) {
            false
        } else {
            runtime.remember_sent_text(&key, text);
            true
        }
    };
    if !should_send {
        if matches!(
            route.platform,
            ImPlatformKind::Telegram | ImPlatformKind::Wechat
        ) {
            let event_kind = format!("{}_turn_reply_skipped", route.platform.key());
            state
                .push_event(
                    "info",
                    &event_kind,
                    format!(
                        "thread={thread_id} chat={} reason=duplicate text_len={}",
                        route.chat_id,
                        text.chars().count()
                    ),
                )
                .await;
        }
        return;
    }
    match route.platform {
        ImPlatformKind::Feishu => {
            let Some(api) = api_registry.feishu_for_route(route) else {
                log_missing_api(state, route, "turn_reply").await;
                return;
            };
            let adapter = FeishuAdapter::new(api);
            if let Err(err) = adapter.send_turn_completed(&route.chat_id, text).await {
                state
                    .push_event(
                        "error",
                        "feishu_turn_completed_failed",
                        format!("thread={thread_id} chat={} err={err}", route.chat_id),
                    )
                    .await;
            }
        }
        ImPlatformKind::Telegram => {
            let rendered = text_renderer::render_agent_message_text(text);
            if let Some(outbound_tx) = outbound_tx {
                log_remote_to_im_enqueue(
                    "turn_reply_enqueue",
                    thread_id,
                    route,
                    "",
                    "agentMessage",
                    &rendered,
                );
                if let Err(err) = outbound_tx.enqueue(ImOutboundMessage {
                    thread_id: thread_id.to_string(),
                    route: route.clone(),
                    item_id: None,
                    item_type: Some("agentMessage".to_string()),
                    kind: ImOutboundKind::TurnReply,
                    payload: ImOutboundPayload::Text(rendered),
                }) {
                    state
                        .push_event(
                            "error",
                            "telegram_turn_enqueue_failed",
                            format!("thread={thread_id} chat={} err={err}", route.chat_id),
                        )
                        .await;
                }
            } else {
                let Some(api) = api_registry.telegram_for_route(route) else {
                    log_missing_api(state, route, "turn_reply").await;
                    return;
                };
                let adapter = TelegramAdapter::new(api);
                match adapter.send_turn_completed(&route.chat_id, &rendered).await {
                    Ok(message_id) => {
                        state
                            .push_event(
                                "info",
                                "telegram_turn_completed_sent",
                                format!(
                                    "thread={thread_id} chat={} message={message_id}",
                                    route.chat_id
                                ),
                            )
                            .await;
                    }
                    Err(err) => {
                        state
                            .push_event(
                                "error",
                                "telegram_turn_completed_failed",
                                format!("thread={thread_id} chat={} err={err}", route.chat_id),
                            )
                            .await;
                    }
                }
            }
        }
        ImPlatformKind::Wechat => {
            let rendered = text_renderer::render_agent_message_text(text);
            if let Some(outbound_tx) = outbound_tx {
                log_remote_to_im_enqueue(
                    "turn_reply_enqueue",
                    thread_id,
                    route,
                    "",
                    "agentMessage",
                    &rendered,
                );
                if let Err(err) = outbound_tx.enqueue(ImOutboundMessage {
                    thread_id: thread_id.to_string(),
                    route: route.clone(),
                    item_id: None,
                    item_type: Some("agentMessage".to_string()),
                    kind: ImOutboundKind::TurnReply,
                    payload: ImOutboundPayload::Text(rendered),
                }) {
                    state
                        .push_event(
                            "error",
                            "wechat_turn_enqueue_failed",
                            format!("thread={thread_id} peer={} err={err}", route.chat_id),
                        )
                        .await;
                }
            } else {
                let Some(api) = api_registry.wechat_for_route(route) else {
                    log_missing_api(state, route, "turn_reply").await;
                    return;
                };
                let adapter = WechatAdapter::new(api);
                match adapter
                    .send_turn_completed(state, &route.account_id, &route.chat_id, &rendered)
                    .await
                {
                    Ok(message_id) => {
                        state
                            .push_event(
                                "info",
                                "wechat_turn_completed_sent",
                                format!(
                                    "thread={thread_id} peer={} message={message_id}",
                                    route.chat_id
                                ),
                            )
                            .await;
                    }
                    Err(err) => {
                        state
                            .push_event(
                                "error",
                                "wechat_turn_completed_failed",
                                format!("thread={thread_id} peer={} err={err}", route.chat_id),
                            )
                            .await;
                    }
                }
            }
        }
    }
}

async fn send_turn_completed_mark(
    state: &SharedState,
    api_registry: &ImApiRegistry,
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
) {
    let text = turn_completed_notice();
    match route.platform {
        ImPlatformKind::Feishu => {
            let Some(api) = api_registry.feishu_for_route(route) else {
                log_missing_api(state, route, "turn_completed_mark").await;
                return;
            };
            let adapter = FeishuAdapter::new(api);
            match adapter.send_turn_completed_mark(&route.chat_id, text).await {
                Ok(message_id) => {
                    state
                        .push_event(
                            "info",
                            "feishu_turn_completed_mark_sent",
                            format!(
                                "thread={thread_id} chat={} message={message_id}",
                                route.chat_id
                            ),
                        )
                        .await;
                }
                Err(err) => {
                    state
                        .push_event(
                            "error",
                            "feishu_turn_completed_mark_failed",
                            format!("thread={thread_id} chat={} err={err}", route.chat_id),
                        )
                        .await;
                }
            }
        }
        ImPlatformKind::Telegram | ImPlatformKind::Wechat => {
            if let Err(err) = outbound_tx.enqueue(ImOutboundMessage {
                thread_id: thread_id.to_string(),
                route: route.clone(),
                item_id: None,
                item_type: Some("turnCompleted".to_string()),
                kind: ImOutboundKind::TurnReply,
                payload: ImOutboundPayload::Text(text.to_string()),
            }) {
                state
                    .push_event(
                        "error",
                        "im_turn_completed_mark_enqueue_failed",
                        format!(
                            "thread={thread_id} platform={} chat={} err={err}",
                            route.platform.key(),
                            route.chat_id
                        ),
                    )
                    .await;
            }
        }
    }
}

pub(crate) async fn handle_codex_notification(
    state: SharedState,
    api_registry: ImApiRegistry,
    outbound_tx: ImOutboundSender,
    notification: &crate::codex::CodexNotification,
) {
    let Some(params) = notification.params.as_ref() else {
        return;
    };
    log_codex_to_im_handler(notification);
    match notification.method.as_str() {
        "turn/started" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(turn_id) = params
                .get("turn")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str())
                .or_else(|| params.get("turnId").and_then(|v| v.as_str()))
            else {
                return;
            };
            if route_for_codex_output(&state, &notification.method, thread_id, params)
                .await
                .is_some()
            {
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_started(thread_id, turn_id);
            }
        }
        "thread/started" => {}
        "thread/status/changed" => {}
        "item/started" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item) = params.get("item") else {
                return;
            };
            let Some(item_type) = item.get("type").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(kind) = structured_streaming_kind(item_type) else {
                return;
            };
            let Some(item_id) = item.get("id").and_then(|v| v.as_str()) else {
                return;
            };
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "item_started").await;
                return;
            };
            let initial_text = if kind == "commandExecution" {
                command_execution_started_text(item)
                    .or_else(|| renderer::item_markdown_summary(item))
            } else {
                renderer::item_markdown_summary(item)
            };
            ensure_started_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                kind,
                &route.account_id,
                &route.chat_id,
                initial_text,
            )
            .await;
        }
        "item/agentMessage/delta" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                state
                    .push_event(
                        "info",
                        "feishu_stream_skipped",
                        format!("thread={thread_id} reason=no_binding"),
                    )
                    .await;
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "agent_delta").await;
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                "agentMessage",
                &route.account_id,
                &route.chat_id,
                delta,
                false,
            )
            .await;
        }
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "reasoning_delta").await;
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                "reasoning",
                &route.account_id,
                &route.chat_id,
                delta,
                false,
            )
            .await;
        }
        "item/plan/delta" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "plan_delta").await;
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                "plan",
                &route.account_id,
                &route.chat_id,
                delta,
                false,
            )
            .await;
        }
        "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            if notification.method == "item/commandExecution/outputDelta" {
                return;
            }
            let Some(delta) = params.get("delta").and_then(|v| v.as_str()) else {
                return;
            };
            let kind = if notification.method == "item/commandExecution/outputDelta" {
                "commandExecution"
            } else {
                "fileChange"
            };
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "output_delta").await;
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                kind,
                &route.account_id,
                &route.chat_id,
                delta,
                false,
            )
            .await;
        }
        "item/mcpToolCall/progress" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = params.get("itemId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(message) = params.get("message").and_then(|v| v.as_str()) else {
                return;
            };
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "mcp_progress").await;
                return;
            };
            upsert_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                "mcpToolCall",
                &route.account_id,
                &route.chat_id,
                message,
                false,
            )
            .await;
        }
        "item/updated" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item) = params.get("item") else {
                return;
            };
            let Some(item_type) = item.get("type").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(kind) = structured_streaming_kind(item_type) else {
                return;
            };
            let Some(item_id) = item.get("id").and_then(|v| v.as_str()) else {
                return;
            };
            let route =
                feishu_route_for_codex_output(&state, &notification.method, thread_id, params)
                    .await;
            let Some(route) = route else {
                return;
            };
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "item_updated").await;
                return;
            };
            let initial_text = if item_type == "commandExecution" {
                command_execution_full_text(item).or_else(|| renderer::item_markdown_summary(item))
            } else {
                renderer::item_markdown_summary(item)
            };
            ensure_started_streaming_card_state(
                state,
                api,
                thread_id,
                item_id,
                kind,
                &route.account_id,
                &route.chat_id,
                initial_text,
            )
            .await;
        }
        "item/completed" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let turn_id = params.get("turnId").and_then(|v| v.as_str());
            let Some(item) = params.get("item") else {
                return;
            };
            let Some(item_type) = item.get("type").and_then(|v| v.as_str()) else {
                return;
            };
            let Some(item_id) = item.get("id").and_then(|v| v.as_str()) else {
                return;
            };
            let route =
                route_for_codex_output(&state, &notification.method, thread_id, params).await;
            let Some(route) = route else {
                return;
            };
            if matches!(
                route.platform,
                ImPlatformKind::Telegram | ImPlatformKind::Wechat
            ) {
                if item_type == "agentMessage"
                    && let Some(text) = extract_agent_message_text(item)
                {
                    send_turn_reply(
                        &state,
                        &api_registry,
                        Some(&outbound_tx),
                        thread_id,
                        &route,
                        &text,
                    )
                    .await;
                } else if item_type == "userMessage" {
                    let should_forward = if let Some(turn_id) = turn_id {
                        state.runtime.lock().await.turn_origin(turn_id)
                            != turn_origin_for_platform(route.platform)
                    } else {
                        true
                    };
                    if should_forward {
                        let _ = send_text_im_codex_item(
                            &state,
                            &outbound_tx,
                            thread_id,
                            &route,
                            item_id,
                            item,
                        )
                        .await;
                    }
                } else if let Err(err) =
                    send_text_im_codex_item(&state, &outbound_tx, thread_id, &route, item_id, item)
                        .await
                {
                    let event_kind = format!("{}_item_failed", route.platform.key());
                    state
                        .push_event(
                            "error",
                            &event_kind,
                            format!(
                                "thread={thread_id} item={item_id} type={item_type} chat={} err={err}",
                                route.chat_id
                            ),
                        )
                        .await;
                }
                return;
            }
            let Some(api) = api_registry.feishu_for_route(&route) else {
                log_missing_api(&state, &route, "item_completed").await;
                return;
            };
            if matches!(item_type, "imageGeneration" | "imageView")
                && feishu_flow::send_image_item_card(
                    &state, &api, &route, item_type, item, thread_id, item_id,
                )
                .await
            {
                return;
            }
            let kind = structured_streaming_kind(item_type).unwrap_or(item_type);
            let text = if item_type == "agentMessage" {
                extract_agent_message_text(item)
            } else if item_type == "commandExecution" {
                command_execution_full_text(item).or_else(|| renderer::item_markdown_summary(item))
            } else {
                renderer::item_markdown_summary(item)
            };
            if matches!(
                item_type,
                "agentMessage"
                    | "reasoning"
                    | "plan"
                    | "commandExecution"
                    | "fileChange"
                    | "mcpToolCall"
            ) {
                let updated = complete_existing_item_card(
                    state.clone(),
                    api.clone(),
                    thread_id,
                    item_id,
                    kind,
                    &route.account_id,
                    &route.chat_id,
                    text.clone(),
                )
                .await;
                if updated {
                    return;
                }
                if let Some(text) = text {
                    upsert_streaming_card_state(
                        state,
                        api,
                        thread_id,
                        item_id,
                        kind,
                        &route.account_id,
                        &route.chat_id,
                        &text,
                        true,
                    )
                    .await;
                }
            } else if item_type == "userMessage" {
                let should_forward = if let Some(turn_id) = turn_id {
                    state.runtime.lock().await.turn_origin(turn_id) != Some(TurnOrigin::Feishu)
                } else {
                    true
                };
                if !should_forward {
                    state
                        .push_event(
                            "info",
                            "feishu_user_message_suppressed",
                            format!(
                                "thread={thread_id} item={item_id} turn={} chat={}",
                                turn_id.unwrap_or(""),
                                route.chat_id
                            ),
                        )
                        .await;
                    return;
                }
                if let Some(card) = renderer::build_item_card(item) {
                    let adapter = FeishuAdapter::new(api.clone());
                    if let Err(err) = adapter.send_interactive(&route.chat_id, &card).await {
                        state
                            .push_event(
                                "error",
                                "feishu_item_card_failed",
                                format!(
                                    "thread={thread_id} item={item_id} chat={} err={err}",
                                    route.chat_id
                                ),
                            )
                            .await;
                    }
                }
            } else if let Some(card) = renderer::build_item_card(item) {
                let adapter = FeishuAdapter::new(api.clone());
                if let Err(err) = adapter.send_interactive(&route.chat_id, &card).await {
                    state
                        .push_event(
                            "error",
                            "feishu_item_card_failed",
                            format!(
                                "thread={thread_id} item={item_id} chat={} err={err}",
                                route.chat_id
                            ),
                        )
                        .await;
                }
            }
        }
        "turn/completed" | "codex/event/turn_completed" => {
            let Some(thread_id) = params.get("threadId").and_then(|v| v.as_str()) else {
                return;
            };
            let turn_id = params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(|v| v.as_str())
                .or_else(|| params.get("turnId").and_then(|v| v.as_str()));
            let route =
                route_for_codex_output(&state, &notification.method, thread_id, params).await;
            let Some(route) = route else {
                state
                    .runtime
                    .lock()
                    .await
                    .mark_turn_completed(thread_id, turn_id);
                return;
            };
            if let Some(text) = extract_turn_reply_text(params) {
                send_turn_reply(
                    &state,
                    &api_registry,
                    Some(&outbound_tx),
                    thread_id,
                    &route,
                    &text,
                )
                .await;
            }
            state
                .runtime
                .lock()
                .await
                .mark_turn_completed(thread_id, turn_id);
            send_turn_completed_mark(&state, &api_registry, &outbound_tx, thread_id, &route).await;
        }
        _ => {}
    }
}

async fn route_for_codex_output(
    state: &SharedState,
    method: &str,
    thread_id: &str,
    _params: &serde_json::Value,
) -> Option<RouteTarget> {
    if let Some(route) = state.runtime.lock().await.route_for_thread(thread_id) {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[im_route] event=codex_route_hit method={} thread={} platform={} account={} chat={} conversation={}",
                method,
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            )
        });
        return Some(route);
    }
    chain_log::write_line(format!(
        "[im_route] level=warn event=codex_route_missing method={} thread={}",
        method, thread_id
    ));
    None
}

async fn log_missing_api(state: &SharedState, route: &RouteTarget, context: &str) {
    state
        .push_event(
            "error",
            "im_api_missing",
            format!(
                "context={} platform={} account={} chat={}",
                context,
                route.platform.key(),
                route.account_id,
                route.chat_id
            ),
        )
        .await;
}

async fn feishu_route_for_codex_output(
    state: &SharedState,
    method: &str,
    thread_id: &str,
    params: &serde_json::Value,
) -> Option<RouteTarget> {
    let route = route_for_codex_output(state, method, thread_id, params).await?;
    if route.platform != ImPlatformKind::Feishu {
        chain_log::write_diagnostic_lazy(|| {
            format!(
                "[im_route] event=codex_route_platform_skip method={} thread={} wanted=feishu actual={} account={} chat={} conversation={}",
                method,
                thread_id,
                route.platform.key(),
                route.account_id,
                route.chat_id,
                route.conversation_key
            )
        });
        return None;
    }
    Some(route)
}

fn turn_origin_for_platform(platform: ImPlatformKind) -> Option<TurnOrigin> {
    match platform {
        ImPlatformKind::Feishu => Some(TurnOrigin::Feishu),
        ImPlatformKind::Telegram => Some(TurnOrigin::Telegram),
        ImPlatformKind::Wechat => Some(TurnOrigin::Wechat),
    }
}

fn structured_streaming_kind(item_type: &str) -> Option<&'static str> {
    match item_type {
        "agentMessage" => Some("agentMessage"),
        "reasoning" => Some("reasoning"),
        "plan" => Some("plan"),
        "commandExecution" => Some("commandExecution"),
        "fileChange" => Some("fileChange"),
        "mcpToolCall" => Some("mcpToolCall"),
        _ => None,
    }
}

fn command_execution_started_text(item: &serde_json::Value) -> Option<String> {
    let command = item
        .get("commandActions")
        .and_then(|v| v.as_array())
        .and_then(|actions| actions.first())
        .and_then(|action| action.get("command"))
        .and_then(|v| v.as_str())
        .or_else(|| item.get("command").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(format!(
        "__COMMAND__\n{}\n__OUTPUT__\n\n__META__\nStatus: in_progress",
        command
    ))
}

fn command_execution_full_text(item: &serde_json::Value) -> Option<String> {
    let command = item
        .get("commandActions")
        .and_then(|v| v.as_array())
        .and_then(|actions| actions.first())
        .and_then(|action| action.get("command"))
        .and_then(|v| v.as_str())
        .or_else(|| item.get("command").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    let output = item
        .get("aggregatedOutput")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let output = truncate_command_output_preview(&output);

    let mut meta = Vec::new();
    if let Some(status) = item
        .get("status")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        meta.push(format!("Status: {}", status));
    }
    if let Some(exit_code) = item.get("exitCode").and_then(|v| v.as_i64()) {
        meta.push(format!("exit {}", exit_code));
    }
    if let Some(duration_ms) = item.get("durationMs").and_then(|v| v.as_u64()) {
        meta.push(format!("{}ms", duration_ms));
    }

    Some(format!(
        "__COMMAND__\n{}\n__OUTPUT__\n{}\n__META__\n{}",
        command,
        output,
        meta.join(" · ")
    ))
}

fn truncate_command_output_preview(text: &str) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count >= COMMAND_OUTPUT_PREVIEW_CHARS {
            out.push_str("\n... output truncated ...");
            break;
        }
        out.push(ch);
        count += 1;
    }
    out
}

async fn send_feishu_approval(
    state: &SharedState,
    api: &FeishuApi,
    route: &RouteTarget,
    approval: &PendingApproval,
) -> Result<()> {
    let adapter = FeishuAdapter::new(api.clone());
    let message_id = adapter.send_approval(&route.chat_id, approval).await?;
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

async fn send_telegram_approval(
    state: &SharedState,
    adapter: &TelegramAdapter,
    route: &RouteTarget,
    approval: &PendingApproval,
) -> Result<()> {
    let message_id = adapter.send_approval(&route.chat_id, approval).await?;
    state
        .runtime
        .lock()
        .await
        .remember_approval_message_id(&approval.request_id, message_id.clone());
    state
        .push_event(
            "info",
            "telegram_approval_sent",
            format!(
                "conversation={} request_id={} message={}",
                route.conversation_key, approval.request_id, message_id
            ),
        )
        .await;
    Ok(())
}

async fn send_wechat_approval(
    state: &SharedState,
    adapter: &WechatAdapter,
    route: &RouteTarget,
    approval: &PendingApproval,
) -> Result<()> {
    let message_id = adapter
        .send_approval(state, &route.account_id, &route.chat_id, approval)
        .await?;
    state
        .runtime
        .lock()
        .await
        .remember_approval_message_id(&approval.request_id, message_id.clone());
    state
        .push_event(
            "info",
            "wechat_approval_sent",
            format!(
                "conversation={} request_id={} message={}",
                route.conversation_key, approval.request_id, message_id
            ),
        )
        .await;
    Ok(())
}

async fn send_text_im_codex_item(
    state: &SharedState,
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
    item_id: &str,
    item: &serde_json::Value,
) -> Result<()> {
    let item_type = item
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let platform = route.platform.key();
    let dedupe_payload = item.to_string();
    let should_send = {
        let mut runtime = state.runtime.lock().await;
        let key = format!("{}:item:{item_id}:{item_type}", route.conversation_key);
        if runtime.should_skip_duplicate_text(&key, &dedupe_payload) {
            false
        } else {
            runtime.remember_sent_text(&key, &dedupe_payload);
            true
        }
    };
    if !should_send {
        let event_kind = format!("{platform}_item_skipped");
        state
            .push_event(
                "info",
                &event_kind,
                format!(
                    "thread={thread_id} item={item_id} type={item_type} chat={} reason=duplicate",
                    route.chat_id
                ),
            )
            .await;
        return Ok(());
    }

    if matches!(item_type, "imageGeneration" | "imageView")
        && let Some(path) =
            text_im_image_path_for_item(state, platform, item_type, item, item_id).await?
    {
        let caption = text_renderer::image_item_caption(item);
        let fallback_text = text_renderer::render_item_text(item);
        outbound_tx.enqueue(ImOutboundMessage {
            thread_id: thread_id.to_string(),
            route: route.clone(),
            item_id: Some(item_id.to_string()),
            item_type: Some(item_type.to_string()),
            kind: ImOutboundKind::ImageItem,
            payload: ImOutboundPayload::Image {
                path,
                caption: Some(caption),
                fallback_text,
            },
        })?;
        let event_kind = format!("{platform}_image_queued");
        state
            .push_event(
                "info",
                &event_kind,
                format!(
                    "thread={thread_id} item={item_id} type={item_type} chat={}",
                    route.chat_id
                ),
            )
            .await;
        return Ok(());
    }

    let Some(text) = text_renderer::render_item_text(item) else {
        let event_kind = format!("{platform}_item_skipped");
        state
            .push_event(
                "info",
                &event_kind,
                format!(
                    "thread={thread_id} item={item_id} type={item_type} chat={} reason=empty_render",
                    route.chat_id
                ),
            )
            .await;
        return Ok(());
    };
    let mcp_tool_image_paths = if item_type == "mcpToolCall" {
        mcp_tool_image_paths_for_item(state, platform, item, item_id).await?
    } else {
        Vec::new()
    };

    log_remote_to_im_enqueue("item_enqueue", thread_id, route, item_id, item_type, &text);
    outbound_tx.enqueue(ImOutboundMessage {
        thread_id: thread_id.to_string(),
        route: route.clone(),
        item_id: Some(item_id.to_string()),
        item_type: Some(item_type.to_string()),
        kind: ImOutboundKind::Item,
        payload: ImOutboundPayload::Text(text.clone()),
    })?;
    let mcp_tool_image_count =
        queue_mcp_tool_images(outbound_tx, thread_id, route, item_id, mcp_tool_image_paths)?;
    let event_kind = format!("{platform}_item_queued");
    state
        .push_event(
            "info",
            &event_kind,
            format!(
                "thread={thread_id} item={item_id} type={item_type} chat={} text_len={}",
                route.chat_id,
                text.chars().count()
            ),
        )
        .await;
    if mcp_tool_image_count > 0 {
        let event_kind = format!("{platform}_mcp_tool_images_queued");
        state
            .push_event(
                "info",
                &event_kind,
                format!(
                    "thread={thread_id} item={item_id} type=mcpToolCall chat={} image_count={mcp_tool_image_count}",
                    route.chat_id
                ),
            )
            .await;
    }
    Ok(())
}

fn queue_mcp_tool_images(
    outbound_tx: &ImOutboundSender,
    thread_id: &str,
    route: &RouteTarget,
    item_id: &str,
    paths: Vec<PathBuf>,
) -> Result<usize> {
    let image_count = paths.len();
    for path in paths {
        outbound_tx.enqueue(ImOutboundMessage {
            thread_id: thread_id.to_string(),
            route: route.clone(),
            item_id: Some(item_id.to_string()),
            item_type: Some("mcpToolCall".to_string()),
            kind: ImOutboundKind::ImageItem,
            payload: ImOutboundPayload::Image {
                path,
                caption: None,
                fallback_text: None,
            },
        })?;
    }
    Ok(image_count)
}

fn log_codex_to_im_handler(notification: &crate::codex::CodexNotification) {
    if !chain_log::diagnostic_enabled() {
        return;
    }
    let Some(params) = notification.params.as_ref() else {
        return;
    };
    let thread_id = params
        .get("threadId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let turn_id = params
        .get("turnId")
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("id"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("");
    let item = params.get("item");
    let item_id = item
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .or_else(|| params.get("itemId").and_then(|v| v.as_str()))
        .unwrap_or("");
    let item_type = item
        .and_then(|v| v.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let text = trace_text_for_notification(&notification.method, params, item);
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[im_trace] event=codex_to_im_handler method={} thread={} turn={} item={} type={} text_len={} preview={}",
            notification.method,
            thread_id,
            turn_id,
            item_id,
            item_type,
            text.chars().count(),
            trace_preview(&text, 500)
        )
    });
}

fn log_remote_to_im_enqueue(
    event: &str,
    thread_id: &str,
    route: &RouteTarget,
    item_id: &str,
    item_type: &str,
    text: &str,
) {
    chain_log::write_diagnostic_lazy(|| {
        format!(
            "[im_trace] event=remote_to_im_{} platform={} account={} chat={} thread={} item={} type={} text_len={} preview={}",
            event,
            route.platform.key(),
            route.account_id,
            route.chat_id,
            thread_id,
            item_id,
            item_type,
            text.chars().count(),
            trace_preview(text, 500)
        )
    });
}

fn trace_text_for_notification(
    method: &str,
    params: &serde_json::Value,
    item: Option<&serde_json::Value>,
) -> String {
    if let Some(delta) = params.get("delta").and_then(|v| v.as_str()) {
        return delta.to_string();
    }
    if let Some(message) = params.get("message").and_then(|v| v.as_str()) {
        return message.to_string();
    }
    if let Some(item) = item {
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            return text.to_string();
        }
        if let Some(text) = item.get("aggregatedOutput").and_then(|v| v.as_str()) {
            return text.to_string();
        }
        if method.contains("commandExecution")
            && let Some(command) = item
                .get("commandActions")
                .and_then(|v| v.as_array())
                .and_then(|actions| actions.first())
                .and_then(|action| action.get("command"))
                .and_then(|v| v.as_str())
                .or_else(|| item.get("command").and_then(|v| v.as_str()))
        {
            return command.to_string();
        }
        return item.to_string();
    }
    params.to_string()
}

fn trace_preview(text: &str, limit: usize) -> String {
    let compact = text.replace("\r\n", "\n").replace('\n', "\\n");
    let mut out = String::new();
    for ch in compact.chars().take(limit) {
        out.push(ch);
    }
    if compact.chars().count() > limit {
        out.push_str("...");
    }
    out
}

async fn text_im_image_path_for_item(
    state: &SharedState,
    platform: &str,
    item_type: &str,
    item: &serde_json::Value,
    item_id: &str,
) -> Result<Option<PathBuf>> {
    if let Some(path) = text_renderer::image_item_path(item)
        && path.is_file()
    {
        return Ok(Some(path));
    }
    if item_type != "imageGeneration" {
        return Ok(None);
    }
    let Some(result) = item.get("result").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let Some(decoded) = decode_image_string(result) else {
        return Ok(None);
    };
    Ok(Some(
        write_im_image_cache(state, platform, item_id, decoded).await?,
    ))
}

async fn mcp_tool_image_paths_for_item(
    state: &SharedState,
    platform: &str,
    item: &serde_json::Value,
    item_id: &str,
) -> Result<Vec<PathBuf>> {
    let Some(content) = item
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(|value| value.as_array())
    else {
        return Ok(Vec::new());
    };
    let mut paths = Vec::new();
    for (index, entry) in content
        .iter()
        .filter(|entry| entry.get("type").and_then(|value| value.as_str()) == Some("image"))
        .enumerate()
    {
        let Some(data) = entry.get("data").and_then(|value| value.as_str()) else {
            continue;
        };
        let mime_type = entry
            .get("mimeType")
            .or_else(|| entry.get("mime_type"))
            .and_then(|value| value.as_str());
        let Some(decoded) = decode_image_content(data, mime_type) else {
            continue;
        };
        let cache_key = format!("{item_id}-mcp-{index}");
        paths.push(write_im_image_cache(state, platform, &cache_key, decoded).await?);
    }
    Ok(paths)
}

async fn write_im_image_cache(
    state: &SharedState,
    platform: &str,
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
        platform,
        safe_file_stem(item_id),
        decoded.extension
    ));
    std::fs::write(&path, decoded.bytes)
        .with_context(|| format!("failed to write image cache {}", path.display()))?;
    Ok(path)
}

struct DecodedImage {
    bytes: Vec<u8>,
    extension: &'static str,
}

fn decode_image_string(value: &str) -> Option<DecodedImage> {
    let trimmed = value.trim();
    if let Some((mime, payload)) = parse_image_data_url(trimmed) {
        let bytes = general_purpose::STANDARD.decode(payload).ok()?;
        let extension =
            image_extension_from_mime(mime).or_else(|| image_extension_from_bytes(&bytes))?;
        return Some(DecodedImage { bytes, extension });
    }
    if !looks_like_inline_image_base64(trimmed) {
        return None;
    }
    let bytes = general_purpose::STANDARD.decode(trimmed).ok()?;
    let extension = image_extension_from_bytes(&bytes)?;
    Some(DecodedImage { bytes, extension })
}

fn decode_image_content(value: &str, mime_type: Option<&str>) -> Option<DecodedImage> {
    let trimmed = value.trim();
    if let Some((mime, payload)) = parse_image_data_url(trimmed) {
        let bytes = general_purpose::STANDARD.decode(payload).ok()?;
        let extension =
            image_extension_from_mime(mime).or_else(|| image_extension_from_bytes(&bytes))?;
        return Some(DecodedImage { bytes, extension });
    }
    let bytes = general_purpose::STANDARD.decode(trimmed).ok()?;
    let extension = mime_type
        .and_then(image_extension_from_mime)
        .or_else(|| image_extension_from_bytes(&bytes))?;
    Some(DecodedImage { bytes, extension })
}

fn parse_image_data_url(value: &str) -> Option<(&str, &str)> {
    let (metadata, payload) = value.split_once(',')?;
    let metadata = metadata.strip_prefix("data:")?;
    let mut parts = metadata.split(';');
    let mime = parts.next()?;
    if !mime.starts_with("image/") || !parts.any(|part| part == "base64") {
        return None;
    }
    Some((mime, payload))
}

fn looks_like_inline_image_base64(value: &str) -> bool {
    value.len() > 1024
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=' | '\r' | '\n'))
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
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn safe_file_stem(value: &str) -> String {
    let stem = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if stem.trim_matches('_').is_empty() {
        "image".to_string()
    } else {
        stem
    }
}

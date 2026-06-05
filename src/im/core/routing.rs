use crate::{app_state::SharedState, im_runtime::RouteTarget, types::InboundMessage};

pub(crate) fn route_for_message(message: &InboundMessage) -> RouteTarget {
    RouteTarget {
        platform: message.platform,
        conversation_key: message.conversation_key(),
        account_id: message.account_id.clone(),
        chat_id: message.chat_id.clone(),
    }
}

pub(crate) async fn live_thread_for_route(
    state: &SharedState,
    route: &RouteTarget,
) -> Option<String> {
    state
        .runtime
        .lock()
        .await
        .route_by_thread
        .iter()
        .find_map(|(thread_id, existing_route)| {
            (existing_route.conversation_key == route.conversation_key).then(|| thread_id.clone())
        })
}

pub(crate) async fn active_turn_for_message(
    state: &SharedState,
    message: &InboundMessage,
) -> Option<(String, String)> {
    let route = route_for_message(message);
    let thread_id = live_thread_for_route(state, &route).await?;
    let runtime = state.runtime.lock().await;
    let turn_id = runtime.current_turn_by_thread.get(&thread_id)?.clone();
    Some((thread_id, turn_id))
}

pub(crate) async fn clear_thread_binding(
    state: &SharedState,
    conversation_key: &str,
) -> anyhow::Result<()> {
    clear_thread_binding_with_reason(state, conversation_key, "clear_thread_binding").await
}

pub(crate) async fn clear_thread_binding_with_reason(
    state: &SharedState,
    conversation_key: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let mut runtime = state.runtime.lock().await;
    runtime.unbind_routes_for_conversation_with_reason(conversation_key, reason);
    Ok(())
}

pub(crate) fn is_stale_thread_error(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("thread not found") || message.contains("is closing")
}

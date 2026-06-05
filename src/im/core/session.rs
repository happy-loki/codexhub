use anyhow::Result;

use crate::{app_state::SharedState, im_runtime::RouteTarget, remote_control_backend};

pub(crate) async fn create_and_bind_thread(
    state: &SharedState,
    route: &RouteTarget,
    options: remote_control_backend::ThreadStartOptions,
    request_id: Option<&str>,
) -> Result<String> {
    let thread_id =
        remote_control_backend::start_thread_for_client(state, &route.conversation_key, options)
            .await?;
    bind_thread_to_route(state, route, &thread_id, request_id).await?;
    Ok(thread_id)
}

pub(crate) async fn resume_and_bind_thread(
    state: &SharedState,
    route: &RouteTarget,
    thread_id: &str,
    request_id: Option<&str>,
) -> Result<serde_json::Value> {
    let response = remote_control_backend::resume_thread_for_client(
        state,
        &route.conversation_key,
        thread_id,
        true,
    )
    .await?;
    bind_thread_to_route(state, route, thread_id, request_id).await?;
    Ok(response
        .get("thread")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

pub(crate) async fn bind_thread_to_route(
    state: &SharedState,
    route: &RouteTarget,
    thread_id: &str,
    request_id: Option<&str>,
) -> Result<()> {
    {
        let mut runtime = state.runtime.lock().await;
        runtime.unbind_routes_for_conversation_with_reason(
            &route.conversation_key,
            "bind_thread_to_route",
        );
        runtime.bind_route(thread_id, route.clone());
        if let Some(request_id) = request_id {
            runtime.clear_thread_routing_request(request_id);
        }
    }
    Ok(())
}

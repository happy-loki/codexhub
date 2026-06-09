use anyhow::Result;

use crate::{app_state::SharedState, im_runtime::RouteTarget, remote_control_backend};

pub(crate) async fn create_and_bind_thread(
    state: &SharedState,
    route: &RouteTarget,
    options: remote_control_backend::ThreadStartOptions,
    request_id: Option<&str>,
) -> Result<String> {
    let remote_client_key = route.remote_client_key.clone();
    let thread_id =
        remote_control_backend::start_thread_for_client(state, &remote_client_key, options).await?;
    bind_thread_to_route(state, route, &thread_id, request_id, remote_client_key).await?;
    Ok(thread_id)
}

pub(crate) async fn resume_and_bind_thread(
    state: &SharedState,
    route: &RouteTarget,
    thread_id: &str,
    request_id: Option<&str>,
) -> Result<serde_json::Value> {
    let remote_client_key = route.remote_client_key.clone();
    let response = remote_control_backend::resume_thread_for_client(
        state,
        &remote_client_key,
        thread_id,
        true,
    )
    .await?;
    bind_thread_to_route(state, route, thread_id, request_id, remote_client_key).await?;
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
    remote_client_key: String,
) -> Result<()> {
    {
        let mut runtime = state.runtime.lock().await;
        runtime.unbind_routes_for_conversation_with_reason(
            &route.conversation_key,
            "bind_thread_to_route",
        );
        let mut route = route.clone();
        route.remote_client_key = remote_client_key;
        runtime.bind_route(thread_id, route);
        if let Some(request_id) = request_id {
            runtime.clear_thread_routing_request(request_id);
        }
    }
    Ok(())
}

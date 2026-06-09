use anyhow::Error;

use crate::{
    app_state::SharedState,
    im::core::routing::{
        clear_thread_binding_with_reason, is_stale_thread_error, live_thread_binding_for_route,
    },
    im_runtime::{RouteTarget, TurnOrigin},
    remote_control_backend,
    types::InboundAttachment,
};

pub(crate) enum TurnStartOutcome {
    Started { thread_id: String, turn_id: String },
    Busy,
    Expired { thread_id: String },
    NoThread,
    Stale { thread_id: String },
    Failed { error: Error },
}

pub(crate) async fn start_turn_for_route(
    state: &SharedState,
    route: &RouteTarget,
    text: &str,
    attachments: &[InboundAttachment],
    received_at_ms: u128,
    origin: TurnOrigin,
) -> TurnStartOutcome {
    let Some((thread_id, bound_route)) = live_thread_binding_for_route(state, route).await else {
        return TurnStartOutcome::NoThread;
    };
    let remote_client_key = bound_route.remote_client_key;
    let blocked = {
        let mut runtime = state.runtime.lock().await;
        match runtime.try_mark_turn_starting(&thread_id) {
            Ok(()) if runtime.message_is_stale_for_latest_turn(&thread_id, received_at_ms) => {
                runtime.clear_turn_starting(&thread_id);
                Some(TurnStartOutcome::Expired {
                    thread_id: thread_id.clone(),
                })
            }
            Ok(()) => None,
            Err(_active_turn) => Some(TurnStartOutcome::Busy),
        }
    };
    if let Some(blocked) = blocked {
        return blocked;
    }

    match remote_control_backend::start_turn_for_client(
        state,
        &remote_client_key,
        &thread_id,
        text,
        attachments,
    )
    .await
    {
        Ok(turn_id) => {
            {
                let mut runtime = state.runtime.lock().await;
                runtime.mark_turn_started(&thread_id, &turn_id);
                runtime.remember_turn_origin(&turn_id, origin);
            }
            TurnStartOutcome::Started { thread_id, turn_id }
        }
        Err(error) if is_stale_thread_error(&error) => {
            let _ =
                clear_thread_binding_with_reason(state, &route.conversation_key, "stale_thread")
                    .await;
            TurnStartOutcome::Stale { thread_id }
        }
        Err(error) => {
            state.runtime.lock().await.clear_turn_starting(&thread_id);
            TurnStartOutcome::Failed { error }
        }
    }
}

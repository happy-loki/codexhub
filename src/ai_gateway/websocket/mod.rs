mod log;
pub(super) mod native;

use std::time::{Duration, Instant};

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as UpstreamMessage;

use super::{
    config::ProviderConfig, context::GatewayContext, error::GatewayError, handler,
    router::resolve_provider_with_state,
};
use crate::app_state::SharedState;

const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub(super) async fn handle_upgrade(
    State(state): State<SharedState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let config = state.config.lock().await.ai_gateway.clone();
    if !config.enabled {
        return (StatusCode::SERVICE_UNAVAILABLE, "AI Gateway is disabled").into_response();
    }
    if !config
        .providers
        .iter()
        .any(|p| p.enabled && p.provider_type.is_openai())
    {
        return (
            StatusCode::UPGRADE_REQUIRED,
            "WebSocket requires an OpenAI Responses or ChatGPT account channel; use HTTP Responses",
        )
            .into_response();
    }
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| serve(socket, state, headers, Session::default()))
        .into_response()
}

async fn serve(
    mut downstream: WebSocket,
    state: SharedState,
    headers: HeaderMap,
    mut session: Session,
) {
    if let Err(error) = session.run(&mut downstream, &state, &headers).await {
        tracing::warn!(code = %error.code, "AI Gateway Responses WebSocket ended with an error");
        // A post-upgrade JSON 426 is not an HTTP 426 handshake. Let Codex handle
        // the transport close through its own stream retry/fallback policy.
        if error.status != StatusCode::UPGRADE_REQUIRED {
            let event = json!({"type":"error","status":error.status.as_u16(),"error":{"type":error.error_type,"code":error.code,"message":error.message}});
            let _ = send_event(&mut downstream, &event, &Value::Null).await;
        }
    }
    // Dropping the upstream immediately also cancels an unfinished generation.
    drop(session);
    let _ = tokio::time::timeout(WRITE_TIMEOUT, downstream.send(Message::Close(None))).await;
}

#[derive(Default)]
struct Session {
    selected: Option<(ProviderConfig, String)>,
    native: Option<native::NativeConnection>,
    last_native_response: Option<String>,
}

impl Session {
    async fn run(
        &mut self,
        downstream: &mut WebSocket,
        state: &SharedState,
        headers: &HeaderMap,
    ) -> Result<(), GatewayError> {
        loop {
            let incoming = tokio::select! {
                incoming = downstream.recv() => incoming,
                upstream = async {
                    match &mut self.native {
                        Some(connection) => connection.socket.next().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match upstream {
                        Some(Ok(UpstreamMessage::Ping(_))) => {
                            self.native.as_mut().unwrap().socket.flush().await.map_err(upstream_error)?;
                            continue;
                        }
                        Some(Ok(UpstreamMessage::Pong(_))) => continue,
                        Some(Ok(UpstreamMessage::Text(text))) => {
                            send_text(downstream, text).await?;
                            continue;
                        }
                        _ => return Ok(()),
                    }
                }
                _ = tokio::time::sleep(IDLE_TIMEOUT) => return Ok(()),
            };
            let text = match incoming {
                Some(Ok(Message::Text(text))) => text,
                Some(Ok(Message::Ping(bytes))) => {
                    downstream
                        .send(Message::Pong(bytes))
                        .await
                        .map_err(upstream_error)?;
                    continue;
                }
                Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                Some(Err(error)) => return Err(upstream_error(error)),
                _ => {
                    return Err(GatewayError::bad_request(
                        "Responses WebSocket expects JSON text frames",
                    ));
                }
            };
            let mut request: Value = serde_json::from_str(&text)
                .map_err(|_| GatewayError::bad_request("invalid WebSocket JSON"))?;
            if request["type"] != "response.create" {
                return Err(GatewayError::bad_request("expected response.create"));
            }
            let model = request["model"]
                .as_str()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| GatewayError::bad_request("response.create requires model"))?
                .to_string();
            let turn_headers = request_headers(headers, &request);
            let context =
                GatewayContext::extract(&turn_headers, request["prompt_cache_key"].as_str());
            let config = state.config.lock().await.ai_gateway.clone();
            if !config.enabled {
                return Err(GatewayError::bad_request("AI Gateway is disabled"));
            }
            let previous = request.get("previous_response_id").filter(|v| !v.is_null());
            let (provider, route_id) = if let Some(previous) = previous {
                let expected = self.last_native_response.as_deref();
                if expected.is_none() || previous.as_str() != expected {
                    return Err(previous_response_error());
                }
                let (provider, route_id) =
                    self.selected.as_ref().ok_or_else(previous_response_error)?;
                if !config.providers.iter().any(|p| {
                    p.enabled
                        && super::config::provider_route_id(p) == *route_id
                        && same_credentials(p, provider)
                        && p.matches_model(&model)
                }) {
                    return Err(previous_response_error());
                }
                (provider.clone(), route_id.clone())
            } else {
                let mut routing = state.ai_gateway_routing.lock().await;
                let now = Instant::now();
                routing.evict_stale(now);
                let (provider, route_id) = resolve_provider_with_state(
                    &model,
                    context.session_id.as_deref(),
                    &config,
                    &mut routing,
                    now,
                )?;
                (provider.clone(), route_id)
            };
            if self.selected.as_ref().is_some_and(|(old_provider, old)| {
                old != &route_id || !same_credentials(old_provider, &provider)
            }) {
                self.native = None;
                self.last_native_response = None;
            }
            self.selected = Some((provider.clone(), route_id.clone()));
            if provider.provider_type.is_openai() {
                let mut log = log::TurnLog::new(
                    &state.ai_gateway_request_logs,
                    &config,
                    &context,
                    &turn_headers,
                    &provider,
                    &request,
                );
                native::prepare(
                    &mut request,
                    &provider,
                    &context,
                    config.filter_image_generation_tool,
                );
                let result = async {
                    if self.native.is_none() {
                        let connect = native::connect(&context, &provider);
                        tokio::pin!(connect);
                        self.native = Some(loop {
                            tokio::select! {
                                result = &mut connect => break result?,
                                incoming = downstream.recv() => match incoming {
                                    Some(Ok(Message::Ping(bytes))) => downstream.send(Message::Pong(bytes)).await.map_err(upstream_error)?,
                                    Some(Ok(Message::Pong(_))) => {},
                                    _ => return Err(client_disconnected()),
                                }
                            }
                        });
                    }
                    let connection = self.native.as_mut().unwrap();
                    log.request(&request, &connection.request_headers, &connection.response_headers);
                    let (id, outcome) = native_turn(downstream, connection, &request, &mut log, provider.timeout_secs).await?;
                    self.last_native_response = id;
                    Ok(outcome)
                }.await;
                handler::record_routing_outcome(
                    state,
                    &route_id,
                    result
                        .as_ref()
                        .copied()
                        .unwrap_or_else(|_| handler::classify_outcome(&result)),
                )
                .await;
                if let Err(error) = &result {
                    log.fail(&error.message);
                    if self.native.is_none() && error.status.as_u16() != 499 {
                        tracing::info!(provider = %provider.name, status = %error.status,
                            "upstream WebSocket handshake failed; closing for Codex HTTP fallback");
                        return Err(GatewayError::upstream(
                            StatusCode::UPGRADE_REQUIRED,
                            "upstream WebSocket handshake failed; use HTTP Responses",
                        ));
                    }
                }
                result?;
            } else {
                return Err(GatewayError::upstream(
                    StatusCode::UPGRADE_REQUIRED,
                    "WebSocket requires an OpenAI Responses or ChatGPT account channel; use HTTP Responses",
                ));
            }
        }
    }
}

fn same_credentials(left: &ProviderConfig, right: &ProviderConfig) -> bool {
    left.chatgpt_auth_id == right.chatgpt_auth_id && left.api_key == right.api_key
}

async fn native_turn(
    downstream: &mut WebSocket,
    connection: &mut native::NativeConnection,
    request: &Value,
    log: &mut log::TurnLog,
    timeout_secs: u64,
) -> Result<(Option<String>, handler::RoutingOutcome), GatewayError> {
    tokio::time::timeout(
        WRITE_TIMEOUT,
        connection
            .socket
            .send(UpstreamMessage::Text(request.to_string())),
    )
    .await
    .map_err(|_| GatewayError::upstream_timeout())?
    .map_err(upstream_error)?;
    let idle = tokio::time::sleep(Duration::from_secs(timeout_secs.max(1)));
    tokio::pin!(idle);
    loop {
        tokio::select! {
            incoming = connection.socket.next() => {
                match incoming {
                    Some(Ok(UpstreamMessage::Text(text))) => {
                        idle.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(timeout_secs.max(1)));
                        let mut event: Value = serde_json::from_str(&text).map_err(upstream_error)?;
                        if !event.is_object() { return Err(upstream_error("upstream WebSocket event must be a JSON object")); }
                        log.event(&event);
                        if !connection.metadata_sent {
                            native::add_response_headers(&mut event, &connection.response_headers);
                            connection.metadata_sent = true;
                        }
                        send_event(downstream, &event, request).await?;
                        if is_terminal(&event) {
                            let outcome = match event["type"].as_str() {
                                Some("error" | "response.failed") => {
                                    let status = event["status"].as_u64().and_then(|s| u16::try_from(s).ok()).and_then(|s| StatusCode::from_u16(s).ok()).unwrap_or(StatusCode::BAD_GATEWAY);
                                    handler::classify_outcome::<()>(&Err(GatewayError::upstream(status, "upstream response failed")))
                                }
                                _ => handler::RoutingOutcome::Success,
                            };
                            return Ok((event.pointer("/response/id").and_then(Value::as_str).map(str::to_owned), outcome));
                        }
                    }
                    Some(Ok(UpstreamMessage::Ping(_))) => { connection.socket.flush().await.map_err(upstream_error)?; }
                    Some(Ok(UpstreamMessage::Pong(_))) => {}
                    Some(Err(error)) => return Err(upstream_error(error)),
                    _ => return Err(upstream_error("upstream WebSocket closed before response completed")),
                }
            }
            incoming = downstream.recv() => {
                match incoming {
                    Some(Ok(Message::Ping(bytes))) => downstream.send(Message::Pong(bytes)).await.map_err(upstream_error)?,
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Text(text))) => {
                        let event: Value = serde_json::from_str(&text).map_err(|_| GatewayError::bad_request("invalid WebSocket JSON"))?;
                        if event["type"] == "response.create" { return Err(GatewayError::bad_request("only one response.create may be active per connection")); }
                        tokio::time::timeout(WRITE_TIMEOUT, connection.socket.send(UpstreamMessage::Text(text.to_string())))
                            .await.map_err(|_| GatewayError::upstream_timeout())?.map_err(upstream_error)?;
                    }
                    _ => return Err(client_disconnected()),
                }
            }
            _ = &mut idle => return Err(GatewayError::upstream_timeout()),
        }
    }
}

fn is_terminal(event: &Value) -> bool {
    matches!(
        event["type"].as_str(),
        Some("response.completed" | "response.failed" | "response.incomplete" | "error")
    )
}

fn upstream_error(error: impl std::fmt::Display) -> GatewayError {
    GatewayError::upstream(
        StatusCode::BAD_GATEWAY,
        format!("Responses WebSocket: {error}"),
    )
}

fn client_disconnected() -> GatewayError {
    GatewayError::upstream(
        StatusCode::from_u16(499).unwrap(),
        "client disconnected before response completed",
    )
}

fn previous_response_error() -> GatewayError {
    let mut error = GatewayError::bad_request(
        "previous_response_id is unavailable on this connection/account; retry with the full input",
    );
    error.code = "previous_response_not_found".into();
    error
}

fn request_headers(headers: &HeaderMap, request: &Value) -> HeaderMap {
    let mut headers = headers.clone();
    for name in [
        "x-codex-turn-state",
        "x-codex-turn-metadata",
        "x-codex-beta-features",
        "x-openai-internal-codex-responses-lite",
        "session_id",
        "session-id",
        "thread-id",
        "x-client-request-id",
    ] {
        if let Some(value) = request["client_metadata"][name]
            .as_str()
            .and_then(|s| axum::http::HeaderValue::from_str(s).ok())
        {
            headers.insert(axum::http::HeaderName::from_static(name), value);
        }
    }
    headers
}

async fn send_event(
    socket: &mut WebSocket,
    event: &Value,
    request: &Value,
) -> Result<(), GatewayError> {
    let mut event = event.clone();
    if event.get("stream_id").is_none()
        && let Some(id) = request.get("stream_id")
    {
        event["stream_id"] = id.clone();
    }
    send_text(socket, event.to_string()).await
}

async fn send_text(socket: &mut WebSocket, text: String) -> Result<(), GatewayError> {
    tokio::time::timeout(WRITE_TIMEOUT, socket.send(Message::Text(text.into())))
        .await
        .map_err(|_| GatewayError::upstream_timeout())?
        .map_err(upstream_error)
}

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use base64::Engine;
use serde_json::{Value, json};

use super::outbound::ack_server_envelope;
use super::*;
use crate::{
    app_state::{AppState, PendingRemoteRequest},
    config::AppConfig,
    im_runtime::RouteTarget,
    types::ImPlatformKind,
};

fn test_state() -> SharedState {
    let mut config = AppConfig::default();
    config.state_path = std::env::temp_dir().join(format!("codexhub-test-{}.json", uuid_like()));
    AppState::new(
        std::env::temp_dir().join("codexhub-test-config.toml"),
        config,
        None,
    )
}

fn remote_inner_for_test(stream_id: &str) -> RemoteControlInner {
    RemoteControlInner {
        connections: HashMap::new(),
        active_connection_id: None,
        next_connection_epoch: 0,
        pending_source_hints_by_installation: HashMap::new(),
        connected: false,
        initialized: false,
        client_id: FEISHU_BRIDGE_CLIENT_ID.to_string(),
        stream_id: stream_id.to_string(),
        server_id: None,
        environment_id: None,
        server_name: None,
        installation_id: None,
        account_id: None,
        current_thread_id: None,
        current_turn_id: None,
        last_error: None,
        connected_at_ms: None,
        last_ws_inbound_at_ms: None,
        last_ws_ping_at_ms: None,
        last_ws_pong_at_ms: None,
        last_app_ping_at_ms: None,
        last_app_pong_at_ms: None,
        last_app_pong_status: None,
        last_initialize_sent_at_ms: None,
        subscribe_cursor: None,
        server_ack_cursors: HashMap::new(),
        outbound_tx: None,
        connection_epoch: 0,
        clients: HashMap::new(),
        authorized_clients: HashMap::new(),
        revoked_clients: std::collections::HashSet::new(),
        stream_diagnostics: HashMap::new(),
        recent_events: std::collections::VecDeque::new(),
    }
}

fn test_connection(
    id: &str,
    connection_epoch: u64,
    connected: bool,
    initialized: bool,
    source_kind: crate::app_state::RemoteControlSourceKind,
    outbound_tx: Option<tokio::sync::mpsc::UnboundedSender<OutboundWsMessage>>,
) -> crate::app_state::RemoteControlServerConnection {
    crate::app_state::RemoteControlServerConnection {
        connection_id: id.to_string(),
        connection_epoch,
        connected,
        initialized,
        source_kind,
        user_agent: None,
        server_id: None,
        environment_id: None,
        server_name: None,
        installation_id: None,
        account_id: None,
        subscribe_cursor: None,
        outbound_tx,
        connected_at_ms: Some(connection_epoch as u128),
        last_ws_inbound_at_ms: Some(connection_epoch as u128),
        last_ws_ping_at_ms: None,
        last_ws_pong_at_ms: None,
        last_error: None,
        clients: HashMap::new(),
        stream_diagnostics: HashMap::new(),
    }
}

fn test_server_message_envelope(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    message: Value,
) -> String {
    json!({
        "type": "server_message",
        "client_id": client_id,
        "stream_id": stream_id,
        "seq_id": seq_id,
        "message": message,
    })
    .to_string()
}

fn test_server_chunk_envelope(
    client_id: &str,
    stream_id: &str,
    seq_id: u64,
    segment_id: usize,
    segment_count: usize,
    message_size_bytes: usize,
    chunk: &[u8],
) -> String {
    json!({
        "type": "server_message_chunk",
        "client_id": client_id,
        "stream_id": stream_id,
        "seq_id": seq_id,
        "segment_id": segment_id,
        "segment_count": segment_count,
        "message_size_bytes": message_size_bytes,
        "message_chunk_base64": base64::engine::general_purpose::STANDARD.encode(chunk),
    })
    .to_string()
}

fn take_text_envelopes(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<OutboundWsMessage>,
) -> Vec<Value> {
    let mut values = Vec::new();
    while let Ok(message) = rx.try_recv() {
        if let OutboundWsMessage::Text(value) = message {
            values.push(value);
        }
    }
    values
}

fn envelope_message_method(envelope: &Value) -> Option<&str> {
    envelope
        .get("message")
        .and_then(|message| message.get("method"))
        .and_then(Value::as_str)
}

fn envelope_is_ack(envelope: &Value) -> bool {
    envelope.get("type").and_then(Value::as_str) == Some("ack")
}

async fn setup_connected_default_client(
    state: &SharedState,
) -> (
    tokio::sync::mpsc::UnboundedSender<OutboundWsMessage>,
    tokio::sync::mpsc::UnboundedReceiver<OutboundWsMessage>,
    String,
    String,
    u64,
) {
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::unbounded_channel();
    let (client_id, stream_id, connection_epoch) = {
        let mut remote = state.remote_control.inner.lock().await;
        remote.connected = true;
        remote.connection_epoch = 7;
        remote.outbound_tx = Some(outbound_tx.clone());
        remote.stream_id = "stream-test".to_string();
        let client = ensure_client_state_locked(&mut remote, DEFAULT_REMOTE_CLIENT_KEY);
        client.initialized = true;
        let client_id = client.client_id.clone();
        let stream_id = client.stream_id.clone();
        sync_default_client_legacy_locked(&mut remote);
        (client_id, stream_id, remote.connection_epoch)
    };
    (
        outbound_tx,
        outbound_rx,
        client_id,
        stream_id,
        connection_epoch,
    )
}

#[test]
fn ack_cursor_orders_whole_message_after_chunks() {
    assert!(ack_cursor_gt((2, None), (1, None)));
    assert!(ack_cursor_gt((2, None), (2, Some(7))));
    assert!(ack_cursor_gt((2, Some(8)), (2, Some(7))));
    assert!(!ack_cursor_gt((2, Some(7)), (2, None)));
    assert!(!ack_cursor_gt((1, None), (2, Some(0))));
}

#[test]
fn observe_server_chunk_reassembles_json_message() {
    let message = json!({
        "method": "turn/completed",
        "params": {
            "threadId": "thread-1"
        }
    });
    let raw = serde_json::to_vec(&message).expect("serialize message");
    let split_at = raw.len() / 2;
    let first = base64::engine::general_purpose::STANDARD.encode(&raw[..split_at]);
    let second = base64::engine::general_purpose::STANDARD.encode(&raw[split_at..]);
    let mut chunks = HashMap::new();

    let pending = observe_server_chunk(
        &mut chunks,
        "client-1",
        "stream-1",
        1,
        0,
        2,
        raw.len(),
        &first,
    );
    assert!(matches!(pending, ServerChunkObservation::Pending));

    let complete = observe_server_chunk(
        &mut chunks,
        "client-1",
        "stream-1",
        1,
        1,
        2,
        raw.len(),
        &second,
    );
    match complete {
        ServerChunkObservation::Complete(complete) => assert_eq!(complete, message),
        ServerChunkObservation::Pending | ServerChunkObservation::Dropped => {
            panic!("expected complete message")
        }
    }
    assert!(chunks.is_empty());
}

#[test]
fn observe_server_chunk_rejects_size_overflow() {
    let chunk = base64::engine::general_purpose::STANDARD.encode(b"too-large");
    let mut chunks = HashMap::new();
    let observation = observe_server_chunk(&mut chunks, "client-1", "stream-1", 1, 0, 1, 1, &chunk);
    assert!(matches!(observation, ServerChunkObservation::Dropped));
    assert!(chunks.is_empty());
}

#[test]
fn observe_server_chunk_ignores_duplicate_without_dropping_current_assembly() {
    let message = json!({"method": "turn/completed", "params": {"threadId": "thread-1"}});
    let raw = serde_json::to_vec(&message).expect("serialize message");
    let split_at = raw.len() / 2;
    let first = base64::engine::general_purpose::STANDARD.encode(&raw[..split_at]);
    let second = base64::engine::general_purpose::STANDARD.encode(&raw[split_at..]);
    let mut chunks = HashMap::new();

    assert!(matches!(
        observe_server_chunk(
            &mut chunks,
            "client-1",
            "stream-1",
            8,
            0,
            2,
            raw.len(),
            &first,
        ),
        ServerChunkObservation::Pending
    ));
    assert!(matches!(
        observe_server_chunk(&mut chunks, "client-1", "stream-1", 8, 0, 2, raw.len(), "",),
        ServerChunkObservation::Dropped
    ));
    match observe_server_chunk(
        &mut chunks,
        "client-1",
        "stream-1",
        8,
        1,
        2,
        raw.len(),
        &second,
    ) {
        ServerChunkObservation::Complete(complete) => assert_eq!(complete, message),
        ServerChunkObservation::Pending | ServerChunkObservation::Dropped => {
            panic!("duplicate chunk should not drop current assembly")
        }
    }
}

#[test]
fn recovery_retry_policy_does_not_replay_non_idempotent_requests() {
    assert!(!should_retry_request_after_reinitialize("turn/start"));
    assert!(!should_retry_request_after_reinitialize("turn/steer"));
    assert!(!should_retry_request_after_reinitialize("thread/start"));
    assert!(!should_retry_request_after_reinitialize("thread/fork"));
    assert!(should_retry_request_after_reinitialize("thread/list"));
    assert!(should_retry_request_after_reinitialize("thread/resume"));
}

#[test]
fn virtual_remote_clients_share_enrolled_client_id_and_use_distinct_streams() {
    let mut remote = remote_inner_for_test("default-stream");

    let feishu = ensure_client_state_locked(&mut remote, "feishu:default:chat-1");
    let feishu_client_id = feishu.client_id.clone();
    let feishu_stream_id = feishu.stream_id.clone();
    let wechat = ensure_client_state_locked(&mut remote, "wechat:bot:user-1");
    let wechat_client_id = wechat.client_id.clone();
    let wechat_stream_id = wechat.stream_id.clone();

    assert_eq!(feishu_client_id, FEISHU_BRIDGE_CLIENT_ID);
    assert_eq!(wechat_client_id, FEISHU_BRIDGE_CLIENT_ID);
    assert_ne!(feishu_stream_id, wechat_stream_id);
    assert_eq!(
        remote_client_key_for_stream_locked(&remote, &feishu_client_id, &feishu_stream_id)
            .as_deref(),
        Some("feishu:default:chat-1")
    );
    assert_eq!(
        remote_client_key_for_stream_locked(&remote, &wechat_client_id, &wechat_stream_id)
            .as_deref(),
        Some("wechat:bot:user-1")
    );
}

#[test]
fn virtual_remote_client_stream_is_namespaced_by_connection_stream() {
    let mut first = remote_inner_for_test("default-stream-1");
    let mut second = remote_inner_for_test("default-stream-2");
    let client_key = "wechat:bot:user-1";
    let first_stream = ensure_client_state_locked(&mut first, client_key)
        .stream_id
        .clone();
    let second_stream = ensure_client_state_locked(&mut second, client_key)
        .stream_id
        .clone();

    assert_ne!(first_stream, second_stream);
}

#[test]
fn connection_reset_removes_stale_initialize_state_but_keeps_replayable_requests() {
    let mut remote = RemoteControlInner {
        connections: HashMap::new(),
        active_connection_id: None,
        next_connection_epoch: 0,
        pending_source_hints_by_installation: HashMap::new(),
        connected: false,
        initialized: false,
        client_id: FEISHU_BRIDGE_CLIENT_ID.to_string(),
        stream_id: "default-stream".to_string(),
        server_id: None,
        environment_id: None,
        server_name: None,
        installation_id: None,
        account_id: None,
        current_thread_id: None,
        current_turn_id: None,
        last_error: None,
        connected_at_ms: None,
        last_ws_inbound_at_ms: None,
        last_ws_ping_at_ms: None,
        last_ws_pong_at_ms: None,
        last_app_ping_at_ms: None,
        last_app_pong_at_ms: None,
        last_app_pong_status: None,
        last_initialize_sent_at_ms: None,
        subscribe_cursor: None,
        server_ack_cursors: HashMap::new(),
        outbound_tx: None,
        connection_epoch: 0,
        clients: HashMap::new(),
        authorized_clients: HashMap::new(),
        revoked_clients: std::collections::HashSet::new(),
        stream_diagnostics: HashMap::new(),
        recent_events: std::collections::VecDeque::new(),
    };
    let client = ensure_client_state_locked(&mut remote, DEFAULT_REMOTE_CLIENT_KEY);
    client.initialized = true;
    client.last_app_ping_at_ms = Some(10);
    client.last_app_pong_at_ms = Some(11);
    client.last_app_pong_status = Some("active".to_string());
    client.last_initialize_sent_at_ms = Some(12);
    let (initialize_tx, _initialize_rx) = tokio::sync::oneshot::channel();
    client.pending.insert(
        "1".to_string(),
        PendingRemoteRequest {
            method: "initialize".to_string(),
            thread_id: None,
            track_thread_active: false,
            response_tx: initialize_tx,
            message: json!({"id": 1, "method": "initialize"}),
            envelopes: Vec::new(),
        },
    );
    let (request_tx, _request_rx) = tokio::sync::oneshot::channel();
    client.pending.insert(
        "2".to_string(),
        PendingRemoteRequest {
            method: "thread/list".to_string(),
            thread_id: None,
            track_thread_active: true,
            response_tx: request_tx,
            message: json!({"id": 2, "method": "thread/list"}),
            envelopes: Vec::new(),
        },
    );

    let ack_keys = reset_remote_clients_for_connection_locked(&mut remote);
    let client = remote
        .clients
        .get(DEFAULT_REMOTE_CLIENT_KEY)
        .expect("default client");

    assert_eq!(ack_keys.len(), 1);
    assert!(!client.initialized);
    assert!(client.last_app_ping_at_ms.is_none());
    assert!(client.last_app_pong_at_ms.is_none());
    assert!(client.last_app_pong_status.is_none());
    assert!(client.last_initialize_sent_at_ms.is_none());
    assert!(!client.pending.contains_key("1"));
    assert!(client.pending.contains_key("2"));
}

#[tokio::test]
async fn record_remote_app_pong_unknown_requests_reinitialize_after_initialize() {
    let state = test_state();
    {
        let mut remote = state.remote_control.inner.lock().await;
        remote.connection_epoch = 7;
        remote.connected = true;
        let client = ensure_client_state_locked(&mut remote, DEFAULT_REMOTE_CLIENT_KEY);
        client.initialized = true;
        let client_id = client.client_id.clone();
        let stream_id = client.stream_id.clone();
        sync_default_client_legacy_locked(&mut remote);
        drop(remote);
        assert!(
            record_remote_app_pong(&state, 7, &client_id, &stream_id, "unknown")
                .await
                .expect("record pong")
        );
    }
    let remote = state.remote_control.inner.lock().await;
    assert_eq!(
        remote
            .clients
            .get(DEFAULT_REMOTE_CLIENT_KEY)
            .and_then(|client| client.last_app_pong_status.as_deref()),
        Some("unknown")
    );
}

#[tokio::test]
async fn unknown_reinitializes_same_stream_without_client_closed() {
    let state = test_state();
    let (_outbound_tx, mut outbound_rx, client_id, stream_id, connection_epoch) =
        setup_connected_default_client(&state).await;

    start_remote_control_client_recovery(
        &state,
        connection_epoch,
        DEFAULT_REMOTE_CLIENT_KEY,
        &client_id,
        &stream_id,
    )
    .await
    .expect("recovery should start");

    let envelopes = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let envelopes = take_text_envelopes(&mut outbound_rx);
            if envelopes
                .iter()
                .any(|envelope| envelope_message_method(envelope) == Some("initialize"))
            {
                return envelopes;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("initialize should be sent");

    let initialize = envelopes
        .iter()
        .find(|envelope| envelope_message_method(envelope) == Some("initialize"))
        .expect("initialize envelope");
    assert_eq!(initialize["client_id"], client_id);
    assert_eq!(initialize["stream_id"], stream_id);
    assert!(
        envelopes
            .iter()
            .all(|envelope| envelope.get("type").and_then(Value::as_str) != Some("client_closed"))
    );

    let remote = state.remote_control.inner.lock().await;
    let client = remote
        .clients
        .get(DEFAULT_REMOTE_CLIENT_KEY)
        .expect("default client");
    assert_eq!(client.stream_id, stream_id);
    assert!(!client.initialized);
    assert_eq!(client.recovery_attempt, 1);
    assert!(client.recovery_started_at_ms.is_some());
}

#[tokio::test]
async fn unbound_thread_started_does_not_replace_bound_im_thread() {
    let state = test_state();
    let (_outbound_tx, _outbound_rx, client_id, stream_id, connection_epoch) =
        setup_connected_default_client(&state).await;
    {
        let mut runtime = state.runtime.lock().await;
        runtime.bind_route(
            "thread-1",
            RouteTarget {
                platform: ImPlatformKind::Feishu,
                conversation_key: "feishu:default:chat-1".to_string(),
                account_id: "default".to_string(),
                chat_id: "chat-1".to_string(),
                remote_client_key: DEFAULT_REMOTE_CLIENT_KEY.to_string(),
            },
        );
    }
    {
        let mut remote = state.remote_control.inner.lock().await;
        let client = remote
            .clients
            .get_mut(DEFAULT_REMOTE_CLIENT_KEY)
            .expect("default client");
        client.current_thread_id = Some("thread-1".to_string());
        sync_default_client_legacy_locked(&mut remote);
    }

    observe_app_server_message(
        &state,
        connection_epoch,
        &client_id,
        &stream_id,
        &json!({
            "method": "thread/started",
            "params": {
                "thread": {
                    "id": "unbound-thread",
                    "status": {
                        "type": "idle"
                    }
                }
            }
        }),
    )
    .await;

    let remote = state.remote_control.inner.lock().await;
    let client = remote
        .clients
        .get(DEFAULT_REMOTE_CLIENT_KEY)
        .expect("default client");
    assert_eq!(client.current_thread_id.as_deref(), Some("thread-1"));
}

#[tokio::test]
async fn non_owner_thread_notification_is_not_forwarded_to_im() {
    let state = test_state();
    let (_outbound_tx, _outbound_rx, client_id, _default_stream_id, connection_epoch) =
        setup_connected_default_client(&state).await;
    let feishu_key = "im:feishu:owner-chat";
    let wechat_key = "im:wechat:other-chat";
    let (feishu_stream_id, wechat_stream_id) = {
        let mut remote = state.remote_control.inner.lock().await;
        let feishu_client = ensure_client_state_locked(&mut remote, feishu_key);
        feishu_client.initialized = true;
        let feishu_stream_id = feishu_client.stream_id.clone();
        let wechat_client = ensure_client_state_locked(&mut remote, wechat_key);
        wechat_client.initialized = true;
        let wechat_stream_id = wechat_client.stream_id.clone();
        (feishu_stream_id, wechat_stream_id)
    };
    {
        let mut runtime = state.runtime.lock().await;
        runtime.bind_route(
            "thread-feishu",
            RouteTarget {
                platform: ImPlatformKind::Feishu,
                conversation_key: "feishu:default:chat-1".to_string(),
                account_id: "default".to_string(),
                chat_id: "chat-1".to_string(),
                remote_client_key: feishu_key.to_string(),
            },
        );
    }
    let mut notifications = state.remote_control.notifications.subscribe();

    observe_app_server_message(
        &state,
        connection_epoch,
        &client_id,
        &wechat_stream_id,
        &json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-feishu",
                "itemId": "message-1",
                "delta": "hello"
            }
        }),
    )
    .await;

    assert!(matches!(
        notifications.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    observe_app_server_message(
        &state,
        connection_epoch,
        &client_id,
        &feishu_stream_id,
        &json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-feishu",
                "itemId": "message-1",
                "delta": "hello"
            }
        }),
    )
    .await;

    let notification = notifications
        .try_recv()
        .expect("owner notification should be forwarded");
    assert_eq!(notification.method, "item/agentMessage/delta");
    assert_eq!(notification.remote_client_key.as_deref(), Some(feishu_key));
}

#[tokio::test]
async fn non_owner_thread_server_request_is_not_forwarded_to_im() {
    let state = test_state();
    let (_outbound_tx, _outbound_rx, client_id, _default_stream_id, connection_epoch) =
        setup_connected_default_client(&state).await;
    let feishu_key = "im:feishu:owner-chat";
    let wechat_key = "im:wechat:other-chat";
    let (feishu_stream_id, wechat_stream_id) = {
        let mut remote = state.remote_control.inner.lock().await;
        let feishu_client = ensure_client_state_locked(&mut remote, feishu_key);
        feishu_client.initialized = true;
        let feishu_stream_id = feishu_client.stream_id.clone();
        let wechat_client = ensure_client_state_locked(&mut remote, wechat_key);
        wechat_client.initialized = true;
        let wechat_stream_id = wechat_client.stream_id.clone();
        (feishu_stream_id, wechat_stream_id)
    };
    {
        let mut runtime = state.runtime.lock().await;
        runtime.bind_route(
            "thread-feishu",
            RouteTarget {
                platform: ImPlatformKind::Feishu,
                conversation_key: "feishu:default:chat-1".to_string(),
                account_id: "default".to_string(),
                chat_id: "chat-1".to_string(),
                remote_client_key: feishu_key.to_string(),
            },
        );
    }
    let mut notifications = state.remote_control.notifications.subscribe();

    observe_app_server_message(
        &state,
        connection_epoch,
        &client_id,
        &wechat_stream_id,
        &json!({
            "id": "server-request-1",
            "method": "approval/requested",
            "params": {
                "threadId": "thread-feishu"
            }
        }),
    )
    .await;

    assert!(matches!(
        notifications.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));

    observe_app_server_message(
        &state,
        connection_epoch,
        &client_id,
        &feishu_stream_id,
        &json!({
            "id": "server-request-2",
            "method": "approval/requested",
            "params": {
                "threadId": "thread-feishu"
            }
        }),
    )
    .await;

    let notification = notifications
        .try_recv()
        .expect("owner server request should be forwarded");
    assert_eq!(notification.method, "approval/requested");
    assert_eq!(notification.remote_client_key.as_deref(), Some(feishu_key));
    assert_eq!(
        notification.request_id.as_ref().and_then(Value::as_str),
        Some("server-request-2")
    );
}

#[tokio::test]
async fn recovery_resubscribes_bound_threads_without_changing_current_session() {
    let state = test_state();
    let (_outbound_tx, mut outbound_rx, client_id, stream_id, connection_epoch) =
        setup_connected_default_client(&state).await;
    {
        let mut remote = state.remote_control.inner.lock().await;
        let client = remote
            .clients
            .get_mut(DEFAULT_REMOTE_CLIENT_KEY)
            .expect("default client");
        client.current_thread_id = Some("unbound-thread".to_string());
        client.current_turn_id = Some("unbound-turn".to_string());
        sync_default_client_legacy_locked(&mut remote);
    }
    {
        let mut runtime = state.runtime.lock().await;
        runtime.bind_route(
            "thread-1",
            RouteTarget {
                platform: ImPlatformKind::Feishu,
                conversation_key: "feishu:default:chat-1".to_string(),
                account_id: "default".to_string(),
                chat_id: "chat-1".to_string(),
                remote_client_key: DEFAULT_REMOTE_CLIENT_KEY.to_string(),
            },
        );
        runtime.mark_turn_started("thread-1", "turn-1");
    }

    let resubscribe_state = state.clone();
    let resubscribe = tokio::spawn(async move {
        resubscribe_bound_threads_after_recovery(
            &resubscribe_state,
            connection_epoch,
            DEFAULT_REMOTE_CLIENT_KEY,
            1,
        )
        .await
    });

    let envelopes = tokio::time::timeout(Duration::from_secs(1), async {
        let mut seen = Vec::new();
        loop {
            seen.extend(take_text_envelopes(&mut outbound_rx));
            if seen
                .iter()
                .any(|envelope| envelope_message_method(envelope) == Some("thread/resume"))
            {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("thread/resume should be sent");

    assert!(
        envelopes
            .iter()
            .all(|envelope| envelope_message_method(envelope) != Some("turn/start"))
    );
    let resume = envelopes
        .iter()
        .find(|envelope| envelope_message_method(envelope) == Some("thread/resume"))
        .expect("thread/resume envelope");
    assert_eq!(resume["client_id"], client_id);
    assert_eq!(resume["stream_id"], stream_id);
    assert_eq!(resume["message"]["params"]["threadId"], "thread-1");
    assert_ne!(
        resume["message"]["params"]["threadId"],
        Value::String("unbound-thread".to_string())
    );
    assert_eq!(resume["message"]["params"]["excludeTurns"], true);
    let request_id = resume["message"]["id"].clone();

    observe_app_server_message(
        &state,
        connection_epoch,
        &client_id,
        &stream_id,
        &json!({
            "id": request_id,
            "result": {
                "thread": {
                    "id": "thread-1",
                    "status": {
                        "type": "idle"
                    }
                }
            }
        }),
    )
    .await;

    tokio::time::timeout(Duration::from_secs(1), resubscribe)
        .await
        .expect("resubscribe task should finish")
        .expect("resubscribe task should not panic")
        .expect("resubscribe should succeed");

    let remote = state.remote_control.inner.lock().await;
    let client = remote
        .clients
        .get(DEFAULT_REMOTE_CLIENT_KEY)
        .expect("default client");
    assert_eq!(client.current_thread_id.as_deref(), Some("unbound-thread"));
    assert_eq!(client.current_turn_id.as_deref(), Some("unbound-turn"));
    drop(remote);
    assert_eq!(
        state
            .runtime
            .lock()
            .await
            .current_turn_by_thread
            .get("thread-1")
            .map(String::as_str),
        Some("turn-1")
    );
}

#[tokio::test]
async fn initialize_remote_clients_for_connection_sends_connection_default_client() {
    let state = test_state();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel();
    let connection_epoch = {
        let mut remote = state.remote_control.inner.lock().await;
        remote.connected = true;
        remote.connection_epoch = 11;
        remote.outbound_tx = Some(outbound_tx);
        remote.stream_id = "stream-root".to_string();
        ensure_client_state_locked(&mut remote, DEFAULT_REMOTE_CLIENT_KEY);
        ensure_client_state_locked(&mut remote, "feishu:default:chat-1");
        ensure_client_state_locked(&mut remote, "wechat:bot:user-1");
        remote.connection_epoch
    };

    initialize_remote_clients_for_connection(&state, connection_epoch)
        .await
        .expect("initialize all clients");

    let envelopes = take_text_envelopes(&mut outbound_rx);
    let initialize_streams = envelopes
        .iter()
        .filter(|envelope| envelope_message_method(envelope) == Some("initialize"))
        .map(|envelope| {
            envelope
                .get("stream_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect::<std::collections::HashSet<_>>();
    let expected_streams = std::collections::HashSet::from(["stream-root".to_string()]);
    assert_eq!(initialize_streams, expected_streams);
}

#[test]
fn active_connection_can_be_selected_before_initialize_completes() {
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut remote = remote_inner_for_test("stream-root");
    remote.connections.insert(
        "conn-pending".to_string(),
        test_connection(
            "conn-pending",
            11,
            true,
            false,
            crate::app_state::RemoteControlSourceKind::Unknown,
            Some(outbound_tx),
        ),
    );

    assert_eq!(
        select_active_connection_id_locked(&remote).as_deref(),
        Some("conn-pending")
    );
}

#[test]
fn inactive_remote_connections_are_not_retained_in_memory() {
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut remote = remote_inner_for_test("stream-root");
    for epoch in 1..=80 {
        let id = format!("conn-{epoch}");
        remote.connections.insert(
            id.clone(),
            test_connection(
                &id,
                epoch,
                false,
                false,
                crate::app_state::RemoteControlSourceKind::Unknown,
                None,
            ),
        );
    }
    remote.connections.insert(
        "conn-active".to_string(),
        test_connection(
            "conn-active",
            10,
            true,
            false,
            crate::app_state::RemoteControlSourceKind::Vscode,
            Some(outbound_tx),
        ),
    );

    prune_inactive_remote_connections_locked(&mut remote);

    assert_eq!(remote.connections.len(), 1);
    assert!(remote.connections.contains_key("conn-active"));
    assert!(!remote.connections.contains_key("conn-1"));
    assert!(!remote.connections.contains_key("conn-80"));
}

#[test]
fn pruning_last_connection_clears_legacy_connected_state() {
    let mut remote = remote_inner_for_test("stream-root");
    remote.connected = true;
    remote.initialized = true;
    remote.connection_epoch = 7;
    let id = "conn-closed";
    remote.connections.insert(
        id.to_string(),
        test_connection(
            id,
            7,
            false,
            true,
            crate::app_state::RemoteControlSourceKind::CodexApp,
            None,
        ),
    );

    prune_inactive_remote_connections_locked(&mut remote);

    assert!(remote.connections.is_empty());
    assert!(!remote.connected);
    assert!(!remote.initialized);
    assert!(remote.outbound_tx.is_none());
}

#[tokio::test]
async fn status_snapshot_excludes_inactive_connections() {
    let state = test_state();
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let mut remote = state.remote_control.inner.lock().await;
        for epoch in 1..=80 {
            let id = format!("conn-{epoch}");
            remote.connections.insert(
                id.clone(),
                test_connection(
                    &id,
                    epoch,
                    false,
                    false,
                    crate::app_state::RemoteControlSourceKind::Unknown,
                    None,
                ),
            );
        }
        remote.connections.insert(
            "conn-active".to_string(),
            test_connection(
                "conn-active",
                10,
                true,
                true,
                crate::app_state::RemoteControlSourceKind::CodexApp,
                Some(outbound_tx),
            ),
        );
    }

    let snapshot = status_snapshot(&state).await;

    assert_eq!(snapshot.connections.len(), 1);
    assert!(
        snapshot
            .connections
            .iter()
            .any(|connection| connection.id == "conn-active")
    );
    assert_eq!(
        snapshot
            .connections
            .first()
            .map(|connection| connection.id.as_str()),
        Some("conn-active")
    );
}

#[tokio::test]
async fn server_flood_fast_ack_does_not_wait_for_work_queue_drain() {
    let state = test_state();
    let (outbound_tx, mut outbound_rx, client_id, stream_id, connection_epoch) =
        setup_connected_default_client(&state).await;
    let (server_work_tx, mut server_work_rx) = tokio::sync::mpsc::channel::<RemoteServerWorkItem>(
        REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY,
    );
    let mut chunks = HashMap::new();

    for seq_id in 1..=300 {
        let message = json!({
            "method": "item/commandExecution/outputDelta",
            "params": {
                "threadId": "thread-1",
                "itemId": format!("item-{seq_id}"),
                "delta": "x"
            }
        });
        handle_server_envelope(
            &state,
            &outbound_tx,
            connection_epoch,
            &test_server_message_envelope(&client_id, &stream_id, seq_id, message),
            &mut chunks,
            &server_work_tx,
        )
        .await
        .expect("server envelope should be acked");
    }

    let ack_count = take_text_envelopes(&mut outbound_rx)
        .iter()
        .filter(|envelope| envelope_is_ack(envelope))
        .count();
    assert_eq!(ack_count, 300);
    assert_eq!(
        server_work_tx.capacity(),
        REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY - 300
    );
    assert_eq!(
        server_work_rx
            .try_recv()
            .ok()
            .map(|item| remote_server_work_item_kind(&item)),
        Some("server_message")
    );

    let remote = state.remote_control.inner.lock().await;
    let key = server_ack_cursor_key(&client_id, &stream_id);
    assert_eq!(remote.server_ack_cursors.get(&key), Some(&(300, None)));
    assert_eq!(
        remote
            .stream_diagnostics
            .get(&key)
            .map(|diagnostics| diagnostics.ack_count),
        Some(300)
    );
}

#[tokio::test]
async fn ack_server_envelope_does_not_wait_for_remote_control_inner_lock() {
    let state = test_state();
    let (outbound_tx, mut outbound_rx, client_id, stream_id, connection_epoch) =
        setup_connected_default_client(&state).await;
    let remote_guard = state.remote_control.inner.lock().await;

    ack_server_envelope(
        &state,
        &outbound_tx,
        connection_epoch,
        &client_id,
        &stream_id,
        42,
        None,
    )
    .await
    .expect("ack should send without taking remote_control.inner");

    let envelope = tokio::time::timeout(Duration::from_millis(100), outbound_rx.recv())
        .await
        .expect("ack should be queued while remote_control.inner is locked")
        .expect("ack envelope");
    drop(remote_guard);
    match envelope {
        OutboundWsMessage::Text(value) => {
            assert!(envelope_is_ack(&value));
            assert_eq!(value["client_id"], client_id);
            assert_eq!(value["stream_id"], stream_id);
            assert_eq!(value["seq_id"], 42);
        }
        _ => panic!("expected text ack envelope"),
    }
}

#[tokio::test]
async fn bad_server_chunk_is_acked_without_closing_connection() {
    let state = test_state();
    let (outbound_tx, mut outbound_rx, client_id, stream_id, connection_epoch) =
        setup_connected_default_client(&state).await;
    let (server_work_tx, mut server_work_rx) = tokio::sync::mpsc::channel::<RemoteServerWorkItem>(
        REMOTE_CONTROL_SERVER_WORK_QUEUE_CAPACITY,
    );
    let mut chunks = HashMap::new();
    let message = json!({"method": "turn/completed", "params": {"threadId": "thread-1"}});
    let raw = serde_json::to_vec(&message).expect("serialize message");
    let split_at = raw.len() / 2;

    handle_server_envelope(
        &state,
        &outbound_tx,
        connection_epoch,
        &test_server_chunk_envelope(&client_id, &stream_id, 1, 0, 2, raw.len(), &raw[..split_at]),
        &mut chunks,
        &server_work_tx,
    )
    .await
    .expect("first chunk should be accepted");
    handle_server_envelope(
        &state,
        &outbound_tx,
        connection_epoch,
        &test_server_chunk_envelope(&client_id, &stream_id, 1, 0, 2, raw.len(), b""),
        &mut chunks,
        &server_work_tx,
    )
    .await
    .expect("duplicate bad chunk should be dropped but acked");

    let ack_count = take_text_envelopes(&mut outbound_rx)
        .iter()
        .filter(|envelope| envelope_is_ack(envelope))
        .count();
    assert_eq!(ack_count, 2);
    assert!(server_work_rx.try_recv().is_err());
    assert!(chunks.contains_key(&(client_id, stream_id, 1)));
}
#[tokio::test]
async fn force_ws_reconnect_sends_close_message() {
    let state = test_state();
    let (_outbound_tx, mut outbound_rx, _client_id, _stream_id, connection_epoch) =
        setup_connected_default_client(&state).await;

    force_remote_control_ws_reconnect(
        &state,
        connection_epoch,
        DEFAULT_REMOTE_CLIENT_KEY,
        "test reconnect",
    )
    .await
    .expect("force reconnect should enqueue close");

    match outbound_rx.try_recv().expect("outbound close message") {
        OutboundWsMessage::Close(reason) => assert_eq!(reason, "test reconnect"),
        OutboundWsMessage::Text(_) | OutboundWsMessage::Ping(_) | OutboundWsMessage::Pong(_) => {
            panic!("expected close message")
        }
    }
}

use super::*;
use crate::ai_gateway::config::{ProviderConfig, ProviderType};
use axum::{Json, Router, routing::get};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Mutex, mpsc, oneshot};

async fn server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, task)
}

fn state(temp: &tempfile::TempDir, providers: Vec<ProviderConfig>) -> SharedState {
    let config = crate::config::AppConfig {
        state_path: temp.path().join("state.json"),
        ai_gateway: crate::ai_gateway::config::AiGatewayConfig {
            enabled: true,
            providers,
            request_logging_enabled: true,
            request_log_details_enabled: true,
            filter_image_generation_tool: true,
            ..Default::default()
        },
        ..Default::default()
    };
    crate::app_state::AppState::new(temp.path().join("config.json"), config, None, None)
}

async fn next_event(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
}

#[test]
fn native_request_preserves_lite_and_incremental_fields() {
    let provider = ProviderConfig {
        provider_type: ProviderType::ChatGptResponses,
        models: vec!["real-model".into()],
        model_aliases: [("client-model".into(), "real-model".into())].into(),
        ..Default::default()
    };
    let context = GatewayContext::extract(&HeaderMap::new(), Some("session-key"));
    let mut request = json!({"type":"response.create","model":"client-model","generate":false,
        "previous_response_id":"resp_previous","stream_id":"stream_1","stream":true,
        "client_metadata":{"x-openai-internal-codex-responses-lite":"true"},
        "tools":[{"type":"custom","name":"apply_patch","format":{"type":"grammar"}}],
        "input":[{"type":"additional_tools","role":"developer","tools":[]},
            {"type":"custom_tool_call_output","call_id":"call_a","output":"done"}],
        "future_field":{"nested":[1,2,3]}});
    let original = request.clone();
    native::prepare(&mut request, &provider, &context, false);
    assert_eq!(request["model"], "real-model");
    assert_eq!(request["prompt_cache_key"], "session-key");
    assert!(request.get("stream").is_none());
    for field in [
        "type",
        "generate",
        "previous_response_id",
        "stream_id",
        "client_metadata",
        "input",
        "tools",
        "future_field",
    ] {
        assert_eq!(request[field], original[field], "{field}");
    }
}

#[test]
fn metadata_cannot_override_credentials_and_event_headers_win() {
    let request = json!({"client_metadata":{"authorization":"secret","chatgpt-account-id":"wrong",
        "x-codex-turn-state":"client-state","session_id":"stable"}});
    let headers = request_headers(&HeaderMap::new(), &request);
    assert!(!headers.contains_key("authorization"));
    assert!(!headers.contains_key("chatgpt-account-id"));
    assert_eq!(headers["session_id"], "stable");
    let headers = HeaderMap::from_iter([
        (
            axum::http::HeaderName::from_static("x-codex-turn-state"),
            "handshake-state".parse().unwrap(),
        ),
        (
            axum::http::HeaderName::from_static("set-cookie"),
            "secret".parse().unwrap(),
        ),
    ]);
    let mut event =
        json!({"type":"response.created","headers":{"x-codex-turn-state":"event-state"}});
    native::add_response_headers(&mut event, &headers);
    assert_eq!(event["headers"]["x-codex-turn-state"], "event-state");
    assert!(event["headers"].get("set-cookie").is_none());
}

#[tokio::test]
async fn websocket_without_openai_channels_returns_426_and_post_stays_available() {
    let temp = tempfile::tempdir().unwrap();
    let state = state(
        &temp,
        vec![ProviderConfig {
            provider_type: ProviderType::DeepSeekResponses,
            models: vec!["other".into()],
            ..Default::default()
        }],
    );
    let (base, task) = server(crate::ai_gateway::router().with_state(state)).await;
    let error =
        tokio_tungstenite::connect_async(format!("{}/v1/responses", base.replace("http:", "ws:")))
            .await
            .err()
            .unwrap();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP 426")
    };
    assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{base}/v1/responses"))
        .json(&json!({"model":"missing","input":[]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response.text().await.unwrap().contains("invalid_model"));
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn openai_api_key_channel_probes_and_reuses_native_websocket() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = attempts.clone();
    let (requests_tx, mut requests_rx) = mpsc::channel(2);
    let (upstream, upstream_task) = server(Router::new().route("/custom/v1/responses", get(move |headers:HeaderMap, ws:WebSocketUpgrade| {
        let seen = seen.clone(); let requests = requests_tx.clone();
        async move {
            assert_eq!(headers["authorization"], "Bearer upstream-secret");
            assert_eq!(headers.get_all("authorization").iter().count(), 1);
            assert_eq!(headers["session_id"], "api-session");
            assert!(!headers.contains_key("x-openai-actor-authorization"));
            seen.fetch_add(1,Ordering::SeqCst);
            ws.on_upgrade(move |mut socket| async move {
                for index in 0..2 {
                    let Some(Ok(Message::Text(text))) = socket.recv().await else { panic!("missing create") };
                    let request:Value=serde_json::from_str(&text).unwrap();
                    requests.send(request).await.unwrap();
                    let response=json!({"type":"response.completed","response":{"id":format!("resp_{index}"),"status":"completed","output":[],"usage":{"input_tokens":12,"output_tokens":1,"total_tokens":13}}});
                    socket.send(Message::Text(response.to_string().into())).await.unwrap();
                }
            })
        }
    }))).await;
    let temp = tempfile::tempdir().unwrap();
    let state = state(
        &temp,
        vec![ProviderConfig {
            name: "openai-compatible".into(),
            provider_type: ProviderType::OpenAiResponses,
            api_key: "upstream-secret".into(),
            base_url: format!("{upstream}/custom/v1"),
            models: vec!["upstream-model".into()],
            model_aliases: [("client-model".into(), "upstream-model".into())].into(),
            ..Default::default()
        }],
    );
    let (base, task) = server(crate::ai_gateway::router().with_state(state.clone())).await;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut handshake = format!("{}/v1/responses", base.replace("http:", "ws:"))
        .into_client_request()
        .unwrap();
    handshake
        .headers_mut()
        .insert("authorization", "Bearer client-secret".parse().unwrap());
    handshake
        .headers_mut()
        .insert("session_id", "api-session".parse().unwrap());
    handshake.headers_mut().insert(
        "x-openai-actor-authorization",
        "codexhub-local".parse().unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(handshake).await.unwrap();
    for index in 0..2 {
        let mut request = json!({"type":"response.create","model":"client-model","input":[],"future_field":{"unchanged":true}});
        if index == 1 {
            request["previous_response_id"] = json!("resp_0");
        }
        socket
            .send(UpstreamMessage::Text(request.to_string()))
            .await
            .unwrap();
        let response = next_event(&mut socket).await;
        assert_eq!(response["type"], "response.completed");
        let forwarded = tokio::time::timeout(Duration::from_secs(5), requests_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(forwarded["model"], "upstream-model");
        assert_eq!(
            forwarded["previous_response_id"],
            request["previous_response_id"]
        );
        assert_eq!(forwarded["future_field"], request["future_field"]);
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    drop(socket);
    task.abort();
    upstream_task.abort();
    let _ = task.await;
    let _ = upstream_task.await;
}

#[tokio::test]
async fn rejected_websocket_closes_for_client_fallback_without_internal_post() {
    for status in [
        StatusCode::BAD_REQUEST,
        StatusCode::UNAUTHORIZED,
        StatusCode::NOT_FOUND,
        StatusCode::METHOD_NOT_ALLOWED,
        StatusCode::UPGRADE_REQUIRED,
    ] {
        let ws_count = Arc::new(AtomicUsize::new(0));
        let post_count = Arc::new(AtomicUsize::new(0));
        let ws_seen = ws_count.clone();
        let post_seen = post_count.clone();
        let (upstream,upstream_task)=server(Router::new().route("/v1/responses",get(move || {
            let seen=ws_seen.clone(); async move { seen.fetch_add(1,Ordering::SeqCst); (status,"WebSocket is not available") }
        }).post(move || { let seen=post_seen.clone(); async move {
            seen.fetch_add(1,Ordering::SeqCst);
            Json(json!({"id":"resp_http","object":"response","status":"completed","output":[]}))
        }}))).await;
        let temp = tempfile::tempdir().unwrap();
        let state = state(
            &temp,
            vec![ProviderConfig {
                name: "openai".into(),
                base_url: upstream,
                models: vec!["gpt".into()],
                api_key: "key".into(),
                ..Default::default()
            }],
        );
        let (base, task) = server(crate::ai_gateway::router().with_state(state.clone())).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(format!(
            "{}/v1/responses",
            base.replace("http:", "ws:")
        ))
        .await
        .unwrap();
        socket
            .send(UpstreamMessage::Text(
                json!({"type":"response.create","model":"gpt","input":[]}).to_string(),
            ))
            .await
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap(),
            Some(Ok(UpstreamMessage::Close(_)))
        ));
        assert_eq!(ws_count.load(Ordering::SeqCst), 1);
        assert_eq!(post_count.load(Ordering::SeqCst), 0);
        let rows = state.ai_gateway_request_logs.list_recent(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "failed");
        assert!(
            rows[0]
                .error_message
                .as_deref()
                .unwrap()
                .contains("WebSocket is not available")
        );
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("{base}/v1/responses"))
            .json(&json!({"model":"gpt","input":[],"stream":false}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.json::<Value>().await.unwrap()["id"], "resp_http");
        assert_eq!(post_count.load(Ordering::SeqCst), 1);
        drop(socket);
        task.abort();
        upstream_task.abort();
        let _ = task.await;
        let _ = upstream_task.await;
    }
}

#[tokio::test]
async fn mixed_channels_do_not_route_other_models_to_chatgpt() {
    let temp = tempfile::tempdir().unwrap();
    let state = state(
        &temp,
        vec![
            ProviderConfig {
                name: "chatgpt".into(),
                provider_type: ProviderType::ChatGptResponses,
                models: vec!["gpt".into()],
                ..Default::default()
            },
            ProviderConfig {
                name: "other".into(),
                provider_type: ProviderType::AnthropicMessages,
                models: vec!["other".into()],
                ..Default::default()
            },
        ],
    );
    let (base, task) = server(crate::ai_gateway::router().with_state(state)).await;
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("{}/v1/responses", base.replace("http:", "ws:")))
            .await
            .unwrap();
    socket
        .send(UpstreamMessage::Text(
            json!({"type":"response.create","model":"other","input":[]}).to_string(),
        ))
        .await
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap(),
        Some(Ok(UpstreamMessage::Close(_)))
    ));
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn continuation_is_rejected_when_channel_account_changes() {
    let old = ProviderConfig {
        name: "account".into(),
        provider_type: ProviderType::ChatGptResponses,
        models: vec!["gpt".into()],
        chatgpt_auth_id: Some("old-account".into()),
        ..Default::default()
    };
    let current = ProviderConfig {
        chatgpt_auth_id: Some("new-account".into()),
        ..old.clone()
    };
    let temp = tempfile::tempdir().unwrap();
    let state = state(&temp, vec![current]);
    let (base, task) = server(Router::new().route(
        "/responses",
        get(move |ws: WebSocketUpgrade| {
            let state = state.clone();
            let old = old.clone();
            async move {
                ws.on_upgrade(move |socket| async move {
                    let session = Session {
                        selected: Some((
                            old.clone(),
                            crate::ai_gateway::config::provider_route_id(&old),
                        )),
                        last_native_response: Some("resp_old".into()),
                        native: None,
                    };
                    serve(socket, state, HeaderMap::new(), session).await;
                })
            }
        }),
    ))
    .await;
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("{}/responses", base.replace("http:", "ws:")))
            .await
            .unwrap();
    socket.send(UpstreamMessage::Text(json!({"type":"response.create","model":"gpt","previous_response_id":"resp_old","input":[]}).to_string())).await.unwrap();
    assert_eq!(
        next_event(&mut socket).await["error"]["code"],
        "previous_response_not_found"
    );
    drop(socket);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn native_connector_uses_the_configured_http_proxy() {
    let (base, task) = server(Router::new().route(
        "/responses",
        get(|uri: axum::http::Uri, ws: WebSocketUpgrade| async move {
            assert_eq!(uri.host(), Some("upstream.invalid"));
            ws.on_upgrade(|mut socket| async move {
                if let Some(Ok(Message::Text(text))) = socket.recv().await {
                    socket.send(Message::Text(text)).await.unwrap();
                }
            })
        }),
    ))
    .await;
    let client = crate::outbound_http::build_client(
        &crate::config::OutboundProxyConfig {
            mode: crate::config::OutboundProxyMode::Custom,
            url: base,
        },
        None,
    )
    .unwrap();
    let context = GatewayContext::extract(&HeaderMap::new(), None);
    let mut connection = native::connect_with_auth(
        &client,
        &context,
        &ProviderConfig::default(),
        "http://upstream.invalid/responses",
        crate::ai_gateway::chatgpt_auth::manager(),
    )
    .await
    .unwrap();
    connection
        .socket
        .send(UpstreamMessage::Text("proxy-test".into()))
        .await
        .unwrap();
    assert_eq!(
        connection
            .socket
            .next()
            .await
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap(),
        "proxy-test"
    );
    drop(connection);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn native_connector_rejects_invalid_accept_before_sending_payloads() {
    let (base, task) = server(Router::new().route(
        "/responses",
        get(|| async {
            (
                StatusCode::SWITCHING_PROTOCOLS,
                [
                    ("connection", "upgrade"),
                    ("upgrade", "websocket"),
                    ("sec-websocket-accept", "invalid"),
                ],
            )
        }),
    ))
    .await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let context = GatewayContext::extract(&HeaderMap::new(), None);
    let error = native::connect_with_auth(
        &client,
        &context,
        &ProviderConfig::default(),
        &format!("{base}/responses"),
        crate::ai_gateway::chatgpt_auth::manager(),
    )
    .await
    .err()
    .unwrap();
    assert!(
        error
            .message
            .contains("invalid upstream WebSocket handshake")
    );
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn native_relay_preserves_prewarm_continuation_tools_and_logs_then_cancels() {
    let (frames_tx, mut frames_rx) = mpsc::channel::<Value>(4);
    let (upstream_closed_tx, upstream_closed_rx) = oneshot::channel();
    let closed = Arc::new(Mutex::new(Some(upstream_closed_tx)));
    let (upstream_base, upstream_task) = server(Router::new().route("/responses", get(move |ws: WebSocketUpgrade| {
        let frames = frames_tx.clone();
        let closed = closed.clone();
        async move {
            let mut response = ws.on_upgrade(move |mut socket| async move {
                let mut index = 0;
                while let Some(Ok(Message::Text(text))) = socket.recv().await {
                    let request: Value = serde_json::from_str(&text).unwrap();
                    frames.send(request).await.unwrap();
                    if index == 3 { break; }
                    let id = format!("resp_{index}");
                    let events = [
                        json!({"type":"response.created","response":{"id":id,"status":"in_progress"}}),
                        json!({"type":"response.output_text.delta","delta":"ok","future_event_field":true}),
                        json!({"type":"response.completed","response":{"id":id,"status":"completed","output":
                            if index==1 { json!([{"type":"function_call","name":"exec_command","call_id":"call_1","arguments":"{}"}]) } else { json!([]) },
                            "usage":{"input_tokens":100+index,"output_tokens":3,"total_tokens":103+index,"input_tokens_details":{"cached_tokens":50}}}}),
                    ];
                    for event in events { socket.send(Message::Text(event.to_string().into())).await.unwrap(); }
                    index += 1;
                }
                // The fourth generation remains unfinished until the downstream disconnects.
                let _ = socket.recv().await;
                if let Some(done) = closed.lock().await.take() { let _ = done.send(()); }
            }).into_response();
            response.headers_mut().insert("x-codex-turn-state", "handshake-state".parse().unwrap());
            response
        }
    }))).await;
    let context = GatewayContext::extract(&HeaderMap::new(), Some("session"));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let connection = native::connect_with_auth(
        &client,
        &context,
        &ProviderConfig::default(),
        &format!("{upstream_base}/responses"),
        crate::ai_gateway::chatgpt_auth::manager(),
    )
    .await
    .unwrap();
    let provider = ProviderConfig {
        provider_type: ProviderType::ChatGptResponses,
        name: "account".into(),
        models: vec!["model".into()],
        chatgpt_auth_id: Some("test-account".into()),
        ..Default::default()
    };
    let temp = tempfile::tempdir().unwrap();
    let state = state(&temp, vec![provider.clone()]);
    let store = state.ai_gateway_request_logs.clone();
    let session = Session {
        selected: Some((
            provider.clone(),
            crate::ai_gateway::config::provider_route_id(&provider),
        )),
        native: Some(connection),
        last_native_response: None,
    };
    let session = Arc::new(Mutex::new(Some(session)));
    let (done_tx, done_rx) = oneshot::channel();
    let done = Arc::new(Mutex::new(Some(done_tx)));
    let (base, task) = server(Router::new().route(
        "/v1/responses",
        get(move |ws: WebSocketUpgrade| {
            let session = session.clone();
            let state = state.clone();
            let done = done.clone();
            async move {
                ws.on_upgrade(move |socket| async move {
                    let session = session.lock().await.take().unwrap();
                    serve(socket, state, HeaderMap::new(), session).await;
                    if let Some(done) = done.lock().await.take() {
                        let _ = done.send(());
                    }
                })
            }
        }),
    ))
    .await;
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("{}/v1/responses", base.replace("http:", "ws:")))
            .await
            .unwrap();
    for index in 0..4 {
        let mut request = json!({"type":"response.create","model":"model","stream_id":format!("stream_{index}"),
            "tools":[{"type":"image_generation"},{"type":"function","name":"exec_command","parameters":{"type":"object"}}],
            "input":[],"client_metadata":{"x-codex-turn-state":format!("state_{index}")},"future_field":{"retain":true}});
        if index == 0 {
            request["generate"] = json!(false);
        }
        if index > 0 {
            request["previous_response_id"] = json!(format!("resp_{}", index - 1));
        }
        if index == 2 {
            request["input"] =
                json!([{"type":"function_call_output","call_id":"call_1","output":"result"}]);
        }
        socket
            .send(UpstreamMessage::Text(request.to_string()))
            .await
            .unwrap();
        let forwarded = tokio::time::timeout(Duration::from_secs(5), frames_rx.recv())
            .await
            .unwrap()
            .unwrap();
        for field in [
            "input",
            "previous_response_id",
            "client_metadata",
            "generate",
            "future_field",
        ] {
            assert_eq!(forwarded[field], request[field], "{field}");
        }
        assert_eq!(forwarded["tools"].as_array().unwrap().len(), 1);
        if index == 3 {
            break;
        }
        let created = next_event(&mut socket).await;
        assert_eq!(created["stream_id"], request["stream_id"]);
        if index == 0 {
            assert_eq!(created["headers"]["x-codex-turn-state"], "handshake-state");
        } else {
            assert!(created.get("headers").is_none());
        }
        assert_eq!(next_event(&mut socket).await["future_event_field"], true);
        assert_eq!(
            next_event(&mut socket).await["response"]["id"],
            format!("resp_{index}")
        );
    }
    socket.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), done_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), upstream_closed_rx)
        .await
        .unwrap()
        .unwrap();
    let rows = store.list_recent(10).unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows.iter().filter(|row| row.status == "completed").count(),
        3
    );
    assert_eq!(rows.iter().filter(|row| row.status == "failed").count(), 1);
    for row in rows.iter().filter(|row| row.status == "completed") {
        assert!(row.input_tokens.unwrap() >= 100);
        assert_eq!(row.read_cache_tokens, Some(50));
        assert!(row.ttft_ms.is_some());
        let detail = store.get_detail(row.id).unwrap().unwrap();
        assert!(
            detail
                .upstream_response_sse
                .unwrap()
                .contains("response.completed")
        );
        assert!(
            detail
                .upstream_response_headers_json
                .unwrap()
                .contains("handshake-state")
        );
        assert!(
            !detail
                .upstream_request_headers_json
                .unwrap()
                .contains("Bearer")
        );
    }
    task.abort();
    upstream_task.abort();
    let _ = task.await;
    let _ = upstream_task.await;
}

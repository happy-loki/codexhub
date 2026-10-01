use super::*;
use axum::{extract::Form, http::HeaderMap, routing::post};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) fn jwt(account: &str, expires: u64) -> String {
    format!("e30.{}.signature", URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({
        "exp": expires, "email":"test@example.invalid",
        "https://api.openai.com/auth":{"chatgpt_account_id":account,"chatgpt_plan_type":"plus"}
    })).unwrap()))
}

pub(super) fn credential(expires: u64) -> Credential {
    Credential::from_tokens(
        TokenResponse {
            access_token: jwt("account-1", expires),
            refresh_token: Some("refresh-original".into()),
            id_token: None,
            expires_in: None,
        },
        None,
    )
    .unwrap()
}

fn session() -> Arc<LoginSession> {
    Arc::new(LoginSession {
        created: Instant::now(),
        status: Mutex::new(LoginStatus::default()),
        stop: Notify::new(),
        verifier: random_secret(),
        state: random_secret(),
        redirect_uri: "http://localhost:1455/auth/callback".into(),
    })
}

pub(super) async fn server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, task)
}

#[test]
fn authorization_uses_pkce_and_independent_state() {
    let session = session();
    let url = url::Url::parse(&authorize_url(ISSUER, &session)).unwrap();
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(url.host_str(), Some("auth.openai.com"));
    assert_eq!(url.path(), "/oauth/authorize");
    assert_eq!(query.len(), 10);
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["client_id"], CLIENT_ID);
    assert_eq!(query["redirect_uri"], "http://localhost:1455/auth/callback");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(
        query["code_challenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(session.verifier.as_bytes()))
    );
    assert_eq!(query["state"], session.state);
    assert_ne!(session.state, session.verifier);
    assert_eq!(
        query["scope"],
        "openid profile email offline_access api.connectors.read api.connectors.invoke"
    );
    assert_eq!(query["id_token_add_organizations"], "true");
    assert_eq!(query["codex_cli_simplified_flow"], "true");
    assert_eq!(query["originator"], "codex_cli_rs");
    assert!(!url.as_str().contains(&session.verifier));
}

#[test]
fn authorization_uses_the_registered_fallback_callback_port() {
    assert_eq!(LOGIN_CALLBACK_PORTS, [1455, 1457]);
    let mut session = session();
    Arc::get_mut(&mut session).unwrap().redirect_uri = "http://localhost:1457/auth/callback".into();
    let url = url::Url::parse(&authorize_url(ISSUER, &session)).unwrap();
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["redirect_uri"], session.redirect_uri);
    assert_eq!(query["client_id"], CLIENT_ID);
    assert_eq!(query["state"], session.state);
}

#[tokio::test]
async fn wrong_state_does_not_consume_login_and_cancel_stops_callback() {
    let temp = tempfile::tempdir().unwrap();
    let manager = Box::leak(Box::new(AuthManager::new(
        temp.path().into(),
        ISSUER.into(),
    )));
    let session = session();
    manager
        .sessions
        .lock()
        .await
        .insert("test".into(), session.clone());
    let result = login_callback(
        State(CallbackState {
            manager,
            session: session.clone(),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }),
        Query(CallbackQuery {
            state: Some("wrong-state".into()),
            code: Some("secret-code".into()),
            error: None,
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::BAD_REQUEST);
    assert!(!session.status.lock().await.done);
    manager.cancel("test").await;
    assert!(session.status.lock().await.done);
    let result = login_callback(
        State(CallbackState {
            manager,
            session: session.clone(),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }),
        Query(CallbackQuery {
            state: Some(session.state.clone()),
            code: Some("secret-code".into()),
            error: None,
        }),
    )
    .await;
    assert_eq!(result.status(), StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn code_exchange_uses_form_and_only_returns_account_metadata() {
    let login = session();
    let verifier = login.verifier.clone();
    let (base, task) = server(Router::new().route("/oauth/token", post(move |Form(form):Form<HashMap<String,String>>| {
        let verifier = verifier.clone();
        async move {
            assert_eq!(form["grant_type"], "authorization_code");
            assert_eq!(form["code"], "one-time-code");
            assert_eq!(form["code_verifier"], verifier);
            assert_eq!(form["redirect_uri"], "http://localhost:1455/auth/callback");
            Json(json!({"access_token":jwt("account-1",now()+3600),"refresh_token":"refresh-issued"}))
        }
    }))).await;
    let temp = tempfile::tempdir().unwrap();
    let auth = AuthManager::new(temp.path().into(), base);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let credential = auth
        .exchange(&client, &login, "one-time-code")
        .await
        .unwrap();
    let summary = auth.save_login(&credential).unwrap();
    assert_eq!(summary.email.as_deref(), Some("test@example.invalid"));
    assert_eq!(summary.plan.as_deref(), Some("plus"));
    let public_json = serde_json::to_string(&summary).unwrap();
    assert!(!public_json.contains("token"));
    assert!(!public_json.contains("refresh-issued"));
    assert_eq!(
        auth.read(&summary.auth_id).unwrap().refresh_token,
        "refresh-issued"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(auth.credential_path(&summary.auth_id).unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn login_callback_prefers_the_first_available_registered_port() {
    // Isolated test ports avoid interfering with real browser login sessions.
    let preferred = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = preferred.local_addr().unwrap().port();
    drop(preferred);
    let listener = bind_login_callback(&[port]).await.unwrap();
    assert_eq!(listener.local_addr().unwrap().port(), port);
    assert!(listener.local_addr().unwrap().ip().is_loopback());
}

#[tokio::test]
async fn login_callback_uses_fallback_without_stopping_the_existing_listener() {
    let preferred = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fallback = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ports = [
        preferred.local_addr().unwrap().port(),
        fallback.local_addr().unwrap().port(),
    ];
    drop(fallback);
    let listener = bind_login_callback(&ports).await.unwrap();
    assert_eq!(listener.local_addr().unwrap().port(), ports[1]);
    let client = tokio::net::TcpStream::connect(preferred.local_addr().unwrap())
        .await
        .unwrap();
    let (_, peer) = tokio::time::timeout(Duration::from_secs(2), preferred.accept())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(peer, client.local_addr().unwrap());
}

#[tokio::test]
async fn login_callback_refuses_random_port_when_both_registered_ports_are_busy() {
    let preferred = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fallback = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ports = [
        preferred.local_addr().unwrap().port(),
        fallback.local_addr().unwrap().port(),
    ];
    let error = bind_login_callback(&ports).await.unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "login_callback_port_unavailable");
    assert!(error.message.contains("import auth.json"));
}

#[tokio::test]
async fn concurrent_refresh_rotates_once_and_persists_before_reuse() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let (base, task) = server(Router::new().route("/oauth/token", post(move |Json(body):Json<Value>| {
        let count = seen.clone();
        async move {
            assert_eq!(body["refresh_token"], "refresh-original");
            assert_eq!(body["grant_type"], "refresh_token");
            count.fetch_add(1, Ordering::SeqCst);
            Json(json!({"access_token":jwt("account-1",now()+7200),"refresh_token":"refresh-rotated"}))
        }
    }))).await;
    let temp = tempfile::tempdir().unwrap();
    let auth = AuthManager::new(temp.path().into(), base);
    let id = uuid::Uuid::new_v4().to_string();
    let old = credential(now() - 10);
    auth.write(&id, &old).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let requests = (0..12).map(|_| auth.credential(&client, &id, Some(&old.access_token)));
    for value in futures_util::future::join_all(requests).await {
        assert_eq!(value.unwrap().refresh_token, "refresh-rotated");
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(auth.read(&id).unwrap().refresh_token, "refresh-rotated");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn revoked_refresh_is_not_repeated_and_does_not_expose_error_body() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let (base, task) = server(Router::new().route(
        "/oauth/token",
        post(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"invalid_grant","detail":"refresh-original"})),
                )
            }
        }),
    ))
    .await;
    let temp = tempfile::tempdir().unwrap();
    let auth = AuthManager::new(temp.path().into(), base);
    let id = uuid::Uuid::new_v4().to_string();
    auth.write(&id, &credential(now() - 10)).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for _ in 0..3 {
        let error = auth.credential(&client, &id, None).await.err().unwrap();
        assert!(error.message.contains("sign in again"));
        assert!(!error.message.contains("refresh-original"));
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(auth.summary(&id).await.unwrap().needs_login);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn unauthorized_retries_once_and_preserves_lite_body_and_account() {
    for reject_again in [false, true] {
        let refreshes = Arc::new(AtomicUsize::new(0));
        let refresh_seen = refreshes.clone();
        let requests = Arc::new(AtomicUsize::new(0));
        let request_seen = requests.clone();
        let body = json!({"model":"gpt-test","stream":true,"store":false,
            "input":[{"type":"additional_tools","role":"developer","tools":[{"type":"namespace","name":"fs","tools":[]}]}],
            "tools":[{"type":"custom","name":"apply_patch","format":{"type":"grammar"}}],"future_field":{"preserve":true}});
        let expected = body.clone();
        let new_token = jwt("account-1", now() + 7200);
        let refreshed = new_token.clone();
        let (base, task) = server(Router::new()
            .route("/oauth/token", post(move |Json(body):Json<Value>| { let seen=refresh_seen.clone(); let token=refreshed.clone(); async move {
                assert_eq!(body["refresh_token"],"refresh-original"); seen.fetch_add(1,Ordering::SeqCst);
                Json(json!({"access_token":token}))
            }}))
            .route("/backend-api/codex/responses",post(move |headers:HeaderMap,Json(body):Json<Value>| {
                let count=request_seen.clone(); let expected=expected.clone(); let token=new_token.clone(); async move {
                    assert_eq!(body,expected); assert_eq!(headers["chatgpt-account-id"],"account-1");
                    assert_eq!(headers.get_all("chatgpt-account-id").iter().count(),1);
                    assert!(!headers.contains_key("openai-organization"));
                    let attempt=count.fetch_add(1,Ordering::SeqCst);
                    if attempt>0 { assert_eq!(headers["authorization"],format!("Bearer {token}")); }
                    if attempt==0 || reject_again { (StatusCode::UNAUTHORIZED,"unauthorized").into_response() }
                    else { ([("content-type","text/event-stream")],"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n").into_response() }
                }
            }))).await;
        let temp = tempfile::tempdir().unwrap();
        let mut auth = AuthManager::new(temp.path().into(), base.clone());
        auth.backend = format!("{base}/backend-api/codex");
        let id = uuid::Uuid::new_v4().to_string();
        auth.write(&id, &credential(now() + 3600)).unwrap();
        let provider = ProviderConfig {
            provider_type: ProviderType::ChatGptResponses,
            chatgpt_auth_id: Some(id.clone()),
            ..Default::default()
        };
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut request = client
            .post(format!("{}/responses", auth.backend))
            .header("chatgpt-account-id", "wrong-client-account")
            .header("openai-organization", "unrelated")
            .json(&body)
            .build()
            .unwrap();
        authorize_with_manager(&auth, &client, &mut request, &provider, None)
            .await
            .unwrap();
        let log = super::super::request_log::headers_to_redacted_json(request.headers()).unwrap();
        assert!(!log.contains(&auth.read(&id).unwrap().access_token));
        let response = super::super::providers::execute_openai_request_with_auth(
            &client,
            request,
            &provider,
            "test request",
            &auth,
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            if reject_again {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            }
        );
        if !reject_again {
            assert!(
                response
                    .text()
                    .await
                    .unwrap()
                    .contains("response.completed")
            );
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(auth.read(&id).unwrap().refresh_token, "refresh-original");
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn credentials_cannot_be_sent_to_configured_or_untrusted_hosts() {
    let temp = tempfile::tempdir().unwrap();
    let auth = AuthManager::new(temp.path().into(), ISSUER.into());
    let provider = ProviderConfig {
        provider_type: ProviderType::ChatGptResponses,
        base_url: "https://proxy.invalid/v1".into(),
        ..Default::default()
    };
    for path in [
        "/v1/responses",
        "/v1/responses/compact",
        "/v1/alpha/search",
        "/v1/images/generations",
    ] {
        assert_eq!(
            endpoint(&provider, path),
            format!("{BASE_URL}{}", path.strip_prefix("/v1").unwrap())
        );
    }
    let client = reqwest::Client::new();
    for url in [
        "https://proxy.invalid/responses",
        "https://chatgpt.com.evil.invalid/backend-api/codex/responses",
        "http://chatgpt.com/backend-api/codex/responses",
        "https://chatgpt.com/backend-api/other",
    ] {
        let mut request = client.post(url).build().unwrap();
        let error = authorize_with_manager(&auth, &client, &mut request, &provider, None)
            .await
            .unwrap_err();
        assert!(error.message.contains("official Codex endpoint"));
        assert!(!request.headers().contains_key("authorization"));
    }
    assert!(auth.credential_path("../../auth").is_err());
    let api = ProviderConfig {
        base_url: "https://proxy.invalid/v1".into(),
        ..Default::default()
    };
    assert_eq!(
        endpoint(&api, "/v1/responses"),
        "https://proxy.invalid/v1/responses"
    );
}

#[tokio::test]
async fn websocket_handshake_refreshes_once_and_reuses_authenticated_connection() {
    use axum::{
        extract::ws::{Message, WebSocketUpgrade},
        routing::get,
    };
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let attempts = Arc::new(AtomicUsize::new(0));
    let refreshes = Arc::new(AtomicUsize::new(0));
    let refreshed_token = jwt("account-1", now() + 7200);
    let seen = attempts.clone();
    let refresh_seen = refreshes.clone();
    let new_token = refreshed_token.clone();
    let (base, task) = server(
        Router::new()
            .route(
                "/oauth/token",
                post(move || {
                    let count = refresh_seen.clone();
                    let token = new_token.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"access_token":token}))
                    }
                }),
            )
            .route(
                "/backend-api/codex/responses",
                get(move |headers: HeaderMap, ws: WebSocketUpgrade| {
                    let count = seen.clone();
                    let token = refreshed_token.clone();
                    async move {
                        assert_eq!(headers["chatgpt-account-id"], "account-1");
                        assert_eq!(headers["session_id"], "stable-session");
                        assert!(!headers.contains_key("openai-organization"));
                        assert!(!headers.contains_key("sec-websocket-extensions"));
                        assert_ne!(headers["sec-websocket-key"], "client-key");
                        assert!(
                            headers["openai-beta"]
                                .to_str()
                                .unwrap()
                                .contains("responses_websockets=")
                        );
                        if count.fetch_add(1, Ordering::SeqCst) == 0 {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        assert_eq!(headers["authorization"], format!("Bearer {token}"));
                        let mut response = ws
                            .on_upgrade(|mut socket| async move {
                                for _ in 0..2 {
                                    let Some(Ok(Message::Text(text))) = socket.recv().await else {
                                        panic!("missing create");
                                    };
                                    socket.send(Message::Text(text)).await.unwrap();
                                }
                            })
                            .into_response();
                        response
                            .headers_mut()
                            .insert("x-codex-turn-state", "mock-state".parse().unwrap());
                        response
                    }
                }),
            ),
    )
    .await;
    let temp = tempfile::tempdir().unwrap();
    let mut auth = AuthManager::new(temp.path().into(), base.clone());
    auth.backend = format!("{base}/backend-api/codex");
    let id = uuid::Uuid::new_v4().to_string();
    auth.write(&id, &credential(now() + 3600)).unwrap();
    let provider = ProviderConfig {
        provider_type: ProviderType::ChatGptResponses,
        chatgpt_auth_id: Some(id),
        ..Default::default()
    };
    let mut headers = HeaderMap::new();
    for (key, value) in [
        ("authorization", "Bearer wrong"),
        ("chatgpt-account-id", "wrong"),
        ("openai-organization", "wrong"),
        ("session_id", "stable-session"),
        ("sec-websocket-key", "client-key"),
        ("sec-websocket-extensions", "permessage-deflate"),
    ] {
        headers.insert(
            axum::http::HeaderName::from_static(key),
            value.parse().unwrap(),
        );
    }
    let context = super::super::context::GatewayContext::extract(&headers, None);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut connection = super::super::websocket::native::connect_with_auth(
        &client,
        &context,
        &provider,
        &format!("{}/responses", auth.backend),
        &auth,
    )
    .await
    .unwrap();
    assert_eq!(
        connection.response_headers["x-codex-turn-state"],
        "mock-state"
    );
    for index in 0..2 {
        let request =
            json!({"type":"response.create","model":"test","input":[],"future_field":index})
                .to_string();
        connection
            .socket
            .send(WsMessage::Text(request.clone()))
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
            request
        );
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    drop(connection);
    task.abort();
    let _ = task.await;
}

#[test]
fn refresh_cannot_switch_account_and_config_contains_no_tokens() {
    let old = credential(now() + 3600);
    let result = Credential::from_tokens(
        TokenResponse {
            access_token: jwt("different-account", now() + 7200),
            refresh_token: None,
            id_token: None,
            expires_in: None,
        },
        Some(&old),
    );
    assert!(result.is_err());
    let provider = ProviderConfig {
        provider_type: ProviderType::ChatGptResponses,
        chatgpt_auth_id: Some(uuid::Uuid::new_v4().to_string()),
        ..Default::default()
    };
    let json = serde_json::to_value(&provider).unwrap();
    assert_eq!(json["providerType"], "chatgpt_responses");
    assert!(json.get("access_token").is_none());
    assert!(json.get("refresh_token").is_none());
    assert_eq!(
        serde_json::from_value::<ProviderConfig>(json)
            .unwrap()
            .provider_type,
        ProviderType::ChatGptResponses
    );
}

#[tokio::test]
async fn cancelling_during_token_exchange_does_not_block_status_or_save_credentials() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let seen = entered.clone();
    let finish = release.clone();
    let (base,task)=server(Router::new().route("/oauth/token",post(move || {
        let seen=seen.clone();let finish=finish.clone();async move {
            seen.notify_one();finish.notified().await;
            Json(json!({"access_token":jwt("account-1",now()+3600),"refresh_token":"issued-after-cancel"}))
        }
    }))).await;
    let temp = tempfile::tempdir().unwrap();
    let manager: &'static AuthManager =
        Box::leak(Box::new(AuthManager::new(temp.path().into(), base)));
    let session = session();
    manager
        .sessions
        .lock()
        .await
        .insert("pending".into(), session.clone());
    let callback = CallbackState {
        manager,
        session: session.clone(),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
    };
    let query = CallbackQuery {
        state: Some(session.state.clone()),
        code: Some("test-code".into()),
        error: None,
    };
    let request = tokio::spawn(async move { login_callback(State(callback), Query(query)).await });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    let status = tokio::time::timeout(Duration::from_millis(100), manager.login_status("pending"))
        .await
        .unwrap()
        .unwrap();
    assert!(!status.done);
    manager.cancel("pending").await;
    release.notify_one();
    assert_eq!(request.await.unwrap().status(), StatusCode::BAD_REQUEST);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn transient_refresh_failure_can_retry_without_revoking_account() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let (base, task) = server(Router::new().route(
        "/oauth/token",
        post(move || {
            let seen = seen.clone();
            async move {
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    (StatusCode::SERVICE_UNAVAILABLE, "temporary outage").into_response()
                } else {
                    Json(json!({"access_token":jwt("account-1",now()+3600)})).into_response()
                }
            }
        }),
    ))
    .await;
    let temp = tempfile::tempdir().unwrap();
    let auth = AuthManager::new(temp.path().into(), base);
    let id = uuid::Uuid::new_v4().to_string();
    auth.write(&id, &credential(now() - 10)).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert!(auth.credential(&client, &id, None).await.is_err());
    assert!(!auth.summary(&id).await.unwrap().needs_login);
    assert!(auth.credential(&client, &id, None).await.is_ok());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    task.abort();
    let _ = task.await;
}

#[test]
fn chatgpt_native_tools_and_compact_routes_share_the_configured_account() {
    use super::super::{
        config::AiGatewayConfig, router::resolve_provider_with_state_for_type,
        routing_state::GatewayRoutingState,
    };
    let provider = ProviderConfig {
        name: "subscription".into(),
        provider_type: ProviderType::ChatGptResponses,
        models: vec!["gpt-test".into()],
        chatgpt_auth_id: Some(uuid::Uuid::new_v4().to_string()),
        ..Default::default()
    };
    let config = AiGatewayConfig {
        providers: vec![provider.clone()],
        ..Default::default()
    };
    let (selected, _) = resolve_provider_with_state_for_type(
        "gpt-test",
        Some("session"),
        &config,
        &mut GatewayRoutingState::default(),
        Instant::now(),
        &ProviderType::OpenAiResponses,
    )
    .unwrap();
    assert_eq!(selected.chatgpt_auth_id, provider.chatgpt_auth_id);
    let original = json!({"input":[{"type":"additional_tools","tools":[{"type":"custom","name":"apply_patch","format":{"type":"grammar"}}]},
        {"type":"reasoning","id":"rs_native","summary":[],"encrypted_content":"official-native-ciphertext"}],"future":{"keep":true}});
    let mut body = original.clone();
    super::super::responses_lite_tools::prepare_for_provider(&mut body, &provider.provider_type)
        .unwrap();
    let scope = super::super::encrypted_content::EncryptedContentScope::for_provider(&provider);
    super::super::encrypted_content::prepare_responses_request(&mut body, &scope);
    assert_eq!(body, original);
}

#[tokio::test]
async fn model_listing_uses_codex_version_and_filters_hidden_entries() {
    let expected_version = super::super::catalog::codex_compatibility_version();
    assert_ne!(expected_version, env!("CARGO_PKG_VERSION"));
    let (base,task)=server(Router::new().route("/backend-api/codex/models",get(move |headers:HeaderMap,Query(query):Query<HashMap<String,String>>| {
        let expected=expected_version.clone();async move {
            assert_eq!(headers["chatgpt-account-id"],"account-1");
            assert_eq!(query["client_version"],expected);
            Json(json!({"models":[{"slug":"gpt-test","visibility":"list"},{"slug":"hidden","visibility":"hide"},{"slug":"gpt-test"}]}))
        }
    }))).await;
    let temp = tempfile::tempdir().unwrap();
    let mut auth = AuthManager::new(temp.path().into(), ISSUER.into());
    auth.backend = format!("{base}/backend-api/codex");
    let id = uuid::Uuid::new_v4().to_string();
    auth.write(&id, &credential(now() + 3600)).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        models_with_manager(&auth, &client, &id).await.unwrap(),
        vec!["gpt-test"]
    );
    task.abort();
    let _ = task.await;
}

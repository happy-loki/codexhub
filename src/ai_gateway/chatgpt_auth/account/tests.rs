use super::*;
use crate::ai_gateway::chatgpt_auth::tests::{credential, jwt, server};
use axum::{http::HeaderMap, routing::post};
use std::sync::atomic::{AtomicUsize, Ordering};

fn auth_file(expiry: u64) -> Value {
    json!({
        "auth_mode":"chatgpt", "OPENAI_API_KEY":null,
        "tokens":{"access_token":jwt("account-1", expiry), "id_token":jwt("account-1", expiry),
            "refresh_token":"imported-refresh", "account_id":"account-1"},
        "last_refresh":"2026-09-20T00:00:00Z"
    })
}

#[test]
fn imports_official_and_legacy_auth_files_without_returning_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let auth = AuthManager::new(temp.path().into(), ISSUER.into());
    for legacy in [false, true] {
        let mut file = auth_file(now() + 3600);
        if legacy {
            file.as_object_mut().unwrap().remove("auth_mode");
        }
        let summary = auth.import(file).unwrap();
        assert_eq!(summary.email.as_deref(), Some("test@example.invalid"));
        assert!(summary.can_refresh);
        assert!(!summary.needs_login);
        let persisted = auth.read(&summary.auth_id).unwrap();
        assert_eq!(persisted.account_id, "account-1");
        assert_eq!(persisted.refresh_token, "imported-refresh");
        let output = serde_json::to_string(&summary).unwrap();
        assert!(!output.contains("imported-refresh"));
        assert!(!output.contains("signature"));
        assert!(!output.contains("expiresAt"));
    }
}

#[test]
fn rejects_api_keys_mismatched_accounts_and_malformed_imports_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let auth = AuthManager::new(temp.path().into(), ISSUER.into());
    let mut mismatch = auth_file(now() + 3600);
    mismatch["tokens"]["account_id"] = json!("another-account");
    let mut access_mismatch = auth_file(now() + 3600);
    access_mismatch["tokens"]["access_token"] = json!(jwt("another-account", now() + 3600));
    let mut api_mode = auth_file(now() + 3600);
    api_mode["auth_mode"] = json!("apikey");
    let mut malformed = auth_file(now() + 3600);
    malformed["tokens"]["access_token"] = json!("secret-invalid-bearer");
    for input in [
        json!({}),
        json!({"OPENAI_API_KEY":"sk-secret"}),
        mismatch,
        access_mismatch,
        api_mode,
        malformed,
    ] {
        let error = auth.import(input).unwrap_err();
        assert!(!error.message.contains("secret"));
        assert!(!error.message.contains("signature"));
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn access_only_import_expires_without_trying_a_refresh_request() {
    let temp = tempfile::tempdir().unwrap();
    let auth = AuthManager::new(temp.path().into(), "http://127.0.0.1:1".into());
    let mut file = auth_file(now() + 30);
    file["auth_mode"] = json!("chatgptAuthTokens");
    file["tokens"]
        .as_object_mut()
        .unwrap()
        .remove("refresh_token");
    let summary = auth.import(file.clone()).unwrap();
    assert!(!summary.can_refresh);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let valid = auth
        .credential(&client, &summary.auth_id, None)
        .await
        .unwrap();
    assert_eq!(valid.account_id, "account-1");
    let error = auth
        .credential(&client, &summary.auth_id, Some(&valid.access_token))
        .await
        .err()
        .unwrap();
    assert!(error.message.contains("sign in again"));
    assert!(auth.read(&summary.auth_id).unwrap().needs_login);
    file["tokens"]["access_token"] = json!(jwt("account-1", now() - 1));
    assert!(auth.import(file).is_err());
}

#[tokio::test]
async fn expired_import_can_refresh_and_keeps_the_selected_identity() {
    let (base, task) = server(Router::new().route("/oauth/token", post(|Json(body): Json<Value>| async move {
        assert_eq!(body["refresh_token"], "imported-refresh");
        Json(json!({"access_token":jwt("account-1",now()+7200),"refresh_token":"rotated-import"}))
    }))).await;
    let temp = tempfile::tempdir().unwrap();
    let auth = AuthManager::new(temp.path().into(), base);
    let summary = auth.import(auth_file(now() - 1)).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let refreshed = auth
        .credential(&client, &summary.auth_id, None)
        .await
        .unwrap();
    assert_eq!(refreshed.refresh_token, "rotated-import");
    assert_eq!(auth.read(&summary.auth_id).unwrap().account_id, "account-1");
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn usage_reads_official_sibling_path_refreshes_once_and_preserves_quota_fields() {
    let count = Arc::new(AtomicUsize::new(0));
    let hits = count.clone();
    let refresh_count = Arc::new(AtomicUsize::new(0));
    let refresh_hits = refresh_count.clone();
    let (base, task) = server(Router::new()
        .route("/backend-api/wham/usage", get(move |headers: HeaderMap| {
            let hits = hits.clone();
            async move {
                assert_eq!(headers["chatgpt-account-id"], "account-1");
                assert_eq!(headers["originator"], "codex_cli_rs");
                if hits.fetch_add(1, Ordering::SeqCst) == 0 {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                assert_eq!(headers["authorization"].to_str().unwrap(), format!("Bearer {}", jwt("account-1", 4_000_000_000)));
                Json(json!({"plan_type":"pro", "rate_limit":{
                    "allowed":true,"limit_reached":false,
                    "primary_window":{"used_percent":35.5,"limit_window_seconds":18000,"reset_after_seconds":123,"reset_at":2_000_000_123},
                    "secondary_window":{"used_percent":76,"limit_window_seconds":604800,"reset_at":2_000_123_456}
                },"credits":{"has_credits":true,"unlimited":false,"balance":"10.5"},
                "additional_rate_limits":[{"limit_name":"GPT additional","metered_feature":"model-x","rate_limit":null}],
                "unknown_future_field":"ignored"
                })).into_response()
            }
        }))
        .route("/oauth/token", post(move || {
            refresh_hits.fetch_add(1, Ordering::SeqCst);
            async { Json(json!({"access_token":jwt("account-1",4_000_000_000),"refresh_token":"rotated"})) }
        }))
    ).await;
    let temp = tempfile::tempdir().unwrap();
    let mut auth = AuthManager::new(temp.path().into(), base.clone());
    auth.backend = format!("{base}/backend-api/codex");
    let summary = auth.save_login(&credential(now() + 3600)).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let result = usage_with_manager(&auth, &client, &summary.auth_id)
        .await
        .unwrap();
    assert_eq!(result.account.plan.as_deref(), Some("pro"));
    let limits = result.usage.rate_limit.as_ref().unwrap();
    assert_eq!(limits.primary_window.as_ref().unwrap().used_percent, 35.5);
    assert_eq!(
        limits
            .secondary_window
            .as_ref()
            .unwrap()
            .limit_window_seconds,
        604800
    );
    assert_eq!(
        limits.primary_window.as_ref().unwrap().reset_at,
        Some(2_000_000_123)
    );
    assert_eq!(
        result.usage.credits.as_ref().unwrap().balance.as_deref(),
        Some("10.5")
    );
    assert_eq!(
        result.usage.additional_rate_limits.as_ref().unwrap().len(),
        1
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(refresh_count.load(Ordering::SeqCst), 1);
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(!serialized.contains("rotated"));
    assert!(!serialized.contains("signature"));
    task.abort();
    let _ = task.await;
}

#[test]
fn missing_usage_stays_unknown_and_unrecognized_responses_fail() {
    let usage: UsagePayload =
        serde_json::from_value(json!({"plan_type":"enterprise","rate_limit":null,"credits":null}))
            .unwrap();
    assert!(usage.rate_limit.is_none());
    assert!(usage.credits.is_none());
    assert!(serde_json::from_value::<UsagePayload>(json!({"error":"not logged in"})).is_err());
    assert!(
        serde_json::from_value::<UsagePayload>(
            json!({"plan_type":"plus","rate_limit":{"primary_window":{}}})
        )
        .is_err()
    );
}

#[tokio::test]
async fn repeated_usage_401_stops_after_one_refresh_and_hides_upstream_body() {
    let count = Arc::new(AtomicUsize::new(0));
    let hits = count.clone();
    let (base, task) = server(Router::new()
        .route("/wham/usage", get(move || {
            hits.fetch_add(1, Ordering::SeqCst);
            async { (StatusCode::UNAUTHORIZED, "secret upstream response") }
        }))
        .route("/oauth/token", post(|| async { Json(json!({"access_token":jwt("account-1",4_000_000_000),"refresh_token":"rotated"})) }))
    ).await;
    let temp = tempfile::tempdir().unwrap();
    let mut auth = AuthManager::new(temp.path().into(), base.clone());
    auth.backend = format!("{base}/codex");
    let summary = auth.save_login(&credential(now() + 3600)).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let error = usage_with_manager(&auth, &client, &summary.auth_id)
        .await
        .unwrap_err();
    assert_eq!(error.status, StatusCode::UNAUTHORIZED);
    assert!(!error.message.contains("secret"));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    task.abort();
    let _ = task.await;
}

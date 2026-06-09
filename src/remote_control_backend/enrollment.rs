use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    app_state::{
        AuthorizedRemoteControlClient, RemoteControlSourceHint, RemoteControlSourceKind,
        SharedState,
    },
    chain_log,
    types::now_ms,
};

use super::auth_tokens::jwt_payload;
use super::{
    format_rfc3339_utc, header_str, log_remote_control_entry_headers, source_kind_from_user_agent,
    stable_base64url_32, stable_id, unix_now_u64,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct EnrollRequest {
    name: Option<String>,
    os: Option<String>,
    arch: Option<String>,
    app_server_version: Option<String>,
    installation_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct EnrollResponse {
    server_id: String,
    environment_id: String,
    remote_control_token: String,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct RefreshRequest {
    server_id: Option<String>,
    installation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RemoteControlClientFinishRequest {
    client_id: String,
    #[allow(dead_code)]
    step_up_token: Option<String>,
    device_identity: Option<RemoteControlDeviceIdentity>,
    #[allow(dead_code)]
    device_key_proof: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct RemoteControlDeviceIdentity {
    algorithm: String,
    key_id: String,
    protection_class: String,
    public_key_spki_der_base64: String,
}

pub(super) async fn remote_control_client_enroll_start(headers: HeaderMap) -> impl IntoResponse {
    log_remote_control_entry_headers("client_enroll_start", &headers);
    Json(remote_control_client_start_response(
        &headers,
        None,
        "enroll/finish",
        None,
    ))
}

pub(super) async fn remote_control_client_refresh_start(
    State(state): State<SharedState>,
    headers: HeaderMap,
    payload: Option<Json<RemoteControlClientFinishRequest>>,
) -> impl IntoResponse {
    log_remote_control_entry_headers("client_refresh_start", &headers);
    let client_id = payload.map(|Json(value)| value.client_id);
    let device_identity_hash = if let Some(client_id) = client_id.as_deref() {
        state
            .remote_control
            .inner
            .lock()
            .await
            .authorized_clients
            .get(client_id)
            .and_then(|client| {
                client
                    .device_identity
                    .as_ref()
                    .and_then(remote_control_device_identity_hash)
            })
    } else {
        None
    };
    Json(remote_control_client_start_response(
        &headers,
        client_id,
        "refresh/finish",
        device_identity_hash,
    ))
}

pub(super) async fn remote_control_client_enroll_finish(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<RemoteControlClientFinishRequest>,
) -> impl IntoResponse {
    log_remote_control_entry_headers("client_enroll_finish", &headers);
    chain_log::write_line(format!(
        "[remote_control] event=client_enroll_finish_identity client_id={} device_identity={}",
        request.client_id,
        remote_control_finish_identity_summary(&request)
    ));
    let token = remote_control_client_token_response(&headers, &request.client_id);
    remember_remote_control_client(&state, &headers, &request).await;
    Json(token)
}

pub(super) async fn remote_control_client_refresh_finish(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<RemoteControlClientFinishRequest>,
) -> impl IntoResponse {
    log_remote_control_entry_headers("client_refresh_finish", &headers);
    chain_log::write_line(format!(
        "[remote_control] event=client_refresh_finish_identity client_id={} device_identity={}",
        request.client_id,
        remote_control_finish_identity_summary(&request)
    ));
    let token = remote_control_client_token_response(&headers, &request.client_id);
    remember_remote_control_client(&state, &headers, &request).await;
    Json(token)
}

fn remote_control_client_start_response(
    headers: &HeaderMap,
    client_id: Option<String>,
    finish_path: &str,
    device_identity_hash: Option<String>,
) -> Value {
    let account_user_id = remote_control_account_user_id(headers);
    let client_id = client_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| stable_id("client", &account_user_id));
    let challenge = remote_control_device_key_challenge(
        headers,
        &account_user_id,
        &client_id,
        finish_path,
        device_identity_hash,
    );
    json!({
        "client_id": client_id,
        "account_user_id": account_user_id,
        "device_key_challenge": challenge,
    })
}

fn remote_control_client_token_response(headers: &HeaderMap, client_id: &str) -> Value {
    let account_user_id = remote_control_account_user_id(headers);
    let expires_at = iso8601_after(Duration::from_secs(24 * 60 * 60));
    json!({
        "client_id": client_id,
        "account_user_id": account_user_id,
        "remote_control_token": local_remote_control_client_token(headers, client_id),
        "expires_at": expires_at,
        "scopes": ["remote_control_controller_websocket"],
    })
}

fn remote_control_device_key_challenge(
    headers: &HeaderMap,
    account_user_id: &str,
    client_id: &str,
    finish_path: &str,
    device_identity_hash: Option<String>,
) -> Value {
    let challenge_id = stable_id(
        "challenge",
        &format!("{account_user_id}:{client_id}:{finish_path}"),
    );
    let target_origin = request_origin(headers);
    json!({
        "challenge_id": challenge_id,
        "challenge_token": local_remote_control_client_token(headers, client_id),
        "nonce": stable_base64url_32("nonce", &challenge_id),
        "purpose": "remote_control_client_enrollment",
        "audience": "remote_control_client_enrollment",
        "account_user_id": account_user_id,
        "client_id": client_id,
        "target_origin": target_origin,
        "target_path": format!("/backend-api/codex/remote/control/client/{finish_path}"),
        "challenge_expires_at": iso8601_after(Duration::from_secs(5 * 60)),
        "device_identity_hash": device_identity_hash,
    })
}

async fn remember_remote_control_client(
    state: &SharedState,
    headers: &HeaderMap,
    request: &RemoteControlClientFinishRequest,
) {
    let account_user_id = remote_control_account_user_id(headers);
    let display_name = "Codex App Remote Control".to_string();
    let device_identity = request
        .device_identity
        .as_ref()
        .map(remote_control_device_identity_json);
    let mut remote = state.remote_control.inner.lock().await;
    remote.authorized_clients.insert(
        request.client_id.clone(),
        AuthorizedRemoteControlClient {
            client_id: request.client_id.clone(),
            account_user_id,
            device_identity,
            display_name,
            last_seen_at_ms: unix_now_u64().saturating_mul(1000),
        },
    );
}

fn local_remote_control_client_token(headers: &HeaderMap, client_id: &str) -> String {
    let now = unix_now_u64();
    let account_id = header_str(headers, "chatgpt-account-id")
        .unwrap_or_else(|| "acct_codex_remote_local".into());
    let account_user_id = remote_control_account_user_id(headers);
    let payload = json!({
        "iss": "codex-remote-local",
        "aud": "remote_control_controller_websocket",
        "iat": now,
        "nbf": now,
        "exp": now + 24 * 60 * 60,
        "sub": client_id,
        "scope": "remote_control_controller_websocket",
        "scp": ["remote_control_controller_websocket"],
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "account_id": account_id,
            "chatgpt_account_user_id": account_user_id,
            "account_user_id": account_user_id,
            "user_id": jwt_bearer_claim(headers, "user_id")
                .or_else(|| jwt_bearer_claim(headers, "chatgpt_user_id"))
                .unwrap_or_else(|| "user_codex_remote_local".into()),
        },
    });
    format!(
        "{}.{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({ "alg": "none", "typ": "JWT" })).unwrap_or_default()
        ),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap_or_default()),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
    )
}

fn local_remote_control_server_token(headers: &HeaderMap, server_id: &str) -> String {
    let now = unix_now_u64();
    let account_id = header_str(headers, "chatgpt-account-id")
        .unwrap_or_else(|| "acct_codex_remote_local".into());
    let account_user_id = remote_control_account_user_id(headers);
    let payload = json!({
        "iss": "codex-remote-local",
        "aud": "remote_control_server_websocket",
        "iat": now,
        "nbf": now,
        "exp": now + 24 * 60 * 60,
        "sub": server_id,
        "scope": "remote_control_server_websocket",
        "scp": ["remote_control_server_websocket"],
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "account_id": account_id,
            "chatgpt_account_user_id": account_user_id,
            "account_user_id": account_user_id,
            "user_id": jwt_bearer_claim(headers, "user_id")
                .or_else(|| jwt_bearer_claim(headers, "chatgpt_user_id"))
                .unwrap_or_else(|| "user_codex_remote_local".into()),
        },
    });
    format!(
        "{}.{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({ "alg": "none", "typ": "JWT" })).unwrap_or_default()
        ),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap_or_default()),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
    )
}

fn remote_control_account_user_id(headers: &HeaderMap) -> String {
    jwt_bearer_claim(headers, "chatgpt_account_user_id")
        .or_else(|| jwt_bearer_claim(headers, "account_user_id"))
        .or_else(|| jwt_bearer_claim(headers, "user_id"))
        .unwrap_or_else(|| "user_codex_remote_local__acct_codex_remote_local".into())
}

fn jwt_bearer_claim(headers: &HeaderMap, claim: &str) -> Option<String> {
    let token = authorization_bearer(headers)?;
    jwt_payload(&token).and_then(|payload| {
        payload
            .get("https://api.openai.com/auth")?
            .get(claim)?
            .as_str()
            .map(str::to_string)
    })
}

fn authorization_bearer(headers: &HeaderMap) -> Option<String> {
    let value = header_str(headers, "authorization")?;
    let mut parts = value.splitn(2, char::is_whitespace);
    let scheme = parts.next()?;
    let token = parts.next()?.trim();
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.to_string())
}

fn request_origin(headers: &HeaderMap) -> String {
    header_str(headers, "origin")
        .or_else(|| header_str(headers, "referer").and_then(|value| origin_from_url(&value)))
        .or_else(|| header_str(headers, "host").map(|host| format!("http://{host}")))
        .unwrap_or_else(default_request_origin)
}

#[cfg(target_os = "windows")]
fn default_request_origin() -> String {
    "http://127.0.0.1:3847".into()
}

#[cfg(not(target_os = "windows"))]
fn default_request_origin() -> String {
    "http://127.0.0.1:3847".into()
}

fn origin_from_url(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    Some(url.origin().ascii_serialization())
}

fn iso8601_after(duration: Duration) -> String {
    let timestamp = unix_now_u64().saturating_add(duration.as_secs());
    format_rfc3339_utc(timestamp)
}

fn remote_control_device_identity_json(identity: &RemoteControlDeviceIdentity) -> Value {
    json!({
        "algorithm": identity.algorithm.as_str(),
        "key_id": identity.key_id.as_str(),
        "protection_class": identity.protection_class.as_str(),
        "public_key_spki_der_base64": identity.public_key_spki_der_base64.as_str(),
    })
}

fn remote_control_device_identity_hash(identity: &Value) -> Option<String> {
    let algorithm = identity.get("algorithm")?.as_str()?;
    let key_id = identity.get("key_id")?.as_str()?;
    let protection_class = identity.get("protection_class")?.as_str()?;
    let public_key_spki_der_base64 = identity.get("public_key_spki_der_base64")?.as_str()?;
    let canonical = json!({
        "algorithm": algorithm,
        "keyId": key_id,
        "protectionClass": protection_class,
        "publicKeySpkiDerBase64": public_key_spki_der_base64,
    });
    Some(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(canonical.to_string().as_bytes())),
    )
}

pub(super) async fn enroll(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<EnrollRequest>,
) -> impl IntoResponse {
    log_remote_control_entry_headers("server_enroll", &headers);
    chain_log::write_line(format!(
        "[remote_control] event=server_enroll_request name={} os={} arch={} app_server_version={} installation_id={}",
        request.name.as_deref().unwrap_or_default(),
        request.os.as_deref().unwrap_or_default(),
        request.arch.as_deref().unwrap_or_default(),
        request.app_server_version.as_deref().unwrap_or_default(),
        request.installation_id.as_deref().unwrap_or_default()
    ));
    let installation_id = request
        .installation_id
        .clone()
        .or_else(|| header_str(&headers, "x-codex-installation-id"))
        .unwrap_or_else(|| "unknown-installation".to_string());
    let server_id = stable_id("srv", &installation_id);
    let environment_id = stable_id("env", &installation_id);
    let expires_at = iso8601_after(Duration::from_secs(24 * 60 * 60));
    let remote_control_token = local_remote_control_server_token(&headers, &server_id);
    {
        let mut remote = state.remote_control.inner.lock().await;
        remote.server_id = Some(server_id.clone());
        remote.environment_id = Some(environment_id.clone());
        remote.server_name = request.name.clone();
        remote.installation_id = Some(installation_id.clone());
        remote.account_id = header_str(&headers, "chatgpt-account-id");
        remote.last_error = None;
    }
    state
        .push_event(
            "info",
            "remote_control_enrolled",
            format!(
                "server={} env={} name={} os={} arch={} version={}",
                server_id,
                environment_id,
                request.name.unwrap_or_default(),
                request.os.unwrap_or_default(),
                request.arch.unwrap_or_default(),
                request.app_server_version.unwrap_or_default()
            ),
        )
        .await;
    (
        StatusCode::OK,
        Json(EnrollResponse {
            server_id,
            environment_id,
            remote_control_token,
            expires_at,
        }),
    )
}

pub(super) async fn refresh(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<RefreshRequest>,
) -> impl IntoResponse {
    log_remote_control_entry_headers("server_refresh", &headers);
    chain_log::write_line(format!(
        "[remote_control] event=server_refresh_request server_id={} installation_id={}",
        request.server_id.as_deref().unwrap_or_default(),
        request.installation_id.as_deref().unwrap_or_default()
    ));
    let installation_id = request
        .installation_id
        .clone()
        .or_else(|| header_str(&headers, "x-codex-installation-id"))
        .unwrap_or_else(|| "unknown-installation".to_string());
    let expected_server_id = stable_id("srv", &installation_id);
    let server_id = request
        .server_id
        .clone()
        .unwrap_or(expected_server_id.clone());
    if server_id != expected_server_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!(
                    "remote control server refresh returned mismatched enrollment: expected server_id={expected_server_id}; got server_id={server_id}"
                )
            })),
        );
    }
    let environment_id = stable_id("env", &installation_id);
    let expires_at = iso8601_after(Duration::from_secs(24 * 60 * 60));
    let remote_control_token = local_remote_control_server_token(&headers, &server_id);
    {
        let mut remote = state.remote_control.inner.lock().await;
        remote.server_id = Some(server_id.clone());
        remote.environment_id = Some(environment_id.clone());
        remote.installation_id = Some(installation_id.clone());
        remote.account_id =
            header_str(&headers, "chatgpt-account-id").or(remote.account_id.clone());
        if let Some(user_agent) = header_str(&headers, "user-agent") {
            let source_kind = source_kind_from_user_agent(&user_agent);
            if source_kind != RemoteControlSourceKind::Unknown {
                remote.pending_source_hints_by_installation.insert(
                    installation_id.clone(),
                    RemoteControlSourceHint {
                        source_kind,
                        user_agent: Some(user_agent),
                        captured_at_ms: now_ms(),
                    },
                );
            }
        }
        remote.last_error = None;
    }
    state
        .push_event(
            "info",
            "remote_control_refreshed",
            format!("server={} env={}", server_id, environment_id),
        )
        .await;
    (
        StatusCode::OK,
        Json(json!({
            "server_id": server_id,
            "environment_id": environment_id,
            "remote_control_token": remote_control_token,
            "expires_at": expires_at,
        })),
    )
}

fn remote_control_finish_identity_summary(request: &RemoteControlClientFinishRequest) -> String {
    request
        .device_identity
        .as_ref()
        .map(|identity| {
            format!(
                "algorithm={} key_id={} protection_class={} public_key_len={}",
                identity.algorithm,
                identity.key_id,
                identity.protection_class,
                identity.public_key_spki_der_base64.len()
            )
        })
        .unwrap_or_else(|| "none".to_string())
}

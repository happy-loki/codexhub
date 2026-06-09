use axum::{
    Json,
    extract::{Form, Query},
    http::StatusCode,
    response::{IntoResponse, Redirect},
};
use base64::Engine;
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct OAuthAuthorizeQuery {
    redirect_uri: String,
    state: Option<String>,
    current_workspace_id: Option<String>,
    allowed_workspace_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct OAuthTokenRequest {
    code: String,
}

pub(super) async fn oauth_authorize(Query(query): Query<OAuthAuthorizeQuery>) -> impl IntoResponse {
    let account_id = query
        .current_workspace_id
        .or(query.allowed_workspace_id)
        .unwrap_or_else(|| "acct_codex_remote_local".to_string());
    let code = local_step_up_code(&account_id);
    let mut redirect_uri = match reqwest::Url::parse(&query.redirect_uri) {
        Ok(url) => url,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid redirect_uri").into_response(),
    };
    {
        let mut pairs = redirect_uri.query_pairs_mut();
        pairs.append_pair("code", &code);
        if let Some(state) = query.state {
            pairs.append_pair("state", &state);
        }
    }
    Redirect::temporary(redirect_uri.as_str()).into_response()
}

pub(super) async fn oauth_token(Form(request): Form<OAuthTokenRequest>) -> impl IntoResponse {
    let account_id = account_id_from_step_up_code(&request.code)
        .unwrap_or_else(|| "acct_codex_remote_local".to_string());
    let user_id = "user_codex_remote_local";
    let account_user_id = format!("{user_id}__{account_id}");
    let now = unix_now();
    let token = jwt_none(&serde_json::json!({
        "iss": "codex-remote-local",
        "aud": ["https://api.openai.com/v1"],
        "iat": now,
        "nbf": now,
        "exp": now + 5 * 60,
        "pwd_auth_time": now * 1000,
        "scope": "codex.remote_control.enroll",
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "account_id": account_id,
            "chatgpt_account_user_id": account_user_id,
            "account_user_id": account_user_id,
            "user_id": user_id,
        },
    }));
    Json(serde_json::json!({ "access_token": token })).into_response()
}

fn local_step_up_code(account_id: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&serde_json::json!({
            "account_id": account_id,
            "iat": unix_now(),
        }))
        .unwrap_or_default(),
    )
}

fn account_id_from_step_up_code(code: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(code)
        .ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok()?;
    value
        .get("account_id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn jwt_none(payload: &serde_json::Value) -> String {
    format!(
        "{}.{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({ "alg": "none", "typ": "JWT" }))
                .unwrap_or_default()
        ),
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(payload).unwrap_or_default()),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"sig")
    )
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

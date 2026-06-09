use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use base64::Engine;
use serde_json::{Value, json};

use crate::app_state::SharedState;

pub(super) async fn local_chatgpt_auth_tokens_response(state: &SharedState) -> Result<Value> {
    let codex_home = std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join(".codex"))
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(|home| std::path::PathBuf::from(home).join(".codex"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".codex"));
    let auth_path = codex_home.join("auth.json");
    let auth = read_auth_json(&auth_path)?;
    let account_id = auth
        .pointer("/tokens/account_id")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| {
            auth.pointer("/tokens/access_token")
                .and_then(|value| value.as_str())
                .and_then(jwt_chatgpt_account_id)
        })
        .or_else(|| {
            state
                .remote_control
                .inner
                .try_lock()
                .ok()
                .and_then(|remote| remote.account_id.clone())
        })
        .unwrap_or_else(|| "acct_codex_remote_local".to_string());
    let plan_type = auth
        .pointer("/tokens/access_token")
        .and_then(|value| value.as_str())
        .and_then(jwt_chatgpt_plan_type)
        .unwrap_or_else(|| "pro".to_string());
    let access_token = auth
        .pointer("/tokens/access_token")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| local_chatgpt_jwt(&account_id, &plan_type));
    Ok(json!({
        "accessToken": access_token,
        "chatgptAccountId": account_id,
        "chatgptPlanType": plan_type,
    }))
}

fn read_auth_json(path: &Path) -> Result<Value> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read Codex App auth {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn jwt_chatgpt_account_id(jwt: &str) -> Option<String> {
    jwt_payload(jwt).and_then(|payload| {
        payload
            .get("https://api.openai.com/auth")?
            .get("chatgpt_account_id")?
            .as_str()
            .map(str::to_string)
    })
}

fn jwt_chatgpt_plan_type(jwt: &str) -> Option<String> {
    jwt_payload(jwt).and_then(|payload| {
        payload
            .get("https://api.openai.com/auth")?
            .get("chatgpt_plan_type")?
            .as_str()
            .map(str::to_string)
    })
}

pub(in crate::remote_control_backend) fn jwt_payload(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn local_chatgpt_jwt(account_id: &str, plan_type: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let exp = now + 10 * 365 * 24 * 60 * 60;
    let payload = json!({
        "iss": "https://auth.openai.com",
        "aud": ["https://api.openai.com/v1"],
        "iat": now,
        "nbf": now,
        "exp": exp,
        "sub": "local|user_codex_remote_local",
        "email": "codex-remote-local@example.local",
        "email_verified": true,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "account_id": account_id,
            "chatgpt_account_user_id": format!("user_codex_remote_local__{account_id}"),
            "account_user_id": format!("user_codex_remote_local__{account_id}"),
            "chatgpt_plan_type": plan_type,
            "chatgpt_user_id": "user_codex_remote_local",
            "user_id": "user_codex_remote_local",
            "chatgpt_account_is_fedramp": false,
            "localhost": true,
            "groups": [],
            "organizations": [{
                "id": account_id,
                "is_default": true,
                "role": "owner",
                "title": "Codex Remote Local"
            }]
        },
        "scp": ["openid", "profile", "email", "offline_access"],
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

use axum::{Json, http::StatusCode};
use serde_json::{Value, json};

pub(super) async fn accounts_check() -> Json<Value> {
    Json(json!({
        "account_ordering": ["acct_codex_remote_local"],
        "current_account_id": "acct_codex_remote_local",
        "accounts": [{
            "id": "acct_codex_remote_local",
            "account_id": "acct_codex_remote_local",
            "account_user_id": "user_codex_remote_local__acct_codex_remote_local",
            "user_id": "user_codex_remote_local",
            "name": "Codex Remote Local",
            "title": "Codex Remote Local",
            "email": "codex-remote-local@example.local",
            "plan_type": "pro",
            "structure": "personal",
            "role": "owner",
            "is_default": true,
            "is_deactivated": false,
            "is_paid": true,
        }],
    }))
}

pub(super) async fn onboarding_context() -> Json<Value> {
    Json(json!({
        "account_id": "acct_codex_remote_local",
        "account_user_id": "user_codex_remote_local__acct_codex_remote_local",
        "completed": true,
        "requires_onboarding": false,
        "items": [],
    }))
}

pub(super) async fn usage() -> Json<Value> {
    Json(json!({
        "plan_type": "pro",
        "rate_limit": {
            "allowed": true,
            "limit_reached": false,
        },
        "credits": {
            "has_credits": true,
            "unlimited": true,
        },
    }))
}

pub(super) async fn beacons_home() -> Json<Value> {
    Json(json!({ "beacon_ui_response": Value::Null }))
}

pub(super) async fn beacons_event() -> Json<Value> {
    Json(json!({ "ok": true }))
}

pub(super) async fn tasks_list() -> Json<Value> {
    Json(json!({
        "items": [],
        "cursor": Value::Null,
    }))
}

pub(super) async fn wham_environments() -> Json<Value> {
    Json(json!({
        "items": [],
        "cursor": Value::Null,
    }))
}

pub(super) async fn wham_apps() -> Json<Value> {
    Json(json!({
        "items": [],
        "cursor": Value::Null,
    }))
}

pub(super) async fn connectors_directory_list() -> Json<Value> {
    Json(json!({
        "apps": [],
        "nextToken": Value::Null,
    }))
}

pub(super) async fn analytics_events() -> StatusCode {
    StatusCode::NO_CONTENT
}

pub(super) async fn accounts_mfa_info() -> Json<Value> {
    Json(json!({ "mfa_enabled_v2": true }))
}

pub(super) async fn remote_control_mfa_requirement() -> Json<Value> {
    Json(json!({ "requirement": "not_required" }))
}

use super::*;

// No Debug or raw JSON errors: an imported file contains bearer and refresh secrets.
#[derive(Serialize, Deserialize)]
pub struct ImportRequest {
    pub auth: Value,
}

impl AuthManager {
    pub fn import(&self, auth: Value) -> Result<AccountSummary, GatewayError> {
        let invalid = || {
            GatewayError::bad_request(
                "Select a Codex ChatGPT auth.json file containing valid account tokens",
            )
        };
        let mode = auth.get("auth_mode").and_then(Value::as_str);
        if mode.is_some_and(|m| !matches!(m, "chatgpt" | "chatgptAuthTokens"))
            || (mode.is_none()
                && auth
                    .get("OPENAI_API_KEY")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty()))
        {
            return Err(invalid());
        }
        let tokens = auth
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or_else(invalid)?;
        let value = |key: &str| {
            tokens
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
        };
        let access_token = value("access_token").ok_or_else(invalid)?;
        let id_token = value("id_token");
        let selected_account = value("account_id");
        let credential = Credential::from_tokens(
            TokenResponse {
                access_token,
                refresh_token: value("refresh_token"),
                id_token,
                expires_in: None,
            },
            None,
        )?;
        let access_account = claims(&credential.access_token)?
            .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if selected_account
            .as_ref()
            .is_some_and(|id| id != &credential.account_id)
            || access_account
                .as_ref()
                .is_some_and(|id| id != &credential.account_id)
        {
            return Err(GatewayError::bad_request(
                "ChatGPT account IDs in auth.json do not match",
            ));
        }
        if credential.refresh_token.is_empty() && credential.expires_at <= now() {
            return Err(auth_error(
                "Imported ChatGPT authorization has expired; sign in again",
            ));
        }
        self.save_login(&credential)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageWindow {
    pub used_percent: f64,
    pub limit_window_seconds: u64,
    pub reset_at: Option<i64>,
    pub reset_after_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageLimits {
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
    pub primary_window: Option<UsageWindow>,
    pub secondary_window: Option<UsageWindow>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageCredits {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AdditionalUsage {
    pub limit_name: String,
    pub metered_feature: Option<String>,
    pub rate_limit: Option<UsageLimits>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsagePayload {
    pub plan_type: String,
    pub rate_limit: Option<UsageLimits>,
    pub credits: Option<UsageCredits>,
    pub additional_rate_limits: Option<Vec<AdditionalUsage>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    pub account: AccountSummary,
    pub fetched_at: u64,
    pub usage: UsagePayload,
}

async fn usage_with_manager(
    auth: &AuthManager,
    client: &reqwest::Client,
    id: &str,
) -> Result<AccountUsage, GatewayError> {
    // Usage is a sibling of /codex, never a configurable provider URL.
    let endpoint = format!("{}/wham/usage", auth.backend.trim_end_matches("/codex"));
    let mut credential = auth.credential(client, id, None).await?;
    for attempt in 0..2 {
        let response = client
            .get(&endpoint)
            .bearer_auth(&credential.access_token)
            .header("ChatGPT-Account-ID", &credential.account_id)
            .header("originator", "codex_cli_rs")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|_| {
                GatewayError::upstream(
                    StatusCode::BAD_GATEWAY,
                    "Cannot connect to ChatGPT account usage",
                )
            })?;
        if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
            credential = auth
                .credential(client, id, Some(&credential.access_token))
                .await?;
            continue;
        }
        if !response.status().is_success() {
            return Err(GatewayError::upstream(
                response.status(),
                format!(
                    "ChatGPT account usage failed (HTTP {})",
                    response.status().as_u16()
                ),
            ));
        }
        let usage: UsagePayload = response.json().await.map_err(|_| {
            GatewayError::upstream(
                StatusCode::BAD_GATEWAY,
                "Invalid ChatGPT account usage response",
            )
        })?;
        let mut account = credential.summary(id);
        account.plan = Some(usage.plan_type.clone());
        return Ok(AccountUsage {
            account,
            fetched_at: now(),
            usage,
        });
    }
    unreachable!("usage request returns after the second attempt")
}

pub async fn import_account_api(
    Json(input): Json<Value>,
) -> Result<Json<AccountSummary>, GatewayError> {
    let request: ImportRequest = serde_json::from_value(input)
        .map_err(|_| GatewayError::bad_request("A Codex auth.json object is required"))?;
    manager().import(request.auth).map(Json)
}

pub async fn account_usage_api(
    Json(input): Json<IdRequest>,
) -> Result<Json<AccountUsage>, GatewayError> {
    usage_with_manager(manager(), &crate::outbound_http::get(), &input.id)
        .await
        .map(Json)
}

#[cfg(test)]
mod tests;

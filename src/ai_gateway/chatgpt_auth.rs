use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Notify};

use super::{
    config::{ProviderConfig, ProviderType},
    error::GatewayError,
};

mod account;
pub use account::{
    AccountUsage, ImportRequest, UsageLimits, UsageWindow, account_usage_api, import_account_api,
};

pub const BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
static MANAGER: LazyLock<AuthManager> =
    LazyLock::new(|| AuthManager::new(auth_directory(), ISSUER.into()));

pub fn manager() -> &'static AuthManager {
    &MANAGER
}

fn auth_directory() -> PathBuf {
    if let Some(root) = std::env::var_os("CODEXHUB_HOME") {
        return PathBuf::from(root).join("chatgpt-auth");
    }
    #[cfg(windows)]
    let root = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .map(|p| p.join("CodexHub"));
    #[cfg(target_os = "macos")]
    let root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join("Library/Application Support/CodexHub"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|p| p.join(".local/share"))
        })
        .map(|p| p.join("CodexHub"));
    root.unwrap_or_default().join("chatgpt-auth")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn random_secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn auth_error(message: impl Into<String>) -> GatewayError {
    GatewayError::upstream(StatusCode::UNAUTHORIZED, message)
}

// Claims are used as metadata only. The official server validates the bearer token.
fn claims(token: &str) -> Result<Value, GatewayError> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| auth_error("Invalid ChatGPT token"))?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| auth_error("Invalid ChatGPT token encoding"))?;
    serde_json::from_slice(&decoded).map_err(|_| auth_error("Invalid ChatGPT token claims"))
}

#[derive(Clone, Deserialize, Serialize)]
struct Credential {
    access_token: String,
    refresh_token: String,
    account_id: String,
    email: Option<String>,
    plan: Option<String>,
    expires_at: u64,
    #[serde(default)]
    needs_login: bool,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

impl Credential {
    fn from_tokens(
        tokens: TokenResponse,
        previous: Option<&Credential>,
    ) -> Result<Self, GatewayError> {
        if tokens.access_token.trim().is_empty() {
            return Err(auth_error("ChatGPT returned an empty access token"));
        }
        let access = claims(&tokens.access_token)?;
        let identity = tokens
            .id_token
            .as_deref()
            .map(claims)
            .transpose()?
            .unwrap_or_else(|| access.clone());
        let auth_claim = |key: &str| {
            identity
                .get("https://api.openai.com/auth")
                .and_then(|a| a.get(key))
                .or_else(|| {
                    access
                        .get("https://api.openai.com/auth")
                        .and_then(|a| a.get(key))
                })
        };
        if auth_claim("chatgpt_account_is_fedramp").and_then(Value::as_bool) == Some(true) {
            return Err(auth_error(
                "This ChatGPT workspace requires a separate regional endpoint",
            ));
        }
        let account_id = auth_claim("chatgpt_account_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| previous.map(|p| p.account_id.clone()))
            .ok_or_else(|| auth_error("ChatGPT account ID is missing"))?;
        if previous.is_some_and(|p| p.account_id != account_id) {
            return Err(auth_error(
                "ChatGPT account changed while refreshing; sign in again",
            ));
        }
        let refresh_token = tokens
            .refresh_token
            .filter(|s| !s.is_empty())
            .or_else(|| previous.map(|p| p.refresh_token.clone()))
            .unwrap_or_default();
        let email = identity
            .get("email")
            .or_else(|| identity.pointer("/https:~1~1api.openai.com~1profile/email"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| previous.and_then(|p| p.email.clone()));
        let plan = auth_claim("chatgpt_plan_type")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| previous.and_then(|p| p.plan.clone()));
        let expires_at = access
            .get("exp")
            .and_then(Value::as_u64)
            .or_else(|| tokens.expires_in.map(|s| now().saturating_add(s)))
            .ok_or_else(|| auth_error("ChatGPT token expiration is missing"))?;
        Ok(Self {
            access_token: tokens.access_token,
            refresh_token,
            account_id,
            email,
            plan,
            expires_at,
            needs_login: false,
        })
    }

    fn summary(&self, auth_id: &str) -> AccountSummary {
        AccountSummary {
            auth_id: auth_id.into(),
            email: self.email.clone(),
            plan: self.plan.clone(),
            needs_login: self.needs_login
                || (self.refresh_token.is_empty() && self.expires_at <= now()),
            can_refresh: !self.refresh_token.is_empty(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub auth_id: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub needs_login: bool,
    pub can_refresh: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStart {
    pub session_id: String,
    pub authorization_url: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStatus {
    pub done: bool,
    pub account: Option<AccountSummary>,
    pub error: Option<String>,
    #[serde(skip)]
    exchanging: bool,
}

struct LoginSession {
    created: Instant,
    status: Mutex<LoginStatus>,
    stop: Notify,
    verifier: String,
    state: String,
    redirect_uri: String,
}

pub struct AuthManager {
    root: PathBuf,
    issuer: String,
    backend: String,
    sessions: Mutex<HashMap<String, Arc<LoginSession>>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl AuthManager {
    fn new(root: PathBuf, issuer: String) -> Self {
        Self {
            root,
            issuer,
            backend: BASE_URL.into(),
            sessions: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
        }
    }

    fn credential_path(&self, id: &str) -> Result<PathBuf, GatewayError> {
        let uuid = uuid::Uuid::parse_str(id)
            .map_err(|_| auth_error("Invalid ChatGPT credential reference"))?;
        Ok(self.root.join(format!("{}.json", uuid.simple())))
    }

    fn read(&self, id: &str) -> Result<Credential, GatewayError> {
        let content = std::fs::read(self.credential_path(id)?)
            .map_err(|_| auth_error("ChatGPT credentials are unavailable; sign in again"))?;
        serde_json::from_slice(&content)
            .map_err(|_| auth_error("ChatGPT credentials are invalid; sign in again"))
    }

    fn write(&self, id: &str, credential: &Credential) -> Result<(), GatewayError> {
        let path = self.credential_path(id)?;
        write_private_json(&path, credential).map_err(|_| {
            GatewayError::upstream(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Cannot save ChatGPT credentials",
            )
        })
    }

    pub async fn summary(&self, id: &str) -> Result<AccountSummary, GatewayError> {
        Ok(self.read(id)?.summary(id))
    }

    async fn account_lock(
        &self,
        id: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, GatewayError> {
        self.credential_path(id)?;
        let lock = self
            .locks
            .lock()
            .await
            .entry(id.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        Ok(lock.lock_owned().await)
    }

    pub async fn logout(&self, id: &str) -> Result<(), GatewayError> {
        let _guard = self.account_lock(id).await?;
        if !self.credential_path(id)?.exists() {
            return Ok(());
        }
        let _file_guard = self.file_lock(id).await?;
        match std::fs::remove_file(self.credential_path(id)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(auth_error("Cannot remove ChatGPT credentials")),
        }
    }

    async fn file_lock(&self, id: &str) -> Result<std::fs::File, GatewayError> {
        // A second daemon must not spend a rotating refresh token concurrently.
        let lock_path = self.credential_path(id)?.with_extension("lock");
        let file_guard = tokio::task::spawn_blocking(move || -> std::io::Result<std::fs::File> {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(lock_path)?;
            fs2::FileExt::try_lock_exclusive(&file)?;
            Ok(file)
        })
        .await
        .map_err(|_| auth_error("ChatGPT credential lock failed"))?
        .map_err(|_| {
            GatewayError::upstream(
                StatusCode::SERVICE_UNAVAILABLE,
                "ChatGPT credentials are being updated; retry shortly",
            )
        })?;
        Ok(file_guard)
    }

    async fn credential(
        &self,
        client: &reqwest::Client,
        id: &str,
        rejected: Option<&str>,
    ) -> Result<Credential, GatewayError> {
        let _guard = self.account_lock(id).await?;
        self.read(id)?;
        let _file_guard = self.file_lock(id).await?;
        let previous = self.read(id)?;
        if previous.needs_login {
            return Err(auth_error("ChatGPT authorization expired; sign in again"));
        }
        let force = rejected.is_some_and(|token| token == previous.access_token);
        let refresh_margin = if previous.refresh_token.is_empty() {
            0
        } else {
            60
        };
        if !force && previous.expires_at > now().saturating_add(refresh_margin) {
            return Ok(previous);
        }
        if previous.refresh_token.is_empty() {
            let mut invalid = previous;
            invalid.needs_login = true;
            self.write(id, &invalid)?;
            return Err(auth_error("ChatGPT authorization expired; sign in again"));
        }
        let response = client.post(format!("{}/oauth/token", self.issuer))
            .timeout(Duration::from_secs(30))
            .json(&json!({"client_id":CLIENT_ID,"grant_type":"refresh_token","refresh_token":previous.refresh_token}))
            .send().await.map_err(|_| GatewayError::upstream(StatusCode::BAD_GATEWAY, "ChatGPT token refresh could not connect"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            let code = body
                .pointer("/error/code")
                .and_then(Value::as_str)
                .or_else(|| body.get("error").and_then(Value::as_str))
                .unwrap_or_default();
            if status == StatusCode::UNAUTHORIZED
                || matches!(
                    code,
                    "invalid_grant"
                        | "refresh_token_expired"
                        | "refresh_token_reused"
                        | "refresh_token_invalidated"
                )
            {
                let mut invalid = previous;
                invalid.needs_login = true;
                self.write(id, &invalid)?;
                return Err(auth_error("ChatGPT authorization expired; sign in again"));
            }
            return Err(GatewayError::upstream(
                StatusCode::BAD_GATEWAY,
                format!("ChatGPT token refresh failed (HTTP {})", status.as_u16()),
            ));
        }
        let tokens = response
            .json::<TokenResponse>()
            .await
            .map_err(|_| auth_error("Invalid ChatGPT token refresh response"))?;
        let updated = Credential::from_tokens(tokens, Some(&previous))?;
        self.write(id, &updated)?;
        Ok(updated)
    }

    pub async fn start(&'static self) -> Result<LoginStart, GatewayError> {
        let listener =
            match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 1455)).await {
                Ok(listener) => listener,
                Err(_) => tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                    .await
                    .map_err(|_| auth_error("Cannot start ChatGPT login callback"))?,
            };
        let port = listener
            .local_addr()
            .map_err(|_| auth_error("Cannot read login callback port"))?
            .port();
        let session = Arc::new(LoginSession {
            created: Instant::now(),
            status: Mutex::new(LoginStatus::default()),
            stop: Notify::new(),
            verifier: random_secret(),
            state: random_secret(),
            redirect_uri: format!("http://localhost:{port}/auth/callback"),
        });
        let session_id = uuid::Uuid::new_v4().to_string();
        let authorization_url = authorize_url(&self.issuer, &session);
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, s| s.created.elapsed() < LOGIN_TIMEOUT);
        if sessions.len() >= 4 {
            return Err(auth_error(
                "Too many pending ChatGPT logins; cancel an earlier login",
            ));
        }
        sessions.insert(session_id.clone(), session.clone());
        drop(sessions);
        let callback_state = CallbackState {
            manager: self,
            session: session.clone(),
            client: crate::outbound_http::get(),
        };
        tokio::spawn(async move {
            let app = Router::new()
                .route("/auth/callback", get(login_callback))
                .with_state(callback_state);
            let shutdown_session = session.clone();
            let _ = axum::serve(listener, app).with_graceful_shutdown(async move {
                tokio::select! { _ = shutdown_session.stop.notified() => {}, _ = tokio::time::sleep(LOGIN_TIMEOUT) => {} }
            }).await;
            let mut status = session.status.lock().await;
            if !status.done {
                status.done = true;
                status.error = Some("ChatGPT login timed out or was cancelled".into());
            }
        });
        Ok(LoginStart {
            session_id,
            authorization_url,
        })
    }

    pub async fn login_status(&self, id: &str) -> Result<LoginStatus, GatewayError> {
        let session = self
            .sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| auth_error("ChatGPT login session expired"))?;
        Ok(session.status.lock().await.clone())
    }

    pub async fn cancel(&self, id: &str) {
        if let Some(session) = self.sessions.lock().await.remove(id) {
            let mut status = session.status.lock().await;
            if !status.done {
                status.done = true;
                status.error = Some("ChatGPT login cancelled".into());
            }
            session.stop.notify_one();
        }
    }

    async fn exchange(
        &self,
        client: &reqwest::Client,
        session: &LoginSession,
        code: &str,
    ) -> Result<Credential, GatewayError> {
        let response = client
            .post(format!("{}/oauth/token", self.issuer))
            .timeout(Duration::from_secs(30))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &session.redirect_uri),
                ("client_id", CLIENT_ID),
                ("code_verifier", &session.verifier),
            ])
            .send()
            .await
            .map_err(|_| {
                auth_error("Cannot exchange ChatGPT authorization; try signing in again")
            })?;
        if !response.status().is_success() {
            return Err(auth_error(format!(
                "ChatGPT login failed (HTTP {})",
                response.status().as_u16()
            )));
        }
        let tokens = response
            .json::<TokenResponse>()
            .await
            .map_err(|_| auth_error("Invalid ChatGPT login response"))?;
        Credential::from_tokens(tokens, None)
    }

    fn save_login(&self, credential: &Credential) -> Result<AccountSummary, GatewayError> {
        let id = uuid::Uuid::new_v4().to_string();
        self.write(&id, credential)?;
        Ok(credential.summary(&id))
    }
}

fn write_private_json(path: &Path, credential: &Credential) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing credential directory"))?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(temp.as_file_mut(), credential)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn authorize_url(issuer: &str, session: &LoginSession) -> String {
    let mut url =
        url::Url::parse(&format!("{issuer}/oauth/authorize")).expect("valid OAuth issuer");
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(session.verifier.as_bytes()));
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", session.redirect_uri.as_str()),
        (
            "scope",
            "openid profile email offline_access api.connectors.read api.connectors.invoke",
        ),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", session.state.as_str()),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", "codex_cli_rs"),
    ]);
    url.into()
}

#[derive(Clone)]
struct CallbackState {
    manager: &'static AuthManager,
    session: Arc<LoginSession>,
    client: reqwest::Client,
}
#[derive(Deserialize)]
struct CallbackQuery {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}

async fn login_callback(
    State(state): State<CallbackState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if query.state.as_deref() != Some(state.session.state.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            "Invalid login state. Return to CodexHub and start again.",
        )
            .into_response();
    }
    let mut status = state.session.status.lock().await;
    if status.done || status.exchanging || state.session.created.elapsed() >= LOGIN_TIMEOUT {
        return (
            StatusCode::BAD_REQUEST,
            "This login has already finished or expired.",
        )
            .into_response();
    }
    if query.error.is_none() && query.code.as_deref().is_none_or(str::is_empty) {
        return (StatusCode::BAD_REQUEST, "Missing authorization code.").into_response();
    }
    status.exchanging = true;
    drop(status);
    let result = if query.error.is_some() {
        Err(auth_error("ChatGPT login was denied"))
    } else if let Some(code) = query.code.filter(|s| !s.is_empty()) {
        state
            .manager
            .exchange(&state.client, &state.session, &code)
            .await
    } else {
        return (StatusCode::BAD_REQUEST, "Missing authorization code.").into_response();
    };
    let mut status = state.session.status.lock().await;
    if status.done || state.session.created.elapsed() >= LOGIN_TIMEOUT {
        return (
            StatusCode::BAD_REQUEST,
            "This login was cancelled or expired.",
        )
            .into_response();
    }
    let result = result.and_then(|credential| state.manager.save_login(&credential));
    status.done = true;
    let success = result.is_ok();
    match result {
        Ok(account) => status.account = Some(account),
        Err(err) => status.error = Some(err.message),
    }
    state.session.stop.notify_one();
    let message = if success {
        "ChatGPT sign-in complete. You can close this page and return to CodexHub."
    } else {
        "ChatGPT sign-in failed. Return to CodexHub to retry."
    };
    (
        [
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
        ],
        Html(format!(
            "<!doctype html><meta charset=utf-8><title>CodexHub</title><p>{message}</p>"
        )),
    )
        .into_response()
}

pub fn endpoint(provider: &ProviderConfig, api_path: &str) -> String {
    if provider.provider_type == ProviderType::ChatGptResponses {
        format!(
            "{BASE_URL}{}",
            api_path.strip_prefix("/v1").unwrap_or(api_path)
        )
    } else {
        format!(
            "{}{}",
            super::config::provider_api_root(&provider.base_url),
            api_path
        )
    }
}

pub async fn authorize(
    client: &reqwest::Client,
    request: &mut reqwest::Request,
    provider: &ProviderConfig,
    rejected: Option<&str>,
) -> Result<(), GatewayError> {
    authorize_with_manager(manager(), client, request, provider, rejected).await
}

pub(super) async fn authorize_with_manager(
    auth: &AuthManager,
    client: &reqwest::Client,
    request: &mut reqwest::Request,
    provider: &ProviderConfig,
    rejected: Option<&str>,
) -> Result<(), GatewayError> {
    if provider.provider_type != ProviderType::ChatGptResponses {
        return Ok(());
    }
    let url = request.url();
    let expected =
        url::Url::parse(&auth.backend).map_err(|_| auth_error("Invalid ChatGPT endpoint"))?;
    if url.origin() != expected.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || !url
            .path()
            .starts_with(&format!("{}/", expected.path().trim_end_matches('/')))
    {
        return Err(auth_error(
            "ChatGPT credentials can only be sent to the official Codex endpoint",
        ));
    }
    let id = provider
        .chatgpt_auth_id
        .as_deref()
        .ok_or_else(|| auth_error("Sign in to the ChatGPT channel first"))?;
    let credential = auth.credential(client, id, rejected).await?;
    let headers = request.headers_mut();
    let mut bearer =
        reqwest::header::HeaderValue::from_str(&format!("Bearer {}", credential.access_token))
            .map_err(|_| auth_error("Invalid ChatGPT access token"))?;
    bearer.set_sensitive(true);
    headers.insert("authorization", bearer);
    headers.insert(
        "chatgpt-account-id",
        reqwest::header::HeaderValue::from_str(&credential.account_id)
            .map_err(|_| auth_error("Invalid ChatGPT account ID"))?,
    );
    headers.remove("openai-organization");
    headers.remove("openai-project");
    headers.insert(
        "originator",
        reqwest::header::HeaderValue::from_static("codex_cli_rs"),
    );
    Ok(())
}

pub async fn models(auth_id: &str) -> Result<Vec<String>, GatewayError> {
    let client = crate::outbound_http::get();
    models_with_manager(manager(), &client, auth_id).await
}

async fn models_with_manager(
    auth: &AuthManager,
    client: &reqwest::Client,
    auth_id: &str,
) -> Result<Vec<String>, GatewayError> {
    let provider = ProviderConfig {
        provider_type: ProviderType::ChatGptResponses,
        chatgpt_auth_id: Some(auth_id.into()),
        ..Default::default()
    };
    let mut request = client
        .get(format!("{}/models", auth.backend))
        .query(&[(
            "client_version",
            super::catalog::codex_compatibility_version(),
        )])
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| auth_error("Cannot build ChatGPT model request"))?;
    authorize_with_manager(auth, client, &mut request, &provider, None).await?;
    let response = super::providers::execute_openai_request_with_auth(
        client,
        request,
        &provider,
        "ChatGPT model request failed",
        auth,
    )
    .await?;
    if !response.status().is_success() {
        return Err(GatewayError::upstream(
            response.status(),
            format!(
                "ChatGPT model list failed (HTTP {})",
                response.status().as_u16()
            ),
        ));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|_| auth_error("Invalid ChatGPT model list"))?;
    let entries = body
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| auth_error("ChatGPT model list is missing"))?;
    let mut models: Vec<String> = entries
        .iter()
        .filter(|m| m.get("visibility").and_then(Value::as_str) != Some("hide"))
        .filter_map(|m| m.get("slug").and_then(Value::as_str).map(str::to_owned))
        .collect();
    models.sort();
    models.dedup();
    Ok(models)
}

pub async fn start_login_api(Json(_): Json<Value>) -> Result<Json<LoginStart>, GatewayError> {
    manager().start().await.map(Json)
}
#[derive(Deserialize)]
pub struct IdRequest {
    pub id: String,
}
pub async fn login_status_api(
    Json(input): Json<IdRequest>,
) -> Result<Json<LoginStatus>, GatewayError> {
    manager().login_status(&input.id).await.map(Json)
}
pub async fn cancel_login_api(Json(input): Json<IdRequest>) -> Json<Value> {
    manager().cancel(&input.id).await;
    Json(json!({"ok":true}))
}
pub async fn account_status_api(
    Json(input): Json<IdRequest>,
) -> Result<Json<AccountSummary>, GatewayError> {
    manager().summary(&input.id).await.map(Json)
}
pub async fn account_models_api(
    Json(input): Json<IdRequest>,
) -> Result<Json<Vec<String>>, GatewayError> {
    models(&input.id).await.map(Json)
}
pub async fn logout_api(Json(input): Json<IdRequest>) -> Result<Json<Value>, GatewayError> {
    manager().logout(&input.id).await?;
    Ok(Json(json!({"ok":true})))
}

#[cfg(test)]
mod tests;

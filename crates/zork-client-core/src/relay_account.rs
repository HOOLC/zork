//! Public relay account operations. Mesh membership and LAN access are independent.
mod login;
mod runtime;
#[cfg(test)]
mod tests;
pub use login::Login;
pub use runtime::{Access, RelayAccountTask};

use anyhow::{ensure, Context, Result};
use reqwest::{Client, Method, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};
use zork_config::relay_account::{self as storage, AccountFile, RelaySession, Revocation};

#[derive(Clone)]
pub struct Account {
    root: PathBuf,
    origin: String,
    http: Client,
}
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub origin: String,
    pub subject: Option<String>,
    pub email: Option<String>,
    pub session_id: Option<String>,
    pub access_expires_at: Option<i64>,
    pub refresh_expires_at: Option<i64>,
    pub authenticated: bool,
    pub revocations_pending: usize,
    pub remote_verified: bool,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub refresh_expires_at: i64,
    pub current: bool,
}
#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    token_type: String,
    subject: String,
    email: String,
    session_id: String,
    expires_at: i64,
    session_expires_at: i64,
    refresh_expires_at: i64,
}
#[derive(Debug, thiserror::Error)]
#[error("relay control plane returned HTTP {status} ({code})")]
struct ApiError {
    status: StatusCode,
    code: &'static str,
}

impl Account {
    pub fn new(root: impl Into<PathBuf>, origin: &str) -> Result<Self> {
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .build()?;
        Ok(Self {
            root: root.into(),
            origin: storage::canonical_origin(origin)?,
            http,
        })
    }
    pub fn configured(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let mut config = zork_config::ensure_layout(&root)?.mesh;
        zork_config::services::ServicesConfig::load_from_install()?.apply_defaults(&mut config)?;
        let origin = storage::control_origin(config.relay_urls.as_deref())
            .context("public relay origin is not configured")?;
        Self::new(root, &origin)
    }
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn data_root(&self) -> &Path {
        &self.root
    }

    async fn lock(&self) -> Result<File> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(lock) = storage::try_lock(&self.root)? {
                return Ok(lock);
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "another relay account operation is still running"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let mut request = self.http.request(method, format!("{}{path}", self.origin));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("relay control plane is unavailable"))?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("relay response was interrupted"))?
        {
            ensure!(
                bytes.len() + chunk.len() <= 64 * 1024,
                "relay response is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            // Never include arbitrary response bodies, URLs, or credentials in diagnostics.
            let code = match serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|body| body["error"].as_str().map(str::to_owned))
                .as_deref()
            {
                Some("refresh_reused") => "refresh_reused",
                Some("rate_limited") => "rate_limited",
                Some("account_blocked") => "account_blocked",
                Some("invalid_grant") => "invalid_grant",
                _ => "request_failed",
            };
            return Err(ApiError { status, code }.into());
        }
        serde_json::from_slice(&bytes).context("invalid relay response")
    }
    fn session_from(&self, tokens: Tokens) -> Result<RelaySession> {
        let now = storage::now();
        ensure!(
            tokens.token_type == "Bearer"
                && tokens.expires_at > now
                && tokens.expires_at <= now + 360
                && tokens.refresh_expires_at > now
                && tokens.refresh_expires_at <= tokens.session_expires_at
                && tokens.session_expires_at <= now + 30 * 86400 + 60
                && ulid::Ulid::from_string(&tokens.session_id).is_ok(),
            "invalid relay session response"
        );
        Ok(RelaySession {
            origin: self.origin.clone(),
            token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            subject: tokens.subject,
            email: (!tokens.email.is_empty()).then_some(tokens.email),
            session_id: tokens.session_id,
            expires_at: tokens.expires_at,
            session_expires_at: tokens.session_expires_at,
            refresh_expires_at: tokens.refresh_expires_at,
            pending_refresh: None,
        })
    }
    fn matching<'a>(&self, account: &'a AccountFile) -> Option<&'a RelaySession> {
        account
            .current
            .as_ref()
            .filter(|session| session.origin == self.origin)
    }
    pub fn cached_access(&self) -> Result<Option<Access>> {
        Ok(self
            .matching(&storage::read(&self.root)?)
            .filter(|s| s.active())
            .map(Access::from))
    }

    /// Only one process rotates at a time. The attempt ID is durable before the
    /// request; retries after a lost response reuse it instead of the old family.
    pub async fn access(&self, force_refresh: bool) -> Result<Option<Access>> {
        let _lock = self.lock().await?;
        let mut account = storage::read(&self.root)?;
        let Some(mut session) = self.matching(&account).cloned() else {
            return Ok(None);
        };
        if !session.renewable() {
            account.current = None;
            storage::write(&self.root, &account)?;
            return Ok(None);
        }
        if !force_refresh
            && session.pending_refresh.is_none()
            && session.expires_at > storage::now() + 60
        {
            return Ok(Some(Access::from(&session)));
        }
        let new_attempt = session.pending_refresh.is_none();
        let request_id = session
            .pending_refresh
            .get_or_insert_with(|| ulid::Ulid::new().to_string())
            .clone();
        if new_attempt {
            account.current = Some(session.clone());
            storage::write(&self.root, &account)?;
        }
        let response = self
            .request::<Tokens>(
                Method::POST,
                "/v1/auth/refresh",
                Some(&session.refresh_token),
                Some(serde_json::json!({ "request_id": request_id })),
            )
            .await;
        let tokens = match response {
            Ok(tokens) => tokens,
            Err(error) => {
                if error.downcast_ref::<ApiError>().is_some_and(|e| {
                    matches!(e.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                }) {
                    account.current = None;
                    storage::write(&self.root, &account)?;
                }
                return Err(error);
            }
        };
        let renewed = self.session_from(tokens)?;
        ensure!(
            renewed.session_id == session.session_id
                && renewed.subject == session.subject
                && renewed.session_expires_at == session.session_expires_at,
            "relay refresh changed session identity"
        );
        let access = Access::from(&renewed);
        account.current = Some(renewed);
        storage::write(&self.root, &account)?;
        Ok(Some(access))
    }

    pub async fn status(&self, verify_remote: bool) -> Result<Status> {
        if verify_remote {
            let _ = self.access(false).await;
        }
        let account = storage::read(&self.root)?;
        let session = self.matching(&account);
        let mut status = Status {
            origin: self.origin.clone(),
            subject: session.map(|s| s.subject.clone()),
            email: session.and_then(|s| s.email.clone()),
            session_id: session.map(|s| s.session_id.clone()),
            access_expires_at: session.map(|s| s.expires_at),
            refresh_expires_at: session.map(|s| s.refresh_expires_at),
            authenticated: session.is_some_and(|s| s.active()),
            revocations_pending: account.pending_revocations.len(),
            remote_verified: false,
        };
        if verify_remote {
            if let Some(session) = session.filter(|s| s.active()) {
                let result = self
                    .request::<serde_json::Value>(
                        Method::GET,
                        "/v1/auth/session",
                        Some(&session.token),
                        None,
                    )
                    .await;
                match result {
                    Ok(_) => status.remote_verified = true,
                    Err(error)
                        if error.downcast_ref::<ApiError>().is_some_and(|e| {
                            matches!(e.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
                        }) =>
                    {
                        self.clear_if_same(session).await?;
                        status.authenticated = false;
                        status.remote_verified = true;
                    }
                    Err(_) => {}
                }
            }
        }
        Ok(status)
    }
    async fn clear_if_same(&self, session: &RelaySession) -> Result<()> {
        let _lock = self.lock().await?;
        let mut account = storage::read(&self.root)?;
        if account.current.as_ref() == Some(session) {
            account.current = None;
            storage::write(&self.root, &account)?;
        }
        Ok(())
    }

    pub async fn sessions(&self) -> Result<Vec<SessionInfo>> {
        let access = self.access(false).await?.context("relay login required")?;
        #[derive(Deserialize)]
        struct List {
            sessions: Vec<SessionInfo>,
        }
        Ok(self
            .request::<List>(Method::GET, "/v1/auth/sessions", Some(access.token()), None)
            .await?
            .sessions)
    }
    pub async fn revoke(&self, id: &str) -> Result<()> {
        ensure!(ulid::Ulid::from_string(id).is_ok(), "invalid session ID");
        let access = self.access(false).await?.context("relay login required")?;
        self.request::<serde_json::Value>(
            Method::DELETE,
            &format!("/v1/auth/sessions/{id}"),
            Some(access.token()),
            None,
        )
        .await?;
        let _lock = self.lock().await?;
        let mut account = storage::read(&self.root)?;
        if self.matching(&account).is_some_and(|s| s.session_id == id) {
            account.current = None;
            storage::write(&self.root, &account)?;
        }
        Ok(())
    }

    /// Local access stops durably before any network request. An offline logout
    /// stays pending, and is never reported as successful remote revocation.
    pub async fn logout(&self, all: bool) -> Result<usize> {
        {
            let _lock = self.lock().await?;
            let mut account = storage::read(&self.root)?;
            account.login_attempt = None;
            if let Some(session) = account.current.take() {
                ensure!(
                    account.pending_revocations.len() < 64,
                    "too many pending relay revocations; reconnect to finish logout"
                );
                account
                    .pending_revocations
                    .push(Revocation { session, all });
            }
            storage::write(&self.root, &account)?;
        }
        self.flush_revocations().await
    }

    pub async fn flush_revocations(&self) -> Result<usize> {
        let _lock = self.lock().await?;
        let mut account = storage::read(&self.root)?;
        if account.pending_revocations.is_empty() {
            return Ok(0);
        }
        let original_count = account.pending_revocations.len();
        let mut pending: Vec<Revocation> = Vec::new();
        for index in 0..original_count {
            let mut revocation = account.pending_revocations[index].clone();
            // A credential is sent only to the origin that originally issued it.
            let issuer = Self::new(self.root.clone(), &revocation.session.origin)?;
            if !revocation.all && revocation.session.session_expires_at <= storage::now() {
                continue;
            }
            // An account-wide logout needs the current refresh credential. If
            // the last rotation's response was lost, finish that exact retry.
            if revocation.all {
                if let Some(request_id) = &revocation.session.pending_refresh {
                    if let Ok(tokens) = issuer
                        .request::<Tokens>(
                            Method::POST,
                            "/v1/auth/refresh",
                            Some(&revocation.session.refresh_token),
                            Some(serde_json::json!({"request_id":request_id})),
                        )
                        .await
                    {
                        let renewed = issuer.session_from(tokens)?;
                        ensure!(
                            renewed.session_id == revocation.session.session_id
                                && renewed.subject == revocation.session.subject,
                            "relay refresh changed session identity"
                        );
                        revocation.session = renewed;
                        account.pending_revocations[index] = revocation.clone();
                        storage::write(&self.root, &account)?;
                    }
                }
            }
            let result = issuer
                .request::<serde_json::Value>(
                    Method::POST,
                    "/v1/auth/logout",
                    Some(&revocation.session.refresh_token),
                    Some(serde_json::json!({ "all": revocation.all })),
                )
                .await;
            // Denied authentication is not acknowledgement of revocation (for
            // example after a signing-key rotation while an old socket lives).
            let finished = result.is_ok();
            if !finished {
                pending.push(revocation);
            } else if revocation.all {
                pending.retain(|older| {
                    older.session.origin != revocation.session.origin
                        || older.session.subject != revocation.session.subject
                });
            }
        }
        account.pending_revocations = pending;
        let count = account.pending_revocations.len();
        if count != original_count {
            storage::write(&self.root, &account)?;
        }
        Ok(count)
    }
}

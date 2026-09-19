//! Browser authorization that also works for headless nodes and mobile clients.
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};

pub struct DeviceLogin {
    account: Account,
    id: String,
    verifier: String,
    attempt: String,
    url: String,
    deadline: tokio::time::Instant,
    interval: Duration,
}
impl Account {
    pub async fn begin_device_login(&self, name: &str) -> Result<DeviceLogin> {
        let attempt = ulid::Ulid::new().to_string();
        self.reserve_login(&attempt).await?;
        self.start_device_login(name, attempt).await
    }
    pub(super) async fn start_device_login(
        &self,
        name: &str,
        attempt: String,
    ) -> Result<DeviceLogin> {
        let result = self.device_flow(name, attempt.clone()).await;
        if result.is_err() {
            let _ = self.cancel_login(&attempt).await;
        }
        result
    }
    async fn device_flow(&self, name: &str, attempt: String) -> Result<DeviceLogin> {
        ensure!(
            storage::read(&self.root)?.login_attempt.as_deref() == Some(&attempt),
            "Google login was cancelled"
        );
        ensure!(
            !name.trim().is_empty() && name.len() <= 80,
            "login device name must be 1–80 bytes"
        );
        let verifier = super::login::secret()?;
        let device_id = super::login::secret()?;
        #[derive(Deserialize)]
        struct Started {
            verification_uri: String,
            expires_at: i64,
            interval: u64,
        }
        let started: Started = self.request(Method::POST, "/v1/auth/device", None,
            Some(serde_json::json!({"id":device_id,"code_challenge": URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())), "name":name}))).await?;
        let url = reqwest::Url::parse(&started.verification_uri)?;
        let id = url
            .path()
            .strip_prefix("/v1/auth/device/")
            .context("invalid authorization URL")?
            .to_owned();
        ensure!(
            url.origin().ascii_serialization() == self.origin
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && id == device_id
                && id.len() == 43
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                && (1..=900).contains(&(started.expires_at - storage::now()))
                && (3..=30).contains(&started.interval),
            "invalid device authorization response"
        );
        if storage::read(&self.root)?.login_attempt.as_deref() != Some(&attempt) {
            let _ = self
                .request::<serde_json::Value>(
                    Method::POST,
                    "/v1/auth/device/cancel",
                    None,
                    Some(serde_json::json!({"id":id,"code_verifier":verifier})),
                )
                .await;
            anyhow::bail!("Google login was cancelled");
        }
        Ok(DeviceLogin {
            account: self.clone(),
            id,
            verifier,
            attempt,
            url: started.verification_uri,
            deadline: tokio::time::Instant::now()
                + Duration::from_secs((started.expires_at - storage::now()).max(1) as u64),
            interval: Duration::from_secs(started.interval),
        })
    }
}
impl DeviceLogin {
    pub fn url(&self) -> &str {
        &self.url
    }
    pub async fn cancel(&self) -> Result<()> {
        self.account.cancel_login(&self.attempt).await?;
        // The locator alone cannot authorize or cancel this request.
        let _ = self
            .account
            .request::<serde_json::Value>(
                Method::POST,
                "/v1/auth/device/cancel",
                None,
                Some(serde_json::json!({"id":self.id,"code_verifier":self.verifier})),
            )
            .await;
        Ok(())
    }
    pub async fn finish(&self) -> Result<Status> {
        let result = tokio::time::timeout_at(self.deadline, self.poll()).await;
        if !matches!(&result, Ok(Ok(_))) {
            let _ = self.cancel().await;
        }
        result.context("Google login timed out")?
    }
    async fn poll(&self) -> Result<Status> {
        loop {
            ensure!(
                storage::read(&self.account.root)?.login_attempt.as_deref() == Some(&self.attempt),
                "Google login was cancelled"
            );
            let response = self
                .account
                .request::<serde_json::Value>(
                    Method::POST,
                    "/v1/auth/device/token",
                    None,
                    Some(serde_json::json!({"id":self.id,"code_verifier":self.verifier})),
                )
                .await;
            match response {
                Ok(value) if value["pending"] == true => {}
                Ok(value) => {
                    let tokens =
                        serde_json::from_value(value).context("invalid device login response")?;
                    self.account.complete_login(&self.attempt, tokens).await?;
                    return self.account.status(false).await;
                }
                Err(error)
                    if error.downcast_ref::<ApiError>().is_some_and(|e| {
                        e.status.is_client_error() && e.status != StatusCode::TOO_MANY_REQUESTS
                    }) =>
                {
                    return Err(error)
                }
                Err(_) => {} // Bounded by the authorization deadline; retry lost poll responses.
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

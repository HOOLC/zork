use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde_json::{json, Value};

use super::{
    nonempty, urlencoding, AuthProvider, BillingKind, DeviceCode, DeviceCodePoll, ProfileTemplate,
    ProviderInfo, QuotaSnapshot,
};

const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
const DEVICE_CODE_URL: &str = "https://auth.kimi.com/api/oauth/device_authorization";
const TOKEN_URL: &str = "https://auth.kimi.com/api/oauth/token";
const API_BASE: &str = "https://api.kimi.com/coding";
const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const REFRESH_LEEWAY_MS: i64 = 5 * 60 * 1000;
const REFRESH_MAX_RETRIES: u32 = 3;
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 5;
const DEVICE_CODE_TIMEOUT_SECONDS: i64 = 15 * 60;

pub struct Kimi;

fn default_models() -> Value {
    json!([
        {
            "id": "kimi-for-coding",
            "api": "openai-completions",
            "streaming": true,
            "thinking": ["off"],
            "default_thinking": "off",
            "capabilities": { "input": ["text"] },
            "default": true
        },
        {
            "id": "kimi-k2.5",
            "api": "openai-completions",
            "streaming": true,
            "thinking": ["off"],
            "default_thinking": "off",
            "capabilities": { "input": ["text", "image"] }
        }
    ])
}

#[async_trait]
impl AuthProvider for Kimi {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "kimi-coding",
            label: "Kimi For Coding",
            billing: &[
                BillingKind {
                    id: "subscription",
                    label: "Kimi Code 订阅",
                },
                BillingKind {
                    id: "usage",
                    label: "API 按量",
                },
            ],
        }
    }

    fn template(&self, billing: &str) -> Result<ProfileTemplate> {
        match billing {
            "subscription" => Ok(ProfileTemplate {
                provider: "kimi-coding".into(),
                billing: "subscription".into(),
                base_url: API_BASE.into(),
                headers: json!({}),
                models: default_models(),
            }),
            "usage" => Ok(ProfileTemplate {
                provider: "kimi-coding".into(),
                billing: "usage".into(),
                base_url: API_BASE.into(),
                headers: json!({}),
                models: default_models(),
            }),
            other => anyhow::bail!("kimi-coding does not support billing {other}"),
        }
    }

    fn bearer(&self, auth: &Value) -> Result<String> {
        nonempty(auth.get("access"))
            .or_else(|| nonempty(auth.get("key")))
            .context("Missing Kimi For Coding credential")
    }

    fn account_identity(&self, billing: &str, auth: &Value) -> Option<String> {
        if billing != "subscription" {
            return None;
        }
        let claims = super::jwt_claims(&nonempty(auth.get("access"))?)?;
        nonempty(claims.get("user_id")).or_else(|| nonempty(claims.get("sub")))
    }

    async fn probe(&self, http: &Client, document: &Value) -> Result<QuotaSnapshot> {
        let billing = document
            .get("billing")
            .and_then(Value::as_str)
            .unwrap_or("subscription");
        let previous = document.get("auth").cloned().unwrap_or(json!({}));
        let auth = self.refresh_if_needed(http, previous.clone()).await?;
        let bearer = self.bearer(&auth)?;
        let base = document
            .get("base_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_else(|| API_BASE.to_string());
        probe_models(http, &base, &bearer).await?;
        Ok(quota_snapshot(
            billing,
            if auth != previous { Some(auth) } else { None },
        ))
    }

    async fn refresh_auth(&self, http: &Client, auth: Value) -> Result<Value> {
        refresh_kimi_auth(http, auth).await
    }

    async fn refresh_if_needed(&self, http: &Client, auth: Value) -> Result<Value> {
        if nonempty(auth.get("refresh")).is_none() {
            return Ok(auth);
        }
        let Some(expires) = expires_ms(&auth) else {
            return Ok(auth);
        };
        let now = Utc::now().timestamp_millis();
        if expires > now + REFRESH_LEEWAY_MS {
            return Ok(auth);
        }
        refresh_kimi_auth(http, auth).await
    }

    fn supports_device_code(&self, billing: &str) -> bool {
        billing == "subscription"
    }

    async fn start_device_code(&self, http: &Client) -> Result<DeviceCode> {
        let body = format!("client_id={}", urlencoding(CLIENT_ID));
        let response = http
            .post(DEVICE_CODE_URL)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(body)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .context("kimi device code")?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Kimi Code device authorization failed with status {status}{}",
                if body.is_empty() {
                    String::new()
                } else {
                    format!(": {body}")
                }
            );
        }
        let payload: Value = response.json().await.context("device code json")?;
        let device_code = payload
            .get("device_code")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .context("missing device_code")?;
        let user_code = payload
            .get("user_code")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .context("missing user_code")?;
        let verification_url = trusted_http_url(payload.get("verification_uri_complete"))
            .or_else(|| trusted_http_url(payload.get("verification_uri")))
            .context("Invalid Kimi Code device authorization response")?;
        let interval = payload
            .get("interval")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS);
        let expires_in = payload
            .get("expires_in")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .unwrap_or(DEVICE_CODE_TIMEOUT_SECONDS);
        let expires_at = Utc::now() + chrono::Duration::seconds(expires_in);
        Ok(DeviceCode {
            provider: "kimi-coding".into(),
            billing: "subscription".into(),
            device_code: device_code.to_string(),
            user_code: user_code.to_string(),
            verification_url,
            interval_seconds: interval,
            expires_at: expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            extra: json!({}),
        })
    }

    async fn poll_device_code(
        &self,
        http: &Client,
        pending: &DeviceCode,
    ) -> Result<DeviceCodePoll> {
        let body = format!(
            "client_id={}&device_code={}&grant_type={}",
            urlencoding(CLIENT_ID),
            urlencoding(&pending.device_code),
            urlencoding(DEVICE_GRANT_TYPE)
        );
        let response = http
            .post(TOKEN_URL)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(body)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .context("kimi device poll")?;
        let status = response.status().as_u16();
        if status >= 500 {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "Kimi Code device token request failed with status {status}{}",
                if body.is_empty() {
                    String::new()
                } else {
                    format!(": {body}")
                }
            );
        }
        let payload: Value = response.json().await.unwrap_or(json!({}));
        if (200..300).contains(&status)
            && payload
                .get("access_token")
                .and_then(Value::as_str)
                .is_some()
        {
            return Ok(DeviceCodePoll::Completed {
                auth: token_auth(&payload, None)?,
            });
        }
        let error = payload.get("error").and_then(Value::as_str).unwrap_or("");
        let description = payload
            .get("error_description")
            .and_then(Value::as_str)
            .unwrap_or("");
        match error {
            "authorization_pending" => Ok(DeviceCodePoll::Pending {
                retry_after_seconds: pending.interval_seconds,
            }),
            "slow_down" => {
                let retry = payload
                    .get("interval")
                    .and_then(Value::as_u64)
                    .filter(|value| *value > 0)
                    .unwrap_or(pending.interval_seconds.max(5) + 5);
                Ok(DeviceCodePoll::Pending {
                    retry_after_seconds: retry,
                })
            }
            "expired_token" => {
                anyhow::bail!("Kimi Code device authorization expired. Please restart login.")
            }
            "access_denied" => anyhow::bail!("Kimi Code login was denied."),
            other => anyhow::bail!(
                "Kimi Code device token request failed (status {status}){}",
                if other.is_empty() {
                    String::new()
                } else if description.is_empty() {
                    format!(": {other}")
                } else {
                    format!(": {other}: {description}")
                }
            ),
        }
    }
}

async fn probe_models(http: &Client, base: &str, bearer: &str) -> Result<()> {
    let response = http
        .get(format!("{base}/models"))
        .header("authorization", format!("Bearer {bearer}"))
        .header("accept", "application/json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("kimi models")?;
    let status = response.status();
    if status.is_success() || matches!(status.as_u16(), 404 | 405) {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    anyhow::bail!("Kimi For Coding probe failed: {body}");
}

fn quota_snapshot(billing: &str, auth: Option<Value>) -> QuotaSnapshot {
    let usage = billing == "usage";
    QuotaSnapshot {
        account: json!({
            "ok": true,
            "account": {
                "type": "kimi-coding",
                "planType": if usage { "API" } else { "Kimi Code" }
            },
            "requiresOpenaiAuth": false
        }),
        rate_limits: json!({
            "ok": true,
            "rateLimits": {
                "limitId": if usage { "kimi_coding_usage" } else { "kimi_coding_subscription" },
                "limitName": "Kimi For Coding",
                "primary": null,
                "secondary": null,
                "credits": if usage {
                    json!({ "unlimited": true, "balance": null })
                } else {
                    Value::Null
                },
                "planType": if usage { "api" } else { "subscription" }
            },
            "rateLimitsByLimitId": {}
        }),
        auth,
    }
}

async fn refresh_kimi_auth(http: &Client, auth: Value) -> Result<Value> {
    let refresh = nonempty(auth.get("refresh")).context("Missing Kimi refresh token")?;
    let body = format!(
        "client_id={}&grant_type=refresh_token&refresh_token={}",
        urlencoding(CLIENT_ID),
        urlencoding(&refresh)
    );
    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 0..=REFRESH_MAX_RETRIES {
        let response = match http
            .post(TOKEN_URL)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(body.clone())
            .timeout(Duration::from_secs(20))
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(error.into());
                continue;
            }
        };
        let status = response.status().as_u16();
        let payload: Value = response.json().await.unwrap_or(json!({}));
        if (200..300).contains(&status) {
            return token_auth(&payload, Some(&auth));
        }
        let error = payload.get("error").and_then(Value::as_str).unwrap_or("");
        if status == 401 || status == 403 || error == "invalid_grant" {
            let description = payload
                .get("error_description")
                .and_then(Value::as_str)
                .unwrap_or("");
            anyhow::bail!(
                "Kimi Code token refresh unauthorized (status {status}){}",
                if description.is_empty() {
                    String::new()
                } else {
                    format!(": {description}")
                }
            );
        }
        if (status == 429 || status >= 500) && attempt < REFRESH_MAX_RETRIES {
            last_error = Some(anyhow::anyhow!(
                "Kimi Code token refresh failed with status {status}"
            ));
            continue;
        }
        anyhow::bail!("Kimi Code token refresh failed with status {status}: {payload}");
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Kimi Code token refresh failed")))
}

fn token_auth(payload: &Value, previous: Option<&Value>) -> Result<Value> {
    let access = payload
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .context("Kimi Code token response missing access_token")?;
    let refresh = payload
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| previous.and_then(|auth| nonempty(auth.get("refresh"))))
        .context("Kimi Code token response missing refresh_token")?;
    let expires_in = payload
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .context("Kimi Code token response missing expires_in")?;
    let expires = Utc::now().timestamp_millis() + expires_in * 1000;
    let mut auth = previous
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some(map) = auth.as_object_mut() {
        map.insert("type".into(), json!("oauth"));
        map.insert("access".into(), json!(access));
        map.insert("refresh".into(), json!(refresh));
        map.insert("expires".into(), json!(expires));
    }
    Ok(auth)
}

fn trusted_http_url(value: Option<&Value>) -> Option<String> {
    let text = nonempty(value)?;
    if text.starts_with("https://") || text.starts_with("http://") {
        Some(text)
    } else {
        None
    }
}

fn expires_ms(auth: &Value) -> Option<i64> {
    auth.get("expires").and_then(Value::as_i64).map(|expires| {
        if expires > 1_000_000_000_000 {
            expires
        } else {
            expires * 1000
        }
    })
}

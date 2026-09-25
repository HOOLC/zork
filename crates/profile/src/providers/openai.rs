use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde_json::{json, Value};

use super::{
    has_credential, nonempty, urlencoding, AuthProvider, BillingKind, DeviceCode, DeviceCodePoll,
    ProfileTemplate, ProviderInfo, QuotaSnapshot,
};

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const DEVICE_VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const API_BASE: &str = "https://api.openai.com/v1";
const SUBSCRIPTION_BASE: &str = "https://chatgpt.com/backend-api/codex";
const REFRESH_LEEWAY_MS: i64 = 5 * 60 * 1000;

pub struct OpenAi;

#[async_trait]
impl AuthProvider for OpenAi {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "openai",
            label: "OpenAI",
            billing: &[
                BillingKind {
                    id: "usage",
                    label: "API 按量",
                },
                BillingKind {
                    id: "subscription",
                    label: "ChatGPT 订阅 (Codex)",
                },
            ],
        }
    }

    fn template(&self, billing: &str) -> Result<ProfileTemplate> {
        match billing {
            "usage" => Ok(ProfileTemplate {
                provider: "openai".into(),
                billing: "usage".into(),
                base_url: API_BASE.into(),
                headers: json!({}),
                models: json!([{
                    "id": "gpt-4.1",
                    "api": "openai-completions",
                    "streaming": true,
                    "thinking": ["off"],
                    "default_thinking": "off",
                    "capabilities": { "input": ["text", "image"] },
                    "limits": {
                        "context_window_tokens": 1_047_576,
                        "max_output_tokens": 32_768
                    },
                    "default": true
                }]),
            }),
            "subscription" => Ok(ProfileTemplate {
                provider: "openai".into(),
                billing: "subscription".into(),
                base_url: SUBSCRIPTION_BASE.into(),
                headers: json!({ "originator": "zork" }),
                models: json!([{
                    "id": "gpt-5.6-luna",
                    "api": "openai-codex-responses",
                    "streaming": true,
                    "thinking": ["low", "medium", "high", "xhigh", "max"],
                    "default_thinking": "medium",
                    "capabilities": { "input": ["text", "image"] },
                    "limits": {
                        "context_window_tokens": 872_000,
                        "max_output_tokens": 128_000
                    },
                    "default": true
                }]),
            }),
            other => anyhow::bail!("openai does not support billing {other}"),
        }
    }

    fn decorate_document(&self, document: &mut Value) {
        let account_id = document
            .pointer("/auth/accountId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if let Some(account_id) = account_id {
            if let Some(headers) = document.as_object_mut().and_then(|object| {
                object
                    .entry("headers".to_string())
                    .or_insert(json!({}))
                    .as_object_mut()
            }) {
                headers.insert("ChatGPT-Account-Id".into(), json!(account_id));
            }
        }
    }

    fn bearer(&self, auth: &Value) -> Result<String> {
        nonempty(auth.get("access"))
            .or_else(|| nonempty(auth.get("key")))
            .context("Missing OpenAI credential")
    }

    /// ChatGPT user within its workspace: a Team workspace id alone is shared.
    fn account_identity(&self, billing: &str, auth: &Value) -> Option<String> {
        if billing != "subscription" {
            return None;
        }
        let claims = super::jwt_claims(&nonempty(auth.get("access"))?)?;
        let scope = claims.get("https://api.openai.com/auth");
        let user = scope
            .and_then(|s| nonempty(s.get("chatgpt_user_id")).or_else(|| nonempty(s.get("user_id"))))
            .or_else(|| nonempty(claims.get("sub")))?;
        let account = nonempty(auth.get("accountId"))
            .or_else(|| scope.and_then(|s| nonempty(s.get("chatgpt_account_id"))))
            .unwrap_or_default();
        Some(format!("{user}/{account}"))
    }

    async fn probe(&self, http: &Client, document: &Value) -> Result<QuotaSnapshot> {
        let billing = document
            .get("billing")
            .and_then(Value::as_str)
            .unwrap_or("usage");
        let auth = document.get("auth").cloned().unwrap_or(json!({}));
        if billing == "subscription" {
            let refreshed = self.refresh_if_needed(http, auth.clone()).await?;
            let snapshot = probe_subscription(http, &refreshed).await?;
            return Ok(super::with_refreshed_auth(snapshot, &auth, refreshed));
        }
        probe_usage(http, &auth).await
    }

    async fn refresh_auth(&self, http: &Client, auth: Value) -> Result<Value> {
        refresh_openai_auth(http, auth).await
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
        refresh_openai_auth(http, auth).await
    }

    fn supports_device_code(&self, billing: &str) -> bool {
        billing == "subscription"
    }

    async fn start_device_code(&self, http: &Client) -> Result<DeviceCode> {
        let response = http
            .post(DEVICE_USER_CODE_URL)
            .header("content-type", "application/json")
            .json(&json!({ "client_id": CLIENT_ID }))
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .context("openai device code")?;
        if response.status().as_u16() == 404 {
            anyhow::bail!(
                "OpenAI Codex device code login is not enabled. Enable it in ChatGPT settings."
            );
        }
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI device code failed: {body}");
        }
        let payload: Value = response.json().await.context("device code json")?;
        let device_auth_id = payload
            .get("device_auth_id")
            .and_then(Value::as_str)
            .context("missing device_auth_id")?;
        let user_code = payload
            .get("user_code")
            .and_then(Value::as_str)
            .or_else(|| payload.get("usercode").and_then(Value::as_str))
            .context("missing user_code")?;
        let interval = match &payload["interval"] {
            Value::Number(number) => number.as_u64().unwrap_or(5),
            Value::String(text) => text.trim().parse().unwrap_or(5),
            _ => 5,
        };
        let expires_at = Utc::now() + chrono::Duration::seconds(15 * 60);
        Ok(DeviceCode {
            provider: "openai".into(),
            billing: "subscription".into(),
            device_code: device_auth_id.to_string(),
            user_code: user_code.to_string(),
            verification_url: DEVICE_VERIFICATION_URI.into(),
            interval_seconds: interval.max(1),
            expires_at: expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            extra: json!({}),
        })
    }

    async fn poll_device_code(
        &self,
        http: &Client,
        pending: &DeviceCode,
    ) -> Result<DeviceCodePoll> {
        let response = http
            .post(DEVICE_TOKEN_URL)
            .header("content-type", "application/json")
            .json(&json!({
                "device_auth_id": pending.device_code,
                "user_code": pending.user_code,
            }))
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .context("openai device poll")?;
        let status = response.status().as_u16();
        if status == 403 || status == 404 {
            return Ok(DeviceCodePoll::Pending {
                retry_after_seconds: pending.interval_seconds,
            });
        }
        if !response.status().is_success() {
            let payload: Value = response.json().await.unwrap_or(json!({}));
            let error = payload.get("error").cloned().unwrap_or(Value::Null);
            let code = error
                .as_str()
                .or_else(|| error.get("code").and_then(Value::as_str));
            return match code {
                Some("deviceauth_authorization_pending") => Ok(DeviceCodePoll::Pending {
                    retry_after_seconds: pending.interval_seconds,
                }),
                Some("slow_down") => Ok(DeviceCodePoll::Pending {
                    retry_after_seconds: pending.interval_seconds.max(5) + 5,
                }),
                other => anyhow::bail!("OpenAI device poll failed: {}", other.unwrap_or("unknown")),
            };
        }
        let payload: Value = response.json().await.context("device poll json")?;
        let authorization_code = payload
            .get("authorization_code")
            .and_then(Value::as_str)
            .context("missing authorization_code")?;
        let code_verifier = payload
            .get("code_verifier")
            .and_then(Value::as_str)
            .context("missing code_verifier")?;
        let token = exchange_authorization_code(http, authorization_code, code_verifier).await?;
        Ok(DeviceCodePoll::Completed { auth: token })
    }
}

async fn exchange_authorization_code(http: &Client, code: &str, verifier: &str) -> Result<Value> {
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        urlencoding(code),
        urlencoding(DEVICE_REDIRECT_URI),
        urlencoding(CLIENT_ID),
        urlencoding(verifier)
    );
    let response = http
        .post(TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("openai token exchange")?;
    read_token_response(response, None).await
}

async fn refresh_openai_auth(http: &Client, auth: Value) -> Result<Value> {
    let refresh = nonempty(auth.get("refresh")).context("Missing OpenAI refresh token")?;
    let body = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        urlencoding(&refresh),
        urlencoding(CLIENT_ID)
    );
    let response = http
        .post(TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("openai token refresh")?;
    read_token_response(response, Some(auth)).await
}

async fn read_token_response(
    response: reqwest::Response,
    previous: Option<Value>,
) -> Result<Value> {
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI token request failed: {body}");
    }
    let payload: Value = response.json().await.context("openai token json")?;
    let access = payload
        .get("access_token")
        .and_then(Value::as_str)
        .context("missing access_token")?;
    let refresh = payload
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            previous
                .as_ref()
                .and_then(|auth| nonempty(auth.get("refresh")))
        })
        .context("missing refresh_token")?;
    let expires_in = payload
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or(3600);
    let expires = Utc::now().timestamp_millis() + expires_in * 1000;
    let account_id = jwt_account_id(access).or_else(|| {
        previous
            .as_ref()
            .and_then(|auth| nonempty(auth.get("accountId")))
    });
    let mut auth = json!({
        "type": "oauth",
        "access": access,
        "refresh": refresh,
        "expires": expires,
    });
    if let Some(account_id) = account_id {
        auth.as_object_mut()
            .map(|object| object.insert("accountId".into(), json!(account_id)));
    }
    Ok(auth)
}

async fn probe_usage(http: &Client, auth: &Value) -> Result<QuotaSnapshot> {
    let key = nonempty(auth.get("key")).context("Missing OpenAI API key")?;
    let response = http
        .get(format!("{API_BASE}/models"))
        .header("authorization", format!("Bearer {key}"))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("openai models")?;
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("OpenAI API key probe failed: {body}");
    }
    Ok(QuotaSnapshot {
        account: json!({
            "ok": true,
            "account": { "type": "openai", "planType": "API" },
            "requiresOpenaiAuth": false
        }),
        rate_limits: json!({
            "ok": true,
            "rateLimits": {
                "limitId": "openai_usage",
                "limitName": "OpenAI API",
                "primary": null,
                "secondary": null,
                "credits": { "unlimited": true, "balance": null },
                "planType": "api"
            },
            "rateLimitsByLimitId": {}
        }),
        auth: None,
    })
}

async fn probe_subscription(http: &Client, auth: &Value) -> Result<QuotaSnapshot> {
    if !has_credential(auth) && nonempty(auth.get("access")).is_none() {
        anyhow::bail!("Missing OpenAI subscription credentials");
    }
    let bearer = nonempty(auth.get("access")).context("Missing OpenAI access token")?;
    let mut request = http
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(bearer)
        .header("originator", "zork")
        .timeout(Duration::from_secs(20));
    if let Some(account_id) = nonempty(auth.get("accountId")) {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    // Keep refreshed credentials even if the independent usage service fails.
    let result = async {
        let response = request.send().await.context("OpenAI usage request")?;
        anyhow::ensure!(
            response.status().is_success(),
            "OpenAI usage HTTP {}",
            response.status()
        );
        let payload: Value = response.json().await.context("OpenAI usage response")?;
        anyhow::ensure!(
            payload.is_object()
                && (payload.get("rate_limit").is_some() || payload.get("plan_type").is_some()),
            "Invalid OpenAI usage response"
        );
        Ok::<_, anyhow::Error>(payload)
    }
    .await;
    let (account, rate_limits) = match result {
        Ok(payload) => (
            json!({"ok":true,"account":{"type":"openai","planType":payload["plan_type"]},"requiresOpenaiAuth":true}),
            subscription_limits(&payload),
        ),
        Err(error) => {
            tracing::warn!(%error, "OpenAI usage query failed");
            (
                json!({"ok":false,"error":"usage_query_failed"}),
                json!({"ok":false,"error":"usage_query_failed"}),
            )
        }
    };
    Ok(QuotaSnapshot {
        account,
        rate_limits,
        auth: None,
    })
}

fn subscription_limits(payload: &Value) -> Value {
    fn window(value: &Value) -> Value {
        let Some(used) = value["used_percent"].as_f64() else {
            return Value::Null;
        };
        json!({"usedPercent": used, "windowDurationMins": value["limit_window_seconds"].as_u64().filter(|v| *v > 0).map(|v| v.div_ceil(60)), "resetsAt":value["reset_at"]})
    }
    let limits = |id: &str, name: &str, rate: &Value| json!({"limitId":id,"limitName":name,"primary":window(&rate["primary_window"]),"secondary":window(&rate["secondary_window"])});
    let mut main = limits("openai_subscription", "ChatGPT", &payload["rate_limit"]);
    if payload["credits"].is_object() {
        main["credits"] = json!({"balance":payload["credits"]["balance"],"unlimited":payload["credits"]["unlimited"],"unit":"credits"});
    }
    let mut additional = serde_json::Map::new();
    for item in payload["additional_rate_limits"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(id) = item["metered_feature"].as_str() {
            additional.insert(
                id.into(),
                limits(
                    id,
                    item["limit_name"].as_str().unwrap_or(id),
                    &item["rate_limit"],
                ),
            );
        }
    }
    json!({"ok":true,"rateLimits":main,"rateLimitsByLimitId":additional})
}

fn jwt_account_id(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let mut normalized = payload.replace('-', "+").replace('_', "/");
    while normalized.len() % 4 != 0 {
        normalized.push('=');
    }
    let bytes = decode_base64(&normalized)?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("https://api.openai.com/auth")
                .and_then(|auth| auth.get("chatgpt_account_id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

pub(super) fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn val(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < bytes.len() {
        let a = val(bytes[i])?;
        let b = val(bytes[i + 1])?;
        let c = if bytes[i + 2] == b'=' {
            0
        } else {
            val(bytes[i + 2])?
        };
        let d = if bytes[i + 3] == b'=' {
            0
        } else {
            val(bytes[i + 3])?
        };
        out.push((a << 2) | (b >> 4));
        if bytes[i + 2] != b'=' {
            out.push(((b & 0x0f) << 4) | (c >> 2));
        }
        if bytes[i + 3] != b'=' {
            out.push(((c & 0x03) << 6) | d);
        }
        i += 4;
    }
    Some(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_template_declares_current_luna_responses_selection() {
        let template = OpenAi.template("subscription").unwrap();
        let model = &template.models[0];

        assert_eq!(template.base_url, "https://chatgpt.com/backend-api/codex");
        assert_eq!(model["id"], "gpt-5.6-luna");
        assert_eq!(model["api"], "openai-codex-responses");
        assert_eq!(model["default_thinking"], "medium");
        assert!(model["thinking"]
            .as_array()
            .unwrap()
            .contains(&json!("max")));
        assert_eq!(model["limits"]["context_window_tokens"], 872_000);
        assert_eq!(model["limits"]["max_output_tokens"], 128_000);
    }
}

#[cfg(test)]
mod quota_tests {
    use super::*;
    #[test]
    fn normalizes_usage_and_additional_windows_without_inventing_limits() {
        let value = subscription_limits(
            &json!({"rate_limit":{"primary_window":{"used_percent":28,"limit_window_seconds":18000,"reset_at":1800000000},"secondary_window":null},"credits":{"balance":"12.5","unlimited":false},"additional_rate_limits":[{"metered_feature":"reviews","limit_name":"Code review","rate_limit":{"primary_window":{"used_percent":100,"limit_window_seconds":604800,"reset_at":1800000300}}}]}),
        );
        assert_eq!(
            value["rateLimits"]["primary"],
            json!({"usedPercent":28.,"windowDurationMins":300,"resetsAt":1800000000})
        );
        assert!(value["rateLimits"]["secondary"].is_null());
        assert_eq!(value["rateLimits"]["credits"]["unit"], "credits");
        assert_eq!(
            value["rateLimitsByLimitId"]["reviews"]["primary"]["usedPercent"],
            100.
        );
        assert!(
            subscription_limits(&json!({"rate_limit":null}))["rateLimits"]["primary"].is_null()
        );
    }
}

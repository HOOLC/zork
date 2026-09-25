use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ulid::Ulid;

use super::{
    nonempty, urlencoding, AuthProvider, BillingKind, DeviceCode, DeviceCodePoll, ProfileTemplate,
    ProviderInfo, QuotaSnapshot,
};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const REDIRECT_URI: &str = "http://localhost:53692/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const API_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const OAUTH_BETA: &str = "claude-code-20250219,oauth-2025-04-20";
const REFRESH_LEEWAY_MS: i64 = 5 * 60 * 1000;

pub struct Anthropic;

fn default_models() -> Value {
    json!([{
        "id": "claude-sonnet-4-6",
        "api": "anthropic-messages",
        "streaming": true,
        "thinking": ["off", "low", "medium", "high"],
        "default_thinking": "high",
        "capabilities": { "input": ["text", "image"] },
        "limits": {
            "context_window_tokens": 1_000_000,
            "max_output_tokens": 128_000
        },
        "default": true
    }])
}

fn oauth_headers() -> Value {
    json!({
        "anthropic-beta": OAUTH_BETA,
        "user-agent": "claude-cli/2.1.75",
        "x-app": "cli",
    })
}

#[async_trait]
impl AuthProvider for Anthropic {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "anthropic",
            label: "Anthropic / Claude",
            billing: &[
                BillingKind {
                    id: "usage",
                    label: "API 按量",
                },
                BillingKind {
                    id: "subscription",
                    label: "Claude 订阅 (Pro/Max)",
                },
            ],
        }
    }

    fn template(&self, billing: &str) -> Result<ProfileTemplate> {
        match billing {
            "usage" => Ok(ProfileTemplate {
                provider: "anthropic".into(),
                billing: "usage".into(),
                base_url: API_BASE.into(),
                headers: json!({}),
                models: default_models(),
            }),
            "subscription" => Ok(ProfileTemplate {
                provider: "anthropic".into(),
                billing: "subscription".into(),
                base_url: API_BASE.into(),
                headers: oauth_headers(),
                models: default_models(),
            }),
            other => anyhow::bail!("anthropic does not support billing {other}"),
        }
    }

    fn bearer(&self, auth: &Value) -> Result<String> {
        nonempty(auth.get("access"))
            .or_else(|| nonempty(auth.get("key")))
            .context("Missing Anthropic credential")
    }

    fn account_identity(&self, billing: &str, auth: &Value) -> Option<String> {
        if billing != "subscription" {
            return None;
        }
        nonempty(auth.get("accountUuid"))
    }

    async fn probe(&self, http: &Client, document: &Value) -> Result<QuotaSnapshot> {
        let billing = document
            .get("billing")
            .and_then(Value::as_str)
            .unwrap_or("usage");
        let auth = document.get("auth").cloned().unwrap_or(json!({}));
        if billing == "subscription" {
            let refreshed = self.refresh_if_needed(http, auth.clone()).await?;
            let refreshed = with_account_uuid(http, refreshed).await;
            let snapshot = probe_subscription(http, &refreshed).await?;
            return Ok(super::with_refreshed_auth(snapshot, &auth, refreshed));
        }
        probe_usage(http, &auth).await
    }

    async fn refresh_auth(&self, http: &Client, auth: Value) -> Result<Value> {
        refresh_anthropic_auth(http, auth).await
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
        refresh_anthropic_auth(http, auth).await
    }

    fn supports_device_code(&self, billing: &str) -> bool {
        billing == "subscription"
    }

    async fn start_device_code(&self, _http: &Client) -> Result<DeviceCode> {
        let (verifier, challenge) = generate_pkce();
        let state = verifier.clone();
        let device_code = Ulid::new().to_string();
        let verification_url = format!(
            "{AUTHORIZE_URL}?code=true&client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
            urlencoding(CLIENT_ID),
            urlencoding(REDIRECT_URI),
            urlencoding(SCOPES),
            urlencoding(&challenge),
            urlencoding(&state),
        );
        let expires_at = Utc::now() + chrono::Duration::seconds(15 * 60);
        Ok(DeviceCode {
            provider: "anthropic".into(),
            billing: "subscription".into(),
            device_code,
            user_code: String::new(),
            verification_url,
            interval_seconds: 5,
            expires_at: expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            extra: json!({
                "code_verifier": verifier,
                "state": state,
            }),
        })
    }

    async fn poll_device_code(
        &self,
        http: &Client,
        pending: &DeviceCode,
    ) -> Result<DeviceCodePoll> {
        if pending.user_code.trim().is_empty() {
            return Ok(DeviceCodePoll::Pending {
                retry_after_seconds: pending.interval_seconds.max(1),
            });
        }
        let parsed = parse_authorization_input(&pending.user_code);
        let code = parsed
            .code
            .filter(|value| !value.is_empty())
            .context("Missing Anthropic authorization code")?;
        let extra_verifier = nonempty(pending.extra.get("code_verifier"));
        let extra_state = nonempty(pending.extra.get("state"));
        let verifier = extra_verifier
            .or_else(|| extra_state.clone())
            .or_else(|| parsed.state.clone())
            .context("Missing PKCE code_verifier")?;
        if parsed
            .state
            .as_ref()
            .is_some_and(|parsed_state| parsed_state != &verifier)
        {
            anyhow::bail!("OAuth state mismatch");
        }
        let state = parsed
            .state
            .or(extra_state)
            .unwrap_or_else(|| verifier.clone());
        let auth = exchange_authorization_code(http, &code, &state, &verifier).await?;
        Ok(DeviceCodePoll::Completed { auth })
    }
}

struct ParsedAuthInput {
    code: Option<String>,
    state: Option<String>,
}

fn parse_authorization_input(input: &str) -> ParsedAuthInput {
    let value = input.trim();
    if value.is_empty() {
        return ParsedAuthInput {
            code: None,
            state: None,
        };
    }
    if let Some((_, query)) = value.split_once('?') {
        return parse_query(query.split('#').next().unwrap_or(query));
    }
    if value.contains('#') {
        let (code, state) = value.split_once('#').unwrap_or((value, ""));
        return ParsedAuthInput {
            code: nonempty_str(code),
            state: nonempty_str(state),
        };
    }
    if value.contains("code=") {
        return parse_query(value);
    }
    ParsedAuthInput {
        code: Some(value.to_string()),
        state: None,
    }
}

fn parse_query(query: &str) -> ParsedAuthInput {
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        let Some((key, raw)) = pair.split_once('=') else {
            continue;
        };
        let decoded = percent_decode(raw);
        match key {
            "code" => code = nonempty_str(&decoded),
            "state" => state = nonempty_str(&decoded),
            _ => {}
        }
    }
    ParsedAuthInput { code, state }
}

fn nonempty_str(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((high << 4) | low);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn generate_pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes);
    let verifier = base64url_nopad(&bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64url_nopad(&digest);
    (verifier, challenge)
}

fn fill_random(buf: &mut [u8]) {
    getrandom::fill(buf).expect("OS randomness unavailable");
}

fn base64url_nopad(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().saturating_mul(4).div_ceil(3));
    let mut chunks = input.chunks_exact(3);
    for chunk in chunks.by_ref() {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
    }
    let rem = chunks.remainder();
    if rem.len() == 1 {
        let n = u32::from(rem[0]) << 16;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
    } else if rem.len() == 2 {
        let n = (u32::from(rem[0]) << 16) | (u32::from(rem[1]) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
    }
    out
}

async fn exchange_authorization_code(
    http: &Client,
    code: &str,
    state: &str,
    verifier: &str,
) -> Result<Value> {
    let response = http
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": state,
            "redirect_uri": REDIRECT_URI,
            "code_verifier": verifier,
        }))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("anthropic token exchange")?;
    read_token_response(response, None).await
}

async fn refresh_anthropic_auth(http: &Client, auth: Value) -> Result<Value> {
    let refresh = nonempty(auth.get("refresh")).context("Missing Anthropic refresh token")?;
    let response = http
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&json!({
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "refresh_token": refresh,
        }))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("anthropic token refresh")?;
    read_token_response(response, Some(auth)).await
}

async fn read_token_response(
    response: reqwest::Response,
    previous: Option<Value>,
) -> Result<Value> {
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Anthropic token request failed: {body}");
    }
    let payload: Value = response.json().await.context("anthropic token json")?;
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
    let mut next = previous.unwrap_or_else(|| json!({}));
    if let Some(map) = next.as_object_mut() {
        map.insert("type".into(), json!("oauth"));
        map.insert("access".into(), json!(access));
        map.insert("refresh".into(), json!(refresh));
        map.insert("expires".into(), json!(expires));
        if let Some(uuid) = nonempty(payload.pointer("/account/uuid")) {
            map.insert("accountUuid".into(), json!(uuid));
        }
    }
    Ok(next)
}

/// Logins saved before the account id was kept learn it once from the profile.
async fn with_account_uuid(http: &Client, mut auth: Value) -> Value {
    if nonempty(auth.get("accountUuid")).is_some() {
        return auth;
    }
    let Some(bearer) = nonempty(auth.get("access")) else {
        return auth;
    };
    let lookup = async {
        let response = http
            .get(format!("{API_BASE}/api/oauth/profile"))
            .bearer_auth(&bearer)
            .header("anthropic-beta", "oauth-2025-04-20")
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .context("Claude profile request")?;
        anyhow::ensure!(
            response.status().is_success(),
            "Claude profile HTTP {}",
            response.status()
        );
        let payload: Value = response.json().await.context("Claude profile response")?;
        nonempty(payload.pointer("/account/uuid")).context("Claude account uuid missing")
    };
    match lookup.await {
        Ok(uuid) => {
            if let Some(map) = auth.as_object_mut() {
                map.insert("accountUuid".into(), json!(uuid));
            }
        }
        Err(error) => tracing::warn!(%error, "Claude account lookup failed"),
    }
    auth
}

async fn probe_usage(http: &Client, auth: &Value) -> Result<QuotaSnapshot> {
    let key = nonempty(auth.get("key")).context("Missing Anthropic API key")?;
    let response = http
        .get(format!("{API_BASE}/v1/models"))
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .context("anthropic models")?;
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Anthropic API key probe failed: {body}");
    }
    Ok(QuotaSnapshot {
        account: json!({
            "ok": true,
            "account": { "type": "anthropic", "planType": "API" },
            "requiresOpenaiAuth": false
        }),
        rate_limits: json!({
            "ok": true,
            "rateLimits": {
                "limitId": "anthropic_usage",
                "limitName": "Anthropic API",
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
    let bearer = nonempty(auth.get("access")).context("Missing Anthropic access token")?;
    let usage = async {
        let response = http
            .get(format!("{API_BASE}/api/oauth/usage"))
            .bearer_auth(&bearer)
            .header("anthropic-beta", "oauth-2025-04-20")
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .context("Claude usage request")?;
        anyhow::ensure!(
            response.status().is_success(),
            "Claude usage HTTP {}",
            response.status()
        );
        let payload: Value = response.json().await.context("Claude usage response")?;
        anyhow::ensure!(
            payload.is_object()
                && (payload.get("five_hour").is_some() || payload.get("seven_day").is_some()),
            "Invalid Claude usage response"
        );
        Ok::<_, anyhow::Error>(subscription_limits(&payload))
    }
    .await;
    Ok(QuotaSnapshot {
        account: json!({
            "ok": usage.is_ok(),
            "account": {
                "email": auth.get("email"),
                "type": "anthropic",
                "planType": "Claude"
            },
            "requiresOpenaiAuth": false
        }),
        rate_limits: usage.unwrap_or_else(|error| {
            tracing::warn!(%error, "Claude usage query failed");
            json!({"ok":false,"error":"usage_query_failed"})
        }),
        auth: None,
    })
}

fn subscription_limits(payload: &Value) -> Value {
    fn window(value: &Value, minutes: u64) -> Value {
        let Some(used) = value["utilization"].as_f64() else {
            return Value::Null;
        };
        let reset = value["resets_at"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|v| v.timestamp());
        json!({"usedPercent":used,"windowDurationMins":minutes,"resetsAt":reset})
    }
    let mut additional = serde_json::Map::new();
    for (key, value) in payload.as_object().into_iter().flatten() {
        if key.starts_with("seven_day_") && value.is_object() {
            additional.insert(key.clone(),json!({"limitId":key,"limitName":key.trim_start_matches("seven_day_"),"secondary":window(value,10080)}));
        }
    }
    json!({"ok":true,"rateLimits":{"limitId":"anthropic_subscription","limitName":"Claude","primary":window(&payload["five_hour"],300),"secondary":window(&payload["seven_day"],10080)},"rateLimitsByLimitId":additional})
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
mod quota_tests {
    use super::*;
    #[test]
    fn normalizes_claude_usage_timestamps_and_model_scoped_limits() {
        let value = subscription_limits(
            &json!({"five_hour":{"utilization":0,"resets_at":"2026-09-08T08:00:00+08:00"},"seven_day":{"utilization":62.5,"resets_at":null},"seven_day_sonnet":{"utilization":100,"resets_at":"invalid"},"extra_usage":{"is_enabled":false}}),
        );
        assert_eq!(value["rateLimits"]["primary"]["usedPercent"], 0.);
        assert_eq!(value["rateLimits"]["primary"]["resetsAt"], 1788825600i64);
        assert_eq!(
            value["rateLimits"]["secondary"]["windowDurationMins"],
            10080
        );
        assert!(
            value["rateLimitsByLimitId"]["seven_day_sonnet"]["secondary"]["resetsAt"].is_null()
        );
        assert!(
            subscription_limits(&json!({"five_hour":null,"seven_day":null}))["rateLimits"]
                ["primary"]
                .is_null()
        );
    }
}

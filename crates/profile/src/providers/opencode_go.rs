use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use super::{
    nonempty, AuthProvider, BillingKind, DeviceCode, DeviceCodePoll, ProfileTemplate, ProviderInfo,
    QuotaSnapshot,
};

const API_BASE: &str = "https://opencode.ai/zen/go/v1";

pub struct OpenCodeGo;

#[async_trait]
impl AuthProvider for OpenCodeGo {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "opencode-go",
            label: "OpenCode Go",
            billing: &[BillingKind {
                id: "subscription",
                label: "OpenCode Go 订阅",
            }],
        }
    }

    fn template(&self, billing: &str) -> Result<ProfileTemplate> {
        match billing {
            "subscription" => Ok(ProfileTemplate {
                provider: "opencode-go".into(),
                billing: "subscription".into(),
                base_url: API_BASE.into(),
                headers: json!({}),
                models: json!([{
                    "id": "muse-spark-1.2-contributor",
                    "api": "openai-responses",
                    "streaming": true,
                    "thinking": ["off", "minimal", "low", "medium", "high", "xhigh"],
                    "default_thinking": "xhigh",
                    "capabilities": { "input": ["text", "image"] },
                    "limits": {
                        "context_window_tokens": 1_048_576,
                        "max_output_tokens": 131_072
                    },
                    "default": true
                }]),
            }),
            other => anyhow::bail!("opencode-go does not support billing {other}"),
        }
    }

    fn bearer(&self, auth: &Value) -> Result<String> {
        nonempty(auth.get("key"))
            .or_else(|| nonempty(auth.get("access")))
            .context("Missing OpenCode Go credential")
    }

    async fn probe(&self, http: &Client, document: &Value) -> Result<QuotaSnapshot> {
        let auth = document.get("auth").cloned().unwrap_or_else(|| json!({}));
        let bearer = self.bearer(&auth)?;
        let base_url = document
            .get("base_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(API_BASE)
            .trim_end_matches('/');
        let response = http
            .get(format!("{base_url}/models"))
            .bearer_auth(&bearer)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .context("OpenCode Go models")?;
        if !response.status().is_success() {
            anyhow::bail!(
                "OpenCode Go model probe failed with status {}",
                response.status().as_u16()
            );
        }
        let usage = async {
            let response = http
                .get(format!("{base_url}/usage"))
                .bearer_auth(&bearer)
                .timeout(Duration::from_secs(20))
                .send()
                .await
                .context("OpenCode Go usage request")?;
            anyhow::ensure!(
                response.status().is_success(),
                "OpenCode Go usage HTTP {}",
                response.status()
            );
            let payload: Value = response.json().await.context("OpenCode Go usage response")?;
            subscription_limits(&payload).context("OpenCode Go usage has no windows")
        }
        .await;
        Ok(QuotaSnapshot {
            account: json!({
                "ok": true,
                "account": { "type": "opencode-go", "planType": "Go" }
            }),
            rate_limits: usage.unwrap_or_else(|error| {
                tracing::warn!(%error, "OpenCode Go usage query failed");
                json!({"ok":false,"error":"usage_query_failed"})
            }),
            auth: None,
        })
    }

    async fn refresh_auth(&self, _http: &Client, auth: Value) -> Result<Value> {
        Ok(auth)
    }

    async fn refresh_if_needed(&self, _http: &Client, auth: Value) -> Result<Value> {
        Ok(auth)
    }

    fn supports_device_code(&self, _billing: &str) -> bool {
        false
    }

    async fn start_device_code(&self, _http: &Client) -> Result<DeviceCode> {
        anyhow::bail!("OpenCode Go uses an API key from the OpenCode subscription page")
    }

    async fn poll_device_code(
        &self,
        _http: &Client,
        _pending: &DeviceCode,
    ) -> Result<DeviceCodePoll> {
        anyhow::bail!("OpenCode Go does not use device-code login")
    }
}

/// Go meters a rolling 5-hour, a weekly and a monthly window, each as a
/// percentage used with its reset time. Rolling and weekly map to the
/// primary/secondary windows; monthly is an additional unnamed limit, so
/// clients label it by its duration like the others.
fn subscription_limits(payload: &Value) -> Option<Value> {
    fn window(value: &Value, minutes: u64) -> Value {
        let Some(used) = value["percent"].as_f64() else {
            return Value::Null;
        };
        let reset = value["resetsAt"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|v| v.timestamp());
        json!({"usedPercent":used,"windowDurationMins":minutes,"resetsAt":reset})
    }
    let usage = &payload["usage"];
    let primary = window(&usage["rolling"], 300);
    let secondary = window(&usage["weekly"], 10080);
    let monthly = window(&usage["monthly"], 43200);
    if primary.is_null() && secondary.is_null() && monthly.is_null() {
        return None;
    }
    let mut additional = serde_json::Map::new();
    if !monthly.is_null() {
        additional.insert(
            "opencode_go_monthly".into(),
            json!({"limitId":"opencode_go_monthly","limitName":"","secondary":monthly}),
        );
    }
    Some(json!({
        "ok": true,
        "rateLimits": {
            "limitId": "opencode_go_subscription",
            "limitName": "OpenCode Go",
            "primary": primary,
            "secondary": secondary,
            "planType": "subscription"
        },
        "rateLimitsByLimitId": additional
    }))
}

#[cfg(test)]
mod quota_tests {
    use super::*;

    #[test]
    fn maps_rolling_weekly_and_monthly_windows() {
        let value = subscription_limits(&json!({"usage":{
            "rolling":{"status":"ok","percent":0,"resetsAt":"2026-09-25T23:22:52.780Z"},
            "weekly":{"status":"ok","percent":67,"resetsAt":"2026-09-28T00:00:00.000Z"},
            "monthly":{"status":"ok","percent":84,"resetsAt":"2026-10-02T21:13:01.000Z"}
        }}))
        .unwrap();
        assert_eq!(value["rateLimits"]["primary"]["usedPercent"], 0.);
        assert_eq!(value["rateLimits"]["primary"]["windowDurationMins"], 300);
        assert_eq!(value["rateLimits"]["secondary"]["usedPercent"], 67.);
        assert_eq!(value["rateLimits"]["secondary"]["resetsAt"], 1790553600i64);
        assert_eq!(
            value["rateLimitsByLimitId"]["opencode_go_monthly"]["secondary"]["usedPercent"],
            84.
        );
    }

    #[test]
    fn missing_windows_are_not_a_quota() {
        assert!(subscription_limits(&json!({"usage":{}})).is_none());
        assert!(subscription_limits(&json!({"error":"nope"})).is_none());
    }
}

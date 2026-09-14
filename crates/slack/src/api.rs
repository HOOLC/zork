use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::Value;
use tracing::warn;

/// In-process Slack Web API client. All calls carry the bot token; form
/// encoding matches the Slack Web API shape.
#[derive(Clone)]
pub struct SlackApi {
    pub(super) http: Client,
    pub(super) bot_token: String,
    pub(super) api_base_url: String,
}

impl SlackApi {
    pub fn new(
        bot_token: impl Into<String>,
        api_base_url: impl Into<String>,
        http: Client,
    ) -> Self {
        Self {
            http,
            bot_token: bot_token.into(),
            api_base_url: api_base_url.into(),
        }
    }

    pub fn bot_token(&self) -> &str {
        &self.bot_token
    }

    pub async fn call(&self, method: &str, fields: &[(&str, &str)]) -> Result<Value> {
        let body = fields
            .iter()
            .map(|(key, value)| format!("{}={}", urlencode(key), urlencode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let response = self
            .http
            .post(format!("{}/{method}", self.api_base_url))
            .header("authorization", format!("Bearer {}", self.bot_token))
            .header(
                "content-type",
                "application/x-www-form-urlencoded; charset=utf-8",
            )
            .body(body)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return Err(error).with_context(|| format!("slack {method}"));
            }
        };
        if response.status().as_u16() == 429 {
            warn!(method, "slack rate limited");
            anyhow::bail!("ratelimited");
        }
        let payload: Value = response
            .json()
            .await
            .with_context(|| format!("slack {method} json"))?;
        if payload.get("ok") != Some(&Value::Bool(true)) {
            let error = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown_error");
            anyhow::bail!("Slack API error for {method}: {error}");
        }
        Ok(payload)
    }

    pub async fn post_binary(&self, url: &str, bytes: Vec<u8>) -> Result<()> {
        let response = self
            .http
            .post(url)
            .body(bytes)
            .send()
            .await
            .context("slack file upload")?;
        if !response.status().is_success() {
            anyhow::bail!("slack file upload failed: {}", response.status());
        }
        Ok(())
    }
}

fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

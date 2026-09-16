//! Slack Web API transport, without a method inventory or permission discovery.
use crate::SlackApi;
use anyhow::{Context, Result};
use serde_json::Value;

const MAX_BYTES: usize = 1024 * 1024;
pub struct ForwardResponse {
    pub status: u16,
    pub retry_after: Option<String>,
    pub body: Value,
}

/// Accept RPC names only. A tool argument must never become an arbitrary URL.
pub fn valid_method(method: &str) -> bool {
    method.len() <= 160
        && method.contains('.')
        && method.split('.').all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_alphabetic()
                && part.bytes().all(|c| c.is_ascii_alphanumeric())
        })
}

impl SlackApi {
    pub async fn forward(&self, method: &str, arguments: &Value) -> Result<ForwardResponse> {
        anyhow::ensure!(valid_method(method), "invalid_slack_method");
        let arguments = arguments
            .as_object()
            .context("slack_arguments_must_be_object")?;
        anyhow::ensure!(
            !arguments.contains_key("token"),
            "token_is_owned_by_connect_id"
        );
        anyhow::ensure!(
            serde_json::to_vec(arguments)?.len() <= MAX_BYTES,
            "slack_request_too_large"
        );
        // Slack accepts POST form parameters for RPC methods. Structured values
        // are encoded as JSON, without selecting, renaming, or dropping fields.
        let fields = arguments
            .iter()
            .map(|(key, value)| {
                (
                    key,
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                )
            })
            .collect::<Vec<_>>();
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut response = client.post(format!("{}/{}", self.api_base_url.trim_end_matches('/'), method))
            .bearer_auth(&self.bot_token).form(&fields).send().await
            .map_err(|_| anyhow::anyhow!("slack_delivery_unknown: transport failed; do not automatically repeat the operation"))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("slack_delivery_unknown: response interrupted"))?
        {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= MAX_BYTES,
                "slack_delivery_unknown: response exceeds 1 MiB; use smaller pages"
            );
            bytes.extend_from_slice(&chunk);
        }
        let body: Value = serde_json::from_slice(&bytes).map_err(|_| {
            anyhow::anyhow!(
                "slack_delivery_unknown: upstream returned a non-JSON response (HTTP {status})"
            )
        })?;
        anyhow::ensure!(
            body.is_object(),
            "slack_delivery_unknown: upstream response is not an object"
        );
        Ok(ForwardResponse {
            status,
            retry_after,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_are_rpc_methods_not_paths_or_legacy_aliases() {
        for name in [
            "chat.postMessage",
            "conversations.replies",
            "future.family.method",
        ] {
            assert!(valid_method(name));
        }
        for name in [
            "history",
            "post_message",
            "../auth.test",
            "https://evil.test",
            "chat.postMessage?token=x",
            "chat..delete",
        ] {
            assert!(!valid_method(name));
        }
    }
}

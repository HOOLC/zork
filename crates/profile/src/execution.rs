use std::collections::HashMap;

use anyhow::Result;
use reqwest::Client;
use serde_json::Value;

use crate::app::ProfileDocument;
use crate::providers::{self, AuthProvider, ProfileTemplate};
use crate::storage::ProfilePaths;

#[derive(Clone, Debug)]
pub struct ProviderExecution {
    pub profile_id: String,
    pub provider: String,
    pub model: String,
    pub api: String,
    pub streaming: bool,
    pub image_input: bool,
    pub parallel_tool_calls: bool,
    pub single_system_message: bool,
    pub service_tier: Option<String>,
    pub thinking: String,
    pub limits: crate::ModelLimits,
    pub base_url: String,
    pub headers: HashMap<String, String>,
    pub bearer: String,
}

pub async fn load_selected(
    paths: &impl ProfilePaths,
    http: &Client,
    profile_id: &str,
    model: &str,
    thinking: &str,
) -> Result<ProviderExecution> {
    let mut document = crate::app::read(paths, profile_id)?;
    crate::app::select_model(&document, model, thinking)?;
    let provider = providers::get(&document.provider)?;
    let refreshed = provider
        .refresh_if_needed(http, document.auth.clone())
        .await?;
    if refreshed != document.auth {
        anyhow::ensure!(
            crate::app::commit_probe_auth(
                paths,
                profile_id,
                &document.auth,
                Some(refreshed.clone())
            )?,
            "profile credentials changed during refresh; retry the request"
        );
        document.auth = refreshed;
    }
    let current = crate::read_profile(paths, profile_id)?;
    anyhow::ensure!(
        current.provider == document.provider && current.billing == document.billing,
        "profile changed during credential refresh; retry the request"
    );
    document = current;
    let (
        api,
        streaming,
        image_input,
        parallel_tool_calls,
        single_system_message,
        service_tier,
        limits,
    ) = {
        let selected_model = crate::app::select_model(&document, model, thinking)?;
        (
            selected_model.api.as_str().to_owned(),
            selected_model.streaming,
            selected_model
                .capabilities
                .input
                .iter()
                .any(|kind| kind == "image"),
            selected_model.parallel_tool_calls,
            selected_model.single_system_message,
            selected_model.service_tier.clone(),
            selected_model
                .limits
                .clone()
                .ok_or_else(|| anyhow::anyhow!("model {model} does not declare token limits"))?,
        )
    };
    let template = provider.template(&document.billing)?;
    let (base_url, mut headers) = prepare_endpoint(provider, &document, &template)?;
    provider.decorate_execution_headers(&document.billing, model, &mut headers);
    Ok(ProviderExecution {
        profile_id: profile_id.to_owned(),
        provider: document.provider,
        model: model.to_owned(),
        api,
        streaming,
        image_input,
        parallel_tool_calls,
        single_system_message,
        service_tier,
        thinking: thinking.to_owned(),
        limits,
        base_url,
        headers,
        bearer: provider.bearer(&document.auth)?,
    })
}

fn prepare_endpoint(
    provider: &dyn AuthProvider,
    document: &ProfileDocument,
    template: &ProfileTemplate,
) -> Result<(String, HashMap<String, String>)> {
    let mut decorated = serde_json::to_value(document)?;
    provider.decorate_document(&mut decorated);
    let base_url = decorated
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or(template.base_url.as_str())
        .trim_end_matches('/')
        .to_owned();
    let mut headers = headers_from_value(&template.headers);
    if let Some(profile_headers) = decorated.get("headers") {
        headers.extend(headers_from_value(profile_headers));
    }
    Ok((base_url, headers))
}

fn headers_from_value(value: &Value) -> HashMap<String, String> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .as_str()
                        .map(|header| (key.clone(), header.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{providers, ProfileDocument};
    use serde_json::json;

    #[test]
    fn openai_subscription_execution_includes_required_and_derived_headers() {
        let document: ProfileDocument = serde_json::from_value(json!({
            "provider": "openai",
            "billing": "subscription",
            "base_url": "https://chatgpt.com/backend-api/codex",
            "headers": { "x-user-header": "present" },
            "auth": {
                "type": "oauth",
                "access": "access-secret",
                "refresh": "refresh-secret",
                "accountId": "account-secret"
            },
            "models": [{
                "id": "gpt-5.6-luna",
                "api": "openai-responses",
                "streaming": true,
                "thinking": ["max"],
                "default_thinking": "max",
                "capabilities": { "input": ["text", "image"] },
                "default": true
            }]
        }))
        .unwrap();
        let provider = providers::get("openai").unwrap();
        let template = provider.template("subscription").unwrap();

        let (_, headers) = prepare_endpoint(provider, &document, &template).unwrap();

        assert_eq!(headers["originator"], "zork");
        assert_eq!(headers["x-user-header"], "present");
        assert_eq!(headers["ChatGPT-Account-Id"], "account-secret");
    }
}

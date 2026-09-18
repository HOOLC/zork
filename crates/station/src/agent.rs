use crate::db::{GatewayDb, ProactiveBindingRow, SessionBindingRow, SessionRow};
use anyhow::{Context, Result};
use axum::http::StatusCode;
#[cfg(test)]
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;
use zork_agent::Agent;
pub use zork_agent_api::{AgentProfile, SessionSelection};
use zork_agent_api::{
    ApiErrorCode, CreateSessionRequest, MailboxRequest, ProfileDocument, SessionView,
};

const IM_SYSTEM_PROMPT: &str = include_str!("../prompts/im-thread-base-instructions.md");
const SLACK_PROACTIVE_SYSTEM_PROMPT: &str =
    include_str!("../prompts/slack-proactive-base-instructions.md");
pub fn system_prompt_for_binding(binding: &SessionBindingRow) -> &'static str {
    match binding {
        SessionBindingRow::Normal(_) => IM_SYSTEM_PROMPT,
        SessionBindingRow::Proactive(_) => SLACK_PROACTIVE_SYSTEM_PROMPT,
    }
}
pub type CreatedSession = SessionView;

/// Gateway response semantics for errors from its local Agent.
#[derive(Debug)]
pub struct AgentError {
    pub status: StatusCode,
    pub message: String,
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for AgentError {}
impl From<zork_agent::AgentError> for AgentError {
    fn from(error: zork_agent::AgentError) -> Self {
        let status = match error.code {
            ApiErrorCode::InvalidRequest | ApiErrorCode::SelectionUnavailable => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            ApiErrorCode::SessionNotFound | ApiErrorCode::ProfileNotFound => StatusCode::NOT_FOUND,
            ApiErrorCode::SessionOverloaded => StatusCode::TOO_MANY_REQUESTS,
            ApiErrorCode::SessionDeleting => StatusCode::CONFLICT,
            ApiErrorCode::InvalidCursor => StatusCode::BAD_REQUEST,
            ApiErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
            _ => StatusCode::BAD_GATEWAY,
        };
        Self {
            status,
            message: error.message,
        }
    }
}
impl From<serde_json::Error> for AgentError {
    fn from(error: serde_json::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

pub async fn list_profiles(agent: &Agent) -> Result<Vec<AgentProfile>> {
    Ok(agent.list_profiles().await?.items)
}
pub async fn profile_list_value(agent: &Agent) -> Result<Value> {
    profiles_value(agent).await
}
pub async fn profiles_value(agent: &Agent) -> Result<Value> {
    serde_json::to_value(agent.list_profiles().await?).context("encode profile list")
}
pub async fn session_statuses(agent: &Agent) -> Result<HashMap<String, String>> {
    Ok(agent
        .list_sessions()
        .await
        .items
        .into_iter()
        .map(|s| (s.session_id, s.status.as_str().to_owned()))
        .collect())
}
pub fn default_selection(profiles: &[AgentProfile]) -> Option<SessionSelection> {
    let (model, thinking) = profiles.iter().find_map(|profile| {
        if !profile.auth_configured {
            return None;
        }
        let model = profile
            .models
            .iter()
            .find(|model| model.default && model.enabled && model.limits.is_some())?;
        model
            .thinking
            .iter()
            .any(|thinking| thinking == &model.default_thinking)
            .then(|| (model.id.clone(), model.default_thinking.clone()))
    })?;
    resolve_selection(
        profiles,
        &SessionSelection {
            profile_id: "auto".to_owned(),
            model,
            thinking,
        },
    )
}

pub fn resolve_selection(
    profiles: &[AgentProfile],
    requested: &SessionSelection,
) -> Option<SessionSelection> {
    let compatible = profiles
        .iter()
        .filter(|profile| {
            profile.auth_configured
                && profile.models.iter().any(|model| {
                    model.enabled
                        && model.limits.is_some()
                        && model.id == requested.model
                        && model
                            .thinking
                            .iter()
                            .any(|thinking| thinking == &requested.thinking)
                })
        })
        .collect::<Vec<_>>();
    let profile_id = if requested.profile_id.trim().is_empty() || requested.profile_id == "auto" {
        compatible.first()?;
        "auto".to_owned()
    } else {
        compatible
            .into_iter()
            .find(|profile| profile.profile_id == requested.profile_id)?
            .profile_id
            .clone()
    };
    Some(SessionSelection {
        profile_id,
        model: requested.model.clone(),
        thinking: requested.thinking.clone(),
    })
}

pub async fn ensure_session(
    agent: &zork_agent::Agent,
    db: &GatewayDb,
    session: &SessionRow,
) -> Result<String> {
    ensure_binding_session(agent, db, &SessionBindingRow::Normal(session.clone())).await
}

pub async fn ensure_proactive_session(
    agent: &zork_agent::Agent,
    db: &GatewayDb,
    binding: &ProactiveBindingRow,
) -> Result<String> {
    ensure_binding_session(agent, db, &SessionBindingRow::Proactive(binding.clone())).await
}

pub async fn ensure_binding_session(
    agent: &zork_agent::Agent,
    db: &GatewayDb,
    binding: &SessionBindingRow,
) -> Result<String> {
    if let Some(session_id) = binding.id() {
        return Ok(session_id.to_owned());
    }
    let profiles = list_profiles(agent).await?;
    let selection = match default_selection(&profiles) {
        Some(selection) => selection,
        None => {
            db.set_binding_selection_block(binding, "no_selectable_profiles")?;
            anyhow::bail!("no selectable Agent profiles");
        }
    };
    let system_prompt = system_prompt_for_binding(binding);
    let created = create_session(
        agent,
        &selection,
        Some(system_prompt),
        binding.workspace_path(),
    )
    .await?;
    db.set_binding_agent_session(
        binding,
        &created.session_id,
        &created.workspace,
        &selection.profile_id,
        &selection.model,
        &selection.thinking,
    )?;
    Ok(created.session_id)
}

pub async fn create_binding_session(
    agent: &zork_agent::Agent,
    db: &GatewayDb,
    binding: &SessionBindingRow,
    selection: &SessionSelection,
) -> std::result::Result<CreatedSession, AgentError> {
    if binding.id().is_some() {
        return Err(AgentError {
            status: StatusCode::CONFLICT,
            message: "binding already has an Agent session".to_owned(),
        });
    }
    let created = create_session(
        agent,
        selection,
        Some(system_prompt_for_binding(binding)),
        binding.workspace_path(),
    )
    .await?;
    db.set_binding_agent_session(
        binding,
        &created.session_id,
        &created.workspace,
        &created.profile_id,
        &created.model,
        &created.thinking,
    )
    .map_err(|error| AgentError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: format!("persist Agent binding: {error}"),
    })?;
    Ok(created)
}

pub async fn create_session(
    agent: &Agent,
    selection: &SessionSelection,
    system_prompt: Option<&str>,
    workspace: &str,
) -> std::result::Result<CreatedSession, AgentError> {
    Ok(agent
        .create_session(CreateSessionRequest {
            context: None,
            profile_id: selection.profile_id.clone(),
            model: selection.model.clone(),
            thinking: selection.thinking.clone(),
            system_prompt: system_prompt.map(str::to_owned),
            workspace: Some(workspace.to_owned()),
        })
        .await?)
}
pub async fn ensure_allocated_session(
    agent: &Agent,
    db: &GatewayDb,
    binding: &SessionBindingRow,
    runtime_id: &str,
    selection: &SessionSelection,
    prompt: &str,
) -> Result<CreatedSession> {
    let created = agent
        .ensure_session(
            runtime_id.to_owned(),
            CreateSessionRequest {
                profile_id: selection.profile_id.clone(),
                model: selection.model.clone(),
                thinking: selection.thinking.clone(),
                system_prompt: Some(prompt.into()),
                workspace: Some(binding.workspace_path().into()),
                context: None,
            },
        )
        .await?;
    db.set_binding_agent_session(
        binding,
        &created.session_id,
        &created.workspace,
        &created.profile_id,
        &created.model,
        &created.thinking,
    )?;
    Ok(created)
}
pub async fn update_selection(
    agent: &Agent,
    session_id: &str,
    selection: &SessionSelection,
) -> std::result::Result<SessionSelection, AgentError> {
    Ok(agent
        .set_selection(session_id.to_owned(), selection.clone())
        .await?
        .selection())
}
/// Context policy lives only in Agent state.
pub async fn session_context(
    agent: &Agent,
    session_id: &str,
    update: Option<&zork_agent_api::ContextConfig>,
) -> std::result::Result<zork_agent_api::ContextConfig, AgentError> {
    Ok(match update {
        Some(context) => {
            agent
                .set_context(session_id.to_owned(), context.clone())
                .await?
        }
        None => agent.get_session(session_id.to_owned()).await?,
    }
    .context)
}
pub async fn append_mailbox(agent: &Agent, session_id: &str, content: &str) -> Result<()> {
    Ok(agent
        .append_mailbox(
            session_id.to_owned(),
            MailboxRequest {
                content: content.to_owned(),
            },
        )
        .await?)
}
pub async fn append_mailbox_id(
    agent: &Agent,
    session_id: &str,
    request_id: &str,
    content: &str,
) -> Result<()> {
    Ok(agent
        .append_mailbox_id(
            session_id.to_owned(),
            request_id.to_owned(),
            MailboxRequest {
                content: content.to_owned(),
            },
        )
        .await?)
}
pub async fn cancel_session(agent: &Agent, session_id: &str) -> Result<bool> {
    match agent.cancel_session(session_id.to_owned()).await {
        Ok(()) => Ok(true),
        Err(error) if error.code == ApiErrorCode::SessionNotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
pub async fn put_profile(
    agent: &Agent,
    profile_id: &str,
    document: &Value,
) -> std::result::Result<Value, AgentError> {
    let document: ProfileDocument =
        serde_json::from_value(document.clone()).map_err(|error| AgentError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: format!("invalid Agent profile document: {error}"),
        })?;
    Ok(serde_json::to_value(
        agent.put_profile(profile_id.to_owned(), document).await?,
    )?)
}
pub async fn discover_profile_models(
    agent: &Agent,
    profile_id: &str,
) -> std::result::Result<zork_profile::ModelDiscovery, AgentError> {
    Ok(agent.discover_models(profile_id.to_owned()).await?)
}
pub async fn get_profile(
    agent: &Agent,
    profile_id: &str,
) -> std::result::Result<Value, AgentError> {
    Ok(serde_json::to_value(
        agent.get_profile(profile_id.to_owned()).await?,
    )?)
}
pub async fn update_profile_models(
    agent: &Agent,
    profile_id: &str,
    models: &[zork_profile::ProfileModel],
    expected_models: Option<&[zork_profile::ProfileModel]>,
) -> std::result::Result<Value, AgentError> {
    Ok(serde_json::to_value(
        agent
            .update_profile_models(
                profile_id.to_owned(),
                zork_agent::application::UpdateProfileModels {
                    models: models.to_vec(),
                    expected_models: expected_models.map(<[_]>::to_vec),
                },
            )
            .await?,
    )?)
}
pub async fn delete_profile(
    agent: &Agent,
    profile_id: &str,
) -> std::result::Result<(), AgentError> {
    Ok(agent.delete_profile(profile_id.to_owned()).await?)
}
pub async fn session_history(
    agent: &Agent,
    session_id: &str,
    query: &zork_agent_api::HistoryQuery,
) -> std::result::Result<
    zork_agent_api::HistoryPage<zork_agent::session::events::SessionEvent>,
    AgentError,
> {
    Ok(agent
        .list_history(session_id.to_owned(), query.clone())
        .await?)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proactive_prompt_preserves_strict_participation_boundaries() {
        assert!(SLACK_PROACTIVE_SYSTEM_PROMPT.contains("unless all three conditions are true"));
        assert!(SLACK_PROACTIVE_SYSTEM_PROMPT.contains("unrelated thread"));
        assert!(SLACK_PROACTIVE_SYSTEM_PROMPT.contains("pass connect_id explicitly"));
        assert!(SLACK_PROACTIVE_SYSTEM_PROMPT.contains("you are Zork, not Codex"));
    }

    fn profile(
        profile_id: &str,
        billing: &str,
        account: Value,
        rate_limits: Value,
    ) -> AgentProfile {
        serde_json::from_value(json!({
            "profile_id": profile_id,
            "provider": "test",
            "billing": billing,
            "auth_configured": true,
            "account": account,
            "rateLimits": rate_limits,
            "models": [{
                "id": "grok-4.6",
                "api": "openai-responses",
                "streaming": true,
                "parallel_tool_calls": false,
                "thinking": ["high", "xhigh"],
                "default_thinking": "xhigh",
                "capabilities": {"input": ["text"]},
                "limits": {"context_window_tokens":32000,"max_output_tokens":4096},
                "default": true
            }]
        }))
        .unwrap()
    }

    #[test]
    fn disabled_models_are_neither_default_nor_eligible_for_explicit_or_auto_selection() {
        let mut profiles = vec![profile("grok", "subscription", json!({}), json!({}))];
        profiles[0].models[0].enabled = false;
        assert!(default_selection(&profiles).is_none());
        for id in ["auto", "grok"] {
            assert!(resolve_selection(
                &profiles,
                &SessionSelection {
                    profile_id: id.into(),
                    model: "grok-4.6".into(),
                    thinking: "xhigh".into()
                }
            )
            .is_none());
        }
    }

    #[test]
    fn selects_only_an_explicit_profile_default_model_and_thinking() {
        let profiles = vec![profile("grok", "subscription", json!({}), json!({}))];
        assert_eq!(
            default_selection(&profiles),
            Some(SessionSelection {
                profile_id: "auto".to_owned(),
                model: "grok-4.6".to_owned(),
                thinking: "xhigh".to_owned(),
            })
        );
    }

    #[test]
    fn automatic_profile_keeps_the_requested_model_and_thinking() {
        let profiles = vec![
            profile(
                "usage",
                "usage",
                json!({ "ok": true }),
                json!({ "ok": true, "rateLimits": { "credits": { "balance": "100" } } }),
            ),
            profile(
                "subscription",
                "subscription",
                json!({ "ok": true }),
                json!({ "ok": true, "rateLimits": { "secondary": { "usedPercent": 60 } } }),
            ),
        ];
        assert_eq!(
            resolve_selection(
                &profiles,
                &SessionSelection {
                    profile_id: "auto".to_owned(),
                    model: "grok-4.6".to_owned(),
                    thinking: "high".to_owned(),
                },
            ),
            Some(SessionSelection {
                profile_id: "auto".to_owned(),
                model: "grok-4.6".to_owned(),
                thinking: "high".to_owned(),
            })
        );
    }
}

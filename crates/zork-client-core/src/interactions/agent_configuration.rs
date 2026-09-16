//! Agent configuration card presentation, input validation and operation dispatch.
use super::*;

pub(super) const ADAPTER: Adapter = Adapter {
    key: AGENT_CONFIGURATION,
    accepts: |request| {
        matches!(
            request,
            Request::AgentConfiguration { .. } | Request::Input { .. } | Request::Approval { .. }
        )
    },
    project,
    resolve,
    #[cfg(not(target_family = "wasm"))]
    execute: |conversation, command| conversation.submit_configuration_review(command),
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    pub response: Response,
    #[serde(default)]
    pub attempted: bool,
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub rejected: bool,
    #[serde(default)]
    pub error: Option<String>,
}

fn resolve(command: Command) -> anyhow::Result<Command> {
    match command {
        Command::Activate {
            message_id,
            choice,
            values,
        } => match choice.as_str() {
            "submit" => Ok(Command::Submit { message_id, values }),
            "decline" => Ok(Command::Decline { message_id }),
            "retry" => Ok(Command::Retry { message_id }),
            _ => anyhow::bail!("Unsupported Agent configuration action"),
        },
        command @ (Command::Submit { .. } | Command::Decline { .. } | Command::Retry { .. }) => {
            Ok(command)
        }
        _ => anyhow::bail!("Unsupported Agent configuration action"),
    }
}

pub(crate) fn validate(
    request: &Request,
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, BTreeMap<String, String>> {
    request.validate_values(values)
}

fn project(
    message_id: &str,
    request: &Request,
    resolution: Option<&Resolution>,
    submission: Option<&Submission>,
    errors: &BTreeMap<String, String>,
) -> Card {
    let (title, localized_title, submit, fields, details) = match request {
        Request::AgentConfiguration {
            creating,
            name,
            fields,
            ..
        } => (
            if *creating {
                "interaction_create_agent"
            } else {
                "interaction_update_agent"
            }
            .into(),
            true,
            if *creating {
                "interaction_confirm_create"
            } else {
                "interaction_confirm_update"
            },
            fields.clone(),
            if *creating {
                vec![]
            } else {
                vec![Detail {
                    label_key: "interaction_target_agent".into(),
                    value: name.clone(),
                }]
            },
        ),
        Request::OAuth { .. } => unreachable!("configuration adapter requires its form"),
        Request::Approval { title, description } => (
            title.clone(),
            false,
            "interaction_approve",
            vec![],
            vec![Detail {
                label_key: "interaction_description".into(),
                value: description.clone(),
            }],
        ),
        Request::Input { title, fields } => (
            title.clone(),
            false,
            "interaction_submit",
            fields.clone(),
            vec![],
        ),
    };
    let mut values = request.defaults();
    if let Some(submission) = submission {
        values.extend(submission.response.values.clone());
    }
    if let Some(resolution) = resolution {
        if let Some(actual) = resolution
            .output
            .get("values")
            .and_then(serde_json::Value::as_object)
        {
            values.extend(
                actual
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))),
            );
        }
    }
    let ready = resolution.is_none() && submission.is_none_or(|s| s.rejected);
    let retry =
        resolution.is_none() && submission.is_some_and(|s| s.error.is_some() && !s.rejected);
    let status = if let Some(result) = resolution {
        match result.outcome {
            Outcome::Completed if matches!(request, Request::Approval { .. }) => {
                "interaction_approved"
            }
            Outcome::Completed
                if matches!(request, Request::AgentConfiguration { creating: true, .. }) =>
            {
                "interaction_agent_created"
            }
            Outcome::Completed
                if matches!(
                    request,
                    Request::AgentConfiguration {
                        creating: false,
                        ..
                    }
                ) =>
            {
                "interaction_agent_updated"
            }
            Outcome::Completed => "interaction_completed",
            Outcome::Declined => "interaction_declined",
            Outcome::Pending => "interaction_processing",
            Outcome::Cancelled => "interaction_cancelled",
            Outcome::Expired => "interaction_expired",
            Outcome::Failed => "interaction_failed",
            Outcome::Unknown => "interaction_unconfirmed",
        }
    } else if submission.is_some_and(|s| s.rejected) {
        "interaction_submission_failed"
    } else if retry {
        "interaction_unconfirmed"
    } else if submission.is_some() {
        "interaction_submitting"
    } else {
        "interaction_confirmation"
    };
    let no_models = matches!(request, Request::AgentConfiguration { fields, .. } if fields.iter().any(|field|field.id=="/selection" && field.options.is_empty()));
    let mut actions = if ready {
        vec![
            CardAction {
                id: "submit".into(),
                label_key: submit.into(),
                primary: true,
                open_url: None,
            },
            CardAction {
                id: "decline".into(),
                label_key: "interaction_decline".into(),
                primary: false,
                open_url: None,
            },
        ]
    } else if retry {
        vec![CardAction {
            id: "retry".into(),
            label_key: "interaction_retry".into(),
            primary: true,
            open_url: None,
        }]
    } else {
        vec![]
    };
    if no_models {
        actions.retain(|action| action.id != "submit");
    }
    Card {
        message_id: message_id.into(),
        title,
        localized_title,
        status_key: status.into(),
        description_key: resolution.is_none_or(|result| !result.outcome.terminal()).then(||
            if matches!(request, Request::AgentConfiguration { creating:true, .. }) {
                "interaction_create_description"
            } else if matches!(request, Request::AgentConfiguration { creating:false, .. }) {
                "interaction_update_description"
            } else if matches!(request, Request::Approval { .. }) {
                "interaction_approval_scope"
            } else if matches!(request, Request::Input { .. }) {
                "interaction_public_input"
            } else {
                "interaction_chat_node"
            }
            .into(),
        ),
        details,
        actions,
        editable: ready,
        error: if no_models && ready {
            Some("interaction_no_models".into())
        } else if resolution.is_some() {
            None
        } else {
            submission
                .and_then(|s| s.error.clone())
                .or_else(|| errors.get("").cloned())
        },
        fields: fields
            .into_iter()
            .map(|field| CardField {
                localized_options: localized_title && field.id == "/role",
                advanced: matches!(request, Request::AgentConfiguration { prominent, .. } if !prominent.contains(&field.id)),
                sensitive: false,
                value: values.get(&field.id).cloned().unwrap_or_default(),
                error_key: if ready {
                    errors.get(&field.id).cloned()
                } else {
                    None
                },
                localized_label: localized_title,
                field,
            })
            .collect(),
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) mod delivery;

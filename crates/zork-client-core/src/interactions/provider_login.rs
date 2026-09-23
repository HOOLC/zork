//! Provider login card presentation and actions. No generic external-action
//! variant selects login; only the login business handler may use this adapter.
use super::*;

pub(super) const ADAPTER: Adapter = Adapter {
    key: PROVIDER_LOGIN,
    accepts: |request| matches!(request, Request::OAuth { .. }),
    project,
    resolve,
    execute: |conversation, command| conversation.login_action(command),
};
fn resolve(command: Command) -> anyhow::Result<Command> {
    match command {
        Command::Activate {
            message_id,
            choice,
            values,
        } => match choice.as_str() {
            "continue_login" => Ok(Command::ContinueLogin { message_id }),
            "cancel_login" => Ok(Command::CancelLogin { message_id }),
            "submit_login_callback" => Ok(Command::LoginCallback {
                message_id,
                callback: values.get("callback").cloned().unwrap_or_default(),
            }),
            _ => anyhow::bail!("Unsupported Provider login action"),
        },
        command @ (Command::ContinueLogin { .. }
        | Command::CancelLogin { .. }
        | Command::LoginCallback { .. }) => Ok(command),
        _ => anyhow::bail!("Unsupported Provider login action"),
    }
}

fn project(
    message_id: &str,
    request: &Request,
    resolution: Option<&Resolution>,
    _: Option<&Submission>,
    _: &BTreeMap<String, String>,
) -> Card {
    let Request::OAuth { title } = request else {
        unreachable!("login adapter requires a login card")
    };
    let status = match resolution.map(|r| r.outcome) {
        Some(Outcome::Completed) => "interaction_completed",
        Some(Outcome::Declined) => "interaction_declined",
        Some(Outcome::Cancelled) => "interaction_cancelled",
        Some(Outcome::Expired) => "interaction_expired",
        Some(Outcome::Failed) => "interaction_failed",
        Some(Outcome::Unknown) => "interaction_unconfirmed",
        Some(Outcome::Pending) => "interaction_waiting_login",
        None => "interaction_preparing_login",
    };
    let mut actions = vec![];
    if resolution.is_none_or(|r| !r.outcome.terminal()) {
        if resolution.is_some() {
            actions.push(CardAction {
                id: "continue_login".into(),
                label_key: "interaction_continue_login".into(),
                primary: true,
                open_url: None,
            });
        }
        actions.push(CardAction {
            id: "cancel_login".into(),
            label_key: "interaction_cancel_login".into(),
            primary: false,
            open_url: None,
        });
    }
    Card {
        message_id: message_id.into(),
        title: "interaction_login".into(),
        localized_title: true,
        status_key: status.into(),
        description_key: Some("interaction_private_login".into()),
        fields: vec![],
        details: vec![Detail {
            label_key: "interaction_provider".into(),
            value: title.clone(),
        }],
        actions,
        editable: false,
        error: None,
    }
}

/// Authenticated, transient view data. Never part of a Chat message or outbox.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct LoginView {
    pub open_url: Option<String>,
    pub user_code: Option<String>,
    pub callback: bool,
    pub busy: bool,
    pub error: Option<String>,
}
impl LoginView {
    pub(crate) fn apply(&self, card: &mut Card) {
        card.error = self.error.clone();
        card.status_key = if self.busy {
            "interaction_verifying_login"
        } else {
            "interaction_waiting_login"
        }
        .into();
        card.description_key = Some(
            if self.callback {
                "interaction_login_browser_steps"
            } else {
                "interaction_login_device_steps"
            }
            .into(),
        );
        card.editable = self.callback && !self.busy;
        card.actions.clear();
        if let Some(url) = &self.open_url {
            card.actions.push(CardAction {
                id: "open_login".into(),
                label_key: "interaction_open_login".into(),
                primary: true,
                open_url: Some(url.clone()),
            });
        }
        if let Some(code) = &self.user_code {
            card.details.push(Detail {
                label_key: "interaction_login_code".into(),
                value: code.clone(),
            });
        }
        if self.callback {
            card.fields = vec![CardField {
                field: Field {
                    id: "callback".into(),
                    label: "interaction_login_callback".into(),
                    kind: FieldKind::Text,
                    required: true,
                    default: String::new(),
                    options: vec![],
                },
                localized_label: true,
                localized_options: false,
                advanced: false,
                value: String::new(),
                error_key: None,
                sensitive: true,
            }];
            if !self.busy {
                card.actions.push(CardAction {
                    id: "submit_login_callback".into(),
                    label_key: "interaction_finish_login".into(),
                    primary: false,
                    open_url: None,
                });
            }
        }
        if self.open_url.is_none() && !self.busy {
            card.actions.push(CardAction {
                id: "continue_login".into(),
                label_key: "interaction_continue_login".into(),
                primary: true,
                open_url: None,
            });
        }
        card.actions.push(CardAction {
            id: "cancel_login".into(),
            label_key: "interaction_cancel_login".into(),
            primary: false,
            open_url: None,
        });
    }
}

pub(crate) fn decorate(metadata: &mut crate::api::MessageMetadata, view: &LoginView) {
    if !matches!(super::request(metadata), Some(Request::OAuth { .. })) {
        return;
    }
    let initial = super::initial_result(metadata);
    if metadata
        .interaction_result
        .as_deref()
        .or(initial.as_ref())
        .is_some_and(|result| result.outcome == Outcome::Pending)
    {
        if let Some(card) = metadata.interaction_view.as_mut() {
            view.apply(card);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_login_fields_do_not_decorate_another_or_unknown_business() {
        for (handler, request) in [
            (
                AGENT_CONFIGURATION,
                Request::Approval {
                    title: "Review".into(),
                    description: "Confirm".into(),
                },
            ),
            (
                "unknown-business",
                Request::OAuth {
                    title: "Unsupported".into(),
                },
            ),
        ] {
            let mut metadata = crate::api::MessageMetadata {
                id: Some("card".into()),
                interaction: Some(Box::new(
                    serde_json::to_value(MessageContent::linked(
                        "owner/request".into(),
                        handler.into(),
                        request,
                        Some(Resolution {
                            request_message_id: "card".into(),
                            response_id: "attempt".into(),
                            revision: 1,
                            outcome: Outcome::Pending,
                            actor: "business".into(),
                            output: serde_json::json!({}),
                        }),
                    ))
                    .unwrap(),
                )),
                ..Default::default()
            };
            metadata.interaction_view = super::super::view(&metadata, None, &BTreeMap::new());
            let before = metadata.interaction_view.clone();
            decorate(
                &mut metadata,
                &LoginView {
                    callback: true,
                    open_url: Some("https://example.invalid/private".into()),
                    ..Default::default()
                },
            );
            assert_eq!(metadata.interaction_view, before);
        }
    }
}

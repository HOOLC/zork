//! Composition of business card adapters and their read-only common view types.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub use zork_client_types::interaction::*;
pub(crate) mod agent_configuration;
pub(crate) mod provider_login;
pub use agent_configuration::Submission;
pub(crate) use provider_login::LoginView;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    LoginCallback {
        message_id: String,
        callback: String,
    },
    ContinueLogin {
        message_id: String,
    },
    CancelLogin {
        message_id: String,
    },
    Activate {
        message_id: String,
        choice: String,
        #[serde(default)]
        values: BTreeMap<String, String>,
    },
    Submit {
        message_id: String,
        values: BTreeMap<String, String>,
    },
    Decline {
        message_id: String,
    },
    Retry {
        message_id: String,
    },
}
impl Command {
    pub fn from_action(
        message_id: &str,
        action: &str,
        values: BTreeMap<String, String>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!action.is_empty(), "Missing card action");
        Ok(Self::Activate {
            message_id: message_id.into(),
            choice: action.into(),
            values,
        })
    }
    pub fn message_id(&self) -> &str {
        match self {
            Self::Activate { message_id, .. }
            | Self::LoginCallback { message_id, .. }
            | Self::ContinueLogin { message_id }
            | Self::CancelLogin { message_id }
            | Self::Submit { message_id, .. }
            | Self::Decline { message_id }
            | Self::Retry { message_id } => message_id,
        }
    }
    pub(crate) fn resolve(self, handler: &str) -> anyhow::Result<Self> {
        (adapter(handler)
            .ok_or_else(|| anyhow::anyhow!("Unsupported business card"))?
            .resolve)(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardField {
    pub field: Field,
    pub localized_label: bool,
    #[serde(default)]
    pub localized_options: bool,
    #[serde(default)]
    pub advanced: bool,
    pub value: String,
    pub error_key: Option<String>,
    #[serde(default)]
    pub sensitive: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detail {
    pub label_key: String,
    pub value: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardAction {
    pub id: String,
    pub label_key: String,
    pub primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_url: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub message_id: String,
    pub title: String,
    pub localized_title: bool,
    pub status_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_key: Option<String>,
    pub fields: Vec<CardField>,
    pub details: Vec<Detail>,
    pub actions: Vec<CardAction>,
    pub editable: bool,
    pub error: Option<String>,
}

/// Portable read-only presentation, also used by deterministic Web fixtures.
struct Adapter {
    key: &'static str,
    accepts: fn(&Request) -> bool,
    project: fn(
        &str,
        &Request,
        Option<&Resolution>,
        Option<&Submission>,
        &BTreeMap<String, String>,
    ) -> Card,
    resolve: fn(Command) -> anyhow::Result<Command>,
    execute: fn(&std::sync::Arc<crate::state::Conversation>, Command) -> anyhow::Result<()>,
}
const ADAPTERS: &[Adapter] = &[agent_configuration::ADAPTER, provider_login::ADAPTER];
fn adapter(handler: &str) -> Option<&'static Adapter> {
    ADAPTERS.iter().find(|a| a.key == handler)
}
fn accepts(handler: &str, request: &Request) -> bool {
    adapter(handler).is_some_and(|a| (a.accepts)(request))
}

pub fn card(
    message_id: &str,
    handler: &str,
    request: &Request,
    resolution: Option<&Resolution>,
    submission: Option<&Submission>,
    errors: &BTreeMap<String, String>,
) -> Card {
    match adapter(handler).filter(|a| (a.accepts)(request)) {
        Some(adapter) => (adapter.project)(message_id, request, resolution, submission, errors),
        None => unsupported(message_id),
    }
}
fn unsupported(message_id: &str) -> Card {
    Card {
        message_id: message_id.into(),
        title: "interaction_unsupported".into(),
        localized_title: true,
        status_key: "interaction_unsupported".into(),
        description_key: None,
        fields: vec![],
        details: vec![],
        actions: vec![],
        editable: false,
        error: None,
    }
}

/// Shared native/Web projection. Platform support never changes the source message.
pub fn local_script_card(
    message_id: &str,
    script: &zork_client_types::local_script::Card,
    android: bool,
) -> Card {
    let mut details = Vec::new();
    if let Some(description) = &script.description {
        details.push(Detail {
            label_key: "local_script_description".into(),
            value: description.clone(),
        });
    }
    details.push(Detail {
        label_key: "local_script_source".into(),
        value: script.source.clone(),
    });
    let can_run = android && script.validate().is_ok();
    Card {
        message_id: message_id.into(),
        title: script.title.clone(),
        localized_title: false,
        status_key: if can_run {
            "local_script_ready"
        } else {
            "local_script_readonly"
        }
        .into(),
        description_key: None,
        fields: vec![],
        details,
        actions: if can_run {
            vec![CardAction {
                id: "run_local_script".into(),
                label_key: "local_script_run".into(),
                primary: true,
                open_url: None,
            }]
        } else {
            vec![]
        },
        editable: false,
        error: None,
    }
}

pub(crate) fn dispatch(
    conversation: &std::sync::Arc<crate::state::Conversation>,
    handler: &str,
    command: Command,
) -> anyhow::Result<()> {
    let adapter = adapter(handler).ok_or_else(|| anyhow::anyhow!("Unsupported business card"))?;
    (adapter.execute)(conversation, command.resolve(handler)?)
}

impl Card {
    pub(crate) fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.message_id.capacity()
            + self.title.capacity()
            + self.status_key.capacity()
            + self.description_key.as_ref().map_or(0, String::capacity)
            + self.error.as_ref().map_or(0, String::capacity)
            + self
                .details
                .iter()
                .map(|d| {
                    std::mem::size_of::<Detail>() + d.label_key.capacity() + d.value.capacity()
                })
                .sum::<usize>()
            + self
                .actions
                .iter()
                .map(|a| {
                    std::mem::size_of::<CardAction>()
                        + a.id.capacity()
                        + a.label_key.capacity()
                        + a.open_url.as_ref().map_or(0, String::capacity)
                })
                .sum::<usize>()
            + self
                .fields
                .iter()
                .map(|f| {
                    std::mem::size_of::<CardField>()
                        + f.field.id.capacity()
                        + f.field.label.capacity()
                        + f.field.default.capacity()
                        + f.value.capacity()
                        + f.error_key.as_ref().map_or(0, String::capacity)
                        + f.field
                            .options
                            .iter()
                            .map(|o| {
                                std::mem::size_of::<Choice>()
                                    + o.label.capacity()
                                    + o.value.capacity()
                            })
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
}

pub(crate) fn view(
    metadata: &crate::api::MessageMetadata,
    submission: Option<&Submission>,
    errors: &BTreeMap<String, String>,
) -> Option<Box<Card>> {
    let raw = metadata.interaction.as_deref()?;
    if let Some(script) = zork_client_types::local_script::Card::parse(raw) {
        return Some(Box::new(local_script_card(
            metadata.id.as_deref().unwrap_or_default(),
            &script,
            cfg!(all(target_os = "android", feature = "local-scripts")),
        )));
    }
    if let Some(request) = request(metadata) {
        let initial = initial_result(metadata);
        let projection = card(
            metadata.id.as_deref()?,
            &MessageContent::parse(raw)?.handler,
            &request,
            metadata.interaction_result.as_deref().or(initial.as_ref()),
            submission,
            errors,
        );
        Some(Box::new(projection))
    } else if MessageContent::parse(raw)
        .is_none_or(|content| matches!(content.content, Content::Request { .. }))
    {
        Some(Box::new(unsupported(
            metadata.id.as_deref().unwrap_or_default(),
        )))
    } else {
        None
    }
}

pub(crate) fn request(metadata: &crate::api::MessageMetadata) -> Option<Request> {
    let content = MessageContent::parse(metadata.interaction.as_deref()?)?;
    match content.content {
        Content::Request { request }
            if accepts(&content.handler, &request) && request.validate().is_ok() =>
        {
            Some(request)
        }
        _ => None,
    }
}

pub(crate) fn initial_result(metadata: &crate::api::MessageMetadata) -> Option<Resolution> {
    let initial = MessageContent::parse(metadata.interaction.as_deref()?)?.snapshot?;
    (metadata.id.as_deref() == Some(&initial.request_message_id)).then_some(initial)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResultEvent {
    pub request_id: String,
    pub handler: String,
    pub result: Resolution,
}

pub(crate) fn result_event(metadata: &crate::api::MessageMetadata) -> Option<ResultEvent> {
    if metadata.author_kind != Some(zork_client_types::chat::AuthorKind::System) {
        return None;
    }
    let content = MessageContent::parse(metadata.interaction.as_deref()?)?;
    adapter(&content.handler)?;
    match content.content {
        Content::Result { result }
            if valid_id(&result.request_message_id) && result.revision > 0 =>
        {
            Some(ResultEvent {
                request_id: content.request_id,
                handler: content.handler,
                result,
            })
        }
        _ => None,
    }
}

pub(crate) fn result(metadata: &crate::api::MessageMetadata) -> Option<Resolution> {
    result_event(metadata).map(|event| event.result)
}

pub(crate) fn cached_result(metadata: &crate::api::MessageMetadata) -> Option<ResultEvent> {
    request(metadata)?;
    let content = MessageContent::parse(metadata.interaction.as_deref()?)?;
    let result = metadata
        .interaction_result
        .as_deref()
        .cloned()
        .or(content.snapshot)?;
    Some(ResultEvent {
        request_id: content.request_id,
        handler: content.handler,
        result,
    })
}

pub(crate) fn merge_result(
    metadata: &mut crate::api::MessageMetadata,
    event: &ResultEvent,
) -> anyhow::Result<bool> {
    let identity = MessageContent::parse(
        metadata
            .interaction
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("interaction_request_mismatch"))?,
    )
    .ok_or_else(|| anyhow::anyhow!("interaction_request_mismatch"))?;
    anyhow::ensure!(
        identity.handler == event.handler && identity.request_id == event.request_id,
        "interaction_business_mismatch"
    );
    let incoming = &event.result;
    anyhow::ensure!(
        metadata.id.as_deref() == Some(&incoming.request_message_id) && request(metadata).is_some(),
        "interaction_request_mismatch"
    );
    let initial = initial_result(metadata);
    if let Some(old) = metadata.interaction_result.as_deref().or(initial.as_ref()) {
        if old.revision > incoming.revision {
            return Ok(false);
        }
        if old.revision == incoming.revision {
            anyhow::ensure!(old == incoming, "interaction_result_conflict");
            return Ok(false);
        }
    }
    metadata.interaction_result = Some(Box::new(incoming.clone()));
    Ok(true)
}

#[cfg(feature = "headless-bench")]
pub mod preview;

#[cfg(test)]
mod participation_tests {
    use super::*;
    #[test]
    fn external_business_preparation_does_not_add_an_approval_step() {
        let request = Request::OAuth {
            title: "Sign in".into(),
        };
        let view = card(
            "card",
            PROVIDER_LOGIN,
            &request,
            None,
            None,
            &BTreeMap::new(),
        );
        assert_eq!(view.status_key, "interaction_preparing_login");
        assert!(!view.actions.iter().any(|a| a.id == "submit"));
        assert!(view.actions.iter().any(|a| a.id == "cancel_login"));
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn each_business_accepts_only_its_card_and_actions() {
        let login = Request::OAuth {
            title: "Connect".into(),
        };
        let wrong = card(
            "card",
            AGENT_CONFIGURATION,
            &login,
            None,
            None,
            &BTreeMap::new(),
        );
        assert!(!wrong.editable && wrong.actions.is_empty());
        assert!(Command::from_action("card", "submit", BTreeMap::new())
            .unwrap()
            .resolve(PROVIDER_LOGIN)
            .is_err());
        assert!(
            Command::from_action("card", "continue_login", BTreeMap::new())
                .unwrap()
                .resolve(AGENT_CONFIGURATION)
                .is_err()
        );
        assert!(Command::from_action("card", "submit", BTreeMap::new())
            .unwrap()
            .resolve("unregistered-business")
            .is_err());
        let unknown = card(
            "card",
            "unregistered-business",
            &login,
            None,
            None,
            &BTreeMap::new(),
        );
        assert_eq!(unknown.status_key, "interaction_unsupported");
        assert!(unknown.actions.is_empty());
    }
}

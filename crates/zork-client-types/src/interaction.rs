//! Business card presentation contracts. These are used by the Agent configuration
//! and Provider login adapters, not by the interaction registration mechanism.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
pub const VERSION: u32 = 5;
pub const AGENT_CONFIGURATION: &str = "agent.configuration";
pub const PROVIDER_LOGIN: &str = "provider.login";
pub const MAX_FIELDS: usize = 64;
pub const MAX_INPUT_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub value: String,
    pub label: String,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    #[default]
    Text,
    Multiline,
    Choice,
    MultiChoice,
    Json,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub kind: FieldKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub options: Vec<Choice>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "agent_configuration")]
    AgentConfiguration {
        creating: bool,
        name: String,
        fields: Vec<Field>,
        prominent: Vec<String>,
    },
    #[serde(rename = "oauth", alias = "provider_login")]
    OAuth { title: String },
    #[serde(rename = "approval")]
    Approval { title: String, description: String },
    #[serde(rename = "input")]
    Input { title: String, fields: Vec<Field> },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Declined,
    Pending,
    Cancelled,
    Expired,
    Failed,
    Unknown,
}
impl Outcome {
    pub fn terminal(self) -> bool {
        self != Self::Pending
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolution {
    pub request_message_id: String,
    pub response_id: String,
    pub revision: u64,
    pub outcome: Outcome,
    pub actor: String,
    pub output: Value,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Request { request: Request },
    Result { result: Resolution },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageContent {
    pub version: u32,
    pub request_id: String,
    pub handler: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Resolution>,
    #[serde(flatten)]
    pub content: Content,
}
impl MessageContent {
    pub fn parse(value: &Value) -> Option<Self> {
        let content: Self = serde_json::from_value(value.clone()).ok()?;
        (content.version == VERSION && valid_id(&content.request_id) && valid_id(&content.handler))
            .then_some(content)
    }
    pub fn linked(
        request_id: String,
        handler: String,
        request: Request,
        snapshot: Option<Resolution>,
    ) -> Self {
        Self {
            version: VERSION,
            request_id,
            handler,
            snapshot,
            content: Content::Request { request },
        }
    }
    pub fn linked_result(request_id: String, handler: String, result: Resolution) -> Self {
        Self {
            version: VERSION,
            request_id,
            handler,
            snapshot: None,
            content: Content::Result { result },
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub response_id: String,
    pub accept: bool,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
}
pub fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
}
impl Request {
    pub fn validate(&self) -> Result<(), &'static str> {
        let title = match self {
            Self::AgentConfiguration { .. } => "Agent configuration",
            Self::OAuth { title } | Self::Approval { title, .. } | Self::Input { title, .. } => {
                title
            }
        };
        if title.trim().is_empty() || title.len() > 512 {
            return Err("invalid_interaction_title");
        }
        if let Self::AgentConfiguration {
            name,
            fields,
            prominent,
            ..
        } = self
        {
            let mut seen = HashSet::new();
            if name.len() > 640
                || prominent.is_empty()
                || prominent
                    .iter()
                    .any(|id| !seen.insert(id) || !fields.iter().any(|field| field.id == *id))
            {
                return Err("invalid_agent_review");
            }
        }
        match self {
            Self::OAuth { .. } => Ok(()),
            Self::Approval { description, .. } => {
                if description.trim().is_empty() || description.len() > MAX_INPUT_BYTES {
                    Err("invalid_approval_request")
                } else {
                    Ok(())
                }
            }
            Self::AgentConfiguration { fields, .. } | Self::Input { fields, .. } => {
                if fields.is_empty()
                    || fields.len() > MAX_FIELDS
                    || fields.iter().map(|f| f.default.len()).sum::<usize>() > MAX_INPUT_BYTES
                {
                    return Err("invalid_input_request");
                }
                let mut ids = HashSet::new();
                for field in fields {
                    if !valid_id(&field.id)
                        || field.id.len() > 64
                        || !ids.insert(&field.id)
                        || field.label.trim().is_empty()
                        || field.label.len() > 512
                        || field.default.len() > MAX_INPUT_BYTES
                    {
                        return Err("invalid_input_field");
                    }
                    let mut options = HashSet::new();
                    if field.options.iter().any(|c| {
                        !valid_id(&c.value) || !valid_id(&c.label) || !options.insert(&c.value)
                    }) {
                        return Err("invalid_input_choices");
                    }
                    if field.kind == FieldKind::Choice {
                        if (field.options.is_empty()
                            && !matches!(self, Self::AgentConfiguration { .. }))
                            || (!field.default.is_empty() && !options.contains(&field.default))
                        {
                            return Err("invalid_input_choices");
                        }
                    } else if field.kind == FieldKind::MultiChoice {
                        if !valid_multi_choice(field, &field.default) {
                            return Err("invalid_input_choices");
                        }
                    } else if !field.options.is_empty() {
                        return Err("unexpected_input_choices");
                    }
                }
                Ok(())
            }
        }
    }
    pub fn defaults(&self) -> BTreeMap<String, String> {
        match self {
            Self::AgentConfiguration { fields, .. } | Self::Input { fields, .. } => fields
                .iter()
                .map(|f| (f.id.clone(), f.default.clone()))
                .collect(),
            _ => BTreeMap::new(),
        }
    }
    pub fn validate_values(
        &self,
        values: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, BTreeMap<String, String>> {
        let mut merged = self.defaults();
        let mut errors = BTreeMap::new();
        for (id, value) in values {
            if !merged.contains_key(id) {
                errors.insert(id.clone(), "unknown_input_field".into());
            } else {
                merged.insert(id.clone(), value.clone());
            }
        }
        if merged.values().map(String::len).sum::<usize>() > MAX_INPUT_BYTES {
            errors.insert(String::new(), "input_too_large".into());
        }
        if let Self::AgentConfiguration { fields, .. } | Self::Input { fields, .. } = self {
            for field in fields {
                let value = &merged[&field.id];
                let error = if field.required && value.trim().is_empty() {
                    Some("input_required")
                } else if field.kind == FieldKind::Choice
                    && !value.is_empty()
                    && !field.options.iter().any(|c| c.value == *value)
                {
                    Some("invalid_input_choice")
                } else if field.kind == FieldKind::MultiChoice && !valid_multi_choice(field, value)
                {
                    Some("invalid_input_choice")
                } else if field.kind == FieldKind::Json
                    && !value.is_empty()
                    && serde_json::from_str::<Value>(value).is_err()
                {
                    Some("invalid_input_json")
                } else {
                    None
                };
                if let Some(error) = error {
                    errors.insert(field.id.clone(), error.into());
                }
            }
        }
        if errors.is_empty() {
            Ok(merged)
        } else {
            Err(errors)
        }
    }
}
fn valid_multi_choice(field: &Field, value: &str) -> bool {
    let Ok(values) = serde_json::from_str::<Vec<String>>(value) else {
        return false;
    };
    let mut seen = HashSet::new();
    values.len() <= 32
        && values.iter().all(|value| {
            seen.insert(value) && field.options.iter().any(|choice| choice.value == *value)
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn multiple_choices_reject_unknown_or_duplicate_selections() {
        let request = Request::AgentConfiguration {
            creating: true,
            name: String::new(),
            prominent: vec!["permissions".into()],
            fields: vec![Field {
                id: "permissions".into(),
                label: "Permissions".into(),
                kind: FieldKind::MultiChoice,
                default: "[\"one\"]".into(),
                options: vec![Choice {
                    value: "one".into(),
                    label: "A known Agent".into(),
                }],
                ..Default::default()
            }],
        };
        request.validate().unwrap();
        for value in ["[\"unknown\"]", "[\"one\",\"one\"]", "not JSON"] {
            assert!(request
                .validate_values(&[("permissions".into(), value.into())].into())
                .is_err());
        }
        assert!(request
            .validate_values(&[("permissions".into(), "[]".into())].into())
            .is_ok());
    }
    #[test]
    fn business_actions_and_unbound_messages_are_not_interaction_contracts() {
        for action in ["agent.create", "agent.update", "provider.login"] {
            assert!(serde_json::from_value::<Request>(json!({"action":action})).is_err());
        }
        for version in [1, 2] {
            assert!(MessageContent::parse(&json!({"version":version,"request_id":"owner/request","kind":"request","request":{"action":"approval","title":"Apply","description":"Apply changes"}})).is_none());
        }
        assert!(MessageContent::parse(&json!({"version":VERSION,"kind":"request","request":{"action":"approval","title":"Apply","description":"Apply changes"}})).is_none());
    }
    #[test]
    fn form_preserves_defaults_and_rejects_unknown_or_malformed_values() {
        let form = Request::Input {
            title: "Configure".into(),
            fields: vec![Field {
                id: "paths".into(),
                label: "Paths".into(),
                kind: FieldKind::Json,
                default: "[]".into(),
                ..Default::default()
            }],
        };
        assert_eq!(
            form.validate_values(&BTreeMap::new()).unwrap()["paths"],
            "[]"
        );
        assert!(form
            .validate_values(&[("paths".into(), "invalid".into())].into())
            .is_err());
        assert!(form
            .validate_values(&[("other".into(), "x".into())].into())
            .is_err());
    }
}

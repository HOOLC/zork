//! Read-only new-Chat form shared by native clients and component fixtures.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OptionItem {
    pub value: String,
    pub label: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub options: Vec<OptionItem>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub text: String,
    #[serde(default)]
    pub device: Choice,
    pub model: Choice,
    pub thinking: Choice,
    pub profile: Choice,
    pub editable: bool,
    pub can_submit: bool,
    pub busy: bool,
    pub loading: bool,
    pub uncertain: bool,
    pub needs_model: bool,
    pub error: Option<String>,
    pub created: Option<crate::navigation::NavigationChat>,
    pub revoked: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Begin,
    Edit { text: String },
    Model { value: String },
    Thinking { value: String },
    Profile { value: String },
    Submit { text: String },
}

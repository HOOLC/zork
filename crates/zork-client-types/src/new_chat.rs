//! Read-only new-Chat form shared by native clients and component fixtures.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OptionItem {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<crate::device::DeviceStatus>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub options: Vec<OptionItem>,
}
/// One model connection and the models it offers, for a grouped picker.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelGroup {
    pub profile: String,
    pub name: String,
    pub provider: String,
    /// False when the connection cannot be used; `reason` says why.
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub models: Vec<ModelEntry>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    /// Short context size such as `128K`; empty when unknown.
    #[serde(default)]
    pub context: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub text: String,
    #[serde(default)]
    pub device: Choice,
    pub model: Choice,
    pub thinking: Choice,
    pub profile: Choice,
    /// Models grouped by connection; the selected pair is `profile` + `model`.
    #[serde(default)]
    pub groups: Vec<ModelGroup>,
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
    /// Picks a model from one connection in a single step.
    Select { profile: String, model: String },
    Submit { text: String },
}

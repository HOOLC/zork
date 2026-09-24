//! Read-only new-Chat form shared by native clients and component fixtures.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OptionItem {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<crate::device::DeviceStatus>,
    /// For model options: the connections that offer this model, in list order,
    /// so pickers can group models by connection. Empty for other choices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<ConnectionRef>,
    /// For model options: the device the connections are saved on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}
/// A model connection as a picker names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionRef {
    pub profile: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
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
    /// Picks a model from one connection in a single step.
    Select { profile: String, model: String },
    Submit { text: String },
}

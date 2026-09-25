//! Read-only new-Chat form shared by native clients and component fixtures.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OptionItem {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<crate::device::DeviceStatus>,
    /// For model options: the connections that offer this model, in list order;
    /// a picker lists the model once and offers these as optional pins.
    /// For the automatic connection option: the connections it may use for the
    /// current model and thinking (the node picks one per call). Empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<ConnectionRef>,
    /// For connection options: the provider behind the connection, for its
    /// mark. Absent for the automatic choice and for other choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// For model options: the device the connections are saved on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// For model options: who made the model (`deepseek`, `qwen`, …), whichever
    /// connection serves it. Absent when unknown: pickers draw a generic mark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maker: Option<String>,
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
    /// The connection: `auto` (default) or a pinned profile id. Options are
    /// `auto` first, then only the connections that serve the current model.
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
    /// Picks a model. A pinned connection stays while it serves the model;
    /// otherwise the connection returns to automatic.
    Model { value: String },
    Thinking { value: String },
    /// Pins a connection serving the current model, or `auto`.
    Profile { value: String },
    /// Picks a model from one connection in a single step.
    Select { profile: String, model: String },
    Submit { text: String },
}

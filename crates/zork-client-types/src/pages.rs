//! Explicit human-facing page entries. Sharing a service does not publish a page.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageLink {
    pub id: String,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationPage {
    /// Reference identity, distinct from the page identity and message identity.
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub page: PageLink,
    #[serde(default)]
    pub source_session_id: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Application {
    pub page: PageLink,
    pub owner_session_id: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageCatalog {
    #[serde(default)]
    pub references: Vec<ConversationPage>,
    #[serde(default)]
    pub applications: Vec<Application>,
    #[serde(default)]
    pub files: Vec<ConversationFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationFile {
    pub id: String,
    pub session_id: String,
    pub artifact_id: String,
    pub source_session_id: String,
}

/// Transport context for deliberate page delivery, including task handoff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveredPage {
    pub page: PageLink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationEntry {
    pub device_status: crate::device::DeviceStatus,
    pub page: PageLink,
    pub device_id: String,
    pub device_name: String,
    pub offline: bool,
}

//! Core-projected conversation navigation entries.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NavigationChat {
    #[serde(default)]
    pub archived: bool,
    /// Message watermark observed by the UI when submitting an archive intent.
    #[serde(default)]
    pub message_count: u64,
    #[serde(default)]
    pub archive_pending: bool,
    #[serde(default)]
    pub archive_error: Option<String>,
    pub chat_id: String,
    pub title: String,
    pub description: String,
    pub workspace: String,
    pub updated_at: String,
    pub unread: bool,
    pub can_send: bool,
    pub can_stop: bool,
    pub executor: Option<String>,
    /// Session model shown under the Chat. Empty when the node has not reported one.
    #[serde(default)]
    pub model: String,
    /// Recent or unread; platforms may expand the group and retain selection.
    pub in_preview: bool,
}

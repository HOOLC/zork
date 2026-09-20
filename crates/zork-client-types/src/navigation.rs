//! Core-projected conversation navigation entries.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NavigationChat {
    pub chat_id: String,
    pub title: String,
    pub description: String,
    pub workspace: String,
    pub updated_at: String,
    pub unread: bool,
    pub can_send: bool,
    pub can_stop: bool,
    pub executor: Option<String>,
    /// Recent or unread; platforms may expand the group and retain selection.
    pub in_preview: bool,
}

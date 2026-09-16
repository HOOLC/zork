//! Core-projected conversation navigation entries.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NavigationAgent {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub instructions: String,
    pub profile_id: String,
    pub model: String,
    pub session_id: Option<String>,
    /// A creator-only group must not allocate a new long-term home on click.
    pub can_open: bool,
    pub unread: bool,
}

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

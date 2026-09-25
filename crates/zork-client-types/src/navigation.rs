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
    /// Stacked avatars of the Chat's Agents (sidebar row, Chat header).
    #[serde(default)]
    pub avatar: ChatAvatar,
}

/// One Agent's avatar: its identity tint disc with the model maker's mark
/// inside, or the initial when no model is known.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAvatar {
    pub agent_id: String,
    /// `model_catalog::MAKERS` key; `generic` for a known but unrecognized
    /// model; `None` when no model is known (draw `initial`).
    #[serde(default)]
    pub maker: Option<String>,
    /// `AGENT_TINTS` slot (see `agent_tint`).
    pub tint: usize,
    pub initial: String,
}

/// A Chat's avatar: up to three Agent avatars overlapping in first-appearance
/// order, then a "+N" disc for the rest. Empty for a Chat without Agent
/// authors (or from an older Station): platforms draw their plain Chat mark.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatAvatar {
    #[serde(default)]
    pub agents: Vec<AgentAvatar>,
    /// Agents beyond those shown; draw "+N" when non-zero.
    #[serde(default)]
    pub more: u64,
}

/// The device a Chat lives on. A chat-list row has two lines (title, then a
/// muted meta line): a remote Chat's meta line shows the display name and
/// " · " before its time ("A · 刚刚", tooltip "在设备 A 上"); a `local` Chat
/// (this client's own Station) shows only the time. `color_key` stays available for device marks elsewhere.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatDevice {
    /// Directory id of the device (the navigation `peer`).
    pub id: String,
    /// Mesh display name (A, B, C…).
    pub name: String,
    /// Registered machine name when it differs, for hints.
    #[serde(default)]
    pub machine: Option<String>,
    /// Stable hue key for `device_hue` (join order or identity; falls back to
    /// the name).
    pub color_key: String,
    pub local: bool,
}

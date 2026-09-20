//! Client-owned observable state. Business revisions and notification filtering
//! live here; platform adapters only deliver a subscription to its view.
#[cfg(not(target_family = "wasm"))]
mod artifacts;
#[cfg(not(target_family = "wasm"))]
mod device;
#[cfg(not(target_family = "wasm"))]
mod navigation;
#[cfg(not(target_family = "wasm"))]
pub use navigation::{NavigationChat, NavigationData};
#[cfg(not(target_family = "wasm"))]
mod upgrade;
#[cfg(not(target_family = "wasm"))]
pub(crate) use upgrade::upgrade_status;
mod agent_catalog;
#[cfg(not(target_family = "wasm"))]
mod agents;
#[cfg(not(target_family = "wasm"))]
mod drafts;
#[cfg(not(target_family = "wasm"))]
mod new_chat;
#[cfg(not(target_family = "wasm"))]
mod outbox;
#[cfg(not(target_family = "wasm"))]
pub use new_chat::{NewChat, NewChatData};
#[cfg(not(target_family = "wasm"))]
mod replication;
pub use agent_catalog::{AgentData, AgentSubscription, AgentUpdate, Agents};
#[cfg(not(target_family = "wasm"))]
pub use outbox::Outbox;
#[cfg(not(target_family = "wasm"))]
mod conversation;
#[cfg(not(target_family = "wasm"))]
mod message_activity;
#[cfg(not(target_family = "wasm"))]
pub use message_activity::{MessageActivity, MessageArrivals};
#[cfg(not(target_family = "wasm"))]
mod history;
#[cfg(not(target_family = "wasm"))]
mod overview;
#[cfg(not(target_family = "wasm"))]
pub use conversation::{
    Conversation, ConversationData, ConversationSubscription, ConversationTopics,
    ConversationUpdate, DeliveryState, MessageDeliveries, MessageSplice, TranscriptLookup,
};
#[cfg(not(target_family = "wasm"))]
pub use device::{Device, DeviceData, DeviceSubscription, DeviceUpdate, Domains};
#[cfg(not(target_family = "wasm"))]
pub use drafts::{Draft, DraftAction, TEXT_ATTACHMENT_LIMIT};
#[cfg(not(target_family = "wasm"))]
pub use history::{
    History, HistoryData, HistoryEntries, HistoryLookup, HistoryRuntime, HistorySubscription,
    HistoryUpdate,
};
#[cfg(not(target_family = "wasm"))]
pub use overview::{InitialSessionSnapshot, SessionOverview, SessionSnapshot};

mod observable;
pub(crate) use observable::Observable;
pub use observable::Subscription;
mod profiles;
pub use profiles::{ProfileData, ProfileSubscription, ProfileUpdate, Profiles};

#[cfg(not(target_family = "wasm"))]
mod files;
#[cfg(not(target_family = "wasm"))]
mod mesh_admin;
#[cfg(not(target_family = "wasm"))]
pub use mesh_admin::{MeshAction, MeshAdmin, MeshAdminData};

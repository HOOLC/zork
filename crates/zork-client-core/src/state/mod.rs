//! Client-owned observable state. Business revisions and notification filtering
//! live here; platform adapters only deliver a subscription to its view.
mod artifacts;
mod device;
mod navigation;
pub use navigation::{NavigationChat, NavigationData};
mod upgrade;
pub(crate) use upgrade::upgrade_status;
mod agent_catalog;
mod agents;
mod drafts;
mod new_chat;
mod outbox;
pub use new_chat::{NewChat, NewChatData};
mod replication;
pub use agent_catalog::{AgentData, AgentSubscription, AgentUpdate, Agents};
pub use outbox::{Outbox, EMPTY_COMMENT_REPLY};
mod conversation;
mod message_activity;
pub use message_activity::{MessageActivity, MessageArrivals};
mod history;
mod overview;
pub use conversation::{
    Conversation, ConversationData, ConversationSubscription, ConversationTopics,
    ConversationUpdate, DeliveryState, MessageDeliveries, MessageSplice, TranscriptLookup,
};
pub use device::{Device, DeviceData, DeviceSubscription, DeviceUpdate, Domains};
pub use drafts::{Draft, DraftAction, TEXT_ATTACHMENT_LIMIT};
pub use history::{
    History, HistoryData, HistoryEntries, HistoryLookup, HistoryRuntime, HistorySubscription,
    HistoryUpdate,
};
pub use overview::{InitialSessionSnapshot, SessionOverview, SessionSnapshot};

mod observable;
pub(crate) use observable::Observable;
pub use observable::Subscription;
mod profiles;
pub use profiles::{ProfileData, ProfileSubscription, ProfileUpdate, Profiles};

mod files;
mod mesh_admin;
pub use mesh_admin::{MeshAction, MeshAdmin, MeshAdminData};

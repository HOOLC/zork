//! Public channels and an Agent's own receiving preferences for each channel.
//! Participation is derived from authorship; a preference is not membership.
use serde::{Deserialize, Serialize};

/// Maximum UTF-8 bytes in Chat text, excluding the file-reference envelope.
pub const MAX_MESSAGE_TEXT_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageFilter {
    #[default]
    All,
    Mentions,
    Replies,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    #[default]
    Immediate,
    OnNextTurn,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    #[serde(default)]
    pub subscribed: bool,
    #[serde(default)]
    pub filter: MessageFilter,
    #[serde(default)]
    pub delivery: DeliveryMode,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreferenceChanges {
    pub subscribed: Option<bool>,
    pub filter: Option<MessageFilter>,
    pub delivery: Option<DeliveryMode>,
}
impl PreferenceChanges {
    pub fn is_empty(&self) -> bool {
        self.subscribed.is_none() && self.filter.is_none() && self.delivery.is_none()
    }

    pub fn apply(&self, previous: &Preferences) -> Preferences {
        Preferences {
            subscribed: self.subscribed.unwrap_or(previous.subscribed),
            filter: self.filter.unwrap_or(previous.filter),
            delivery: self.delivery.unwrap_or(previous.delivery),
            revision: previous.revision,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StartAt {
    Now,
    After { message_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePreferences {
    pub changes: PreferenceChanges,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    #[serde(default)]
    pub start: Option<StartAt>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub id: String,
    pub kind: AuthorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorKind {
    User,
    Agent,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub author: Author,
    pub subscribed: bool,
    pub message_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    pub chat_id: String,
    pub title: String,
    pub created_at: String,
    pub message_count: u64,
    /// Authenticated creation provenance, independent of authors and subscribers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<Author>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub message_id: String,
    pub chat_id: String,
    pub author: Author,
    /// Origin-qualified client connection that submitted this user message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    pub text: String,
    pub attachments: Vec<crate::files::FileRef>,
    pub mentions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction: Option<serde_json::Value>,
    pub created_at: String,
}

impl Preferences {
    /// Delivery policy never changes authorship, reading permission or posting.
    /// Own output must not wake the same Agent through a channel subscription.
    pub fn receives(&self, agent: &str, message: &Message, reply_author: Option<&str>) -> bool {
        if !self.subscribed
            || (message.author.kind == AuthorKind::Agent && message.author.id == agent)
        {
            return false;
        }
        match self.filter {
            MessageFilter::All => true,
            MessageFilter::Mentions => message.mentions.iter().any(|id| id == agent),
            MessageFilter::Replies => reply_author == Some(agent),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_rules_are_independent_and_do_not_echo_own_output() {
        let mut message = Message {
            client_id: None,
            message_id: "message".into(),
            chat_id: "channel".into(),
            author: Author {
                id: "writer".into(),
                kind: AuthorKind::Agent,
                name: None,
            },
            text: "hello".into(),
            attachments: vec![],
            mentions: vec!["reader".into()],
            reply_to: None,
            interaction: None,
            created_at: "time".into(),
        };
        let mut preferences = Preferences {
            subscribed: true,
            ..Default::default()
        };
        assert!(preferences.receives("reader", &message, None));
        assert!(!preferences.receives("writer", &message, None));
        preferences.filter = MessageFilter::Mentions;
        assert!(preferences.receives("reader", &message, None));
        assert!(!preferences.receives("other", &message, None));
        preferences.filter = MessageFilter::Replies;
        assert!(!preferences.receives("reader", &message, None));
        assert!(preferences.receives("reader", &message, Some("reader")));
        preferences.subscribed = false;
        assert!(!preferences.receives("reader", &message, Some("reader")));
        message.author.kind = AuthorKind::User;
        preferences.subscribed = true;
        preferences.filter = MessageFilter::All;
        assert!(preferences.receives("writer", &message, None));
    }

    #[test]
    fn patches_preserve_other_settings_and_cannot_supply_another_agent() {
        let previous = Preferences {
            subscribed: true,
            filter: MessageFilter::Replies,
            delivery: DeliveryMode::OnNextTurn,
            revision: 7,
        };
        let changes: PreferenceChanges = serde_json::from_str(r#"{"subscribed":false}"#).unwrap();
        assert_eq!(
            changes.apply(&previous),
            Preferences {
                subscribed: false,
                ..previous
            }
        );
        assert!(serde_json::from_str::<UpdatePreferences>(
            r#"{"agent_id":"another","changes":{"subscribed":true}}"#
        )
        .is_err());
    }
}

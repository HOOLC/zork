//! Public channels and an Agent's own receiving preferences for each channel.
//! Participation is derived from authorship; a preference is not membership.
use serde::{Deserialize, Serialize};

mod quote;
pub use quote::{
    lenient_kind, validate_quote, MessageQuote, QuoteKind, MAX_QUOTE_CHARS, SHORT_ORIGINAL_CHARS,
};

/// Maximum UTF-8 bytes in Chat text, excluding the file-reference envelope.
pub const MAX_MESSAGE_TEXT_BYTES: usize = 32 * 1024;

/// One explicit first send. request_id identifies the entire creation operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartChat {
    pub request_id: String,
    pub content: String,
    pub model: String,
    pub thinking: String,
    #[serde(default)]
    pub profile_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
}

impl StartChat {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.request_id.len() != 26
            || !self.request_id.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err("invalid_request_id");
        }
        if self.content.trim().is_empty() {
            return Err("empty_message");
        }
        if self.content.len() > MAX_MESSAGE_TEXT_BYTES {
            return Err("message_too_large_submit_as_file");
        }
        if self.model.trim().is_empty() || self.thinking.trim().is_empty() {
            return Err("selection_required");
        }
        if self.model.len() > 512 || self.thinking.len() > 64 || self.profile_id.len() > 512 {
            return Err("invalid_selection");
        }
        if self.title.as_ref().is_some_and(|s| s.len() > 512) {
            return Err("invalid_chat_title");
        }
        Ok(())
    }
}

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
    #[serde(default)]
    pub archived: bool,
    pub chat_id: String,
    pub title: String,
    pub created_at: String,
    pub message_count: u64,
    /// Authenticated creation provenance, independent of authors and subscribers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<Author>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<String>,
    /// The first Agent authors in order of first appearance (at most
    /// [`CHAT_AVATAR_AGENTS`]` + 1`), for the Chat avatar. Absent from older
    /// Stations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<ChatAgent>,
    /// All Agent authors of the Chat, for the "+N" disc.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub agent_count: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// Agents shown in a Chat avatar before the "+N" disc.
pub const CHAT_AVATAR_AGENTS: usize = 3;

/// An Agent author of a Chat as its avatar needs it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatAgent {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Model of its latest recorded message in this Chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
    /// The replying author's quote of the part it answers (see [`quote`]).
    /// Only meaningful with `reply_to`; absent on old records and peers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// `excerpt` (verbatim) or `summary`; absent means excerpt. Unknown kinds
    /// from newer peers read as absent.
    #[serde(
        default,
        deserialize_with = "lenient_kind",
        skip_serializing_if = "Option::is_none"
    )]
    pub quote_kind: Option<QuoteKind>,
    /// Model the authoring Agent ran when it posted, recorded by the Station
    /// that accepted the post (absent for users, remote authors, old records).
    /// Clients draw the model maker's mark in the author's identity disc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction: Option<serde_json::Value>,
    pub created_at: String,
}

impl Message {
    /// The reply quote, when this message carries one.
    pub fn reply_quote(&self) -> Option<MessageQuote> {
        MessageQuote::from_fields(self.quote.as_deref(), self.quote_kind)
    }
    /// Stores a validated quote (or clears it).
    pub fn set_quote(&mut self, quote: Option<MessageQuote>) {
        self.quote_kind = quote.as_ref().map(|q| q.kind);
        self.quote = quote.map(|q| q.text);
    }
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
            quote: None,
            quote_kind: None,
            author_model: None,
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

    #[test]
    fn quote_fields_are_optional_on_the_wire() {
        let legacy = r#"{"message_id":"m","chat_id":"c","author":{"id":"a","kind":"agent"},
            "text":"hi","attachments":[],"mentions":[],"reply_to":"o","created_at":"t"}"#;
        let message: Message = serde_json::from_str(legacy).unwrap();
        assert_eq!((message.quote.as_deref(), message.quote_kind), (None, None));
        let encoded = serde_json::to_value(&message).unwrap();
        assert!(encoded.get("quote").is_none() && encoded.get("quote_kind").is_none());

        let mut quoted = message.clone();
        quoted.set_quote(Some(MessageQuote {
            text: "对比度只有 3.9:1".into(),
            kind: QuoteKind::Summary,
        }));
        let encoded = serde_json::to_value(&quoted).unwrap();
        assert_eq!(encoded["quote"], "对比度只有 3.9:1");
        assert_eq!(encoded["quote_kind"], "summary");
        let decoded: Message = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded, quoted);
        assert_eq!(decoded.reply_quote().unwrap().kind, QuoteKind::Summary);

        // A kind from a newer peer does not reject the message.
        let mut future = encoded;
        future["quote_kind"] = serde_json::json!("paraphrase");
        let decoded: Message = serde_json::from_value(future).unwrap();
        assert_eq!(decoded.quote_kind, None);
        assert_eq!(decoded.reply_quote().unwrap().kind, QuoteKind::Excerpt);
    }
}

//! Agent reply quotes: the part of the original message a reply answers.
//!
//! A reply (`reply_to`) to a longer message may carry `quote` with
//! `quote_kind`: an `excerpt` is a short passage copied verbatim from the
//! original, a `summary` a short paraphrase of it. Both are presentation data
//! written by the replying author; they never change delivery or permissions.
//! Old Stations, peers and clients simply lack the fields.
use serde::{Deserialize, Deserializer, Serialize};

/// Longest accepted quote, in characters (Unicode scalar values).
pub const MAX_QUOTE_CHARS: usize = 120;
/// Originals up to this many characters are shown verbatim by clients, so a
/// reply to them needs no quote.
pub const SHORT_ORIGINAL_CHARS: usize = 10;

/// How a quote relates to the original text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteKind {
    /// Copied exactly from the original; clients can mark the passage.
    #[default]
    Excerpt,
    /// A short paraphrase; clients show it with a "大意" pill.
    Summary,
}

impl QuoteKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "excerpt" => Some(Self::Excerpt),
            "summary" => Some(Self::Summary),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Excerpt => "excerpt",
            Self::Summary => "summary",
        }
    }
}

/// A validated reply quote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageQuote {
    pub text: String,
    pub kind: QuoteKind,
}

impl MessageQuote {
    /// The quote stored on a message: `kind` absent means an excerpt.
    pub fn from_fields(text: Option<&str>, kind: Option<QuoteKind>) -> Option<Self> {
        text.filter(|text| !text.trim().is_empty())
            .map(|text| Self {
                text: text.to_owned(),
                kind: kind.unwrap_or_default(),
            })
    }
}

/// Validates the optional quote fields of a new message. Returns the trimmed
/// quote, or an error code (`snake_case`, safe to return to callers):
///
/// - `quote_requires_reply_to`: a quote only describes a reply target;
/// - `quote_kind_requires_quote`: a kind without a quote;
/// - `invalid_quote`: empty after trimming;
/// - `quote_too_long`: more than [`MAX_QUOTE_CHARS`] characters;
/// - `invalid_quote_kind`: not `excerpt` or `summary`.
pub fn validate_quote(
    reply_to: Option<&str>,
    quote: Option<&str>,
    kind: Option<&str>,
) -> Result<Option<MessageQuote>, &'static str> {
    let Some(quote) = quote else {
        return match kind {
            Some(_) => Err("quote_kind_requires_quote"),
            None => Ok(None),
        };
    };
    if reply_to.is_none_or(|id| id.trim().is_empty()) {
        return Err("quote_requires_reply_to");
    }
    let text = quote.trim();
    if text.is_empty() {
        return Err("invalid_quote");
    }
    if text.chars().count() > MAX_QUOTE_CHARS {
        return Err("quote_too_long");
    }
    let kind = match kind {
        None => QuoteKind::Excerpt,
        Some(kind) => QuoteKind::parse(kind).ok_or("invalid_quote_kind")?,
    };
    Ok(Some(MessageQuote {
        text: text.to_owned(),
        kind,
    }))
}

/// Reads `quote_kind` leniently: a kind this version does not know (from a
/// newer peer) becomes `None` instead of rejecting the whole message.
pub fn lenient_kind<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<QuoteKind>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?
        .as_deref()
        .and_then(QuoteKind::parse))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rules_and_error_codes() {
        let ok = validate_quote(Some("m1"), Some("  密码框缺少切换  "), None).unwrap();
        assert_eq!(
            ok,
            Some(MessageQuote {
                text: "密码框缺少切换".into(),
                kind: QuoteKind::Excerpt
            })
        );
        assert_eq!(
            validate_quote(Some("m1"), Some("大意"), Some("summary"))
                .unwrap()
                .unwrap()
                .kind,
            QuoteKind::Summary
        );
        assert_eq!(validate_quote(None, None, None), Ok(None));
        assert_eq!(validate_quote(Some("m1"), None, None), Ok(None));
        assert_eq!(
            validate_quote(Some("m1"), None, Some("excerpt")),
            Err("quote_kind_requires_quote")
        );
        assert_eq!(
            validate_quote(None, Some("text"), None),
            Err("quote_requires_reply_to")
        );
        assert_eq!(
            validate_quote(Some(" "), Some("text"), None),
            Err("quote_requires_reply_to")
        );
        assert_eq!(
            validate_quote(Some("m1"), Some("   "), None),
            Err("invalid_quote")
        );
        assert_eq!(
            validate_quote(Some("m1"), Some("text"), Some("verbatim")),
            Err("invalid_quote_kind")
        );
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        // 120 CJK characters are 360 bytes and still fit.
        let cjk = "字".repeat(MAX_QUOTE_CHARS);
        assert!(validate_quote(Some("m"), Some(&cjk), None).is_ok());
        let longer = "字".repeat(MAX_QUOTE_CHARS + 1);
        assert_eq!(
            validate_quote(Some("m"), Some(&longer), None),
            Err("quote_too_long")
        );
        // Surrounding whitespace does not count.
        let padded = format!("  {cjk}\n");
        assert!(validate_quote(Some("m"), Some(&padded), None).is_ok());
    }

    #[test]
    fn stored_fields_default_to_excerpt() {
        assert_eq!(
            MessageQuote::from_fields(Some("原文"), None).unwrap().kind,
            QuoteKind::Excerpt
        );
        assert_eq!(MessageQuote::from_fields(Some(" "), None), None);
        assert_eq!(
            MessageQuote::from_fields(None, Some(QuoteKind::Summary)),
            None
        );
    }
}

//! What a reply line shows of the message it answers.
use serde::Serialize;
use zork_client_types::chat::{MessageQuote, QuoteKind, MAX_QUOTE_CHARS, SHORT_ORIGINAL_CHARS};

/// Where the shown quote text comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteSource {
    /// The whole original (≤ [`SHORT_ORIGINAL_CHARS`] characters), in 「」.
    Original,
    /// The replying author's verbatim excerpt, in 「」.
    Excerpt,
    /// The replying author's paraphrase, after a "大意" pill.
    Summary,
    /// A legacy reply without a quote: the start of the original, no marks.
    Fallback,
}

/// The quote part of a reply line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QuoteContent {
    pub source: QuoteSource,
    /// One line of text; the UI ellipsizes it to the available width and
    /// adds 「」 for `original`/`excerpt` or the pill for `summary`.
    pub text: String,
    /// Full text for the tooltip (the UI prefixes "大意：" for a summary);
    /// `None` when the line already shows everything (`original`).
    pub tooltip: Option<String>,
    /// Passage to mark in the original after jumping; `None` washes the
    /// whole message instead.
    pub mark: Option<String>,
}

/// Selects the quote content of a reply to `original` (the target's
/// quotable text, see `zork_client_types::comments::quotable_text`):
///
/// 1. an original of at most [`SHORT_ORIGINAL_CHARS`] characters (Unicode
///    scalar values of the trimmed text, so each CJK character counts one)
///    is shown verbatim;
/// 2. otherwise the reply's own quote: an excerpt, or a summary;
/// 3. otherwise (older replies) the start of the original.
pub fn quote_content(original: &str, quote: Option<&MessageQuote>) -> QuoteContent {
    let trimmed = original.trim();
    if trimmed.chars().count() <= SHORT_ORIGINAL_CHARS {
        return QuoteContent {
            source: QuoteSource::Original,
            text: one_line(trimmed),
            tooltip: None,
            mark: None,
        };
    }
    if let Some(quote) = quote {
        let text = one_line(quote.text.trim());
        return match quote.kind {
            QuoteKind::Excerpt => QuoteContent {
                source: QuoteSource::Excerpt,
                tooltip: Some(text.clone()),
                // Marking uses the stored passage exactly as written.
                mark: Some(quote.text.trim().to_owned()),
                text,
            },
            QuoteKind::Summary => QuoteContent {
                source: QuoteSource::Summary,
                tooltip: Some(text.clone()),
                mark: None,
                text,
            },
        };
    }
    let text = fallback_excerpt(trimmed);
    QuoteContent {
        source: QuoteSource::Fallback,
        tooltip: Some(text.clone()),
        mark: None,
        text,
    }
}

/// The start of an original as one line: numbered-list breaks and other
/// line breaks become " · ", runs of spaces collapse, and the result is cut
/// at [`MAX_QUOTE_CHARS`] characters (the UI ellipsizes further).
pub fn fallback_excerpt(original: &str) -> String {
    let lines: Vec<String> = original
        .lines()
        .map(|line| {
            let line = line.trim();
            let digits = line.chars().take_while(char::is_ascii_digit).count();
            match line[digits..].strip_prefix(". ") {
                Some(rest) if digits > 0 => rest.trim().to_owned(),
                _ => line.to_owned(),
            }
        })
        .filter(|line| !line.is_empty())
        .collect();
    let joined = one_line(&lines.join(" · "));
    if joined.chars().count() <= MAX_QUOTE_CHARS {
        joined
    } else {
        joined.chars().take(MAX_QUOTE_CHARS).collect()
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

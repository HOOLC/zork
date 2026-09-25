//! Transcript presentation from core `message_presentation::present`.
//!
//! Core owns every rule (groups, time labels, tints, identity, quote content,
//! the "only own messages in between" part of the omission rule, comment
//! pairs). The desktop app and the component stories both pass core's JSON
//! (the same shape Android's bridge returns) through these read-only mirrors,
//! so the two draw from one mapping. The UI adds only layout: the "within one
//! laid-out screen" measurement, hover, scrolling and the jump wash.
use super::identity::{AgentIdentity, Disc, QuoteAuthor, QuoteKind, QuoteText, Reply};
use serde::Deserialize;
use std::{collections::HashMap, ops::Range};
use zork_client_types::comments::CommentPair;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Transcript {
    #[serde(default)]
    pub rows: Vec<Row>,
    #[serde(default)]
    pub multi_device: bool,
    #[serde(default)]
    pub next_change_ms: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Row {
    pub id: Option<String>,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub user: bool,
    #[serde(default)]
    pub group_head: bool,
    #[serde(default)]
    pub group_tail: bool,
    pub time: Option<Time>,
    pub identity: Option<Identity>,
    pub reply: Option<ReplyLine>,
    pub comments: Option<Comments>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimePlacement {
    Head,
    Tail,
    #[default]
    Hover,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Time {
    pub label: String,
    pub full: String,
    #[serde(default)]
    pub placement: TimePlacement,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Author {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub user: bool,
    pub agent_id: Option<String>,
    pub tint: Option<usize>,
    pub initial: Option<String>,
    pub maker: Option<String>,
}
impl Author {
    pub fn disc(&self) -> Option<Disc> {
        self.tint.map(|tint| Disc {
            tint,
            maker: self.maker.clone(),
            initial: self.initial.clone().unwrap_or_else(|| "?".into()),
        })
    }
    pub fn quote_author(&self) -> QuoteAuthor {
        QuoteAuthor {
            name: self.name.clone(),
            disc: self.disc(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Identity {
    #[serde(flatten)]
    pub author: Author,
    pub device: Option<String>,
    pub device_name: Option<String>,
    pub machine: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub detail: String,
}
impl Identity {
    /// `show_device`: core's `multi_device` and not phone width.
    pub fn agent_identity(&self, show_device: bool) -> AgentIdentity {
        AgentIdentity {
            name: self.author.name.clone(),
            disc: self.author.disc().unwrap_or_else(|| Disc {
                tint: 0,
                maker: None,
                initial: self.author.initial.clone().unwrap_or_else(|| "?".into()),
            }),
            device: self.device_name.clone().filter(|_| show_device),
            detail: self.detail.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    #[default]
    Linked,
    NotLoaded,
    Deleted,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplyPlacement {
    InHead,
    #[default]
    AboveBody,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuoteSource {
    Original,
    Excerpt,
    Summary,
    #[default]
    Fallback,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct QuoteContent {
    #[serde(default)]
    pub source: QuoteSource,
    #[serde(default)]
    pub text: String,
    pub tooltip: Option<String>,
    /// Passage to mark in the original after jumping; `None` washes the
    /// whole message.
    pub mark: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ReplyLine {
    pub target_id: String,
    #[serde(default)]
    pub state: TargetState,
    pub target_index: Option<usize>,
    pub target: Option<Author>,
    pub content: Option<QuoteContent>,
    #[serde(default)]
    pub placement: ReplyPlacement,
    #[serde(default)]
    pub own_run: bool,
}
impl ReplyLine {
    /// The drawable line. `loading`: older history is loading after a click
    /// on this line.
    pub fn reply(&self, loading: bool) -> Reply {
        match self.state {
            TargetState::Linked => Reply::Linked {
                author: self
                    .target
                    .as_ref()
                    .map(Author::quote_author)
                    .unwrap_or(QuoteAuthor {
                        name: String::new(),
                        disc: None,
                    }),
                quote: self
                    .content
                    .as_ref()
                    .map(|content| QuoteText {
                        kind: match content.source {
                            QuoteSource::Original => QuoteKind::Original,
                            QuoteSource::Excerpt => QuoteKind::Excerpt,
                            QuoteSource::Summary => QuoteKind::Summary,
                            QuoteSource::Fallback => QuoteKind::Fallback,
                        },
                        text: content.text.clone(),
                        tooltip: content.tooltip.clone(),
                    })
                    .unwrap_or(QuoteText {
                        kind: QuoteKind::Fallback,
                        text: String::new(),
                        tooltip: None,
                    }),
            },
            TargetState::NotLoaded if loading => Reply::Loading,
            TargetState::NotLoaded => Reply::NotLoaded,
            TargetState::Deleted => Reply::Deleted {
                author: self.target.as_ref().map(Author::quote_author),
            },
        }
    }
    /// The passage to mark after jumping.
    pub fn mark(&self) -> Option<String> {
        self.content
            .as_ref()
            .and_then(|content| content.mark.clone())
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct CommentLine {
    #[serde(flatten)]
    pub pair: CommentPair,
    #[serde(default)]
    pub source: Author,
    #[serde(default)]
    pub state: TargetState,
    pub source_index: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Comments {
    #[serde(default)]
    pub pairs: Vec<CommentLine>,
    #[serde(default)]
    pub extra_text: String,
}

/// Parses core's presentation JSON (`message_presentation::handle` output or
/// a serialized `TranscriptPresentation`).
pub fn parse(value: serde_json::Value) -> Transcript {
    serde_json::from_value(value).unwrap_or_default()
}

/// The UI half of the omission rule: core said only this author's own
/// messages lie between target and reply (`own_run`); the line is omitted
/// only when the original also starts within one laid-out screen above the
/// reply. `heights` are the laid-out (or estimated) heights of the rows from
/// the target up to, not including, the reply. Scroll position never enters.
pub fn omit_reply_line(own_run: bool, heights: impl IntoIterator<Item = f32>, screen: f32) -> bool {
    if !own_run || screen <= 0. {
        return false;
    }
    let mut distance = 0.;
    for height in heights {
        distance += height.max(0.);
        if distance > screen {
            return false;
        }
    }
    true
}

/// Applies [`omit_reply_line`] to row `at`: the heights of rows from its
/// reply target up to `at` come from `height_of(index)`.
pub fn omit_row_reply(
    rows: &[Row],
    at: usize,
    height_of: impl Fn(usize) -> f32,
    screen: f32,
) -> bool {
    let Some(reply) = rows.get(at).and_then(|row| row.reply.as_ref()) else {
        return false;
    };
    let Some(target) = reply.target_index.filter(|target| *target < at) else {
        return false;
    };
    omit_reply_line(reply.own_run, (target..at).map(height_of), screen)
}

/// First-appearance agent avatars of a presented transcript (the Chat header
/// stack when the host has no navigation row for it): up to three, then the
/// count of the rest.
pub fn agent_stack(transcript: &Transcript) -> (Vec<Disc>, u64) {
    let mut seen: Vec<String> = Vec::new();
    let mut discs = Vec::new();
    for identity in transcript
        .rows
        .iter()
        .filter_map(|row| row.identity.as_ref())
    {
        let key = identity
            .author
            .agent_id
            .clone()
            .unwrap_or_else(|| identity.author.name.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if discs.len() < 3 {
            if let Some(disc) = identity.author.disc() {
                discs.push(disc);
            }
        }
    }
    let more = seen.len().saturating_sub(discs.len()) as u64;
    (discs, more)
}

/// Rough laid-out height of a message for rows that were never rendered:
/// paragraphs wrapped at `width` with CJK and Latin advance estimates, the
/// 20 px line box, block gaps, and the identity row when it heads a group.
pub fn estimate_height(text: &str, width: f32, head: bool, user: bool) -> f32 {
    let width = if user {
        (width * 0.78).max(80.) - 36.
    } else {
        width.max(80.)
    };
    let mut lines: f32 = 0.;
    let mut blocks: f32 = 0.;
    for paragraph in text.split('\n') {
        let advance: f32 = paragraph
            .chars()
            .map(|c| if c.is_ascii() { 7.2 } else { 13. })
            .sum();
        if paragraph.trim().is_empty() {
            blocks += 0.5;
            continue;
        }
        lines += (advance / width).ceil().max(1.);
        blocks += 1.;
    }
    let body = lines * 20. + (blocks - 1.).max(0.) * 6.;
    let chrome = if user { 20. + 14. } else { 14. };
    body + chrome + if head && !user { 24. } else { 0. }
}

/// Laid-out row heights by message id at one column width. Rows record their
/// height when painted; unrendered rows fall back to [`estimate_height`].
/// Changing the width clears it, so the omission rule re-evaluates on resize.
#[derive(Default, Debug)]
pub struct RowHeights {
    width: f32,
    heights: HashMap<String, f32>,
}
impl RowHeights {
    /// Returns true when the height changed enough to matter.
    pub fn record(&mut self, id: &str, width: f32, height: f32) -> bool {
        if (self.width - width).abs() > 0.5 {
            self.width = width;
            self.heights.clear();
        }
        match self.heights.insert(id.to_owned(), height) {
            Some(old) => (old - height).abs() > 0.5,
            None => true,
        }
    }
    pub fn get(&self, id: &str, width: f32) -> Option<f32> {
        ((self.width - width).abs() <= 0.5)
            .then(|| self.heights.get(id).copied())
            .flatten()
    }
}

/// Byte range of `passage` in a message's text, for the jump wash and draft
/// marks: the exact passage, else the trimmed passage.
pub fn find_passage(text: &str, passage: &str) -> Option<Range<usize>> {
    let passage = passage.trim();
    if passage.is_empty() {
        return None;
    }
    text.find(passage).map(|start| start..start + passage.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_core_presentation_json() {
        let parsed = parse(json!({
            "rows": [{
                "id": "m2", "author": "agent:builder", "user": false,
                "group_head": true, "group_tail": true,
                "time": {"label": "刚刚", "full": "2026年9月26日 周六 14:30:00", "placement": "head"},
                "identity": {"name": "Builder", "user": false, "agent_id": "builder", "tint": 2,
                    "initial": "B", "maker": "deepseek", "device": "o1", "device_name": "B",
                    "machine": null, "model": "deepseek-flash", "detail": "Builder · B · deepseek-flash"},
                "reply": {"target_id": "m1", "state": "linked", "target_index": 0,
                    "target": {"name": "你", "user": true, "agent_id": null, "tint": null, "initial": null, "maker": null},
                    "content": {"source": "summary", "text": "错误文案要具体", "tooltip": "错误文案要具体", "mark": null},
                    "placement": "in_head", "own_run": false},
                "comments": null
            }],
            "multi_device": true, "next_change_ms": 30000
        }));
        assert!(parsed.multi_device);
        let row = &parsed.rows[0];
        assert_eq!(row.time.as_ref().unwrap().placement, TimePlacement::Head);
        let identity = row.identity.as_ref().unwrap().agent_identity(true);
        assert_eq!(identity.disc.maker.as_deref(), Some("deepseek"));
        assert_eq!(identity.device.as_deref(), Some("B"));
        let reply = row.reply.as_ref().unwrap();
        assert_eq!(reply.placement, ReplyPlacement::InHead);
        match reply.reply(false) {
            Reply::Linked { author, quote } => {
                assert_eq!(author.disc, None);
                assert_eq!(quote.kind, QuoteKind::Summary);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn omission_needs_own_run_and_one_screen() {
        assert!(omit_reply_line(true, [200., 300.], 600.));
        assert!(!omit_reply_line(true, [400., 300.], 600.));
        assert!(!omit_reply_line(false, [10.], 600.));
        assert!(!omit_reply_line(true, [10.], 0.));
    }

    #[test]
    fn row_heights_reset_on_resize() {
        let mut heights = RowHeights::default();
        assert!(heights.record("a", 700., 80.));
        assert!(!heights.record("a", 700., 80.2));
        assert_eq!(heights.get("a", 700.), Some(80.2));
        assert_eq!(heights.get("a", 500.), None);
        heights.record("b", 500., 60.);
        assert_eq!(heights.get("a", 500.), None);
    }

    #[test]
    fn estimates_grow_with_text_and_width() {
        let short = estimate_height("好。", 700., true, false);
        let long = estimate_height(&"很长的一段话".repeat(40), 700., true, false);
        assert!(long > short + 60.);
        assert!(estimate_height("x", 700., false, false) < short);
    }

    #[test]
    fn passages_are_found_exactly() {
        assert_eq!(find_passage("看过了：密码框缺少", " 密码框 "), Some(12..21));
        assert_eq!(find_passage("abc", "x"), None);
        assert_eq!(find_passage("abc", "  "), None);
    }
}

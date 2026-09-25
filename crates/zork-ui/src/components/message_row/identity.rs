//! Who wrote a message and what it replies to, for Chats shared by several agents.
//! Contract: docs/design/interface.md (多 Agent 消息). Values and rules come from
//! core `message_presentation`; this module only draws them.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::tooltip,
    design::{agent_tint, ZORK_UI},
};
use gpui::{div, prelude::*, px, rgb, AnyElement, App, FontWeight, SharedString, Window};
use std::rc::Rc;

/// An agent avatar: its identity tint disc with the model maker's mark inside,
/// or the initial when no model is known. Users have no disc.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Disc {
    /// `design::AGENT_TINTS` slot.
    pub tint: usize,
    /// `makers/<key>.svg`; `None` draws `initial`.
    pub maker: Option<String>,
    pub initial: String,
}

/// The agent that wrote a message, shown once at the start of its group.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentIdentity {
    pub name: String,
    pub disc: Disc,
    /// Device display name; set only when core reports `multi_device` and the
    /// column is not phone width.
    pub device: Option<String>,
    /// Hover detail on the name: "名称 · 设备（机器名） · 模型".
    pub detail: String,
}

/// Position of a message inside a run by the same author.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grouping {
    pub starts: bool,
    pub ends: bool,
}
impl Grouping {
    pub const SINGLE: Self = Self {
        starts: true,
        ends: true,
    };
}

/// Author of a quoted message: the name plus the small disc (agents only).
#[derive(Clone, Debug, PartialEq)]
pub struct QuoteAuthor {
    pub name: String,
    pub disc: Option<Disc>,
}

/// How the quote content is marked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteKind {
    /// A short original shown whole, in 「」.
    Original,
    /// The replying author's verbatim passage, in 「」.
    Excerpt,
    /// The replying author's paraphrase, after the 大意 pill.
    Summary,
    /// Legacy reply without a quote: the start of the original, unmarked.
    Fallback,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuoteText {
    pub kind: QuoteKind,
    pub text: String,
    /// Full text for the tooltip; `None` when the line shows everything.
    pub tooltip: Option<String>,
}

/// The one reply line: ↩ 回复, target author, quote content.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// The original is loaded: clicking jumps to it.
    Linked {
        author: QuoteAuthor,
        quote: QuoteText,
    },
    /// The original is older than the loaded history: the first click loads.
    NotLoaded,
    /// Loading older history after a click on a not-loaded original.
    Loading,
    /// The original is gone; not clickable.
    Deleted { author: Option<QuoteAuthor> },
}
impl Reply {
    pub fn clickable(&self) -> bool {
        matches!(self, Self::Linked { .. } | Self::NotLoaded)
    }
}

pub type Callback = Rc<dyn Fn(&mut Window, &mut App)>;

/// Short wording, injected so clients keep their own catalogs.
#[derive(Clone, Debug)]
pub struct ReplyText {
    pub reply: String,
    pub earlier: String,
    pub not_loaded: String,
    pub loading: String,
    pub deleted: String,
    pub summary: String,
}
impl Default for ReplyText {
    fn default() -> Self {
        Self {
            reply: "回复".into(),
            earlier: "更早的消息".into(),
            not_loaded: "尚未加载 · 点击加载".into(),
            loading: "正在加载…".into(),
            deleted: "原消息已删除".into(),
            summary: "大意".into(),
        }
    }
}

/// Identity row and reply-line geometry (13 px transcript type).
pub const DISC: f32 = 20.;
pub const SMALL_DISC: f32 = 14.;
const LINE_TEXT: f32 = 12.;
const LINE_HEIGHT: f32 = 20.;

fn maker_path(key: &str) -> SharedString {
    format!("makers/{key}.svg").into()
}

/// The round tonal avatar: tint fill, maker mark (or initial) in the ink.
/// Round and tonal so it never reads as a device mark (solid squircle with the
/// folded corner) or a status dot.
pub fn disc(disc: &Disc, size: f32) -> gpui::Div {
    let (fill, ink) = agent_tint(disc.tint);
    div()
        .flex_shrink_0()
        .size(px(size))
        .rounded_full()
        .bg(rgb(fill))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(ink))
        .map(
            |v| match disc.maker.as_deref().filter(|key| !key.is_empty()) {
                Some(key) => v.child(
                    gpui::svg()
                        .path(maker_path(key))
                        .size(px((size * 0.68).round()))
                        .text_color(rgb(ink)),
                ),
                None => v
                    .text_size(px((size * 0.54).round()))
                    .line_height(px(size))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(disc.initial.clone()),
            },
        )
}

/// A Chat's avatar: up to three discs overlapping in first-appearance order,
/// then "+N" for the rest. `ring` is the surface colour behind the stack; each
/// disc gets a 2 px ring of it so the overlap reads as separate discs.
pub fn stack(discs: &[Disc], more: u64, size: f32, overlap: f32, ring: u32) -> AnyElement {
    let p = ZORK_UI.palette;
    const RING: f32 = 2.;
    // Each disc sits in a ring-coloured box 2 px larger on every side, so an
    // overlapping disc is cut by a clean gap instead of touching its
    // neighbour. Discs are placed at fixed steps (no negative margins), and
    // the stack's box is exactly the discs' extent.
    let step = (size - overlap).max(1.);
    let ringed = |inner: gpui::Div, index: usize| {
        div()
            .absolute()
            .top(px(-RING))
            .left(px(index as f32 * step - RING))
            .size(px(size + 2. * RING))
            .rounded_full()
            .bg(rgb(ring))
            .flex()
            .items_center()
            .justify_center()
            .child(inner)
            .into_any_element()
    };
    let mut children: Vec<AnyElement> = discs
        .iter()
        .take(3)
        .enumerate()
        .map(|(index, item)| ringed(disc(item, size), index))
        .collect();
    if more > 0 {
        let index = children.len();
        children.push(ringed(
            div()
                .size(px(size))
                .rounded_full()
                .bg(rgb(crate::design::INTERACTION.neutral_hover))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px((size * 0.5).round()))
                .line_height(px(size))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(p.muted))
                .child(format!("+{more}")),
            index,
        ));
    }
    let count = children.len().max(1);
    div()
        .relative()
        .flex_shrink_0()
        .w(px(size + (count - 1) as f32 * step))
        .h(px(size))
        .children(children)
        .into_any_element()
}

/// The Chat header: the Chat's stacked agent avatars before its title. With no
/// agents only the title shows (the plain Chat mark belongs to the list row).
pub fn chat_title(id: &str, discs: &[Disc], more: u64, title: &str, ring: u32) -> AnyElement {
    let p = ZORK_UI.palette;
    div()
        .id(id.to_owned())
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .when(!discs.is_empty() || more > 0, |v| {
            v.child(stack(discs, more, DISC, 7., ring))
        })
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(13.5))
                .line_height(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(p.text))
                .child(title.to_owned()),
        )
        .automation(AutomationRole::Status, title.to_owned())
        .into_any_element()
}

/// Identity at the head of an agent group: disc, name, the device when it
/// distinguishes agents, then (placed by the caller) the reply line and time.
/// The model is only in the hover detail.
pub fn header_identity(id: &str, identity: &AgentIdentity) -> AnyElement {
    let p = ZORK_UI.palette;
    let trigger = div()
        .id(format!("{id}-identity"))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(disc(&identity.disc, DISC))
        .child(
            div()
                .max_w(px(180.))
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(p.text))
                .child(identity.name.clone()),
        )
        .automation(AutomationRole::Status, identity.detail.clone());
    let trigger = tooltip::hint(trigger, format!("{id}-identity"), identity.detail.clone());
    div()
        .flex_shrink_0()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(trigger)
        .when_some(identity.device.clone(), |v, device| {
            v.child(
                div()
                    .id(format!("{id}-device"))
                    .min_w_0()
                    .max_w(px(140.))
                    .truncate()
                    .text_color(rgb(p.muted))
                    .child(device.clone())
                    .automation(AutomationRole::Status, device),
            )
        })
        .into_any_element()
}

/// Muted time with the full timestamp on hover (desktop) or long press.
pub fn time(id: &str, label: String, full: Option<String>) -> AnyElement {
    let p = ZORK_UI.palette;
    let label_div = div()
        .id(format!("{id}-time"))
        .flex_shrink_0()
        .whitespace_nowrap()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(rgb(p.subtle))
        .child(label.clone());
    match full {
        Some(full) => tooltip::hint(
            label_div.automation(AutomationRole::Status, full.clone()),
            format!("{id}-time"),
            full,
        )
        .into_any_element(),
        None => label_div
            .automation(AutomationRole::Status, label)
            .into_any_element(),
    }
}

pub fn reply_icon(color: u32) -> impl IntoElement {
    gpui::svg()
        .path("icons/reply.svg")
        .flex_shrink_0()
        .size(px(12.))
        .text_color(rgb(color))
}

/// The "大意" pill before a summary.
fn summary_pill(label: &str) -> AnyElement {
    div()
        .flex_shrink_0()
        .px(px(5.))
        .h(px(15.))
        .rounded_full()
        .bg(rgb(crate::design::INTERACTION.neutral_hover))
        .flex()
        .items_center()
        .text_size(px(10.5))
        .line_height(px(15.))
        .text_color(rgb(ZORK_UI.palette.muted))
        .child(label.to_owned())
        .into_any_element()
}

/// Who a quote line points at: the small disc (agents) and the name.
pub fn quote_author(author: &QuoteAuthor) -> AnyElement {
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(ZORK_UI.palette.text))
        .when_some(author.disc.as_ref(), |v, item| {
            v.child(disc(item, SMALL_DISC))
        })
        .child(div().max_w(px(140.)).truncate().child(author.name.clone()))
        .into_any_element()
}

/// The quote part of a line: 「text」, the 大意 pill and text, or the plain
/// start of the original. One line, ellipsized.
pub fn quote_content(quote: &QuoteText, text: &ReplyText, hover_group: &str) -> AnyElement {
    quote_content_colored(quote, text, hover_group, ZORK_UI.palette.subtle)
}

fn quote_content_colored(
    quote: &QuoteText,
    text: &ReplyText,
    hover_group: &str,
    color: u32,
) -> AnyElement {
    let p = ZORK_UI.palette;
    let shown = match quote.kind {
        QuoteKind::Original | QuoteKind::Excerpt => format!("「{}」", quote.text),
        QuoteKind::Summary | QuoteKind::Fallback => quote.text.clone(),
    };
    div()
        .min_w_0()
        .flex_1()
        .flex()
        .items_center()
        .gap(px(4.))
        .when(quote.kind == QuoteKind::Summary, |v| {
            v.child(summary_pill(&text.summary))
        })
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(color))
                .group_hover(hover_group.to_owned(), |s| {
                    s.text_color(rgb(p.text)).underline()
                })
                .child(shown),
        )
        .into_any_element()
}

/// Plain text of a reply line, for accessibility and tooltips.
pub fn reply_label(reply: &Reply, text: &ReplyText) -> String {
    match reply {
        Reply::Linked { author, quote } => {
            let content = match quote.kind {
                QuoteKind::Original | QuoteKind::Excerpt => format!("「{}」", quote.text),
                QuoteKind::Summary => format!("{} {}", text.summary, quote.text),
                QuoteKind::Fallback => quote.text.clone(),
            };
            format!("{} {} {}", text.reply, author.name, content)
        }
        Reply::NotLoaded => format!("{} {} · {}", text.reply, text.earlier, text.not_loaded),
        Reply::Loading => format!("{} {} · {}", text.reply, text.earlier, text.loading),
        Reply::Deleted { author } => match author {
            Some(author) => format!("{} {} · {}", text.reply, author.name, text.deleted),
            None => format!("{} · {}", text.reply, text.deleted),
        },
    }
}

/// The single muted reply line. In an identity row it shrinks between the
/// name and the time; above a body it takes the row width. Never a block.
pub fn reply_line(
    id: &str,
    reply: &Reply,
    text: &ReplyText,
    on_click: Option<Callback>,
) -> AnyElement {
    let p = ZORK_UI.palette;
    let key = format!("{id}-reply");
    let hover_group = format!("{key}-hover");
    let label = reply_label(reply, text);
    let muted_tail = |value: String| {
        div()
            .min_w_0()
            .truncate()
            .text_color(rgb(p.subtle))
            .child(value)
            .into_any_element()
    };
    let (who, content): (Option<AnyElement>, AnyElement) = match reply {
        Reply::Linked { author, quote } => (
            Some(quote_author(author)),
            quote_content(quote, text, &hover_group),
        ),
        Reply::NotLoaded => (
            Some(
                div()
                    .flex_shrink_0()
                    .child(text.earlier.clone())
                    .into_any_element(),
            ),
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(p.subtle))
                .group_hover(hover_group.clone(), |s| {
                    s.text_color(rgb(p.text)).underline()
                })
                .child(format!("· {}", text.not_loaded))
                .into_any_element(),
        ),
        Reply::Loading => (
            Some(
                div()
                    .flex_shrink_0()
                    .child(text.earlier.clone())
                    .into_any_element(),
            ),
            muted_tail(format!("· {}", text.loading)),
        ),
        Reply::Deleted { author } => (
            author.as_ref().map(quote_author),
            muted_tail(format!("· {}", text.deleted)),
        ),
    };
    let clickable = reply.clickable() && on_click.is_some();
    let tip = match reply {
        Reply::Linked { quote, .. } => quote.tooltip.clone().map(|tip| {
            if quote.kind == QuoteKind::Summary {
                format!("{}：{tip}", text.summary)
            } else {
                tip
            }
        }),
        _ => None,
    };
    let line = div()
        .id(key.clone())
        .group(hover_group)
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(LINE_HEIGHT))
        .text_size(px(LINE_TEXT))
        .line_height(px(LINE_HEIGHT))
        .text_color(rgb(p.muted))
        .child(reply_icon(p.muted))
        .child(div().flex_shrink_0().child(text.reply.clone()))
        .children(who)
        .child(content)
        .when_some(on_click.filter(|_| clickable), |v, on_click| {
            v.cursor_pointer()
                .on_click(move |_, window, cx| on_click(window, cx))
        })
        .automation(
            if clickable {
                AutomationRole::Button
            } else {
                AutomationRole::Status
            },
            label,
        );
    match tip {
        Some(tip) => tooltip::hint(line, key, tip).into_any_element(),
        None => line.into_any_element(),
    }
}

/// A quote line of a user comment: ↩, the source author, 「passage」, and an
/// optional trailing control (the draft's ×). Same style as the reply line.
pub fn passage_line(
    id: &str,
    author: &QuoteAuthor,
    passage: &str,
    on_click: Option<Callback>,
    trailing: Option<AnyElement>,
) -> AnyElement {
    let p = ZORK_UI.palette;
    let hover_group = format!("{id}-hover");
    let quote = QuoteText {
        kind: QuoteKind::Excerpt,
        text: passage.split_whitespace().collect::<Vec<_>>().join(" "),
        tooltip: None,
    };
    let clickable = on_click.is_some();
    let line = div()
        .id(id.to_owned())
        .group(hover_group.clone())
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(LINE_HEIGHT))
        .text_size(px(LINE_TEXT))
        .line_height(px(LINE_HEIGHT))
        .text_color(rgb(p.muted))
        .child(reply_icon(p.muted))
        .child(quote_author(author))
        .child(quote_content_colored(
            &quote,
            &ReplyText::default(),
            &hover_group,
            p.muted,
        ))
        .children(trailing)
        .when_some(on_click, |v, on_click| {
            v.cursor_pointer()
                .on_click(move |_, window, cx| on_click(window, cx))
        })
        .automation(
            if clickable {
                AutomationRole::Button
            } else {
                AutomationRole::Status
            },
            format!("{}：{}", author.name, passage),
        );
    tooltip::hint(line, id.to_owned(), format!("{}：{}", author.name, passage)).into_any_element()
}

/// The floating dark pill offered next to a text selection ("引用回复"), and
/// the same shape for short confirmations such as "已加载…".
pub fn dark_pill(id: impl Into<gpui::ElementId>, label: String) -> gpui::Stateful<gpui::Div> {
    let p = ZORK_UI.palette;
    div()
        .id(id)
        .flex_shrink_0()
        .px(px(12.))
        .h(px(28.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(rgb(p.text))
        .text_color(rgb(p.window))
        .text_size(px(12.5))
        .line_height(px(16.))
        .shadow(vec![gpui::BoxShadow {
            color: gpui::hsla(0., 0., 0., 0.15),
            offset: gpui::point(px(0.), px(4.)),
            blur_radius: px(12.),
            spread_radius: px(0.),
            inset: false,
        }])
        .child(label)
}

/// Plain one-paragraph excerpt of a message: whitespace collapsed and bounded.
pub fn excerpt(plain: &str, max_chars: usize) -> String {
    let joined = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_labels_read_as_one_line() {
        let text = ReplyText::default();
        let author = QuoteAuthor {
            name: "Builder".into(),
            disc: None,
        };
        let excerpt = Reply::Linked {
            author: author.clone(),
            quote: QuoteText {
                kind: QuoteKind::Excerpt,
                text: "对比度只有 3.9:1".into(),
                tooltip: None,
            },
        };
        assert_eq!(
            reply_label(&excerpt, &text),
            "回复 Builder 「对比度只有 3.9:1」"
        );
        let summary = Reply::Linked {
            author,
            quote: QuoteText {
                kind: QuoteKind::Summary,
                text: "错误文案要具体".into(),
                tooltip: None,
            },
        };
        assert_eq!(
            reply_label(&summary, &text),
            "回复 Builder 大意 错误文案要具体"
        );
        assert_eq!(
            reply_label(&Reply::NotLoaded, &text),
            "回复 更早的消息 · 尚未加载 · 点击加载"
        );
        assert_eq!(
            reply_label(&Reply::Deleted { author: None }, &text),
            "回复 · 原消息已删除"
        );
    }

    #[test]
    fn excerpts_collapse_whitespace_and_stay_bounded() {
        assert_eq!(
            excerpt("一、梳理\n\n二、实现  细节", 60),
            "一、梳理 二、实现 细节"
        );
        assert_eq!(excerpt(&"字".repeat(100), 60).chars().count(), 60);
    }
}

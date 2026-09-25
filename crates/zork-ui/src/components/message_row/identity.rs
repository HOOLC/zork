//! Who wrote a message and what it replies to, for Chats shared by several agents.
//! Contract: docs/design/interface.md (agent identity, reply quotes, message time).
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{loading, tooltip},
    design::{agent_tint, INTERACTION, RADIUS, ZORK_UI},
};
use gpui::{div, prelude::*, px, rgb, AnyElement, App, FontWeight, HighlightStyle, Window};
use std::rc::Rc;

/// The agent that wrote a message, shown once at the start of its group.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentIdentity {
    /// Display name from the message's author facts.
    pub name: String,
    /// Tint slot from `design::agent_tint_slots`, keyed by the agent id.
    pub tint: usize,
    /// Execution device, set only when it distinguishes agents in this Chat.
    pub device: Option<String>,
    /// Model, revealed with the device in the identity's hover details.
    pub model: Option<String>,
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

/// Author of the message being replied to.
#[derive(Clone, Debug, PartialEq)]
pub enum QuoteAuthor {
    User,
    Agent { name: String, tint: usize },
}

/// What a message replies to, by how much of the original can be shown.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// The original sits just above: a muted "回复 X" marker is enough.
    Marker { author: QuoteAuthor },
    /// The original is further up: author and a clamped excerpt.
    Quote {
        author: QuoteAuthor,
        excerpt: String,
    },
    /// The original was deleted; its author may still be known.
    Deleted { author: Option<QuoteAuthor> },
    /// The original is in history not loaded yet; clicking loads and jumps.
    NotLoaded,
    /// Loading history after a click on a not-yet-loaded original.
    Loading,
}

pub type Callback = Rc<dyn Fn(&mut Window, &mut App)>;

/// Short wording, injected so clients keep their own catalogs.
#[derive(Clone, Debug)]
pub struct ReplyText {
    pub you: String,
    pub reply_to: String,
    pub deleted: String,
    pub deleted_by: String,
    pub not_loaded: String,
    pub load: String,
    pub loading: String,
}
impl Default for ReplyText {
    fn default() -> Self {
        Self {
            you: "你".into(),
            reply_to: "回复 {name}".into(),
            deleted: "原消息已删除".into(),
            deleted_by: "{name} 的原消息已删除".into(),
            not_loaded: "回复较早的消息".into(),
            load: "点击加载".into(),
            loading: "正在加载原消息…".into(),
        }
    }
}

const QUOTE_TEXT: f32 = 12.;
const QUOTE_LINE: f32 = 18.;

/// The agent's round tonal mark: its initial on its tint. Round and tonal so
/// it never reads as a device mark (solid squircle, folded corner) or a status dot.
pub fn mark(name: &str, tint: usize, size: f32) -> impl IntoElement {
    let (fill, ink) = agent_tint(tint);
    div()
        .flex_shrink_0()
        .size(px(size))
        .rounded_full()
        .bg(rgb(fill))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px((size * 0.54).round()))
        .line_height(px(size))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(ink))
        .child(initial(name))
}

/// First letter or CJK character of a name, upper-cased.
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into())
}

/// Identity for a group's first message: mark, name, and the device only when
/// the Chat's agents span several devices. The model joins the device in the
/// hover details instead of competing with the name; on a phone-width column
/// the device moves into those details too.
pub fn header_identity(id: &str, identity: &AgentIdentity, compact: bool) -> AnyElement {
    let p = ZORK_UI.palette;
    let details = [
        Some(identity.name.clone()),
        identity.device.clone(),
        identity.model.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let trigger = div()
        .id(format!("{id}-identity"))
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(mark(&identity.name, identity.tint, 20.))
        .child(
            div()
                .min_w_0()
                .max_w(px(180.))
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(p.text))
                .child(identity.name.clone()),
        )
        .when_some(identity.device.clone().filter(|_| !compact), |v, device| {
            v.child(
                div()
                    .min_w_0()
                    .max_w(px(140.))
                    .truncate()
                    .text_size(px(12.))
                    .text_color(rgb(p.muted))
                    .child(device),
            )
        })
        .automation(AutomationRole::Status, details.clone());
    tooltip::hint(trigger, format!("{id}-identity"), details).into_any_element()
}

/// Muted time with the full timestamp on hover (desktop) or long press (phone).
pub fn time(id: &str, label: String, full: Option<String>) -> AnyElement {
    let p = ZORK_UI.palette;
    let label_div = div()
        .id(format!("{id}-time"))
        .flex_shrink_0()
        .whitespace_nowrap()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(rgb(p.muted))
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

fn author_name(author: &QuoteAuthor, text: &ReplyText) -> String {
    match author {
        QuoteAuthor::User => text.you.clone(),
        QuoteAuthor::Agent { name, .. } => name.clone(),
    }
}

fn author_mark(author: &QuoteAuthor, size: f32) -> Option<AnyElement> {
    match author {
        QuoteAuthor::User => None,
        QuoteAuthor::Agent { name, tint } => Some(mark(name, *tint, size).into_any_element()),
    }
}

fn reply_icon(color: u32) -> impl IntoElement {
    gpui::svg()
        .path("icons/reply.svg")
        .flex_shrink_0()
        .size(px(13.))
        .text_color(rgb(color))
}

/// The on-screen marker: "↩ 回复 X", muted, one line. Clicking still locates
/// and highlights the original so the gesture is the same as for a quote.
pub fn marker(
    id: &str,
    author: &QuoteAuthor,
    text: &ReplyText,
    on_click: Option<Callback>,
) -> AnyElement {
    let p = ZORK_UI.palette;
    let name = author_name(author, text);
    let label = text.reply_to.replace("{name}", &name);
    div()
        .id(format!("{id}-reply"))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(4.))
        .px(px(6.))
        .mx(px(-6.))
        .h(px(20.))
        .rounded(px(10.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(rgb(p.muted))
        .child(reply_icon(p.muted))
        .child(div().whitespace_nowrap().child(label.clone()))
        .when_some(on_click, |v, on_click| {
            v.cursor_pointer()
                .hover(|s| s.bg(rgb(INTERACTION.neutral_hover)))
                .on_click(move |_, window, cx| on_click(window, cx))
        })
        .automation(AutomationRole::Button, label)
        .into_any_element()
}

/// The quote above a reply whose original is out of view: a soft block with
/// the original author's mark and name leading a two-line excerpt.
pub fn quote(
    id: &str,
    reply: &Reply,
    text: &ReplyText,
    max_width: f32,
    on_click: Option<Callback>,
) -> AnyElement {
    let p = ZORK_UI.palette;
    let (lead, body, clickable, busy): (Option<AnyElement>, Vec<(String, bool)>, bool, bool) =
        match reply {
            Reply::Quote { author, excerpt } => (
                author_mark(author, 16.),
                vec![(author_name(author, text), true), (excerpt.clone(), false)],
                true,
                false,
            ),
            Reply::Deleted { author } => (
                author.as_ref().and_then(|a| author_mark(a, 16.)),
                vec![(
                    match author {
                        Some(author) => text
                            .deleted_by
                            .replace("{name}", &author_name(author, text)),
                        None => text.deleted.clone(),
                    },
                    false,
                )],
                false,
                false,
            ),
            Reply::NotLoaded => (
                None,
                vec![(text.not_loaded.clone(), true), (text.load.clone(), false)],
                true,
                false,
            ),
            Reply::Loading => (None, vec![(text.loading.clone(), false)], false, true),
            Reply::Marker { author } => return marker(id, author, text, on_click),
        };
    // One text run so the name and the excerpt share the two-line clamp.
    let mut content = String::new();
    let mut highlights = Vec::new();
    for (index, (part, strong)) in body.iter().enumerate() {
        if index > 0 {
            content.push_str("  ");
        }
        let start = content.len();
        content.push_str(part);
        if *strong {
            highlights.push((
                start..content.len(),
                HighlightStyle {
                    color: Some(rgb(p.text).into()),
                    font_weight: Some(FontWeight::MEDIUM),
                    ..Default::default()
                },
            ));
        }
    }
    let accessible = content.clone();
    let deleted = matches!(reply, Reply::Deleted { .. });
    div()
        .id(format!("{id}-quote"))
        .max_w(px(max_width))
        .flex()
        .items_start()
        .gap(px(6.))
        .pl(px(10.))
        .pr(px(12.))
        .py(px(7.))
        .rounded(px(RADIUS.block))
        .bg(rgb(p.prompt))
        .child(
            div()
                .h(px(QUOTE_LINE))
                .flex()
                .items_center()
                .child(if busy {
                    loading::indicator(format!("{id}-quote-loading"), 12.)
                        .without_delay()
                        .into_any_element()
                } else {
                    reply_icon(p.muted).into_any_element()
                }),
        )
        .when_some(lead, |v, lead| {
            v.child(div().h(px(QUOTE_LINE)).flex().items_center().child(lead))
        })
        .child(
            div()
                .min_w_0()
                .text_size(px(QUOTE_TEXT))
                .line_height(px(QUOTE_LINE))
                .text_color(rgb(if deleted { p.subtle } else { p.muted }))
                .line_clamp(2)
                .text_ellipsis()
                .child(gpui::StyledText::new(content).with_highlights(highlights)),
        )
        .when(clickable, |v| {
            v.cursor_pointer()
                .hover(|s| s.bg(rgb(INTERACTION.neutral_hover)))
                .active(|s| s.bg(rgb(INTERACTION.neutral_pressed)))
        })
        .when_some(on_click.filter(|_| clickable), |v, on_click| {
            v.on_click(move |_, window, cx| on_click(window, cx))
        })
        .automation(
            if clickable {
                AutomationRole::Button
            } else {
                AutomationRole::Status
            },
            accessible,
        )
        .into_any_element()
}

/// Plain one-paragraph excerpt of a message for a quote: whitespace collapsed
/// and bounded, so the clamp does not shape a whole long document.
pub fn excerpt(plain: &str) -> String {
    let mut out = String::new();
    for line in plain.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        // Lines keep a visible seam unless the previous one already ends
        // in punctuation (a heading colon, a sentence stop).
        if !out.is_empty() {
            let ends_in_stop = out
                .chars()
                .last()
                .is_some_and(|c| "：:，,。.；;！!？?、".contains(c));
            out.push_str(if ends_in_stop { " " } else { " · " });
        }
        out.push_str(&line);
        if out.chars().count() > 240 {
            let cut: String = out.chars().take(240).collect();
            return format!("{cut}…");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_the_first_letter_or_character() {
        assert_eq!(initial("planner"), "P");
        assert_eq!(initial("审阅助手"), "审");
        assert_eq!(initial("  @builder"), "B");
        assert_eq!(initial(""), "?");
    }

    #[test]
    fn excerpts_collapse_whitespace_and_stay_bounded() {
        assert_eq!(
            excerpt("一、梳理\n\n二、实现  细节"),
            "一、梳理 · 二、实现 细节"
        );
        assert_eq!(
            excerpt("验收标准：\n\n- 首屏\n- 字段"),
            "验收标准： - 首屏 · - 字段"
        );
        let long = "字".repeat(1000);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), 241);
        assert!(cut.ends_with('…'));
    }
}

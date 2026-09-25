//! Complete transcript message row shared by the application and Playground.
use super::{
    message::{MessageDocument, TextMarks},
    selection::SelectionContext,
};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::{INTERACTION, ZORK_UI},
};
use gpui::{prelude::*, *};
pub use identity::{
    AgentIdentity, Callback, Disc, Grouping, QuoteAuthor, QuoteKind, QuoteText, Reply, ReplyText,
};
pub use presentation::TimePlacement;
use std::rc::Rc;
#[allow(non_snake_case)]
fn DIM() -> u32 {
    ZORK_UI.palette.muted
}

pub struct Row<'a> {
    pub index: usize,
    pub user: bool,
    pub document: &'a MessageDocument,
    pub content_width: f32,
    pub selection: Option<&'a SelectionContext>,
    pub author_name: Option<String>,
    pub device: Option<String>,
    pub model: Option<String>,
    pub time: Option<String>,
}

/// A row's time label as placed by core.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeView {
    pub label: String,
    pub full: String,
    pub placement: TimePlacement,
}

/// One pair of a sent comment batch: a quote line, then the reply text.
pub struct CommentPairView {
    pub author: QuoteAuthor,
    pub passage: String,
    pub reply: Rc<MessageDocument>,
    /// Jumps to the source and marks the passage; `None` when not loaded.
    pub on_click: Option<Callback>,
}

/// A sent comment batch inside the user bubble.
pub struct CommentsView {
    pub pairs: Vec<CommentPairView>,
    pub extra: Option<Rc<MessageDocument>>,
}

/// Multi-agent presentation of a row (docs/design/interface.md 多 Agent 消息).
/// With every field absent the row renders the plain single-author layout.
#[derive(Default)]
pub struct Decorations {
    /// Agent identity at the head of an agent group.
    pub identity: Option<AgentIdentity>,
    /// Run position; a continued message drops its header and tightens spacing.
    pub grouping: Option<Grouping>,
    /// The reply line, already filtered by the omission rule.
    pub reply: Option<Reply>,
    /// The reply line sits in the identity row (group head) rather than above
    /// the body.
    pub reply_in_head: bool,
    pub reply_text: ReplyText,
    /// Jumps to the original, or loads older history for a not-loaded one.
    pub on_reply: Option<Callback>,
    pub time: Option<TimeView>,
    /// A sent comment batch replaces the plain bubble body.
    pub comments: Option<CommentsView>,
    /// Whole-message jump wash opacity (0 = none); used when the quoted
    /// passage cannot be marked in place.
    pub wash: f32,
    /// In-place passage marks (jump wash, draft underlines).
    pub marks: Option<TextMarks>,
    /// Hover action quoting the whole message: label and handler.
    pub quote_action: Option<(String, Callback)>,
}

impl Decorations {
    /// The decorations core's presentation gives a row. `show_device`: core's
    /// `multi_device` and not phone width. `omit_reply`: the UI's omission
    /// decision ([`presentation::omit_row_reply`]). `reply_loading`: older
    /// history is loading after a click on this row's not-loaded line.
    /// Hosts add callbacks, the wash, marks and comment documents.
    pub fn from_presentation(
        row: &presentation::Row,
        show_device: bool,
        omit_reply: bool,
        reply_loading: bool,
        reply_text: ReplyText,
    ) -> Self {
        Self {
            identity: row
                .identity
                .as_ref()
                .filter(|_| row.group_head && !row.user)
                .map(|identity| identity.agent_identity(show_device)),
            grouping: Some(Grouping {
                starts: row.group_head,
                ends: row.group_tail,
            }),
            reply: row
                .reply
                .as_ref()
                .filter(|_| !omit_reply)
                .map(|reply| reply.reply(reply_loading)),
            reply_in_head: row
                .reply
                .as_ref()
                .is_some_and(|reply| reply.placement == presentation::ReplyPlacement::InHead),
            reply_text,
            time: row.time.as_ref().map(|time| TimeView {
                label: time.label.clone(),
                full: time.full.clone(),
                placement: time.placement,
            }),
            ..Default::default()
        }
    }

    fn decorated(&self) -> bool {
        self.identity.is_some()
            || self.grouping.is_some()
            || self.reply.is_some()
            || self.time.is_some()
            || self.comments.is_some()
            || self.wash > 0.
            || self.marks.is_some()
            || self.quote_action.is_some()
    }
}

/// Spacing between groups and inside a run by one author: runs read as one
/// voice (about 6 px apart) and the larger gap between groups (about 20 px)
/// replaces any divider.
const GROUP_GAP: f32 = 10.;
const RUN_GAP: f32 = 3.;
/// Space between the pairs of a comment batch; no boxes inside the bubble.
const PAIR_GAP: f32 = 12.;

impl Row<'_> {
    pub fn render(self, window: &mut Window) -> AnyElement {
        self.render_with(window, Decorations::default())
    }

    fn bubble_limit(content_width: f32) -> f32 {
        (content_width - ZORK_UI.thread.user_left_clearance)
            .min(ZORK_UI.thread.user_max_width)
            .max(40.)
    }

    fn measure_bubble(document: &MessageDocument, content_width: f32, window: &mut Window) -> f32 {
        let max_width = Self::bubble_limit(content_width);
        let measured = document.bounded_text_width(
            gpui::font("Inter Variable"),
            13.,
            (max_width - 2. * ZORK_UI.thread.user_padding_x - 2.).max(1.),
            window,
        );
        // Retain rounding slack so fractional glyph advances do not orphan punctuation.
        (measured.ceil() + 2. * ZORK_UI.thread.user_padding_x + 2.).clamp(40., max_width)
    }

    pub fn render_with(self, window: &mut Window, decor: Decorations) -> AnyElement {
        let Self {
            index,
            user,
            document,
            content_width,
            selection,
            author_name,
            device,
            model,
            time,
        } = self;
        if decor.decorated() {
            return decorated_row(
                index,
                user,
                document,
                content_width,
                selection,
                decor,
                window,
            );
        }
        // Only user bubbles need intrinsic width. Assistant prose already fills
        // its column, so shaping its full document here duplicated every code run.
        let bubble_width = if user {
            Self::measure_bubble(document, content_width, window)
        } else {
            0.
        };
        let prose = if let Some(selection) = selection {
            crate::components::message::render_selectable_document(
                &format!("message-{index}"),
                document,
                selection,
            )
        } else {
            crate::components::message::render_document(&format!("message-{index}"), document)
        };
        match user {
            true => transcript_row()
                .child(
                    div()
                        .w(px(content_width))
                        .max_w_full()
                        .flex()
                        .justify_end()
                        .child(
                            div()
                                .w(px(bubble_width))
                                .flex()
                                .flex_col()
                                .items_end()
                                .gap_1()
                                .child(user_bubble().w_full().child(prose))
                                .when_some(time, |v, time| {
                                    v.child(
                                        div().text_size(px(12.)).text_color(rgb(DIM())).child(time),
                                    )
                                }),
                        ),
                )
                .into_any(),
            false => transcript_row()
                .child(
                    div().w(px(content_width)).max_w_full().flex().child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .when_some(device.clone(), |v, device| {
                                        v.child(crate::device_name::mark(&device, 18.))
                                    })
                                    .when_some(device.or(author_name), |v, device| {
                                        v.child(
                                            div()
                                                .max_w(px(140.))
                                                .truncate()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(device),
                                        )
                                    })
                                    .when_some(
                                        model.filter(|model| !model.is_empty()),
                                        |v, model| {
                                            v.child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(px(12.))
                                                    .text_color(rgb(DIM()))
                                                    .child(model),
                                            )
                                        },
                                    )
                                    .when_some(time, |v, time| {
                                        v.child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(px(12.))
                                                .text_color(rgb(DIM()))
                                                .child(time),
                                        )
                                    }),
                            )
                            .child(prose),
                    ),
                )
                .into_any(),
        }
    }
}
fn transcript_row() -> Div {
    div().w_full().px_6().py(px(7.)).flex().justify_center()
}
fn user_bubble() -> Div {
    div()
        .rounded(px(ZORK_UI.thread.user_radius))
        .rounded_br(px(crate::design::RADIUS.fold))
        .bg(rgb(ZORK_UI.thread.user_fill))
        .px(px(ZORK_UI.thread.user_padding_x))
        .py(px(ZORK_UI.thread.user_padding_y))
        .text_size(px(13.))
        .line_height(px(20.))
}

fn prose(
    key: &str,
    document: &MessageDocument,
    selection: Option<&SelectionContext>,
    marks: Option<&TextMarks>,
) -> AnyElement {
    crate::components::message::render_document_marked(key, document, selection, marks)
}

/// The width a comment batch bubble needs: its widest reply or quote line.
fn comments_width(comments: &CommentsView, content_width: f32, window: &mut Window) -> f32 {
    let limit = Row::bubble_limit(content_width);
    let mut widest: f32 = 40.;
    for pair in &comments.pairs {
        widest = widest.max(Row::measure_bubble(&pair.reply, content_width, window));
        // Quote line: icon, disc, name and passage at 12 px.
        let line =
            MessageDocument::plain(&format!("回复 {} 「{}」", pair.author.name, pair.passage));
        let measured = line.bounded_text_width(gpui::font("Inter Variable"), 12., limit, window);
        widest = widest.max(measured + 40. + 2. * ZORK_UI.thread.user_padding_x);
    }
    if let Some(extra) = &comments.extra {
        widest = widest.max(Row::measure_bubble(extra, content_width, window));
    }
    widest.clamp(40., limit)
}

fn decorated_row(
    index: usize,
    user: bool,
    document: &MessageDocument,
    content_width: f32,
    selection: Option<&SelectionContext>,
    decor: Decorations,
    window: &mut Window,
) -> AnyElement {
    let Decorations {
        identity,
        grouping,
        reply,
        reply_in_head,
        reply_text,
        on_reply,
        time,
        comments,
        wash,
        marks,
        quote_action,
    } = decor;
    let key = format!("message-{index}");
    let starts = grouping.is_none_or(|g| g.starts);
    let ends = grouping.is_none_or(|g| g.ends);
    let (top, bottom) = if grouping.is_some() {
        (
            if starts { GROUP_GAP } else { RUN_GAP },
            if ends { GROUP_GAP } else { RUN_GAP },
        )
    } else {
        (7., 7.)
    };
    let hover_group = format!("{key}-row");
    let reply_line = reply
        .as_ref()
        .map(|reply| identity::reply_line(&key, reply, &reply_text, on_reply.clone()));
    let p = ZORK_UI.palette;
    // The user bubble's hover action sits just left of the bubble.
    let mut quote_right = 0.;
    let content: AnyElement = if user {
        let (bubble_width, body) = match &comments {
            Some(comments) => {
                let width = comments_width(comments, content_width, window);
                let mut pairs = Vec::new();
                for (pair_index, pair) in comments.pairs.iter().enumerate() {
                    pairs.push(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .when(pair_index > 0, |v| v.mt(px(PAIR_GAP)))
                            .child(identity::passage_line(
                                &format!("{key}-pair-{pair_index}"),
                                &pair.author,
                                &pair.passage,
                                pair.on_click.clone(),
                                None,
                            ))
                            .child(prose(
                                &format!("{key}-pair-{pair_index}-text"),
                                &pair.reply,
                                selection,
                                marks.as_ref(),
                            ))
                            .into_any_element(),
                    );
                }
                if let Some(extra) = &comments.extra {
                    pairs.push(
                        div()
                            .w_full()
                            .when(!comments.pairs.is_empty(), |v| v.mt(px(PAIR_GAP)))
                            .child(prose(
                                &format!("{key}-extra"),
                                extra,
                                selection,
                                marks.as_ref(),
                            ))
                            .into_any_element(),
                    );
                }
                (
                    width,
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .children(pairs)
                        .into_any_element(),
                )
            }
            None => (
                Row::measure_bubble(document, content_width, window),
                prose(&key, document, selection, marks.as_ref()),
            ),
        };
        quote_right = bubble_width + 8.;
        div()
            .max_w(px((content_width - ZORK_UI.thread.user_left_clearance).max(40.)))
            .flex()
            .flex_col()
            .items_end()
            .gap_1()
            .when_some(reply_line, |v, line| v.child(div().max_w_full().child(line)))
            .child(
                user_bubble()
                    .id(format!("{key}-bubble"))
                    .w(px(bubble_width))
                    .when(comments.is_some(), |v| v.px(px(14.)).py(px(10.)))
                    .child(body),
            )
            // A user group shows its time once, under the last bubble.
            .when_some(
                time.clone()
                    .filter(|time| time.placement == TimePlacement::Tail),
                |v, time| v.child(identity::time(&key, time.label, Some(time.full))),
            )
            .into_any_element()
    } else {
        let head = (starts && identity.is_some()).then(|| {
            let identity = identity.as_ref().unwrap();
            div()
                .id(format!("{key}-head"))
                .w_full()
                .min_w_0()
                .h(px(22.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(13.))
                .child(identity::header_identity(&key, identity))
        });
        let (head, reply_line) = match (head, reply_line) {
            (Some(head), Some(line)) if reply_in_head => (
                Some(head.child(div().min_w_0().flex_shrink_1().child(line))),
                None,
            ),
            (head, line) => (head, line),
        };
        let head = head.map(|head| {
            head.when_some(
                time.clone()
                    .filter(|time| time.placement == TimePlacement::Head),
                |v, time| v.child(identity::time(&key, time.label, Some(time.full))),
            )
        });
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .line_height(px(20.))
            .when_some(head, |v, head| v.child(head.mb(px(2.))))
            .when_some(reply_line, |v, line| {
                v.child(div().w_full().min_w_0().flex().mb(px(2.)).child(line))
            })
            .child(prose(&key, document, selection, marks.as_ref()))
            .into_any_element()
    };
    // A continued agent message reveals its own time at the row end on hover.
    let hover_time = (!user)
        .then_some(time.clone())
        .flatten()
        .filter(|time| time.placement == TimePlacement::Hover)
        .map(|time| {
            div()
                .absolute()
                .top(px(1.))
                .right(px(if quote_action.is_some() { 64. } else { 0. }))
                .pl(px(12.))
                .opacity(0.)
                .group_hover(hover_group.clone(), |s| s.opacity(1.))
                .child(identity::time(
                    &format!("{key}-run"),
                    time.label,
                    Some(time.full),
                ))
        });
    let quote_button = quote_action.map(|(label, on_click)| {
        div()
            .absolute()
            .when(user, |v| v.top(px(6.)).right(px(quote_right)))
            .when(!user, |v| v.top(px(-14.)).right(px(0.)))
            .opacity(0.)
            .group_hover(hover_group.clone(), |s| s.opacity(1.))
            .child(
                div()
                    .id(format!("{key}-quote-all"))
                    .h(px(24.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(rgb(p.canvas))
                    .border(px(crate::design::BORDER_WIDTH))
                    .border_color(rgb(p.border))
                    .text_size(px(12.))
                    .text_color(rgb(p.muted))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(INTERACTION.neutral_hover)).text_color(rgb(p.text)))
                    .child(label.clone())
                    .on_click(move |_, window, cx| on_click(window, cx))
                    .automation(AutomationRole::Button, label),
            )
    });
    let wash_alpha = (wash.clamp(0., 1.) * 255.).round() as u32;
    div()
        .id(format!("{key}-decorated"))
        .group(hover_group)
        .w_full()
        .px_6()
        .pt(px(top))
        .pb(px(bottom))
        .flex()
        .justify_center()
        .child(
            div()
                .relative()
                .w(px(content_width))
                .max_w_full()
                .flex()
                .when(user, |v| v.justify_end())
                .when(wash_alpha > 0, |v| {
                    v.child(
                        div()
                            .absolute()
                            .top(px(-top.min(4.)))
                            .bottom(px(-bottom.min(4.)))
                            .left(px(-10.))
                            .right(px(-10.))
                            .rounded(px(crate::design::RADIUS.container))
                            .bg(rgba((*crate::design::JUMP_WASH << 8) | wash_alpha)),
                    )
                })
                .child(content)
                .children(hover_time)
                .children(quote_button),
        )
        .into_any()
}

pub mod identity;
#[cfg(feature = "stories")]
pub mod multi_agent;
pub mod presentation;
#[cfg(feature = "stories")]
pub mod stories;

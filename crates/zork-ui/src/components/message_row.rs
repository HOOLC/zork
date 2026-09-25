//! Complete transcript message row shared by the application and Playground.
use super::{message::MessageDocument, selection::SelectionContext};
use crate::design::ZORK_UI;
use gpui::{prelude::*, *};
pub use identity::{AgentIdentity, Grouping, QuoteAuthor, Reply, ReplyText};
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
/// Optional multi-agent presentation. With every field absent the row renders
/// exactly as before, so existing single-agent transcripts are unchanged.
#[derive(Default)]
pub struct Decorations {
    /// Agent identity replaces the device-led header on agent messages.
    pub identity: Option<AgentIdentity>,
    /// Run position; a continued message drops its header and tightens spacing.
    pub grouping: Option<Grouping>,
    pub reply: Option<Reply>,
    pub reply_text: ReplyText,
    /// Locates the original; set when the host can scroll to it.
    pub on_reply: Option<identity::Callback>,
    /// Full timestamp for the time label's hover details.
    pub time_full: Option<String>,
    /// Brief wash after a jump from a reply quote.
    pub highlighted: bool,
}

impl Row<'_> {
    pub fn render(self, window: &mut Window) -> AnyElement {
        self.render_with(window, Decorations::default())
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
        // Only user bubbles need intrinsic width. Assistant prose already fills
        // its column, so shaping its full document here duplicated every code run.
        let bubble_width = if user {
            let max_width = (content_width - ZORK_UI.thread.user_left_clearance)
                .min(ZORK_UI.thread.user_max_width)
                .max(40.);
            let measured = document.bounded_text_width(
                gpui::font("Inter Variable"),
                13.,
                (max_width - 2. * ZORK_UI.thread.user_padding_x - 2.).max(1.),
                window,
            );
            // Retain rounding slack so fractional glyph advances do not orphan punctuation.
            (measured.ceil() + 2. * ZORK_UI.thread.user_padding_x + 2.).clamp(
                40.,
                (content_width - ZORK_UI.thread.user_left_clearance)
                    .min(ZORK_UI.thread.user_max_width)
                    .max(40.),
            )
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
        let Decorations {
            identity,
            grouping,
            reply,
            reply_text,
            on_reply,
            time_full,
            highlighted,
        } = decor;
        let key = format!("message-{index}");
        let starts = grouping.is_none_or(|g| g.starts);
        let ends = grouping.is_none_or(|g| g.ends);
        let quote_width = (content_width
            - if user {
                ZORK_UI.thread.user_left_clearance
            } else {
                0.
            })
        .min(560.)
        .max(40.);
        let reply_block = reply
            .as_ref()
            .map(|reply| identity::quote(&key, reply, &reply_text, quote_width, on_reply.clone()));
        let decorated = identity.is_some()
            || grouping.is_some()
            || reply.is_some()
            || time_full.is_some()
            || highlighted;
        if decorated {
            return decorated_row(
                &key,
                user,
                bubble_width,
                content_width,
                prose,
                identity,
                (device, author_name, model),
                starts,
                ends,
                grouping.is_some(),
                reply.as_ref(),
                reply_block,
                (time, time_full),
                highlighted,
            );
        }
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
                                .child(
                                    div()
                                        .w_full()
                                        .rounded(px(ZORK_UI.thread.user_radius))
                                        .rounded_br(px(crate::design::RADIUS.fold))
                                        .bg(rgb(ZORK_UI.thread.user_fill))
                                        .px(px(ZORK_UI.thread.user_padding_x))
                                        .py(px(ZORK_UI.thread.user_padding_y))
                                        .text_size(px(13.))
                                        .line_height(px(20.))
                                        .child(prose),
                                )
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

/// Spacing between groups and inside a run by one author: runs read as one
/// voice, and the larger gap between groups replaces any divider.
const GROUP_GAP: f32 = 10.;
const RUN_GAP: f32 = 3.;
/// Below this column width the header keeps only mark, name and time.
const COMPACT_WIDTH: f32 = 480.;

#[allow(clippy::too_many_arguments)]
fn decorated_row(
    key: &str,
    user: bool,
    bubble_width: f32,
    content_width: f32,
    prose: AnyElement,
    identity: Option<AgentIdentity>,
    (device, author_name, model): (Option<String>, Option<String>, Option<String>),
    starts: bool,
    ends: bool,
    grouped: bool,
    reply: Option<&Reply>,
    reply_block: Option<AnyElement>,
    (time, time_full): (Option<String>, Option<String>),
    highlighted: bool,
) -> AnyElement {
    let (top, bottom) = if grouped {
        (
            if starts { GROUP_GAP } else { RUN_GAP },
            if ends { GROUP_GAP } else { RUN_GAP },
        )
    } else {
        (7., 7.)
    };
    let marker_in_header = matches!(reply, Some(Reply::Marker { .. })) && starts && !user;
    let hover_group = format!("{key}-row");
    let content: AnyElement = if user {
        div()
            .max_w(px((content_width - ZORK_UI.thread.user_left_clearance).max(40.)))
            .flex()
            .flex_col()
            .items_end()
            .gap_1()
            .when_some(reply_block, |v, block| v.child(block))
            .child(
                div()
                    .w(px(bubble_width))
                    .rounded(px(ZORK_UI.thread.user_radius))
                    .rounded_br(px(crate::design::RADIUS.fold))
                    .bg(rgb(ZORK_UI.thread.user_fill))
                    .px(px(ZORK_UI.thread.user_padding_x))
                    .py(px(ZORK_UI.thread.user_padding_y))
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .child(prose),
            )
            // A user run shows its time once, under the last bubble.
            .when(ends, |v| {
                v.when_some(time.clone(), |v, time| {
                    v.child(identity::time(key, time, time_full.clone()))
                })
            })
            .into_any_element()
    } else {
        let header = starts.then(|| {
            let lead = match &identity {
                Some(identity) => {
                    identity::header_identity(key, identity, content_width < COMPACT_WIDTH)
                }
                None => div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(device.clone(), |v, device| {
                        v.child(crate::device_name::mark(&device, 18.))
                    })
                    .when_some(device.clone().or(author_name.clone()), |v, name| {
                        v.child(
                            div()
                                .max_w(px(140.))
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .child(name),
                        )
                    })
                    .when_some(model.clone().filter(|m| !m.is_empty()), |v, model| {
                        v.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(rgb(DIM()))
                                .child(model),
                        )
                    })
                    .into_any_element(),
            };
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w_0()
                .child(lead)
        });
        // Identity, then who it answers, then when.
        let (header, reply_block) = match (header, reply_block) {
            (Some(header), Some(block)) if marker_in_header => (Some(header.child(block)), None),
            (header, block) => (header, block),
        };
        let header = header.map(|header| {
            header.when_some(time.clone(), |v, time| {
                v.child(identity::time(key, time, time_full.clone()))
            })
        });
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_size(px(13.))
            .line_height(px(20.))
            .when_some(header, |v, header| v.child(header))
            .when_some(reply_block, |v, block| {
                v.child(div().flex().items_start().child(block))
            })
            .child(prose)
            .into_any_element()
    };
    // A continued agent message reveals its own time at the row end on hover.
    let hover_time = (!user && !starts).then_some(time).flatten().map(|time| {
        div()
            .absolute()
            .top_0()
            .right(px(-4.))
            .pl(px(12.))
            .bg(rgb(if highlighted {
                *crate::design::JUMP_WASH
            } else {
                ZORK_UI.palette.canvas
            }))
            .opacity(0.)
            .group_hover(hover_group.clone(), |s| s.opacity(1.))
            .child(identity::time(&format!("{key}-run"), time, time_full))
    });
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
                .when(highlighted, |v| {
                    v.child(
                        div()
                            .absolute()
                            .top(px(-top.min(8.)))
                            .bottom(px(-bottom.min(8.)))
                            .left(px(-12.))
                            .right(px(-12.))
                            .rounded(px(crate::design::RADIUS.container))
                            .bg(rgb(*crate::design::JUMP_WASH)),
                    )
                })
                .child(content)
                .children(hover_time),
        )
        .into_any()
}

pub mod identity;
#[cfg(feature = "stories")]
pub mod multi_agent;
#[cfg(feature = "stories")]
pub mod stories;

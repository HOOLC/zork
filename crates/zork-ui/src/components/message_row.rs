//! Complete transcript message row shared by the application and Playground.
use super::{
    liquid::overlay::SourceBinding, message::MessageDocument, selection::SelectionContext,
};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::CUE_UI,
};
use gpui::{prelude::*, *};
const TEXT: u32 = CUE_UI.palette.text;
const DIM: u32 = CUE_UI.palette.muted;
const BORDER: u32 = CUE_UI.palette.border;
const BG: u32 = CUE_UI.palette.canvas;

pub struct Row<'a> {
    pub index: usize,
    pub user: bool,
    pub document: &'a MessageDocument,
    pub content_width: f32,
    pub selection: Option<&'a SelectionContext>,
    pub preview_limit: f32,
    pub expanded: bool,
    pub text: crate::resources::Text,
    pub reader_source: SourceBinding,
    pub author_is_agent: bool,
    pub avatar: Option<&'a str>,
    pub author_name: Option<String>,
    pub device: Option<String>,
    pub time: Option<String>,
}
impl Row<'_> {
    pub fn render(
        self,
        window: &mut Window,
        on_expand: impl Fn(&mut Window, &mut App) + 'static,
        on_read: impl Fn(&mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let Self {
            index,
            user,
            document,
            content_width,
            selection,
            preview_limit,
            expanded,
            text,
            reader_source,
            author_is_agent,
            avatar,
            author_name,
            device,
            time,
        } = self;
        // Only user bubbles need intrinsic width. Assistant prose already fills
        // its column, so shaping its full document here duplicated every code run.
        let bubble_width = if user {
            let max_width = (content_width - CUE_UI.thread.user_left_clearance)
                .min(CUE_UI.thread.user_max_width)
                .max(40.);
            let measured = document.bounded_text_width(
                gpui::font("Inter Variable"),
                13.,
                (max_width - 2. * CUE_UI.thread.user_padding_x - 2.).max(1.),
                window,
            );
            // Retain rounding slack so fractional glyph advances do not orphan punctuation.
            let footer_width = if document.is_truncated()
                || document.shared_plain_text().lines().count() as f32 * 20. > preview_limit
            {
                220_f32.min(max_width)
            } else {
                40.
            };
            (measured.ceil() + 2. * CUE_UI.thread.user_padding_x + 2.).clamp(
                footer_width,
                (content_width - CUE_UI.thread.user_left_clearance)
                    .min(CUE_UI.thread.user_max_width)
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
        let width = if user {
            bubble_width - 2. * CUE_UI.thread.user_padding_x
        } else {
            content_width - 34.
        };
        let expand_label = text.text(if expanded {
            "message_collapse"
        } else {
            "message_expand"
        });
        let footer = div()
            .w_full()
            .min_h(px(46.))
            .border_t(gpui::px(crate::design::BORDER_WIDTH))
            .border_color(rgb(BORDER))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .when(width < 190., |v| v.flex_col().items_start())
            .child(
                div()
                    .id(format!("message-expand-{index}"))
                    .min_h(px(44.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .text_color(rgb(DIM))
                    .child(expand_label.clone())
                    .on_click(move |_, window, cx| {
                        on_expand(window, cx);
                    })
                    .automation(AutomationRole::Button, expand_label),
            )
            .child(
                div()
                    .id(format!("message-full-{index}"))
                    .min_h(px(44.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT))
                    .child(text.text("message_view_full"))
                    .child(
                        svg()
                            .path("icons/open.svg")
                            .size(px(14.))
                            .text_color(rgb(TEXT)),
                    )
                    .on_click(move |_, window, cx| {
                        on_read(window, cx);
                    })
                    .map(|button| {
                        reader_source.bind(
                            button,
                            text.text("message_view_full"),
                            crate::controls::ActionStyle {
                                quiet: true,
                                icon: Some("icons/open.svg"),
                                ..Default::default()
                            },
                        )
                    })
                    .automation(AutomationRole::Button, text.text("message_view_full")),
            )
            .into_any_element();
        let prose = crate::components::message_preview::MessagePreview {
            body: prose,
            footer,
            width: width.max(1.),
            limit: preview_limit,
            more: document.is_truncated(),
            expanded,
            fade: true,
            background: rgb(if user { CUE_UI.thread.user_fill } else { BG }).into(),
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
                                .child(
                                    div()
                                        .w_full()
                                        .rounded(px(CUE_UI.thread.user_radius))
                                        .bg(rgb(CUE_UI.thread.user_fill))
                                        .px(px(CUE_UI.thread.user_padding_x))
                                        .py(px(CUE_UI.thread.user_padding_y))
                                        .text_size(px(13.))
                                        .line_height(px(20.))
                                        .child(prose),
                                )
                                .when_some(time, |v, time| {
                                    v.child(
                                        div().text_size(px(10.)).text_color(rgb(DIM)).child(time),
                                    )
                                }),
                        ),
                )
                .into_any(),
            false => transcript_row()
                .child(
                    div()
                        .w(px(content_width))
                        .max_w_full()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(if author_is_agent || avatar.is_some() {
                            crate::controls::agent_avatar(avatar, 26.)
                        } else {
                            div().size(px(26.)).flex_shrink_0().child(
                                svg()
                                    .path("brand/mark.svg")
                                    .size(px(22.))
                                    .text_color(rgb(TEXT)),
                            )
                        })
                        .child(
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
                                        .when_some(author_name, |v, name| {
                                            v.child(
                                                div().font_weight(FontWeight::MEDIUM).child(name),
                                            )
                                        })
                                        .when_some(device, |v, device| {
                                            v.child(
                                                div()
                                                    .max_w(px(140.))
                                                    .truncate()
                                                    .text_size(px(11.))
                                                    .text_color(rgb(DIM))
                                                    .child(device),
                                            )
                                        })
                                        .when_some(time, |v, time| {
                                            v.child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(rgb(DIM))
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

#[cfg(feature = "stories")]
pub mod stories;

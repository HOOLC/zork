//! Complete transcript message row shared by the application and Playground.
use super::{message::MessageDocument, selection::SelectionContext};
use crate::design::ZORK_UI;
use gpui::{prelude::*, *};
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
impl Row<'_> {
    pub fn render(self, window: &mut Window) -> AnyElement {
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

#[cfg(feature = "stories")]
pub mod stories;

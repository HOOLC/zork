//! Native first-use composition shared by the product and the design browser.
use crate::{controls as ui, design::TextRole};
use gpui::{div, prelude::*, px, Div, MouseButton, SharedString};

pub fn frame() -> Div {
    div().size_full().flex().flex_col().child(
        div()
            .h(px(48.))
            .flex_shrink_0()
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move()),
    )
}

pub fn hero(title: impl Into<SharedString>, description: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .max_w(px(408.))
        .px_6()
        .flex()
        .flex_col()
        .items_center()
        .text_center()
        .child(gpui::img("brand/mark-orange.svg").size(px(72.)).mb(px(28.)))
        .child(ui::page_title(title))
        .child(
            div()
                .mt_3()
                .child(ui::text_role(description, TextRole::Description)),
        )
}

pub fn center(body: impl IntoElement) -> Div {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .pb(px(48.))
        .child(body)
}

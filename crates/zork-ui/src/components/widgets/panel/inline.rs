//! Ordinary GPUI card in document flow.
use gpui::{div, prelude::*, px, rgb, Div, SharedString, Stateful};

pub type InlinePanel = Stateful<Div>;

pub fn inline(id: impl Into<SharedString>) -> InlinePanel {
    div()
        .id(id.into())
        .w_full()
        .flex()
        .flex_col()
        .p(px(16.))
        .gap(px(12.))
        .rounded(px(crate::controls::CARD_RADIUS))
        .bg(rgb(crate::design::ZORK_UI.palette.canvas))
        .border(px(crate::design::BORDER_WIDTH))
        .border_color(rgb(crate::design::UI_OUTLINE))
}

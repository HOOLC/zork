//! Native first-use composition shared by the product and the design browser.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::widgets::primitives::{feedback, surface},
    controls::{self as ui, NoticeKind},
    design::TextRole,
};
use gpui::{div, prelude::*, px, rgb, Div, Role, SharedString};

pub fn frame() -> Div {
    div().size_full().flex().flex_col().child(
        div()
            .h(px(48.))
            .flex_shrink_0()
            .window_control_area(gpui::WindowControlArea::Drag),
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

pub fn failure_notice(message: String) -> Div {
    let (ink, fill) = feedback::colors(NoticeKind::Info);
    div().w_auto().max_w(px(340.)).min_w_0().child(
        surface("onboarding-notice-surface", ui::FIELD_RADIUS, fill, false)
            .w_auto()
            .px_3()
            .py_2()
            .child(
                div()
                    .id("onboarding-error")
                    .role(Role::Alert)
                    .aria_label(message.clone())
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(rgb(ink))
                    .child(ui::icon("icons/attention.svg", 14.))
                    .child(div().min_w_0().child(message.clone()))
                    .automation(AutomationRole::Status, message),
            ),
    )
}

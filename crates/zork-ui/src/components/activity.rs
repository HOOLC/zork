//! Compact participant activity line from the approved Zork conversation design.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, Animation, AnimationExt, AnyElement, Div};
use std::{rc::Rc, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub icon: &'static str,
    pub label: String,
    pub summary: String,
    pub failed: bool,
    pub running: bool,
}

/// The live transcript slot has the same 32 + 3 × 26 px geometry in either
/// one-row or three-row mode. The host owns history reads and navigation.
pub fn render_session(
    name: &str,
    stopped: bool,
    leaving: bool,
    animate: bool,
    expanded: bool,
    rows: &[SessionRow],
    more: &str,
    less: &str,
    status: &str,
    on_expand: Rc<dyn Fn(&mut gpui::App)>,
    on_open: Rc<dyn Fn(String, &mut gpui::App)>,
) -> AnyElement {
    let p = ZORK_UI.palette;
    let count = if expanded { 3 } else { 1 };
    let shown = rows.iter().rev().take(count).cloned().collect::<Vec<_>>();
    let view = div()
        .id("session-activity-compact")
        .w_full()
        .h(px(110.))
        .min_w_0()
        .flex()
        .gap(px(8.))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .h(px(110.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(32.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(p.text))
                                .child(name.to_owned()),
                        )
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(rgb(p.subtle))
                                .child(status.to_owned()),
                        )
                        .child(
                            div()
                                .id("session-activity-expand")
                                .tab_stop(true)
                                .cursor_pointer()
                                .text_size(px(10.))
                                .text_color(rgb(p.subtle))
                                .child(if expanded { less } else { more }.to_owned())
                                .on_click(move |_, _, cx| on_expand(cx))
                                .automation(
                                    AutomationRole::Button,
                                    if expanded { less } else { more },
                                ),
                        ),
                )
                .child(
                    div()
                        .h(px(78.))
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .children(shown.into_iter().rev().map(move |row| {
                            let id = row.id.clone();
                            let open = on_open.clone();
                            let accessible = format!("{} {}", row.label, row.summary);
                            div()
                                .id(format!("session-activity-{}", row.id))
                                .tab_stop(true)
                                .h(px(26.))
                                .flex_shrink_0()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .cursor_pointer()
                                .on_click(move |_, _, cx| open(id.clone(), cx))
                                .child(
                                    crate::controls::icon(row.icon, 14.)
                                        .text_color(rgb(if row.failed {
                                            p.danger
                                        } else {
                                            p.subtle
                                        }))
                                        .flex_shrink_0(),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_shrink_0()
                                        .text_size(px(11.))
                                        .text_color(rgb(if row.failed { p.danger } else { p.text }))
                                        .child(row.label),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(rgb(p.subtle))
                                        .child(row.summary),
                                )
                                .when(row.running && !stopped, |v| {
                                    v.child(crate::components::loading::activity(
                                        format!("session-activity-loading-{}", row.id),
                                        animate,
                                    ))
                                })
                                .automation(AutomationRole::Button, accessible)
                        })),
                ),
        );
    if leaving && animate {
        view.with_animation(
            "session-activity-leave",
            Animation::new(Duration::from_millis(220))
                .with_easing(|t| t * t)
                .with_max_fps(60.),
            |view, progress| view.opacity(1. - progress),
        )
        .into_any_element()
    } else if leaving {
        view.opacity(0.).into_any_element()
    } else {
        view.into_any_element()
    }
}

#[derive(Clone)]
pub struct Presentation {
    pub id: String,
    pub name: String,
    pub label: String,
    pub failed: bool,
    pub running: bool,
}

pub fn render(items: &[Presentation], animate: bool) -> Div {
    render_with_id("participant-activity".into(), items, animate)
}
pub fn render_with_id(id: String, items: &[Presentation], animate: bool) -> Div {
    let p = ZORK_UI.palette;
    let label = items
        .iter()
        .map(|item| {
            if item.name == "Agent" && items.len() == 1 {
                item.label.clone()
            } else {
                format!("{} · {}", item.name, item.label)
            }
        })
        .collect::<Vec<_>>()
        .join("；");
    let detail = label.clone();
    div()
        .w_full()
        .h(px(23.))
        .px(px(2.))
        .flex()
        .items_center()
        .gap(px(6.))
        .when(
            items.iter().any(|item| item.running && !item.failed),
            |view| {
                view.child(div().text_color(rgb(p.subtle)).child(
                    crate::components::loading::activity(format!("{id}-loading"), animate),
                ))
            },
        )
        .when(!items.is_empty(), |view| {
            view.child(
                div()
                    .id(id)
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(10.))
                    .line_height(px(16.))
                    .text_color(rgb(if items.iter().any(|item| item.failed) {
                        p.danger
                    } else {
                        p.subtle
                    }))
                    .child(label.clone())
                    .tooltip(move |_, cx| cx.new(|_| ActivityTooltip(detail.clone())).into())
                    .automation(AutomationRole::Status, label),
            )
        })
}

struct ActivityTooltip(String);
impl gpui::Render for ActivityTooltip {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("activity-detail")
            .max_w(px(600.))
            .p_3()
            .rounded(px(crate::controls::COMPACT_CARD_RADIUS))
            .border(gpui::px(crate::design::BORDER_WIDTH))
            .border_color(rgb(ZORK_UI.palette.border))
            .bg(rgb(ZORK_UI.palette.canvas))
            .text_size(px(13.))
            .line_height(px(20.))
            .text_color(rgb(ZORK_UI.palette.text))
            .child(self.0.clone())
            .automation(AutomationRole::Status, self.0.clone())
    }
}

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

/// A quiet one-line capsule: who is working and on what. Expanding adds the
/// last three steps and a link to the full history; the host remeasures it.
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
    // Placeholder steps such as "thinking" carry no information for the reader.
    let steps: Vec<SessionRow> = rows
        .iter()
        .filter(|row| !(row.summary.is_empty() && row.label.contains("思考")))
        .cloned()
        .collect();
    let working = !stopped && rows.iter().any(|row| row.running);
    let latest = steps.last().cloned();
    let step = |row: &SessionRow, strong: bool| {
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                crate::controls::icon(row.icon, 14.)
                    .text_color(rgb(if row.failed { p.danger } else { p.muted }))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(12.5))
                    .text_color(rgb(if row.failed {
                        p.danger
                    } else if strong {
                        p.text
                    } else {
                        p.muted
                    }))
                    .child(row.label.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(crate::assets::CODE_FONT_FAMILY)
                    .text_size(px(12.))
                    .text_color(rgb(p.subtle))
                    .child(row.summary.clone()),
            )
    };
    let toggle_label = if expanded { less } else { more };
    let latest_open = on_open.clone();
    let header = div()
        .h(px(36.))
        .flex_shrink_0()
        .pl(px(12.))
        .pr(px(4.))
        .flex()
        .items_center()
        .gap(px(8.))
        // Persimmon means work in progress; it disappears when the round ends.
        .when(working, |v| {
            v.child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .flex_shrink_0()
                    .bg(rgb(crate::design::INTERACTION.accent)),
            )
        })
        .child(crate::device_name::mark(name, 18.))
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(12.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(rgb(p.text))
                .child(name.to_owned()),
        )
        .map(|v| match (&latest, expanded) {
            (Some(row), false) => {
                let id = row.id.clone();
                let open = latest_open.clone();
                v.child(
                    div()
                        .id(format!("session-activity-{}", row.id))
                        .flex_1()
                        .min_w_0()
                        .cursor_pointer()
                        .on_click(move |_, _, cx| open(id.clone(), cx))
                        .child(step(row, true))
                        .automation(
                            AutomationRole::Button,
                            format!("{} {}", row.label, row.summary),
                        ),
                )
            }
            _ => v.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.))
                    .text_color(rgb(p.subtle))
                    .child(status.to_owned()),
            ),
        })
        .when(!steps.is_empty(), |v| {
            v.child(
                crate::controls::quiet_button(
                    "session-activity-expand",
                    "",
                    true,
                    crate::controls::IconButtonSize::Compact,
                )
                .aria_label(toggle_label.to_owned())
                .child(
                    crate::controls::icon("icons/chevron-down.svg", 12.).when(expanded, |icon| {
                        icon.with_transformation(gpui::Transformation::rotate(gpui::radians(
                            std::f32::consts::PI,
                        )))
                    }),
                )
                .on_click(move |_, _, cx| on_expand(cx))
                .automation(AutomationRole::Button, toggle_label),
            )
        });
    let recent: Vec<SessionRow> = steps.iter().rev().take(3).rev().cloned().collect();
    let history = recent.last().map(|row| row.id.clone());
    let open_history = on_open.clone();
    let view = div()
        .id("session-activity-compact")
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .rounded(px(if expanded { 20. } else { 18. }))
        .bg(rgb(p.prompt))
        .child(header)
        .when(expanded, |v| {
            v.child(
                div()
                    .pl(px(40.))
                    .pr(px(12.))
                    .pb(px(6.))
                    .flex()
                    .flex_col()
                    .children(recent.into_iter().map(move |row| {
                        let id = row.id.clone();
                        let open = on_open.clone();
                        let accessible = format!("{} {}", row.label, row.summary);
                        let current = row.running && !stopped;
                        div()
                            .id(format!("session-activity-{}", row.id))
                            .tab_stop(true)
                            .h(px(26.))
                            .flex_shrink_0()
                            .min_w_0()
                            .px(px(8.))
                            .ml(px(-8.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .hover(|v| v.bg(rgb(crate::design::INTERACTION.neutral_hover)))
                            .focus_visible(|v| v.shadow(crate::controls::focus_ring()))
                            .on_click(move |_, _, cx| open(id.clone(), cx))
                            .child(step(&row, current))
                            .automation(AutomationRole::Button, accessible)
                    }))
                    .when_some(history, |v, id| {
                        v.child(
                            crate::controls::quiet_button(
                                "session-activity-history",
                                "完整历史",
                                true,
                                crate::controls::IconButtonSize::Compact,
                            )
                            .ml(px(-10.))
                            .mt(px(2.))
                            .child(crate::controls::icon("icons/arrow-right.svg", 12.))
                            .on_click(move |_, _, cx| open_history(id.clone(), cx))
                            .automation(AutomationRole::Button, "完整历史"),
                        )
                    }),
            )
        });
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
                    .text_size(px(12.))
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

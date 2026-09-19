//! Compact participant activity line from the approved Zork conversation design.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, Div};

#[derive(Clone)]
pub struct Presentation {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
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
        .children(items.iter().map(|item| {
            div()
                .id(format!("activity-avatar-{}", item.id))
                .size(px(15.))
                .flex_shrink_0()
                .rounded(px(5.))
                .child(ui::agent_avatar(item.avatar.as_deref(), 15.))
        }))
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

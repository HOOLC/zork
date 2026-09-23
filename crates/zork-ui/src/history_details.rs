//! Complete history entry/agent dialog. The host supplies projected data and actions.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::ZORK_UI,
    history::Entry,
    resources::Text,
};
use gpui::{prelude::*, *};
use std::rc::Rc;
#[derive(Clone, PartialEq)]
pub enum Presentation {
    Agent {
        id: String,
        name: String,
        role: Option<String>,
    },
    Entry(Entry),
}
#[derive(Clone)]
pub struct Resource {
    pub label: String,
    pub open: Rc<dyn Fn(&mut App)>,
}
#[derive(Clone)]
struct Data {
    key: String,
    presentation: Presentation,
    resource: Option<Resource>,
}
pub struct Closed;
pub struct Details {
    data: Option<Data>,
    modal: crate::modal::ModalState,
    text: Text,
}
impl EventEmitter<Closed> for Details {}
impl Details {
    pub fn new(text: Text, cx: &mut Context<Self>) -> Self {
        let mut modal = crate::modal::ModalState::new(cx);
        modal.retain::<Data>("history-detail-dialog", None, cx);
        Self {
            data: None,
            modal,
            text,
        }
    }
    pub fn configure(
        &mut self,
        key: String,
        presentation: Option<Presentation>,
        resource: Option<Resource>,
        text: Text,
        cx: &mut Context<Self>,
    ) {
        let updated = self
            .data
            .as_ref()
            .map(|data| (&data.key, &data.presentation))
            != presentation.as_ref().map(|p| (&key, p))
            || self.text.text("history_summary_details") != text.text("history_summary_details");
        self.data = presentation.map(|presentation| Data {
            key,
            presentation,
            resource,
        });
        self.text = text;
        if updated {
            cx.notify();
        }
    }
    pub fn inspect(&self, window: &Window, cx: &App) -> serde_json::Value {
        let material = self.modal.inspect("history-detail-dialog");
        serde_json::json!({"open":self.data.is_some(),"progress":material["progress"],"moving":material["moving"],
            "focused":self.modal.focus.is_focused(window),"contains_focus":self.modal.focus.contains_focused(window,cx),"reduced":cx.reduce_motion()})
    }
}
impl Render for Details {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.modal.sync(
            self.data.as_ref().map(|_| "history-detail-dialog"),
            window,
            cx,
        );
        let Some(data) = self
            .modal
            .retain("history-detail-dialog", self.data.clone(), cx)
        else {
            return gpui::Empty.into_any_element();
        };
        let body = match &data.presentation {
            Presentation::Agent { id, name, role } => div()
                .p_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().text_size(px(16.)).child(name.clone())),
                )
                .child(
                    div()
                        .mt_3()
                        .text_size(px(12.))
                        .text_color(rgb(ZORK_UI.palette.muted))
                        .child(id.clone()),
                )
                .when_some(role.clone(), |v, role| v.child(div().mt_2().child(role)))
                .into_any_element(),
            Presentation::Entry(entry) => {
                let projection = crate::history::activity::Projection::new([entry]);
                let file = projection
                    .activities
                    .first()
                    .and_then(|a| a.details.files.first());
                div()
                    .id("history-detail-scroll")
                    .max_h(px((window.viewport_size().height.as_f32() - 180.).max(120.)))
                    .overflow_y_scroll()
                    .p_3()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .when_some(entry.start.or(entry.end), |v, at| {
                        // The record clock: absolute, 9px, tertiary.
                        v.child(
                            div()
                                .mb(px(6.))
                                .text_size(px(9.))
                                .text_color(rgb(ZORK_UI.palette.subtle))
                                .child(crate::history::clock(Some(at))),
                        )
                    })
                    .child(if let Some(file) = file {
                        div()
                            .mb_3()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(ZORK_UI.palette.subtle))
                                    .mb_3()
                                    .child(file.path.clone()),
                            )
                            .when_some(file.previous.clone(), |v, previous| {
                                v.child(
                                    div()
                                        .font_family(crate::assets::CODE_FONT_FAMILY)
                                        .child(previous),
                                )
                            })
                            .child(
                                div()
                                    .font_family(crate::assets::CODE_FONT_FAMILY)
                                    .text_size(px(11.))
                                    .line_height(px(19.8))
                                    .child(file.content.clone()),
                            )
                    } else {
                        div().mb_3().child(entry.summary.clone())
                    })
                    .when_some(data.resource.clone(), |body, resource| {
                        let open = resource.open;
                        body.child(
                            ui::button(
                                "history-resource-details",
                                resource.label.clone(),
                                false,
                                true,
                            )
                            .on_click(cx.listener(move |_, _, _, cx| open(cx)))
                            .automation(AutomationRole::Button, resource.label),
                        )
                    })
                    .into_any_element()
            }
        };
        let title = self
            .text
            .text(if matches!(data.presentation, Presentation::Agent { .. }) {
                "history_agent_details"
            } else {
                "history_summary_details"
            });
        crate::modal::detail_modal(
            "history-detail-dialog",
            title,
            body,
            None,
            &self.modal,
            window,
            cx,
            true,
            |v, _, cx| {
                v.data = None;
                cx.emit(Closed);
                cx.notify();
            },
        )
    }
}

#[cfg(feature = "stories")]
pub mod stories;

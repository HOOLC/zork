//! Complete member activity hover: anchor changes, retention and dismissal.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        liquid::panel::{Content, FloatingPanel, FloatingStyle, Side},
        tooltip::DetailsTooltip,
    },
    design::CUE_UI,
};
use gpui::{prelude::*, *};
use std::{collections::HashMap, rc::Rc, time::Duration};
#[derive(Clone)]
struct Presentation {
    id: String,
    details: DetailsTooltip,
    footer: String,
}
pub struct Closed(pub String);
#[derive(Default)]
pub struct Overlay {
    active: Option<Presentation>,
    retained: Option<Presentation>,
    panel: FloatingPanel,
    anchor: Bounds<Pixels>,
    anchors: HashMap<String, Bounds<Pixels>>,
    trigger_hover: bool,
    panel_hover: bool,
    close: Option<Task<()>>,
}
impl EventEmitter<Closed> for Overlay {}
impl Overlay {
    pub fn show(
        &mut self,
        id: String,
        details: DetailsTooltip,
        footer: String,
        anchor: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.close.take();
        self.trigger_hover = true;
        self.panel_hover = false;
        self.anchor = anchor;
        self.active = Some(Presentation {
            id,
            details,
            footer,
        });
        cx.notify();
    }
    pub fn update(
        &mut self,
        id: &str,
        details: DetailsTooltip,
        footer: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(active) = self.active.as_mut().filter(|active| active.id == id) {
            active.details = details;
            active.footer = footer;
            cx.notify();
        }
    }
    pub fn anchor(&mut self, id: &str, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.anchors.insert(id.into(), bounds) != Some(bounds) {
            if self.active.as_ref().is_some_and(|active| active.id == id) {
                self.anchor = bounds;
            }
            if self.active.is_some() {
                cx.notify();
            }
        }
    }
    pub fn retain(&mut self, ids: &[String], cx: &mut Context<Self>) {
        self.anchors.retain(|id, _| ids.contains(id));
        if self
            .active
            .as_ref()
            .is_some_and(|active| !ids.contains(&active.id))
        {
            self.dismiss(cx);
        }
    }
    pub fn leave(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.active.as_ref().is_some_and(|active| active.id == id) {
            self.trigger_hover = false;
            self.schedule_close(cx);
        }
    }
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.close = None;
        if let Some(active) = self.active.take() {
            cx.emit(Closed(active.id));
        }
        cx.notify();
    }
    fn schedule_close(&mut self, cx: &mut Context<Self>) {
        self.close.take();
        if self.trigger_hover || self.panel_hover {
            return;
        }
        self.close = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
            let _ = view.update(cx, |v, cx| v.dismiss(cx));
        }));
    }
}
impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.active.is_some();
        if let Some(active) = &self.active {
            self.retained = Some(active.clone());
        }
        let Some(presentation) = &self.retained else {
            return gpui::Empty.into_any_element();
        };
        let details = &presentation.details;
        let content = div()
            .id(format!("detail-tooltip-{}", details.key))
            .w_full()
            .text_color(rgb(CUE_UI.palette.text))
            .child(details.content())
            .child(
                div()
                    .mt(px(12.))
                    .text_size(px(10.))
                    .text_color(rgb(CUE_UI.palette.muted))
                    .child(presentation.footer.clone()),
            )
            .automation(
                AutomationRole::Status,
                format!("{}详情：{}", details.kind, details.title),
            )
            .into_any_element();
        let mut anchor = self.anchor;
        for bounds in self.anchors.values() {
            anchor.origin.y = anchor.origin.y.min(bounds.top());
        }
        let hover = Rc::new(cx.listener(|v, hovered: &bool, _, cx| {
            v.panel_hover = *hovered;
            v.schedule_close(cx);
        }));
        let panel = self.panel.render(
            "composer-member-popup-surface",
            anchor,
            open,
            FloatingStyle::details(320., Side::Above),
            Content {
                sections: vec![content],
                padding: 16.,
                gap: 0.,
            },
            Some(hover),
            window,
            cx,
        );
        if !open && !self.panel.alive() {
            self.retained = None;
        }
        panel.unwrap_or_else(|| gpui::Empty.into_any_element())
    }
}

#[cfg(feature = "stories")]
pub mod stories;

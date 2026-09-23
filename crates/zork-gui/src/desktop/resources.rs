//! Core subscription and intent adapter for the shared resource component.
use crate::i18n::Locale;
use gpui::{prelude::*, Context, Entity, Task, Window};
use std::{rc::Rc, sync::Arc};
use zork_client_core::resources::{Inspection, Resources, ResourcesData};
use zork_ui::{
    components::frame_delivery::FrameDelivery,
    resources::{Mode, Refresh, Text},
};

pub struct ResourcesView {
    core: Arc<Resources>,
    view: Entity<zork_ui::resources::ResourcesView>,
    updates: zork_client_core::state::Subscription<ResourcesData>,
    frame: FrameDelivery,
    _task: Task<()>,
}
impl ResourcesView {
    pub fn services(
        core: Arc<Resources>,
        node: String,
        locale: Locale,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(core, locale, Mode::Services(node), cx)
    }
    pub fn inspector(
        core: Arc<Resources>,
        node: String,
        query: Inspection,
        locale: Locale,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = Self::create(core, locale, Mode::Inspector, cx);
        view.view.update(cx, |v, cx| v.select(node, query, cx));
        view
    }
    pub fn unavailable(
        core: Arc<Resources>,
        error: String,
        locale: Locale,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = Self::create(core, locale, Mode::Inspector, cx);
        view.view.update(cx, |v, cx| v.unavailable(error, cx));
        view
    }
    #[cfg(feature = "headless-bench")]
    pub fn fixture(data: ResourcesData, locale: Locale, cx: &mut Context<Self>) -> Self {
        Self::create(Resources::fixture(data), locale, Mode::Inspector, cx)
    }
    fn create(core: Arc<Resources>, locale: Locale, mode: Mode, cx: &mut Context<Self>) -> Self {
        let mut updates = core.subscribe();
        let data = updates.snapshot();
        let mut readiness = updates.readiness();
        let refresh = !matches!(mode, Mode::Inspector);
        let view =
            cx.new(|cx| zork_ui::resources::ResourcesView::new(data, text(locale), mode, cx));
        cx.subscribe(&view, |v, _, event: &Refresh, cx| {
            let core = v.core.clone();
            let query = event.query.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Some((node, query)) = query {
                        core.inspect(&node, query).await;
                    } else {
                        core.refresh().await;
                    }
                })
                .detach();
        })
        .detach();
        let task = cx.spawn(async move |view, cx| {
            while readiness.changed().await.is_ok() {
                if readiness.take_urgent() {
                    if view.update(cx, |v, cx| v.deliver(cx)).is_err() {
                        return;
                    }
                } else if !FrameDelivery::request(&view, cx, |v| &mut v.frame) {
                    return;
                }
            }
        });
        let result = Self {
            core,
            view,
            updates,
            frame: Default::default(),
            _task: task,
        };
        if refresh {
            result.refresh(cx);
        }
        result
    }
    fn deliver(&mut self, cx: &mut Context<Self>) {
        if let Some(batch) = self.updates.prepare() {
            let id = batch.id;
            self.view
                .update(cx, |v, cx| v.set_data(batch.snapshot.value.clone(), cx));
            self.updates.acknowledge(id);
        }
    }

    pub fn refresh(&self, cx: &mut Context<Self>) {
        self.view.update(cx, |v, cx| v.refresh(cx));
    }
}
impl Render for ResourcesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.frame.enter() {
            self.deliver(cx);
        }
        self.view.clone()
    }
}
fn text(locale: Locale) -> Text {
    Text(Rc::new(move |key| locale.text(key).into()))
}

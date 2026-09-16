//! Core and platform adapter for the complete shared-file browser.
use crate::i18n::Locale;
use gpui::{prelude::*, Context, Entity, Task, Window};
use std::{rc::Rc, sync::Arc};
use zork_client_core::shared_files::{Action, SharedFiles, SharedFilesData};
use zork_ui::{components::frame_delivery::FrameDelivery, resources::Text};
pub struct SharedFilesView {
    core: Arc<SharedFiles>,
    data: Arc<SharedFilesData>,
    view: Entity<zork_ui::shared_files::SharedFilesView>,
    updates: zork_client_core::state::Subscription<SharedFilesData>,
    frame: FrameDelivery,
    _task: Task<()>,
    image_requested: Option<String>,
    image_task: Option<Task<()>>,
    save_picker: Option<String>,
}
impl SharedFilesView {
    pub fn new(core: Arc<SharedFiles>, locale: Locale, cx: &mut Context<Self>) -> Self {
        Self::create(core, locale, true, cx)
    }
    #[cfg(feature = "headless-bench")]
    pub fn rendering_fixture(
        core: Arc<SharedFiles>,
        locale: Locale,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(core, locale, false, cx)
    }
    fn create(
        core: Arc<SharedFiles>,
        locale: Locale,
        activate: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut updates = core.subscribe();
        let data = updates.snapshot();
        let mut readiness = updates.readiness();
        let view = cx.new(|cx| {
            zork_ui::shared_files::SharedFilesView::new(
                data.clone(),
                text(locale),
                Rc::new(|time| {
                    chrono::DateTime::from_timestamp_nanos(time)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                }),
                cx,
            )
        });
        cx.subscribe(&view, |v, _, action: &Action, cx| v.run(action.clone(), cx))
            .detach();
        let task = cx.spawn(async move |view, cx| {
            while readiness.changed().await.is_ok() {
                if readiness.take_urgent() {
                    if view.update(cx, |v, cx| v.deliver(cx)).is_err() {
                        return;
                    }
                } else if !FrameDelivery::request(&view, cx, |v| &mut v.frame, Self::deliver) {
                    return;
                }
            }
        });
        let mut result = Self {
            core,
            data,
            view,
            updates,
            frame: Default::default(),
            _task: task,
            image_requested: None,
            image_task: None,
            save_picker: None,
        };
        result.project_image(cx);
        if activate {
            result.run(Action::Activate { active: true }, cx);
        }
        result
    }
    pub fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.view.update(cx, |v, cx| v.set_width(width, cx));
    }
    pub fn set_active(&self, active: bool, cx: &Context<Self>) {
        self.run(Action::Activate { active }, cx);
    }
    pub fn set_locale(&mut self, locale: Locale, cx: &mut Context<Self>) {
        self.view.update(cx, |v, cx| v.set_text(text(locale), cx));
    }
    fn run(&self, action: Action, cx: &Context<Self>) {
        let core = self.core.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = core.dispatch(action).await;
            })
            .detach();
    }
    fn deliver(&mut self, cx: &mut Context<Self>) {
        if let Some(batch) = self.updates.prepare() {
            let id = batch.id;
            self.data = batch.snapshot.value.clone();
            self.view
                .update(cx, |v, cx| v.set_data(self.data.clone(), cx));
            self.updates.acknowledge(id);
            self.project_image(cx);
            self.pick_save(cx);
            cx.notify();
        }
    }
    fn project_image(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = &self.data.preview else {
            self.image_requested = None;
            self.image_task = None;
            self.view.update(cx, |v, cx| v.set_image(None, cx));
            return;
        };
        if let Some(bytes) = preview
            .bytes
            .as_ref()
            .filter(|_| preview.mime.starts_with("image/"))
        {
            if self.image_requested.as_ref() != Some(&preview.selected) {
                let format = match preview.mime.as_str() {
                    "image/jpeg" => gpui::ImageFormat::Jpeg,
                    "image/webp" => gpui::ImageFormat::Webp,
                    "image/gif" => gpui::ImageFormat::Gif,
                    _ => gpui::ImageFormat::Png,
                };
                let root = preview.selected.clone();
                let bytes = bytes.clone();
                let renderer = cx.svg_renderer();
                self.view.update(cx, |v, cx| v.set_image(None, cx));
                self.image_requested = Some(root.clone());
                self.image_task = Some(cx.spawn(async move |view, cx| {
                    let image =
                        cx.background_executor()
                            .spawn(async move {
                                crate::views::shared_file_image(&bytes, format, &renderer)
                            })
                            .await
                            .ok();
                    let _ = view.update(cx, |v, cx| {
                        if v.image_requested.as_ref() == Some(&root) {
                            v.view.update(cx, |v, cx| {
                                v.set_image(image.map(|image| (root, image)), cx)
                            });
                            cx.notify();
                        }
                    });
                }));
            }
        } else {
            self.view.update(cx, |v, cx| v.set_image(None, cx));
            self.image_requested = None;
            self.image_task = None;
        }
    }
    fn pick_save(&mut self, cx: &mut Context<Self>) {
        let Some(ticket) = self.data.save.ticket.clone() else {
            return;
        };
        if self.save_picker.as_ref() == Some(&ticket) {
            return;
        }
        self.save_picker = Some(ticket.clone());
        let picker = cx.prompt_for_new_path(
            &zork_client_core::file_io::default_destination(),
            Some(&self.data.save.name),
        );
        let source = self.core.clone();
        cx.spawn(async move |view, cx| {
            let result = picker.await;
            match result {
                Ok(Ok(Some(path))) => {
                    let token = ticket.clone();
                    cx.background_executor()
                        .spawn(async move {
                            let _ = source.save_copy(&token, &path);
                        })
                        .await;
                }
                _ => {
                    let _ = source
                        .dispatch(Action::CancelSave {
                            ticket: ticket.clone(),
                        })
                        .await;
                }
            }
            let _ = view.update(cx, |v, cx| {
                v.save_picker = None;
                cx.notify();
            });
        })
        .detach();
    }
}
impl Render for SharedFilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.frame.enter(window) {
            self.deliver(cx);
        }
        self.pick_save(cx);
        self.view.clone()
    }
}
fn text(locale: Locale) -> Text {
    Text(Rc::new(move |key| locale.text(key).into()))
}

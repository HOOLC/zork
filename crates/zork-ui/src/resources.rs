//! Complete resource list and inspector; hosts supply snapshots and read intents.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::message::{render_document, MessageDocument},
    controls as ui,
    design::TextRole,
};
use gpui::{div, prelude::*, px, rgb, Context, Div, Render, Window};
use std::{rc::Rc, sync::Arc};
use zork_client_types::resources::{Inspection, InspectionContent, ResourceKind, ResourcesData};
#[derive(Clone)]
pub struct Text(pub Rc<dyn Fn(&str) -> String>);
impl Text {
    pub fn text(&self, key: &str) -> String {
        (self.0)(key)
    }
}
pub struct Refresh {
    pub query: Option<(String, Inspection)>,
}
impl gpui::EventEmitter<Refresh> for ResourcesView {}
#[derive(Clone)]
pub enum Mode {
    Services(String),
    Inspector,
}

#[derive(Clone)]
struct DetailPresentation {
    title: String,
    selected: Option<(String, Inspection)>,
    error: Option<String>,
    data: Arc<ResourcesData>,
    document: Option<(String, bool, MessageDocument)>,
    back: bool,
    facts_open: bool,
}

pub struct ResourcesView {
    data: Arc<ResourcesData>,
    locale: Text,
    mode: Mode,
    rows: Arc<Vec<(usize, usize)>>,
    selected: Option<(String, Inspection)>,
    opening_error: Option<String>,
    trail: Vec<(String, Inspection)>,
    modal: ui::ModalState,
    document: Option<(String, bool, MessageDocument)>,
    facts_open: bool,
    focus_pending: bool,
}
impl ResourcesView {
    pub fn new(data: Arc<ResourcesData>, locale: Text, mode: Mode, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            data,
            locale,
            mode,
            rows: Arc::new(vec![]),
            selected: None,
            opening_error: None,
            trail: vec![],
            modal: ui::ModalState::new(cx),
            document: None,
            facts_open: false,
            focus_pending: false,
        };
        view.project();
        view
    }
    pub fn set_data(&mut self, data: Arc<ResourcesData>, cx: &mut Context<Self>) {
        self.data = data;
        self.project();
        cx.notify();
    }
    pub fn set_text(&mut self, text: Text, cx: &mut Context<Self>) {
        self.locale = text;
        cx.notify();
    }
    pub fn unavailable(&mut self, error: String, cx: &mut Context<Self>) {
        self.opening_error = Some(error);
        cx.notify();
    }
    pub fn refresh(&self, cx: &mut Context<Self>) {
        let query = self.selected.clone();
        cx.emit(Refresh { query });
    }
    fn project(&mut self) {
        if self
            .selected
            .as_ref()
            .is_some_and(|(node, _)| !self.data.devices.iter().any(|device| &device.id == node))
        {
            self.selected = None;
            self.trail.clear();
            self.document = None;
        }
        self.rows = Arc::new(match &self.mode {
            Mode::Services(node) => self.data.rows(ResourceKind::Service, Some(node)),
            _ => vec![],
        });
        let source = self
            .selected
            .as_ref()
            .and_then(|(node, query)| self.data.inspection(node, query))
            .and_then(|state| state.content.as_deref())
            .and_then(|content| match content {
                InspectionContent::Details(details) => details.document.as_ref().map(|d| {
                    (
                        d.text.clone(),
                        d.path.ends_with(".md") || d.path.ends_with(".markdown"),
                    )
                }),
            });
        if self
            .document
            .as_ref()
            .map(|(text, markdown, _)| (text, *markdown))
            != source.as_ref().map(|(text, markdown)| (text, *markdown))
        {
            self.document = source.map(|(text, markdown)| {
                let doc = if markdown {
                    MessageDocument::parse(&text)
                } else {
                    MessageDocument::plain(&text)
                };
                (text, markdown, doc)
            });
        }
    }
    pub fn select(&mut self, node: String, query: Inspection, cx: &mut Context<Self>) {
        if let Some(previous) = self.selected.replace((node, query)) {
            self.trail.push(previous);
        }
        self.facts_open = false;
        self.focus_pending = true;
        self.project();
        self.refresh(cx);
        cx.notify();
    }
    fn back(&mut self, cx: &mut Context<Self>) {
        self.focus_pending = true;
        self.selected = self.trail.pop();
        self.project();
        // The shared bounded cache may have evicted an earlier file while the
        // user explored another resource. Returning is an explicit read intent.
        self.refresh(cx);
        cx.notify();
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.opening_error = None;
        self.trail.clear();
        self.document = None;
        cx.notify();
    }
    fn title(&self) -> String {
        self.selected
            .as_ref()
            .and_then(|(node, query)| self.data.inspection(node, query))
            .and_then(|s| s.content.as_deref())
            .and_then(|content| match content {
                InspectionContent::Details(d) => Some(
                    d.document
                        .as_ref()
                        .map(|f| f.path.clone())
                        .filter(|_| !self.trail.is_empty())
                        .unwrap_or_else(|| d.title.clone()),
                ),
            })
            .unwrap_or_else(|| self.locale.text("resource_details").into())
    }
    fn notice(&self, text: String) -> Div {
        div().py_2().child(ui::feedback(text))
    }
    fn list(&self, cx: &mut Context<Self>) -> Div {
        let kind = ResourceKind::Service;
        let title = self.locale.text("device_services");
        let mut body = div().w_full().flex().flex_col().gap_3().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(ui::text_role(title, TextRole::SectionTitle))
                .child(
                    ui::icon_button("resource-refresh", true)
                        .child(ui::icon("icons/reload.svg", 14.))
                        .on_click(cx.listener(|view, _, _, cx| view.refresh(cx)))
                        .automation(AutomationRole::Button, self.locale.text("refresh")),
                ),
        );
        let scope = match &self.mode {
            Mode::Services(node) => Some(node.as_str()),
            _ => None,
        };
        for device in self
            .data
            .devices
            .iter()
            .filter(|d| scope.is_none_or(|id| d.id == id))
        {
            if let Some(error) = &device.error {
                body = body.child(self.notice(format!(
                    "{} · {error}",
                    crate::device_name::summary(&device.name, &device.status, Some(&self.locale))
                )));
            }
            if let Some(catalog) = &device.catalog {
                for issue in catalog.issues.iter().filter(|issue| issue.kind == kind) {
                    body = body.child(self.notice(format!(
                        "{} · {}",
                        crate::device_name::summary(
                            &device.name,
                            &device.status,
                            Some(&self.locale)
                        ),
                        issue.error
                    )));
                }
            }
        }
        if self.rows.is_empty() {
            let failed = self
                .data
                .devices
                .iter()
                .filter(|device| scope.is_none_or(|id| device.id == id))
                .any(|device| {
                    device.error.is_some()
                        || device.catalog.as_ref().is_some_and(|catalog| {
                            catalog.issues.iter().any(|issue| issue.kind == kind)
                        })
                });
            if failed {
                return body;
            }
            return body.child(ui::text_role(
                self.locale
                    .text(if self.data.devices.iter().any(|d| d.loading) {
                        "resource_loading"
                    } else {
                        "device_services_empty"
                    }),
                TextRole::Description,
            ));
        }
        let rows = self.rows.clone();
        let data = self.data.clone();
        let locale = self.locale.clone();
        body = body.child(
            gpui::uniform_list(
                "registered-resources",
                rows.len(),
                cx.processor(move |_: &mut Self, range: std::ops::Range<usize>, _, cx| {
                    range
                        .map(|index| {
                            let (di, ri) = rows[index];
                            let device = &data.devices[di];
                            let item = &device.catalog.as_ref().unwrap().items[ri];
                            let node = device.id.clone();
                            let query = Inspection::Service {
                                id: item.id.clone(),
                                log: None,
                            };
                            let meta = if item.description.is_empty() {
                                format!(
                                    "{} · {}",
                                    crate::device_name::summary(
                                        &device.name,
                                        &device.status,
                                        Some(&locale)
                                    ),
                                    status(&locale, &item.status)
                                )
                            } else {
                                format!(
                                    "{} · {} · {}",
                                    item.description,
                                    crate::device_name::summary(
                                        &device.name,
                                        &device.status,
                                        Some(&locale)
                                    ),
                                    status(&locale, &item.status)
                                )
                            };
                            div().h(px(56.)).pb_2().child(
                                crate::components::attachments::content_row(
                                    format!("resource-row-{index}"),
                                    "icons/node.svg",
                                    item.name.clone(),
                                    meta,
                                    cx,
                                    move |view, cx| view.select(node.clone(), query.clone(), cx),
                                ),
                            )
                        })
                        .collect()
                }),
            )
            .h(px((self.rows.len().min(8) * 56) as f32))
            .w_full(),
        );
        body
    }
    fn detail_presentation(&self) -> DetailPresentation {
        DetailPresentation {
            title: self.title(),
            selected: self.selected.clone(),
            error: self.opening_error.clone(),
            data: self.data.clone(),
            document: self.document.clone(),
            back: !self.trail.is_empty(),
            facts_open: self.facts_open,
        }
    }
    fn render_detail(&self, shown: &DetailPresentation, cx: &mut Context<Self>) -> Div {
        if let Some(error) = &shown.error {
            return self.notice(error.clone());
        }
        let Some((node, query)) = &shown.selected else {
            return div();
        };
        let mut body = div().w_full().flex().flex_col().gap_3();
        if shown.back {
            body = body.child(
                ui::quiet_button("resource-back", "", true, ui::IconButtonSize::Compact)
                    .self_start()
                    .child(ui::icon("icons/arrow-left.svg", 14.))
                    .child(self.locale.text("back"))
                    .on_click(cx.listener(|view, _, _, cx| view.back(cx)))
                    .automation(AutomationRole::Button, self.locale.text("back")),
            );
        }
        let Some(state) = shown.data.inspection(node, query) else {
            if let Some(error) = shown
                .data
                .devices
                .iter()
                .find(|device| &device.id == node)
                .and_then(|device| device.error.as_ref())
            {
                return body.child(self.notice(error.clone())).child(
                    ui::button("resource-retry", self.locale.text("retry"), false, true)
                        .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
                );
            }
            return body.child(ui::text_role(
                self.locale.text("resource_loading"),
                TextRole::Description,
            ));
        };
        if state.loading {
            body = body.child(ui::text_role(
                self.locale.text("resource_loading"),
                TextRole::Description,
            ));
        }
        if let Some(error) = &state.error {
            body = body.child(self.notice(error.clone())).child(
                ui::button(
                    "resource-retry",
                    self.locale.text("retry"),
                    false,
                    !state.loading,
                )
                .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
            );
        }
        let Some(InspectionContent::Details(details)) = state.content.as_deref() else {
            return body;
        };
        if let Some(device) = shown.data.devices.iter().find(|d| &d.id == node) {
            body = body.child(crate::device_name::label(
                "resource-device-name",
                device.name.clone(),
                &device.status,
                Some(&self.locale),
            ));
        }
        if !details.description.is_empty() {
            body = body.child(ui::text_role(
                details.description.clone(),
                TextRole::Description,
            ));
        }
        for (key, value) in &details.facts {
            if matches!(key.as_str(), "last_error" | "inspection_error") {
                body = body.child(self.notice(value.clone()));
            }
        }
        if let Some((_, _, document)) = &shown.document {
            body = body.child(
                div()
                    .id("resource-document-scroll")
                    .w_full()
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .child(render_document("resource-document", document)),
            );
            if details.document.as_ref().is_some_and(|d| d.truncated) {
                body = body.child(ui::text_role(
                    self.locale.text("resource_document_truncated"),
                    TextRole::Metadata,
                ));
            }
        }
        if !details.files.is_empty() {
            body = body.child(ui::text_role(
                self.locale
                    .text(if matches!(query, Inspection::Service { .. }) {
                        "resource_logs"
                    } else {
                        "resource_attached_files"
                    }),
                TextRole::SectionTitle,
            ));
            for file in &details.files {
                let node = node.clone();
                let next = match query {
                    Inspection::Service { id, .. } => Some(Inspection::Service {
                        id: id.clone(),
                        log: Some(file.path.clone()),
                    }),
                };
                if let Some(next) = next {
                    body = body.child(crate::components::attachments::content_row(
                        format!("resource-file-{}", file.path),
                        "icons/file.svg",
                        file.path.clone(),
                        String::new(),
                        cx,
                        move |view, cx| view.select(node.clone(), next.clone(), cx),
                    ));
                }
            }
        }
        body = body.child(
            ui::quiet_button(
                "resource-more",
                self.locale.text("resource_information"),
                true,
                ui::IconButtonSize::Compact,
            )
            .self_start()
            .child(ui::icon("icons/phosphor-caret-down.svg", 12.))
            .on_click(cx.listener(|view, _, _, cx| {
                view.facts_open = !view.facts_open;
                cx.notify();
            }))
            .automation(
                AutomationRole::Button,
                self.locale.text("resource_information"),
            ),
        );
        if shown.facts_open {
            for (key, value) in &details.facts {
                if matches!(
                    key.as_str(),
                    "files_truncated" | "last_error" | "inspection_error"
                ) {
                    continue;
                }
                let label = self.locale.text(match key.as_str() {
                    "status" => "resource_status",
                    "mode" => "resource_mode",
                    "port" => "resource_port",
                    "shared" => "resource_sharing",
                    "owner_session" => "resource_owner",
                    "last_started_at" => "resource_started",
                    "source" => "resource_source",
                    "path" => "resource_path",
                    "protocol" => "resource_protocol",
                    _ => "resource_information",
                });
                let value = if key == "status" {
                    status(&self.locale, value)
                } else {
                    value.clone()
                };
                body = body.child(
                    div()
                        .flex()
                        .gap_4()
                        .child(ui::text_role(label, TextRole::Label).w(px(90.)))
                        .child(ui::text_role(value, TextRole::Body).flex_1().min_w_0()),
                );
            }
        }
        body
    }
}
impl Render for ResourcesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let detail_open = (self.selected.is_some() || self.opening_error.is_some());
        self.modal
            .sync(detail_open.then_some("resource-detail-modal"), window, cx);
        let presentation = detail_open.then(|| self.detail_presentation());
        let displayed = self.modal.retain("resource-detail-modal", presentation, cx);
        if self.focus_pending {
            window.focus(&self.modal.focus, cx);
            self.focus_pending = false;
        }
        let mut body = if matches!(self.mode, Mode::Inspector) {
            div()
        } else {
            self.list(cx)
        };
        if let Some(displayed) = displayed {
            body = body.child(ui::detail_modal(
                "resource-detail-modal",
                displayed.title.clone(),
                self.render_detail(&displayed, cx),
                None,
                &self.modal,
                window,
                cx,
                true,
                |view, _, cx| view.close(cx),
            ));
        }
        body.font_family("Inter Variable")
            .text_size(px(13.))
            .text_color(rgb(crate::design::ZORK_UI.palette.text))
    }
}
fn status(locale: &Text, value: &str) -> String {
    locale
        .text(match value {
            "ready" => "resource_ready",
            "disabled" => "resource_disabled",
            "running" => "resource_running",
            "stopped" => "resource_stopped",
            "external" => "resource_external",
            "starting" => "resource_starting",
            "failed" => "resource_failed",
            "auth_required" => "resource_auth_required",
            "unprobed" => "resource_unprobed",
            _ => return value.into(),
        })
        .into()
}

#[cfg(feature = "stories")]
pub mod stories;

use super::worker::Worker;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::{ComposerInput, ComposerSubmit},
    i18n::Locale,
};
use gpui::{
    div, img, prelude::*, px, rgb, Bounds, Context, Entity, EventEmitter, FocusHandle, Pixels,
    Task, Window,
};
use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use zork_client_core::desktop::browser_engine::{Action, Inspection, Tab};
use zork_ui::components::frame_delivery::FrameDelivery;
#[path = "chrome.rs"]
mod chrome;
#[path = "input.rs"]
mod input;

pub struct BrowserSelection {
    pub host: String,
    pub inspection: Inspection,
}
/// Native application pages share the browser tab strip and panel geometry.
pub use zork_ui::browser_chrome::NativePage;
pub struct NativePageClosed(pub String);
pub struct BrowserVisibility;
pub struct BrowserResized;
impl EventEmitter<BrowserSelection> for BrowserPanel {}
impl EventEmitter<BrowserVisibility> for BrowserPanel {}
impl EventEmitter<BrowserResized> for BrowserPanel {}
impl EventEmitter<NativePageClosed> for BrowserPanel {}
pub(crate) struct PanelMotion {
    from: f32,
    to: f32,
    velocity: f32,
    began: Instant,
}
impl PanelMotion {
    pub(crate) fn stationary(value: f32, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            velocity: 0.,
            began: now,
        }
    }
    pub(crate) fn retarget(&mut self, target: f32, now: Instant, snap: bool) {
        if snap {
            *self = Self::stationary(target, now);
        } else if target != self.to {
            let (from, velocity) = self.sample(now);
            *self = Self {
                from,
                to: target,
                velocity,
                began: now,
            };
        }
    }
    pub(crate) fn sample(&self, now: Instant) -> (f32, f32) {
        let t = now.duration_since(self.began).as_secs_f32();
        if t >= 0.28 {
            return (self.to, 0.);
        }
        let omega = 36.;
        let d = self.from - self.to;
        let c = self.velocity + omega * d;
        let decay = (-omega * t).exp();
        (
            self.to + (d + c * t) * decay,
            (self.velocity - omega * c * t) * decay,
        )
    }
}

/// Presentation belongs to a conversation host, just like its web tabs.
#[derive(Default)]
struct HostPresentation {
    native_pages: Vec<NativePage>,
    active_native: Option<String>,
    open: bool,
    expanded: bool,
    custom_width: Option<f32>,
    address: String,
    shown: Option<(String, String)>,
}

pub struct BrowserPanel {
    applications: Arc<Vec<zork_client_core::pages::ApplicationEntry>>,
    presentations: HashMap<String, HostPresentation>,
    native_pages: Vec<NativePage>,
    active_native: Option<String>,
    locale: Locale,
    width: f32,
    available_width: f32,
    display_width: f32,
    motion: PanelMotion,
    device_scale: f32,
    expanded: bool,
    custom_width: Option<f32>,
    panel_resizing: bool,
    resize_offset: f32,
    resize_max_width: f32,
    chrome: zork_ui::browser_chrome::Chrome,
    connection: Option<(Arc<crate::api::StationClient>, String)>,
    grants: HashMap<String, super::bridge::Grant>,
    grant_connected: bool,
    worker: Worker,
    host: String,
    selected: HashMap<String, String>,
    blank: std::collections::HashSet<String>,
    open: bool,
    visible: bool,
    tabs: Vec<Tab>,
    address: Entity<ComposerInput>,
    shown: Option<(String, String)>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    viewport: Option<(String, u32, u32)>,
    resizing: bool,
    awaiting_resize_paint: bool,
    frame: Option<Arc<gpui::RenderImage>>,
    decoding: bool,
    sequence: u64,
    page_size: (f64, f64),
    generation: u64,
    focus: FocusHandle,
    ime_text: String,
    ime_selection: std::ops::Range<usize>,
    caret: gpui::Point<Pixels>,
    inspecting: bool,
    busy: usize,
    error: Option<String>,
    wake: Arc<tokio::sync::Notify>,
    _observe: Task<()>,
    core_dirty: bool,
    frame_delivery: FrameDelivery,
}
impl BrowserPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<Worker>() {
            let worker = Worker::new();
            worker.install_shutdown(cx);
            cx.set_global(worker);
        }
        let worker = cx.global::<Worker>().clone();
        let address = cx.new(|cx| {
            ComposerInput::new(Locale::default().text("browser_address_placeholder"), cx)
                .single_line()
        });
        cx.subscribe(&address, |panel, _, _: &ComposerSubmit, cx| {
            panel.submit_address(cx);
        })
        .detach();
        cx.on_release(|panel, cx| panel.set_frame(None, cx))
            .detach();
        let wake = Arc::new(tokio::sync::Notify::new());
        let changed = wake.clone();
        let source = worker.clone();
        let observe = cx.spawn(async move |weak, cx| {
            let mut host = String::new();
            let mut updates = source.subscribe(&host);
            loop {
                updates.checkpoint();
                let Ok((next, visible)) = weak.update(cx, |panel, _| {
                    panel.core_dirty = true;
                    (panel.host.clone(), panel.open && panel.visible)
                }) else {
                    break;
                };
                if visible && !FrameDelivery::request(&weak, cx, |panel| &mut panel.frame_delivery)
                {
                    break;
                }
                if next != host {
                    host = next;
                    updates = source.subscribe(&host);
                    continue;
                }
                tokio::select! {
                    result = updates.changed() => if result.is_err() { break; },
                    _ = changed.notified() => {},
                }
            }
        });
        Self {
            applications: Arc::new(vec![]),
            presentations: HashMap::new(),
            native_pages: Vec::new(),
            active_native: None,
            locale: Locale::default(),
            width: 520.,
            available_width: 520.,
            display_width: 0.,
            motion: PanelMotion {
                from: 0.,
                to: 0.,
                velocity: 0.,
                began: cx.background_executor().now(),
            },
            device_scale: 1.,
            expanded: false,
            custom_width: None,
            panel_resizing: false,
            resize_offset: 0.,
            resize_max_width: f32::MAX,
            chrome: zork_ui::browser_chrome::Chrome::new(
                address.clone(),
                zork_ui::resources::Text(Rc::new(|key| Locale::default().text(key).into())),
                cx,
            ),
            connection: None,
            grants: HashMap::new(),
            grant_connected: false,
            worker,
            host: "global".into(),
            selected: HashMap::new(),
            blank: Default::default(),
            open: false,
            visible: false,
            tabs: vec![],
            address,
            shown: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            viewport: None,
            resizing: false,
            awaiting_resize_paint: false,
            frame: None,
            decoding: false,
            sequence: 0,
            page_size: (1., 1.),
            generation: 0,
            focus: cx.focus_handle(),
            ime_text: String::new(),
            ime_selection: 0..0,
            caret: Default::default(),
            inspecting: false,
            busy: 0,
            error: None,
            wake,
            _observe: observe,
            core_dirty: true,
            frame_delivery: Default::default(),
        }
    }
    pub fn set_applications(
        &mut self,
        applications: Arc<Vec<zork_client_core::pages::ApplicationEntry>>,
        cx: &mut Context<Self>,
    ) {
        if !Arc::ptr_eq(&self.applications, &applications) {
            self.applications = applications;
            cx.notify();
        }
    }
    pub fn open_native_page(&mut self, page: NativePage, cx: &mut Context<Self>) {
        let id = page.id.clone();
        if let Some(existing) = self.native_pages.iter_mut().find(|p| p.id == id) {
            *existing = page;
        } else {
            self.native_pages.push(page);
        }
        self.open = true;
        self.select_native_page(id, cx);
    }
    pub fn close_native_page(&mut self, id: &str, cx: &mut Context<Self>) {
        let existed = self.native_pages.iter().any(|p| p.id == id);
        self.native_pages.retain(|p| p.id != id);
        if existed
            && self.native_pages.is_empty()
            && self.tabs.is_empty()
            && !self.blank.contains(&self.host)
        {
            self.open = false;
        }
        if self.active_native.as_deref() == Some(id) {
            self.active_native = self.native_pages.last().map(|p| p.id.clone());
        }
        self.wake.notify_one();
        cx.emit(BrowserVisibility);
        cx.notify();
    }
    pub fn is_native_page_active(&self, id: &str) -> bool {
        self.active_native.as_deref() == Some(id)
    }
    fn select_native_page(&mut self, id: String, cx: &mut Context<Self>) {
        self.active_native = Some(id);
        self.chrome.menu.dismiss();
        self.inspecting = false;
        self.stop_viewport();
        self.wake.notify_one();
        cx.emit(BrowserVisibility);
        cx.notify();
    }
    fn show_web_page(&mut self, cx: &mut Context<Self>) {
        if self.active_native.take().is_some() {
            self.wake.notify_one();
            cx.emit(BrowserVisibility);
        }
    }
    pub fn panel_width(&self) -> f32 {
        self.display_width
    }
    pub fn is_present(&self) -> bool {
        self.open || self.display_width > 0.01
    }
    pub fn animate_panel(&mut self, cx: &Context<Self>) -> bool {
        let now = cx.background_executor().now();
        let target = if self.open { self.width } else { 0. };
        self.motion
            .retarget(target, now, cx.reduce_motion() || self.panel_resizing);
        // The target can shrink while the painted panel is still wider.
        // Only the actual workspace edge may clip an in-flight transition.
        self.display_width = self.motion.sample(now).0.clamp(0., self.available_width);
        self.is_animating(now)
    }
    pub fn begin_resize(&mut self, pointer_width: f32) {
        self.panel_resizing = !self.expanded;
        self.resize_offset = self.display_width - pointer_width;
    }
    pub fn is_resizing(&self) -> bool {
        self.panel_resizing
    }
    pub fn is_animating(&self, now: Instant) -> bool {
        (self.motion.from != self.motion.to || self.motion.velocity != 0.)
            && now.duration_since(self.motion.began) < Duration::from_millis(280)
    }
    pub fn target_width(&self) -> f32 {
        if self.open {
            self.width
        } else {
            0.
        }
    }
    pub fn finish_resize(&mut self) {
        self.panel_resizing = false;
    }
    pub fn resize_panel(&mut self, width: f32, cx: &mut Context<Self>) {
        if self.panel_resizing {
            let width =
                ((width + self.resize_offset) * self.device_scale).round() / self.device_scale;
            let width = width.clamp(260., self.resize_max_width);
            if self.custom_width.unwrap_or(self.width) == width {
                return;
            }
            self.custom_width = Some(width);
            cx.emit(BrowserResized);
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn is_expanded(&self) -> bool {
        self.open && self.expanded && self.display_width >= self.width - 0.5
    }
    pub fn resolved_width(&self, available_width: f32) -> f32 {
        if self.expanded {
            available_width.max(0.)
        } else {
            self.restored_width(available_width)
        }
    }
    fn restored_width(&self, available_width: f32) -> f32 {
        self.custom_width
            .unwrap_or_else(|| Self::width(available_width))
            .clamp(260., (available_width - 300.).max(260.))
    }
    fn content_width(&self) -> f32 {
        // Opening/closing reveals a full split pane through its aperture.
        // Expanding/restoring keeps its chrome and page on the moving edge.
        self.display_width.max(if self.open {
            self.restored_width(self.available_width)
        } else {
            self.width
        })
    }
    pub fn width(available_width: f32) -> f32 {
        (available_width * 0.5)
            .clamp(300., 840.)
            .min((available_width - 300.).max(260.))
    }
    pub fn set_presentation(
        &mut self,
        available_width: f32,
        locale: Locale,
        cx: &mut Context<Self>,
    ) {
        self.available_width = available_width.max(0.);
        self.resize_max_width = (available_width - 300.).max(260.);
        let width = self.resolved_width(available_width);
        if self.width != width || self.locale != locale {
            self.width = width;
            self.locale = locale;
            self.address.update(cx, |input, cx| {
                input.set_placeholder(locale.text("browser_address_placeholder"), cx)
            });
            cx.notify();
        }
    }
    pub fn set_connection(
        &mut self,
        client: Arc<crate::api::StationClient>,
        session: Option<String>,
    ) {
        self.connection = session.map(|s| (client, s));
    }
    pub fn set_host(&mut self, host: String, cx: &mut Context<Self>) {
        if self.host != host {
            self.wake.notify_one();
            self.stop_viewport();
            self.presentations.insert(
                self.host.clone(),
                HostPresentation {
                    native_pages: std::mem::take(&mut self.native_pages),
                    active_native: self.active_native.take(),
                    open: self.open,
                    expanded: self.expanded,
                    custom_width: self.custom_width,
                    address: self.address.read(cx).value().to_owned(),
                    shown: self.shown.take(),
                },
            );
            let presentation = self.presentations.remove(&host).unwrap_or_default();
            self.native_pages = presentation.native_pages;
            self.active_native = presentation.active_native;
            self.open = presentation.open;
            self.expanded = presentation.expanded;
            self.custom_width = presentation.custom_width;
            self.shown = presentation.shown;
            self.address
                .update(cx, |input, cx| input.set_value(&presentation.address, cx));
            self.display_width = if self.open { self.width } else { 0. };
            self.motion = PanelMotion {
                from: self.display_width,
                to: self.display_width,
                velocity: 0.,
                began: cx.background_executor().now(),
            };
            self.panel_resizing = false;
            self.connection = None;
            self.host = host;
            self.generation += 1;
            self.tabs = self.worker.browser.tabs(&self.host);
            self.error = None;
            self.set_frame(None, cx);
            self.decoding = false;
            self.sequence = 0;
            self.inspecting = false;
            self.chrome.menu.dismiss();
            self.sync_address(cx);
            cx.notify();
        }
    }
    pub fn set_visible(&mut self, visible: bool) {
        if self.visible != visible {
            self.wake.notify_one();
            self.visible = visible;
            if !visible {
                self.stop_viewport();
            }
        }
    }
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.wake.notify_one();
        self.open = !self.open;
        if self.open && self.active_native.is_none() && self.active_id().is_none() {
            self.blank.insert(self.host.clone());
        }
        self.visible = self.open;
        if !self.open {
            self.stop_viewport();
            self.chrome.menu.dismiss();
        }
        cx.emit(BrowserVisibility);
        cx.notify();
    }
    fn active_id(&self) -> Option<String> {
        self.selected.get(&self.host).cloned()
    }
    pub fn open_shared_link(&mut self, url: String, cx: &mut Context<Self>) {
        self.open = true;
        self.visible = true;
        self.show_web_page(cx);
        self.wake.notify_one();
        cx.emit(BrowserVisibility);
        let Some((client, _)) = &self.connection else {
            self.error = Some(self.locale.text("service_mesh_required").into());
            cx.notify();
            return;
        };
        let client = client.clone();
        let host = self.host.clone();
        let worker = self.worker.clone();
        self.busy += 1;
        self.error = None;
        cx.notify();
        cx.spawn(async move |weak, cx| {
            let result = worker.open_service(client, host.clone(), url).await;
            let _ = weak.update(cx, |panel, cx| {
                panel.busy = panel.busy.saturating_sub(1);
                panel.core_dirty = true;
                if panel.host == host {
                    match result {
                        Ok(value) => {
                            if let Some(id) = value["tab"]["id"].as_str() {
                                panel.tabs = panel.worker.browser.tabs(&host);
                                panel.select(id.into(), cx);
                            }
                        }
                        Err(error) => panel.error = Some(format!("{error:#}")),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn submit_address(&mut self, cx: &mut Context<Self>) {
        let input = self.address.read(cx).value().trim().to_owned();
        if input.is_empty() {
            return;
        }
        if input.starts_with("zork://service/") {
            self.open_shared_link(input, cx);
            return;
        }
        match zork_client_core::desktop::browser_engine::address_url(&input) {
            Ok(url) => {
                self.chrome.menu.dismiss();
                let action = match self.active_id() {
                    Some(tab_id) => Action::Navigate { tab_id, url },
                    None => Action::Open { url },
                };
                self.command(action, cx);
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                cx.notify();
            }
        }
    }
    fn active(&self) -> Option<&Tab> {
        let id = self.selected.get(&self.host)?;
        self.tabs.iter().find(|t| &t.id == id)
    }
    fn stop_viewport(&mut self) {
        self.awaiting_resize_paint = false;
        if let Some((id, _, _)) = self.viewport.take() {
            let host = self.host.clone();
            let _ = self
                .worker
                .submit_ordered(move |b| b.viewport(&host, &id, 0, 0, false));
        }
    }
    fn refresh_presentation(&mut self, cx: &mut Context<Self>) {
        let grants = self.grants.len();
        if let Some(error) = self.grants.get(&self.host).and_then(|grant| grant.error()) {
            self.error = Some(error);
        }
        self.grants.retain(|_, grant| grant.active());
        if grants != self.grants.len() {
            cx.notify();
        }
        let connected = self
            .grants
            .get(&self.host)
            .is_some_and(|grant| grant.connected());
        if connected != self.grant_connected {
            self.grant_connected = connected;
            cx.notify();
        }
        let tabs = self.worker.browser.tabs(&self.host);
        let mut changed = tabs != self.tabs;
        if changed {
            self.tabs = tabs;
        }
        // A command reply can refresh the tab list before this poll. Selection
        // still needs reconciliation even when the list now compares equal.
        if self
            .active_id()
            .is_some_and(|id| !self.tabs.iter().any(|tab| tab.id == id))
        {
            self.stop_viewport();
            self.selected.remove(&self.host);
            self.generation += 1;
            self.decoding = false;
            self.inspecting = false;
            self.set_frame(None, cx);
            self.sequence = 0;
            self.error = None;
            changed = true;
        }
        if self.active_id().is_none() && !self.tabs.is_empty() && !self.blank.contains(&self.host) {
            self.selected
                .insert(self.host.clone(), self.tabs.last().unwrap().id.clone());
            changed = true;
        }
        if changed {
            self.sync_address(cx);
            cx.notify();
        }
        if !self.open || !self.visible || self.active_native.is_some() {
            return;
        }
        if let Some(selected) = self.worker.selected(&self.host) {
            if self.active_id().as_ref() != Some(&selected)
                && self.tabs.iter().any(|tab| tab.id == selected)
            {
                self.select(selected, cx);
            }
        }
        let Some(id) = self.active_id() else {
            return;
        };
        let bounds = self.bounds.get();
        let width = bounds.size.width.as_f32().round().max(0.) as u32;
        let height = bounds.size.height.as_f32().round().max(0.) as u32;
        let next = (id.clone(), width, height);
        let frame = self.worker.browser.frame(&self.host, &id);
        if let (Some(frame), Some((target, width, height))) = (&frame, &self.viewport) {
            if target == &id
                && frame.width.round() as u32 == *width
                && frame.height.round() as u32 == *height
            {
                self.awaiting_resize_paint = false;
            }
        }
        if !self.resizing
            && !self.awaiting_resize_paint
            && width >= 200
            && height >= 100
            && self.viewport.as_ref() != Some(&next)
        {
            // Resize the existing visible surface. Keep its last painted frame
            // until CEF delivers the replacement, and coalesce newer bounds
            // until that paint arrives. The RPC ack precedes the paint: sending
            // another size on the ack makes CEF discard intermediate paints.
            self.resizing = true;
            let host = self.host.clone();
            let target = id.clone();
            let scale = self.device_scale;
            let owner = host.clone();
            let result = self.worker.submit_ordered(move |b| {
                b.viewport_scaled(&host, &target, width, height, true, scale)
            });
            if let Ok(rx) = result {
                self.viewport = Some(next.clone());
                self.awaiting_resize_paint = true;
                // A navigation or tab switch must not strand the resize latch.
                // Completion releases it even when the page generation changed.
                cx.spawn(async move |weak, cx| {
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            rx.recv()
                                .unwrap_or_else(|_| Err(anyhow::anyhow!("浏览器任务已结束")))
                        })
                        .await;
                    let _ = weak.update(cx, |panel, cx| {
                        panel.resizing = false;
                        panel.core_dirty = true;
                        if panel.host == owner && panel.viewport.as_ref() == Some(&next) {
                            if let Err(error) = result {
                                panel.awaiting_resize_paint = false;
                                panel.error = Some(format!("{error:#}"));
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            } else {
                self.resizing = false;
            }
        }
        // CEF can paint while another viewport request is in flight. Deliver
        // those frames now; waiting for resize requests to stop freezes the
        // document at its old width throughout the panel animation.
        if let Some(frame) = frame {
            if !self.decoding && frame.sequence != self.sequence {
                self.sequence = frame.sequence;
                self.decoding = true;
                let pixels = frame.bgra.clone();
                let dimensions = (frame.pixel_width, frame.pixel_height);
                let generation = self.generation;
                cx.spawn(async move |weak, cx| {
                    let decoded = cx
                        .background_executor()
                        .spawn(async move {
                            image::RgbaImage::from_raw(
                                dimensions.0,
                                dimensions.1,
                                pixels.as_ref().clone(),
                            )
                            .map(|image| {
                                Arc::new(gpui::RenderImage::new(vec![image::Frame::new(image)]))
                            })
                        })
                        .await;
                    let _ = weak.update(cx, |panel, cx| {
                        if panel.generation != generation {
                            return;
                        }
                        panel.decoding = false;
                        panel.core_dirty = true;
                        if let Some(image) = decoded {
                            panel.page_size = (frame.width, frame.height);
                            panel.set_frame(Some(image), cx);
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
    }
    fn set_frame(&mut self, frame: Option<Arc<gpui::RenderImage>>, cx: &mut gpui::App) {
        if let Some(previous) = std::mem::replace(&mut self.frame, frame) {
            // Dropping RenderImage frees its pixels, but not GPUI's atlas entry.
            // Defer until the current window is back in App.windows, including
            // when tab reconciliation or panel release runs during its update.
            cx.defer(move |cx| cx.drop_image(previous, None));
        }
    }
    fn sync_address(&mut self, cx: &mut Context<Self>) {
        let next = self.active().map(|t| (t.id.clone(), t.url.clone()));
        if next != self.shown {
            self.address.update(cx, |input, cx| {
                input.set_value(next.as_ref().map(|(_, u)| u.as_str()).unwrap_or(""), cx)
            });
            self.shown = next;
        }
    }
    fn await_result<T: Send + 'static>(
        &mut self,
        rx: std::sync::mpsc::Receiver<anyhow::Result<T>>,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.busy += 1;
        let generation = self.generation;
        cx.spawn(async move |weak, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    rx.recv()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("浏览器任务已结束")))
                })
                .await;
            let _ = weak.update(cx, |panel, cx| {
                panel.busy = panel.busy.saturating_sub(1);
                panel.core_dirty = true;
                if generation != panel.generation {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(value) => {
                        panel.error = None;
                        done(panel, value, cx);
                    }
                    Err(error) => panel.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn command(&mut self, action: Action, cx: &mut Context<Self>) {
        if matches!(
            action,
            Action::Navigate { .. }
                | Action::Open { .. }
                | Action::Back { .. }
                | Action::Forward { .. }
                | Action::Reload { .. }
                | Action::Stop { .. }
        ) {
            self.generation += 1;
            self.decoding = false;
        }
        let host = self.host.clone();
        match self.worker.submit(move |b| b.execute(&host, action)) {
            Ok(rx) => self.await_result(
                rx,
                |panel, result, cx| {
                    panel.tabs = panel.worker.browser.tabs(&panel.host);
                    if let Some(id) = result["tab"]["id"].as_str() {
                        panel.select(id.to_owned(), cx);
                    }
                    panel.sync_address(cx);
                },
                cx,
            ),
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                cx.notify();
            }
        }
    }
    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        self.show_web_page(cx);
        self.chrome.menu.dismiss();
        self.blank.remove(&self.host);
        self.worker.select(&self.host, &id);
        self.stop_viewport();
        self.selected.insert(self.host.clone(), id);
        self.set_frame(None, cx);
        self.decoding = false;
        self.sequence = 0;
        self.inspecting = false;
        self.generation += 1;
        self.sync_address(cx);
        cx.notify();
    }
    fn nav(&mut self, op: &str, cx: &mut Context<Self>) {
        self.chrome.menu.dismiss();
        let Some(tab_id) = self.active_id() else {
            return;
        };
        let action = match op {
            "back" => Action::Back { tab_id },
            "forward" => Action::Forward { tab_id },
            "reload" => Action::Reload { tab_id },
            _ => Action::Stop { tab_id },
        };
        self.command(action, cx);
    }
    fn point(&self, p: gpui::Point<Pixels>) -> (f64, f64) {
        let b = self.bounds.get();
        let (w, h) = self.page_size;
        let bw = (b.size.width.as_f32() as f64).max(1.);
        let bh = (b.size.height.as_f32() as f64).max(1.);
        (
            ((p.x - b.origin.x).as_f32() as f64).max(0.) * w / bw,
            ((p.y - b.origin.y).as_f32() as f64).max(0.) * h / bh,
        )
    }
    fn pointer(
        &mut self,
        p: gpui::Point<Pixels>,
        kind: &str,
        button: &str,
        clicks: u32,
        mods: u8,
        cx: &mut Context<Self>,
    ) {
        if self.frame.is_none() {
            return;
        }
        let Some(id) = self.active_id() else {
            return;
        };
        let host = self.host.clone();
        let (x, y) = self.point(p);
        if self.inspecting {
            if kind != "mouseReleased" {
                return;
            }
            self.inspecting = false;
            if let Ok(rx) = self.worker.submit(move |b| b.inspect(&host, &id, x, y)) {
                self.await_result(
                    rx,
                    |panel, inspection, cx| {
                        cx.emit(BrowserSelection {
                            host: panel.host.clone(),
                            inspection,
                        })
                    },
                    cx,
                );
            }
            return;
        }
        let kind = kind.to_owned();
        let button = button.to_owned();
        if let Ok(rx) = self
            .worker
            .submit_ordered(move |b| b.pointer(&host, &id, x, y, &kind, &button, clicks, mods))
        {
            self.await_result(rx, |_, _, _| {}, cx);
        }
    }
    fn keyboard(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        if self.inspecting && event.keystroke.key == "escape" {
            self.inspecting = false;
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let Some(id) = self.active_id() else {
            return;
        };
        let host = self.host.clone();
        let mods = &event.keystroke.modifiers;
        let mask = modifiers(mods);
        let key = event.keystroke.key.clone();
        if !self.ime_text.is_empty() && !mods.platform && !mods.control {
            return;
        }
        if mods.platform && key == "c" {
            if let Ok(rx) = self.worker.submit(move |b| b.selected_text(&host, &id)) {
                self.await_result(
                    rx,
                    |_, text, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
                    cx,
                );
            }
            cx.stop_propagation();
            return;
        }
        let special = matches!(
            key.as_str(),
            "enter"
                | "tab"
                | "backspace"
                | "delete"
                | "escape"
                | "left"
                | "right"
                | "up"
                | "down"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
        );
        if !mods.platform && !mods.control && !special {
            return;
        }
        let text = if mods.platform && key == "v" {
            cx.read_from_clipboard().and_then(|i| i.text())
        } else {
            None
        };
        if let Ok(rx) = self.worker.submit_ordered(move |b| match text {
            Some(text) => b.input(&host, &id, &text),
            None => b.key(&host, &id, &key, mask),
        }) {
            self.await_result(rx, |_, _, _| {}, cx);
        }
        cx.stop_propagation();
    }
}
fn modifiers(m: &gpui::Modifiers) -> u8 {
    u8::from(m.alt)
        | (u8::from(m.control) << 1)
        | (u8::from(m.platform) << 2)
        | (u8::from(m.shift) << 3)
}
impl Render for BrowserPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.frame_delivery.enter() {
            self.core_dirty = true;
        }
        if self.device_scale != window.scale_factor() {
            self.device_scale = window.scale_factor();
            self.viewport = None;
            self.awaiting_resize_paint = false;
            self.core_dirty = true;
        }
        if self.core_dirty {
            self.core_dirty = false;
            self.refresh_presentation(cx);
        }
        self.sync_chrome();
        let width = self.content_width();
        if self.active_native.is_some() {
            return div()
                .w(px(width))
                .flex_shrink_0()
                .border_color(rgb(crate::design::ZORK_UI.palette.border))
                .bg(rgb(crate::design::ZORK_UI.palette.canvas))
                .child(self.render_tabs(cx))
                .into_any_element();
        }
        let palette = crate::design::ZORK_UI.palette;
        let bounds = self.bounds.clone();
        let layout_changed = self.wake.clone();
        let input = cx.entity();
        let focus = self.focus.clone();
        div()
            .relative()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(palette.canvas))
            .border_color(rgb(palette.border))
            .text_size(px(13.))
            .text_color(rgb(palette.text))
            .child(self.render_tabs(cx))
            .child(self.render_navigation(window, cx))
            .children(self.chrome.render_error())
            .child(
                div()
                    .id("browser-page")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .track_focus(&self.focus)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|v, e: &gpui::MouseDownEvent, w, cx| {
                            v.caret = e.position;
                            w.focus(&v.focus, cx);
                            v.pointer(
                                e.position,
                                "mousePressed",
                                "left",
                                e.click_count as u32,
                                modifiers(&e.modifiers),
                                cx,
                            );
                        }),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|v, e: &gpui::MouseUpEvent, _, cx| {
                            v.pointer(
                                e.position,
                                "mouseReleased",
                                "left",
                                1,
                                modifiers(&e.modifiers),
                                cx,
                            )
                        }),
                    )
                    .on_mouse_move(cx.listener(|v, e: &gpui::MouseMoveEvent, _, cx| {
                        v.pointer(
                            e.position,
                            "mouseMoved",
                            if e.pressed_button.is_some() {
                                "left"
                            } else {
                                "none"
                            },
                            0,
                            modifiers(&e.modifiers),
                            cx,
                        )
                    }))
                    .on_key_down(cx.listener(|v, e: &gpui::KeyDownEvent, w, cx| {
                        if e.keystroke.modifiers.platform && e.keystroke.key == "l" {
                            w.focus(&v.address.read(cx).focus_handle(), cx);
                            cx.stop_propagation();
                        } else {
                            v.keyboard(e, cx);
                        }
                    }))
                    .on_scroll_wheel(cx.listener(|v, e: &gpui::ScrollWheelEvent, _, cx| {
                        if let Some(tab_id) = v.active_id() {
                            let (x, y) = v.point(e.position);
                            let delta = e.delta.pixel_delta(px(20.));
                            v.command(
                                Action::Scroll {
                                    tab_id,
                                    x,
                                    y,
                                    delta_x: -(delta.x.as_f32() as f64),
                                    delta_y: -(delta.y.as_f32() as f64),
                                },
                                cx,
                            );
                            cx.stop_propagation();
                        }
                    }))
                    .when_some(self.frame.clone(), |v, frame| {
                        v.child(
                            img(frame)
                                .absolute()
                                .size_full()
                                // Bridge the asynchronous CEF paint with a
                                // filled frame, without centered white gutters.
                                .object_fit(gpui::ObjectFit::Fill),
                        )
                    })
                    .when(self.frame.is_none(), |v| v.child(self.render_empty(cx)))
                    .child(
                        gpui::canvas(
                            move |b, _, _| {
                                if bounds.get() != b {
                                    bounds.set(b);
                                    layout_changed.notify_one();
                                }
                            },
                            move |b, _, w, cx| {
                                w.handle_input(
                                    &focus,
                                    gpui::ElementInputHandler::new(b, input.clone()),
                                    cx,
                                )
                            },
                        )
                        .size_full(),
                    )
                    .automation(
                        AutomationRole::ScrollArea,
                        if self.frame.is_some() {
                            self.locale.text("browser_page_ready")
                        } else {
                            self.locale.text("browser_page_loading")
                        },
                    ),
            )
            .when(self.inspecting, |v| {
                v.child(
                    div()
                        .id("browser-inspection-hint")
                        .absolute()
                        .bottom_3()
                        .left_3()
                        .right_3()
                        .occlude()
                        .px_3()
                        .py_2()
                        .rounded(px(crate::desktop::ui::FIELD_RADIUS))
                        .border(gpui::px(zork_ui::design::BORDER_WIDTH))
                        .border_color(rgb(palette.border_strong))
                        .bg(rgb(palette.canvas))
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(rgb(palette.muted))
                        .child(self.locale.text("browser_inspect_instruction"))
                        .automation(
                            AutomationRole::Status,
                            self.locale.text("browser_inspect_instruction"),
                        ),
                )
            })
            .children(self.render_menu(window, cx))
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod frame_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn pointer_tracks_the_filled_frame_during_resize(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(BrowserPanel::new);
        panel.update(cx, |panel, _| {
            panel.page_size = (400., 600.);
            panel.bounds.set(Bounds::new(
                gpui::point(px(20.), px(30.)),
                gpui::size(px(800.), px(600.)),
            ));
            assert_eq!(panel.point(gpui::point(px(220.), px(180.))), (100., 150.));
            panel.bounds.set(Bounds::new(
                gpui::point(px(20.), px(30.)),
                gpui::size(px(200.), px(300.)),
            ));
            assert_eq!(panel.point(gpui::point(px(70.), px(105.))), (100., 150.));
        });
    }

    #[gpui::test]
    fn conversation_tabs_restore_their_own_presentation(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(BrowserPanel::new);
        panel.update(cx, |panel, cx| {
            panel.set_host("device/chat-a".into(), cx);
            panel.open_native_page(
                NativePage {
                    id: "history".into(),
                    title: "History A".into(),
                    icon: "icons/history.svg",
                },
                cx,
            );
            panel.expanded = true;
            panel.custom_width = Some(640.);
            panel.blank.insert(panel.host.clone());
            panel.address.update(cx, |input, cx| {
                input.set_value("https://draft-a.example", cx)
            });

            panel.set_host("device/chat-b".into(), cx);
            assert!(!panel.is_open());
            assert!(panel.native_pages.is_empty());
            assert!(panel.active_native.is_none());
            assert!(!panel.expanded);
            assert!(panel.custom_width.is_none());
            assert!(!panel.blank.contains(&panel.host));
            assert!(panel.address.read(cx).value().is_empty());
            panel.toggle(cx);
            panel.address.update(cx, |input, cx| {
                input.set_value("https://draft-b.example", cx)
            });

            panel.set_host("device/chat-a".into(), cx);
            assert!(panel.is_open());
            assert!(panel.is_native_page_active("history"));
            assert_eq!(panel.native_pages[0].title, "History A");
            assert!(panel.expanded);
            assert_eq!(panel.custom_width, Some(640.));
            assert_eq!(panel.address.read(cx).value(), "https://draft-a.example");
            panel.close_native_page("history", cx);
            panel.toggle(cx);
            assert!(!panel.is_open());

            panel.set_host("device/chat-b".into(), cx);
            assert!(panel.is_open());
            assert!(panel.native_pages.is_empty());
            assert!(panel.blank.contains(&panel.host));
            assert_eq!(panel.address.read(cx).value(), "https://draft-b.example");
            panel.set_host("device/chat-a".into(), cx);
            assert!(!panel.is_open());
            assert!(panel.native_pages.is_empty());
        });
    }
}

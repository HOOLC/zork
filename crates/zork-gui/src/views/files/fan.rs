use super::motion::{DraftFiles, Frame};
use super::*;
use std::cell::RefCell;
use zork_client_core::files::FileRef;

const FRONT_ANGLE: usize = 48;

#[derive(Clone, Copy, Default)]
pub(in crate::views) struct FanState {
    pub hovered: bool,
    pub pinned: bool,
    pub active: Option<usize>,
    pub progress: f32,
    row: Option<usize>,
}
impl FanState {
    fn advance(&mut self, _: std::time::Instant, _: bool) -> bool {
        self.progress = if self.open() { 1. } else { 0. };
        false
    }
    fn transition(&mut self, _: std::time::Instant, _: bool) {
        self.progress = if self.open() { 1. } else { 0. };
    }
    fn set_hover(&mut self, hover: bool, _: std::time::Instant, _: bool) {
        self.hovered = hover;
    }
    pub fn open(self) -> bool {
        self.hovered || self.pinned
    }
}

struct PreviewEntry {
    page: Option<Arc<::image::RgbaImage>>,
    images: HashMap<usize, Arc<gpui::RenderImage>>,
    rotating: bool,
    ready: bool,
    used: u64,
}
pub(in crate::views) struct PreviewCache {
    placeholder: HashMap<usize, Arc<gpui::RenderImage>>,
    entries: HashMap<String, PreviewEntry>,
    clock: u64,
    loading: usize,
    retired: Vec<Arc<gpui::RenderImage>>,
}
impl Default for PreviewCache {
    fn default() -> Self {
        Self {
            placeholder: HashMap::new(),
            entries: HashMap::new(),
            clock: 0,
            loading: 0,
            retired: Vec::new(),
        }
    }
}

fn key(file: &FileRef) -> String {
    format!("{}:{}", file.id, file.content_root)
}
impl PreviewCache {
    fn begin(&mut self, file: &FileRef) -> bool {
        if self.loading >= 2 || self.entries.contains_key(&key(file)) {
            return false;
        }
        if self.entries.len() >= 24 {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, v)| v.ready)
                .min_by_key(|(_, v)| v.used)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                if let Some(entry) = self.entries.remove(&oldest) {
                    self.retired.extend(entry.images.into_values());
                }
            }
        }
        self.clock += 1;
        self.loading += 1;
        self.entries.insert(
            key(file),
            PreviewEntry {
                page: None,
                images: HashMap::new(),
                rotating: false,
                ready: false,
                used: self.clock,
            },
        );
        true
    }
    fn image(&mut self, file: &FileRef, angle: usize) -> Arc<gpui::RenderImage> {
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(&key(file)) {
            entry.used = self.clock;
            if let Some((_, image)) = entry
                .images
                .iter()
                .min_by_key(|(index, _)| index.abs_diff(angle))
            {
                return image.clone();
            }
        }
        self.placeholder
            .entry(angle)
            .or_insert_with(|| super::preview::placeholder(angle))
            .clone()
    }
    #[cfg(feature = "headless-bench")]
    pub(in crate::views) fn image_count(&self) -> usize {
        self.entries
            .values()
            .filter(|e| !e.images.is_empty())
            .count()
    }
}

#[derive(Default)]
pub(in crate::views) struct UiState {
    pub draft: FanState,
    pub draft_files: DraftFiles,
    pub messages: Rc<RefCell<HashMap<String, FanState>>>,
    pub previews: Rc<RefCell<PreviewCache>>,
    pub message_previews: Rc<RefCell<super::message::PreviewCache>>,
}

impl RootView {
    pub(in crate::views) fn advance_file_fans(&mut self, window: &Window, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let mut moving = self.file_ui.draft.advance(now, cx.reduce_motion());
        moving |= self.file_ui.draft_files.advance(
            &self.draft_state.files,
            &mut self.file_ui.draft,
            now,
            cx.reduce_motion(),
            (self.composer_surface_width - 72.).max(168.),
        );
        let mut remeasure = Vec::new();
        for state in self.file_ui.messages.borrow_mut().values_mut() {
            let previous = state.progress;
            moving |= state.advance(now, cx.reduce_motion());
            if state.progress != previous {
                if let Some(row) = state.row {
                    remeasure.push(row);
                }
            }
        }
        if !remeasure.is_empty() {
            let was_following = self.transcript_list.is_following_tail();
            let anchor = self.transcript_list.logical_scroll_top();
            for row in remeasure {
                if row < self.lines.len() {
                    self.transcript_list.splice(row..row + 1, 1);
                }
            }
            if was_following {
                self.transcript_list.set_follow_mode(FollowMode::Tail);
                self.transcript_list.scroll_to_end();
            } else {
                self.transcript_list.scroll_to(anchor);
            }
        }
        if moving {
            let root = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = root.update(cx, |_, cx| {
                    zork_ui::components::region::invalidate(cx, &["composer", "transcript"])
                });
            });
        }
    }
    pub(in crate::views) fn draft_file_frame(&self) -> Frame {
        self.file_ui.draft_files.frame(
            self.file_ui.draft.progress,
            (self.composer_surface_width - 72.).max(168.),
        )
    }
    pub(in crate::views) fn file_fan_center(&self) -> f32 {
        let frame = self.draft_file_frame();
        let half = frame.width * 0.5;
        (self.composer_surface_width - half - 24.).max(half + 24.)
    }
    pub(in crate::views) fn file_fan_dimensions(&self) -> (f32, f32) {
        let frame = self.draft_file_frame();
        (frame.width, frame.height)
    }
    pub(in crate::views) fn ensure_file_previews(
        &mut self,
        files: &[(FileRef, usize)],
        cx: &mut Context<Self>,
    ) {
        let retired = std::mem::take(&mut self.file_ui.previews.borrow_mut().retired);
        if !retired.is_empty() {
            cx.defer(move |cx| {
                for image in retired {
                    cx.drop_image(image, None);
                }
            });
        }
        for (file, angle) in files {
            let angle = *angle;
            let rotation = {
                let mut cache = self.file_ui.previews.borrow_mut();
                cache.entries.get_mut(&key(file)).and_then(|entry| {
                    if entry.rotating || entry.images.contains_key(&angle) {
                        return None;
                    }
                    let page = entry.page.clone()?;
                    entry.rotating = true;
                    Some(page)
                })
            };
            if let Some(page) = rotation {
                let file = file.clone();
                let cache = self.file_ui.previews.clone();
                cx.spawn(async move |this, cx| {
                    let image = cx
                        .background_executor()
                        .spawn(async move { super::preview::render(&page, angle) })
                        .await;
                    if let Some(entry) = cache.borrow_mut().entries.get_mut(&key(&file)) {
                        entry.images.insert(angle, image);
                        entry.rotating = false;
                    }
                    let _ = this.update(cx, |_, cx| {
                        zork_ui::components::region::invalidate(cx, &["composer"]);
                    });
                })
                .detach();
                continue;
            }
            if !self.file_ui.previews.borrow_mut().begin(file) {
                continue;
            }
            let file = file.clone();
            let device = self.core_device.clone();
            let cache = self.file_ui.previews.clone();
            #[cfg(feature = "headless-bench")]
            let offline = self.benchmark_offline;
            #[cfg(not(feature = "headless-bench"))]
            let offline = false;
            let local = self.local_cache.clone();
            let renderer = cx.svg_renderer();
            cx.spawn(async move |this, cx| {
                let bytes = if offline {
                    local.and_then(|(store, node)| {
                        store
                            .blob(&node, &format!("upload:{}", file.id))
                            .ok()
                            .flatten()
                    })
                } else {
                    device.artifact_content(&file.id).await.ok()
                };
                let prepared = if let Some(bytes) = bytes {
                    let name = file.name.clone();
                    cx.background_executor()
                        .spawn(async move {
                            let page = Arc::new(super::preview::load(&name, &bytes, &renderer)?);
                            let started = std::time::Instant::now();
                            let image = super::preview::render(&page, angle);
                            #[cfg(feature = "headless-bench")]
                            eprintln!(
                                "preview first angle {:.2} ms",
                                started.elapsed().as_secs_f64() * 1000.
                            );
                            #[cfg(not(feature = "headless-bench"))]
                            let _ = started;
                            Some((page, image))
                        })
                        .await
                } else {
                    None
                };
                let mut cache = cache.borrow_mut();
                cache.loading = cache.loading.saturating_sub(1);
                if let Some(entry) = cache.entries.get_mut(&key(&file)) {
                    if let Some((page, image)) = prepared {
                        entry.page = Some(page);
                        entry.images.insert(angle, image);
                    }
                    entry.ready = true;
                }
                drop(cache);
                let _ = this.update(cx, |_, cx| {
                    zork_ui::components::region::invalidate(cx, &["composer", "transcript"])
                });
            })
            .detach();
        }
    }
    pub(in crate::views) fn release_file_previews(&mut self, cx: &mut gpui::App) {
        self.file_ui.message_previews.borrow_mut().release(cx);
        let mut cache = self.file_ui.previews.borrow_mut();
        for (_, entry) in cache.entries.drain() {
            for image in entry.images.into_values() {
                cx.drop_image(image, None);
            }
        }
        for paper in cache.placeholder.drain().map(|(_, image)| image) {
            cx.drop_image(paper, None);
        }
        for image in cache.retired.drain(..) {
            cx.drop_image(image, None);
        }
    }
    pub(in crate::views) fn render_draft_fan(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let frame = self.draft_file_frame();
        self.ensure_file_previews(
            &frame
                .files
                .iter()
                .map(|v| (v.file.clone(), FRONT_ANGLE))
                .collect::<Vec<_>>(),
            cx,
        );
        let width = frame.width;
        render(
            frame,
            self.file_ui.draft,
            self.file_ui.previews.clone(),
            cx.entity().downgrade(),
            None,
            true,
            self.selected_session.clone().unwrap_or_default(),
            self.locale,
        )
        .absolute()
        .right(px(self.composer_surface_width
            - self.file_fan_center()
            - width * 0.5))
        .bottom(px(zork_ui::components::composer_layout::TOP_EXTENSION))
    }
    pub(in crate::views) fn close_draft_fan(&mut self, cx: &mut Context<Self>) {
        self.file_ui.draft.hovered = false;
        self.file_ui.draft.pinned = false;
        self.file_ui
            .draft
            .transition(cx.background_executor().now(), cx.reduce_motion());
        zork_ui::components::region::invalidate(cx, &["composer", "transcript"]);
    }
    fn highlight_file(
        &mut self,
        message: Option<(String, usize)>,
        index: usize,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some((key, _)) = message {
            let mut states = self.file_ui.messages.borrow_mut();
            let state = states.entry(key).or_default();
            if hovered {
                state.active = Some(index);
            } else if state.active == Some(index) {
                state.active = None;
            }
            drop(states);
            zork_ui::components::region::invalidate(cx, &["transcript"]);
        } else {
            if hovered {
                self.file_ui.draft.active = Some(index);
            } else if self.file_ui.draft.active == Some(index) {
                self.file_ui.draft.active = None;
            }
            zork_ui::components::region::invalidate(cx, &["composer"]);
        }
    }
    fn change_fan(
        &mut self,
        message: Option<(String, usize)>,
        hover: Option<bool>,
        toggle: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some((key, index)) = message {
            let mut states = self.file_ui.messages.borrow_mut();
            let state = states.entry(key).or_default();
            state.row = Some(index);
            let old = state.open();
            if let Some(hover) = hover {
                state.set_hover(hover, cx.background_executor().now(), cx.reduce_motion());
            }
            if toggle {
                state.pinned = !state.pinned;
            }
            if old != state.open() {
                state.transition(cx.background_executor().now(), cx.reduce_motion());
            }
            if old != state.open() && index < self.lines.len() {
                let anchor = self.transcript_list.logical_scroll_top();
                self.transcript_list.splice(index..index + 1, 1);
                self.transcript_list.scroll_to(anchor);
            }
            drop(states);
            zork_ui::components::region::invalidate(cx, &["transcript"]);
        } else {
            let old = self.file_ui.draft.open();
            if let Some(hover) = hover {
                self.file_ui.draft.set_hover(
                    hover,
                    cx.background_executor().now(),
                    cx.reduce_motion(),
                );
            }
            if toggle {
                self.file_ui.draft.pinned = !self.file_ui.draft.pinned;
            }
            if old != self.file_ui.draft.open() {
                self.file_ui
                    .draft
                    .transition(cx.background_executor().now(), cx.reduce_motion());
            }
            zork_ui::components::region::invalidate(cx, &["composer", "transcript"]);
        }
    }
}

fn render(
    frame: Frame,
    state: FanState,
    cache: Rc<RefCell<PreviewCache>>,
    root: gpui::WeakEntity<RootView>,
    message: Option<(String, usize)>,
    draft: bool,
    session: String,
    locale: Locale,
) -> gpui::Stateful<Div> {
    use zork_ui::components::widgets::composer::fan as component;
    let files: HashMap<_, _> = frame
        .files
        .iter()
        .map(|visual| (visual.file.id.clone(), visual.file.clone()))
        .collect();
    let indices: HashMap<_, _> = frame
        .files
        .iter()
        .enumerate()
        .map(|(index, visual)| (visual.file.id.clone(), index))
        .collect();
    let open = state.open();
    let key = message.as_ref().map(|m| m.0.clone()).unwrap_or_default();
    let ids = component::Ids {
        root: if draft {
            "draft-file-fan".into()
        } else {
            format!("file-fan-{key}")
        },
        toggle: if draft {
            "draft-file-fan-toggle".into()
        } else {
            format!("file-fan-toggle-{key}")
        },
        file_prefix: if draft {
            "draft-preview-".into()
        } else {
            format!("message-file-{}-", message.as_ref().unwrap().1)
        },
        remove_prefix: "remove-".into(),
    };
    let component_frame = component::Frame {
        files: frame
            .files
            .iter()
            .map(|visual| component::File {
                id: visual.file.id.clone(),
                name: visual.file.name.clone(),
                image: Some(cache.borrow_mut().image(&visual.file, FRONT_ANGLE)),
                removable: draft && open && state.progress > 0.98,
            })
            .collect(),
        width: frame.width,
        height: frame.height,
        expanded: frame.expanded,
    };
    let handler = Rc::new(move |action, window: &mut Window, cx: &mut gpui::App| {
        let _ = root.update(cx, |v, cx| match action {
            component::Action::Hover(hover) => {
                v.change_fan(message.clone(), Some(hover), false, cx)
            }
            component::Action::Toggle => {
                v.change_fan(message.clone(), None, true, cx);
                v.focus_composer(window, cx);
            }
            component::Action::Highlight(id, hover) => {
                if let Some(index) = indices.get(&id) {
                    v.highlight_file(message.clone(), *index, hover, cx);
                }
            }
            component::Action::Open(id) => {
                if !open || (draft && !state.pinned) {
                    v.change_fan(message.clone(), None, true, cx);
                } else if let Some(file) = files.get(&id) {
                    v.open_message_file(file, &session, cx);
                }
            }
            component::Action::Remove(id) => {
                if draft && v.selected_session.as_deref() == Some(&session) {
                    if let Err(error) = v.core_device.remove_file(&session, &id) {
                        v.error = Some(error.to_string());
                    }
                    zork_ui::components::region::invalidate(cx, &["composer"]);
                }
            }
        });
    });
    component::render(
        ids,
        component_frame,
        locale.text("conversation_files").into(),
        locale.text("remove_attachment").into(),
        handler,
    )
}

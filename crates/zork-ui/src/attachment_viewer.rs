//! Complete read-only attachment viewer. Data and business actions come from the host.
use crate::modal::PlainDialog;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        loading,
        message::{render_selectable_document, MessageDocument},
        widgets::overlay::DialogOptions,
    },
    controls as ui,
    design::ZORK_UI,
    resources::Text,
};
use gpui::{prelude::*, *};
use std::{rc::Rc, sync::Arc};
/// Image view top bar and thumbnail strip heights.
const IMAGE_BAR: f32 = 52.;
const STRIP: f32 = 36.;
#[allow(non_snake_case)]
fn BG() -> u32 {
    ZORK_UI.palette.canvas
}
#[derive(Clone)]
pub struct DecodedImage {
    pub rendered: Arc<RenderImage>,
    pub size: Size<f32>,
    pub svg: Option<Arc<ParsedSvg>>,
}
#[derive(Clone)]
pub struct Info {
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub image_view: bool,
    pub source_path: String,
    pub version: i64,
    pub created_at: String,
}
#[derive(Clone, Default)]
pub struct Data {
    pub info: Option<Info>,
    pub epoch: u64,
    pub group: Arc<Vec<Info>>,
    pub index: usize,
    pub loaded: bool,
    pub failed: bool,
    pub saving: bool,
    pub choosing_save: bool,
    pub notice: Option<&'static str>,
    pub can_reuse: bool,
    pub image: Option<DecodedImage>,
    pub image_failed: bool,
    pub overview: bool,
    pub text: Option<Arc<str>>,
    pub document: Option<MessageDocument>,
    pub source_document: Option<MessageDocument>,
    pub text_truncated: bool,
    /// Thumbnails for `group`, by position, when the host already has them.
    /// The image view shows them as a strip for switching files.
    pub thumbnails: Arc<Vec<Option<Arc<RenderImage>>>>,
}
pub enum Action {
    Close,
    Move(isize),
    Save,
    Retry,
    Reuse,
}
pub struct Viewer {
    data: Data,
    viewer: PreviewState,
    locale: Text,
    focused: bool,
    image_focus: FocusHandle,
    input_image: Option<usize>,
    dialog: PlainDialog,
    retired: Option<Info>,
}
impl EventEmitter<Action> for Viewer {}
fn preview_icon(id: impl Into<gpui::ElementId>, enabled: bool) -> crate::controls::Action {
    ui::icon_button(id, enabled)
}
fn preview_quiet(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
    enabled: bool,
    size: ui::IconButtonSize,
) -> crate::controls::Action {
    ui::quiet_button(id, text, enabled, size)
}

#[derive(Default)]
struct PreviewState {
    group: Arc<Vec<Info>>,
    index: usize,
    image: Option<DecodedImage>,
    image_failed: bool,
    overview: bool,
    text: Option<Arc<str>>,
    document: Option<MessageDocument>,
    source_document: Option<MessageDocument>,
    text_truncated: bool,
    selection: Rc<std::cell::RefCell<crate::components::selection::TranscriptSelection>>,
    source: bool,
    menu: crate::components::standard_menu::Menu,
    zoom: Option<f32>,
    scroll: gpui::ScrollHandle,
    drag: Option<(gpui::Point<gpui::Pixels>, gpui::Point<gpui::Pixels>)>,
    focus: Option<FocusHandle>,
    return_focus: Option<FocusHandle>,
    view_size: gpui::Size<f32>,
    target_raster_width: u32,
    raster_request: u64,
}
impl PreviewState {
    fn scale(&self) -> f32 {
        self.zoom.unwrap_or_else(|| {
            self.image
                .as_ref()
                .map(|image| {
                    ((self.view_size.width - 64.) / image.size.width)
                        .min((self.view_size.height - 64.) / image.size.height)
                        .max(0.01)
                })
                .unwrap_or(1.)
        })
    }
    fn reset_file(&mut self, cx: &mut gpui::App) {
        if let Some(image) = self.image.take() {
            cx.drop_image(image.rendered, None);
        }
        self.image_failed = false;
        self.overview = false;
        self.text = None;
        self.document = None;
        self.source_document = None;
        self.source = false;
        self.menu.dismiss();
        self.text_truncated = false;
        self.selection.borrow_mut().clear();
        self.zoom = None;
        self.scroll = gpui::ScrollHandle::new();
        self.drag = None;
        self.target_raster_width = 0;
        self.raster_request = self.raster_request.wrapping_add(1);
    }
}

impl Viewer {
    pub fn new(locale: Text, cx: &mut Context<Self>) -> Self {
        Self {
            data: Default::default(),
            viewer: Default::default(),
            locale,
            focused: false,
            image_focus: cx.focus_handle(),
            input_image: None,
            dialog: PlainDialog::new(cx),
            retired: None,
        }
    }
    pub fn set_data(&mut self, data: Data, locale: Text, cx: &mut Context<Self>) {
        let changed = self.data.epoch != data.epoch
            || self.data.info.as_ref().map(|v| &v.id) != data.info.as_ref().map(|v| &v.id);
        if changed {
            self.viewer.reset_file(cx);
            self.input_image = None;
        }
        let input_image = data
            .image
            .as_ref()
            .map(|image| Arc::as_ptr(&image.rendered) as usize);
        if input_image != self.input_image {
            self.viewer.image = data.image.clone();
            self.input_image = input_image;
        }
        self.viewer.group = data.group.clone();
        self.viewer.index = data.index;
        self.viewer.image_failed = data.image_failed;
        self.viewer.overview = data.overview;
        self.viewer.text = data.text.clone();
        self.viewer.document = data.document.clone();
        self.viewer.source_document = data.source_document.clone();
        self.viewer.text_truncated = data.text_truncated;
        self.data = data;
        self.locale = locale;
        cx.notify();
    }
    pub fn remember_source(&mut self, focus: Option<FocusHandle>) {
        self.viewer.return_focus = focus;
    }
    pub fn inspect(&self) -> serde_json::Value {
        self.dialog.inspect()
    }
    fn close(&mut self, cx: &mut Context<Self>) {
        self.viewer.menu.dismiss();
        self.data.info = None;
        cx.emit(Action::Close);
        cx.notify();
    }
    fn move_preview(&mut self, step: isize, cx: &mut Context<Self>) {
        cx.emit(Action::Move(step));
    }
    fn save_artifact_copy(&mut self, cx: &mut Context<Self>) {
        cx.emit(Action::Save);
    }
    fn retry(&mut self, cx: &mut Context<Self>) {
        cx.emit(Action::Retry);
    }
    fn zoom_preview(&mut self, zoom: Option<f32>, cx: &mut Context<Self>) {
        self.viewer.zoom = zoom.map(|value| value.clamp(0.1, 20.));
        self.viewer.scroll.set_offset(gpui::point(px(0.), px(0.)));
        cx.notify();
    }

    fn ensure_preview_raster(&mut self, scale_factor: f32, cx: &mut Context<Self>) {
        if self.viewer.source {
            return;
        }
        let Some(image) = &self.viewer.image else {
            return;
        };
        let Some(svg) = image.svg.clone() else {
            return;
        };
        let width = (image.size.width * self.viewer.scale() * scale_factor)
            .ceil()
            .clamp(1., 4096.) as u32;
        if self.viewer.target_raster_width == width {
            return;
        }
        self.viewer.target_raster_width = width;
        self.viewer.raster_request = self.viewer.raster_request.wrapping_add(1);
        let request = self.viewer.raster_request;
        let renderer = cx.svg_renderer();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    renderer.render_parsed(
                        &svg,
                        gpui::SvgSize::Size(gpui::size(
                            gpui::DevicePixels(width as i32),
                            gpui::DevicePixels(width as i32),
                        )),
                    )
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if v.viewer.raster_request != request {
                    return;
                }
                if let (Ok(rendered), Some(image)) = (result, &mut v.viewer.image) {
                    let old = std::mem::replace(&mut image.rendered, rendered);
                    cx.drop_image(old, None);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn sync_preview_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let image_open = self.data.info.as_ref().is_some_and(|info| info.image_view);
        if image_open && !self.focused {
            let focus = self.image_focus.clone();
            if self.viewer.return_focus.is_none() {
                self.viewer.return_focus = window.focused(cx);
            }
            window.focus(&focus, cx);
            self.focused = true;
        } else if !image_open && self.focused {
            if let Some(previous) = self.viewer.return_focus.take() {
                window.focus(&previous, cx);
            }
            self.focused = false;
        }
    }

    fn render_preview(
        &mut self,
        artifact: Info,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let image_view = artifact.image_view;
        let available = window.viewport_size();
        let width =
            (available.width.as_f32() - 40.).clamp(280., if image_view { f32::MAX } else { 960. });
        let height =
            (available.height.as_f32() - 40.).clamp(280., if image_view { f32::MAX } else { 760. });
        let narrow = width < 560.;
        let gutter = if image_view {
            12.
        } else if narrow {
            20.
        } else {
            24.
        };
        let strip = image_view && self.viewer.group.len() > 1;
        let stage_height = if image_view {
            (height - gutter * 2. - IMAGE_BAR - 16. - if strip { STRIP + 16. } else { 0. }).max(80.)
        } else {
            (available.height.as_f32() - 240.).clamp(80., 440.)
        };
        self.viewer.view_size = gpui::size(width - gutter * 2., stage_height);
        self.ensure_preview_raster(window.scale_factor(), cx);
        self.viewer.focus = Some(if image_view {
            self.image_focus.clone()
        } else {
            self.dialog.focus_handle()
        });
        let focus = self
            .viewer
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let menu_focus =
            crate::components::widgets::controls::action_focus("preview-more", window, cx);
        let key_focus = focus.clone();
        let subtitle = artifact.subtitle.clone();
        let subtitle = if self.data.saving {
            self.locale.text("saving_file").to_owned()
        } else if let Some(notice) = self.data.notice {
            self.locale.text(notice).to_owned()
        } else {
            subtitle
        };
        let can_save = self.data.loaded && !self.data.saving && !self.data.choosing_save;
        let header = if image_view {
            self.image_bar(&artifact, &subtitle, &menu_focus, cx)
        } else {
            div().w_full().h(px(32.)).flex().justify_end().child(
                preview_icon("drive-close-preview", true)
                    .child(ui::icon("icons/x.svg", 12.))
                    .on_click(cx.listener(|v, _, _, cx| v.close(cx)))
                    .automation(AutomationRole::Button, self.locale.text("close")),
            )
        };
        let body = self.preview_body(&artifact, stage_height, cx).automation(
            AutomationRole::ScrollArea,
            self.locale.text("drive_preview"),
        );
        if !image_view {
            let footer = self.preview_footer(narrow, cx);
            let title_actions = div()
                .flex()
                .gap_1()
                .child(
                    preview_icon("drive-save", can_save)
                        .child(ui::icon("icons/download.svg", 16.))
                        .on_click(cx.listener(|v, _, _, cx| v.save_artifact_copy(cx)))
                        .automation_enabled(
                            can_save,
                            AutomationRole::Button,
                            self.locale.text("drive_save_copy"),
                        ),
                )
                .child(
                    self.viewer
                        .menu
                        .trigger_element(
                            preview_icon("preview-more", true)
                                .child(ui::icon("icons/settings-three.svg", 16.)),
                            &menu_focus,
                            true,
                            cx,
                        )
                        .automation(AutomationRole::Button, self.locale.text("preview_more")),
                );
            let content = div()
                .flex()
                .flex_col()
                .min_h_0()
                .gap_3()
                .capture_key_down(cx.listener(|v, event: &KeyDownEvent, _, cx| {
                    if v.viewer.menu.is_open() {
                        return;
                    }
                    if event.keystroke.modifiers.platform && event.keystroke.key == "c" {
                        if let Some(source) = v.viewer.selection.borrow_mut().finish() {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(source.quote));
                        }
                        cx.stop_propagation();
                    } else if !event.keystroke.modifiers.modified()
                        && v.viewer.selection.borrow_mut().finish().is_none()
                    {
                        match event.keystroke.key.as_str() {
                            "left" => {
                                v.move_preview(-1, cx);
                                cx.stop_propagation();
                            }
                            "right" => {
                                v.move_preview(1, cx);
                                cx.stop_propagation();
                            }
                            _ => {}
                        }
                    }
                }))
                .child(ui::text_role(subtitle, crate::design::TextRole::Metadata))
                .child(body)
                .children(self.preview_menu(window, cx));
            return self
                .dialog
                .render_with_options(
                    "attachment-preview-dialog",
                    artifact.name.clone(),
                    content,
                    Some(footer.into_any_element()),
                    self.data.info.as_ref().is_some_and(|info| !info.image_view),
                    DialogOptions {
                        title_action: Some(title_actions.into_any_element()),
                        notice: self.data.notice.map(|key| self.locale.text(key).to_owned()),
                        ..Default::default()
                    },
                    window,
                    cx,
                    |v, _, cx| v.close(cx),
                )
                .unwrap_or_else(|| gpui::Empty.into_any_element());
        }
        let panel = div()
            .id("attachment-preview-dialog")
            .track_focus(&focus)
            .tab_group()
            .tab_stop(false)
            .relative()
            .w(px(width))
            .h(px(height))
            .p(px(gutter))
            .flex()
            .flex_col()
            .gap_4()
            .when(!image_view, |v| v.rounded(px(ui::MODAL_RADIUS)).bg(rgb(BG())))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(move |v, event: &KeyDownEvent, window, cx| {
                if v.viewer.menu.is_open() {
                    return;
                }
                if event.keystroke.key == "escape" {
                    v.close(cx);
                    cx.notify();
                    cx.stop_propagation();
                } else if event.keystroke.key == "tab" {
                    crate::modal::cycle_focus(
                        &key_focus,
                        event.keystroke.modifiers.shift,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                } else if event.keystroke.modifiers.platform && event.keystroke.key == "c" {
                    if let Some(source) = v.viewer.selection.borrow_mut().finish() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(source.quote));
                    }
                    cx.stop_propagation();
                } else if !event.keystroke.modifiers.modified()
                    && !v.viewer.menu.is_open()
                    && v.viewer.selection.borrow_mut().finish().is_none()
                {
                    match event.keystroke.key.as_str() {
                        "left" => {
                            v.move_preview(-1, cx);
                            cx.stop_propagation();
                        }
                        "right" => {
                            v.move_preview(1, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }
            }))
            .child(header)
            .child(self.image_stage(body, cx))
            .when(strip, |v| v.child(self.image_strip(cx)));
        let panel = if image_view {
            self.viewer
                .menu
                .context_element(panel, &focus, true, cx)
                .into_any_element()
        } else {
            panel.into_any_element()
        };
        let panel = div().child(panel).children(self.preview_menu(window, cx));
        div()
            .relative()
            .w(window.viewport_size().width)
            .h(window.viewport_size().height)
            .occlude()
            .bg(if image_view {
                rgb(ZORK_UI.palette.sidebar)
            } else {
                rgba(0x00000059)
            })
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    v.close(cx);
                    cx.notify();
                }),
            )
            .child(panel)
            .into_any_element()
    }

    /// Image view top bar: name with "i / n · type · size", zoom, more, close.
    fn image_bar(
        &self,
        artifact: &Info,
        subtitle: &str,
        menu_focus: &FocusHandle,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = ZORK_UI.palette;
        let v = &self.viewer;
        let position = if v.group.len() > 1 {
            format!("{} / {} · {subtitle}", v.index + 1, v.group.len())
        } else {
            subtitle.to_owned()
        };
        let mut bar = div()
            .w_full()
            .h(px(IMAGE_BAR))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .pl_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.text))
                            .truncate()
                            .child(artifact.name.clone()),
                    )
                    .child(ui::text_role(position, crate::design::TextRole::Metadata).truncate()),
            );
        if v.image.is_some() && !v.source {
            let scale = v.scale();
            bar = bar
                .child(
                    preview_icon("preview-zoom-out", scale > 0.1)
                        .child(ui::icon("icons/minus.svg", 14.))
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.zoom_preview(Some(v.viewer.scale() / 1.25), cx)
                        }))
                        .automation(AutomationRole::Button, self.locale.text("preview_zoom_out")),
                )
                .child(
                    preview_quiet(
                        "preview-actual",
                        format!("{:.0}%", scale * 100.),
                        true,
                        ui::IconButtonSize::Standard,
                    )
                    .w(px(56.))
                    .on_click(cx.listener(|v, _, _, cx| v.zoom_preview(Some(1.), cx)))
                    .automation(AutomationRole::Button, self.locale.text("preview_actual")),
                )
                .child(
                    preview_icon("preview-zoom-in", scale < 20.)
                        .child(ui::icon("icons/plus.svg", 14.))
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.zoom_preview(Some(v.viewer.scale() * 1.25), cx)
                        }))
                        .automation(AutomationRole::Button, self.locale.text("preview_zoom_in")),
                )
                .child(
                    preview_quiet(
                        "preview-fit",
                        self.locale.text("preview_fit"),
                        true,
                        ui::IconButtonSize::Standard,
                    )
                    .on_click(cx.listener(|v, _, _, cx| v.zoom_preview(None, cx)))
                    .automation(AutomationRole::Button, self.locale.text("preview_fit")),
                );
        }
        bar.child(
            self.viewer
                .menu
                .trigger_element(
                    preview_icon("preview-more", true)
                        .child(ui::icon("icons/more-horizontal.svg", 16.)),
                    menu_focus,
                    true,
                    cx,
                )
                .automation(AutomationRole::Button, self.locale.text("preview_more")),
        )
        .child(
            preview_icon("drive-close-preview", true)
                .child(ui::icon("icons/x.svg", 14.))
                .on_click(cx.listener(|v, _, _, cx| v.close(cx)))
                .automation(AutomationRole::Button, self.locale.text("close")),
        )
    }

    /// The image stage with round previous/next buttons at its sides.
    fn image_stage(&self, body: impl IntoElement, cx: &mut Context<Self>) -> Div {
        let p = &self.viewer;
        let mut stage = div().relative().w_full().flex_shrink_0().child(body);
        if p.group.len() < 2 {
            return stage;
        }
        for (step, id, icon, key) in [
            (-1isize, "preview-previous", "icons/arrow-left.svg", "preview_previous"),
            (1, "preview-next", "icons/arrow-right.svg", "preview_next"),
        ] {
            let enabled = if step < 0 {
                p.index > 0
            } else {
                p.index + 1 < p.group.len()
            };
            let button = div()
                .rounded_full()
                .bg(rgb(ZORK_UI.palette.canvas))
                .shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., 0.12),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(8.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .child(
                    preview_icon(id, enabled)
                        .radius(ui::IconButtonSize::Standard.extent() / 2.)
                        .child(ui::icon(icon, 16.))
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if enabled {
                                v.move_preview(step, cx)
                            }
                        }))
                        .automation_enabled(enabled, AutomationRole::Button, self.locale.text(key)),
                );
            let rail = div().absolute().top_0().bottom_0().flex().items_center();
            stage = stage.child(if step < 0 {
                rail.left_2().child(button)
            } else {
                rail.right_2().child(button)
            });
        }
        stage
    }

    /// Thumbnails of the message's files; the current one has an ink ring.
    fn image_strip(&self, cx: &mut Context<Self>) -> Div {
        let p = ZORK_UI.palette;
        let current = self.viewer.index;
        let mut strip = div()
            .w_full()
            .h(px(STRIP))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .gap_2();
        for (index, info) in self.viewer.group.iter().enumerate() {
            let selected = index == current;
            let thumbnail = self.data.thumbnails.get(index).cloned().flatten();
            let step = index as isize - current as isize;
            let radius = px(8.);
            let tile = div()
                .id(SharedString::from(format!("preview-strip-{index}")))
                .w(px(44.))
                .h(px(32.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius)
                .bg(rgb(p.canvas))
                .text_color(rgb(p.subtle))
                .cursor_pointer()
                .when(selected, |v| v.border_2().border_color(rgb(p.text)))
                .when(!selected, |v| v.opacity(0.75).hover(|s| s.opacity(1.)))
                .when_some(thumbnail, |v, image| {
                    v.child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                let source = image.size(0);
                                let (w, h) = (
                                    (i32::from(source.width) as f32).max(1.),
                                    (i32::from(source.height) as f32).max(1.),
                                );
                                let scale = (bounds.size.width.as_f32() / w)
                                    .max(bounds.size.height.as_f32() / h);
                                let size = gpui::size(px(w * scale), px(h * scale));
                                let image_bounds = Bounds::new(
                                    bounds.center() - point(size.width / 2., size.height / 2.),
                                    size,
                                );
                                let _ = window.paint_image(
                                    bounds,
                                    image_bounds,
                                    Corners::all(px(6.)),
                                    image.clone(),
                                    0,
                                    false,
                                );
                            },
                        )
                        .size_full(),
                    )
                })
                .when(self.data.thumbnails.get(index).cloned().flatten().is_none(), |v| {
                    v.child(ui::icon("icons/file.svg", 14.))
                })
                .on_click(cx.listener(move |v, _, _, cx| {
                    if step != 0 {
                        v.move_preview(step, cx)
                    }
                }))
                .automation(AutomationRole::Button, info.name.clone());
            strip = strip.child(tile);
        }
        strip
    }

    fn preview_body(
        &self,
        artifact: &Info,
        height: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let viewer = &self.viewer;
        let mut body = div()
            .id("drive-preview")
            .w_full()
            .h(px(height))
            .flex_shrink_0()
            .overflow_scroll()
            .track_scroll(&viewer.scroll);
        if self.data.failed {
            return body.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(ui::text_role(
                        self.locale.text("drive_preview_failed"),
                        crate::design::TextRole::Body,
                    ))
                    .child(
                        ui::button(
                            "drive-retry-preview",
                            self.locale.text("inbox_retry"),
                            false,
                            true,
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            if v.data.info.is_some() {
                                v.retry(cx);
                            }
                        }))
                        .automation(AutomationRole::Button, self.locale.text("inbox_retry")),
                    ),
            );
        }
        if !self.data.loaded {
            return body.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(loading::status(
                        "drive-preview-loading",
                        self.locale.text("loading_preview"),
                    )),
            );
        }
        if let Some(image) = viewer.image.as_ref().filter(|_| !viewer.source) {
            let scale = viewer.scale();
            let size = gpui::size(image.size.width * scale, image.size.height * scale);
            let content_width = viewer.view_size.width.max(size.width + 64.);
            let content_height = height.max(size.height + 64.);
            body = body
                .cursor(gpui::CursorStyle::OpenHand)
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|v, event: &gpui::MouseDownEvent, _, cx| {
                        v.viewer.menu.dismiss();
                        v.viewer.drag = Some((event.position, v.viewer.scroll.offset()));
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_move(cx.listener(|v, event: &gpui::MouseMoveEvent, _, cx| {
                    if let Some((start, offset)) = v.viewer.drag {
                        v.viewer.scroll.set_offset(offset + event.position - start);
                        cx.notify();
                        cx.stop_propagation();
                    }
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|v, _, _, _| v.viewer.drag = None),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|v, _, _, _| v.viewer.drag = None),
                );
            return body
                .relative()
                .child(crate::components::attachments::image_viewport(
                    image.rendered.clone(),
                    gpui::size(px(size.width), px(size.height)),
                    gpui::size(px(content_width), px(content_height)),
                ))
                .when(viewer.overview, |v| {
                    v.child(div().absolute().left_3().top_3().child(ui::text_role(
                        self.locale.text("preview_overview"),
                        crate::design::TextRole::Metadata,
                    )))
                });
        }
        if viewer.image_failed && !viewer.source {
            return body.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_6()
                    .child(ui::text_role(
                        self.locale.text("preview_content_failed"),
                        crate::design::TextRole::Body,
                    )),
            );
        }
        let document = if viewer.source {
            viewer.source_document.as_ref()
        } else {
            viewer.document.as_ref()
        };
        if let Some(document) = document {
            if viewer.text.as_ref().is_some_and(|text| text.is_empty()) {
                return body.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui::text_role(
                            self.locale.text("preview_empty"),
                            crate::design::TextRole::Body,
                        )),
                );
            }
            viewer.selection.borrow_mut().begin_frame();
            let root = cx.entity().downgrade();
            let selection = crate::components::selection::SelectionContext::new(
                "attachment-preview-selection".into(),
                crate::comments::CommentSource::default(),
                document.shared_plain_text(),
                viewer.selection.clone(),
                viewer.focus.as_ref().unwrap().clone(),
                Rc::new(move |cx| {
                    let _ = root.update(cx, |_, cx| cx.notify());
                }),
            );
            body = body
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|v, _, window, cx| {
                        v.viewer.selection.borrow_mut().clear();
                        v.viewer.menu.dismiss();
                        if let Some(focus) = &v.viewer.focus {
                            window.focus(focus, cx);
                        }
                        cx.notify();
                    }),
                )
                .on_mouse_move(cx.listener(|v, event: &gpui::MouseMoveEvent, _, cx| {
                    if v.viewer.selection.borrow_mut().update(event.position) {
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|v, _, _, _| {
                        v.viewer.selection.borrow_mut().finish();
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|v, _, _, _| {
                        v.viewer.selection.borrow_mut().finish();
                    }),
                );
            return body.child(
                div()
                    .p_6()
                    .text_size(px(13.))
                    .line_height(px(21.))
                    .when(viewer.source, |v| v.font_family("JetBrains Mono"))
                    .child(render_selectable_document(
                        "attachment-content",
                        document,
                        &selection,
                    ))
                    .when(viewer.text_truncated, |v| {
                        v.child(ui::text_role(
                            self.locale.text("preview_truncated"),
                            crate::design::TextRole::Metadata,
                        ))
                    }),
            );
        }
        body.child(
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .flex_col()
                .gap_3()
                .p_6()
                .child(ui::icon("icons/file.svg", 32.))
                .child(ui::text_role(
                    artifact.name.clone(),
                    crate::design::TextRole::Label,
                ))
                .child(ui::text_role(
                    self.locale.text("drive_download_to_view"),
                    crate::design::TextRole::Body,
                ))
                .child(
                    preview_quiet(
                        "preview-fallback-save",
                        self.locale.text("drive_save_copy"),
                        !self.data.saving && !self.data.choosing_save,
                        ui::IconButtonSize::Standard,
                    )
                    .on_click(cx.listener(|v, _, _, cx| v.save_artifact_copy(cx)))
                    .automation(AutomationRole::Button, self.locale.text("drive_save_copy")),
                ),
        )
    }

    fn preview_footer(&self, narrow: bool, cx: &mut Context<Self>) -> Div {
        let p = &self.viewer;
        let previous = p.index > 0;
        let next = p.index + 1 < p.group.len();
        let navigation = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                preview_icon("preview-previous", previous)
                    .child(ui::icon("icons/arrow-left.svg", 16.))
                    .on_click(cx.listener(|v, _, _, cx| v.move_preview(-1, cx)))
                    .automation_enabled(
                        previous,
                        AutomationRole::Button,
                        self.locale.text("preview_previous"),
                    ),
            )
            .child(
                ui::text_role(
                    format!("{} / {}", p.index + 1, p.group.len().max(1)),
                    crate::design::TextRole::Metadata,
                )
                .w(px(40.))
                .text_center(),
            )
            .child(
                preview_icon("preview-next", next)
                    .child(ui::icon("icons/arrow-right.svg", 16.))
                    .on_click(cx.listener(|v, _, _, cx| v.move_preview(1, cx)))
                    .automation_enabled(
                        next,
                        AutomationRole::Button,
                        self.locale.text("preview_next"),
                    ),
            );
        let mut footer = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .when(narrow, |v| v.flex_wrap())
            .child(navigation);
        if p.image.is_some() && !p.source {
            let scale = p.scale();
            let tools = div()
                .flex()
                .items_center()
                .px_1()
                .child(
                    preview_quiet(
                        "preview-zoom-out",
                        "−",
                        scale > 0.1,
                        ui::IconButtonSize::Standard,
                    )
                    .on_click(
                        cx.listener(|v, _, _, cx| {
                            v.zoom_preview(Some(v.viewer.scale() / 1.25), cx)
                        }),
                    )
                    .automation(AutomationRole::Button, self.locale.text("preview_zoom_out")),
                )
                .child(
                    preview_quiet(
                        "preview-actual",
                        format!("{:.0}%", scale * 100.),
                        true,
                        ui::IconButtonSize::Standard,
                    )
                    .w(px(56.))
                    .on_click(cx.listener(|v, _, _, cx| v.zoom_preview(Some(1.), cx)))
                    .automation(AutomationRole::Button, self.locale.text("preview_actual")),
                )
                .child(
                    preview_icon("preview-zoom-in", scale < 20.)
                        .child(ui::icon("icons/plus.svg", 14.))
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.zoom_preview(Some(v.viewer.scale() * 1.25), cx)
                        }))
                        .automation(AutomationRole::Button, self.locale.text("preview_zoom_in")),
                )
                .child(
                    preview_quiet(
                        "preview-fit",
                        self.locale.text("preview_fit"),
                        true,
                        ui::IconButtonSize::Standard,
                    )
                    .on_click(cx.listener(|v, _, _, cx| v.zoom_preview(None, cx)))
                    .automation(AutomationRole::Button, self.locale.text("preview_fit")),
                );
            footer = footer.child(tools);
        } else {
            footer = footer.child(ui::text_role(
                self.locale.text("preview_read_only"),
                crate::design::TextRole::Metadata,
            ));
        }
        footer
    }

    fn preview_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        use crate::components::standard_menu::Item;
        let can_save = self.data.loaded && !self.data.saving && !self.data.choosing_save;
        let save = Item::new("preview-save-copy", self.locale.text("drive_save_copy"))
            .icon("icons/download.svg");
        let mut items = vec![if can_save { save } else { save.disabled() }];
        if self.viewer.text.is_some() {
            items.push(Item::new(
                "preview-source",
                self.locale.text(if self.viewer.source {
                    "preview_rendered"
                } else {
                    "preview_source"
                }),
            ));
        }
        if let Some(artifact) = &self.data.info {
            let mut info = vec![Item::new("preview-info-name", artifact.name.clone()).disabled()];
            if !artifact.source_path.starts_with("file-") && !artifact.source_path.is_empty() {
                info.push(Item::new("preview-info-path", artifact.source_path.clone()).disabled());
            }
            if artifact.version > 1 {
                info.push(
                    Item::new("preview-info-version", format!("v{}", artifact.version)).disabled(),
                );
            }
            if !artifact.created_at.is_empty() {
                info.push(
                    Item::new("preview-info-created", artifact.created_at.clone()).disabled(),
                );
            }
            items.push(Item::new("preview-info", self.locale.text("preview_info")).submenu(info));
        }
        if self.data.can_reuse {
            items.push(Item::new(
                "drive-reuse",
                self.locale.text("reuse_attachment"),
            ));
        }
        self.viewer
            .menu
            .render("preview-menu", items, window, cx, |v, key, window, cx| {
                match key.as_str() {
                    "preview-save-copy" => v.save_artifact_copy(cx),
                    "preview-source" => {
                        v.viewer.source = !v.viewer.source;
                        v.viewer.selection.borrow_mut().clear();
                        v.viewer.scroll.set_offset(gpui::point(px(0.), px(0.)));
                        if let Some(focus) = &v.viewer.focus {
                            window.focus(focus, cx);
                        }
                    }
                    "drive-reuse" => {
                        cx.emit(Action::Reuse);
                        if let Some(focus) = &v.viewer.focus {
                            window.focus(focus, cx);
                        }
                    }
                    _ => {}
                }
                cx.notify();
            })
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_preview_focus(window, cx);
        let document_open = self.data.info.as_ref().is_some_and(|info| !info.image_view);
        if let Some(info) = self.data.info.as_ref().filter(|info| !info.image_view) {
            self.retired = Some(info.clone());
        }
        let release = !document_open && !self.dialog.alive();
        let Some(info) = self.data.info.clone().or_else(|| self.retired.clone()) else {
            return gpui::Empty.into_any_element();
        };
        if !info.image_view {
            let result = self.render_preview(info, window, cx);
            if release {
                self.retired = None;
            }
            return result;
        }
        let retiring = self
            .retired
            .clone()
            .map(|info| self.render_preview(info, window, cx));
        if release {
            self.retired = None;
        }
        div()
            .children(retiring)
            .child(
                deferred(
                    anchored()
                        .position(point(px(0.), px(0.)))
                        .child(self.render_preview(info, window, cx)),
                )
                .with_priority(100),
            )
            .into_any_element()
    }
}

#[cfg(feature = "stories")]
pub mod stories;

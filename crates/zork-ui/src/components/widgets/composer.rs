//! Composer layout on ordinary GPUI surfaces.
//! Hosts supply core capabilities and handle intents. Editor, hover and focus
//! are presentation state; this module contains no send policy. Draft files sit
//! in a row of capsules at the top of the surface, above the editor.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        attachment_row as row_geometry, composer_layout as spec, text_input::ComposerInput,
    },
};
use gpui::{prelude::*, *};
use std::rc::Rc;
pub mod files;
mod scene;
pub use scene::Scene;
pub use zork_client_types::composer::{Capabilities, Snapshot};

pub const EDITOR_MIN: f32 = 24.;
pub const EDITOR_MAX: f32 = 60.;
pub const TEXT_INSET: f32 = 20.;

#[derive(Clone)]
pub enum Action {
    Primary,
    FocusEditor,
    ChooseFiles,
    RemoveFile(u64),
    OpenFile(u64),
}
pub type Handler = Rc<dyn Fn(Action, &mut Window, &mut App)>;

fn positioned(x: f64, y: f64, w: f64, h: f64) -> Div {
    div()
        .absolute()
        .left(px(x as f32))
        .top(px(y as f32))
        .w(px(w.max(0.) as f32))
        .h(px(h.max(0.) as f32))
}

fn action(
    id: String,
    attach: bool,
    size: f32,
    enabled: bool,
    stop: bool,
    busy: bool,
    handler: Handler,
    window: &mut Window,
    cx: &mut App,
) -> super::controls::Action {
    super::controls::action(
        id,
        "",
        size,
        size,
        super::controls::ActionStyle {
            primary: !attach,
            accent: !attach && !stop,
            icon: Some(if attach {
                "icons/paperclip.svg"
            } else if stop {
                "icons/phosphor-stop-fill.svg"
            } else {
                "icons/arrow-up.svg"
            }),
            disabled: !enabled,
            busy: !attach && busy,
            ..Default::default()
        },
        spec::SURFACE_COLOR(),
        window,
        cx,
    )
    .on_click(move |_, w, cx| {
        if enabled {
            handler(
                if attach {
                    Action::ChooseFiles
                } else {
                    Action::Primary
                },
                w,
                cx,
            );
        }
    })
}

pub struct Presentation {
    pub editor_id: String,
    pub attach_id: String,
    pub primary_id: String,
    /// Host-rendered draft file row (see [`files::render`]); it occupies
    /// [`row_geometry::FILES_BAND`] at the top of the surface.
    pub files: Option<AnyElement>,
    /// Host-rendered draft comments (`components::comments::drafts`) and the
    /// band they occupy at the top of the surface, above files and the editor.
    pub drafts: Option<(AnyElement, f32)>,
    pub busy: bool,
    pub editor_label: SharedString,
    pub attach_label: SharedString,
    pub show_attach: bool,
    pub primary_label: SharedString,
}

pub struct Props<'a> {
    pub id: &'a str,
    pub scene: &'a Scene,
    pub width: f32,
    pub height: f32,
    pub editor: &'a Entity<ComposerInput>,
    pub snapshot: &'a Snapshot,
    pub handler: Handler,
    pub action_size: f32,
    pub presentation: Option<Presentation>,
    /// Extra space kept above the action row so accessories do not cover the editor.
    pub accessory_band: f32,
    pub accessories: Vec<AnyElement>,
}
/// The GPUI editor retains selection, IME, scrolling and key bindings.
pub fn render(props: Props<'_>, window: &mut Window, cx: &mut App) -> AnyElement {
    let Props {
        id,
        scene,
        width,
        height,
        editor,
        snapshot,
        handler,
        action_size,
        mut presentation,
        accessory_band,
        accessories,
    } = props;
    let p = scene.body();
    let c = snapshot.capabilities;
    let custom_files = presentation.as_mut().and_then(|p| p.files.take());
    let drafts = presentation.as_mut().and_then(|p| p.drafts.take());
    let drafts_band = drafts.as_ref().map_or(0., |(_, height)| height.max(0.));
    let has_files = custom_files.is_some() || !snapshot.files.is_empty();
    let files_band = if has_files { row_geometry::FILES_BAND } else { 0. };
    let band = files_band + drafts_band;
    let editor_height = (p.h as f32
        - band
        - spec::TOP_EXTENSION
        - spec::COMPOSER_CHROME
        - accessory_band.max(0.))
    .clamp(0., EDITOR_MAX);
    let input = editor.clone();
    let editor_id = presentation
        .as_ref()
        .map_or_else(|| format!("{id}-editor"), |p| p.editor_id.clone());
    let editor_view = positioned(
        p.left() + TEXT_INSET as f64,
        p.top() + (band + spec::TOP_EXTENSION + spec::EDITOR_TOP_INSET) as f64,
        p.w - 2. * TEXT_INSET as f64,
        editor_height as f64,
    )
    .id(editor_id)
    .text_size(px(13.))
    .line_height(px(20.))
    .text_color(rgb(spec::TEXT_COLOR()))
    .overflow_hidden()
    .when(c.editable, |v| {
        v.child(input.clone())
            .on_click(move |_, w, cx| w.focus(&input.read(cx).focus_handle(), cx))
    })
    .when(!c.editable, |v| {
        v.child(if snapshot.text.is_empty() {
            "此会话不可发送消息".to_owned()
        } else {
            snapshot.text.clone()
        })
    });
    let focus_editor = handler.clone();
    let plate = positioned(p.left(), p.top(), p.w, p.h)
        .id(format!("{id}-surface"))
        .rounded(px(spec::SURFACE_RADIUS))
        .bg(rgb(spec::SURFACE_COLOR()))
        .border(px(spec::BORDER_WIDTH))
        .border_color(rgb(spec::BORDER_COLOR()))
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_, w, cx| {
            if c.editable {
                focus_editor(Action::FocusEditor, w, cx);
            }
        });
    let mut content = div()
        .relative()
        .size_full()
        .child(plate.automation(AutomationRole::Status, "消息输入区"))
        .child(editor_view.automation_enabled(
            c.editable,
            AutomationRole::TextInput,
            presentation.as_ref().map_or_else(
                || SharedString::from("消息输入"),
                |p| p.editor_label.clone(),
            ),
        ));
    let action_size = action_size.max(2.) as f64;
    let inset = spec::ACTION_INSET as f64;
    for (key, attach, x) in [
        ("attach", true, p.left() + inset),
        ("send", false, p.left() + p.w - inset - action_size),
    ] {
        if attach && presentation.as_ref().is_some_and(|p| !p.show_attach) {
            continue;
        }
        content = content.child(
            positioned(
                x,
                p.top() + p.h - inset - action_size,
                action_size,
                action_size,
            )
            .child(
                action(
                    presentation.as_ref().map_or_else(
                        || format!("{id}-{key}"),
                        |p| {
                            if attach {
                                p.attach_id.clone()
                            } else {
                                p.primary_id.clone()
                            }
                        },
                    ),
                    attach,
                    action_size as f32,
                    if attach { c.editable } else { c.enabled },
                    c.stop,
                    presentation.as_ref().is_some_and(|p| p.busy),
                    handler.clone(),
                    window,
                    cx,
                )
                .automation_enabled(
                    if attach { c.editable } else { c.enabled },
                    AutomationRole::Button,
                    presentation.as_ref().map_or_else(
                        || {
                            SharedString::from(if attach {
                                "添加附件"
                            } else if c.stop {
                                "停止当前运行"
                            } else {
                                "发送消息"
                            })
                        },
                        |p| {
                            if attach {
                                p.attach_label.clone()
                            } else {
                                p.primary_label.clone()
                            }
                        },
                    ),
                ),
            ),
        );
    }
    if !accessories.is_empty() {
        let action = action_size;
        let inset = spec::ACTION_INSET as f64;
        let show_attach = presentation.as_ref().is_none_or(|item| item.show_attach);
        let left = if show_attach {
            inset + action + inset
        } else {
            TEXT_INSET as f64
        };
        let right = inset + action + inset;
        content = content.child(
            positioned(
                p.left() + left,
                p.top() + p.h - action - inset,
                (p.w - left - right).max(2.),
                action,
            )
            .flex()
            .items_center()
            .gap(px(12.))
            .children(accessories),
        );
    }
    let inset = spec::ACTION_INSET as f64;
    let row_width = (p.w - 2. * inset).max(2.);
    let files_row = custom_files.or_else(|| {
        (!snapshot.files.is_empty()).then(|| render_files(id, snapshot, row_width as f32, handler))
    });
    div()
        .relative()
        .w(px(width))
        .h(px(height))
        .child(content)
        .when_some(drafts, |v, (drafts, height)| {
            v.child(
                positioned(
                    p.left() + TEXT_INSET as f64,
                    p.top() + (spec::TOP_EXTENSION + spec::EDITOR_TOP_INSET) as f64,
                    p.w - 2. * TEXT_INSET as f64,
                    height as f64,
                )
                .child(drafts),
            )
        })
        .when_some(files_row, |v, row| {
            v.child(
                positioned(
                    p.left() + inset,
                    p.top() + inset + drafts_band as f64,
                    row_width,
                    row_geometry::CHIP_HEIGHT as f64,
                )
                .child(row),
            )
        })
        .into_any_element()
}

fn render_files(id: &str, state: &Snapshot, width: f32, handler: Handler) -> AnyElement {
    let ids = files::Ids {
        root: format!("{id}-files"),
        file_prefix: format!("{id}-file-"),
        remove_prefix: format!("{id}-remove-"),
    };
    let entries = state
        .files
        .iter()
        .map(|file| files::File {
            id: file.id.to_string(),
            name: file.name.clone(),
            meta: crate::components::attachments::file_badge(&file.name),
            image: None,
            state: files::State::Ready,
            removable: true,
        })
        .collect();
    let handler = Rc::new(move |action, w: &mut Window, cx: &mut App| match action {
        files::Action::Open(id) => {
            if let Ok(id) = id.parse() {
                handler(Action::OpenFile(id), w, cx);
            }
        }
        files::Action::Remove(id) => {
            if let Ok(id) = id.parse() {
                handler(Action::RemoveFile(id), w, cx);
            }
        }
        files::Action::Retry(_) => {}
    });
    files::render(ids, entries, width, "移除附件".into(), handler).into_any_element()
}

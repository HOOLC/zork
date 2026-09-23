//! Composer layout on ordinary GPUI surfaces.
//! Hosts supply core capabilities and handle intents. Editor, hover, focus and
//! fan expansion are presentation state; this module contains no send policy.
use super::Pose;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        attachment_fan as fan_geometry, composer_layout as spec, text_input::ComposerInput,
    },
};
use gpui::{prelude::*, *};
use std::rc::Rc;
pub mod fan;
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
    FanHover(bool),
    ToggleFan,
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
        spec::SURFACE_COLOR,
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
    pub fan: Option<AnyElement>,
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
    pub fan_progress: f32,
    pub fan_pinned: bool,
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
        fan_progress,
        fan_pinned,
        handler,
        action_size,
        mut presentation,
        accessory_band,
        accessories,
    } = props;
    let p = scene.body();
    let c = snapshot.capabilities;
    let editor_height =
        (p.h as f32 - spec::TOP_EXTENSION - spec::COMPOSER_CHROME - accessory_band.max(0.))
            .clamp(0., EDITOR_MAX);
    let input = editor.clone();
    let editor_id = presentation
        .as_ref()
        .map_or_else(|| format!("{id}-editor"), |p| p.editor_id.clone());
    let editor_view = positioned(
        p.left() + TEXT_INSET as f64,
        p.top() + spec::TOP_EXTENSION as f64 + spec::EDITOR_TOP_INSET as f64,
        p.w - 2. * TEXT_INSET as f64,
        editor_height as f64,
    )
    .id(editor_id)
    .text_size(px(13.))
    .line_height(px(20.))
    .text_color(rgb(spec::TEXT_COLOR))
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
        .bg(rgb(spec::SURFACE_COLOR))
        .border(px(spec::BORDER_WIDTH))
        .border_color(rgb(spec::BORDER_COLOR))
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
    let custom_fan = presentation.as_mut().and_then(|p| p.fan.take());
    let has_custom_fan = custom_fan.is_some();
    div()
        .relative()
        .w(px(width))
        .h(px(height))
        .child(content)
        .children(custom_fan)
        .when(!has_custom_fan && !snapshot.files.is_empty(), |v| {
            v.child(render_fan(
                id,
                p,
                snapshot,
                fan_progress,
                fan_pinned,
                handler,
                window,
                cx,
            ))
        })
        .into_any_element()
}

fn render_fan(
    id: &str,
    body: Pose,
    state: &Snapshot,
    expanded: f32,
    pinned: bool,
    handler: Handler,
    _: &mut Window,
    _: &mut App,
) -> AnyElement {
    let available = (body.w as f32 - 24.).max(100.);
    let (width, height) =
        fan_geometry::dimensions(state.files.len().min(fan_geometry::MAX_FILES), available);
    let center = (body.left() + body.w - width as f64 / 2. - 12.).max(body.cx);
    let ids = fan::Ids {
        root: format!("{id}-fan"),
        toggle: format!("{id}-fan-toggle"),
        file_prefix: format!("{id}-file-"),
        remove_prefix: format!("{id}-remove-"),
    };
    let frame = fan::Frame {
        files: state
            .files
            .iter()
            .map(|file| fan::File {
                id: file.id.to_string(),
                name: file.name.clone(),
                image: None,
                removable: pinned && expanded > 0.98,
            })
            .collect(),
        width,
        height,
        expanded,
    };
    let handler = Rc::new(move |action, w: &mut Window, cx: &mut App| match action {
        fan::Action::Hover(hover) => handler(Action::FanHover(hover), w, cx),
        fan::Action::Toggle => handler(Action::ToggleFan, w, cx),
        fan::Action::Open(id) => {
            if pinned {
                if let Ok(id) = id.parse() {
                    handler(Action::OpenFile(id), w, cx);
                }
            } else {
                handler(Action::ToggleFan, w, cx);
            }
        }
        fan::Action::Remove(id) => {
            if let Ok(id) = id.parse() {
                handler(Action::RemoveFile(id), w, cx);
            }
        }
        fan::Action::Highlight(_, _) => {}
    });
    fan::render(
        ids,
        frame,
        "展开或固定附件预览".into(),
        "移除附件".into(),
        handler,
    )
    .absolute()
    .left(px(center as f32 - width / 2.))
    .top(px(body.top() as f32 - height))
    .into_any_element()
}

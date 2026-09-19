//! The approved composer layout rendered with the shared liquid material.
//! Hosts supply core capabilities and handle intents. Editor, hover, focus and
//! fan expansion are presentation state; this module contains no send policy.
use super::controls::ControlElement;
use super::{Pose, Surface, SurfaceColors};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        attachment_fan as fan_geometry, liquid_composer as spec, text_input::ComposerInput,
    },
    controls,
    design::{BRAND_ACCENT, ZORK_UI},
};
use gpui::{prelude::*, *};
use std::rc::Rc;
pub mod fan;
mod scene;
pub use scene::Scene;
pub use zork_client_types::composer::{Capabilities, Member, Snapshot};

pub const AVATAR: f32 = 22.4;
pub const PORTRAIT_INSET: f32 = 4.8;
pub const EDITOR_MIN: f32 = 24.;
pub const EDITOR_MAX: f32 = 60.;
pub const TEXT_INSET: f32 = 20.;
pub const BODY_BOTTOM: f64 = 244.;

#[derive(Clone)]
pub enum Action {
    Primary,
    FocusEditor,
    MemberAnchor(String, Bounds<Pixels>),
    ChooseFiles,
    FanHover(bool),
    ToggleFan,
    RemoveFile(u64),
    OpenFile(u64),
    Member(String),
    MemberHover(Option<String>),
}
pub type Handler = Rc<dyn Fn(Action, &mut Window, &mut App)>;

pub fn body(width: f32, editor_height: f32) -> Pose {
    let height =
        editor_height.clamp(EDITOR_MIN, EDITOR_MAX) + spec::TOP_EXTENSION + spec::COMPOSER_CHROME;
    Pose::rect(
        18.,
        BODY_BOTTOM - height as f64,
        (width - 36.).max(140.) as f64,
        height as f64,
        spec::SURFACE_RADIUS as f64,
    )
}
/// Keep member slots stable through idle/active transitions. Hidden slots lie
/// inside the plate; they never become a second membership source.
pub fn poses(body: Pose, state: &Snapshot, label_widths: &[f32]) -> Vec<Pose> {
    let mut poses = vec![body];
    let (mut active, mut idle) = (0, 0);
    for (i, member) in state.members.iter().enumerate() {
        let (x, lift, w) = if member.active {
            let lift = spec::RADIUS + spec::DOCK_GAP + active as f32 * spec::ROW_SPACING;
            active += 1;
            (
                0.,
                lift,
                (AVATAR + 8. + label_widths.get(i).copied().unwrap_or(0.) + 12. + PORTRAIT_INSET)
                    .min((body.w as f32 - 22.).max(32.)),
            )
        } else {
            let x = spec::EDGE + idle as f32 * spec::SPACING;
            idle += 1;
            (x, -spec::IMMERSION, 32.)
        };
        poses.push(Pose::rect(
            body.left() + x as f64,
            body.top() - lift as f64 - 16.,
            w as f64,
            32.,
            16.,
        ));
    }
    poses
}
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
    enabled: bool,
    stop: bool,
    busy: bool,
    handler: Handler,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    super::controls::action(
        id,
        "",
        24.,
        24.,
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
    pub member_groups: Vec<usize>,
    pub member_colors: Vec<u32>,
    pub member_names: Vec<String>,
    pub fan: Option<AnyElement>,
    pub busy: bool,
    pub editor_label: SharedString,
    pub attach_label: SharedString,
    pub primary_label: SharedString,
}

pub struct Props<'a> {
    pub id: &'a str,
    pub surface: &'a Surface,
    pub width: f32,
    pub height: f32,
    pub editor: &'a Entity<ComposerInput>,
    pub snapshot: &'a Snapshot,
    pub fan_progress: f32,
    pub fan_pinned: bool,
    pub bubbles: &'a [super::departure::Bubble<'a>],
    pub handler: Handler,
    pub presentation: Option<Presentation>,
}
/// Content positions follow the current material pose, including reversals.
/// The ordinary GPUI editor retains selection, IME, scrolling and key bindings.
pub fn render(props: Props<'_>, window: &mut Window, cx: &mut App) -> AnyElement {
    let Props {
        id,
        surface,
        width,
        height,
        editor,
        snapshot,
        fan_progress,
        fan_pinned,
        bubbles,
        handler,
        mut presentation,
    } = props;
    surface.set_border_width(spec::BORDER_WIDTH);
    let p = surface.simulation.pose();
    let c = snapshot.capabilities;
    let editor_height =
        (p.h as f32 - spec::TOP_EXTENSION - spec::COMPOSER_CHROME).clamp(0., EDITOR_MAX);
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
    .when(!c.editable, |v| v.child("此会话不可发送消息"));
    let focus_editor = handler.clone();
    let plate = positioned(p.left(), p.top(), p.w, p.h)
        .id(format!("{id}-surface"))
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_, w, cx| {
            if c.editable {
                focus_editor(Action::FocusEditor, w, cx);
            }
        });
    let mut content = div()
        .relative()
        .size_full()
        .child(
            surface
                .guard(plate)
                .automation(AutomationRole::Status, "消息输入区"),
        )
        .child(surface.guard(editor_view).automation_enabled(
            c.editable,
            AutomationRole::TextInput,
            presentation.as_ref().map_or_else(
                || SharedString::from("消息输入"),
                |p| p.editor_label.clone(),
            ),
        ));
    for (key, attach, x) in [
        ("attach", true, p.left() + 6.),
        ("send", false, p.left() + p.w - 30.),
    ] {
        content = content.child(
            positioned(x, p.top() + p.h - 30., 24., 24.).child(
                surface
                    .guard(action(
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
                        if attach { c.editable } else { c.enabled },
                        c.stop,
                        presentation.as_ref().is_some_and(|p| p.busy),
                        handler.clone(),
                        window,
                        cx,
                    ))
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
    for (i, member) in snapshot.members.iter().enumerate() {
        let mp = surface
            .simulation
            .group_pose(presentation.as_ref().map_or(i + 1, |p| p.member_groups[i]));
        let member_id = member.id.clone();
        let hover_id = member.id.clone();
        let hover_handler = handler.clone();
        let handler = handler.clone();
        let reveal = zork_liquid::recipes::member_reveal(mp.w) as f32;
        let measured = handler.clone();
        let measured_id = member.id.clone();
        let avatar = controls::quiet_button(
            format!("{id}-member-{}", member.id),
            "",
            true,
            controls::IconButtonSize::Standard,
        )
        .size(px(AVATAR))
        .p_0()
        .radius(8.)
        .control_overlay(
            canvas(
                move |bounds, w, cx| {
                    measured(Action::MemberAnchor(measured_id.clone(), bounds), w, cx)
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
            .into_any_element(),
        )
        .child(controls::agent_portrait(Some(&member.avatar), AVATAR))
        .on_hover(move |hovered, w, cx| {
            hover_handler(
                Action::MemberHover(hovered.then(|| hover_id.clone())),
                w,
                cx,
            )
        })
        .on_click(move |_, w, cx| handler(Action::Member(member_id.clone()), w, cx))
        .automation(
            AutomationRole::Button,
            format!(
                "{} · 活动记录",
                presentation
                    .as_ref()
                    .map_or(member.id.as_str(), |p| p.member_names[i].as_str())
            ),
        );
        content = content.child(
            positioned(
                mp.left() + PORTRAIT_INSET as f64,
                mp.cy - AVATAR as f64 / 2.,
                (mp.w - 2. * PORTRAIT_INSET as f64).max(AVATAR as f64),
                AVATAR as f64,
            )
            .flex()
            .items_center()
            .gap(px(8.))
            .overflow_hidden()
            .child(avatar)
            .when(member.active && mp.w > 32.5, |row| {
                row.child(
                    div()
                        .id(format!("{id}-activity-{}", member.id))
                        .flex_1()
                        .min_w_0()
                        .pr(px(12. - PORTRAIT_INSET))
                        .text_size(px(12.))
                        .line_height(px(20.))
                        .text_color(rgb(presentation
                            .as_ref()
                            .map_or(spec::TEXT_COLOR, |p| p.member_colors[i])))
                        .truncate()
                        .opacity(reveal)
                        .child(member.label.clone())
                        .automation(AutomationRole::Status, member.label.clone()),
                )
            }),
        );
    }
    for bubble in bubbles {
        if let Some(accent) = &bubble.accent {
            content = content.child(
                div()
                    .absolute()
                    .inset_0()
                    .size_full()
                    .opacity(bubble.accent_opacity)
                    .child(accent.background(BRAND_ACCENT, None))
                    .child(
                        positioned(bubble.pose.cx - 7., bubble.pose.cy - 7., 14., 14.)
                            .opacity(1. - bubble.opacity)
                            .child(
                                controls::icon("icons/arrow-up.svg", 14.).text_color(rgb(0xFFFFFF)),
                            ),
                    ),
            );
        }
        content = content.child(
            positioned(
                bubble.pose.left() + 12.,
                bubble.pose.top() + 8.,
                (bubble.pose.w - 24.).max(0.),
                (bubble.pose.h - 16.).max(0.),
            )
            .id(format!("{id}-departure-{}", bubble.id))
            .overflow_hidden()
            .opacity(bubble.opacity)
            .child(
                div()
                    .w(px(bubble.text_width))
                    .flex_shrink_0()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(rgb(spec::TEXT_COLOR))
                    .line_clamp(2)
                    .child(bubble.text.clone()),
            ),
        );
    }
    let custom_fan = presentation.as_mut().and_then(|p| p.fan.take());
    let has_custom_fan = custom_fan.is_some();
    div()
        .relative()
        .w(px(width))
        .h(px(height))
        .child(surface.layer(
            format!("{id}-material"),
            width,
            height,
            SurfaceColors::filled(spec::SURFACE_COLOR, ZORK_UI.palette.canvas),
            content,
        ))
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

pub fn opening(body: Pose, count: usize, expanded: f32) -> Option<fan_geometry::Opening> {
    (count > 0).then(|| {
        let available = (body.w as f32 - 24.).max(100.);
        let (width, _) = fan_geometry::dimensions(count.min(fan_geometry::MAX_FILES), available);
        let center = (body.left() + body.w - width as f64 / 2. - 12.).max(body.cx);
        fan_geometry::Opening::new(fan_geometry::Shape::for_count(count), expanded)
            .translated(point(center as f32, body.top() as f32))
    })
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
    let geometry = fan_geometry::poses(
        state.files.len().min(fan_geometry::MAX_FILES),
        expanded,
        available,
    );
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
            .zip(geometry)
            .map(|(file, pose)| fan::File {
                id: file.id.to_string(),
                name: file.name.clone(),
                pose,
                image: None,
                visible: true,
                departing: false,
                active: false,
                removable: pinned && expanded > 0.98,
            })
            .collect(),
        width,
        height,
        opening: fan_geometry::Opening::new(
            fan_geometry::Shape::for_count(state.files.len()),
            expanded,
        ),
        expanded,
        rim: true,
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

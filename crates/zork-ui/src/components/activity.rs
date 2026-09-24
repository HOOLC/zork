//! Session activity: the live "who is doing what" preview of a member's
//! Session. The host places it directly above the composer, at the composer's
//! width; it is a read-only projection, never a Chat message.
//!
//! Collapsed it is one 36 px capsule on the sunken fill: a persimmon dot while
//! running, the member, and the current step. Expanded, the same surface grows
//! to show the latest three steps and a way into the full history.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::{INTERACTION, ZORK_UI},
    motion,
};
use gpui::{div, prelude::*, px, rgb, AnimationExt, AnyElement, App, ElementId, Window};
use std::{rc::Rc, time::Instant};

/// Capsule height, and the height of the expanded surface's header row.
pub const HEIGHT: f32 = 36.;
/// Height of one step in the expanded list.
const STEP_HEIGHT: f32 = 26.;
/// Steps line up under the member name's leading edge.
const STEP_INSET: f32 = 44.;
const TEXT_SIZE: f32 = 12.5;
const CODE_SIZE: f32 = 12.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub icon: &'static str,
    /// Tool name, such as "写入文件".
    pub label: String,
    /// The object the tool works on, shown in monospace.
    pub summary: String,
    pub failed: bool,
    pub running: bool,
    /// Measured duration of a finished step.
    pub duration: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Running,
    /// The round ended; the host keeps it briefly, then lets it leave.
    Finished,
    /// The device is unreachable: keep the last reliable state, quietly.
    Disconnected,
}

#[derive(Clone, Default)]
pub struct Labels {
    pub expand: String,
    pub collapse: String,
    pub stop: String,
    pub history: String,
    /// Right-aligned marker of the step still running ("进行中").
    pub now: String,
}

pub struct SessionActivity {
    /// Display name of the member; never a raw id.
    pub name: String,
    pub phase: Phase,
    pub expanded: bool,
    /// Tool steps of the current round, oldest first.
    pub rows: Vec<SessionRow>,
    /// Live status while no record has arrived yet (and whether it failed).
    pub live: Option<(String, bool)>,
    /// Muted state in the expanded header ("执行中").
    pub status: String,
    pub labels: Labels,
}

/// A member's live status, used while its Session records have not arrived.
#[derive(Clone)]
pub struct Presentation {
    pub id: String,
    pub name: String,
    pub label: String,
    pub failed: bool,
    pub running: bool,
}

pub struct Actions {
    pub expand: Rc<dyn Fn(&mut App)>,
    /// Opens the member's history, at the given entry when there is one.
    pub open: Rc<dyn Fn(Option<String>, &mut App)>,
    /// Present while the member can be stopped.
    pub stop: Option<Rc<dyn Fn(&mut App)>>,
}

#[derive(Clone, PartialEq)]
enum Line {
    Step(SessionRow),
    /// Live status before any record arrives; `true` when it failed.
    Live(String, bool),
    /// The expanded header's muted state.
    State(String),
    Empty,
}

/// Renders the activity surface. Returns whether a transition that changes
/// its height is still running, so a host with a cached layout can remeasure.
pub fn render_session(
    data: &SessionActivity,
    actions: Actions,
    window: &mut Window,
    cx: &mut App,
) -> (AnyElement, bool) {
    let p = ZORK_UI.palette;
    let running = data.phase == Phase::Running;
    let offline = data.phase == Phase::Disconnected;
    let latest = data.rows.last().cloned();
    let expandable = !data.rows.is_empty();
    let expanded = data.expanded && expandable;
    let (turn, turning) = tween("session-activity-turn", expanded, window, cx);

    let line = if expanded {
        Line::State(data.status.clone())
    } else if let Some(row) = latest.clone() {
        Line::Step(row)
    } else if let Some((text, failed)) = data.live.clone() {
        Line::Live(text, failed)
    } else {
        Line::Empty
    };
    let dim = data.phase != Phase::Running;
    let open = actions.open.clone();
    let slot = crossfade(
        "session-activity-step",
        line,
        move |line| match line {
            Line::Step(row) => {
                let id = row.id.clone();
                let open = open.clone();
                div()
                    .id(format!("session-activity-{}", row.id))
                    .min_w_0()
                    .max_w_full()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| open(Some(id.clone()), cx))
                    .child(step(row, if dim { p.subtle } else { p.muted }, true))
                    .automation(
                        AutomationRole::Button,
                        format!("{} {}", row.label, row.summary),
                    )
                    .into_any_element()
            }
            Line::Live(text, failed) => div()
                .id("participant-activity")
                .min_w_0()
                .truncate()
                .text_color(rgb(if *failed { p.danger } else { p.subtle }))
                .child(text.clone())
                .automation(AutomationRole::Status, text.clone())
                .into_any_element(),
            Line::State(text) => div()
                .id("session-activity-state")
                .min_w_0()
                .truncate()
                .text_size(px(CODE_SIZE))
                .text_color(rgb(p.subtle))
                .child(text.clone())
                .into_any_element(),
            Line::Empty => div().into_any_element(),
        },
        window,
        cx,
    );

    let member_open = actions.open.clone();
    let expand = actions.expand.clone();
    let toggle_label = if expanded {
        data.labels.collapse.clone()
    } else {
        data.labels.expand.clone()
    };
    let header = div()
        .h(px(HEIGHT))
        .flex_shrink_0()
        .pl(px(12.))
        .pr(px(4.))
        .flex()
        .items_center()
        .gap(px(8.))
        .when(running || offline, |v| v.child(dot(running)))
        .child(
            div()
                .id("session-activity-member")
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .on_click(move |_, _, cx| member_open(None, cx))
                .child(crate::device_name::mark(&data.name, 18.))
                .child(
                    div()
                        .text_size(px(TEXT_SIZE))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(rgb(if offline { p.muted } else { p.text }))
                        .child(data.name.clone()),
                )
                .automation(AutomationRole::Button, data.name.clone()),
        )
        .child(slot)
        .when(expandable, |v| {
            v.child(
                crate::controls::icon_button_sized(
                    "session-activity-expand",
                    true,
                    crate::controls::IconButtonSize::Compact,
                )
                .aria_label(toggle_label.clone())
                .child(
                    crate::controls::icon("icons/chevron-down.svg", 14.).with_transformation(
                        gpui::Transformation::rotate(gpui::radians(std::f32::consts::PI * turn)),
                    ),
                )
                .on_click(move |_, _, cx| expand(cx))
                .automation(AutomationRole::Button, toggle_label),
            )
        })
        // Stop is always there while running; an unreachable device cannot
        // be stopped, so it keeps only the still dot.
        .when(running, |v| {
            let stop = actions.stop.clone();
            v.child(
                crate::controls::icon_button_sized(
                    "session-activity-stop",
                    stop.is_some(),
                    crate::controls::IconButtonSize::Compact,
                )
                .aria_label(data.labels.stop.clone())
                .child(crate::controls::icon("icons/stop.svg", 14.))
                .when_some(stop.clone(), |b, stop| b.on_click(move |_, _, cx| stop(cx)))
                .automation_enabled(
                    stop.is_some(),
                    AutomationRole::Button,
                    data.labels.stop.clone(),
                ),
            )
        });

    let recent: Vec<SessionRow> = data.rows.iter().rev().take(3).rev().cloned().collect();
    let open = actions.open.clone();
    let history = data.labels.history.clone();
    let now = data.labels.now.clone();
    let stopped = !running;
    let body = motion::collapse(
        "session-activity-steps",
        expanded,
        move || {
            let count = recent.len();
            div()
                .pl(px(STEP_INSET))
                .pr(px(12.))
                .pb(px(6.))
                .flex()
                .flex_col()
                .children(recent.into_iter().enumerate().map(|(index, row)| {
                    let current = index + 1 == count && row.running && !stopped;
                    let color = if current { p.text } else { p.subtle };
                    let id = row.id.clone();
                    let open = open.clone();
                    let accessible = format!("{} {}", row.label, row.summary);
                    let trailing = if current {
                        Some(now.clone())
                    } else {
                        row.duration.clone()
                    };
                    div()
                        .id(format!("session-activity-{}", row.id))
                        .h(px(STEP_HEIGHT))
                        .flex_shrink_0()
                        .min_w_0()
                        .ml(px(-8.))
                        .px(px(8.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .hover(|v| v.bg(rgb(INTERACTION.neutral_hover)))
                        .on_click(move |_, _, cx| open(Some(id.clone()), cx))
                        .child(div().flex_1().min_w_0().child(step(&row, color, current)))
                        .when_some(trailing, |v, text| {
                            v.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(if current { CODE_SIZE } else { TEXT_SIZE }))
                                    .text_color(rgb(p.subtle))
                                    .child(text),
                            )
                        })
                        .automation(AutomationRole::Button, accessible)
                }))
                .child({
                    let open = open.clone();
                    // A quiet ghost action: secondary ink, no fill until hovered.
                    div()
                        .id("session-activity-history")
                        .tab_stop(true)
                        .self_start()
                        .h(px(28.))
                        .ml(px(-12.))
                        .mt(px(4.))
                        .pl(px(12.))
                        .pr(px(9.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(rgb(p.muted))
                        .cursor_pointer()
                        .hover(|v| v.bg(rgb(INTERACTION.neutral_hover)).text_color(rgb(p.text)))
                        .focus_visible(|v| v.shadow(crate::controls::focus_ring()))
                        .child(history.clone())
                        .child(crate::controls::icon("icons/arrow-right.svg", 14.))
                        .on_click(move |_, _, cx| open(None, cx))
                        .automation(AutomationRole::Button, history.clone())
                })
                .into_any_element()
        },
        window,
        cx,
    );
    let open_surface = expanded || body.is_some();
    let view = div()
        .id("session-activity")
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(if open_surface { 20. } else { HEIGHT / 2. }))
        .bg(rgb(p.prompt))
        .text_size(px(TEXT_SIZE))
        .child(header)
        .children(body)
        .automation(AutomationRole::Status, data.name.clone());
    (view.into_any_element(), turning)
}

/// One step: tool icon, tool name and the object in monospace.
fn step(row: &SessionRow, color: u32, strong: bool) -> impl IntoElement {
    let p = ZORK_UI.palette;
    let color = if row.failed { p.danger } else { color };
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(TEXT_SIZE))
        .child(
            crate::controls::icon(row.icon, 14.)
                .text_color(rgb(color))
                .flex_shrink_0(),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_color(rgb(color))
                .child(row.label.clone()),
        )
        .when(!row.summary.is_empty(), |v| {
            v.child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(crate::assets::CODE_FONT_FAMILY)
                    .text_size(px(CODE_SIZE))
                    .text_color(rgb(if strong && !row.failed && color == p.text {
                        p.text
                    } else {
                        p.subtle
                    }))
                    .child(row.summary.clone()),
            )
        })
}

/// The working dot: persimmon and breathing while running (static with
/// reduced motion); a still, muted dot while the device is unreachable.
fn dot(running: bool) -> AnyElement {
    let dot = div().size(px(8.)).rounded_full().flex_shrink_0();
    if running {
        dot.bg(rgb(INTERACTION.accent))
            .with_animation("session-activity-pulse", motion::pulse(), |dot, value| {
                dot.opacity(value)
            })
            .into_any_element()
    } else {
        dot.bg(rgb(ZORK_UI.palette.subtle)).into_any_element()
    }
}

struct Fade {
    current: Option<Line>,
    previous: Option<Line>,
    start: Option<Instant>,
}

/// The current step slot. A changed step crossfades over the fast token while
/// the old one fades out in place; the slot keeps its size, so nothing moves.
fn crossfade(
    key: &'static str,
    line: Line,
    render: impl Fn(&Line) -> AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let state = window.use_keyed_state(ElementId::Name(key.into()), cx, |_, _| Fade {
        current: None,
        previous: None,
        start: None,
    });
    let now = cx.background_executor().now();
    let animate = motion::mode(cx) != motion::Mode::Static;
    let total = motion::duration(motion::FAST).as_secs_f32().max(0.001);
    let (previous, t) = state.update(cx, |s, _| {
        if s.current.as_ref() != Some(&line) {
            if animate && s.current.is_some() {
                s.previous = s.current.take();
                s.start = Some(now);
            }
            s.current = Some(line.clone());
        }
        let t = s.start.map_or(1., |start| {
            (now.saturating_duration_since(start).as_secs_f32() / total).clamp(0., 1.)
        });
        if t >= 1. {
            s.previous = None;
            s.start = None;
        }
        (s.previous.clone(), t)
    });
    if previous.is_some() {
        window.request_animation_frame();
    }
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .child(
            div()
                .min_w_0()
                .max_w_full()
                .flex()
                .items_center()
                .opacity(t)
                .child(render(&line)),
        )
        .when_some(previous, |v, previous| {
            v.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .opacity(1. - t)
                    .child(render(&previous)),
            )
        })
        .into_any_element()
}

struct Tween {
    on: bool,
    from: f32,
    value: f32,
    start: Option<Instant>,
}

/// A 0 → 1 value that follows `on` over the base token on the move curve and
/// reverses from wherever it is. Returns the value and whether it is moving.
fn tween(key: &'static str, on: bool, window: &mut Window, cx: &mut App) -> (f32, bool) {
    let state = window.use_keyed_state(ElementId::Name(key.into()), cx, |_, _| Tween {
        on,
        from: 0.,
        value: if on { 1. } else { 0. },
        start: None,
    });
    let now = cx.background_executor().now();
    let mode = motion::mode(cx);
    let total = motion::duration(motion::BASE).as_secs_f32().max(0.001);
    let (value, moving) = state.update(cx, |s, _| {
        if s.on != on {
            s.on = on;
            s.from = s.value;
            s.start = Some(now);
        }
        let target = if on { 1. } else { 0. };
        let Some(start) = s.start.filter(|_| mode == motion::Mode::Full) else {
            s.start = None;
            s.value = target;
            return (target, false);
        };
        let t = (now.saturating_duration_since(start).as_secs_f32() / total).clamp(0., 1.);
        s.value = s.from + (target - s.from) * motion::bezier(0.3, 0., 0.2, 1.)(t);
        if t >= 1. {
            s.start = None;
            s.value = target;
        }
        (s.value, t < 1.)
    });
    if moving {
        window.request_animation_frame();
    }
    (value, moving)
}

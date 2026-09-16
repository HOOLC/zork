//! Complete liquid popovers and dialogs, shared by examples and application chrome.
//! The component owns the live material, exit lifetime, input and focus handling.
use super::presentation::{FramePaint, Presentation, Recipe, Target};
use super::{controls, Material, Pose, Surface};
use controls::ControlElement;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::{CUE_UI, LIQUID_OUTLINE},
};
use gpui::{prelude::*, *};
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[cfg(target_family = "wasm")]
use web_time::Instant;

mod anchor;
mod source;

pub(super) use anchor::MeasuredAnchor;
pub use source::{BoundTrigger, SourceBinding};

pub use super::motion::FrameSample;
pub(super) use super::motion::Motion;

fn placed(p: Pose) -> Div {
    div()
        .absolute()
        .left(px(p.left() as f32))
        .top(px(p.top() as f32))
        .w(px(p.w as f32))
        .h(px(p.h as f32))
}
pub(super) fn reveal(progress: f64) -> f32 {
    zork_liquid::recipes::reveal(progress)
}
fn pose(bounds: Bounds<Pixels>, radius: f64) -> Pose {
    Pose::rect(
        bounds.origin.x.as_f32() as f64,
        bounds.origin.y.as_f32() as f64,
        bounds.size.width.as_f32().max(2.) as f64,
        bounds.size.height.as_f32().max(2.) as f64,
        radius,
    )
}

/// Inline stories supply layout dimensions; window popovers measure their trigger.
/// Both placements run the same component, including the source/child material.
pub enum Placement {
    Inline {
        width: f32,
        height: f32,
        source: Pose,
        target: Pose,
    },
    Window {
        width: f32,
    },
}
pub struct Choice {
    pub id: String,
    pub label: SharedString,
    pub checked: Option<bool>,
    pub disabled: bool,
}
#[derive(Clone, Copy, PartialEq)]
pub enum Selection {
    Single,
    Multiple,
    Actions,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Trigger {
    Field,
    Button,
    Icon,
}

pub struct Popover {
    motion: Motion,
    source_material: super::render::SourceMaterial,
    option_hover: Motion,
    hot_option: Rc<Cell<Option<usize>>>,
    last_option: Option<usize>,
    anchor: MeasuredAnchor,
    hover: Rc<Cell<bool>>,
    held: Rc<Cell<bool>>,
    focus: FocusHandle,
    trigger_measure: Option<(SharedString, Font, Trigger, f32)>,
    menu_measure: Option<(Vec<SharedString>, Font, bool, f32)>,
    typeahead: Rc<RefCell<(String, Option<Instant>)>>,
}
impl Popover {
    pub fn new(cx: &mut App) -> Self {
        let motion = Motion::default();
        let option_hover = Motion {
            scheduled: motion.scheduled.clone(),
            ..Motion::persistent()
        };
        Self {
            motion,
            source_material: Default::default(),
            option_hover,
            hot_option: Default::default(),
            last_option: None,
            anchor: Default::default(),
            hover: Default::default(),
            held: Default::default(),
            focus: cx.focus_handle().tab_stop(true),
            trigger_measure: None,
            menu_measure: None,
            typeahead: Default::default(),
        }
    }
    pub fn trigger_width(&mut self, label: &str, trigger: Trigger, window: &mut Window) -> f32 {
        if trigger == Trigger::Icon {
            return 24.;
        }
        let font = window.text_style().font();
        if let Some((text, prior_font, prior_trigger, width)) = &self.trigger_measure {
            if text.as_ref() == label && *prior_font == font && *prior_trigger == trigger {
                return *width;
            }
        }
        let width = measure_label(label, 13., window)
            + 24.
            + if trigger == Trigger::Field { 20. } else { 0. };
        self.trigger_measure = Some((label.to_owned().into(), font, trigger, width.ceil()));
        width.ceil()
    }
    fn menu_width(&mut self, choices: &[Choice], window: &mut Window) -> f32 {
        let font = window.text_style().font();
        let checked = choices.iter().any(|item| item.checked.is_some());
        if let Some((labels, prior_font, prior_checked, width)) = &self.menu_measure {
            if *prior_font == font
                && *prior_checked == checked
                && labels.len() == choices.len()
                && labels
                    .iter()
                    .zip(choices)
                    .all(|(label, item)| *label == item.label)
            {
                return *width;
            }
        }
        let width = (choices
            .iter()
            .map(|item| measure_label(&item.label, 12., window))
            .fold(0_f32, f32::max)
            + 2. * (controls::MENU_INSET + controls::MENU_ROW_PADDING)
            + if checked { 22. } else { 0. })
        .max(controls::MENU_MIN_WIDTH);
        self.menu_measure = Some((
            choices.iter().map(|item| item.label.clone()).collect(),
            font,
            checked,
            width.ceil(),
        ));
        width.ceil()
    }
    pub fn inspect(&self) -> serde_json::Value {
        let mut value = self.motion.inspect();
        if let Some(fields) = value.as_object_mut() {
            fields.insert(
                "anchor".into(),
                serde_json::json!(pose(self.anchor.bounds.get(), 12.)),
            );
            fields.insert("hover".into(), self.option_hover.inspect());
            fields.insert(
                "hoveredOption".into(),
                serde_json::json!(self.hot_option.get()),
            );
        }
        value
    }
    pub fn pose(&self) -> Option<Pose> {
        self.motion.surface.as_ref().map(|s| s.simulation.pose())
    }
    pub fn hovered(&self) -> bool {
        self.hover.get()
    }
    pub fn held(&self) -> bool {
        self.held.get()
    }
    pub fn samples(&self) -> &[FrameSample] {
        &self.motion.samples
    }
    pub fn reset_samples(&mut self) {
        self.motion.samples.clear();
    }
    pub fn visible(&self) -> bool {
        self.anchor.visible.get()
    }
    fn window_slot<V: 'static>(
        &self,
        width: f32,
        children: Vec<AnyElement>,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let anchor = self.anchor.clone();
        div()
            .relative()
            .w(px(width))
            .h(px(32.))
            .child(anchor.measure(self.motion.alive(), cx))
            .children(children)
            .into_any_element()
    }
    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        choices: Vec<Choice>,
        selection: Selection,
        trigger_style: Trigger,
        open: bool,
        enabled: bool,
        placement: Placement,
        material: Material,
        window: &mut Window,
        cx: &mut Context<V>,
        set_open: impl Fn(&mut V, bool, &mut Window, &mut Context<V>) + 'static,
        choose: impl Fn(&mut V, usize, &mut Window, &mut Context<V>) + 'static,
    ) -> AnyElement {
        self.render_with_icons(
            id,
            label,
            choices,
            selection,
            trigger_style,
            open,
            enabled,
            placement,
            material,
            None,
            vec![],
            window,
            cx,
            set_open,
            choose,
        )
    }
    /// Provider images use the same source, keyboard model and retained material.
    pub fn render_with_icons<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        choices: Vec<Choice>,
        selection: Selection,
        trigger_style: Trigger,
        open: bool,
        enabled: bool,
        placement: Placement,
        material: Material,
        leading: Option<&'static str>,
        option_icons: Vec<Option<&'static str>>,
        window: &mut Window,
        cx: &mut Context<V>,
        set_open: impl Fn(&mut V, bool, &mut Window, &mut Context<V>) + 'static,
        choose: impl Fn(&mut V, usize, &mut Window, &mut Context<V>) + 'static,
    ) -> AnyElement {
        let id = id.into();
        let label = label.into();
        let floating = matches!(placement, Placement::Window { .. });
        let menu_width = self.menu_width(&choices, window)
            + if option_icons.iter().any(Option::is_some) {
                22.
            } else {
                0.
            };
        let count = choices.len();
        let inset = controls::MENU_INSET as f64;
        let row_height = controls::MENU_ROW_HEIGHT as f64;
        let row_gap = controls::MENU_ROW_GAP as f64;
        let menu_height =
            2. * inset + count as f64 * row_height + count.saturating_sub(1) as f64 * row_gap;
        let selected = choices
            .iter()
            .position(|c| c.checked == Some(true))
            .unwrap_or(0);
        let viewport = window.viewport_size();
        if let Placement::Window { width } = &placement {
            if self.anchor.bounds.get().size.width <= px(0.)
                || viewport.width <= px(24.)
                || viewport.height <= px(88.)
            {
                return self.window_slot(*width, vec![], cx);
            }
        }
        let offset = if floating {
            self.anchor.bounds.get().origin
        } else {
            point(px(0.), px(0.))
        };

        let (width, height, source, target) = match placement {
            Placement::Inline {
                width,
                height,
                source,
                target,
            } => {
                let w = menu_width.max(source.w as f32).min(target.w as f32) as f64;
                let h = target.h.min(menu_height);
                // Fitting content must preserve the caller's aligned edge.
                let x = if (source.left() + source.w - target.left() - target.w).abs() < 0.1 {
                    target.left() + target.w - w
                } else {
                    target.left()
                };
                let y = if target.top() + target.h <= source.top() {
                    target.top() + target.h - h
                } else {
                    target.top()
                };
                (width, height, source, Pose::rect(x, y, w, h, target.r))
            }
            Placement::Window { width } => {
                let mut source = pose(self.anchor.bounds.get(), 12.);
                source.w = width as f64;
                source.cx = self.anchor.bounds.get().origin.x.as_f32() as f64 + width as f64 / 2.;
                let w = menu_width
                    .max(width)
                    .min(viewport.width.as_f32() - 24.)
                    .max(2.) as f64;
                let h = menu_height
                    .min(238.)
                    .min(viewport.height.as_f32() as f64 - 88.)
                    .max(2.);
                let fitted = self.anchor.fit(
                    self.anchor.bounds.get(),
                    size(px(w as f32), px(h as f32)),
                    viewport,
                    8.,
                );
                let x = fitted.bounds.origin.x.as_f32() as f64;
                let y = fitted.bounds.origin.y.as_f32() as f64;
                let h = fitted.bounds.size.height.as_f32() as f64;
                (
                    viewport.width.as_f32(),
                    viewport.height.as_f32(),
                    Pose::rect(0., 0., source.w, source.h, source.r),
                    Pose::rect(
                        x - offset.x.as_f32() as f64,
                        y - offset.y.as_f32() as f64,
                        w,
                        h,
                        crate::controls::MENU_RADIUS as f64,
                    ),
                )
            }
        };
        if !open && !self.motion.alive() {
            self.anchor.reset_placement();
        }
        let trigger_pose = source;
        let source = if !floating && self.held.get() && self.hover.get() {
            Pose::rect(
                source.left() + 1.,
                source.top() + 2.,
                (source.w - 2.).max(2.),
                (source.h - 3.).max(2.),
                source.r,
            )
        } else {
            source
        };
        self.motion.frame(
            source,
            target,
            true,
            open && enabled,
            material,
            self.anchor.bounds.get().size.width == px(0.) || self.anchor.visible.get(),
            window,
            cx,
        );
        if !open || !enabled || self.hot_option.get().is_some_and(|i| i >= count) {
            self.hot_option.set(None);
        }
        if let Some(index) = self.hot_option.get() {
            self.last_option = Some(index);
        }
        let row_radius = controls::menu_row_radius(
            self.motion.surface.as_ref().unwrap().simulation.pose().r,
            (target.w - 2. * inset).max(2.),
        );
        let hover_target = Pose::rect(
            0.,
            self.last_option
                .unwrap_or(selected)
                .min(count.saturating_sub(1)) as f64
                * (row_height + row_gap),
            (target.w - 2. * inset).max(2.),
            row_height,
            row_radius,
        );
        self.option_hover.frame(
            hover_target,
            hover_target,
            false,
            false,
            material,
            (!floating || self.anchor.visible.get())
                && (open || reveal(self.motion.progress()) > 0.),
            window,
            cx,
        );
        let surface = self.motion.surface.as_ref().unwrap();
        let body_part = if floating && (open || self.motion.alive()) {
            Some(self.source_material.bind(surface, trigger_pose, None))
        } else {
            self.source_material.clear();
            None
        };
        let content_clip = surface.content_clip();
        let current = surface.simulation.pose();
        let carrier = surface.simulation.source_pose();
        let focus = self.focus.clone().tab_stop(enabled);
        let option_focus: Vec<_> = choices
            .iter()
            .map(|c| controls::action_focus(c.id.clone(), window, cx))
            .collect();
        let active: Vec<_> = choices
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.disabled)
            .map(|(i, _)| i)
            .collect();
        let key_active = active.clone();
        let option_labels: Vec<_> = choices.iter().map(|c| c.label.to_lowercase()).collect();
        let typeahead = self.typeahead.clone();
        if !open {
            *self.typeahead.borrow_mut() = Default::default();
        }
        let set_open = Rc::new(set_open);
        let choose = Rc::new(choose);
        let trigger_toggle = set_open.clone();
        let trigger_focus = focus.clone();
        let key_open = set_open.clone();
        let key_handles = option_focus.clone();
        let hover = self.hover.clone();
        let held = self.held.clone();
        let release = self.held.clone();
        let release_out = self.held.clone();
        let cancelled_press = self.held.clone();
        let hit_pose = if floating {
            Pose::rect(0., 0., trigger_pose.w, trigger_pose.h, trigger_pose.r)
        } else {
            trigger_pose
        };
        let p = CUE_UI.palette;
        let focused = enabled && focus.is_focused(window) && window.last_input_was_keyboard();
        let trigger = if floating {
            let face = popover_face(
                label.clone(),
                trigger_style,
                self.motion.progress(),
                leading,
            );
            super::press::content_surface_with_source(
                id.clone().into(),
                trigger_pose.w as f32,
                trigger_pose.h as f32,
                trigger_pose.r as f32,
                material.smoothing,
                super::SurfaceColors {
                    fill: p.canvas,
                    border: (trigger_style != Trigger::Icon || self.hover.get() || open || focused)
                        .then_some(if focused {
                            crate::design::INTERACTION.focus_border
                        } else if self.hover.get() {
                            crate::controls::FIELD_HOVER_BORDER
                        } else {
                            LIQUID_OUTLINE
                        }),
                    parent: p.canvas,
                    focused,
                },
                true,
                move |clip, _, _, _| clip.unwrap().content(face, 22.).into_any_element(),
                enabled,
                Some(self.source_material.clone()),
                false,
                window,
                cx,
            )
        } else {
            placed(hit_pose).id(id.clone())
        }
        .track_focus(&focus)
        .tab_stop(enabled)
        .when(enabled, |v| v.cursor_pointer())
        .on_hover(cx.listener(move |_, value, _, cx| {
            hover.set(*value);
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |_, _, _, cx| {
                if enabled {
                    held.set(true);
                    cx.notify();
                }
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |_, _, _, cx| {
                release.set(false);
                cx.notify();
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(move |_, _, _, cx| {
                release_out.set(false);
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(move |_, event: &MouseMoveEvent, _, cx| {
            if event.pressed_button != Some(MouseButton::Left) && cancelled_press.replace(false) {
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |v, _, w, cx| {
            if enabled {
                window_focus(w, &trigger_focus, cx);
                trigger_toggle(v, !open, w, cx);
            }
        }))
        .on_key_down(cx.listener(move |v, e: &KeyDownEvent, w, cx| {
            if enabled
                && !key_active.is_empty()
                && matches!(e.keystroke.key.as_str(), "down" | "up")
            {
                key_open(v, true, w, cx);
                let index = if e.keystroke.key == "up" {
                    *key_active.last().unwrap()
                } else {
                    if key_active.contains(&selected) {
                        selected
                    } else {
                        key_active[0]
                    }
                };
                let focus = key_handles[index].clone();
                w.on_next_frame(move |w, cx| w.focus(&focus, cx));
                cx.stop_propagation();
            } else if open && e.keystroke.key == "escape" {
                key_open(v, false, w, cx);
                cx.stop_propagation();
            }
        }));
        let mut stage = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .text_color(rgb(if enabled { p.text } else { p.subtle }));
        // Transparent exterior lets the same liquid surface cross arbitrary page
        // content. Children stay inside the current material's inset rectangle.
        let separated = (current.cx - carrier.cx).hypot(current.cy - carrier.cy) > 1.
            || (current.w - carrier.w).abs() > 1.
            || (current.h - carrier.h).abs() > 1.;
        let visible_material =
            trigger_style != Trigger::Icon || self.hover.get() || open || separated || focused;
        let background = div()
            .absolute()
            .inset_0()
            .child(if let Some(part) = &body_part {
                part.background(
                    Some(p.canvas),
                    Some(LIQUID_OUTLINE),
                    point(px(0.), px(0.)),
                    false,
                )
            } else {
                surface
                    .background_colors(
                        (floating || open || separated).then_some(p.canvas),
                        Some(if focused {
                            crate::design::INTERACTION.focus_border
                        } else if self.hover.get() {
                            crate::controls::FIELD_HOVER_BORDER
                        } else {
                            LIQUID_OUTLINE
                        }),
                        point(px(0.), px(0.)),
                        false,
                    )
                    .into_any_element()
            })
            .child(surface.background_colors(None, None, point(px(0.), px(0.)), false))
            .with_spring(
                format!("{id}-reveal"),
                crate::components::motion::spring(if visible_material { 1. } else { 0. }),
                |v, a| v.opacity(a),
            );
        let mut trigger = Some(
            trigger
                .map(|trigger| {
                    if floating {
                        trigger
                    } else {
                        surface.guard(trigger)
                    }
                })
                .automation_enabled(enabled, AutomationRole::Button, label.to_string())
                .into_any_element(),
        );
        stage = stage.when(!floating, |stage| {
            stage.child(
                placed(carrier).child(
                    content_clip.content(
                        popover_face(
                            label.clone(),
                            trigger_style,
                            self.motion.progress(),
                            leading,
                        )
                        .w(px(trigger_pose.w as f32))
                        .h(px(trigger_pose.h as f32)),
                        22.,
                    ),
                ),
            )
        });
        if !floating {
            stage = stage.child(trigger.take().unwrap());
        }
        if open || reveal(self.motion.progress()) > 0. {
            let return_focus = focus.clone();
            let key_close = set_open.clone();
            let handles = option_focus;
            let mut rows = div()
                .id(format!("{id}-scroll"))
                .flex()
                .flex_col()
                .relative()
                .gap(px(row_gap as f32))
                .w(px((target.w - 2. * inset) as f32))
                .max_h(px((current.h - 2. * inset).max(2.) as f32))
                .overflow_y_scroll();
            rows = rows.child(
                placed(
                    self.option_hover
                        .surface
                        .as_ref()
                        .unwrap()
                        .simulation
                        .pose(),
                )
                .child(content_clip.fill(
                    format!("{id}-sliding-hover"),
                    row_radius,
                    Some(crate::design::INTERACTION.neutral_hover),
                    None,
                    window,
                    cx,
                ))
                .with_spring(
                    format!("{id}-hover-visible"),
                    crate::components::motion::spring(if self.hot_option.get().is_some() {
                        1.
                    } else {
                        0.
                    }),
                    |v, alpha| v.opacity(alpha),
                ),
            );
            for (i, item) in choices.into_iter().enumerate() {
                let choose = choose.clone();
                let close = set_open.clone();
                let focus = focus.clone();
                let hot = self.hot_option.clone();
                let item_enabled = enabled && open && !item.disabled;
                rows = rows.child(
                    surface
                        .guard(controls::menu_item_with_icon(
                            item.id,
                            item.label.clone(),
                            (target.w - 2. * inset) as f32,
                            item.checked,
                            item_enabled,
                            option_icons.get(i).copied().flatten(),
                            controls::MenuSurface::Grouped {
                                clip: content_clip.clone(),
                                radius: row_radius,
                            },
                            window,
                            cx,
                        ))
                        .track_focus(&handles[i].clone().tab_stop(false))
                        .tab_stop(false)
                        .role(if selection == Selection::Actions {
                            Role::MenuItem
                        } else {
                            Role::ListBoxOption
                        })
                        .when_some(item.checked, |v, checked| v.aria_selected(checked))
                        .on_hover(cx.listener(move |_, inside, _, cx| {
                            if item_enabled && *inside {
                                hot.set(Some(i));
                                cx.notify();
                            } else if hot.get() == Some(i) {
                                hot.set(None);
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |v, _, w, cx| {
                            if item_enabled {
                                choose(v, i, w, cx);
                                if selection != Selection::Multiple {
                                    close(v, false, w, cx);
                                    w.focus(&focus, cx);
                                }
                            }
                        }))
                        .automation_enabled(
                            item_enabled,
                            if selection == Selection::Actions {
                                AutomationRole::Button
                            } else {
                                AutomationRole::Option
                            },
                            item.label.to_string(),
                        ),
                );
            }
            let origin = self.anchor.bounds.clone();
            let outside_close = set_open.clone();
            let outside_focus = focus.clone();
            let panel = placed(Pose::rect(
                current.left(),
                current.top(),
                current.w,
                current.h,
                current.r,
            ))
            .id(format!("{id}-menu"))
            .role(if selection == Selection::Actions {
                Role::Menu
            } else {
                Role::ListBox
            })
            .aria_label(label.clone())
            .when(open && self.motion.progress() > 0.45, |v| v.occlude())
            .opacity(reveal(self.motion.progress()))
            .child(
                div()
                    .absolute()
                    .left(px(inset as f32))
                    .top(px(inset as f32))
                    .w(px((target.w - 2. * inset) as f32))
                    .child(rows),
            )
            .on_mouse_down_out(cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                let origin = origin.get().origin;
                let point = e.position - origin;
                let in_source = point.x.as_f32() as f64 >= source.left()
                    && (point.x.as_f32() as f64) <= source.left() + source.w
                    && point.y.as_f32() as f64 >= source.top()
                    && (point.y.as_f32() as f64) <= source.top() + source.h;
                if open && !in_source {
                    outside_close(v, false, w, cx);
                    w.focus(&outside_focus, cx);
                }
            }))
            .on_key_down(cx.listener(move |v, e: &KeyDownEvent, w, cx| {
                if e.keystroke.key == "escape" {
                    key_close(v, false, w, cx);
                    w.focus(&return_focus, cx);
                    cx.stop_propagation();
                    return;
                }
                if e.keystroke.key == "tab" {
                    key_close(v, false, w, cx);
                    w.focus(&return_focus, cx);
                    if e.keystroke.modifiers.shift {
                        w.focus_prev(cx);
                    } else {
                        w.focus_next(cx);
                    }
                    w.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if active.is_empty() {
                    return;
                }
                let current = active
                    .iter()
                    .position(|i| handles[*i].is_focused(w))
                    .unwrap_or(0);
                let next = match e.keystroke.key.as_str() {
                    "down" => Some((current + 1) % active.len()),
                    "up" => Some((current + active.len() - 1) % active.len()),
                    "home" => Some(0),
                    "end" => Some(active.len() - 1),
                    _ => None,
                };
                if let Some(next) = next {
                    w.focus(&handles[active[next]], cx);
                    w.prevent_default();
                    cx.stop_propagation();
                } else if let Some(text) = e.keystroke.key_char.as_deref().filter(|s| {
                    !s.is_empty()
                        && !e.keystroke.modifiers.control
                        && !e.keystroke.modifiers.platform
                        && *s != " "
                }) {
                    let mut state = typeahead.borrow_mut();
                    if state.1.is_none_or(|at| at.elapsed().as_millis() > 700) {
                        state.0.clear();
                    }
                    state.1 = Some(Instant::now());
                    state.0.push_str(&text.to_lowercase());
                    let repeated = state.0.chars().all(|c| Some(c) == state.0.chars().next());
                    let query = if repeated {
                        text.to_lowercase()
                    } else {
                        state.0.clone()
                    };
                    if let Some(index) = (1..=active.len())
                        .map(|step| active[(current + step) % active.len()])
                        .find(|i| option_labels[*i].starts_with(&query))
                    {
                        w.focus(&handles[index], cx);
                    }
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .automation_enabled(open && enabled, AutomationRole::Status, "下拉菜单");
            stage = stage.child(panel);
        }
        let stage = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .child(background)
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .w(px(width))
                    .h(px(height))
                    .child(stage),
            );
        let bounds = self.anchor.bounds.clone();
        let visibility = self.anchor.visible.clone();
        let owner = cx.entity().downgrade();
        if floating {
            let anchor = self.anchor.clone();
            let mut children = vec![trigger.take().unwrap()];
            if open || self.motion.alive() {
                children.push(anchor.layer(stage, 200));
            }
            self.window_slot(trigger_pose.w as f32, children, cx)
        } else {
            stage
                .child(
                    canvas(
                        move |b, window, cx| {
                            bounds.set(b);
                            let visible = bounds_visible(b, window);
                            if visibility.replace(visible) != visible {
                                let _ = owner.update(cx, |_, cx| cx.notify());
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .into_any_element()
        }
    }
}

fn popover_face(
    label: SharedString,
    trigger: Trigger,
    progress: f64,
    leading: Option<&'static str>,
) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .px_3()
        .text_size(px(13.))
        .whitespace_nowrap()
        .when(trigger == Trigger::Icon, |v| v.px_0())
        .when_some(leading, |v, path| {
            v.child(gpui::img(path).size(px(18.)).flex_shrink_0())
        })
        .child(if trigger == Trigger::Icon {
            crate::controls::icon("icons/more-horizontal.svg", 14.).into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(label)
                .into_any_element()
        })
        .when(trigger == Trigger::Field, |v| {
            v.child(
                crate::controls::icon("icons/chevron-down.svg", 12.).with_transformation(
                    Transformation::rotate(radians(std::f32::consts::PI * progress as f32)),
                ),
            )
        })
}

fn window_focus(window: &mut Window, focus: &FocusHandle, cx: &mut App) {
    window.focus(focus, cx);
}

pub(crate) fn measure_label(label: &str, size: f32, window: &mut Window) -> f32 {
    let style = window.text_style();
    window
        .text_system()
        .shape_line(
            label.to_owned().into(),
            px(size),
            &[TextRun {
                len: label.len(),
                font: style.font(),
                color: style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        )
        .width
        .as_f32()
}

fn bounds_visible(bounds: Bounds<Pixels>, window: &Window) -> bool {
    let size = bounds
        .intersect(&window.content_mask().bounds)
        .intersect(&Bounds::new(point(px(0.), px(0.)), window.viewport_size()))
        .size;
    size.width > px(0.) && size.height > px(0.)
}

#[derive(Clone)]
struct DialogTrigger {
    id: SharedString,
    focus: FocusHandle,
}

pub struct DialogOptions {
    pub title_editor: Option<AnyElement>,
    pub title_action: Option<AnyElement>,
    pub notice: Option<String>,
    pub dismissible: bool,
}
impl Default for DialogOptions {
    fn default() -> Self {
        Self {
            title_editor: None,
            title_action: None,
            notice: None,
            dismissible: true,
        }
    }
}

pub struct Dialog {
    motion: Rc<RefCell<Motion>>,
    source_material: super::render::SourceMaterial,
    measured: super::panel::ContentSize,
    activation: Rc<Cell<u64>>,
    seen_activation: u64,
    motion_source: Option<SharedString>,
    origin: Rc<Cell<Bounds<Pixels>>>,
    focus: crate::modal::FocusScope,
    content_id: Option<SharedString>,
    trigger: Rc<RefCell<Option<DialogTrigger>>>,
    trigger_visible: Rc<Cell<bool>>,
    pending_source: Rc<RefCell<Option<source::Activation>>>,
    alert: bool,
    initial_focus: Option<FocusHandle>,
    focus_pending: bool,
    presentation: Presentation,
    input_gate: InteractionGate,
    layer_open: bool,
    source_priority: usize,
    closing_anchor: Option<Bounds<Pixels>>,
    paint_offset: Rc<Cell<Point<Pixels>>>,
    body_drawing: Rc<RefCell<super::render::BodyDrawing>>,
}
impl Dialog {
    pub fn new(cx: &mut App) -> Self {
        Self {
            // A modal moves freely. An edge anchor adds resizing tether motion
            // to its position, visibly swinging the entire content panel.
            motion: Rc::new(RefCell::new(Motion {
                anchor: [0.5, 0.5],
                ..Default::default()
            })),
            measured: Default::default(),
            source_material: Default::default(),
            activation: Default::default(),
            seen_activation: 0,
            motion_source: None,
            origin: Default::default(),
            focus: crate::modal::FocusScope::new(cx),
            content_id: None,
            trigger: Default::default(),
            trigger_visible: Default::default(),
            pending_source: Default::default(),
            alert: false,
            initial_focus: None,
            focus_pending: false,
            presentation: Default::default(),
            input_gate: InteractionGate::new(false),
            layer_open: false,
            source_priority: 0,
            closing_anchor: None,
            paint_offset: Default::default(),
            body_drawing: Default::default(),
        }
    }
    /// Confirmation dialogs dismiss through an explicit action or Escape,
    /// and initially focus the cancel action supplied by the composition.
    pub fn alert(mut self) -> Self {
        self.alert = true;
        self
    }
    pub fn initial_focus(&mut self, focus: FocusHandle) {
        self.initial_focus = Some(focus);
    }
    pub fn inspect(&self) -> serde_json::Value {
        let mut value = self.motion.borrow().inspect();
        if let Some(fields) = value.as_object_mut() {
            fields.insert("destinationLayer".into(), serde_json::json!(if self.layer_open { "modal" } else { "source" }));
            fields.insert("destinationPriority".into(), serde_json::json!(self.source_priority + if self.layer_open { 100 } else { 0 }));
            fields.insert("paintOffset".into(), serde_json::json!([self.paint_offset.get().x.as_f32(), self.paint_offset.get().y.as_f32()]));
            fields.insert(
                "anchor".into(),
                serde_json::json!(pose(self.origin.get(), 16.)),
            );
            fields.insert(
                "contentAlpha".into(),
                serde_json::json!(self.presentation.opacity()),
            );
            fields.insert(
                "backdropAlpha".into(),
                serde_json::json!(self.presentation.backdrop.get().opacity()),
            );
            fields.insert(
                "contentScale".into(),
                serde_json::json!(dialog_recipe(true).scale(self.presentation.opacity())),
            );
            fields.insert(
                "paintOnlyFrames".into(),
                serde_json::json!(self.presentation.frames()),
            );
        }
        value
    }
    pub fn alive(&self) -> bool {
        self.motion.borrow().alive()
            || self.presentation.opacity() > 0.
            || self.presentation.backdrop.get().opacity() > 0.
    }
    pub fn pose(&self) -> Option<Pose> {
        self.motion
            .borrow()
            .surface
            .as_ref()
            .map(|s| s.simulation.pose())
    }
    pub fn samples(&self) -> std::cell::Ref<'_, [FrameSample]> {
        std::cell::Ref::map(self.motion.borrow(), |motion| motion.samples.as_slice())
    }
    pub fn reset_samples(&mut self) {
        self.motion.borrow_mut().samples.clear();
    }
    pub fn visible(&self) -> bool {
        self.trigger_visible.get()
            || (self.alive()
                && self
                    .motion
                    .borrow()
                    .surface
                    .as_ref()
                    .is_some_and(Surface::visible))
    }
    pub fn trigger<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        width: f32,
        style: controls::ActionStyle,
        parent: u32,
        window: &mut Window,
        cx: &mut Context<V>,
        open: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> BoundTrigger<controls::Action> {
        let id = id.into();
        let label = label.into();
        let focus = controls::action_focus(id.clone(), window, cx);
        let style = controls::ActionStyle {
            opens_panel: true,
            ..style
        };
        self.source_binding()
            .bind(
                controls::adaptive_action(id.clone(), label.clone(), style, parent)
                    .w(px(width))
                    .h(px(32.)),
                label,
                style,
            )
            .control_focus(&focus)
            .on_click(cx.listener(move |v, _, w, cx| {
                w.focus(&focus, cx);
                open(v, w, cx);
            }))
    }
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.focus.clone()
    }
    pub fn source_binding(&self) -> SourceBinding {
        SourceBinding {
            material: self.source_material.clone(),
            anchor: MeasuredAnchor {
                bounds: self.origin.clone(),
                visible: self.trigger_visible.clone(),
                ..Default::default()
            },
            trigger: self.trigger.clone(),
            activation: self.activation.clone(),
            pending: self.pending_source.clone(),
        }
    }
    pub fn bind_source(&mut self, source: SourceBinding) {
        self.source_material = source.material;
        self.origin = source.anchor.bounds;
        self.trigger_visible = source.anchor.visible;
        self.trigger = source.trigger;
        self.activation = source.activation;
        self.pending_source = source.pending;
    }
    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl IntoElement,
        footer: Option<AnyElement>,
        open: bool,
        placement: Placement,
        material: Material,
        window: &mut Window,
        cx: &mut Context<V>,
        close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        self.render_with_options(
            id,
            title,
            body,
            footer,
            open,
            placement,
            material,
            DialogOptions::default(),
            window,
            cx,
            close,
        )
    }
    pub fn render_with_options<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl IntoElement,
        footer: Option<AnyElement>,
        open: bool,
        placement: Placement,
        material: Material,
        options: DialogOptions,
        window: &mut Window,
        cx: &mut Context<V>,
        close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        let owner = cx.entity().into_any().downgrade();
        let close = crate::modal::bind_close(cx, close);
        self.render_content(
            id.into(), title.into(), body.into_any_element(), footer, open,
            placement, material, options, owner, window, cx, close,
        )
    }

    // Keep the full renderer independent of caller, body and callback types.
    // Native and WASM share this implementation instead of compiling it once
    // for every application view and event closure.
    fn render_content(
        &mut self,
        id: SharedString,
        title: SharedString,
        body: AnyElement,
        footer: Option<AnyElement>,
        open: bool,
        placement: Placement,
        material: Material,
        options: DialogOptions,
        owner: AnyWeakEntity,
        window: &mut Window,
        cx: &mut App,
        close: crate::modal::CloseAction,
    ) -> Option<AnyElement> {
        self.presentation.begin();
        self.presentation.release_presented_initial(window);
        let mut motion = self.motion.borrow_mut();
        if !open && !motion.alive() && self.presentation.opacity() == 0. {
            self.source_material
                .relocate_drawing(false, None, window, cx);
            self.presentation.region.borrow_mut().take();
            self.presentation.initial.borrow_mut().take();
            self.input_gate.set_state(false, false);
            self.source_material.clear();
            self.focus.sync(None, window, cx);
            return None;
        }
        let floating = matches!(placement, Placement::Window { .. });
        let paint_only = floating
            && window.gpu_mask_layers_enabled()
            && !window.is_a11y_active()
            && (!open || self.initial_focus.is_none());
        motion.paint_only = paint_only;
        let viewport = window.viewport_size();
        let mut source_drawing = None;
        if open && self.seen_activation != self.activation.get() {
            if let Some(source) = self.pending_source.borrow_mut().take() {
                self.origin.set(source.bounds);
                self.source_priority = source.priority;
                source_drawing = source.drawing.borrow().clone();
                self.source_material.register(source.trigger.id.clone(), source.view);
                *self.trigger.borrow_mut() = Some(source.trigger);
            }
        }
        if floating && self.layer_open != open {
            let fallback = Bounds::new(point(px(0.), px(0.)), viewport);
            self.presentation.transfer(source_drawing.as_ref(), fallback, window);
            if cx.reduce_motion() { self.presentation.initial.borrow_mut().take(); }
            if open {
                motion.translate(self.paint_offset.replace(point(px(0.), px(0.))));
                self.closing_anchor = None;
            } else { self.closing_anchor = Some(self.origin.get()); }
            motion.last = None;
            self.layer_open = open;
        }
        if self.presentation.initial.borrow().is_some() {
            motion.last = None;
            self.presentation.hold();
        }
        let (width, height, source, target) = match placement {
            Placement::Inline {
                width,
                height,
                source,
                target,
            } => (width, height, source, target),
            Placement::Window { width } => {
                let w = width.min(viewport.width.as_f32() - 40.).max(2.);
                let h = (viewport.height.as_f32() - 64.).clamp(2., 660.);
                (
                    viewport.width.as_f32(),
                    viewport.height.as_f32(),
                    pose(self.closing_anchor.map_or_else(|| self.origin.get(), |anchor| {
                        Bounds::new(anchor.origin, self.origin.get().size)
                    }), 16.),
                    Pose::rect(
                        (viewport.width.as_f32() - w) as f64 / 2.,
                        (viewport.height.as_f32() - h) as f64 / 2.,
                        w as f64,
                        h as f64,
                        crate::controls::MODAL_RADIUS as f64,
                    ),
                )
            }
        };
        if open && self.seen_activation != self.activation.get() {
            self.seen_activation = self.activation.get();
            self.focus_pending = true;
            if let Some(trigger) = self.trigger.borrow().as_ref() {
                if self.motion_source.as_ref() != Some(&trigger.id) {
                    *motion = Motion {
                        anchor: [0.5, 0.5],
                        scheduled: motion.scheduled.clone(),
                        samples: std::mem::take(&mut motion.samples),
                        ..Default::default()
                    };
                    if self.content_id.as_ref() != Some(&id) {
                        self.measured = Default::default();
                    }
                    self.motion_source = Some(trigger.id.clone());
                }
                self.focus.activate("dialog", &trigger.focus, window, cx);
            }
        }
        self.focus.sync(open.then_some("dialog"), window, cx);
        if open && self.content_id.as_ref() != Some(&id) {
            window.focus(&self.focus.focus, cx);
        }
        self.content_id = Some(id.clone());
        let clip = super::ContentClipBinding::fixed_layout();
        let contents = open.then(|| crate::modal::panel_contents_with_title_action(
            id.clone(),
            title.clone(),
            options
                .title_action
                .map(|action| crate::modal::TitleAction {
                    editor: options.title_editor,
                    action,
                }),
            body,
            footer,
            options.notice,
            &self.focus.focus,
            px(target.h as f32),
            Some(clip.clone()),
            window,
            cx,
            open && options.dismissible,
            close.clone(),
        ));
        let content_height = self.measured.height(target.h).unwrap_or(target.h);
        let target = Pose::rect(
            target.left(),
            if floating {
                (height as f64 - content_height) / 2.
            } else {
                target.top()
            },
            target.w,
            content_height,
            target.r,
        );
        // The component schedules normal rendering when input/focus needs it;
        // otherwise the shared presentation driver advances both motions.
        motion.paint_only = paint_only;
        let content_moving = self.presentation.advance(open, cx);
        if content_moving && !paint_only {
            super::motion::schedule(&motion.scheduled, &owner, window);
        }
        let material_visible = floating
            || self.trigger_visible.get()
            || motion.surface.as_ref().is_none_or(Surface::visible);
        motion.frame_for_owner(
            source,
            target,
            true,
            open,
            material,
            material_visible,
            &owner,
            window,
            cx,
        );
        if !open {
            self.focus_pending = false;
        }
        if open && self.focus_pending && motion.progress() > 0.5 {
            self.focus_pending = false;
            if let Some(focus) = &self.initial_focus {
                window.focus(focus, cx);
            }
        }
        let material_moving = motion
            .surface
            .as_ref()
            .is_some_and(|surface| motion.state.moving(&surface.simulation));
        let backdrop_moving = self.presentation.advance_backdrop(
            floating && open,
            motion.expansion() >= 0.5,
            cx,
        );
        if backdrop_moving && !paint_only {
            super::motion::schedule(&motion.scheduled, &owner, window);
        }
        let draw_material = open
            || motion.alive()
            || self.presentation.opacity() > 0.
            || self.presentation.backdrop.get().opacity() > 0.;
        self.source_material.relocate_drawing(
            floating && draw_material,
            draw_material
                .then(|| {
                    self.trigger
                        .borrow()
                        .as_ref()
                        .map(|trigger| trigger.id.clone())
                })
                .flatten(),
            window,
            cx,
        );
        self.input_gate.set_state(draw_material, open);
        if !draw_material {
            self.source_material.clear();
            return None;
        }
        let surface = motion.surface.as_ref().unwrap();
        self.source_material.clear();
        clip.bind(surface.content_clip());
        let contents = contents.map(|contents| self.measured.measure(
            contents, px(target.w as f32), true, motion.scheduled.clone(), owner.clone(),
        ));
        let recipe = dialog_recipe(floating);
        let paint = FramePaint {
            motion: self.motion.clone(),
            content: self.presentation.content.clone(),
            backdrop: self.presentation.backdrop.clone(),
            source: Some(self.source_material.clone()),
            body: Some(self.body_drawing.clone()),
            offset: self.paint_offset.clone(),
            part: None,
            recipe,
        };
        let content_alpha = self.presentation.opacity();
        let contents = contents.map(|contents| placed(target)
            .child(surface.content_clip().transformed(
                contents,
                recipe.scale(content_alpha),
                content_alpha,
                format!("{id}-retained-content"),
                self.presentation.snapshot.clone(),
            ))
            .id(id.clone())
            .role(if self.alert {
                Role::AlertDialog
            } else {
                Role::Dialog
            })
            .aria_label(title.clone())
            .map(|panel| surface.guard(panel))
            .occlude()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .automation_enabled(open, AutomationRole::Status, title.to_string()).into_any_element());
        let mut stage = div()
            .id(format!("{id}-backdrop"))
            .relative()
            .w(px(width))
            .h(px(height));
        if floating && open {
            let dismiss_outside = !self.alert && options.dismissible;
            stage = stage.occlude().on_mouse_down(
                MouseButton::Left,
                move |_, w, cx| {
                    if dismiss_outside {
                        close(w, cx);
                    }
                    cx.stop_propagation();
                },
            );
        }
        let stage = if let Some(contents) = contents {
            interaction_scope(
                format!("{id}-input-scope"), self.input_gate.clone(),
                stage.child(paint.underlay()).child(contents).child(paint.outline()),
            ).into_any_element()
        } else {
            let drawing = paint.clone();
            let snapshot = self.presentation.snapshot.clone();
            stage.child(canvas(|_, _, _| {}, move |_, _, window, _| {
                if let Some(snapshot) = snapshot.borrow().as_ref() { drawing.replay(snapshot, window); }
            }).absolute().inset_0()).into_any_element()
        };
        let stage = if paint_only && (material_moving || content_moving || backdrop_moving) {
            self.presentation.playback(
                stage,
                paint,
                Target {
                    from: source,
                    to: target,
                    pair: true,
                    open,
                    material,
                },
                Rc::new(move |cx| {
                    super::motion::notify(&owner, cx);
                }),
            )
        } else {
            self.presentation.record(stage)
        };
        Some(if floating {
            anchor::destination_layer(stage, self.origin.clone(),
                self.closing_anchor.map(|bounds| bounds.origin), self.paint_offset.clone(),
                self.source_priority + if open { 100 } else { 0 })
        } else {
            stage.into_any_element()
        })
    }
}

fn dialog_recipe(floating: bool) -> Recipe {
    Recipe {
        fill: CUE_UI.palette.canvas,
        border: Some(LIQUID_OUTLINE),
        backdrop: floating.then_some(56),
        initial_scale: 0.96,
        fade_material: false,
    }
}

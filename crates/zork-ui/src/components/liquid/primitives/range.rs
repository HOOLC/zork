use super::super::{controls, Pose};
use super::{disabled_node, surface};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::{BRAND_ACCENT, ZORK_UI},
};
use gpui::{prelude::*, *};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub minimum_gap: f64,
    pub vertical: bool,
    pub reversed: bool,
}
impl Default for Scale {
    fn default() -> Self {
        Self {
            min: 0.,
            max: 100.,
            step: 1.,
            minimum_gap: 0.,
            vertical: false,
            reversed: false,
        }
    }
}
impl Scale {
    fn valid(self) -> bool {
        self.min.is_finite()
            && self.max.is_finite()
            && self.max > self.min
            && self.step.is_finite()
            && self.step > 0.
            && self.minimum_gap.is_finite()
            && self.minimum_gap >= 0.
    }
    pub fn set(self, values: &[f64], index: usize, raw: f64) -> Vec<f64> {
        let mut next = values.to_vec();
        if !self.valid() || index >= values.len() || !raw.is_finite() {
            return next;
        }
        let min = if index == 0 {
            self.min
        } else {
            values[index - 1] + self.minimum_gap
        };
        let max = if index + 1 == values.len() {
            self.max
        } else {
            values[index + 1] - self.minimum_gap
        };
        if min > max {
            return next;
        }
        let low = self.min + ((min - self.min) / self.step).ceil() * self.step;
        let high = self.min + ((max - self.min) / self.step).floor() * self.step;
        if low > high {
            return next;
        }
        next[index] =
            (self.min + ((raw - self.min) / self.step).round() * self.step).clamp(low, high);
        next
    }
    fn fraction(self, value: f64) -> f32 {
        let fraction = if self.valid() {
            ((value - self.min) / (self.max - self.min)).clamp(0., 1.) as f32
        } else {
            0.
        };
        if self.reversed ^ self.vertical {
            1. - fraction
        } else {
            fraction
        }
    }
    fn point(self, position: Point<Pixels>, bounds: Bounds<Pixels>, end_inset: f32) -> f64 {
        let fraction = if self.vertical {
            (position.y - bounds.top() - px(end_inset)).as_f32()
                / (bounds.size.height.as_f32() - end_inset * 2.).max(1.)
        } else {
            (position.x - bounds.left() - px(end_inset)).as_f32()
                / (bounds.size.width.as_f32() - end_inset * 2.).max(1.)
        };
        let fraction = if self.reversed ^ self.vertical {
            1. - fraction
        } else {
            fraction
        };
        self.min + fraction.clamp(0., 1.) as f64 * (self.max - self.min)
    }
}
#[derive(Default)]
struct Drag {
    bounds: Cell<Bounds<Pixels>>,
    active: Cell<Option<usize>>,
    values: RefCell<Vec<f64>>,
}

/// One or more ordered thumbs. Pointer changes emit previews; release/keyboard
/// actions emit a commit, leaving business persistence to the consuming core.
pub fn slider<V: 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    values: Vec<f64>,
    scale: Scale,
    length: f32,
    disabled: bool,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, Vec<f64>, bool, &mut Context<V>) + 'static,
) -> AnyElement {
    render_slider(
        id, label, values, scale, length, disabled, false, window, cx, change,
    )
}

pub fn step_slider<V: 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    values: Vec<f64>,
    scale: Scale,
    length: f32,
    disabled: bool,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, Vec<f64>, bool, &mut Context<V>) + 'static,
) -> AnyElement {
    render_slider(
        id, label, values, scale, length, disabled, true, window, cx, change,
    )
}

fn render_slider<V: 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    values: Vec<f64>,
    scale: Scale,
    length: f32,
    disabled: bool,
    capsule: bool,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, Vec<f64>, bool, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let label = label.into();
    let disabled = disabled || !scale.valid() || values.is_empty();
    let model = window.use_keyed_state(format!("{id}-drag"), cx, |_, _| Rc::new(Drag::default()));
    let state = model.read(cx).clone();
    *state.values.borrow_mut() = values.clone();
    if disabled {
        state.active.set(None);
    }
    let callback: Rc<dyn Fn(&Vec<f64>, bool, &mut Window, &mut App)> = Rc::new({
        let owner = cx.entity().downgrade();
        move |values, commit, _, cx| {
            let _ = owner.update(cx, |v, cx| change(v, values.clone(), commit, cx));
        }
    });
    let (width, height) = if scale.vertical {
        (32., length)
    } else {
        (length, if capsule { 28. } else { 32. })
    };
    let mut track = div()
        .id(id.clone())
        .relative()
        .w(px(width))
        .h(px(height))
        .role(Role::Group)
        .aria_label(label.clone())
        .when(disabled, |v| v.opacity(0.4));
    let diameter = if capsule { 28. } else { 20. };
    let end_inset = diameter / 2.;
    let span = (length - diameter).max(1.);
    let bounds = Pose::rect(0., 0., width as f64, height as f64, 0.);
    let rail_pose = if capsule && !scale.vertical {
        Pose::rect(0., 2., width as f64, 24., 12.)
    } else {
        zork_liquid::recipes::slider_rail(bounds, scale.vertical)
    };
    let rail = surface(
        format!("{id}-rail"),
        rail_pose.r as f32,
        ZORK_UI.palette.border,
        false,
    )
    .absolute()
    .left(px(rail_pose.left() as f32))
    .top(px(rail_pose.top() as f32))
    .w(px(rail_pose.w as f32))
    .h(px(rail_pose.h as f32));
    let mut layers = Vec::new();
    let mut presented = Vec::new();
    let handles: Vec<_> = (0..values.len())
        .map(|i| controls::action_focus(format!("{id}-thumb-{i}"), window, cx).tab_stop(!disabled))
        .collect();
    for (i, value) in values.iter().copied().enumerate() {
        let focus = handles[i].clone();
        let callback = callback.clone();
        let key_values = values.clone();
        let increase = callback.clone();
        let decrease = callback.clone();
        let increase_values = values.clone();
        let decrease_values = values.clone();
        let accessible_min = if i == 0 {
            scale.min
        } else {
            values[i - 1] + scale.minimum_gap
        };
        let accessible_max = if i + 1 == values.len() {
            scale.max
        } else {
            values[i + 1] - scale.minimum_gap
        };
        let active = !disabled && state.active.get() == Some(i);
        let target = zork_liquid::recipes::slider_target(
            bounds,
            scale.fraction(value) as f64,
            scale.vertical,
            active,
        );
        let target = if capsule {
            let visual_width = target.w * 1.4;
            Pose {
                w: visual_width,
                h: target.h * 1.4,
                r: target.r * 1.4,
                cx: if scale.vertical {
                    target.cx
                } else {
                    visual_width / 2.
                        + scale.fraction(value) as f64 * (width as f64 - visual_width).max(1.)
                },
                ..target
            }
        } else {
            target
        };
        let inset = ((if scale.vertical { width } else { height }) - diameter) / 2.;
        let material_id = format!("{id}-material-{i}");
        layers.push(
            controls::with_control_surface(
                &material_id,
                target,
                zork_liquid::recipes::SLIDER_PLAYBACK_RATE,
                window,
                cx,
                |surface, _, _| {
                    let pose = surface.simulation.pose();
                    presented.push(if scale.vertical {
                        pose.cy as f32
                    } else {
                        pose.cx as f32
                    });
                    div()
                        .id(material_id.clone())
                        .absolute()
                        .inset_0()
                        .w(px(width))
                        .h(px(height))
                        .child(surface.background(
                            if capsule {
                                ZORK_UI.palette.canvas
                            } else {
                                BRAND_ACCENT
                            },
                            capsule.then_some(ZORK_UI.palette.border_strong),
                        ))
                },
            )
            .into_any_element(),
        );
        let thumb = div()
            .id(format!("{id}-thumb-{i}"))
            .absolute()
            .size(px(diameter))
            .when(scale.vertical, |v| {
                v.left(px(inset)).top(px(scale.fraction(value) * span))
            })
            .when(!scale.vertical, |v| {
                v.top(px(inset)).left(px(scale.fraction(value) * span))
            })
            .role(Role::Slider)
            .aria_label(format!("{label} {}", i + 1))
            .aria_numeric_value(value)
            .aria_min_numeric_value(accessible_min)
            .aria_max_numeric_value(accessible_max)
            .aria_numeric_value_step(scale.step)
            .aria_orientation(if scale.vertical {
                Orientation::Vertical
            } else {
                Orientation::Horizontal
            })
            .a11y_synthetic_children(move |builder| disabled_node(builder.parent_node(), disabled))
            .when(!disabled, |v| {
                v.on_a11y_action(AccessibleAction::Increment, move |_, w, cx| {
                    increase(
                        &scale.set(&increase_values, i, value + scale.step),
                        true,
                        w,
                        cx,
                    )
                })
                .on_a11y_action(AccessibleAction::Decrement, move |_, w, cx| {
                    decrease(
                        &scale.set(&decrease_values, i, value - scale.step),
                        true,
                        w,
                        cx,
                    )
                })
            })
            .track_focus(&focus)
            .tab_stop(!disabled)
            .when(
                focus.is_focused(window) && window.last_input_was_keyboard(),
                |v| {
                    v.child(
                        div()
                            .absolute()
                            .inset(px(if capsule { 0. } else { 5. }))
                            .when(capsule, |v| {
                                v.border_2()
                                    .border_color(rgb(crate::design::INTERACTION.focus_border))
                            })
                            .when(!capsule, |v| v.bg(rgb(ZORK_UI.palette.canvas)))
                            .rounded_full(),
                    )
                },
            )
            .on_key_down(move |e: &KeyDownEvent, w, cx| {
                if disabled {
                    return;
                }
                let large = if e.keystroke.modifiers.shift { 10. } else { 1. };
                let raw = match e.keystroke.key.as_str() {
                    "home" => scale.min,
                    "end" => scale.max,
                    "pageup" => value + scale.step * 10.,
                    "pagedown" => value - scale.step * 10.,
                    "up" => value + scale.step * large,
                    "down" => value - scale.step * large,
                    "right" => value + scale.step * large * if scale.reversed { -1. } else { 1. },
                    "left" => value - scale.step * large * if scale.reversed { -1. } else { 1. },
                    _ => return,
                };
                callback(&scale.set(&key_values, i, raw), true, w, cx);
                w.prevent_default();
                cx.stop_propagation();
            })
            .automation_enabled(
                !disabled,
                AutomationRole::Button,
                format!("{label}滑块 {}", i + 1),
            );
        layers.push(thumb.into_any_element());
    }
    let low = if values.len() == 1 {
        end_inset + scale.fraction(scale.min) * span
    } else {
        presented.first().copied().unwrap_or(end_inset)
    };
    let high = presented.last().copied().unwrap_or(low);
    let fill_pose = if capsule && !scale.vertical {
        Pose::rect(0., 2., high as f64, 24., 12.)
    } else {
        zork_liquid::recipes::slider_range(bounds, low as f64, high as f64, scale.vertical)
    };
    let fill = surface(
        format!("{id}-range"),
        fill_pose.r as f32,
        BRAND_ACCENT,
        false,
    )
    .absolute()
    .left(px(fill_pose.left() as f32))
    .top(px(fill_pose.top() as f32))
    .w(px(fill_pose.w as f32))
    .h(px(fill_pose.h as f32));
    track = track.child(rail).child(fill);
    if capsule && !scale.vertical && scale.valid() {
        let count = (((scale.max - scale.min) / scale.step).round() as usize + 1).min(32);
        for index in 0..count {
            let value = scale.min + index as f64 * scale.step;
            let x = end_inset + scale.fraction(value) * span;
            track = track.child(
                surface(
                    format!("{id}-tick-{index}"),
                    2.,
                    if x <= high {
                        ZORK_UI.palette.canvas
                    } else {
                        ZORK_UI.palette.muted
                    },
                    false,
                )
                .absolute()
                .left(px(x - 2.))
                .top(px(height / 2. - 2.))
                .size(px(4.)),
            );
        }
    }
    track = track.children(layers);
    let down = state.clone();
    let down_callback = callback.clone();
    let measured = state.clone();
    let painted = state.clone();
    track
        .on_mouse_down(MouseButton::Left, move |e, w, cx| {
            if disabled {
                return;
            }
            let raw = scale.point(e.position, down.bounds.get(), end_inset);
            let index = down
                .values
                .borrow()
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (*a - raw).abs().total_cmp(&(*b - raw).abs()))
                .map_or(0, |(i, _)| i);
            down.active.set(Some(index));
            w.focus(&handles[index], cx);
            let next = scale.set(&down.values.borrow(), index, raw);
            *down.values.borrow_mut() = next.clone();
            down_callback(&next, false, w, cx);
            cx.stop_propagation();
        })
        .child(
            canvas(
                move |bounds, _, _| measured.bounds.set(bounds),
                move |_, _, w, _| {
                    let move_state = painted.clone();
                    let move_callback = callback.clone();
                    w.on_mouse_event(move |e: &MouseMoveEvent, phase, w, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        if let Some(i) = move_state.active.get() {
                            if !e.dragging() {
                                move_state.active.set(None);
                                return;
                            }
                            let next = scale.set(
                                &move_state.values.borrow(),
                                i,
                                scale.point(e.position, move_state.bounds.get(), end_inset),
                            );
                            *move_state.values.borrow_mut() = next.clone();
                            move_callback(&next, false, w, cx);
                            cx.stop_propagation();
                        }
                    });
                    w.on_mouse_event(move |e: &MouseUpEvent, phase, w, cx| {
                        if phase == DispatchPhase::Capture
                            && e.button == MouseButton::Left
                            && painted.active.take().is_some()
                        {
                            callback(&painted.values.borrow().clone(), true, w, cx);
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        )
        .automation_enabled(!disabled, AutomationRole::Status, label)
        .into_any_element()
}

pub fn progress(
    id: impl Into<SharedString>,
    value: Option<f64>,
    max: f64,
    width: f32,
) -> AnyElement {
    let id = id.into();
    let fraction = value.map(|v| {
        if max.is_finite() && max > 0. {
            (v / max).clamp(0., 1.)
        } else {
            0.
        }
    });
    let rail = surface(format!("{id}-track"), 4., ZORK_UI.palette.border, false)
        .w(px(width))
        .h(px(8.))
        .relative()
        .child(
            surface(format!("{id}-value"), 4., BRAND_ACCENT, false)
                .absolute()
                .left_0()
                .top_0()
                .h_full()
                .w(px(width * fraction.unwrap_or(0.35) as f32)),
        );
    div()
        .id(id)
        .role(Role::ProgressIndicator)
        .aria_label("进度")
        .aria_min_numeric_value(0.)
        .aria_max_numeric_value(max)
        .when_some(value, |v, value| {
            v.aria_numeric_value(value.clamp(0., max.max(0.)))
        })
        .aria_value(fraction.map_or_else(|| "进行中".into(), |v| format!("{:.0}%", v * 100.)))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(rail)
        .when(fraction.is_none(), |v| {
            v.child(crate::components::loading::status(
                "progress-loading",
                "进行中…",
            ))
        })
        .automation(AutomationRole::Status, "进度")
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::Scale;
    #[test]
    fn thumbs_respect_step_endpoints_and_minimum_gap() {
        let scale = Scale {
            min: 10.,
            max: 90.,
            step: 5.,
            minimum_gap: 10.,
            ..Default::default()
        };
        assert_eq!(scale.set(&[25., 65.], 0, 68.), vec![55., 65.]);
        assert_eq!(scale.set(&[25., 65.], 1, 24.), vec![25., 35.]);
        assert_eq!(scale.set(&[25., 65.], 0, -30.), vec![10., 65.]);
        assert_eq!(scale.set(&[25., 65.], 1, 100.), vec![25., 90.]);
    }
    #[test]
    fn malformed_range_does_not_panic_or_emit_nan() {
        assert_eq!(
            Scale {
                min: 20.,
                max: 10.,
                ..Default::default()
            }
            .set(&[15.], 0, 5.),
            vec![15.]
        );
        assert_eq!(Scale::default().set(&[20.], 0, f64::NAN), vec![20.]);
    }
}

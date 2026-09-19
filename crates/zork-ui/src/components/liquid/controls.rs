//! Reusable control surfaces. Hosts provide actions, validation and busy state.
use super::{skin, SurfaceColors};
use crate::{
    components::{motion::HoverFill, text_input::ComposerInput},
    controls::CONTROL_HEIGHT,
    design::{BRAND_ACCENT, CUE_UI, INTERACTION, LIQUID_OUTLINE},
};
use gpui::{prelude::*, *};
mod adaptive;
pub use adaptive::{adaptive_action, adaptive_input, Action, Field};

/// Shared focus binding for controls that defer their material construction and
/// for ordinary compositional rows used as overlay sources.
pub trait ControlElement: Element + StatefulInteractiveElement + ParentElement + Styled {
    fn control_focus(self, focus: &FocusHandle) -> Self;
    fn panel_source(self) -> Self {
        self
    }
    fn source_material(self, _material: super::render::SourceMaterial) -> Self {
        self
    }
    fn control_overlay(mut self, overlay: AnyElement) -> Self {
        self.extend([overlay]);
        self
    }
}
impl ControlElement for Stateful<Div> {
    fn control_focus(self, focus: &FocusHandle) -> Self {
        self.track_focus(focus)
    }
}
impl ControlElement for Action {
    fn control_focus(self, focus: &FocusHandle) -> Self {
        self.track_focus(focus)
    }
    fn panel_source(self) -> Self {
        self.opens_panel()
    }
    fn source_material(self, material: super::render::SourceMaterial) -> Self {
        self.material_source(material)
    }
    fn control_overlay(self, overlay: AnyElement) -> Self {
        self.overlay(overlay)
    }
}
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
#[cfg(target_family = "wasm")]
use web_time::Instant;

const SEGMENT_INSET: f32 = zork_liquid::recipes::SEGMENT_INSET as f32;
const SEGMENT_GAP: f32 = zork_liquid::recipes::SEGMENT_GAP as f32;

pub fn segment_pose(width: f32, count: usize, selected: usize) -> super::Pose {
    zork_liquid::recipes::segment_pose(width as f64, CONTROL_HEIGHT as f64, count, selected)
}

pub fn toggle_pose(checked: bool) -> super::Pose {
    zork_liquid::recipes::toggle_pose(checked)
}

struct ControlMotion {
    surface: super::Surface,
    target: super::Pose,
    bounds: Bounds<Pixels>,
    last: Option<Instant>,
    scheduled: bool,
}

/// Ordinary consumers own the same presentation model that a measured gallery
/// may supply explicitly through the `*_with_surface` entry points.
pub(super) fn with_control_surface(
    id: &str,
    target: super::Pose,
    speed: f64,
    window: &mut Window,
    cx: &mut App,
    render: impl FnOnce(&mut super::Surface, &mut Window, &mut App) -> Stateful<Div>,
) -> Stateful<Div> {
    let state = window.use_keyed_state(format!("{id}-motion"), cx, |_, _| {
        let mut simulation = super::Simulation::new(
            target,
            super::Material::default(),
            super::Options::default(),
        );
        simulation.finish();
        std::rc::Rc::new(std::cell::RefCell::new(ControlMotion {
            surface: super::Surface::new(simulation).expect("valid control material"),
            target,
            bounds: Bounds::default(),
            last: None,
            scheduled: false,
        }))
    });
    let shared = state.read(cx).clone();
    let moving = {
        let mut model = shared.borrow_mut();
        if model.target != target {
            model.target = target;
            model.surface.simulation.set_target(target);
        }
        let now = Instant::now();
        let elapsed = model
            .last
            .replace(now)
            .map_or(0., |then| now.duration_since(then).as_secs_f64());
        let visible = model
            .bounds
            .intersect(&Bounds::new(point(px(0.), px(0.)), window.viewport_size()))
            .size;
        // Reduced/hidden advancement finishes the simulation and changes its
        // contour revision. Do it only for changing material, so scrolling past
        // settled controls does not rebuild their geometry on every frame.
        if model.surface.simulation.moving() {
            model.surface.simulation.advance(
                elapsed * speed * super::press::playback_rate(cx),
                cx.reduce_motion() || visible.width <= px(0.) || visible.height <= px(0.),
            );
        }
        model.surface.prepare();
        let moving = model.surface.simulation.moving();
        if !moving {
            model.last = None;
        }
        moving
    };
    if moving && !shared.borrow().scheduled {
        shared.borrow_mut().scheduled = true;
        let weak = state.downgrade();
        window.on_next_frame(move |_, cx| {
            let _ = weak.update(cx, |state, cx| {
                state.borrow_mut().scheduled = false;
                cx.notify();
            });
        });
    }
    let control = render(&mut shared.borrow_mut().surface, window, cx);
    control.child(
        canvas(
            move |bounds, _, _| shared.borrow_mut().bounds = bounds,
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0(),
    )
}

#[cfg(all(test, feature = "headless-bench"))]
mod retained_control_tests {
    use super::with_control_surface;
    use gpui::{div, prelude::*, px, AppContext, Context, Render, TestAppContext, Window};
    use std::{cell::RefCell, rc::Rc};
    struct Fixture {
        target: super::super::Pose,
        drawing: Rc<RefCell<Option<(Rc<super::super::Contour>, super::super::Pose)>>>,
    }
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let target = self.target;
            let drawing = self.drawing.clone();
            let control = with_control_surface(
                "hidden-control",
                target,
                1.,
                window,
                cx,
                move |surface, _, _| {
                    *drawing.borrow_mut() = Some((surface.contour(), surface.simulation.pose()));
                    div()
                        .id("hidden-control")
                        .w(px(target.w as f32))
                        .h(px(target.h as f32))
                },
            );
            div()
                .size_full()
                .child(div().absolute().top(px(2000.)).child(control))
        }
    }
    #[test]
    fn hidden_controls_reuse_settled_geometry_and_still_apply_new_targets() {
        let mut cx = TestAppContext::single();
        let drawing = Rc::new(RefCell::new(None));
        let target = super::super::Pose::rect(0., 0., 96., 32., 16.);
        let window = cx.add_window(|_, _| Fixture {
            target,
            drawing: drawing.clone(),
        });
        cx.run_until_parked();
        let first = drawing.borrow().as_ref().unwrap().0.clone();
        for _ in 0..12 {
            window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
            cx.run_until_parked();
            assert!(
                Rc::ptr_eq(&first, &drawing.borrow().as_ref().unwrap().0),
                "an unchanged hidden control regenerated its contour"
            );
        }
        let next = super::super::Pose::rect(0., 0., 156., 40., 20.);
        window
            .update(&mut cx, |view, _, cx| {
                view.target = next;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let drawing = drawing.borrow();
        let (path, pose) = drawing.as_ref().unwrap();
        assert!(!Rc::ptr_eq(&first, path));
        assert_eq!(*pose, next);
    }
}

type ControlCallback<T> = std::rc::Rc<dyn Fn(&T, &mut Window, &mut App)>;

/// Borderless single-choice options retain a visible selection mark and share
/// the ordinary action's hover, press, focus and keyboard activation behavior.
pub fn radio_group<V: 'static>(
    id: impl Into<SharedString>,
    width: f32,
    options: Vec<(String, SharedString)>,
    selected: usize,
    enabled: bool,
    parent: u32,
    window: &mut Window,
    cx: &mut Context<V>,
    choose: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    use crate::automation::{AutomationElementExt, AutomationRole};
    let count = options.len();
    let selected = selected.min(count.saturating_sub(1));
    let choose: ControlCallback<usize> =
        std::rc::Rc::new(cx.listener(move |view, index: &usize, _, cx| choose(view, *index, cx)));
    let mut row = div()
        .id(id.into())
        .role(Role::RadioGroup)
        .w(px(width))
        .flex()
        .gap(px(4.));
    let mut handles = Vec::with_capacity(count);
    for (index, (key, label)) in options.into_iter().enumerate() {
        let focus = action_focus(key.clone(), window, cx).tab_stop(enabled && index == selected);
        handles.push(focus.clone());
        let callback = choose.clone();
        row = row.child(
            render_action(
                key.into(),
                label.clone(),
                (width - 4. * count.saturating_sub(1) as f32) / count as f32,
                32.,
                ActionStyle {
                    quiet: true,
                    disabled: !enabled,
                    radio: Some(index == selected),
                    ..Default::default()
                },
                parent,
                focus.clone(),
                window,
                cx,
            )
            .tab_stop(enabled && index == selected)
            .role(Role::RadioButton)
            .aria_toggled(if index == selected {
                Toggled::True
            } else {
                Toggled::False
            })
            .on_click(move |_, window, cx| {
                if enabled {
                    window.focus(&focus, cx);
                    callback(&index, window, cx);
                }
            })
            .automation_enabled(enabled, AutomationRole::Option, label.to_string()),
        );
    }
    row.on_key_down(move |event: &KeyDownEvent, window, cx| {
        if !enabled || count == 0 {
            return;
        }
        let next = match event.keystroke.key.as_str() {
            "left" | "up" => Some((selected + count - 1) % count),
            "right" | "down" => Some((selected + 1) % count),
            "home" => Some(0),
            "end" => Some(count - 1),
            _ => None,
        };
        if let Some(next) = next {
            window.focus(&handles[next], cx);
            choose(&next, window, cx);
            cx.stop_propagation();
        }
    })
}

pub fn segmented<V: 'static>(
    id: impl Into<SharedString>,
    width: f32,
    options: Vec<(String, SharedString)>,
    selected: usize,
    enabled: bool,
    parent: u32,
    window: &mut Window,
    cx: &mut Context<V>,
    choose: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    segments(
        id,
        width,
        options
            .into_iter()
            .map(|(id, label)| Segment {
                id,
                label,
                disabled: false,
            })
            .collect(),
        Some(selected),
        SegmentKind::Choice,
        enabled,
        parent,
        window,
        cx,
        choose,
    )
}

pub struct Segment {
    pub id: String,
    pub label: SharedString,
    pub disabled: bool,
}

#[derive(Clone, Copy)]
pub enum SegmentKind {
    Choice,
    Toggle,
    Tabs { manual: bool },
}

/// Optional selection, disabled items and manual tabs use the same moving
/// material and clipped ink as the ordinary segmented control.
pub fn segments<V: 'static>(
    id: impl Into<SharedString>,
    width: f32,
    options: Vec<Segment>,
    selected: Option<usize>,
    kind: SegmentKind,
    enabled: bool,
    parent: u32,
    window: &mut Window,
    cx: &mut Context<V>,
    choose: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let choose: ControlCallback<usize> =
        std::rc::Rc::new(cx.listener(move |view, index: &usize, _, cx| choose(view, *index, cx)));
    with_control_surface(
        &id,
        segment_pose(width, options.len(), selected.unwrap_or(0)),
        1.,
        window,
        cx,
        |surface, window, cx| {
            render_segmented(
                id.clone(),
                width,
                options,
                selected,
                kind,
                enabled,
                parent,
                surface,
                window,
                cx,
                vec![],
                choose,
            )
        },
    )
}

/// Responsive forms use the same segmented material and keyboard selection as
/// fixed gallery controls. Hints remain attached to the actual option handles.
pub fn deferred_segmented(
    id: impl Into<SharedString>,
    options: Vec<Segment>,
    hints: Vec<Option<SharedString>>,
    selected: Option<usize>,
    kind: SegmentKind,
    enabled: bool,
    parent: u32,
    choose: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let choose: ControlCallback<usize> = std::rc::Rc::new(choose);
    adaptive::fill_slot(
        format!("{id}-slot"),
        CONTROL_HEIGHT,
        move |size, window, cx| {
            let width = size.width.as_f32().max(2.);
            with_control_surface(
                &id,
                segment_pose(width, options.len(), selected.unwrap_or(0)),
                1.,
                window,
                cx,
                |surface, window, cx| {
                    render_segmented(
                        id.clone(),
                        width,
                        options,
                        selected,
                        kind,
                        enabled,
                        parent,
                        surface,
                        window,
                        cx,
                        hints,
                        choose,
                    )
                },
            )
            .into_any_element()
        },
    )
}

pub fn segmented_with_surface<V: 'static>(
    id: impl Into<SharedString>,
    width: f32,
    options: Vec<(String, SharedString)>,
    selected: usize,
    enabled: bool,
    parent: u32,
    surface: &super::Surface,
    window: &mut Window,
    cx: &mut Context<V>,
    choose: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let choose: ControlCallback<usize> =
        std::rc::Rc::new(cx.listener(move |view, index: &usize, _, cx| choose(view, *index, cx)));
    render_segmented(
        id.into(),
        width,
        options
            .into_iter()
            .map(|(id, label)| Segment {
                id,
                label,
                disabled: false,
            })
            .collect(),
        Some(selected),
        SegmentKind::Choice,
        enabled,
        parent,
        surface,
        window,
        cx,
        vec![],
        choose,
    )
}

fn render_segmented(
    id: SharedString,
    width: f32,
    options: Vec<Segment>,
    selected: Option<usize>,
    kind: SegmentKind,
    enabled: bool,
    parent: u32,
    surface: &super::Surface,
    window: &mut Window,
    cx: &mut App,
    hints: Vec<Option<SharedString>>,
    choose: ControlCallback<usize>,
) -> Stateful<Div> {
    use crate::automation::{AutomationElementExt, AutomationRole};
    let p = CUE_UI.palette;
    let count = options.len().max(1);
    let active: Vec<_> = options
        .iter()
        .enumerate()
        .filter_map(|(i, option)| (enabled && !option.disabled).then_some(i))
        .collect();
    let handles: Vec<_> = options
        .iter()
        .map(|option| action_focus(option.id.clone(), window, cx))
        .collect();
    let entry = active
        .iter()
        .copied()
        .find(|i| handles[*i].is_focused(window))
        .or_else(|| selected.filter(|i| active.contains(i)))
        .or_else(|| active.first().copied());
    let mut row = div()
        .absolute()
        .left(px(SEGMENT_INSET))
        .top_0()
        .w(px(width - 2. * SEGMENT_INSET))
        .h(px(CONTROL_HEIGHT))
        .flex()
        .gap(px(SEGMENT_GAP));
    for (
        index,
        Segment {
            id: key,
            label,
            disabled,
        },
    ) in options.into_iter().enumerate()
    {
        let callback = choose.clone();
        let enabled = enabled && !disabled;
        let focus = handles[index]
            .clone()
            .tab_stop(enabled && Some(index) == entry);
        let checked = selected == Some(index);
        let hint_focus = focus.clone();
        let hint_id = format!("{key}-hint");
        row = row.child(
            div()
                .id(key)
                .w(px(segment_pose(width, count, index).w as f32))
                .h_full()
                .track_focus(&focus)
                .tab_stop(enabled && Some(index) == entry)
                .role(if matches!(kind, SegmentKind::Tabs { .. }) {
                    Role::Tab
                } else {
                    Role::Button
                })
                .aria_label(label.clone())
                .when(matches!(kind, SegmentKind::Tabs { .. }), |v| {
                    v.aria_selected(checked)
                })
                .when(!matches!(kind, SegmentKind::Tabs { .. }), |v| {
                    v.aria_toggled(if checked {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                })
                .a11y_synthetic_children(move |builder| {
                    if !enabled {
                        builder.parent_node().set_disabled();
                    }
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .when(enabled, |v| {
                    v.cursor_pointer().focus_visible(|v| v.underline())
                })
                .when(!enabled, |v| v.opacity(0.4).cursor_default())
                .child(if selected.is_some() {
                    surface
                        .content_clip()
                        .contrast_label(label.clone(), p.text, p.canvas)
                        .into_any_element()
                } else {
                    div()
                        .text_color(rgb(p.text))
                        .child(label.clone())
                        .into_any_element()
                })
                .on_click(move |_, window, cx| {
                    if enabled {
                        window.focus(&focus, cx);
                        callback(&index, window, cx);
                    }
                })
                .automation_enabled(enabled, AutomationRole::Button, label.to_string())
                .map(|control| {
                    if let Some(hint) = hints.get(index).cloned().flatten() {
                        crate::components::tooltip::hint(control, hint_id, hint.to_string())
                            .focus_handle(&hint_focus)
                            .into_any_element()
                    } else {
                        control.into_any_element()
                    }
                }),
        );
    }
    div()
        .id(id.clone())
        .role(if matches!(kind, SegmentKind::Tabs { .. }) {
            Role::TabList
        } else {
            Role::Group
        })
        .relative()
        .w(px(width))
        .h(px(CONTROL_HEIGHT))
        .child(skin(
            format!("{id}-track"),
            width,
            CONTROL_HEIGHT,
            CONTROL_HEIGHT / 2.,
            0.6,
            SurfaceColors::outlined(LIQUID_OUTLINE, parent),
            div(),
            window,
            cx,
        ))
        .when(selected.is_some(), |v| {
            v.child(surface.background(p.accent, None))
        })
        .child(row)
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if active.is_empty() {
                return;
            }
            let current = active
                .iter()
                .position(|i| handles[*i].is_focused(window))
                .unwrap_or(0);
            let next = match event.keystroke.key.as_str() {
                "left" | "up" => Some((current + active.len() - 1) % active.len()),
                "right" | "down" => Some((current + 1) % active.len()),
                "home" => Some(0),
                "end" => Some(active.len() - 1),
                _ => None,
            };
            if let Some(next) = next {
                let index = active[next];
                window.focus(&handles[index], cx);
                if matches!(
                    kind,
                    SegmentKind::Choice | SegmentKind::Tabs { manual: false }
                ) {
                    choose(&index, window, cx);
                }
                window.prevent_default();
                cx.stop_propagation();
            }
        })
}

pub fn toggle<V: 'static>(
    id: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
    parent: u32,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let change: ControlCallback<bool> = std::rc::Rc::new(
        cx.listener(move |view, checked: &bool, _, cx| change(view, *checked, cx)),
    );
    with_control_surface(
        &id,
        toggle_pose(checked),
        1.,
        window,
        cx,
        |surface, window, cx| {
            render_toggle(
                id.clone(),
                checked,
                enabled,
                parent,
                None,
                surface,
                window,
                cx,
                change,
            )
        },
    )
}

pub fn toggle_with_surface<V: 'static>(
    id: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
    parent: u32,
    surface: &mut super::Surface,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
) -> Stateful<Div> {
    let change: ControlCallback<bool> = std::rc::Rc::new(
        cx.listener(move |view, checked: &bool, _, cx| change(view, *checked, cx)),
    );
    render_toggle(
        id.into(),
        checked,
        enabled,
        parent,
        None,
        surface,
        window,
        cx,
        change,
    )
}

fn render_toggle(
    id: SharedString,
    checked: bool,
    enabled: bool,
    parent: u32,
    focus: Option<&FocusHandle>,
    surface: &mut super::Surface,
    window: &mut Window,
    cx: &mut App,
    change: ControlCallback<bool>,
) -> Stateful<Div> {
    let p = CUE_UI.palette;
    let travel = ((surface.simulation.pose().cx - 16.) / 17.).clamp(0., 1.) as f32;
    let mix = |a, b| u32::from(crate::components::motion::mix_rgb(a, b, travel)) >> 8;
    let fill = if enabled {
        mix(p.selected, p.accent)
    } else {
        p.prompt
    };
    let focus = focus
        .cloned()
        .unwrap_or_else(|| action_focus(id.clone(), window, cx))
        .tab_stop(enabled);
    let focused = enabled && focus.is_focused(window) && window.last_input_was_keyboard();
    surface.set_border_width(crate::design::BORDER_WIDTH);
    div()
        .id(id.clone())
        .relative()
        .w(px(48.))
        .h(px(32.))
        .role(Role::Switch)
        .aria_label("开关")
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        .a11y_synthetic_children(move |builder| {
            if !enabled {
                builder.parent_node().set_disabled();
            }
        })
        .track_focus(&focus)
        .tab_stop(enabled)
        .when(enabled, |v| v.cursor_pointer())
        .when(!enabled, |v| v.cursor_default())
        .child(div().absolute().left(px(4.)).top(px(4.)).child(skin(
            format!("{id}-track"),
            41.,
            24.,
            12.,
            0.6,
            SurfaceColors::filled(fill, parent),
            div(),
            window,
            cx,
        )))
        .child(surface.background(if enabled { p.elevated } else { p.border_strong }, None))
        .child(
            crate::components::smooth::fill(format!("{id}-focus"), 16.)
                .border(gpui::px(crate::design::BORDER_WIDTH))
                .border_color(if focused {
                    rgb(INTERACTION.focus_border)
                } else {
                    rgba(0)
                }),
        )
        .on_click(move |_, window, cx| {
            if enabled {
                change(&!checked, window, cx);
            }
            cx.stop_propagation();
        })
}

/// Adapter for existing form builders that receive their Window only at layout.
/// It delegates to the same renderer and material model as the gallery toggle.
pub fn deferred_toggle(
    id: impl Into<SharedString>,
    checked: bool,
    enabled: bool,
    parent: u32,
    focus: FocusHandle,
    change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .w(px(48.))
        .h(px(32.))
        .flex_shrink_0()
        .child(DeferredToggle {
            id,
            checked,
            enabled,
            parent,
            focus,
            change: std::rc::Rc::new(change),
        })
}
#[derive(IntoElement)]
struct DeferredToggle {
    id: SharedString,
    checked: bool,
    enabled: bool,
    parent: u32,
    focus: FocusHandle,
    change: ControlCallback<bool>,
}
impl RenderOnce for DeferredToggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        with_control_surface(
            &self.id,
            toggle_pose(self.checked),
            1.,
            window,
            cx,
            |surface, window, cx| {
                render_toggle(
                    self.id.clone(),
                    self.checked,
                    self.enabled,
                    self.parent,
                    Some(&self.focus),
                    surface,
                    window,
                    cx,
                    self.change,
                )
            },
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum ButtonVariant {
    Solid,
    Soft,
    Outline,
    Ghost,
}

#[derive(Clone, Copy, Default, PartialEq)]
pub struct ActionStyle {
    pub variant: Option<ButtonVariant>,
    pub primary: bool,
    pub quiet: bool,
    pub selected: bool,
    pub disabled: bool,
    pub busy: bool,
    pub icon: Option<&'static str>,
    pub image: Option<&'static str>,
    /// Select/field triggers share the action's pressure and focus machinery.
    pub field: bool,
    /// Full-width disclosure actions keep the label leading and the icon trailing.
    pub leading: bool,
    pub trailing: Option<&'static str>,
    pub expanded: bool,
    /// Icon triggers reveal an outline when they can separate into a panel.
    pub opens_panel: bool,
    pub radio: Option<bool>,
    /// Custom-content controls declare whether they have an icon-only face.
    pub icon_only: Option<bool>,
    /// Rich rows can retain their own corner family at any measured height.
    pub radius: Option<f32>,
    /// Navigation paints one travelling hover; this control keeps pressure/focus.
    pub hover_group: bool,
}

impl ActionStyle {
    fn resolved(mut self) -> Self {
        if let Some(variant) = self.variant {
            self.primary = variant == ButtonVariant::Solid;
            self.quiet = variant == ButtonVariant::Ghost;
        }
        self
    }
}

pub fn action_focus(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> FocusHandle {
    let id = id.into();
    let focus = window.use_keyed_state(format!("liquid-focus-{id:?}"), cx, |_, cx| {
        cx.focus_handle()
    });
    focus.read(cx).clone().tab_stop(true)
}

pub fn action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    width: f32,
    height: f32,
    style: ActionStyle,
    parent: u32,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id = id.into();
    let label = label.into();
    let focus = action_focus(id.clone(), window, cx).tab_stop(!style.disabled && !style.busy);
    // GPUI's subtree paint cache does not key inherited opacity. Keep the
    // control's geometry/text caches, but render its scene under the current
    // ancestor state so fades and restored triggers cannot retain alpha zero.
    render_action(id, label, width, height, style, parent, focus, window, cx)
}

fn render_action(
    id: ElementId,
    label: SharedString,
    width: f32,
    height: f32,
    style: ActionStyle,
    parent: u32,
    focus: FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    render_action_content(
        id, label, width, height, style, parent, focus, None, None, window, cx,
    )
}

fn render_action_content(
    id: ElementId,
    label: SharedString,
    width: f32,
    height: f32,
    style: ActionStyle,
    parent: u32,
    focus: FocusHandle,
    content: Option<(AnyElement, super::ContentClipBinding)>,
    source: Option<super::render::SourceMaterial>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let adaptive = content.is_some();
    let style = style.resolved();
    let soft = style.variant == Some(ButtonVariant::Soft) || (style.quiet && style.selected);
    let enabled = !style.disabled && !style.busy;
    let group: SharedString = format!("liquid-action-{id:?}").into();
    let icon_only = style.icon_only.unwrap_or(label.is_empty() && !style.field);
    let primary_icon = icon_only && style.primary;
    let solid = style.primary && !style.field;
    let background_feedback =
        solid || soft || style.quiet || (icon_only && !style.opens_panel) || style.radio.is_some();
    let hover = window.use_keyed_state(format!("liquid-action-hover-{id:?}"), cx, |_, _| false);
    let hovered = enabled && *hover.read(cx);
    let color = action_ink(&label, style);
    let radius = style.radius.unwrap_or_else(|| {
        if style.field {
            crate::controls::FIELD_RADIUS
        } else if width == height && !primary_icon {
            height / 2. - 4.
        } else {
            height / 2.
        }
    });
    let pressed_color = if solid {
        INTERACTION.accent_pressed
    } else {
        INTERACTION.neutral_pressed
    };
    let build =
        |clip: Option<super::ContentClip>, pressed: bool, window: &mut Window, cx: &mut App| {
            let mut contents = div()
                .size_full()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(7.))
                .text_size(px(if style.field { 13. } else { 12. }))
                .text_color(rgb(color))
                .whitespace_nowrap();
            if enabled && background_feedback {
                let hover = HoverFill {
                    id: format!("liquid-hover-{id:?}").into(),
                    color: if solid {
                        INTERACTION.accent_hover
                    } else {
                        INTERACTION.neutral_hover
                    },
                    radius: 0.,
                    pressed: None,
                };
                if let Some(clip) = &clip {
                    if !style.hover_group {
                        let fill = clip.fill(
                            format!("liquid-hover-fill-{id:?}"),
                            radius as f64,
                            None,
                            None,
                            window,
                            cx,
                        );
                        contents = contents.child(hover.render_with(fill, window, cx));
                    }
                    if pressed {
                        contents = contents.child(
                            div()
                                .id(format!("liquid-press-fill-{id:?}"))
                                .absolute()
                                .inset_0()
                                .child(clip.fill(
                                    format!("liquid-press-path-{id:?}"),
                                    radius as f64,
                                    Some(pressed_color),
                                    None,
                                    window,
                                    cx,
                                )),
                        );
                    }
                } else {
                    contents = contents
                        .when(!style.hover_group, |contents| contents.child(hover))
                        .when(pressed, |contents| {
                            contents.child(div().absolute().inset_0().bg(rgb(pressed_color)))
                        });
                }
            }
            contents = if let Some((ink, binding)) = content {
                // The root keeps the entire control's hit region. Clip each flow
                // child at its own measured height, including multiline rows and
                // portraits; geometry probes remain outside these ink regions.
                if let Some(clip) = clip {
                    binding.bind(clip);
                }
                contents.child(ink)
            } else {
                let ink = action_content(&id, label.clone(), height, style).into_any_element();
                if let Some(clip) = clip {
                    contents.child(
                        clip.content(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(ink),
                            height.min(20.),
                        ),
                    )
                } else {
                    contents.child(ink)
                }
            };
            contents
                .id(action_content_scope(&id))
                .on_hover(move |value, _, cx| {
                    hover.update(cx, |hovered, cx| {
                        if *hovered != *value {
                            *hovered = *value;
                            cx.notify();
                        }
                    });
                })
                .into_any_element()
        };
    let colors = SurfaceColors {
        fill: if solid {
            if style.disabled {
                INTERACTION.neutral_pressed
            } else {
                BRAND_ACCENT
            }
        } else if soft {
            CUE_UI.palette.selected
        } else {
            parent
        },
        border: if soft
            || solid
            || style.quiet
            || style.radio.is_some()
            || (icon_only && !(style.opens_panel && (hovered || style.expanded)))
        {
            None
        } else if style.expanded || style.selected {
            Some(crate::controls::FIELD_FOCUS_BORDER)
        } else if hovered {
            Some(crate::controls::FIELD_HOVER_BORDER)
        } else {
            Some(LIQUID_OUTLINE)
        },
        parent,
        focused: enabled && focus.is_focused(window),
    };
    let control = super::press::content_surface_with_source(
        id.clone(),
        width,
        height,
        radius,
        0.6,
        colors,
        !soft && ((icon_only && !primary_icon) || style.quiet || style.radio.is_some()),
        build,
        enabled,
        source,
        adaptive,
        window,
        cx,
    );
    control
        .when(!adaptive, |v| {
            v.role(Role::Button)
                .aria_label(label)
                .a11y_synthetic_children(move |builder| {
                    if !enabled {
                        builder.parent_node().set_disabled();
                    }
                })
        })
        .when(style.busy && !adaptive, |v| v.aria_description("正在处理"))
        .group(group)
        .when(!adaptive, |v| v.track_focus(&focus).tab_stop(enabled))
        .when(enabled, |v| v.cursor_pointer())
        .when(!enabled, |v| v.cursor_default())
        .capture_any_mouse_down(move |_, _, cx| {
            if !enabled {
                cx.stop_propagation();
            }
        })
        .capture_key_down(move |_, _, cx| {
            if !enabled {
                cx.stop_propagation();
            }
        })
}

fn action_content_scope(id: &ElementId) -> String {
    format!("liquid-action-hover-zone-{id:?}")
}

fn action_ink(_label: &str, style: ActionStyle) -> u32 {
    let style = style.resolved();
    let p = CUE_UI.palette;
    if style.disabled {
        p.subtle
    } else if style.primary && !style.field {
        p.canvas
    } else {
        p.text
    }
}

/// Crisp labels and icons also ride the same material during a dialog morph.
pub(crate) fn action_content(
    id: &ElementId,
    label: SharedString,
    height: f32,
    style: ActionStyle,
) -> Stateful<Div> {
    let p = CUE_UI.palette;
    let color = action_ink(&label, style);
    let ink = div()
        .id(format!("liquid-action-ink-{id:?}"))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(7.))
        .when(style.field || style.leading, |v| {
            v.w_full().px_3().justify_start()
        })
        .when_some(style.radio, |v, selected| {
            v.child(
                div()
                    .size(px(14.))
                    .flex_shrink_0()
                    .rounded_full()
                    .border(gpui::px(crate::design::BORDER_WIDTH))
                    .border_color(rgb(if selected { color } else { p.subtle }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |v| {
                        v.child(div().size(px(6.)).rounded_full().bg(rgb(color)))
                    }),
            )
        })
        .when_some(style.icon, |v, icon| {
            v.child(
                crate::controls::icon(icon, if height <= 24. { 14. } else { 15. })
                    .text_color(rgb(color)),
            )
        })
        .when_some(style.image, |v, path| {
            v.child(gpui::img(path).size(px(18.)).flex_shrink_0())
        })
        .when(!label.is_empty(), |v| {
            v.child(
                div()
                    .min_w_0()
                    .when(style.field || style.leading, |v| v.flex_1().truncate())
                    .child(label.clone()),
            )
        })
        .when_some(style.trailing, |v, path| {
            v.child(crate::controls::icon(path, 12.).with_spring(
                "action-chevron",
                crate::components::motion::spring(if style.expanded { 1. } else { 0. }),
                |v, t| {
                    v.with_transformation(Transformation::rotate(radians(std::f32::consts::PI * t)))
                },
            ))
        })
        .text_size(px(if style.field || style.leading {
            13.
        } else {
            12.
        }))
        .text_color(rgb(color))
        .whitespace_nowrap()
        .when(style.busy, |v| v.opacity(0.));
    div()
        .id(format!("liquid-action-content-{id:?}"))
        .relative()
        .when(style.field || style.leading, |v| v.w_full())
        .child(ink)
        .when(style.busy, |v| {
            v.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::components::loading::indicator(format!("{id:?}-loading"), 14.)
                            .without_delay(),
                    ),
            )
        })
}

/// Press feedback is independent of the hover fade, including a quick press
/// before the hover spring has advanced. Keyboard activation uses this group too.
pub fn press_fill(group: SharedString, color: u32) -> Stateful<Div> {
    div()
        .id(format!("liquid-press-{group}"))
        .absolute()
        .inset_0()
        .opacity(0.)
        .bg(rgb(color))
        .group_active(group, |style| style.opacity(1.))
}

/// Menu labels share one leading edge. Selection belongs in the trailing slot;
/// action menus pass `None` and do not acquire single-choice semantics.
pub(super) const MENU_INSET: f32 = 6.;
pub(super) const MENU_ROW_PADDING: f32 = 8.;
pub(super) const MENU_ROW_HEIGHT: f32 = 32.;
pub(super) const MENU_ROW_GAP: f32 = 2.;
pub(super) const MENU_MIN_WIDTH: f32 = 120.;

pub(super) fn menu_row_radius(outer_radius: f64, width: f64) -> f64 {
    super::inset_radius(
        outer_radius,
        MENU_INSET as f64,
        width,
        MENU_ROW_HEIGHT as f64,
    )
}

/// A grouped menu paints its single moving hover underneath rows.
pub enum MenuSurface {
    Local(u32),
    /// A static popup shares one moving hover across its rows.
    StaticGroup {
        radius: f64,
    },
    Grouped {
        clip: super::ContentClip,
        radius: f64,
    },
}

pub fn menu_item(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    width: f32,
    checked: Option<bool>,
    enabled: bool,
    surface: MenuSurface,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    menu_item_with_icon(
        id, label, width, checked, enabled, None, surface, window, cx,
    )
}

pub fn menu_item_with_icon(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    width: f32,
    checked: Option<bool>,
    enabled: bool,
    icon: Option<&'static str>,
    surface: MenuSurface,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id = id.into();
    let p = CUE_UI.palette;
    let (parent, clip, radius, grouped) = match surface {
        MenuSurface::Local(parent) => (parent, None, 10., false),
        MenuSurface::StaticGroup { radius } => (p.canvas, None, radius, true),
        MenuSurface::Grouped { clip, radius } => (p.canvas, Some(clip), radius, true),
    };
    let focus = action_focus(id.clone(), window, cx).tab_stop(enabled);
    let group: SharedString = format!("liquid-menu-{id:?}").into();
    let fill = parent;
    let contents = div()
        .size_full()
        .relative()
        .flex()
        .items_center()
        .px(px(if clip.is_some() {
            MENU_ROW_PADDING
        } else {
            12.
        }))
        .gap_2()
        .text_size(px(12.))
        .text_color(rgb(if enabled { p.text } else { p.subtle }))
        .text_align(TextAlign::Left)
        .whitespace_nowrap()
        .when_some(icon, |v, path| {
            v.child(gpui::img(path).relative().size(px(18.)).flex_shrink_0())
        })
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(label.into()),
        )
        .when_some(checked, |v, checked| {
            v.child(
                div()
                    .relative()
                    .w(px(14.))
                    .h(px(14.))
                    .flex_shrink_0()
                    .opacity(if checked { 1. } else { 0. })
                    .child(crate::controls::icon("icons/check.svg", 14.)),
            )
        });
    let mut layer = div().size_full().absolute().inset_0();
    if enabled {
        let hover = HoverFill {
            id: format!("liquid-menu-hover-{id:?}").into(),
            color: INTERACTION.neutral_hover,
            radius: 0.,
            pressed: Some((group.clone(), INTERACTION.neutral_pressed)),
        };
        layer = if let Some(clip) = &clip {
            layer.child(
                div()
                    .id(format!("{id:?}-pressed"))
                    .absolute()
                    .inset_0()
                    .opacity(0.)
                    .text_color(rgb(INTERACTION.neutral_pressed))
                    .group_active(group.clone(), |v| v.opacity(1.))
                    .child(clip.fill(
                        format!("{id:?}-pressed-fill"),
                        radius,
                        None,
                        None,
                        window,
                        cx,
                    )),
            )
        } else if grouped {
            layer.child(
                div()
                    .id(format!("{id:?}-grouped-press"))
                    .absolute()
                    .inset_0()
                    .opacity(0.)
                    .group_active(group.clone(), |v| v.opacity(1.))
                    .child(
                        crate::components::smooth::fill(
                            format!("{id:?}-pressed-fill"),
                            radius as f32,
                        )
                        .bg(rgb(INTERACTION.neutral_pressed)),
                    ),
            )
        } else {
            layer
                .child(hover)
                .child(press_fill(group.clone(), INTERACTION.neutral_pressed))
        };
    }
    let control = if let Some(clip) = clip {
        let stroke = (enabled && focus.is_focused(window) && window.last_input_was_keyboard())
            .then_some(INTERACTION.focus_border);
        div()
            .id(id.clone())
            .relative()
            .w(px(width))
            .h(px(MENU_ROW_HEIGHT))
            .flex_shrink_0()
            .when(stroke.is_some(), |v| {
                v.child(clip.fill(
                    format!("{id:?}-selected-fill"),
                    radius,
                    Some(fill),
                    stroke,
                    window,
                    cx,
                ))
            })
            .child(layer)
            .child(clip.content(contents, 22.))
    } else if grouped {
        div()
            .id(id.clone())
            .relative()
            .w(px(width))
            .h(px(MENU_ROW_HEIGHT))
            .flex_shrink_0()
            .when(
                enabled && focus.is_focused(window) && window.last_input_was_keyboard(),
                |v| {
                    v.child(
                        crate::components::smooth::fill(format!("{id:?}-focus"), radius as f32)
                            .border(px(crate::design::BORDER_WIDTH))
                            .border_color(rgb(INTERACTION.focus_border)),
                    )
                },
            )
            .child(layer)
            .child(contents)
    } else {
        skin(
            id,
            width,
            32.,
            10.,
            0.6,
            SurfaceColors {
                fill,
                border: None,
                parent,
                focused: enabled && focus.is_focused(window),
            },
            layer.child(contents),
            window,
            cx,
        )
    };
    control
        .group(group)
        .track_focus(&focus)
        .tab_stop(enabled)
        .when(enabled, |v| v.cursor_pointer())
        .capture_any_mouse_down(move |_, _, cx| {
            if !enabled {
                cx.stop_propagation();
            }
        })
        .capture_key_down(move |_, _, cx| {
            if !enabled {
                cx.stop_propagation();
            }
        })
}

pub fn input(
    id: impl Into<ElementId>,
    input: &Entity<ComposerInput>,
    width: f32,
    height: f32,
    invalid: bool,
    parent: u32,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    input_content(
        id.into(),
        input,
        width,
        height,
        invalid,
        parent,
        None,
        window,
        cx,
    )
}

fn input_content(
    id: ElementId,
    input: &Entity<ComposerInput>,
    width: f32,
    height: f32,
    invalid: bool,
    parent: u32,
    content: Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let focus = input.read(cx).focus_handle();
    let target = input.clone();
    skin(
        id,
        width,
        height,
        12.,
        0.6,
        SurfaceColors {
            fill: if invalid {
                crate::design::FORM.error_surface
            } else {
                parent
            },
            border: Some(if invalid { 0xC9837E } else { LIQUID_OUTLINE }),
            parent,
            focused: focus.is_focused(window),
        },
        content.unwrap_or_else(|| {
            div()
                .absolute()
                .left(px(12.))
                .top(px(5.))
                .w(px((width - 24.).max(2.)))
                .h(px((height - 10.).max(2.)))
                .line_height(px(20.))
                .text_size(px(13.))
                .child(input.clone())
                .into_any_element()
        }),
        window,
        cx,
    )
    .track_focus(&focus)
    .on_click(move |_, w, cx| w.focus(&target.read(cx).focus_handle(), cx))
}

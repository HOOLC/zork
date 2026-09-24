//! Reusable control surfaces. Hosts provide actions, validation and busy state.
use super::{skin, SurfaceColors};
use crate::{
    components::text_input::ComposerInput,
    controls::CONTROL_HEIGHT,
    design::{FORM, INTERACTION, ZORK_UI},
};
use gpui::{prelude::*, *};
mod adaptive;
pub use adaptive::{adaptive_action, adaptive_input, Action, Field};

/// Shared focus binding for controls and
/// for ordinary compositional rows used as overlay sources.
pub trait ControlElement: Element + StatefulInteractiveElement + ParentElement + Styled {
    fn control_focus(self, focus: &FocusHandle) -> Self;
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
    fn control_overlay(self, overlay: AnyElement) -> Self {
        self.overlay(overlay)
    }
}
const SEGMENT_INSET: f32 = 4.;
const SEGMENT_GAP: f32 = 2.;

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
            action(
                key,
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
                window,
                cx,
            )
            .track_focus(&focus)
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

/// Optional selection, disabled items and manual tabs share one static track.
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
    render_segmented(
        id,
        width,
        options,
        selected,
        kind,
        enabled,
        parent,
        window,
        cx,
        vec![],
        choose,
    )
}

/// Responsive forms use the same segmented control and keyboard selection as
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
            render_segmented(
                id, width, options, selected, kind, enabled, parent, window, cx, hints, choose,
            )
            .into_any_element()
        },
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
    window: &mut Window,
    cx: &mut App,
    hints: Vec<Option<SharedString>>,
    choose: ControlCallback<usize>,
) -> Stateful<Div> {
    use crate::automation::{AutomationElementExt, AutomationRole};
    let p = ZORK_UI.palette;
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
        .top(px(SEGMENT_INSET))
        .w(px(width - 2. * SEGMENT_INSET))
        .h(px(CONTROL_HEIGHT - 2. * SEGMENT_INSET))
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
                .w(px(((width
                    - 2. * SEGMENT_INSET
                    - SEGMENT_GAP * count.saturating_sub(1) as f32)
                    / count.max(1) as f32)
                    .max(2.)))
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
                .when(checked, |v| {
                    v.rounded(px(CONTROL_HEIGHT / 2. - SEGMENT_INSET))
                        .bg(rgb(p.accent))
                })
                .child(
                    div()
                        .text_color(rgb(if checked { p.canvas } else { p.text }))
                        .child(label.clone()),
                )
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
            SurfaceColors::outlined(FORM.outline, parent),
            div(),
            window,
            cx,
        ))
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
    let change: ControlCallback<bool> = std::rc::Rc::new(
        cx.listener(move |view, checked: &bool, _, cx| change(view, *checked, cx)),
    );
    render_toggle(
        id.into(),
        checked,
        enabled,
        parent,
        None,
        window,
        cx,
        change,
    )
}

fn render_toggle(
    id: SharedString,
    checked: bool,
    enabled: bool,
    _parent: u32,
    focus: Option<&FocusHandle>,
    window: &mut Window,
    cx: &mut App,
    change: ControlCallback<bool>,
) -> Stateful<Div> {
    let focus = focus
        .cloned()
        .unwrap_or_else(|| action_focus(id.clone(), window, cx));
    let focused = enabled && focus.is_focused(window) && window.last_input_was_keyboard();
    let track = if checked {
        ZORK_UI.palette.accent
    } else {
        FORM.switch_off
    };
    // A dark resting thumb would sink into the dark off track.
    let thumb = if enabled && !checked && crate::design::theme() == crate::design::Theme::Dark {
        ZORK_UI.palette.muted
    } else if enabled {
        ZORK_UI.palette.elevated
    } else {
        ZORK_UI.palette.border_strong
    };
    let switch = gpui_base::Switch::new(format!("{id}-switch"))
        .checked(checked)
        .disabled(!enabled)
        .track_focus(&focus)
        .accessibility_label("开关")
        .on_change(move |next, _, window, cx| change(&next, window, cx))
        .relative()
        .w(px(48.))
        .h(px(32.))
        .when(enabled, |v| v.cursor_pointer())
        .child(
            div()
                .absolute()
                .left(px(4.))
                .top(px(4.))
                .w(px(41.))
                .h(px(24.))
                .rounded(px(12.))
                .bg(rgb(track))
                .when(focused, |v| v.shadow(crate::controls::focus_ring())),
        )
        .child(
            div()
                .absolute()
                .top(px(7.))
                .left(px(if checked { 24. } else { 7. }))
                .size(px(18.))
                .rounded(px(9.))
                .bg(rgb(thumb)),
        )
;
    div().id(id).w(px(48.)).h(px(32.)).child(switch)
}

/// Form builders provide a focus handle before layout.
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
        render_toggle(
            self.id,
            self.checked,
            self.enabled,
            self.parent,
            Some(&self.focus),
            window,
            cx,
            self.change,
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum ButtonVariant {
    Solid,
    Danger,
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
    /// A select trigger keeps its surface stable and emphasizes its border.
    pub select_trigger: bool,
    /// Full-width disclosure actions keep the label leading and the icon trailing.
    pub leading: bool,
    pub trailing: Option<&'static str>,
    pub expanded: bool,
    pub radio: Option<bool>,
    /// Custom-content controls declare whether they have an icon-only face.
    pub icon_only: Option<bool>,
    /// Rich rows can retain their own corner family at any measured height.
    pub radius: Option<f32>,
    /// A primary action that starts work (sending) keeps the persimmon accent.
    pub accent: bool,
}

impl ActionStyle {
    fn resolved(mut self) -> Self {
        if let Some(variant) = self.variant {
            self.primary = matches!(variant, ButtonVariant::Solid | ButtonVariant::Danger);
            self.quiet = variant == ButtonVariant::Ghost;
        }
        self
    }
}

pub fn action_focus(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> FocusHandle {
    let id = id.into();
    let focus = window.use_keyed_state(format!("widget-focus-{id:?}"), cx, |_, cx| {
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
) -> Action {
    let id = id.into();
    let focus = action_focus(id.clone(), window, cx).tab_stop(!style.disabled && !style.busy);
    adaptive_action(id, label, style, parent)
        .track_focus(&focus)
        .w(px(width))
        .min_h_0()
        .py_0()
        .h(px(height))
}

fn action_ink(_label: &str, style: ActionStyle) -> u32 {
    let style = style.resolved();
    let p = ZORK_UI.palette;
    if style.disabled {
        crate::design::FORM.disabled_text
    } else if style.primary && !style.select_trigger {
        p.canvas
    } else {
        p.text
    }
}

/// Icon and label content inside the native action button.
pub(crate) fn action_content(
    id: &ElementId,
    label: SharedString,
    height: f32,
    style: ActionStyle,
) -> Stateful<Div> {
    let p = ZORK_UI.palette;
    let color = action_ink(&label, style);
    let ink = div()
        .id(format!("widget-action-ink-{id:?}"))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(7.))
        .when(style.select_trigger || style.leading, |v| {
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
                    .when(style.select_trigger || style.leading, |v| {
                        v.flex_1().truncate()
                    })
                    .child(label.clone()),
            )
        })
        .when_some(style.trailing, |v, path| {
            let icon = crate::controls::icon(path, 12.);
            v.child(if style.expanded {
                icon.with_transformation(Transformation::rotate(radians(std::f32::consts::PI)))
                    .into_any_element()
            } else {
                icon.into_any_element()
            })
        })
        .text_color(rgb(color))
        .whitespace_nowrap()
        .when(style.busy, |v| v.opacity(0.));
    div()
        .id(format!("widget-action-content-{id:?}"))
        .relative()
        .when(style.select_trigger || style.leading, |v| v.w_full())
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

/// Fixed layout slots use the same field as content-sized production forms.
pub fn input(
    id: impl Into<ElementId>,
    input: &Entity<ComposerInput>,
    width: f32,
    height: f32,
    invalid: bool,
    parent: u32,
    _window: &mut Window,
    _cx: &mut App,
) -> Stateful<Div> {
    let id = id.into();
    let field = adaptive_input(id.clone(), input, invalid, parent)
        .w(px(width))
        .min_h_0()
        .h(px(height));
    div().id(id).w(px(width)).h(px(height)).child(field)
}

use super::super::{
    controls::{self, ActionStyle},
    skin, SurfaceColors,
};
use super::{disabled_node, surface};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::{BRAND_ACCENT, LIQUID_OUTLINE, ZORK_UI},
};
use gpui::{prelude::*, *};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Checked {
    Off,
    On,
    Mixed,
}
impl Checked {
    pub fn toggled(self) -> Toggled {
        match self {
            Self::Off => Toggled::False,
            Self::On => Toggled::True,
            Self::Mixed => Toggled::Mixed,
        }
    }
    pub fn next(self) -> Self {
        if self == Self::On {
            Self::Off
        } else {
            Self::On
        }
    }
    pub fn aggregate(values: &[bool]) -> Self {
        if values.iter().all(|v| !v) {
            Self::Off
        } else if values.iter().all(|v| *v) {
            Self::On
        } else {
            Self::Mixed
        }
    }
}

pub fn checkbox<V: 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    checked: Checked,
    disabled: bool,
    parent: u32,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, Checked, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let label = label.into();
    let focus = controls::action_focus(id.clone(), window, cx).tab_stop(!disabled);
    let mark = div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .when(checked != Checked::Off, |v| {
            v.child(
                crate::controls::icon(
                    if checked == Checked::Mixed {
                        "icons/minus.svg"
                    } else {
                        "icons/check.svg"
                    },
                    12.,
                )
                .text_color(rgb(ZORK_UI.palette.canvas)),
            )
        });
    let square = skin(
        format!("{id}-mark"),
        18.,
        18.,
        5.,
        0.6,
        SurfaceColors {
            fill: if checked == Checked::Off {
                parent
            } else {
                BRAND_ACCENT
            },
            border: (checked == Checked::Off).then_some(LIQUID_OUTLINE),
            parent,
            focused: checked == Checked::Off && focus.is_focused(window),
        },
        mark,
        window,
        cx,
    );
    div()
        .id(id.clone())
        .track_focus(&focus)
        .tab_stop(!disabled)
        .role(Role::CheckBox)
        .aria_label(label.clone())
        .aria_toggled(checked.toggled())
        .a11y_synthetic_children(move |builder| disabled_node(builder.parent_node(), disabled))
        .flex()
        .items_center()
        .gap(px(9.))
        .min_h(px(32.))
        .text_size(px(13.))
        .line_height(px(20.))
        .when(disabled, |v| v.opacity(0.4))
        .when(!disabled, |v| v.cursor_pointer())
        .focus_visible(|v| v.underline())
        .child(square)
        .child(div().min_w_0().child(label.clone()))
        .on_click(cx.listener(move |v, _, w, cx| {
            if !disabled {
                w.focus(&focus, cx);
                change(v, checked.next(), cx);
            }
        }))
        .automation_enabled(!disabled, AutomationRole::Button, label)
        .into_any_element()
}

#[derive(Clone)]
pub struct Item {
    pub id: String,
    pub label: SharedString,
    pub description: Option<SharedString>,
    pub disabled: bool,
}
impl Item {
    pub fn new(id: impl Into<String>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            disabled: false,
        }
    }
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Radio,
    Checkboxes,
    SingleToggle,
    MultipleToggle,
}
impl Mode {
    pub fn multiple(self) -> bool {
        matches!(self, Self::Checkboxes | Self::MultipleToggle)
    }
    pub fn toggle(self) -> bool {
        matches!(self, Self::SingleToggle | Self::MultipleToggle)
    }
    pub fn change(self, values: &[usize], index: usize) -> Vec<usize> {
        let mut next = if self.multiple() {
            values.to_vec()
        } else {
            vec![]
        };
        if values.contains(&index) && self != Self::Radio {
            next.retain(|i| *i != index);
        } else {
            next.push(index);
        }
        next.sort_unstable();
        next.dedup();
        next
    }
}

/// Choice cards, checkbox groups and toggle groups share disabled-item traversal.
/// Radio arrows select immediately; toggle arrows only move focus.
pub fn group<V: 'static>(
    id: impl Into<SharedString>,
    items: Vec<Item>,
    selected: Vec<usize>,
    mode: Mode,
    cards: bool,
    width: f32,
    disabled: bool,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, Vec<usize>, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let group_width = if mode.toggle() {
        (items
            .iter()
            .map(|item| super::super::overlay::measure_label(&item.label, 12., window) + 24.)
            .fold(0f32, f32::max)
            * items.len() as f32
            + 8.
            + 2. * items.len().saturating_sub(1) as f32)
            .min(width)
    } else {
        width
    };
    if mode == Mode::SingleToggle && !cards {
        return controls::segments(
            id,
            group_width,
            items
                .into_iter()
                .map(|item| controls::Segment {
                    id: item.id,
                    label: item.label,
                    disabled: item.disabled,
                })
                .collect(),
            selected.first().copied(),
            controls::SegmentKind::Toggle,
            !disabled,
            ZORK_UI.palette.canvas,
            window,
            cx,
            move |v, i, cx| change(v, mode.change(&selected, i), cx),
        )
        .into_any_element();
    }
    let active: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| !disabled && !i.disabled)
        .map(|(i, _)| i)
        .collect();
    let handles: Vec<_> = items
        .iter()
        .map(|item| controls::action_focus(item.id.clone(), window, cx))
        .collect();
    let entry = active
        .iter()
        .copied()
        .find(|i| handles[*i].is_focused(window))
        .or_else(|| selected.iter().copied().find(|i| active.contains(i)))
        .or_else(|| active.first().copied());
    for (i, handle) in handles.iter().enumerate() {
        handle
            .clone()
            .tab_stop(active.contains(&i) && (mode == Mode::Checkboxes || Some(i) == entry));
    }
    let change = Rc::new(change);
    let item_width =
        (group_width - 8. - 2. * items.len().saturating_sub(1) as f32) / items.len().max(1) as f32;
    let mut root = div()
        .id(id.clone())
        .relative()
        .w(px(group_width))
        .flex()
        .gap(px(8.))
        .role(if mode == Mode::Radio {
            Role::RadioGroup
        } else {
            Role::Group
        });
    if cards || !mode.toggle() {
        root = root.flex_col();
    } else {
        root = root.h(px(32.)).p(px(4.)).gap(px(2.));
    }
    for (index, item) in items.into_iter().enumerate() {
        let checked = selected.contains(&index);
        let inert = disabled || item.disabled;
        let focus = handles[index].clone();
        let values = selected.clone();
        let callback = change.clone();
        let mark = if mode.toggle() {
            None
        } else {
            Some(skin(
                format!("{}-mark", item.id),
                18.,
                18.,
                if mode == Mode::Radio { 9. } else { 5. },
                0.6,
                SurfaceColors {
                    fill: if checked {
                        BRAND_ACCENT
                    } else {
                        if cards {
                            ZORK_UI.palette.prompt
                        } else {
                            ZORK_UI.palette.canvas
                        }
                    },
                    border: (!checked).then_some(LIQUID_OUTLINE),
                    parent: if cards {
                        ZORK_UI.palette.prompt
                    } else {
                        ZORK_UI.palette.canvas
                    },
                    focused: false,
                },
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(checked, |v| {
                        v.child(
                            crate::controls::icon("icons/check.svg", 12.)
                                .text_color(rgb(ZORK_UI.palette.canvas)),
                        )
                    }),
                window,
                cx,
            ))
        };
        let text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(item.label.clone())
            .when_some(item.description, |v, text| {
                v.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(ZORK_UI.palette.muted))
                        .child(text),
                )
            });
        let row = if mode.toggle() {
            controls::action(
                item.id.clone(),
                item.label.clone(),
                item_width,
                24.,
                ActionStyle {
                    primary: checked,
                    quiet: !checked,
                    disabled: inert,
                    ..Default::default()
                },
                ZORK_UI.palette.canvas,
                window,
                cx,
            )
        } else {
            surface(
                item.id.clone(),
                crate::controls::CARD_RADIUS,
                if cards {
                    ZORK_UI.palette.prompt
                } else {
                    ZORK_UI.palette.canvas
                },
                false,
            )
            .when(cards, |v| v.p(px(18.)))
            .min_h(px(32.))
            .flex()
            .items_center()
            .gap(px(10.))
            .text_size(px(13.))
            .line_height(px(20.))
            .when(inert, |v| v.opacity(0.4))
            .when(!inert, |v| v.cursor_pointer())
            .when_some(mark, |v, mark| v.child(mark))
            .child(text)
        }
        .role(if mode == Mode::Radio {
            Role::RadioButton
        } else if mode == Mode::Checkboxes {
            Role::CheckBox
        } else {
            Role::Button
        })
        .aria_label(item.label.clone())
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        .a11y_synthetic_children(move |builder| disabled_node(builder.parent_node(), inert))
        .track_focus(&focus)
        .tab_stop(!inert && (mode == Mode::Checkboxes || Some(index) == entry))
        .focus_visible(|v| v.underline())
        .on_click(cx.listener(move |v, _, w, cx| {
            if !inert {
                w.focus(&focus, cx);
                callback(v, mode.change(&values, index), cx);
            }
        }))
        .automation_enabled(!inert, AutomationRole::Option, item.label);
        root = root.child(row);
    }
    if mode.toggle() {
        root = root.child(
            crate::components::smooth::fill(format!("{id}-outline"), 16.)
                .border(px(crate::design::BORDER_WIDTH))
                .border_color(rgb(LIQUID_OUTLINE)),
        );
    }
    root.on_key_down(cx.listener(move |v, e: &KeyDownEvent, w, cx| {
        if mode == Mode::Checkboxes || active.is_empty() {
            return;
        }
        let current = active
            .iter()
            .position(|i| handles[*i].is_focused(w))
            .unwrap_or(0);
        let next = match e.keystroke.key.as_str() {
            "left" | "up" => (current + active.len() - 1) % active.len(),
            "right" | "down" => (current + 1) % active.len(),
            "home" => 0,
            "end" => active.len() - 1,
            _ => return,
        };
        let index = active[next];
        w.focus(&handles[index], cx);
        if mode == Mode::Radio {
            change(v, vec![index], cx);
        }
        w.prevent_default();
        cx.stop_propagation();
    }))
    .into_any_element()
}

pub fn toggle<V: 'static>(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    pressed: bool,
    disabled: bool,
    width: f32,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let text = text.into();
    controls::action(
        id,
        text.clone(),
        width,
        32.,
        ActionStyle {
            primary: pressed,
            disabled,
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
        window,
        cx,
    )
    .aria_toggled(if pressed {
        Toggled::True
    } else {
        Toggled::False
    })
    .on_click(cx.listener(move |v, _, _, cx| {
        if !disabled {
            change(v, !pressed, cx);
        }
    }))
    .automation_enabled(!disabled, AutomationRole::Button, text)
    .into_any_element()
}

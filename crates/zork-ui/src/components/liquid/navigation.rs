//! Liquid navigation, tabs and selectable rows. Consumers provide destinations;
//! the component owns hover travel, selection motion, focus and frame lifetime.
use super::{
    controls,
    overlay::{FrameSample, Motion},
    skin, Material, Pose, SurfaceColors,
};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::CUE_UI,
};
use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc};

mod group;
mod visible_rows;
pub use group::{Group, GroupSurface};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sidebar,
    Tabs,
    Rows,
    Actions,
}

pub struct Item {
    pub id: String,
    pub label: SharedString,
    pub detail: Option<SharedString>,
    pub gap_before: f32,
    pub heading: bool,
    pub trailing: Option<AnyElement>,
}
impl Item {
    pub fn new(id: impl Into<String>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            gap_before: 0.,
            heading: false,
            trailing: None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Style {
    pub kind: Kind,
    pub framed: bool,
    pub parent: u32,
    pub activate_on_arrow: bool,
    pub row_radius: f32,
}

pub struct Layout {
    pub panel: Pose,
    pub rows: Vec<Pose>,
    pub height: f32,
}
pub fn layout(width: f32, items: &[Item], style: Style) -> Layout {
    let margin = if style.framed { 18. } else { 0. };
    let panel_radius = if style.kind == Kind::Tabs {
        crate::controls::COMPACT_CARD_RADIUS
    } else {
        crate::controls::CARD_RADIUS
    } as f64;
    let inset = if style.framed {
        super::row_inset(panel_radius, 6., 32.)
    } else {
        0.
    };
    let width = width as f64;
    let inner = (width - 2. * (margin + inset)).max(2.);
    let mut y = margin + inset;
    let rows = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let height = if item.detail.is_some() {
                56.
            } else if style.kind == Kind::Tabs {
                36.
            } else {
                32.
            };
            let row_radius = if style.framed {
                super::inset_radius(panel_radius, inset, inner, height)
            } else {
                super::inset_radius(style.row_radius as f64, 0., inner, height)
            };
            if style.kind == Kind::Tabs {
                let slot = inner / items.len().max(1) as f64;
                Pose::rect(
                    margin + inset + i as f64 * slot,
                    y,
                    slot,
                    height,
                    row_radius,
                )
            } else {
                if i > 0 {
                    y += if item.gap_before > 0. {
                        item.gap_before as f64
                    } else if item.detail.is_some() {
                        6.
                    } else if style.framed {
                        4.
                    } else {
                        2.
                    };
                } else {
                    y += item.gap_before as f64;
                }
                let row = Pose::rect(margin + inset, y, inner, height, row_radius);
                y += height;
                row
            }
        })
        .collect();
    let height = if style.kind == Kind::Tabs {
        2. * margin + 48.
    } else {
        y + inset + margin
    };
    Layout {
        panel: Pose::rect(
            margin,
            margin,
            width - 2. * margin,
            height - 2. * margin,
            panel_radius,
        ),
        rows,
        height: height as f32,
    }
}

/// Fixed gallery layouts and application trees use the same group and rows.
pub struct Navigation {
    group: Option<Group>,
}
impl Navigation {
    pub fn new() -> Self {
        Self { group: None }
    }
    pub fn samples(&self) -> Vec<FrameSample> {
        self.group.as_ref().map_or_else(Vec::new, Group::samples)
    }
    pub fn reset_samples(&mut self) {
        if let Some(group) = &self.group {
            group.reset_samples();
        }
    }
    pub fn visible(&self) -> bool {
        self.group.as_ref().is_some_and(Group::visible)
    }
    pub fn surfaces(&self) -> Vec<serde_json::Value> {
        self.group.as_ref().map_or_else(Vec::new, Group::surfaces)
    }
    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        width: f32,
        items: Vec<Item>,
        selected: usize,
        enabled: bool,
        style: Style,
        material: Material,
        window: &mut Window,
        cx: &mut Context<V>,
        choose: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
    ) -> AnyElement {
        let id = id.into();
        if items.is_empty() {
            return div().into_any_element();
        }
        let layout = layout(width, &items, style);
        let selected = selected.min(items.len() - 1);
        let group = self.group.get_or_insert_with(|| Group::new(cx));
        group.configure(style, material);
        let hot = group.hovered().and_then(|id| {
            items
                .iter()
                .position(|item| ElementId::from(item.id.clone()) == id)
        });
        if !enabled || hot.is_none() {
            group.set_hover(None, cx);
        }
        let focus: Vec<_> = items
            .iter()
            .map(|item| controls::action_focus(item.id.clone(), window, cx).tab_stop(enabled))
            .collect();
        let keyboard_row = focus
            .iter()
            .position(|focus| focus.contains_focused(window, cx))
            .filter(|_| window.last_input_was_keyboard());
        let p = CUE_UI.palette;
        let mut panel = div()
            .id(id.clone())
            .relative()
            .w(px(width))
            .h(px(layout.height))
            .overflow_hidden();
        if style.framed {
            panel = panel.child(
                div()
                    .absolute()
                    .left(px(layout.panel.left() as f32))
                    .top(px(layout.panel.top() as f32))
                    .child(skin(
                        format!("{id}-base"),
                        layout.panel.w as f32,
                        layout.panel.h as f32,
                        layout.panel.r as f32,
                        0.6,
                        SurfaceColors::filled(p.canvas, style.parent),
                        div(),
                        window,
                        cx,
                    )),
            );
        }
        let choose: Rc<dyn Fn(&mut V, usize, &mut Context<V>)> = Rc::new(choose);
        let ids: Vec<_> = items.iter().map(|item| item.id.clone()).collect();
        let rows = if style.kind != Kind::Tabs
            && items.len() > 24
            && items.iter().all(|item| item.trailing.is_none())
            && !window.is_a11y_active()
        {
            visible_rows::Rows::new(
                id.clone(),
                items,
                layout.rows.clone(),
                width,
                layout.height,
                group.clone(),
                focus.clone(),
                style,
                enabled,
                selected,
                hot,
                keyboard_row,
                choose.clone(),
                cx.entity().downgrade(),
            )
            .into_any_element()
        } else {
            let mut rows = div().size_full().relative();
            for (i, item) in items.into_iter().enumerate() {
                rows = rows.child(navigation_row(
                    id.clone(),
                    item,
                    layout.rows[i],
                    group,
                    &focus[i],
                    style,
                    enabled,
                    selected == i,
                    hot == Some(i) || keyboard_row == Some(i),
                    i,
                    choose.clone(),
                    cx,
                ));
            }
            rows.into_any_element()
        };
        let group = group.clone();
        let keyboard_group = group.clone();
        let count = focus.len();
        panel
            .child(group.surface(rows))
            .on_key_down(cx.listener(move |v, e: &KeyDownEvent, w, cx| {
                if !enabled {
                    return;
                }
                let current = focus
                    .iter()
                    .position(|f| f.is_focused(w))
                    .unwrap_or(selected);
                let next = match e.keystroke.key.as_str() {
                    "up" | "left" => Some((current + count - 1) % count),
                    "down" | "right" => Some((current + 1) % count),
                    "home" => Some(0),
                    "end" => Some(count - 1),
                    _ => None,
                };
                if let Some(next) = next {
                    keyboard_group.set_hover(Some(ids[next].clone().into()), cx);
                    w.focus(&focus[next], cx);
                    if style.activate_on_arrow {
                        choose(v, next, cx);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }
}

fn navigation_row<V: 'static>(
    navigation_id: SharedString,
    item: Item,
    pose: Pose,
    group: &Group,
    focus: &FocusHandle,
    style: Style,
    enabled: bool,
    selected: bool,
    highlighted: bool,
    i: usize,
    choose: Rc<dyn Fn(&mut V, usize, &mut Context<V>)>,
    cx: &mut Context<V>,
) -> AnyElement {
    let p = CUE_UI.palette;
    let id = navigation_id;
    let item_key = item.id.clone();
    let activate_focus = focus.clone();
    let scroll_focus = focus.clone();
    group
        .row(item.id, selected, enabled)
        .absolute()
        .left(px(pose.left() as f32))
        .top(px(pose.top() as f32))
        .w(px(pose.w as f32))
        .h(px(pose.h as f32))
        .px_3()
        .track_focus(&focus)
        .child(
            gpui::canvas(
                move |bounds, window, _| {
                    if scroll_focus.is_focused(window) && window.last_input_was_keyboard() {
                        window.request_autoscroll(bounds);
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .when(style.kind != Kind::Tabs, |v| v.flex_1())
                .child(
                    div()
                        .truncate()
                        .when(item.heading, |v| v.font_weight(FontWeight::MEDIUM))
                        .child(item.label.clone()),
                )
                .when_some(item.detail, |v, detail| {
                    v.child(div().text_color(rgb(p.subtle)).child(detail))
                }),
        )
        .when(style.kind == Kind::Rows, |v| {
            v.child(
                div()
                    .size(px(14.))
                    .flex_shrink_0()
                    .opacity(if selected { 1. } else { 0. })
                    .child(crate::controls::icon("icons/check.svg", 14.)),
            )
        })
        .when_some(item.trailing, |v, trailing| {
            v.child(div().flex_shrink_0().child(trailing).with_spring(
                format!("{id}-trailing-{}", item_key),
                crate::components::motion::spring(if enabled && highlighted { 1. } else { 0. }),
                |v, alpha| v.opacity(alpha),
            ))
        })
        .on_click(cx.listener(move |v, _, w, cx| {
            if enabled {
                w.focus(&activate_focus, cx);
                choose(v, i, cx);
            }
        }))
        .automation_enabled(enabled, AutomationRole::Button, item.label.to_string())
        .into_any_element()
}

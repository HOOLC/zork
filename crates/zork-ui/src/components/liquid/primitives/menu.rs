use super::super::{
    controls::{self, ActionStyle, ControlElement},
    overlay::{MeasuredAnchor, Motion},
    Material, Pose,
};
use super::{disabled_node, surface};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{prelude::*, *};
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
#[cfg(target_family = "wasm")]
use web_time::Instant;

#[derive(Clone, Copy, PartialEq)]
pub enum ItemKind {
    Action,
    Check(bool),
    Radio(bool),
    Heading,
    Separator,
}
#[derive(Clone)]
pub struct Item {
    pub key: String,
    pub label: SharedString,
    pub kind: ItemKind,
    pub disabled: bool,
    pub shortcut: Option<SharedString>,
    pub children: Vec<Item>,
    pub keep_open: bool,
    pub text_value: Option<SharedString>,
    pub icon: Option<&'static str>,
    pub detail: Option<SharedString>,
}
impl Item {
    pub fn new(key: impl Into<String>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ItemKind::Action,
            disabled: false,
            shortcut: None,
            children: vec![],
            keep_open: false,
            text_value: None,
            icon: None,
            detail: None,
        }
    }
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
    pub fn icon(mut self, path: &'static str) -> Self {
        self.icon = Some(path);
        self
    }
    pub fn detail(mut self, text: impl Into<SharedString>) -> Self {
        self.detail = Some(text.into());
        self
    }
    pub fn check(mut self, checked: bool) -> Self {
        self.kind = ItemKind::Check(checked);
        self.keep_open = true;
        self
    }
    pub fn radio(mut self, checked: bool) -> Self {
        self.kind = ItemKind::Radio(checked);
        self
    }
    pub fn shortcut(mut self, text: impl Into<SharedString>) -> Self {
        self.shortcut = Some(text.into());
        self
    }
    pub fn text_value(mut self, text: impl Into<SharedString>) -> Self {
        self.text_value = Some(text.into());
        self
    }
    pub fn submenu(mut self, items: Vec<Self>) -> Self {
        self.children = items;
        self
    }
    pub fn heading(key: impl Into<String>, label: impl Into<SharedString>) -> Self {
        Self {
            kind: ItemKind::Heading,
            ..Self::new(key, label)
        }
    }
    pub fn separator(key: impl Into<String>) -> Self {
        Self {
            kind: ItemKind::Separator,
            ..Self::new(key, "")
        }
    }
    fn active(&self) -> bool {
        !self.disabled
            && matches!(
                self.kind,
                ItemKind::Action | ItemKind::Check(_) | ItemKind::Radio(_)
            )
    }
    fn height(&self) -> f32 {
        match self.kind {
            ItemKind::Heading => 26.,
            ItemKind::Separator => 9.,
            _ => {
                if self.detail.is_some() {
                    48.
                } else {
                    32.
                }
            }
        }
    }
}

struct State {
    open: bool,
    origin: Point<Pixels>,
    source: Option<FocusHandle>,
    source_bounds: Option<Bounds<Pixels>>,
    path: Vec<usize>,
    focus_pending: Option<usize>,
    query: String,
    typed: Option<Instant>,
    generation: u64,
    press: Option<Point<Pixels>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            origin: point(px(0.), px(0.)),
            source: None,
            source_bounds: None,
            path: vec![],
            focus_pending: None,
            query: String::new(),
            typed: None,
            generation: 0,
            press: None,
        }
    }
}
impl State {
    fn show(&mut self, origin: Point<Pixels>, source: FocusHandle) {
        self.open = true;
        self.origin = origin;
        self.source = Some(source);
        self.source_bounds = None;
        self.path.clear();
        self.focus_pending = Some(0);
        self.query.clear();
        self.generation = self.generation.wrapping_add(1);
    }
    fn track_source(&mut self, bounds: Bounds<Pixels>) -> bool {
        if let Some(previous) = self.source_bounds {
            self.source_bounds = Some(bounds);
            self.origin += bounds.origin - previous.origin;
            previous != bounds
        } else {
            false
        }
    }
    fn close(&mut self, w: &mut Window, cx: &mut App) {
        self.open = false;
        self.path.clear();
        self.generation = self.generation.wrapping_add(1);
        if let Some(source) = &self.source {
            w.focus(source, cx);
        }
    }
}

/// The same nested menu implements context menus and menubar popups. Only
/// navigation state is retained; checked/disabled values are read each render.
pub struct Menu {
    state: Rc<RefCell<State>>,
    bounds: Rc<RefCell<Vec<Bounds<Pixels>>>>,
    anchor: MeasuredAnchor,
    material: Rc<RefCell<Vec<Motion>>>,
    retained: Rc<RefCell<Vec<Vec<Item>>>>,
}
impl Default for Menu {
    fn default() -> Self {
        Self {
            state: Default::default(),
            bounds: Default::default(),
            anchor: Default::default(),
            material: Default::default(),
            retained: Default::default(),
        }
    }
}
impl Menu {
    pub fn is_open(&self) -> bool {
        self.state.borrow().open
    }
    pub fn inspect(&self) -> serde_json::Value {
        let state = self.state.borrow();
        serde_json::json!({"open":state.open,"path":state.path,"query":state.query, "materials":self.material.borrow().iter().map(Motion::inspect).collect::<Vec<_>>()})
    }
    pub fn close(&self, w: &mut Window, cx: &mut App) {
        self.state.borrow_mut().close(w, cx);
    }
    pub fn open_at(&self, position: Point<Pixels>, focus: FocusHandle) {
        self.anchor
            .bounds
            .set(Bounds::new(position, size(px(1.), px(1.))));
        self.anchor.visible.set(true);
        self.state.borrow_mut().show(position, focus);
    }
    pub fn trigger<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        width: f32,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let id = id.into();
        let label = label.into();
        let focus = controls::action_focus(id.clone(), window, cx);
        let element = controls::action(
            id,
            label.clone(),
            width,
            32.,
            ActionStyle {
                expanded: self.is_open(),
                opens_panel: true,
                ..Default::default()
            },
            ZORK_UI.palette.canvas,
            window,
            cx,
        );
        self.trigger_element(element, &focus, true, cx)
            .automation(AutomationRole::Button, label)
            .into_any_element()
    }
    /// Bind an icon, a rich row or an intrinsic action to the same menu lifecycle.
    pub fn trigger_element<V: 'static, E: ControlElement>(
        &self,
        element: E,
        focus: &FocusHandle,
        enabled: bool,
        cx: &mut Context<V>,
    ) -> E {
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let capture = bounds.clone();
        let focus = focus.clone();
        let key_focus = focus.clone();
        let state = self.state.clone();
        let keys = self.state.clone();
        let key_bounds = bounds.clone();
        let layout_state = self.state.clone();
        let anchor = self.anchor.clone();
        let owner = cx.entity().downgrade();
        element
            .control_focus(&focus)
            .panel_source()
            .aria_expanded(self.is_open())
            .on_click(cx.listener(move |_, _, w, cx| {
                if !enabled {
                    return;
                }
                let mut state = state.borrow_mut();
                if state.open {
                    state.close(w, cx);
                } else {
                    state.show(
                        bounds.get().bottom_left() + point(px(0.), px(6.)),
                        focus.clone(),
                    );
                    state.source_bounds = Some(bounds.get());
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(move |_, e: &KeyDownEvent, w, cx| {
                if enabled && matches!(e.keystroke.key.as_str(), "down" | "up") {
                    let mut state = keys.borrow_mut();
                    state.show(
                        key_bounds.get().bottom_left() + point(px(0.), px(6.)),
                        key_focus.clone(),
                    );
                    state.source_bounds = Some(key_bounds.get());
                    cx.notify();
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .control_overlay(
                canvas(
                    move |b, w, cx| {
                        capture.set(b);
                        let changed = layout_state.borrow_mut().track_source(b);
                        if anchor.update(b, w) || changed {
                            let _ = owner.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .into_any_element(),
            )
    }
    /// Host navigation can dismiss without moving focus away from its new target.
    pub fn dismiss(&self) {
        let mut state = self.state.borrow_mut();
        state.open = false;
        state.path.clear();
        state.generation = state.generation.wrapping_add(1);
    }
    pub fn context_trigger<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        content: impl IntoElement,
        width: f32,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let id = id.into();
        let focus = controls::action_focus(id.clone(), window, cx);
        let element = surface(
            id,
            crate::controls::CARD_RADIUS,
            ZORK_UI.palette.prompt,
            false,
        )
        .w(px(width))
        .min_h(px(100.))
        .p(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .role(Role::Group)
        .aria_label("上下文菜单区域")
        .child(content);
        self.context_element(element, &focus, true, cx)
            .automation(AutomationRole::Button, "右键或长按打开菜单")
            .into_any_element()
    }
    /// Bind the full context-menu interaction to an existing control's actual
    /// geometry. Images and rich rows keep their original content and size.
    pub fn context_element<V: 'static, E: ControlElement>(
        &self,
        element: E,
        focus: &FocusHandle,
        enabled: bool,
        cx: &mut Context<V>,
    ) -> E {
        let focus = focus.clone();
        let rect = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let capture = rect.clone();
        let right = self.state.clone();
        let right_focus = focus.clone();
        let left = self.state.clone();
        let left_focus = focus.clone();
        let release = self.state.clone();
        let moved = self.state.clone();
        let keys = self.state.clone();
        let layout_state = self.state.clone();
        let anchor = self.anchor.clone();
        let owner = cx.entity().downgrade();
        element
            .control_focus(&focus)
            .aria_description("右键、长按或 Shift F10 打开菜单")
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_, e: &MouseDownEvent, w, cx| {
                    if !enabled {
                        return;
                    }
                    right.borrow_mut().show(e.position, right_focus.clone());
                    w.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, e: &MouseDownEvent, w, cx| {
                    if !enabled {
                        return;
                    }
                    let generation = {
                        let mut state = left.borrow_mut();
                        state.generation = state.generation.wrapping_add(1);
                        state.press = Some(e.position);
                        state.generation
                    };
                    let state = left.clone();
                    let focus = left_focus.clone();
                    let position = e.position;
                    cx.spawn_in(w, async move |owner, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(550))
                            .await;
                        let _ = cx.update(|_, cx| {
                            if state.borrow().generation == generation
                                && state.borrow().press.is_some()
                            {
                                state.borrow_mut().show(position, focus);
                                let _ = owner.update(cx, |_, cx| cx.notify());
                            }
                        });
                    })
                    .detach();
                }),
            )
            .on_mouse_move(move |e: &MouseMoveEvent, _, _| {
                let mut state = moved.borrow_mut();
                if state
                    .press
                    .is_some_and(|p| (p - e.position).magnitude() > 6.)
                {
                    state.press = None;
                    state.generation = state.generation.wrapping_add(1);
                }
            })
            .on_mouse_up_out(MouseButton::Left, {
                let release = release.clone();
                move |_, _, _| {
                    let mut state = release.borrow_mut();
                    state.press = None;
                    state.generation = state.generation.wrapping_add(1);
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, _| {
                let mut state = release.borrow_mut();
                state.press = None;
                state.generation = state.generation.wrapping_add(1);
            })
            .on_key_down(cx.listener(move |_, e: &KeyDownEvent, w, cx| {
                if enabled
                    && (e.keystroke.key == "menu"
                        || (e.keystroke.key == "f10" && e.keystroke.modifiers.shift))
                {
                    keys.borrow_mut()
                        .show(rect.get().bottom_left(), focus.clone());
                    cx.notify();
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .control_overlay(
                canvas(
                    move |b, w, cx| {
                        capture.set(b);
                        let mut state = layout_state.borrow_mut();
                        let changed = if state.open {
                            let prior = anchor.bounds.get();
                            state.origin += b.origin - prior.origin;
                            prior != b
                        } else {
                            false
                        };
                        if anchor.update(b, w) || changed {
                            let _ = owner.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .into_any_element(),
            )
    }
    pub fn render<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        items: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<V>,
        choose: impl Fn(&mut V, String, &mut Window, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        let open = self.is_open();
        if !open && !self.material.borrow().iter().any(Motion::alive) {
            self.anchor.reset_placement();
            return None;
        }
        let id = id.into();
        let choose = Rc::new(choose);
        let viewport = window.viewport_size();
        let state = self.state.borrow();
        let path = state.path.clone();
        let origin = state.origin;
        drop(state);
        let mut levels = vec![items];
        for index in &path {
            let Some(item) = levels.last().and_then(|items| items.get(*index)) else {
                break;
            };
            if item.children.is_empty() || !item.active() {
                break;
            }
            levels.push(item.children.clone());
        }
        let active_levels = if open { levels.len() } else { 0 };
        if open {
            let mut retained = self.retained.borrow_mut();
            for (depth, items) in levels.iter().enumerate() {
                if depth < retained.len() {
                    retained[depth] = items.clone();
                } else {
                    retained.push(items.clone());
                }
            }
        }
        levels = self.retained.borrow().clone();
        let mut materials = self.material.borrow_mut();
        while materials.len() < levels.len() {
            materials.push(Motion::travelling());
        }
        let anchor_origin = self.anchor.bounds.get().origin;
        let scopes: Vec<Vec<FocusHandle>> = levels
            .iter()
            .enumerate()
            .map(|(depth, items)| {
                items
                    .iter()
                    .map(|item| {
                        controls::action_focus(format!("{id}-{depth}-{}", item.key), window, cx)
                            .tab_stop(false)
                    })
                    .collect()
            })
            .collect();
        if let Some(depth) = self.state.borrow_mut().focus_pending.take() {
            if let Some((index, _)) = levels
                .get(depth)
                .and_then(|items| items.iter().enumerate().find(|(_, item)| item.active()))
            {
                window.focus(&scopes[depth][index], cx);
            }
        }
        self.bounds
            .borrow_mut()
            .resize(levels.len(), Bounds::default());
        let mut layers = div().relative().w(viewport.width).h(viewport.height);
        let mut origin = origin;
        let outside = self.state.clone();
        let outside_bounds = self.bounds.clone();
        for (depth, items) in levels.into_iter().enumerate() {
            let level_open = depth < active_levels;
            let mut group =
                super::super::navigation::Group::keyed(format!("{id}-{depth}-hover"), window, cx);
            group.configure(
                super::super::navigation::Style {
                    kind: super::super::navigation::Kind::Actions,
                    framed: false,
                    parent: ZORK_UI.palette.canvas,
                    activate_on_arrow: false,
                    row_radius: crate::controls::MENU_RADIUS - 6.,
                },
                Material::default(),
            );
            let width = (items
                .iter()
                .map(|item| {
                    super::super::overlay::measure_label(&item.label, 12., window).max(
                        item.detail.as_ref().map_or(0., |detail| {
                            super::super::overlay::measure_label(detail, 11., window)
                        }),
                    ) + item.shortcut.as_ref().map_or(0., |text| {
                        super::super::overlay::measure_label(text, 11., window) + 20.
                    })
                })
                .fold(120_f32, f32::max)
                + 52.)
                .min(viewport.width.as_f32() - 24.);
            let content_height: f32 = items.iter().map(Item::height).sum();
            let height = (content_height + 12.).min(viewport.height.as_f32() - 24.);
            let (x, y, height) =
                if let Some(source) = self.state.borrow().source_bounds.filter(|_| depth == 0) {
                    let fitted = self
                        .anchor
                        .fit(source, size(px(width), px(height)), viewport, 6.);
                    (
                        fitted.bounds.origin.x.as_f32(),
                        fitted.bounds.origin.y.as_f32(),
                        fitted.bounds.size.height.as_f32(),
                    )
                } else {
                    (
                        origin
                            .x
                            .as_f32()
                            .clamp(12., (viewport.width.as_f32() - width - 12.).max(12.)),
                        origin
                            .y
                            .as_f32()
                            .clamp(12., (viewport.height.as_f32() - height - 12.).max(12.)),
                        height,
                    )
                };
            let target = Pose::rect(
                (px(x) - anchor_origin.x).as_f32() as f64,
                (px(y) - anchor_origin.y).as_f32() as f64,
                width as f64,
                height as f64,
                crate::controls::MENU_RADIUS as f64,
            );
            let from = Pose::rect(target.left(), target.top(), target.w, 2., 1.);
            let motion = &mut materials[depth];
            motion.frame(
                from,
                target,
                false,
                level_open,
                Material::default(),
                self.anchor.visible.get(),
                window,
                cx,
            );
            if !level_open && !motion.alive() {
                continue;
            }
            let surface = motion.surface.as_ref().unwrap();
            let current = surface.simulation.pose();
            let clip = surface.content_clip();
            let alpha = super::super::overlay::reveal(motion.progress());
            let mut rows = div()
                .id(format!("{id}-{depth}-scroll"))
                .w(px(width - 12.))
                .max_h(px(height - 12.))
                .overflow_y_scroll()
                .flex()
                .flex_col();
            let active: Vec<_> = items
                .iter()
                .enumerate()
                .filter(|(_, i)| i.active())
                .map(|(i, _)| i)
                .collect();
            let mut y_offset = 0.;
            for (index, item) in items.iter().cloned().enumerate() {
                if path.get(depth) == Some(&index) {
                    origin = point(
                        px(if x + width + 6. + width <= viewport.width.as_f32() - 12. {
                            x + width + 6.
                        } else {
                            (x - width - 6.).max(12.)
                        }),
                        px(y + 6. + y_offset),
                    );
                }
                y_offset += item.height();
                let item_id = format!("{id}-{depth}-{}", item.key);
                if item.kind == ItemKind::Separator {
                    rows = rows.child(
                        clip.content(
                            div().h(px(9.)).py(px(4.)).child(
                                div()
                                    .h(px(crate::design::BORDER_WIDTH))
                                    .w_full()
                                    .bg(rgb(ZORK_UI.palette.border)),
                            ),
                            crate::design::BORDER_WIDTH,
                        ),
                    );
                    continue;
                }
                if item.kind == ItemKind::Heading {
                    rows = rows.child(
                        clip.content(
                            div()
                                .px(px(12.))
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .text_size(px(11.))
                                .text_color(rgb(ZORK_UI.palette.muted))
                                .child(item.label),
                            14.,
                        ),
                    );
                    continue;
                }
                let state = self.state.clone();
                let hover = self.state.clone();
                let callback = choose.clone();
                let has_children = !item.children.is_empty();
                let focus = scopes[depth][index].clone();
                let click_focus = focus.clone();
                let disabled = item.disabled || !level_open;
                let checked = match item.kind {
                    ItemKind::Check(v) | ItemKind::Radio(v) => Some(v),
                    _ => None,
                };
                let mut row = group
                    .row(item_id, false, !disabled)
                    .w(px(width - 12.))
                    .h(px(item.height()))
                    .px(px(12.))
                    .when_some(item.icon, |row, icon| {
                        row.child(crate::controls::icon(icon, 16.))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().truncate().child(item.label.clone()))
                            .when_some(item.detail.clone(), |v, detail| {
                                v.child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.))
                                        .line_height(px(16.))
                                        .text_color(rgb(ZORK_UI.palette.muted))
                                        .child(detail),
                                )
                            }),
                    )
                    .when_some(checked, |row, checked| {
                        row.child(
                            div()
                                .size(px(14.))
                                .flex_shrink_0()
                                .opacity(if checked { 1. } else { 0. })
                                .child(crate::controls::icon("icons/check.svg", 14.)),
                        )
                    })
                    .role(match item.kind {
                        ItemKind::Check(_) => Role::MenuItemCheckBox,
                        ItemKind::Radio(_) => Role::MenuItemRadio,
                        _ => Role::MenuItem,
                    })
                    .aria_label(item.label.clone())
                    .track_focus(&focus)
                    .tab_stop(false)
                    .when_some(checked, |v, checked| {
                        v.aria_toggled(if checked {
                            Toggled::True
                        } else {
                            Toggled::False
                        })
                    })
                    .when(has_children, |v| {
                        v.aria_expanded(path.get(depth) == Some(&index))
                    })
                    .a11y_synthetic_children(move |b| disabled_node(b.parent_node(), disabled));
                if has_children || item.shortcut.is_some() {
                    row = row.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(rgb(ZORK_UI.palette.muted))
                            .child(if has_children {
                                crate::controls::icon("icons/chevron-right.svg", 12.)
                                    .into_any_element()
                            } else {
                                item.shortcut.clone().unwrap().into_any_element()
                            }),
                    );
                }
                let key = item.key.clone();
                let keep_open = item.keep_open;
                rows = rows.child(
                    row.on_hover(cx.listener(move |_, inside: &bool, w, cx| {
                        if disabled {
                            return;
                        }
                        if !*inside {
                            return;
                        }
                        w.focus(&focus, cx);
                        let mut state = hover.borrow_mut();
                        state.path.truncate(depth);
                        if has_children {
                            state.path.push(index);
                        }
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if disabled {
                            return;
                        }
                        w.focus(&click_focus, cx);
                        if has_children {
                            let mut state = state.borrow_mut();
                            state.path.truncate(depth);
                            state.path.push(index);
                            state.focus_pending = Some(depth + 1);
                        } else {
                            if !keep_open {
                                state.borrow_mut().close(w, cx);
                            }
                            callback(v, key.clone(), w, cx);
                        }
                        cx.notify();
                    }))
                    .automation_enabled(
                        !disabled,
                        AutomationRole::Option,
                        item.label,
                    ),
                );
            }
            let handles = scopes[depth].clone();
            let parent = depth
                .checked_sub(1)
                .and_then(|parent| path.get(parent).map(|index| scopes[parent][*index].clone()));
            let state = self.state.clone();
            let measured = self.bounds.clone();
            let panel = div()
                .id(format!("{id}-level-{depth}"))
                .absolute()
                .left(px(current.left() as f32))
                .top(px(current.top() as f32))
                .w(px(current.w as f32))
                .h(px(current.h as f32))
                .when(level_open, |v| surface.guard(v).occlude())
                .opacity(alpha)
                .role(Role::Menu)
                .aria_label("菜单")
                .child(
                    div()
                        .absolute()
                        .left(px(6.))
                        .top(px(6.))
                        .w(px(width - 12.))
                        .child(clip.content(group.surface(rows), height - 12.)),
                )
                .capture_any_mouse_down(move |_, _, cx| {
                    if !level_open {
                        cx.stop_propagation();
                    }
                })
                .on_key_down(cx.listener(move |_, e: &KeyDownEvent, w, cx| {
                    let current = active
                        .iter()
                        .position(|i| handles[*i].is_focused(w))
                        .unwrap_or(0);
                    let mut state = state.borrow_mut();
                    match e.keystroke.key.as_str() {
                        "escape" => {
                            state.close(w, cx);
                        }
                        "tab" => {
                            state.close(w, cx);
                            crate::modal::advance_focus(e.keystroke.modifiers.shift, w, cx);
                        }
                        "left" if depth > 0 => {
                            state.path.truncate(depth - 1);
                            if let Some(parent) = &parent {
                                w.focus(parent, cx);
                            }
                        }
                        "right" if !active.is_empty() => {
                            let index = active[current];
                            if !items[index].children.is_empty() {
                                state.path.truncate(depth);
                                state.path.push(index);
                                state.focus_pending = Some(depth + 1);
                            } else {
                                return;
                            }
                        }
                        "enter" | "space" => return,
                        "down" | "up" | "home" | "end" if !active.is_empty() => {
                            let next = match e.keystroke.key.as_str() {
                                "down" => (current + 1) % active.len(),
                                "up" => (current + active.len() - 1) % active.len(),
                                "home" => 0,
                                _ => active.len() - 1,
                            };
                            w.focus(&handles[active[next]], cx);
                        }
                        _ => {
                            let Some(text) = e.keystroke.key_char.as_deref().filter(|s| {
                                !s.is_empty()
                                    && !e.keystroke.modifiers.control
                                    && !e.keystroke.modifiers.platform
                            }) else {
                                return;
                            };
                            if state
                                .typed
                                .is_none_or(|at| at.elapsed() > Duration::from_millis(700))
                            {
                                state.query.clear();
                            }
                            state.typed = Some(Instant::now());
                            state.query.push_str(&text.to_lowercase());
                            let repeated = state
                                .query
                                .chars()
                                .all(|ch| Some(ch) == state.query.chars().next());
                            let query = if repeated {
                                text.to_lowercase()
                            } else {
                                state.query.clone()
                            };
                            if let Some(index) = (1..=active.len())
                                .map(|step| active[(current + step) % active.len()])
                                .find(|i| {
                                    items[*i].label.to_lowercase().starts_with(&query)
                                        || items[*i].text_value.as_ref().is_some_and(|text| {
                                            text.to_lowercase().starts_with(&query)
                                        })
                                })
                            {
                                w.focus(&handles[index], cx);
                            }
                        }
                    }
                    cx.notify();
                    w.prevent_default();
                    cx.stop_propagation();
                }))
                .child(
                    canvas(
                        move |b, _, _| measured.borrow_mut()[depth] = b,
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .automation_enabled(level_open, AutomationRole::Status, "菜单内容");
            layers = layers
                .child(surface.background(ZORK_UI.palette.canvas, None))
                .child(panel)
                .child(surface.background_colors(
                    None,
                    Some(crate::design::LIQUID_OUTLINE),
                    point(px(0.), px(0.)),
                    false,
                ));
        }
        let layers = layers.on_mouse_down_out(cx.listener(move |_, e: &MouseDownEvent, w, cx| {
            if open
                && !outside_bounds
                    .borrow()
                    .iter()
                    .any(|b| b.contains(&e.position))
                && !outside
                    .borrow()
                    .source_bounds
                    .is_some_and(|b| b.contains(&e.position))
            {
                outside.borrow_mut().close(w, cx);
                cx.notify();
            }
        }));
        Some(self.anchor.layer(layers, 220))
    }
}

pub struct Group {
    pub id: SharedString,
    pub label: SharedString,
    pub items: Vec<Item>,
}
impl Group {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        items: Vec<Item>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            items,
        }
    }
}
#[derive(Default)]
pub struct Menubar {
    menus: Vec<Menu>,
    material: Rc<RefCell<Vec<Motion>>>,
    anchor: MeasuredAnchor,
    presented: Option<usize>,
    retained: Rc<RefCell<Vec<Vec<Item>>>>,
}
impl Menubar {
    pub fn inspect(&self) -> serde_json::Value {
        serde_json::json!(self.menus.iter().map(Menu::inspect).collect::<Vec<_>>())
    }
    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        groups: Vec<Group>,
        window: &mut Window,
        cx: &mut Context<V>,
        choose: impl Fn(&mut V, String, &mut Window, &mut Context<V>) + 'static,
    ) -> AnyElement {
        let id = id.into();
        self.menus.resize_with(groups.len(), Menu::default);
        for menu in &mut self.menus {
            menu.material = self.material.clone();
            menu.anchor = self.anchor.clone();
            menu.retained = self.retained.clone();
        }
        let count = groups.len();
        let states: Vec<_> = self.menus.iter().map(|m| m.state.clone()).collect();
        let handles: Vec<_> = groups
            .iter()
            .map(|group| controls::action_focus(group.id.clone(), window, cx))
            .collect();
        let active = self.menus.iter().position(Menu::is_open);
        if active.is_some() {
            self.presented = active;
        }
        let presented = active.or(self.presented).filter(|i| *i < groups.len());
        let entry = handles
            .iter()
            .position(|h| h.is_focused(window))
            .or(active)
            .unwrap_or(0);
        let bounds: Vec<_> = groups
            .iter()
            .map(|group| {
                let state = window.use_keyed_state(format!("{}-bounds", group.id), cx, |_, _| {
                    Rc::new(Cell::new(Bounds::<Pixels>::default()))
                });
                state.read(cx).clone()
            })
            .collect();
        let callback = Rc::new(choose);
        let mut row = div()
            .id(id.clone())
            .role(Role::MenuBar)
            .flex()
            .flex_wrap()
            .gap(px(6.));
        for (index, group) in groups.into_iter().enumerate() {
            let rect = bounds[index].clone();
            let measured = rect.clone();
            let hover_rect = rect.clone();
            let states = states.clone();
            let hover_states = states.clone();
            let layout_state = states[index].clone();
            let layout_owner = cx.entity().downgrade();
            let focus = handles[index].clone().tab_stop(index == entry);
            let hover_focus = focus.clone();
            let button_width =
                super::super::overlay::measure_label(&group.label, 12., window) + 32.;
            let trigger = controls::action(
                group.id,
                group.label.clone(),
                button_width,
                32.,
                ActionStyle {
                    expanded: active == Some(index),
                    opens_panel: true,
                    ..Default::default()
                },
                ZORK_UI.palette.canvas,
                window,
                cx,
            )
            .role(Role::MenuItem)
            .aria_expanded(active == Some(index))
            .track_focus(&focus)
            .tab_stop(index == entry)
            .on_click(cx.listener(move |_, _, w, cx| {
                let was_open = states[index].borrow().open;
                for state in &states {
                    state.borrow_mut().open = false;
                }
                if !was_open {
                    let mut state = states[index].borrow_mut();
                    state.show(
                        rect.get().bottom_left() + point(px(0.), px(6.)),
                        focus.clone(),
                    );
                    state.source_bounds = Some(rect.get());
                } else {
                    w.focus(&focus, cx);
                }
                cx.notify();
            }))
            .on_hover(cx.listener(move |_, inside: &bool, _, cx| {
                if *inside
                    && hover_states.iter().any(|s| s.borrow().open)
                    && !hover_states[index].borrow().open
                {
                    for state in &hover_states {
                        state.borrow_mut().open = false;
                    }
                    let mut state = hover_states[index].borrow_mut();
                    state.show(
                        hover_rect.get().bottom_left() + point(px(0.), px(6.)),
                        hover_focus.clone(),
                    );
                    state.source_bounds = Some(hover_rect.get());
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |b, _, cx| {
                        measured.set(b);
                        if layout_state.borrow_mut().track_source(b) {
                            let _ = layout_owner.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .automation(AutomationRole::Button, group.label);
            row = row.child(trigger);
            let callback = callback.clone();
            if presented == Some(index) {
                if let Some(menu) = self.menus[index].render(
                    format!("{id}-menu-{index}"),
                    group.items,
                    window,
                    cx,
                    move |v, key, w, cx| callback(v, key, w, cx),
                ) {
                    row = row.child(menu);
                }
            }
        }
        let following = self
            .menus
            .iter()
            .any(|menu| menu.is_open() || menu.material.borrow().iter().any(Motion::alive));
        row = row.child(self.anchor.measure(following, cx));
        row.on_key_down(cx.listener(move |_, e: &KeyDownEvent, w, cx| {
            if count == 0 {
                return;
            }
            let current = handles
                .iter()
                .position(|h| h.is_focused(w))
                .or_else(|| states.iter().position(|s| s.borrow().open))
                .unwrap_or(0);
            let next = match e.keystroke.key.as_str() {
                "left" => (current + count - 1) % count,
                "right" => (current + 1) % count,
                "home" => 0,
                "end" => count - 1,
                "down" | "up" => current,
                _ => return,
            };
            let open = states.iter().any(|s| s.borrow().open)
                || matches!(e.keystroke.key.as_str(), "down" | "up");
            for state in &states {
                state.borrow_mut().open = false;
            }
            if open {
                let mut state = states[next].borrow_mut();
                state.show(
                    bounds[next].get().bottom_left() + point(px(0.), px(6.)),
                    handles[next].clone(),
                );
                state.source_bounds = Some(bounds[next].get());
            } else {
                w.focus(&handles[next], cx);
            }
            cx.notify();
            w.prevent_default();
            cx.stop_propagation();
        }))
        .into_any_element()
    }
}

/// Toolbar children keep their individual pressed states and share one Tab stop.
pub fn toolbar<V: 'static>(
    id: impl Into<SharedString>,
    items: Vec<Item>,
    window: &mut Window,
    cx: &mut Context<V>,
    activate: impl Fn(&mut V, String, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let callback = Rc::new(activate);
    let active: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.active())
        .map(|(i, _)| i)
        .collect();
    let handles: Vec<_> = items
        .iter()
        .map(|i| controls::action_focus(i.key.clone(), window, cx))
        .collect();
    let entry = active
        .iter()
        .copied()
        .find(|i| handles[*i].is_focused(window))
        .or_else(|| active.first().copied());
    let mut row = surface(
        id,
        crate::controls::COMPACT_CARD_RADIUS,
        ZORK_UI.palette.prompt,
        false,
    )
    .p(px(4.))
    .role(Role::Toolbar)
    .aria_label("格式工具栏")
    .flex()
    .flex_wrap()
    .gap(px(2.));
    for (i, item) in items.into_iter().enumerate() {
        if item.kind == ItemKind::Separator {
            row = row.child(
                div()
                    .w(px(crate::design::BORDER_WIDTH))
                    .h(px(24.))
                    .mx(px(4.))
                    .bg(rgb(ZORK_UI.palette.border)),
            );
            continue;
        }
        if item.kind == ItemKind::Heading {
            row = row.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child(item.label),
            );
            continue;
        }
        let focus = handles[i].clone().tab_stop(Some(i) == entry);
        let callback = callback.clone();
        let disabled = item.disabled;
        let pressed = match item.kind {
            ItemKind::Check(v) | ItemKind::Radio(v) => Some(v),
            _ => None,
        };
        row = row.child(
            controls::action(
                item.key.clone(),
                item.label.clone(),
                super::super::overlay::measure_label(&item.label, 12., window) + 24.,
                32.,
                ActionStyle {
                    primary: pressed == Some(true),
                    quiet: pressed != Some(true),
                    disabled,
                    ..Default::default()
                },
                ZORK_UI.palette.prompt,
                window,
                cx,
            )
            .track_focus(&focus)
            .tab_stop(Some(i) == entry)
            .when_some(pressed, |v, pressed| {
                v.aria_toggled(if pressed {
                    Toggled::True
                } else {
                    Toggled::False
                })
            })
            .on_click(cx.listener(move |v, _, w, cx| {
                if !disabled {
                    w.focus(&focus, cx);
                    callback(v, item.key.clone(), cx);
                }
            }))
            .automation_enabled(!disabled, AutomationRole::Button, item.label),
        );
    }
    row.on_key_down(move |e: &KeyDownEvent, w, cx| {
        if active.is_empty() {
            return;
        }
        let current = active
            .iter()
            .position(|i| handles[*i].is_focused(w))
            .unwrap_or(0);
        let next = match e.keystroke.key.as_str() {
            "left" => (current + active.len() - 1) % active.len(),
            "right" => (current + 1) % active.len(),
            "home" => 0,
            "end" => active.len() - 1,
            _ => return,
        };
        w.focus(&handles[active[next]], cx);
        w.prevent_default();
        cx.stop_propagation();
    })
    .into_any_element()
}

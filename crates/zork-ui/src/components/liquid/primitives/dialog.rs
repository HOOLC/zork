use super::{
    super::{
        controls::{self, ActionStyle, ControlElement},
        overlay::{Dialog, MeasuredAnchor, Placement},
        panel::{Content, ContentPanel, Source},
        Material, Pose, SurfaceColors,
    },
    surface,
};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{prelude::*, *};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub struct AlertDialog {
    dialog: Dialog,
    cancel_label: Option<SharedString>,
}
impl AlertDialog {
    pub fn new(cx: &mut App) -> Self {
        Self {
            dialog: Dialog::new(cx).alert(),
            cancel_label: None,
        }
    }
    pub fn cancel_label(&mut self, label: impl Into<SharedString>) {
        self.cancel_label = Some(label.into());
    }
    pub fn inspect(&self) -> serde_json::Value {
        self.dialog.inspect()
    }
    pub fn trigger<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        width: f32,
        window: &mut Window,
        cx: &mut Context<V>,
        open: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> AnyElement {
        let label = label.into();
        self.dialog
            .trigger(
                id,
                label.clone(),
                width,
                ActionStyle::default(),
                ZORK_UI.palette.canvas,
                window,
                cx,
                open,
            )
            .automation(AutomationRole::Button, label)
            .into_any_element()
    }
    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        confirm_label: impl Into<SharedString>,
        open: bool,
        busy: bool,
        material: Material,
        window: &mut Window,
        cx: &mut Context<V>,
        cancel: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
        confirm: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        let id = id.into();
        let title = title.into();
        let confirm_label = confirm_label.into();
        let cancel_label = self.cancel_label.clone().unwrap_or_else(|| "取消".into());
        let cancel_accessible = self
            .cancel_label
            .clone()
            .unwrap_or_else(|| "取消确认".into());
        let cancel = Rc::new(cancel);
        let callback = cancel.clone();
        let cancel_id = format!("{id}-cancel");
        self.dialog
            .initial_focus(controls::action_focus(cancel_id.clone(), window, cx));
        let footer = div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .child(
                controls::action(
                    cancel_id,
                    cancel_label,
                    76.,
                    32.,
                    ActionStyle {
                        disabled: busy,
                        ..Default::default()
                    },
                    ZORK_UI.palette.canvas,
                    window,
                    cx,
                )
                .on_click(cx.listener(move |v, _, w, cx| {
                    if !busy {
                        callback(v, w, cx);
                    }
                }))
                .automation_enabled(
                    !busy,
                    AutomationRole::Button,
                    cancel_accessible,
                ),
            )
            .child(
                controls::action(
                    format!("{id}-confirm"),
                    confirm_label.clone(),
                    104.,
                    32.,
                    ActionStyle {
                        primary: true,
                        busy,
                        ..Default::default()
                    },
                    ZORK_UI.palette.canvas,
                    window,
                    cx,
                )
                .on_click(cx.listener(move |v, _, w, cx| {
                    if !busy {
                        confirm(v, w, cx);
                    }
                }))
                .automation_enabled(!busy, AutomationRole::Button, confirm_label),
            );
        self.dialog.render(
            id,
            title,
            div()
                .text_size(px(13.))
                .line_height(px(22.))
                .child(description.into()),
            Some(footer.into_any_element()),
            open,
            Placement::Window { width: 420. },
            material,
            window,
            cx,
            move |v, w, cx| {
                if !busy {
                    cancel(v, w, cx);
                }
            },
        )
    }
}

struct FlyoutState {
    open: bool,
    source: Option<FocusHandle>,
    pending: bool,
    hoverable: bool,
    opened_by_hover: bool,
    keyboard: bool,
    trigger_hover: bool,
    panel_hover: bool,
    timer: Option<Task<()>>,
}
fn hover_flyout<V: 'static>(
    state: &Rc<RefCell<FlyoutState>>,
    hovered: bool,
    panel: bool,
    cx: &mut Context<V>,
) {
    let mut value = state.borrow_mut();
    if !value.hoverable {
        return;
    }
    value.timer.take();
    if panel {
        value.panel_hover = hovered;
    } else {
        value.trigger_hover = hovered;
    }
    if value.trigger_hover || value.panel_hover || value.keyboard {
        return;
    }
    let pending = state.clone();
    value.timer = Some(cx.spawn(async move |owner, cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(160))
            .await;
        let _ = owner.update(cx, |_, cx| {
            pending.borrow_mut().open = false;
            cx.notify();
        });
    }));
}
#[derive(Clone)]
struct FlyoutTrigger {
    id: SharedString,
    label: SharedString,
    focus: FocusHandle,
    material: bool,
}
pub struct Flyout {
    state: Rc<RefCell<FlyoutState>>,
    anchor: MeasuredAnchor,
    panel: Rc<Cell<Bounds<Pixels>>>,
    material: RefCell<ContentPanel>,
    trigger: RefCell<Option<FlyoutTrigger>>,
    focus: FocusHandle,
    scope: RefCell<crate::modal::FocusScope>,
    was_open: Cell<bool>,
    initial_focus: RefCell<Option<FocusHandle>>,
    align_end: Cell<bool>,
}
fn step_flyout(
    state: &Rc<RefCell<FlyoutState>>,
    focus: &FocusHandle,
    backwards: bool,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    crate::modal::advance_focus(backwards, window, cx);
    if !focus.contains_focused(window, cx) {
        state.borrow_mut().open = false;
        true
    } else {
        false
    }
}
impl Flyout {
    pub fn new(cx: &mut App) -> Self {
        let scope = crate::modal::FocusScope::new(cx);
        Self {
            state: Rc::new(RefCell::new(FlyoutState {
                open: false,
                source: None,
                pending: false,
                hoverable: false,
                opened_by_hover: false,
                keyboard: false,
                trigger_hover: false,
                panel_hover: false,
                timer: None,
            })),
            anchor: Default::default(),
            panel: Default::default(),
            material: Default::default(),
            trigger: Default::default(),
            focus: scope.focus.clone(),
            scope: RefCell::new(scope),
            was_open: Cell::new(false),
            initial_focus: Default::default(),
            align_end: Cell::new(false),
        }
    }
    pub fn alive(&self) -> bool {
        self.material.borrow().alive()
    }
    pub fn samples(&self) -> Vec<super::super::overlay::FrameSample> {
        self.material.borrow().samples().to_vec()
    }
    pub fn reset_samples(&self) {
        self.material.borrow_mut().reset_samples();
    }
    pub fn visible(&self) -> bool {
        self.material.borrow().visible()
    }
    pub fn initial_focus(&self, focus: Option<FocusHandle>) {
        *self.initial_focus.borrow_mut() = focus;
    }
    /// Prefer the source's upper edge, falling back below when content cannot fit.
    pub fn prefer_above(&self) {
        self.anchor.side.set(Some(true));
    }
    /// Align the outer panel's trailing edge with its source control.
    pub fn align_end(&self) {
        self.align_end.set(true);
    }
    pub fn dismiss(&self) {
        self.state.borrow_mut().open = false;
    }
    /// A selection or another measured surface can open the same flyout without
    /// manufacturing a fixed-width trigger or taking over its source's layout.
    pub fn open_at(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        bounds: Bounds<Pixels>,
        source: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.anchor.set_window_bounds(bounds, window);
        let source = source
            .or_else(|| window.focused(cx))
            .unwrap_or_else(|| self.focus.clone());
        *self.trigger.borrow_mut() = Some(FlyoutTrigger {
            id: id.into(),
            label: label.into(),
            focus: source.clone(),
            material: false,
        });
        let mut state = self.state.borrow_mut();
        state.pending = !state.open;
        state.open = true;
        state.source = Some(source);
        state.keyboard = window.last_input_was_keyboard();
    }
    pub fn is_open(&self) -> bool {
        self.state.borrow().open
    }
    pub fn inspect(&self) -> serde_json::Value {
        let bounds = self.anchor.bounds.get();
        serde_json::json!({
            "material": self.material.borrow().inspect(),
            "anchor": [bounds.origin.x.as_f32(), bounds.origin.y.as_f32(), bounds.size.width.as_f32(), bounds.size.height.as_f32()],
            "sourceVisible": self.anchor.visible.get(),
        })
    }
    pub fn close(&self, w: &mut Window, cx: &mut App) {
        let mut state = self.state.borrow_mut();
        state.open = false;
        if let Some(source) = &state.source {
            w.focus(source, cx);
        }
    }
    pub fn trigger_element<V: 'static, E: ControlElement>(
        &self,
        element: E,
        label: impl Into<SharedString>,
        focus: &FocusHandle,
        enabled: bool,
        cx: &mut Context<V>,
    ) -> E {
        let id: SharedString = format!(
            "{:?}",
            Element::id(&element).expect("a flyout source needs a stable ID")
        )
        .into();
        *self.trigger.borrow_mut() = Some(FlyoutTrigger {
            id,
            label: label.into(),
            focus: focus.clone(),
            material: true,
        });
        let state = self.state.clone();
        let focus = focus.clone();
        element
            .control_focus(&focus)
            .panel_source()
            .source_material(self.material.borrow().source_material())
            .aria_expanded(self.is_open())
            .on_click(cx.listener(move |_, _, w, cx| {
                if !enabled {
                    return;
                }
                let mut state = state.borrow_mut();
                state.open = !state.open;
                state.pending = state.open;
                state.source = Some(focus.clone());
                if !state.open {
                    w.focus(&focus, cx);
                }
                cx.notify();
            }))
            .control_overlay(
                self.anchor
                    .measure(self.is_open() || self.material.borrow().alive(), cx)
                    .into_any_element(),
            )
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
        let action = controls::adaptive_action(
            id,
            label.clone(),
            ActionStyle {
                expanded: self.is_open(),
                opens_panel: true,
                ..Default::default()
            },
            ZORK_UI.palette.canvas,
        )
        .w(px(width))
        .h(px(32.));
        self.trigger_element(action, label.clone(), &focus, true, cx)
            .automation(AutomationRole::Button, label)
            .into_any_element()
    }
    pub fn render<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl FnOnce(bool, f32, &mut Window, &mut Context<V>) -> AnyElement,
        width: f32,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Option<AnyElement> {
        self.render_body(id, title, body, width, 20., true, window, cx)
    }
    /// Rich callers supply their header and unpadded content. The material owns
    /// the declared padding so its content clips exclude the empty corner area.
    pub fn render_content<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl FnOnce(bool, f32, &mut Window, &mut Context<V>) -> AnyElement,
        width: f32,
        padding: f32,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Option<AnyElement> {
        self.render_body(id, title, body, width, padding, false, window, cx)
    }
    fn render_body<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl FnOnce(bool, f32, &mut Window, &mut Context<V>) -> AnyElement,
        width: f32,
        padding: f32,
        heading: bool,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Option<AnyElement> {
        let open = self.is_open();
        if !open && !self.material.borrow().alive() {
            self.material.borrow().source_material().clear();
            self.anchor.reset_placement();
            return None;
        }
        let trigger = self.trigger.borrow().clone()?;
        let id = id.into();
        let title = title.into();
        let viewport = window.viewport_size();
        let source = self.anchor.bounds.get();
        let width = width.min((viewport.width.as_f32() - 24.).max(2.));
        let padding = padding.max(0.).min(width / 2. - 1.);
        let inner_width = (width - 2. * padding).max(2.);
        let natural_height = self.material.borrow().content_height().unwrap_or(2.);
        let placement = self
            .anchor
            .fit(source, size(px(width), px(natural_height)), viewport, 8.);
        // Measure against all usable space. Capping measurement to the prior
        // side would trap a growing panel there and prevent an automatic flip.
        let available = placement.maximum_height;
        let height = placement.bounds.size.height.as_f32();
        let x = if self.align_end.get() {
            (source.right().as_f32() - width)
                .clamp(12., (viewport.width.as_f32() - width - 12.).max(12.))
        } else {
            placement.bounds.origin.x.as_f32()
        };
        let y = placement.bounds.origin.y.as_f32();
        self.panel.set(Bounds::new(
            point(px(x) - source.origin.x, px(y) - source.origin.y),
            size(px(width), px(height)),
        ));
        let visible = self.anchor.visible.get();
        let active = open && visible;
        if active && !self.was_open.replace(active) {
            self.scope
                .borrow_mut()
                .activate("flyout", &trigger.focus, window, cx);
        } else {
            self.was_open.set(active);
            self.scope
                .borrow_mut()
                .sync(active.then_some("flyout"), window, cx);
        }
        if !visible && self.focus.contains_focused(window, cx) {
            window.focus(&trigger.focus, cx);
        }
        let interactive =
            open && visible && (cx.reduce_motion() || self.material.borrow().progress() >= 0.4);
        let body = body(interactive, inner_width, window, cx);
        let state = self.state.clone();
        let focus = self.focus.clone();
        let outside = self.state.clone();
        let outside_source = self.anchor.bounds.clone();
        let outside_panel = self.panel.clone();
        let next_state = self.state.clone();
        let next_focus = self.focus.clone();
        let previous_state = self.state.clone();
        let previous_focus = self.focus.clone();
        let hover = self.state.clone();
        let contents = div()
            .id(id.clone())
            .occlude()
            .w(px(inner_width))
            .max_h(px((available
                - 2. * padding
                - if heading { 32. } else { 0. })
            .max(2.)))
            .overflow_y_scroll()
            .role(Role::Dialog)
            .aria_label(title.clone())
            .track_focus(&focus)
            .tab_stop(false)
            .tab_group()
            .on_hover(
                cx.listener(move |_, inside: &bool, _, cx| hover_flyout(&hover, *inside, true, cx)),
            )
            .capture_any_mouse_down(move |_, _, cx| {
                if !interactive {
                    cx.stop_propagation();
                }
            })
            .capture_key_down(cx.listener(move |_, e: &KeyDownEvent, w, cx| {
                if !open {
                    cx.stop_propagation();
                    return;
                }
                match e.keystroke.key.as_str() {
                    "escape" => {
                        let mut state = state.borrow_mut();
                        state.open = false;
                        if let Some(source) = &state.source {
                            w.focus(source, cx);
                        }
                        cx.notify();
                        cx.stop_propagation();
                    }
                    "tab" => {
                        if step_flyout(&state, &focus, e.keystroke.modifiers.shift, w, cx) {
                            cx.notify();
                        }
                        w.prevent_default();
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .on_action(cx.listener(move |_, _: &crate::navigation::Next, w, cx| {
                if step_flyout(&next_state, &next_focus, false, w, cx) {
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .on_action(
                cx.listener(move |_, _: &crate::navigation::Previous, w, cx| {
                    if step_flyout(&previous_state, &previous_focus, true, w, cx) {
                        cx.notify();
                    }
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down_out(cx.listener(move |_, e: &MouseDownEvent, _, cx| {
                if open
                    && !outside_source.get().contains(&e.position)
                    && !outside_panel
                        .get()
                        .contains(&(e.position - outside_source.get().origin))
                {
                    outside.borrow_mut().open = false;
                    cx.notify();
                }
            }))
            .child(body)
            .automation_enabled(interactive, AutomationRole::Status, title.clone());
        let change = self.state.clone();
        let mut origin = Source::new(
            trigger.id,
            trigger.label,
            Pose::rect(
                0.,
                0.,
                source.size.width.as_f32().max(2.) as f64,
                source.size.height.as_f32().max(2.) as f64,
                (source.size.height.as_f32() / 2.) as f64,
            ),
            open,
            trigger.material,
            move |_, open, w, cx| {
                let mut state = change.borrow_mut();
                state.open = open || state.opened_by_hover;
                state.pending = state.open;
                state.opened_by_hover = false;
                state.keyboard = w.last_input_was_keyboard();
                state.timer.take();
                cx.notify();
            },
        )
        .in_layout();
        origin.focus = Some(trigger.focus);
        origin.focus_on_change = false;
        origin.visible = self.anchor.visible.get();
        origin.style.opens_panel = true;
        let hover = self.state.clone();
        origin.hover = Some(Rc::new(move |_, inside, _, cx| {
            hover_flyout(&hover, inside, false, cx);
        }));
        let mut sections = Vec::new();
        if heading {
            sections.push(
                div()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title)
                    .into_any_element(),
            );
        }
        sections.push(contents.into_any_element());
        let panel = self.material.borrow_mut().render(
            format!("{id}-material"),
            viewport.width.as_f32(),
            super::super::panel::Placement::new(
                x - source.left().as_f32(),
                y - source.top().as_f32(),
                width,
            ),
            Content {
                sections,
                padding,
                gap: if heading { 12. } else { 0. },
            },
            Some(origin),
            SurfaceColors::outlined(crate::design::LIQUID_OUTLINE, ZORK_UI.palette.canvas),
            Material::default(),
            window,
            cx,
        );
        let pending = self.state.borrow().pending && interactive;
        if pending {
            self.state.borrow_mut().pending = false;
            let focus = self.focus.clone();
            let initial = self.initial_focus.borrow().clone();
            let state = self.state.clone();
            window.on_next_frame(move |w, cx| {
                if state.borrow().open {
                    w.focus(initial.as_ref().unwrap_or(&focus), cx);
                    if initial.is_none() {
                        crate::modal::advance_focus(false, w, cx);
                    }
                }
            });
        }
        let escape = self.state.clone();
        let panel = div()
            .capture_key_down(cx.listener(move |_, e: &KeyDownEvent, w, cx| {
                if open && e.keystroke.key == "escape" {
                    let mut state = escape.borrow_mut();
                    state.open = false;
                    if let Some(source) = &state.source {
                        w.focus(source, cx);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(panel);
        Some(self.anchor.layer(panel, 210))
    }
}

pub struct NavLink {
    pub key: String,
    pub label: SharedString,
    pub description: SharedString,
}
pub struct NavGroup {
    pub key: SharedString,
    pub label: SharedString,
    pub links: Vec<NavLink>,
}
#[derive(Default)]
pub struct NavigationMenu {
    flyouts: Vec<Flyout>,
}
impl NavigationMenu {
    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        groups: Vec<NavGroup>,
        width: f32,
        window: &mut Window,
        cx: &mut Context<V>,
        navigate: impl Fn(&mut V, String, &mut Context<V>) + 'static,
    ) -> AnyElement {
        let id = id.into();
        self.flyouts.resize_with(groups.len(), || Flyout::new(cx));
        let count = groups.len();
        let states: Vec<_> = self.flyouts.iter().map(|f| f.state.clone()).collect();
        let handles: Vec<_> = groups
            .iter()
            .map(|g| controls::action_focus(g.key.clone(), window, cx))
            .collect();
        let active = states.iter().position(|s| s.borrow().open);
        let callback = Rc::new(navigate);
        let mut row = div()
            .id(id.clone())
            .role(Role::Navigation)
            .aria_label("导航菜单")
            .flex()
            .flex_wrap()
            .gap(px(8.));
        for (index, group) in groups.into_iter().enumerate() {
            self.flyouts[index].state.borrow_mut().hoverable = true;
            let focus = handles[index].clone();
            let hover_focus = focus.clone();
            let all = states.clone();
            let hover_all = states.clone();
            let hover_state = states[index].clone();
            *self.flyouts[index].trigger.borrow_mut() = Some(FlyoutTrigger {
                id: group.key.clone(),
                label: group.label.clone(),
                focus: focus.clone(),
                material: true,
            });
            let trigger_width =
                (super::super::overlay::measure_label(&group.label, 12., window) + 36.).min(width);
            let trigger = controls::adaptive_action(
                group.key,
                group.label.clone(),
                ActionStyle {
                    expanded: active == Some(index),
                    opens_panel: true,
                    ..Default::default()
                },
                ZORK_UI.palette.canvas,
            )
            .w(px(trigger_width))
            .h(px(32.))
            .source_material(self.flyouts[index].material.borrow().source_material())
            .aria_expanded(active == Some(index))
            .track_focus(&focus)
            .tab_stop(true)
            .on_click(cx.listener(move |_, _, w, cx| {
                let toggle_closed = {
                    let state = all[index].borrow();
                    state.open && !state.opened_by_hover
                };
                for state in &all {
                    state.borrow_mut().open = false;
                }
                let mut state = all[index].borrow_mut();
                state.open = !toggle_closed;
                state.opened_by_hover = false;
                state.keyboard = w.last_input_was_keyboard();
                state.pending = state.open;
                state.source = Some(focus.clone());
                if toggle_closed {
                    w.focus(&focus, cx);
                }
                cx.notify();
            }))
            .on_hover(cx.listener(move |_, inside: &bool, _, cx| {
                if *inside && !hover_state.borrow().open {
                    for state in &hover_all {
                        state.borrow_mut().open = false;
                    }
                    let mut state = hover_state.borrow_mut();
                    state.open = true;
                    state.opened_by_hover = true;
                    state.keyboard = false;
                    state.pending = false;
                    state.source = Some(hover_focus.clone());
                    cx.notify();
                }
                hover_flyout(&hover_state, *inside, false, cx);
            }))
            .child(self.flyouts[index].anchor.measure(
                self.flyouts[index].is_open() || self.flyouts[index].material.borrow().alive(),
                cx,
            ))
            .automation(AutomationRole::Button, group.label.clone());
            row = row.child(trigger);
            let navigate = callback.clone();
            let state = states[index].clone();
            let scope = id.clone();
            if let Some(panel) = self.flyouts[index].render(
                format!("{id}-panel-{index}"),
                group.label,
                move |interactive, _, _, cx| {
                    let mut links = div().flex().flex_col().gap(px(8.));
                    for link in group.links {
                        let state = state.clone();
                        let callback = navigate.clone();
                        let key = link.key.clone();
                        links = links.child(
                            surface(
                                format!("{scope}-link-{}", link.key),
                                8.,
                                ZORK_UI.palette.canvas,
                                false,
                            )
                            .role(Role::Link)
                            .aria_label(link.label.clone())
                            .focusable()
                            .tab_stop(interactive)
                            .focus_visible(|v| v.underline())
                            .when(interactive, |v| v.cursor_pointer())
                            .p(px(6.))
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(link.label.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(19.))
                                    .text_color(rgb(ZORK_UI.palette.muted))
                                    .child(link.description),
                            )
                            .on_click(cx.listener(move |v, _, w, cx| {
                                if interactive {
                                    state.borrow_mut().open = false;
                                    callback(v, key.clone(), cx);
                                    if let Some(source) = state.borrow().source.as_ref() {
                                        w.focus(source, cx);
                                    }
                                    cx.notify();
                                }
                            }))
                            .automation_enabled(
                                interactive,
                                AutomationRole::Button,
                                link.label,
                            ),
                        );
                    }
                    links.into_any_element()
                },
                280.,
                window,
                cx,
            ) {
                row = row.child(panel);
            }
        }
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
                "down" => current,
                _ => return,
            };
            let open = states.iter().any(|s| s.borrow().open) || e.keystroke.key == "down";
            for state in &states {
                let mut state = state.borrow_mut();
                state.open = false;
                state.timer.take();
            }
            w.focus(&handles[next], cx);
            if open {
                let mut state = states[next].borrow_mut();
                state.open = true;
                state.opened_by_hover = false;
                state.keyboard = true;
                state.pending = true;
                state.source = Some(handles[next].clone());
            }
            cx.notify();
            w.prevent_default();
            cx.stop_propagation();
        }))
        .into_any_element()
    }
}

//! Anchored content with native GPUI focus and gpui-base positioning.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::widgets::controls::ControlElement,
    design::{BORDER_WIDTH, UI_OUTLINE, ZORK_UI},
};
use gpui::{prelude::*, *};
use gpui_base::{Align, Placement, Positioner};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Default)]
struct State {
    open: bool,
    focus_pending: bool,
    source: Option<FocusHandle>,
}

pub struct Flyout {
    state: Rc<RefCell<State>>,
    anchor: Rc<Cell<Bounds<Pixels>>>,
    panel: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    initial_focus: Rc<RefCell<Option<FocusHandle>>>,
    above: Cell<bool>,
    align_end: Cell<bool>,
}

impl Flyout {
    pub fn new(cx: &mut App) -> Self {
        Self {
            state: Default::default(),
            anchor: Default::default(),
            panel: Default::default(),
            focus: cx.focus_handle(),
            initial_focus: Default::default(),
            above: Cell::new(false),
            align_end: Cell::new(false),
        }
    }
    pub fn is_open(&self) -> bool {
        self.state.borrow().open
    }
    pub fn alive(&self) -> bool {
        self.is_open()
    }
    pub fn visible(&self) -> bool {
        self.is_open()
    }
    pub fn inspect(&self) -> serde_json::Value {
        let bounds = self.anchor.get();
        serde_json::json!({
            "material":{"moving":false},
            "anchor":[bounds.origin.x.as_f32(), bounds.origin.y.as_f32(),
                bounds.size.width.as_f32(), bounds.size.height.as_f32()],
            "sourceVisible":self.is_open(),
        })
    }
    pub fn initial_focus(&self, focus: Option<FocusHandle>) {
        *self.initial_focus.borrow_mut() = focus;
    }
    pub fn prefer_above(&self) {
        self.above.set(true);
    }
    pub fn align_end(&self) {
        self.align_end.set(true);
    }
    pub fn dismiss(&self) {
        self.state.borrow_mut().open = false;
    }
    pub fn close(&self, window: &mut Window, cx: &mut App) {
        let mut state = self.state.borrow_mut();
        state.open = false;
        if let Some(source) = &state.source {
            window.focus(source, cx);
        }
    }
    pub fn open_at(
        &self,
        _id: impl Into<SharedString>,
        _label: impl Into<SharedString>,
        bounds: Bounds<Pixels>,
        source: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.anchor.set(bounds);
        let mut state = self.state.borrow_mut();
        state.source = source.or_else(|| window.focused(cx));
        state.open = true;
        state.focus_pending = true;
    }
    pub fn trigger_element<V: 'static, E: ControlElement>(
        &self,
        element: E,
        _label: impl Into<SharedString>,
        focus: &FocusHandle,
        enabled: bool,
        cx: &mut Context<V>,
    ) -> E {
        let state = self.state.clone();
        let source = focus.clone();
        let anchor = self.anchor.clone();
        element
            .control_focus(focus)
            .aria_expanded(self.is_open())
            .on_click(cx.listener(move |_, _, _, cx| {
                if enabled {
                    let mut state = state.borrow_mut();
                    state.open = !state.open;
                    state.focus_pending = state.open;
                    state.source = Some(source.clone());
                    cx.notify();
                }
            }))
            .control_overlay(
                canvas(move |bounds, _, _| anchor.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .inset_0()
                    .into_any_element(),
            )
    }
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
        if !self.is_open() {
            return None;
        }
        let id = id.into();
        let title = title.into();
        let viewport = window.viewport_size();
        let width = width.min((viewport.width.as_f32() - 24.).max(2.));
        let padding = padding.clamp(0., width / 2. - 1.);
        let focus = self.focus.clone();
        let anchor = self.anchor.get();
        let panel_bounds = self.panel.clone();
        let measured_panel = self.panel.clone();
        let state = self.state.clone();
        let outside = self.state.clone();
        let source_bounds = self.anchor.clone();
        let content = body(true, (width - 2. * padding).max(2.), window, cx);
        let panel = div()
            .id(id)
            .role(Role::Dialog)
            .aria_label(title.clone())
            .w(px(width))
            .max_h(px((viewport.height.as_f32() - 24.).max(2.)))
            .overflow_y_scroll()
            .rounded(px(crate::controls::CARD_RADIUS))
            .bg(rgb(ZORK_UI.palette.canvas))
            .border(px(BORDER_WIDTH))
            .border_color(rgb(UI_OUTLINE))
            .p(px(padding))
            .occlude()
            .track_focus(&focus)
            .tab_group()
            .on_key_down(cx.listener(move |_, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    let mut state = state.borrow_mut();
                    state.open = false;
                    if let Some(source) = &state.source {
                        window.focus(source, cx);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down_out(cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                if !source_bounds.get().contains(&event.position)
                    && !panel_bounds.get().contains(&event.position)
                {
                    outside.borrow_mut().open = false;
                    cx.notify();
                }
            }))
            .child(content)
            .child(
                canvas(
                    move |bounds, _, _| measured_panel.set(bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .automation(AutomationRole::Status, title);
        if self.state.borrow_mut().focus_pending {
            self.state.borrow_mut().focus_pending = false;
            let initial = self.initial_focus.borrow().clone();
            let focus = self.focus.clone();
            let state = self.state.clone();
            window.on_next_frame(move |window, cx| {
                if state.borrow().open {
                    window.focus(initial.as_ref().unwrap_or(&focus), cx);
                }
            });
        }
        let placement = if self.above.get() {
            Placement::Top
        } else {
            Placement::Bottom
        };
        let align = if self.align_end.get() {
            Align::End
        } else {
            Align::Start
        };
        Some(
            deferred(
                Positioner::side(anchor)
                    .placement(placement)
                    .align(align)
                    .offset(px(8.))
                    .margin(px(12.))
                    .child(panel),
            )
            .with_priority(210)
            .into_any_element(),
        )
    }
}

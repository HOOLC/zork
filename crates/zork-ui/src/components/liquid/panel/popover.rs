//! Plain anchored content with the same focus scope as the richer picker.
use super::super::controls::{adaptive_action, ActionStyle, ButtonVariant, ControlElement};
use super::ContentSize;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::liquid::overlay::MeasuredAnchor,
    controls,
    design::{LIQUID_OUTLINE, ZORK_UI},
    modal,
};
use gpui::{prelude::*, *};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use zork_liquid::motion::Reveal;

pub struct PopoverPanel {
    size: ContentSize,
    reveal: Reveal,
    scheduled: Rc<Cell<bool>>,
    anchor: MeasuredAnchor,
    focus: modal::FocusScope,
    trigger_focus: FocusHandle,
    label: RefCell<SharedString>,
    was_open: bool,
}

impl PopoverPanel {
    pub fn new(cx: &mut App) -> Self {
        Self {
            size: Default::default(),
            reveal: Default::default(),
            scheduled: Default::default(),
            anchor: Default::default(),
            focus: modal::FocusScope::new(cx),
            trigger_focus: cx.focus_handle(),
            label: RefCell::new(SharedString::default()),
            was_open: false,
        }
    }

    pub fn trigger<V: 'static>(
        &self,
        id: &'static str,
        label: String,
        width: f32,
        open: bool,
        enabled: bool,
        cx: &Context<V>,
        change: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
    ) -> AnyElement {
        self.label.replace(label.clone().into());
        adaptive_action(
            id,
            label.clone(),
            ActionStyle {
                variant: Some(ButtonVariant::Ghost),
                disabled: !enabled,
                ..Default::default()
            },
            ZORK_UI.palette.canvas,
        )
        .track_focus(&self.trigger_focus.clone().tab_stop(enabled))
        .w(px(width))
        .h(px(28.))
        .font_weight(FontWeight::NORMAL)
        .radius(14.)
        .px_3()
        .gap_2()
        .child(controls::icon("icons/chevron-down.svg", 12.))
        .control_overlay(self.anchor.measure(open, cx).into_any_element())
        .aria_expanded(open)
        .on_click(cx.listener(move |view, _, _, cx| {
            if enabled {
                change(view, !open, cx);
            }
        }))
        .automation_enabled(enabled, AutomationRole::Button, label)
        .into_any_element()
    }

    pub fn alive(&self) -> bool {
        self.was_open || self.reveal.opacity() > 0.
    }

    /// A different page has its own natural height and scroll position.
    pub fn reset_content(&mut self) {
        self.panel = FloatingPanel::default();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus.focus, cx);
    }

    pub fn render<V: 'static>(
        &mut self,
        id: &'static str,
        open: bool,
        width: f32,
        end_offset: f32,
        content: AnyElement,
        window: &mut Window,
        cx: &mut Context<V>,
        close: impl Fn(&mut V, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        if open && !self.was_open {
            self.focus.activate(id, &self.trigger_focus, window, cx);
        }
        self.focus.sync(open.then_some(id), window, cx);
        let was_alive = self.alive();
        self.was_open = open;
        let moving = self.reveal.advance(
            open,
            if cx.reduce_motion() { 1. } else { 1. / 60. },
            cx.reduce_motion(),
        );
        if (moving || (was_alive && !self.alive())) && !self.scheduled.replace(true) {
            let scheduled = self.scheduled.clone();
            let owner = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                scheduled.set(false);
                let _ = owner.update(cx, |_, cx| cx.notify());
            });
        }
        if !self.alive() {
            self.anchor.reset_placement();
            return None;
        }
        let close = Rc::new(close);
        let escape = close.clone();
        let anchor = self.anchor.bounds.clone();
        let body = modal::trap_focus(
            div()
                .id(id)
                .w_full()
                .aria_label(self.label.borrow().clone()),
            &self.focus.focus,
        )
        .when(open, |body| body.occlude())
        .on_key_down(cx.listener(move |view, event: &KeyDownEvent, _, cx| {
            if open && event.keystroke.key == "escape" {
                escape(view, cx);
                cx.stop_propagation();
            }
        }))
        .on_mouse_down_out(cx.listener(move |view, event: &MouseDownEvent, _, cx| {
            if open && !anchor.get().contains(&event.position) {
                close(view, cx);
            }
        }))
        .child(content);
        let viewport = window.viewport_size();
        let width = width.min((viewport.width.as_f32() - 24.).max(2.));
        let measured = self.size.measure(
            div().w(px(width)).p(px(12.)).child(body),
            px(width),
            true,
            self.scheduled.clone(),
            cx.entity().into_any().downgrade(),
        );
        let source = self.anchor.bounds.get();
        let desired = self.size.height(f64::INFINITY).unwrap_or(2.) as f32;
        let above = (source.top().as_f32() - 20.).max(2.);
        let below = (viewport.height.as_f32() - source.bottom().as_f32() - 20.).max(2.);
        let on_top = above >= desired || above > below;
        let available = if on_top { above } else { below };
        let height = desired
            .min(available)
            .min((viewport.height.as_f32() - 24.).max(2.))
            .max(2.);
        let x = (source.right().as_f32() - width + end_offset)
            .clamp(12., (viewport.width.as_f32() - width - 12.).max(12.));
        let y = if on_top {
            (source.top().as_f32() - height - 8.).max(12.)
        } else {
            source.bottom().as_f32() + 8.
        };
        let panel = div()
            .id(format!("{id}-surface"))
            .absolute()
            .left(px(x - source.left().as_f32()))
            .top(px(y - source.top().as_f32()))
            .w(px(width))
            .h(px(height))
            .rounded(px(controls::PLAIN_POPOVER_RADIUS))
            .shadow_sm()
            .bg(rgb(ZORK_UI.palette.canvas))
            .border(px(crate::design::BORDER_WIDTH))
            .border_color(rgb(LIQUID_OUTLINE))
            .role(Role::Dialog)
            .opacity(self.reveal.opacity())
            .when(open, |panel| panel.occlude())
            .child(
                div()
                    .id(format!("{id}-scroll"))
                    .size_full()
                    .overflow_y_scroll()
                    .child(measured),
            );
        Some(
            self.anchor.layer(
                div()
                    .relative()
                    .w(viewport.width)
                    .h(viewport.height)
                    .child(panel),
                230,
            ),
        )
    }
}

//! Interactive anchored content reuses the measured floating panel and focus scope.
use super::super::controls::{adaptive_action, ActionStyle, ButtonVariant, ControlElement};
use super::{Content, FloatingPanel, FloatingStyle, Side};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls,
    design::ZORK_UI,
    modal,
};
use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc};

pub struct PopoverPanel {
    panel: FloatingPanel,
    anchor: super::super::overlay::MeasuredAnchor,
    focus: modal::FocusScope,
    trigger_focus: FocusHandle,
    label: RefCell<SharedString>,
    was_open: bool,
}

impl PopoverPanel {
    pub fn new(cx: &mut App) -> Self {
        Self {
            panel: FloatingPanel::default(),
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
                variant: Some(ButtonVariant::Soft),
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
        .on_click(cx.listener(move |view, _, _, cx| change(view, !open, cx)))
        .automation_enabled(enabled, AutomationRole::Button, label)
        .into_any_element()
    }

    pub fn alive(&self) -> bool {
        self.panel.alive()
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
        self.was_open = open;
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
        self.panel.render(
            id,
            self.anchor.bounds.get(),
            open,
            FloatingStyle {
                width,
                side: Side::AboveEnd(end_offset),
                radius: controls::CARD_RADIUS,
                priority: 230,
                role: Role::Dialog,
                placement_min_height: 2.,
            },
            Content {
                sections: vec![body.into_any_element()],
                padding: 12.,
                gap: 0.,
            },
            None,
            window,
            cx,
        )
    }
}

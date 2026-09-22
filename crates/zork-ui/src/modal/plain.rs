//! Plain window dialog: backdrop + smooth rounded panel + opacity fade.
//! No liquid pair morph / source ink transfer.
use super::{bind_close, panel_contents_with_title_action, FocusScope, TitleAction};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        liquid::overlay::{DialogOptions, Placement, SourceBinding},
        smooth,
    },
    controls as ui,
    design::ZORK_UI,
};
use gpui::{
    anchored, deferred, div, point, prelude::*, px, rgb, AnyElement, App, Context, FocusHandle,
    MouseButton, SharedString, Window,
};
use std::{cell::Cell, rc::Rc};
use zork_liquid::motion::Reveal;

pub struct PlainDialog {
    focus: FocusScope,
    content_id: Option<SharedString>,
    open: bool,
    reveal: Reveal,
    backdrop: Reveal,
    seen_open: bool,
    initial_focus: Option<FocusHandle>,
    focus_pending: bool,
    alert: bool,
    /// Kept for API compatibility with product trigger bindings.
    source: SourceBinding,
    scheduled: Rc<Cell<bool>>,
}

impl PlainDialog {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: FocusScope::new(cx),
            content_id: None,
            open: false,
            reveal: Default::default(),
            backdrop: Default::default(),
            seen_open: false,
            initial_focus: None,
            focus_pending: false,
            alert: false,
            source: SourceBinding::default(),
            scheduled: Rc::new(Cell::new(false)),
        }
    }

    pub fn alert(mut self) -> Self {
        self.alert = true;
        self
    }

    pub fn initial_focus(&mut self, focus: FocusHandle) {
        self.initial_focus = Some(focus);
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.focus.clone()
    }

    pub fn source_binding(&self) -> SourceBinding {
        self.source.clone()
    }

    pub fn bind_source(&mut self, source: SourceBinding) {
        self.source = source;
    }

    pub fn alive(&self) -> bool {
        // Retain the host until Reveal snaps both springs to zero. Dropping at
        // the paint threshold would leave a nonzero tail that can never settle.
        self.reveal.opacity() > 0. || self.backdrop.opacity() > 0. || self.open
    }

    pub fn inspect(&self) -> serde_json::Value {
        serde_json::json!({
            "engine": "plain",
            "open": self.open,
            "contentAlpha": self.reveal.opacity(),
            "backdropAlpha": self.backdrop.opacity(),
            "alert": self.alert,
        })
    }

    pub fn visible(&self) -> bool {
        self.alive()
    }

    pub fn pose(&self) -> Option<zork_liquid::Pose> {
        None
    }

    pub fn samples(&self) -> Vec<crate::components::liquid::motion::FrameSample> {
        Vec::new()
    }

    pub fn reset_samples(&mut self) {}

    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl IntoElement,
        footer: Option<AnyElement>,
        open: bool,
        placement: Placement,
        material: crate::components::liquid::Material,
        window: &mut Window,
        cx: &mut Context<V>,
        close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        self.render_with_options(
            id,
            title,
            body,
            footer,
            open,
            placement,
            material,
            DialogOptions::default(),
            window,
            cx,
            close,
        )
    }

    /// Plain trigger button — opens via host callback, no liquid source morph.
    pub fn trigger<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        width: f32,
        style: crate::components::liquid::controls::ActionStyle,
        fill: u32,
        window: &mut Window,
        cx: &mut Context<V>,
        open: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let id = id.into();
        let label = label.into();
        crate::components::liquid::controls::action(id, label, width, 32., style, fill, window, cx)
            .on_click(cx.listener(move |v, _, w, cx| open(v, w, cx)))
    }

    pub fn render_with_options<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl IntoElement,
        footer: Option<AnyElement>,
        open: bool,
        _placement: Placement,
        _material: crate::components::liquid::Material,
        options: DialogOptions,
        window: &mut Window,
        cx: &mut Context<V>,
        close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        let id = id.into();
        let title = title.into();
        let close = bind_close(cx, close);
        let owner = cx.entity().into_any().downgrade();

        if open && !self.seen_open {
            self.focus_pending = true;
            let source = window
                .focused(cx)
                .unwrap_or_else(|| self.focus.focus.clone());
            self.focus.activate("dialog", &source, window, cx);
        }
        if !open {
            self.focus_pending = false;
        }
        self.seen_open = open;
        self.open = open;
        self.focus.sync(open.then_some("dialog"), window, cx);
        self.content_id = Some(id.clone());

        let reduced = cx.reduce_motion();
        let dt = if reduced { 1. } else { 1. / 60. };
        let content_moving = self.reveal.advance(open, dt, reduced);
        let backdrop_moving = self.backdrop.advance(open, dt, reduced);
        let moving = content_moving || backdrop_moving;
        if moving && !self.scheduled.replace(true) {
            let scheduled = self.scheduled.clone();
            let owner = owner.clone();
            window.on_next_frame(move |_, cx| {
                scheduled.set(false);
                if let Some(owner) = owner.upgrade() {
                    cx.notify(owner.entity_id());
                }
            });
        }

        let alpha = self.reveal.opacity();
        let backdrop_alpha = self.backdrop.opacity();
        if alpha <= 0.001 && backdrop_alpha <= 0.001 && !open {
            return None;
        }

        if open && self.focus_pending && alpha > 0.5 {
            self.focus_pending = false;
            if let Some(focus) = &self.initial_focus {
                window.focus(focus, cx);
            } else {
                window.focus(&self.focus.focus, cx);
            }
        }

        let viewport = window.viewport_size();
        let width = ui::DIALOG_WIDTH.min(viewport.width.as_f32() - 40.).max(2.);
        let max_height = (viewport.height.as_f32() - 64.).clamp(2., 660.);
        let contents = panel_contents_with_title_action(
            id.clone(),
            title.clone(),
            options.title_action.map(|action| TitleAction {
                editor: options.title_editor,
                action,
            }),
            body.into_any_element(),
            footer,
            options.notice,
            &self.focus.focus,
            px(max_height),
            None,
            window,
            cx,
            open && options.dismissible,
            close.clone(),
        );

        let panel = smooth::surface(id.clone(), ui::MODAL_RADIUS)
            .occlude()
            .w(px(width))
            .max_h(px(max_height))
            .flex()
            .flex_col()
            .bg(rgb(ZORK_UI.palette.canvas))
            .border(px(crate::design::BORDER_WIDTH))
            .border_color(rgb(crate::design::LIQUID_OUTLINE))
            .opacity(alpha)
            .child(contents)
            .automation(AutomationRole::Status, title.to_string());

        let dismissible = options.dismissible && !self.alert;
        let close_outside = close.clone();
        let dismiss_outside = dismissible && open;
        // Anchor to the window: a short/scrolling settings column must not size or
        // position the backdrop. Deferred drawing also escapes the host clip.
        // occlude + stop_propagation block hover/click from reaching content underneath.
        // Dismiss must live on the backdrop hit target — GPUI delivers mouse_down to the
        // occluded child, not the parent layer listener.
        let layer = deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id(format!("{id}-layer"))
                    .relative()
                    .w(viewport.width)
                    .h(viewport.height)
                    .occlude()
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_move(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .id(format!("{id}-backdrop"))
                            .absolute()
                            .inset_0()
                            .occlude()
                            .bg(gpui::hsla(0., 0., 0., 0.45 * backdrop_alpha))
                            .on_mouse_down(MouseButton::Left, {
                                let close = close_outside.clone();
                                move |_, window, cx| {
                                    if dismiss_outside {
                                        close(window, cx);
                                    }
                                    cx.stop_propagation();
                                }
                            }),
                    )
                    .child(
                        div()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(panel),
                    )
                    .when(open, |el| {
                        el.capture_key_down({
                            let close = close.clone();
                            let dismissible = dismissible;
                            move |e: &gpui::KeyDownEvent, window, cx| {
                                if dismissible && e.keystroke.key == "escape" {
                                    close(window, cx);
                                    cx.stop_propagation();
                                }
                            }
                        })
                    }),
            ),
        )
        .with_priority(300);

        Some(layer.into_any_element())
    }
}

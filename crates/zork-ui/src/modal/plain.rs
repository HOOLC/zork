//! Plain window dialog: backdrop + smooth rounded panel + opacity fade.
//! Window dialog uses ordinary panel opacity.
use super::{bind_close, panel_contents_with_title_action, FocusScope, TitleAction};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{smooth, widgets::overlay::DialogOptions},
    controls as ui,
    design::ZORK_UI,
};
use gpui::{
    anchored, deferred, div, point, prelude::*, px, rgb, AnyElement, App, Context, FocusHandle,
    MouseButton, SharedString, Window,
};

/// A time-based opacity tween. Reversing mid-flight continues from the current
/// value, so a quick open → close never flashes.
struct Fade {
    alpha: f32,
    from: f32,
    target: f32,
    start: std::time::Instant,
    enter_ms: u64,
}
impl Fade {
    fn new(enter_ms: u64) -> Self {
        Self {
            alpha: 0.,
            from: 0.,
            target: 0.,
            start: std::time::Instant::now(),
            enter_ms,
        }
    }
    fn opacity(&self) -> f32 {
        self.alpha
    }
    /// `now` comes from the app executor so tests on a virtual clock advance it.
    fn advance(&mut self, open: bool, reduced: crate::motion::Mode, now: std::time::Instant) -> bool {
        use crate::motion;
        let target = if open { 1. } else { 0. };
        if target != self.target {
            self.from = self.alpha;
            self.target = target;
            self.start = now;
        }
        // Enter eases in over the surface token; exit leaves in two thirds of it.
        // Reduced motion keeps a short fade.
        let (ms, curve): (u64, Box<dyn Fn(f32) -> f32>) = if open {
            (self.enter_ms, Box::new(motion::bezier(0.2, 0.7, 0.2, 1.0)))
        } else {
            (motion::exit(self.enter_ms), Box::new(motion::bezier(0.4, 0.0, 1.0, 1.0)))
        };
        let ms = match reduced {
            motion::Mode::Full => ms,
            motion::Mode::Short => ms.min(motion::REDUCED_FADE),
            motion::Mode::Static => {
                self.alpha = target;
                return false;
            }
        };
        let total = motion::duration(ms).as_secs_f32().max(0.001);
        let t = (now.saturating_duration_since(self.start).as_secs_f32() / total).clamp(0., 1.);
        self.alpha = self.from + (self.target - self.from) * curve(t);
        if t >= 1. {
            self.alpha = target;
        }
        self.alpha != target
    }
}

pub struct PlainDialog {
    focus: FocusScope,
    content_id: Option<SharedString>,
    open: bool,
    reveal: Fade,
    backdrop: Fade,
    seen_open: bool,
    initial_focus: Option<FocusHandle>,
    focus_pending: bool,
    alert: bool,
    content_transition_frames: u64,
    backdrop_transition_frames: u64,
}

impl PlainDialog {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: FocusScope::new(cx),
            content_id: None,
            open: false,
            reveal: Fade::new(crate::motion::SURFACE),
            backdrop: Fade::new(crate::motion::SCRIM),
            seen_open: false,
            initial_focus: None,
            focus_pending: false,
            alert: false,
            content_transition_frames: 0,
            backdrop_transition_frames: 0,
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

    pub fn alive(&self) -> bool {
        // Retain the host until both ordinary opacity fades reach zero.
        self.reveal.opacity() > 0. || self.backdrop.opacity() > 0. || self.open
    }

    pub fn inspect(&self) -> serde_json::Value {
        serde_json::json!({
            "engine": "plain",
            "open": self.open,
            "contentAlpha": self.reveal.opacity(),
            "backdropAlpha": self.backdrop.opacity(),
            "alert": self.alert,
            "contentTransitionFrames": self.content_transition_frames,
            "backdropTransitionFrames": self.backdrop_transition_frames,
        })
    }

    pub fn visible(&self) -> bool {
        self.alive()
    }

    pub fn render<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl IntoElement,
        footer: Option<AnyElement>,
        open: bool,
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
            DialogOptions::default(),
            window,
            cx,
            close,
        )
    }

    /// Plain trigger button that opens via the host callback.
    pub fn trigger<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        width: f32,
        style: crate::components::widgets::controls::ActionStyle,
        fill: u32,
        window: &mut Window,
        cx: &mut Context<V>,
        open: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> crate::components::widgets::controls::Action {
        let id = id.into();
        let label = label.into();
        crate::components::widgets::controls::action(id, label, width, 32., style, fill, window, cx)
            .on_click(cx.listener(move |v, _, w, cx| open(v, w, cx)))
    }

    pub fn render_with_options<V: 'static>(
        &mut self,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl IntoElement,
        footer: Option<AnyElement>,
        open: bool,
        options: DialogOptions,
        window: &mut Window,
        cx: &mut Context<V>,
        close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        let id = id.into();
        let title = title.into();
        let close = bind_close(cx, close);

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

        let reduced = crate::motion::mode(cx);
        let was_alive = self.alive();
        let now = cx.background_executor().now();
        let content_moving = self.reveal.advance(open, reduced, now);
        let backdrop_moving = self.backdrop.advance(open, reduced, now);
        let moving = content_moving || backdrop_moving;
        // Redraw until the fade reaches its endpoint, including the final
        // frame that releases a retained dialog payload.
        if moving || (was_alive && !self.alive()) {
            window.request_animation_frame();
        }

        let alpha = self.reveal.opacity();
        let backdrop_alpha = self.backdrop.opacity();
        if (0.001..0.999).contains(&alpha) {
            self.content_transition_frames += 1;
        }
        if (0.001..0.999).contains(&backdrop_alpha) {
            self.backdrop_transition_frames += 1;
        }
        // Keep advancing both reveals until they snap to the exact closed state.
        if !moving && alpha <= 0.001 && backdrop_alpha <= 0.001 && !open {
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
        let wanted = options.width.unwrap_or(if self.alert {
            ui::DIALOG_CONFIRM_WIDTH
        } else {
            ui::DIALOG_FORM_WIDTH
        });
        let radius = ui::dialog_radius(wanted);
        let width = wanted.min(viewport.width.as_f32() - 40.).max(2.);
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
            window,
            cx,
            open && options.dismissible,
            close.clone(),
        );

        let panel = smooth::surface(id.clone(), radius)
            .occlude()
            .w(px(width))
            .max_h(px(max_height))
            .flex()
            .flex_col()
            .bg(rgb(ZORK_UI.palette.canvas))
            .border(px(crate::design::BORDER_WIDTH))
            .border_color(rgb(crate::design::FORM.outline))
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

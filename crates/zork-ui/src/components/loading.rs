//! Visible-only loading indicator with an isolated animation clock.
//! Timers notify this indicator, never an application-owned request state.
use crate::automation::{AutomationElementExt, AutomationRole};
use gpui::{
    div, prelude::*, px, App, Context, ElementId, Render, RenderOnce, Transformation, Window,
};
use std::time::Instant;
use std::{cell::Cell, rc::Rc, time::Duration};

#[derive(gpui::IntoElement)]
pub struct Indicator {
    id: ElementId,
    size: f32,
    kind: Kind,
    animate: bool,
    delay: bool,
}

pub fn indicator(id: impl Into<ElementId>, size: f32) -> Indicator {
    Indicator {
        id: id.into(),
        size,
        kind: Kind::Ring,
        animate: true,
        delay: true,
    }
}

impl Indicator {
    /// Use when the indicator replaces a label, so the action never appears blank.
    pub fn without_delay(mut self) -> Self {
        self.delay = false;
        self
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Ring,
    Dots,
}

pub fn activity(id: impl Into<ElementId>, animate: bool) -> Indicator {
    Indicator {
        id: id.into(),
        size: 16.,
        kind: Kind::Dots,
        animate,
        delay: true,
    }
}

pub fn status(id: impl Into<ElementId>, label: impl Into<gpui::SharedString>) -> impl IntoElement {
    let id = id.into();
    let label = label.into();
    div()
        .id(id.clone())
        .flex()
        .items_center()
        .gap_2()
        .child(indicator("spinner", 16.))
        .child(label.clone())
        .automation(AutomationRole::Status, label)
}

impl RenderOnce for Indicator {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Unlike use_keyed_state, this does not observe the child and notify
        // the owning application view on every animation tick.
        let state = window.with_global_id(self.id, |id, window| {
            window.with_element_state(id, |state: Option<gpui::Entity<Clock>>, _| {
                let state = state.unwrap_or_else(|| {
                    cx.new(|cx| Clock {
                        started: cx.background_executor().now(),
                        pending: Rc::new(Cell::new(false)),
                        kind: self.kind,
                        animate: self.animate,
                        delay: self.delay,
                    })
                });
                (state.clone(), state)
            })
        });
        state.update(cx, |state, _| {
            state.kind = self.kind;
            state.animate = self.animate;
            state.delay = self.delay;
        });
        div().size(px(self.size)).flex_shrink_0().child(state)
    }
}

struct Clock {
    started: Instant,
    pending: Rc<Cell<bool>>,
    kind: Kind,
    animate: bool,
    delay: bool,
}
impl Render for Clock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reduced = cx.reduce_motion() || !self.animate;
        let elapsed = (cx.background_executor().now() - self.started).as_secs_f32();
        // Reserve the slot immediately, but suppress flashes for quick requests.
        let visible = !self.delay || elapsed >= 0.2;
        let angle = if reduced {
            0.
        } else {
            elapsed.fract() * std::f32::consts::TAU
        };
        let pending = self.pending.clone();
        let weak = cx.entity().downgrade();
        let glyph = match self.kind {
            Kind::Ring => gpui::svg()
                .path("loading/native-ring.svg")
                .text_color(window.text_style().color)
                .size_full()
                .opacity(if visible { 1. } else { 0. })
                .with_transformation(Transformation::rotate(gpui::radians(angle)))
                .into_any_element(),
            Kind::Dots => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(2.))
                .children((0..3).map(|i| {
                    let phase = (elapsed / 1.5 - i as f32 * 0.12).rem_euclid(1.);
                    let alpha = if reduced {
                        0.6
                    } else {
                        0.3 + 0.7 * ((phase * std::f32::consts::TAU).cos() + 1.) / 2.
                    };
                    div()
                        .size(px(2.5))
                        .rounded_full()
                        .bg(window.text_style().color)
                        .opacity(if visible { alpha } else { 0. })
                }))
                .into_any_element(),
        };
        div().relative().size_full().child(glyph).child(
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    if bounds.intersect(&window.content_mask().bounds).is_empty()
                        || (reduced && visible)
                        || pending.replace(true)
                    {
                        return;
                    }
                    let pending = pending.clone();
                    let weak = weak.clone();
                    let delay = if visible {
                        Duration::from_millis(34)
                    } else {
                        Duration::from_secs_f32((0.2 - elapsed).max(0.001))
                    };
                    window
                        .spawn(cx, async move |cx| {
                            cx.background_executor().timer(delay).await;
                            let _ = cx.update(|window, _| {
                                window.on_next_frame(move |_, cx| {
                                    pending.set(false);
                                    let _ = weak.update(cx, |_, cx| cx.notify());
                                });
                            });
                        })
                        .detach();
                },
            )
            .absolute()
            .inset_0(),
        )
    }
}

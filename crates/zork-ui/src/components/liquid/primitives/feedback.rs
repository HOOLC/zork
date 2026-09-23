use super::{
    super::controls::{self, ActionStyle},
    surface,
};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls::NoticeKind,
    design::{FORM, ZORK_UI},
};
use gpui::{prelude::*, *};
use std::time::Instant;
use std::{collections::VecDeque, time::Duration};

pub fn colors(kind: NoticeKind) -> (u32, u32) {
    match kind {
        NoticeKind::Info | NoticeKind::Loading => (ZORK_UI.palette.text, ZORK_UI.palette.prompt),
        NoticeKind::Success => (ZORK_UI.palette.success, FORM.success_surface),
        NoticeKind::Warning => (ZORK_UI.palette.warning, FORM.warning_surface),
        NoticeKind::Error => (ZORK_UI.palette.danger, FORM.error_surface),
    }
}
pub fn notice_content(
    id: impl Into<SharedString>,
    message: impl Into<SharedString>,
    kind: NoticeKind,
    action: Option<AnyElement>,
) -> Stateful<Div> {
    let id = id.into();
    let message = message.into();
    let (ink, _) = colors(kind);
    let glyph = if matches!(kind, NoticeKind::Loading) {
        crate::components::loading::indicator(format!("{id}-spinner"), 14.)
            .without_delay()
            .into_any_element()
    } else {
        crate::controls::icon(
            if matches!(kind, NoticeKind::Success) {
                "icons/check.svg"
            } else {
                "icons/attention.svg"
            },
            14.,
        )
        .text_color(rgb(ink))
        .into_any_element()
    };
    div()
        .id(id)
        .role(if matches!(kind, NoticeKind::Error) {
            Role::Alert
        } else {
            Role::Status
        })
        .aria_label(message.clone())
        .a11y_synthetic_children(move |b| {
            b.parent_node()
                .set_live(if matches!(kind, NoticeKind::Error) {
                    accesskit::Live::Assertive
                } else {
                    accesskit::Live::Polite
                })
        })
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(rgb(ink))
        .child(
            div()
                .size(px(20.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(glyph),
        )
        .child(div().flex_1().min_w_0().child(message))
        .when_some(action, |v, action| {
            v.child(div().flex_shrink_0().child(action))
        })
}
pub fn badge(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    kind: NoticeKind,
) -> AnyElement {
    let id = id.into();
    let text = text.into();
    let (ink, fill) = colors(kind);
    surface(id, crate::controls::BUTTON_RADIUS, fill, false)
        .role(Role::Label)
        .aria_label(text.clone())
        .px(px(7.))
        .py(px(2.))
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(rgb(ink))
        .child(text)
        .automation(AutomationRole::Status, "标记")
        .into_any_element()
}
/// Skeletons reserve content geometry without publishing placeholder text.
pub fn skeleton(
    id: impl Into<SharedString>,
    width: f32,
    height: f32,
    circular: bool,
) -> AnyElement {
    surface(
        id,
        if circular { width.min(height) / 2. } else { 6. },
        ZORK_UI.palette.border,
        false,
    )
    .w(px(width))
    .h(px(height))
    .into_any_element()
}

#[derive(Clone)]
pub struct ToastAction {
    pub key: SharedString,
    pub label: SharedString,
}
pub struct ToastEvent {
    pub key: SharedString,
}
struct Toast {
    id: u64,
    message: SharedString,
    description: Option<SharedString>,
    action: Option<ToastAction>,
    kind: NoticeKind,
    remaining: Option<Duration>,
    closing: Option<Instant>,
    focus: FocusHandle,
}
/// Bounded, local presentation queue. Notifications carry display text and an
/// optional action key; application request state never lives in this queue.
pub struct Toasts {
    entries: VecDeque<Toast>,
    next: u64,
    last: Instant,
    hovered: Option<u64>,
    focused: bool,
    focus: FocusHandle,
    previous: Option<FocusHandle>,
    subscriptions: Vec<Subscription>,
    wake: Option<Task<()>>,
}
impl Toasts {
    pub const CAPACITY: usize = 4;
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            entries: VecDeque::new(),
            next: 0,
            last: Instant::now(),
            hovered: None,
            focused: false,
            focus: cx.focus_handle(),
            previous: None,
            subscriptions: vec![],
            wake: None,
        }
    }
    pub fn push(
        &mut self,
        message: impl Into<SharedString>,
        description: Option<SharedString>,
        kind: NoticeKind,
        action: Option<ToastAction>,
        timeout: Option<Duration>,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.advance();
        if self.entries.len() == Self::CAPACITY {
            self.entries.pop_front();
        }
        self.next = self.next.wrapping_add(1);
        self.entries.push_back(Toast {
            id: self.next,
            message: message.into(),
            description,
            action,
            kind,
            remaining: timeout,
            closing: None,
            focus: cx.focus_handle(),
        });
        cx.notify();
        self.next
    }
    fn advance(&mut self) {
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last);
        self.last = now;
        for toast in &mut self.entries {
            if toast.closing.is_none() && self.hovered.is_none() && !self.focused {
                if let Some(remaining) = &mut toast.remaining {
                    *remaining = remaining.saturating_sub(elapsed);
                    if remaining.is_zero() {
                        toast.closing = Some(now);
                    }
                }
            }
        }
        self.entries.retain(|toast| {
            toast
                .closing
                .is_none_or(|at| now.saturating_duration_since(at) < Duration::from_millis(160))
        });
        if self.hovered.is_some_and(|id| {
            !self
                .entries
                .iter()
                .any(|toast| toast.id == id && toast.closing.is_none())
        }) {
            self.hovered = None;
        }
    }
    pub fn dismiss(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.advance();
        let focus_removed = self
            .entries
            .iter()
            .any(|t| t.id == id && t.focus.contains_focused(window, cx));
        if let Some(toast) = self.entries.iter_mut().find(|t| t.id == id) {
            toast.closing = Some(Instant::now());
        }
        if focus_removed {
            if let Some(next) = self.entries.iter().rev().find(|t| t.closing.is_none()) {
                window.focus(&next.focus, cx);
                window.focus_next(cx);
            } else if let Some(previous) = self.previous.take() {
                window.focus(&previous, cx);
            } else {
                window.blur();
            }
        }
        cx.notify();
    }
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(toast) = self.entries.iter().rev().find(|t| t.closing.is_none()) {
            if !self.focus.contains_focused(window, cx) {
                self.previous = window.focused(cx);
            }
            window.focus(&toast.focus, cx);
            window.focus_next(cx);
        }
    }
    pub fn inspect(&self) -> serde_json::Value {
        serde_json::json!({"capacity":Self::CAPACITY,"paused":self.hovered.is_some()||self.focused,"items":self.entries.iter().map(|t|serde_json::json!({"id":t.id,"message":t.message.as_ref(),"closing":t.closing.is_some(),"remainingMs":t.remaining.map(|d|d.as_millis())})).collect::<Vec<_>>()})
    }
}
impl EventEmitter<ToastEvent> for Toasts {}
impl Render for Toasts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.subscriptions.is_empty() {
            self.subscriptions
                .push(cx.on_focus_in(&self.focus, window, |v, _, cx| {
                    v.advance();
                    v.focused = true;
                    cx.notify();
                }));
            self.subscriptions
                .push(cx.on_focus_out(&self.focus, window, |v, _, _, cx| {
                    v.advance();
                    v.focused = false;
                    cx.notify();
                }));
            self.subscriptions
                .push(cx.observe_window_activation(window, |v, window, cx| {
                    v.advance();
                    v.focused = !window.is_window_active() || v.focus.contains_focused(window, cx);
                    cx.notify();
                }));
        }
        self.advance();
        self.wake = None;
        let closing = self.entries.iter().any(|t| t.closing.is_some());
        let delay = if closing {
            Some(Duration::from_millis(16))
        } else if self.hovered.is_some() || self.focused {
            None
        } else {
            self.entries.iter().filter_map(|t| t.remaining).min()
        };
        if let Some(delay) = delay {
            self.wake = Some(cx.spawn(async move |weak, cx| {
                cx.background_executor().timer(delay).await;
                let _ = weak.update(cx, |_, cx| cx.notify());
            }));
        }
        let width = (window.viewport_size().width.as_f32() - 32.).clamp(100., 360.);
        let mut stack = div()
            .id("toast-viewport")
            .w(px(width))
            .flex()
            .flex_col()
            .gap(px(8.))
            .role(Role::Region)
            .aria_label("通知（F8 聚焦）")
            .track_focus(&self.focus)
            .tab_stop(false);
        for toast in &self.entries {
            let id = toast.id;
            let live = toast.closing.is_none();
            let alpha = toast.closing.map_or(1., |at| {
                1. - (at.elapsed().as_secs_f32() / 0.16).clamp(0., 1.)
            });
            let action = toast.action.clone();
            let close = controls::action(
                format!("toast-{id}-close"),
                "",
                24.,
                24.,
                ActionStyle {
                    icon: Some("icons/x.svg"),
                    disabled: !live,
                    ..Default::default()
                },
                ZORK_UI.palette.canvas,
                window,
                cx,
            )
            .aria_label("关闭通知")
            .on_click(cx.listener(move |v, _, w, cx| v.dismiss(id, w, cx)))
            .automation_enabled(live, AutomationRole::Button, "关闭通知");
            let mut body = div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(notice_content(
                    format!("toast-{id}-message"),
                    toast.message.clone(),
                    toast.kind,
                    Some(close.into_any_element()),
                ))
                .when_some(toast.description.clone(), |v, description| {
                    v.child(
                        div()
                            .pl(px(28.))
                            .text_size(px(12.))
                            .line_height(px(19.))
                            .text_color(rgb(ZORK_UI.palette.muted))
                            .child(description),
                    )
                });
            if let Some(action) = action {
                body = body.child(
                    controls::action(
                        format!("toast-{id}-action"),
                        action.label.clone(),
                        90.,
                        28.,
                        ActionStyle {
                            disabled: !live,
                            ..Default::default()
                        },
                        ZORK_UI.palette.canvas,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if live {
                            cx.emit(ToastEvent {
                                key: action.key.clone(),
                            });
                            v.dismiss(id, w, cx);
                        }
                    }))
                    .automation_enabled(
                        live,
                        AutomationRole::Button,
                        action.label,
                    ),
                );
            }
            stack = stack.child(
                surface(
                    format!("toast-{id}"),
                    crate::controls::CARD_RADIUS,
                    ZORK_UI.palette.canvas,
                    true,
                )
                .occlude()
                .w_full()
                .p(px(20.))
                .opacity(alpha)
                .on_hover(cx.listener(move |v, inside: &bool, _, cx| {
                    v.advance();
                    if *inside {
                        v.hovered = Some(id);
                    } else if v.hovered == Some(id) {
                        v.hovered = None;
                    }
                    cx.notify();
                }))
                .track_focus(&toast.focus.clone().tab_stop(false))
                .tab_stop(false)
                .on_key_down(cx.listener(move |v, e: &KeyDownEvent, w, cx| {
                    if e.keystroke.key == "escape" {
                        v.dismiss(id, w, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(body)
                .automation_enabled(
                    live,
                    AutomationRole::Status,
                    toast.message.clone(),
                ),
            );
        }
        let viewport = window.viewport_size();
        div().when(!self.entries.is_empty(), |v| {
            v.child(
                deferred(
                    anchored()
                        .anchor(Anchor::BottomRight)
                        .position(point(viewport.width - px(16.), viewport.height - px(16.)))
                        .child(stack),
                )
                .with_priority(300),
            )
        })
    }
}

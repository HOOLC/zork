//! Read-only hover details shared by navigation and component examples.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::CUE_UI,
};
use gpui::{div, prelude::*, px, rgb, Context, FontWeight, Window};

use super::liquid::panel::{Content, FloatingPanel, FloatingStyle, Side};
const CONTENT_SECONDS: f32 = 0.14;
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
#[cfg(target_family = "wasm")]
use web_time::Instant;

#[derive(Clone)]
pub struct DetailsTooltip {
    pub key: String,
    pub title: String,
    pub kind: String,
    pub avatar: Option<String>,
    pub description: String,
    pub rows: Vec<(String, String)>,
}
impl DetailsTooltip {
    pub fn content(&self) -> gpui::Div {
        let p = CUE_UI.palette;
        let description: String = self.description.trim().chars().take(320).collect();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(match &self.avatar {
                        Some(avatar) => ui::agent_avatar(Some(avatar), 32.).into_any_element(),
                        None => div()
                            .size(px(32.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(12.))
                            .bg(rgb(p.prompt))
                            .child(ui::icon("icons/checklist.svg", 18.))
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(19.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .line_clamp(2)
                                    .child(self.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .line_height(px(16.))
                                    .text_color(rgb(p.muted))
                                    .child(self.kind.clone()),
                            ),
                    ),
            )
            .when(!description.is_empty(), |v| {
                v.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(19.))
                        .line_clamp(3)
                        .child(description),
                )
            })
            .child(
                div().flex().flex_col().gap(px(6.)).children(
                    self.rows
                        .iter()
                        .filter(|(_, value)| !value.trim().is_empty())
                        .map(|(label, value)| {
                            div()
                                .flex()
                                .items_start()
                                .gap(px(8.))
                                .text_size(px(11.))
                                .line_height(px(17.))
                                .child(
                                    div()
                                        .w(px(60.))
                                        .flex_shrink_0()
                                        .text_color(rgb(p.muted))
                                        .child(label.clone()),
                                )
                                .child(div().flex_1().min_w_0().line_clamp(2).child(value.clone()))
                        }),
                ),
            )
    }
    pub fn card(&self) -> crate::automation::element::AutomationElement<gpui::Stateful<gpui::Div>> {
        Self::surface(format!("detail-tooltip-{}", self.key))
            .p(px(16.))
            .child(self.content())
            .automation(
                AutomationRole::Status,
                format!("{}详情：{}", self.kind, self.title),
            )
    }
    fn surface(id: String) -> gpui::Stateful<gpui::Div> {
        crate::components::liquid::primitives::surface(
            id,
            ui::CARD_RADIUS,
            CUE_UI.palette.canvas,
            true,
        )
        .occlude()
        .w(px(320.))
        .max_w_full()
        .flex()
        .flex_col()
    }
}

impl gpui::Render for DetailsTooltip {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(format!("detail-tooltip-scroll-{}", self.key))
            .max_w(window.viewport_size().width - px(24.))
            .child(self.card().map_inner(|card| {
                card.max_h(window.viewport_size().height - px(24.))
                    .overflow_y_scroll()
            }))
    }
}

#[derive(Default)]
struct HoverState {
    panel: FloatingPanel,
    bounds: gpui::Bounds<gpui::Pixels>,
    open: bool,
    trigger_hover: bool,
    panel_hover: bool,
    focused: bool,
    dismissed: bool,
    focus: Option<gpui::FocusHandle>,
    subscriptions: Vec<gpui::Subscription>,
    timer: Option<gpui::Task<()>>,
}
impl HoverState {
    fn hover(&mut self, hovered: bool, panel: bool, cx: &mut Context<Self>) {
        if panel {
            self.panel_hover = hovered;
        } else {
            if hovered && !self.trigger_hover {
                self.dismissed = false;
            }
            self.trigger_hover = hovered;
        }
        let hovered = self.trigger_hover || self.panel_hover || self.focused;
        self.timer.take();
        if hovered {
            if self.dismissed {
                return;
            }
            if self.focused || self.open {
                self.open = true;
                cx.notify();
            } else {
                self.timer = Some(cx.spawn(async move |state, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(250))
                        .await;
                    let _ = state.update(cx, |state, cx| {
                        state.open = !state.dismissed;
                        cx.notify();
                    });
                }));
            }
            return;
        }
        // Allow the pointer to cross the gap into the card.
        self.timer = Some(cx.spawn(async move |state, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(80))
                .await;
            let _ = state.update(cx, |state, cx| {
                state.open = false;
                cx.notify();
            });
        }));
    }
}

#[derive(Default)]
struct PopupPose {
    panel: FloatingPanel,
}

/// One keyed floating material follows the actual measured content and target.
#[derive(gpui::IntoElement)]
pub struct SlidingPopup<F: Fn(&mut Window, &mut gpui::App) -> gpui::AnyElement + 'static> {
    key: String,
    anchor: gpui::Bounds<gpui::Pixels>,
    width: f32,
    open: bool,
    content: F,
    hover: Option<super::liquid::panel::Hover>,
}
pub fn sliding_popup<F: Fn(&mut Window, &mut gpui::App) -> gpui::AnyElement + 'static>(
    key: impl Into<String>,
    _content_key: impl Into<String>,
    anchor: gpui::Bounds<gpui::Pixels>,
    width: f32,
    content: F,
) -> SlidingPopup<F> {
    SlidingPopup {
        key: key.into(),
        anchor,
        width,
        content,
        open: true,
        hover: None,
    }
}
impl<F: Fn(&mut Window, &mut gpui::App) -> gpui::AnyElement + 'static> SlidingPopup<F> {
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
    pub fn on_hover(
        mut self,
        listener: impl Fn(&bool, &mut Window, &mut gpui::App) + 'static,
    ) -> Self {
        self.hover = Some(std::rc::Rc::new(listener));
        self
    }
}
impl<F: Fn(&mut Window, &mut gpui::App) -> gpui::AnyElement + 'static> gpui::RenderOnce
    for SlidingPopup<F>
{
    fn render(self, window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let state = window.use_keyed_state(self.key.clone(), cx, |_, _| PopupPose::default());
        let content = Content {
            sections: vec![(self.content)(window, cx)],
            padding: 0.,
            gap: 0.,
        };
        state
            .update(cx, |value, cx| {
                value.panel.render(
                    format!("{}-surface", self.key),
                    self.anchor,
                    self.open,
                    FloatingStyle::details(self.width, Side::Above),
                    content,
                    self.hover,
                    window,
                    cx,
                )
            })
            .unwrap_or_else(|| gpui::Empty.into_any_element())
    }
}

/// One persistent surface per navigation group, shared by all its triggers.
#[derive(Default)]
pub struct DetailsOverlay {
    active: Option<DetailsTooltip>,
    outgoing: Vec<(DetailsTooltip, f32)>,
    painted_opacities: Vec<f32>,
    active_opacity: f32,
    content_started: Option<Instant>,
    panel: FloatingPanel,
    dismissing: bool,
    bounds: gpui::Bounds<gpui::Pixels>,
    trigger_hover: bool,
    panel_hover: bool,
    timer: Option<gpui::Task<()>>,
}
impl DetailsOverlay {
    #[cfg(feature = "stories")]
    pub(crate) fn inspect(&self) -> serde_json::Value {
        serde_json::json!({"active":self.active.is_some(), "closing":self.dismissing,
            "material":self.panel.inspect()})
    }

    fn show(
        &mut self,
        details: DetailsTooltip,
        bounds: gpui::Bounds<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.timer.take();
        self.dismissing = false;
        let now = Instant::now();
        if self.active.as_ref().is_some_and(|v| v.key != details.key) {
            for (layer, opacity) in self.outgoing.iter_mut().zip(&self.painted_opacities) {
                layer.1 = *opacity;
            }
            self.outgoing
                .push((self.active.as_ref().unwrap().clone(), self.active_opacity));
            self.outgoing.retain(|(_, opacity)| *opacity > 0.01);
            self.outgoing.sort_by(|a, b| b.1.total_cmp(&a.1));
            self.outgoing.truncate(4);
            self.painted_opacities = self.outgoing.iter().map(|(_, opacity)| *opacity).collect();
            self.active_opacity = 0.;
            self.content_started = Some(now);
        } else if self.active.is_none() {
            self.outgoing.clear();
            self.painted_opacities.clear();
            self.active_opacity = 1.;
            self.content_started = None;
        }
        self.active = Some(details);
        self.bounds = bounds;
        self.trigger_hover = true;
        self.panel_hover = false;
        cx.notify();
    }
    fn leave(&mut self, key: &str, cx: &mut Context<Self>) {
        // A late leave event from the previous row must not close the new target.
        if self.active.as_ref().is_some_and(|v| v.key == key) {
            self.trigger_hover = false;
            self.schedule_close(cx);
        }
    }
    fn schedule_close(&mut self, cx: &mut Context<Self>) {
        self.timer.take();
        if self.trigger_hover || self.panel_hover {
            if self.dismissing {
                self.dismissing = false;
                cx.notify();
            }
            return;
        }
        self.timer = Some(cx.spawn(async move |state, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(80))
                .await;
            let _ = state.update(cx, |state, cx| {
                state.dismissing = true;
                cx.notify();
            });
        }));
    }
}
impl gpui::Render for DetailsOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(details) = self.active.as_ref() else {
            return gpui::Empty.into_any_element();
        };
        let progress = if cx.reduce_motion() {
            1.
        } else {
            self.content_started.map_or(1., |start| {
                (start.elapsed().as_secs_f32() / CONTENT_SECONDS).min(1.)
            })
        };
        let fade = 1. - (1. - progress).powi(3);
        self.active_opacity = fade;
        self.painted_opacities = self
            .outgoing
            .iter()
            .map(|(_, opacity)| opacity * (1. - fade))
            .collect();
        if progress >= 1. {
            self.outgoing.clear();
            self.painted_opacities.clear();
        } else {
            window.request_animation_frame();
        }
        let content = div()
            .relative()
            .child(details.content().opacity(fade))
            .children(self.outgoing.iter().zip(&self.painted_opacities).map(
                |((details, _), opacity)| {
                    div()
                        .absolute()
                        .inset_0()
                        .opacity(*opacity)
                        .child(details.content())
                },
            ));
        let hover = std::rc::Rc::new(cx.listener(|v, inside: &bool, _, cx| {
            v.panel_hover = *inside;
            v.schedule_close(cx);
        }));
        let panel = self.panel.render(
            "detail-tooltip-shared",
            self.bounds,
            !self.dismissing,
            FloatingStyle::details(320., Side::Beside),
            Content::new(vec![content.into_any_element()]),
            Some(hover),
            window,
            cx,
        );
        if self.dismissing && !self.panel.alive() {
            self.active = None;
            self.outgoing.clear();
            self.painted_opacities.clear();
        }
        panel.unwrap_or_else(|| gpui::Empty.into_any_element())
    }
}

#[derive(gpui::IntoElement)]
pub struct TooltipTrigger<E: ControlElement> {
    row: crate::automation::element::AutomationElement<E>,
    details: DetailsTooltip,
    overlay: gpui::Entity<DetailsOverlay>,
}
pub fn trigger<E: ControlElement>(
    row: crate::automation::element::AutomationElement<E>,
    details: DetailsTooltip,
    overlay: gpui::Entity<DetailsOverlay>,
) -> TooltipTrigger<E> {
    TooltipTrigger {
        row,
        details,
        overlay,
    }
}
impl<E: ControlElement> gpui::RenderOnce for TooltipTrigger<E> {
    fn render(self, window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let bounds = window.use_keyed_state(
            format!("tooltip-anchor-{}", self.details.key),
            cx,
            |_, _| gpui::Bounds::default(),
        );
        let anchor = bounds.clone();
        self.row.map_inner(|row| {
            row.on_hover(move |hovered, _, cx| {
                let rect = *bounds.read(cx);
                self.overlay.update(cx, |v, cx| {
                    if *hovered {
                        v.show(self.details.clone(), rect, cx);
                    } else {
                        v.leave(&self.details.key, cx);
                    }
                });
            })
            .control_overlay(
                gpui::canvas(
                    move |bounds, _, cx| {
                        anchor.update(cx, |v, _| *v = bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .into_any_element(),
            )
        })
    }
}

/// Shared short-hint surface, used by controls and glyph-level text tooltips.
fn hint_surface(
    id: impl Into<gpui::SharedString>,
    text: gpui::SharedString,
) -> crate::automation::element::AutomationElement<gpui::Stateful<gpui::Div>> {
    let id: gpui::SharedString = id.into();
    crate::components::liquid::primitives::surface(id, 9., CUE_UI.palette.canvas, true)
        .role(gpui::Role::Tooltip)
        .aria_label(text.clone())
        .px(px(8.))
        .py(px(5.))
        .text_color(rgb(CUE_UI.palette.text))
        .text_size(px(12.))
        .line_height(px(20.))
        .whitespace_normal()
        .child(text.clone())
        .automation(AutomationRole::Status, text.to_string())
}

/// A view adapter for callers whose trigger already owns positioning and hover.
pub struct Hint {
    id: gpui::SharedString,
    text: gpui::SharedString,
}
impl Hint {
    pub fn new(id: impl Into<gpui::SharedString>, text: impl Into<gpui::SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}
impl gpui::Render for Hint {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        hint_surface(self.id.clone(), self.text.clone())
            .map_inner(|card| card.max_w((window.viewport_size().width - px(24.)).min(px(480.))))
    }
}

use crate::components::liquid::controls::ControlElement;

/// A short control hint, centred below its trigger and flipped above near the edge.
#[derive(gpui::IntoElement)]
pub struct HintTrigger<E: ControlElement> {
    row: crate::automation::element::AutomationElement<E>,
    key: String,
    text: String,
    focus: Option<gpui::FocusHandle>,
}
impl<E: ControlElement> HintTrigger<E> {
    pub fn focus_handle(mut self, focus: &gpui::FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
}
pub fn hint<E: ControlElement>(
    row: crate::automation::element::AutomationElement<E>,
    key: impl Into<String>,
    text: impl Into<String>,
) -> HintTrigger<E> {
    HintTrigger {
        row,
        key: key.into(),
        text: text.into(),
        focus: None,
    }
}
impl<E: ControlElement> gpui::RenderOnce for HintTrigger<E> {
    fn render(self, window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let state = window.use_keyed_state(format!("hint-state-{}", self.key), cx, |_, _| {
            HoverState::default()
        });
        let focus = self.focus.clone().or_else(|| {
            gpui::Element::id(&self.row)
                .map(|id| crate::components::liquid::controls::action_focus(id, window, cx))
        });
        if let Some(focus) = focus.clone() {
            state.update(cx, |v, cx| {
                if v.focus.as_ref() != Some(&focus) {
                    v.subscriptions.clear();
                    v.focus = Some(focus.clone());
                    v.subscriptions
                        .push(cx.on_focus(&focus, window, |v, _, cx| {
                            v.focused = true;
                            v.dismissed = false;
                            v.hover(v.trigger_hover, false, cx);
                        }));
                    v.subscriptions.push(cx.on_blur(&focus, window, |v, _, cx| {
                        v.focused = false;
                        v.hover(v.trigger_hover, false, cx);
                    }));
                }
            });
        }
        let bounds = state.read(cx).bounds;
        let open = state.read(cx).open;
        let anchor_state = state.clone();
        let hover_state = state.clone();
        let panel_state = state.clone();
        let escape_state = state.clone();
        let width = (crate::components::liquid::overlay::measure_label(&self.text, 12., window)
            + 16.)
            .min(360.);
        let text = self.text.clone();
        let content = div()
            .id(format!("control-hint-{}", self.key))
            .px(px(8.))
            .py(px(5.))
            .text_size(px(12.))
            .line_height(px(20.))
            .text_color(rgb(CUE_UI.palette.text))
            .whitespace_normal()
            .child(text.clone())
            .automation(AutomationRole::Status, text);
        let hover = std::rc::Rc::new(move |inside: &bool, _: &mut Window, cx: &mut gpui::App| {
            panel_state.update(cx, |value, cx| value.hover(*inside, true, cx));
        });
        let panel = state.update(cx, |value, cx| {
            value.panel.render(
                format!("hint-panel-{}", self.key),
                bounds,
                open,
                FloatingStyle {
                    width,
                    side: Side::Below,
                    radius: 9.,
                    priority: 110,
                    role: gpui::Role::Tooltip,
                },
                Content {
                    sections: vec![content.into_any_element()],
                    padding: 0.,
                    gap: 0.,
                },
                Some(hover),
                window,
                cx,
            )
        });
        self.row.map_inner(|row| {
            row.when_some(focus, |row, focus| row.control_focus(&focus))
                .aria_description(self.text.clone())
                .on_hover(move |hovered, _, cx| {
                    hover_state.update(cx, |v, cx| v.hover(*hovered, false, cx))
                })
                .on_key_down(move |event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" && escape_state.read(cx).open {
                        escape_state.update(cx, |v, cx| {
                            v.open = false;
                            v.dismissed = true;
                            v.timer.take();
                            cx.notify();
                        });
                        cx.stop_propagation();
                    }
                })
                .control_overlay(
                    gpui::canvas(
                        move |bounds, _, cx| anchor_state.update(cx, |v, _| v.bounds = bounds),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0()
                    .into_any_element(),
                )
                .when_some(panel, |row, panel| row.control_overlay(panel))
        })
    }
}

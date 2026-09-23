//! Read-only hover details shared by navigation and component examples.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, Context, FontWeight, Window};

use super::widgets::panel::{Content, FloatingPanel, FloatingStyle, Side};

#[derive(Clone)]
pub struct DetailsTooltip {
    pub key: String,
    pub title: String,
    pub kind: String,
    pub description: String,
    pub rows: Vec<(String, String)>,
}
impl DetailsTooltip {
    fn content_width(&self, window: &mut Window) -> f32 {
        let measure = |text: &str, size, window: &mut Window| {
            text.lines()
                .map(|line| super::widgets::overlay::measure_label(line, size, window))
                .fold(0., f32::max)
        };
        let header = 42. + measure(&self.title, 13., window).max(measure(&self.kind, 10., window));
        let description: String = self.description.trim().chars().take(320).collect();
        let mut content = header.max(measure(&description, 12., window));
        for (label, value) in self
            .rows
            .iter()
            .filter(|(_, value)| !value.trim().is_empty())
        {
            content = content.max(60. + 8. + measure(value, 11., window));
            content = content.max(measure(label, 11., window) + 8. + measure(value, 11., window));
        }
        (content.ceil() + 32.)
            .min(320.)
            .min((window.viewport_size().width.as_f32() - 24.).max(2.))
    }

    pub fn content(&self) -> gpui::Div {
        let p = ZORK_UI.palette;
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
                    .child(
                        div()
                            .size(px(32.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(12.))
                            .bg(rgb(p.prompt))
                            .child(ui::icon("icons/checklist.svg", 18.)),
                    )
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
    pub fn card(
        &self,
        window: &mut Window,
    ) -> crate::automation::element::AutomationElement<gpui::Stateful<gpui::Div>> {
        Self::surface(format!("detail-tooltip-{}", self.key))
            .w(px(self.content_width(window)))
            .p(px(16.))
            .child(self.content())
            .automation(
                AutomationRole::Status,
                format!("{}详情：{}", self.kind, self.title),
            )
    }
    fn surface(id: String) -> gpui::Stateful<gpui::Div> {
        crate::components::widgets::primitives::surface(
            id,
            ui::CARD_RADIUS,
            ZORK_UI.palette.canvas,
            true,
        )
        .occlude()
        .max_w(px(320.))
        .flex()
        .flex_col()
    }
}

impl gpui::Render for DetailsTooltip {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        div()
            .id(format!("detail-tooltip-scroll-{}", self.key))
            .max_w(viewport.width - px(24.))
            .child(
                self.card(window)
                    .map_inner(|card| card.max_h(viewport.height - px(24.)).overflow_y_scroll()),
            )
    }
}

#[derive(Default)]
struct HoverState {
    panel: FloatingPanel,
    window_state: Option<gpui::Entity<HintWindowState>>,
    bounds: gpui::Bounds<gpui::Pixels>,
    open: bool,
    trigger_hover: bool,
    panel_hover: bool,
    focused: bool,
    suppress_focus_until_blur: bool,
    dismissed: bool,
    focus: Option<gpui::FocusHandle>,
    subscriptions: Vec<gpui::Subscription>,
    timer: Option<gpui::Task<()>>,
}
impl HoverState {
    fn close_for_switch(&mut self, cx: &mut Context<Self>) {
        self.timer.take();
        self.panel = FloatingPanel::default();
        self.open = false;
        self.trigger_hover = false;
        self.panel_hover = false;
        self.focused = false;
        self.suppress_focus_until_blur = true;
        self.dismissed = true;
        cx.notify();
    }
    fn hover(&mut self, hovered: bool, panel: bool, cx: &mut Context<Self>) {
        if panel {
            self.panel_hover = hovered;
        } else {
            if hovered && !self.trigger_hover {
                self.dismissed = false;
                self.suppress_focus_until_blur = false;
            }
            self.trigger_hover = hovered;
        }
        let hovered = self.trigger_hover || self.panel_hover || self.focused;
        self.timer.take();
        if hovered {
            if !self.dismissed && !self.open {
                self.open = true;
                if let Some(window_state) = &self.window_state {
                    let current = cx.entity().downgrade();
                    window_state.update(cx, |state, cx| state.activate(current, cx));
                }
                cx.notify();
            }
            return;
        }
        if !self.open {
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
struct HintWindowState {
    current: Option<gpui::WeakEntity<HoverState>>,
}
impl HintWindowState {
    fn activate(&mut self, current: gpui::WeakEntity<HoverState>, cx: &mut Context<Self>) {
        if self.current.as_ref() == Some(&current) {
            return;
        }
        if let Some(previous) = self.current.take() {
            let _ = previous.update(cx, |previous, cx| previous.close_for_switch(cx));
        }
        self.current = Some(current);
    }
}

/// One persistent surface per navigation group, shared by all its triggers.
#[derive(Default)]
pub struct DetailsOverlay {
    active: Option<DetailsTooltip>,
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
        let content = details.content();
        let hover = std::rc::Rc::new(cx.listener(|v, inside: &bool, _, cx| {
            v.panel_hover = *inside;
            v.schedule_close(cx);
        }));
        let width = details.content_width(window);
        let panel = self.panel.render(
            "detail-tooltip-shared",
            self.bounds,
            !self.dismissing,
            FloatingStyle::details(width, Side::Beside),
            Content::new(vec![content.into_any_element()]),
            Some(hover),
            window,
            cx,
        );
        if self.dismissing && !self.panel.alive() {
            self.active = None;
        }
        panel.unwrap_or_else(|| gpui::Empty.into_any_element())
    }
}

#[derive(gpui::IntoElement)]
pub struct TooltipTrigger<E: ControlElement> {
    row: crate::automation::element::AutomationElement<E>,
    details: DetailsTooltip,
    overlay: gpui::Entity<DetailsOverlay>,
    hover: Option<std::rc::Rc<dyn Fn(bool, &mut gpui::App)>>,
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
        hover: None,
    }
}
pub fn trigger_with_hover<E: ControlElement>(
    row: crate::automation::element::AutomationElement<E>,
    details: DetailsTooltip,
    overlay: gpui::Entity<DetailsOverlay>,
    hover: impl Fn(bool, &mut gpui::App) + 'static,
) -> TooltipTrigger<E> {
    TooltipTrigger {
        row,
        details,
        overlay,
        hover: Some(std::rc::Rc::new(hover)),
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
        let hover = self.hover;
        self.row.map_inner(|row| {
            row.on_hover(move |hovered, _, cx| {
                if let Some(callback) = &hover {
                    callback(*hovered, cx);
                }
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
    crate::components::widgets::primitives::surface(id, 9., ZORK_UI.palette.canvas, true)
        .role(gpui::Role::Tooltip)
        .aria_label(text.clone())
        .px(px(8.))
        .py(px(5.))
        .text_color(rgb(ZORK_UI.palette.text))
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
        hint_surface(self.id.clone(), self.text.clone()).map_inner(|card| {
            card.max_w(
                (window.viewport_size().width - px(24.))
                    .max(px(2.))
                    .min(px(480.)),
            )
            .max_h((window.viewport_size().height - px(24.)).max(px(2.)))
            .overflow_y_scroll()
        })
    }
}

use crate::components::widgets::controls::ControlElement;

/// A short control hint, centred below its trigger and flipped above near the edge.
#[derive(gpui::IntoElement)]
pub struct HintTrigger<E: ControlElement> {
    row: crate::automation::element::AutomationElement<E>,
    key: String,
    text: String,
    focus: Option<gpui::FocusHandle>,
    side: Side,
    min_width: f32,
}
impl<E: ControlElement> HintTrigger<E> {
    pub fn above(mut self) -> Self {
        self.side = Side::Above;
        self
    }
    pub fn focus_handle(mut self, focus: &gpui::FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
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
        side: Side::Below,
        min_width: 0.,
    }
}
impl<E: ControlElement> gpui::RenderOnce for HintTrigger<E> {
    fn render(self, window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let window_state =
            window.use_keyed_state("hint-window-state", cx, |_, _| HintWindowState::default());
        let state = window.use_keyed_state(format!("hint-state-{}", self.key), cx, |_, _| {
            HoverState::default()
        });
        let focus = self.focus.clone().or_else(|| {
            gpui::Element::id(&self.row)
                .map(|id| crate::components::widgets::controls::action_focus(id, window, cx))
        });
        state.update(cx, |v, _| v.window_state = Some(window_state.clone()));
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
                        v.suppress_focus_until_blur = false;
                        v.hover(v.trigger_hover, false, cx);
                    }));
                }
            });
        }
        let actually_focused = focus.as_ref().is_some_and(|focus| focus.is_focused(window));
        state.update(cx, |v, cx| {
            if !actually_focused {
                v.suppress_focus_until_blur = false;
            }
            let focused = actually_focused && !v.suppress_focus_until_blur;
            if v.focused != focused {
                v.focused = focused;
                if focused {
                    v.dismissed = false;
                }
                v.hover(v.trigger_hover, false, cx);
            }
        });
        let bounds = state.read(cx).bounds;
        let open = state.read(cx).open;
        let anchor_state = state.clone();
        let hover_state = state.clone();
        let panel_state = state.clone();
        let escape_state = state.clone();
        let panel = if open || state.read(cx).panel.alive() {
            // Avoid measuring and building every hidden hint in long control lists.
            // Round glyph widths outwards so short labels do not wrap at the edge.
            let natural_width =
                crate::components::widgets::overlay::measure_label(&self.text, 12., window).ceil()
                    + 18.;
            let available_width = (window.viewport_size().width.as_f32() - 24.)
                .max(2.)
                .min(360.);
            let width = natural_width.max(self.min_width).min(available_width);
            let single_line = natural_width <= available_width;
            let text = self.text.clone();
            let content = div()
                .id(format!("control-hint-{}", self.key))
                .px(px(8.))
                .py(px(5.))
                .text_size(px(12.))
                .line_height(px(20.))
                .text_color(rgb(ZORK_UI.palette.text))
                .when(single_line, |v| v.whitespace_nowrap())
                .when(!single_line, |v| v.whitespace_normal())
                .child(text.clone())
                .automation(AutomationRole::Status, text);
            let hover =
                std::rc::Rc::new(move |inside: &bool, _: &mut Window, cx: &mut gpui::App| {
                    panel_state.update(cx, |value, cx| value.hover(*inside, true, cx));
                });
            state.update(cx, |value, cx| {
                value.panel.render(
                    format!("hint-panel-{}", self.key),
                    bounds,
                    open,
                    FloatingStyle {
                        width,
                        side: self.side,
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
            })
        } else {
            None
        };
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
                        move |bounds, _, cx| {
                            anchor_state.update(cx, |v, cx| {
                                if v.bounds != bounds {
                                    v.bounds = bounds;
                                    if v.open || v.panel.alive() {
                                        cx.notify();
                                    }
                                }
                            });
                        },
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

//! Message preview sizing, including pointer capture and draft/commit lifecycle.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::CUE_UI,
    resources::Text,
};
use gpui::{prelude::*, *};
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Data {
    pub height: u32,
    pub automatic: u32,
    pub minimum: u32,
    pub maximum: u32,
}
pub struct Changed(pub u32);
pub struct Appearance {
    data: Data,
    text: Text,
    drag: Option<(f32, u32)>,
    draft: Option<u32>,
}
impl EventEmitter<Changed> for Appearance {}
pub fn dragged_height(start: u32, delta: f32, minimum: u32, maximum: u32) -> u32 {
    (start as f32 + delta)
        .round()
        .clamp(minimum as f32, maximum as f32) as u32
}
impl Appearance {
    pub fn new(data: Data, text: Text, _: &mut Context<Self>) -> Self {
        Self {
            data,
            text,
            drag: None,
            draft: None,
        }
    }
    pub fn configure(&mut self, data: Data, text: Text, cx: &mut Context<Self>) {
        let changed = self.data != data
            || self.text.text("client_message_preview_sample")
                != text.text("client_message_preview_sample");
        self.data = data;
        self.text = text;
        if changed {
            cx.notify();
        }
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.draft = None;
            cx.notify();
        }
    }
    fn commit(&mut self, value: u32, cx: &mut Context<Self>) {
        self.drag = None;
        self.draft = None;
        cx.emit(Changed(value));
        cx.notify();
    }
}
impl Render for Appearance {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = CUE_UI.palette;
        let height = self.draft.unwrap_or(if self.data.height == 0 {
            self.data.automatic
        } else {
            self.data.height
        });
        let owner = cx.entity().downgrade();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(crate::settings::row(
                self.text.text("client_message_preview_height"),
                self.text.text("client_message_preview_height_detail"),
                ui::button(
                    "message-preview-reset",
                    self.text.text("client_message_preview_auto"),
                    false,
                    true,
                )
                .on_click(cx.listener(|v, _, _, cx| v.commit(0, cx)))
                .automation(
                    AutomationRole::Button,
                    self.text.text("client_message_preview_auto"),
                ),
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(p.muted))
                    .child(format!("{height} px")),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .border(px(crate::design::BORDER_WIDTH))
                    .border_color(rgb(p.border))
                    .rounded_lg()
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(height as f32))
                            .flex_shrink_0()
                            .overflow_hidden()
                            .child(
                                div().p_4().text_size(px(13.)).line_height(px(20.)).child(
                                    self.text.text("client_message_preview_sample").repeat(8),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .id("message-preview-resize")
                            .h(px(20.))
                            .flex_shrink_0()
                            .w_full()
                            .border_t(px(crate::design::BORDER_WIDTH))
                            .border_color(rgb(p.border))
                            .cursor_row_resize()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().w(px(40.)).h(px(3.)).rounded_full().bg(rgb(p.muted)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |v, event: &MouseDownEvent, _, cx| {
                                    v.drag = Some((event.position.y.as_f32(), height));
                                    v.draft = Some(height);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            )
                            .automation(
                                AutomationRole::Button,
                                self.text.text("client_message_preview_drag"),
                            ),
                    ),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let moving = owner.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            let _ = moving.update(cx, |v, cx| {
                                if let Some((start_y, start_height)) = v.drag {
                                    if !event.dragging() {
                                        v.cancel(cx);
                                        return;
                                    }
                                    let height = dragged_height(
                                        start_height,
                                        event.position.y.as_f32() - start_y,
                                        v.data.minimum,
                                        v.data.maximum,
                                    );
                                    if v.draft != Some(height) {
                                        v.draft = Some(height);
                                        cx.notify();
                                    }
                                    cx.stop_propagation();
                                }
                            });
                        });
                        let ended = owner.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture || event.button != MouseButton::Left
                            {
                                return;
                            }
                            let _ = ended.update(cx, |v, cx| {
                                if v.drag.is_some() {
                                    if let Some(height) = v.draft {
                                        v.commit(height, cx);
                                        cx.stop_propagation();
                                    }
                                }
                            });
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

#[cfg(feature = "stories")]
pub struct Story {
    view: Entity<Appearance>,
    data: Data,
    text: Text,
}
#[cfg(feature = "stories")]
impl Story {
    pub fn new(state: &str, text: Text, cx: &mut Context<Self>) -> Self {
        let data = Data {
            height: match state {
                "minimum" => 80,
                "custom" => 360,
                "maximum" => 720,
                _ => 0,
            },
            automatic: 240,
            minimum: 80,
            maximum: 720,
        };
        let view = cx.new(|cx| Appearance::new(data, text.clone(), cx));
        cx.subscribe(&view, |v, _, event: &Changed, cx| {
            v.data.height = event.0;
            v.view
                .update(cx, |view, cx| view.configure(v.data, v.text.clone(), cx));
        })
        .detach();
        Self { view, data, text }
    }
}
#[cfg(feature = "stories")]
impl Render for Story {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.view.clone()
    }
}

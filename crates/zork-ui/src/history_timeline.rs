//! Complete execution timeline, including picking, pan/zoom, range selection and hover material.
use crate::history::{self as model, Entry};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::CUE_UI,
};
use gpui::{prelude::*, *};
use std::{cell::Cell, rc::Rc};
const DIM: u32 = CUE_UI.palette.muted;
const BORDER: u32 = CUE_UI.palette.border;
#[derive(Default)]
struct State {
    entries: zork_observe::List<Entry>,
    now: i64,
    selected: Option<String>,
    hovered: Option<String>,
    hover_anchor: Bounds<Pixels>,
    hover_close: Option<Task<()>>,
    zoom: f64,
    pan: f64,
    range: Option<(f64, f64)>,
    dragging: bool,
    bounds: Rc<Cell<Bounds<Pixels>>>,
}
impl State {
    fn now(&self) -> i64 {
        self.now
    }
}
pub struct Selected(pub usize);
pub struct Changed;
pub struct Timeline {
    state: State,
    text: crate::resources::Text,
    popup: crate::components::liquid::panel::FloatingPanel,
    retained: Option<(Entry, Bounds<Pixels>)>,
}
impl EventEmitter<Selected> for Timeline {}
impl EventEmitter<Changed> for Timeline {}
impl Timeline {
    pub fn new(text: crate::resources::Text, _: &mut Context<Self>) -> Self {
        Self {
            state: State {
                zoom: 1.,
                ..Default::default()
            },
            text,
            popup: Default::default(),
            retained: None,
        }
    }
    pub fn configure(
        &mut self,
        entries: zork_observe::List<Entry>,
        selected: Option<String>,
        now: i64,
        text: crate::resources::Text,
        cx: &mut Context<Self>,
    ) {
        let changed = !self.state.entries.ptr_eq(&entries)
            || self.state.selected != selected
            || self.state.now != now
            || self.text.text("history_tool") != text.text("history_tool");
        self.state.entries = entries;
        self.state.selected = selected;
        self.state.now = now;
        self.text = text;
        if changed {
            cx.notify();
        }
    }
    pub fn selection_range(&self) -> Option<(i64, i64)> {
        self.state.range.map(|(a, b)| {
            let timeline = model::timeline(self.state.entries.iter(), self.state.now);
            let axis = model::Axis::new(self.state.entries.iter(), &timeline);
            (axis.time_at(a.min(b)), axis.time_at(a.max(b)))
        })
    }
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.state.hovered = None;
        self.state.hover_close = None;
        cx.notify();
    }
    fn history_action(&self, e: &Entry) -> String {
        match e.lane {
            1 => self
                .text
                .text(match e.action.as_str() {
                    "compaction" => "history_compaction",
                    "handoff" => "history_handoff",
                    _ => "history_model",
                })
                .into(),
            0 if e.action == "input" => self.text.text("history_input").into(),
            _ => e.action.clone(),
        }
    }
    fn history_state(&self, e: &Entry) -> String {
        self.text.text(match e.state.as_str() {
            "running" => "history_running",
            "succeeded" => "history_success",
            "received" => "history_received",
            "cancelled" | "interrupted" => "history_cancelled",
            "failed" | "timed_out" => "history_error",
            _ => "history_notice",
        })
    }
    fn render_history_bars(
        &self,
        lane: usize,
        row_height: f32,
        timeline: &model::Timeline,
        position: &impl Fn(i64) -> f64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let automated = crate::automation::element::is_enabled(cx);
        let bars = Rc::new(
            timeline
                .spans
                .iter()
                .filter_map(|span| {
                    let entry = &self.state.entries[span.index];
                    let left = position(span.start);
                    let right = position(span.end);
                    if entry.lane != lane || right < 0. || left > 1. {
                        return None;
                    }
                    Some(HistoryBar {
                        index: span.index,
                        id: entry.id.clone(),
                        left: left.max(0.) as f32,
                        width: (right.min(1.) - left.max(0.)).max(0.) as f32,
                        top: span.track as f32 * row_height,
                        height: 11.,
                        color: history_color(entry),
                        open: entry.end.is_none(),
                        selected: self.state.selected.as_ref() == Some(&entry.id),
                        layout: Default::default(),
                        label: automated.then(|| {
                            format!(
                                "{} · {} · {}",
                                self.history_action(entry),
                                self.history_state(entry),
                                entry.summary.chars().take(80).collect::<String>()
                            )
                        }),
                    })
                })
                .collect::<Vec<_>>(),
        );
        let bounds = Rc::new(std::cell::Cell::new(gpui::Bounds::default()));
        let hover_bars = bars.clone();
        let hover_bounds = bounds.clone();
        let leave_bounds = bounds.clone();
        let click_bars = bars.clone();
        let click_bounds = bounds.clone();
        let layout_owner = cx.entity().downgrade();
        div()
            .id(("history-bars", lane))
            .absolute()
            .size_full()
            .on_mouse_move(
                cx.listener(move |v, event: &gpui::MouseMoveEvent, window, cx| {
                    if let Some(bar) =
                        history_bar_at(&hover_bars, hover_bounds.get(), event.position)
                    {
                        v.state.hover_close.take();
                        let anchor = painted_history_anchor(
                            bar.bounds(hover_bounds.get()),
                            window.scale_factor(),
                        );
                        if v.state.hovered.as_ref() != Some(&bar.id)
                            || v.state.hover_anchor != anchor
                        {
                            v.state.hovered = Some(bar.id.clone());
                            v.state.hover_anchor = anchor;
                            cx.notify();
                        }
                    } else {
                        v.history_leave_hover(cx);
                    }
                }),
            )
            .on_hover(cx.listener(move |v, hovered, _, cx| {
                if !hovered && leave_bounds.get().contains(&v.state.hover_anchor.center()) {
                    v.history_leave_hover(cx);
                }
            }))
            .on_click(cx.listener(move |_v, event: &gpui::ClickEvent, _, cx| {
                if let Some(bar) = history_bar_at(&click_bars, click_bounds.get(), event.position())
                {
                    cx.emit(Selected(bar.index));
                }
            }))
            .child(
                gpui::canvas(
                    move |area, window, cx| {
                        bounds.set(area);
                        layout_history_bars(&bars, area, lane == 1);
                        // Wheel zoom/pan, new data and a moving axis can change
                        // the hit target without any MouseMoveEvent.
                        let hovered =
                            history_bar_at(&bars, area, window.mouse_position()).map(|bar| {
                                (
                                    bar.id.clone(),
                                    painted_history_anchor(bar.bounds(area), window.scale_factor()),
                                )
                            });
                        let owner = layout_owner.clone();
                        cx.defer(move |cx| {
                            let _ = owner.update(cx, |v, cx| {
                                if let Some((id, anchor)) = hovered {
                                    v.state.hover_close.take();
                                    if v.state.hovered.as_ref() != Some(&id)
                                        || v.state.hover_anchor != anchor
                                    {
                                        v.state.hovered = Some(id);
                                        v.state.hover_anchor = anchor;
                                        cx.notify();
                                    }
                                } else if area.contains(&v.state.hover_anchor.center()) {
                                    v.history_leave_hover(cx);
                                }
                            });
                        });
                        let hitboxes = bars
                            .iter()
                            .map(|bar| {
                                let rect = bar.bounds(area);
                                if let Some(label) = &bar.label {
                                    crate::automation::element::record_canvas_control(
                                        cx,
                                        window,
                                        format!("{}-{}", "history-bar", bar.index),
                                        label.clone(),
                                        rect,
                                    );
                                }
                                window.insert_hitbox(rect, gpui::HitboxBehavior::Normal)
                            })
                            .collect::<Vec<_>>();
                        (bars, hitboxes)
                    },
                    move |area, (bars, hitboxes), window, _| {
                        for hitbox in &hitboxes {
                            window.set_cursor_style(gpui::CursorStyle::PointingHand, hitbox);
                        }
                        // Batch consecutive quads in one layer, avoiding a scene ordering
                        // tree insertion for every overlapping point marker. Open paths
                        // retain separate ordering relative to the quads around them.
                        let mut start = 0;
                        while start < bars.len() {
                            if bars[start].open {
                                bars[start].paint(area, window);
                                start += 1;
                            } else {
                                let end = bars[start..]
                                    .iter()
                                    .position(|bar| bar.open)
                                    .map_or(bars.len(), |offset| start + offset);
                                window.paint_layer(area, |window| {
                                    for bar in &bars[start..end] {
                                        bar.paint(area, window);
                                    }
                                });
                                start = end;
                            }
                        }
                    },
                )
                .size_full(),
            )
    }

    fn render_history_timeline(&self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let now = self.state.now();
        let t = model::timeline(self.state.entries.iter(), now);
        let axis = model::Axis::new(self.state.entries.iter(), &t);
        let zoom = self.state.zoom;
        let pan = self.state.pan;
        let start = axis.time_at(pan);
        let end = axis.time_at(pan + 1. / zoom);
        let position = |time: i64| (axis.position(time) - pan) * zoom;
        let row_height = 11.;
        let mut offsets = [0.; 3];
        let mut height = 4.;
        for lane in 0..3 {
            offsets[lane] = height;
            height += t.tracks[lane].max(1) as f32 * row_height;
        }
        let bounds = self.state.bounds.clone();
        div()
            .flex()
            .flex_col()
            .relative()
            .map(|d| {
                d.child(
                    div()
                        .h(px(18.))
                        .ml(px(44.))
                        .pr(px(12.))
                        .flex()
                        .justify_between()
                        .pt(px(3.))
                        .text_size(px(10.))
                        .font_family("Menlo")
                        .text_color(rgb(DIM))
                        .children((0..5).map(|i| {
                            let fraction = i as f64 / 4.;
                            let time = axis.time_at(pan + fraction / zoom);
                            div().child(model::axis_clock(time, end - start))
                        })),
                )
            })
            .child(
                div()
                    .id("history-plot")
                    .max_h(px(128.))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .relative()
                            .h(px(height))
                            .map(|d| {
                                d.child(
                                    gpui::canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                                        .absolute()
                                        .size_full(),
                                )
                            })
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left(px(44.))
                                    .right_0()
                                    .h(px(3.))
                                    .children(axis.gaps.iter().filter_map(|gap| {
                                        let x = position(gap.0);
                                        (0. ..=1.).contains(&x).then(|| {
                                            div()
                                                .absolute()
                                                .left(relative(x as f32))
                                                .ml(px(-1.5))
                                                .size(px(3.))
                                                .rounded(px(1.5))
                                                .bg(rgb(0x9CA3AF))
                                        })
                                    })),
                            )
                            .children((0..3).map(|lane| {
                                let lane_height = t.tracks[lane].max(1) as f32 * row_height;
                                div()
                                    .absolute()
                                    .top(px(offsets[lane]))
                                    .h(px(lane_height))
                                    .w_full()
                                    .flex()
                                    .child(
                                        div()
                                            .w(px(44.))
                                            .h_full()
                                            .flex_shrink_0()
                                            .px(px(6.))
                                            .text_right()
                                            .text_size(px(10.))
                                            .line_height(px(11.))
                                            .text_color(rgb(DIM))
                                            .child(self.text.text(
                                                [
                                                    "history_lane_input",
                                                    "history_lane_model",
                                                    "history_tool",
                                                ][lane],
                                            )),
                                    )
                                    .child(
                                        div()
                                            .relative()
                                            .flex_1()
                                            .h_full()
                                            .overflow_hidden()
                                            .bg(rgb(CUE_UI.palette.canvas))
                                            .map(|d| {
                                                d.children(self.state.range.map(|(a, b)| {
                                                    let left =
                                                        ((a.min(b) - pan) * zoom).clamp(0., 1.);
                                                    let right =
                                                        ((a.max(b) - pan) * zoom).clamp(0., 1.);
                                                    div()
                                                        .absolute()
                                                        .h_full()
                                                        .left(relative(left as f32))
                                                        .w(relative((right - left) as f32))
                                                        .bg(rgba(
                                                            (crate::components::history::SEND_COLOR
                                                                << 8)
                                                                | 0x1A,
                                                        ))
                                                        .border_x(gpui::px(
                                                            crate::design::BORDER_WIDTH,
                                                        ))
                                                        .border_color(rgba(
                                                            (crate::components::history::SEND_COLOR
                                                                << 8)
                                                                | 0x4D,
                                                        ))
                                                }))
                                            })
                                            .child(self.render_history_bars(
                                                lane, row_height, &t, &position, cx,
                                            )),
                                    )
                            })),
                    ),
            )
    }

    fn history_leave_hover(&mut self, cx: &mut Context<Self>) {
        if self.state.hovered.is_none() || self.state.hover_close.is_some() {
            return;
        }
        self.state.hover_close = Some(cx.spawn(async move |view, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(80))
                .await;
            let _ = view.update(cx, |v, cx| {
                v.state.hovered = None;
                v.state.hover_close = None;
                cx.notify();
            });
        }));
    }
    fn render_history_span_detail(&self, e: &Entry) -> impl IntoElement {
        let now = self.state.now();
        let color = history_color(e);
        div()
            .id("history-span-detail")
            .w_full()
            .px(px(2.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .text_size(px(12.))
            .line_height(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(div().size(px(7.)).rounded(px(2.)).bg(rgb(color)))
                            .child(if e.lane == 2 {
                                self.text.text("history_tool").to_owned()
                            } else {
                                self.history_action(e)
                            }),
                    )
                    .child(
                        div()
                            .px(px(6.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .bg(rgb(crate::design::CUE_UI.palette.sidebar_hover))
                            .text_size(px(10.))
                            .text_color(rgb(DIM))
                            .child(self.history_state(e)),
                    ),
            )
            .when(e.lane == 2, |d| {
                d.child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(e.action.clone()),
                )
            })
            .child(
                div()
                    .max_h(px(54.))
                    .overflow_hidden()
                    .child(e.summary.clone()),
            )
            .when_some(e.duration(now).filter(|_| e.lane != 0), |d, ms| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(DIM))
                                .child(self.text.text("history_duration")),
                        )
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(model::duration(ms)),
                        ),
                )
            })
            .when(e.lane == 1, |d| d.child(self.render_history_metrics(e)))
            .child(div().text_size(px(10.)).text_color(rgb(DIM)).child(format!(
                "{}  →  {}",
                model::clock(e.start),
                model::clock(e.end)
            )))
            .automation(
                AutomationRole::Status,
                format!(
                    "{} · {}{}",
                    e.summary,
                    self.history_state(e),
                    e.duration(now)
                        .filter(|_| e.lane != 0)
                        .map(|ms| format!(" · {}", model::duration(ms)))
                        .unwrap_or_default()
                ),
            )
    }
    fn render_history_metrics(&self, e: &Entry) -> Div {
        crate::components::history::metrics(
            e,
            &self.text.text("history_input_tokens"),
            &self.text.text("history_output_tokens"),
            &self.text.text("history_cache"),
        )
    }
    fn render_history_timeline_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("history-timeline-panel")
            .pb(px(4.))
            .flex_shrink_0()
            .border_t(gpui::px(crate::design::BORDER_WIDTH))
            .border_color(rgb(BORDER))
            .child(
                div()
                    .id("history-axis")
                    .relative()
                    .on_scroll_wheel(cx.listener(|v, event: &gpui::ScrollWheelEvent, _, cx| {
                        let delta = event.delta.pixel_delta(px(16.));
                        let h = &mut v.state;
                        let bounds = h.bounds.get();
                        let width = (f32::from(bounds.size.width) - 44.).max(1.) as f64;
                        let x = f32::from(delta.x) as f64;
                        let y = f32::from(delta.y) as f64;
                        if event.modifiers.shift || x.abs() > y.abs() {
                            h.pan -= if x.abs() > 0. { x } else { y } / width / h.zoom;
                        } else if y != 0. {
                            let pointer = ((f32::from(event.position.x - bounds.origin.x) as f64
                                - 44.)
                                / width)
                                .clamp(0., 1.);
                            let old = 1. / h.zoom;
                            h.zoom = (h.zoom * (y * 0.006).exp()).clamp(1., 64.);
                            h.pan += pointer * (old - 1. / h.zoom);
                        }
                        h.pan = h.pan.clamp(0., 1. - 1. / h.zoom);
                        h.range = None;
                        cx.stop_propagation();
                        cx.emit(Changed);
                        cx.notify();
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|v, event: &gpui::MouseDownEvent, _, cx| {
                            let h = &mut v.state;
                            let b = h.bounds.get();
                            let width = f32::from(b.size.width) - 44.;
                            if width > 0. {
                                let f = ((f32::from(event.position.x - b.origin.x) - 44.) / width)
                                    .clamp(0., 1.) as f64
                                    / h.zoom
                                    + h.pan;
                                h.range = Some((f, f));
                                h.dragging = true;
                            }
                            cx.emit(Changed);
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|v, event: &gpui::MouseMoveEvent, _, cx| {
                        let h = &mut v.state;
                        if !h.dragging {
                            return;
                        }
                        let b = h.bounds.get();
                        let width = f32::from(b.size.width) - 44.;
                        if width > 0. {
                            let f = ((f32::from(event.position.x - b.origin.x) - 44.) / width)
                                .clamp(0., 1.) as f64
                                / h.zoom
                                + h.pan;
                            if let Some((a, _)) = h.range {
                                h.range = Some((a, f));
                            }
                        }
                        cx.emit(Changed);
                        cx.notify();
                    }))
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|v, _, _, cx| {
                            v.state.dragging = false;
                            if v.state.range.is_some_and(|(a, b)| (a - b).abs() < 0.004) {
                                v.state.range = None;
                            }
                            cx.emit(Changed);
                            cx.notify();
                        }),
                    )
                    .on_mouse_up_out(
                        gpui::MouseButton::Left,
                        cx.listener(|v, _, _, cx| {
                            v.state.dragging = false;
                            cx.emit(Changed);
                            cx.notify();
                        }),
                    )
                    .child(self.render_history_timeline(window, cx)),
            )
            .automation(AutomationRole::ScrollArea, "History timeline")
    }
}
impl Render for Timeline {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let timeline = self
            .render_history_timeline_panel(window, cx)
            .into_any_element();
        let entry = self
            .state
            .hovered
            .as_ref()
            .and_then(|id| self.state.entries.iter().find(|e| &e.id == id));
        let open = entry.is_some();
        if let Some(entry) = entry {
            let mut anchor = self.state.hover_anchor;
            anchor.origin.y = self.state.bounds.get().top();
            self.retained = Some((entry.clone(), anchor));
        }
        let popup = self.retained.as_ref().map(|(entry, anchor)| {
            (
                *anchor,
                self.render_history_span_detail(entry).into_any_element(),
            )
        });
        let popup = popup.and_then(|(anchor, body)| {
            self.popup.render(
                "history-hover-popup-surface",
                anchor,
                open,
                crate::components::liquid::panel::FloatingStyle::details(
                    284.,
                    crate::components::liquid::panel::Side::Above,
                ),
                crate::components::liquid::panel::Content {
                    sections: vec![body],
                    padding: 12.,
                    gap: 0.,
                },
                None,
                window,
                cx,
            )
        });
        if !open && !self.popup.alive() {
            self.retained = None;
        }
        div().w_full().child(timeline).children(popup)
    }
}
fn history_color(e: &Entry) -> u32 {
    // Timeline marks need stronger separation than the reading-list accents.
    if matches!(e.state.as_str(), "failed" | "timed_out") {
        0xD43D45
    } else {
        [0x2878CE, 0x8056C4, 0x21865B][e.lane.min(2)]
    }
}

const HISTORY_BAR_MIN_WIDTH: f32 = 2.;

struct HistoryBar {
    index: usize,
    id: String,
    left: f32,
    width: f32,
    top: f32,
    height: f32,
    color: u32,
    open: bool,
    selected: bool,
    label: Option<String>,
    layout: std::cell::Cell<Option<(gpui::Pixels, gpui::Pixels)>>,
}
impl HistoryBar {
    fn paint(&self, area: gpui::Bounds<gpui::Pixels>, window: &mut Window) {
        let rect = self.bounds(area);
        let inset = 0.5;
        let fill = gpui::Bounds::new(
            rect.origin + gpui::point(px(0.), px(inset)),
            gpui::size(rect.size.width, rect.size.height - px(inset * 2.)),
        );
        let radius = px(HISTORY_BAR_MIN_WIDTH / 2.);
        if self.selected {
            window.paint_quad(
                gpui::fill(fill.dilate(px(3.)), rgba((self.color << 8) | 27))
                    .corner_radii(radius + px(3.)),
            );
            window.paint_quad(
                gpui::fill(fill.dilate(px(1.)), rgba((self.color << 8) | 191))
                    .corner_radii(radius + px(1.)),
            );
        }
        window.paint_quad(gpui::fill(fill, rgb(self.color)).corner_radii(radius));
    }

    fn bounds(&self, lane: gpui::Bounds<gpui::Pixels>) -> gpui::Bounds<gpui::Pixels> {
        let (left, width) = self.layout.get().unwrap_or((
            lane.size.width * self.left,
            (lane.size.width * self.width).max(px(HISTORY_BAR_MIN_WIDTH)),
        ));
        gpui::Bounds::new(
            lane.origin + gpui::point(left, px(self.top)),
            gpui::size(width, px(self.height)),
        )
    }
}
// Resolve spacing in pixels after idle compression and zoom. Use the same
// geometry for painting, pointer picking and accessibility hitboxes.
// Hover placement follows rendered pixels. Subpixel drift of a running time
// axis must not turn a post-layout observation into a redraw feedback loop.
fn painted_history_anchor(
    bounds: gpui::Bounds<gpui::Pixels>,
    scale: f32,
) -> gpui::Bounds<gpui::Pixels> {
    let snap = |value: gpui::Pixels| px((value.as_f32() * scale).round() / scale);
    gpui::Bounds::from_corners(bounds.origin.map(snap), bounds.bottom_right().map(snap))
}

fn layout_history_bars(bars: &[HistoryBar], area: gpui::Bounds<gpui::Pixels>, separate: bool) {
    for bar in bars {
        bar.layout.set(None);
    }
    if !separate {
        return;
    }
    let mut order = bars.iter().collect::<Vec<_>>();
    order.sort_by(|a, b| a.top.total_cmp(&b.top).then(a.left.total_cmp(&b.left)));
    let mut previous: Option<(f32, gpui::Pixels)> = None;
    for bar in order {
        let mut left = area.size.width * bar.left + px(0.5);
        if let Some((track, right)) = previous {
            if track == bar.top {
                left = left.max(right + px(1.));
            }
        }
        let right = (area.size.width * (bar.left + bar.width) - px(0.5))
            .max(left + px(HISTORY_BAR_MIN_WIDTH));
        bar.layout.set(Some((left, right - left)));
        previous = Some((bar.top, right));
    }
}

fn history_bar_at(
    bars: &[HistoryBar],
    lane: gpui::Bounds<gpui::Pixels>,
    point: gpui::Point<gpui::Pixels>,
) -> Option<&HistoryBar> {
    if !lane.contains(&point) {
        return None;
    }
    // Later bars paint above earlier bars, including overlapping point markers.
    bars.iter()
        .rev()
        .find(|bar| bar.bounds(lane).contains(&point))
}

#[cfg(test)]
mod tests {
    use super::{history_bar_at, layout_history_bars, HistoryBar};
    use gpui::px;
    #[test]
    fn point_markers_pick_the_topmost_visible_bar_after_scaling() {
        let bar = |index, left| HistoryBar {
            index,
            id: index.to_string(),
            left,
            width: 0.,
            top: 4.,
            height: 16.,
            color: 0,
            open: false,
            selected: false,
            label: None,
            layout: Default::default(),
        };
        let bars = [bar(0, 0.5), bar(1, 0.505), bar(2, 1.)];
        let area = gpui::Bounds::new(
            gpui::point(px(100.), px(50.)),
            gpui::size(px(200.), px(32.)),
        );
        assert_eq!(
            history_bar_at(&bars, area, gpui::point(px(202.), px(60.)))
                .unwrap()
                .index,
            1
        );
        assert_eq!(
            history_bar_at(&bars, area, gpui::point(px(200.5), px(60.)))
                .unwrap()
                .index,
            0
        );
        assert!(history_bar_at(&bars, area, gpui::point(px(302.), px(60.))).is_none());
        assert!(history_bar_at(&bars, area, gpui::point(px(202.), px(52.))).is_none());

        // Two sub-pixel model requests keep their minimum width and gain a
        // real empty pixel, including in pointer picking after zoom/resize.
        layout_history_bars(&bars, area, true);
        let first = bars[0].bounds(area);
        let second = bars[1].bounds(area);
        assert_eq!(first.size.width, px(2.));
        assert_eq!(second.origin.x - first.right(), px(1.));
        assert!(
            history_bar_at(&bars, area, gpui::point(first.right() + px(0.5), px(60.))).is_none()
        );
        assert_eq!(
            history_bar_at(&bars, area, second.center()).unwrap().index,
            1
        );
    }
}

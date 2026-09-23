#[cfg(feature = "native-screenshots")]
use std::io::Cursor;

use gpui::{
    point, px, AnyWindowHandle, App, KeyUpEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, ScrollDelta, ScrollWheelEvent, TouchPhase, Window,
};
#[cfg(feature = "native-screenshots")]
use image::{DynamicImage, ImageFormat};
use tokio::sync::{mpsc, oneshot};
use unicode_segmentation::UnicodeSegmentation;

use super::element::AutomationRegistry;
use super::protocol::{
    ActionResult, ActionTarget, ModifierState, MouseButtonName, Point, UserAction,
};

const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_KEYSTROKE_BYTES: usize = 128;
const MAX_DRAG_STEPS: usize = 120;

pub struct DriverEnvelope {
    pub command: DriverCommand,
    pub response: oneshot::Sender<Result<DriverOutput, DriverError>>,
}

pub enum DriverCommand {
    Action(UserAction),
    Screenshot,
    /// Explicit frame delivery for offscreen automation without a platform loop.
    TestFrame,
}

pub enum DriverOutput {
    Action(ActionResult),
    Screenshot(Screenshot),
}

pub struct Screenshot {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub revision: u64,
}

#[derive(Clone, Debug)]
pub struct DriverError {
    pub code: &'static str,
    pub message: String,
}

impl DriverError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub fn attach_driver(
    window_handle: AnyWindowHandle,
    registry: AutomationRegistry,
    mut receiver: mpsc::UnboundedReceiver<DriverEnvelope>,
    cx: &App,
) {
    cx.spawn(async move |cx| {
        while let Some(envelope) = receiver.recv().await {
            let result = window_handle
                .update(cx, |_, window, cx| {
                    execute(envelope.command, window, cx, &registry)
                })
                .map_err(|error| DriverError::new("window_unavailable", error.to_string()))
                .and_then(std::convert::identity);
            let _ = envelope.response.send(result);
        }
    })
    .detach();
}

fn execute(
    command: DriverCommand,
    window: &mut Window,
    cx: &mut App,
    registry: &AutomationRegistry,
) -> Result<DriverOutput, DriverError> {
    match command {
        DriverCommand::Action(action) => {
            execute_action(action, window, cx, registry).map(DriverOutput::Action)
        }
        DriverCommand::Screenshot => capture_screenshot(window, registry),
        DriverCommand::TestFrame => deliver_test_frame(window, cx, registry),
    }
}

#[cfg(feature = "native-screenshots")]
fn deliver_test_frame(
    window: &mut Window,
    cx: &mut App,
    registry: &AutomationRegistry,
) -> Result<DriverOutput, DriverError> {
    let revision_before = registry.revision();
    window.simulate_next_frame(cx);
    Ok(DriverOutput::Action(ActionResult {
        accepted: true,
        action: "test_frame",
        dispatched_events: 0,
        position: None,
        revision_before,
    }))
}

#[cfg(not(feature = "native-screenshots"))]
fn deliver_test_frame(
    _: &mut Window,
    _: &mut App,
    _: &AutomationRegistry,
) -> Result<DriverOutput, DriverError> {
    Err(DriverError::new(
        "test_frame_unavailable",
        "Enable native-screenshots in the host application",
    ))
}

#[cfg(feature = "native-screenshots")]
fn capture_screenshot(
    window: &Window,
    registry: &AutomationRegistry,
) -> Result<DriverOutput, DriverError> {
    let image = window.render_to_image().map_err(|error| {
        DriverError::new(
            "screenshot_failed",
            format!("GPUI could not render the current window: {error}"),
        )
    })?;
    let width = image.width();
    let height = image.height();
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, ImageFormat::Png)
        .map_err(|error| {
            DriverError::new(
                "screenshot_encode_failed",
                format!("could not encode PNG: {error}"),
            )
        })?;
    Ok(DriverOutput::Screenshot(Screenshot {
        bytes: cursor.into_inner(),
        width,
        height,
        revision: registry.revision(),
    }))
}

#[cfg(not(feature = "native-screenshots"))]
fn capture_screenshot(_: &Window, _: &AutomationRegistry) -> Result<DriverOutput, DriverError> {
    Err(DriverError::new(
        "capture_unavailable",
        "Enable native-screenshots in the host application",
    ))
}

#[cfg(feature = "headless-bench")]
pub fn headless_action(
    action: UserAction,
    window: &mut Window,
    cx: &mut App,
    registry: &AutomationRegistry,
) -> Result<ActionResult, DriverError> {
    execute_action(action, window, cx, registry)
}

fn execute_action(
    action: UserAction,
    window: &mut Window,
    cx: &mut App,
    registry: &AutomationRegistry,
) -> Result<ActionResult, DriverError> {
    let revision_before = registry.revision();
    match action {
        #[cfg(feature = "frame-profiler")]
        UserAction::ScrollMeasure {
            target,
            duration_ms,
            pixels_per_second,
            start_down,
        } => {
            if !pixels_per_second.is_finite() || !(1.0..=10000.0).contains(&pixels_per_second) {
                return Err(DriverError::new(
                    "invalid_speed",
                    "scroll speed must be 1..=10000 px/s",
                ));
            }
            if !(1000..=30000).contains(&duration_ms) {
                return Err(DriverError::new(
                    "invalid_duration",
                    "duration must be 1000..=30000 ms",
                ));
            }
            let position = resolve_target(&target, "scroll", window, registry)?;
            let report = std::env::var_os("ZORK_FRAME_REPORT")
                .ok_or_else(|| DriverError::new("missing_report", "set ZORK_FRAME_REPORT"))?;
            // GPUI only records presentation intervals for an active window.
            // Synthetic wheel input alone does not foreground the test window.
            cx.activate(true);
            window.activate_window();
            // A wheel target must also be the pointer target. Otherwise native
            // runs inherit arbitrary hover animations from the desktop cursor.
            dispatch_move(position, None, gpui::Modifiers::default(), window, cx);
            let state = FrameMeasurement {
                started: std::time::Instant::now(),
                pixels_per_second,
                start_down,
                duration: std::time::Duration::from_millis(duration_ms),
                before: window.frame_duration_snapshot(),
                position,
                report: report.into(),
                input_events: std::rc::Rc::new(std::cell::Cell::new(0)),
                active_input_events: std::rc::Rc::new(std::cell::Cell::new(0)),
                window_active_at_start: window.is_window_active(),
                finished: std::rc::Rc::new(std::cell::Cell::new(false)),
            };
            start_scroll_input(state.clone(), window.window_handle(), cx);
            let deadline_state = state.clone();
            let handle = window.window_handle();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(deadline_state.duration)
                    .await;
                let _ = handle.update(cx, |_, window, _| {
                    finish_frame_measurement(&deadline_state, window)
                });
            })
            .detach();
            measure_next_frame(state, window);
            Ok(ActionResult {
                accepted: true,
                action: "scroll_measure",
                dispatched_events: 0,
                position: Some(position),
                revision_before,
            })
        }

        UserAction::Click {
            target,
            button,
            click_count,
            modifiers,
        } => {
            if !(1..=3).contains(&click_count) {
                return Err(DriverError::new(
                    "invalid_click_count",
                    "click_count must be between 1 and 3",
                ));
            }
            let position = resolve_target(&target, "click", window, registry)?;
            dispatch_click(
                position,
                button.into(),
                click_count,
                modifiers.into(),
                window,
                cx,
            );
            Ok(ActionResult {
                accepted: true,
                action: "click",
                dispatched_events: 1 + click_count * 2,
                position: Some(position),
                revision_before,
            })
        }
        UserAction::Move { target, modifiers } => {
            let position = resolve_target(&target, "move", window, registry)?;
            dispatch_move(position, None, modifiers.into(), window, cx);
            Ok(ActionResult {
                accepted: true,
                action: "move",
                dispatched_events: 1,
                position: Some(position),
                revision_before,
            })
        }
        UserAction::TypeText { text, target } => {
            if text.len() > MAX_TEXT_BYTES {
                return Err(DriverError::new(
                    "text_too_large",
                    format!("text must not exceed {MAX_TEXT_BYTES} UTF-8 bytes"),
                ));
            }
            let mut dispatched_events = 0;
            let mut position = None;
            if let Some(target) = target.as_ref() {
                let target_position = resolve_target(target, "type_text", window, registry)?;
                dispatch_click(
                    target_position,
                    MouseButton::Left,
                    1,
                    Modifiers::default(),
                    window,
                    cx,
                );
                position = Some(target_position);
                dispatched_events += 3;
            }
            dispatched_events += dispatch_text(&text, window, cx)?;
            Ok(ActionResult {
                accepted: true,
                action: "type_text",
                dispatched_events,
                position,
                revision_before,
            })
        }
        UserAction::Key { keystroke } => {
            if keystroke.is_empty() || keystroke.len() > MAX_KEYSTROKE_BYTES {
                return Err(DriverError::new(
                    "invalid_keystroke",
                    format!("keystroke must contain 1 to {MAX_KEYSTROKE_BYTES} UTF-8 bytes"),
                ));
            }
            let keystroke = Keystroke::parse(&keystroke)
                .map_err(|error| DriverError::new("invalid_keystroke", error.to_string()))?;
            dispatch_keystroke(keystroke, window, cx);
            Ok(ActionResult {
                accepted: true,
                action: "key",
                dispatched_events: 2,
                position: None,
                revision_before,
            })
        }
        UserAction::Scroll {
            target,
            delta_x,
            delta_y,
            modifiers,
        } => {
            if !delta_x.is_finite() || !delta_y.is_finite() {
                return Err(DriverError::new(
                    "invalid_scroll_delta",
                    "scroll deltas must be finite numbers",
                ));
            }
            let position = resolve_target(&target, "scroll", window, registry)?;
            let modifiers = modifiers.into();
            dispatch_move(position, None, modifiers, window, cx);
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: gpui_point(position),
                    delta: ScrollDelta::Pixels(point(px(delta_x), px(delta_y))),
                    modifiers,
                    touch_phase: TouchPhase::Moved,
                }),
                cx,
            );
            Ok(ActionResult {
                accepted: true,
                action: "scroll",
                dispatched_events: 2,
                position: Some(position),
                revision_before,
            })
        }
        UserAction::Drag {
            from,
            to,
            steps,
            modifiers,
        } => {
            if !(1..=MAX_DRAG_STEPS).contains(&steps) {
                return Err(DriverError::new(
                    "invalid_drag_steps",
                    format!("steps must be between 1 and {MAX_DRAG_STEPS}"),
                ));
            }
            let from = resolve_target(&from, "move", window, registry)?;
            let to = resolve_target(&to, "move", window, registry)?;
            let modifiers = modifiers.into();
            dispatch_drag(from, to, steps, modifiers, window, cx);
            Ok(ActionResult {
                accepted: true,
                action: "drag",
                dispatched_events: steps + 3,
                position: Some(to),
                revision_before,
            })
        }
    }
}

fn resolve_target(
    target: &ActionTarget,
    action: &str,
    window: &Window,
    registry: &AutomationRegistry,
) -> Result<Point, DriverError> {
    let point = match target {
        ActionTarget::Element(target) => {
            let element = registry.element(&target.element_id).ok_or_else(|| {
                DriverError::new(
                    "element_not_found",
                    format!("no rendered element has id {:?}", target.element_id),
                )
            })?;
            if !element.visible {
                return Err(DriverError::new(
                    "element_not_visible",
                    format!("element {:?} is clipped or off-screen", target.element_id),
                ));
            }
            if !element.role.supports(action) {
                return Err(DriverError::new(
                    "unsupported_element_action",
                    format!(
                        "element {:?} does not expose the {action:?} user action",
                        target.element_id
                    ),
                ));
            }
            element.center
        }
        ActionTarget::Point(target) => (*target).into(),
    };

    validate_point(point, window)?;
    Ok(point)
}

fn validate_point(position: Point, window: &Window) -> Result<(), DriverError> {
    if !position.is_finite() {
        return Err(DriverError::new(
            "invalid_coordinate",
            "coordinates must be finite numbers",
        ));
    }
    let viewport = window.viewport_size();
    if position.x < 0.0
        || position.y < 0.0
        || position.x > viewport.width.as_f32()
        || position.y > viewport.height.as_f32()
    {
        return Err(DriverError::new(
            "coordinate_out_of_bounds",
            format!(
                "point ({}, {}) is outside the {} x {} window content area",
                position.x,
                position.y,
                viewport.width.as_f32(),
                viewport.height.as_f32()
            ),
        ));
    }
    Ok(())
}

fn dispatch_click(
    position: Point,
    button: MouseButton,
    click_count: usize,
    modifiers: Modifiers,
    window: &mut Window,
    cx: &mut App,
) {
    dispatch_move(position, None, modifiers, window, cx);
    for current_count in 1..=click_count {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position: gpui_point(position),
                modifiers,
                click_count: current_count,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position: gpui_point(position),
                modifiers,
                click_count: current_count,
            }),
            cx,
        );
    }
}

fn dispatch_move(
    position: Point,
    pressed_button: Option<MouseButton>,
    modifiers: Modifiers,
    window: &mut Window,
    cx: &mut App,
) {
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: gpui_point(position),
            pressed_button,
            modifiers,
        }),
        cx,
    );
}

fn dispatch_drag(
    from: Point,
    to: Point,
    steps: usize,
    modifiers: Modifiers,
    window: &mut Window,
    cx: &mut App,
) {
    dispatch_move(from, None, modifiers, window, cx);
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: gpui_point(from),
            modifiers,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    for step in 1..=steps {
        let progress = step as f32 / steps as f32;
        let position = Point {
            x: from.x + (to.x - from.x) * progress,
            y: from.y + (to.y - from.y) * progress,
        };
        dispatch_move(position, Some(MouseButton::Left), modifiers, window, cx);
    }
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: gpui_point(to),
            modifiers,
            click_count: 1,
        }),
        cx,
    );
}

fn dispatch_text(text: &str, window: &mut Window, cx: &mut App) -> Result<usize, DriverError> {
    let mut dispatched = 0;
    for grapheme in text.graphemes(true) {
        let keystroke = match grapheme {
            "\n" | "\r\n" => Keystroke::parse("shift-enter")
                .map_err(|error| DriverError::new("invalid_keystroke", error.to_string()))?,
            "\t" => Keystroke::parse("tab")
                .map_err(|error| DriverError::new("invalid_keystroke", error.to_string()))?,
            _ => Keystroke {
                modifiers: Modifiers::default(),
                key: grapheme.to_owned(),
                key_char: Some(grapheme.to_owned()),
            },
        };
        dispatch_keystroke(keystroke, window, cx);
        dispatched += 2;
    }
    Ok(dispatched)
}

fn dispatch_keystroke(keystroke: Keystroke, window: &mut Window, cx: &mut App) {
    window.dispatch_keystroke(keystroke.clone(), cx);
    window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
}

fn gpui_point(position: Point) -> gpui::Point<gpui::Pixels> {
    point(px(position.x), px(position.y))
}

impl From<ModifierState> for Modifiers {
    fn from(value: ModifierState) -> Self {
        Self {
            control: value.control,
            alt: value.alt,
            shift: value.shift,
            platform: value.platform,
            function: value.function,
        }
    }
}

impl From<MouseButtonName> for MouseButton {
    fn from(value: MouseButtonName) -> Self {
        match value {
            MouseButtonName::Left => Self::Left,
            MouseButtonName::Right => Self::Right,
            MouseButtonName::Middle => Self::Middle,
        }
    }
}

/// Dev-only continuous wheel workload. The GPUI profiler counts actual presents;
/// HTTP request timing and callback counts are not used as FPS samples.
#[cfg(feature = "frame-profiler")]
#[derive(Clone)]
struct FrameMeasurement {
    pixels_per_second: f64,
    start_down: bool,
    started: std::time::Instant,
    duration: std::time::Duration,
    before: gpui::profiler::FrameDurationSnapshot,
    position: Point,
    report: std::path::PathBuf,
    input_events: std::rc::Rc<std::cell::Cell<usize>>,
    active_input_events: std::rc::Rc<std::cell::Cell<usize>>,
    window_active_at_start: bool,
    finished: std::rc::Rc<std::cell::Cell<bool>>,
}

#[cfg(feature = "frame-profiler")]
fn measure_next_frame(state: FrameMeasurement, window: &mut Window) {
    window.on_next_frame(move |window, _cx| {
        if state.finished.get() {
            return;
        }
        if state.started.elapsed() >= state.duration {
            finish_frame_measurement(&state, window);
            return;
        }
        measure_next_frame(state, window);
    });
}

/// Finish on a deadline even if scrolling ends exactly at a list boundary and
/// the platform stops producing frames. Coverage exposes missing frame demand.
#[cfg(feature = "frame-profiler")]
fn finish_frame_measurement(state: &FrameMeasurement, window: &Window) {
    if state.finished.replace(true) {
        return;
    }
    let mut after = window.frame_duration_snapshot();
    after
        .draw_duration_histogram
        .subtract(&state.before.draw_duration_histogram)
        .unwrap();
    after
        .present_interval_histogram
        .subtract(&state.before.present_interval_histogram)
        .unwrap();
    let intervals = &after.present_interval_histogram;
    let draws = &after.draw_duration_histogram;
    let value = serde_json::json!({
        "method": "GPUI native present interval histogram during continuous wheel input",
        "elapsed_seconds": state.started.elapsed().as_secs_f64(),
        "present_intervals": intervals.len(),
        "present_interval_coverage": intervals.mean() * intervals.len() as f64 / 1e9 / state.started.elapsed().as_secs_f64(),
        "fps": if intervals.is_empty() { 0.0 } else { 1e9 / intervals.mean() },
        "p50_frame_ms": intervals.value_at_quantile(0.5) as f64 / 1e6,
        "p95_frame_ms": intervals.value_at_quantile(0.95) as f64 / 1e6,
        "max_frame_ms": intervals.max() as f64 / 1e6,
        "draws": draws.len(),
        "input_events": state.input_events.get(),
        "active_input_events": state.active_input_events.get(),
        "window_active_at_start": state.window_active_at_start,
        "window_active_at_end": window.is_window_active(),
        "p95_draw_ms": draws.value_at_quantile(0.95) as f64 / 1e6,
        "p99_draw_ms": draws.value_at_quantile(0.99) as f64 / 1e6,
        "max_draw_ms": draws.max() as f64 / 1e6,
        "draws_within_120hz_cpu_budget_percent": if draws.is_empty() { 0.0 }
            else { draws.count_between(0, 8_333_333) as f64 * 100.0 / draws.len() as f64 },
    });
    std::fs::write(&state.report, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

/// Inject wheel events independently of display callbacks, as a real trackpad
/// does. Frame-driven input can stop itself when one frame is coalesced.
#[cfg(feature = "frame-profiler")]
fn start_scroll_input(state: FrameMeasurement, handle: AnyWindowHandle, cx: &App) {
    cx.spawn(async move |cx| {
        let mut previous = 0.;
        while !state.finished.get() {
            let seconds = state.started.elapsed().as_secs_f64();
            if seconds >= state.duration.as_secs_f64() {
                break;
            }
            let direction = if (seconds as u64) % 2 == 0 { 1.0 } else { -1.0 }
                * if state.start_down { -1.0 } else { 1.0 };
            let delta = (state.pixels_per_second * (seconds - previous)) as f32 * direction;
            previous = seconds;
            if handle
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        PlatformInput::ScrollWheel(ScrollWheelEvent {
                            position: gpui_point(state.position),
                            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                            modifiers: Modifiers::default(),
                            touch_phase: TouchPhase::Moved,
                        }),
                        cx,
                    );
                    state.input_events.set(state.input_events.get() + 1);
                    if window.is_window_active() {
                        state
                            .active_input_events
                            .set(state.active_input_events.get() + 1);
                    }
                })
                .is_err()
            {
                break;
            }
            cx.background_executor()
                .timer(std::time::Duration::from_micros(8333))
                .await;
        }
    })
    .detach();
}

//! Shared motion tokens. Durations, curves and springs live here so pages never
//! carry raw numbers; see `apps/zork-design-pc/docs/07-motion.md`.
//!
//! Reduced motion is `App::reduce_motion`, seeded from the macOS accessibility
//! preference at startup. GPUI's `with_animation` and `with_spring` already
//! render the end state without scheduling frames when it is set.
use gpui::{
    div, prelude::*, px, AnyElement, Animation, AnimationPhase, App, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, SharedString, SpringAnimation, SpringConfig, Window,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use std::{cell::Cell, rc::Rc};

/// With reduced motion, fades are capped at this length and nothing moves.
pub const REDUCED_FADE: u64 = 150;

static USER_REDUCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Records the system "reduce motion" preference. `App::reduce_motion` is also
/// set by tests and still exports that want end states; only the user's
/// preference turns motion into short fades.
pub fn set_user_reduced(reduced: bool) {
    USER_REDUCED.store(reduced, Ordering::Relaxed);
}

/// How transitions play right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Full motion.
    Full,
    /// The user asked for reduced motion: fades up to 150 ms, nothing moves.
    Short,
    /// Static rendering (tests, exported stills): jump to the end state.
    Static,
}
pub fn mode(cx: &App) -> Mode {
    if !cx.reduce_motion() {
        Mode::Full
    } else if USER_REDUCED.load(Ordering::Relaxed) {
        Mode::Short
    } else {
        Mode::Static
    }
}

/// Hover, press, switch and selection feedback.
pub const FAST: u64 = 120;
/// Popovers, menus, expanders, segmented selection, list insert and removal.
pub const BASE: u64 = 180;
/// Dialogs, sheets and status notices.
pub const SURFACE: u64 = 240;
/// Phone page transitions.
pub const PAGE: u64 = 280;
/// Desktop content switch: the content area fades, chrome stays put.
pub const DESKTOP_PAGE: u64 = 150;
/// Popovers enter slightly faster than the base token.
pub const POPOVER: u64 = BASE - 20;
/// Dialog backdrop fade.
pub const SCRIM: u64 = 200;
/// Switch thumb travel.
pub const SWITCH: u64 = 150;
/// Numeric tweens such as quota bars.
pub const VALUE: u64 = 300;
/// The working pulse period.
pub const PULSE: u64 = 1600;
/// Loading placeholders appear only after this delay.
pub const LOADING_DELAY: u64 = 300;
/// Once shown, a loading placeholder stays at least this long.
pub const LOADING_MIN: u64 = 400;

/// Popovers travel this far from their anchor side while entering.
pub const POPOVER_OFFSET: f32 = 6.;
/// New list rows rise this far into place.
pub const ROW_OFFSET: f32 = 6.;
/// Status notices drop this far from their region's top edge.
pub const NOTICE_OFFSET: f32 = 8.;

/// Exits leave faster than enters.
pub const fn exit(enter: u64) -> u64 {
    enter * 2 / 3
}

static SLOW: AtomicU32 = AtomicU32::new(1000);

/// Stretches every motion token; Zork Design uses 5 to inspect transitions.
pub fn set_slow(factor: f32) {
    SLOW.store((factor.clamp(1., 20.) * 1000.) as u32, Ordering::Relaxed);
}
pub fn slow() -> f32 {
    SLOW.load(Ordering::Relaxed) as f32 / 1000.
}

/// A token as a duration, including the inspection slow-down.
pub fn duration(ms: u64) -> Duration {
    Duration::from_micros((ms as f32 * slow() * 1000.) as u64)
}

/// Enter: fast start, gentle landing.
pub fn enter(ms: u64) -> Animation {
    Animation::new(duration(ms)).with_easing(bezier(0.2, 0.7, 0.2, 1.0))
}
/// Exit: accelerates away. Pass the enter token; the exit is two thirds of it.
pub fn leave(enter_ms: u64) -> Animation {
    Animation::new(duration(exit(enter_ms))).with_easing(bezier(0.4, 0.0, 1.0, 1.0))
}
/// Move: something already on screen changes position.
pub fn moving(ms: u64) -> Animation {
    Animation::new(duration(ms)).with_easing(bezier(0.3, 0.0, 0.2, 1.0))
}
/// Enter that stays invisible for `delay_ms` first (loading placeholders).
pub fn delayed(delay_ms: u64, ms: u64) -> Animation {
    let total = delay_ms + ms;
    let start = delay_ms as f32 / total as f32;
    let curve = bezier(0.2, 0.7, 0.2, 1.0);
    Animation::new(duration(total)).with_easing(move |t| {
        if t <= start {
            0.
        } else {
            curve((t - start) / (1. - start))
        }
    })
}
/// The only looping motion: the working dot breathes between 1 and 0.35.
pub fn pulse() -> Animation {
    Animation::new(duration(PULSE))
        .repeat()
        .with_max_fps(30.)
        .with_easing(|t| 0.675 + 0.325 * (t * std::f32::consts::TAU).cos())
}

/// A critically damped spring that settles in about `ms`, for positions that
/// retarget mid-flight (switch thumbs, segmented capsules). A new target keeps
/// the current position and velocity, so rapid input never jumps.
pub fn spring_config(ms: u64) -> SpringConfig {
    // Critical damping reaches 1% of the distance after about 6.6 / ω.
    let omega = 6.6 / (ms as f32 * slow() / 1000.);
    SpringConfig::new(omega * omega, 2. * omega, 1.)
}
pub fn spring(ms: u64) -> SpringAnimation<()> {
    SpringAnimation::new(spring_config(ms))
}
/// A boolean spring phase, 0 → 1.
pub fn toggle(ms: u64, on: bool) -> SpringAnimation<AnimationPhase> {
    spring(ms).to(AnimationPhase::from(on))
}

/// CSS `cubic-bezier(x1, y1, x2, y2)` as an easing function.
pub fn bezier(x1: f32, y1: f32, x2: f32, y2: f32) -> impl Fn(f32) -> f32 + Clone + 'static {
    move |t: f32| {
        if t <= 0. {
            return 0.;
        }
        if t >= 1. {
            return 1.;
        }
        let curve = |a: f32, b: f32, s: f32| {
            let u = 1. - s;
            3. * u * u * s * a + 3. * u * s * s * b + s * s * s
        };
        let slope = |a: f32, b: f32, s: f32| {
            let u = 1. - s;
            3. * u * u * a + 6. * u * s * (b - a) + 3. * s * s * (1. - b)
        };
        // Newton steps from t, falling back to bisection for flat slopes.
        let mut s = t;
        for _ in 0..8 {
            let dx = slope(x1, x2, s);
            if dx.abs() < 1e-5 {
                break;
            }
            let next = s - (curve(x1, x2, s) - t) / dx;
            if !(0.0..=1.0).contains(&next) {
                break;
            }
            s = next;
        }
        if (curve(x1, x2, s) - t).abs() > 1e-4 {
            let (mut lo, mut hi) = (0f32, 1f32);
            for _ in 0..24 {
                s = (lo + hi) / 2.;
                if curve(x1, x2, s) < t {
                    lo = s
                } else {
                    hi = s
                }
            }
        }
        curve(y1, y2, s)
    }
}

fn ease_enter(t: f32) -> f32 {
    bezier(0.2, 0.7, 0.2, 1.0)(t)
}
fn ease_exit(t: f32) -> f32 {
    bezier(0.4, 0.0, 1.0, 1.0)(t)
}
fn ease_move(t: f32) -> f32 {
    bezier(0.3, 0.0, 0.2, 1.0)(t)
}
fn progress(start: Instant, now: Instant, ms: u64) -> f32 {
    let total = duration(ms).as_secs_f32();
    if total <= 0. {
        return 1.;
    }
    (now.saturating_duration_since(start).as_secs_f32() / total).clamp(0., 1.)
}

/// One frame of a presence transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub opacity: f32,
    /// Remaining share of the enter travel, 1 → 0. Exits fade without moving.
    pub travel: f32,
    /// The content is leaving: render it without input or automation.
    pub closing: bool,
}

/// Keeps content on screen for the exit after `open` turns false.
///
/// Enter fades in on the enter curve; exit fades out on the exit curve in two
/// thirds of the time. Reversing mid-flight continues from the current value.
/// With reduced motion both fades are capped at [`REDUCED_FADE`] and nothing
/// travels.
pub struct Presence {
    enter_ms: u64,
    open: bool,
    from: f32,
    alpha: f32,
    start: Option<Instant>,
}
impl Presence {
    pub fn new(enter_ms: u64) -> Self {
        Self {
            enter_ms,
            open: false,
            from: 0.,
            alpha: 0.,
            start: None,
        }
    }
    /// True while the content should still be rendered.
    pub fn visible(&self) -> bool {
        self.open || self.alpha > 0.
    }
    /// Advances to `now`. Returns the frame to render (None once fully gone)
    /// and whether another animation frame is needed.
    pub fn step(&mut self, open: bool, now: Instant, mode: Mode) -> (Option<Frame>, bool) {
        if mode == Mode::Static {
            self.open = open;
            self.start = None;
            self.alpha = if open { 1. } else { 0. };
            let frame = Frame {
                opacity: 1.,
                travel: 0.,
                closing: false,
            };
            return (open.then_some(frame), false);
        }
        let reduced = mode == Mode::Short;
        if open != self.open {
            self.open = open;
            self.from = self.alpha;
            self.start = Some(now);
        }
        let Some(start) = self.start else {
            return (None, false);
        };
        let target = if open { 1. } else { 0. };
        let ms = if open { self.enter_ms } else { exit(self.enter_ms) };
        let ms = if reduced { ms.min(REDUCED_FADE) } else { ms };
        let t = progress(start, now, ms);
        let eased = if open { ease_enter(t) } else { ease_exit(t) };
        self.alpha = self.from + (target - self.from) * eased;
        if t >= 1. {
            self.alpha = target;
        }
        if !open && self.alpha <= 0. {
            self.start = None;
            return (None, false);
        }
        let travel = if open && !reduced { 1. - eased } else { 0. };
        (
            Some(Frame {
                opacity: self.alpha,
                travel,
                closing: !open,
            }),
            t < 1.,
        )
    }
}

/// Presence kept in window state under `key`; call it on every render, open or
/// not, and render the content only while it returns a frame.
pub fn presence(
    key: impl Into<ElementId>,
    open: bool,
    enter_ms: u64,
    window: &mut Window,
    cx: &mut App,
) -> Option<Frame> {
    let state = window.use_keyed_state(key, cx, |_, _| Presence::new(enter_ms));
    let now = cx.background_executor().now();
    let mode = mode(cx);
    let (frame, moving) = state.update(cx, |p, _| p.step(open, now, mode));
    if moving {
        window.request_animation_frame();
    }
    frame
}

/// Wraps content that appears on mount: fades in on the enter curve while
/// travelling `travel` px into place. Reduced motion keeps a short fade and no
/// travel. Unlike GPUI's `with_animation`, this never jumps to the end state.
pub fn appear<E: Styled + IntoElement + 'static>(
    id: impl Into<ElementId>,
    element: E,
    ms: u64,
    travel: f32,
) -> Appear<E> {
    Appear {
        id: id.into(),
        element: Some(element),
        ms,
        travel,
    }
}
/// `element.appear(id, ms, travel)` — see [`appear`].
pub trait MotionExt: Styled + IntoElement + Sized + 'static {
    fn appear(self, id: impl Into<ElementId>, ms: u64, travel: f32) -> Appear<Self> {
        appear(id, self, ms, travel)
    }
}
impl<E: Styled + IntoElement + 'static> MotionExt for E {}

pub struct Appear<E> {
    id: ElementId,
    element: Option<E>,
    ms: u64,
    travel: f32,
}
impl<E: Styled + IntoElement + 'static> IntoElement for Appear<E> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl<E: Styled + IntoElement + 'static> Element for Appear<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        window.with_element_state(global_id.unwrap(), |start: Option<Instant>, window| {
            let now = cx.background_executor().now();
            let start = start.unwrap_or(now);
            let (ms, travel) = match mode(cx) {
                Mode::Full => (self.ms, self.travel),
                Mode::Short => (self.ms.min(REDUCED_FADE), 0.),
                Mode::Static => (0, 0.),
            };
            let t = progress(start, now, ms);
            let element = self.element.take().expect("request_layout runs once");
            let mut element = if t < 1. {
                let e = ease_enter(t);
                let element = element.opacity(e);
                if travel != 0. {
                    element.relative().top(px(travel * (1. - e))).into_any_element()
                } else {
                    element.into_any_element()
                }
            } else {
                element.into_any_element()
            };
            if t < 1. {
                window.request_animation_frame();
            }
            ((element.request_layout(window, cx), element), start)
        })
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

/// One frame of a disclosure: the clamp height while moving, the content
/// opacity and whether the content is on its way out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fold {
    /// Height the container clamps and clips to; `None` once settled open, so
    /// the content keeps its natural height.
    pub height: Option<f32>,
    pub opacity: f32,
    /// The content is leaving: render it without input or automation.
    pub closing: bool,
}

/// Height transition for disclosure content.
///
/// Every change of `open` continues from what is on screen: the height from
/// the last rendered height and the opacity from the last rendered opacity, so
/// reversing mid-way never jumps. Opening waits for the first measurement of
/// the content before the clock starts, so the first frame never renders the
/// target or a guessed height. Height moves on the move curve; opening fades
/// the content in over the second half, closing fades it out over the first
/// half and then removes it. Reduced motion snaps the height and keeps a short
/// fade.
pub struct CollapseState {
    open: bool,
    /// Waiting for a measurement before the transition clock starts.
    pending: bool,
    start: Option<Instant>,
    from_height: f32,
    from_opacity: f32,
    shown: f32,
    opacity: f32,
}
impl CollapseState {
    /// `open` is the state on the first frame. A state created closed and
    /// then stepped open animates in, which is how newly revealed rows enter.
    pub fn new(open: bool) -> Self {
        Self {
            open,
            pending: false,
            start: None,
            from_height: 0.,
            from_opacity: 0.,
            shown: 0.,
            opacity: if open { 1. } else { 0. },
        }
    }
    /// Advances to `now` given the content's last measured natural height
    /// (`None` before it was ever laid out). Returns the frame to render (None
    /// once closed) and whether another animation frame is needed.
    pub fn step(
        &mut self,
        open: bool,
        natural: Option<f32>,
        now: Instant,
        mode: Mode,
    ) -> (Option<Fold>, bool) {
        let settled_open = Fold {
            height: None,
            opacity: 1.,
            closing: false,
        };
        if mode == Mode::Static {
            self.open = open;
            self.pending = false;
            self.start = None;
            self.opacity = if open { 1. } else { 0. };
            self.shown = if open { natural.unwrap_or(0.) } else { 0. };
            return (open.then_some(settled_open), false);
        }
        if open != self.open {
            self.open = open;
            self.from_height = self.shown;
            self.from_opacity = self.opacity;
            self.start = None;
            self.pending = true;
        }
        if self.pending {
            if open && natural.is_none() {
                // Lay the content out once, clipped at the current height and
                // invisible, before the clock starts.
                return (
                    Some(Fold {
                        height: Some(self.shown),
                        opacity: self.opacity,
                        closing: false,
                    }),
                    true,
                );
            }
            self.pending = false;
            self.start = Some(now);
        }
        let Some(start) = self.start else {
            if open {
                self.shown = natural.unwrap_or(self.shown);
                self.opacity = 1.;
                return (Some(settled_open), false);
            }
            self.shown = 0.;
            self.opacity = 0.;
            return (None, false);
        };
        let reduced = mode == Mode::Short;
        let th = if reduced { 1. } else { progress(start, now, BASE) };
        let tf = progress(start, now, if reduced { REDUCED_FADE } else { BASE });
        let target = if open { natural.unwrap_or(0.) } else { 0. };
        let height = self.from_height + (target - self.from_height) * ease_move(th);
        let opacity = if open {
            let phase = if reduced { tf } else { ((tf - 0.5) * 2.).clamp(0., 1.) };
            self.from_opacity + (1. - self.from_opacity) * ease_enter(phase)
        } else {
            let phase = if reduced { tf } else { (tf * 2.).clamp(0., 1.) };
            self.from_opacity * (1. - phase)
        };
        if th >= 1. && tf >= 1. {
            self.start = None;
            self.shown = target;
            self.opacity = if open { 1. } else { 0. };
            return (open.then_some(settled_open), false);
        }
        self.shown = height;
        self.opacity = opacity;
        (
            Some(Fold {
                height: Some(height),
                opacity,
                closing: !open,
            }),
            true,
        )
    }
}

struct Collapse {
    state: CollapseState,
    measured: Rc<Cell<Option<f32>>>,
}

/// A disclosure frame to wrap content in; see [`fold`].
pub struct FoldFrame {
    pub fold: Fold,
    measured: Rc<Cell<Option<f32>>>,
}
impl FoldFrame {
    /// Clamps, clips and fades `content`, measuring its natural height for the
    /// next frame.
    pub fn wrap(self, content: impl IntoElement) -> AnyElement {
        let measured = self.measured;
        let body = div()
            .w_full()
            .flex_shrink_0()
            .relative()
            .opacity(self.fold.opacity)
            .child(content)
            .child(
                gpui::canvas(
                    move |bounds, _, _| measured.set(Some(bounds.size.height.as_f32())),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        match self.fold.height {
            Some(h) => div()
                .w_full()
                .h(px(h))
                .overflow_hidden()
                .child(body)
                .into_any_element(),
            None => body.into_any_element(),
        }
    }
}

/// Disclosure state kept in window state under `id`. Call it on every render,
/// open or not, and render the content (through [`FoldFrame::wrap`]) only
/// while it returns a frame. `initially_open` is the state the first frame
/// assumes: pass `false` for content that should grow in when it first
/// appears open (rows revealed by expanding a group).
pub fn fold(
    id: impl Into<SharedString>,
    open: bool,
    initially_open: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<FoldFrame> {
    let id: SharedString = id.into();
    let key = ElementId::Name(format!("{id}-collapse").into());
    let state = window.use_keyed_state(key, cx, |_, _| Collapse {
        state: CollapseState::new(initially_open),
        measured: Rc::new(Cell::new(None)),
    });
    let now = cx.background_executor().now();
    let mode = mode(cx);
    let (fold, moving, measured) = state.update(cx, |s, _| {
        let natural = s.measured.get();
        let (fold, moving) = s.state.step(open, natural, now, mode);
        (fold, moving, s.measured.clone())
    });
    if moving {
        window.request_animation_frame();
    }
    fold.map(|fold| FoldFrame { fold, measured })
}

/// Height transition for disclosure content that is open or closed from its
/// first frame; see [`CollapseState`] and [`fold`].
pub fn collapse(
    id: impl Into<SharedString>,
    open: bool,
    content: impl FnOnce() -> AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    fold(id, open, open, window, cx).map(|frame| frame.wrap(content()))
}

/// Tests that read motion durations hold this while another stretches them.
#[cfg(test)]
static CLOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
fn clock() -> std::sync::MutexGuard<'static, ()> {
    CLOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_hits_endpoints_and_is_monotonic() {
        for curve in [
            bezier(0.2, 0.7, 0.2, 1.0),
            bezier(0.4, 0.0, 1.0, 1.0),
            bezier(0.3, 0.0, 0.2, 1.0),
        ] {
            assert_eq!(curve(0.), 0.);
            assert_eq!(curve(1.), 1.);
            let mut last = 0.;
            for i in 1..=100 {
                let v = curve(i as f32 / 100.);
                assert!(v + 1e-4 >= last, "not monotonic at {i}");
                last = v;
            }
        }
    }

    #[test]
    fn enter_front_loads_and_exit_back_loads() {
        let enter = bezier(0.2, 0.7, 0.2, 1.0);
        let exit = bezier(0.4, 0.0, 1.0, 1.0);
        assert!(enter(0.3) > 0.6);
        assert!(exit(0.3) < 0.2);
        // Linear control points reproduce linear time.
        let linear = bezier(0.25, 0.25, 0.75, 0.75);
        assert!((linear(0.4) - 0.4).abs() < 1e-3);
    }

    #[test]
    fn exits_are_two_thirds_and_slow_stretches_tokens() {
        let _clock = super::clock();
        assert_eq!(exit(SURFACE), 160);
        assert_eq!(exit(BASE), 120);
        set_slow(5.);
        assert_eq!(duration(BASE), Duration::from_millis(900));
        set_slow(1.);
        assert_eq!(duration(BASE), Duration::from_millis(180));
    }

    #[test]
    fn presence_fades_out_and_drops() {
        let _clock = super::clock();
        let t0 = Instant::now();
        let mut p = Presence::new(POPOVER);
        let (f, moving) = p.step(true, t0, Mode::Full);
        let f = f.unwrap();
        assert!(moving && f.opacity < 0.05 && f.travel > 0.9);
        let open = t0 + Duration::from_millis(400);
        assert_eq!(p.step(true, open, Mode::Full).0.unwrap().opacity, 1.);
        let (f, moving) = p.step(false, open, Mode::Full);
        let f = f.unwrap();
        assert!(moving && f.closing && f.travel == 0.);
        let mid = p.step(false, open + Duration::from_millis(60), Mode::Full).0.unwrap();
        assert!(mid.opacity > 0. && mid.opacity < 1.);
        let (f, moving) = p.step(false, open + Duration::from_millis(200), Mode::Full);
        assert!(f.is_none() && !moving);
    }

    #[test]
    fn reduced_presence_is_a_short_fade_without_travel() {
        let _clock = super::clock();
        let t0 = Instant::now();
        let mut p = Presence::new(SURFACE);
        let (f, _) = p.step(true, t0, Mode::Short);
        assert_eq!(f.unwrap().travel, 0.);
        let (f, moving) = p.step(true, t0 + Duration::from_millis(REDUCED_FADE + 1), Mode::Short);
        assert!(!moving && f.unwrap().opacity == 1.);
    }

    #[test]
    fn static_presence_jumps() {
        let t0 = Instant::now();
        let mut p = Presence::new(SURFACE);
        assert_eq!(p.step(true, t0, Mode::Static).0.unwrap().opacity, 1.);
        assert!(p.step(false, t0, Mode::Static).0.is_none());
    }

    fn run(state: &mut CollapseState, open: bool, natural: f32, t0: Instant, from_ms: u64, to_ms: u64) -> Vec<Option<Fold>> {
        (from_ms..=to_ms)
            .step_by(8)
            .map(|ms| state.step(open, Some(natural), t0 + Duration::from_millis(ms), Mode::Full).0)
            .collect()
    }

    #[test]
    fn collapse_opens_from_zero_after_measuring_and_never_jumps() {
        let _clock = super::clock();
        let t0 = Instant::now();
        let mut s = CollapseState::new(false);
        assert_eq!(s.step(false, None, t0, Mode::Full).0, None);
        // First open frame: the content is unmeasured, so it lays out clipped at
        // the current height and invisible; the clock has not started.
        let (f, moving) = s.step(true, None, t0, Mode::Full);
        let f = f.unwrap();
        assert!(moving && f.height == Some(0.) && f.opacity == 0.);
        let frames = run(&mut s, true, 120., t0 + Duration::from_millis(16), 0, 400);
        let mut last = 0.;
        for f in &frames {
            let h = f.unwrap().height.unwrap_or(120.);
            assert!(h + 1e-3 >= last, "height went back: {last} -> {h}");
            assert!(h - last <= 0.2 * 120., "height jumped: {last} -> {h}");
            last = h;
        }
        assert_eq!(frames.last().unwrap().unwrap().height, None);
    }

    #[test]
    fn collapse_reverses_from_the_current_height_and_opacity() {
        let _clock = super::clock();
        let t0 = Instant::now();
        let mut s = CollapseState::new(true);
        s.step(true, Some(100.), t0, Mode::Full);
        // Close, then reopen mid-way: both height and opacity continue.
        let closing = run(&mut s, false, 100., t0, 0, 48);
        let before = closing.last().unwrap().unwrap();
        assert!(before.closing && before.height.unwrap() < 100. && before.opacity < 1.);
        let t = t0 + Duration::from_millis(48);
        let after = s.step(true, Some(100.), t, Mode::Full).0.unwrap();
        assert!((after.height.unwrap() - before.height.unwrap()).abs() < 1e-3);
        assert!((after.opacity - before.opacity).abs() < 1e-3);
        // Close again mid-opening: still continuous.
        let mid = s.step(true, Some(100.), t + Duration::from_millis(40), Mode::Full).0.unwrap();
        let back = s.step(false, Some(100.), t + Duration::from_millis(40), Mode::Full).0.unwrap();
        assert!((mid.height.unwrap() - back.height.unwrap()).abs() < 1e-3);
        assert!((mid.opacity - back.opacity).abs() < 1e-3);
        let (gone, moving) = s.step(false, Some(100.), t + Duration::from_millis(600), Mode::Full);
        assert!(gone.is_none() && !moving);
    }

    #[test]
    fn collapse_created_open_is_settled_and_static_jumps() {
        let t0 = Instant::now();
        let mut s = CollapseState::new(true);
        let (f, moving) = s.step(true, None, t0, Mode::Full);
        assert_eq!(f.unwrap().height, None);
        assert!(!moving);
        let mut s = CollapseState::new(false);
        assert_eq!(s.step(true, Some(50.), t0, Mode::Static).0.unwrap().height, None);
        assert!(s.step(false, Some(50.), t0, Mode::Static).0.is_none());
    }

    #[test]
    fn delayed_stays_hidden_through_the_delay() {
        let _clock = super::clock();
        let easing = delayed(LOADING_DELAY, 150).easing;
        assert_eq!(easing(0.5), 0.);
        assert!(easing(0.9) > 0.5);
        assert_eq!(easing(1.), 1.);
    }
}

#[cfg(all(test, feature = "headless-bench"))]
mod frame_tests {
    use crate::components::region::Regions;
    use gpui::{div, point, prelude::*, px, Context, Modifiers, TestAppContext, Window};
    use std::{cell::Cell, rc::Rc, time::Duration};

    struct Fx {
        regions: Regions<Self>,
        in_region: bool,
        below: Rc<Cell<f32>>,
    }
    fn body(v: &mut Fx, window: &mut Window, cx: &mut Context<Fx>) -> gpui::AnyElement {
        let below = v.below.clone();
        div()
            .w_full()
            .pl(px(20.))
            .flex()
            .flex_col()
            .child(crate::components::disclosure::expander(
                "fx",
                "技术信息",
                div().w_full().h(px(120.)),
                window,
                cx,
            ))
            .child(
                gpui::canvas(move |b, _, _| below.set(b.origin.y.as_f32()), |_, _, _, _| {})
                    .w_full()
                    .h(px(10.)),
            )
            .into_any_element()
    }
    impl Render for Fx {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let inner = if self.in_region {
                self.regions.auto_height("fx", 300., cx, body)
            } else {
                body(self, window, cx)
            };
            div().size_full().flex().flex_col().child(inner)
        }
    }

    /// Samples the top of the element under the expander every 8 ms frame
    /// while it opens, closes, and reverses mid-way.
    fn sample(in_region: bool) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut cx = TestAppContext::single();
        let below = Rc::new(Cell::new(-1.));
        let b = below.clone();
        let (_view, cx) = cx.add_window_view(move |_, _| Fx {
            regions: Regions::default(),
            in_region,
            below: b,
        });
        let frame = |cx: &mut gpui::VisualTestContext| {
            cx.executor().advance_clock(Duration::from_millis(8));
            cx.update(|w, cx| {
                w.simulate_next_frame(cx);
            });
            cx.run_until_parked();
        };
        let frames = |n: usize, cx: &mut gpui::VisualTestContext| {
            let mut out = vec![below.get()];
            for _ in 0..n {
                frame(cx);
                out.push(below.get());
            }
            out
        };
        frames(4, cx);
        let toggle = point(px(34.), px(10.));
        cx.simulate_click(toggle, Modifiers::none());
        let open = frames(50, cx);
        cx.simulate_click(toggle, Modifiers::none());
        let close = frames(50, cx);
        // Open, then close again after six frames.
        cx.simulate_click(toggle, Modifiers::none());
        let mut reverse = frames(6, cx);
        cx.simulate_click(toggle, Modifiers::none());
        reverse.extend(frames(50, cx).into_iter().skip(1));
        (open, close, reverse)
    }

    fn assert_smooth(label: &str, frames: &[f32], total: f32) {
        for pair in frames.windows(2) {
            assert!(
                (pair[1] - pair[0]).abs() <= 0.2 * total,
                "{label}: jump {} -> {} in {frames:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn expander_height_moves_without_a_jump() {
        let _clock = super::clock();
        for in_region in [false, true] {
            let (open, close, reverse) = sample(in_region);
            let (low, high) = (open[0], *open.last().unwrap());
            let total = high - low;
            assert!(total > 100., "the expander did not open: {open:?}");
            assert!(open.windows(2).all(|p| p[1] + 1e-3 >= p[0]), "not monotonic: {open:?}");
            assert!(close.windows(2).all(|p| p[1] <= p[0] + 1e-3), "not monotonic: {close:?}");
            assert_eq!(*close.last().unwrap(), low);
            assert_smooth("open", &open, total);
            assert_smooth("close", &close, total);
            assert_smooth("reverse", &reverse, total);
            assert_eq!(*reverse.last().unwrap(), low);
        }
    }

    struct Titlebar;
    impl Render for Titlebar {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(48.))
                        .window_control_area(gpui::WindowControlArea::Drag),
                )
                .child(
                    // A scroll area reaching into the strip only scrolls.
                    div()
                        .id("scroll")
                        .absolute()
                        .top_0()
                        .left(px(200.))
                        .size(px(200.))
                        .overflow_y_scroll()
                        .child(
                            div()
                                .id("toggle")
                                .size(px(40.))
                                .on_click(|_, _, _| {}),
                        ),
                )
        }
    }

    #[test]
    fn titlebar_controls_take_their_presses() {
        let mut cx = TestAppContext::single();
        let (_view, cx) = cx.add_window_view(|_, _| Titlebar);
        let at = |cx: &mut gpui::VisualTestContext, x: f32, y: f32| {
            cx.simulate_mouse_move(point(px(x), px(y)), None, Modifiers::none());
            cx.update(|w, _| w.window_control_area_at_mouse())
        };
        // Empty strip: drag area. Over a scroll area: still the strip below.
        assert_eq!(at(cx, 20., 20.), Some(gpui::WindowControlArea::Drag));
        assert_eq!(at(cx, 300., 20.), Some(gpui::WindowControlArea::Drag));
        // A clickable control over the strip takes the press.
        assert_eq!(at(cx, 210., 20.), None);
        // Below the strip nothing is a window control.
        assert_eq!(at(cx, 20., 100.), None);
    }
}

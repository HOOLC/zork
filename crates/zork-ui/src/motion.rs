//! Shared motion tokens. Durations, curves and springs live here so pages never
//! carry raw numbers; see `apps/zork-design-pc/docs/07-motion.md`.
//!
//! Reduced motion is `App::reduce_motion`, seeded from the macOS accessibility
//! preference at startup. GPUI's `with_animation` and `with_spring` already
//! render the end state without scheduling frames when it is set.
use gpui::{Animation, AnimationPhase, SpringAnimation, SpringConfig};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

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
        assert_eq!(exit(SURFACE), 160);
        assert_eq!(exit(BASE), 120);
        set_slow(5.);
        assert_eq!(duration(BASE), Duration::from_millis(900));
        set_slow(1.);
        assert_eq!(duration(BASE), Duration::from_millis(180));
    }

    #[test]
    fn delayed_stays_hidden_through_the_delay() {
        let easing = delayed(LOADING_DELAY, 150).easing;
        assert_eq!(easing(0.5), 0.);
        assert!(easing(0.9) > 0.5);
        assert_eq!(easing(1.), 1.);
    }
}

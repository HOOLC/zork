//! Intrinsic-height disclosure. Content keeps its layout while its aperture moves.
use gpui::{prelude::*, px, App, ElementId, Entity, FocusHandle, Window};
use std::time::Instant;

const SPEED: f32 = 1200.;
const ACCELERATION: f32 = 16000.;
const RETRACT: f32 = 8.;

/// An analytical accelerate/cruise/brake trajectory. Both endpoint speeds are
/// zero for an ordinary toggle; short disclosures never reach the speed limit.
struct Travel {
    start: f32,
    target: f32,
    direction: f32,
    initial_speed: f32,
    peak: f32,
    acceleration: f32,
    accelerate_for: f32,
    cruise_for: f32,
    started: Instant,
}
impl Travel {
    fn new(start: f32, target: f32, velocity: f32, started: Instant) -> Self {
        let distance = (target - start).abs();
        let direction = (target - start).signum();
        // Reverse immediately from the painted position, without coasting
        // toward an obsolete destination. Same-direction content updates keep
        // their current speed instead of restarting the acceleration ramp.
        let initial_speed = (velocity * direction).clamp(0., SPEED);
        let acceleration = ACCELERATION.max(initial_speed.powi(2) / (2. * distance));
        let peak = (acceleration * distance + initial_speed.powi(2) / 2.)
            .sqrt()
            .min(SPEED);
        let accelerate_for = ((peak - initial_speed) / acceleration).max(0.);
        let ramp_distance = (2. * peak.powi(2) - initial_speed.powi(2)) / (2. * acceleration);
        let cruise_for = ((distance - ramp_distance) / peak).max(0.);
        Self {
            start,
            target,
            direction,
            initial_speed,
            peak,
            acceleration,
            accelerate_for,
            cruise_for,
            started,
        }
    }

    fn duration(&self) -> f32 {
        self.accelerate_for + self.cruise_for + self.peak / self.acceleration
    }

    fn sample(&self, elapsed: f32) -> (f32, f32, bool) {
        if elapsed >= self.duration() {
            return (self.target, 0., true);
        }
        let ramp_distance = (self.initial_speed + self.peak) * self.accelerate_for / 2.;
        let (distance, speed) = if elapsed < self.accelerate_for {
            (
                self.initial_speed * elapsed + self.acceleration * elapsed.powi(2) / 2.,
                self.initial_speed + self.acceleration * elapsed,
            )
        } else if elapsed < self.accelerate_for + self.cruise_for {
            (
                ramp_distance + self.peak * (elapsed - self.accelerate_for),
                self.peak,
            )
        } else {
            let brake_time = elapsed - self.accelerate_for - self.cruise_for;
            (
                ramp_distance + self.peak * self.cruise_for + self.peak * brake_time
                    - self.acceleration * brake_time.powi(2) / 2.,
                (self.peak - self.acceleration * brake_time).max(0.),
            )
        };
        (
            self.start + self.direction * distance,
            self.direction * speed,
            false,
        )
    }
}

#[derive(Default)]
struct Height {
    current: Option<f32>,
    velocity: f32,
    travel: Option<Travel>,
}
impl Height {
    fn advance(&mut self, target: f32, reduced: bool, now: Instant) -> f32 {
        let current = *self.current.get_or_insert(target);
        if reduced || current == target {
            self.current = Some(target);
            self.velocity = 0.;
            self.travel = None;
            return target;
        }
        if self
            .travel
            .as_ref()
            .is_none_or(|travel| travel.target != target)
        {
            self.travel = Some(Travel::new(current, target, self.velocity, now));
        }
        let travel = self.travel.as_ref().unwrap();
        let (position, velocity, done) =
            travel.sample(now.duration_since(travel.started).as_secs_f32());
        self.current = Some(position);
        self.velocity = velocity;
        if done {
            self.travel = None;
        }
        position
    }
}

struct State {
    height: Height,
    natural_height: f32,
    header: FocusHandle,
    body: FocusHandle,
}

/// Retain by stable ID even while closed. `width` is the available logical
/// width, used to measure the uncompressed child before laying out the aperture.
pub struct Collapse {
    id: ElementId,
    state: Entity<State>,
    expanded: bool,
    width: f32,
}
impl Collapse {
    pub fn new(
        id: impl Into<ElementId>,
        expanded: bool,
        width: f32,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let id = id.into();
        let state = window.use_keyed_state(id.clone(), cx, |_, cx| State {
            height: Height::default(),
            natural_height: 0.,
            header: cx.focus_handle().tab_stop(true),
            body: cx.focus_handle(),
        });
        if !expanded && state.read(cx).body.contains_focused(window, cx) {
            let header = state.read(cx).header.clone();
            header.focus(window, cx);
        }
        Self {
            id,
            state,
            expanded,
            width,
        }
    }

    pub fn header_focus(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).header.clone()
    }

    /// Do not construct business rows once the exit has finished.
    pub fn mounted(&self, cx: &App) -> bool {
        self.expanded || self.state.read(cx).height.current.is_some_and(|h| h > 0.)
    }

    /// Partially clipped children must not enter the keyboard traversal order.
    pub fn interactive(&self, cx: &App) -> bool {
        let height = &self.state.read(cx).height;
        self.expanded && height.travel.is_none() && height.current != Some(0.)
    }

    /// `on_frame` invalidates the owning layout region, including any cached
    /// intrinsic height. It is invoked only while the aperture is moving.
    pub fn element(
        self,
        content: Option<gpui::AnyElement>,
        on_frame: impl FnOnce(&mut Window, &mut App) + 'static,
        cx: &App,
    ) -> impl IntoElement {
        let state = self.state.read(cx);
        let body = state.body.clone();
        let opacity = if cx.reduce_motion() || state.natural_height == 0. {
            1.
        } else {
            (state.height.current.unwrap_or(state.natural_height) / state.natural_height)
                .clamp(0., 1.)
        };
        Aperture {
            collapse: self,
            content_offset: 0.,
            content: content.map(|content| {
                gpui::div()
                    .id("collapse-body")
                    .track_focus(&body)
                    .tab_stop(false)
                    .opacity(opacity)
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(content)
                    .into_any_element()
            }),
            on_frame: Some(Box::new(on_frame)),
        }
    }
}

type NextFrame = Box<dyn FnOnce(&mut Window, &mut App)>;
struct Aperture {
    collapse: Collapse,
    content_offset: f32,
    content: Option<gpui::AnyElement>,
    on_frame: Option<NextFrame>,
}
impl IntoElement for Aperture {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl gpui::Element for Aperture {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(self.collapse.id.clone())
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, ()) {
        let natural = self.content.as_mut().map_or(0., |content| {
            content
                .layout_as_root(
                    gpui::size(
                        gpui::AvailableSpace::Definite(px(self.collapse.width)),
                        gpui::AvailableSpace::MinContent,
                    ),
                    window,
                    cx,
                )
                .height
                .as_f32()
        });
        let target = if self.collapse.expanded { natural } else { 0. };
        let reduced = cx.reduce_motion();
        let previous = self.collapse.state.read(cx).height.current;
        let was_moving = self.collapse.state.read(cx).height.travel.is_some();
        let height = self.collapse.state.update(cx, |state, _| {
            state.natural_height = natural;
            state.height.advance(target, reduced, Instant::now())
        });
        self.content_offset = if natural > 0. && !reduced {
            -RETRACT * (1. - (height / natural).clamp(0., 1.))
        } else {
            0.
        };
        // Re-render a settled endpoint once to restore tab stops / unmount the
        // exit, including instantaneous reduced-motion changes.
        if height != target || was_moving || previous.is_some_and(|old| old != height) {
            if let Some(next) = self.on_frame.take() {
                window.on_next_frame(next);
            }
        }
        let mut style = gpui::Style::default();
        style.size.width = px(self.collapse.width).into();
        style.size.height = px(height).into();
        style.flex_shrink = 0.;
        (window.request_layout(style, None, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if bounds.size.height > px(0.) {
            window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                if let Some(content) = &mut self.content {
                    content.prepaint_at(
                        bounds.origin + gpui::point(px(0.), px(self.content_offset)),
                        window,
                        cx,
                    );
                }
            });
        }
    }
    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if bounds.size.height > px(0.) {
            window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                if let Some(content) = &mut self.content {
                    content.paint(window, cx);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn short_disclosure_remains_readable_during_the_first_frames() {
        let travel = Travel::new(0., 68., 0., Instant::now());
        assert!(travel.sample(1. / 60.).0 < 68. * 0.1);
        assert!(travel.sample(2. / 60.).0 < 68. * 0.25);
        assert!((0.12..0.15).contains(&travel.duration()));
        assert!(travel.sample(travel.duration() - 0.001).1 < 20.);
        assert_eq!(travel.sample(travel.duration()), (68., 0., true));
    }

    #[test]
    fn longer_disclosures_share_a_speed_limit_not_a_duration() {
        let now = Instant::now();
        let short = Travel::new(0., 136., 0., now);
        let long = Travel::new(0., 1360., 0., now);
        assert_eq!(short.peak, SPEED);
        assert_eq!(long.peak, SPEED);
        assert!((long.duration() - short.duration() - 1224. / SPEED).abs() < 0.0001);
        for step in 0..=100 {
            let (position, velocity, _) = long.sample(long.duration() * step as f32 / 100.);
            assert!((0. ..=1360.).contains(&position));
            assert!((0. ..=SPEED).contains(&velocity));
        }
    }

    #[test]
    fn retarget_starts_at_the_painted_height_without_spending_stale_time() {
        let now = Instant::now();
        let mut height = Height {
            current: Some(0.),
            ..Default::default()
        };
        height.advance(136., false, now);
        let painted = height.advance(136., false, now + Duration::from_millis(50));
        assert!((painted - 20.).abs() < 0.001);
        let changed = now + Duration::from_millis(200);
        assert_eq!(height.advance(0., false, changed), painted);
        let reversing = height.advance(0., false, changed + Duration::from_millis(25));
        assert!(reversing > 0. && reversing < painted);
        assert_eq!(
            height.advance(0., false, changed + Duration::from_secs(1)),
            0.
        );
        assert!(height.travel.is_none());
    }

    #[test]
    fn content_updates_keep_forward_velocity_and_reduced_motion_settles() {
        let now = Instant::now();
        let mut height = Height {
            current: Some(0.),
            ..Default::default()
        };
        height.advance(136., false, now);
        let changed = now + Duration::from_millis(50);
        let painted = height.advance(136., false, changed);
        let velocity = height.velocity;
        assert_eq!(height.advance(300., false, changed), painted);
        assert_eq!(height.velocity, velocity);
        assert!(height.advance(300., false, changed + Duration::from_millis(25)) > painted + 20.);
        assert_eq!(
            height.advance(300., true, changed + Duration::from_millis(30)),
            300.
        );
        assert_eq!(height.velocity, 0.);
        assert!(height.travel.is_none());
    }
}

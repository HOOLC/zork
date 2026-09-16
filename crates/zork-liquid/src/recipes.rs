//! Target geometry and feedback recipes. Measurements use logical units.
use crate::Pose;

pub const SEGMENT_INSET: f64 = 4.;
pub const SEGMENT_GAP: f64 = 2.;

pub fn segment_pose(width: f64, height: f64, count: usize, selected: usize) -> Pose {
    let count = count.max(1);
    let item =
        ((width - 2. * SEGMENT_INSET - SEGMENT_GAP * (count - 1) as f64) / count as f64).max(2.);
    let height = (height - 2. * SEGMENT_INSET).max(2.);
    Pose::rect(
        SEGMENT_INSET + selected.min(count - 1) as f64 * (item + SEGMENT_GAP),
        SEGMENT_INSET,
        item,
        height,
        height / 2.,
    )
}

pub fn toggle_pose(checked: bool) -> Pose {
    Pose::rect(7. + if checked { 17. } else { 0. }, 7., 18., 18., 9.)
}

pub fn pressed_pose(rest: Pose, point: [f64; 2]) -> Pose {
    let squeeze = (rest.h * 0.14).clamp(2., 4.5);
    let width = (rest.w - (rest.h * 0.2).min(rest.w * 0.08)).max(2.);
    let height = (rest.h - squeeze).max(2.);
    let mut target = Pose::rect(0., 0., width, height, rest.r * height / rest.h);
    target.cx = rest.cx + (point[0].clamp(0., 1.) - 0.5) * 1.4;
    target.cy = rest.cy + 0.65;
    target
}

pub fn press_depth(rest: Pose) -> f64 {
    (rest.h * 0.075).clamp(1.5, 2.5).min((rest.h - 2.).max(0.))
}
pub fn reveal(progress: f64) -> f32 {
    ((progress - 0.45) / 0.4).clamp(0., 1.) as f32
}
pub fn member_reveal(width: f64) -> f64 {
    ((width - 32.) / 60.).clamp(0., 1.)
}

pub fn slider_pose(bounds: Pose, fraction: f64) -> Pose {
    slider_target(bounds, fraction, false, false)
}
pub const SLIDER_PLAYBACK_RATE: f64 = 3.;
pub fn slider_target(bounds: Pose, fraction: f64, vertical: bool, pressed: bool) -> Pose {
    let diameter = bounds.w.min(bounds.h).min(20.).max(2.);
    let (mut w, mut h) = if pressed {
        (diameter * 1.25, diameter * 0.9)
    } else {
        (diameter, diameter)
    };
    if vertical {
        std::mem::swap(&mut w, &mut h);
    }
    let center = |start: f64, length: f64| {
        start + diameter / 2. + (length - diameter).max(1.) * fraction.clamp(0., 1.)
    };
    let cx = if vertical {
        bounds.cx
    } else {
        center(bounds.left(), bounds.w)
    };
    let cy = if vertical {
        center(bounds.top(), bounds.h)
    } else {
        bounds.cy
    };
    Pose::rect(cx - w / 2., cy - h / 2., w, h, w.min(h) / 2.)
}
pub fn slider_track(bounds: Pose) -> Pose {
    slider_rail(bounds, false)
}
pub fn slider_rail(bounds: Pose, vertical: bool) -> Pose {
    let diameter = bounds.w.min(bounds.h).min(20.).max(2.);
    let start = if vertical {
        bounds.top()
    } else {
        bounds.left()
    };
    let length = if vertical { bounds.h } else { bounds.w };
    slider_range(
        bounds,
        start + diameter / 2.,
        start + (length - diameter / 2.).max(diameter / 2. + 2.),
        vertical,
    )
}
pub fn slider_range(bounds: Pose, first: f64, last: f64, vertical: bool) -> Pose {
    let thickness = bounds.w.min(bounds.h).min(6.).max(2.);
    if vertical {
        Pose::rect(
            bounds.cx - thickness / 2.,
            first.min(last),
            thickness,
            (last - first).abs(),
            thickness / 2.,
        )
    } else {
        Pose::rect(
            first.min(last),
            bounds.cy - thickness / 2.,
            (last - first).abs(),
            thickness,
            thickness / 2.,
        )
    }
}
pub fn slider_fill(bounds: Pose, thumb: Pose) -> Pose {
    let track = slider_track(bounds);
    Pose::rect(
        track.left(),
        track.top(),
        (thumb.cx - track.left()).clamp(2., track.w),
        track.h,
        track.r,
    )
}

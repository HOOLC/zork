//! Native, hover-triggered adaptations of the approved Fold motion studies.
//! Timelines are single-shot; macOS accessibility preference is bridged to GPUI.
use crate::automation::{AutomationElementExt, AutomationRole};
use gpui::{div, prelude::*, px, rgb, Animation, AnimationExt, Context, Window};
use serde::Deserialize;
use std::time::Instant;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::OnceLock,
    time::Duration,
};

#[derive(Clone, Copy)]
pub enum BrandMotion {
    Icon,
    Wordmark,
    Linked,
    Morph,
    Header,
}
pub struct Brand {
    mode: BrandMotion,
    sequence: u64,
    hovered: bool,
    started: Option<Instant>,
    background: u32,
    morph_progress: Rc<Cell<f32>>,
    morph_from: f32,
    morph_to: f32,
    morph_paths: Rc<RefCell<MorphPathCache>>,
}
impl Brand {
    pub fn new(mode: BrandMotion, background: u32) -> Self {
        Self {
            mode,
            sequence: 0,
            hovered: false,
            started: None,
            background,
            morph_progress: Rc::new(Cell::new(1.)),
            morph_from: 1.,
            morph_to: 1.,
            morph_paths: Rc::new(RefCell::new(MorphPathCache::default())),
        }
    }
    pub fn morph_progress(&self) -> f32 {
        self.morph_progress.get()
    }
    fn render_header(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let from = self.morph_from;
        let to = self.morph_to;
        let duration = (geometry().duration_ms as f32 * (to - from).abs()).max(1.) as u64;
        let progress = self.morph_progress.clone();
        let painted = self.morph_progress.clone();
        let paths = self.morph_paths.clone();
        let background = self.background;
        let animated = !cx.reduce_motion() && from != to && progress.get() != to;
        if !animated {
            progress.set(to);
        }
        let canvas = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                paint_morph(bounds, painted.get(), background, window, &paths)
            },
        )
        .size_full();
        div()
            .id("brand-header")
            .h(px(48.))
            .w(px(120.))
            .max_w_full()
            .on_hover(cx.listener(|v, hovered, _, cx| {
                if v.hovered != *hovered {
                    v.morph_from = v.morph_progress.get();
                    v.morph_to = if *hovered { 0. } else { 1. };
                    v.hovered = *hovered;
                    v.sequence = v.sequence.wrapping_add(1);
                    cx.notify();
                }
            }))
            .child(if animated {
                canvas
                    .with_animation(
                        format!("header-morph-{}", self.sequence),
                        timeline(duration),
                        move |canvas, t| {
                            progress.set(from + (to - from) * t);
                            canvas
                        },
                    )
                    .into_any_element()
            } else {
                canvas.into_any_element()
            })
            .automation(AutomationRole::Status, "Zork")
            .into_any_element()
    }
}
/// Evaluate a CSS cubic-bezier by x, rather than using x as the curve parameter.
fn bezier(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let x = x.clamp(0., 1.);
    let (mut lo, mut hi): (f32, f32) = (0., 1.);
    for _ in 0..16 {
        let t = (lo + hi) * 0.5;
        let bx = 3. * (1. - t).powi(2) * t * x1 + 3. * (1. - t) * t * t * x2 + t * t * t;
        if bx < x {
            lo = t
        } else {
            hi = t
        }
    }
    let t = (lo + hi) * 0.5;
    3. * (1. - t).powi(2) * t * y1 + 3. * (1. - t) * t * t * y2 + t * t * t
}
fn keyframe(t: f32, values: [f32; 4]) -> f32 {
    let keys = [0., 0.35, 0.65, 1.];
    let i = if t <= keys[1] {
        0
    } else if t <= keys[2] {
        1
    } else {
        2
    };
    let local = bezier((t - keys[i]) / (keys[i + 1] - keys[i]), 0.2, 0.7, 0.2, 1.);
    values[i] + (values[i + 1] - values[i]) * local
}
fn timeline(duration: u64) -> Animation {
    Animation::new(Duration::from_millis(duration))
}
#[derive(Clone, Deserialize)]
struct Morph {
    #[serde(rename = "durationMs")]
    duration_ms: u64,
    #[serde(rename = "viewBox")]
    view_box: [f32; 4],
    frames: Vec<MorphFrame>,
}
#[derive(Clone, Deserialize)]
struct MorphFrame {
    time_ms: f32,
    contours: Vec<MorphContour>,
}
#[derive(Clone, Deserialize)]
struct MorphContour {
    id: String,
    fill: MorphFill,
    points: Vec<[f32; 2]>,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum MorphFill {
    Ink,
    Counter,
}
impl Morph {
    fn validate(&self) -> Result<(), &'static str> {
        if self.duration_ms == 0 || self.frames.len() < 2 {
            return Err("morph requires a positive duration and at least two frames");
        }
        if !self.view_box.iter().all(|v| v.is_finite())
            || self.view_box[2] <= 0.
            || self.view_box[3] <= 0.
        {
            return Err("morph requires a finite positive viewBox");
        }
        let first = &self.frames[0];
        if first.time_ms != 0. || self.frames.last().unwrap().time_ms != self.duration_ms as f32 {
            return Err("morph frames must cover both timeline endpoints");
        }
        if first.contours.is_empty()
            || self
                .frames
                .windows(2)
                .any(|frames| frames[0].time_ms >= frames[1].time_ms)
        {
            return Err("morph frames must be nonempty and strictly ordered");
        }
        let mut ids = std::collections::HashSet::new();
        if first
            .contours
            .iter()
            .any(|contour| contour.id.is_empty() || !ids.insert(&contour.id))
        {
            return Err("morph contour identifiers must be nonempty and unique");
        }
        for frame in &self.frames {
            if !frame.time_ms.is_finite() || frame.contours.len() != first.contours.len() {
                return Err("morph frame topology changed");
            }
            for (contour, base) in frame.contours.iter().zip(&first.contours) {
                if contour.id != base.id
                    || contour.fill != base.fill
                    || contour.points.len() != 128
                    || !contour.points.iter().flatten().all(|v| v.is_finite())
                {
                    return Err("morph contour identity, fill, or point topology changed");
                }
            }
        }
        Ok(())
    }

    /// The design data already contains the hop/crawl rhythm. Interpolate only
    /// between adjacent authored frames, without adding easing or path staging.
    fn bracket(&self, elapsed_ms: f32) -> (&MorphFrame, &MorphFrame, f32) {
        let elapsed = elapsed_ms.clamp(0., self.duration_ms as f32);
        let right = self
            .frames
            .partition_point(|frame| frame.time_ms <= elapsed);
        if right == self.frames.len() {
            let last = self.frames.last().unwrap();
            return (last, last, 0.);
        }
        let left = &self.frames[right.saturating_sub(1)];
        let right = &self.frames[right];
        let progress = (elapsed - left.time_ms) / (right.time_ms - left.time_ms);
        (left, right, progress)
    }
}
fn geometry() -> &'static Morph {
    static GEOMETRY: OnceLock<Morph> = OnceLock::new();
    GEOMETRY.get_or_init(|| {
        let geometry: Morph =
            serde_json::from_str(include_str!("../../assets/motion/morph-points.json"))
                .expect("approved morph geometry");
        geometry
            .validate()
            .expect("consistent approved morph keyframes");
        geometry
    })
}
fn interpolate_point(from: [f32; 2], to: [f32; 2], progress: f32) -> [f32; 2] {
    [
        from[0] + (to[0] - from[0]) * progress,
        from[1] + (to[1] - from[1]) * progress,
    ]
}
fn append_polygon(
    path: &mut gpui::PathBuilder,
    bounds: gpui::Bounds<gpui::Pixels>,
    view_box: [f32; 4],
    points: impl IntoIterator<Item = [f32; 2]>,
) {
    for (i, point) in points.into_iter().enumerate() {
        let position = gpui::point(
            bounds.origin.x + bounds.size.width * ((point[0] - view_box[0]) / view_box[2]),
            bounds.origin.y + bounds.size.height * ((point[1] - view_box[1]) / view_box[3]),
        );
        if i == 0 {
            path.move_to(position)
        } else {
            path.line_to(position)
        }
    }
    path.close();
}
#[derive(Default)]
struct MorphPathCache {
    key: Option<(gpui::Bounds<gpui::Pixels>, u32)>,
    paths: Vec<(MorphFill, gpui::Path<gpui::Pixels>)>,
    #[cfg(test)]
    rebuilds: usize,
}
impl MorphPathCache {
    fn get(
        &mut self,
        bounds: gpui::Bounds<gpui::Pixels>,
        t: f32,
    ) -> &[(MorphFill, gpui::Path<gpui::Pixels>)] {
        let t = t.clamp(0., 1.);
        let key = (bounds, t.to_bits());
        if self.key != Some(key) {
            self.paths = tessellate_morph(bounds, t);
            self.key = Some(key);
            #[cfg(test)]
            {
                self.rebuilds += 1;
            }
        }
        &self.paths
    }
}

fn tessellate_morph(
    bounds: gpui::Bounds<gpui::Pixels>,
    t: f32,
) -> Vec<(MorphFill, gpui::Path<gpui::Pixels>)> {
    let geometry = geometry();
    let (from, to, progress) = geometry.bracket(t * geometry.duration_ms as f32);
    let mut paths = Vec::with_capacity(2);
    // The four body contours share nonnegative winding. Fill their subpaths once
    // with nonzero winding to merge shared edges and overlaps without AA seams.
    // The counter contours cut out the eyes and letter openings using the panel background.
    for fill in [MorphFill::Ink, MorphFill::Counter] {
        let mut path = gpui::PathBuilder::fill().with_style(gpui::PathStyle::Fill(
            gpui::FillOptions::default().with_fill_rule(gpui::FillRule::NonZero),
        ));
        for (from, to) in from
            .contours
            .iter()
            .zip(&to.contours)
            .filter(|(from, _)| from.fill == fill)
        {
            append_polygon(
                &mut path,
                bounds,
                geometry.view_box,
                from.points
                    .iter()
                    .zip(&to.points)
                    .map(|(&from, &to)| interpolate_point(from, to, progress)),
            );
        }
        if let Ok(path) = path.build() {
            paths.push((fill, path));
        }
    }
    paths
}

fn paint_morph(
    bounds: gpui::Bounds<gpui::Pixels>,
    t: f32,
    background: u32,
    window: &mut Window,
    cache: &RefCell<MorphPathCache>,
) {
    // Scrolling unrelated content must not tessellate a stationary header logo.
    // Each carrier retains one geometry; hover frames and resized bounds replace it.
    for (fill, path) in cache.borrow_mut().get(bounds, t) {
        window.paint_path(
            path.clone(),
            rgb(match fill {
                MorphFill::Ink => crate::design::ZORK_UI.palette.text,
                MorphFill::Counter => background,
            }),
        );
    }
}

impl Render for Brand {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        {
            // Startup seeds the preference; afterwards follow its changes
            // only, so an explicit setting (tests, exported stills) stays.
            use std::sync::atomic::{AtomicU8, Ordering};
            static SEEN: AtomicU8 = AtomicU8::new(2);
            let system = system_reduce_motion();
            let seen = SEEN.swap(system as u8, Ordering::Relaxed);
            if seen != 2 && seen != system as u8 {
                cx.set_reduce_motion(system);
            }
        }
        if matches!(self.mode, BrandMotion::Header) {
            return self.render_header(cx);
        }
        let sequence = self.sequence;
        let mode = self.mode;
        let duration = match mode {
            BrandMotion::Icon => 680,
            BrandMotion::Wordmark => 675,
            BrandMotion::Linked => 805,
            BrandMotion::Morph | BrandMotion::Header => geometry().duration_ms,
        };
        // Entity state survives navigation; an old hover must not replay when
        // this carrier is mounted again on a later visit.
        let animated = self.hovered
            && sequence > 0
            && !cx.reduce_motion()
            && self
                .started
                .is_some_and(|started| started.elapsed() < Duration::from_millis(duration));
        let id = match mode {
            BrandMotion::Icon => "brand-icon",
            BrandMotion::Wordmark => "brand-wordmark",
            BrandMotion::Linked => "brand-linked",
            BrandMotion::Morph => "brand-morph",
            BrandMotion::Header => "brand-header",
        };
        let mut row = div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .on_hover(cx.listener(|v, hovered, _, cx| {
                if *hovered && !v.hovered {
                    v.sequence = v.sequence.wrapping_add(1);
                    v.started = Some(Instant::now());
                }
                v.hovered = *hovered;
                cx.notify();
            }));
        if matches!(mode, BrandMotion::Morph) {
            let progress = Rc::new(Cell::new(if self.hovered { 1. } else { 0. }));
            let paint_progress = progress.clone();
            let paths = self.morph_paths.clone();
            let background = self.background;
            let canvas = gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    paint_morph(bounds, paint_progress.get(), background, window, &paths);
                },
            )
            .w(px(220.))
            .h(px(88.));
            return row
                .child(if animated && self.hovered {
                    canvas
                        .with_animation(
                            format!("morph-{sequence}"),
                            timeline(duration),
                            move |canvas, t| {
                                progress.set(t);
                                canvas
                            },
                        )
                        .into_any_element()
                } else {
                    canvas.into_any_element()
                })
                .automation(AutomationRole::Status, id)
                .into_any_element();
        }
        if matches!(mode, BrandMotion::Icon | BrandMotion::Linked) {
            let size = if matches!(mode, BrandMotion::Icon) {
                38.
            } else {
                20.
            };
            let mark = gpui::svg()
                .path("brand/mark.svg")
                .size(px(size))
                .text_color(rgb(crate::design::ZORK_UI.palette.text));
            row = row.child(if animated {
                let duration = if matches!(mode, BrandMotion::Icon) {
                    680
                } else {
                    805
                };
                mark.with_animation(
                    format!("icon-{sequence}"),
                    timeline(duration),
                    move |mark, t| {
                        let transform = if matches!(mode, BrandMotion::Icon) {
                            gpui::Transformation::rotate(gpui::radians(
                                keyframe(t, [0., -7., 3., 0.]).to_radians(),
                            ))
                        } else {
                            gpui::Transformation::rotate(gpui::radians(0.)).with_translation(
                                gpui::point(
                                    px(keyframe((t * 805. / 700.).min(1.), [0., 11., 2., 0.])
                                        * size
                                        / 128.),
                                    px(0.),
                                ),
                            )
                        };
                        mark.with_transformation(transform)
                    },
                )
                .into_any_element()
            } else {
                mark.into_any_element()
            });
        }
        if matches!(mode, BrandMotion::Wordmark | BrandMotion::Linked) {
            for (index, (left, right)) in [(0., 34.), (34., 68.), (68., 96.), (96., 136.)]
                .into_iter()
                .enumerate()
            {
                let letter = div()
                    .relative()
                    .w(px((right - left) * 0.6))
                    .h(px(28.))
                    .overflow_hidden()
                    .child(
                        // Tinted like the mark: the asset's own ink fill vanishes in dark.
                        gpui::svg()
                            .path("brand/zork-wordmark.svg")
                            .relative()
                            .left(px(-left * 0.6))
                            .w(px(81.6))
                            .h(px(26.4))
                            .flex_shrink_0()
                            .text_color(rgb(crate::design::ZORK_UI.palette.text)),
                    );
                // Letter wrappers share one contiguous wordmark. Translation never changes layout.
                let element = if animated {
                    let linked = matches!(mode, BrandMotion::Linked);
                    let total = if linked { 805 } else { 675 };
                    letter
                        .with_animation(
                            format!("letter-{sequence}-{index}"),
                            timeline(total),
                            move |letter, t| {
                                let delay = if linked {
                                    140. + index as f32 * 55.
                                } else {
                                    index as f32 * 65.
                                };
                                let local = ((t * total as f32 - delay)
                                    / if linked { 500. } else { 480. })
                                .clamp(0., 1.);
                                letter
                                    .left(px(if linked {
                                        keyframe(local, [0., 2., -0.5, 0.]) * 0.6
                                    } else {
                                        0.
                                    }))
                                    .top(px(keyframe(
                                        local,
                                        if linked {
                                            [0., -1.8, 0., 0.]
                                        } else {
                                            [0., -2.5, 0.5, 0.]
                                        },
                                    ) * 0.6))
                            },
                        )
                        .into_any_element()
                } else {
                    letter.into_any_element()
                };
                // The mark-wordmark gap is a sibling margin, not a gap between letters.
                row = row.child(
                    div()
                        .ml(px(if index == 0 && matches!(mode, BrandMotion::Linked) {
                            8.
                        } else {
                            0.
                        }))
                        .child(element),
                );
            }
            row = row.gap_0();
        }
        row.automation(AutomationRole::Status, id)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stationary_morph_reuses_mesh_and_hover_or_resize_invalidates_it() {
        let bounds = gpui::Bounds::new(gpui::point(px(8.), px(12.)), gpui::size(px(120.), px(48.)));
        let mut cache = MorphPathCache::default();
        let initial = cache.get(bounds, 1.)[0].1.vertices[0].xy_position;
        for _ in 0..240 {
            assert_eq!(cache.get(bounds, 1.)[0].1.vertices[0].xy_position, initial);
        }
        assert_eq!(
            cache.rebuilds, 1,
            "scrolling rebuilt stationary logo geometry"
        );
        assert_ne!(cache.get(bounds, 0.)[0].1.vertices[0].xy_position, initial);
        assert_eq!(cache.rebuilds, 2);
        let mut moved = bounds;
        moved.origin.x += px(20.);
        let before = cache.get(bounds, 0.)[0].1.vertices[0].xy_position;
        assert_eq!(
            cache.get(moved, 0.)[0].1.vertices[0].xy_position.x,
            before.x + px(20.)
        );
        assert_eq!(cache.rebuilds, 3);
    }

    fn sample_morph() -> Morph {
        Morph {
            duration_ms: 1800,
            view_box: [0., 0., 400., 160.],
            frames: [(0., [0., 20.]), (300., [30., -10.]), (1800., [90., 50.])]
                .into_iter()
                .map(|(time_ms, point)| MorphFrame {
                    time_ms,
                    contours: vec![MorphContour {
                        id: "z".into(),
                        fill: MorphFill::Ink,
                        points: vec![point; 128],
                    }],
                })
                .collect(),
        }
    }
    fn sample_point(morph: &Morph, time_ms: f32) -> [f32; 2] {
        let (from, to, t) = morph.bracket(time_ms);
        interpolate_point(from.contours[0].points[0], to.contours[0].points[0], t)
    }
    #[test]
    fn keyframes_use_authored_intervals_and_preserve_exact_endpoints() {
        let morph = sample_morph();
        assert!(morph.validate().is_ok());
        assert_eq!(sample_point(&morph, -10.), [0., 20.]);
        assert_eq!(sample_point(&morph, 150.), [15., 5.]);
        assert_eq!(sample_point(&morph, 300.), [30., -10.]);
        assert_eq!(sample_point(&morph, 1050.), [60., 20.]);
        assert_eq!(sample_point(&morph, 1800.), [90., 50.]);
        assert_eq!(sample_point(&morph, 2000.), [90., 50.]);
    }
    #[test]
    fn malformed_keyframes_cannot_pair_different_contours_or_times() {
        let base = sample_morph();
        let mut changed = base.clone();
        changed.frames[1].contours[0].id = "eye-left".into();
        assert!(changed.validate().is_err());
        changed = base.clone();
        changed.frames[1].contours[0].fill = MorphFill::Counter;
        assert!(changed.validate().is_err());
        changed = base.clone();
        changed.frames[1].contours[0].points.pop();
        assert!(changed.validate().is_err());
        changed = base.clone();
        changed.frames[1].time_ms = 0.;
        assert!(changed.validate().is_err());
        changed = base;
        changed.frames[1].contours[0].points[0][0] = f32::NAN;
        assert!(changed.validate().is_err());
    }
    #[test]
    fn crawling_notch_anchors_follow_the_authored_z_contour_in_every_frame() {
        let raw: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/motion/morph-points.json")).unwrap();
        let geometry = geometry();
        let anchors = raw["notch"]["frames"].as_array().unwrap();
        assert_eq!(anchors.len(), geometry.frames.len());
        assert_eq!(
            raw["notch"]["vertex_indices"],
            serde_json::json!([0, 12, 36])
        );
        for (frame, anchors) in geometry.frames.iter().zip(anchors) {
            assert_eq!(frame.time_ms, anchors["time_ms"].as_f64().unwrap() as f32);
            let z = frame
                .contours
                .iter()
                .find(|contour| contour.id == "z")
                .unwrap();
            for (anchor, index) in [0, 12, 36].into_iter().enumerate() {
                let expected: [f32; 2] =
                    serde_json::from_value(anchors["points"][anchor].clone()).unwrap();
                assert_eq!(z.points[index], expected);
            }
        }
        assert_ne!(
            geometry.frames[0].contours[0].points[12],
            geometry.frames.last().unwrap().contours[0].points[12]
        );
    }
    #[test]
    fn visible_body_stays_centered_and_inside_the_viewbox() {
        let geometry = geometry();
        for frame in &geometry.frames {
            let body_points = frame
                .contours
                .iter()
                .filter(|c| ["z", "o", "r", "k"].contains(&c.id.as_str()))
                .flat_map(|c| &c.points);
            let (mut left, mut right) = (f32::INFINITY, f32::NEG_INFINITY);
            for point in body_points {
                left = left.min(point[0]);
                right = right.max(point[0]);
            }
            assert!(
                ((left + right) * 0.5 - 200.).abs() <= 0.001,
                "body center at {} ms",
                frame.time_ms
            );
            for point in frame.contours.iter().flat_map(|c| &c.points) {
                assert!(
                    point[0] >= -0.001
                        && point[0] <= 400.001
                        && point[1] >= -0.001
                        && point[1] <= 160.001,
                    "geometry clips at {} ms: {:?}",
                    frame.time_ms,
                    point
                );
            }
        }
    }
    #[test]
    fn approved_asset_preserves_four_body_edges_and_two_counter_contours() {
        let raw: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/motion/morph-points.json")).unwrap();
        assert_eq!(raw["mode"], "rounded-hop-unfold");
        let geometry = geometry();
        for frame in &geometry.frames {
            for contour in frame
                .contours
                .iter()
                .filter(|contour| contour.fill == MorphFill::Ink)
            {
                let twice_area: f64 = contour
                    .points
                    .iter()
                    .zip(contour.points.iter().cycle().skip(1))
                    .map(|(a, b)| a[0] as f64 * b[1] as f64 - b[0] as f64 * a[1] as f64)
                    .sum();
                assert!(
                    twice_area >= -0.001,
                    "{} at {} ms must share the solid contours' winding",
                    contour.id,
                    frame.time_ms
                );
            }
        }
        assert_eq!(geometry.frames[0].contours.len(), 6);
        assert_eq!(
            geometry.frames[0]
                .contours
                .iter()
                .filter(|c| c.fill == MorphFill::Ink)
                .count(),
            4
        );
        assert_eq!(
            geometry.frames[0]
                .contours
                .iter()
                .filter(|c| c.fill == MorphFill::Counter)
                .count(),
            2
        );
        assert_eq!(
            geometry.frames[0]
                .contours
                .iter()
                .map(|c| (c.id.as_str(), c.fill))
                .collect::<Vec<_>>(),
            [
                ("z", MorphFill::Ink),
                ("o", MorphFill::Ink),
                ("r", MorphFill::Ink),
                ("k", MorphFill::Ink),
                ("eye-left", MorphFill::Counter),
                ("eye-right", MorphFill::Counter)
            ]
        );
    }
}

/// Read AppKit on the GPUI main thread. GPUI's preference is otherwise unset by
/// this platform version. Re-reading during decorative frames and on hover also
/// picks up settings changed while the client is open, without a polling service.
#[cfg(target_os = "macos")]
fn system_reduce_motion() -> bool {
    #[cfg(debug_assertions)]
    if let Ok(value) = std::env::var("ZORK_GUI_TEST_REDUCE_MOTION") {
        return value == "1";
    }
    use std::ffi::{c_char, c_void};
    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    // SAFETY: These two no-argument AppKit selectors have stable Objective-C
    // pointer and BOOL return signatures; calls run only on the GPUI main thread.
    unsafe {
        let class = objc_getClass(c"NSWorkspace".as_ptr());
        if class.is_null() {
            return false;
        }
        let object: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        let boolean: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i8 =
            std::mem::transmute(objc_msgSend as *const ());
        let workspace = object(class, sel_registerName(c"sharedWorkspace".as_ptr()));
        !workspace.is_null()
            && boolean(
                workspace,
                sel_registerName(c"accessibilityDisplayShouldReduceMotion".as_ptr()),
            ) != 0
    }
}

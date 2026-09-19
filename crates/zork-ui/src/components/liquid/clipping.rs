//! Clip fixed content regions and feedback without reflowing their layout.
use super::*;
#[path = "contrast.rs"]
mod contrast;
type Point = super::super::Point;
pub(super) type Polygons = Vec<Vec<Point>>;

#[derive(Default)]
pub(super) struct InkCache {
    path: Option<Rc<Contour>>,
    values: std::collections::HashMap<[u32; 4], Bounds<Pixels>>,
}

#[derive(Clone)]
pub struct ContentClip {
    path: Rc<Contour>,
    paint: PaintHandle,
    polygons: std::sync::Arc<Polygons>,
}

/// Sections retain their layout and element identity; prepaint selects either
/// a contour-safe region or fixed section padding for independent content.
#[derive(Clone, Default)]
pub struct ContentClipBinding(Rc<RefCell<Option<ContentClip>>>, bool);

impl ContentClipBinding {
    /// Keep section padding independent of the animated material contour.
    pub(crate) fn fixed_layout() -> Self {
        Self(Default::default(), true)
    }
    pub(crate) fn bind(&self, clip: ContentClip) {
        *self.0.borrow_mut() = Some(clip);
    }
    pub(crate) fn region(
        &self,
        content: impl IntoElement,
        top: f32,
        bottom: f32,
    ) -> impl IntoElement {
        ClippedContent {
            child: content.into_any_element(),
            clip: if self.1 {
                ClipSource::Layout
            } else {
                ClipSource::Bound(self.clone())
            },
            ink_height: f32::MAX,
            vertical_insets: Some([top, bottom]),
        }
    }
}

enum ClipSource {
    Layout,
    Fixed(ContentClip),
    Bound(ContentClipBinding),
}

impl Surface {
    pub fn content_clip(&self) -> ContentClip {
        ContentClip::new(self.model.contour(), self.paint.clone())
    }
}
impl ContentClip {
    pub(super) fn new(path: Rc<Contour>, paint: PaintHandle) -> Self {
        let polygons = {
            let cache = paint.0.borrow();
            let mut cached_polygons = cache.clip_polygons.borrow_mut();
            match &*cached_polygons {
                Some((prior, polygons)) if Rc::ptr_eq(prior, &path) => polygons.clone(),
                _ => {
                    let polygons: std::sync::Arc<Polygons> = std::sync::Arc::new(
                        path.loops.iter().map(|curves| flatten(curves)).collect(),
                    );
                    *cached_polygons = Some((path.clone(), polygons.clone()));
                    polygons
                }
            }
        };
        Self {
            path,
            paint,
            polygons,
        }
    }
}

#[derive(Default)]
struct FillCache {
    key: Option<(Rc<Contour>, super::super::Pose)>,
    fill: Option<Path<Pixels>>,
    stroke: Option<Path<Pixels>>,
}

impl ContentClip {
    /// Feedback is intersected with the liquid contour, so a clipped rounded
    /// row never becomes a rectangular stripe over the underlying page.
    pub fn fill(
        &self,
        id: impl Into<ElementId>,
        radius: f64,
        color: Option<u32>,
        stroke: Option<u32>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let cache = window
            .use_keyed_state(id, cx, |_, _| Rc::new(RefCell::new(FillCache::default())))
            .read(cx)
            .clone();
        let clip = self.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let origin = clip.paint.0.borrow().bounds.origin;
                let pose = super::super::Pose::rect(
                    (bounds.origin.x - origin.x).as_f32() as f64,
                    (bounds.origin.y - origin.y).as_f32() as f64,
                    bounds.size.width.as_f32() as f64,
                    bounds.size.height.as_f32() as f64,
                    radius,
                );
                let mut cache = cache.borrow_mut();
                if cache
                    .key
                    .as_ref()
                    .is_none_or(|(path, prior)| !Rc::ptr_eq(path, &clip.path) || *prior != pose)
                {
                    let region = flatten(&rounded_rectangle(pose, 0.6));
                    let mut loops = Vec::new();
                    for polygon in &*clip.polygons {
                        let points = intersect(polygon.clone(), &region);
                        if points.len() < 3 {
                            continue;
                        }
                        loops.push(
                            points
                                .iter()
                                .enumerate()
                                .map(|(i, &from)| {
                                    let to = points[(i + 1) % points.len()];
                                    super::super::Cubic {
                                        from,
                                        c1: from,
                                        c2: to,
                                        to,
                                    }
                                })
                                .collect(),
                        );
                    }
                    let contour = Contour {
                        loops,
                        sampled_points: 0,
                        full_grid_points: 0,
                        used_fallback: false,
                        revision: 0,
                    };
                    cache.fill = super::super::tessellation::fill_path(&contour);
                    cache.stroke = super::super::tessellation::stroke_path(
                        &contour,
                        crate::design::BORDER_WIDTH,
                    );
                    cache.key = Some((clip.path.clone(), pose));
                }
                if let Some(path) = &cache.fill {
                    paint_colored_at(
                        window,
                        path,
                        origin,
                        color
                            .map(|v| rgb(v).into())
                            .unwrap_or_else(|| window.text_style().color),
                    );
                }
                if let (Some(color), Some(path)) = (stroke, &cache.stroke) {
                    paint_at(window, path, origin, color);
                }
            },
        )
        .absolute()
        .inset_0()
        .size_full()
        .into_any_element()
    }

    /// Keep the complete label/icon layout. Only its ink band is clipped; the
    /// viewport does not change available text width or request an ellipsis.
    pub fn content(&self, content: impl IntoElement, ink_height: f32) -> impl IntoElement {
        ClippedContent {
            child: content.into_any_element(),
            clip: ClipSource::Fixed(self.clone()),
            ink_height,
            vertical_insets: None,
        }
    }

    fn ink_bounds(&self, bounds: Bounds<Pixels>, height: f32) -> Bounds<Pixels> {
        let origin = self.paint.0.borrow().bounds.origin;
        let left = (bounds.origin.x - origin.x).as_f32() as f64;
        let right = left + bounds.size.width.as_f32() as f64;
        let height = height.min(bounds.size.height.as_f32()) as f64;
        let top = (bounds.origin.y - origin.y).as_f32() as f64
            + (bounds.size.height.as_f32() as f64 - height) / 2.;
        let key = [left as f32, top as f32, right as f32, height as f32].map(f32::to_bits);
        {
            let cache = self.paint.0.borrow();
            let ink = cache.clip_ink.borrow();
            if ink
                .path
                .as_ref()
                .is_some_and(|path| Rc::ptr_eq(path, &self.path))
            {
                if let Some(mask) = ink.values.get(&key) {
                    return Bounds::new(mask.origin + origin, mask.size);
                }
            }
        }
        let mut safe_left = left;
        let mut safe_right = right;
        let mut first = None;
        let mut last = top;
        // Sweep only edges crossing the current ink row. Most of a rounded
        // control's contour cannot intersect a given row of its text.
        let mut edges: Vec<_> = self
            .polygons
            .iter()
            .flat_map(|polygon| {
                polygon
                    .iter()
                    .zip(polygon.iter().cycle().skip(1))
                    .take(polygon.len())
                    .filter(|(a, b)| a[1] != b[1])
                    .map(|(&a, &b)| (a, b, a[1].min(b[1]), a[1].max(b[1])))
            })
            .collect();
        edges.sort_by(|a, b| a.2.total_cmp(&b.2));
        let mut active: Vec<usize> = Vec::with_capacity(4);
        let mut xs = Vec::with_capacity(4);
        let mut next = 0;
        // Trace fitting can oscillate by fractions of a pixel along an otherwise
        // straight edge. Sample inside the ink band, not on those near-tangent
        // edge vertices, where the even/odd ray has multiple tiny intervals.
        let edge = (height / 2.).min(0.25);
        for i in 0..=((height - 2. * edge).max(0.) * 2.).ceil() as usize {
            let y = top + edge + (i as f64 * 0.5).min((height - 2. * edge).max(0.));
            active.retain(|&index| edges[index].3 > y);
            while next < edges.len() && edges[next].2 <= y {
                if edges[next].3 > y {
                    active.push(next);
                }
                next += 1;
            }
            xs.clear();
            for &index in &active {
                let (a, b, _, _) = edges[index];
                xs.push(a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]));
            }
            xs.sort_by(f64::total_cmp);
            let interval = xs
                .chunks_exact(2)
                .map(|v| (v[0].max(left), v[1].min(right)))
                .filter(|(a, b)| b > a)
                .max_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0)));
            let Some((a, b)) = interval else {
                if first.is_some() {
                    break;
                } else {
                    continue;
                }
            };
            first.get_or_insert(y);
            last = y;
            safe_left = safe_left.max(a);
            safe_right = safe_right.min(b);
        }
        // Interior sampling avoids tangent ambiguity; it must not shrink a
        // fully covered band's vertical extent by the sampling offset.
        let mask_top = first.map_or(top, |y| if y <= top + edge { top } else { y });
        let mask_bottom = if first.is_some() && last >= top + height - edge {
            top + height
        } else {
            last
        };
        let mask = Bounds::new(
            point(px(safe_left as f32), px(mask_top as f32)),
            size(
                px((safe_right - safe_left).max(0.) as f32),
                px((mask_bottom - mask_top).max(0.) as f32),
            ),
        );
        let cache = self.paint.0.borrow();
        let mut ink = cache.clip_ink.borrow_mut();
        if ink
            .path
            .as_ref()
            .is_none_or(|path| !Rc::ptr_eq(path, &self.path))
            || ink.values.len() >= 64
        {
            ink.path = Some(self.path.clone());
            ink.values.clear();
        }
        ink.values.insert(key, mask);
        Bounds::new(mask.origin + origin, mask.size)
    }
}

struct ClippedContent {
    child: AnyElement,
    clip: ClipSource,
    ink_height: f32,
    vertical_insets: Option<[f32; 2]>,
}
impl IntoElement for ClippedContent {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for ClippedContent {
    type RequestLayoutState = ();
    type PrepaintState = ContentMask<Pixels>;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> ContentMask<Pixels> {
        let clip = match &self.clip {
            ClipSource::Layout => None,
            ClipSource::Fixed(clip) => Some(clip.clone()),
            ClipSource::Bound(binding) => Some(
                binding
                    .0
                    .borrow()
                    .as_ref()
                    .expect("bind the measured content before prepaint")
                    .clone(),
            ),
        };
        let region = if let Some([top, bottom]) = self.vertical_insets {
            Bounds::new(
                bounds.origin + point(px(0.), px(top)),
                size(
                    bounds.size.width,
                    (bounds.size.height - px(top + bottom)).max(px(0.)),
                ),
            )
        } else {
            bounds
        };
        let mask = ContentMask {
            bounds: clip.map_or(region, |clip| clip.ink_bounds(region, self.ink_height)),
        };
        window.with_content_mask(Some(mask), |window| {
            self.child.prepaint(window, cx);
        });
        mask
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        mask: &mut ContentMask<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(*mask), |window| self.child.paint(window, cx));
    }
}

pub(super) fn flatten(curves: &[super::super::Cubic]) -> Vec<Point> {
    fn curve(c: super::super::Cubic, depth: u8, out: &mut Vec<Point>) {
        let cross = |p: Point| {
            ((c.to[0] - c.from[0]) * (p[1] - c.from[1])
                - (c.to[1] - c.from[1]) * (p[0] - c.from[0]))
                .abs()
        };
        let length = ((c.to[0] - c.from[0]).powi(2) + (c.to[1] - c.from[1]).powi(2)).sqrt();
        let tolerance = super::super::tessellation::TOLERANCE as f64;
        // A capsule's collapsed straight edge is a point, not a curved loop.
        // Coincident endpoints with distant handles still need subdivision.
        let point_like = [c.c1, c.c2, c.to].into_iter().all(|p| {
            (p[0] - c.from[0]).powi(2) + (p[1] - c.from[1]).powi(2) <= tolerance * tolerance
        });
        if depth == 0
            || point_like
            || (length > 0. && cross(c.c1).max(cross(c.c2)) <= length * tolerance)
        {
            if out.last() != Some(&c.to) {
                out.push(c.to);
            }
            return;
        }
        let mid = |a: Point, b: Point| [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.];
        let a = mid(c.from, c.c1);
        let b = mid(c.c1, c.c2);
        let d = mid(c.c2, c.to);
        let e = mid(a, b);
        let f = mid(b, d);
        let g = mid(e, f);
        curve(
            super::super::Cubic {
                from: c.from,
                c1: a,
                c2: e,
                to: g,
            },
            depth - 1,
            out,
        );
        curve(
            super::super::Cubic {
                from: g,
                c1: f,
                c2: d,
                to: c.to,
            },
            depth - 1,
            out,
        );
    }
    let mut points = Vec::new();
    if let Some(first) = curves.first() {
        points.push(first.from);
    }
    for c in curves {
        curve(*c, 8, &mut points);
    }
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points
}
fn intersect(mut points: Vec<Point>, region: &[Point]) -> Vec<Point> {
    let cross = |a: Point, b: Point, p: Point| {
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
    };
    let orientation = region
        .iter()
        .zip(region.iter().cycle().skip(1))
        .take(region.len())
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f64>()
        .signum();
    let mut output = Vec::with_capacity(points.len() + region.len());
    for (&a, &b) in region
        .iter()
        .zip(region.iter().cycle().skip(1))
        .take(region.len())
    {
        let Some(mut prior) = points.last().copied() else {
            break;
        };
        output.clear();
        for &p in &points {
            let x = cross(a, b, prior) * orientation;
            let y = cross(a, b, p) * orientation;
            if (x >= 0.) != (y >= 0.) {
                let t = x / (x - y);
                output.push([
                    prior[0] + (p[0] - prior[0]) * t,
                    prior[1] + (p[1] - prior[1]) * t,
                ]);
            }
            if y >= 0. {
                output.push(p);
            }
            prior = p;
        }
        std::mem::swap(&mut points, &mut output);
    }
    points
}

#[cfg(test)]
mod tests {
    use super::{flatten, intersect, rounded_rectangle};

    #[test]
    fn shared_static_geometry_keeps_independent_placement_and_bounded_retention() {
        use super::super::{StaticSurface, STATIC_GEOMETRY};
        use gpui::{point, px, size, Bounds};
        use std::{rc::Rc, sync::Arc};
        let pose = super::super::super::Pose::rect(0., 0., 96., 32., 16.);
        let first = StaticSurface::new(pose, 0.6);
        let second = StaticSurface::new(pose, 0.6);
        assert!(Rc::ptr_eq(&first.contour, &second.contour));
        assert!(!Rc::ptr_eq(&first.paint.0, &second.paint.0));
        let a = point(px(24.), px(40.));
        let b = point(px(470.), px(610.));
        first.paint.0.borrow_mut().bounds = Bounds::new(a, size(px(96.), px(32.)));
        second.paint.0.borrow_mut().bounds = Bounds::new(b, size(px(96.), px(32.)));
        let first_clip = first.content_clip();
        let second_clip = second.content_clip();
        assert!(Arc::ptr_eq(&first_clip.polygons, &second_clip.polygons));
        let ink = |origin| Bounds::new(origin + point(px(12.), px(8.)), size(px(72.), px(16.)));
        let first_mask = first_clip.ink_bounds(ink(a), 16.);
        let second_mask = second_clip.ink_bounds(ink(b), 16.);
        assert_eq!(first_mask.size, second_mask.size);
        assert_eq!(second_mask.origin - first_mask.origin, b - a);
        for i in 0..80 {
            StaticSurface::new(
                super::super::super::Pose::rect(0., 0., 100. + i as f64, 32., 16.),
                0.6,
            );
        }
        STATIC_GEOMETRY.with_borrow(|cache| assert!(cache.len() <= 64));
        assert_eq!(
            first_clip.ink_bounds(ink(a), 16.),
            first_mask,
            "eviction invalidated geometry still held by a control"
        );
    }

    #[test]
    fn rounded_feedback_intersects_the_parent_curve() {
        let parent = flatten(&rounded_rectangle(
            super::super::super::Pose::rect(18., 64., 122., 152., 18.),
            0.6,
        ));
        let region = flatten(&rounded_rectangle(
            super::super::super::Pose::rect(28., 74., 102., 32., 10.),
            0.6,
        ));
        let polygon = intersect(parent, &region);
        assert!(polygon.len() > 4, "{polygon:?}");
        let area = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(polygon.len())
            .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
            .sum::<f64>()
            .abs()
            / 2.;
        assert!(area > 2800. && area < 3264., "{area}: {polygon:?}");
    }

    #[test]
    fn collapsed_capsule_edges_stay_compact_without_losing_closed_curves() {
        let curves = rounded_rectangle(super::super::super::Pose::rect(0., 0., 64., 32., 16.), 0.6);
        let pill = flatten(&curves);
        let redundant = curves
            .iter()
            .flat_map(|curve| {
                [
                    *curve,
                    super::super::super::Cubic {
                        from: curve.to,
                        c1: curve.to,
                        c2: curve.to,
                        to: curve.to,
                    },
                ]
            })
            .collect::<Vec<_>>();
        assert_eq!(
            pill,
            flatten(&redundant),
            "point segments expanded the clipping polygon"
        );
        assert!(pill.windows(2).all(|points| points[0] != points[1]));
        let looped = flatten(&[super::super::super::Cubic {
            from: [0., 0.],
            c1: [40., 0.],
            c2: [0., 40.],
            to: [0., 0.],
        }]);
        let area = looped
            .iter()
            .zip(looped.iter().cycle().skip(1))
            .take(looped.len())
            .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
            .sum::<f64>()
            .abs()
            / 2.;
        assert!(
            area > 200.,
            "coincident endpoints erased a real curve: {area}"
        );
    }
}

/// Paint a fixed-layout subtree through the current contour. Scaling and alpha
/// belong to the content transition, not to the material's width or height.
struct TransformedContent {
    id: ElementId,
    child: AnyElement,
    clip: ContentClip,
    scale: f32,
    opacity: f32,
    snapshot: Rc<RefCell<Option<PaintSnapshot>>>,
}
impl IntoElement for TransformedContent {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl ContentClip {
    pub(crate) fn transformed(
        &self,
        content: impl IntoElement,
        scale: f32,
        opacity: f32,
        id: impl Into<ElementId>,
        snapshot: Rc<RefCell<Option<PaintSnapshot>>>,
    ) -> impl IntoElement {
        TransformedContent {
            id: id.into(),
            child: content.into_any_element(),
            clip: self.clone(),
            scale,
            opacity,
            snapshot,
        }
    }
    fn raster_bands(&self, bounds: Bounds<Pixels>, device_scale: f32) -> Vec<Bounds<Pixels>> {
        raster_bands(
            &self.polygons,
            self.paint.0.borrow().bounds.origin,
            bounds,
            device_scale,
        )
    }
}

fn raster_bands(
    polygons: &Polygons,
    origin: gpui::Point<Pixels>,
    bounds: Bounds<Pixels>,
    device_scale: f32,
) -> Vec<Bounds<Pixels>> {
    struct Edge {
        a: Point,
        b: Point,
        low: f64,
        high: f64,
    }
    let mut edges = Vec::new();
    for polygon in polygons {
        for (&a, &b) in polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .take(polygon.len())
        {
            if a[1] != b[1] {
                edges.push(Edge {
                    a,
                    b,
                    low: a[1].min(b[1]),
                    high: a[1].max(b[1]),
                });
            }
        }
    }
    edges.sort_by(|a, b| a.low.total_cmp(&b.low));
    let step = 1. / device_scale as f64;
    let top = (bounds.top().as_f32() as f64 / step).floor() * step;
    let bottom = bounds.bottom().as_f32() as f64;
    let mut bands = Vec::with_capacity(((bottom - top) / step).max(0.).ceil() as usize);
    let mut active: Vec<usize> = Vec::with_capacity(4);
    let mut xs = Vec::with_capacity(4);
    let mut next = 0;
    let mut y = top;
    while y < bottom {
        let sample = y + step * 0.5 - origin.y.as_f32() as f64;
        active.retain(|&index| edges[index].high > sample);
        while next < edges.len() && edges[next].low <= sample {
            if edges[next].high > sample {
                active.push(next);
            }
            next += 1;
        }
        xs.clear();
        for &index in &active {
            let Edge { a, b, .. } = edges[index];
            // Evaluate from the original endpoints, avoiding accumulated slope
            // error and retaining exactly the previous pixel intersections.
            xs.push(
                a[0] + (sample - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) + origin.x.as_f32() as f64,
            );
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks_exact(2) {
            let left = pair[0].max(bounds.left().as_f32() as f64);
            let right = pair[1].min(bounds.right().as_f32() as f64);
            if right > left {
                bands.push(Bounds::new(
                    point(px(left as f32), px(y as f32)),
                    size(px((right - left) as f32), px(step as f32)),
                ));
            }
        }
        y += step;
    }
    bands
}

impl Element for TransformedContent {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if window.gpu_mask_layers_enabled() {
            let (origin, mask) = {
                let cache = self.clip.paint.0.borrow();
                let fill = cache.geometry.borrow_mut().fill(&self.clip.path);
                (cache.bounds.origin, fill)
            };
            if let Some(mut mask) = mask {
                mask.bounds.origin += origin;
                for vertex in mask.vertices_mut() {
                    vertex.xy_position += origin;
                }
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                "retained-liquid-content".hash(&mut hasher);
                global_id.hash(&mut hasher);
                let polygons = self.clip.polygons.clone();
                let factor = window.scale_factor();
                let key = hasher.finish();
                if window
                    .with_masked_retained_paint(
                        key,
                        bounds,
                        self.scale,
                        self.opacity,
                        &mask,
                        move || raster_bands(&polygons, origin, bounds, factor),
                        |window| self.child.paint(window, cx),
                    )
                    .is_some()
                {
                    *self.snapshot.borrow_mut() = window.retained_paint_snapshot(key);
                    return;
                }
            }
        }
        let bands = self.clip.raster_bands(bounds, window.scale_factor());
        use std::hash::{Hash, Hasher};
        let mut key = std::collections::hash_map::DefaultHasher::new();
        "liquid-fallback-content".hash(&mut key);
        global_id.hash(&mut key);
        let previous = self.snapshot.borrow().clone();
        let (_, snapshot) =
            window.capture_paint_snapshot(key.finish(), bounds, previous.as_ref(), |window| {
                self.child.paint(window, cx)
            });
        *self.snapshot.borrow_mut() = Some(snapshot.clone());
        window.with_scaled_alpha_paint_clip(
            bounds.center(),
            self.scale,
            self.opacity,
            &bands,
            |window| window.paint_snapshot(&snapshot),
        );
    }
}

impl ContentClip {
    pub(crate) fn paint_snapshot_at(
        &self,
        snapshot: &PaintSnapshot,
        scale: f32,
        opacity: f32,
        offset: gpui::Point<Pixels>,
        window: &mut Window,
    ) -> bool {
        let factor = window.scale_factor();
        let bounds = snapshot.bounds().map(|value| px(value.0 / factor));
        let (origin, mask) = {
            let cache = self.paint.0.borrow();
            let mask = cache.geometry.borrow_mut().fill(&self.path);
            (cache.bounds.origin, mask)
        };
        let Some(mut mask) = mask else {
            return false;
        };
        mask.bounds.origin += origin;
        for vertex in mask.vertices_mut() {
            vertex.xy_position += origin;
        }
        let polygons = self.polygons.clone();
        let destination = Bounds::new(bounds.origin + offset, bounds.size);
        if window.paint_masked_snapshot_at(
            snapshot,
            bounds,
            scale,
            opacity,
            &mask,
            move || raster_bands(&polygons, origin, destination, factor),
            offset,
        ) {
            return true;
        }
        let bands = raster_bands(&self.polygons, origin, destination, factor);
        window.with_scaled_alpha_paint_clip(
            destination.center(),
            scale,
            opacity,
            &bands,
            |window| window.paint_snapshot_at(snapshot, offset),
        );
        true
    }
}

#[cfg(test)]
mod scanline_tests {
    use super::{flatten, raster_bands, rounded_rectangle, Polygons};
    use gpui::{point, px, size, Bounds};
    use zork_liquid::Pose;

    #[test]
    fn active_edges_match_full_scan_pixel_for_pixel() {
        let cases: Vec<Polygons> = vec![
            vec![],
            vec![
                flatten(&rounded_rectangle(Pose::rect(10., 15., 64., 32., 16.), 0.6)),
                flatten(&rounded_rectangle(
                    Pose::rect(30., 70., 180., 240., 32.),
                    0.6,
                )),
            ],
            vec![vec![
                [0., 0.],
                [200., 0.],
                [200., 40.],
                [40., 40.],
                [40., 200.],
                [0., 200.],
            ]],
            vec![
                vec![[0., 0.], [200., 0.], [200., 200.], [0., 200.]],
                vec![[40., 40.], [40., 160.], [160., 160.], [160., 40.]],
            ],
        ];
        let bounds = Bounds::new(point(px(-20.), px(-20.)), size(px(330.), px(440.)));
        for polygons in cases {
            for dpr in [1., 1.25, 2., 3.] {
                let origin = point(px(2.25), px(-7.5));
                let step = 1. / dpr as f64;
                let mut y = (bounds.top().as_f32() as f64 / step).floor() * step;
                let mut expected = vec![];
                while y < bounds.bottom().as_f32() as f64 {
                    let sample = y + step * 0.5 - origin.y.as_f32() as f64;
                    let mut xs = vec![];
                    for polygon in &polygons {
                        for (a, b) in polygon
                            .iter()
                            .zip(polygon.iter().cycle().skip(1))
                            .take(polygon.len())
                        {
                            if (a[1] <= sample && b[1] > sample)
                                || (b[1] <= sample && a[1] > sample)
                            {
                                xs.push(
                                    a[0] + (sample - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
                                        + origin.x.as_f32() as f64,
                                );
                            }
                        }
                    }
                    xs.sort_by(f64::total_cmp);
                    for pair in xs.chunks_exact(2) {
                        let left = pair[0].max(bounds.left().as_f32() as f64);
                        let right = pair[1].min(bounds.right().as_f32() as f64);
                        if right > left {
                            expected.push(Bounds::new(
                                point(px(left as f32), px(y as f32)),
                                size(px((right - left) as f32), px(step as f32)),
                            ));
                        }
                    }
                    y += step;
                }
                assert_eq!(
                    raster_bands(&polygons, origin, bounds, dpr),
                    expected,
                    "DPR={dpr}"
                );
            }
        }
    }
}

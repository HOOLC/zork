//! GPUI adapter for the same material path on native and Web.
//! Opaque skins can cover their exterior in the supplied parent color. Floating
//! content derives masks and feedback intersections from the live contour;
//! pointer capture uses that same path. No DOM or GPU readback is involved.
use super::{rounded_rectangle, Contour, ContourError, Pose, Simulation};
use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc};

#[path = "clipping.rs"]
mod clipping;
pub use clipping::{ContentClip, ContentClipBinding};
mod ownership;
mod presentation;
pub(crate) use ownership::MaterialPart;
pub use ownership::SourceMaterial;

/// A read-only projection of the body's actual material, never a layer
/// partition or an independently advanced simulation.
#[derive(Default)]
pub(super) struct BodyDrawing {
    parent: Option<Rc<Contour>>,
    surface: Option<Surface>,
}
impl BodyDrawing {
    pub fn paint(
        &mut self,
        parent: &Surface,
        bounds: Bounds<Pixels>,
        fill: Option<u32>,
        stroke: Option<u32>,
        window: &mut Window,
    ) {
        let contour = parent.contour();
        if self
            .parent
            .as_ref()
            .is_none_or(|old| !Rc::ptr_eq(old, &contour))
        {
            let projection = parent.simulation.group_snapshot(1);
            if let Some(surface) = &mut self.surface {
                surface.simulation = projection;
                surface.prepare();
            } else {
                self.surface = Surface::new(projection).ok();
            }
            self.parent = Some(contour);
        }
        if let Some(surface) = &self.surface {
            surface.paint_geometry(bounds, fill, stroke, window);
        }
    }
}

pub struct Surface {
    model: zork_liquid::Surface,
    live_contour: Rc<RefCell<Rc<Contour>>>,
    paint: PaintHandle,
    owners: [PaintHandle; 2],
}
impl std::ops::Deref for Surface {
    type Target = zork_liquid::Surface;
    fn deref(&self) -> &Self::Target {
        &self.model
    }
}
impl std::ops::DerefMut for Surface {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.model
    }
}
impl Surface {
    pub fn new(simulation: Simulation) -> Result<Self, ContourError> {
        let geometry = Rc::new(RefCell::new(MaterialGeometry::default()));
        let paint = || {
            PaintHandle(Rc::new(RefCell::new(PaintCache {
                geometry: geometry.clone(),
                ..Default::default()
            })))
        };
        let model = zork_liquid::Surface::new(simulation)?;
        let live_contour = Rc::new(RefCell::new(model.contour()));
        Ok(Self {
            model,
            live_contour,
            paint: paint(),
            owners: [paint(), paint()],
        })
    }
    pub fn prepare(&mut self) -> bool {
        let changed = self.model.prepare();
        *self.live_contour.borrow_mut() = self.model.contour();
        changed
    }
    pub fn set_cutouts(&mut self, cutouts: Vec<Vec<super::Cubic>>) {
        self.model.set_cutouts(cutouts);
        *self.live_contour.borrow_mut() = self.model.contour();
    }
    pub fn set_border_width(&self, width: f32) {
        assert!(width.is_finite() && width >= 0.);
        for paint in [&self.paint, &self.owners[0], &self.owners[1]] {
            let mut cache = paint.0.borrow_mut();
            if cache.border_width != Some(width) {
                cache.border_width = Some(width);
                cache.key = None;
            }
        }
    }
    pub fn paint_failed(&self) -> bool {
        self.paint.0.borrow().failed || self.owners.iter().any(|paint| paint.0.borrow().failed)
    }
    pub fn visible(&self) -> bool {
        self.paint.0.borrow().visible
    }
    /// Apply the material hit region to an interactive descendant. Keeping this
    /// on controls avoids intercepting sibling controls outside the surface.
    pub fn guard(&self, element: Stateful<Div>) -> Stateful<Div> {
        let path = self.live_contour.clone();
        let paint = self.paint.clone();
        element.capture_any_mouse_down(move |event, _, cx| {
            let origin = paint.0.borrow().bounds.origin;
            if !path.borrow().contains([
                (event.position.x - origin.x).as_f32() as f64,
                (event.position.y - origin.y).as_f32() as f64,
            ]) {
                cx.stop_propagation();
            }
        })
    }

    pub(crate) fn paint_geometry(
        &self,
        bounds: Bounds<Pixels>,
        fill: Option<u32>,
        stroke: Option<u32>,
        window: &mut Window,
    ) {
        let path = self.model.contour();
        let mut cache = self.paint.0.borrow_mut();
        cache.visible = visible(bounds, window);
        cache.prepare(&path, bounds);
        if let (Some(path), Some(color)) = (&cache.fill, fill) {
            paint_at(window, path, bounds.origin, color);
        }
        if let (Some(path), Some(color)) = (&cache.stroke, stroke) {
            paint_at(window, path, bounds.origin, color);
        }
    }
    pub fn accepts_click(&self, event: &ClickEvent) -> bool {
        if matches!(event, ClickEvent::Keyboard(_)) {
            return true;
        }
        let origin = self.paint.0.borrow().bounds.origin;
        let position = event.position();
        self.model.contour().contains([
            (position.x - origin.x).as_f32() as f64,
            (position.y - origin.y).as_f32() as f64,
        ])
    }
    pub fn layer(
        &self,
        id: impl Into<ElementId>,
        width: f32,
        height: f32,
        colors: SurfaceColors,
        content: impl IntoElement,
    ) -> Stateful<Div> {
        layer(
            id,
            self.model.contour(),
            self.paint.clone(),
            width,
            height,
            colors,
            content,
        )
    }
    pub fn background(&self, fill: u32, stroke: Option<u32>) -> impl IntoElement {
        self.background_at(fill, stroke, point(px(0.), px(0.)))
    }
    /// Position a floating material without changing its local geometry or
    /// invalidating contour/tessellation caches when only its anchor scrolls.
    pub fn background_at(
        &self,
        fill: u32,
        stroke: Option<u32>,
        offset: gpui::Point<Pixels>,
    ) -> impl IntoElement {
        self.background_colors(Some(fill), stroke, offset, false)
    }
    pub(crate) fn background_colors(
        &self,
        fill: Option<u32>,
        stroke: Option<u32>,
        offset: Point<Pixels>,
        focused: bool,
    ) -> impl IntoElement {
        background(
            self.model.contour(),
            self.paint.clone(),
            fill,
            stroke,
            offset,
            focused,
        )
    }
    /// A conservative content viewport inside the live contour. Floating
    /// surfaces cannot paint an opaque exterior over the underlying page. This
    /// bounds their children to a rectangle proven not to cross any contour
    /// curve, while keeping the material's exterior transparent.
    pub fn inset_content(&self, pose: Pose, padding: f64, content: impl IntoElement) -> Div {
        let Some(inset) = content_inset(&self.model.contour(), pose, padding) else {
            return div();
        };
        div()
            .absolute()
            .left(px(inset as f32))
            .top(px(inset as f32))
            .w(px((pose.w - 2. * inset) as f32))
            .h(px((pose.h - 2. * inset) as f32))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left(px(-inset as f32))
                    .top(px(-inset as f32))
                    .w(px(pose.w as f32))
                    .h(px(pose.h as f32))
                    .child(content),
            )
    }
    /// The body is the foreground of a paired material. Its own fill covers
    /// source ink where the two overlap; the shared contour bounds that fill.
    /// Callers paint the carrier first and mount this layer afterwards.
    pub(super) fn content_layer(
        &self,
        id: impl Into<ElementId>,
        pose: Pose,
        fill: u32,
        alpha: f32,
        content: impl IntoElement,
        window: &mut Window,
        cx: &mut App,
    ) -> Div {
        div()
            .absolute()
            .left(px(pose.left() as f32))
            .top(px(pose.top() as f32))
            .w(px(pose.w as f32))
            .h(px(pose.h as f32))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(alpha)
                    .child(
                        self.content_clip()
                            .fill(id, pose.r, Some(fill), None, window, cx),
                    ),
            )
            .child(div().size_full().opacity(alpha).child(content))
    }
    /// Reveal the material using the same spring as HoverFill. Content remains
    /// visible while the source is quiet, and always clips to the current contour.
    pub fn layer_revealed(
        &self,
        id: impl Into<ElementId>,
        width: f32,
        height: f32,
        colors: SurfaceColors,
        content: impl IntoElement,
        revealed: bool,
    ) -> Stateful<Div> {
        let id = id.into();
        let key: ElementId = format!("{id:?}-reveal").into();
        let path = self.model.contour();
        let paint = self.paint.clone();
        div()
            .id(id)
            .relative()
            .w(px(width))
            .h(px(height))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .size_full()
                    .child(self.background(colors.fill, colors.border))
                    .with_spring(
                        key,
                        crate::components::motion::spring(if revealed { 1. } else { 0. }),
                        |v, opacity| v.opacity(opacity),
                    ),
            )
            .child(content)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let mut cache = paint.0.borrow_mut();
                        cache.prepare(&path, bounds);
                        cache.prepare_exterior(&path, bounds);
                        if !cache.visible {
                            return;
                        }
                        if let Some(exterior) = &cache.exterior {
                            paint_at(window, exterior, bounds.origin, colors.parent);
                        }
                        if colors.focused && window.last_input_was_keyboard() {
                            cache.prepare_focus(&path);
                            if let Some(focus) = &cache.focus {
                                paint_at(
                                    window,
                                    focus,
                                    bounds.origin,
                                    crate::design::INTERACTION.focus_border,
                                );
                            }
                        }
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
    }
}

fn background(
    path: Rc<Contour>,
    paint: PaintHandle,
    fill: Option<u32>,
    stroke: Option<u32>,
    offset: Point<Pixels>,
    focused: bool,
) -> impl IntoElement {
    let placement = paint.clone();
    canvas(
        move |bounds, _, _| {
            placement.0.borrow_mut().bounds = Bounds::new(bounds.origin + offset, bounds.size)
        },
        move |bounds, _, window, _| {
            let mut cache = paint.0.borrow_mut();
            let placed = Bounds::new(bounds.origin + offset, bounds.size);
            cache.visible = visible(bounds, window);
            if !cache.visible {
                return;
            }
            if fill.is_none() && stroke.is_none() && !focused {
                return;
            }
            cache.prepare(&path, placed);
            if let (Some(path), Some(fill)) = (&cache.fill, fill) {
                paint_at(window, path, placed.origin, fill);
            }
            if focused && window.last_input_was_keyboard() {
                cache.prepare_focus(&path);
                if let Some(focus) = &cache.focus {
                    paint_at(
                        window,
                        focus,
                        placed.origin,
                        crate::design::INTERACTION.focus_border,
                    );
                }
            } else if let (Some(path), Some(stroke)) = (&cache.stroke, stroke) {
                paint_at(window, path, placed.origin, stroke);
            }
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

fn content_inset(contour: &Contour, pose: Pose, padding: f64) -> Option<f64> {
    let maximum = pose.short() / 2. - 1.;
    if maximum <= 0. || !contour.contains([pose.cx, pose.cy]) {
        return None;
    }
    let safe = |inset: f64| {
        let rect = [
            pose.left() + inset,
            pose.top() + inset,
            pose.left() + pose.w - inset,
            pose.top() + pose.h - inset,
        ];
        !contour
            .loops
            .iter()
            .flatten()
            .any(|curve| curve_intersects_rect(*curve, rect, 5))
    };
    let mut low = padding.min(maximum).max(0.);
    if safe(low) {
        return Some(low);
    }
    if !safe(maximum) {
        return None;
    }
    let mut high = maximum;
    for _ in 0..8 {
        let mid = (low + high) / 2.;
        if safe(mid) {
            high = mid;
        } else {
            low = mid;
        }
    }
    Some(high)
}

fn curve_intersects_rect(curve: super::Cubic, rect: [f64; 4], depth: usize) -> bool {
    let points = [curve.from, curve.c1, curve.c2, curve.to];
    let min = |axis| points.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min);
    let max = |axis| {
        points
            .iter()
            .map(|p| p[axis])
            .fold(f64::NEG_INFINITY, f64::max)
    };
    if max(0) < rect[0] || min(0) > rect[2] || max(1) < rect[1] || min(1) > rect[3] {
        return false;
    }
    // A cubic lies in its control hull. Subdivision tightens the bound; a
    // remaining overlap is conservatively rejected, never treated as a mask.
    if depth == 0 {
        return true;
    }
    let mid = |a: super::Point, b: super::Point| [(a[0] + b[0]) / 2., (a[1] + b[1]) / 2.];
    let a = mid(curve.from, curve.c1);
    let b = mid(curve.c1, curve.c2);
    let c = mid(curve.c2, curve.to);
    let d = mid(a, b);
    let e = mid(b, c);
    let f = mid(d, e);
    curve_intersects_rect(
        super::Cubic {
            from: curve.from,
            c1: a,
            c2: d,
            to: f,
        },
        rect,
        depth - 1,
    ) || curve_intersects_rect(
        super::Cubic {
            from: f,
            c1: e,
            c2: c,
            to: curve.to,
        },
        rect,
        depth - 1,
    )
}

#[cfg(test)]
mod floating_content_tests {
    use super::{content_inset, Pose, Simulation, Surface};

    #[test]
    fn content_stays_inside_the_live_material_during_reversal() {
        let source = Pose::rect(22., 18., 116., 32., 16.);
        let target = Pose::rect(18., 62., 420., 310., 32.);
        for pair in [false, true] {
            let options = super::super::Options {
                anchor: [0., 0.],
                capacity: target.w * target.h,
                ..Default::default()
            };
            let simulation = if pair {
                Simulation::pair(source, target, Default::default(), options)
            } else {
                Simulation::new(source, Default::default(), options)
            };
            let mut surface = Surface::new(simulation).unwrap();
            for frame in 0..180 {
                if [0, 24, 40, 100].contains(&frame) {
                    let open = matches!(frame, 0 | 40);
                    if pair {
                        surface.simulation.set_open(open);
                    } else {
                        surface
                            .simulation
                            .set_target(if open { target } else { source });
                    }
                }
                surface.simulation.advance(1. / 60., false);
                surface.prepare();
                let pose = surface.simulation.pose();
                if let Some(inset) = content_inset(&surface.contour(), pose, 10.) {
                    for x in 0..=8 {
                        for y in 0..=8 {
                            let point = [
                                pose.left() + inset + (pose.w - 2. * inset) * x as f64 / 8.,
                                pose.top() + inset + (pose.h - 2. * inset) * y as f64 / 8.,
                            ];
                            assert!(
                                surface.contour().contains(point),
                                "content escaped at {pair}/{frame}: {point:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn no_content_viewport_for_collapsed_geometry() {
        let pose = Pose::rect(0., 0., 2., 2., 1.);
        let surface = Surface::new(Simulation::new(
            pose,
            Default::default(),
            Default::default(),
        ))
        .unwrap();
        assert!(content_inset(&surface.contour(), pose, 10.).is_none());
    }
}

#[derive(Clone, Copy)]
pub struct SurfaceColors {
    pub fill: u32,
    pub border: Option<u32>,
    pub parent: u32,
    pub focused: bool,
}
impl SurfaceColors {
    pub fn plain(fill: u32, border: u32, parent: u32) -> Self {
        Self {
            fill,
            border: Some(border),
            parent,
            focused: false,
        }
    }
    /// Filled surfaces use their tone to establish the boundary.
    pub fn filled(fill: u32, parent: u32) -> Self {
        Self {
            fill,
            border: None,
            parent,
            focused: false,
        }
    }
    /// Outlined controls follow the surrounding surface's color.
    pub fn outlined(border: u32, parent: u32) -> Self {
        Self::plain(parent, border, parent)
    }
}
#[derive(Clone, Default)]
struct PaintHandle(Rc<RefCell<PaintCache>>);
#[derive(Default)]
struct PaintCache {
    partition: Option<ownership::Partition>,
    clip_polygons: Rc<RefCell<Option<(Rc<Contour>, std::sync::Arc<clipping::Polygons>)>>>,
    geometry: Rc<RefCell<MaterialGeometry>>,
    clip_ink: Rc<RefCell<clipping::InkCache>>,
    border_width: Option<f32>,
    // Retain the contour while its tessellation is cached. A bare address can
    // be reused after offscreen resize/variant changes and falsely hit an old path.
    key: Option<(Rc<Contour>, Size<Pixels>)>,
    fill: Option<Path<Pixels>>,
    stroke: Option<Path<Pixels>>,
    focus: Option<Path<Pixels>>,
    exterior: Option<Path<Pixels>>,
    bounds: Bounds<Pixels>,
    visible: bool,
    failed: bool,
}

#[derive(Default)]
struct MaterialGeometry {
    path: Option<Rc<Contour>>,
    fill: Option<Path<Pixels>>,
    stroke: Option<Path<Pixels>>,
    stroke_width: Option<f32>,
}
impl MaterialGeometry {
    fn fill(&mut self, path: &Rc<Contour>) -> Option<Path<Pixels>> {
        if self.path.as_ref().is_none_or(|old| !Rc::ptr_eq(old, path)) {
            let mut b = builder();
            append(&mut b, path);
            self.fill = b.build().ok();
            self.path = Some(path.clone());
            self.stroke = None;
            self.stroke_width = None;
        }
        self.fill.clone()
    }
    fn stroke(&mut self, path: &Rc<Contour>, width: f32) -> Option<Path<Pixels>> {
        if self.stroke_width != Some(width) {
            self.stroke = super::tessellation::stroke_path(path, width);
            self.stroke_width = Some(width);
        }
        self.stroke.clone()
    }
}

fn builder() -> PathBuilder {
    super::tessellation::fill_builder()
}
pub(super) fn append(builder: &mut PathBuilder, path: &Contour) {
    let point = |p: super::Point| gpui::point(px(p[0] as f32), px(p[1] as f32));
    for curves in &path.loops {
        if let Some(first) = curves.first() {
            builder.move_to(point(first.from));
            for c in curves {
                builder.cubic_bezier_to(point(c.to), point(c.c1), point(c.c2));
            }
            builder.close();
        }
    }
}
impl PaintCache {
    fn prepare(&mut self, path: &Rc<Contour>, bounds: Bounds<Pixels>) {
        self.bounds = bounds;
        if self
            .key
            .as_ref()
            .is_some_and(|(cached, size)| Rc::ptr_eq(cached, path) && *size == bounds.size)
        {
            return;
        }
        self.key = Some((path.clone(), bounds.size));
        let mut geometry = self.geometry.borrow_mut();
        self.fill = geometry.fill(path);
        self.stroke = geometry.stroke(
            path,
            self.border_width.unwrap_or(crate::design::BORDER_WIDTH),
        );
        drop(geometry);
        if let Some(partition) = self.partition {
            self.fill = self
                .fill
                .take()
                .map(|path| ownership::partition(path, partition));
            self.stroke = self
                .stroke
                .take()
                .map(|path| ownership::partition(path, partition));
        }
        self.focus = None;
        self.exterior = None;
        self.failed = self.fill.is_none() || self.stroke.is_none();
    }
    fn prepare_exterior(&mut self, path: &Contour, bounds: Bounds<Pixels>) {
        if self.exterior.is_some() {
            return;
        }
        let mut exterior = builder();
        exterior.move_to(point(px(-1.), px(-1.)));
        exterior.line_to(point(bounds.size.width + px(1.), px(-1.)));
        exterior.line_to(point(
            bounds.size.width + px(1.),
            bounds.size.height + px(1.),
        ));
        exterior.line_to(point(px(-1.), bounds.size.height + px(1.)));
        exterior.close();
        append(&mut exterior, path);
        self.exterior = exterior.build().ok();
        self.failed |= self.exterior.is_none();
    }
    fn prepare_focus(&mut self, path: &Contour) {
        if self.focus.is_none() {
            self.focus = super::tessellation::stroke_path(path, crate::design::BORDER_WIDTH);
            if let Some(partition) = self.partition {
                self.focus = self
                    .focus
                    .take()
                    .map(|path| ownership::partition(path, partition));
            }
        }
    }
}
fn visible(bounds: Bounds<Pixels>, window: &Window) -> bool {
    let clip = bounds
        .intersect(&window.content_mask().bounds)
        .intersect(&Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
    clip.size.width > px(0.) && clip.size.height > px(0.)
}
fn paint_at(window: &mut Window, path: &Path<Pixels>, origin: gpui::Point<Pixels>, color: u32) {
    paint_colored_at(window, path, origin, rgb(color));
}
fn paint_colored_at(
    window: &mut Window,
    path: &Path<Pixels>,
    origin: gpui::Point<Pixels>,
    color: impl Into<Background>,
) {
    window.paint_path_at(path, origin, color);
}
fn layer(
    id: impl Into<ElementId>,
    path: Rc<Contour>,
    paint: PaintHandle,
    width: f32,
    height: f32,
    colors: SurfaceColors,
    content: impl IntoElement,
) -> Stateful<Div> {
    let back = paint.clone();
    let front = paint.clone();
    let back_path = path.clone();
    let front_path = path.clone();
    div()
        .id(id)
        .relative()
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let mut cache = back.0.borrow_mut();
                    cache.bounds = bounds;
                    cache.visible = visible(bounds, window);
                    if cache.visible {
                        cache.prepare(&back_path, bounds);
                        if let Some(path) = &cache.fill {
                            paint_at(window, path, bounds.origin, colors.fill);
                        }
                    }
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
        .child(content)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let mut cache = front.0.borrow_mut();
                    if !cache.visible {
                        return;
                    }
                    let focus_visible = colors.focused && window.last_input_was_keyboard();
                    cache.prepare(&front_path, bounds);
                    cache.prepare_exterior(&front_path, bounds);
                    if focus_visible {
                        cache.prepare_focus(&front_path);
                    }
                    if let Some(path) = &cache.exterior {
                        paint_at(window, path, bounds.origin, colors.parent);
                    }
                    let path = if focus_visible {
                        &cache.focus
                    } else {
                        &cache.stroke
                    };
                    let stroke = if focus_visible {
                        Some(crate::design::INTERACTION.focus_border)
                    } else {
                        colors.border
                    };
                    if let (Some(path), Some(stroke)) = (path, stroke) {
                        paint_at(window, path, bounds.origin, stroke);
                    }
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
}

fn guard(element: Stateful<Div>, path: Rc<Contour>, paint: PaintHandle) -> Stateful<Div> {
    element.capture_any_mouse_down(move |event, _, cx| {
        let origin = paint.0.borrow().bounds.origin;
        if !path.contains([
            (event.position.x - origin.x).as_f32() as f64,
            (event.position.y - origin.y).as_f32() as f64,
        ]) {
            cx.stop_propagation();
        }
    })
}

pub(super) struct StaticSurface {
    pose: Pose,
    smoothing: f64,
    contour: Rc<Contour>,
    paint: PaintHandle,
}

struct StaticGeometry {
    pose: Pose,
    smoothing: f64,
    contour: Rc<Contour>,
    geometry: Rc<RefCell<MaterialGeometry>>,
    polygons: Rc<RefCell<Option<(Rc<Contour>, std::sync::Arc<clipping::Polygons>)>>>,
    ink: Rc<RefCell<clipping::InkCache>>,
}
thread_local! {
    static STATIC_GEOMETRY: RefCell<Vec<Rc<StaticGeometry>>> = const { RefCell::new(Vec::new()) };
}
impl StaticGeometry {
    fn get(pose: Pose, smoothing: f64) -> Rc<Self> {
        STATIC_GEOMETRY.with_borrow_mut(|cache| {
            let hit = cache
                .iter()
                .position(|entry| entry.pose == pose && entry.smoothing == smoothing);
            let entry = hit.map(|index| cache.remove(index)).unwrap_or_else(|| {
                Rc::new(Self {
                    pose,
                    smoothing,
                    contour: Rc::new(Contour {
                        loops: vec![rounded_rectangle(pose, smoothing)],
                        sampled_points: 0,
                        full_grid_points: 0,
                        used_fallback: false,
                        revision: 0,
                    }),
                    geometry: Default::default(),
                    polygons: Default::default(),
                    ink: Default::default(),
                })
            });
            // Bound both retained variants and corner subdivision. Oversized
            // surfaces keep their ordinary per-control cache.
            if pose.w <= 4096. && pose.h <= 4096. && pose.r <= 64. {
                cache.push(entry.clone());
                if cache.len() > 64 {
                    cache.remove(0);
                }
            }
            entry
        })
    }
}
impl StaticSurface {
    pub fn new(pose: Pose, smoothing: f64) -> Self {
        let shared = StaticGeometry::get(pose, smoothing);
        Self {
            pose,
            smoothing,
            contour: shared.contour.clone(),
            paint: PaintHandle(Rc::new(RefCell::new(PaintCache {
                geometry: shared.geometry.clone(),
                clip_polygons: shared.polygons.clone(),
                clip_ink: shared.ink.clone(),
                ..Default::default()
            }))),
        }
    }
    pub fn matches(&self, pose: Pose, smoothing: f64) -> bool {
        self.pose == pose && self.smoothing == smoothing
    }
    pub fn content_clip(&self) -> ContentClip {
        ContentClip::new(self.contour.clone(), self.paint.clone())
    }
    pub fn background(
        &self,
        fill: Option<u32>,
        border: Option<u32>,
        focused: bool,
    ) -> impl IntoElement {
        background(
            self.contour.clone(),
            self.paint.clone(),
            fill,
            border,
            point(px(0.), px(0.)),
            focused,
        )
    }
    pub fn guard(&self, element: Stateful<Div>) -> Stateful<Div> {
        guard(element, self.contour.clone(), self.paint.clone())
    }
}
/// Compact actions, fields, rows and tracks share the engine's corner geometry.
/// Interaction handlers belong to the caller; this wrapper owns only the shape.
pub fn skin(
    id: impl Into<ElementId>,
    width: f32,
    height: f32,
    radius: f32,
    smoothing: f64,
    colors: SurfaceColors,
    content: impl IntoElement,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id = id.into();
    let pose = Pose::rect(
        0.,
        0.,
        width.max(2.) as f64,
        height.max(2.) as f64,
        radius as f64,
    );
    let state = window.use_keyed_state(id.clone(), cx, |_, _| StaticSurface::new(pose, smoothing));
    state.update(cx, |state, _| {
        if state.pose != pose || state.smoothing != smoothing {
            *state = StaticSurface::new(pose, smoothing);
        }
    });
    let state = state.read(cx);
    guard(
        layer(
            id,
            state.contour.clone(),
            state.paint.clone(),
            width,
            height,
            colors,
            content,
        ),
        state.contour.clone(),
        state.paint.clone(),
    )
}

//! Reusable content presentation. Components own input, focus and placement;
//! this layer owns paint snapshots, independent reveal and paint-only playback.
use super::{
    motion::Motion,
    render::{BodyDrawing, MaterialPart, SourceMaterial},
    Material, Pose,
};
use gpui::{prelude::*, *};
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[cfg(target_family = "wasm")]
use web_time::Instant;

#[derive(Clone, Copy, Default)]
pub(super) struct ContentMotion {
    reveal: zork_liquid::motion::Reveal,
    last: Option<Instant>,
}
impl ContentMotion {
    pub fn advance(&mut self, open: bool, rate: f64, reduced: bool) -> bool {
        let now = Instant::now();
        let elapsed = self
            .last
            .replace(now)
            .map_or(0., |last| now.duration_since(last).as_secs_f64())
            * rate;
        let moving = self.reveal.advance(open, elapsed, reduced);
        if !moving {
            self.last = None;
        }
        moving
    }
    pub fn opacity(&self) -> f32 {
        self.reveal.opacity()
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct BackdropMotion {
    reveal: zork_liquid::motion::DelayedReveal,
    last: Option<Instant>,
}
impl BackdropMotion {
    fn advance(&mut self, open: bool, ready: bool, rate: f64, reduced: bool) -> bool {
        let now = Instant::now();
        let elapsed = self
            .last
            .replace(now)
            .map_or(0., |last| now.duration_since(last).as_secs_f64())
            * rate;
        let moving = self.reveal.advance(open, ready, elapsed, reduced);
        if !moving {
            self.last = None;
        }
        moving
    }
    pub fn opacity(&self) -> f32 {
        self.reveal.opacity()
    }
}

#[derive(Default)]
pub(super) struct Presentation {
    pub content: Rc<Cell<ContentMotion>>,
    pub backdrop: Rc<Cell<BackdropMotion>>,
    pub snapshot: Rc<RefCell<Option<PaintSnapshot>>>,
    pub initial: Rc<RefCell<Option<PaintSnapshot>>>,
    initial_frame: Rc<Cell<Option<u64>>>,
    pub region: Rc<RefCell<Option<PaintRegion>>>,
    epoch: Rc<Cell<u64>>,
    frames: Rc<Cell<u64>>,
}
impl Drop for Presentation {
    fn drop(&mut self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
    }
}
impl Presentation {
    pub fn release_presented_initial(&self, window: &Window) {
        release_presented_initial(&self.initial, &self.initial_frame, window);
    }
    pub fn begin(&self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
    }
    pub fn hold(&self) {
        let mut content = self.content.get();
        content.last = None;
        self.content.set(content);
        let mut backdrop = self.backdrop.get();
        backdrop.last = None;
        self.backdrop.set(backdrop);
    }
    pub fn transfer(
        &self,
        source: Option<&PaintRegion>,
        fallback: Bounds<Pixels>,
        window: &Window,
    ) {
        use std::hash::{Hash, Hasher};
        let mut key = std::collections::hash_map::DefaultHasher::new();
        "liquid-destination-picture".hash(&mut key);
        (Rc::as_ptr(&self.epoch) as usize).hash(&mut key);
        self.epoch.get().hash(&mut key);
        let region = self.region.borrow();
        self.initial_frame.set(None);
        *self.initial.borrow_mut() = region
            .as_ref()
            .and_then(|region| window.snapshot_paint_region(key.finish(), region))
            .or_else(|| {
                source.and_then(|region| window.snapshot_paint_region(key.finish(), region))
            })
            .or_else(|| Some(window.snapshot_presented(key.finish(), fallback)));
        self.hold();
    }
    pub fn record(&self, child: AnyElement) -> AnyElement {
        Playback {
            child,
            driver: None,
            initial: self.initial.clone(),
            initial_frame: self.initial_frame.clone(),
            region: self.region.clone(),
        }
        .into_any_element()
    }
    pub fn opacity(&self) -> f32 {
        self.content.get().opacity()
    }
    pub fn frames(&self) -> u64 {
        self.frames.get()
    }
    pub fn advance(&self, open: bool, cx: &App) -> bool {
        let mut content = self.content.get();
        let moving = content.advance(open, super::press::playback_rate(cx), cx.reduce_motion());
        self.content.set(content);
        moving
    }
    pub fn advance_backdrop(&self, open: bool, ready: bool, cx: &App) -> bool {
        let mut backdrop = self.backdrop.get();
        let moving = backdrop.advance(
            open,
            ready,
            super::press::playback_rate(cx),
            cx.reduce_motion(),
        );
        self.backdrop.set(backdrop);
        moving
    }
    pub fn playback(
        &self,
        child: AnyElement,
        paint: FramePaint,
        target: Target,
        notify: Rc<dyn Fn(&mut App)>,
    ) -> AnyElement {
        Playback {
            child,
            initial: self.initial.clone(),
            initial_frame: self.initial_frame.clone(),
            region: self.region.clone(),
            driver: Some(Rc::new(Driver {
                paint,
                target,
                snapshot: self.snapshot.clone(),
                initial: self.initial.clone(),
                initial_frame: self.initial_frame.clone(),
                epoch: self.epoch.clone(),
                version: self.epoch.get(),
                frames: self.frames.clone(),
                notify,
            })),
        }
        .into_any_element()
    }
}

/// Values come from the complete component's visual recipe, never the driver.
#[derive(Clone, Copy)]
pub(super) struct Recipe {
    pub fill: u32,
    pub border: Option<u32>,
    pub backdrop: Option<u32>,
    pub initial_scale: f32,
    pub fade_material: bool,
}
impl Recipe {
    pub fn backdrop_alpha(self, opacity: f32) -> u32 {
        self.backdrop
            .map_or(0, |color| ((color & 0xff) as f32 * opacity) as u32)
    }
    pub fn scale(self, opacity: f32) -> f32 {
        self.initial_scale + (1. - self.initial_scale) * opacity
    }
}

#[derive(Clone, Copy)]
pub(super) struct Target {
    pub from: Pose,
    pub to: Pose,
    pub pair: bool,
    pub open: bool,
    pub material: Material,
}

/// The same ordered description is used during normal UI painting and playback.
#[derive(Clone)]
pub(super) struct FramePaint {
    pub motion: Rc<RefCell<Motion>>,
    pub content: Rc<Cell<ContentMotion>>,
    pub backdrop: Rc<Cell<BackdropMotion>>,
    pub source: Option<SourceMaterial>,
    pub body: Option<Rc<RefCell<BodyDrawing>>>,
    pub offset: Rc<Cell<Point<Pixels>>>,
    pub part: Option<MaterialPart>,
    pub recipe: Recipe,
}
impl FramePaint {
    fn background(&self, viewport: Bounds<Pixels>, window: &mut Window) {
        let motion = self.motion.borrow();
        let Some(surface) = &motion.surface else {
            return;
        };
        let bounds = Bounds::new(viewport.origin + self.offset.get(), viewport.size);
        let paint = |window: &mut Window| {
            if let Some(part) = &self.part {
                part.paint_geometry(bounds, Some(self.recipe.fill), None, window);
            } else {
                surface.paint_geometry(bounds, Some(self.recipe.fill), None, window);
            }
            if let Some(source) = &self.source {
                source.paint_ink(window);
            }
            let alpha = self.recipe.backdrop_alpha(self.backdrop.get().opacity());
            if alpha > 0 {
                surface.paint_geometry(bounds, None, self.recipe.border, window);
                window.paint_barrier(viewport);
                window.paint_quad(gpui::fill(
                    viewport,
                    rgba((self.recipe.backdrop.unwrap() & 0xffffff00) | alpha),
                ));
                window.paint_barrier(viewport);
            }
            if let Some(body) = &self.body {
                // The body is a whole material parcel. Its content surface
                // naturally covers source ink as content becomes visible.
                window.with_scaled_alpha_paint_clip(
                    point(px(0.), px(0.)),
                    1.,
                    self.content.get().opacity(),
                    &[viewport],
                    |window| {
                        body.borrow_mut().paint(
                            surface,
                            bounds,
                            Some(self.recipe.fill),
                            None,
                            window,
                        );
                    },
                );
            }
        };
        if self.recipe.fade_material {
            window.with_scaled_alpha_paint_clip(
                point(px(0.), px(0.)),
                1.,
                motion.progress() as f32,
                &[viewport],
                paint,
            );
        } else {
            paint(window);
        }
        window.paint_barrier(viewport);
    }
    fn border(&self, viewport: Bounds<Pixels>, window: &mut Window) {
        window.paint_barrier(viewport);
        let motion = self.motion.borrow();
        let Some(surface) = &motion.surface else {
            return;
        };
        let bounds = Bounds::new(viewport.origin + self.offset.get(), viewport.size);
        let paint = |window: &mut Window| {
            if let Some(body) = self
                .body
                .as_ref()
                .filter(|_| self.recipe.backdrop_alpha(self.backdrop.get().opacity()) > 0)
            {
                body.borrow_mut()
                    .paint(surface, bounds, None, self.recipe.border, window);
            } else if let Some(part) = &self.part {
                part.paint_geometry(bounds, None, self.recipe.border, window);
            } else {
                surface.paint_geometry(bounds, None, self.recipe.border, window);
            }
        };
        if self.recipe.fade_material {
            window.with_scaled_alpha_paint_clip(
                point(px(0.), px(0.)),
                1.,
                motion.progress() as f32,
                &[viewport],
                paint,
            );
        } else {
            paint(window);
        }
    }
    pub fn underlay(&self) -> AnyElement {
        let paint = self.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| paint.background(bounds, window),
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }
    pub fn outline(&self) -> AnyElement {
        let paint = self.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| paint.border(bounds, window),
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }
    pub fn replay(&self, snapshot: &PaintSnapshot, window: &mut Window) -> bool {
        let bounds = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let mut painted = false;
        window.paint_layer(bounds, |window| {
            self.background(bounds, window);
            let motion = self.motion.borrow();
            let Some(surface) = &motion.surface else {
                return;
            };
            let opacity = self.content.get().opacity();
            painted = surface.content_clip().paint_snapshot_at(
                snapshot,
                self.recipe.scale(opacity),
                opacity,
                self.offset.get(),
                window,
            );
            drop(motion);
            self.border(bounds, window);
        });
        painted
    }
}

fn release_presented_initial(
    initial: &RefCell<Option<PaintSnapshot>>,
    recorded: &Cell<Option<u64>>,
    window: &Window,
) {
    if recorded
        .get()
        .is_some_and(|sequence| sequence != window.presentation_sequence())
    {
        initial.borrow_mut().take();
        recorded.set(None);
    }
}

struct Driver {
    paint: FramePaint,
    target: Target,
    snapshot: Rc<RefCell<Option<PaintSnapshot>>>,
    initial: Rc<RefCell<Option<PaintSnapshot>>>,
    initial_frame: Rc<Cell<Option<u64>>>,
    epoch: Rc<Cell<u64>>,
    version: u64,
    frames: Rc<Cell<u64>>,
    notify: Rc<dyn Fn(&mut App)>,
}
impl Driver {
    fn schedule(driver: Rc<Self>, region: PaintRegion, window: &Window) {
        window.on_next_frame(move |window, cx| {
            if driver.epoch.get() != driver.version {
                return;
            }
            if !window.paint_region_is_current(&region) {
                (driver.notify)(cx);
                return;
            }
            release_presented_initial(&driver.initial, &driver.initial_frame, window);
            let initial = driver.initial.borrow().clone();
            if let Some(initial) = initial {
                let updated = window.repaint_region(&region, |window| {
                    window.paint_snapshot_at(&initial, point(px(0.), px(0.)));
                    window.retain_presented_frame();
                });
                if updated {
                    Self::schedule(driver, region, window);
                } else {
                    (driver.notify)(cx);
                }
                return;
            }
            let target = driver.target;
            let mut content = driver.paint.content.get();
            let content_moving = content.advance(
                target.open,
                super::press::playback_rate(cx),
                cx.reduce_motion(),
            );
            driver.paint.content.set(content);
            let (moving, _) = driver.paint.motion.borrow_mut().advance_frame(
                target.from,
                target.to,
                target.pair,
                target.open,
                target.material,
                true,
                window,
                cx,
            );
            let entered = !moving && !content_moving;
            let backdrop_moving = if driver.paint.recipe.backdrop.is_some() {
                let mut backdrop = driver.paint.backdrop.get();
                let moving = backdrop.advance(
                    target.open,
                    driver.paint.motion.borrow().expansion() >= 0.5,
                    super::press::playback_rate(cx),
                    cx.reduce_motion(),
                );
                driver.paint.backdrop.set(backdrop);
                moving
            } else {
                false
            };
            let settled = entered && !backdrop_moving;
            if settled && !target.open {
                (driver.notify)(cx);
                return;
            }
            let mut painted = false;
            let updated = window.repaint_region(&region, |window| {
                if let Some(snapshot) = driver.snapshot.borrow().as_ref() {
                    painted = driver.paint.replay(snapshot, window);
                }
            });
            if updated && painted {
                driver.frames.set(driver.frames.get() + 1);
                // A fully open panel already has its final layout/input tree.
                // Submit its exact terminal geometry and keep that valid tree;
                // new input or content still invalidates the region normally.
                if !settled {
                    Self::schedule(driver, region, window);
                }
            } else {
                (driver.notify)(cx);
            }
        });
    }
}

struct Playback {
    child: AnyElement,
    driver: Option<Rc<Driver>>,
    initial: Rc<RefCell<Option<PaintSnapshot>>>,
    initial_frame: Rc<Cell<Option<u64>>>,
    region: Rc<RefCell<Option<PaintRegion>>>,
}
impl IntoElement for Playback {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Playback {
    type RequestLayoutState = ();
    type PrepaintState = ();
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
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let initial = self.initial.borrow().clone();
        let (_, region) = window.record_paint_region(|window| {
            if let Some(initial) = initial {
                if self.initial_frame.get().is_none() {
                    self.initial_frame.set(Some(window.presentation_sequence()));
                }
                // Visit the one live input tree, but present the exact old
                // drawing at the new layer before advancing any motion.
                let bounds = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
                window
                    .capture_paint_snapshot(0, bounds, None, |window| self.child.paint(window, cx));
                window.paint_snapshot_at(&initial, point(px(0.), px(0.)));
                window.retain_presented_frame();
            } else {
                self.child.paint(window, cx);
            }
        });
        *self.region.borrow_mut() = region.clone();
        if let Some(driver) = &self.driver {
            if let Some(region) = region.filter(|_| driver.snapshot.borrow().is_some()) {
                Driver::schedule(driver.clone(), region, window);
            } else {
                (driver.notify)(cx);
            }
        }
    }
}

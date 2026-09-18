//! Paint-only updates keep the last UI frame, input tree and cache indices.
//! A normal UI draw retires all regions and resumes the regular lifecycle.
use super::*;

/// Completed frames share immutable drawing without copying the scene on
/// every presentation. The next UI frame always paints into a unique scene.
#[derive(Clone, Default)]
pub(crate) struct FrameScene(Arc<Scene>);
impl std::ops::Deref for FrameScene {
    type Target = Scene;
    fn deref(&self) -> &Scene { &self.0 }
}
impl std::ops::DerefMut for FrameScene {
    fn deref_mut(&mut self) -> &mut Scene {
        Arc::get_mut(&mut self.0).expect("completed scenes are immutable")
    }
}
impl FrameScene {
    pub fn insert_primitive(&mut self, primitive: impl Into<crate::Primitive>) {
        self.deref_mut().insert_primitive(primitive);
    }
    pub fn begin_retained(&mut self, key: u64, bounds: Bounds<ScaledPixels>) {
        self.deref_mut().begin_retained(key, bounds);
    }
    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        self.deref_mut().push_layer(bounds);
    }
    pub fn paint_barrier(&mut self, bounds: Bounds<ScaledPixels>) {
        self.deref_mut().paint_barrier(bounds);
    }
    pub fn clear(&mut self) {
        if let Some(scene) = Arc::get_mut(&mut self.0) { scene.clear(); }
        else { *self = Self::default(); }
    }
}
impl From<Scene> for FrameScene {
    fn from(scene: Scene) -> Self { Self(Arc::new(scene)) }
}

pub(super) struct SubmittedScene {
    pub scene: FrameScene,
    pub base: FrameScene,
    pub replacements: std::collections::BTreeMap<usize, (usize, FrameScene)>,
    pub generation: u64,
    pub viewport: Size<Pixels>,
    pub scale: f32,
}

/// A paint range in one completed UI frame. Playback expires on UI redraw;
/// a replayed range can still supply the last presented snapshot until resize.
#[derive(Clone, Debug)]
pub struct PaintRegion {
    id: (u64, usize),
    window: WindowId,
    generation: u64,
    range: Range<usize>,
    viewport: Size<Pixels>,
    scale: f32,
    mask: ContentMask<Pixels>,
    opacity: f32,
}

impl Window {
    /// Keep the already submitted scene for a layer handoff's first frame.
    /// Layout and input still commit normally. Reusing the exact prepared
    /// scene avoids raster changes caused by repacking identical paths into
    /// different atlas cells. The next UI draw or paint-only update releases
    /// this hold, and the submitted region/version baseline stays truthful.
    pub fn retain_presented_frame(&mut self) -> bool {
        self.invalidator.debug_assert_paint();
        if self.submitted_scene.as_ref().is_some_and(|frame| {
            frame.viewport == self.viewport_size() && frame.scale == self.scale_factor()
        }) {
            self.retain_submitted_frame = true;
            true
        } else {
            false
        }
    }

    /// Copy a bounded area of the actual last presentation. This fallback
    /// also works when an element was reused by an intervening UI frame and
    /// its original region token has expired.
    pub fn snapshot_presented(&self, key: u64, bounds: Bounds<Pixels>) -> crate::PaintSnapshot {
        let viewport = Bounds::new(point(px(0.), px(0.)), self.viewport_size()).scale(self.scale_factor());
        let bounds = bounds.scale(self.scale_factor()).intersect(&viewport);
        let source = self.submitted_scene.as_ref().map_or(&self.rendered_frame.scene, |frame| &frame.scene);
        let mut scene = Scene::default();
        if bounds != viewport {
            scene.push_paint_effect(crate::scene::PaintEffect::new(
                point(ScaledPixels(0.), ScaledPixels(0.)), 1., vec![bounds]));
        }
        scene.replay(0..source.len(), source);
        if bounds != viewport { scene.pop_paint_effect(); }
        scene.finish();
        crate::PaintSnapshot(std::sync::Arc::new(crate::RetainedContent { id: key, bounds, scene }))
    }

    /// Copy the last presented drawing of a region, including paint-only
    /// replacements. This is also valid during the next UI draw: the old
    /// submitted scene remains the authority until the next presentation.
    pub fn snapshot_paint_region(&self, key: u64, region: &PaintRegion) -> Option<crate::PaintSnapshot> {
        let submitted = self.submitted_scene.as_ref()?;
        if self.handle.window_id() != region.window
            || self.viewport_size() != region.viewport
            || self.scale_factor() != region.scale
        {
            return None;
        }
        let range = if submitted.generation == region.generation {
            region.range.clone()
        } else {
            // Cached source controls keep their original input identity. Their
            // paint markers move with replay into the actual submitted frame.
            submitted.base.region_range(region.id)?
        };
        let mut scene = Scene::default();
        let mut position = range.start;
        for (start, (end, replacement)) in &submitted.replacements {
            if *end <= range.start || *start >= range.end { continue; }
            if *start < range.start || *end > range.end { return None; }
            scene.replay(position..*start, &submitted.base);
            scene.replay(0..replacement.len(), replacement);
            position = *end;
        }
        scene.replay(position..range.end, &submitted.base);
        scene.finish();
        Some(crate::PaintSnapshot(std::sync::Arc::new(crate::RetainedContent {
            id: key,
            bounds: region.mask.bounds.scale(region.scale),
            scene,
        })))
    }

    /// Record a top-level paint range that can later be replaced without
    /// visiting elements or replaying their input handlers. Captured texture
    /// contents and enclosing paint transforms are not independent regions.
    pub fn record_paint_region<R>(
        &mut self,
        paint: impl FnOnce(&mut Self) -> R,
    ) -> (R, Option<PaintRegion>) {
        self.invalidator.debug_assert_paint();
        let independent = self.next_frame.scene.can_replace_paint();
        let id = (self.next_frame.generation, self.next_frame.scene.len());
        if independent { self.next_frame.scene.region_marker(id, true); }
        let start = self.next_frame.scene.len();
        let mask = self.content_mask();
        let opacity = self.element_opacity;
        let result = paint(self);
        let end = self.next_frame.scene.len();
        if independent { self.next_frame.scene.region_marker(id, false); }
        let region = independent.then(|| PaintRegion {
            id,
            window: self.handle.window_id(),
            generation: self.next_frame.generation,
            range: start..end,
            viewport: self.viewport_size(),
            scale: self.scale_factor(),
            mask,
            opacity,
        });
        (result, region)
    }

    /// Whether paint-only playback can still use the recorded layout/input
    /// frame. Real input/content invalidation always wins over animation.
    pub fn paint_region_is_current(&self, region: &PaintRegion) -> bool {
        self.invalidator.not_drawing()
            && !self.invalidator.is_dirty()
            && self.handle.window_id() == region.window
            && self.rendered_frame.generation == region.generation
            && self.viewport_size() == region.viewport
            && self.scale_factor() == region.scale
    }

    /// Update pixels only. The callback must use paint APIs, not build elements
    /// or register input. Non-overlapping regions compose in their original
    /// order; a normal UI draw discards the replacements and their tokens.
    pub fn repaint_region(&mut self, region: &PaintRegion, paint: impl FnOnce(&mut Self)) -> bool {
        if !self.paint_region_is_current(region)
            || self.paint_replacements.iter().any(|(start, (end, _))| {
                !(*start == region.range.start && *end == region.range.end)
                    && *start < region.range.end
                    && region.range.start < *end
            })
        {
            return false;
        }
        let scratch = mem::take(&mut self.next_frame.scene);
        let old_opacity = mem::replace(&mut self.element_opacity, region.opacity);
        self.retain_submitted_frame = false;
        self.invalidator.set_phase(DrawPhase::Paint);
        self.with_content_mask(Some(region.mask), paint);
        self.invalidator.set_phase(DrawPhase::None);
        self.element_opacity = old_opacity;
        let mut replacement = mem::replace(&mut self.next_frame.scene, scratch);
        replacement.finish();
        self.paint_replacements
            .insert(region.range.start, (region.range.end, replacement));

        // Keep the logical scene intact: ViewElement paint ranges and all
        // input-cache baselines still refer to the original rendered frame.
        let mut composed = Scene::default();
        let mut position = 0;
        for (start, (end, replacement)) in &self.paint_replacements {
            composed.replay(position..*start, &self.rendered_frame.scene);
            composed.replay(0..replacement.len(), replacement);
            position = *end;
        }
        composed.replay(
            position..self.rendered_frame.scene.len(),
            &self.rendered_frame.scene,
        );
        composed.finish();
        self.presentation_scene = if self.retain_submitted_frame {
            self.submitted_scene.as_ref().map(|frame| frame.scene.clone())
        } else {
            Some(composed.into())
        };
        self.needs_present.set(true);
        self.invalidator.wake_platform();
        true
    }

    /// Borrow the immutable pixels/commands produced by a retained paint key.
    pub fn retained_paint_snapshot(&self, key: u64) -> Option<crate::PaintSnapshot> {
        self.retained_paint
            .get(&key)
            .map(|(_, content)| crate::PaintSnapshot(content.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IntoElement, ParentElement, Styled, TestAppContext, canvas, div, fill, rgb};

    #[derive(Default)]
    struct RegionView {
        regions: Rc<RefCell<Vec<PaintRegion>>>,
        renders: Rc<Cell<usize>>,
    }
    impl Render for RegionView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            let regions = self.regions.clone();
            div()
                .id("original-input")
                .size_full()
                .bg(rgb(0xffffff))
                .on_mouse_down(MouseButton::Left, |_, _, _| {})
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            regions.borrow_mut().clear();
                            window.paint_quad(fill(bounds, rgb(0xff0000)));
                            for color in [0x00ff00, 0x0000ff] {
                                let (_, region) = window.record_paint_region(|window| {
                                    window.paint_quad(fill(bounds, rgb(color)));
                                });
                                regions.borrow_mut().push(region.unwrap());
                            }
                        },
                    )
                    .size_full(),
                )
        }
    }

    #[gpui::test]
    fn replacements_preserve_the_input_frame_and_compose_in_original_order(
        cx: &mut TestAppContext,
    ) {
        let handle = cx.add_window(|_, _| RegionView::default());
        let test_window = cx.test_window(handle.into());
        let wakes = test_window.frame_wake_count();
        let region = handle
            .update(cx, |view, window, _| {
                let regions = view.regions.borrow().clone();
                let generation = window.rendered_frame.generation;
                let length = window.rendered_frame.scene.len();
                let quads = window.rendered_frame.scene.quads.clone();
                let listeners = window.rendered_frame.mouse_listeners.len();
                let hitboxes = window.rendered_frame.hitboxes.len();
                let renders = view.renders.get();
                assert!(listeners > 0 && hitboxes > 0);
                let bounds = regions[0].mask.bounds;
                assert!(window.repaint_region(&regions[0], |window| {
                    window.paint_quad(fill(bounds, rgb(0x123456)));
                    window.paint_quad(fill(bounds, rgb(0x654321)));
                }));
                assert!(window.repaint_region(&regions[1], |_| {}));
                let visible = &window.presentation_scene.as_ref().unwrap().quads;
                assert_eq!(visible.len(), quads.len());
                assert_eq!(visible[visible.len() - 2].background, rgb(0x123456).into());
                assert_eq!(visible[visible.len() - 1].background, rgb(0x654321).into());
                window.present();
                let drawing = window.snapshot_paint_region(0x7a01, &regions[0]).unwrap();
                assert_eq!(drawing.0.scene.quads.len(), 2);
                assert_eq!(drawing.0.scene.quads[0].background, rgb(0x123456).into());
                assert_eq!(drawing.0.scene.quads[1].background, rgb(0x654321).into());
                assert!(window.repaint_region(&regions[0], |window| {
                    window.paint_quad(fill(bounds, rgb(0x112233)));
                }));
                let pending = window.snapshot_paint_region(0x7a05, &regions[0]).unwrap();
                assert_eq!(pending.0.scene.quads.len(), 2, "prepared drawing replaced the submitted picture");
                window.present();
                // An input invalidation must retain the last presented
                // picture long enough for a layer transfer to copy it.
                window.invalidator.set_dirty(true);
                let drawing = window.snapshot_paint_region(0x7a02, &regions[0]).unwrap();
                assert_eq!(drawing.0.scene.quads.len(), 1);
                window.invalidator.set_dirty(false);
                assert_eq!(window.rendered_frame.generation, generation);
                assert_eq!(window.rendered_frame.scene.len(), length);
                assert_eq!(window.rendered_frame.scene.quads, quads);
                assert_eq!(window.rendered_frame.mouse_listeners.len(), listeners);
                assert_eq!(window.rendered_frame.hitboxes.len(), hitboxes);
                assert_eq!(view.renders.get(), renders);
                assert!(!window.needs_present.get());
                regions[0].clone()
            })
            .unwrap();
        assert!(test_window.frame_wake_count() > wakes);
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.presentation_scene.is_none());
            assert!(window.paint_replacements.is_empty());
            assert!(!window.repaint_region(&region, |_| panic!("expired region painted")));
            // Preparing a UI frame does not replace the last submitted image.
            assert!(window.snapshot_paint_region(0x7a03, &region).is_some());
            window.present();
            assert!(window.snapshot_paint_region(0x7a04, &region).is_none());
        })
        .unwrap();
    }

    #[gpui::test]
    fn handoff_retains_the_submitted_picture_and_its_region_baseline(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, _| RegionView::default());
        let (old_region, old_generation, old_picture) = handle.update(cx, |view, window, _| {
            window.present();
            let region = view.regions.borrow()[0].clone();
            (region, window.rendered_frame.generation, window.submitted_scene.as_ref().unwrap().scene.quads.clone())
        }).unwrap();
        cx.update_window(handle.into(), |root, window, cx| {
            window.draw(cx).clear(cx);
            let view = root.downcast::<RegionView>().unwrap();
            let region = view.read(cx).regions.borrow()[0].clone();
            let input_generation = window.rendered_frame.generation;
            let listeners = window.rendered_frame.mouse_listeners.len();
            assert_ne!(input_generation, old_generation);
            assert!(window.repaint_region(&region, |window| {
                window.paint_quad(fill(region.mask.bounds, rgb(0xabcdef)));
                assert!(window.retain_presented_frame());
            }));
            assert_eq!(window.presentation_scene.as_ref().unwrap().quads, old_picture);
            window.present();
            assert_eq!(window.submitted_scene.as_ref().unwrap().generation, old_generation);
            assert!(window.snapshot_paint_region(11, &old_region).is_some());
            assert!(window.snapshot_paint_region(12, &region).is_none());
            assert_eq!(window.rendered_frame.generation, input_generation);
            assert_eq!(window.rendered_frame.mouse_listeners.len(), listeners);

            assert!(window.repaint_region(&region, |window| {
                window.paint_quad(fill(region.mask.bounds, rgb(0x123456)));
            }));
            assert!(!window.retain_submitted_frame);
            window.present();
            assert_eq!(window.submitted_scene.as_ref().unwrap().generation, input_generation);
            assert!(window.snapshot_paint_region(13, &region).is_some());
        }).unwrap();
    }

    #[gpui::test]
    fn invalidation_resize_and_other_windows_reject_old_regions(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, _| RegionView::default());
        let region = handle
            .update(cx, |view, window, _| {
                window.present();
                let region = view.regions.borrow()[0].clone();
                assert!(window.paint_region_is_current(&region));
                window.invalidator.set_dirty(true);
                assert!(!window.repaint_region(&region, |_| panic!("dirty frame painted")));
                window.invalidator.set_dirty(false);
                window.viewport_size.width += px(1.);
                assert!(!window.paint_region_is_current(&region));
                window.invalidator.set_phase(DrawPhase::Paint);
                assert!(!window.retain_presented_frame());
                window.invalidator.set_phase(DrawPhase::None);
                window.viewport_size.width -= px(1.);
                let mut scaled = region.clone();
                scaled.scale += 1.;
                assert!(!window.paint_region_is_current(&scaled));
                region
            })
            .unwrap();
        let other = cx.add_window(|_, _| RegionView::default());
        other
            .update(cx, |_, window, _| {
                assert!(!window.repaint_region(&region, |_| panic!("another window painted")));
            })
            .unwrap();
    }

    #[gpui::test]
    fn overlapping_regions_are_rejected_even_when_they_start_together(cx: &mut TestAppContext) {
        let handle = cx.add_window(|_, _| RegionView::default());
        handle
            .update(cx, |view, window, _| {
                let regions = view.regions.borrow().clone();
                assert!(window.repaint_region(&regions[0], |_| {}));
                assert!(window.repaint_region(&regions[0], |_| {}));
                let mut enclosing = regions[0].clone();
                enclosing.range.end = regions[1].range.end;
                assert!(!window.repaint_region(&enclosing, |_| panic!("overlap painted")));
                assert!(window.repaint_region(&regions[1], |_| {}));
            })
            .unwrap();
    }
}

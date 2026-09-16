//! Retained scene fragments. Paint indices remain in the original operation
//! stream so nested view caches and input replay keep their existing contract.
use super::*;
use std::sync::Arc;

/// An immutable drawing version and one of its compositions in a frame.
/// Cache entries must retain the corresponding content Arc with this key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RetainedLayerKey {
    content: usize,
    masked: bool,
    instance: usize,
}

/// Protect all drawing versions needed by a frame before recycling textures.
/// A source picture and its replacement can share a logical content ID.
#[derive(Default)]
pub struct RetainedFramePlan {
    required: collections::FxHashSet<RetainedLayerKey>,
    next: collections::FxHashMap<(usize, bool), usize>,
    textures: u64,
    supported: bool,
}
impl RetainedFramePlan {
    /// Collect compositions, including nested layers, with a bounded traversal.
    pub fn reset(&mut self, scene: &Scene, layer_limit: usize) {
        self.required.clear();
        self.next.clear();
        self.textures = 0;
        self.supported = true;
        self.visit(scene, layer_limit);
        self.next.clear();
    }
    fn visit(&mut self, scene: &Scene, limit: usize) {
        for layer in &scene.retained_layers {
            if self.required.len() >= limit || layer.effects.len() > 1
                || layer.effects.iter().any(|effect| effect.clips.len() > 1)
            {
                self.supported = false;
                return;
            }
            let key = self.next_key(layer);
            self.textures += 1 + u64::from(key.masked);
            self.required.insert(key);
            self.visit(&layer.content.scene, limit);
            if !self.supported { return; }
        }
    }
    /// Check allocation of every composition, including repeated versions.
    pub fn fits(&self, size: [u32; 2], bytes_per_pixel: u32, max_bytes: u64) -> bool {
        self.supported && u64::from(size[0]).checked_mul(u64::from(size[1]))
            .and_then(|pixels| pixels.checked_mul(u64::from(bytes_per_pixel)))
            .and_then(|bytes| bytes.checked_mul(self.textures))
            .is_some_and(|bytes| bytes <= max_bytes)
    }
    /// Assign an independent slot to each composition of an immutable version.
    pub fn next_key(&mut self, layer: &RetainedLayer) -> RetainedLayerKey {
        let content = Arc::as_ptr(&layer.content) as usize;
        let masked = layer.effects.iter().any(|effect| effect.gpu_mask.is_some());
        let next = self.next.entry((content, masked)).or_default();
        let key = RetainedLayerKey { content, masked, instance: *next };
        *next += 1;
        key
    }
    /// Whether a cached slot is needed by this frame, possibly later in order.
    pub fn contains(&self, key: &RetainedLayerKey) -> bool { self.required.contains(key) }
}

#[expect(missing_docs)]
pub struct RetainedContent {
    pub id: u64,
    pub bounds: Bounds<ScaledPixels>,
    pub scene: Scene,
}

/// Drawing captured without retaining a second element, input tree or focus
/// identity. Replaying it only submits paint operations.
#[derive(Clone)]
pub struct PaintSnapshot(pub(crate) Arc<RetainedContent>);
impl PaintSnapshot {
    /// Original window-space bounds in physical pixels.
    pub fn bounds(&self) -> Bounds<ScaledPixels> {
        self.0.bounds
    }
}

#[derive(Clone)]
#[expect(missing_docs)]
pub struct RetainedLayer {
    pub order: DrawOrder,
    pub content: Arc<RetainedContent>,
    pub effects: Vec<PaintEffect>,
}
impl RetainedLayer {
    /// Transformed bounds used to preserve paint ordering.
    pub fn bounds(&self) -> Bounds<ScaledPixels> {
        self.effects
            .iter()
            .rev()
            .fold(self.content.bounds, |b, e| e.bounds(b))
    }
}

pub(super) struct Capture {
    pub id: u64,
    pub bounds: Bounds<ScaledPixels>,
    pub start: usize,
    pub scene: Scene,
}

#[expect(missing_docs)]
pub enum SceneBatch {
    Primitives(PrimitiveBatch),
    RetainedLayer(usize),
}

impl Scene {
    pub(crate) fn can_retain(&self) -> bool {
        if let Some(capture) = self.captures.last() {
            return capture.scene.can_retain();
        }
        self.paint_effects.len() <= 1
            && self
                .paint_effects
                .iter()
                .all(|effect| effect.clips.len() <= 1)
    }

    pub(crate) fn can_mask_retain(&self) -> bool {
        self.captures
            .last()
            .map_or(self.paint_effects.is_empty(), |capture| {
                capture.scene.can_mask_retain()
            })
    }

    pub(crate) fn begin_retained(&mut self, id: u64, bounds: Bounds<ScaledPixels>) {
        let start = self.paint_operations.len();
        self.paint_operations.push(PaintOperation::RetainedStart {
            content: None,
            length: 0,
        });
        self.captures.push(Capture {
            id,
            bounds,
            start,
            scene: Scene::default(),
        });
    }

    pub(crate) fn end_retained(
        &mut self,
        previous: Option<&Arc<RetainedContent>>,
    ) -> Arc<RetainedContent> {
        self.end_capture(previous, true)
    }

    pub(crate) fn end_snapshot(
        &mut self,
        previous: Option<&Arc<RetainedContent>>,
    ) -> Arc<RetainedContent> {
        self.end_capture(previous, false)
    }

    fn end_capture(
        &mut self,
        previous: Option<&Arc<RetainedContent>>,
        composite: bool,
    ) -> Arc<RetainedContent> {
        let mut capture = self.captures.pop().expect("balanced retained capture");
        // Nested captures route their drawing to another scene. Keep the
        // complete stream for paint-only playback and renderer fallbacks.
        capture.scene.paint_operations = self.paint_operations[capture.start + 1..].to_vec();
        capture.scene.finish();
        let content = previous
            .filter(|p| p.bounds == capture.bounds && p.scene.same_drawing(&capture.scene))
            .cloned()
            .unwrap_or_else(|| {
                Arc::new(RetainedContent {
                    id: capture.id,
                    bounds: capture.bounds,
                    scene: capture.scene,
                })
            });
        self.paint_operations.push(PaintOperation::RetainedEnd);
        let length = self.paint_operations.len() - capture.start;
        self.paint_operations[capture.start] = if composite {
            PaintOperation::RetainedStart {
                content: Some(content.clone()),
                length,
            }
        } else {
            PaintOperation::SnapshotStart { length }
        };
        if composite {
            self.insert_retained(content.clone());
        }
        content
    }

    pub(crate) fn paint_snapshot(&mut self, snapshot: &PaintSnapshot) {
        let scene = &snapshot.0.scene;
        // Small source drawings can join the destination's normal batches;
        // they need no additional color texture or input replay.
        self.paint_barrier(snapshot.0.bounds);
        self.replay_with_retained(0..scene.len(), scene, false);
        self.paint_barrier(snapshot.0.bounds);
    }

    pub(super) fn insert_retained(&mut self, content: Arc<RetainedContent>) {
        if let Some(capture) = self.captures.last_mut() {
            capture.scene.insert_retained(content);
            return;
        }
        let mut layer = RetainedLayer {
            order: 0,
            content,
            effects: self.paint_effects.clone(),
        };
        let bounds = layer.bounds();
        layer.order = self.primitive_bounds.insert(bounds);
        // A texture composition is an ordering barrier, including within an
        // enclosing paint_layer. Later primitives must not batch ahead of it.
        let following = self.primitive_bounds.insert(bounds);
        for order in &mut self.layer_stack {
            *order = (*order).max(following);
        }
        self.retained_layers.push(layer);
    }

    pub(crate) fn record_retained_content(&mut self, content: Arc<RetainedContent>) {
        self.paint_operations
            .push(PaintOperation::RetainedContent(content.clone()));
        self.insert_retained(content);
    }

    fn same_drawing(&self, other: &Self) -> bool {
        self.surfaces.is_empty()
            && other.surfaces.is_empty()
            && self.quads == other.quads
            && self.shadows == other.shadows
            && self.paths.len() == other.paths.len()
            && self.paths.iter().zip(&other.paths).all(|(a, b)| a.same_drawing(b))
            && self.underlines == other.underlines
            && self.monochrome_sprites == other.monochrome_sprites
            && self.subpixel_sprites == other.subpixel_sprites
            && self.polychrome_sprites == other.polychrome_sprites
            && self.retained_layers.len() == other.retained_layers.len()
            && self
                .retained_layers
                .iter()
                .zip(&other.retained_layers)
                .all(|(a, b)| {
                    a.order == b.order
                        && Arc::ptr_eq(&a.content, &b.content)
                        && a.effects == b.effects
                })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScaledPixels, point, size};
    fn bounds() -> Bounds<ScaledPixels> {
        Bounds::new(
            point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(100.), ScaledPixels(100.)),
        )
    }
    fn quad() -> Quad {
        Quad {
            bounds: bounds(),
            content_mask: ContentMask { bounds: bounds() },
            background: crate::rgb(0xff0000).into(),
            ..Default::default()
        }
    }

    #[test]
    fn path_snapshots_share_geometry_and_detach_before_mutation() {
        let mut path = Path::new(point(crate::px(0.), crate::px(0.)));
        path.line_to(point(crate::px(50.), crate::px(0.)));
        path.line_to(point(crate::px(0.), crate::px(50.)));
        path.content_mask.bounds = path.bounds;
        let original = path.scale(2.);
        let mut changed = original.clone();
        assert!(Arc::ptr_eq(&original.vertices, &changed.vertices));
        assert!(original.same_drawing(&changed));

        changed.vertices_mut()[0].xy_position.x = ScaledPixels(10.);
        assert!(!Arc::ptr_eq(&original.vertices, &changed.vertices));
        assert_eq!(original.vertices[0].xy_position.x, ScaledPixels(0.));
        assert!(!original.same_drawing(&changed));

        let mut equal = original.clone();
        equal.vertices = Arc::new(original.vertices.as_ref().clone());
        assert!(!Arc::ptr_eq(&original.vertices, &equal.vertices));
        assert!(original.same_drawing(&equal));
        equal.color = crate::rgb(0xff0000).into();
        assert!(!original.same_drawing(&equal));

        let mut first = Scene::default();
        first.begin_retained(8, bounds());
        first.insert_primitive(original.clone());
        let initial = first.end_retained(None);
        let mut next = Scene::default();
        next.begin_retained(8, bounds());
        next.insert_primitive(original);
        let reused = next.end_retained(Some(&initial));
        assert!(Arc::ptr_eq(&initial, &reused));
        let mut next = Scene::default();
        next.begin_retained(8, bounds());
        next.insert_primitive(changed);
        let replaced = next.end_retained(Some(&initial));
        assert!(!Arc::ptr_eq(&initial, &replaced));
    }

    #[test]
    fn detached_snapshot_stays_hidden_on_replay_and_paints_only_at_its_destination() {
        let mut source = Scene::default();
        source.begin_retained(17, bounds());
        source.insert_primitive(quad());
        let snapshot = PaintSnapshot(source.end_snapshot(None));
        assert!(source.quads.is_empty());
        let mut cached = Scene::default();
        cached.replay(0..source.len(), &source);
        assert!(cached.quads.is_empty());
        assert!(cached.retained_layers.is_empty());
        cached.paint_snapshot(&snapshot);
        assert_eq!(cached.quads.len(), 1);
        let mut again = Scene::default();
        again.replay(0..cached.len(), &cached);
        assert_eq!(again.quads.len(), 1);
    }

    #[test]
    fn detached_snapshot_keeps_nested_cached_drawing_without_moving_input_ranges() {
        let mut scene = Scene::default();
        scene.begin_retained(17, bounds());
        let child_start = scene.len();
        scene.begin_retained(18, bounds());
        scene.insert_primitive(quad());
        scene.end_retained(None);
        let child_end = scene.len();
        let snapshot = PaintSnapshot(scene.end_snapshot(None));
        let mut destination = Scene::default();
        destination.paint_snapshot(&snapshot);
        assert_eq!(destination.quads.len(), 1);
        assert!(destination.retained_layers.is_empty());
        let mut child = Scene::default();
        child.replay_with_retained(child_start..child_end, &scene, false);
        assert_eq!(child.quads.len(), 1);
    }

    #[test]
    fn snapshot_drawing_preserves_order_inside_an_enclosing_paint_layer() {
        let mut captured = Scene::default();
        captured.begin_retained(17, bounds());
        let mut path = Path::new(point(crate::px(0.), crate::px(0.)));
        path.line_to(point(crate::px(100.), crate::px(0.)));
        path.line_to(point(crate::px(100.), crate::px(100.)));
        let mut path = path.scale(1.);
        path.content_mask.bounds = bounds();
        captured.insert_primitive(path);
        let snapshot = PaintSnapshot(captured.end_snapshot(None));
        let mut scene = Scene::default();
        scene.push_layer(bounds());
        scene.insert_primitive(quad());
        scene.paint_snapshot(&snapshot);
        scene.insert_primitive(quad());
        scene.pop_layer();
        scene.finish();
        let batches = scene.batches().collect::<Vec<_>>();
        assert_eq!(batches.len(), 3);
        assert!(matches!(batches[0], PrimitiveBatch::Quads(_)));
        assert!(matches!(batches[1], PrimitiveBatch::Paths(_)));
        assert!(matches!(batches[2], PrimitiveBatch::Quads(_)));
        let mut replay = Scene::default();
        replay.replay(0..scene.len(), &scene);
        replay.finish();
        assert_eq!(replay.batches().count(), 3);
    }

    #[test]
    fn direct_snapshot_retains_nested_drawing_when_expanded_by_a_fallback() {
        let mut capture = Scene::default();
        capture.begin_retained(17, bounds());
        capture.begin_retained(18, bounds());
        capture.insert_primitive(quad());
        capture.end_retained(None);
        let content = capture.end_retained(None);
        let mut destination = Scene::default();
        destination.record_retained_content(content);
        assert_eq!(destination.retained_layers.len(), 1);
        assert!(destination.quads.is_empty());
        let mut fallback = Scene::default();
        fallback.replay_with_retained(0..destination.len(), &destination, false);
        assert!(fallback.retained_layers.is_empty());
        assert_eq!(fallback.quads.len(), 1);
        assert_eq!(fallback.quads[0].background, quad().background);
    }

    #[test]
    fn gpu_mask_fallback_is_lazy_and_preserves_the_contour_clip() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let computed = calls.clone();
        let clipped = Bounds::new(
            point(ScaledPixels(35.), ScaledPixels(25.)),
            size(ScaledPixels(20.), ScaledPixels(30.)),
        );
        let mut effect = PaintEffect::new(bounds().origin, 1., vec![bounds()]);
        effect.gpu_mask = Some(Arc::new(
            Path::new(point(crate::px(0.), crate::px(0.))).scale(1.),
        ));
        effect.mask_clips = Some(Arc::new(DeferredPaintClips::new(move || {
            computed.fetch_add(1, Ordering::Relaxed);
            vec![clipped]
        })));
        let mut previous = Scene::default();
        previous.push_paint_effect(effect);
        previous.begin_retained(1, bounds());
        previous.insert_primitive(quad());
        previous.insert_primitive(quad());
        previous.end_retained(None);
        previous.pop_paint_effect();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        let mut fallback = Scene::default();
        fallback.replay_with_retained(0..previous.len(), &previous, false);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(fallback.quads.len(), 2);
        assert!(
            fallback
                .quads
                .iter()
                .all(|quad| quad.content_mask.bounds == clipped)
        );
    }

    #[test]
    fn replay_rechecks_retention_after_entering_a_multi_clip_effect() {
        let a = Bounds::new(bounds().origin, size(ScaledPixels(25.), ScaledPixels(100.)));
        let b = Bounds::new(point(ScaledPixels(75.), ScaledPixels(0.)), a.size);
        let mut previous = Scene::default();
        previous.push_paint_effect(PaintEffect::new(bounds().origin, 1., vec![a, b]));
        previous.begin_retained(1, bounds());
        previous.insert_primitive(quad());
        previous.end_retained(None);
        previous.pop_paint_effect();
        let mut next = Scene::default();
        next.replay(0..previous.len(), &previous);
        assert!(next.retained_layers.is_empty());
        assert_eq!(next.quads.len(), 2);
        assert_eq!(next.quads[0].content_mask.bounds, a);
        assert_eq!(next.quads[1].content_mask.bounds, b);
    }
    #[test]
    fn mask_admission_follows_the_active_capture_effects() {
        let mut scene = Scene::default();
        assert!(scene.can_mask_retain());
        scene.push_paint_effect(PaintEffect::new(bounds().origin, 1., vec![bounds()]));
        assert!(!scene.can_mask_retain());
        scene.begin_retained(1, bounds());
        assert!(scene.can_mask_retain());
        scene.push_paint_effect(PaintEffect::new(bounds().origin, 1., vec![bounds()]));
        assert!(!scene.can_mask_retain());
        scene.pop_paint_effect();
        scene.end_retained(None);
        scene.pop_paint_effect();
        assert!(scene.can_mask_retain());
    }

    #[test]
    fn cached_layer_reuses_pixels_but_preserves_nested_paint_indices() {
        let mut previous = Scene::default();
        previous.begin_retained(1, bounds());
        let inner_start = previous.len();
        previous.insert_primitive(quad());
        let inner_end = previous.len();
        let content = previous.end_retained(None);
        assert!(previous.quads.is_empty());
        assert_eq!(content.scene.quads.len(), 1);
        let mut next = Scene::default();
        next.replay(0..previous.len(), &previous);
        assert_eq!(next.len(), previous.len());
        assert!(Arc::ptr_eq(&next.retained_layers[0].content, &content));
        let mut changed = Scene::default();
        changed.begin_retained(1, bounds());
        changed.replay(inner_start..inner_end, &next);
        let reused = changed.end_retained(Some(&content));
        assert!(Arc::ptr_eq(&reused, &content));
    }
    #[test]
    fn retained_layer_keeps_current_parent_effect_out_of_cached_pixels() {
        let mut previous = Scene::default();
        let mut effect = PaintEffect::new(
            point(ScaledPixels(0.), ScaledPixels(0.)),
            1.,
            vec![bounds()],
        );
        effect.opacity = 0.2;
        previous.push_paint_effect(effect.clone());
        let start = previous.len();
        previous.begin_retained(1, bounds());
        previous.insert_primitive(quad());
        let content = previous.end_retained(None);
        let end = previous.len();
        previous.pop_paint_effect();
        let mut next = Scene::default();
        effect.opacity = 0.8;
        effect.scale = 0.9;
        next.push_paint_effect(effect);
        next.replay(start..end, &previous);
        next.pop_paint_effect();
        assert!(Arc::ptr_eq(&next.retained_layers[0].content, &content));
        assert_eq!(next.retained_layers[0].effects[0].opacity, 0.8);
        assert_eq!(next.retained_layers[0].effects[0].scale, 0.9);
    }
    #[test]
    fn texture_composition_is_an_ordering_barrier_inside_paint_layer() {
        let mut scene = Scene::default();
        scene.push_layer(bounds());
        scene.insert_primitive(quad());
        scene.begin_retained(1, bounds());
        scene.insert_primitive(quad());
        scene.end_retained(None);
        scene.insert_primitive(quad());
        scene.pop_layer();
        scene.finish();
        let batches = scene.render_batches().collect::<Vec<_>>();
        assert_eq!(batches.len(), 3);
        assert!(matches!(
            &batches[0],
            SceneBatch::Primitives(PrimitiveBatch::Quads(_))
        ));
        assert!(matches!(&batches[1], SceneBatch::RetainedLayer(0)));
        assert!(matches!(
            &batches[2],
            SceneBatch::Primitives(PrimitiveBatch::Quads(_))
        ));
    }
    #[test]
    fn changed_content_replaces_cached_scene_and_cpu_fallback_expands_it() {
        let mut a = Scene::default();
        a.begin_retained(1, bounds());
        a.insert_primitive(quad());
        let old = a.end_retained(None);
        let mut b = Scene::default();
        b.begin_retained(1, bounds());
        let mut q = quad();
        q.background = crate::rgb(0x0000ff).into();
        b.insert_primitive(q);
        let new = b.end_retained(Some(&old));
        assert!(!Arc::ptr_eq(&old, &new));
        let mut fallback = Scene::default();
        fallback.replay_with_retained(0..b.len(), &b, false);
        assert!(fallback.retained_layers.is_empty());
        assert_eq!(fallback.quads.len(), 1);
        assert_eq!(fallback.quads[0].background, q.background);
    }
}

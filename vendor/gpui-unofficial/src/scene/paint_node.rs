//! Persistent drawing references. Placement is resolved for the complete frame
//! before any pixels are submitted, including cached and paint-only replay.
use super::*;
use std::{collections::HashMap, sync::{Arc, atomic::{AtomicU64, Ordering}}};

/// A drawing identity shared by a control and its presentation layers.
/// Its ordinary placement remains in every frame; later placements replace
/// it atomically rather than drawing another copy or hiding the control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaintNode(u64);
impl Default for PaintNode {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{rgba, size};

    fn drawing(color: u32) -> Arc<RetainedContent> {
        let bounds = Bounds::new(point(ScaledPixels(0.), ScaledPixels(0.)), size(ScaledPixels(20.), ScaledPixels(20.)));
        let mut scene = Scene::default();
        scene.insert_primitive(Quad {
            bounds,
            content_mask: ContentMask { bounds },
            background: rgba(color).into(),
            ..Default::default()
        });
        scene.finish();
        Arc::new(RetainedContent { id: 1, bounds, scene })
    }

    #[test]
    fn a_translucent_node_is_composed_once_and_keeps_its_normal_placement() {
        let node = PaintNode::default();
        let source = drawing(0xff000080);
        let mut frame = Scene::default();
        frame.paint_node(node, source.clone());
        frame.paint_node(node, source.clone());
        frame.finish();
        assert_eq!(frame.quads.len(), 1);
        assert_eq!(frame.quads[0].background, source.scene.quads[0].background);

        // Removing playback leaves the normal source in the next scene,
        // without a callback, invalidation, or per-control visibility state.
        let mut closed = Scene::default();
        closed.paint_node(node, source);
        closed.finish();
        assert_eq!(closed.quads, frame.quads);
    }

    #[test]
    fn cached_replay_selects_the_current_placement_without_changing_indices() {
        let node = PaintNode::default();
        let source = drawing(0xff000080);
        let moved = drawing(0x0000ff80);
        let mut cached = Scene::default();
        cached.paint_node(node, source);
        cached.finish();
        let mut frame = Scene::default();
        frame.replay(0..cached.len(), &cached);
        frame.paint_node(node, moved.clone());
        let length = frame.len();
        frame.finish();
        assert_eq!(frame.len(), length);
        assert_eq!(frame.quads.len(), 1);
        assert_eq!(frame.quads[0].background, moved.scene.quads[0].background);
        let mut replay = Scene::default();
        replay.replay(0..frame.len(), &frame);
        replay.finish();
        assert_eq!(replay.quads, frame.quads);
    }

    #[test]
    fn superseded_parents_do_not_duplicate_children() {
        let child = PaintNode::default();
        let parent = PaintNode::default();
        let red = drawing(0xff000080);
        let blue = drawing(0x0000ff80);
        let mut nested = Scene::default();
        nested.paint_node(child, red.clone());
        nested.finish();
        let nested = Arc::new(RetainedContent { id: 2, bounds: red.bounds, scene: nested });
        let mut frame = Scene::default();
        frame.paint_node(parent, nested);
        frame.paint_node(child, blue.clone());
        frame.paint_node(parent, red.clone());
        frame.finish();
        assert_eq!(frame.quads.len(), 2);
        assert_eq!(frame.quads[0].background, blue.scene.quads[0].background);
        assert_eq!(frame.quads[1].background, red.scene.quads[0].background);
        assert!(frame.retained_layers.is_empty());
    }

    #[test]
    fn a_cache_keeps_its_texture_until_a_child_moves_outside_it() {
        let node = PaintNode::default();
        let picture = drawing(0xff000080);
        let mut nested = Scene::default();
        nested.paint_node(node, picture.clone());
        nested.finish();
        let nested = Arc::new(RetainedContent { id: 2, bounds: picture.bounds, scene: nested });
        let mut cached = Scene::default();
        cached.record_retained_content(nested.clone());
        cached.finish();
        assert_eq!(cached.retained_layers.len(), 1);
        assert!(Arc::ptr_eq(&cached.retained_layers[0].content, &nested));

        let mut moved = Scene::default();
        moved.replay(0..cached.len(), &cached);
        moved.paint_node(node, picture);
        moved.finish();
        assert!(moved.retained_layers.is_empty());
        assert_eq!(moved.quads.len(), 1);
    }

    #[test]
    fn a_node_cannot_restore_layers_when_the_renderer_requires_fallback() {
        let picture = drawing(0xff000080);
        let mut content = Scene::default();
        content.record_retained_content(picture.clone());
        content.finish();
        let content = Arc::new(RetainedContent { id: 2, bounds: picture.bounds, scene: content });
        let mut frame = Scene::default();
        frame.paint_node(PaintNode::default(), content);
        frame.finish();
        assert_eq!(frame.retained_layers.len(), 1);
        let fallback = frame.without_retained_layers();
        assert!(fallback.retained_layers.is_empty());
        assert_eq!(fallback.quads.len(), 1);
        assert_eq!(fallback.quads[0].background, picture.scene.quads[0].background);
    }

    #[test]
    fn a_partial_cached_capture_does_not_consume_following_drawing() {
        let red = drawing(0xff000080);
        let blue = drawing(0x0000ff80);
        let mut capture = Scene::default();
        capture.begin_retained(2, red.bounds);
        capture.paint_node(PaintNode::default(), red);
        capture.end_retained(None);
        capture.finish();
        let mut frame = Scene::default();
        frame.replay(0..capture.len() - 1, &capture);
        frame.insert_primitive(blue.scene.quads[0].clone());
        frame.finish();
        assert!(frame.retained_layers.is_empty());
        assert_eq!(frame.quads.len(), 2);
        assert_eq!(frame.quads[1].background, blue.scene.quads[0].background);
    }

    #[test]
    fn capturing_a_replacement_does_not_hide_a_live_node() {
        let node = PaintNode::default();
        let red = drawing(0xff000080);
        let mut frame = Scene::default();
        frame.paint_node(node, red.clone());
        frame.begin_retained(1, red.bounds);
        frame.paint_node(node, drawing(0x0000ff80));
        frame.end_snapshot(None);
        frame.finish();
        assert_eq!(frame.quads.len(), 1);
        assert_eq!(frame.quads[0].background, red.scene.quads[0].background);
    }
}

type Placements = HashMap<PaintNode, Vec<usize>>;

impl Scene {
    pub(crate) fn paint_node(&mut self, node: PaintNode, content: Arc<RetainedContent>) {
        self.has_paint_nodes = true;
        self.paint_operations.push(PaintOperation::Node { node, content: content.clone() });
        if let Some(capture) = self.captures.last_mut() {
            capture.scene.paint_node(node, content);
        }
    }

    pub(super) fn resolve_paint_nodes(&mut self) {
        let original = std::mem::take(self);
        self.retained_paint_disabled = original.retained_paint_disabled;
        let mut placements = Placements::new();
        original.collect_placements(&mut Vec::new(), &mut placements);
        self.compose_placements(&original, &mut Vec::new(), &placements);
        // View caches and PaintRegion ranges refer to the original operation
        // stream. Keep it intact; only the submitted drawing is resolved.
        self.paint_operations = original.paint_operations;
        self.has_paint_nodes = true;
    }

    /// Visit the actual drawing stream. Capture-only operations contain input
    /// traversal records but are never placements in the presented frame.
    fn drawing_operations(&self) -> Vec<usize> {
        let mut indices = Vec::new();
        let mut index = 0;
        while index < self.paint_operations.len() {
            match &self.paint_operations[index] {
                PaintOperation::SnapshotStart { length } if *length > 0 && index + length <= self.len() => {
                    index += length;
                    continue;
                }
                PaintOperation::RetainedStart { content: Some(_), length } if *length > 0 && index + length <= self.len() => {
                    indices.push(index);
                    index += length;
                    continue;
                }
                _ => indices.push(index),
            }
            index += 1;
        }
        indices
    }

    fn collect_placements(&self, path: &mut Vec<usize>, placements: &mut Placements) {
        // Walking back to front also skips children of superseded placements.
        // A nested copy cannot suppress a live child from a different layer.
        for index in self.drawing_operations().into_iter().rev() {
            path.push(index);
            match &self.paint_operations[index] {
                PaintOperation::Node { node, content, .. } => {
                    if !placements.contains_key(node) {
                        placements.insert(*node, path.clone());
                        content.scene.collect_placements(path, placements);
                    }
                }
                PaintOperation::RetainedContent(content)
                | PaintOperation::RetainedStart { content: Some(content), .. } => {
                    if content.scene.has_paint_nodes { content.scene.collect_placements(path, placements); }
                }
                _ => {}
            }
            path.pop();
        }
    }

    fn compose_placements(&mut self, source: &Scene, path: &mut Vec<usize>, placements: &Placements) {
        for index in source.drawing_operations() {
            path.push(index);
            match &source.paint_operations[index] {
                PaintOperation::Node { node, content } => {
                    if placements.get(node) == Some(path) {
                        self.paint_barrier(content.bounds);
                        self.compose_content(content, false, path, placements);
                        self.paint_barrier(content.bounds);
                    }
                }
                PaintOperation::RetainedContent(content)
                | PaintOperation::RetainedStart { content: Some(content), .. } => {
                    self.compose_content(content, true, path, placements);
                }
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::SharedPrimitive(primitive) => self.insert_shared_primitive(primitive.clone()),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
                PaintOperation::Barrier(bounds) => self.paint_barrier(*bounds),
                PaintOperation::StartEffect(effect) => self.push_paint_effect(effect.clone()),
                PaintOperation::EndEffect => self.pop_paint_effect(),
                PaintOperation::SnapshotStart { .. } | PaintOperation::RetainedStart { .. }
                | PaintOperation::RetainedEnd | PaintOperation::RegionMarker { .. } => {}
            }
            path.pop();
        }
    }

    fn compose_content(&mut self, content: &Arc<RetainedContent>, retained: bool, path: &mut Vec<usize>, placements: &Placements) {
        let compatible = if content.scene.has_paint_nodes && retained && self.can_retain() {
            let mut local = Placements::new();
            content.scene.collect_placements(path, &mut local);
            local.iter().all(|(node, placement)| placements.get(node) == Some(placement))
        } else {
            !content.scene.has_paint_nodes
        };
        if compatible && retained && self.can_retain() {
            self.record_retained_content(content.clone());
        } else {
            // Reuse the prepared cache when all its local placements match
            // the complete frame. A child moved outside this cache requires
            // ordinary drawing with the same parent effects and clipping.
            self.compose_placements(&content.scene, path, placements);
        }
    }
}

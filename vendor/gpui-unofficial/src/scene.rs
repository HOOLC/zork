// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Pixels,
    Point, Radians, ScaledPixels, Size, bounds_tree::BoundsTree, point,
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
};

mod retained;
mod paint_node;
pub use paint_node::PaintNode;
pub use retained::{PaintSnapshot, RetainedContent, RetainedFramePlan, RetainedLayer, RetainedLayerKey, SceneBatch};
use retained::Capture;

#[allow(non_camel_case_types, unused)]
#[expect(missing_docs)]
pub type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

#[expect(missing_docs)]
pub type DrawOrder = u32;

/// A boolean stored as a `u32` so that GPU-facing structs contain no
/// compiler-inserted padding bytes, which would be undefined behavior to
/// reinterpret as `&[u8]` when writing instance buffers. Guaranteed to be
/// `0` or `1` by construction; shaders read it as a `u32`/`uint`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct PaddedBool32(u32);

impl From<bool> for PaddedBool32 {
    fn from(value: bool) -> Self {
        PaddedBool32(value as u32)
    }
}

#[derive(Default)]
#[expect(missing_docs)]
pub struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    has_paint_nodes: bool,
    retained_paint_disabled: bool,
    captures: Vec<Capture>,
    pub retained_layers: Vec<RetainedLayer>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    paint_effects: Vec<PaintEffect>,
    layer_stack: Vec<DrawOrder>,
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<Path<ScaledPixels>>,
    pub underlines: Vec<Underline>,
    pub monochrome_sprites: Vec<MonochromeSprite>,
    pub subpixel_sprites: Vec<SubpixelSprite>,
    pub polychrome_sprites: Vec<PolychromeSprite>,
    pub surfaces: Vec<PaintSurface>,
}

#[expect(missing_docs)]
impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.has_paint_nodes = false;
        self.retained_paint_disabled = false;
        self.captures.clear();
        self.retained_layers.clear();
        self.paint_effects.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.subpixel_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub(crate) fn can_replace_paint(&self) -> bool {
        self.captures.is_empty() && self.paint_effects.is_empty()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        if let Some(capture) = self.captures.last_mut() {
            capture.scene.push_layer(bounds);
            self.paint_operations.push(PaintOperation::StartLayer(bounds));
            return;
        }
        let rendered_bounds = self.paint_effects.iter().rev().fold(bounds, |b, effect| effect.bounds(b));
        let order = self.primitive_bounds.insert(rendered_bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        if let Some(capture) = self.captures.last_mut() {
            capture.scene.pop_layer();
            self.paint_operations.push(PaintOperation::EndLayer);
            return;
        }
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let primitive = primitive.into();
        if self.captures.is_empty() {
            self.paint_operations.push(PaintOperation::Primitive(primitive.clone()));
            self.insert_transformed_primitive(primitive);
        } else {
            self.insert_shared_primitive(std::sync::Arc::new(primitive));
        }
    }

    fn insert_shared_primitive(&mut self, primitive: std::sync::Arc<Primitive>) {
        self.paint_operations.push(PaintOperation::SharedPrimitive(primitive.clone()));
        if let Some(capture) = self.captures.last_mut() {
            capture.scene.insert_shared_primitive(primitive);
            return;
        }
        self.insert_transformed_primitive((*primitive).clone());
    }

    fn insert_transformed_primitive(&mut self, primitive: Primitive) {
        if self.paint_effects.is_empty() {
            self.insert_rendered_primitive(primitive);
            return;
        }
        let mut primitives: smallvec::SmallVec<[Primitive; 1]> = smallvec::smallvec![primitive];
        for effect in self.paint_effects.iter().rev() {
            let mut transformed = smallvec::SmallVec::new();
            for primitive in primitives {
                transformed.extend(effect.apply(primitive));
            }
            primitives = transformed;
        }
        for primitive in primitives { self.insert_rendered_primitive(primitive); }
    }

    pub(crate) fn push_paint_effect(&mut self, effect: PaintEffect) {
        self.paint_operations.push(PaintOperation::StartEffect(effect.clone()));
        if let Some(capture) = self.captures.last_mut() { capture.scene.push_paint_effect(effect); }
        else { self.paint_effects.push(effect); }
    }

    pub(crate) fn pop_paint_effect(&mut self) {
        if let Some(capture) = self.captures.last_mut() { capture.scene.pop_paint_effect(); }
        else { self.paint_effects.pop().expect("balanced paint effects"); }
        self.paint_operations.push(PaintOperation::EndEffect);
    }

    pub(crate) fn region_marker(&mut self, id: (u64, usize), start: bool) {
        self.paint_operations.push(PaintOperation::RegionMarker { id, start });
    }

    pub(crate) fn region_range(&self, id: (u64, usize)) -> Option<Range<usize>> {
        let mut start = None;
        let mut found = None;
        for (index, operation) in self.paint_operations.iter().enumerate() {
            if let PaintOperation::RegionMarker { id: marker, start: begins } = operation {
                if *marker != id { continue; }
                if *begins {
                    if start.is_some() || found.is_some() { return None; }
                    start = Some(index + 1);
                } else {
                    found = Some(start.take()?..index);
                }
            }
        }
        if start.is_some() { None } else { found }
    }

    fn insert_rendered_primitive(&mut self, primitive: Primitive) {
        let clipped_bounds = primitive
            .bounds()
            .intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            return;
        }

        let order = self
            .layer_stack
            .last()
            .copied()
            .unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds));
        match primitive {
            Primitive::Shadow(mut shadow) => {
                shadow.order = order;
                self.shadows.push(shadow);
            }
            Primitive::Quad(mut quad) => {
                quad.order = order;
                self.quads.push(quad);
            }
            Primitive::Path(mut path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path);
            }
            Primitive::Underline(mut underline) => {
                underline.order = order;
                self.underlines.push(underline);
            }
            Primitive::MonochromeSprite(mut sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(sprite);
            }
            Primitive::SubpixelSprite(mut sprite) => {
                sprite.order = order;
                self.subpixel_sprites.push(sprite);
            }
            Primitive::PolychromeSprite(mut sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(sprite);
            }
            Primitive::Surface(mut surface) => {
                surface.order = order;
                self.surfaces.push(surface);
            }
        }

    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        self.replay_with_retained(range, prev_scene, true);
    }

    pub(crate) fn paint_barrier(&mut self, bounds: Bounds<ScaledPixels>) {
        self.paint_operations.push(PaintOperation::Barrier(bounds));
        if let Some(capture) = self.captures.last_mut() {
            capture.scene.paint_barrier(bounds);
            return;
        }
        let bounds = self.paint_effects.iter().rev().fold(bounds, |b, effect| effect.bounds(b));
        let order = self.primitive_bounds.insert(bounds);
        for current in &mut self.layer_stack {
            *current = (*current).max(order);
        }
    }

    pub(crate) fn replay_with_retained(&mut self, range: Range<usize>, prev_scene: &Scene, retained: bool) {
        let mut index = range.start;
        while index < range.end {
            let operation = &prev_scene.paint_operations[index];
            match operation {
                PaintOperation::Node { node, content } => {
                    self.paint_node(*node, content.clone());
                }
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::SharedPrimitive(primitive) => self.insert_shared_primitive(primitive.clone()),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::Barrier(bounds) => self.paint_barrier(*bounds),
                PaintOperation::RetainedContent(content) => {
                    if retained && self.can_retain() {
                        self.record_retained_content(content.clone());
                    } else {
                        self.replay_with_retained(0..content.scene.len(), &content.scene, false);
                    }
                }
                PaintOperation::EndLayer => self.pop_layer(),
                PaintOperation::StartEffect(effect) => self.push_paint_effect(effect.clone()),
                PaintOperation::EndEffect => self.pop_paint_effect(),
                PaintOperation::SnapshotStart { length } if index + length <= range.end => {
                    // Capture-only drawing stays hidden on cached replay.
                    // Its original input records are replayed separately by Window.
                    self.paint_operations.extend(prev_scene.paint_operations[index..index+length].iter().cloned());
                    index += length;
                    continue;
                }
                PaintOperation::RetainedStart { content: Some(content), length } if retained && self.can_retain() && index + length <= range.end => {
                    self.paint_operations.extend(prev_scene.paint_operations[index..index+length].iter().cloned());
                    self.insert_retained(content.clone());
                    index += length;
                    continue;
                }
                PaintOperation::RetainedStart { length, .. } => {
                    // This capture is being flattened, or the cached view's
                    // range contains only part of it. Do not let a later
                    // replay mistake following operations for its remainder.
                    self.paint_operations.push(PaintOperation::RetainedStart { content: None, length: *length });
                }
                PaintOperation::SnapshotStart { .. } | PaintOperation::RetainedEnd
                    | PaintOperation::RegionMarker { .. } => {
                    // A nested view's range can be inside a retained region.
                    // Its original primitives are replayed into the new capture.
                    self.paint_operations.push(operation.clone());
                }
            }
            index += 1;
        }
    }

    pub fn finish(&mut self) {
        if self.has_paint_nodes {
            self.resolve_paint_nodes();
        }
        self.shadows.sort_by_key(|shadow| shadow.order);
        self.quads.sort_by_key(|quad| quad.order);
        self.paths.sort_by_key(|path| path.order);
        self.underlines.sort_by_key(|underline| underline.order);
        self.monochrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.subpixel_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.polychrome_sprites
            .sort_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.surfaces.sort_by_key(|surface| surface.order);
        self.retained_layers.sort_by_key(|layer| layer.order);
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub fn batches(&self) -> impl Iterator<Item = PrimitiveBatch> + '_ {
        self.render_batches().filter_map(|batch| match batch { SceneBatch::Primitives(batch) => Some(batch), SceneBatch::RetainedLayer(_) => None })
    }

    /// Expand captured drawing through the same transforms and clips when a
    /// renderer cannot admit this frame to its bounded layer cache.
    pub fn without_retained_layers(&self) -> Self {
        let mut scene = Self { retained_paint_disabled: true, ..Self::default() };
        scene.replay_with_retained(0..self.len(), self, false);
        scene.finish();
        scene
    }

    pub fn render_batches(&self) -> impl Iterator<Item = SceneBatch> + '_ {
        BatchIterator {
            retained_start: 0,
            retained_iter: self.retained_layers.iter().peekable(),
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            subpixel_sprites_start: 0,
            subpixel_sprites_iter: self.subpixel_sprites.iter().peekable(),
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    SubpixelSprite,
    PolychromeSprite,
    Surface,
    RetainedLayer,
}

#[derive(Clone)]
pub(crate) enum PaintOperation {
    Node { node: PaintNode, content: std::sync::Arc<RetainedContent> },
    RegionMarker { id: (u64, usize), start: bool },
    RetainedContent(std::sync::Arc<RetainedContent>),
    Barrier(Bounds<ScaledPixels>),
    SnapshotStart { length: usize },
    RetainedStart { content: Option<std::sync::Arc<RetainedContent>>, length: usize },
    RetainedEnd,
    StartEffect(PaintEffect),
    EndEffect,
    Primitive(Primitive),
    SharedPrimitive(std::sync::Arc<Primitive>),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

#[derive(Clone)]
#[expect(missing_docs)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    SubpixelSprite(SubpixelSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
}

#[expect(missing_docs)]
impl Primitive {
    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::SubpixelSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
        }
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::SubpixelSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    retained_start: usize,
    retained_iter: Peekable<slice::Iter<'a, RetainedLayer>>,
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    subpixel_sprites_start: usize,
    subpixel_sprites_iter: Peekable<slice::Iter<'a, SubpixelSprite>>,
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = SceneBatch;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (self.retained_iter.peek().map(|l| l.order), PrimitiveKind::RetainedLayer),
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.subpixel_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::SubpixelSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        if batch_kind == PrimitiveKind::RetainedLayer {
            let index = self.retained_start;
            self.retained_start += 1;
            self.retained_iter.next();
            return Some(SceneBatch::RetainedLayer(index));
        }
        let batch = match batch_kind {
            PrimitiveKind::RetainedLayer => unreachable!(),
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(shadows_start..shadows_end))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(quads_start..quads_end))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(paths_start..paths_end))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(underlines_start..underlines_end))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::SubpixelSprite => {
                let texture_id = self.subpixel_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.subpixel_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.subpixel_sprites_iter.next();
                while self
                    .subpixel_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.subpixel_sprites_start = sprites_end;
                Some(PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(surfaces_start..surfaces_end))
            }
        };
        batch.map(SceneBatch::Primitives)
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
#[allow(missing_docs)]
pub enum PrimitiveBatch {
    Shadows(Range<usize>),
    Quads(Range<usize>),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    SubpixelSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    Surfaces(Range<usize>),
}

impl PrimitiveBatch {
    #[expect(missing_docs)]
    pub fn label(&self) -> String {
        match self {
            Self::Shadows(range) => format!("shadows ({})", range.len()),
            Self::Quads(range) => format!("quads ({})", range.len()),
            Self::Paths(range) => format!("paths ({})", range.len()),
            Self::Underlines(range) => format!("underlines ({})", range.len()),
            Self::MonochromeSprites { texture_id, range } => {
                format!(
                    "monochrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::SubpixelSprites { texture_id, range } => {
                format!(
                    "subpixel sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::PolychromeSprites { texture_id, range } => {
                format!(
                    "polychrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::Surfaces(range) => format!("surfaces ({})", range.len()),
        }
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: PaddedBool32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub element_bounds: Bounds<ScaledPixels>,
    pub element_corner_radii: Corners<ScaledPixels>,
    /// 0 = drop shadow (rendered outside the element), 1 = inset shadow (rendered inside).
    pub inset: u32,
    pub pad: u32, // align to 8 bytes
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(mut self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
#[expect(missing_docs)]
pub struct MonochromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
#[expect(missing_docs)]
pub struct SubpixelSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
}

impl From<SubpixelSprite> for Primitive {
    fn from(sprite: SubpixelSprite) -> Self {
        Primitive::SubpixelSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub grayscale: PaddedBool32,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    #[cfg(target_os = "macos")]
    pub image_buffer: core_video::pixel_buffer::CVPixelBuffer,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[expect(missing_docs)]
pub struct PathId(pub usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug, PartialEq)]
#[expect(missing_docs)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub id: PathId,
    pub order: DrawOrder,
    pub bounds: Bounds<P>,
    pub content_mask: ContentMask<P>,
    pub vertices: std::sync::Arc<Vec<PathVertex<P>>>,
    pub color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl<P: Clone + Debug + Default + PartialEq> Path<P> {
    /// Mutate geometry without changing snapshots that share these vertices.
    pub fn vertices_mut(&mut self) -> &mut Vec<PathVertex<P>> {
        std::sync::Arc::make_mut(&mut self.vertices)
    }
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: std::sync::Arc::default(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        self.scale_with_offset(factor, Point::default())
    }

    /// Translate the path geometry and scale it in one pass. The content mask
    /// stays in window coordinates, so translation does not move the clip.
    pub fn scale_with_offset(&self, factor: f32, offset: Point<Pixels>) -> Path<ScaledPixels> {
        let position = |value: Point<Pixels>| {
            if offset == Point::default() { value.scale(factor) }
            else { (value + offset).scale(factor) }
        };
        Path {
            id: self.id,
            order: self.order,
            bounds: Bounds::new(position(self.bounds.origin), self.bounds.size.scale(factor)),
            content_mask: self.content_mask.scale(factor),
            vertices: std::sync::Arc::new(self
                .vertices
                .iter()
                .map(|vertex| PathVertex {
                    xy_position: position(vertex.xy_position),
                    st_position: vertex.st_position,
                    content_mask: vertex.content_mask.scale(factor),
                })
                .collect()),
            start: position(self.start),
            current: position(self.current),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        let vertices = self.vertices_mut();
        vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    #[expect(missing_docs)]
    pub fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl Path<ScaledPixels> {
    fn same_drawing(&self, other: &Self) -> bool {
        self.id == other.id
            && self.order == other.order
            && self.bounds == other.bounds
            && self.content_mask == other.content_mask
            && self.color == other.color
            && self.start == other.start
            && self.current == other.current
            && self.contour_count == other.contour_count
            && (std::sync::Arc::ptr_eq(&self.vertices, &other.vertices)
                || self.vertices == other.vertices)
    }

    /// Triangles that can contribute inside the content clip. The conservative
    /// hull check retains crossing triangles and one physical pixel of AA.
    pub fn visible_triangles(&self) -> impl Iterator<Item = &[PathVertex<ScaledPixels>]> {
        self.visible_triangles_in(self.clipped_bounds())
    }

    /// Also exclude triangles outside the actual render target. Geometry and
    /// gradient coordinates remain unchanged; crossing and AA edges are kept.
    pub fn visible_triangles_in(&self, target: Bounds<ScaledPixels>) -> impl Iterator<Item = &[PathVertex<ScaledPixels>]> {
        let mask = self.clipped_bounds().intersect(&target);
        self.vertices.chunks_exact(3).filter(move |triangle| {
            !mask.is_empty()
                && !triangle.iter().all(|v| v.xy_position.x.0 < mask.left().0 - 1.)
                && !triangle.iter().all(|v| v.xy_position.x.0 > mask.right().0 + 1.)
                && !triangle.iter().all(|v| v.xy_position.y.0 < mask.top().0 - 1.)
                && !triangle.iter().all(|v| v.xy_position.y.0 > mask.bottom().0 + 1.)
        })
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug, PartialEq)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub xy_position: Point<P>,
    pub st_position: Point<f32>,
    pub content_mask: ContentMask<P>,
}

#[expect(missing_docs)]
impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}

/// Paint-only uniform scaling followed by a union of non-overlapping clip bands.
/// Raw operations retain the effect boundary so cached replay applies it once.
#[derive(Clone)]
pub struct PaintEffect {
    /// Uniform scale of the captured content.
    pub scale: f32,
    /// Alpha applied when the content is presented.
    pub opacity: f32,
    /// Window-space center of the scale transform.
    pub origin: Point<ScaledPixels>,
    /// Translation applied after scaling, in destination window coordinates.
    pub translation: Point<ScaledPixels>,
    /// Disjoint window-space clip rectangles, applied after the transform.
    pub clips: std::sync::Arc<Vec<Bounds<ScaledPixels>>>,
    /// An optional contour mask in window coordinates.
    pub gpu_mask: Option<std::sync::Arc<Path<ScaledPixels>>>,
    pub(crate) mask_clips: Option<std::sync::Arc<DeferredPaintClips>>,
    ordered_rows: bool,
}

pub(crate) struct DeferredPaintClips {
    compute: Box<dyn Fn() -> Vec<Bounds<ScaledPixels>> + Send + Sync>,
    value: std::sync::OnceLock<Vec<Bounds<ScaledPixels>>>,
}
impl DeferredPaintClips {
    pub(crate) fn new(compute: impl Fn() -> Vec<Bounds<ScaledPixels>> + Send + Sync + 'static) -> Self {
        Self { compute: Box::new(compute), value: Default::default() }
    }
    fn get(&self) -> &[Bounds<ScaledPixels>] {
        self.value.get_or_init(|| (self.compute)())
    }
}
impl PartialEq for PaintEffect {
    fn eq(&self, other: &Self) -> bool {
        self.scale == other.scale && self.opacity == other.opacity
            && self.origin == other.origin && self.translation == other.translation && self.clips == other.clips
            && self.gpu_mask == other.gpu_mask
        // The deferred bands represent gpu_mask, not an independent effect.
    }
}
impl PaintEffect {
    /// Construct a compositor effect in physical window coordinates.
    pub fn new(origin: Point<ScaledPixels>, scale: f32, clips: Vec<Bounds<ScaledPixels>>) -> Self {
        let ordered_rows = clips.windows(2).all(|pair| pair[0].top() <= pair[1].top() && pair[0].bottom() <= pair[1].bottom());
        Self { origin, translation: Point::default(), scale, opacity: 1., gpu_mask: None, mask_clips: None, clips: std::sync::Arc::new(clips), ordered_rows }
    }
    fn relevant_clips(&self, bounds: Bounds<ScaledPixels>) -> &[Bounds<ScaledPixels>] {
        let clips = self.mask_clips.as_ref().map_or(&**self.clips, |mask| mask.get());
        if self.mask_clips.is_none() && !self.ordered_rows { return clips; }
        let start = clips.partition_point(|band| band.bottom() <= bounds.top());
        let end = clips.partition_point(|band| band.top() < bounds.bottom());
        &clips[start..end.max(start)]
    }
    fn point(&self, p: Point<ScaledPixels>) -> Point<ScaledPixels> {
        self.origin + (p - self.origin) * self.scale + self.translation
    }
    fn bounds(&self, b: Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
        Bounds::new(self.point(b.origin), crate::size(b.size.width * self.scale, b.size.height * self.scale))
    }
    fn apply(&self, mut primitive: Primitive) -> smallvec::SmallVec<[Primitive; 1]> {
        // Raw operations were already recorded by Scene::insert_primitive, so
        // an invisible cached subtree can still replay at a later opacity.
        if self.opacity == 0. && !matches!(primitive, Primitive::Surface(_)) {
            return smallvec::SmallVec::new();
        }
        let scale = self.scale;
        let opacity = self.opacity;
        match &mut primitive {
            Primitive::Quad(p) => { p.background = p.background.opacity(opacity); p.border_color = p.border_color.opacity(opacity); }
            Primitive::Shadow(p) => p.color = p.color.opacity(opacity),
            Primitive::Path(p) => p.color = p.color.opacity(opacity),
            Primitive::Underline(p) => p.color = p.color.opacity(opacity),
            Primitive::MonochromeSprite(p) => p.color = p.color.opacity(opacity),
            Primitive::SubpixelSprite(p) => p.color = p.color.opacity(opacity),
            Primitive::PolychromeSprite(p) => p.opacity *= opacity,
            Primitive::Surface(_) => {},
        }
        macro_rules! common { ($p:expr) => {{
            $p.bounds = self.bounds($p.bounds);
            $p.content_mask.bounds = self.bounds($p.content_mask.bounds);
        }} }
        macro_rules! sprite { ($p:expr) => {{
            common!($p);
            let m = &mut $p.transformation;
            let x = self.origin.x.0 * (1. - scale) + self.translation.x.0;
            let y = self.origin.y.0 * (1. - scale) + self.translation.y.0;
            m.translation = [scale * m.translation[0] + x - m.rotation_scale[0][0]*x - m.rotation_scale[0][1]*y,
                scale * m.translation[1] + y - m.rotation_scale[1][0]*x - m.rotation_scale[1][1]*y];
        }} }
        // Opacity and clipping alone do not change geometry. In particular,
        // avoid copy-on-write of every cached path for an identity transform.
        if scale != 1. || self.translation != Point::default() {
        match &mut primitive {
            Primitive::Quad(p) => { common!(p); p.corner_radii = p.corner_radii.map(|v| *v * scale); p.border_widths = p.border_widths.map(|v| *v * scale); }
            Primitive::Shadow(p) => { common!(p); p.blur_radius = p.blur_radius * scale; p.corner_radii = p.corner_radii.map(|v| *v * scale); p.element_bounds = self.bounds(p.element_bounds); p.element_corner_radii = p.element_corner_radii.map(|v| *v * scale); }
            Primitive::Path(p) => { common!(p); p.start = self.point(p.start); p.current = self.point(p.current); for vertex in p.vertices_mut() { vertex.xy_position = self.point(vertex.xy_position); } }
            Primitive::Underline(p) => { common!(p); p.thickness = p.thickness * scale; }
            Primitive::MonochromeSprite(p) => { sprite!(p); }
            Primitive::SubpixelSprite(p) => { sprite!(p); }
            Primitive::PolychromeSprite(p) => { common!(p); p.corner_radii = p.corner_radii.map(|v| *v * scale); }
            Primitive::Surface(p) => { common!(p); }
        }
        }
        let bounds = primitive.bounds().intersect(&primitive.content_mask().bounds);
        if bounds.is_empty() { return smallvec::SmallVec::new(); }
        let mut masks: smallvec::SmallVec<[Bounds<ScaledPixels>; 1]> = smallvec::SmallVec::new();
        for band in self.relevant_clips(bounds) {
            let clip = bounds.intersect(band);
            if clip.is_empty() { continue; }
            if let Some(last) = masks.last_mut() {
                if last.left() == clip.left() && last.right() == clip.right() && last.bottom() == clip.top() {
                    *last = last.union(&clip);
                    continue;
                }
            }
            masks.push(clip);
        }
        let count = masks.len();
        let mut source = Some(primitive);
        masks.into_iter().enumerate().map(|(index, mask)| {
            let mut copy = if index + 1 == count { source.take().unwrap() }
                else { source.as_ref().unwrap().clone() };
            match &mut copy {
                Primitive::Quad(p) => p.content_mask.bounds = mask,
                Primitive::Shadow(p) => p.content_mask.bounds = mask,
                Primitive::Path(p) => {
                    p.content_mask.bounds = mask;
                    // A narrow clip band must not upload the complete path
                    // again. Keep triangles whose hull can affect this band;
                    // one physical pixel protects antialiasing at its edges.
                    if mask == p.bounds { return copy; }
                    let visible: Vec<bool> = p.vertices.chunks_exact(3).map(|triangle| {
                        !triangle.iter().all(|v| v.xy_position.x.0 < mask.left().0 - 1.)
                            && !triangle.iter().all(|v| v.xy_position.x.0 > mask.right().0 + 1.)
                            && !triangle.iter().all(|v| v.xy_position.y.0 < mask.top().0 - 1.)
                            && !triangle.iter().all(|v| v.xy_position.y.0 > mask.bottom().0 + 1.)
                    }).collect();
                    let mut index = 0;
                    p.vertices_mut().retain(|_| { let keep = visible[index / 3]; index += 1; keep });
                },
                Primitive::Underline(p) => p.content_mask.bounds = mask,
                Primitive::MonochromeSprite(p) => p.content_mask.bounds = mask,
                Primitive::SubpixelSprite(p) => p.content_mask.bounds = mask,
                Primitive::PolychromeSprite(p) => p.content_mask.bounds = mask,
                Primitive::Surface(p) => p.content_mask.bounds = mask,
            }
            copy
        }).collect()
    }
}

#[cfg(test)]
mod paint_effect_tests {
    use super::*;
    use crate::size;
    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
        Bounds::new(point(ScaledPixels(x), ScaledPixels(y)), size(ScaledPixels(w), ScaledPixels(h)))
    }
    fn quad() -> Quad {
        Quad { bounds: rect(0., 0., 100., 100.), content_mask: ContentMask { bounds: rect(0., 0., 100., 100.) }, ..Default::default() }
    }
    fn effect() -> PaintEffect {
        PaintEffect::new(point(ScaledPixels(50.), ScaledPixels(50.)), 0.5, vec![rect(0., 0., 100., 50.), rect(50., 50., 50., 50.)])
    }

    #[test]
    fn borrowed_path_translation_and_scaling_preserve_the_separate_passes() {
        let mut path = Path::new(point(crate::px(-3.125), crate::px(2.25)));
        path.curve_to(point(crate::px(31.5), crate::px(44.25)), point(crate::px(8.75), crate::px(-5.5)));
        path.line_to(point(crate::px(21.125), crate::px(22.5)));
        path.content_mask.bounds = Bounds::new(point(crate::px(-20.), crate::px(-30.)), crate::size(crate::px(100.), crate::px(200.)));
        let original = path.clone();
        for factor in [0.75, 1., 1.25, 2., 2.5] {
            for offset in [Point::default(), point(crate::px(13.125), crate::px(-7.25)), point(crate::px(-47.5), crate::px(812.625))] {
                let mut translated = path.clone();
                translated.bounds.origin += offset;
                translated.start += offset;
                translated.current += offset;
                for vertex in translated.vertices_mut() { vertex.xy_position += offset; }
                assert_eq!(path.scale_with_offset(factor, offset), translated.scale(factor));
                assert_eq!(path, original);
                assert!(std::sync::Arc::ptr_eq(&path.vertices, &original.vertices));
            }
        }
    }

    #[test]
    fn identity_composition_keeps_cached_vertices_shared() {
        let mut path = Path::new(point(crate::px(0.), crate::px(0.)));
        path.line_to(point(crate::px(100.), crate::px(0.)));
        path.line_to(point(crate::px(100.), crate::px(100.)));
        let mut path = path.scale(1.);
        path.content_mask.bounds = rect(-10., -10., 120., 120.);
        let source = path.clone();
        let mut effect = PaintEffect::new(point(ScaledPixels(37.), ScaledPixels(29.)), 1.,
            vec![rect(-10., -10., 120., 120.)]);
        effect.opacity = 0.4;
        let output = effect.apply(Primitive::Path(path));
        assert_eq!(output.len(), 1);
        assert!(!output.spilled(), "one composition needs no primitive heap buffer");
        let Primitive::Path(composed) = &output[0] else { panic!("path changed kind") };
        assert!(std::sync::Arc::ptr_eq(&source.vertices, &composed.vertices));
        assert_eq!(composed.bounds, source.bounds);
        assert_eq!(composed.color, source.color.opacity(0.4));
        assert_eq!(composed.content_mask.bounds, source.bounds);
    }

    #[test]
    fn paint_region_markers_follow_cached_replay_and_reject_ambiguous_copies() {
        let id = (7, 2);
        let mut original = Scene::default();
        original.insert_primitive(quad());
        original.region_marker(id, true);
        original.insert_primitive(quad());
        original.region_marker(id, false);
        assert_eq!(original.region_range(id), Some(2..3));
        let mut replayed = Scene::default();
        replayed.insert_primitive(quad());
        replayed.replay(0..original.len(), &original);
        assert_eq!(replayed.region_range(id), Some(3..4));
        let mut copied = Scene::default();
        copied.replay(replayed.region_range(id).unwrap(), &replayed);
        assert_eq!(copied.quads.len(), 1);
        replayed.replay(0..original.len(), &original);
        assert!(replayed.region_range(id).is_none());
    }
    #[test]
    fn paint_effect_preserves_each_clip_region_and_scales_geometry() {
        let mut scene = Scene::default(); scene.push_paint_effect(effect()); scene.insert_primitive(quad()); scene.pop_paint_effect();
        assert_eq!(scene.quads.len(), 2);
        assert_eq!(scene.quads[0].bounds, rect(25., 25., 50., 50.));
        assert_eq!(scene.quads[0].content_mask.bounds, rect(25., 25., 50., 25.));
        assert_eq!(scene.quads[1].content_mask.bounds, rect(50., 50., 25., 25.));
    }
    #[test]
    fn paint_effect_replay_does_not_scale_twice() {
        let mut previous = Scene::default(); previous.push_paint_effect(effect()); previous.insert_primitive(quad()); previous.pop_paint_effect();
        let mut next = Scene::default(); next.replay(0..previous.len(), &previous);
        assert_eq!(next.quads.len(), previous.quads.len());
        for (a,b) in next.quads.iter().zip(previous.quads.iter()) { assert_eq!(a.bounds,b.bounds); assert_eq!(a.content_mask.bounds,b.content_mask.bounds); }
    }
    #[test]
    fn destination_translation_is_applied_once_before_window_clipping() {
        let mut effect = PaintEffect::new(point(ScaledPixels(0.), ScaledPixels(0.)), 0.5,
            vec![rect(0., 0., 100., 100.)]);
        effect.translation = point(ScaledPixels(20.), ScaledPixels(-10.));
        let mut source = Scene::default();
        source.push_paint_effect(effect);
        source.insert_primitive(quad());
        source.pop_paint_effect();
        assert_eq!(source.quads[0].bounds, rect(20., -10., 50., 50.));
        assert_eq!(source.quads[0].content_mask.bounds, rect(20., 0., 50., 40.));
        let mut replay = Scene::default();
        replay.replay(0..source.len(), &source);
        assert_eq!(replay.quads[0].bounds, source.quads[0].bounds);
        assert_eq!(replay.quads[0].content_mask, source.quads[0].content_mask);
    }
    #[test]
    fn paint_effect_cached_child_uses_current_parent_effect() {
        let mut previous = Scene::default(); previous.push_paint_effect(effect()); let start = previous.len(); previous.insert_primitive(quad()); let end = previous.len(); previous.pop_paint_effect();
        let mut next = Scene::default(); let mut current = effect(); current.scale = 1.; next.push_paint_effect(current); next.replay(start..end, &previous); next.pop_paint_effect();
        assert_eq!(next.quads[0].bounds, rect(0., 0., 100., 100.));
    }
    #[test]
    fn cached_child_replays_current_alpha_without_accumulating_it() {
        let mut input = quad();
        input.border_color = crate::hsla(0.2, 0.5, 0.4, 0.8);
        let mut previous = Scene::default();
        let mut faded = effect(); faded.opacity = 0.25;
        previous.push_paint_effect(faded);
        let start = previous.len(); previous.insert_primitive(input); let end = previous.len();
        previous.pop_paint_effect();
        assert_eq!(previous.quads[0].border_color.a, 0.2);
        let mut next = Scene::default();
        let mut current = effect(); current.opacity = 0.75;
        next.push_paint_effect(current); next.replay(start..end, &previous); next.pop_paint_effect();
        assert_eq!(next.quads[0].border_color.a, 0.6);
        next.insert_primitive(input);
        assert_eq!(next.quads.last().unwrap().border_color.a, 0.8);
        let mut replay = Scene::default(); replay.replay(0..next.len(), &next);
        assert_eq!(replay.quads[0].border_color.a, 0.6);
    }
    #[test]
    fn invisible_cached_child_retains_operations_for_later_reveal() {
        let mut previous = Scene::default();
        let mut hidden = effect(); hidden.opacity = 0.;
        previous.push_paint_effect(hidden);
        let start = previous.len(); previous.insert_primitive(quad()); let end = previous.len();
        previous.pop_paint_effect();
        assert!(previous.quads.is_empty());
        let mut next = Scene::default(); next.push_paint_effect(effect());
        next.replay(start..end, &previous); next.pop_paint_effect();
        assert_eq!(next.quads.len(), 2);
    }
    #[test]
    fn visible_path_triangles_preserve_crossings_and_antialias_margin() {
        let mut path=Path::new(crate::point(crate::px(0.),crate::px(0.)));
        for vertices in [
            [(2.,2.),(8.,2.),(2.,8.)],
            [(80.,80.),(90.,80.),(80.,90.)],
            [(-0.8,5.),(-0.2,8.),(-0.5,9.)],
            [(-20.,10.),(40.,10.),(10.,40.)],
        ] {
            let v=vertices.map(|(x,y)| crate::point(crate::px(x),crate::px(y)));
            path.push_triangle((v[0],v[1],v[2]),(point(0.,1.),point(0.,1.),point(0.,1.)));
        }
        let mut path=path.scale(1.);
        path.content_mask.bounds=rect(0.,0.,20.,20.);
        assert_eq!(path.visible_triangles().count(),3);
        path.content_mask.bounds=rect(-100.,-100.,300.,300.);
        assert_eq!(path.visible_triangles().count(),4);
        assert_eq!(path.visible_triangles_in(rect(0.,0.,20.,20.)).count(),3);
        path.content_mask.bounds=rect(1000.,1000.,20.,20.);
        assert_eq!(path.visible_triangles().count(),0);
    }

    #[test]
    fn paint_effect_clipped_paths_only_upload_relevant_triangles() {
        let mut path = Path::new(crate::point(crate::px(0.), crate::px(0.)));
        for start in [0., 80.] {
            path.push_triangle((crate::point(crate::px(start),crate::px(start)),crate::point(crate::px(start+10.),crate::px(start)),crate::point(crate::px(start),crate::px(start+10.))), (point(0.,1.),point(0.,1.),point(0.,1.)));
        }
        path.content_mask.bounds = path.bounds;
        let effect = PaintEffect::new(point(ScaledPixels(0.),ScaledPixels(0.)), 1., vec![rect(0.,0.,20.,20.),rect(80.,80.,20.,20.)]);
        let paths = effect.apply(Primitive::Path(path.scale(1.)));
        assert_eq!(paths.len(), 2);
        for path in paths { let Primitive::Path(path) = path else { unreachable!() }; assert_eq!(path.vertices.len(),3); }
    }
    #[test]
    fn paint_effect_indexes_rows_without_changing_clipping() {
        let bands: Vec<_> = (0..1000).map(|y| rect(0., y as f32, 100., 1.)).collect();
        let effect = PaintEffect::new(point(ScaledPixels(0.),ScaledPixels(0.)), 1., bands);
        assert_eq!(effect.relevant_clips(rect(10.,400.,20.,12.)).len(),12);
        let mut full_scan = effect.clone(); full_scan.ordered_rows = false;
        let mut input = quad(); input.bounds = rect(10.,400.,20.,12.); input.content_mask.bounds=input.bounds;
        let a=effect.apply(input.into()); let b=full_scan.apply(input.into());
        assert_eq!(a.len(),b.len());
        for (a,b) in a.iter().zip(&b) { assert_eq!(a.bounds(),b.bounds()); assert_eq!(a.content_mask().bounds,b.content_mask().bounds); }
        let reversed = PaintEffect::new(effect.origin,1.,effect.clips.iter().rev().copied().collect());
        assert!(!reversed.ordered_rows);
        assert_eq!(reversed.relevant_clips(input.bounds).len(),1000);
    }
    #[test]
    fn paint_effect_is_scoped() {
        let mut scene = Scene::default(); scene.push_paint_effect(effect()); scene.insert_primitive(quad()); scene.pop_paint_effect(); scene.insert_primitive(quad());
        assert_eq!(scene.quads.last().unwrap().bounds, rect(0., 0., 100., 100.));
    }
}

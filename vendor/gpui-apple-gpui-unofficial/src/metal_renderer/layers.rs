//! Immutable content textures with independently updated contour masks. The
//! command buffer retains resources while the bounded cache owns future reuse.
use super::*;
use gpui::{RetainedContent, RetainedFramePlan, RetainedLayer, RetainedLayerKey};
use std::collections::HashMap;

#[cfg(test)]
mod tests;

const MAX_LAYERS: usize = 8;
const MAX_BYTES: u64 = 128 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    viewport: [f32; 2],
    translation: [f32; 2],
    scale: f32,
    opacity: f32,
    has_mask: u32,
    pad: u32,
    clip: [f32; 4],
}

struct CachedLayer {
    content: Arc<RetainedContent>,
    texture: metal::Texture,
    mask_texture: Option<metal::Texture>,
    mask: Option<Arc<Path<ScaledPixels>>>,
    used: u64,
    bytes: u64,
}

#[derive(Default)]
pub(super) struct LayerCache {
    entries: HashMap<RetainedLayerKey, CachedLayer>,
    epoch: u64,
    size: Option<Size<DevicePixels>>,
    plan: RetainedFramePlan,
    pending_bytes: u64,
    pending_layers: usize,
}

impl LayerCache {
    pub(super) fn begin_frame(&mut self, size: Size<DevicePixels>, scene: &Scene) {
        if self.size != Some(size) {
            self.entries.clear();
            self.size = Some(size);
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.plan.reset(scene, MAX_LAYERS);
        self.pending_bytes = 0;
        self.pending_layers = 0;
    }

    fn reserve(&mut self, bytes: u64, count: usize) {
        while self.entries.len() + self.pending_layers + count > MAX_LAYERS
            || self.entries.values().map(|v| v.bytes).sum::<u64>() + self.pending_bytes + bytes > MAX_BYTES
        {
            let Some(id) = self
                .entries
                .iter()
                .filter(|(key, v)| v.used != self.epoch && !self.plan.contains(key))
                .min_by_key(|(_, v)| v.used)
                .map(|(id, _)| *id)
            else {
                break;
            };
            self.entries.remove(&id);
        }
    }

    pub(super) fn trim(&mut self) {
        self.reserve(0, 0);
    }
}

/// Reject the complete frame before allocating or hiding any ordinary drawing.
pub(super) fn admitted(scene: &Scene, size: Size<DevicePixels>) -> bool {
    let mut plan = RetainedFramePlan::default();
    plan.reset(scene, MAX_LAYERS);
    plan.fits([size.width.0.max(0) as u32, size.height.0.max(0) as u32], 4, MAX_BYTES)
}

pub(super) struct PreparedLayer {
    texture: metal::Texture,
    mask: Option<metal::Texture>,
    params: Params,
}

impl MetalRenderer {
    pub(super) fn prepare_retained(
        &mut self,
        scene: &Scene,
        writer: &mut InstanceBufferWriter,
        size: Size<DevicePixels>,
        command: &metal::CommandBufferRef,
    ) -> Result<Vec<PreparedLayer>> {
        let mut prepared = Vec::with_capacity(scene.retained_layers.len());
        for layer in &scene.retained_layers {
            let key = self.retained.plan.next_key(layer);
            let mask = layer
                .effects
                .last()
                .and_then(|effect| effect.gpu_mask.clone());
            let mut old = self.retained.entries.remove(&key).filter(|old| {
                old.used != self.retained.epoch && old.mask_texture.is_some() == mask.is_some()
            });
            if old.is_none() {
                // Scrolling changes drawing versions without needing a new
                // allocation. Never recycle another version still in this frame.
                let reusable = self.retained.entries.iter().filter(|(key, entry)| {
                    entry.content.id == layer.content.id && entry.used != self.retained.epoch
                        && entry.mask_texture.is_some() == mask.is_some() && !self.retained.plan.contains(key)
                }).min_by_key(|(_, entry)| entry.used).map(|(key, _)| *key);
                old = reusable.and_then(|key| self.retained.entries.remove(&key));
            }
            let redraw = old
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(&old.content, &layer.content));
            let remask = old.as_ref().is_none_or(|old| old.mask != mask);
            let bytes =
                size.width.0 as u64 * size.height.0 as u64 * 4 * (1 + u64::from(mask.is_some()));
            self.retained.reserve(bytes, 1);
            self.retained.pending_bytes += bytes;
            self.retained.pending_layers += 1;
            let mut entry = old.unwrap_or_else(|| CachedLayer {
                content: layer.content.clone(),
                texture: texture(&self.device, size),
                mask_texture: mask.as_ref().map(|_| texture(&self.device, size)),
                mask: None,
                used: self.retained.epoch,
                bytes,
            });
            if redraw {
                self.encode_scene(
                    &layer.content.scene,
                    writer,
                    &entry.texture,
                    size,
                    command,
                    0.,
                )?;
                self.render_stats.content_redraws += 1;
            }
            if remask && let (Some(mask), Some(target)) = (&mask, &entry.mask_texture) {
                self.draw_paths_to_texture(
                    std::slice::from_ref(mask.as_ref()),
                    writer,
                    size,
                    command,
                    target,
                )?;
                self.render_stats.mask_redraws += 1;
            }
            self.render_stats.retained_layers += 1;
            prepared.push(PreparedLayer {
                texture: entry.texture.clone(),
                mask: entry.mask_texture.clone(),
                params: params(layer, size),
            });
            entry.content = layer.content.clone();
            entry.mask = mask;
            entry.used = self.retained.epoch;
            self.retained.pending_bytes -= bytes;
            self.retained.pending_layers -= 1;
            self.retained.entries.insert(key, entry);
        }
        Ok(prepared)
    }

    pub(super) fn draw_retained(
        &self,
        layer: &PreparedLayer,
        encoder: &metal::RenderCommandEncoderRef,
    ) {
        encoder.set_render_pipeline_state(&self.retained_pipeline_state);
        let pointer = &layer.params as *const Params as *const c_void;
        encoder.set_vertex_bytes(0, mem::size_of::<Params>() as u64, pointer);
        encoder.set_fragment_bytes(0, mem::size_of::<Params>() as u64, pointer);
        encoder.set_fragment_texture(0, Some(&layer.texture));
        encoder.set_fragment_texture(1, Some(layer.mask.as_ref().unwrap_or(&layer.texture)));
        encoder.draw_primitives(metal::MTLPrimitiveType::TriangleStrip, 0, 4);
    }
}

fn texture(device: &metal::DeviceRef, size: Size<DevicePixels>) -> metal::Texture {
    let descriptor = metal::TextureDescriptor::new();
    descriptor.set_width(size.width.0 as u64);
    descriptor.set_height(size.height.0 as u64);
    descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    descriptor.set_storage_mode(metal::MTLStorageMode::Private);
    descriptor.set_usage(metal::MTLTextureUsage::RenderTarget | metal::MTLTextureUsage::ShaderRead);
    device.new_texture(&descriptor)
}

fn params(layer: &RetainedLayer, size: Size<DevicePixels>) -> Params {
    let viewport = [size.width.0 as f32, size.height.0 as f32];
    let mut params = Params {
        viewport,
        translation: [0.; 2],
        scale: 1.,
        opacity: 1.,
        has_mask: 0,
        pad: 0,
        clip: [0., 0., viewport[0], viewport[1]],
    };
    if let Some(effect) = layer.effects.last() {
        params.scale = effect.scale;
        params.opacity = effect.opacity;
        params.has_mask = u32::from(effect.gpu_mask.is_some());
        params.translation = [
            effect.origin.x.0 * (1. - effect.scale) + effect.translation.x.0,
            effect.origin.y.0 * (1. - effect.scale) + effect.translation.y.0,
        ];
        params.clip = effect.clips.first().map_or([0.; 4], |clip| {
            [clip.left().0, clip.top().0, clip.right().0, clip.bottom().0]
        });
    }
    params
}

pub(super) fn pipeline(
    device: &metal::DeviceRef,
    library: &metal::LibraryRef,
) -> metal::RenderPipelineState {
    let descriptor = metal::RenderPipelineDescriptor::new();
    descriptor.set_label("retained content and contour composition");
    let vertex = library
        .get_function("retained_vertex", None)
        .expect("retained vertex shader");
    let fragment = library
        .get_function("retained_fragment", None)
        .expect("retained fragment shader");
    descriptor.set_vertex_function(Some(&vertex));
    descriptor.set_fragment_function(Some(&fragment));
    let color = descriptor.color_attachments().object_at(0).unwrap();
    color.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    color.set_blending_enabled(true);
    color.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
    color.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    color.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    color.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    device
        .new_render_pipeline_state(&descriptor)
        .expect("retained pipeline")
}

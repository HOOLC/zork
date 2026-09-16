//! Offscreen color layers and dynamic contour masks share the normal GPUI
//! renderer. Cached pixels are premultiplied, so composition uses One blending.
use super::*;
use gpui::{RetainedContent, RetainedFramePlan, RetainedLayer, RetainedLayerKey};
use std::collections::HashMap;

const MAX_LAYERS: usize = 8;
const MAX_BYTES: u64 = 128 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    viewport: [f32; 2],
    translation: [f32; 2],
    scale: f32,
    opacity: f32,
    has_mask: u32,
    pad: u32,
    clip: [f32; 4],
}
pub(super) struct LayerPipeline {
    pub pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}
pub(super) struct CachedLayer {
    content: Arc<RetainedContent>,
    size: [u32; 2],
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    _mask_texture: Option<wgpu::Texture>,
    mask_view: wgpu::TextureView,
    mask: Option<Arc<Path<ScaledPixels>>>,
    uniform: wgpu::Buffer,
    pub binding: wgpu::BindGroup,
    used: u64,
    bytes: u64,
}
struct MaskTarget {
    size: [u32; 2],
    samples: u32,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}
#[derive(Default)]
pub(super) struct LayerCache {
    pub entries: HashMap<RetainedLayerKey, CachedLayer>,
    pub epoch: u64,
    plan: RetainedFramePlan,
    mask_target: Option<MaskTarget>,
    pending_bytes: u64,
    pending_layers: usize,
}
impl LayerCache {
    pub(super) fn begin_frame(&mut self, scene: &Scene) {
        self.epoch = self.epoch.wrapping_add(1);
        self.plan.reset(scene, MAX_LAYERS);
        self.pending_bytes = 0;
        self.pending_layers = 0;
    }
    fn reserve(&mut self, bytes: u64, count: usize) {
        while self.entries.len() + self.pending_layers + count > MAX_LAYERS
            || self.entries.values().map(|entry| entry.bytes).sum::<u64>() + self.pending_bytes + bytes > MAX_BYTES
        {
            let Some(key) = self.entries.iter()
                .filter(|(key, entry)| entry.used != self.epoch && !self.plan.contains(key))
                .min_by_key(|(_, entry)| entry.used).map(|(key, _)| *key)
            else { break; };
            self.entries.remove(&key);
        }
    }
}
pub(super) fn admitted(scene: &Scene, size: [u32; 2], bytes_per_pixel: u32) -> bool {
    let mut plan = RetainedFramePlan::default();
    plan.reset(scene, MAX_LAYERS);
    plan.fits(size, bytes_per_pixel, MAX_BYTES)
}
impl LayerPipeline {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("retained layer layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<Params>() as u64),
                    },
                    count: None,
                },
                texture(1),
                texture(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("retained layer pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("retained layer shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("layers.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("retained layer composition"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("retained layer sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
        }
    }
}
impl WgpuRenderer {
    pub(super) fn prepare_retained(
        &mut self,
        layers: &[RetainedLayer],
        encoder: &mut wgpu::CommandEncoder,
        instance_offset: &mut u64,
    ) -> Result<Vec<wgpu::BindGroup>> {
        let mut bindings = Vec::with_capacity(layers.len());
        for layer in layers {
            let key = self.resources_mut().layer_cache.plan.next_key(layer);
            let dimensions = [self.surface_config.width, self.surface_config.height];
            let effect = layer.effects.last();
            anyhow::ensure!(
                layer.effects.len() <= 1,
                "nested paint effects require the ordinary drawing path"
            );
            let mask = effect.and_then(|e| e.gpu_mask.clone());
            let mut old = self.resources_mut().layer_cache.entries.remove(&key);
            let compatible = old.as_ref().is_some_and(|v| {
                v.size == dimensions
                    && v.mask.is_some() == mask.is_some()
                    && v.used != self.resources().layer_cache.epoch
            });
            if !compatible {
                old = None;
            }
            if old.is_none() {
                let cache = &self.resources().layer_cache;
                let reusable = cache.entries.iter().filter(|(key, entry)| {
                    entry.content.id == layer.content.id && entry.size == dimensions
                        && entry.mask.is_some() == mask.is_some() && entry.used != cache.epoch
                        && !cache.plan.contains(key)
                }).min_by_key(|(_, entry)| entry.used).map(|(key, _)| *key);
                old = reusable.and_then(|key| self.resources_mut().layer_cache.entries.remove(&key));
            }
            let redraw = old
                .as_ref()
                .is_none_or(|v| !Arc::ptr_eq(&v.content, &layer.content));
            let remask = old.as_ref().is_none_or(|v| v.mask != mask);
            let bytes = u64::from(dimensions[0]) * u64::from(dimensions[1])
                * u64::from(self.surface_config.format.block_copy_size(None).unwrap_or(4))
                * if mask.is_some() { 2 } else { 1 };
            self.resources_mut().layer_cache.reserve(bytes, 1);
            self.resources_mut().layer_cache.pending_bytes += bytes;
            self.resources_mut().layer_cache.pending_layers += 1;
            let mut entry =
                old.unwrap_or_else(|| self.create_cached_layer(layer, dimensions, mask.clone()));
            if redraw {
                self.record_scene(
                    &layer.content.scene,
                    &entry.view,
                    None,
                    encoder,
                    instance_offset,
                    true,
                )?;
            }
            if remask && let Some(mask) = &mask {
                self.record_retained_mask(mask, &entry.mask_view, encoder, instance_offset)?;
            }
            let mut params = Params {
                viewport: [dimensions[0] as f32, dimensions[1] as f32],
                translation: [0., 0.],
                scale: 1.,
                opacity: 1.,
                has_mask: u32::from(mask.is_some()),
                pad: 0,
                clip: [0., 0., dimensions[0] as f32, dimensions[1] as f32],
            };
            if let Some(effect) = effect {
                params.scale = effect.scale;
                params.opacity = effect.opacity;
                params.translation = [
                    effect.origin.x.0 * (1. - effect.scale) + effect.translation.x.0,
                    effect.origin.y.0 * (1. - effect.scale) + effect.translation.y.0,
                ];
                if let Some(clip) = effect.clips.first() {
                    params.clip = [clip.left().0, clip.top().0, clip.right().0, clip.bottom().0];
                } else {
                    params.clip = [0.; 4];
                }
            }
            self.resources()
                .queue
                .write_buffer(&entry.uniform, 0, bytemuck::bytes_of(&params));
            entry.content = layer.content.clone();
            entry.mask = mask;
            entry.used = self.resources().layer_cache.epoch;
            bindings.push(entry.binding.clone());
            self.resources_mut().layer_cache.pending_bytes -= bytes;
            self.resources_mut().layer_cache.pending_layers -= 1;
            self.resources_mut().layer_cache.entries.insert(key, entry);
        }
        // Entries own their GPU resources and disappear together on recovery.
        // Keep only a small number of hidden layers for reopening.
        self.resources_mut().layer_cache.reserve(0, 0);
        Ok(bindings)
    }
    fn record_retained_mask(
        &mut self,
        mask: &Path<ScaledPixels>,
        target: &wgpu::TextureView,
        encoder: &mut wgpu::CommandEncoder,
        instance_offset: &mut u64,
    ) -> Result<()> {
        let bounds = mask.clipped_bounds();
        let (binding, vertex_count) = self.path_binding("retained mask vertices", instance_offset,
            std::iter::once((mask, Point::default(), bounds)))?;
        let size = [self.surface_config.width, self.surface_config.height];
        let samples = self.rendering_params.path_sample_count;
        if samples > 1
            && self
                .resources()
                .layer_cache
                .mask_target
                .as_ref()
                .is_none_or(|mask| mask.size != size || mask.samples != samples)
        {
            let (texture, view) = Self::create_msaa_if_needed(
                &self.resources().device,
                self.surface_config.format,
                size[0],
                size[1],
                samples,
            )
            .unwrap();
            self.resources_mut().layer_cache.mask_target = Some(MaskTarget {
                size,
                samples,
                _texture: texture,
                view,
            });
        }
        let resources = self.resources();
        let (view, resolve) = if samples > 1 {
            (
                &resources.layer_cache.mask_target.as_ref().unwrap().view,
                Some(target),
            )
        } else {
            (target, None)
        };
        // A white contour already produces exactly the coverage alpha needed
        // by composition. Resolve directly into the mask, avoiding an
        // intermediate path texture and a second full-window sprite pass.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("retained contour mask"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: resolve,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: if resolve.is_some() {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        if let Some(binding) = binding {
            let left = bounds.left().0.floor().max(0.) as u32;
            let top = bounds.top().0.floor().max(0.) as u32;
            let right = (bounds.right().0.ceil().max(0.) as u32).min(size[0]);
            let bottom = (bounds.bottom().0.ceil().max(0.) as u32).min(size[1]);
            if right <= left || bottom <= top {
                return Ok(());
            }
            pass.set_scissor_rect(left, top, right - left, bottom - top);
            pass.set_pipeline(&resources.pipelines.path_rasterization);
            pass.set_bind_group(0, &resources.globals_bind_group, &[]);
            pass.set_bind_group(1, &binding.bind_group, &[]);
            pass.draw(
                binding.first_instance..binding.first_instance + vertex_count,
                0..1,
            );
            // The GLES MSAA resolve blit observes the last scissor. Resolve
            // the cleared exterior too, otherwise a shrinking mask retains
            // coverage from its larger previous frame.
            pass.set_scissor_rect(0, 0, size[0], size[1]);
        }
        Ok(())
    }

    fn create_cached_layer(
        &self,
        layer: &RetainedLayer,
        size: [u32; 2],
        mask: Option<Arc<Path<ScaledPixels>>>,
    ) -> CachedLayer {
        let resources = self.resources();
        let make = |label, dimensions: [u32; 2]| {
            resources.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: dimensions[0],
                    height: dimensions[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.surface_config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let texture = make("retained color", size);
        let view = texture.create_view(&Default::default());
        let mask_texture = mask.as_ref().map(|_| make("retained mask", size));
        let mask_view = mask_texture.as_ref().map_or_else(|| view.clone(), |texture| texture.create_view(&Default::default()));
        let uniform = resources.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("retained composite parameters"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = resources
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("retained composite binding"),
                layout: &resources.layer_pipeline.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&mask_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&resources.layer_pipeline.sampler),
                    },
                ],
            });
        let bytes = u64::from(size[0])
            * u64::from(size[1])
            * u64::from(
                self.surface_config
                    .format
                    .block_copy_size(None)
                    .unwrap_or(4),
            )
            * if mask.is_some() { 2 } else { 1 };
        CachedLayer {
            bytes,
            content: layer.content.clone(),
            size,
            _texture: texture,
            view,
            _mask_texture: mask_texture,
            mask_view,
            mask,
            uniform,
            binding,
            used: resources.layer_cache.epoch,
        }
    }
}

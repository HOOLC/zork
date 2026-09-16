//! First-window Metal resources can be prepared before AppKit creates a window.
use super::*;
use std::thread::JoinHandle;

#[cfg(test)]
mod tests;

static PREPARATION: Mutex<Option<JoinHandle<Resources>>> = Mutex::new(None);

pub(super) fn prepare() {
    let mut pending = PREPARATION.lock();
    if pending.is_some() {
        return;
    }
    match std::thread::Builder::new()
        .name("gpui-metal-startup".into())
        .spawn(|| objc::rc::autoreleasepool(Resources::new))
    {
        Ok(worker) => *pending = Some(worker),
        Err(error) => log::warn!("could not prepare Metal resources in background: {error}"),
    }
}

pub(super) fn take() -> Resources {
    let pending = PREPARATION.lock().take();
    match pending {
        Some(worker) => worker
            .join()
            .unwrap_or_else(|error| std::panic::resume_unwind(error)),
        None => Resources::new(),
    }
}

pub(super) struct Resources {
    pub(super) device: metal::Device,
    pub(super) is_apple_gpu: bool,
    pub(super) is_unified_memory: bool,
    pub(super) unit_vertices: metal::Buffer,
    pub(super) command_queue: metal::CommandQueue,
    pub(super) sprite_atlas: Arc<MetalAtlas>,
    pub(super) paths_rasterization_pipeline_state: metal::RenderPipelineState,
    pub(super) path_sprites_pipeline_state: metal::RenderPipelineState,
    pub(super) shadows_pipeline_state: metal::RenderPipelineState,
    pub(super) quads_pipeline_state: metal::RenderPipelineState,
    pub(super) underlines_pipeline_state: metal::RenderPipelineState,
    pub(super) monochrome_sprites_pipeline_state: metal::RenderPipelineState,
    pub(super) polychrome_sprites_pipeline_state: metal::RenderPipelineState,
    pub(super) surfaces_pipeline_state: metal::RenderPipelineState,
    pub(super) retained_pipeline_state: metal::RenderPipelineState,
}

impl Resources {
    pub(super) fn new() -> Self {
        gpui::observe_startup("gpui.metal_prepare_begin");
        let device = MetalRenderer::create_device();
        gpui::observe_startup("gpui.metal_device_ready");
        #[cfg(feature = "runtime_shaders")]
        let library = device
            .new_library_with_source(&SHADERS_SOURCE_FILE, &metal::CompileOptions::new())
            .expect("error building metal library");
        #[cfg(not(feature = "runtime_shaders"))]
        let library = device
            .new_library_with_data(SHADERS_METALLIB)
            .expect("error building metal library");

        fn to_float2_bits(point: PointF) -> u64 {
            let mut output = point.y.to_bits() as u64;
            output <<= 32;
            output |= point.x.to_bits() as u64;
            output
        }

        // Shared memory can be used only if CPU and GPU share the same memory space.
        // https://developer.apple.com/documentation/metal/setting-resource-storage-modes
        let is_unified_memory = device.has_unified_memory();
        // Apple GPU families support memoryless textures, which can significantly reduce
        // memory usage by keeping render targets in on-chip tile memory instead of
        // allocating backing store in system memory.
        // https://developer.apple.com/documentation/metal/mtlgpufamily
        let is_apple_gpu = device.supports_family(MTLGPUFamily::Apple1);

        let unit_vertices = [
            to_float2_bits(point(0., 0.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(0., 1.)),
            to_float2_bits(point(1., 0.)),
            to_float2_bits(point(1., 1.)),
        ];
        let unit_vertices = device.new_buffer_with_data(
            unit_vertices.as_ptr() as *const c_void,
            mem::size_of_val(&unit_vertices) as u64,
            if is_unified_memory {
                MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined
            } else {
                MTLResourceOptions::StorageModeManaged
            },
        );

        let paths_rasterization_pipeline_state = build_path_rasterization_pipeline_state(
            &device,
            &library,
            "paths_rasterization",
            "path_rasterization_vertex",
            "path_rasterization_fragment",
            MTLPixelFormat::BGRA8Unorm,
            PATH_SAMPLE_COUNT,
        );
        let path_sprites_pipeline_state = build_path_sprite_pipeline_state(
            &device,
            &library,
            "path_sprites",
            "path_sprite_vertex",
            "path_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let shadows_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "shadows",
            "shadow_vertex",
            "shadow_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let quads_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "quads",
            "quad_vertex",
            "quad_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let underlines_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "underlines",
            "underline_vertex",
            "underline_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let monochrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "monochrome_sprites",
            "monochrome_sprite_vertex",
            "monochrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let polychrome_sprites_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "polychrome_sprites",
            "polychrome_sprite_vertex",
            "polychrome_sprite_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );
        let surfaces_pipeline_state = build_pipeline_state(
            &device,
            &library,
            "surfaces",
            "surface_vertex",
            "surface_fragment",
            MTLPixelFormat::BGRA8Unorm,
        );

        let retained_pipeline_state = layers::pipeline(&device, &library);
        let command_queue = device.new_command_queue();
        let sprite_atlas = Arc::new(MetalAtlas::new(device.clone(), is_apple_gpu));
        gpui::observe_startup("gpui.metal_prepare_ready");

        Self {
            device,
            is_apple_gpu,
            is_unified_memory,
            unit_vertices,
            command_queue,
            sprite_atlas,
            paths_rasterization_pipeline_state,
            path_sprites_pipeline_state,
            shadows_pipeline_state,
            quads_pipeline_state,
            underlines_pipeline_state,
            monochrome_sprites_pipeline_state,
            polychrome_sprites_pipeline_state,
            surfaces_pipeline_state,
            retained_pipeline_state,
        }
    }
}

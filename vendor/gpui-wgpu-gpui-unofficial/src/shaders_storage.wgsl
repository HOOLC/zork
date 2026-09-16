// Storage-buffer instance transport. Concatenated after `shaders.wgsl` on backends
// with storage buffer support. Paths share color/clip records; the WebGL2
// counterpart keeps complete vertex records in its texture transport.
//
// All buffers share `@group(1) @binding(0)` because each pipeline binds only the
// buffer its own entry points read.

@group(1) @binding(0) var<storage, read> b_quads: array<Quad>;
@group(1) @binding(0) var<storage, read> b_shadows: array<Shadow>;
@group(1) @binding(0) var<storage, read> b_path_words: array<u32>;
@group(1) @binding(0) var<storage, read> b_path_sprites: array<PathSprite>;
@group(1) @binding(0) var<storage, read> b_underlines: array<Underline>;
@group(1) @binding(0) var<storage, read> b_mono_sprites: array<MonochromeSprite>;
@group(1) @binding(0) var<storage, read> b_poly_sprites: array<PolychromeSprite>;

fn load_quad(instance_id: u32) -> Quad {
    return b_quads[instance_id];
}

fn load_shadow(instance_id: u32) -> Shadow {
    return b_shadows[instance_id];
}

fn path_float(word: u32) -> f32 {
    return bitcast<f32>(b_path_words[word]);
}

fn path_hsla(word: u32) -> Hsla {
    return Hsla(path_float(word), path_float(word + 1u), path_float(word + 2u), path_float(word + 3u));
}

fn load_path_vertex(vertex_id: u32) -> PathRasterizationVertex {
    let base = 1u + vertex_id * 6u;
    let paint_offset = b_path_words[0] + b_path_words[base + 4u];
    let background = Background(
        b_path_words[paint_offset], b_path_words[paint_offset + 1u],
        path_hsla(paint_offset + 2u), path_float(paint_offset + 6u),
        array<LinearColorStop, 2>(
            LinearColorStop(path_hsla(paint_offset + 7u), path_float(paint_offset + 11u)),
            LinearColorStop(path_hsla(paint_offset + 12u), path_float(paint_offset + 16u)),
        ),
        b_path_words[paint_offset + 17u],
    );
    return PathRasterizationVertex(
        vec2<f32>(path_float(base), path_float(base + 1u)),
        vec2<f32>(path_float(base + 2u), path_float(base + 3u)),
        background,
        Bounds(
            vec2<f32>(path_float(paint_offset + 18u), path_float(paint_offset + 19u)),
            vec2<f32>(path_float(paint_offset + 20u), path_float(paint_offset + 21u)),
        ),
    );
}

fn load_path_sprite(instance_id: u32) -> PathSprite {
    return b_path_sprites[instance_id];
}

fn load_underline(instance_id: u32) -> Underline {
    return b_underlines[instance_id];
}

fn load_mono_sprite(instance_id: u32) -> MonochromeSprite {
    return b_mono_sprites[instance_id];
}

fn load_poly_sprite(instance_id: u32) -> PolychromeSprite {
    return b_poly_sprites[instance_id];
}

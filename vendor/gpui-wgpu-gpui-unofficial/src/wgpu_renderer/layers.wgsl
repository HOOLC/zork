struct Params {
    viewport: vec2<f32>,
    translation: vec2<f32>,
    scale: f32,
    opacity: f32,
    has_mask: u32,
    pad: u32,
    clip: vec4<f32>,
}
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var mask: texture_2d<f32>;
@group(0) @binding(3) var image_sampler: sampler;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let corner = vec2<f32>(f32(index & 1u), 0.5 * f32(index & 2u));
    let end = params.translation + params.viewport * params.scale;
    let low = max(vec2<f32>(0.0), max(params.clip.xy, min(params.translation, end)));
    let high = min(params.viewport, min(params.clip.zw, max(params.translation, end)));
    let position = low + corner * max(high - low, vec2<f32>(0.0));
    var uv = vec2<f32>(0.0);
    if (params.scale != 0.0) { uv = (position - params.translation) / (params.viewport * params.scale); }
    var out: Vertex;
    out.position = vec4<f32>(position.x / params.viewport.x * 2.0 - 1.0, 1.0 - position.y / params.viewport.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}
@fragment fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    let color = textureSample(image, image_sampler, input.uv);
    let coverage = textureSample(mask, image_sampler, input.position.xy / params.viewport).a;
    let p = input.position.xy;
    if (p.x < params.clip.x || p.y < params.clip.y || p.x >= params.clip.z || p.y >= params.clip.w) { return vec4<f32>(0.0); }
    return color * params.opacity * select(1.0, coverage, (params.has_mask & 1u) != 0u);
}

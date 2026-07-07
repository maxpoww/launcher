// Single rounded card (all four corners), drawn as a fullscreen
// triangle with an SDF in the fragment shader. The card may extend
// below the surface edge; the framebuffer clips that part. Output is
// premultiplied alpha over a transparent surface.

struct Params {
    rect_min: vec2<f32>,  // top-left corner, pixels
    rect_max: vec2<f32>,  // bottom-right corner, pixels (may be off-surface)
    color: vec4<f32>,     // straight-alpha background color
    radius: f32,          // corner radius, pixels
    alpha: f32,           // animation opacity multiplier
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    // Fullscreen triangle: (-1,-1) (3,-1) (-1,3).
    let x = f32(i32(vi & 1u) * 4 - 1);
    let y = f32(i32(vi >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let center = (params.rect_min + params.rect_max) * 0.5;
    let half = (params.rect_max - params.rect_min) * 0.5;
    let p = pos.xy - center;

    let r = params.radius;
    let q = abs(p) - half + vec2<f32>(r, r);
    let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;

    // ~1px antialiased edge.
    let coverage = clamp(0.5 - d, 0.0, 1.0);
    let a = params.color.a * params.alpha * coverage;
    return vec4<f32>(params.color.rgb * a, a);
}

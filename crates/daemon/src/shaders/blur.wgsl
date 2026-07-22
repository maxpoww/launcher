// Separable Gaussian blur, run as two fullscreen passes (horizontal then
// vertical) to frost the box's backdrop. Each pass samples the source
// texture along one axis with Gaussian weights; two 1-D passes give a
// smooth 2-D blur far more cheaply (and more smoothly) than one 2-D tap
// grid. Premultiplied RGBA is blurred directly (a valid linear op).
//
// The texel step comes from `textureDimensions`, so no size uniform is
// needed; the two entry points hardcode the axis.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var out: VsOut;
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

// Kernel half-width in source texels. Larger = stronger frost (and more
// taps). Reach is RADIUS texels each side.
const RADIUS: i32 = 14;
const SIGMA: f32 = 7.0;

fn blur(uv: vec2<f32>, dir: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(src));
    let texel = dir / dims;
    var acc = vec4<f32>(0.0);
    var wsum = 0.0;
    for (var i: i32 = -RADIUS; i <= RADIUS; i++) {
        let fi = f32(i);
        let w = exp(-(fi * fi) / (2.0 * SIGMA * SIGMA));
        acc += textureSampleLevel(src, src_sampler, uv + texel * fi, 0.0) * w;
        wsum += w;
    }
    return acc / wsum;
}

@fragment
fn fs_horizontal(in: VsOut) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(1.0, 0.0));
}

@fragment
fn fs_vertical(in: VsOut) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(0.0, 1.0));
}

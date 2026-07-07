// Instanced icon quads sampling one layer of the app-icon texture
// array. Icon pixels are premultiplied RGBA; the animation alpha
// multiplies all channels so icons fade with the card.

struct Globals {
    screen: vec2<f32>,   // surface size, pixels
    alpha: f32,          // animation opacity multiplier
    _pad: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var icons: texture_2d_array<f32>;
@group(1) @binding(1) var icon_sampler: sampler;

struct Instance {
    @location(0) rect_min: vec2<f32>,  // top-left, pixels
    @location(1) rect_max: vec2<f32>,  // bottom-right, pixels
    @location(2) layer: u32,           // texture array layer
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = mix(inst.rect_min, inst.rect_max, corner);
    var out: VsOut;
    let ndc = px / globals.screen * 2.0 - 1.0;
    out.pos = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = corner;
    out.layer = inst.layer;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(icons, icon_sampler, in.uv, i32(in.layer));
    return texel * globals.alpha;
}

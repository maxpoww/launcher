// Instanced icon quads sampling one layer of the app-icon texture
// array. Icon pixels are premultiplied RGBA; the animation alpha
// multiplies all channels so icons fade with the card.

// Prefix of the shared Globals uniform (the rect/shadow shaders declare
// the rest — a shorter struct over the same buffer is fine).
struct Globals {
    screen: vec2<f32>,   // surface size, logical pixels
    alpha: f32,          // animation opacity multiplier
    time: f32,
    cursor: vec2<f32>,
    squircle: f32,       // icon corner superellipse exponent; <= 0 = off
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
    // Squircle (superellipse) corner mask: |x|^n + |y|^n = 1 in the tile's
    // [-1,1] space. This only trims the extreme corners, so full-bleed
    // square icons pick up macOS-style rounded corners while already-round
    // or padded icons (transparent corners) are untouched. fwidth gives a
    // ~1px anti-aliased edge that stays crisp at every icon size.
    var mask = 1.0;
    let n = globals.squircle;
    if (n > 0.0) {
        let p = in.uv * 2.0 - vec2<f32>(1.0);
        let e = pow(abs(p.x), n) + pow(abs(p.y), n);
        let w = max(fwidth(e), 1e-5);
        mask = 1.0 - smoothstep(1.0 - w, 1.0 + w, e);
    }
    return texel * globals.alpha * mask;
}

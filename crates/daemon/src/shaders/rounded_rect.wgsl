// Instanced rounded rectangles (card background, hover highlights),
// SDF-shaded per fragment. Quads may extend below the surface edge;
// the framebuffer clips that part. Output is premultiplied alpha over
// a transparent surface.

struct Globals {
    screen: vec2<f32>,   // surface size, pixels
    alpha: f32,          // animation opacity multiplier
    _pad: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect_min: vec2<f32>,  // top-left, pixels
    @location(1) rect_max: vec2<f32>,  // bottom-right, pixels
    @location(2) color: vec4<f32>,     // straight-alpha fill color
    @location(3) radius: f32,          // corner radius, pixels
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) px: vec2<f32>,               // fragment position, pixels
    @location(1) @interpolate(flat) rect_min: vec2<f32>,
    @location(2) @interpolate(flat) rect_max: vec2<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) radius: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    // Triangle-strip corner (0,0) (1,0) (0,1) (1,1), expanded 1px for AA.
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = mix(inst.rect_min - vec2<f32>(1.0), inst.rect_max + vec2<f32>(1.0), corner);
    var out: VsOut;
    let ndc = px / globals.screen * 2.0 - 1.0;
    out.pos = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.px = px;
    out.rect_min = inst.rect_min;
    out.rect_max = inst.rect_max;
    out.color = inst.color;
    out.radius = inst.radius;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let center = (in.rect_min + in.rect_max) * 0.5;
    let half = (in.rect_max - in.rect_min) * 0.5;
    let p = in.px - center;

    let r = min(in.radius, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r, r);
    let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;

    // ~1px antialiased edge.
    let coverage = clamp(0.5 - d, 0.0, 1.0);
    let a = in.color.a * globals.alpha * coverage;
    return vec4<f32>(in.color.rgb * a, a);
}

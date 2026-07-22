// Frosted-glass backdrop for the open box: a rounded quad over the box
// region that samples the already-blurred scene texture (a separable
// Gaussian, see blur.wgsl), so the box shows a smooth, soft image of the
// grid icons behind it. The box panel (a translucent glass rect) is drawn
// over this, then the members.
//
// The blurred texture holds premultiplied RGBA; we un-premultiply to an
// opaque colour so the box fully replaces the sharp base underneath it.

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_samp: sampler;

struct Instance {
    @location(0) rect_min: vec2<f32>,  // box rect, logical px
    @location(1) rect_max: vec2<f32>,
    @location(2) radius:   f32,        // corner radius, logical px
    @location(3) screen:   vec2<f32>,  // logical surface size
};

struct VsOut {
    @builtin(position)              pos:      vec4<f32>,
    @location(0)                    px:       vec2<f32>,
    @location(1) @interpolate(flat) rect_min: vec2<f32>,
    @location(2) @interpolate(flat) rect_max: vec2<f32>,
    @location(3) @interpolate(flat) radius:   f32,
    @location(4) @interpolate(flat) screen:   vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = mix(inst.rect_min, inst.rect_max, corner);
    var out: VsOut;
    let ndc = px / inst.screen * 2.0 - 1.0;
    out.pos      = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.px       = px;
    out.rect_min = inst.rect_min;
    out.rect_max = inst.rect_max;
    out.radius   = inst.radius;
    out.screen   = inst.screen;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Rounded-rect SDF coverage (matches rounded_rect.wgsl).
    let center = (in.rect_min + in.rect_max) * 0.5;
    let half   = (in.rect_max - in.rect_min) * 0.5;
    let p      = in.px - center;
    let r      = min(in.radius, min(half.x, half.y));
    let q      = abs(p) - half + vec2<f32>(r);
    let d      = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    let coverage = clamp(0.5 - d, 0.0, 1.0);
    if (coverage <= 0.0) {
        discard;
    }

    // Sample the pre-blurred scene at this pixel (uv is resolution-agnostic).
    let uv = in.px / in.screen;
    let blurred  = textureSampleLevel(scene, scene_samp, uv, 0.0); // premultiplied
    let straight = blurred.rgb / max(blurred.a, 1e-4);
    // Opaque inside (alpha = coverage), so it replaces the sharp base; the
    // translucent box panel drawn on top re-tints it. Premultiplied output.
    return vec4<f32>(straight * coverage, coverage);
}

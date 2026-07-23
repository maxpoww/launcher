// Frosted-glass backdrop for the open box: a rounded quad over the box
// region that samples the already-blurred scene texture (a separable
// Gaussian, see blur.wgsl), so the box shows a smooth, soft image of the
// grid icons behind it. The box panel (a translucent glass rect) is drawn
// over this, then the members.
//
// The blurred texture holds premultiplied RGBA and we keep it that way, so
// the box preserves the base's *translucency*: transparent areas (e.g. a
// dock stack floating over the wallpaper) stay transparent and let the
// compositor's blur show through — never forced opaque/black. It replaces
// (rather than composites over) the sharp base via a preceding erase pass
// (`fs_erase`), so the two together are `mix(base, blurred, coverage)`.

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

// Rounded-rect SDF coverage (matches rounded_rect.wgsl).
fn box_coverage(in: VsOut) -> f32 {
    let center = (in.rect_min + in.rect_max) * 0.5;
    let half   = (in.rect_max - in.rect_min) * 0.5;
    let p      = in.px - center;
    let r      = min(in.radius, min(half.x, half.y));
    let q      = abs(p) - half + vec2<f32>(r);
    let d      = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    return clamp(0.5 - d, 0.0, 1.0);
}

// Erase pass: with blend (Zero, OneMinusSrcAlpha) this multiplies the
// destination by (1 - coverage), clearing the box region (AA at the edge)
// so the fill can replace it rather than stack on it.
@fragment
fn fs_erase(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, box_coverage(in));
}

// Fill pass: composite the blurred (premultiplied) scene over the erased
// region. Alpha is preserved, so the box is as translucent as the base was.
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let coverage = box_coverage(in);
    if (coverage <= 0.0) {
        discard;
    }
    let uv = in.px / in.screen;
    let blurred = textureSampleLevel(scene, scene_samp, uv, 0.0); // premultiplied
    return blurred * coverage;
}

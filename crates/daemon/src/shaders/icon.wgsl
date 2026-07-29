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
    thumb_base: f32,     // first texture layer of the thumbnail block; icons
                         // at/above it skip the squircle (file thumbnails
                         // keep their true square corners)
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var icons: texture_2d_array<f32>;
@group(1) @binding(1) var icon_sampler: sampler;

struct Instance {
    @location(0) rect_min: vec2<f32>,  // top-left, pixels
    @location(1) rect_max: vec2<f32>,  // bottom-right, pixels
    @location(2) layer: u32,           // texture array layer
    @location(3) tint: vec4<f32>,      // silhouette tint: rgb + strength (a)
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) @interpolate(flat) tint: vec4<f32>,
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
    out.tint = inst.tint;
    return out;
}

// Signed distance to an axis-aligned box (negative inside).
fn sd_box(p: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = abs(p) - b;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
}
// Anti-aliased fill / stroke coverage from a signed distance; `aa` is one
// pixel in uv units, passed in so no derivatives are taken in here.
fn fill_cov(d: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(-aa, aa, d);
}
fn stroke_cov(d: f32, half: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(half - aa, half + aa, abs(d));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(icons, icon_sampler, in.uv, i32(in.layer));
    // One pixel in uv units (the ghost quad is square) for the SDF edges below.
    let px = max(fwidth(in.uv.x), 1e-5);
    // Squircle (superellipse) corner mask: |x|^n + |y|^n = 1 in the tile's
    // [-1,1] space. This only trims the extreme corners, so full-bleed
    // square icons pick up macOS-style rounded corners while already-round
    // or padded icons (transparent corners) are untouched. fwidth gives a
    // ~1px anti-aliased edge that stays crisp at every icon size.
    var mask = 1.0;
    let n = globals.squircle;
    // Thumbnails (file previews) keep their real square corners — the
    // squircle mask applies only to app/asset icons below `thumb_base`.
    let is_thumb = f32(in.layer) >= globals.thumb_base;
    if (n > 0.0 && !is_thumb) {
        let p = in.uv * 2.0 - vec2<f32>(1.0);
        let e = pow(abs(p.x), n) + pow(abs(p.y), n);
        let w = max(fwidth(e), 1e-5);
        mask = 1.0 - smoothstep(1.0 - w, 1.0 + w, e);
    }
    // Silhouette tint (drag ghost over the uninstall target): blend the
    // colour in *premultiplied* space — `tint.rgb * texel.a` matches the
    // icon's own coverage, so only the opaque pixels redden and transparent
    // corners stay clear. `tint.a` is the blend strength (0 = untinted).
    var a = texel.a;
    var rgb = mix(texel.rgb, in.tint.rgb * a, in.tint.a);
    // Trim the icon's corners to the squircle here — *before* the X — so the
    // X, composited next, is exempt and can spill a little past the squircle.
    rgb = rgb * mask;
    a = a * mask;
    // A red trash can composited over the reddened icon — the "this will be
    // removed" mark, drawn only while the ghost is tinted. Built from SDFs in
    // centred uv space (`q`), y-down, spanning [-0.5, 0.5].
    if (in.tint.a > 0.0) {
        // Overall scale of the trash can within the icon (smaller = tinier).
        let s = 0.68;
        let q = (in.uv - vec2<f32>(0.5)) / s;
        let aa = px / s; // one pixel, in the scaled q-space
        var can = 0.0;
        // Handle nub on top of the lid.
        can = max(can, fill_cov(sd_box(q - vec2<f32>(0.0, -0.30), vec2<f32>(0.075, 0.030)) - 0.02, aa));
        // Lid bar, wider than the body.
        can = max(can, fill_cov(sd_box(q - vec2<f32>(0.0, -0.225), vec2<f32>(0.27, 0.040)) - 0.02, aa));
        // Body: a rounded-rect outline (the bin).
        let body = sd_box(q - vec2<f32>(0.0, 0.09), vec2<f32>(0.175, 0.20)) - 0.03;
        can = max(can, stroke_cov(body, 0.028, aa));
        // Three vertical slats inside the bin.
        can = max(can, fill_cov(sd_box(q - vec2<f32>(-0.085, 0.095), vec2<f32>(0.016, 0.14)), aa));
        can = max(can, fill_cov(sd_box(q - vec2<f32>( 0.000, 0.095), vec2<f32>(0.016, 0.14)), aa));
        can = max(can, fill_cov(sd_box(q - vec2<f32>( 0.085, 0.095), vec2<f32>(0.016, 0.14)), aa));
        // Composite the opaque red mark "over" the icon (premultiplied alpha),
        // so it reads across the icon's transparent gaps.
        let cc = vec3<f32>(0.45, 0.08, 0.06);
        rgb = cc * can + rgb * (1.0 - can);
        a = can + a * (1.0 - can);
    }
    return vec4<f32>(rgb, a) * globals.alpha;
}

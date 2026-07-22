// Instanced soft drop shadow for a rounded rect (the dock). One continuous
// penumbra wraps the whole shape; only the exterior is drawn (nothing under
// the shape, so the glass stays clean). Per-edge strengths are blended by the
// outward direction, so corners transition smoothly between their two
// neighbouring edges with no seams. Output is premultiplied alpha.

struct Globals {
    screen:    vec2<f32>,
    alpha:     f32,
    time:      f32,
    cursor:    vec2<f32>,
    _pad:      vec2<f32>,
    ripples:   array<vec4<f32>, 4>,
    box_waves: array<vec4<f32>, 2>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect_min: vec2<f32>,
    @location(1) rect_max: vec2<f32>,
    @location(2) color:    vec4<f32>,   // rgb + overall strength in .a
    @location(3) radius:   f32,
    @location(4) blur:     f32,
    @location(5) edges:    vec4<f32>,   // per-edge base alpha: top, bottom, left, right
};

struct VsOut {
    @builtin(position)              pos:      vec4<f32>,
    @location(0)                    px:       vec2<f32>,
    @location(1) @interpolate(flat) rect_min: vec2<f32>,
    @location(2) @interpolate(flat) rect_max: vec2<f32>,
    @location(3) @interpolate(flat) color:    vec4<f32>,
    @location(4) @interpolate(flat) radius:   f32,
    @location(5) @interpolate(flat) blur:     f32,
    @location(6) @interpolate(flat) edges:    vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    // Expand the quad past the shape so the whole penumbra fits.
    let margin = inst.blur + 2.0;
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = mix(inst.rect_min - vec2<f32>(margin), inst.rect_max + vec2<f32>(margin), corner);
    var out: VsOut;
    let ndc      = px / globals.screen * 2.0 - 1.0;
    out.pos      = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.px       = px;
    out.rect_min = inst.rect_min;
    out.rect_max = inst.rect_max;
    out.color    = inst.color;
    out.radius   = inst.radius;
    out.blur     = inst.blur;
    out.edges    = inst.edges;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let center = (in.rect_min + in.rect_max) * 0.5;
    let half   = (in.rect_max - in.rect_min) * 0.5;
    let p      = in.px - center;

    // Signed distance to the rounded rect (negative inside, positive outside).
    let r       = min(in.radius, min(half.x, half.y));
    let q       = abs(p) - half + vec2<f32>(r);
    let outside = max(q, vec2<f32>(0.0));
    let d       = length(outside) + min(max(q.x, q.y), 0.0) - r;

    // Penumbra: darkest at the edge, easing to zero `blur` px out. Squared
    // for a soft, filmic falloff. Exterior mask keeps it off the glass.
    let fade     = 1.0 - smoothstep(0.0, in.blur, max(d, 0.0));
    let penumbra = fade * fade;
    let exterior = smoothstep(-1.0, 1.0, d);

    // Per-edge strength blended by the outward direction. On a straight edge
    // one axis dominates (pure top/bottom/left/right); at a corner both are
    // ~0.5, averaging the two neighbours — a seamless transition.
    let dir  = outside / max(length(outside), 0.0001);
    let vert  = select(in.edges.x, in.edges.y, p.y > 0.0);   // top / bottom
    let horiz = select(in.edges.z, in.edges.w, p.x > 0.0);   // left / right
    let edge_alpha = vert * dir.y * dir.y + horiz * dir.x * dir.x;

    let a = in.color.a * edge_alpha * penumbra * exterior;
    return vec4<f32>(in.color.rgb * a, a);
}

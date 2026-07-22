// Instanced rounded rectangles (card background, hover highlights),
// SDF-shaded per fragment.  The card background uses a liquid-glass
// material (glass == 1); hover fills and other rects are plain solid fills.
// Output is premultiplied alpha over a transparent surface.

struct Globals {
    screen:    vec2<f32>,
    alpha:     f32,
    time:      f32,
    cursor:    vec2<f32>,  // pointer in surface pixels; (-9999,-9999) = absent
    _pad:      vec2<f32>,
    // Up to 4 simultaneous ripples: (x, y, age, 0); x < -9000 = inactive.
    ripples:   array<vec4<f32>, 4>,
    // Up to 2 box open/close waves: (cx, cy, age, 0); cx < -9000 = inactive.
    box_waves: array<vec4<f32>, 2>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect_min: vec2<f32>,
    @location(1) rect_max: vec2<f32>,
    @location(2) color:    vec4<f32>,
    @location(3) radius:   f32,
    @location(4) glass:    f32,     // 0 = solid fill, 1 = liquid-glass material
};

struct VsOut {
    @builtin(position)              pos:      vec4<f32>,
    @location(0)                    px:       vec2<f32>,
    @location(1) @interpolate(flat) rect_min: vec2<f32>,
    @location(2) @interpolate(flat) rect_max: vec2<f32>,
    @location(3) @interpolate(flat) color:    vec4<f32>,
    @location(4) @interpolate(flat) radius:   f32,
    @location(5) @interpolate(flat) glass:    f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    let px = mix(inst.rect_min - vec2<f32>(1.0), inst.rect_max + vec2<f32>(1.0), corner);
    var out: VsOut;
    let ndc    = px / globals.screen * 2.0 - 1.0;
    out.pos      = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.px       = px;
    out.rect_min = inst.rect_min;
    out.rect_max = inst.rect_max;
    out.color    = inst.color;
    out.radius   = inst.radius;
    out.glass    = inst.glass;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let center = (in.rect_min + in.rect_max) * 0.5;
    let half   = (in.rect_max - in.rect_min) * 0.5;
    let p      = in.px - center;

    let r = min(in.radius, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;

    let coverage = clamp(0.5 - d, 0.0, 1.0);

    // Box open/close wave: applied to ALL rects so the effect is visible
    // even when the box overlay covers the card background.
    //
    // Two layers, both full card-width (no lateral taper):
    //   flash — immediate bright pulse at the icon, fades in ~0.15 s
    //   ring  — Gaussian envelope riding the wave front at 200 px/s
    //
    // Result is mixed toward white so it reads clearly on any background.
    var box_wave = 0.0;
    for (var i = 0u; i < 2u; i++) {
        let bw  = globals.box_waves[i];
        if bw.x > -9000.0 {
            let dy    = in.px.y - bw.y;
            let age   = bw.z;
            let front = abs(dy) - age * 900.0;
            let age_decay = exp(-age * 4.0);
            // Three bands trailing the wave front, each fatter than the last.
            let b0 = exp(-front * front / 80000.0) * 0.65;
            let b1 = exp(-(front + 70.0)  * (front + 70.0)  / 160000.0) * 0.48;
            let b2 = exp(-(front + 150.0) * (front + 150.0) / 350000.0) * 0.32;
            let flash = exp(-abs(dy) * 0.06) * exp(-age * 14.0) * 0.40;
            box_wave += (b0 + b1 + b2) * age_decay + flash;
        }
    }
    // Subtle darkening sweep; negative = darken.
    let box_lum = -box_wave * 0.015;

    // Solid fill (hover highlights, dividers, box overlay, etc.)
    if in.glass < 0.5 {
        var rgb = clamp(in.color.rgb + vec3<f32>(box_lum), vec3<f32>(0.0), vec3<f32>(1.0));
        let a   = in.color.a * globals.alpha * coverage;
        return vec4<f32>(rgb * a, a);
    }

    // --- Liquid glass material (card background) ---
    let rect_w      = in.rect_max.x - in.rect_min.x;
    let rect_h      = in.rect_max.y - in.rect_min.y;
    let from_top    = in.px.y - in.rect_min.y;
    let from_bottom = in.rect_max.y - in.px.y;
    let from_left   = in.px.x - in.rect_min.x;
    let from_right  = in.rect_max.x - in.px.x;
    // Distance from nearest card edge (0 at boundary, grows inward).
    let edge = max(-d, 0.0);
    let t    = globals.time;

    let cx             = globals.cursor.x;
    let cy             = globals.cursor.y;
    let cursor_present = cx > -9000.0;

    // Layer 4 — bottom vignette: bottom 32 % dims to simulate curvature.
    let norm_y = from_top / rect_h;
    let vig    = clamp((norm_y - 0.68) / 0.32, 0.0, 1.0) * -0.004;

    // Layer 5 — side Fresnel: outer edges brighten at grazing angles.
    // Scaled by in.glass so group boxes (0.5) get half the gradient of the main card (1.0).
    let norm_x  = from_left / rect_w;
    let side    = abs(norm_x - 0.5) * 2.0;
    let fresnel = clamp((side - 0.48) / 0.52, 0.0, 1.0) * 0.02 * in.glass;

    // Ripples: each active ripple adds an expanding circular sine wave.
    var ripple = 0.0;
    for (var i = 0u; i < 4u; i++) {
        let rp = globals.ripples[i];
        if rp.x > -9000.0 {
            let dist  = length(in.px - rp.xy);
            let front = dist - rp.z * 150.0;
            let decay = exp(-rp.z * 1.0) * exp(-abs(front) * 0.03);
            ripple   += sin(front * 0.10) * decay * 0.0013;
        }
    }

    // Layer 6 — boundary rim glow: thin bright ring at the SDF card edge.
    // Scaled by `in.glass` so group boxes (0.5) get half the glow of the main card (1.0).
    let boundary = exp(-d * d * 0.45) * 0.13 * in.glass;

    // Layer 8 — iridescent tint on all edges, cursor-driven phase.
    // `in.glass` doubles as an iridescence scale: 1.0 = full (main card),
    // 0.5 = half (group-box panel — smaller rect, edges already read stronger).
    let iri_mask = clamp(1.0 - edge / 30.0, 0.0, 1.0) * 0.016 * in.glass;
    var phase: f32;
    if cursor_present {
        phase = atan2(in.px.y - cy, in.px.x - cx) * 2.0 + t * 0.15;
    } else {
        phase = (in.px.x - in.rect_min.x) / rect_w * 6.2831853 + t * 0.25;
    }
    let iri = vec3<f32>(
        sin(phase)          * 0.5 + 0.5,
        sin(phase + 2.0944) * 0.5 + 0.5,
        sin(phase + 4.1888) * 0.5 + 0.5,
    ) * iri_mask;

    // Layer 9 — edge reflection that tracks the cursor.
    var spotlight = 0.0;
    if cursor_present {
        let prox  = 120.0;
        let along = 130.0;
        let thick =  10.0;

        let w_top = exp(-(cy - in.rect_min.y) * (cy - in.rect_min.y) / (prox * prox));
        spotlight += w_top
            * exp(-(in.px.x - cx) * (in.px.x - cx) / (along * along))
            * exp(-from_top   * from_top   / (thick * thick));

        let w_bot = exp(-(in.rect_max.y - cy) * (in.rect_max.y - cy) / (prox * prox));
        spotlight += w_bot
            * exp(-(in.px.x - cx)  * (in.px.x - cx)  / (along * along))
            * exp(-from_bottom * from_bottom / (thick * thick));

        let w_left = exp(-(cx - in.rect_min.x) * (cx - in.rect_min.x) / (prox * prox));
        spotlight += w_left
            * exp(-(in.px.y - cy) * (in.px.y - cy) / (along * along))
            * exp(-from_left  * from_left  / (thick * thick));

        let w_right = exp(-(in.rect_max.x - cx) * (in.rect_max.x - cx) / (prox * prox));
        spotlight += w_right
            * exp(-(in.px.y - cy) * (in.px.y - cy) / (along * along))
            * exp(-from_right * from_right / (thick * thick));

        spotlight *= 0.06;
    }

    let lum = vig + fresnel + ripple + box_lum + boundary + spotlight;
    var rgb  = clamp(in.color.rgb + vec3<f32>(lum), vec3<f32>(0.0), vec3<f32>(1.0));
    rgb     += iri;

    let a = in.color.a * globals.alpha * coverage;
    return vec4<f32>(rgb * a, a);
}

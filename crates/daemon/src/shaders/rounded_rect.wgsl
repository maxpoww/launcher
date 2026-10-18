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
    // Banner blister: (bar_edge_y, k, _, _). A blister-flagged rect (glass≈2)
    // is the bar's own material smooth-unioned with the half-plane above
    // bar_edge_y — the banner swells around the module like a snake with the
    // pill inside. x < -9000 = off.
    neck:      vec4<f32>,
    // x = the night light: 1 in the dark look (renderer.rs `Globals::light`).
    light:     vec4<f32>,
};

// Polynomial smooth-min: blends two SDFs with a fillet of radius ~k — the
// metaball union that makes two shapes merge with a smooth bulge.
fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect_min: vec2<f32>,
    @location(1) rect_max: vec2<f32>,
    @location(2) color:    vec4<f32>,
    @location(3) radius:   f32,
    @location(4) glass:    f32,     // 0 = solid fill, 1 = liquid-glass material
    @location(5) border:   f32,     // 0 = filled; >0 = stroke width inside the edge
};

struct VsOut {
    @builtin(position)              pos:      vec4<f32>,
    @location(0)                    px:       vec2<f32>,
    @location(1) @interpolate(flat) rect_min: vec2<f32>,
    @location(2) @interpolate(flat) rect_max: vec2<f32>,
    @location(3) @interpolate(flat) color:    vec4<f32>,
    @location(4) @interpolate(flat) radius:   f32,
    @location(5) @interpolate(flat) glass:    f32,
    @location(6) @interpolate(flat) border:   f32,
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
    out.border   = inst.border;
    return out;
}

const WINDOW_POWER: f32 = 3.0;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let center = (in.rect_min + in.rect_max) * 0.5;
    let half   = (in.rect_max - in.rect_min) * 0.5;
    let p      = in.px - center;

    // A NEGATIVE radius asks for THE WINDOWS' CORNER and not a circle's
    // quarter: the superellipse the compositor rounds a window with
    // (Hyprland's `rounding.glsl`: |x|^p + |y|^p = r^p), at the power Golem
    // sets — `decoration.rounding_power` in hyprland.lua; `WINDOW_POWER`
    // here must say the same. See `content::WINDOW_CORNER`.
    let r = min(abs(in.radius), min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    var d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
    if in.radius < 0.0 && q.x > 0.0 && q.y > 0.0 {
        d = pow(pow(q.x, WINDOW_POWER) + pow(q.y, WINDOW_POWER), 1.0 / WINDOW_POWER) - r;
    }

    // A stroke keeps only a band of `border` px just INSIDE the edge: fold
    // the signed distance about the band's midline, so the outline follows
    // the rounded corners exactly (four straight rects cannot).
    var sd = d;
    if in.border > 0.0 {
        sd = abs(d + in.border * 0.5) - in.border * 0.5;
    }
    let coverage = clamp(0.5 - sd, 0.0, 1.0);

    // THE VEIL (glass < -1.5; Max, 2026-10-18, Auto Lighting "include the
    // background"): a wash over the whole surface that the scene's fade does
    // not touch — the desktop's icons fade away when they are put away, the
    // light over the wallpaper stays. A plain colour, no material.
    if in.glass < -1.5 {
        let a = in.color.a * coverage;
        return vec4<f32>(in.color.rgb * a, a);
    }

    // Banner blister (glass ≈ 2): a SOLID fill of the bar's own colour whose
    // shape is the rounded rect smooth-unioned with the half-plane above the
    // bar edge — so the banner bulges around the module (drawn on a quad
    // expanded by `k`, the SDF inset back by `k` to the true rect).
    if abs(in.glass - 2.0) < 0.5 && globals.neck.x > -9000.0 {
        let k = globals.neck.y;
        let half2 = half - vec2<f32>(k);
        let r2 = min(in.radius, min(half2.x, half2.y));
        let q2 = abs(p) - half2 + vec2<f32>(r2);
        let d_rect = length(max(q2, vec2<f32>(0.0))) + min(max(q2.x, q2.y), 0.0) - r2;
        let d_bar = in.px.y - globals.neck.x; // <0 above the bar edge (in the bar)
        let du = smin(d_rect, d_bar, k);
        // Clip the blister to BELOW the bar edge: the full-width bar strip
        // already paints the banner above the line, and the strip + this
        // blister are the SAME (translucent) banner material — drawing both
        // over the same pixels stacked their opacity and darkened the banner
        // (Max). So the strip owns the bar band; the blister only adds the
        // bulge below it. `below` fades in across the edge (±0.5px AA).
        let below = clamp((in.px.y - globals.neck.x) + 0.5, 0.0, 1.0);
        let cov = clamp(0.5 - du, 0.0, 1.0) * below;
        let rgb = in.color.rgb;
        let a = in.color.a * globals.alpha * cov;
        return vec4<f32>(rgb * a, a);
    }

    // Solid fill (hover highlights, dividers, box overlay, etc.)
    if in.glass < 0.5 {
        var rgb = in.color.rgb;
        // Hard-edged fill (glass < -0.5): no SDF anti-aliasing. Used by the
        // colour-matched OPTIONS bar, whose bottom edge abuts a window of the
        // SAME colour. A fractional-alpha AA edge there gets gamma-lifted by
        // the compositor's premultiplied-sRGB blend and shows as a faint seam;
        // a crisp cut between two identical colours is invisible instead.
        var cov = coverage;
        if in.glass < -0.5 {
            cov = select(0.0, 1.0, sd < 0.0);
        }
        // THE NIGHT LIGHT on the solid boxes too (Max, 2026-10-15: the boxes
        // and menus lit like the dock): a filled panel of box size — not a
        // pill, a row, a frame or a whole window's ground — catches a line of
        // light along its top and falls into shadow toward its bottom.
        let w = in.rect_max.x - in.rect_min.x;
        let h = in.rect_max.y - in.rect_min.y;
        let whole = w >= globals.screen.x - 2.0 && h >= globals.screen.y - 2.0;
        if globals.light.x > 0.0 && in.border <= 0.0 && in.glass > -0.5 && h >= 80.0 && w >= 120.0 && !whole {
            let from_top = in.px.y - in.rect_min.y;
            let edge = max(-sd, 0.0);
            let top = 0.10 * exp(-from_top * from_top / 4.0) * clamp(1.0 - edge / 3.0, 0.0, 1.0);
            let vig = clamp((from_top / h - 0.5) / 0.5, 0.0, 1.0) * -0.04;
            rgb = clamp(rgb + vec3<f32>((top + vig) * globals.light.x), vec3<f32>(0.0), vec3<f32>(1.0));
        }
        let a   = in.color.a * globals.alpha * cov;
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
    // THE NIGHT LIGHT: in the dark look the glass is lit from ABOVE, hard —
    // the bottom falls into shadow, the rim catches more, a razor line of
    // light runs along the top edge, the pointer's reflection is stronger.
    // Apple's and Material's dark rule: depth by light, not by shadow.
    let night  = globals.light.x;
    let vig    = clamp((norm_y - 0.50) / 0.50, 0.0, 1.0) * (-0.004 - night * 0.05);

    // Layer 5 — side Fresnel: outer edges brighten at grazing angles.
    // Scaled by in.glass so group boxes (0.5) get half the gradient of the main card (1.0).
    let norm_x  = from_left / rect_w;
    let side    = abs(norm_x - 0.5) * 2.0;
    let fresnel = clamp((side - 0.48) / 0.52, 0.0, 1.0) * 0.02 * in.glass;

    // Layer 6 — boundary rim glow: thin bright ring at the SDF card edge.
    // Scaled by `in.glass` so group boxes (0.5) get half the glow of the main card (1.0).
    let boundary = exp(-d * d * 0.45) * (0.04 + night * 0.05) * in.glass;
    let top_rim  = night * 0.16 * in.glass * exp(-from_top * from_top / 4.0) * clamp(1.0 - edge / 3.0, 0.0, 1.0);

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

        spotlight *= 0.06 + night * 0.08;
    }

    let lum = vig + fresnel + boundary + spotlight + top_rim;
    var rgb  = clamp(in.color.rgb + vec3<f32>(lum), vec3<f32>(0.0), vec3<f32>(1.0));
    rgb     += iri;

    let a = in.color.a * globals.alpha * coverage;
    return vec4<f32>(rgb * a, a);
}

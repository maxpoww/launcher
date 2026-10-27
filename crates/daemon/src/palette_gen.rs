//! THE GENERATOR (Max's palettes, 2026-10-16; `~/Golem/work/palettes/gen.py`,
//! ported through the Colours mockup): six colours → sixteen, a dark and a
//! light variant. A palette the owner MAKES is theirs: each of the six is a
//! role and goes straight there (Accent, Ground, Panels, Web, Text, Detail);
//! the generator only fills what was not chosen (the terminal's hues).

use crate::palette::tint::{from_lab, from_lch, lab, lch};
use crate::palette::{contrast, rel_lum, Palette, Rgb};

const SLOTS: [f32; 8] = [25.0, 55.0, 95.0, 145.0, 200.0, 258.0, 330.0, 60.0];
const MINC: [f32; 8] = [0.10, 0.09, 0.08, 0.07, 0.06, 0.07, 0.07, 0.04];
const DARK_L: [f32; 8] = [0.180, 0.215, 0.272, 0.375, 0.640, 0.885, 0.930, 0.968];
const LIGHT_L: [f32; 8] = [0.955, 0.918, 0.872, 0.780, 0.520, 0.285, 0.225, 0.170];

/// The roles of the six, in the editor's order.
pub(crate) const ROLES: [&str; 6] = ["Accent", "Ground", "Panels", "Web", "Text", "Detail"];

fn gap(a: f32, b: f32) -> f32 {
    let d = (a - b).abs() % 360.0;
    d.min(360.0 - d)
}

fn wrap(d: f32) -> f32 {
    (d + 540.0).rem_euclid(360.0) - 180.0
}

fn ratio(a: Rgb, b: Rgb) -> f32 {
    contrast(rel_lum(a), rel_lum(b))
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let (p, q) = (lab(a), lab(b));
    from_lab(p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t, p[2] + (q[2] - p[2]) * t)
}

#[derive(Clone, Copy)]
struct Soul {
    l: f32,
    c: f32,
    h: f32,
    w: f32,
}

fn mean_hue(items: &[Soul], by_w: bool) -> f32 {
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for i in items {
        let w = if by_w { i.w } else { i.c };
        x += w * i.h.to_radians().cos();
        y += w * i.h.to_radians().sin();
    }
    if x == 0.0 && y == 0.0 {
        60.0
    } else {
        y.atan2(x).to_degrees().rem_euclid(360.0)
    }
}

/// Six colours → (dark, light, the accent's slot in each).
fn build(soul: &[Rgb; 6], acc_idx: usize) -> ([Rgb; 16], [Rgb; 16], usize, usize) {
    let s: Vec<Soul> = soul
        .iter()
        .map(|&c| {
            let (l, ch, h) = lch(c);
            Soul { l, c: ch, h, w: ch * (1.0 - l).powi(2) }
        })
        .collect();
    let mut by_l = s.clone();
    by_l.sort_by(|a, b| a.l.total_cmp(&b.l));
    let (darkest, lightest) = (by_l[0], by_l[5]);
    let (dh, lh) = (&by_l[..3], &by_l[3..]);
    let med_c = |half: &[Soul]| {
        let mut c: Vec<f32> = half.iter().map(|i| i.c).collect();
        c.sort_by(f32::total_cmp);
        c[1]
    };
    let g_h = if darkest.c > 0.02 { darkest.h } else { mean_hue(dh, true) };
    let g_c = (med_c(dh) * 0.35).clamp(0.004, 0.016);
    let p_h = if lightest.c > 0.012 { lightest.h } else { mean_hue(lh, false) };
    let p_c = (med_c(lh) * 0.35).clamp(0.004, 0.016);
    let i_h = if lightest.c > 0.01 { lightest.h } else { g_h };
    let i_c = (lightest.c * 0.5).clamp(0.003, 0.01);
    let mut cs: Vec<f32> = s.iter().map(|i| i.c).collect();
    cs.sort_by(f32::total_cmp);
    let vivid = cs[4];
    let mean_c = s.iter().map(|i| i.c).sum::<f32>() / 6.0;
    let coloured: Vec<Soul> = s.iter().copied().filter(|i| i.c > 0.03).collect();
    let lean_h = mean_hue(if coloured.is_empty() { &s } else { &coloured }, false);
    let max_c = s.iter().map(|i| i.c).fold(0.0, f32::max);

    let one = |dark: bool| -> ([Rgb; 16], usize) {
        let ls = if dark { DARK_L } else { LIGHT_L };
        let (gh, gc, ih) = if dark { (g_h, g_c, i_h) } else { (p_h, p_c, g_h) };
        let mut out = [[0u8; 3]; 16];
        for (k, &l) in ls.iter().enumerate() {
            let t = k as f32 / 7.0;
            let h = if gap(gh, ih) < 120.0 { gh + wrap(ih - gh) * t } else if t < 0.5 { gh } else { ih };
            out[k] = from_lch(l, gc * (1.0 - t) + i_c * t, h.rem_euclid(360.0));
        }
        let surface = out[1];
        let (lo, hi, base_l) = if dark { (0.68, 0.86, 0.76) } else { (0.40, 0.58, 0.50) };
        let acc = s[acc_idx];
        let acc_slot = (0..7).min_by(|&a, &b| gap(SLOTS[a], acc.h).total_cmp(&gap(SLOTS[b], acc.h))).unwrap_or(5);
        let mut taken: [Option<usize>; 8] = [None; 8];
        taken[acc_slot] = Some(acc_idx);
        let mut used = vec![acc_idx];
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (n, i) in s.iter().enumerate() {
            if i.c > 0.05 {
                for (k, &sh) in SLOTS.iter().enumerate().take(7) {
                    if gap(i.h, sh) < 22.0 {
                        pairs.push((gap(i.h, sh), k, n));
                    }
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, k, n) in pairs {
            if taken[k].is_none() && !used.contains(&n) {
                taken[k] = Some(n);
                used.push(n);
            }
        }
        for k in 0..8 {
            let (mut l, c, mut h);
            if k == 7 {
                h = 58.0 + wrap(lean_h - 58.0).clamp(-12.0, 12.0);
                c = mean_c.clamp(0.045, 0.075);
                l = if dark { 0.64 } else { 0.44 };
            } else if let Some(n) = taken[k] {
                let i = s[n];
                h = i.h;
                c = i.c.max(MINC[k]);
                l = (if dark || i.l < 0.62 { i.l } else { 0.52 }).clamp(lo, hi);
                if !dark && (75.0..=115.0).contains(&h) && i.c > 0.1 {
                    h = 68.0 + (h - 75.0) * 0.25;
                }
            } else {
                h = SLOTS[k] + wrap(lean_h - SLOTS[k]).clamp(-16.0, 16.0);
                let far = gap(h, lean_h) / 180.0;
                c = (mean_c.max(vivid * 0.8) * (1.0 - 0.4 * far)).min(0.15).max(MINC[k]);
                l = base_l;
            }
            let mut x = from_lch(l, c, h);
            let mut n = 0;
            while ratio(x, surface) < 3.0 && n < 40 {
                l += if dark { 0.015 } else { -0.015 };
                x = from_lch(l, c, h);
                n += 1;
            }
            out[8 + k] = x;
        }
        // A grey palette is grey all through (the accent included).
        if max_c < 0.03 {
            let lg = if dark { [0.70, 0.78, 0.86, 0.64, 0.74, 0.60, 0.82, 0.56] } else { [0.46, 0.38, 0.30, 0.50, 0.42, 0.54, 0.34, 0.48] };
            for k in 0..8 {
                out[8 + k] = from_lch(lg[k], 0.004, gh);
            }
            out[8 + acc_slot] = from_lch(if dark { 0.93 } else { 0.26 }, 0.003, gh);
        }
        (out, 8 + acc_slot)
    };
    let (d, a) = one(true);
    let (l, al) = one(false);
    (d, l, a, al)
}

/// The owner's palette: each chosen colour exactly in its place in ITS look
/// (a dark ground: the dark one; a light ground: the light one), the rest
/// made — the other look keeps the greys made from the same colours (worn as
/// they were in the Light look, a dark palette stayed dark there, and its
/// Lighting pushed the words past reading). Its words read on its ground and
/// panels (4.5:1): moved toward white or black as little as it takes.
pub(crate) fn direct(id: &str, name: &str, soul: [Rgb; 6]) -> Palette {
    let (mut d, mut l, a, al) = build(&soul, 0);
    let [acc, ground, panels, _web, text, detail] = soul;
    let dark = ratio(ground, [255, 255, 255]) >= ratio(ground, [0, 0, 0]);
    let (edge, away) = if dark { ([0, 0, 0], [255, 255, 255]) } else { ([255, 255, 255], [0, 0, 0]) };
    let reads = |t: Rgb| ratio(t, ground) >= 4.5 && ratio(t, panels) >= 4.5;
    let text = if reads(text) { text } else { (1..=20).map(|i| mix(text, away, i as f32 / 20.0)).find(|t| reads(*t)).unwrap_or(away) };
    let x = if dark { &mut d } else { &mut l };
    x[0] = mix(ground, edge, 0.18);
    x[1] = ground;
    x[2] = panels;
    x[3] = detail;
    x[4] = mix(text, ground, 0.45);
    x[5] = text;
    x[6] = mix(text, away, 0.3);
    x[7] = mix(text, away, 0.6);
    d[a] = acc;
    l[al] = acc;
    Palette { id: id.to_owned(), name: name.to_owned(), dark: d, light: l, accent: a, accent_light: al, soul: soul.to_vec(), family: "own".to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owners_colours_land_where_they_were_put() {
        let soul = [[0xe0, 0x52, 0x52], [0x1a, 0x1d, 0x22], [0x2a, 0x2e, 0x36], [0x30, 0x30, 0x40], [0xe6, 0xe6, 0xe0], [0x44, 0x48, 0x50]];
        let p = direct("mine", "Mine", soul);
        assert_eq!(p.dark[1], soul[1]);
        assert_eq!(p.dark[2], soul[2]);
        assert_eq!(p.dark[5], soul[4]);
        assert_eq!(p.dark[p.accent], soul[0]);
        assert!((8..16).contains(&p.accent));
        // The made hues read on the ground.
        for k in 8..16 {
            assert!(ratio(p.dark[k], p.dark[1]) >= 2.5 || k == p.accent, "base{k:02X}");
        }
        // …and the Light look is light: the made greys, not these.
        assert!(ratio(p.light[1], [0, 0, 0]) > ratio(p.light[1], [255, 255, 255]), "a light ground in the Light look");
    }

    #[test]
    fn a_light_palette_is_exact_in_the_light_and_its_words_always_read() {
        let light = [[0x2b, 0x6c, 0xd0], [0xf4, 0xf1, 0xea], [0xe6, 0xe2, 0xd8], [0xf4, 0xf1, 0xea], [0x22, 0x22, 0x26], [0xc8, 0xc4, 0xba]];
        let p = direct("mine", "Mine", light);
        assert_eq!((p.light[1], p.light[2], p.light[5]), (light[1], light[2], light[4]));
        assert!(ratio(p.dark[1], [255, 255, 255]) > ratio(p.dark[1], [0, 0, 0]), "a dark ground in the Dark look");
        // Words the colour of the ground (or nearly): moved until they read.
        for ground in [[0x1d, 0x20, 0x26], [0xf4, 0xf1, 0xea], [0x80, 0x80, 0x80]] {
            let p = direct("mine", "Mine", [[0xe0, 0x52, 0x52], ground, ground, ground, ground, ground]);
            let half = if ratio(ground, [255, 255, 255]) >= ratio(ground, [0, 0, 0]) { p.dark } else { p.light };
            assert!(ratio(half[5], half[1]) >= 4.5, "words on {ground:?}: {:.2}", ratio(half[5], half[1]));
        }
    }
}

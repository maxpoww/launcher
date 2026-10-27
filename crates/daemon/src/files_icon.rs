//! Golem Files' LAUNCHER on the dock (Max, 2026-10-15, the Folder Stack Lab:
//! "al pasar: separación 5.0 × 9.0 · 80 ms · suave · oscuras 20% · se achica
//! y entra, y esto es solo para el lanzador del dock"). At rest it is Golem's
//! folder with a folder engraved on it — the app's icon everywhere. Under the
//! pointer two plain folders slide out from behind it, up and to the right,
//! each a step darker, and the front one shrinks so all three fit the tile.
//!
//! The figures are drawn here (as the dock's own trio are) in the look's tone,
//! one layer each: [`BLANK`] (carries the dock plate, which must not move),
//! the front, and the two behind; [`quads`] places them for a hover value.

use crate::content::Rect;

/// The layers, in upload order from the first one (`App::files_layer`).
pub(crate) const BLANK: u32 = 0;
pub(crate) const FRONT: u32 = 1;
pub(crate) const BEHIND: u32 = 2; // and BEHIND + 1

/// Max's numbers from the lab: the step of each folder behind (on the
/// 64-unit icon, right and UP), the time, how much darker a step.
const STEP_X: f32 = 5.0;
const STEP_Y: f32 = 9.0;
pub(crate) const SECONDS: f32 = 0.080;
const DARKER: f32 = 0.20;

/// Where Golem's folder (52 × 45) stands on its 64 grid.
const REST: (f32, f32) = (6.0, 9.5);

/// The tones of the lab (`TONES`): silver, and graphite in the dark.
struct Tone {
    f0: &'static str,
    f1: &'static str,
    b0: &'static str,
    b1: &'static str,
    sym: &'static str,
    edge: f32,
    gloss: f32,
    hair: f32,
}

const SILVER: Tone = Tone {
    f0: "#eef0f3",
    f1: "#c2c6cd",
    b0: "#b3b8c0",
    b1: "#8f949e",
    sym: "#a1a6af",
    edge: 0.6,
    gloss: 0.35,
    hair: 0.55,
};
const GRAPHITE: Tone = Tone {
    f0: "#737882",
    f1: "#4f535c",
    b0: "#50545d",
    b1: "#3b3e46",
    sym: "#3c4048",
    edge: 0.18,
    gloss: 0.16,
    hair: 0.22,
};

const BACK: &str = "M0,4.5A4.5,4.5 0 0 1 4.5,0H17.5C20,0 21.2,.9 22.6,2.6L24,4.3C24.6,5 25.2,5.3 26.2,5.3H47.5A4.5,4.5 0 0 1 52,9.8V40H0Z";
const FOLDER_GLYPH: &str = "M3.6 6.6a2 2 0 0 1 2-2h4.3l2 2.1h6.5a2 2 0 0 1 2 2v9.1a2 2 0 0 1-2 2H5.6a2 2 0 0 1-2-2z";

/// A colour `k` of the way to black.
fn shade(hex: &str, k: f32) -> String {
    let c = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0) as f32 * (1.0 - k);
    format!("#{:02x}{:02x}{:02x}", c(1).round() as u8, c(3).round() as u8, c(5).round() as u8)
}

/// One folder at its rest place on the 64 grid, `k` darker, with or without
/// the engraved folder.
fn folder_svg(t: &Tone, k: f32, symbol: bool) -> String {
    let (x, y) = REST;
    let mut s = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64"><defs>
<linearGradient id="f" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{f0}"/><stop offset="1" stop-color="{f1}"/></linearGradient>
<linearGradient id="b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{b0}"/><stop offset="1" stop-color="{b1}"/></linearGradient>
<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="{gloss}"/><stop offset=".55" stop-color="#fff" stop-opacity="0"/></linearGradient>
<filter id="bev" x="-20%" y="-20%" width="140%" height="140%"><feDropShadow dx="0" dy="-.6" stdDeviation="0" flood-color="#fff" flood-opacity="{hair9}"/><feDropShadow dx="0" dy="1.4" stdDeviation="1" flood-color="#000" flood-opacity=".45"/></filter></defs>
<g transform="translate({x} {y})">
<path fill="url(#b)" filter="url(#bev)" d="{BACK}"/>
<rect fill="url(#f)" filter="url(#bev)" x="0" y="8" width="52" height="37" rx="4.5"/>
<rect fill="url(#g)" x="0" y="8" width="52" height="37" rx="4.5"/>
<rect fill="#fff" opacity="{hair}" x="4.5" y="8" width="43" height=".7" rx=".35"/>"##,
        f0 = shade(t.f0, k),
        f1 = shade(t.f1, k),
        b0 = shade(t.b0, k),
        b1 = shade(t.b1, k),
        gloss = t.gloss,
        hair9 = t.hair * 0.9,
        hair = t.hair,
    );
    if symbol {
        // Engraved: a light edge under it, then the darker grey.
        let (gx, gy) = (26.0 - 12.0 * 1.15, 26.5 - 12.2 * 1.15);
        for (fill, op, dy) in [("#fff", t.edge, 0.75), (t.sym, 1.0, 0.0)] {
            s += &format!(
                r#"<path d="{FOLDER_GLYPH}" fill="{fill}" stroke="{fill}" stroke-width="1.2" stroke-linejoin="round" opacity="{op}" transform="translate({gx} {}) scale(1.15)"/>"#,
                gy + dy
            );
        }
    }
    s + "</g></svg>"
}

/// The four layers' SVGs in the present look, in upload order.
pub(crate) fn figures() -> [Vec<u8>; 4] {
    let t = if crate::look::icons_dark() { &GRAPHITE } else { &SILVER };
    [
        br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64"/>"#.to_vec(),
        folder_svg(t, 0.0, true).into_bytes(),
        folder_svg(t, DARKER, false).into_bytes(),
        folder_svg(t, DARKER * 2.0, false).into_bytes(),
    ]
}

/// `cubic-bezier(.22, .8, .28, 1)` — the lab's "suave".
pub(crate) fn soft(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    let (x1, y1, x2, y2) = (0.22f32, 0.8f32, 0.28f32, 1.0f32);
    let bez = |a: f32, b: f32, t: f32| 3.0 * a * t * (1.0 - t).powi(2) + 3.0 * b * t * t * (1.0 - t) + t.powi(3);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if bez(x1, x2, mid) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) / 2.0)
}

/// The CLICK's wave (Max, 2026-10-18, the App Motion Lab —
/// https://claude.ai/artifact/BP4E7xwMBtEWNgwqrN11Ta): each folder jumps up
/// `WAVE_LIFT`, back one first, `WAVE_GAP` apart — a snap up, a drop, a small
/// bounce below rest — `WAVE_JUMP` each.
const WAVE_LIFT: f32 = 10.0;
const WAVE_GAP: f32 = 0.065;
const WAVE_JUMP: f32 = 0.240;
const WAVE_SECONDS: f32 = WAVE_JUMP + 2.0 * WAVE_GAP;

/// One folder's jump `t` seconds in (1 = the top, 0 = rest).
fn jump(t: f32) -> f32 {
    if t <= 0.0 || t >= WAVE_JUMP {
        return 0.0;
    }
    let x = t / WAVE_JUMP;
    let bez = crate::youtube_icon::bezier;
    if x < 0.32 {
        bez(0.15, 0.8, 0.3, 1.0, x / 0.32)
    } else if x < 0.72 {
        1.0 - bez(0.55, 0.0, 0.85, 0.4, (x - 0.32) / 0.4)
    } else {
        -0.14 * (std::f32::consts::PI * (x - 0.72) / 0.28).sin()
    }
}

/// The hover as CSS runs a transition: from wherever it stood when the
/// pointer came or went, to the other end, in [`SECONDS`] on the soft curve;
/// and the click's wave.
#[derive(Default)]
pub(crate) struct Hover {
    from: f32,
    to: f32,
    t: f32,
    /// The wave, seconds in.
    wave: Option<f32>,
}

impl Hover {
    /// How far out the folders are (0 rest, 1 hovered).
    pub(crate) fn value(&self) -> f32 {
        self.from + (self.to - self.from) * soft(self.t)
    }

    /// A click: the wave.
    pub(crate) fn click(&mut self) {
        self.wave = Some(0.0);
    }

    /// The folders' lifts now (back, middle, front).
    pub(crate) fn lifts(&self) -> [f32; 3] {
        let t = self.wave.unwrap_or(-1.0);
        std::array::from_fn(|i| WAVE_LIFT * jump(t - i as f32 * WAVE_GAP))
    }

    /// Step by `dt` toward `on`; whether it still moves.
    pub(crate) fn step(&mut self, on: bool, dt: f32) -> bool {
        let waving = self.wave.is_some();
        self.wave = self.wave.map(|t| t + dt).filter(|t| *t < WAVE_SECONDS);
        self.slide(on, dt) || waving
    }

    fn slide(&mut self, on: bool, dt: f32) -> bool {
        let to = if on { 1.0 } else { 0.0 };
        if to != self.to {
            self.from = self.value();
            self.to = to;
            self.t = 0.0;
        }
        if self.t >= 1.0 {
            return false;
        }
        self.t = (self.t + dt / SECONDS).min(1.0);
        true
    }
}

/// The three folders' quads inside `icon` for a hover value `h`, back to
/// front: (layer offset, rect). Each layer is the folder drawn at its rest
/// place on a full 64 tile, so a pose (top-left `p`, scale `s` of the
/// folder) is the tile moved by `p − s·REST` and scaled by `s`.
pub(crate) fn quads(icon: Rect, h: f32, lifts: [f32; 3]) -> [(u32, Rect); 3] {
    // "Se achica y entra": the front shrinks so the three fit the 64 tile.
    let span = (STEP_X * 2.0, STEP_Y * 2.0);
    let s = 1.0f32.min((56.0 - span.0) / 52.0).min((52.0 - span.1) / 45.0);
    let fx = (64.0 - (52.0 * s + span.0)) / 2.0;
    let fy = (64.0 - (45.0 * s + span.1)) / 2.0 + span.1;
    let u = icon.w / 64.0;
    let pose = |i: f32, lift: f32| {
        let lerp = |a: f32, b: f32| a + (b - a) * h;
        let k = lerp(1.0, s);
        let (px, py) = (lerp(REST.0, fx + STEP_X * i), lerp(REST.1, fy - STEP_Y * i) - lift);
        Rect::new(
            icon.x + (px - k * REST.0) * u,
            icon.y + (py - k * REST.1) * u,
            64.0 * k * u,
            64.0 * k * u,
        )
    };
    [(BEHIND + 1, pose(2.0, lifts[0])), (BEHIND, pose(1.0, lifts[1])), (FRONT, pose(0.0, lifts[2]))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_rest_every_folder_is_the_tile_and_hovered_they_fit() {
        let icon = Rect::new(100.0, 50.0, 64.0, 64.0);
        for (_, r) in quads(icon, 0.0, [0.0; 3]) {
            assert_eq!((r.x, r.y, r.w, r.h), (100.0, 50.0, 64.0, 64.0));
        }
        // Hovered: each folder (52 × 45 of its tile, from REST) lies inside
        // the icon, the ones behind up and to the right.
        let out = quads(icon, 1.0, [0.0; 3]);
        for (_, r) in out {
            let k = r.w / 64.0;
            let (x0, y0) = (r.x + REST.0 * k, r.y + REST.1 * k);
            assert!(x0 >= 100.0 - 0.01 && x0 + 52.0 * k <= 164.01, "{x0}");
            assert!(y0 >= 50.0 - 0.01 && y0 + 45.0 * k <= 114.01, "{y0}");
        }
        assert!(out[0].1.x > out[2].1.x && out[0].1.y < out[2].1.y);
    }

    #[test]
    fn a_click_waves_the_folders_back_first_and_ends_still() {
        let mut h = Hover::default();
        h.click();
        h.step(true, 0.05);
        let l = h.lifts();
        assert!(l[0] > 0.0 && l[2] == 0.0, "back first: {l:?}");
        let mut top = 0.0f32;
        for _ in 0..40 {
            h.step(true, 0.01);
            top = top.max(h.lifts()[2]);
        }
        assert!(top > WAVE_LIFT * 0.9, "the front jumps too ({top})");
        assert_eq!(h.lifts(), [0.0; 3]);
    }

    #[test]
    fn the_hover_runs_its_time_on_the_soft_curve() {
        assert!(soft(0.0).abs() < 1e-3 && (soft(1.0) - 1.0).abs() < 1e-3);
        assert!(soft(0.5) > 0.8, "soft is fast out of the gate");
        let mut h = Hover::default();
        assert!(h.step(true, 0.040));
        assert!(h.value() > 0.5 && h.value() < 1.0);
        h.step(true, 0.040);
        assert!((h.value() - 1.0).abs() < 1e-4);
        assert!(!h.step(true, 0.016), "settled");
        h.step(false, 0.0);
        assert!((h.value() - 1.0).abs() < 1e-4, "leaving starts where it stood");
        h.step(false, 0.1);
        assert!(h.value().abs() < 1e-4);
    }

    #[test]
    fn the_figures_parse() {
        for svg in figures() {
            assert!(resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).is_ok());
        }
    }
}

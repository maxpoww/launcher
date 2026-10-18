//! CHROME's icon on the dock (Max, 2026-10-17, the App Motion Lab —
//! https://claude.ai/artifact/BP4E7xwMBtEWNgwqrN11Ta): the real mark drawn as a
//! camera's IRIS — the white ring and blue centre are the lens, the three
//! colours its blades.
//!
//! Each blade's long straight side pivots on its outer end (fixed on the rim)
//! and stays tangent to the lens: a smaller lens means the long sides reach
//! further in, as an iris closes. A HOVER opens the lens `OPEN` times its size
//! (held while the pointer stays) and turns the whole plate `TURN` degrees
//! with it, spinning twice as it opens; a CLICK snaps the iris shut to a dot
//! (the shot) and springs it back to rest spinning twice the other way, where
//! it stays until the pointer has left.
//!
//! Plain shapes, so a frame is drawn as it is needed (as `youtube_icon`).

use crate::youtube_icon::bezier;

/// Chrome's entry on the dock (its `.desktop` id).
pub(crate) const CHROME_ID: &str = "google-chrome";

/// The lab drew on a 64 square (rim r 28 at 32, lens r 12.6); the dock's
/// figure is that disc fitted to 80 of 100, as every theme icon is.
const LAB: &str = r#"transform="translate(10 10) scale(1.428571) translate(-4 -4)""#;
const RIM: f32 = 28.0;
const MID: f32 = 32.0;
const LENS: f32 = 12.6;
const COLOURS: [&str; 3] = ["#ea4335", "#fbbc05", "#34a853"];

/// Max's numbers from the lab.
const OPEN: f32 = 1.6;
/// The plate's turn when fully open (degrees; − = anticlockwise).
const TURN: f32 = -45.0;
/// How fast the lens follows the pointer (ms, an exponential approach).
const TAU_MS: f32 = 110.0;
/// The click: shut to `SHUT` in `CLOSE_MS`, open again by `CLICK_MS`.
const SHUT: f32 = 0.22;
const CLOSE_MS: f32 = 180.0;
const CLICK_MS: f32 = 700.0;
/// The spins: two as a hover opens it, two the other way after the shot.
const HOVER_SPIN_MS: f32 = 700.0;
const SHOT_SPIN_MS: f32 = 800.0;
const SPINS: f32 = 2.0;

/// The figure with the lens `s` times its size and the plate spun `spin`
/// degrees past its tilt.
fn figure(s: f32, spin: f32) -> String {
    let ri = LENS * s;
    let c = MID;
    let at = |a: f32, r: f32| format!("{:.2} {:.2}", c + r * a.cos(), c + r * a.sin());
    // Each blade's outer ends (where it met the rim at rest) and inner ends
    // (where its long sides touch the lens now).
    let rim_at = |k: usize| (-210.0 + 120.0 * k as f32).to_radians() + (LENS / RIM).acos();
    let inner = |a: f32| a - (ri / RIM).acos();
    let mut blades = String::new();
    for (k, colour) in COLOURS.iter().enumerate() {
        let (t0, t1) = (rim_at(k), rim_at(k + 1));
        blades.push_str(&format!(
            r##"<path fill="{colour}" d="M{c} {c}L{}L{}A{RIM} {RIM} 0 0 1 {}L{}Z"/>"##,
            at(inner(t0), ri),
            at(t0, RIM),
            at(t1, RIM),
            at(inner(t1), ri)
        ));
    }
    let turn = TURN * ((s - 1.0) / (OPEN - 1.0)).clamp(0.0, 1.0) + spin;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256"><g {LAB}><g transform="rotate({turn:.2} {c} {c})">{blades}<circle cx="{c}" cy="{c}" r="{ri:.3}" fill="#fff"/><circle cx="{c}" cy="{c}" r="{:.3}" fill="#4285f4"/></g></g></svg>"##,
        ri * 10.2 / LENS
    )
}

/// The figure at rest.
pub(crate) fn rest_svg() -> String {
    figure(1.0, 0.0)
}

/// A frame's side.
pub(crate) const FRAME: u32 = 128;

/// The frame with the lens `s` times its size, the plate spun `spin`.
pub(crate) fn frame(s: f32, spin: f32) -> Option<Vec<u8>> {
    let svg = figure(s, spin);
    let tree =
        resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(FRAME, FRAME)?;
    let k = FRAME as f32 / tree.size().width();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(k, k),
        &mut pixmap.as_mut(),
    );
    Some(pixmap.take())
}

/// Chrome's motion: the lens's size, following the pointer, and a click.
pub(crate) struct Motion {
    hovered: bool,
    s: f32,
    /// The click: ms in, and the size it started from.
    click: Option<(f32, f32)>,
    /// A click was made with the pointer on it: the lens rests until the
    /// pointer leaves (Max: the hover must not come back by itself).
    spent: bool,
    /// Ms since the pointer came on it, and since the click.
    hover_t: Option<f32>,
    shot_t: Option<f32>,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            hovered: false,
            s: 1.0,
            click: None,
            spent: false,
            hover_t: None,
            shot_t: None,
        }
    }
}

impl Motion {
    /// A click on Chrome's icon.
    pub(crate) fn click(&mut self) {
        self.click = Some((0.0, self.s));
        self.spent = true;
        self.shot_t = Some(0.0);
    }

    /// Step by `dt` seconds with the pointer on the icon or not; whether
    /// anything moved.
    pub(crate) fn step(&mut self, hover: bool, dt: f32) -> bool {
        self.hovered = hover;
        if !hover {
            self.spent = false;
        }
        let ms = dt * 1000.0;
        self.hover_t = match (hover, self.hover_t) {
            (true, None) if !self.spent => Some(0.0),
            (true, Some(t)) => Some(t + ms),
            _ => None,
        };
        self.shot_t = self.shot_t.map(|t| t + ms).filter(|t| *t < CLOSE_MS + SHOT_SPIN_MS);
        let spinning = self.spin() != 0.0 || self.shot_t.is_some();
        let target = if hover && !self.spent { OPEN } else { 1.0 };
        let before = self.s;
        if let Some((t, from)) = self.click {
            let t = t + ms;
            self.s = if t < CLOSE_MS {
                from + (SHUT - from) * bezier(0.5, 0.0, 0.75, 0.0, t / CLOSE_MS)
            } else if t < CLICK_MS {
                SHUT + (target - SHUT)
                    * bezier(0.2, 0.7, 0.3, 1.15, (t - CLOSE_MS) / (CLICK_MS - CLOSE_MS))
            } else {
                target
            };
            self.click = (t < CLICK_MS).then_some((t, from));
            return true;
        }
        self.s += (target - self.s) * (1.0 - (-ms / TAU_MS).exp());
        if (target - self.s).abs() < 0.002 {
            self.s = target;
        }
        self.s != before || spinning
    }

    /// The spin now (degrees, past the plate's tilt).
    pub(crate) fn spin(&self) -> f32 {
        let run = |t: f32, ms: f32, turns: f32| {
            if (0.0..ms).contains(&t) {
                -360.0 * turns * bezier(0.45, 0.0, 0.25, 1.0, t / ms)
            } else {
                0.0
            }
        };
        let hover = match self.hover_t {
            Some(t) if !self.spent => run(t, HOVER_SPIN_MS, SPINS),
            _ => 0.0,
        };
        let shot = self.shot_t.map_or(0.0, |t| run(t - CLOSE_MS, SHOT_SPIN_MS, -SPINS));
        hover + shot
    }

    /// The lens's size now.
    pub(crate) fn pose(&self) -> f32 {
        self.s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_opening_draws() {
        for s in [SHUT, 0.5, 1.0, 1.3, OPEN] {
            assert!(frame(s, 100.0).is_some());
        }
    }

    #[test]
    fn the_outer_ends_stay_on_the_rim_whatever_the_opening() {
        let ends = |s: f32| {
            let svg = figure(s, 0.0);
            let a = svg.find("A28 28").unwrap();
            svg[a..].split('L').next().unwrap_or_default().to_string()
        };
        assert_eq!(ends(1.0), ends(OPEN));
        assert_eq!(ends(1.0), ends(SHUT));
    }

    #[test]
    fn a_hover_opens_and_holds_a_click_shuts_and_reopens() {
        let mut m = Motion::default();
        let mut most = 0.0f32;
        for _ in 0..60 {
            m.step(true, 0.016);
            most = most.min(m.spin());
        }
        assert!(most < -600.0, "two turns as it opens ({most})");
        assert_eq!(m.pose(), OPEN);
        m.click();
        let mut least = OPEN;
        for _ in 0..60 {
            m.step(true, 0.016);
            least = least.min(m.pose());
        }
        assert!(least < 0.45, "nearly shut ({least})");
        assert_eq!(m.pose(), 1.0, "back to rest, though still pointed at");
        for _ in 0..30 {
            m.step(true, 0.016);
        }
        assert_eq!(m.pose(), 1.0, "and the hover does not come back by itself");
        assert_eq!(m.spin(), 0.0);
        for _ in 0..60 {
            m.step(false, 0.016);
        }
        assert_eq!(m.pose(), 1.0);
        assert!(!m.step(false, 0.016));
    }
}

//! SPOTIFY's icon on the dock (Max, 2026-10-17, the App Motion Lab —
//! https://claude.ai/artifact/BP4E7xwMBtEWNgwqrN11Ta): the real mark, the green
//! DISC and its three black LINES, the lines drawn apart.
//!
//! A HOVER swings each line round an axis below the disc, out past its edge
//! (cut there) and back in from the other side, a little past its rest and
//! back: the top and bottom lines one way, the middle one the other, each
//! starting at its own moment. A CLICK presses the lines down together, then
//! they jump up spread apart while the disc swells, and all settle.
//!
//! Plain shapes, so a frame is drawn as it is needed (as `youtube_icon`).

use crate::youtube_icon::bezier;

/// Spotify's entry on the dock (the webapp's `.desktop` id).
pub(crate) const SPOTIFY_ID: &str = "webapp-spotify";

/// The lines (simple-icons' 24-unit square), top to bottom.
const LINES: [&str; 3] = ["M19.081 10.680 C 15.24 8.4 8.82 8.16 5.16 9.301 c -0.6 0.179 -1.2 -0.181 -1.38 -0.721 c -0.18 -0.601 0.18 -1.2 0.72 -1.381 c 4.26 -1.26 11.28 -1.02 15.721 1.621 c 0.539 0.3 0.719 1.02 0.419 1.56 c -0.299 0.421 -1.02 0.599 -1.559 0.3 z", "M18.961 14.040 c -0.301 0.42 -0.841 0.6 -1.262 0.3 c -3.239 -1.98 -8.159 -2.58 -11.939 -1.38 c -0.479 0.12 -1.02 -0.12 -1.14 -0.6 c -0.12 -0.48 0.12 -1.021 0.6 -1.141 C 9.6 9.9 15 10.561 18.72 12.84 c 0.361 0.181 0.54 0.78 0.241 1.2 z", "M17.521 17.340 c -0.24 0.359 -0.66 0.48 -1.021 0.24 c -2.82 -1.74 -6.36 -2.101 -10.561 -1.141 c -0.418 0.122 -0.779 -0.179 -0.899 -0.539 c -0.12 -0.421 0.18 -0.78 0.54 -0.9 c 4.56 -1.021 8.52 -0.6 11.64 1.32 c 0.42 0.18 0.479 0.659 0.301 1.02 z"];

/// The lab drew on a 64 square (disc r 28 at 32, the lines at 4 + 2.333×); the
/// dock's figure is that disc fitted to 80 of 100, as every theme icon is.
const LAB: &str = r#"transform="translate(10 10) scale(1.428571) translate(-4 -4)""#;
const GLYPH: &str = r#"transform="translate(4 4) scale(2.33333)""#;
/// The axis the lines swing round, in the lab's square (below the disc).
const AXIS: (f32, f32) = (32.0, 74.0);

/// Max's numbers from the lab (ms, degrees, lab units).
const SWING_MS: f32 = 1300.0;
/// Each line: when it starts, and which way it goes out (+ = clockwise).
const STARTS: [f32; 3] = [0.0, 80.0, 220.0];
const WAYS: [f32; 3] = [-1.0, 1.0, -1.0];
const OUT: f32 = 55.0;
const PAST: f32 = 8.0;
const HOVER_MS: f32 = SWING_MS + 220.0;
const CLICK_MS: f32 = 900.0;
/// The click: each line's drop and its jump (top to bottom), the disc's swell.
const DROPS: [f32; 3] = [14.0, 10.0, 6.0];
const JUMPS: [f32; 3] = [-7.0, -4.0, -1.5];
const SWELL: f32 = 1.1;

fn eased(x: f32, e: (f32, f32, f32, f32)) -> f32 {
    bezier(e.0, e.1, e.2, e.3, x)
}

/// A line's swing `t` ms after it starts, going out `way`.
fn swing(t: f32, way: f32) -> f32 {
    let p = t / SWING_MS;
    if p <= 0.0 || p >= 1.0 {
        0.0
    } else if p < 0.4 {
        way * OUT * eased(p / 0.4, (0.5, 0.0, 0.9, 0.6))
    } else if p < 0.78 {
        let from = -way * OUT;
        from + (way * PAST - from) * eased((p - 0.4) / 0.38, (0.2, 0.5, 0.4, 1.0))
    } else {
        way * PAST * (1.0 - eased((p - 0.78) / 0.22, (0.4, 0.0, 0.3, 1.0)))
    }
}

/// The click's three steps: rest → `down` → `up` → rest, `t` ms in.
fn bounce(t: f32, down: f32, up: f32) -> f32 {
    let p = t / CLICK_MS;
    if p <= 0.0 || p >= 1.0 {
        0.0
    } else if p < 0.25 {
        down * eased(p / 0.25, (0.4, 0.0, 0.6, 1.0))
    } else if p < 0.55 {
        down + (up - down) * eased((p - 0.25) / 0.3, (0.2, 0.8, 0.3, 1.0))
    } else {
        up * (1.0 - eased((p - 0.55) / 0.45, (0.4, 0.0, 0.3, 1.0)))
    }
}

/// Everything that moves.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Pose {
    /// Each line's swing (degrees) and lift (lab units, + = down).
    turn: [f32; 3],
    lift: [f32; 3],
    disc: f32,
}

const REST: Pose = Pose {
    turn: [0.0; 3],
    lift: [0.0; 3],
    disc: 1.0,
};

fn hover(t: f32) -> Pose {
    let turn = std::array::from_fn(|i| swing(t - STARTS[i], WAYS[i]));
    Pose { turn, ..REST }
}

fn click(t: f32) -> Pose {
    let lift = std::array::from_fn(|i| bounce(t, DROPS[i], JUMPS[i]));
    // The disc swells as the lines jump (from 1 at the press to `SWELL`).
    let disc = 1.0 + (SWELL - 1.0) * (bounce(t, 0.0, -1.0) / -1.0);
    Pose { lift, disc, ..REST }
}

/// The figure in pose `p`.
fn figure(p: &Pose) -> String {
    let (ax, ay) = AXIS;
    let mut lines = String::new();
    for (i, line) in LINES.iter().enumerate() {
        lines.push_str(&format!(
            r##"<g transform="translate(0 {:.3}) rotate({:.2} {ax} {ay})"><path {GLYPH} d="{line}"/></g>"##,
            p.lift[i], p.turn[i]
        ));
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256"><defs><clipPath id="d"><circle cx="32" cy="32" r="28"/></clipPath></defs><g {LAB}><circle cx="32" cy="32" r="28" fill="#1ed760" transform="translate(32 32) scale({:.4}) translate(-32 -32)"/><g clip-path="url(#d)" fill="#000">{lines}</g></g></svg>"##,
        p.disc
    )
}

/// The figure at rest.
pub(crate) fn rest_svg() -> String {
    figure(&REST)
}

/// A frame's side.
pub(crate) const FRAME: u32 = 128;

/// The frame for pose `p`.
pub(crate) fn frame(p: &Pose) -> Option<Vec<u8>> {
    let svg = figure(p);
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

#[derive(Clone, Copy)]
enum Move {
    Swing(f32),
    Press(f32),
}

/// Spotify's motion.
#[derive(Default)]
pub(crate) struct Motion {
    hovered: bool,
    now: Option<Move>,
    /// Whether the last step moved (one more step draws the rest pose).
    moving: bool,
}

impl Motion {
    /// A click on Spotify's icon.
    pub(crate) fn click(&mut self) {
        self.now = Some(Move::Press(0.0));
    }

    /// Step by `dt` seconds with the pointer on the icon or not; whether
    /// anything moved.
    pub(crate) fn step(&mut self, hover: bool, dt: f32) -> bool {
        if hover && !self.hovered && !matches!(self.now, Some(Move::Press(_))) {
            self.now = Some(Move::Swing(0.0));
        }
        self.hovered = hover;
        let ms = dt * 1000.0;
        self.now = match self.now {
            Some(Move::Swing(t)) => Some(Move::Swing(t + ms)).filter(|_| t + ms < HOVER_MS),
            Some(Move::Press(t)) => Some(Move::Press(t + ms)).filter(|_| t + ms < CLICK_MS),
            None => None,
        };
        let now = self.now.is_some();
        let moved = now || self.moving;
        self.moving = now;
        moved
    }

    /// Where everything is now.
    pub(crate) fn pose(&self) -> Pose {
        match self.now {
            None => REST,
            Some(Move::Swing(t)) => hover(t),
            Some(Move::Press(t)) => click(t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pose_draws_and_both_moves_end_at_rest() {
        for t in [0.0, 200.0, 500.0, 520.0, 800.0, 1100.0, 1400.0] {
            assert!(frame(&hover(t)).is_some());
            assert!(frame(&click(t.min(899.0))).is_some());
        }
        assert_eq!(hover(HOVER_MS), REST);
        assert_eq!(click(CLICK_MS), REST);
    }

    #[test]
    fn a_hover_sends_the_middle_line_the_other_way_and_each_passes_its_rest() {
        let out = hover(500.0);
        assert!(out.turn[0] < -40.0 && out.turn[1] > 30.0, "{out:?}");
        // Back in from the other side and past rest, the way it is going.
        let past = swing(0.78 * SWING_MS, -1.0);
        assert!((past + PAST).abs() < 1e-3, "{past}");
    }

    #[test]
    fn a_click_presses_the_lines_together_then_spreads_them_as_the_disc_swells() {
        let down = click(0.25 * CLICK_MS);
        assert!(down.lift[0] > down.lift[1] && down.lift[1] > down.lift[2], "closer together");
        let up = click(0.55 * CLICK_MS);
        assert!(up.lift[0] < up.lift[1] && up.lift[1] < up.lift[2], "spread apart");
        assert!((up.disc - SWELL).abs() < 1e-3 && (down.disc - 1.0).abs() < 1e-3);
    }
}

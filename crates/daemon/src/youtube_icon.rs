//! YOUTUBE's icon on the dock (Max, 2026-10-17, the App Motion Lab —
//! https://claude.ai/artifact/BP4E7xwMBtEWNgwqrN11Ta): the real mark, the red
//! TILE and the white PLAY on it.
//!
//! A HOVER runs a seek bar along the tile's foot while the play turns once
//! round, the tile turning `TILT` degrees the other way and back. A CLICK
//! squeezes the tile and lets it grow back while the play becomes a PAUSE
//! that swells away; then the play comes back.
//!
//! Plain shapes and no filters, so a frame is drawn as it is needed (well
//! under a millisecond at `FRAME`), not ahead like the compass's.

/// YouTube's entry on the dock (the webapp's `.desktop` id).
pub(crate) const YOUTUBE_ID: &str = "webapp-youtube";

/// The tile's way round (simple-icons' 24-unit square) and the play's.
const TILE: &str = "M23.498 6.186a3.016 3.016 0 0 0-2.122-2.136C19.505 3.545 12 3.545 12 3.545s-7.505 0-9.377.505A3.017 3.017 0 0 0 .502 6.186C0 8.07 0 12 0 12s0 3.93.502 5.814a3.016 3.016 0 0 0 2.122 2.136c1.871.505 9.376.505 9.376.505s7.505 0 9.377-.505a3.015 3.015 0 0 0 2.122-2.136C24 15.93 24 12 24 12s0-3.93-.502-5.814z";
const PLAY: &str = "M9.545 15.568V8.432L15.818 12z";
/// The logo fitted to 80 of the figure's 100, as every theme icon is.
const FIT: &str = r#"transform="translate(10 10) scale(3.33333)""#;
/// Where the play turns (its centroid) and where the rest turn and swell.
const PLAY_AT: (f32, f32) = (48.79, 50.0);
const MID: (f32, f32) = (50.0, 50.0);

/// Max's numbers from the lab (ms, degrees, scales).
const SEEK_MS: f32 = 1100.0;
const TURN_MS: f32 = 900.0;
const TILT: f32 = -20.0;
const PRESS_MS: f32 = 1100.0;
const SQUEEZE: f32 = 0.8;
const SWELL: f32 = 2.4;

/// Everything that moves.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Pose {
    tile_turn: f32,
    tile_scale: f32,
    /// The seek bar: how far filled (0..1) and how seen.
    bar: f32,
    bar_alpha: f32,
    play_turn: f32,
    play_scale: f32,
    play_alpha: f32,
    pause_scale: f32,
    pause_alpha: f32,
}

const REST: Pose = Pose {
    tile_turn: 0.0,
    tile_scale: 1.0,
    bar: 0.0,
    bar_alpha: 0.0,
    play_turn: 0.0,
    play_scale: 1.0,
    play_alpha: 1.0,
    pause_scale: 1.0,
    pause_alpha: 0.0,
};

/// The figure in pose `p`.
fn figure(p: &Pose) -> String {
    let (mx, my) = MID;
    let (px, py) = PLAY_AT;
    let fill = 17.6 * p.bar.clamp(0.0, 1.0);
    let tile = format!(
        r##"<g transform="translate({mx} {my}) rotate({:.2}) scale({:.4}) translate(-{mx} -{my})"><g {FIT}><path fill="#ff0000" d="{TILE}"/><g opacity="{:.3}"><rect x="3.2" y="17.6" width="17.6" height=".9" rx=".45" fill="#fff" opacity=".35"/><rect x="3.2" y="17.6" width="{fill:.3}" height=".9" rx=".45" fill="#fff"/></g></g></g>"##,
        p.tile_turn, p.tile_scale, p.bar_alpha
    );
    let play = format!(
        r##"<g opacity="{:.3}" transform="translate({px} {py}) rotate({:.2}) scale({:.4}) translate(-{px} -{py})"><path fill="#fff" {FIT} d="{PLAY}"/></g>"##,
        p.play_alpha, p.play_turn, p.play_scale
    );
    let pause = format!(
        r##"<g opacity="{:.3}" transform="translate({mx} {my}) scale({:.4}) translate(-{mx} -{my})"><g {FIT} fill="#fff"><rect x="9.3" y="8.6" width="2" height="6.8" rx=".4"/><rect x="12.7" y="8.6" width="2" height="6.8" rx=".4"/></g></g>"##,
        p.pause_alpha, p.pause_scale
    );
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256">{tile}{play}{pause}</svg>"##
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

/// A cubic Bézier easing (CSS's `cubic-bezier(x1, y1, x2, y2)`) at `x`.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let at = |a: f32, b: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if at(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(y1, y2, (lo + hi) / 2.0)
}

/// From `a` to `b` between `t0` and `t1` ms, eased by `e`.
fn span(t: f32, t0: f32, t1: f32, a: f32, b: f32, e: impl Fn(f32) -> f32) -> f32 {
    a + (b - a) * e((t - t0) / (t1 - t0))
}

fn seek(t: f32) -> Pose {
    let turn = 360.0 * bezier(0.45, 0.0, 0.2, 1.0, t / TURN_MS);
    let tilt = if t < 0.25 * SEEK_MS {
        span(t, 0.0, 0.25 * SEEK_MS, 0.0, TILT, |x| bezier(0.3, 0.0, 0.3, 1.0, x))
    } else {
        span(t, 0.25 * SEEK_MS, 0.82 * SEEK_MS, TILT, 0.0, |x| bezier(0.4, 0.0, 0.3, 1.0, x))
    };
    let ease = |x: f32| bezier(0.25, 0.1, 0.25, 1.0, x);
    let bar_alpha = if t < 0.1 * SEEK_MS {
        span(t, 0.0, 0.1 * SEEK_MS, 0.0, 1.0, ease)
    } else if t < 0.8 * SEEK_MS {
        1.0
    } else {
        span(t, 0.8 * SEEK_MS, SEEK_MS, 1.0, 0.0, ease)
    };
    Pose {
        tile_turn: tilt,
        bar: t / TURN_MS,
        bar_alpha,
        play_turn: turn % 360.0,
        ..REST
    }
}

fn press(t: f32) -> Pose {
    let d = PRESS_MS;
    let back = |x: f32| bezier(0.3, 0.7, 0.4, 1.0, x);
    let out = |x: f32| bezier(0.0, 0.0, 0.58, 1.0, x);
    let tile_scale = if t < 0.08 * d {
        span(t, 0.0, 0.08 * d, 1.0, SQUEEZE, |x| bezier(0.4, 0.0, 1.0, 1.0, x))
    } else {
        span(t, 0.08 * d, 0.6 * d, SQUEEZE, 1.0, |x| bezier(0.2, 0.7, 0.3, 1.0, x))
    };
    let (play_scale, play_alpha) = if t < 0.08 * d {
        let k = back(t / (0.08 * d));
        (1.0 - 0.4 * k, 1.0 - k)
    } else if t < 0.62 * d {
        (0.6, 0.0)
    } else if t < 0.85 * d {
        let k = back((t - 0.62 * d) / (0.23 * d));
        (0.6 + 0.52 * k, k)
    } else {
        (span(t, 0.85 * d, d, 1.12, 1.0, back), 1.0)
    };
    let (pause_scale, pause_alpha) = if t < 0.1 * d {
        let k = out(t / (0.1 * d));
        (0.8 + 0.2 * k, k)
    } else if t < 0.6 * d {
        let k = out((t - 0.1 * d) / (0.5 * d));
        (1.0 + (SWELL - 1.0) * k, 1.0 - k)
    } else {
        (SWELL, 0.0)
    };
    Pose {
        tile_scale,
        play_scale,
        play_alpha,
        pause_scale,
        pause_alpha,
        ..REST
    }
}

#[derive(Clone, Copy)]
enum Move {
    Seek(f32),
    Press(f32),
}

/// YouTube's motion.
#[derive(Default)]
pub(crate) struct Motion {
    hovered: bool,
    now: Option<Move>,
    /// Whether the last step moved (one more step draws the rest pose).
    moving: bool,
}

impl Motion {
    /// A click on YouTube's icon.
    pub(crate) fn click(&mut self) {
        self.now = Some(Move::Press(0.0));
    }

    /// Step by `dt` seconds with the pointer on the icon or not; whether
    /// anything moved.
    pub(crate) fn step(&mut self, hover: bool, dt: f32) -> bool {
        if hover && !self.hovered && !matches!(self.now, Some(Move::Press(_))) {
            self.now = Some(Move::Seek(0.0));
        }
        self.hovered = hover;
        let ms = dt * 1000.0;
        self.now = match self.now {
            Some(Move::Seek(t)) => Some(Move::Seek(t + ms)).filter(|_| t + ms < SEEK_MS),
            Some(Move::Press(t)) => Some(Move::Press(t + ms)).filter(|_| t + ms < PRESS_MS),
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
            Some(Move::Seek(t)) => seek(t),
            Some(Move::Press(t)) => press(t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pose_parses_and_draws() {
        for t in [0.0, 100.0, 300.0, 600.0, 900.0, 1099.0] {
            assert!(frame(&seek(t)).is_some());
            assert!(frame(&press(t)).is_some());
        }
    }

    #[test]
    fn a_hover_turns_the_play_round_the_tile_back_and_runs_the_bar() {
        let mut m = Motion::default();
        let (mut tilt, mut turned, mut bar) = (0.0f32, 0.0f32, 0.0f32);
        for _ in 0..100 {
            m.step(true, 0.016);
            let p = m.pose();
            tilt = tilt.min(p.tile_turn);
            turned = turned.max(p.play_turn);
            bar = bar.max(p.bar * p.bar_alpha);
        }
        assert!(tilt < -19.0 && turned > 300.0 && bar > 0.9, "{tilt} {turned} {bar}");
        assert_eq!(m.pose(), REST);
        assert!(!m.step(true, 0.016), "once a hover");
    }

    #[test]
    fn a_click_squeezes_the_tile_swells_the_pause_and_brings_the_play_back() {
        let mut m = Motion::default();
        m.click();
        let (mut least, mut swell, mut gone) = (1.0f32, 0.0f32, false);
        for _ in 0..100 {
            m.step(false, 0.016);
            let p = m.pose();
            least = least.min(p.tile_scale);
            if p.pause_alpha > 0.05 {
                swell = swell.max(p.pause_scale);
            }
            gone |= p.play_alpha < 0.01;
        }
        assert!(least < 0.82 && swell > 1.8 && gone, "{least} {swell}");
        assert_eq!(m.pose(), REST);
    }
}

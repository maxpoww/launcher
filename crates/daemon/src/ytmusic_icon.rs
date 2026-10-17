//! YOUTUBE MUSIC's icon on the dock (Max, 2026-10-17, the App Motion Lab —
//! https://claude.ai/artifact/BP4E7xwMBtEWNgwqrN11Ta): the real mark, the red
//! DISC, the white RING on it and the white PLAY in the ring.
//!
//! A HOVER: the play swells and SWALLOWS the ring, turns once round fast at
//! its biggest, gulps back down, and the ring comes back in from outside. A
//! CLICK: the disc beats (as YouTube's tile does) while a PAUSE swells and
//! pushes the ring off the disc; the play comes back and SPITS the ring out.
//!
//! Plain shapes, so a frame is drawn as it is needed (as `youtube_icon`).
//! The motion is the lab's keyframes, each with its own easing.

use crate::youtube_icon::bezier;

/// YouTube Music's entry on the dock (the webapp's `.desktop` id).
pub(crate) const YTMUSIC_ID: &str = "webapp-youtube-music";

/// The logo fitted to 80 of the figure's 100, as every theme icon is.
const FIT: &str = r#"transform="translate(10 10) scale(3.33333)""#;
/// Where the play turns and swells (its centroid); everything else, the middle.
const PLAY_AT: (f32, f32) = (49.09, 50.0);
const MID: f32 = 50.0;

const GULP_MS: f32 = 1200.0;
const SPIT_MS: f32 = 1400.0;

/// An easing: CSS's `cubic-bezier`, or `ease`.
type Ease = (f32, f32, f32, f32);
const EASE: Ease = (0.25, 0.1, 0.25, 1.0);
const OUT: Ease = (0.0, 0.0, 0.58, 1.0);

/// A keyframe: (percent of the move, value, easing on to the next).
type Key = (f32, f32, Ease);

/// A track's value at `pct` percent.
fn track(keys: &[Key], pct: f32) -> f32 {
    let pct = pct.clamp(0.0, 100.0);
    for w in keys.windows(2) {
        let ((p0, v0, (a, b, c, d)), (p1, v1, _)) = (w[0], w[1]);
        if pct <= p1 {
            return v0 + (v1 - v0) * bezier(a, b, c, d, (pct - p0) / (p1 - p0));
        }
    }
    keys.last().map_or(0.0, |k| k.1)
}

/// Everything that moves.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Pose {
    disc_scale: f32,
    ring_scale: f32,
    ring_alpha: f32,
    play_scale: f32,
    play_turn: f32,
    play_alpha: f32,
    pause_scale: f32,
    pause_alpha: f32,
}

const REST: Pose = Pose {
    disc_scale: 1.0,
    ring_scale: 1.0,
    ring_alpha: 1.0,
    play_scale: 1.0,
    play_turn: 0.0,
    play_alpha: 1.0,
    pause_scale: 1.0,
    pause_alpha: 0.0,
};

fn gulp(pct: f32) -> Pose {
    let ring_in = (0.5, 0.0, 0.8, 0.4);
    let back = (0.2, 0.7, 0.3, 1.0);
    Pose {
        ring_scale: track(
            &[(0.0, 1.0, ring_in), (32.0, 0.15, EASE), (50.0, 1.9, back), (85.0, 0.96, OUT), (100.0, 1.0, EASE)],
            pct,
        ),
        ring_alpha: track(
            &[(0.0, 1.0, ring_in), (32.0, 0.0, EASE), (50.0, 0.0, back), (85.0, 1.0, OUT), (100.0, 1.0, EASE)],
            pct,
        ),
        play_scale: track(
            &[
                (0.0, 1.0, (0.4, 0.0, 0.6, 1.0)),
                (22.0, 1.65, (0.5, 0.0, 0.2, 1.0)),
                (40.0, 1.7, (0.4, 0.0, 0.6, 1.0)),
                (52.0, 0.9, OUT),
                (64.0, 1.0, EASE),
                (100.0, 1.0, EASE),
            ],
            pct,
        ),
        play_turn: track(
            &[(0.0, 0.0, EASE), (22.0, 0.0, (0.5, 0.0, 0.2, 1.0)), (40.0, 360.0, EASE), (100.0, 360.0, EASE)],
            pct,
        ) % 360.0,
        ..REST
    }
}

fn spit(pct: f32) -> Pose {
    let push = (0.3, 0.0, 0.4, 1.0);
    let back = (0.2, 0.7, 0.3, 1.0);
    let out = (0.15, 0.8, 0.3, 1.15);
    Pose {
        disc_scale: track(
            &[(0.0, 1.0, (0.4, 0.0, 1.0, 1.0)), (6.0, 0.8, back), (34.0, 1.0, EASE), (100.0, 1.0, EASE)],
            pct,
        ),
        play_scale: track(
            &[
                (0.0, 1.0, EASE),
                (8.0, 1.4, EASE),
                (40.0, 0.3, back),
                (52.0, 1.25, (0.5, 0.0, 0.5, 1.0)),
                (60.0, 0.85, EASE),
                (72.0, 1.0, EASE),
                (100.0, 1.0, EASE),
            ],
            pct,
        ),
        play_alpha: track(
            &[(0.0, 1.0, EASE), (8.0, 0.0, EASE), (40.0, 0.0, back), (52.0, 1.0, EASE), (100.0, 1.0, EASE)],
            pct,
        ),
        pause_scale: track(&[(0.0, 0.8, EASE), (8.0, 1.2, push), (34.0, 2.5, EASE), (100.0, 2.5, EASE)], pct),
        pause_alpha: track(&[(0.0, 0.0, EASE), (8.0, 1.0, push), (34.0, 0.0, EASE), (100.0, 0.0, EASE)], pct),
        ring_scale: track(
            &[
                (0.0, 1.0, push),
                (34.0, 2.2, EASE),
                (54.0, 0.1, out),
                (58.0, 0.35, EASE),
                (88.0, 1.04, EASE),
                (100.0, 1.0, EASE),
            ],
            pct,
        ),
        ring_alpha: track(
            &[(0.0, 1.0, push), (34.0, 0.0, EASE), (54.0, 0.0, out), (58.0, 1.0, EASE), (100.0, 1.0, EASE)],
            pct,
        ),
        ..REST
    }
}

/// `transform` turning and scaling about (x, y).
fn about(x: f32, y: f32, turn: f32, scale: f32) -> String {
    format!(r#"transform="translate({x} {y}) rotate({turn:.2}) scale({scale:.4}) translate(-{x} -{y})""#)
}

/// The figure in pose `p`.
fn figure(p: &Pose) -> String {
    let (px, py) = PLAY_AT;
    let disc = format!(
        r##"<g {}><g {FIT}><circle cx="12" cy="12" r="12" fill="#ff0000"/></g></g>"##,
        about(MID, MID, 0.0, p.disc_scale)
    );
    let ring = format!(
        r##"<g opacity="{:.3}" {}><circle {FIT} cx="12" cy="12" r="6.666" fill="none" stroke="#fff" stroke-width=".876"/></g>"##,
        p.ring_alpha.clamp(0.0, 1.0),
        about(MID, MID, 0.0, p.ring_scale.max(0.0))
    );
    let play = format!(
        r##"<g opacity="{:.3}" {}><path {FIT} fill="#fff" d="M9.684 15.54V8.46L15.816 12z"/></g>"##,
        p.play_alpha.clamp(0.0, 1.0),
        about(px, py, p.play_turn, p.play_scale.max(0.0))
    );
    let pause = format!(
        r##"<g opacity="{:.3}" {}><g {FIT} fill="#fff"><rect x="9.6" y="8.7" width="1.9" height="6.6" rx=".4"/><rect x="12.6" y="8.7" width="1.9" height="6.6" rx=".4"/></g></g>"##,
        p.pause_alpha.clamp(0.0, 1.0),
        about(MID, MID, 0.0, p.pause_scale.max(0.0))
    );
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256">{disc}{ring}{play}{pause}</svg>"##
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
    Gulp(f32),
    Spit(f32),
}

/// YouTube Music's motion.
#[derive(Default)]
pub(crate) struct Motion {
    hovered: bool,
    now: Option<Move>,
    /// Whether the last step moved (one more step draws the rest pose).
    moving: bool,
}

impl Motion {
    /// A click on YouTube Music's icon.
    pub(crate) fn click(&mut self) {
        self.now = Some(Move::Spit(0.0));
    }

    /// Step by `dt` seconds with the pointer on the icon or not; whether
    /// anything moved.
    pub(crate) fn step(&mut self, hover: bool, dt: f32) -> bool {
        if hover && !self.hovered && !matches!(self.now, Some(Move::Spit(_))) {
            self.now = Some(Move::Gulp(0.0));
        }
        self.hovered = hover;
        let ms = dt * 1000.0;
        self.now = match self.now {
            Some(Move::Gulp(t)) => Some(Move::Gulp(t + ms)).filter(|_| t + ms < GULP_MS),
            Some(Move::Spit(t)) => Some(Move::Spit(t + ms)).filter(|_| t + ms < SPIT_MS),
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
            Some(Move::Gulp(t)) => gulp(100.0 * t / GULP_MS),
            Some(Move::Spit(t)) => spit(100.0 * t / SPIT_MS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_moves_start_and_end_at_rest_and_draw() {
        for f in [gulp as fn(f32) -> Pose, spit] {
            for pct in [0.0, 10.0, 30.0, 45.0, 55.0, 70.0, 90.0] {
                assert!(frame(&f(pct)).is_some());
            }
            let end = f(100.0);
            assert!((end.ring_scale - 1.0).abs() < 1e-3 && (end.play_scale - 1.0).abs() < 1e-3);
            assert!(end.play_turn.abs() < 1e-3 && end.pause_alpha.abs() < 1e-3);
        }
    }

    #[test]
    fn a_hover_swallows_the_ring_and_spins_the_play_at_its_biggest() {
        let mid = gulp(32.0);
        assert!(mid.ring_alpha < 0.05 && mid.play_scale > 1.6, "{mid:?}");
        let spin = gulp(31.0).play_turn;
        assert!(spin > 90.0 && spin < 300.0, "turning at its biggest ({spin})");
    }

    #[test]
    fn a_click_beats_the_disc_and_spits_the_ring_back() {
        let mut m = Motion::default();
        m.click();
        let (mut least, mut pushed, mut small) = (1.0f32, 0.0f32, false);
        for _ in 0..120 {
            m.step(false, 0.016);
            let p = m.pose();
            least = least.min(p.disc_scale);
            if p.ring_alpha > 0.05 {
                pushed = pushed.max(p.ring_scale);
                small |= p.ring_scale < 0.5;
            }
        }
        assert!(least < 0.82 && pushed > 1.3 && small, "{least} {pushed} {small}");
        assert_eq!(m.pose(), REST);
    }
}

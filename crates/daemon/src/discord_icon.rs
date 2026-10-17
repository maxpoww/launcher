//! DISCORD's icon on the dock (Max, 2026-10-17, the App Motion Lab —
//! https://claude.ai/artifact/BP4E7xwMBtEWNgwqrN11Ta): the real mark, the
//! blurple TILE and CLYDE on it, his eyes drawn apart (the logo cuts them out
//! of the face; here the face is whole and the eyes lie on it, so they can
//! grow and close).
//!
//! A HOVER tips Clyde 20° to the right and back, his right eye closing with
//! the tip (shut at the low point) and opening as he comes up. A CLICK drops
//! him and lets him rise while his eyes grow a fifth and come back; once he
//! has landed both eyes blink.
//!
//! Plain shapes, so a frame is drawn as it is needed (as `youtube_icon`).

use crate::youtube_icon::bezier;

/// Discord's entry on the dock (the webapp's `.desktop` id).
pub(crate) const DISCORD_ID: &str = "webapp-discord";

/// The logo's paths, on simple-icons' 24-unit square.
const FACE: &str = "M20.317 4.3698a19.7913 19.7913 0 00-4.8851-1.5152.0741.0741 0 00-.0785.0371c-.211.3753-.4447.8648-.6083 1.2495-1.8447-.2762-3.68-.2762-5.4868 0-.1636-.3933-.4058-.8742-.6177-1.2495a.077.077 0 00-.0785-.037 19.7363 19.7363 0 00-4.8852 1.515.0699.0699 0 00-.0321.0277C.5334 9.0458-.319 13.5799.0992 18.0578a.0824.0824 0 00.0312.0561c2.0528 1.5076 4.0413 2.4228 5.9929 3.0294a.0777.0777 0 00.0842-.0276c.4616-.6304.8731-1.2952 1.226-1.9942a.076.076 0 00-.0416-.1057c-.6528-.2476-1.2743-.5495-1.8722-.8923a.077.077 0 01-.0076-.1277c.1258-.0943.2517-.1923.3718-.2914a.0743.0743 0 01.0776-.0105c3.9278 1.7933 8.18 1.7933 12.0614 0a.0739.0739 0 01.0785.0095c.1202.099.246.1981.3728.2924a.077.077 0 01-.0066.1276 12.2986 12.2986 0 01-1.873.8914.0766.0766 0 00-.0407.1067c.3604.698.7719 1.3628 1.225 1.9932a.076.076 0 00.0842.0286c1.961-.6067 3.9495-1.5219 6.0023-3.0294a.077.077 0 00.0313-.0552c.5004-5.177-.8382-9.6739-3.5485-13.6604a.061.061 0 00-.0312-.0286z";
const LEFT: &str = "M8.02 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9555-2.4189 2.157-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.9555 2.4189-2.1569 2.4189z";
const RIGHT: &str = "M15.9948 15.3312c-1.1825 0-2.1569-1.0857-2.1569-2.419 0-1.3332.9554-2.4189 2.1569-2.4189 1.2108 0 2.1757 1.0952 2.1568 2.419 0 1.3332-.946 2.4189-2.1568 2.4189Z";
/// The eyes' centres there.
const LEFT_AT: (f32, f32) = (8.02, 12.912);
const RIGHT_AT: (f32, f32) = (15.995, 12.912);

/// The lab drew on a 64 square (tile 4..60, Clyde at 13 + 1.5833×); the dock's
/// figure is that tile fitted to 80 of 100, as every theme icon is.
const LAB: &str = r#"transform="translate(10 10) scale(1.428571) translate(-4 -4)""#;
const CLYDE: &str = r#"transform="translate(13 13) scale(1.5833)""#;
/// Where Clyde tips, in the lab's square.
const TIP_AT: (f32, f32) = (32.0, 32.6);

/// Max's numbers from the lab (ms; the hover's are its own clock).
const HOVER_MS: f32 = 900.0;
const HOVER_PACE: f32 = 0.9;
const TIP: f32 = 20.0;
const CLICK_MS: f32 = 1100.0;
const DROP: f32 = 5.0;
const GROW: f32 = 0.2;
/// How far an eye closes (its height left).
const SHUT: f32 = 0.2;

fn ease(t: f32, t0: f32, t1: f32, e: (f32, f32, f32, f32)) -> f32 {
    bezier(e.0, e.1, e.2, e.3, (t - t0) / (t1 - t0))
}

/// Everything that moves.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Pose {
    tip: f32,
    drop: f32,
    /// Each eye's size and how open (left, right).
    grow: f32,
    open: (f32, f32),
}

const REST: Pose = Pose {
    tip: 0.0,
    drop: 0.0,
    grow: 1.0,
    open: (1.0, 1.0),
};

fn hover(ms: f32) -> Pose {
    let t = ms / HOVER_PACE;
    let tip = if t < 400.0 {
        TIP * ease(t, 0.0, 400.0, (0.3, 0.0, 0.3, 1.0))
    } else if t < 600.0 {
        TIP
    } else if t < 1000.0 {
        TIP - TIP * ease(t, 600.0, 1000.0, (0.4, 0.0, 0.3, 1.0))
    } else {
        0.0
    };
    Pose {
        tip,
        open: (1.0, 1.0 - (1.0 - SHUT) * tip / TIP),
        ..REST
    }
}

fn click(t: f32) -> Pose {
    let grow = if t < 150.0 {
        1.0 + GROW * ease(t, 0.0, 150.0, (0.3, 0.0, 0.3, 1.0))
    } else if t < 380.0 {
        1.0 + GROW
    } else if t < 620.0 {
        1.0 + GROW - GROW * ease(t, 380.0, 620.0, (0.4, 0.0, 0.3, 1.0))
    } else {
        1.0
    };
    let drop = if t < 220.0 {
        DROP * ease(t, 0.0, 220.0, (0.4, 0.0, 0.3, 1.0))
    } else if t < 800.0 {
        DROP - DROP * ease(t, 220.0, 800.0, (0.3, 0.0, 0.2, 1.0))
    } else {
        0.0
    };
    let blink = if t < 800.0 {
        1.0
    } else if t < 870.0 {
        1.0 - (1.0 - SHUT) * ease(t, 800.0, 870.0, (0.4, 0.0, 0.6, 1.0))
    } else if t < 960.0 {
        SHUT
    } else if t < 1080.0 {
        SHUT + (1.0 - SHUT) * ease(t, 960.0, 1080.0, (0.3, 0.0, 0.3, 1.0))
    } else {
        1.0
    };
    Pose {
        drop,
        grow,
        open: (blink, blink),
        ..REST
    }
}

/// An eye `d` about its centre `at`, `grow` big and `open` of its height.
fn eye(d: &str, at: (f32, f32), grow: f32, open: f32) -> String {
    let (x, y) = at;
    format!(
        r##"<path fill="#5865f2" transform="translate({x} {y}) scale({grow:.4} {:.4}) translate(-{x} -{y})" d="{d}"/>"##,
        grow * open
    )
}

/// The figure in pose `p`.
fn figure(p: &Pose) -> String {
    let (tx, ty) = TIP_AT;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256"><g {LAB}><rect x="4" y="4" width="56" height="56" rx="14" fill="#5865f2"/><g transform="translate(0 {:.3}) rotate({:.2} {tx} {ty})"><g {CLYDE}><path fill="#fff" d="{FACE}"/>{}{}</g></g></g></svg>"##,
        p.drop,
        p.tip,
        eye(LEFT, LEFT_AT, p.grow, p.open.0),
        eye(RIGHT, RIGHT_AT, p.grow, p.open.1),
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
    Wink(f32),
    Press(f32),
}

/// Discord's motion.
#[derive(Default)]
pub(crate) struct Motion {
    hovered: bool,
    now: Option<Move>,
    /// Whether the last step moved (one more step draws the rest pose).
    moving: bool,
}

impl Motion {
    /// A click on Discord's icon.
    pub(crate) fn click(&mut self) {
        self.now = Some(Move::Press(0.0));
    }

    /// Step by `dt` seconds with the pointer on the icon or not; whether
    /// anything moved.
    pub(crate) fn step(&mut self, hover: bool, dt: f32) -> bool {
        if hover && !self.hovered && !matches!(self.now, Some(Move::Press(_))) {
            self.now = Some(Move::Wink(0.0));
        }
        self.hovered = hover;
        let ms = dt * 1000.0;
        self.now = match self.now {
            Some(Move::Wink(t)) => Some(Move::Wink(t + ms)).filter(|_| t + ms < HOVER_MS),
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
            Some(Move::Wink(t)) => hover(t),
            Some(Move::Press(t)) => click(t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pose_draws_and_both_moves_end_at_rest() {
        for t in [0.0, 150.0, 300.0, 500.0, 800.0, 900.0, 1000.0] {
            assert!(frame(&hover(t)).is_some());
            assert!(frame(&click(t)).is_some());
        }
        assert_eq!(hover(HOVER_MS), REST);
        assert_eq!(click(CLICK_MS), REST);
    }

    #[test]
    fn a_hover_shuts_the_right_eye_at_the_low_point() {
        let low = hover(450.0);
        assert!((low.tip - TIP).abs() < 1e-3, "{low:?}");
        assert!((low.open.1 - SHUT).abs() < 1e-3 && low.open.0 == 1.0);
    }

    #[test]
    fn a_click_drops_grows_and_blinks_after_landing() {
        assert!(click(220.0).drop > DROP - 0.01 && click(200.0).grow > 1.19);
        let landed = click(900.0);
        assert!(landed.drop == 0.0 && landed.grow == 1.0 && landed.open == (SHUT, SHUT));
    }
}

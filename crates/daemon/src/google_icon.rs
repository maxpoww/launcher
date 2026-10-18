//! GOOGLE's icon on the dock (Max, 2026-10-17, the App Motion Lab —
//! https://claude.ai/artifact/BP4E7xwMBtEWNgwqrN11Ta): the real four-colour G
//! on its white disc, the blue BAR drawn apart as a clock's hand.
//!
//! A HOVER sweeps the bar once round clockwise, erasing the G behind it, and
//! without stopping the G grows back clockwise from it. A CLICK spins the
//! whole G four times round the other way, fast.
//!
//! Plain shapes, so a frame is drawn as it is needed (as `youtube_icon`).

use crate::youtube_icon::bezier;

/// Google's entry on the dock (the webapp's `.desktop` id).
pub(crate) const GOOGLE_ID: &str = "webapp-google";

/// The G (its 48-unit square, centre 24,24).
const RED: &str = "M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z";
const BLUE: &str = "M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z";
const YELLOW: &str = "M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z";
const GREEN: &str = "M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z";
/// Where the hand turns (the bar's root).
const ROOT: (f32, f32) = (24.0, 24.5);

/// The lab drew on a 64 square (disc r 28 at 32, the G ×0.8587 about it); the
/// dock's figure is that disc fitted to 80 of 100, as every theme icon is.
const LAB: &str = r#"transform="translate(10 10) scale(1.428571) translate(-4 -4)""#;
const G_SCALE: f32 = 0.8587;

/// Max's numbers from the lab.
const HOVER_MS: f32 = 1400.0;
const CLICK_MS: f32 = 800.0;
const TURNS: f32 = 4.0;

/// Everything that moves.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Pose {
    /// The hand's angle (degrees clockwise from pointing right).
    hand: f32,
    /// The part of the G away: clockwise from `cut.0` to `cut.1` (none when equal).
    cut: (f32, f32),
    /// The whole G's turn.
    spin: f32,
}

const REST: Pose = Pose {
    hand: 0.0,
    cut: (0.0, 0.0),
    spin: 0.0,
};

fn hover(t: f32) -> Pose {
    if t >= HOVER_MS {
        return REST;
    }
    // One continuous sweep: round erasing (0..360), round growing back (360..720).
    let u = 720.0 * bezier(0.4, 0.0, 0.6, 1.0, t / HOVER_MS);
    if u < 360.0 {
        Pose { hand: u, cut: (-4.0, u), ..REST }
    } else {
        Pose { cut: (u - 364.0, 356.0), ..REST }
    }
}

fn click(t: f32) -> Pose {
    if t >= CLICK_MS {
        return REST;
    }
    Pose {
        spin: -360.0 * TURNS * bezier(0.3, 0.0, 0.15, 1.0, t / CLICK_MS),
        ..REST
    }
}

/// The wedge clockwise from `a0` to `a1` about the hand's root, big enough to
/// hold the whole G.
fn wedge(a0: f32, a1: f32) -> String {
    let (cx, cy) = ROOT;
    if a1 - a0 <= 0.0 {
        return String::new();
    }
    if a1 - a0 >= 359.9 {
        return r##"<path d="M-10 -10H58V58H-10Z" fill="#000"/>"##.into();
    }
    let at = |d: f32| {
        let r = d.to_radians();
        format!("{:.2} {:.2}", cx + 40.0 * r.cos(), cy + 40.0 * r.sin())
    };
    format!(
        r##"<path d="M{cx} {cy}L{}A40 40 0 {} 1 {}Z" fill="#000"/>"##,
        at(a0),
        u8::from(a1 - a0 > 180.0),
        at(a1)
    )
}

/// The figure in pose `p`.
fn figure(p: &Pose) -> String {
    let (rx, ry) = ROOT;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256"><g {LAB}><circle cx="32" cy="32" r="28" fill="#fff"/><g transform="translate(32 32) rotate({:.2}) scale({G_SCALE}) translate(-24 -24)"><defs><mask id="m" maskUnits="userSpaceOnUse" x="-10" y="-10" width="68" height="68"><rect x="-10" y="-10" width="68" height="68" fill="#fff"/>{}<rect x="23.6" y="19.4" width="25" height="9.62" fill="#000"/></mask></defs><g mask="url(#m)"><path fill="#EA4335" d="{RED}"/><path fill="#4285F4" d="{BLUE}"/><path fill="#FBBC05" d="{YELLOW}"/><path fill="#34A853" d="{GREEN}"/></g><rect x="24" y="20" width="22.6" height="9.02" fill="#4285F4" transform="rotate({:.2} {rx} {ry})"/></g></g></svg>"##,
        p.spin,
        wedge(p.cut.0, p.cut.1),
        p.hand
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
    Sweep(f32),
    Spin(f32),
}

/// Google's motion.
#[derive(Default)]
pub(crate) struct Motion {
    hovered: bool,
    now: Option<Move>,
    /// Whether the last step moved (one more step draws the rest pose).
    moving: bool,
}

impl Motion {
    /// A click on Google's icon.
    pub(crate) fn click(&mut self) {
        self.now = Some(Move::Spin(0.0));
    }

    /// Step by `dt` seconds with the pointer on the icon or not; whether
    /// anything moved.
    pub(crate) fn step(&mut self, hover: bool, dt: f32) -> bool {
        if hover && !self.hovered && !matches!(self.now, Some(Move::Spin(_))) {
            self.now = Some(Move::Sweep(0.0));
        }
        self.hovered = hover;
        let ms = dt * 1000.0;
        self.now = match self.now {
            Some(Move::Sweep(t)) => Some(Move::Sweep(t + ms)).filter(|_| t + ms < HOVER_MS),
            Some(Move::Spin(t)) => Some(Move::Spin(t + ms)).filter(|_| t + ms < CLICK_MS),
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
            Some(Move::Sweep(t)) => hover(t),
            Some(Move::Spin(t)) => click(t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pose_draws() {
        for t in [0.0, 200.0, 500.0, 700.0, 701.0, 1000.0, 1300.0] {
            assert!(frame(&hover(t)).is_some());
            assert!(frame(&click(t.min(799.0))).is_some());
        }
    }

    #[test]
    fn the_hover_is_one_sweep_the_bar_alone_halfway() {
        let half = hover(HOVER_MS / 2.0);
        assert!(half.cut.1 - half.cut.0 >= 355.0, "all away at the turn: {half:?}");
        assert_eq!(hover(HOVER_MS), REST);
        let early = hover(300.0);
        assert!(early.hand > 0.0 && (early.cut.1 - early.hand).abs() < 1e-3);
    }

    #[test]
    fn a_click_spins_four_times_the_other_way_and_stops_at_rest() {
        let mut m = Motion::default();
        m.click();
        let mut most = 0.0f32;
        for _ in 0..60 {
            m.step(false, 0.016);
            most = most.min(m.pose().spin);
        }
        assert!(most < -1400.0, "{most}");
        assert_eq!(m.pose(), REST);
    }
}

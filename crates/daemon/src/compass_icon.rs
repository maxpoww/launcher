//! SEAM's icon on the dock: a COMPASS (Max, 2026-10-16, the Dock Motion Lab —
//! https://claude.ai/artifact/Pn8Ao3JUeU6JKDiwxUhobo). A bezel in the dock's
//! cast, its rose GROOVED in the face (a ring, the four cardinal ticks long,
//! the four between short), and a two-tone needle on Safari's slant, pointing
//! at a long tick. A hover nudges it: the needle swings side to side, the
//! swing dying out back onto its slant, as a real compass's does.
//!
//! As the gears are, the figure is drawn whole for each angle of the needle
//! (the light fixed) — once per look, a frame a degree over the swing's
//! reach, on a low-priority thread — and shown by the needle's angle.

use std::sync::{Arc, Mutex, OnceLock};

/// Seam's entry on the dock (its `.desktop` id).
pub(crate) const SEAM_ID: &str = "seam";

/// Max's numbers from the lab: the needle's slant at rest (degrees from up,
/// clockwise), the swing (degrees each way), one swing there and back (ms),
/// and how long until it is still (ms).
const SLANT: f32 = 45.0;
const SWING: f32 = 30.0;
const PERIOD_MS: f32 = 560.0;
const SETTLE_MS: f32 = 2000.0;

/// The figure with its needle at `angle` degrees, in the look's tone.
pub(crate) fn svg(angle: f32) -> String {
    let (stops, gloss, hi) = if crate::look::dark() {
        (["#9a9fa8", "#8d929b", "#757a83", "#666a73"], 0.12, 0.25)
    } else {
        (["#fefefe", "#fafafa", "#eeeeee", "#e6e6e6"], 0.22, 0.5)
    };
    let lit = (hi * 3.0f32).min(0.95);
    let fx = r#"filterUnits="userSpaceOnUse" x="-20" y="-20" width="140" height="140""#;
    let bx = r#"maskUnits="userSpaceOnUse" x="-20" y="-20" width="140" height="140""#;
    let lit_filter = |id: &str, k: f32, shade: f32, source: bool| {
        format!(
            r##"<filter id="{id}" {fx}><feOffset in="SourceAlpha" dy="{:.2}" result="down"/><feGaussianBlur in="down" stdDeviation="{:.2}" result="soft"/><feFlood flood-color="#000" flood-opacity="{:.2}"/><feComposite in2="soft" operator="in" result="shade"/><feOffset in="SourceAlpha" dy="{:.2}" result="up"/><feFlood flood-color="#fff" flood-opacity="{:.2}"/><feComposite in2="up" operator="in" result="lit"/><feMerge><feMergeNode in="shade"/><feMergeNode in="lit"/>{}</feMerge></filter>"##,
            3.0 * k,
            1.3 * k,
            shade * k,
            -1.5 * k,
            lit * k,
            if source { r#"<feMergeNode in="SourceGraphic"/>"# } else { "" }
        )
    };
    // The rose, turned with the slant: the long ticks are where it points.
    let ticks = |c: &str| {
        let mut h = format!(r#"<circle cx="50" cy="50" r="33" fill="none" stroke="{c}" stroke-width="2.2"/>"#);
        for k in 0..8 {
            let a = (SLANT + 45.0 * k as f32).to_radians();
            let long = k % 2 == 0;
            let (r0, r1) = (if long { 20.0 } else { 26.0 }, 30.0);
            h.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{c}" stroke-width="{}" stroke-linecap="round"/>"#,
                50.0 + r0 * a.sin(),
                50.0 - r0 * a.cos(),
                50.0 + r1 * a.sin(),
                50.0 - r1 * a.cos(),
                if long { 2.8 } else { 1.8 }
            ));
        }
        h
    };
    let disc = r#"<circle cx="50" cy="50" r="40"/>"#;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256"><defs>
<linearGradient id="c" x1=".44" y1="0" x2=".56" y2="1"><stop offset="0" stop-color="{s0}"/><stop offset=".38" stop-color="{s1}"/><stop offset=".72" stop-color="{s2}"/><stop offset="1" stop-color="{s3}"/></linearGradient>
<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="{gloss}"/><stop offset=".55" stop-color="#fff" stop-opacity="0"/></linearGradient>
{edge}{depth}{needle_f}
<mask id="cut" {bx}><rect x="-20" y="-20" width="140" height="140" fill="#fff"/>{cut}</mask>
<mask id="pl" {bx}><circle cx="50" cy="50" r="40" fill="#fff"/>{cut}</mask>
<mask id="op" {bx}>{open}</mask></defs>
<g opacity=".8"><g transform="translate(5 5) scale(.9)">
<g mask="url(#cut)"><g filter="url(#ce)"><g fill="url(#c)">{disc}</g></g><g fill="url(#g)">{disc}</g></g>
<g mask="url(#op)"><g filter="url(#cd)"><rect mask="url(#pl)" fill="#000" x="-20" y="-20" width="140" height="140"/></g></g>
<g filter="url(#cn)"><g transform="rotate({angle:.2} 50 50)">
<path d="M50 17 L56.5 50 L43.5 50 Z" fill="url(#c)"/><path d="M50 17 L56.5 50 L50 50 Z" fill="#000" opacity=".22"/>
<path d="M50 83 L56.5 50 L43.5 50 Z" fill="url(#c)" opacity=".72"/><circle cx="50" cy="50" r="4.2" fill="url(#c)"/></g></g>
</g></g></svg>"##,
        s0 = stops[0],
        s1 = stops[1],
        s2 = stops[2],
        s3 = stops[3],
        edge = lit_filter("ce", 1.0, 0.7, true),
        depth = lit_filter("cd", 1.0, 0.95, false),
        needle_f = lit_filter("cn", 0.8, 0.7, true),
        cut = ticks("#000"),
        open = ticks("#fff"),
    )
}

/// The figure at rest.
pub(crate) fn rest_svg() -> String {
    svg(SLANT)
}

/// THE FRAMES: one a degree from `SLANT - REACH` to `SLANT + REACH`.
pub(crate) const FRAME: u32 = 128;
const REACH: i32 = SWING as i32 + 2;

struct Frames {
    look: u32,
    frames: Vec<Option<Arc<Vec<u8>>>>,
}

fn cache() -> &'static Mutex<Frames> {
    static CACHE: OnceLock<Mutex<Frames>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Frames { look: u32::MAX, frames: Vec::new() }))
}

/// Draw the frames for the present look, unless they are (being) drawn.
pub(crate) fn warm() {
    let look = crate::look::generation();
    let n = (2 * REACH + 1) as usize;
    {
        let Ok(mut c) = cache().lock() else { return };
        if c.look == look {
            return;
        }
        c.look = look;
        c.frames = vec![None; n];
    }
    let spawned = std::thread::Builder::new().name("compass-frames".into()).spawn(move || {
        // SAFETY: setpriority on the calling thread only.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 15) };
        // (From the rest outward: a swing wants the near ones first.)
        let mut order: Vec<i32> = (0..=REACH).flat_map(|d| [d, -d]).collect();
        order.dedup();
        for d in order {
            let Some(px) = draw(SLANT + d as f32) else { continue };
            let Ok(mut c) = cache().lock() else { return };
            if c.look != look {
                return;
            }
            c.frames[(d + REACH) as usize] = Some(Arc::new(px));
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("compass: no frame thread: {e}");
    }
}

fn draw(angle: f32) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_data(svg(angle).as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(FRAME, FRAME)?;
    let k = FRAME as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    Some(pixmap.take())
}

/// The drawn frame nearest the needle at `angle`, if it is drawn yet.
pub(crate) fn frame(angle: f32) -> Option<Arc<Vec<u8>>> {
    let c = cache().lock().ok()?;
    let d = ((angle - SLANT).round() as i32).clamp(-REACH, REACH);
    c.frames.get((d + REACH) as usize).cloned().flatten()
}

/// The needle's swing: nudged as the pointer comes on the icon.
#[derive(Default)]
pub(crate) struct Wobble {
    hovered: bool,
    /// ms since the nudge (None = still).
    t: Option<f32>,
}

impl Wobble {
    /// Step by `dt` seconds with the pointer on Seam's icon or not; whether
    /// the needle moved.
    pub(crate) fn step(&mut self, hover: bool, dt: f32) -> bool {
        if hover && !self.hovered {
            self.t = Some(0.0);
        }
        self.hovered = hover;
        let Some(t) = &mut self.t else { return false };
        *t += dt * 1000.0;
        if *t >= SETTLE_MS {
            self.t = None;
        }
        true
    }

    /// The needle's angle now.
    pub(crate) fn angle(&self) -> f32 {
        let Some(t) = self.t else { return SLANT };
        let swing = SWING * (-t / (SETTLE_MS / 3.0)).exp() * (2.0 * std::f32::consts::PI * t / PERIOD_MS).sin();
        SLANT + swing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_figure_parses() {
        assert!(resvg::usvg::Tree::from_data(rest_svg().as_bytes(), &resvg::usvg::Options::default()).is_ok());
    }

    #[test]
    fn a_hover_swings_the_needle_and_it_settles_on_its_slant() {
        let mut w = Wobble::default();
        let mut most: f32 = 0.0;
        for _ in 0..200 {
            w.step(true, 0.016);
            most = most.max((w.angle() - SLANT).abs());
        }
        assert!(most > SWING * 0.6, "it swung ({most})");
        assert_eq!(w.angle(), SLANT, "and is still on its slant");
        assert!(!w.step(true, 0.016), "once a hover");
    }
}

//! The APPS button on the dock (Max, 2026-10-15, the Dock Motion Lab —
//! https://claude.ai/artifact/Pn8Ao3JUeU6JKDiwxUhobo): a disc with nine
//! squares cut in it, lit as the gears and the bin are (a line of light over
//! each top edge, the shadow thrown past each bottom one; the squares are
//! openings with a depth of their own).
//!
//! - HOVER: the pages turn, once a hover — two pages to the left, one back
//!   to the right — and it ends there (every page is alike: it ends looking
//!   as it began);
//! - the GRID OPEN (a click, Super+Space): the squares slide down out of the
//!   disc and it stays bare while the grid is up; as it closes they come back
//!   up from below. The disc BEATS once at each.
//!
//! Drawn whole for each place of the squares (`svg`), the light fixed; as the
//! gears are, the frames are drawn once per look on a low-priority thread
//! (one a unit of the figure's 100 across a page, and down a drop) and shown
//! by where the squares stand.

use std::sync::{Arc, Mutex, OnceLock};

/// Max's numbers from the lab: the squares' size and spread (units of the
/// figure's 100), their corners (a share of half the size), the speeds (ms),
/// the beat (its size and length).
const SIZE: f32 = 10.5;
const GAP: f32 = 6.5;
const ROUND: f32 = 0.55;
const LEFT_MS: f32 = 360.0;
const RIGHT_MS: f32 = 560.0;
const REST_MS: f32 = 160.0;
const DOWN_MS: f32 = 380.0;
const UP_MS: f32 = 420.0;
const BEAT: f32 = 0.07;
const BEAT_MS: f32 = 520.0;

/// The grid's top-left corner on the 100 square (centred).
fn origin() -> f32 {
    50.0 - (2.0 * (SIZE + GAP) + SIZE) / 2.0
}

/// How far a page is (the next page wholly off the disc), and how deep the
/// squares go for the grid (as far).
pub(crate) fn page() -> u32 {
    (91.0 - origin()).ceil() as u32
}

/// The figure with the squares moved by (dx, dy) units, in the look's tone.
pub(crate) fn svg(dx: f32, dy: f32) -> String {
    let (stops, gloss, hi) = if crate::look::dark() {
        (["#9a9fa8", "#8d929b", "#757a83", "#666a73"], 0.12, 0.25)
    } else {
        (["#fefefe", "#fafafa", "#eeeeee", "#e6e6e6"], 0.22, 0.5)
    };
    let lit = (hi * 3.0f32).min(0.95);
    let (o, pitch, page) = (origin(), SIZE + GAP, page() as f32);
    let rx = SIZE * ROUND / 2.0;
    // Three pages of nine: this one and the next two off the disc.
    let holes = |fill: &str| {
        let mut h = String::new();
        for p in 0..3 {
            for j in 0..3 {
                for i in 0..3 {
                    h.push_str(&format!(
                        r#"<rect x="{:.2}" y="{:.2}" width="{SIZE}" height="{SIZE}" rx="{rx:.2}" fill="{fill}"/>"#,
                        o + pitch * i as f32 + page * p as f32 + dx,
                        o + pitch * j as f32 + dy
                    ));
                }
            }
        }
        format!(r#"<g clip-path="url(#dc)">{h}</g>"#)
    };
    let fx = r#"filterUnits="userSpaceOnUse" x="-20" y="-20" width="140" height="140""#;
    let bx = r#"maskUnits="userSpaceOnUse" x="-20" y="-20" width="140" height="140""#;
    let lit_filter = |id: &str, shade: f32, source: bool| {
        format!(
            r##"<filter id="{id}" {fx}><feOffset in="SourceAlpha" dy="3" result="down"/><feGaussianBlur in="down" stdDeviation="1.3" result="soft"/><feFlood flood-color="#000" flood-opacity="{shade}"/><feComposite in2="soft" operator="in" result="shade"/><feOffset in="SourceAlpha" dy="-1.5" result="up"/><feFlood flood-color="#fff" flood-opacity="{lit:.2}"/><feComposite in2="up" operator="in" result="lit"/><feMerge><feMergeNode in="shade"/><feMergeNode in="lit"/>{}</feMerge></filter>"##,
            if source { r#"<feMergeNode in="SourceGraphic"/>"# } else { "" }
        )
    };
    let disc = r#"<circle cx="50" cy="50" r="40"/>"#;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256"><defs>
<linearGradient id="c" x1=".44" y1="0" x2=".56" y2="1"><stop offset="0" stop-color="{s0}"/><stop offset=".38" stop-color="{s1}"/><stop offset=".72" stop-color="{s2}"/><stop offset="1" stop-color="{s3}"/></linearGradient>
<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="{gloss}"/><stop offset=".55" stop-color="#fff" stop-opacity="0"/></linearGradient>
<clipPath id="dc"><circle cx="50" cy="50" r="40"/></clipPath>{edge}{depth}
<mask id="cut" {bx}><rect x="-20" y="-20" width="140" height="140" fill="#fff"/>{cut}</mask>
<mask id="pl" {bx}><circle cx="50" cy="50" r="40" fill="#fff"/>{cut}</mask>
<mask id="op" {bx}>{open}</mask></defs>
<g opacity=".8"><g transform="translate(5 5) scale(.9)">
<g mask="url(#cut)"><g filter="url(#ae)"><g fill="url(#c)">{disc}</g></g><g fill="url(#g)">{disc}</g></g>
<g mask="url(#op)"><g filter="url(#ad)"><rect mask="url(#pl)" fill="#000" x="-20" y="-20" width="140" height="140"/></g></g>
</g></g></svg>"##,
        s0 = stops[0],
        s1 = stops[1],
        s2 = stops[2],
        s3 = stops[3],
        edge = lit_filter("ae", 0.7, true),
        depth = lit_filter("ad", 0.95, false),
        cut = holes("#000"),
        open = holes("#fff"),
    )
}

/// THE FRAMES: `page()` across (the squares moved left 0..page units — every
/// page alike, so that is every place a turn passes) and `page()` down.
pub(crate) const FRAME: u32 = 128;

struct Frames {
    look: u32,
    across: Vec<Option<Arc<Vec<u8>>>>,
    down: Vec<Option<Arc<Vec<u8>>>>,
}

fn cache() -> &'static Mutex<Frames> {
    static CACHE: OnceLock<Mutex<Frames>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Frames { look: u32::MAX, across: Vec::new(), down: Vec::new() }))
}

/// Draw the frames for the present look, unless they are (being) drawn.
pub(crate) fn warm() {
    let look = crate::look::generation();
    let n = page() as usize;
    {
        let Ok(mut c) = cache().lock() else { return };
        if c.look == look {
            return;
        }
        c.look = look;
        c.across = vec![None; n];
        c.down = vec![None; n + 1];
    }
    let spawned = std::thread::Builder::new().name("apps-frames".into()).spawn(move || {
        // SAFETY: setpriority on the calling thread only.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 15) };
        // (Across first, from where it rests: a hover wants those soonest.)
        for i in 0..n {
            let Some(px) = draw(-(i as f32), 0.0) else { continue };
            let Ok(mut c) = cache().lock() else { return };
            if c.look != look {
                return;
            }
            c.across[i] = Some(Arc::new(px));
        }
        for i in 0..=n {
            let Some(px) = draw(0.0, i as f32) else { continue };
            let Ok(mut c) = cache().lock() else { return };
            if c.look != look {
                return;
            }
            c.down[i] = Some(Arc::new(px));
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("apps: no frame thread: {e}");
    }
}

fn draw(dx: f32, dy: f32) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_data(svg(dx, dy).as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(FRAME, FRAME)?;
    let k = FRAME as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    Some(pixmap.take())
}

/// The drawn frame for the squares `pages` pages to the left (any number:
/// pages are alike) and `down` of a drop down, if it is drawn yet.
pub(crate) fn frame(pages: f32, down: f32) -> Option<Arc<Vec<u8>>> {
    let c = cache().lock().ok()?;
    let n = c.across.len();
    if n == 0 {
        return None;
    }
    if down > 0.0 {
        let i = (down.clamp(0.0, 1.0) * n as f32).round() as usize;
        return c.down.get(i).cloned().flatten();
    }
    let i = (pages.rem_euclid(1.0) * n as f32).round() as usize % n;
    c.across[i].clone()
}

/// The soft curve of the lab, `cubic-bezier(.45, 0, .25, 1)`.
fn ease(p: f32) -> f32 {
    let p = p.clamp(0.0, 1.0);
    let bez = |a: f32, b: f32, t: f32| 3.0 * a * t * (1.0 - t).powi(2) + 3.0 * b * t * t * (1.0 - t) + t.powi(3);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if bez(0.45, 0.25, mid) < p {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(0.0, 1.0, (lo + hi) / 2.0)
}

/// What the button does, step by step.
#[derive(Default)]
pub(crate) struct AppsMotion {
    hovered: bool,
    open: bool,
    /// The hover's round, ms in (None = still).
    round: Option<f32>,
    /// The squares' drop: (from, to, ms in, of ms).
    slide: Option<(f32, f32, f32, f32)>,
    /// How far down they stand (0 up .. 1 gone).
    down: f32,
    /// The beat, ms in.
    beat: Option<f32>,
    pages: f32,
}

impl AppsMotion {
    /// Step by `dt` seconds with the pointer on the button or not and the
    /// grid open or not; whether anything moved.
    pub(crate) fn step(&mut self, hover: bool, open: bool, dt: f32) -> bool {
        let ms = dt * 1000.0;
        if hover && !self.hovered && !open && self.round.is_none() && self.slide.is_none() {
            self.round = Some(0.0);
        }
        self.hovered = hover;
        if open != self.open {
            self.open = open;
            self.round = None;
            let to = if open { 1.0 } else { 0.0 };
            self.slide = Some((self.down, to, 0.0, if open { DOWN_MS } else { UP_MS }));
            self.beat = Some(0.0);
        }
        let mut moved = false;
        if let Some(t) = &mut self.round {
            *t += ms;
            // Two pages left, one back, a rest after each.
            let (l, r, p) = (LEFT_MS, RIGHT_MS, REST_MS);
            let steps = [(l, 0.0, 1.0), (p, 1.0, 1.0), (l, 1.0, 2.0), (p, 2.0, 2.0), (r, 2.0, 1.0), (p, 1.0, 1.0)];
            let mut at = *t;
            let mut done = true;
            for (len, from, to) in steps {
                if at < len {
                    self.pages = from + (to - from) * ease(at / len);
                    done = false;
                    break;
                }
                at -= len;
            }
            if done {
                self.pages = 0.0; // (one page on is where it began)
                self.round = None;
            }
            moved = true;
        }
        if let Some((from, to, t, len)) = &mut self.slide {
            *t += ms;
            let k = (*t / *len).min(1.0);
            // (ease-in-out, the lab's)
            let k2 = if k < 0.5 { 2.0 * k * k } else { 1.0 - (-2.0 * k + 2.0).powi(2) / 2.0 };
            self.down = *from + (*to - *from) * k2;
            if k >= 1.0 {
                self.slide = None;
            }
            moved = true;
        }
        if let Some(t) = &mut self.beat {
            *t += ms;
            if *t >= BEAT_MS {
                self.beat = None;
            }
            moved = true;
        }
        moved
    }

    /// Where the squares stand: pages across, and down (0..1).
    pub(crate) fn place(&self) -> (f32, f32) {
        (self.pages, self.down)
    }

    /// The disc's size now (1 at rest): the beat.
    pub(crate) fn scale(&self) -> f32 {
        let Some(t) = self.beat else { return 1.0 };
        let x = t / BEAT_MS;
        let keys = [(0.0, 1.0), (0.35, 1.0 + BEAT), (0.7, 1.0 - BEAT * 0.3), (1.0, 1.0)];
        for w in keys.windows(2) {
            let ((t0, v0), (t1, v1)) = (w[0], w[1]);
            if x <= t1 {
                let k = ((x - t0) / (t1 - t0)).clamp(0.0, 1.0);
                return v0 + (v1 - v0) * (k * k * (3.0 - 2.0 * k));
            }
        }
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_figure_parses_and_a_page_clears_the_disc() {
        assert!(resvg::usvg::Tree::from_data(svg(0.0, 0.0).as_bytes(), &resvg::usvg::Options::default()).is_ok());
        assert!(origin() + page() as f32 >= 90.0, "the next page waits off the disc");
    }

    #[test]
    fn a_hover_turns_two_pages_and_one_back_then_rests() {
        let mut m = AppsMotion::default();
        let mut most: f32 = 0.0;
        m.step(true, false, 0.016);
        for _ in 0..400 {
            m.step(true, false, 0.016);
            most = most.max(m.place().0);
        }
        assert!(most > 1.9, "two pages went by");
        assert_eq!(m.place(), (0.0, 0.0), "and it rests as it began");
        assert!(!m.step(true, false, 0.016), "still: one round a hover");
    }

    #[test]
    fn the_grid_open_sends_the_squares_down_and_back_with_a_beat() {
        let mut m = AppsMotion::default();
        m.step(false, true, 0.016);
        assert!(m.scale() != 1.0 || m.beat.is_some());
        for _ in 0..60 {
            m.step(false, true, 0.016);
        }
        assert_eq!(m.place().1, 1.0, "gone down while it is open");
        for _ in 0..60 {
            m.step(false, false, 0.016);
        }
        assert_eq!(m.place().1, 0.0, "back up as it closes");
        assert_eq!(m.scale(), 1.0);
    }
}

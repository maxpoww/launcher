//! SEAM's icon on the dock: a COMPASS (Max, 2026-10-16, the Dock Motion Lab —
//! https://claude.ai/artifact/Pn8Ao3JUeU6JKDiwxUhobo). A plate in the dock's
//! cast with a fine graduation and a ring grooved in it, a WIND ROSE on it
//! made of light alone ("Solo luz": eight points, each a lit face and a
//! shaded one, the long four out to the plate's edge), and a needle on
//! Safari's slant, its north half red, its south half blue.
//!
//! A HOVER nudges it: the needle swings side to side and settles back on its
//! slant, and the plate turns the other way, by `PLATE` of the needle's
//! swing, `LAG` behind it. A CLICK turns the needle once round (the plate
//! turning against it and back), then it settles the same way.
//!
//! The needle and the plate move apart, so they are drawn apart (the light
//! fixed, each frame drawn whole by resvg — once per look, on a low-priority
//! thread): the plate a frame a degree over its reach, the needle a frame a
//! degree near its slant and every `COARSE` degrees round the rest (only the
//! spin goes there, fast). A frame on the dock is the needle laid over the
//! plate, then the whole at the figure's opacity — exactly the one figure.

use std::sync::{Arc, Mutex, OnceLock};

/// Seam's entry on the dock (its `.desktop` id).
pub(crate) const SEAM_ID: &str = "seam";

/// Max's numbers from the lab: the needle's slant at rest (degrees from up,
/// clockwise), the swing (degrees each way), one swing there and back (ms),
/// how long until it is still (ms), the click's turn (ms), and the plate:
/// its share of the needle's move, the other way, and how far behind (ms).
const SLANT: f32 = 45.0;
const SWING: f32 = 30.0;
const PERIOD_MS: f32 = 560.0;
const SETTLE_MS: f32 = 2000.0;
const SPIN_MS: f32 = 900.0;
const PLATE: f32 = 0.35;
const LAG_MS: f32 = 80.0;
/// After the click's turn the needle settles from a swing of at most this.
const SPIN_SETTLE_SWING: f32 = 22.0;
/// While the needle goes round, the plate's turn against it (at most).
const SPIN_PLATE: f32 = 90.0;

/// What of the figure to draw.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Whole,
    Plate,
    Needle,
}

fn at(a: f32, r: f32) -> String {
    format!("{:.2} {:.2}", 50.0 + r * a.sin(), 50.0 - r * a.cos())
}

/// The figure: the needle at `needle` degrees, the plate turned `plate`.
fn figure(needle: f32, plate: f32, part: Part) -> String {
    let dark = crate::look::dark();
    let (stops, gloss, hi) = if dark {
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
            if source {
                r#"<feMergeNode in="SourceGraphic"/>"#
            } else {
                ""
            }
        )
    };
    let slant = SLANT.to_radians();
    // The grooves: the ring and the graduation, a tick every 15°, a longer
    // one every 45° (turned with the slant).
    let ticks = |c: &str| {
        let mut h = format!(
            r#"<circle cx="50" cy="50" r="33" fill="none" stroke="{c}" stroke-width="2.2"/>"#
        );
        for k in 0..24 {
            let a = slant + k as f32 * std::f32::consts::PI / 12.0;
            let major = k % 3 == 0;
            let (r0, r1) = (if major { 34.4 } else { 35.4 }, 38.0);
            h.push_str(&format!(
                r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{c}" stroke-width="{}" stroke-linecap="round"/>"#,
                50.0 + r0 * a.sin(),
                50.0 - r0 * a.cos(),
                50.0 + r1 * a.sin(),
                50.0 - r1 * a.cos(),
                if major { 1.2 } else { 0.7 }
            ));
        }
        h
    };
    // The wind rose: the short points first, under; each point a lit face
    // and a shaded one, slim as the needle (≈11° each side).
    let mut rose = String::new();
    for long in [false, true] {
        for k in (if long { 0 } else { 1 }..8).step_by(2) {
            let a = slant + k as f32 * std::f32::consts::FRAC_PI_4;
            let (tip, r0) = if long { (40.0, 9.0) } else { (34.0, 7.5) };
            let w = std::f32::consts::FRAC_PI_4;
            let (t, l, r) = (at(a, tip), at(a - w, r0), at(a + w, r0));
            rose.push_str(&format!(
                r##"<path d="M50 50 L{t} L{l} Z" fill="#fff" opacity=".5"/><path d="M50 50 L{t} L{r} Z" fill="#000" opacity=".38"/>"##
            ));
        }
    }
    let turn = format!(r#"transform="rotate({plate:.2} 50 50)""#);
    let disc = r#"<circle cx="50" cy="50" r="40"/>"#;
    let (n0, n1, tint) = ("#e3685a", "#a8372d", if dark { 0.78 } else { 0.62 });
    let plate_g = format!(
        r##"<g mask="url(#cut)"><g filter="url(#ce)"><g fill="url(#c)">{disc}</g></g><g fill="url(#g)">{disc}</g></g>
<g mask="url(#op)"><g filter="url(#cd)"><rect mask="url(#pl)" fill="#000" x="-20" y="-20" width="140" height="140"/></g></g>
<g clip-path="url(#cl)"><g {turn}>{rose}</g></g>"##
    );
    let needle_g = format!(
        r##"<g filter="url(#cn)"><g transform="rotate({needle:.2} 50 50)">
<path d="M50 17 L56.5 50 L43.5 50 Z" fill="url(#c)"/><path d="M50 17 L56.5 50 L43.5 50 Z" fill="url(#cr)" opacity="{tint}"/><path d="M50 17 L56.5 50 L50 50 Z" fill="#000" opacity=".22"/>
<path d="M50 83 L56.5 50 L43.5 50 Z" fill="url(#c)"/><path d="M50 83 L56.5 50 L43.5 50 Z" fill="url(#cb)" opacity="{tint}"/><path d="M50 83 L56.5 50 L50 50 Z" fill="#000" opacity=".22"/>
<circle cx="50" cy="50" r="4.2" fill="url(#c)"/></g></g>"##
    );
    let body = match part {
        Part::Whole => format!("{plate_g}\n{needle_g}"),
        Part::Plate => plate_g,
        Part::Needle => needle_g,
    };
    // (Apart, each is drawn opaque: the opacity is the whole figure's, put on
    // once they are laid together — `lay`.)
    let opacity = if part == Part::Whole { ".8" } else { "1" };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256"><defs>
<linearGradient id="c" x1=".44" y1="0" x2=".56" y2="1"><stop offset="0" stop-color="{s0}"/><stop offset=".38" stop-color="{s1}"/><stop offset=".72" stop-color="{s2}"/><stop offset="1" stop-color="{s3}"/></linearGradient>
<linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="{gloss}"/><stop offset=".55" stop-color="#fff" stop-opacity="0"/></linearGradient>
<linearGradient id="cr" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{n0}"/><stop offset="1" stop-color="{n1}"/></linearGradient>
<linearGradient id="cb" x1="0" y1="1" x2="0" y2="0"><stop offset="0" stop-color="#5a8de3"/><stop offset="1" stop-color="#2d58a8"/></linearGradient>
{edge}{depth}{needle_f}
<clipPath id="cl"><circle cx="50" cy="50" r="40"/></clipPath>
<mask id="cut" {bx}><rect x="-20" y="-20" width="140" height="140" fill="#fff"/><g {turn}>{cut}</g></mask>
<mask id="pl" {bx}><circle cx="50" cy="50" r="40" fill="#fff"/><g {turn}>{cut}</g></mask>
<mask id="op" {bx}><g {turn}>{open}</g></mask></defs>
<g opacity="{opacity}"><g transform="translate(5 5) scale(.9)">
{body}
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

/// The figure at rest (the needle on its slant, the plate square).
pub(crate) fn rest_svg() -> String {
    figure(SLANT, 0.0, Part::Whole)
}

/// THE FRAMES, `FRAME` square.
pub(crate) const FRAME: u32 = 128;
/// The needle: a frame a degree within `FINE` of its slant (the swings),
/// one every `COARSE` degrees round the rest (only the spin is there).
const FINE: i32 = SWING as i32 + 2;
const COARSE: i32 = 3;
/// The plate's reach: against the needle's spin, with the needle's swings.
const PLATE_MIN: i32 = -((PLATE * SPIN_PLATE) as i32 + 2);
const PLATE_MAX: i32 = (PLATE * SWING) as i32 + 2;

type Px = Arc<Vec<u8>>;

struct Frames {
    look: u32,
    /// By the needle's degree from up (0..360; only the drawn ones).
    needle: Vec<Option<Px>>,
    /// By the plate's degree, from `PLATE_MIN`.
    plate: Vec<Option<Px>>,
}

fn cache() -> &'static Mutex<Frames> {
    static CACHE: OnceLock<Mutex<Frames>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Frames {
            look: u32::MAX,
            needle: Vec::new(),
            plate: Vec::new(),
        })
    })
}

/// The needle's drawn degree nearest `angle` (0..360).
fn needle_key(angle: f32) -> usize {
    let a = (angle.round() as i32).rem_euclid(360);
    let off = (a - SLANT as i32 + 180).rem_euclid(360) - 180;
    if off.abs() <= FINE {
        return a as usize;
    }
    ((((a as f32) / COARSE as f32).round() as i32 * COARSE).rem_euclid(360)) as usize
}

fn plate_key(angle: f32) -> usize {
    ((angle.round() as i32).clamp(PLATE_MIN, PLATE_MAX) - PLATE_MIN) as usize
}

/// Draw the frames for the present look, unless they are (being) drawn.
pub(crate) fn warm() {
    let look = crate::look::generation();
    {
        let Ok(mut c) = cache().lock() else { return };
        if c.look == look {
            return;
        }
        c.look = look;
        c.needle = vec![None; 360];
        c.plate = vec![None; (PLATE_MAX - PLATE_MIN + 1) as usize];
    }
    let spawned = std::thread::Builder::new()
        .name("compass-frames".into())
        .spawn(move || {
            // SAFETY: setpriority on the calling thread only.
            unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 15) };
            // The hover's first: the plate and the needle near their rest, from
            // the rest outward; then the rest of the needle's way round.
            let near = |reach: i32| (0..=reach).flat_map(|d| [d, -d]).collect::<Vec<_>>();
            let mut work: Vec<(bool, f32)> = Vec::new();
            for d in near(FINE.max(-PLATE_MIN)) {
                if d.abs() <= FINE {
                    work.push((true, SLANT + d as f32));
                }
                if (PLATE_MIN..=PLATE_MAX).contains(&d) {
                    work.push((false, d as f32));
                }
            }
            for a in (0..360).step_by(COARSE as usize) {
                work.push((true, a as f32));
            }
            for (is_needle, angle) in work {
                let key = if is_needle {
                    needle_key(angle)
                } else {
                    plate_key(angle)
                };
                {
                    let Ok(c) = cache().lock() else { return };
                    if c.look != look {
                        return;
                    }
                    let slot = if is_needle { &c.needle } else { &c.plate };
                    if slot.get(key).is_some_and(Option::is_some) {
                        continue;
                    }
                }
                let svg = if is_needle {
                    figure(angle, 0.0, Part::Needle)
                } else {
                    figure(SLANT, angle, Part::Plate)
                };
                let Some(px) = draw(&svg) else { continue };
                let Ok(mut c) = cache().lock() else { return };
                if c.look != look {
                    return;
                }
                let slot = if is_needle {
                    &mut c.needle
                } else {
                    &mut c.plate
                };
                if let Some(s) = slot.get_mut(key) {
                    *s = Some(Arc::new(px));
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("compass: no frame thread: {e}");
    }
}

fn draw(svg: &str) -> Option<Vec<u8>> {
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

/// The needle laid over the plate (premultiplied), then the figure's opacity.
fn lay(needle: &[u8], plate: &[u8]) -> Vec<u8> {
    const OPACITY: u32 = 204; // 0.8
    needle
        .chunks_exact(4)
        .zip(plate.chunks_exact(4))
        .flat_map(|(n, p)| {
            let keep = 255 - n[3] as u32;
            let px = |i: usize| {
                let v = n[i] as u32 + (p[i] as u32 * keep + 127) / 255;
                ((v.min(255) * OPACITY + 127) / 255) as u8
            };
            [px(0), px(1), px(2), px(3)]
        })
        .collect()
}

/// The frame with the needle at `needle` and the plate at `plate`, if both
/// are drawn yet.
pub(crate) fn frame(needle: f32, plate: f32) -> Option<Vec<u8>> {
    let (n, p) = {
        let c = cache().lock().ok()?;
        (
            c.needle.get(needle_key(needle)).cloned().flatten()?,
            c.plate.get(plate_key(plate)).cloned().flatten()?,
        )
    };
    Some(lay(&n, &p))
}

/// What the compass is doing.
#[derive(Clone, Copy)]
enum Move {
    /// Nudged by the pointer coming on it, `t` ms ago.
    Swing(f32),
    /// Turning once round from a click, `t` ms ago.
    Spin(f32),
}

/// The compass's motion: a hover swings it, a click turns it round.
#[derive(Default)]
pub(crate) struct Motion {
    hovered: bool,
    now: Option<Move>,
}

fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) / 2.0
    }
}

/// A swing of `amp` degrees, `t` ms after the nudge (0 before it).
fn swing(amp: f32, t: f32) -> f32 {
    if t < 0.0 {
        return 0.0;
    }
    amp * (-t / (SETTLE_MS / 3.0)).exp() * (2.0 * std::f32::consts::PI * t / PERIOD_MS).sin()
}

impl Motion {
    /// A click on Seam's icon: once round.
    pub(crate) fn click(&mut self) {
        self.now = Some(Move::Spin(0.0));
    }

    /// Step by `dt` seconds with the pointer on Seam's icon or not; whether
    /// anything moved.
    pub(crate) fn step(&mut self, hover: bool, dt: f32) -> bool {
        // (The pointer coming on it nudges it, unless it is turning round.)
        if hover && !self.hovered && !matches!(self.now, Some(Move::Spin(_))) {
            self.now = Some(Move::Swing(0.0));
        }
        self.hovered = hover;
        let ms = dt * 1000.0;
        self.now = match self.now {
            None => return false,
            Some(Move::Swing(t)) => {
                Some(Move::Swing(t + ms)).filter(|_| t + ms < SETTLE_MS + LAG_MS)
            }
            Some(Move::Spin(t)) => {
                Some(Move::Spin(t + ms)).filter(|_| t + ms - SPIN_MS < SETTLE_MS + LAG_MS)
            }
        };
        true
    }

    /// The needle's angle and the plate's now.
    pub(crate) fn pose(&self) -> (f32, f32) {
        match self.now {
            None => (SLANT, 0.0),
            Some(Move::Swing(t)) => (SLANT + swing(SWING, t), -PLATE * swing(SWING, t - LAG_MS)),
            Some(Move::Spin(t)) => {
                let amp = SWING.min(SPIN_SETTLE_SWING);
                let needle = if t < SPIN_MS {
                    SLANT + 360.0 * ease(t / SPIN_MS)
                } else {
                    SLANT + swing(amp, t - SPIN_MS)
                };
                let k = t - LAG_MS;
                let plate = if k < 0.0 {
                    0.0
                } else if k < SPIN_MS {
                    -PLATE * SPIN_PLATE * (std::f32::consts::PI * ease(k / SPIN_MS)).sin()
                } else {
                    -PLATE * swing(amp, k - SPIN_MS)
                };
                (needle, plate)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_part_parses() {
        for part in [Part::Whole, Part::Plate, Part::Needle] {
            let svg = figure(SLANT + 10.0, -5.0, part);
            assert!(
                resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default())
                    .is_ok()
            );
        }
    }

    #[test]
    fn the_parts_laid_together_are_the_figure() {
        let (n, p) = (
            draw(&figure(SLANT + 12.0, -4.0, Part::Needle)).unwrap(),
            draw(&figure(SLANT, -4.0, Part::Plate)).unwrap(),
        );
        let whole = draw(&figure(SLANT + 12.0, -4.0, Part::Whole)).unwrap();
        let laid = lay(&n, &p);
        let worst = laid
            .iter()
            .zip(&whole)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap();
        assert!(worst <= 6, "off by {worst}");
    }

    #[test]
    fn a_hover_swings_the_needle_the_plate_the_other_way_and_both_settle() {
        let mut m = Motion::default();
        let (mut most, mut with) = (0.0f32, 0.0f32);
        for _ in 0..200 {
            m.step(true, 0.016);
            let (n, p) = m.pose();
            most = most.max((n - SLANT).abs());
            with += (n - SLANT) * p;
        }
        assert!(most > SWING * 0.6, "it swung ({most})");
        assert!(with < 0.0, "the plate went the other way (a little behind)");
        assert_eq!(m.pose(), (SLANT, 0.0), "and both are still");
        assert!(!m.step(true, 0.016), "once a hover");
    }

    #[test]
    fn a_click_turns_it_once_round_and_it_settles() {
        let mut m = Motion::default();
        m.click();
        let mut furthest = 0.0f32;
        for _ in 0..300 {
            m.step(true, 0.016);
            furthest = furthest.max(m.pose().0 - SLANT);
        }
        assert!(furthest > 350.0, "round ({furthest})");
        assert_eq!(m.pose(), (SLANT, 0.0));
    }

    #[test]
    fn every_pose_has_its_frames() {
        assert_eq!(needle_key(SLANT + 361.0), SLANT as usize + 1);
        assert_eq!(needle_key(200.4) % COARSE as usize, 0);
        let mut m = Motion::default();
        m.click();
        for _ in 0..300 {
            m.step(false, 0.016);
            let (_, p) = m.pose();
            assert!(
                (PLATE_MIN as f32..=PLATE_MAX as f32).contains(&p.round()),
                "plate {p}"
            );
        }
    }
}

//! The CONTROL PANEL's figure on the dock: three classic gears, meshed (Max,
//! 2026-10-15, the Dock Motion Lab — https://claude.ai/artifact/Pn8Ao3JUeU6JKDiwxUhobo).
//! The big one on the right, the middle one up left, the small one down left;
//! STANDARD teeth (involute, 20° pressure angle, AGMA stub: addendum 0.8 and
//! dedendum 1 module), a rim, spokes and a hub on each. Under the pointer
//! they turn together, each by its teeth; let go, they coast and stop exactly
//! on a pose that IS the rest pose, so nothing jumps.
//!
//! The figure is drawn whole for each angle (`svg`), the light fixed: the
//! bevel is cast by the outlines, the openings are cut after it and have a
//! depth of their own, lit from above as the rest is.

use std::f32::consts::PI;
use std::sync::OnceLock;

/// The gears: teeth, and (after the first) the gear it meshes with and the
/// angle from it (screen degrees: 0 right, 90 down).
const LAYOUT: [(u32, usize, f32); 3] = [(18, 0, 0.0), (9, 0, 212.0), (8, 0, 146.0)];
/// The big gear's speed while the pointer is on it, degrees a second.
const SPEED: f32 = 60.0;
/// How long the speed takes to come in (the lab's "Tiempo"), seconds.
const EASE_IN: f32 = 0.42;

struct Gear {
    n: u32,
    x: f32,
    y: f32,
    /// Its turn at rest, degrees.
    phase: f32,
    /// Its turn for one degree of the big one's.
    ratio: f32,
    outline: String,
    holes: String,
    /// How much of the depth it gets.
    depth: f32,
}

fn inv(x: f32) -> f32 {
    x.tan() - x
}

/// Spokes: the most of 6, 5, 4, 3 that divides the teeth, so a gear looks
/// the same again after a short turn.
fn spokes_of(n: u32) -> u32 {
    [6, 5, 4, 3].into_iter().find(|&k| n.is_multiple_of(k)).unwrap_or(4)
}

/// One gear centred on 0,0, tooth 0 along +x: (outline, openings).
fn gear_path(n: u32, m: f32, play: f32) -> (String, String) {
    let pressure = 20f32.to_radians();
    let r = m * n as f32 / 2.0;
    let (ra, rf, rb) = (r + 0.8 * m, r - m, r * pressure.cos());
    let half = PI / (2.0 * n as f32) - play / r;
    let at = |rho: f32| half + inv(pressure) - inv((rb / rho).min(1.0).acos());
    let r0 = rf.max(rb);
    let mut flank = Vec::new();
    if rf < rb {
        flank.push((rf, at(rb)));
    }
    for i in 0..=8 {
        let rho = r0 + (ra - r0) * i as f32 / 8.0;
        flank.push((rho, at(rho)));
    }
    let step = 2.0 * PI / n as f32;
    let mut outline = String::from("M");
    let mut first = true;
    for k in 0..n {
        let c = k as f32 * step;
        let up = flank.iter().map(|&(rho, t)| (rho, c - t));
        let down = flank.iter().rev().map(|&(rho, t)| (rho, c + t));
        for (rho, t) in up.chain(down) {
            if !first {
                outline.push_str(" L");
            }
            first = false;
            outline.push_str(&format!("{:.2} {:.2}", rho * t.cos(), rho * t.sin()));
        }
    }
    outline.push_str(" Z");
    // The plate: a rim, a hub with the axle, windows between the spokes.
    let circle = |h: f32| format!("M {:.2} 0 a {h:.2} {h:.2} 0 1 0 {:.2} 0 a {h:.2} {h:.2} 0 1 0 {:.2} 0 Z", -h, 2.0 * h, -2.0 * h);
    let (rim, hub, w, s) = (rf * 0.76, rf * 0.4, rf * 0.17, spokes_of(n));
    let mut holes = circle(rf * 0.2);
    let (ai, ao) = ((w / 2.0 / hub).asin(), (w / 2.0 / rim).asin());
    let pt = |rho: f32, t: f32| format!("{:.2} {:.2}", rho * t.cos(), rho * t.sin());
    for k in 0..s {
        let t0 = (k as f32 + 0.5) * 2.0 * PI / s as f32;
        let t1 = (k as f32 + 1.5) * 2.0 * PI / s as f32;
        holes.push_str(&format!(
            " M {} L {} A {rim:.2} {rim:.2} 0 0 1 {} L {} A {hub:.2} {hub:.2} 0 0 0 {} Z",
            pt(hub, t0 + ai),
            pt(rim, t0 + ao),
            pt(rim, t1 - ao),
            pt(hub, t1 - ai),
            pt(hub, t0 + ai)
        ));
    }
    (outline, holes)
}

/// The three gears, placed, meshed and fitted to the figure's 100 box.
fn gears() -> &'static [Gear] {
    static SET: OnceLock<Vec<Gear>> = OnceLock::new();
    SET.get_or_init(|| {
        let m = 7.0f32;
        let gap = m * 0.12;
        let r = |n: u32| m * n as f32 / 2.0;
        // (x, y, phase, ratio) before fitting.
        let mut at: Vec<(f32, f32, f32, f32)> = vec![(0.0, 0.0, 0.0, 1.0)];
        for &(n, on, a) in &LAYOUT[1..] {
            let (pn, (px, py, pp, pr)) = (LAYOUT[on].0, at[on]);
            let d = r(pn) + r(n) + gap;
            let rad = a.to_radians();
            // Mesh: the parent's tooth nearest the line between them sits
            // `off` from it; this gear's gap sits mirrored across it.
            let pitch = 360.0 / pn as f32;
            let rel = a - pp;
            let off = rel - (rel / pitch).round() * pitch;
            let phase = a + 180.0 + off * pn as f32 / n as f32 + 180.0 / n as f32;
            at.push((px + d * rad.cos(), py + d * rad.sin(), phase, -pr * pn as f32 / n as f32));
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (i, &(x, y, ..)) in at.iter().enumerate() {
            let big = r(LAYOUT[i].0) + m;
            (x0, y0, x1, y1) = (x0.min(x - big), y0.min(y - big), x1.max(x + big), y1.max(y + big));
        }
        let k = 96.0 / (x1 - x0).max(y1 - y0);
        at.iter()
            .enumerate()
            .map(|(i, &(x, y, phase, ratio))| {
                let n = LAYOUT[i].0;
                let (outline, holes) = gear_path(n, m * k, m * k * 0.09);
                Gear {
                    n,
                    x: 50.0 + (x - (x0 + x1) / 2.0) * k,
                    y: 50.0 + (y - (y0 + y1) / 2.0) * k,
                    phase,
                    ratio,
                    outline,
                    holes,
                    depth: if i == 0 { 1.0 } else { 0.9 },
                }
            })
            .collect()
    })
}

/// After how many degrees of the big gear the whole figure looks exactly as
/// at rest: each gear repeats every 360/gcd(teeth, spokes) of its own turn.
pub(crate) fn rest_cycle() -> f32 {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let set = gears();
    let l = set.iter().map(|g| 360 * g.n / gcd(g.n, spokes_of(g.n))).fold(1, |a, b| a / gcd(a, b) * b);
    l as f32 / set[0].n as f32
}

/// The figure with the big gear turned by `angle` degrees, in the look's tone.
pub(crate) fn svg(angle: f32) -> String {
    let dark = crate::look::icons_dark();
    // The dock figures' cast (silver, or graphite: `look::figure`).
    let (stops, gloss, hi) = if dark {
        (["#9a9fa8", "#8d929b", "#757a83", "#666a73"], 0.12, 0.25)
    } else {
        (["#fefefe", "#fafafa", "#eeeeee", "#e6e6e6"], 0.22, 0.5)
    };
    let set = gears();
    let lit = (hi * 3.0f32).min(0.95);
    let place = |g: &Gear| format!(r#"transform="translate({:.2} {:.2}) rotate({:.2})""#, g.x, g.y, g.phase + angle * g.ratio);
    let bx = r#"maskUnits="userSpaceOnUse" x="-10" y="-10" width="120" height="120""#;
    let fx = r#"filterUnits="userSpaceOnUse" x="-10" y="-10" width="120" height="120""#;
    // Light from above: the shadow thrown down, a line of light up.
    let depth = |id: String, k: f32, shade: f32, with_source: bool| {
        format!(
            r##"<filter id="{id}" {fx}><feOffset in="SourceAlpha" dy="{:.2}" result="down"/><feGaussianBlur in="down" stdDeviation="{:.2}" result="soft"/><feFlood flood-color="#000" flood-opacity="{:.2}"/><feComposite in2="soft" operator="in" result="shade"/><feOffset in="SourceAlpha" dy="{:.2}" result="up"/><feFlood flood-color="#fff" flood-opacity="{:.2}"/><feComposite in2="up" operator="in" result="lit"/><feMerge><feMergeNode in="shade"/><feMergeNode in="lit"/>{}</feMerge></filter>"##,
            3.0 * k,
            1.3 * k,
            shade * k,
            -1.5 * k,
            lit * k,
            if with_source { r#"<feMergeNode in="SourceGraphic"/>"# } else { "" }
        )
    };
    let mut defs = format!(
        r##"<linearGradient id="c" x1=".44" y1="0" x2=".56" y2="1"><stop offset="0" stop-color="{}"/><stop offset=".38" stop-color="{}"/><stop offset=".72" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity="{gloss}"/><stop offset=".55" stop-color="#fff" stop-opacity="0"/></linearGradient>"##,
        stops[0], stops[1], stops[2], stops[3]
    );
    let (mut outlines, mut cut) = (String::new(), String::new());
    for (i, g) in set.iter().enumerate() {
        let p = place(g);
        outlines.push_str(&format!(r##"<path {p} fill="#fff" d="{}"/>"##, g.outline));
        cut.push_str(&format!(r##"<path {p} fill="#000" d="{}"/>"##, g.holes));
        defs.push_str(&format!(r##"<mask id="e{i}" {bx}><path {p} fill="#fff" d="{}"/></mask>"##, g.outline));
        defs.push_str(&depth(format!("f{i}"), g.depth, 0.7, true));
        defs.push_str(&format!(
            r##"<mask id="p{i}" {bx}><path {p} fill="#fff" d="{}"/><path {p} fill="#000" d="{}"/></mask><mask id="o{i}" {bx}><path {p} fill="#fff" d="{}"/></mask>"##,
            g.outline, g.holes, g.holes
        ));
        defs.push_str(&depth(format!("d{i}"), g.depth, 0.95, false));
    }
    defs.push_str(&format!(r##"<mask id="m" {bx}>{outlines}</mask><mask id="h" {bx}><rect x="-10" y="-10" width="120" height="120" fill="#fff"/>{cut}</mask>"##));
    let mut body = String::from(r#"<g mask="url(#h)">"#);
    for i in 0..set.len() {
        body.push_str(&format!(r#"<g filter="url(#f{i})"><rect mask="url(#e{i})" fill="url(#c)" width="100" height="100"/></g>"#));
    }
    body.push_str(r#"<rect mask="url(#m)" fill="url(#g)" width="100" height="100"/></g>"#);
    for i in 0..set.len() {
        body.push_str(&format!(
            r##"<g mask="url(#o{i})"><g filter="url(#d{i})"><rect mask="url(#p{i})" fill="#000" x="-10" y="-10" width="120" height="120"/></g></g>"##
        ));
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="256" height="256"><defs>{defs}</defs><g opacity=".8"><g transform="translate(5 5) scale(.9)">{body}</g></g></svg>"#
    )
}

/// THE FRAMES. A frame drawn with its light takes tens of milliseconds, too
/// slow to draw as it turns; the figure repeats every `rest_cycle` degrees, so
/// one frame a degree of that is all there is to see. A thread of its own, at
/// low priority, draws them at FRAME px once per look (`warm`); a turn shows
/// the frame nearest its angle (`frame`), one not drawn yet is skipped.
pub(crate) const FRAME: u32 = 128;

struct Frames {
    look: u32,
    frames: Vec<Option<std::sync::Arc<Vec<u8>>>>,
}

fn cache() -> &'static std::sync::Mutex<Frames> {
    static CACHE: OnceLock<std::sync::Mutex<Frames>> = OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(Frames { look: u32::MAX, frames: Vec::new() }))
}

/// Draw the frames for the present look, unless they are (being) drawn.
pub(crate) fn warm() {
    let look = crate::look::generation();
    let count = rest_cycle().round() as usize;
    {
        let Ok(mut c) = cache().lock() else { return };
        if c.look == look {
            return;
        }
        c.look = look;
        c.frames = vec![None; count];
    }
    let spawned = std::thread::Builder::new().name("gear-frames".into()).spawn(move || {
        // Background work: never in the way of the dock's own.
        // SAFETY: setpriority on the calling thread only.
        unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 15) };
        for i in 0..count {
            let Some(px) = draw(i as f32) else { continue };
            let Ok(mut c) = cache().lock() else { return };
            if c.look != look {
                return; // the look changed: a newer thread draws
            }
            c.frames[i] = Some(std::sync::Arc::new(px));
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("gears: no frame thread: {e}");
    }
}

/// The figure at `angle`, FRAME px square, premultiplied RGBA.
fn draw(angle: f32) -> Option<Vec<u8>> {
    let tree = resvg::usvg::Tree::from_data(svg(angle).as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(FRAME, FRAME)?;
    let k = FRAME as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    Some(pixmap.take())
}

/// The drawn frame nearest `angle`, if it is drawn yet.
pub(crate) fn frame(angle: f32) -> Option<std::sync::Arc<Vec<u8>>> {
    let c = cache().lock().ok()?;
    let n = c.frames.len();
    if n == 0 {
        return None;
    }
    let i = (angle.rem_euclid(rest_cycle()) / rest_cycle() * n as f32).round() as usize % n;
    c.frames[i].clone()
}

/// The turn under the pointer: the speed comes in on the soft curve, the
/// angle runs on; let go, it coasts to the next rest pose.
#[derive(Default)]
pub(crate) struct Spin {
    angle: f32,
    on: bool,
    /// The speed's share (0..1) and its ease: from, to, progress.
    k: (f32, f32, f32),
    coast: Option<Coast>,
}

struct Coast {
    a0: f32,
    v0: f32,
    to: f32,
    secs: f32,
    t: f32,
}

impl Spin {
    fn speed_share(&self) -> f32 {
        let (from, to, p) = self.k;
        from + (to - from) * crate::files_icon::soft(p)
    }

    /// The big gear's angle now.
    pub(crate) fn angle(&self) -> f32 {
        self.angle
    }

    /// Whether it is turning (a frame is wanted).
    #[cfg(test)]
    pub(crate) fn moving(&self) -> bool {
        self.on || self.coast.is_some()
    }

    /// Step by `dt` seconds with the pointer `on` it or not; whether the
    /// angle changed.
    pub(crate) fn step(&mut self, on: bool, dt: f32) -> bool {
        if on && !self.on {
            // Back on it (coasting or not): spin up from the speed it has.
            let now = match self.coast.take() {
                Some(c) => c.v0 * (1.0 - (c.t / c.secs).min(1.0)) / SPEED,
                None => 0.0,
            };
            self.k = (now, 1.0, 0.0);
            self.on = true;
        } else if !on && self.on {
            // Off it: coast to the next rest pose at least a natural stop away.
            self.on = false;
            let c = rest_cycle();
            let v0 = self.speed_share() * SPEED;
            let a0 = self.angle;
            let to = ((a0 + v0 * EASE_IN * 0.75 + 1e-4) / c).ceil() * c;
            let secs = (2.0 * (to - a0) / v0.max(c / 1.2)).min(2.5);
            self.coast = Some(Coast { a0, v0, to, secs, t: 0.0 });
        }
        if let Some(c) = &mut self.coast {
            c.t += dt;
            let x = (c.t / c.secs).min(1.0);
            let (h00, h10, h01) = (2.0 * x.powi(3) - 3.0 * x * x + 1.0, x.powi(3) - 2.0 * x * x + x, -2.0 * x.powi(3) + 3.0 * x * x);
            self.angle = h00 * c.a0 + h10 * c.secs * c.v0 + h01 * c.to;
            if x >= 1.0 {
                // On a rest pose: it IS the rest pose.
                self.angle = 0.0;
                self.coast = None;
            }
            return true;
        }
        if !self.on {
            return false;
        }
        self.k.2 = (self.k.2 + dt / EASE_IN).min(1.0);
        self.angle += dt * SPEED * self.speed_share();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_figure_parses_and_repeats_soon() {
        assert!(resvg::usvg::Tree::from_data(svg(0.0).as_bytes(), &resvg::usvg::Options::default()).is_ok());
        assert_eq!(rest_cycle(), 120.0, "18, 9 and 8 teeth with 6, 3 and 4 spokes");
    }

    #[test]
    fn let_go_it_coasts_onto_a_rest_pose() {
        let mut s = Spin::default();
        for _ in 0..50 {
            s.step(true, 0.016);
        }
        assert!(s.angle() > 10.0);
        let mut last = s.angle();
        let mut steps = 0;
        while s.step(false, 0.016) {
            steps += 1;
            assert!(s.angle() >= last - 1e-3 || s.angle() == 0.0, "never turns back");
            if s.angle() != 0.0 {
                last = s.angle();
            }
            assert!(steps < 400);
        }
        assert!((last - rest_cycle()).abs() < 1.0, "stopped a breath short of the rest pose at {last}");
        assert_eq!(s.angle(), 0.0);
        assert!(!s.moving());
    }
}

//! The main card's two doors on the dock: the Apps button, which opens it on
//! the apps grid (macOS's Launchpad), and Golem's configuration panel, which
//! opens it as a field of settings.
//!
//! OPTIONS offers the right control at the right moment, but it can't guess
//! every one (a screen resolution, a scale). The panel is the place to go for
//! the rest: a gear among the apps ([`crate::apps::SETTINGS_ID`]) that opens
//! the same card the apps live in, with the sections and search cleared away
//! so the card itself is the surface. The dock band stays, so the gear that
//! opened the panel is the one that closes it.
//!
//! The field (designed in a browser mockup with Max, 2026-09-29):
//!
//! - **Three lit layers.** Every setting is an OPTIONS pill on one of three
//!   depth layers: near (large, most lit, a glow), middle (the bar's own pill
//!   size and wash), far (small, dim, no glow). Light, not blur, carries the
//!   depth.
//! - **Balanced.** The pills relax apart like soft discs until the spacing is
//!   even, then the heap is fitted up under the dock with more air below
//!   than above.
//! - **Still, not static.** Each pill circles its own spot on a tiny, slow
//!   orbit. Under the pointer it settles, comes 7% closer and takes the bar's
//!   hover wash.
//! - **Scroll moves through the layers.** Down: the near layer falls away past
//!   the viewer and returns at the back, everything behind steps closer. Up
//!   undoes it. Each step re-lays the field and every pill glides.
//! - **A click becomes the setting.** The pill grows into the whole card; the
//!   others open away from it and fade; the setting's content expands out of
//!   the pill and floats in the air. Escape or a click on the empty space
//!   folds it back.

use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use tracing::info;

use crate::content::{self, Label, Layout, Rect, RectInst, ShadowInst};
use crate::options::{FONT_PX, LINE_PX, PILL_PAD_X, TEXT_FONT};
use crate::state::Target;
use crate::{apps, groups, App};
use waverunner_proto::Command;

/// The settings and the layer each starts on (0 near, 1 middle, 2 far):
/// the ones people reach for float nearest. Placeholders until each becomes
/// a real setting.
const SETTINGS: [(&str, u8); 20] = [
    ("Resolution", 0),
    ("Scale", 0),
    ("Wi-Fi", 0),
    ("Sound output", 0),
    ("Dark mode", 0),
    ("Brightness", 0),
    ("Bluetooth", 0),
    ("Volume", 0),
    ("Wallpaper", 0),
    ("Night light", 1),
    ("Accent colour", 1),
    ("Keyboard layout", 1),
    ("Power mode", 1),
    ("Notifications", 1),
    ("Touchpad speed", 1),
    ("Language", 1),
    ("Time zone", 2),
    ("Natural scrolling", 2),
    ("Updates", 2),
    ("About Golem", 2),
];

// ---- The layers --------------------------------------------------------
/// Size of each layer against the OPTIONS bar's own pill (the middle layer
/// IS the bar's pill).
const LAYER_SCALE: [f32; 3] = [1.53, 1.0, 0.8];
/// Resting wash alpha per layer, on a dark card (white wash) and a bright
/// one (black wash, which reads stronger at equal alpha).
const LAYER_WASH_DARK: [f32; 3] = [0.19, 0.10, 0.055];
const LAYER_WASH_BRIGHT: [f32; 3] = [0.16, 0.085, 0.045];
/// The OPTIONS hover wash, the same on every layer.
const HOVER_WASH_DARK: f32 = 0.27;
const HOVER_WASH_BRIGHT: f32 = 0.30;
/// Ink strength per layer.
const LAYER_INK: [f32; 3] = [1.0, 0.84, 0.62];
/// Soft glow per layer, the pill's edge: (blur px, alpha). Smaller and
/// fainter further back, but never gone — without it the far pills lost
/// their edge (Max, 2026-09-29).
const LAYER_GLOW: [(f32, f32); 3] = [(7.0, 0.17), (3.5, 0.08), (2.0, 0.05)];

/// The middle layer: the OPTIONS bar's own pill, and the look any other
/// pill on the card borrows (the search pill).
pub(crate) const MIDDLE: usize = 1;

/// A pill's fill on `layer`, `hover` 0 → 1 easing to the hover wash.
pub(crate) fn pill_wash(layer: usize, bright: bool, hover: f32) -> [f32; 4] {
    let (rest, hov) = if bright {
        (LAYER_WASH_BRIGHT[layer], HOVER_WASH_BRIGHT)
    } else {
        (LAYER_WASH_DARK[layer], HOVER_WASH_DARK)
    };
    crate::options::wash(!bright, lerp(rest, hov, hover))
}

/// A pill's soft edge on `layer`, at `alpha` presence.
pub(crate) fn pill_glow(layer: usize, bright: bool, rect: Rect, scale: f32, alpha: f32) -> ShadowInst {
    let (blur, a) = LAYER_GLOW[layer];
    let v = if bright { 0.0 } else { 1.0 };
    ShadowInst {
        rect,
        radius: rect.h / 2.0,
        blur: blur * scale,
        color: [v, v, v, a * alpha],
        edges: [1.0, 1.0, 1.0, 1.0],
    }
}

/// A pill's text colour on `layer`, at `alpha` presence.
pub(crate) fn pill_ink(layer: usize, ink: [f32; 4], alpha: f32) -> [f32; 4] {
    [ink[0], ink[1], ink[2], ink[3] * LAYER_INK[layer] * alpha]
}
/// How "near" each layer is: sets the orbit radius.
const LAYER_Z: [f32; 3] = [1.0, 0.62, 0.35];

// ---- Layout (px at bar scale 1) -----------------------------------------
/// Air kept off the card's sides: the ends of the widest row stop this far
/// from them ("the middle side pills almost touch the sides", Max
/// 2026-09-30).
const EDGE: f32 = 6.0;
/// Air under the dock, and above the card's floor as a share of the field:
/// the heap sits up under the dock with more room below.
const GAP_TOP: f32 = 16.0;
const GAP_BOTTOM_FRAC: f32 = 0.30;
/// The whole heap sits this much lower than those gaps alone would put it:
/// the top gap grows by it and the bottom one shrinks by it (Max: "a little
/// lower").
const DROP: f32 = 34.0;
/// The rows, top to bottom, as each one's width against the widest: brick
/// courses on a gentle ellipse, so the middle reaches the sides and the
/// top and bottom rows are held in — wide, and a little round.
const ROWS: [f32; 5] = [0.74, 0.92, 1.0, 0.92, 0.74];
/// How far a pill strays from its brick spot, as a share of the room around
/// it (x: of its row's mean gap, y: of the row height): enough to read
/// organic, never enough to break the courses.
const JITTER_X: f32 = 0.35;
const JITTER_Y: f32 = 0.22;

/// The field's (top, bottom) air for a field `h` tall at bar scale `s`.
fn gaps(h: f32, s: f32) -> (f32, f32) {
    ((GAP_TOP + DROP) * s, (GAP_BOTTOM_FRAC * h - DROP * s).max(GAP_TOP * s))
}

/// How many of `n` pills each row holds: in proportion to its width
/// (largest remainders get the leftovers, so no row is crowded — every row
/// ends up about as dense as the others). Rows of different widths and
/// counts put their gaps in different places: the pills cross like bricks.
fn row_counts(n: usize) -> Vec<usize> {
    let total: f32 = ROWS.iter().sum();
    let exact: Vec<f32> = ROWS.iter().map(|f| n as f32 * f / total).collect();
    let mut counts: Vec<usize> = exact.iter().map(|e| e.floor() as usize).collect();
    let mut rest: Vec<usize> = (0..ROWS.len()).collect();
    rest.sort_by(|&a, &b| (exact[b] - exact[b].floor()).total_cmp(&(exact[a] - exact[a].floor())));
    let placed: usize = counts.iter().sum();
    for &r in rest.iter().cycle().take(n.saturating_sub(placed)) {
        counts[r] += 1;
    }
    counts
}

// ---- Motion (rates are 1/s for exponential approach) --------------------
const GLIDE_RATE: f32 = 16.0;
const SIZE_RATE: f32 = 17.0;
const FADE_IN_RATE: f32 = 15.0;
/// Seconds for a layer to leave from its end.
const EXIT_SECS: f32 = 0.15;
/// Wheel travel for one layer step, and the shortest gap between steps.
const WHEEL_STEP: f64 = 10.0;
const SHIFT_COOLDOWN: Duration = Duration::from_millis(200);
/// Hover: how fast a pill settles, and comes closer (and by how much).
const SETTLE_RATE: f32 = 6.0;
const LIFT_RATE: f32 = 12.0;
const LIFT: f32 = 0.07;
/// The open-box morph's rate.
const OPEN_RATE: f32 = 14.0;
/// At rest the only motion is the tiny orbits (well under a pixel a second),
/// so the panel redraws on this slow cadence instead of every vsync: a
/// continuously animating card at 165 Hz saturated an iGPU and stalled the
/// dock's loop (Lenovo Iris Xe, 2026-09-30). Anything that actually moves
/// runs at full rate.
pub(crate) const IDLE_TICK: Duration = Duration::from_millis(80);

/// A tiny deterministic generator: the field looks the same every time it
/// opens (and across restarts) instead of reshuffling under the eye.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        // xorshift32
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Approach `target` at `rate`/s over `dt`.
fn approach(v: f32, target: f32, rate: f32, dt: f32) -> f32 {
    v + (target - v) * (1.0 - (-dt * rate).exp())
}

/// A layer leaving from its end of the stack.
#[derive(Clone, Copy)]
struct Exit {
    /// +1: the near layer falling away past the viewer; -1: the far layer
    /// receding into the distance.
    dir: i8,
    /// 0 → 1 over [`EXIT_SECS`].
    k: f32,
}

/// One setting's pill. Positions are field-relative centres.
struct Pill {
    label: &'static str,
    /// Which settings travel together (the layer it started on).
    group: u8,
    /// The layer it is on now, and the one it is drawn as (they differ only
    /// while it leaves one end for the other).
    layer: u8,
    drawn: u8,
    /// Measured size at each layer.
    w: [f32; 3],
    h: [f32; 3],
    /// Where the layout wants it (`ideal` before settling, `home` after).
    ideal: (f32, f32),
    home: (f32, f32),
    /// The spot it orbits, gliding toward `home`.
    anchor: (f32, f32),
    /// Drawn centre this frame.
    pos: (f32, f32),
    /// Size multiplier easing home after a layer change, and presence.
    sc: f32,
    op: f32,
    exit: Option<Exit>,
    /// Orbit clock and its rate (eases to a stop under the pointer).
    clock: f32,
    rate: f32,
    lift: f32,
    phase: [f32; 2],
    period: f32,
    spin: f32,
    /// Where it strays from its brick spot (x, y) and how much room it
    /// takes after it in its row, each a random -1..1.
    stray: [f32; 3],
}

impl Pill {
    fn size(&self) -> (f32, f32) {
        let l = self.layer as usize;
        (self.w[l], self.h[l])
    }
}

/// The pill that became the whole card.
struct OpenBox {
    pill: usize,
    /// 0 = the pill, 1 = the whole card; eases toward `want`.
    k: f32,
    want: f32,
    /// The pill's drawn rect when clicked (field-relative).
    from: Rect,
}

/// What the panel draws this frame, in surface coordinates. `content` puts
/// the rects and labels in a grid clipped to the card and the glows among
/// the overlay shadows.
#[derive(Debug, Default)]
pub(crate) struct PanelDraw {
    pub rects: Vec<RectInst>,
    pub labels: Vec<Label>,
    pub glows: Vec<ShadowInst>,
}

/// The card's colours the panel draws in.
#[derive(Clone, Copy)]
pub(crate) struct PanelPaint {
    pub ink: [f32; 4],
    /// The card reads bright (dark ink): black washes and shadows.
    pub bright: bool,
}

/// The settings field: its pills, their layout and motion, the layer cycle
/// and the open setting.
#[derive(Default)]
pub(crate) struct Panel {
    pills: Vec<Pill>,
    /// Field size and bar scale the pills were measured and laid out for.
    key: (f32, f32, f32),
    /// The field this frame, in surface coordinates.
    field: Rect,
    scale: f32,
    /// Layer steps taken (the cycle's position).
    shift: i32,
    /// Seconds the panel has been drifting.
    clock: f32,
    wheel: f64,
    wheel_at: Option<Instant>,
    last_shift: Option<Instant>,
    hot: Option<usize>,
    open: Option<OpenBox>,
    /// The order the pills fill the rows in: shuffled once, so near, middle
    /// and far mix through every row, and fixed, so a layer change keeps the
    /// arrangement and only the sizes change.
    order: Vec<usize>,
    /// Wall-clock time of the last step: the panel keeps its own time, since
    /// on the idle cadence the frame loop's dt (which skips idle gaps) would
    /// slow the orbits to a crawl.
    last_step: Option<Instant>,
}

impl Panel {
    /// Build (or rebuild for a new field size or bar scale) the pills:
    /// measure every label at every layer's size, then lay them out.
    fn ensure(&mut self, size: (f32, f32), scale: f32, pill_h: f32, measure: &mut dyn FnMut(&str, f32) -> f32) {
        let key = (size.0, size.1, scale);
        let fresh = self.pills.is_empty();
        if !fresh
            && (self.key.0 - key.0).abs() < 0.5
            && (self.key.1 - key.1).abs() < 0.5
            && (self.key.2 - key.2).abs() < 0.001
        {
            return;
        }
        self.key = key;
        self.scale = scale;
        if fresh {
            let mut rng = Rng(0x85EB_CA6B);
            self.pills = SETTINGS
                .iter()
                .map(|&(label, group)| Pill {
                    label,
                    group,
                    layer: group,
                    drawn: group,
                    w: [0.0; 3],
                    h: [0.0; 3],
                    ideal: (0.0, 0.0),
                    home: (0.0, 0.0),
                    anchor: (0.0, 0.0),
                    pos: (0.0, 0.0),
                    sc: 1.0,
                    op: 1.0,
                    exit: None,
                    clock: rng.next() * 20.0,
                    rate: 1.0,
                    lift: 0.0,
                    phase: [rng.next() * TAU, rng.next() * TAU],
                    period: 12.0 + rng.next() * 6.0,
                    spin: if rng.next() < 0.5 { -1.0 } else { 1.0 },
                    stray: [rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0],
                })
                .collect();
            self.order = brick_order(&self.pills, &mut rng);
        }
        for p in &mut self.pills {
            for (l, &f) in LAYER_SCALE.iter().enumerate() {
                let h = pill_h * f;
                p.h[l] = h;
                p.w[l] = (measure(p.label, FONT_PX * scale * f) + 2.0 * PILL_PAD_X * scale * f).max(h);
            }
        }
        self.compose(size, true);
    }

    /// Lay the field out for the current layer order: brick courses, wide
    /// and a little round, up under the dock, settled so nothing crowds. `snap` puts every pill
    /// there at once; otherwise they glide.
    fn compose(&mut self, size: (f32, f32), snap: bool) {
        let s = self.scale.max(0.01);
        let (w, h) = size;
        let (top, bottom) = gaps(h, s);
        bricks(&mut self.pills, &self.order, (w, h), EDGE * s, (top, bottom));
        for p in &mut self.pills {
            p.home = p.ideal;
        }
        clamp_all(&mut self.pills, (w, h), EDGE * s, (top, bottom));
        // Only where a stray pushed two too close: nudge apart, held near
        // the brick spots.
        settle(&mut self.pills, (w, h), EDGE * s, (top, bottom), 12.0 * s, 0.12, 160);
        if snap {
            for p in &mut self.pills {
                p.anchor = p.home;
                p.pos = p.home;
            }
        }
    }

    /// Move through the layers: +1 the near layer falls away and returns at
    /// the back, -1 the far layer recedes and returns in front.
    fn shift_layers(&mut self, dir: i8) {
        if self.open.is_some() || self.pills.is_empty() {
            return;
        }
        self.shift += dir as i32;
        for p in &mut self.pills {
            let old = p.layer;
            p.layer = (p.group as i32 - self.shift).rem_euclid(3) as u8;
            let wraps = if dir > 0 { old == 0 } else { old == 2 };
            if wraps {
                p.exit = Some(Exit { dir, k: 0.0 });
            } else if p.exit.is_none() {
                // Hold the old size by scale; it eases to the new one while
                // the pill glides to its new spot.
                p.sc *= p.h[p.drawn as usize] / p.h[p.layer as usize];
                p.drawn = p.layer;
            }
        }
        self.hot = None;
        let size = (self.key.0, self.key.1);
        self.compose(size, false);
    }

    /// Wheel travel over the panel: a layer step once it adds up, never
    /// faster than the move itself.
    fn wheel(&mut self, value: f64) {
        let now = Instant::now();
        if self.wheel_at.is_none_or(|t| now - t > Duration::from_millis(250)) {
            self.wheel = 0.0;
        }
        self.wheel_at = Some(now);
        self.wheel += value;
        if self.wheel.abs() >= WHEEL_STEP && self.last_shift.is_none_or(|t| now - t > SHIFT_COOLDOWN) {
            self.shift_layers(if self.wheel > 0.0 { 1 } else { -1 });
            self.last_shift = Some(now);
            self.wheel = 0.0;
        }
    }

    /// The pill under `pos` (surface coordinates), nearest layer first.
    fn pill_at(&self, pos: (f32, f32)) -> Option<usize> {
        let (fx, fy) = (pos.0 - self.field.x, pos.1 - self.field.y);
        let mut best: Option<(usize, u8)> = None;
        for (i, p) in self.pills.iter().enumerate() {
            if p.exit.is_some() || p.op < 0.5 {
                continue;
            }
            let d = p.drawn as usize;
            let k = p.sc * (1.0 + LIFT * p.lift);
            let (hw, hh) = (p.w[d] * k / 2.0, p.h[d] * k / 2.0);
            if (fx - p.pos.0).abs() <= hw && (fy - p.pos.1).abs() <= hh && best.is_none_or(|(_, l)| p.drawn < l) {
                best = Some((i, p.drawn));
            }
        }
        best.map(|(i, _)| i)
    }

    /// A click on the card below the dock. Returns whether it did anything.
    fn click(&mut self, pos: (f32, f32)) -> bool {
        if let Some(open) = &mut self.open {
            // Open, the setting is the whole card: a click on its floating
            // controls is theirs; anywhere else folds it back.
            if open.want > 0.5 && !content_hit(self.key, self.scale, pos, self.field) {
                open.want = 0.0;
                info!("settings: closing {}", self.pills[open.pill].label);
            }
            return true;
        }
        let Some(i) = self.pill_at(pos) else {
            return false;
        };
        let p = &self.pills[i];
        let d = p.drawn as usize;
        let k = p.sc * (1.0 + LIFT * p.lift);
        let (w, h) = (p.w[d] * k, p.h[d] * k);
        info!("settings: opening {}", p.label);
        self.open = Some(OpenBox {
            pill: i,
            k: 0.0,
            want: 1.0,
            from: Rect::new(p.pos.0 - w / 2.0, p.pos.1 - h / 2.0, w, h),
        });
        self.hot = None;
        true
    }

    /// Whether anything is moving beyond the idle orbits: a layer leaving or
    /// arriving, a pill gliding, easing in size or presence, settling or
    /// lifting under the pointer, or the open setting growing or folding.
    fn is_moving(&self) -> bool {
        if self.open.as_ref().is_some_and(|o| (o.k - o.want).abs() > 0.002) {
            return true;
        }
        self.pills.iter().enumerate().any(|(i, p)| {
            let hot = self.hot == Some(i);
            let (lift_to, rate_to) = if hot { (1.0, 0.0) } else { (0.0, 1.0) };
            p.exit.is_some()
                || (p.sc - 1.0).abs() > 0.002
                || p.op < 0.995
                || (p.lift - lift_to).abs() > 0.002
                || (p.rate - rate_to).abs() > 0.01
                || (p.anchor.0 - p.home.0).abs() > 0.05
                || (p.anchor.1 - p.home.1).abs() > 0.05
        })
    }

    /// Escape: fold an open setting back. Returns whether one was open.
    fn escape(&mut self) -> bool {
        match &mut self.open {
            Some(open) if open.want > 0.5 => {
                open.want = 0.0;
                true
            }
            _ => false,
        }
    }

    /// Forget any open setting (the panel is opening afresh).
    pub(crate) fn reset(&mut self) {
        self.open = None;
        self.hot = None;
    }

    /// Advance the field by `dt`; `pointer` is in surface coordinates.
    fn step(&mut self, dt: f32, pointer: Option<(f32, f32)>) {
        self.clock += dt;
        if let Some(open) = &mut self.open {
            open.k = approach(open.k, open.want, OPEN_RATE, dt);
            if open.want == 0.0 && open.k < 0.01 {
                self.open = None;
            }
        }
        self.hot = if self.open.is_none() { pointer.and_then(|p| self.pill_at(p)) } else { None };
        let s = self.scale;
        for (i, p) in self.pills.iter_mut().enumerate() {
            let hot = self.hot == Some(i);
            if p.exit.is_none() {
                p.anchor.0 = approach(p.anchor.0, p.home.0, GLIDE_RATE, dt);
                p.anchor.1 = approach(p.anchor.1, p.home.1, GLIDE_RATE, dt);
            }
            p.rate = approach(p.rate, if hot { 0.0 } else { 1.0 }, SETTLE_RATE, dt);
            p.clock += dt * p.rate;
            p.lift = approach(p.lift, if hot { 1.0 } else { 0.0 }, LIFT_RATE, dt);
            p.sc = approach(p.sc, 1.0, SIZE_RATE, dt);
            if let Some(exit) = &mut p.exit {
                exit.k = (exit.k + dt / EXIT_SECS).min(1.0);
                p.op = 1.0 - smoothstep(exit.k);
                if exit.k >= 1.0 {
                    // Arrive at the other end: the back, small and faint, or
                    // (going back) the front, rising from below.
                    let back = exit.dir < 0;
                    p.exit = None;
                    p.drawn = p.layer;
                    p.anchor = (p.home.0, p.home.1 + if back { 70.0 * s } else { 0.0 });
                    p.sc = if back { 1.25 } else { 0.6 };
                    p.op = 0.0;
                }
            } else {
                p.op = approach(p.op, 1.0, FADE_IN_RATE, dt);
            }
            // A tiny orbit round the spot: about a pixel, one slow turn every
            // 12–18 s, its radius breathing a touch.
            let z = LAYER_Z[p.drawn as usize];
            let radius = (0.6 + 0.6 * z) * s * (1.0 + 0.1 * (p.clock * 0.07 * TAU + p.phase[1]).sin());
            let turn = p.clock * TAU / p.period + p.phase[0];
            p.pos = (p.anchor.0 + radius * turn.cos() * p.spin, p.anchor.1 + radius * turn.sin());
        }
    }

    /// Everything to draw this frame.
    fn draw(&self, paint: PanelPaint) -> PanelDraw {
        let mut out = PanelDraw::default();
        let s = self.scale;
        let f = self.field;
        let open_k = self.open.as_ref().map_or(0.0, |o| o.k);
        let open_pill = self.open.as_ref().map(|o| o.pill);
        let open_from = self.open.as_ref().map(|o| o.from);

        // Far first, near last; a leaving near layer and a lifted pill on top.
        let mut order: Vec<usize> = (0..self.pills.len()).collect();
        let rank = |p: &Pill| -> i32 {
            match p.exit {
                Some(e) if e.dir > 0 => 10,
                Some(_) => -1,
                None if p.lift > 0.02 => 9,
                None => 3 - p.drawn as i32,
            }
        };
        order.sort_by_key(|&i| rank(&self.pills[i]));

        for i in order {
            let p = &self.pills[i];
            let d = p.drawn as usize;
            let (mut dx, mut dy, mut grow, mut fade, mut shrink) = (0.0, 0.0, 1.0, 1.0, 1.0);
            if let Some(e) = p.exit {
                let ease = 1.0 - (1.0 - e.k) * (1.0 - e.k);
                if e.dir > 0 {
                    dy = 90.0 * s * ease; // falls past the viewer
                    grow = 1.0 + 0.4 * ease;
                } else {
                    dy = -8.0 * s * e.k; // recedes
                    grow = 1.0 - 0.5 * e.k;
                }
            }
            if let (Some(op), Some(from)) = (open_pill, open_from) {
                if op == i {
                    fade = 0.0; // the box has taken its place
                } else {
                    // Opened out of the clicked pill: pushed away along the
                    // line from it, fading, the nearest furthest.
                    let (cx, cy) = (from.x + from.w / 2.0, from.y + from.h / 2.0);
                    let (ox, oy) = (p.pos.0 - cx, p.pos.1 - cy);
                    let dist = ox.hypot(oy).max(1.0);
                    let push = 140.0 * s * open_k * (0.6 + 0.4 * (260.0 * s / dist).min(1.0));
                    dx += ox / dist * push;
                    dy += oy / dist * push;
                    fade = 1.0 - open_k;
                    shrink = 1.0 - 0.12 * open_k;
                }
            }
            let a = p.op * fade;
            if a < 0.004 {
                continue;
            }
            let k = p.sc * grow * shrink * (1.0 + LIFT * p.lift);
            let (w, h) = (p.w[d] * k, p.h[d] * k);
            let rect = Rect::new(f.x + p.pos.0 + dx - w / 2.0, f.y + p.pos.1 + dy - h / 2.0, w, h);
            let radius = h / 2.0;
            out.glows.push(pill_glow(d, paint.bright, rect, s, a));
            let c = pill_wash(d, paint.bright, p.lift);
            out.rects.push(RectInst {
                rect,
                radius,
                color: [c[0], c[1], c[2], c[3] * a],
                glass: 0.0,
                border: 0.0,
            });
            let font = FONT_PX * s * LAYER_SCALE[d] * k;
            let line = LINE_PX * s * LAYER_SCALE[d] * k;
            // Glyphs are cached only at rest: an easing size would fill the
            // cache with one entry per frame.
            let resting = (k - 1.0).abs() < 0.001 && p.exit.is_none();
            out.labels.push(Label {
                text: p.label.to_owned(),
                pos: (rect.x + w / 2.0, rect.y + (h - line) / 2.0),
                max_w: w,
                font_px: font,
                line_px: line,
                centered: true,
                dim: false,
                cache: resting,
                family: TEXT_FONT,
                color: Some(pill_ink(d, paint.ink, a)),
                clip: None,
            });
        }

        if let Some(open) = &self.open {
            self.draw_open(open, paint, &mut out);
        }
        out
    }

    /// The pill that became the card, and the setting growing out of it.
    fn draw_open(&self, open: &OpenBox, paint: PanelPaint, out: &mut PanelDraw) {
        let s = self.scale;
        let f = self.field;
        let k = open.k;
        let p = &self.pills[open.pill];
        let d = p.drawn as usize;
        let (fw, fh) = (f.w, f.h);
        // The box: the pill's rect easing to the whole field, its stadium
        // easing to the card's corner.
        let r = Rect::new(
            f.x + lerp(open.from.x, 0.0, k),
            f.y + lerp(open.from.y, 0.0, k),
            lerp(open.from.w, fw, k),
            lerp(open.from.h, fh, k),
        );
        // Its fill is cleared by halfway, so it is gone before the content
        // comes to rest: open, the card itself is the surface.
        let clear = smoothstep(k / 0.5);
        let layer_wash = if paint.bright { LAYER_WASH_BRIGHT } else { LAYER_WASH_DARK };
        let c = crate::options::wash(!paint.bright, layer_wash[d]);
        if clear < 0.999 {
            let radius = lerp(open.from.h / 2.0, content::BOX_CORNER_RADIUS, k);
            out.rects.push(RectInst {
                rect: r,
                radius,
                color: [c[0], c[1], c[2], c[3] * (1.0 - clear)],
                glass: 0.0,
                border: 0.0,
            });
            // Its label fades out early, as the setting takes over.
            let la = 1.0 - (k / 0.35).clamp(0.0, 1.0);
            if la > 0.004 {
                let font = FONT_PX * s * LAYER_SCALE[d];
                let line = LINE_PX * s * LAYER_SCALE[d];
                out.labels.push(Label {
                    text: p.label.to_owned(),
                    pos: (r.x + r.w / 2.0, r.y + (r.h - line) / 2.0),
                    max_w: r.w,
                    font_px: font,
                    line_px: line,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: TEXT_FONT,
                    color: Some([paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * la]),
                    clip: None,
                });
            }
        }

        // The setting's content, laid out at rest in the whole field, then
        // expanded out of the one point the box grows about (the point its
        // pill-rect → field-rect lerp leaves fixed), so the two read as one
        // motion instead of a slide.
        let ca = ((k - 0.25) / 0.55).clamp(0.0, 1.0);
        if ca < 0.004 {
            return;
        }
        let fx = open.from.x * fw / (fw - open.from.w).max(1.0);
        let fy = open.from.y * fh / (fh - open.from.h).max(1.0);
        let cs = lerp(0.08, 1.0, k);
        let map = |x: f32, y: f32| (f.x + fx + (x - fx) * cs, f.y + fy + (y - fy) * cs);
        let ink = |a: f32| Some([paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * a * ca]);
        let resting = (cs - 1.0).abs() < 0.001;
        let text = |out: &mut PanelDraw, t: String, x: f32, y: f32, px: f32, lpx: f32, a: f32| {
            let (mx, my) = map(x, y);
            out.labels.push(Label {
                text: t,
                pos: (mx, my),
                max_w: fw * cs,
                font_px: px * s * cs,
                line_px: lpx * s * cs,
                centered: true,
                dim: false,
                cache: resting,
                family: TEXT_FONT,
                color: ink(a),
                clip: None,
            });
        };
        let rect = |out: &mut PanelDraw, x: f32, y: f32, w: f32, h: f32, radius: f32, color: [f32; 4]| {
            let (mx, my) = map(x, y);
            out.rects.push(RectInst {
                rect: Rect::new(mx, my, w * cs, h * cs),
                radius: radius * cs,
                color: [color[0], color[1], color[2], color[3] * ca],
                glass: 0.0,
                border: 0.0,
            });
        };
        let cx = fw / 2.0;
        let mut y = 30.0 * s;
        text(out, p.label.to_owned(), cx, y, 30.0, 36.0, 1.0);
        y += 44.0 * s;
        text(out, format!("Placeholder for the {} controls.", p.label.to_lowercase()), cx, y, 14.0, 18.0, 0.55);
        y += 40.0 * s;
        // The controls float in the air like the pills did, in their
        // material, each on its own tiny orbit.
        let (rw, rh) = (440.0 * s, 46.0 * s);
        let row_wash = crate::options::wash(!paint.bright, if paint.bright { 0.10 } else { 0.11 });
        let track = crate::options::wash(!paint.bright, 0.16);
        let fill = crate::options::wash(!paint.bright, 0.6);
        let amber = [
            crate::options::srgb_to_linear(0.94),
            crate::options::srgb_to_linear(0.70),
            crate::options::srgb_to_linear(0.35),
            1.0,
        ];
        let rows: [(&str, u8); 3] = [("Level", 0), ("Automatic", 1), ("Apply to", 2)];
        for (i, (name, kind)) in rows.iter().enumerate() {
            let a = self.clock * TAU / (13.0 + 2.5 * i as f32) + 1.9 * i as f32;
            let (ox, oy) = (1.1 * s * a.cos(), 1.1 * s * a.sin());
            let (x0, y0) = (cx - rw / 2.0 + ox, y + oy);
            rect(out, x0, y0, rw, rh, rh / 2.0, row_wash);
            let line = 20.0 * s;
            text(out, (*name).to_owned(), x0 + 70.0 * s, y0 + (rh - line) / 2.0, 16.0, 20.0, 1.0);
            match kind {
                0 => {
                    let (tx, tw) = (x0 + 150.0 * s, 200.0 * s);
                    rect(out, tx, y0 + rh / 2.0 - 3.0 * s, tw, 6.0 * s, 3.0 * s, track);
                    rect(out, tx, y0 + rh / 2.0 - 3.0 * s, tw * 0.62, 6.0 * s, 3.0 * s, fill);
                    text(out, "62%".to_owned(), x0 + rw - 45.0 * s, y0 + (rh - 17.0 * s) / 2.0, 13.0, 17.0, 0.7);
                }
                1 => {
                    let (tw, th) = (40.0 * s, 23.0 * s);
                    let (tx, ty) = (x0 + rw - 22.0 * s - tw, y0 + (rh - th) / 2.0);
                    rect(out, tx, ty, tw, th, th / 2.0, amber);
                    let knob = 16.0 * s;
                    let dark = [0.01, 0.006, 0.0, 1.0];
                    rect(out, tx + tw - 3.5 * s - knob, ty + (th - knob) / 2.0, knob, knob, knob / 2.0, dark);
                }
                _ => {
                    text(out, "this screen".to_owned(), x0 + rw - 75.0 * s, y0 + (rh - 17.0 * s) / 2.0, 13.0, 17.0, 0.7);
                }
            }
            y += rh + 22.0 * s;
        }
        text(out, "Esc or a click on the empty space closes".to_owned(), cx, y + 6.0 * s, 12.0, 16.0, 0.4);
    }
}

/// Whether `pos` (surface) lands on one of an open setting's floating
/// controls — the rows laid out by [`Panel::draw_open`] at rest.
fn content_hit(key: (f32, f32, f32), scale: f32, pos: (f32, f32), field: Rect) -> bool {
    let s = scale;
    let (rw, rh) = (440.0 * s, 46.0 * s);
    let x0 = field.x + key.0 / 2.0 - rw / 2.0;
    let mut y = field.y + 30.0 * s + 44.0 * s + 40.0 * s;
    for _ in 0..3 {
        if Rect::new(x0, y, rw, rh).contains(pos) {
            return true;
        }
        y += rh + 22.0 * s;
    }
    false
}

/// Keep a pill inside the field: `edge` off the sides, `gaps` (top, bottom)
/// off the dock and the floor.
fn clamp_pill(p: &mut Pill, size: (f32, f32), edge: f32, gaps: (f32, f32)) {
    let (w, h) = p.size();
    p.home.0 = p.home.0.clamp(edge + w / 2.0, (size.0 - edge - w / 2.0).max(edge + w / 2.0));
    p.home.1 = p.home.1.clamp(gaps.0 + h / 2.0, (size.1 - gaps.1 - h / 2.0).max(gaps.0 + h / 2.0));
}

fn clamp_all(pills: &mut [Pill], size: (f32, f32), edge: f32, gaps: (f32, f32)) {
    for p in pills {
        clamp_pill(p, size, edge, gaps);
    }
}

/// Signed air between two pills at their homes: positive = gap.
fn air(a: &Pill, b: &Pill) -> f32 {
    let (aw, ah) = a.size();
    let (bw, bh) = b.size();
    ((a.home.0 - b.home.0).abs() - (aw + bw) / 2.0).max((a.home.1 - b.home.1).abs() - (ah + bh) / 2.0)
}

/// The order the pills fill the rows in: each group (layer) spread evenly
/// through the sequence — at every step the group furthest behind its
/// share goes next — so near, middle and far alternate across every row
/// instead of bunching; which pill of a group comes next is random.
fn brick_order(pills: &[Pill], rng: &mut Rng) -> Vec<usize> {
    let mut groups: [Vec<usize>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (i, p) in pills.iter().enumerate() {
        groups[(p.group as usize).min(2)].push(i);
    }
    for g in &mut groups {
        for i in (1..g.len()).rev() {
            let j = ((rng.next() * (i + 1) as f32) as usize).min(i);
            g.swap(i, j);
        }
    }
    let n = pills.len().max(1) as f32;
    let share: Vec<f32> = groups.iter().map(|g| g.len() as f32 / n).collect();
    let mut used = [0usize; 3];
    let mut order = Vec::with_capacity(pills.len());
    for step in 0..pills.len() {
        let due = |k: usize| share[k] * (step + 1) as f32 - used[k] as f32;
        let Some(k) = (0..3)
            .filter(|&k| used[k] < groups[k].len())
            .max_by(|&a, &b| due(a).total_cmp(&due(b)))
        else {
            break;
        };
        order.push(groups[k][used[k]]);
        used[k] += 1;
    }
    order
}

/// Brick courses: the pills, in `order`, fill [`ROWS`] top to bottom. Each
/// row is centred and justified across its width (the widest reaching `edge`
/// off the card's sides), its gaps varied a little per pill, and every pill
/// strays a touch off its spot. Neighbouring rows hold different counts, so
/// the gaps cross like bricks; the strays keep it organic. Writes `ideal`.
fn bricks(pills: &mut [Pill], order: &[usize], size: (f32, f32), edge: f32, gaps: (f32, f32)) {
    let (w, _) = size;
    let (top, bottom) = (gaps.0, size.1 - gaps.1);
    let counts = row_counts(order.len());
    let row_h = (bottom - top).max(1.0) / ROWS.len() as f32;
    let full = (w - 2.0 * edge).max(1.0);
    let mut next = 0;
    for (r, (&frac, &n)) in ROWS.iter().zip(&counts).enumerate() {
        let row: Vec<usize> = order[next..(next + n).min(order.len())].to_vec();
        next += n;
        if row.is_empty() {
            continue;
        }
        let span = full * frac;
        let widths: f32 = row.iter().map(|&i| pills[i].size().0).sum();
        // Each gap takes a varied share of the free room.
        let weights: Vec<f32> = row.iter().skip(1).map(|&i| 1.0 + 0.2 * pills[i].stray[2]).collect();
        let wsum: f32 = weights.iter().sum::<f32>().max(0.001);
        let free = (span - widths).max(0.0);
        let mean_gap = if row.len() > 1 { free / (row.len() - 1) as f32 } else { 0.0 };
        let cy = top + (r as f32 + 0.5) * row_h;
        // A lone pill sits in the middle; a row starts at its left end.
        let mut x = if row.len() == 1 { w / 2.0 - pills[row[0]].size().0 / 2.0 } else { w / 2.0 - span / 2.0 };
        for (k, &i) in row.iter().enumerate() {
            if k > 0 {
                x += free * weights[k - 1] / wsum;
            }
            let pw = pills[i].size().0;
            let p = &mut pills[i];
            // Ends stray inward only, so the widest row still nearly
            // touches the sides.
            let mut sx = p.stray[0] * JITTER_X * mean_gap;
            if k == 0 {
                sx = sx.abs();
            } else if k == row.len() - 1 {
                sx = -sx.abs();
            }
            p.ideal = (x + pw / 2.0 + sx, cy + p.stray[1] * JITTER_Y * row_h);
            x += pw;
        }
    }
    for p in pills.iter_mut() {
        p.home = p.ideal;
    }
}

/// Settle: pills closer than `gap` push apart, each is pulled back toward
/// its `ideal` by `pull`, and all stay inside the field.
fn settle(pills: &mut [Pill], size: (f32, f32), edge: f32, gaps: (f32, f32), gap: f32, pull: f32, iters: usize) {
    for _ in 0..iters {
        for i in 0..pills.len() {
            for j in i + 1..pills.len() {
                let g = air(&pills[i], &pills[j]);
                if g >= gap {
                    continue;
                }
                let (ax, ay) = pills[i].home;
                let (bx, by) = pills[j].home;
                let (dx, dy) = (bx - ax, (by - ay) * 2.2);
                let d = dx.hypot(dy).max(1.0);
                let push = (gap - g) * 0.25;
                let (ux, uy) = (dx / d, dy / d);
                pills[i].home = (ax - ux * push, ay - uy * push * 0.6);
                pills[j].home = (bx + ux * push, by + uy * push * 0.6);
            }
        }
        for p in pills.iter_mut() {
            p.home.0 += (p.ideal.0 - p.home.0) * pull;
            p.home.1 += (p.ideal.1 - p.home.1) * pull;
            clamp_pill(p, size, edge, gaps);
        }
    }
}

impl App {
    /// One-shot: pin the Settings gear to the dock, just before the Recycle
    /// Bin so the bin keeps its place. Same marker rule as `pin_trash_once`: a
    /// user who later unpins or moves it isn't overruled on the next start.
    pub(crate) fn pin_settings_once(&mut self) {
        let trash = format!("group:{}", groups::TRASH_ID);
        let slot = self
            .pins
            .pins()
            .iter()
            .position(|p| *p == trash)
            .unwrap_or(self.pins.pins().len());
        self.pin_once(apps::SETTINGS_ID, "settings-pinned", slot);
    }

    /// One-shot: pin the Apps button first on the dock, where Launchpad sits
    /// on a Mac. Same marker rule as [`Self::pin_settings_once`].
    pub(crate) fn pin_apps_grid_once(&mut self) {
        self.pin_once(apps::APPS_GRID_ID, "apps-grid-pinned", 0);
    }

    /// Pin `id` at `slot` unless the `marker` says it was done before.
    fn pin_once(&mut self, id: &str, marker: &str, slot: usize) {
        let marker_path = crate::persist::data_path(marker);
        if marker_path.exists() {
            return;
        }
        if !self.pins.is_pinned(id) {
            self.pins.pin_at(id, slot);
        }
        crate::persist::write_text(marker, &marker_path, "1\n");
    }

    /// The Apps button was clicked: open the card on the apps grid, or close
    /// it if the apps are what's showing. Over the settings panel the apps
    /// come back in place.
    pub(crate) fn toggle_apps_grid(&mut self) {
        self.close_group();
        if self.ui.target() == Target::Open {
            if self.settings_panel {
                info!("apps: in place of the settings panel");
                self.settings_panel = false;
                self.settings_from_apps = false;
                self.schedule_frame();
            } else {
                info!("apps: closing the grid");
                self.handle_command(Command::Collapse);
            }
            return;
        }
        info!("apps: opening the grid");
        self.handle_command(Command::Toggle);
    }

    /// The gear was clicked: open the card as the panel, or close it if the
    /// panel is what's showing. With the card already open on the apps, the
    /// sections clear in place.
    pub(crate) fn toggle_settings_panel(&mut self) {
        self.close_group();
        self.panel.reset();
        if self.ui.target() == Target::Open {
            if self.settings_panel && self.settings_from_apps {
                // Entered from the apps: the gear goes back to them.
                info!("settings: back to the apps");
                self.settings_panel = false;
                self.settings_from_apps = false;
                self.schedule_frame();
            } else if self.settings_panel {
                info!("settings: closing the panel");
                self.handle_command(Command::Collapse);
            } else {
                info!("settings: panel in place of the apps");
                self.settings_panel = true;
                self.settings_from_apps = true;
                self.search.open = false;
                self.schedule_frame();
            }
            return;
        }
        info!("settings: opening the panel");
        self.settings_opening = true;
        self.handle_command(Command::Toggle);
        // Refused (e.g. the dock is suppressed): don't let a later open
        // come up as the panel.
        self.settings_opening = false;
    }

    /// The field the panel lives in, in surface coordinates: the whole card
    /// below the dock band at its settled open size, riding with the card
    /// as it rises and sinks.
    fn panel_field(&self, layout: &Layout) -> Rect {
        let settled = self.layout_at(self.ui.extent_of(Target::Open));
        let dock_h = self.config.window.input_bar_height as f32 * self.icon_scale();
        let ride = layout.sections[content::SECTION_APPS].title_pos.1
            - settled.sections[content::SECTION_APPS].title_pos.1;
        Rect::new(
            settled.card_x,
            settled.card_top + dock_h + ride,
            settled.card_w,
            (settled.card_h - dock_h).max(1.0),
        )
    }

    /// Advance the panel (on its own clock while `live`, frozen otherwise)
    /// and return what it draws this frame, and whether anything is moving
    /// beyond the idle orbits (then the caller keeps full-rate frames; else
    /// [`Self::panel_idle_tick`] paces it).
    pub(crate) fn panel_frame(&mut self, layout: &Layout, live: bool, paint: PanelPaint) -> (PanelDraw, bool) {
        let now = Instant::now();
        let dt = match (live, self.panel.last_step) {
            (true, Some(t)) => (now - t).as_secs_f32().min(0.1),
            _ => 0.0,
        };
        self.panel.last_step = live.then_some(now);
        let field = self.panel_field(layout);
        let scale = self.options_scale();
        let pill_h = self.options_pill_h();
        if let Some(r) = self.renderer.as_mut() {
            let mut measure = |t: &str, px: f32| r.measure_text(t, px, TEXT_FONT);
            self.panel.ensure((field.w, field.h), scale, pill_h, &mut measure);
        }
        self.panel.field = field;
        let pointer = if self.ui.target() == Target::Open { self.pointer_pos } else { None };
        self.panel.step(dt, pointer);
        (self.panel.draw(paint), self.panel.is_moving())
    }

    /// At rest, redraw the panel on the slow [`IDLE_TICK`] cadence (one
    /// timer at a time) instead of every vsync.
    pub(crate) fn panel_idle_tick(&mut self) {
        if self.panel_tick_armed {
            return;
        }
        let timer = calloop::timer::Timer::from_duration(IDLE_TICK);
        let armed = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.panel_tick_armed = false;
                app.schedule_frame();
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
        self.panel_tick_armed = armed;
    }

    /// Wheel over the open panel: move through its layers.
    pub(crate) fn panel_wheel(&mut self, value: f64) {
        self.panel.wheel(value);
        self.schedule_frame();
    }

    /// A click on the panel (below the dock band, inside the card).
    pub(crate) fn panel_click(&mut self, pos: (f32, f32)) {
        if self.panel.click(pos) {
            self.schedule_frame();
        }
    }

    /// Escape on the panel: fold an open setting back first. Returns whether
    /// it did.
    pub(crate) fn panel_escape(&mut self) -> bool {
        let closed = self.panel.escape();
        if closed {
            self.schedule_frame();
        }
        closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panel laid out at bar scale 1 over a typical card field, with
    /// estimated label widths (no text shaper in tests).
    fn panel() -> Panel {
        let mut p = Panel::default();
        let mut est = |t: &str, px: f32| crate::options::est_text_w(t, px);
        p.ensure((960.0, 560.0), 1.0, 25.0, &mut est);
        p
    }

    fn assert_clean(p: &Panel, what: &str) {
        let (w, h) = (p.key.0, p.key.1);
        for (i, a) in p.pills.iter().enumerate() {
            let (aw, ah) = a.size();
            assert!(
                a.home.0 - aw / 2.0 >= EDGE - 0.5
                    && a.home.0 + aw / 2.0 <= w - EDGE + 0.5
                    && a.home.1 - ah / 2.0 >= gaps(h, 1.0).0 - 0.5
                    && a.home.1 + ah / 2.0 <= h - gaps(h, 1.0).1 + 0.5,
                "{what}: {} leaves the field at {:?}",
                a.label,
                a.home
            );
            for b in p.pills.iter().skip(i + 1) {
                assert!(air(a, b) > 0.0, "{what}: {} and {} overlap", a.label, b.label);
            }
        }
    }

    #[test]
    fn the_field_is_wide_bricks_inside_its_gaps_without_overlaps() {
        let p = panel();
        assert_eq!(p.pills.len(), 20);
        let per_layer = [0, 1, 2].map(|l| p.pills.iter().filter(|q| q.layer == l).count());
        assert_eq!(per_layer, [9, 7, 4]);
        assert_clean(&p, "at rest");
        // Up under the dock: the top row sits in the first course below the
        // top gap.
        let (g_top, g_bottom) = gaps(p.key.1, 1.0);
        let course = (p.key.1 - g_top - g_bottom) / ROWS.len() as f32;
        let top = p.pills.iter().map(|q| q.home.1 - q.size().1 / 2.0).fold(f32::MAX, f32::min);
        assert!(top >= g_top - 0.5 && top <= g_top + course, "heap top at {top}");
        // Wide: the widest row reaches nearly to both sides.
        let left = p.pills.iter().map(|q| q.home.0 - q.size().0 / 2.0).fold(f32::MAX, f32::min);
        let right = p.pills.iter().map(|q| q.home.0 + q.size().0 / 2.0).fold(f32::MIN, f32::max);
        assert!(left < EDGE + 20.0 && right > p.key.0 - EDGE - 20.0, "heap spans {left}..{right}");
    }

    #[test]
    fn scrolling_cycles_the_layers_and_every_order_lays_out_clean() {
        let mut p = panel();
        let counts = |p: &Panel| [0, 1, 2].map(|l| p.pills.iter().filter(|q| q.layer == l).count());
        p.shift_layers(1);
        assert_eq!(counts(&p), [7, 4, 9], "down: the near group goes to the back");
        assert_clean(&p, "one step down");
        p.shift_layers(1);
        assert_eq!(counts(&p), [4, 9, 7]);
        assert_clean(&p, "two steps down");
        p.shift_layers(-1);
        p.shift_layers(-1);
        assert_eq!(counts(&p), [9, 7, 4], "up undoes it");
        // The near group left from the front, so it is leaving now.
        assert!(p.pills.iter().any(|q| q.exit.is_some()));
    }

    #[test]
    fn a_leaving_layer_arrives_at_the_other_end() {
        let mut p = panel();
        p.shift_layers(1);
        for _ in 0..60 {
            p.step(1.0 / 60.0, None);
        }
        assert!(p.pills.iter().all(|q| q.exit.is_none() && q.drawn == q.layer));
        assert!(p.pills.iter().all(|q| q.op > 0.95), "everyone back in view");
    }

    /// Not a check: dumps the laid-out field (label, layer, x, y, w, h) so a
    /// layout change can be looked at off-screen. Run with
    /// `cargo test dump_field -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_field() {
        let size: (f32, f32) = std::env::var("FIELD")
            .ok()
            .and_then(|v| v.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))))
            .unwrap_or((900.0, 560.0));
        let mut p = Panel::default();
        let mut est = |t: &str, px: f32| crate::options::est_text_w(t, px);
        p.ensure(size, 1.0, 25.0, &mut est);
        println!("FIELD {} {}", size.0, size.1);
        for q in &p.pills {
            let (w, h) = q.size();
            println!("PILL {}|{}|{:.1}|{:.1}|{:.1}|{:.1}", q.label, q.layer, q.home.0, q.home.1, w, h);
        }
    }

    #[test]
    fn at_rest_only_the_orbits_move_so_it_can_idle() {
        let mut p = panel();
        for _ in 0..120 {
            p.step(1.0 / 60.0, None);
        }
        assert!(!p.is_moving(), "settled: the idle cadence may take over");
        p.shift_layers(1);
        assert!(p.is_moving(), "a layer shift runs at full rate");
        for _ in 0..240 {
            p.step(1.0 / 60.0, None);
        }
        assert!(!p.is_moving(), "and settles back to rest");
    }

    #[test]
    fn a_click_opens_the_pill_and_escape_folds_it_back() {
        let mut p = panel();
        p.field = Rect::new(0.0, 0.0, 960.0, 560.0);
        p.step(0.0, None);
        let target = p.pills[0].pos;
        assert!(p.click(target), "a click on a pill opens it");
        assert_eq!(p.open.as_ref().map(|o| o.pill), Some(0));
        for _ in 0..30 {
            p.step(1.0 / 60.0, None);
        }
        assert!(p.open.as_ref().is_some_and(|o| o.k > 0.95));
        p.shift_layers(1);
        assert_eq!(p.shift, 0, "an open setting holds the layers still");
        assert!(p.escape());
        for _ in 0..60 {
            p.step(1.0 / 60.0, None);
        }
        assert!(p.open.is_none(), "folded back and gone");
    }
}

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
const SETTINGS: [(&str, u8, &str); 20] = [
    // (name, first layer, search keywords: what else people call it or
    // look for it by).
    ("Resolution", 0, "display screen monitor size pixels"),
    ("Scale", 0, "display screen zoom size hidpi text bigger smaller"),
    ("Wi-Fi", 0, "wifi wireless network internet connection"),
    ("Sound output", 0, "audio speakers speaker headphones output"),
    ("Dark mode", 0, "theme appearance light night colours colors"),
    ("Brightness", 0, "display screen backlight dim"),
    ("Bluetooth", 0, "wireless devices headphones pairing"),
    ("Volume", 0, "audio sound loud quiet mute"),
    ("Wallpaper", 0, "background desktop picture image appearance"),
    ("Night light", 1, "display screen warm blue light sunset eyes"),
    ("Accent colour", 1, "color theme appearance highlight"),
    ("Keyboard layout", 1, "input language typing keys"),
    ("Power mode", 1, "battery performance energy saver"),
    ("Notifications", 1, "alerts do not disturb dnd messages"),
    ("Touchpad speed", 1, "trackpad mouse pointer cursor gestures"),
    ("Language", 1, "region locale translation input"),
    ("Time zone", 2, "clock date time region"),
    ("Natural scrolling", 2, "touchpad trackpad mouse scroll direction"),
    ("Updates", 2, "upgrade system software version"),
    ("About Golem", 2, "system version info hardware computer"),
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
/// Soft glow per layer: (blur px, alpha). The far layer has none.
const LAYER_GLOW: [(f32, f32); 3] = [(7.0, 0.17), (3.5, 0.08), (0.0, 0.0)];
/// How "near" each layer is: sets the orbit radius.
const LAYER_Z: [f32; 3] = [1.0, 0.62, 0.35];

// ---- Layout (px at bar scale 1) -----------------------------------------
/// Air kept off the card's sides.
const MARGIN: f32 = 22.0;
/// Air under the dock, and above the card's floor as a share of the field:
/// the heap sits up under the dock with more room below.
const GAP_TOP: f32 = 32.0;
const GAP_BOTTOM_FRAC: f32 = 0.21;

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
/// Search: how fast pills rise or sink as the query changes, how much a
/// miss shrinks, and how far it dims.
const SEARCH_RATE: f32 = 14.0;
const SEARCH_SHRINK: f32 = 0.15;
const SEARCH_DIM: f32 = 0.85;
/// The open-box morph's rate.
const OPEN_RATE: f32 = 14.0;

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
    /// What else people call it or look for it by (search).
    keywords: &'static str,
    /// Search focus, easing: +1 a match (it rises to the front: the near
    /// layer's size and light), -1 a miss (it sinks back and dims), 0 no
    /// search.
    focus: f32,
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
}

impl Pill {
    fn size(&self) -> (f32, f32) {
        let l = self.layer as usize;
        (self.w[l], self.h[l])
    }

    /// The extra size its search focus gives it: a match eases up to the
    /// near layer's size, a miss shrinks a little.
    fn focus_scale(&self) -> f32 {
        let d = self.drawn as usize;
        if self.focus >= 0.0 {
            lerp(1.0, LAYER_SCALE[0] / LAYER_SCALE[d], self.focus)
        } else {
            1.0 - SEARCH_SHRINK * -self.focus
        }
    }

    /// Whether it matches a (lowercased) search: in its name or any of its
    /// keywords.
    fn matches(&self, query: &str) -> bool {
        self.label.to_lowercase().contains(query) || self.keywords.contains(query)
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
    /// The search, lowercased (empty: none).
    query: String,
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
        tracing::debug!(
            "settings: field laid out for {:.0}x{:.0} at scale {:.2} (fresh {fresh})",
            key.0, key.1, key.2
        );
        self.key = key;
        self.scale = scale;
        if fresh {
            let mut rng = Rng(0x85EB_CA6B);
            self.pills = SETTINGS
                .iter()
                .map(|&(label, group, keywords)| Pill {
                    label,
                    keywords,
                    focus: 0.0,
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
                })
                .collect();
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

    /// Lay the field out for the current layer order: balanced, fitted up
    /// under the dock, settled so nothing crowds. `snap` puts every pill
    /// there at once; otherwise they glide.
    fn compose(&mut self, size: (f32, f32), snap: bool) {
        let s = self.scale.max(0.01);
        let (w, h) = size;
        let margin = MARGIN * s;
        let (top, bottom) = (GAP_TOP * s, GAP_BOTTOM_FRAC * h);
        balanced(&mut self.pills, (w, h), margin, (top, bottom));
        for p in &mut self.pills {
            p.home = p.ideal;
        }
        clamp_all(&mut self.pills, (w, h), margin, (top, bottom));
        settle(&mut self.pills, (w, h), margin, (top, bottom), 18.0 * s, 0.0, 220);
        fit_vertical(&mut self.pills, h, top, bottom);
        settle(&mut self.pills, (w, h), margin, (top, bottom), 12.0 * s, 0.08, 120);
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
            let k = p.sc * p.focus_scale() * (1.0 + LIFT * p.lift);
            let (hw, hh) = (p.w[d] * k / 2.0, p.h[d] * k / 2.0);
            // Nearest layer first; a risen search match counts as near.
            let depth = if p.focus > 0.5 { 0 } else { p.drawn };
            if (fx - p.pos.0).abs() <= hw && (fy - p.pos.1).abs() <= hh && best.is_none_or(|(_, l)| depth < l) {
                best = Some((i, depth));
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
        self.open_pill(i);
        true
    }

    /// Grow pill `i` into its setting.
    fn open_pill(&mut self, i: usize) {
        let p = &self.pills[i];
        let d = p.drawn as usize;
        let k = p.sc * p.focus_scale() * (1.0 + LIFT * p.lift);
        let (w, h) = (p.w[d] * k, p.h[d] * k);
        info!("settings: opening {}", p.label);
        self.open = Some(OpenBox {
            pill: i,
            k: 0.0,
            want: 1.0,
            from: Rect::new(p.pos.0 - w / 2.0, p.pos.1 - h / 2.0, w, h),
        });
        self.hot = None;
    }

    /// Set the search (from the card's search pill): matches rise to the
    /// front, the rest sink back and dim.
    pub(crate) fn set_query(&mut self, query: &str) {
        self.query = query.trim().to_lowercase();
    }

    /// Where a pill's search focus is heading: see [`Pill::focus`].
    fn focus_target(&self, p: &Pill) -> f32 {
        if self.query.is_empty() {
            0.0
        } else if p.matches(&self.query) {
            1.0
        } else {
            -1.0
        }
    }

    /// Open the best match of the search: a name that starts with it, then
    /// a name that contains it, then a keyword match; nearest layer first
    /// among equals. Returns whether there was one.
    fn open_best(&mut self) -> bool {
        if self.query.is_empty() || self.open.is_some() {
            return false;
        }
        let q = &self.query;
        let rank = |p: &Pill| -> u8 {
            let name = p.label.to_lowercase();
            if name.starts_with(q.as_str()) {
                0
            } else if name.contains(q.as_str()) {
                1
            } else {
                2
            }
        };
        let best = (0..self.pills.len())
            .filter(|&i| self.pills[i].exit.is_none() && self.pills[i].matches(q))
            .min_by_key(|&i| (rank(&self.pills[i]), self.pills[i].layer));
        match best {
            Some(i) => {
                self.open_pill(i);
                true
            }
            None => false,
        }
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
        self.query.clear();
        for p in &mut self.pills {
            p.focus = 0.0;
        }
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
        let focus_to: Vec<f32> = self.pills.iter().map(|p| self.focus_target(p)).collect();
        for (i, p) in self.pills.iter_mut().enumerate() {
            let hot = self.hot == Some(i);
            p.focus = approach(p.focus, focus_to[i], SEARCH_RATE, dt);
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
        let wash = |a: f32| crate::options::wash(!paint.bright, a);
        let layer_wash = if paint.bright { LAYER_WASH_BRIGHT } else { LAYER_WASH_DARK };
        let hover_a = if paint.bright { HOVER_WASH_BRIGHT } else { HOVER_WASH_DARK };
        let glow_rgb = if paint.bright { [0.0, 0.0, 0.0] } else { [1.0, 1.0, 1.0] };
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
                // A search match rises over everything at rest.
                None if p.focus > 0.5 => 8,
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
            // Search: a match rises to the near layer's size and light, a
            // miss sinks back and dims.
            let rise = p.focus.max(0.0);
            let a = p.op * fade * (1.0 - SEARCH_DIM * (-p.focus).max(0.0));
            if a < 0.004 {
                continue;
            }
            let k = p.sc * grow * shrink * p.focus_scale() * (1.0 + LIFT * p.lift);
            let (w, h) = (p.w[d] * k, p.h[d] * k);
            let rect = Rect::new(f.x + p.pos.0 + dx - w / 2.0, f.y + p.pos.1 + dy - h / 2.0, w, h);
            let radius = h / 2.0;
            let (blur, glow_a) = (
                lerp(LAYER_GLOW[d].0, LAYER_GLOW[0].0, rise),
                lerp(LAYER_GLOW[d].1, LAYER_GLOW[0].1, rise),
            );
            if glow_a > 0.0 {
                out.glows.push(ShadowInst {
                    rect,
                    radius,
                    blur: blur * s,
                    color: [glow_rgb[0], glow_rgb[1], glow_rgb[2], glow_a * a],
                    edges: [1.0, 1.0, 1.0, 1.0],
                });
            }
            let wa = lerp(lerp(layer_wash[d], layer_wash[0], rise), hover_a, p.lift);
            let c = wash(wa);
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
                color: Some([paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * lerp(LAYER_INK[d], LAYER_INK[0], rise) * a]),
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

/// Keep a pill inside the field: `margin` off the sides, `gaps` (top,
/// bottom) off the dock and the floor.
fn clamp_pill(p: &mut Pill, size: (f32, f32), margin: f32, gaps: (f32, f32)) {
    let (w, h) = p.size();
    p.home.0 = p.home.0.clamp(margin + w / 2.0, (size.0 - margin - w / 2.0).max(margin + w / 2.0));
    p.home.1 = p.home.1.clamp(gaps.0 + h / 2.0, (size.1 - gaps.1 - h / 2.0).max(gaps.0 + h / 2.0));
}

fn clamp_all(pills: &mut [Pill], size: (f32, f32), margin: f32, gaps: (f32, f32)) {
    for p in pills {
        clamp_pill(p, size, margin, gaps);
    }
}

/// Signed air between two pills at their homes: positive = gap.
fn air(a: &Pill, b: &Pill) -> f32 {
    let (aw, ah) = a.size();
    let (bw, bh) = b.size();
    ((a.home.0 - b.home.0).abs() - (aw + bw) / 2.0).max((a.home.1 - b.home.1).abs() - (ah + bh) / 2.0)
}

/// Soft discs: start scattered, then each pill nudges only the neighbours
/// inside its reach (and the edges, the same way) until the spacing is even
/// everywhere. The short reach is what fills the middle; long-range
/// repulsion would press everything against the rim. Distances count x at
/// half, since pills are wide. Writes each pill's `ideal`.
fn balanced(pills: &mut [Pill], size: (f32, f32), margin: f32, gaps: (f32, f32)) {
    const SX: f32 = 0.55;
    let (w, h) = size;
    let n = pills.len().max(1) as f32;
    let area = ((w - 2.0 * margin) * SX * (h - gaps.0 - gaps.1)).max(1.0);
    let reach = 1.08 * (area / n).sqrt();
    let mut rng = Rng(0x2545_F491);
    for p in pills.iter_mut() {
        let (pw, ph) = p.size();
        p.home = (
            margin + pw / 2.0 + rng.next() * (w - 2.0 * margin - pw).max(0.0),
            gaps.0 + ph / 2.0 + rng.next() * (h - gaps.0 - gaps.1 - ph).max(0.0),
        );
    }
    let iters = 400;
    let mut next = vec![(0.0f32, 0.0f32); pills.len()];
    for it in 0..iters {
        let k = 0.35 * (1.0 - it as f32 / iters as f32) + 0.05;
        for (i, a) in pills.iter().enumerate() {
            let (mut fx, mut fy) = (0.0, 0.0);
            for (j, b) in pills.iter().enumerate() {
                if i == j {
                    continue;
                }
                let dx = (a.home.0 - b.home.0) * SX;
                let dy = a.home.1 - b.home.1;
                let d = dx.hypot(dy).max(0.01);
                if d < reach {
                    let f = (reach - d) / d;
                    fx += dx * f;
                    fy += dy * f;
                }
            }
            // The edges push like a neighbour at half the reach.
            let (aw, ah) = a.size();
            let edge = reach / 2.0;
            let l = (a.home.0 - aw / 2.0 - margin) * SX;
            let r = (w - margin - a.home.0 - aw / 2.0) * SX;
            let t = a.home.1 - ah / 2.0 - gaps.0;
            let bt = h - gaps.1 - a.home.1 - ah / 2.0;
            if l < edge {
                fx += edge - l;
            }
            if r < edge {
                fx -= edge - r;
            }
            if t < edge {
                fy += edge - t;
            }
            if bt < edge {
                fy -= edge - bt;
            }
            next[i] = (a.home.0 + fx / SX * k, a.home.1 + fy * k);
        }
        for (p, &n) in pills.iter_mut().zip(&next) {
            p.home = n;
            clamp_pill(p, size, margin, gaps);
        }
    }
    for p in pills {
        p.ideal = p.home;
    }
}

/// Settle: pills closer than `gap` push apart, each is pulled back toward
/// its `ideal` by `pull`, and all stay inside the field.
fn settle(pills: &mut [Pill], size: (f32, f32), margin: f32, gaps: (f32, f32), gap: f32, pull: f32, iters: usize) {
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
            clamp_pill(p, size, margin, gaps);
        }
    }
}

/// Fit the heap to the vertical gaps: its top edge `top` under the dock,
/// its bottom edge `bottom` above the floor.
fn fit_vertical(pills: &mut [Pill], h: f32, top: f32, bottom: f32) {
    let lo = pills.iter().map(|p| p.home.1 - p.size().1 / 2.0).fold(f32::MAX, f32::min);
    let hi = pills.iter().map(|p| p.home.1 + p.size().1 / 2.0).fold(f32::MIN, f32::max);
    let k = (h - top - bottom) / (hi - lo).max(1.0);
    for p in pills {
        let ph = p.size().1;
        p.home.1 = top + (p.home.1 - ph / 2.0 - lo) * k + ph / 2.0;
        p.ideal = p.home;
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
                self.search.query.clear();
                self.panel_search();
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
        // At rest: the card's AGUA spring wobbles its width on every frame
        // of an open or close, and laying the field out against that re-laid
        // it on every frame — the pills jumped while the panel revealed.
        let settled = self.layout_at_rest(self.ui.extent_of(Target::Open));
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

    /// Advance the panel by `dt` and return what it draws this frame.
    pub(crate) fn panel_frame(&mut self, layout: &Layout, dt: f32, paint: PanelPaint) -> PanelDraw {
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
        self.panel.draw(paint)
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

    /// The card's search query changed while the panel is up: the panel
    /// searches its settings.
    pub(crate) fn panel_search(&mut self) {
        let q = self.search.query.clone();
        self.panel.set_query(&q);
        self.schedule_frame();
    }

    /// Enter on the panel: open the best match of the search. Returns
    /// whether one opened.
    pub(crate) fn panel_open_best(&mut self) -> bool {
        let opened = self.panel.open_best();
        if opened {
            self.schedule_frame();
        }
        opened
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
                a.home.0 - aw / 2.0 >= MARGIN - 0.5
                    && a.home.0 + aw / 2.0 <= w - MARGIN + 0.5
                    && a.home.1 - ah / 2.0 >= GAP_TOP - 0.5
                    && a.home.1 + ah / 2.0 <= h - GAP_BOTTOM_FRAC * h + 0.5,
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
    fn the_field_is_balanced_inside_its_gaps_without_overlaps() {
        let p = panel();
        assert_eq!(p.pills.len(), 20);
        let per_layer = [0, 1, 2].map(|l| p.pills.iter().filter(|q| q.layer == l).count());
        assert_eq!(per_layer, [9, 7, 4]);
        assert_clean(&p, "at rest");
        // Up under the dock: the heap starts at the top gap.
        let top = p.pills.iter().map(|q| q.home.1 - q.size().1 / 2.0).fold(f32::MAX, f32::min);
        assert!((top - GAP_TOP).abs() < 3.0, "heap top at {top}");
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

    #[test]
    fn search_raises_the_matches_and_sinks_the_rest() {
        let mut p = panel();
        p.set_query("display");
        for _ in 0..60 {
            p.step(1.0 / 60.0, None);
        }
        for q in &p.pills {
            if q.matches("display") {
                assert!(q.focus > 0.95 && q.focus_scale() >= 1.0, "{} should rise", q.label);
            } else {
                assert!(q.focus < -0.95 && q.focus_scale() < 1.0, "{} should sink", q.label);
            }
        }
        let risen: Vec<&str> = p.pills.iter().filter(|q| q.focus > 0.5).map(|q| q.label).collect();
        assert!(risen.contains(&"Resolution") && risen.contains(&"Scale"), "display → {risen:?}");
        p.set_query("");
        for _ in 0..60 {
            p.step(1.0 / 60.0, None);
        }
        assert!(p.pills.iter().all(|q| q.focus.abs() < 0.01), "clearing settles every pill back");
    }

    #[test]
    fn enter_opens_the_best_name_match_before_a_keyword_one() {
        let mut p = panel();
        // "sc" matches Resolution by its keyword "screen", Scale by name.
        p.set_query("sc");
        assert!(p.open_best());
        assert_eq!(p.pills[p.open.as_ref().unwrap().pill].label, "Scale");
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

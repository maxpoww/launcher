//! THE MONITORS PAGE (Max, 2026-10-21; the lab
//! https://claude.ai/artifact/R8qrwi81LUvQnvVYuHjZJ1): the control panel's
//! page for the screens. A drawing of them, to arrange by dragging (their
//! sides stick to each other's, their ends line up) and to turn by a
//! corner; the screen picked in it named under it with its Resolution and
//! Scale; then, with more than one screen, how they work together —
//! Screens (Extend · Mirror · Isolated), where the Options bar and the Dock
//! are (Main screen · Every screen · Follows you) and what a closed lid
//! does.
//!
//! THE PAGE FIRST, THE FEATURES ONE BY ONE (the Launcher page's way, Max
//! 2026-10-20): every choice here is SAVED (`settings.json`'s `monitors`,
//! [`MonitorsChoice`]) and drawn. Applied so far: RESOLUTION AND SCALE, per
//! screen (Max, 2026-10-21: "now make resolution and scale work per
//! monitor") — the picked screen's, live, through the display settings'
//! own way ([`crate::display`]: a resolution is tried and goes back by
//! itself unless kept; a scale is made under a dissolve), the page showing
//! what the screen has; and THE ARRANGEMENT (Max, 2026-10-21: "now make
//! the arrange work, drag and spin the real monitors"): a drop or a turn
//! puts every screen where the drawing has it, turned as it has it, in one
//! go, and saves those places and turns as the screens' own rules
//! (`DisplayChoice::position`, `transform`), so they come back so.
//!
//! THE DRAWING IS IN MILLIMETRES (Max, 2026-10-21: "I'd like an
//! approximation to the real sizes on the preview"): each screen at the
//! size it says it is (`physicalWidth/Height`: the laptop 340×220, the TV
//! 1150×650 — not the pixels, which drew the TV twice the laptop where it
//! is three and a half times it), and the arrangement is kept so, each
//! screen's corner in mm and its turn in quarters. Applied, it becomes the
//! compositor's logical pixels screen by screen: the main one at 0,0, each
//! other against the one it touches, its offset along that side measured at
//! THAT screen's pixels per mm ([`logical_from_mm`]; the way back,
//! [`mm_from_logical`], draws an arrangement the page has not made). A
//! screen that does not say its size, or says it wrong (some TVs give their
//! aspect, 16×9), is drawn as if at 96 dots an inch.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::controls::{Choice, ControlView, Group, ScreenShape};
use crate::display::{DisplayChoice, GolemSettings, Monitor};
use crate::i18n::tr;
use crate::App;

/// Where one screen stands on the page: its corner (mm) and turn.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Placed {
    pub x: f64,
    pub y: f64,
    /// Quarter turns clockwise, in degrees (0, 90, 180, 270).
    #[serde(default)]
    pub rot: u16,
}

/// What the page has saved (not applied yet: see the module's note).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct MonitorsChoice {
    /// The screen picked in the drawing (`Monitor::selector`).
    pub picked: Option<String>,
    /// Each screen's place in the drawing (mm), by selector.
    pub places: BTreeMap<String, Placed>,
    /// Screens: `extend` (absent), `mirror`, `isolated`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screens: Option<String>,
    /// Where the Options bar is: `main` (absent), `every`, `follows`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<String>,
    /// Where the dock is: `follows` (absent), `main`, `every`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dock: Option<String>,
    /// A closed lid leaves the built-in ON (the switch off).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub lid_keeps_built_in: bool,
}

impl MonitorsChoice {
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A box (in mm in the drawing, in logical px on the screens).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Box2 {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Box2 {
    fn overlaps(&self, o: &Box2) -> bool {
        self.x < o.x + o.w && self.x + self.w > o.x && self.y < o.y + o.h && self.y + self.h > o.y
    }
}

/// Which side of the other screen a screen stands at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Right,
    Left,
    Below,
    Above,
}

/// How far apart Isolated screens are kept in the drawing (mm), and how
/// much of two sides must face each other when one is laid against the
/// other (mm).
pub(crate) const ISOLATED_GAP: f64 = 60.0;
const KEEP: f64 = 30.0;
/// The same two on the screens (logical px), where a placing would
/// otherwise cover one.
const ISOLATED_GAP_PX: f64 = 240.0;
const KEEP_PX: f64 = 120.0;
/// Two sides this near in the drawing (mm) touch: places saved rounded, or
/// sizes of half a millimetre, must not part two screens drawn together.
const TOUCH_MM: f64 = 1.0;

/// THE MAGNET: where a screen of `size` held at `raw` lands among `others`
/// — a side that comes within `reach` of one of theirs sticks to it (a
/// `gap` apart: 0 touching), and along that side its ends line up with
/// that screen's ends or middle when within `reach` of them. The side it
/// stuck to, and to which screen, when it did.
pub(crate) fn magnet(raw: Box2, others: &[Box2], reach: f64, gap: f64) -> (f64, f64, Option<(Side, usize)>) {
    let mut best: Option<(f64, Side, usize, f64)> = None;
    for (i, o) in others.iter().enumerate() {
        let facing_x = raw.y < o.y + o.h && raw.y + raw.h > o.y;
        let facing_y = raw.x < o.x + o.w && raw.x + raw.w > o.x;
        let sides = [
            (Side::Right, o.x + o.w + gap, (raw.x - (o.x + o.w + gap)).abs(), facing_x),
            (Side::Left, o.x - raw.w - gap, (raw.x + raw.w - (o.x - gap)).abs(), facing_x),
            (Side::Below, o.y + o.h + gap, (raw.y - (o.y + o.h + gap)).abs(), facing_y),
            (Side::Above, o.y - raw.h - gap, (raw.y + raw.h - (o.y - gap)).abs(), facing_y),
        ];
        for (side, at, dist, facing) in sides {
            if facing && dist < reach && best.is_none_or(|b| dist < b.0) {
                best = Some((dist, side, i, at));
            }
        }
    }
    let Some((_, side, i, at)) = best else {
        return (raw.x, raw.y, None);
    };
    let o = others[i];
    let (mut x, mut y) = (raw.x, raw.y);
    match side {
        Side::Right | Side::Left => {
            x = at;
            if let Some(&l) = [o.y, o.y + o.h - raw.h, o.y + (o.h - raw.h) / 2.0].iter().find(|&&l| (l - raw.y).abs() < reach) {
                y = l;
            }
        }
        Side::Below | Side::Above => {
            y = at;
            if let Some(&l) = [o.x, o.x + o.w - raw.w, o.x + (o.w - raw.w) / 2.0].iter().find(|&&l| (l - raw.x).abs() < reach) {
                x = l;
            }
        }
    }
    (x, y, Some((side, i)))
}

/// DROPPED: where a screen let go at `d` rests (in the drawing, mm).
/// Extend: touching — on the side it stuck to, or slid to the nearest side
/// of the nearest screen; Isolated: where it was, unless it covers another
/// (then just off it).
pub(crate) fn settle(d: Box2, others: &[Box2], isolated: bool) -> (f64, f64) {
    settle_in(d, others, isolated.then_some(ISOLATED_GAP), KEEP)
}

/// [`settle`] in any unit: a `gap` kept apart (Isolated) or none, and
/// `keep` of two sides facing.
fn settle_in(d: Box2, others: &[Box2], gap: Option<f64>, keep: f64) -> (f64, f64) {
    if others.is_empty() {
        return (d.x, d.y);
    }
    let isolated = gap.is_some();
    let gap = gap.unwrap_or(0.0);
    let free = |b: &Box2| !others.iter().any(|o| b.overlaps(o));
    if isolated && free(&d) {
        return (d.x, d.y);
    }
    // (Touching already, and over nothing: it stays.)
    if !isolated && free(&d) {
        let (x, y, side) = magnet(d, others, 1.0, 0.0);
        if side.is_some() {
            return (x, y);
        }
    }
    // Every side of every screen it could rest at, with at least a little
    // of the two sides facing; the nearest that covers nothing.
    let mut spots = Vec::new();
    for o in others {
        let cy = d.y.clamp(o.y - d.h + keep, o.y + o.h - keep);
        let cx = d.x.clamp(o.x - d.w + keep, o.x + o.w - keep);
        spots.push((o.x + o.w + gap, cy));
        spots.push((o.x - d.w - gap, cy));
        spots.push((cx, o.y + o.h + gap));
        spots.push((cx, o.y - d.h - gap));
    }
    spots.sort_by(|a, b| {
        let da = (a.0 - d.x).hypot(a.1 - d.y);
        let db = (b.0 - d.x).hypot(b.1 - d.y);
        da.total_cmp(&db)
    });
    let at = spots
        .iter()
        .copied()
        .find(|&(x, y)| free(&Box2 { x, y, ..d }))
        .unwrap_or(spots[0]);
    // (Lined up with what it rests against, when near. Not rounded: a
    // screen of 190.5 mm rounded off its neighbour's side stood half a
    // millimetre away — a gap of pixels on the screens.)
    let (x, y, _) = magnet(Box2 { x: at.0, y: at.1, ..d }, others, keep, gap);
    (x, y)
}

/// The side of `b` that `a` stands at, touching it (within `eps`) with
/// some of the two sides facing.
fn touching(a: Box2, b: Box2, eps: f64) -> Option<Side> {
    let facing_x = a.y < b.y + b.h - eps && a.y + a.h > b.y + eps;
    let facing_y = a.x < b.x + b.w - eps && a.x + a.w > b.x + eps;
    if facing_x && (a.x - (b.x + b.w)).abs() < eps {
        Some(Side::Right)
    } else if facing_x && (a.x + a.w - b.x).abs() < eps {
        Some(Side::Left)
    } else if facing_y && (a.y - (b.y + b.h)).abs() < eps {
        Some(Side::Below)
    } else if facing_y && (a.y + a.h - b.y).abs() < eps {
        Some(Side::Above)
    } else {
        None
    }
}

/// ALONG A SHARED SIDE, from the drawing (mm) to the screens (logical px):
/// where screen `i` starts along it, given screen `j`'s start and length in
/// both and `i`'s in both. Lined up in the drawing — starts, ends or
/// middles — they are lined up on the screens; else the middle of the
/// side they share is the same point on both. A side's offset can NOT be
/// carried at one screen's pixels per mm: the TV at 200 % has a third of
/// the laptop's, and taken at the laptop's it slid off the laptop's side
/// altogether — nowhere left to cross (Max, 2026-10-21).
fn along_to_logical(i: (f64, f64), i_px: f64, j: (f64, f64), j_at: f64, j_px: f64) -> f64 {
    let ((i0, il), (j0, jl)) = (i, j);
    if (i0 - j0).abs() < 0.5 {
        j_at
    } else if (i0 + il - (j0 + jl)).abs() < 0.5 {
        j_at + j_px - i_px
    } else if (i0 + il / 2.0 - (j0 + jl / 2.0)).abs() < 0.5 {
        j_at + (j_px - i_px) / 2.0
    } else {
        let c = (i0.max(j0) + (i0 + il).min(j0 + jl)) / 2.0;
        let on_j = j_at + (c - j0) * j_px / jl.max(1e-6);
        on_j - (c - i0) * i_px / il.max(1e-6)
    }
}

/// The way back of [`along_to_logical`]: where `i` starts along the shared
/// side in the drawing, from the screens.
fn along_to_mm(i: (f64, f64), i_mm: f64, j: (f64, f64), j_at: f64, j_mm: f64) -> f64 {
    let ((i0, il), (j0, jl)) = (i, j);
    if (i0 - j0).abs() < 1.0 {
        j_at
    } else if (i0 + il - (j0 + jl)).abs() < 1.0 {
        j_at + j_mm - i_mm
    } else if (i0 + il / 2.0 - (j0 + jl / 2.0)).abs() < 1.0 {
        j_at + (j_mm - i_mm) / 2.0
    } else {
        let c = (i0.max(j0) + (i0 + il).min(j0 + jl)) / 2.0;
        let on_j = j_at + (c - j0) * j_mm / jl.max(1e-6);
        on_j - (c - i0) * i_mm / il.max(1e-6)
    }
}

/// Place every screen from the main one outward: `place(i, j, side)` puts
/// screen `i` against placed screen `j` it touches (`touch(i, j)` says on
/// which side), `alone(i)` one touching none, from the main one. The main
/// one is placed first by the caller.
fn outward(n: usize, main: usize, touch: impl Fn(usize, usize) -> Option<Side>, mut place: impl FnMut(usize, usize, Side), mut alone: impl FnMut(usize)) {
    let mut done = vec![false; n];
    if main < n {
        done[main] = true;
    }
    loop {
        let mut more = false;
        for i in 0..n {
            if done[i] {
                continue;
            }
            if let Some((j, side)) = (0..n).filter(|&j| done[j]).find_map(|j| touch(i, j).map(|side| (j, side))) {
                place(i, j, side);
                done[i] = true;
                more = true;
            }
        }
        if !more {
            break;
        }
    }
    for (i, d) in done.iter().enumerate() {
        if !d {
            alone(i);
        }
    }
}

/// THE DRAWING ON THE SCREENS: each screen's logical corner from its place
/// in mm — the main one at 0,0; each other against the screen it touches,
/// its offset along that side at that screen's pixels per mm; one touching
/// none from the main one, at its. A placing that would cover another is
/// moved just off it.
pub(crate) fn logical_from_mm(screens: &[PageScreen], main: usize, isolated: bool) -> Vec<(f64, f64)> {
    let n = screens.len();
    let drawn: Vec<Box2> = screens.iter().map(PageScreen::drawn).collect();
    let size: Vec<Box2> = screens.iter().map(PageScreen::logical).collect();
    let per_mm = |j: usize| (size[j].w / drawn[j].w.max(1e-6), size[j].h / drawn[j].h.max(1e-6));
    let mut at = vec![(0.0, 0.0); n];
    let at_cell = std::cell::RefCell::new(&mut at);
    outward(
        n,
        main,
        |i, j| touching(drawn[i], drawn[j], TOUCH_MM),
        |i, j, side| {
            let mut at = at_cell.borrow_mut();
            let (jx, jy) = at[j];
            let (di, dj) = (drawn[i], drawn[j]);
            let down = || along_to_logical((di.y, di.h), size[i].h, (dj.y, dj.h), jy, size[j].h);
            let across = || along_to_logical((di.x, di.w), size[i].w, (dj.x, dj.w), jx, size[j].w);
            // (Whole pixels as each is placed: a screen placed from one
            // rounded on its own stood a pixel off it.)
            let (x, y) = match side {
                Side::Right => (jx + size[j].w, down()),
                Side::Left => (jx - size[i].w, down()),
                Side::Below => (across(), jy + size[j].h),
                Side::Above => (across(), jy - size[i].h),
            };
            at[i] = (x.round(), y.round());
        },
        |i| {
            let mut at = at_cell.borrow_mut();
            let k = per_mm(main);
            at[i] = ((drawn[i].x - drawn[main].x) * k.0, (drawn[i].y - drawn[main].y) * k.1);
        },
    );
    // (Never one over another: pixels per mm that differ can bring two
    // together that the drawing has apart. And, side by side, never one
    // apart from all the others: the pointer could not get to it.)
    for i in 0..n {
        if i == main {
            continue;
        }
        let b = Box2 { x: at[i].0, y: at[i].1, ..size[i] };
        let others: Vec<Box2> = (0..n).filter(|&j| j != i).map(|j| Box2 { x: at[j].0, y: at[j].1, ..size[j] }).collect();
        let over = others.iter().any(|o| b.overlaps(o));
        let alone = !isolated && !others.iter().any(|&o| touching(b, o, 1.0).is_some());
        if over || alone {
            at[i] = settle_in(b, &others, isolated.then_some(ISOLATED_GAP_PX), KEEP_PX);
        }
    }
    at.iter().map(|&(x, y)| (x.round(), y.round())).collect()
}

/// THE SCREENS IN THE DRAWING: each screen's place in mm from its logical
/// corner — the way back of [`logical_from_mm`], for an arrangement the
/// page has not made (the compositor's own, a screen plugged in since).
pub(crate) fn mm_from_logical(screens: &mut [PageScreen], main: usize) {
    let n = screens.len();
    let size: Vec<Box2> = screens.iter().map(PageScreen::logical).collect();
    let drawn: Vec<Box2> = screens.iter().map(PageScreen::drawn).collect();
    let per_mm = |j: usize| (size[j].w / drawn[j].w.max(1e-6), size[j].h / drawn[j].h.max(1e-6));
    let mut at = vec![(0.0, 0.0); n];
    let at_cell = std::cell::RefCell::new(&mut at);
    outward(
        n,
        main,
        |i, j| touching(size[i], size[j], 2.0),
        |i, j, side| {
            let mut at = at_cell.borrow_mut();
            let (jx, jy) = at[j];
            let (li, lj) = (size[i], size[j]);
            let down = || along_to_mm((li.y, li.h), drawn[i].h, (lj.y, lj.h), jy, drawn[j].h);
            let across = || along_to_mm((li.x, li.w), drawn[i].w, (lj.x, lj.w), jx, drawn[j].w);
            at[i] = match side {
                Side::Right => (jx + drawn[j].w, down()),
                Side::Left => (jx - drawn[i].w, down()),
                Side::Below => (across(), jy + drawn[j].h),
                Side::Above => (across(), jy - drawn[i].h),
            };
        },
        |i| {
            let mut at = at_cell.borrow_mut();
            let k = per_mm(main);
            at[i] = ((size[i].x - size[main].x) / k.0, (size[i].y - size[main].y) / k.1);
        },
    );
    for (s, a) in screens.iter_mut().zip(at) {
        s.at = (a.0.round(), a.1.round());
    }
}

/// A screen's real size in mm as made (not turned): what it says, else —
/// nothing said, or a size that cannot be (some TVs give their aspect,
/// 16×9; a density no screen has) — its logical pixels at 96 dots an inch.
fn real_mm(m: &Monitor, logical: (f64, f64)) -> (f64, f64) {
    let (w, h) = (f64::from(m.mm.0), f64::from(m.mm.1));
    let (pw, ph) = (f64::from(m.mode.w), f64::from(m.mode.h));
    let sane = w > 0.0
        && h > 0.0
        && (1.0..=30.0).contains(&(pw / w))
        && (1.0..=30.0).contains(&(ph / h))
        && ((w / h) / (pw / ph) - 1.0).abs() < 0.2;
    if sane {
        (w, h)
    } else {
        (logical.0 * 25.4 / 96.0, logical.1 * 25.4 / 96.0)
    }
}

/// A quarter turn (`by` 90 clockwise, -90 back) of a screen about its
/// middle: its new box.
pub(crate) fn turned(b: Box2, rot: u16, by: i32) -> (Box2, u16) {
    let rot = (i32::from(rot) + by).rem_euclid(360) as u16;
    let (w, h) = (b.h, b.w);
    (Box2 { x: b.x + (b.w - w) / 2.0, y: b.y + (b.h - h) / 2.0, w, h }, rot)
}

/// One screen as the page has it: the compositor's, with the page's
/// choices over it.
#[derive(Debug, Clone)]
pub(crate) struct PageScreen {
    pub sel: String,
    pub title: String,
    pub short: String,
    pub builtin: bool,
    /// Its resolution and scale, as the compositor has them.
    pub mode: (u32, u32),
    pub scale: f64,
    pub rot: u16,
    /// Its corner on the screens (logical px) and in the drawing (mm).
    pub lpos: (f64, f64),
    pub at: (f64, f64),
    /// Its real size in mm, as made (not turned).
    pub mm: (f64, f64),
    pub sizes: Vec<(u32, u32)>,
    pub stops: Vec<f64>,
    /// The one in use (the compositor's focus).
    pub focused: bool,
}

impl PageScreen {
    /// Its box on the screens (logical px), turned.
    pub(crate) fn logical(&self) -> Box2 {
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (w, h) = (f64::from(self.mode.0) / s, f64::from(self.mode.1) / s);
        let (w, h) = if self.rot.is_multiple_of(180) { (w, h) } else { (h, w) };
        Box2 { x: self.lpos.0, y: self.lpos.1, w, h }
    }

    /// Its box in the drawing (mm), turned.
    pub(crate) fn drawn(&self) -> Box2 {
        let (w, h) = if self.rot.is_multiple_of(180) { self.mm } else { (self.mm.1, self.mm.0) };
        Box2 { x: self.at.0, y: self.at.1, w, h }
    }
}

/// The screens as the page shows them: placed as the page saved them when
/// it has every one, else drawn from where the compositor has them.
pub(crate) fn page_screens(screens: &[Monitor], choice: &MonitorsChoice) -> Vec<PageScreen> {
    let mut out: Vec<PageScreen> = screens
        .iter()
        .map(|m| {
            let sel = m.selector();
            let builtin = crate::launcher_prefs::built_in(m);
            // (What the screen has: these two are applied, not only saved.)
            let mode = (m.mode.w, m.mode.h);
            let scale = m.scale;
            let placed = choice.places.get(&sel).copied();
            let s = if scale > 0.0 { scale } else { 1.0 };
            let mm = real_mm(m, (f64::from(mode.0) / s, f64::from(mode.1) / s));
            // The sizes: the one in use and the biggest few others.
            let mut sizes: Vec<(u32, u32)> = Vec::new();
            for md in &m.modes {
                if !sizes.contains(&(md.w, md.h)) {
                    sizes.push((md.w, md.h));
                }
            }
            sizes.sort_by(|a, b| (u64::from(b.0) * u64::from(b.1)).cmp(&(u64::from(a.0) * u64::from(a.1))));
            sizes.truncate(4);
            if !sizes.contains(&mode) {
                sizes.pop();
                sizes.push(mode);
            }
            // The scales: 100 %, the one in use, and the nearest to it.
            let all = crate::display::scale_stops(mode.0, mode.1, scale);
            let mut stops: Vec<f64> = all.clone();
            stops.sort_by(|a, b| (a - scale).abs().total_cmp(&(b - scale).abs()));
            stops.truncate(5);
            if !stops.iter().any(|s| (s - 1.0).abs() < 1e-6) && all.iter().any(|s| (s - 1.0).abs() < 1e-6) {
                stops.pop();
                stops.push(1.0);
            }
            stops.sort_by(f64::total_cmp);
            PageScreen {
                short: if builtin { tr("Built-in").to_owned() } else { m.title() },
                title: m.title(),
                sel,
                builtin,
                mode,
                scale,
                // (Turned as the page put it, else as the compositor has it.)
                rot: placed.map_or(u16::from(m.transform % 4) * 90, |p| p.rot),
                lpos: m.pos,
                at: placed.map_or((0.0, 0.0), |p| (p.x, p.y)),
                mm,
                sizes,
                stops,
                focused: m.focused,
            }
        })
        .collect();
    if out.iter().any(|s| !choice.places.contains_key(&s.sel)) {
        let main = crate::launcher_prefs::main_screen(screens).unwrap_or(0);
        mm_from_logical(&mut out, main);
    }
    out
}

/// Screens whose places were saved but whose sizes have changed since (a
/// new resolution or scale) are laid against the others again, so the
/// drawing never shows two overlapping, or (Extend) one standing apart:
/// the first screen stays, each other is settled in turn.
fn keep_together(screens: &mut [PageScreen], isolated: bool) {
    for i in 1..screens.len() {
        let others: Vec<Box2> = screens.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, o)| o.drawn()).collect();
        let b = screens[i].drawn();
        let touching = magnet(b, &others, 1.0, if isolated { ISOLATED_GAP } else { 0.0 }).2.is_some();
        let overlaps = others.iter().any(|o| b.overlaps(o));
        if overlaps || (!isolated && !touching) {
            screens[i].at = settle(b, &others, isolated);
        }
    }
}

/// Every screen's place as the drawing has it now, kept: once the page has
/// arranged one, it has them all.
/// From the MAIN screen's corner (`main`), so places kept in another room
/// — the main screen drawn elsewhere there — still mean the same.
fn keep_places(c: &mut MonitorsChoice, screens: &[PageScreen], main: usize) {
    let origin = screens.get(main).map_or((0.0, 0.0), |m| m.at);
    for s in screens {
        c.places.insert(s.sel.clone(), Placed { x: s.at.0 - origin.0, y: s.at.1 - origin.1, rot: s.rot });
    }
}

/// The screens as the page draws them: where they were put, and any no
/// longer touching (a size changed, one unplugged) put against the others.
fn kept_together(monitors: &[crate::display::Monitor], c: &MonitorsChoice) -> Vec<PageScreen> {
    let mut screens = page_screens(monitors, c);
    if !c.places.is_empty() && c.screens.as_deref() != Some("mirror") {
        keep_together(&mut screens, c.screens.as_deref() == Some("isolated"));
    }
    screens
}

/// The screen picked in the drawing: the saved one (here, or on the Scale
/// and Resolution pages), else the one in use, as those pages set it.
fn picked<'a>(screens: &'a [PageScreen], choice: &MonitorsChoice) -> Option<&'a PageScreen> {
    choice
        .picked
        .as_ref()
        .and_then(|p| screens.iter().find(|s| &s.sel == p))
        .or_else(|| screens.iter().find(|s| s.focused))
        .or_else(|| screens.iter().find(|s| s.builtin))
        .or(screens.first())
}

/// A TURN BEING TRIED (Max, 2026-10-21: which way the compositor's numbers
/// turn a screen is only seen on the screen itself — and a picture turned
/// the wrong way turns the pointer with it): on the screens and saved, the
/// arrangement from before kept beside (`turn-trial.json`, so a session
/// ended meanwhile comes back to it); kept, or it goes back by itself after
/// `KEEP_SECS`, as a resolution does.
pub(crate) struct TurnTrial {
    before: Before,
    deadline: std::time::Instant,
}

/// The screens' choices as they were before a turn.
#[derive(Clone, Serialize, Deserialize)]
struct Before {
    displays: BTreeMap<String, DisplayChoice>,
    monitors: MonitorsChoice,
}

fn turn_trial_path() -> std::path::PathBuf {
    crate::display::golem_path("turn-trial.json")
}

/// The words of a three-way row, and which value each sends.
const SCREENS: [(&str, &str); 3] = [("Extend", "extend"), ("Mirror", "mirror"), ("Isolated", "isolated")];
const WHERE: [(&str, &str); 3] = [("Main screen", "main"), ("Every screen", "every"), ("Follows you", "follows")];

fn words(caption: &'static str, key: &'static str, list: &[(&'static str, &'static str)], lit: &str) -> Group {
    let choices = list.iter().map(|&(word, value)| choice(tr(word), value == lit, value)).collect();
    Group { labelled: true, ..crate::controls::group_of(Some(caption), key, choices) }
}

fn choice(text: impl Into<String>, on: bool, value: impl Into<String>) -> Choice {
    Choice { text: text.into(), on, value: value.into(), swatch: Vec::new(), caption: false, icon: None, hint: None }
}

/// The page (`controls::read("Monitors")`, on the controls' worker).
pub(crate) fn view(view: &mut ControlView) {
    let saved = GolemSettings::load().monitors;
    let monitors = crate::display::monitors();
    let screens = kept_together(&monitors, &saved);
    let main_sel = crate::launcher_prefs::main_screen(&monitors).and_then(|i| screens.get(i)).map(|s| s.sel.clone());
    view.subtitle = tr("How your screens work together.").to_owned();
    let Some(p) = picked(&screens, &saved) else {
        view.subtitle = tr("No screen to set.").to_owned();
        return;
    };
    let many = screens.len() > 1;
    let options = saved.options.as_deref().unwrap_or("main");
    let dock = saved.dock.as_deref().unwrap_or("follows");
    let mirrored = many && saved.screens.as_deref() == Some("mirror");
    // THE DRAWING: each screen a box (clicked: picked), the picked one's two
    // turning corners after them.
    let shapes: Vec<ScreenShape> = screens
        .iter()
        .map(|s| {
            let b = s.drawn();
            ScreenShape {
                sel: s.sel.clone(),
                short: s.short.clone(),
                x: b.x,
                y: b.y,
                w: b.w,
                h: b.h,
                rot: s.rot,
                bar: !mirrored && (s.builtin || options == "every" || (!screens.iter().any(|x| x.builtin) && Some(&s.sel) == main_sel.as_ref())),
                dock: !mirrored && (s.builtin || dock == "every" || (!screens.iter().any(|x| x.builtin) && Some(&s.sel) == main_sel.as_ref())),
                picked: s.sel == p.sel,
                mirrored,
                isolated: saved.screens.as_deref() == Some("isolated"),
            }
        })
        .collect();
    let mut picks: Vec<Choice> = screens.iter().map(|s| choice(s.short.clone(), s.sel == p.sel, format!("pick:{}", s.sel))).collect();
    if !mirrored {
        picks.push(choice(tr("Turn back"), false, format!("ccw:{}", p.sel)));
        picks.push(choice(tr("Turn"), false, format!("cw:{}", p.sel)));
    }
    view.groups.push(Group { screens: shapes, ..crate::controls::group_of(None, "monitors-arrange", picks) });
    // ITS NAME under the drawing, and what it is set to.
    view.groups.push(Group { centred: true, ..crate::controls::line_of(p.title.clone()) });
    let sizes = p
        .sizes
        .iter()
        .map(|&(w, h)| choice(format!("{w} × {h}"), (w, h) == p.mode, format!("{w}x{h}")))
        .collect();
    view.groups.push(Group { labelled: true, ..crate::controls::group_of(Some("Resolution"), "monitors-mode", sizes) });
    let stops = p
        .stops
        .iter()
        .map(|&st| choice(format!("{}%", (st * 100.0).round() as u32), (st - p.scale).abs() < 1e-4, format!("{st}")))
        .collect();
    view.groups.push(Group { labelled: true, ..crate::controls::group_of(Some("Scale"), "monitors-scale", stops) });
    if !many {
        return;
    }
    view.groups.push(words("Screens", "monitors-screens", &SCREENS, saved.screens.as_deref().unwrap_or("extend")));
    if !mirrored {
        view.groups.push(words("Options", "monitors-options", &WHERE, options));
        view.groups.push(words("Dock", "monitors-dock", &WHERE, dock));
    }
    if screens.iter().any(|s| s.builtin) {
        let mut lid = crate::controls::on_off_of(Some("Lid closed"), "monitors-lid", !saved.lid_keeps_built_in);
        if let Some(on) = lid.choices.first_mut() {
            on.hint = Some(tr("The built-in turns off and everything goes to the other screen.").to_owned());
        }
        view.groups.push(lid);
    }
}

impl App {
    /// A pick on the Monitors page (`monitors-*` keys): saved, and the page
    /// read again. Nothing is applied to the screens yet.
    pub(crate) fn monitors_pick(&mut self, key: &str, value: &str) {
        let monitors = crate::display::monitors();
        let mut arranged = false;
        // RESOLUTION AND SCALE are the picked screen's own, applied live
        // (the display settings' way: tried and kept, or under a dissolve).
        if key == "monitors-mode" || key == "monitors-scale" {
            let saved = GolemSettings::load().monitors;
            let screens = page_screens(&monitors, &saved);
            let Some(sel) = picked(&screens, &saved).map(|p| p.sel.clone()) else { return };
            info!("monitors: {key} = {value} on {sel}");
            if key == "monitors-mode" {
                let size = value.split_once('x').and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)));
                if let Some((w, h)) = size {
                    self.set_display_mode_on(Some(&sel), w, h, None);
                }
            } else if let Ok(s) = value.parse::<f64>() {
                self.set_display_scale_on(Some(&sel), s);
            }
            return;
        }
        let main = crate::launcher_prefs::main_screen(&monitors).unwrap_or(0);
        // A TURN is tried (`TurnTrial`): what was there before is kept
        // aside — before a first turn, not a second one on top.
        let turning = key == "monitors-arrange" && (value.starts_with("cw:") || value.starts_with("ccw:"));
        let before = turning.then(|| match self.monitors_turn.take() {
            Some(t) => t.before,
            None => {
                let g = GolemSettings::load();
                Before { displays: g.displays, monitors: g.monitors }
            }
        });
        if turning {
            // (One thing tried at a time: a resolution being tried goes back.)
            if self.display_trial_screen().is_some() {
                self.revert_display();
            }
        }
        // (Another screen picked: a resolution tried on one goes back.)
        if let Some(sel) = value.strip_prefix("pick:").filter(|_| key == "monitors-arrange") {
            self.display_trial_leaves(sel);
        }
        GolemSettings::change(|g| {
            let c = &mut g.monitors;
            let mut screens = kept_together(&monitors, c);
            let isolated = c.screens.as_deref() == Some("isolated");
            match key {
                "monitors-arrange" => {
                    let (verb, sel) = value.split_once(':').unwrap_or((value, ""));
                    match verb {
                        "pick" => c.picked = Some(sel.to_owned()),
                        "cw" | "ccw" => {
                            if let Some(i) = screens.iter().position(|s| s.sel == sel) {
                                let (b, rot) = turned(screens[i].drawn(), screens[i].rot, if verb == "cw" { 90 } else { -90 });
                                let others: Vec<Box2> = screens.iter().filter(|o| o.sel != sel).map(PageScreen::drawn).collect();
                                let (x, y) = settle(b, &others, isolated);
                                screens[i].at = (x, y);
                                screens[i].rot = rot;
                                keep_places(c, &screens, main);
                                c.picked = Some(sel.to_owned());
                                arranged = true;
                            }
                        }
                        _ => {}
                    }
                }
                "monitors-screens" => c.screens = (value != "extend").then(|| value.to_owned()),
                "monitors-options" => c.options = (value != "main").then(|| value.to_owned()),
                "monitors-dock" => c.dock = (value != "follows").then(|| value.to_owned()),
                "monitors-lid" => c.lid_keeps_built_in = value == "off",
                _ => return,
            }
            g.store();
        });
        info!("monitors: {key} = {value}");
        if arranged {
            self.monitors_arrange();
        }
        if let (true, Some(before)) = (arranged && turning, before) {
            self.monitors_turn_start(before);
        }
    }

    /// A turn is on the screens: kept, or back by itself.
    fn monitors_turn_start(&mut self, before: Before) {
        use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
        crate::persist::write_json("turn trial", &turn_trial_path(), &before);
        let span = std::time::Duration::from_secs(crate::display::KEEP_SECS);
        let deadline = std::time::Instant::now() + span;
        self.monitors_turn = Some(TurnTrial { before, deadline });
        info!("monitors: a turn tried, {} s to keep it", crate::display::KEEP_SECS);
        let armed = self.loop_handle.insert_source(Timer::from_duration(span), move |_, _, app: &mut App| {
            if app.monitors_turn.as_ref().is_some_and(|t| t.deadline == deadline) {
                info!("monitors: the turn not kept in time");
                app.monitors_turn_back();
            }
            TimeoutAction::Drop
        });
        if let Err(e) = armed {
            tracing::warn!("monitors: cannot arm the turn's keep timer ({e}), going back now");
            self.monitors_turn_back();
            return;
        }
        // (The seconds left counted down where it is asked.)
        let tick = self.loop_handle.insert_source(Timer::from_duration(std::time::Duration::from_secs(1)), move |_, _, app: &mut App| {
            if app.monitors_turn.as_ref().is_none_or(|t| t.deadline != deadline) {
                return TimeoutAction::Drop;
            }
            app.dirty = true;
            app.schedule_frame();
            TimeoutAction::ToDuration(std::time::Duration::from_secs(1))
        });
        if let Err(e) = tick {
            tracing::warn!("monitors: cannot count the turn's seconds down ({e})");
        }
    }

    /// Seconds the turn being tried still has.
    pub(crate) fn monitors_turn_left(&self) -> Option<f32> {
        self.monitors_turn
            .as_ref()
            .map(|t| t.deadline.saturating_duration_since(std::time::Instant::now()).as_secs_f32())
    }

    /// Keep the turn being tried (false: none).
    pub(crate) fn monitors_turn_keep(&mut self) -> bool {
        if self.monitors_turn.take().is_none() {
            return false;
        }
        let _ = std::fs::remove_file(turn_trial_path());
        info!("monitors: the turn kept");
        self.monitors_read_soon(std::time::Duration::from_millis(1));
        self.schedule_frame();
        true
    }

    /// The turn being tried goes back: the choices from before, on the
    /// screens again (false: none).
    pub(crate) fn monitors_turn_back(&mut self) -> bool {
        let Some(t) = self.monitors_turn.take() else {
            return false;
        };
        info!("monitors: the turn goes back");
        self.monitors_put_back(t.before);
        true
    }

    fn monitors_put_back(&mut self, before: Before) {
        GolemSettings::change(|g| {
            g.displays = before.displays;
            g.monitors = before.monitors;
            g.store();
        });
        let _ = std::fs::remove_file(turn_trial_path());
        self.monitors_arrange();
        self.schedule_frame();
    }

    /// At start: a turn tried and never kept (the session ended during it)
    /// goes back.
    pub(crate) fn monitors_turn_left_over(&mut self) {
        let Some(before) = crate::persist::read_json::<Before>(&turn_trial_path()) else {
            return;
        };
        info!("monitors: a turn tried and never kept goes back");
        self.monitors_put_back(before);
    }

    /// THE ARRANGEMENT, ON THE SCREENS: every screen put where the page has
    /// it — the whole moved so the main screen is at 0,0 — and turned as it has it,
    /// in one evaluation, then kept as the screens' own rules (their mode
    /// and scale as they are) and the page's places made those.
    pub(crate) fn monitors_arrange(&mut self) {
        self.monitors_place(false);
    }

    /// A screen changed size (a scale, a resolution, a turn): the places it
    /// had were for its old size — a gap or an overlap now, and the pointer
    /// no longer crosses. Every screen is put back where the page has it,
    /// if one is not there.
    pub(crate) fn monitors_keep_arranged(&mut self) {
        self.monitors_place(true);
    }

    /// Two connected screens that say they are the same: each is kept by
    /// its connector from now on, its choices so far handed to each.
    pub(crate) fn monitors_part_twins(&mut self) {
        let monitors = crate::display::monitors();
        let mut dock = Vec::new();
        crate::display::part_twins(&monitors, |key, names| dock.push((key.to_owned(), names.to_vec())));
        let mut changed = false;
        for (key, names) in dock {
            if let Some(size) = self.settings.dock_sizes.remove(&key) {
                for name in &names {
                    self.settings.dock_sizes.entry(name.clone()).or_insert(size);
                }
                changed = true;
            }
            if let Some(i) = self.settings.launcher_screens_off.iter().position(|s| *s == key) {
                self.settings.launcher_screens_off.remove(i);
                for name in names {
                    if !self.settings.launcher_screens_off.contains(&name) {
                        self.settings.launcher_screens_off.push(name);
                    }
                }
                changed = true;
            }
        }
        if changed {
            self.settings.save();
        }
    }

    /// A screen plugged in or out: once the compositor has put its own
    /// rules on them, the screens as the page has them (one gone from the
    /// middle of a row left the others apart).
    pub(crate) fn monitors_plugged(&mut self) {
        use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
        self.monitors_part_twins();
        let armed = self.loop_handle.insert_source(Timer::from_duration(std::time::Duration::from_millis(700)), |_, _, app: &mut App| {
            app.monitors_keep_arranged();
            TimeoutAction::Drop
        });
        if let Err(e) = armed {
            tracing::warn!("monitors: cannot arrange after a plug ({e})");
        }
    }

    /// [`Self::monitors_arrange`]; `only_moved`: only when the page has been
    /// arranged and a screen is not where it has it. What is ON each screen
    /// is what is put back with it — a resolution being tried included (its
    /// own keep or its going back decides what is saved of it).
    fn monitors_place(&mut self, only_moved: bool) {
        let monitors = crate::display::monitors();
        let trial = self.display_trial_screen();
        let applied = GolemSettings::change(|g| {
            if g.monitors.screens.as_deref() == Some("mirror") {
                return None;
            }
            if only_moved && (monitors.len() < 2 || g.monitors.places.is_empty()) {
                return None;
            }
            let screens = kept_together(&monitors, &g.monitors);
            // The MAIN screen stays where it is (what makes it the main one
            // with no built-in — and 0,0 put on a lone TV, with the lid shut,
            // met the laptop's own 0,0 when it opened): the others stand
            // where the drawing has them from it.
            let main = crate::launcher_prefs::main_screen(&monitors).unwrap_or(0);
            let anchor = monitors.get(main).map_or((0.0, 0.0), |m| (m.pos.0.round(), m.pos.1.round()));
            let corners = logical_from_mm(&screens, main, g.monitors.screens.as_deref() == Some("isolated"));
            let mut choices = Vec::new();
            let mut there = true;
            for ((s, (x, y)), m) in screens.iter().zip(corners).zip(&monitors) {
                // (A flipped screen stays flipped: the page only turns.)
                let transform = (s.rot / 90 % 4) as u8 + if m.transform >= 4 { 4 } else { 0 };
                let (x, y) = ((x + anchor.0) as i64, (y + anchor.1) as i64);
                there &= m.pos.0.round() as i64 == x && m.pos.1.round() as i64 == y && m.transform == transform;
                let choice = crate::display::DisplayChoice {
                    mode: m.mode.rule(),
                    scale: m.scale,
                    position: Some(format!("{x}x{y}")),
                    transform: Some(transform),
                };
                choices.push((s.sel.clone(), choice));
            }
            if only_moved && there {
                return None;
            }
            if !crate::display::apply_all(&choices) {
                return Some(Err(()));
            }
            keep_places(&mut g.monitors, &screens, main);
            let mut tried = None;
            for (sel, choice) in choices {
                if trial.as_deref() == Some(sel.as_str()) {
                    // (Being tried: only where it stands is its own yet.)
                    if let Some(saved) = g.displays.get_mut(&sel) {
                        saved.position.clone_from(&choice.position);
                        saved.transform = choice.transform;
                    }
                    tried = Some(choice);
                } else {
                    g.displays.insert(sel, choice);
                }
            }
            g.store();
            Some(Ok(tried))
        });
        let applied = match applied {
            Some(Ok(tried)) => {
                if let Some(choice) = tried {
                    self.display_trial_placed(choice.position, choice.transform);
                }
                Some(true)
            }
            Some(Err(())) => Some(false),
            None => None,
        };
        match applied {
            Some(true) => {
                info!("monitors: the screens arranged as the page has them");
                // The layers on the screens moved with them, once the
                // compositor has moved the screens.
                use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
                let armed = self.loop_handle.insert_source(Timer::from_duration(std::time::Duration::from_millis(120)), |_, _, app: &mut App| {
                    app.nudge_layers();
                    TimeoutAction::Drop
                });
                if armed.is_err() {
                    self.nudge_layers();
                }
            }
            Some(false) => tracing::warn!("monitors: the compositor did not take the arrangement"),
            None => {}
        }
        // (The pages read the screens again once they have moved.)
        self.monitors_read_soon(std::time::Duration::from_millis(400));
    }

    /// The Monitors page, when it is open, read again in `after`: the
    /// compositor takes a moment to make a change of resolution or scale.
    pub(crate) fn monitors_read_soon(&mut self, after: std::time::Duration) {
        use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
        let open = |p: &crate::panel::Panel| p.open_control_label() == Some("Monitors");
        if !open(self.settings_panel()) && !open(&self.quick) {
            return;
        }
        let armed = self.loop_handle.insert_source(Timer::from_duration(after), |_, _, app: &mut App| {
            let ui = app.ui_facts();
            app.controls_send(crate::controls::ControlCommand::Read("Monitors", ui));
            TimeoutAction::Drop
        });
        if let Err(e) = armed {
            tracing::warn!("monitors: cannot read the page again ({e})");
        }
    }

    /// A screen dragged in the drawing and let go at `(x, y)` (mm): it rests
    /// where the magnet and its sides say, saved; the screens are arranged
    /// so and the page is read again.
    pub(crate) fn monitors_moved(&mut self, sel: &str, x: f64, y: f64) {
        let monitors = crate::display::monitors();
        let main = crate::launcher_prefs::main_screen(&monitors).unwrap_or(0);
        let found = GolemSettings::change(|g| {
            let c = &mut g.monitors;
            // (Against the screens as the page drew them: the drop was.)
            let mut screens = kept_together(&monitors, c);
            let Some(i) = screens.iter().position(|s| s.sel == sel) else {
                return false;
            };
            let others: Vec<Box2> = screens.iter().filter(|o| o.sel != sel).map(PageScreen::drawn).collect();
            let d = Box2 { x, y, ..screens[i].drawn() };
            let (x, y) = settle(d, &others, c.screens.as_deref() == Some("isolated"));
            screens[i].at = (x, y);
            keep_places(c, &screens, main);
            c.picked = Some(sel.to_owned());
            g.store();
            info!("monitors: {sel} arranged at {x},{y}");
            true
        });
        if !found {
            tracing::warn!("monitors: no screen known as {sel:?}");
            return;
        }
        // …and the real screens moved so.
        self.monitors_arrange();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Box2 = Box2 { x: 0.0, y: 0.0, w: 2000.0, h: 1250.0 };

    #[test]
    fn a_side_near_another_sticks_to_it_and_lines_up() {
        // Dropped a little off the laptop's right side, a little low.
        let held = Box2 { x: 2060.0, y: 40.0, w: 3840.0, h: 2160.0 };
        let (x, y, side) = magnet(held, &[LAPTOP], 100.0, 0.0);
        assert_eq!((x, y, side), (2000.0, 0.0, Some((Side::Right, 0))), "touching, tops lined up");
        // Far from every side: free.
        let far = Box2 { x: 2600.0, y: 900.0, ..held };
        assert_eq!(magnet(far, &[LAPTOP], 100.0, 0.0), (2600.0, 900.0, None));
    }

    #[test]
    fn dropped_anywhere_it_rests_against_the_nearest_side() {
        let held = Box2 { x: 300.0, y: 1700.0, w: 3840.0, h: 2160.0 };
        let (x, y) = settle(held, &[LAPTOP], false);
        assert_eq!(y, 1250.0, "below the laptop, touching");
        assert!(x < 2000.0 - 120.0 && x + 3840.0 > 120.0, "and facing it");
        // On top of it: off it.
        let over = Box2 { x: 200.0, y: 100.0, w: 3840.0, h: 2160.0 };
        let (x, y) = settle(over, &[LAPTOP], false);
        assert!(!Box2 { x, y, ..over }.overlaps(&LAPTOP));
    }

    #[test]
    fn isolated_screens_stay_apart() {
        let away = Box2 { x: 5000.0, y: 3000.0, w: 1920.0, h: 1080.0 };
        assert_eq!(settle(away, &[LAPTOP], true), (5000.0, 3000.0), "left where it was");
        let over = Box2 { x: 100.0, y: 100.0, ..away };
        let (x, y) = settle(over, &[LAPTOP], true);
        let b = Box2 { x, y, ..away };
        assert!(!b.overlaps(&LAPTOP));
        let gap = (b.x - (LAPTOP.x + LAPTOP.w)).max(LAPTOP.x - (b.x + b.w)).max(b.y - (LAPTOP.y + LAPTOP.h)).max(LAPTOP.y - (b.y + b.h));
        assert!((gap - ISOLATED_GAP).abs() < 1.0, "a gap apart");
    }

    #[test]
    fn a_turn_swaps_the_sides_about_the_middle_and_back() {
        let tv = Box2 { x: 2000.0, y: 0.0, w: 3840.0, h: 2160.0 };
        let (b, rot) = turned(tv, 0, 90);
        assert_eq!(rot, 90);
        assert_eq!((b.w, b.h), (2160.0, 3840.0));
        assert_eq!((b.x + b.w / 2.0, b.y + b.h / 2.0), (tv.x + tv.w / 2.0, tv.y + tv.h / 2.0));
        let (back, rot) = turned(b, rot, -90);
        assert_eq!((back, rot), (tv, 0));
        assert_eq!(turned(tv, 0, -90).1, 270);
    }

    fn page(sel: &str, mode: (u32, u32), scale: f64, mm: (f64, f64), lpos: (f64, f64)) -> PageScreen {
        PageScreen {
            sel: sel.to_owned(),
            title: sel.to_owned(),
            short: sel.to_owned(),
            builtin: sel == "laptop",
            mode,
            scale,
            rot: 0,
            lpos,
            at: (0.0, 0.0),
            mm,
            sizes: Vec::new(),
            stops: Vec::new(),
            focused: false,
        }
    }

    #[test]
    fn the_drawing_is_in_real_sizes_and_goes_to_the_screens_in_pixels() {
        // Max's two (2026-10-21): the laptop 3200×2000 at 160 % (2000×1250
        // logical), 340×220 mm; the TV 3840×2160 at 200 % (1920×1080),
        // 1150×650 mm, at its right.
        let mut s = vec![
            page("laptop", (3200, 2000), 1.6, (340.0, 220.0), (0.0, 0.0)),
            page("tv", (3840, 2160), 2.0, (1150.0, 650.0), (2000.0, 0.0)),
        ];
        mm_from_logical(&mut s, 0);
        assert_eq!((s[0].at, s[1].at), ((0.0, 0.0), (340.0, 0.0)), "drawn at their real sizes, side by side");
        assert!(s[1].drawn().w / s[0].drawn().w > 3.3, "the TV three times and more the laptop");
        assert_eq!(logical_from_mm(&s, 0, false), vec![(0.0, 0.0), (2000.0, 0.0)], "and back");
        // On the laptop's left, tops lined up: so on the screens; below
        // zero, the laptop keeps 0,0.
        s[1].at = (-1150.0, 0.0);
        assert_eq!(logical_from_mm(&s, 0, false), vec![(0.0, 0.0), (-1920.0, 0.0)]);
        // Bottoms lined up: so.
        s[1].at = (340.0, 220.0 - 650.0);
        assert_eq!(logical_from_mm(&s, 0, false)[1], (2000.0, 1250.0 - 1080.0));
        // Under it, its left ends lined up.
        s[1].at = (0.0, 220.0);
        assert_eq!(logical_from_mm(&s, 0, false)[1], (0.0, 1250.0));
        // MAX'S, ON TOP (2026-10-21: "I put the TV on top of the built-in
        // and now I can't cross"): the TV centred over the laptop in the
        // drawing — it is centred over it on the screens, their sides
        // shared, not slid off at the laptop's pixels per mm.
        s[0].at = (419.0, 0.0);
        s[1].at = (14.0, -650.0);
        s[1].mode = (4096, 2160);
        let at = logical_from_mm(&s, 0, false);
        let tv = (at[1].0, at[1].0 + 2048.0);
        assert_eq!(at[1], (-24.0, -1080.0), "centred, its bottom on the laptop's top");
        assert!(tv.0 < 2000.0 && tv.1 > 0.0, "a side to cross");
        // A raise that lines nothing up: the middle of what they share is
        // the same point on both.
        s[0].at = (0.0, 0.0);
        s[1].mode = (3840, 2160);
        s[1].at = (340.0, -215.0);
        let y = logical_from_mm(&s, 0, false)[1].1;
        let middle_on_laptop = 110.0 * 1250.0 / 220.0;
        assert!((y + (110.0 + 215.0) * 1080.0 / 650.0 - middle_on_laptop).abs() < 1.0, "{y}");
    }

    #[test]
    fn screens_drawn_together_stay_together_on_the_screens() {
        // A screen that tells no size (96 dpi from its logical size) made
        // 150 %: 1280×720, 338.67×190.5 mm, its place saved rounded half a
        // millimetre off the laptop's top. It came 2 px above it on the
        // screens — the pointer could not cross.
        let mut s = vec![
            page("laptop", (1440, 900), 1.0, (381.0, 238.125), (0.0, 0.0)),
            page("ext", (1920, 1080), 1.5, (338.67, 190.5), (-756.0, -722.0)),
        ];
        s[1].at = (-200.0, -191.0);
        assert_eq!(logical_from_mm(&s, 0, false)[1].1, -720.0, "its bottom on the laptop's top");
        // Drawn apart where they must be together: put against the laptop.
        s[1].at = (-200.0, -260.0);
        let at = logical_from_mm(&s, 0, false)[1];
        assert!(at.1 == -720.0 || at.0 == 1440.0 || at.0 == -1280.0 || at.1 == 900.0, "{at:?}");
        // …but ISOLATED, apart they stay.
        s[1].at = (-200.0, -260.0);
        let at = logical_from_mm(&s, 0, true)[1];
        assert!(at.1 + 720.0 < -100.0, "{at:?}");
        // A drop settles exactly against the side, not rounded off it.
        let laptop = Box2 { x: 0.0, y: 0.0, w: 381.0, h: 238.125 };
        let (_, y) = settle(Box2 { x: -200.0, y: -230.0, w: 338.67, h: 190.5 }, &[laptop], false);
        assert_eq!(y, -190.5);
    }

    #[test]
    fn the_screens_drawn_from_the_compositor_line_up_as_they_are() {
        // The TV centred above the laptop on the screens: centred above it
        // in the drawing too.
        let mut s = vec![
            page("laptop", (3200, 2000), 1.6, (340.0, 220.0), (0.0, 0.0)),
            page("tv", (4096, 2160), 2.0, (1150.0, 650.0), (-24.0, -1080.0)),
        ];
        mm_from_logical(&mut s, 0);
        assert_eq!(s[1].at, (-405.0, -650.0));
        assert_eq!(logical_from_mm(&s, 0, false)[1], (-24.0, -1080.0), "and back");
    }

    #[test]
    fn a_screen_that_does_not_say_its_size_or_says_its_aspect_is_drawn_at_96_dpi() {
        let mut m = crate::display::Monitor {
            name: "HDMI-A-1".to_owned(),
            desc: String::new(),
            model: String::new(),
            mode: crate::display::Mode { w: 1920, h: 1080, hz: 60.0 },
            scale: 1.0,
            modes: Vec::new(),
            focused: false,
            pos: (0.0, 0.0),
            transform: 0,
            mm: (530, 300),
            twin: false,
        };
        assert_eq!(real_mm(&m, (1920.0, 1080.0)), (530.0, 300.0), "a 24-inch says so");
        let guess = (1920.0 * 25.4 / 96.0, 1080.0 * 25.4 / 96.0);
        m.mm = (0, 0);
        assert_eq!(real_mm(&m, (1920.0, 1080.0)), guess);
        m.mm = (16, 9);
        assert_eq!(real_mm(&m, (1920.0, 1080.0)), guess, "an aspect is not a size");
    }

    #[test]
    fn nothing_chosen_is_nothing_saved() {
        assert!(MonitorsChoice::default().is_default());
        let json = serde_json::to_string(&MonitorsChoice::default()).unwrap();
        let back: MonitorsChoice = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MonitorsChoice::default());
    }
}

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
use crate::display::{GolemSettings, Monitor};
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
    // (Lined up with what it rests against, when near.)
    let (x, y, _) = magnet(Box2 { x: at.0, y: at.1, ..d }, others, keep, gap);
    (x.round(), y.round())
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
pub(crate) fn logical_from_mm(screens: &[PageScreen], main: usize) -> Vec<(f64, f64)> {
    let n = screens.len();
    let drawn: Vec<Box2> = screens.iter().map(PageScreen::drawn).collect();
    let size: Vec<Box2> = screens.iter().map(PageScreen::logical).collect();
    let per_mm = |j: usize| (size[j].w / drawn[j].w.max(1e-6), size[j].h / drawn[j].h.max(1e-6));
    let mut at = vec![(0.0, 0.0); n];
    let at_cell = std::cell::RefCell::new(&mut at);
    outward(
        n,
        main,
        |i, j| touching(drawn[i], drawn[j], 0.5),
        |i, j, side| {
            let mut at = at_cell.borrow_mut();
            let (k, (jx, jy)) = (per_mm(j), at[j]);
            at[i] = match side {
                Side::Right => (jx + size[j].w, jy + (drawn[i].y - drawn[j].y) * k.1),
                Side::Left => (jx - size[i].w, jy + (drawn[i].y - drawn[j].y) * k.1),
                Side::Below => (jx + (drawn[i].x - drawn[j].x) * k.0, jy + size[j].h),
                Side::Above => (jx + (drawn[i].x - drawn[j].x) * k.0, jy - size[i].h),
            };
        },
        |i| {
            let mut at = at_cell.borrow_mut();
            let k = per_mm(main);
            at[i] = ((drawn[i].x - drawn[main].x) * k.0, (drawn[i].y - drawn[main].y) * k.1);
        },
    );
    // (Never one over another: pixels per mm that differ can bring two
    // together that the drawing has apart.)
    for i in 0..n {
        if i == main {
            continue;
        }
        let b = Box2 { x: at[i].0, y: at[i].1, ..size[i] };
        let others: Vec<Box2> = (0..n).filter(|&j| j != i).map(|j| Box2 { x: at[j].0, y: at[j].1, ..size[j] }).collect();
        if others.iter().any(|o| b.overlaps(o)) {
            at[i] = settle_in(b, &others, Some(ISOLATED_GAP_PX), KEEP_PX);
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
            let (k, (jx, jy)) = (per_mm(j), at[j]);
            at[i] = match side {
                Side::Right => (jx + drawn[j].w, jy + (size[i].y - size[j].y) / k.1),
                Side::Left => (jx - drawn[i].w, jy + (size[i].y - size[j].y) / k.1),
                Side::Below => (jx + (size[i].x - size[j].x) / k.0, jy + drawn[j].h),
                Side::Above => (jx + (size[i].x - size[j].x) / k.0, jy - drawn[i].h),
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
                rot: placed.map_or(u16::from(m.transform.min(3)) * 90, |p| p.rot),
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
fn keep_places(c: &mut MonitorsChoice, screens: &[PageScreen]) {
    for s in screens {
        c.places.insert(s.sel.clone(), Placed { x: s.at.0, y: s.at.1, rot: s.rot });
    }
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
    let mut screens = page_screens(&monitors, &saved);
    if !saved.places.is_empty() && saved.screens.as_deref() != Some("mirror") {
        keep_together(&mut screens, saved.screens.as_deref() == Some("isolated"));
    }
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
                bar: !mirrored && (s.builtin || options == "every" || (!screens.iter().any(|x| x.builtin) && s.sel == screens[0].sel)),
                dock: !mirrored && (s.builtin || dock == "every" || (!screens.iter().any(|x| x.builtin) && s.sel == screens[0].sel)),
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
        GolemSettings::change(|g| {
            let c = &mut g.monitors;
            let screens = page_screens(&monitors, c);
            let isolated = c.screens.as_deref() == Some("isolated");
            match key {
                "monitors-arrange" => {
                    let (verb, sel) = value.split_once(':').unwrap_or((value, ""));
                    match verb {
                        "pick" => c.picked = Some(sel.to_owned()),
                        "cw" | "ccw" => {
                            if let Some(s) = screens.iter().find(|s| s.sel == sel) {
                                let (b, rot) = turned(s.drawn(), s.rot, if verb == "cw" { 90 } else { -90 });
                                let others: Vec<Box2> = screens.iter().filter(|o| o.sel != sel).map(PageScreen::drawn).collect();
                                let (x, y) = settle(b, &others, isolated);
                                keep_places(c, &screens);
                                c.places.insert(sel.to_owned(), Placed { x, y, rot });
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
    }

    /// THE ARRANGEMENT, ON THE SCREENS: every screen put where the page has
    /// it — the whole moved so the main screen is at 0,0 — and turned as it has it,
    /// in one evaluation, then kept as the screens' own rules (their mode
    /// and scale as they are) and the page's places made those.
    pub(crate) fn monitors_arrange(&mut self) {
        let monitors = crate::display::monitors();
        let applied = GolemSettings::change(|g| {
            if g.monitors.screens.as_deref() == Some("mirror") {
                return None;
            }
            let screens = page_screens(&monitors, &g.monitors);
            // The MAIN screen keeps the corner 0,0 (what makes it the main one
            // with no built-in): the others stand where the drawing has them
            // from it, left of it or above it below zero.
            let main = crate::launcher_prefs::main_screen(&monitors).unwrap_or(0);
            let corners = logical_from_mm(&screens, main);
            let mut choices = Vec::new();
            for ((s, (x, y)), m) in screens.iter().zip(corners).zip(&monitors) {
                let (sel, rot) = (&s.sel, &s.rot);
                let mut choice = g.displays.get(sel).cloned().unwrap_or(crate::display::DisplayChoice {
                    mode: m.mode.rule(),
                    scale: m.scale,
                    position: None,
                    transform: None,
                });
                choice.position = Some(format!("{}x{}", x as i64, y as i64));
                choice.transform = Some((rot / 90 % 4) as u8);
                choices.push((sel.clone(), choice));
            }
            if !crate::display::apply_all(&choices) {
                return Some(false);
            }
            for (sel, choice) in choices {
                g.displays.insert(sel, choice);
            }
            g.store();
            Some(true)
        });
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
        GolemSettings::change(|g| {
            let c = &mut g.monitors;
            let screens = page_screens(&monitors, c);
            let Some(s) = screens.iter().find(|s| s.sel == sel) else { return };
            let others: Vec<Box2> = screens.iter().filter(|o| o.sel != sel).map(PageScreen::drawn).collect();
            let d = Box2 { x, y, ..s.drawn() };
            let (x, y) = settle(d, &others, c.screens.as_deref() == Some("isolated"));
            keep_places(c, &screens);
            c.places.insert(sel.to_owned(), Placed { x, y, rot: s.rot });
            c.picked = Some(sel.to_owned());
            g.store();
            info!("monitors: {sel} arranged at {x},{y}");
        });
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
        assert_eq!(logical_from_mm(&s, 0), vec![(0.0, 0.0), (2000.0, 0.0)], "and back");
        // Raised 215 mm: its corner above by 215 mm at the LAPTOP's pixels
        // per mm (the side it is against).
        s[1].at = (340.0, -215.0);
        assert_eq!(logical_from_mm(&s, 0)[1], (2000.0, (-215.0_f64 * 1250.0 / 220.0).round()));
        // On the laptop's left: below zero; the laptop keeps 0,0.
        s[1].at = (-1150.0, 0.0);
        assert_eq!(logical_from_mm(&s, 0), vec![(0.0, 0.0), (-1920.0, 0.0)]);
        // Under it, its left ends lined up.
        s[1].at = (0.0, 220.0);
        assert_eq!(logical_from_mm(&s, 0)[1], (0.0, 1250.0));
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

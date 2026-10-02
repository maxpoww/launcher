//! FAST LAUNCH (Max, 2026-10-01): Super+J → a glass figure in the middle of
//! the screen, a search for APPS only, warm with what you use. It opens
//! holding your three most-used apps (the most used in front); type, and the
//! front takes your best guess, the sides showing again once three or fewer
//! match. The front app's name is written under it, dimmed, with what you
//! typed bright. Enter launches the front app (a NEW instance), Tab/arrows
//! bring another to the front, a click on an icon launches that one; Escape
//! or a click anywhere else closes it — the opening played backward. The
//! dock surface draws it with the app icons it already holds, so it costs no
//! extra memory. (The look: Max's Fast Launch mockup, 2026-10-02.)
//!
//! Ranking, best first: a name that starts with the letters, then a word of
//! the name that does ("code" → Visual Studio Code), then the initials ("vs"
//! → Visual Studio Code), then the letters anywhere in the name (three or more); ties go to
//! the app used most, then the shorter name.

use smithay_client_toolkit::seat::keyboard::Keysym;

use crate::content::Rect;
use crate::{apps, App, KbSurface, LaunchFrom};

/// How many matches have to remain before they all show (more than that:
/// only the best guess, in the middle).
pub(crate) const REVEAL_AT: usize = 3;

/// Max's look, the Fast Launch mockup as he left it (2026-10-02), in the
/// mockup's px, made tighter step by step — and then "not a square card
/// anymore", "a little contoured, kind of like a cross": THE GLASS IS A
/// CROSS. A column round the front app's bare icon and the word under it
/// (the word's pill shows past it only when the name is wider), and a bar
/// across it holding the side icons, nearly as tall ("still too much": a
/// small step in at each corner) — the arms grow out with the icons —
/// joined by a smooth union with small rounded inner corners (the bar
/// banner's blister, `rounded_rect.wgsl`). (`GROW`: the dock reads the mockup a fifth bigger — "a
/// little bigger" on the first port.)
const GROW: f32 = 1.2;
/// The front icon, and the glass round it: margin and corner radius.
const ICON: f32 = 58.0;
const ICON_PAD: f32 = 7.0;
const ICON_R: f32 = 19.0;
/// A side icon, its glass, and the gap between its glass and the front's.
const SIDE_ICON: f32 = 42.0;
const SIDE_PAD: f32 = 6.0;
const SIDE_R: f32 = 15.0;
const SIDE_GAP: f32 = 0.0;
/// The word: its size and line; its pill's margins; how far the pill tucks
/// under the icon's glass; how wide it may grow (longer: cut).
const LETTER_PX: f32 = 20.0;
const LINE: f32 = 1.25;
const WORD_PAD_X: f32 = 10.0;
const WORD_PAD_Y: f32 = 2.0;
const WORD_TUCK: f32 = 4.0;
const WORD_MAX: f32 = 320.0;
/// How soft the inner corners of the cross are (the union's fillet).
const MELT: f32 = 5.0;
/// The cross's step: how far the side bar's edges sit in from the column's.
const NOTCH: f32 = 7.0;

/// "Super snappy" (seconds). Opening: the front icon pops, the sides wait a
/// beat, then slide out from behind it the glass melting out with them.
const POP: f32 = 0.09;
const INTRO_WAIT: f32 = 0.04;
const SLIDE: f32 = 0.15;
/// A side that comes or goes later, while typing.
const SLIDE_QUICK: f32 = 0.11;
/// Closing — the opening backward: the sides slide back in (SLIDE), and
/// from this far into it the front icon shrinks away and the glass fades.
const CLOSE_HOLD: f32 = SLIDE * 0.8;
const SHRINK: f32 = 0.08;
const FADE: f32 = 0.09;

/// CSS's `cubic-bezier(x1, y1, x2, y2)` at time `t` — the mockup's curves.
fn bezier((x1, y1, x2, y2): (f32, f32, f32, f32), t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let at = |a: f32, b: f32, s: f32| 3.0 * a * s * (1.0 - s).powi(2) + 3.0 * b * s * s * (1.0 - s) + s.powi(3);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if at(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(y1, y2, (lo + hi) / 2.0)
}
/// Out with a touch of overshoot; back in accelerating; the pop.
const OUT: (f32, f32, f32, f32) = (0.2, 0.9, 0.3, 1.12);
const IN: (f32, f32, f32, f32) = (0.5, 0.0, 0.75, 0.15);
const POP_CURVE: (f32, f32, f32, f32) = (0.2, 0.9, 0.25, 1.3);

/// How well an app `name` matches `q` (both lowercased): 4 its name starts
/// with it, 3 a word does, 2 the initials do, 1 it is anywhere in the name;
/// 0 no match (see `ANYWHERE_FROM`).
pub(crate) fn match_tier(name: &str, q: &str) -> u8 {
    if q.is_empty() {
        return 0;
    }
    if name.starts_with(q) {
        return 4;
    }
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    if words.iter().any(|w| w.starts_with(q)) {
        return 3;
    }
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    if initials.starts_with(q) {
        return 2;
    }
    // Anywhere in the name only from three letters on: two letters inside a
    // word are noise ("be" is not YouTube — Max, 2026-10-02).
    if q.chars().count() >= ANYWHERE_FROM && name.contains(q) {
        return 1;
    }
    0
}

/// How many letters a match in the middle of a word needs.
const ANYWHERE_FROM: usize = 3;

/// Rank `apps` — (name, times used) — for the query `q`: the indices of the
/// matches, best first (see the module doc). Nothing typed: every app, the
/// most used first.
pub(crate) fn rank(apps: &[(&str, u32)], q: &str) -> Vec<usize> {
    let q = q.trim().to_lowercase();
    let mut hits: Vec<(u8, u32, usize, usize)> = apps
        .iter()
        .enumerate()
        .filter_map(|(i, (name, used))| {
            let name = name.to_lowercase();
            let tier = if q.is_empty() { 1 } else { match_tier(&name, &q) };
            (tier > 0).then_some((tier, *used, name.len(), i))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    hits.into_iter().map(|(.., i)| i).collect()
}

/// One of the three places (0 the front, 1 left, 2 right) as it moves.
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct Place {
    /// The app there (kept while a side slides back in).
    pub entry: Option<usize>,
    /// The icon's pop, 0 → 1 (linear time; a new app in the place pops).
    pub pop: f32,
    /// A side's way out, 0 (behind the front icon) → 1 (in its place),
    /// linear time; `outward` says which curve it is on.
    pub out: f32,
    pub outward: bool,
}

impl Place {
    /// How far out a side is shown (the curve applied; overshoots a hair).
    fn shown_out(&self) -> f32 {
        if self.outward {
            bezier(OUT, self.out)
        } else {
            1.0 - bezier(IN, 1.0 - self.out)
        }
    }

    /// Turn around, picking the time on the new curve that shows the side
    /// where it is now (no jump).
    fn turn(&mut self, outward: bool) {
        if self.outward == outward {
            return;
        }
        let now = self.shown_out();
        self.outward = outward;
        let mut best = (f32::MAX, self.out);
        for i in 0..=64 {
            let p = i as f32 / 64.0;
            self.out = p;
            let d = (self.shown_out() - now).abs();
            if d < best.0 {
                best = (d, p);
            }
        }
        self.out = best.1;
    }
}

/// The fast-launch state.
#[derive(Default)]
pub(crate) struct FastLaunch {
    pub open: bool,
    pub query: String,
    /// Which of the shown apps is in the middle (0 = the best).
    pub sel: usize,
    /// The matching apps (entry indices), best first; nothing typed: every
    /// app, the most used first.
    pub matches: Vec<usize>,
    /// The card's glass, 0..1 (fades only as it closes).
    pub card_a: f32,
    pub places: [Place; 3],
    /// Time since it opened / since it began to close.
    pub since_open: f32,
    pub since_close: f32,
    /// The word's width as last drawn (its pill's size).
    pub word_w: f32,
}

/// Where everything sits, logical px on the dock surface.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Geo {
    /// The front icon, and the side icons in their places (left, right).
    pub icon: Rect,
    pub sides: [Rect; 2],
    /// How far a side travels out from behind the front icon.
    pub travel: f32,
    /// The glass: margins round the front / a side icon, their corner
    /// radii, and the fillet that melts the shapes together.
    pub icon_pad: f32,
    pub side_pad: f32,
    pub icon_r: f32,
    pub side_r: f32,
    pub melt: f32,
    /// How far the bar's top and bottom step in from the column's.
    pub notch: f32,
    /// The word's pill: its top, margins, widest; the word's type.
    pub word_top: f32,
    pub word_pad: (f32, f32),
    pub word_max: f32,
    pub font_px: f32,
    pub line_px: f32,
}

/// What to draw this frame.
pub(crate) struct View {
    /// The glass shapes melted together: the front icon's, the word's
    /// pill, the left and right icons' (`None` = not there).
    pub shapes: [Option<Rect>; 4],
    /// All of it (for the draw quad and the click-away test).
    pub bounds: Rect,
    pub a: f32,
    /// Icons in draw order (the front one last, on top): entry, rect.
    pub icons: Vec<(usize, Rect)>,
    /// Where the word is written (centred in it, clipped to it).
    pub text: Rect,
}

impl FastLaunch {
    /// Opened fresh: the opening plays from the start.
    pub(crate) fn opened() -> Self {
        Self { open: true, card_a: 1.0, ..Self::default() }
    }

    /// The apps on show: nothing typed, the three you use most; typed, the
    /// matches once at most three are left — before that, the best guess.
    pub(crate) fn shown(&self) -> &[usize] {
        let n = if self.query.trim().is_empty() || self.matches.len() <= REVEAL_AT {
            self.matches.len().min(REVEAL_AT)
        } else {
            1
        };
        &self.matches[..n]
    }

    /// The app each place wants now: the selected one in front, the others
    /// left then right of it.
    pub(crate) fn want(&self) -> [Option<usize>; 3] {
        let shown = self.shown();
        let mut want = [None; 3];
        for (p, &i) in placement(shown.len(), self.sel).iter().enumerate().take(3) {
            want[p] = shown.get(i).copied();
        }
        want
    }

    /// Anything left to draw.
    pub(crate) fn visible(&self) -> bool {
        self.open || self.card_a > 0.0
    }

    /// Advance the motion by `dt` seconds; true while anything still moves.
    pub(crate) fn step(&mut self, dt: f32) -> bool {
        let dt = dt.clamp(0.0, 0.05);
        if !self.visible() {
            return false;
        }
        let mut moving = false;
        let want = if self.open {
            self.since_open += dt;
            self.since_close = 0.0;
            self.card_a = 1.0;
            self.want()
        } else {
            self.since_close += dt;
            [self.places[0].entry, None, None]
        };
        // The front: a new app pops in; closing, it shrinks away and the
        // glass fades, once the sides are nearly back in.
        let front = &mut self.places[0];
        if want[0] != front.entry {
            front.entry = want[0];
            front.pop = 0.0;
        }
        if !self.open && self.since_close >= CLOSE_HOLD {
            front.pop = (front.pop - dt / SHRINK).max(0.0);
            self.card_a = (self.card_a - dt / FADE).max(0.0);
            moving |= self.card_a > 0.0;
        } else {
            moving |= !self.open || front.pop < 1.0;
            front.pop = (front.pop + dt / POP).min(1.0);
        }
        // The sides: out from behind the front icon (after a beat, the
        // first time), back in behind it when they go.
        let intro = self.since_open < INTRO_WAIT + SLIDE;
        let dur = if intro || !self.open { SLIDE } else { SLIDE_QUICK };
        let waiting = self.open && self.since_open < INTRO_WAIT;
        for (side, &w) in self.places[1..].iter_mut().zip(&want[1..]) {
            match w {
                Some(e) => {
                    if side.entry != Some(e) {
                        // Out already (another app was there): this one pops.
                        side.pop = if side.out > 0.0 { 0.0 } else { 1.0 };
                        side.entry = Some(e);
                    }
                    side.turn(true);
                    if !waiting {
                        side.out = (side.out + dt / dur).min(1.0);
                    }
                    side.pop = (side.pop + dt / POP).min(1.0);
                    moving |= side.out < 1.0 || side.pop < 1.0;
                }
                None if side.entry.is_some() => {
                    side.turn(false);
                    side.out = (side.out - dt / dur).max(0.0);
                    if side.out <= 0.0 {
                        side.entry = None;
                    }
                    moving = true;
                }
                None => {}
            }
        }
        moving
    }

    /// The glass and the icons where they are this frame; `word_w` is the
    /// word's shaped width.
    pub(crate) fn view(&self, g: &Geo, word_w: f32) -> View {
        let mut icons = Vec::with_capacity(3);
        let mut shapes = [None; 4];
        let [_, left, right] = self.places;
        for (i, side) in [left, right].iter().enumerate() {
            let Some(entry) = side.entry else {
                continue;
            };
            let v = side.shown_out();
            // From behind the front icon (toward the middle, smaller) to
            // its place, its glass round it; a new app there pops.
            let toward = if i == 0 { 1.0 } else { -1.0 };
            let s = 0.55 + 0.45 * v;
            let r = g.sides[i];
            let r = grow(Rect::new(r.x + toward * g.travel * (1.0 - v), r.y, r.w, r.h), s);
            shapes[2 + i] = Some(pad(r, g.side_pad * s));
            icons.push((entry, grow(r, pop_scale(side.pop))));
        }
        // The front's glass stays whole while it is open (a new app pops on
        // it); closing, it shrinks away with the icon.
        let closing = !self.open && self.since_close >= CLOSE_HOLD;
        let p = self.places[0].pop;
        let shrink = if closing { 0.4 * p + 0.6 * p * p } else { 1.0 };
        let front = grow(pad(g.icon, g.icon_pad), shrink);
        shapes[0] = Some(front);
        if let Some(entry) = self.places[0].entry {
            let s = if closing { shrink } else { pop_scale(p) };
            if s > 0.02 {
                icons.push((entry, grow(g.icon, s)));
            }
        }
        // The word's pill, tucked under the front's glass, as wide as the word.
        let cx = g.icon.x + g.icon.w / 2.0;
        let tw = word_w.min(g.word_max);
        let text = Rect::new(cx - tw / 2.0, g.word_top + g.word_pad.1, tw, g.line_px);
        if word_w > 0.0 {
            let pill = grow(pad2(text, g.word_pad), shrink);
            shapes[1] = Some(pill);
            // A CROSS, only a little contoured (Max, 2026-10-02): the front's
            // glass runs down as one column to the word's bottom (the pill
            // only shows past it when the name is wider)...
            shapes[0] = Some(Rect::new(front.x, front.y, front.w, (pill.y + pill.h - front.y).max(front.h)));
        }
        // ...and the side icons sit on one bar across it.
        let arm = shapes[2..].iter().flatten().fold(None::<Rect>, |b, r| Some(b.map_or(*r, |b| union(b, *r))));
        if let Some(arm) = arm {
            let mid = front.x + front.w / 2.0;
            // "Still too much": the bar is nearly as tall as the column — the
            // cross shows only as a small step in at each corner.
            let col = shapes[0].unwrap_or(front);
            let notch = g.notch.min(col.h / 4.0);
            let arm = union(arm, Rect::new(arm.x, col.y + notch, arm.w, col.h - 2.0 * notch));
            shapes[2] = Some(union(arm, Rect::new(mid, arm.y, 0.0, arm.h)));
            shapes[3] = None;
        }
        let bounds = shapes.iter().flatten().fold(front, |b, r| union(b, *r));
        View { shapes, bounds, a: self.card_a, icons, text }
    }
}

/// `r` with a margin `m` all round.
fn pad(r: Rect, m: f32) -> Rect {
    pad2(r, (m, m))
}

fn pad2(r: Rect, (mx, my): (f32, f32)) -> Rect {
    Rect::new(r.x - mx, r.y - my, r.w + 2.0 * mx, r.h + 2.0 * my)
}

fn union(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect::new(x, y, (a.x + a.w).max(b.x + b.w) - x, (a.y + a.h).max(b.y + b.h) - y)
}

/// An icon's size through its pop: from four tenths, overshooting a little.
fn pop_scale(p: f32) -> f32 {
    0.4 + 0.6 * bezier(POP_CURVE, p)
}

/// `r` scaled by `s` about its centre.
fn grow(r: Rect, s: f32) -> Rect {
    Rect::new(r.x + r.w * (1.0 - s) / 2.0, r.y + r.h * (1.0 - s) / 2.0, r.w * s, r.h * s)
}

/// The order the shown apps take their places in: the selected one in
/// front, the others left then right of it.
pub(crate) fn placement(n: usize, sel: usize) -> Vec<usize> {
    let mut order = Vec::with_capacity(n);
    if n > 0 {
        order.push(sel.min(n - 1));
        order.extend((0..n).filter(|&i| i != sel.min(n - 1)));
    }
    order
}

/// The word in the card: the front app's name, dimmed, with what you typed
/// bright where the name starts with it — `(bright, dim)`. A name that does
/// not start with it shows alone (dimmed); no app, just what you typed.
pub(crate) fn words(name: Option<&str>, query: &str) -> (String, String) {
    let Some(name) = name else {
        return (query.to_owned(), String::new());
    };
    let n = query.chars().count();
    if n > 0 && name.to_lowercase().starts_with(&query.to_lowercase()) {
        let cut = name.char_indices().nth(n).map_or(name.len(), |(i, _)| i);
        (name[..cut].to_owned(), name[cut..].to_owned())
    } else {
        (String::new(), name.to_owned())
    }
}

/// Where the figure sits on the dock surface: the front icon and its word
/// centred on the screen's centre (or as near as the surface reaches); it
/// never moves.
pub(crate) fn geometry(surface: (f32, f32), screen_h: f32, scale: f32) -> Geo {
    let (w, h) = surface;
    let u = scale * GROW;
    let icon = ICON * u;
    let font_px = LETTER_PX * u;
    let line_px = font_px * LINE;
    let word_pad = (WORD_PAD_X * u, WORD_PAD_Y * u);
    let glass = icon + 2.0 * ICON_PAD * u;
    let tall = glass - WORD_TUCK * u + line_px + 2.0 * word_pad.1;
    // The surface is anchored to the screen's bottom edge: the screen's
    // centre is `screen_h / 2` above that edge.
    let cy = (h - screen_h / 2.0).clamp(tall / 2.0, (h - tall / 2.0).max(tall / 2.0));
    let top = cy - tall / 2.0;
    let icon_r = Rect::new(w / 2.0 - icon / 2.0, top + ICON_PAD * u, icon, icon);
    let side = SIDE_ICON * u;
    let icy = icon_r.y + icon / 2.0;
    // Glass to glass, SIDE_GAP apart.
    let reach = glass / 2.0 + SIDE_GAP * u + SIDE_PAD * u + side / 2.0;
    let side_at = |dir: f32| Rect::new(w / 2.0 + dir * reach - side / 2.0, icy - side / 2.0, side, side);
    Geo {
        icon: icon_r,
        sides: [side_at(-1.0), side_at(1.0)],
        travel: reach,
        icon_pad: ICON_PAD * u,
        side_pad: SIDE_PAD * u,
        icon_r: ICON_R * u,
        side_r: SIDE_R * u,
        melt: MELT * u,
        notch: NOTCH * u,
        word_top: top + glass - WORD_TUCK * u,
        word_pad,
        word_max: WORD_MAX * u,
        font_px,
        line_px,
    }
}

impl App {
    /// Super+Alt+Space: open the bubble, or close it if it is up.
    pub(crate) fn toggle_fast_launch(&mut self) {
        if self.fast.open {
            self.close_fast_launch();
            return;
        }
        // The big card is the other way to launch: out of the way.
        if self.ui.target() == crate::state::Target::Open {
            self.handle_command(waverunner_proto::Command::Collapse);
        }
        self.fast = FastLaunch::opened();
        // Nothing typed yet: the card already holds the three apps you use
        // most — Enter launches the middle one straight away.
        self.fast_rematch();
        // Take the keyboard: what you type goes into the bubble.
        self.cancel_keyboard_handback(KbSurface::Launcher);
        crate::surface::set_interactive(&self.layer, true);
        self.interactive = true;
        self.sync_input_region();
        self.schedule_frame();
    }

    /// Close the bubble and hand the keyboard back to the window you were on.
    pub(crate) fn close_fast_launch(&mut self) {
        if !self.fast.open {
            return;
        }
        self.fast.open = false;
        if self.interactive && self.ui.target() != crate::state::Target::Open {
            self.begin_keyboard_handback(KbSurface::Launcher, None);
            crate::surface::set_interactive(&self.layer, false);
            self.interactive = false;
        }
        self.sync_input_region();
        self.schedule_frame();
    }

    /// Re-rank for the current query: apps only, the real ones (no catalog
    /// webapps, no Apps button / control panel / Bin, no transients).
    pub(crate) fn fast_rematch(&mut self) {
        let candidates: Vec<usize> = (0..self.base_len.min(self.entries.len()))
            .filter(|&i| {
                self.kinds.get(i) == Some(&apps::EntryKind::App)
                    && !apps::is_dock_fixed(&self.entries[i].id)
                    && !self.is_catalog_webapp(i)
            })
            .collect();
        let named: Vec<(&str, u32)> = candidates
            .iter()
            .map(|&i| (self.entries[i].name.as_str(), self.usage.count(&self.entries[i].id)))
            .collect();
        let order = rank(&named, &self.fast.query);
        let matches: Vec<usize> = order.into_iter().map(|j| candidates[j]).collect();
        self.fast.matches = matches;
        self.fast.sel = self.fast.sel.min(self.fast.shown().len().saturating_sub(1));
    }

    /// A key while the bubble is open: it takes every key.
    pub(crate) fn fast_key(&mut self, keysym: Keysym, utf8: Option<&str>) {

        match keysym {
            Keysym::Escape => self.close_fast_launch(),
            Keysym::Return | Keysym::KP_Enter => self.fast_launch_pick(None),
            Keysym::BackSpace => {
                self.fast.query.pop();
                self.fast.sel = 0;
                self.fast_rematch();
            }
            Keysym::Tab | Keysym::Right | Keysym::Down => {
                let n = self.fast.shown().len();
                if n > 0 {
                    self.fast.sel = (self.fast.sel + 1) % n;
                }
            }
            Keysym::ISO_Left_Tab | Keysym::Left | Keysym::Up => {
                let n = self.fast.shown().len();
                if n > 0 {
                    self.fast.sel = (self.fast.sel + n - 1) % n;
                }
            }
            _ => {
                if let Some(text) = utf8 {
                    let printable: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !printable.is_empty() {
                        self.fast.query.push_str(&printable);
                        self.fast.sel = 0;
                        self.fast_rematch();
                    }
                }
            }
        }
        self.schedule_frame();
    }

    /// Launch `entry` if given, else the app in front, else the best match.
    pub(crate) fn fast_launch_pick(&mut self, entry: Option<usize>) {
        let pick = entry
            .or(self.fast.want()[0])
            .or_else(|| self.fast.matches.first().copied());
        let Some(idx) = pick else {
            return;
        };
        self.fast.open = false;
        // Always a NEW instance — the box's rule: you asked to launch.
        self.activate(idx, LaunchFrom::Box);
        self.close_fast_launch_after_launch();
    }

    /// After `activate` took care of the keyboard (it hands it to the app it
    /// launches), just let the bubble fade and the input go.
    fn close_fast_launch_after_launch(&mut self) {
        self.fast.open = false;
        self.sync_input_region();
        self.schedule_frame();
    }

    /// A left click while the launcher is up: an icon launches its app; the
    /// card itself does nothing; anywhere else closes.
    pub(crate) fn fast_click(&mut self, pos: (f32, f32)) {
        let g = self.fast_geometry();
        let want = self.fast.want();
        let hit = [(g.icon, want[0]), (g.sides[0], want[1]), (g.sides[1], want[2])]
            .into_iter()
            .find_map(|(r, e)| e.filter(|_| r.contains(pos)));
        if let Some(entry) = hit {
            self.fast_launch_pick(Some(entry));
        } else if !self.fast.view(&g, self.fast.word_w).bounds.contains(pos) {
            self.close_fast_launch();
        }
    }

    /// Where the card and its icons sit.
    pub(crate) fn fast_geometry(&self) -> Geo {
        let screen_h = self.output_logical_height().unwrap_or(self.buffer_size.1 as f32);
        geometry(
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            screen_h,
            self.options_scale(),
        )
    }

    /// The word in the card (see [`words`]).
    pub(crate) fn fast_words(&self) -> (String, String) {
        let name = self.fast.places[0].entry.and_then(|e| self.entries.get(e)).map(|e| e.name.as_str());
        words(name, &self.fast.query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_rank_prefix_word_initials_anywhere() {
        assert_eq!(match_tier("firefox", "fi"), 4);
        assert_eq!(match_tier("visual studio code", "code"), 3);
        assert_eq!(match_tier("visual studio code", "vsc"), 2);
        assert_eq!(match_tier("thunar file manager", "ile"), 1);
        assert_eq!(match_tier("spotify", "zz"), 0);
        assert_eq!(match_tier("youtube", "be"), 0, "two letters inside a word are noise");
        assert_eq!(match_tier("youtube", "tub"), 1);
    }

    #[test]
    fn the_app_you_use_wins_a_tie_and_a_prefix_beats_usage() {
        let apps = [("Spotify", 3), ("Speedcrunch", 40), ("Inkscape", 90)];
        assert_eq!(rank(&apps, "sp"), vec![1, 0], "both start with sp: the used one first");
        assert_eq!(rank(&apps, "cape"), vec![2], "anywhere in the name still counts");
        assert_eq!(rank(&apps, "spo"), vec![0]);
    }

    #[test]
    fn nothing_typed_ranks_by_use() {
        let apps = [("Spotify", 3), ("Speedcrunch", 40), ("Inkscape", 90), ("Foot", 7)];
        assert_eq!(rank(&apps, ""), vec![2, 1, 3, 0]);
        let f = FastLaunch { matches: vec![2, 1, 3, 0], ..FastLaunch::opened() };
        assert_eq!(f.shown(), &[2, 1, 3], "the three most used");
        assert_eq!(f.want(), [Some(2), Some(1), Some(3)], "the most used in front, then left, right");
        let f = FastLaunch { query: "s".into(), matches: vec![1, 0, 2, 3], ..FastLaunch::opened() };
        assert_eq!(f.shown(), &[1], "more than three: only the best guess");
    }

    #[test]
    fn the_word_is_the_name_with_what_you_typed_bright() {
        assert_eq!(words(Some("Spotify"), "sp"), ("Sp".into(), "otify".into()));
        assert_eq!(words(Some("Spotify"), ""), ("".into(), "Spotify".into()));
        assert_eq!(words(Some("Visual Studio Code"), "code"), ("".into(), "Visual Studio Code".into()));
        assert_eq!(words(None, "zzq"), ("zzq".into(), "".into()));
    }

    #[test]
    fn it_opens_and_closes_in_a_blink() {
        let mut f = FastLaunch { matches: vec![5, 6, 7], ..FastLaunch::opened() };
        let g = geometry((2000.0, 760.0), 1250.0, 1.0);
        let mut t = 0.0;
        while f.step(1.0 / 144.0) {
            t += 1.0 / 144.0;
            assert!(t < 0.3, "the opening settles fast");
        }
        let v = f.view(&g, 80.0);
        assert_eq!(v.icons.len(), 3);
        assert_eq!(v.icons[2].0, 5, "the front icon is drawn last, on top");
        assert!(v.shapes[0].is_some() && v.shapes[1].is_some() && v.shapes[2].is_some(), "a cross: the column, the word, the bar");
        assert!(v.bounds.x < g.sides[0].x && v.bounds.x + v.bounds.w > g.sides[1].x + g.sides[1].w);
        f.open = false;
        t = 0.0;
        while f.step(1.0 / 144.0) {
            t += 1.0 / 144.0;
            assert!(t < 0.3, "the closing too");
        }
        assert!(!f.visible());
        assert!(f.places[1].entry.is_none() && f.places[2].entry.is_none(), "the sides went back in");
    }

    #[test]
    fn the_card_is_anchored_mid_screen() {
        // A 1250-tall screen, the dock surface its bottom 760.
        let g = geometry((2000.0, 760.0), 1250.0, 1.0);
        let top = g.icon.y - g.icon_pad;
        let bottom = g.word_top + g.line_px + 2.0 * g.word_pad.1;
        assert!(((top + bottom) / 2.0 - (760.0 - 625.0)).abs() < 0.01, "the screen's centre");
        assert!((g.icon.x + g.icon.w / 2.0 - 1000.0).abs() < 0.01, "and its middle");
        assert!(g.sides[0].x + g.sides[0].w < g.icon.x && g.sides[1].x > g.icon.x + g.icon.w, "beside the front icon");
        assert_eq!(placement(3, 1), vec![1, 0, 2], "the selected one goes in front");
        assert_eq!(placement(1, 0), vec![0]);
    }
}

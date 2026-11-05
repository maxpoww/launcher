//! THE KIT — one form per job, the same in every surface (Max, 2026-11-05:
//! "quiero full consistencia… formá una consistencia real"; the sheet is
//! https://claude.ai/artifact/QSig7uAfBuGvzASySGNNEg, board 1). Built on the
//! QUIET direction: nothing behind anything at rest; a fill only for a state;
//! the accent only for where you are, what is on and the one main action; red
//! only for what cannot be undone; the palette's warning colour for a problem.
//!
//! Every surface (the banner's boxes, the dock's box, the control panel,
//! menus, Files, Quick Look) takes its sizes and signal colours from here, so
//! they cannot drift apart again. The card is the one exception (Max: never
//! change the card).

use std::time::{Duration, Instant};

use crate::content::{GridContent, Label, Rect, RectInst, Scene};

/// The slim switch: 36 × 20, a 12 px knob 4 px in. Off: an outline at
/// `SWITCH_LINE` of the ink, its knob at the left at `SWITCH_KNOB_OFF`; on:
/// filled with the accent, the knob at the right in the accent's own
/// contrast. Centred on its row; the whole row switches it.
pub(crate) const SWITCH_W: f32 = 36.0;
pub(crate) const SWITCH_H: f32 = 20.0;
pub(crate) const SWITCH_KNOB: f32 = 12.0;
pub(crate) const SWITCH_LINE: f32 = 0.38;
pub(crate) const SWITCH_KNOB_OFF: f32 = 0.75;

/// A row's corner radius (times the scale).
pub(crate) const ROW_RADIUS: f32 = 7.0;

/// ⏏ — what lets a drive or a phone go (× only closes; the can throws away).
pub(crate) const GLYPH_EJECT: &str = "\u{f052}";

/// How long a "Click again to …" waits for its second click.
pub(crate) const ASK_FOR: Duration = Duration::from_secs(4);
/// Its words' size, and the air at their ends.
const ASK_PX: f32 = 14.0;
const ASK_PAD: f32 = 16.0;

/// A question asked IN PLACE before what cannot be undone (THE KIT: the
/// control becomes a red line saying what a second click does, "Click again to
/// …"; it ends by itself after `ASK_FOR`, or with any other click). What Undo
/// covers is never asked.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Ask(Option<Instant>);

impl Ask {
    /// Asking now (and not run out).
    pub(crate) fn asking(&self) -> bool {
        self.0.is_some_and(|t| t.elapsed() < ASK_FOR)
    }

    /// A press on the control: `true` = the second (do it now), `false` = the
    /// first (it asks from now).
    pub(crate) fn press(&mut self) -> bool {
        if self.asking() {
            self.0 = None;
            true
        } else {
            self.0 = Some(Instant::now());
            false
        }
    }

    /// Withdrawn (another click, the box closing).
    pub(crate) fn clear(&mut self) {
        self.0 = None;
    }
}

/// The asked line's place: a stadium `h` high round its words, centred on `c`
/// (`s` the scale).
pub(crate) fn ask_rect(c: (f32, f32), h: f32, words: &str, s: f32) -> Rect {
    let w = words.chars().count() as f32 * ASK_PX * s * 0.54 + 2.0 * ASK_PAD * s;
    Rect::new(c.0 - w / 2.0, c.1 - h / 2.0, w, h)
}

/// Draw the asked line at `r`: the red's quiet ground, its words in the red
/// (a step stronger under the pointer).
pub(crate) fn push_ask(scene: &mut Scene, r: Rect, words: &str, dark: bool, over: bool, a: f32, s: f32) {
    let red = danger(dark);
    let mut ground = danger_tint(dark);
    if over {
        ground[3] *= 1.6;
    }
    ground[3] *= a;
    scene.rects.push(RectInst { rect: r, radius: r.h / 2.0, color: ground, glass: 0.0, border: 0.0 });
    let px = ASK_PX * s;
    scene.labels.push(Label {
        text: words.to_owned(),
        pos: (r.x + r.w / 2.0, r.y + (r.h - px * 1.25) / 2.0),
        max_w: r.w,
        font_px: px,
        line_px: px * 1.25,
        centered: true,
        dim: false,
        cache: false,
        family: None,
        color: Some([red[0], red[1], red[2], a]),
        clip: None,
    });
}

impl crate::App {
    /// An asked line runs out by itself: the bar is drawn again when it has.
    pub(crate) fn ask_runs_out(&mut self) {
        let timer = calloop::timer::Timer::from_duration(ASK_FOR + Duration::from_millis(30));
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut crate::App| {
            app.draw_options();
            calloop::timer::TimeoutAction::Drop
        });
    }
}

/// `c` at `a` of its own alpha.
pub(crate) fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}

/// Whether an ink is a light one (so its ground is dark).
pub(crate) fn ink_is_light(ink: [f32; 4]) -> bool {
    ink[0] + ink[1] + ink[2] > 1.5
}

/// An icon button's name shows this long after the pointer rests on it, then
/// at once on its neighbours while the pointer stays among them.
pub(crate) const NAME_AFTER: Duration = Duration::from_millis(500);
/// The name's pill: its height, its words' size, the air at their ends, and
/// how far from its button.
const NAME_H: f32 = 24.0;
const NAME_PX: f32 = 12.0;
const NAME_PAD: f32 = 11.0;
const NAME_GAP: f32 = 6.0;

/// THE NAMES of icon buttons (THE KIT, 2026-11-05: an icon alone says what it
/// does once the pointer rests on it `NAME_AFTER`; after one has shown, the
/// others show at once while the pointer stays among them — the dock's names'
/// and Quick Look's head's way, one for every box, window and menu; the
/// banner's own pills stay unnamed). `T` says which button.
#[derive(Debug, Clone)]
pub(crate) struct Names<T> {
    on: Option<(T, Instant)>,
    warm: bool,
}

impl<T> Default for Names<T> {
    fn default() -> Self {
        Self { on: None, warm: false }
    }
}

impl<T: PartialEq + Clone> Names<T> {
    /// The pointer is on button `on` (`None`: on none), still `among` the
    /// buttons' surface or not. Returns how long until a frame should show a
    /// name, when one has to be waited for.
    pub(crate) fn follow(&mut self, on: Option<T>, among: bool) -> Option<Duration> {
        if self.on.as_ref().is_some_and(|(_, at)| at.elapsed() >= NAME_AFTER) {
            self.warm = true;
        }
        if !among {
            self.warm = false;
        }
        if self.on.as_ref().map(|(b, _)| b) == on.as_ref() {
            return None;
        }
        let now = Instant::now();
        let since = if self.warm { now.checked_sub(NAME_AFTER).unwrap_or(now) } else { now };
        let wait = on.is_some() && !self.warm;
        self.on = on.map(|b| (b, since));
        wait.then_some(NAME_AFTER + Duration::from_millis(20))
    }

    /// The button whose name shows now.
    pub(crate) fn shown(&self) -> Option<&T> {
        self.on.as_ref().filter(|(_, at)| at.elapsed() >= NAME_AFTER).map(|(b, _)| b)
    }
}

/// Where button `at`'s name stands: under it, or over it (`above`: a footer's
/// buttons, at the foot of their box), kept inside `room`.
pub(crate) fn name_rect(at: Rect, words: &str, above: bool, room: Rect, s: f32) -> Rect {
    let w = words.chars().count() as f32 * NAME_PX * s * 0.56 + 2.0 * NAME_PAD * s;
    let h = NAME_H * s;
    let x = (at.x + at.w / 2.0 - w / 2.0).min(room.x + room.w - w - 4.0 * s).max(room.x + 4.0 * s);
    let y = if above { at.y - NAME_GAP * s - h } else { at.y + at.h + NAME_GAP * s };
    Rect::new(x.round(), y.round(), w.round(), h.round())
}

/// Draw a name at `r` (Quick Look's dark pill, white words) over everything:
/// a grid of its own, last, and every word under it left out (all words are
/// drawn in one pass over every fill, so nothing else can cover them).
/// `linear`: the surface's colours are linear (the dock's, Files').
pub(crate) fn push_name(scene: &mut Scene, r: Rect, words: &str, a: f32, s: f32, linear: bool) {
    let covered = |l: &Label| {
        let w = (l.text.chars().count() as f32 * l.font_px * 0.56).min(l.max_w.max(0.0));
        let x0 = if l.centered { l.pos.0 - w / 2.0 } else { l.pos.0 };
        x0 < r.x + r.w && x0 + w > r.x && l.pos.1 < r.y + r.h && l.pos.1 + l.line_px > r.y
    };
    scene.labels.retain(|l| !covered(l));
    for g in &mut scene.grids {
        g.labels.retain(|l| !covered(l));
    }
    let ground = if linear { to_linear([0.06, 0.07, 0.09, 1.0]) } else { [0.06, 0.07, 0.09, 1.0] };
    let px = NAME_PX * s;
    scene.grids.push(GridContent {
        clip: Rect::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0),
        rects: vec![RectInst { rect: r, radius: r.h / 2.0, color: [ground[0], ground[1], ground[2], 0.88 * a], glass: 0.0, border: 0.0 }],
        labels: vec![Label {
            text: words.to_owned(),
            pos: (r.x + r.w / 2.0, r.y + (r.h - px * 1.3) / 2.0),
            max_w: r.w,
            font_px: px,
            line_px: px * 1.3,
            centered: true,
            dim: false,
            cache: false,
            family: None,
            color: Some([1.0, 1.0, 1.0, a]),
            clip: None,
        }],
        ..Default::default()
    });
}

/// The accent (the palette's own), as sRGB fractions.
pub(crate) fn accent() -> [f32; 4] {
    crate::palette::accent_srgb()
}

/// A row under the pointer or the keys: THE ONE MENU's quiet tint of the
/// accent, as a wash over whatever the row stands on (a menu mixes it into
/// its solid panel; a box's glass takes it as this alpha).
pub(crate) fn row_tint(bright: bool) -> [f32; 4] {
    let a = accent();
    [a[0], a[1], a[2], if bright { 0.16 } else { 0.22 }]
}

/// The switch's knob on the accent: dark on a light accent, light on a dark
/// one.
pub(crate) fn on_accent() -> [f32; 4] {
    let a = accent();
    if 0.2126 * a[0] + 0.7152 * a[1] + 0.0722 * a[2] > 0.55 {
        [0.06, 0.06, 0.07, 1.0]
    } else {
        [0.98, 0.98, 0.98, 1.0]
    }
}

/// A problem (a wrong password, a connection that failed): the palette's
/// warning colour — never the accent (it means "on"), never red (it means
/// "destroys").
pub(crate) fn warn() -> [f32; 4] {
    crate::palette::warn_srgb()
}

/// What cannot be undone, asked: THE ONE MENU's red (`desktop_menu::danger_on`:
/// the palette's own when it is clearly red and reads at 4.5 : 1, else Golem's),
/// for a box of the `dark` or the light look, as sRGB fractions.
pub(crate) fn danger(dark: bool) -> [f32; 4] {
    let ground = if dark { [0.086, 0.09, 0.106, 1.0] } else { [0.97, 0.97, 0.96, 1.0] };
    let (red, _) = crate::desktop_menu::danger_on(to_linear(crate::palette::danger_srgb()), to_linear(ground), dark);
    to_srgb(red)
}

/// The red's quiet ground under a question asked in place ("Click again to …").
pub(crate) fn danger_tint(dark: bool) -> [f32; 4] {
    let r = danger(dark);
    [r[0], r[1], r[2], if dark { 0.14 } else { 0.10 }]
}

fn to_linear(c: [f32; 4]) -> [f32; 4] {
    let f = |v: f32| if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

fn to_srgb(c: [f32; 4]) -> [f32; 4] {
    let f = |v: f32| {
        if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    };
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_red_goes_there_and_back() {
        let c = [0.776, 0.157, 0.157, 1.0];
        let back = to_srgb(to_linear(c));
        for i in 0..3 {
            assert!((back[i] - c[i]).abs() < 1e-4);
        }
    }
}

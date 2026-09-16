//! The settings gear's stats child: CPU / RAM / DISK / BATTERY.
//!
//! Hovering the gear at the bar's left end slides a readout out from behind it —
//! **the clipboard's peek, mirrored** (Max, 2026-09-13: *"on hover, a child opens
//! and show CPU/RAM/DISK/BATTERY, same animation of clipboard"*). Same geometry
//! (`left + (ph + BOND_GAP) * t`, width easing `ph → STATS_W`), the same
//! `MORPH_RATE` and `settle_t` the whole surface shares (`OptionUXRules.md` §3),
//! and the same leave-hold before it retracts, so the two edges of the bar
//! behave identically.
//!
//! The numbers come from the Brain's own `ContextState.metrics` — the daemon
//! reads no `/proc` of its own (method.md §1, "the Brain is the only sense").
//! A reading the engine does not have is simply not shown: a desktop with no
//! battery gets three readouts, not a fourth saying nothing.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};

use crate::animation::{ease_toward, lerp, settle_t, LEAVE_HOLD, MORPH_RATE, SETTLE_PX};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{PillId, BOND_GAP, EDGE_PAD, LINE_PX, NERD, PILL_MARGIN_Y, SCROLL_DEADZONE};
use crate::App;
use options_engine::NetworkLink;

/// The readout's full width (logical px, pre-scale). Sized from the widest
/// column it has to hold, NOT picked by eye: at the bar's own 17px type the
/// first cut gave 67px columns for text needing ~80, and the renderer WRAPPED
/// the overflow onto a second line that the band then clipped away — the values
/// simply vanished, live. Symbols instead of words bought most of it back: four
/// columns of "<icon> 100%" in monospaced type fit here with room.
const STATS_W: f32 = 280.0;
/// The readout's type: a step under the bar's own (`FONT_PX` = 17), which is
/// what a row of figures wants — it reads as a gauge rather than as a title.
const VALUE_PX: f32 = 15.0;
/// The symbols, a size up from their numbers (Max, 2026-09-13: *"make the icons
/// little bigger"*). They carry the meaning of the column — which reading this
/// is, and for the battery roughly how full — so they lead, and the figure
/// follows.
const ICON_PX: f32 = 19.0;
/// The breath between a symbol and its own number (Max, 2026-09-13: *"move the
/// icons and their number closer, and all the same distances"*).
///
/// Every couple gets EXACTLY this, because the layout spaces by each glyph's
/// measured ink ([`Icon::ink`]) rather than by its advance or a shared cell —
/// both of which leave slack that lands in the gap and differs per glyph.
const PAIR_GAP: f32 = 3.0;
/// The gap BETWEEN couples — the readings' own rhythm (Max, 2026-09-13: *"move
/// the icons on the gear pill, closer"*). Comfortably wider than [`PAIR_GAP`],
/// which is what keeps each symbol reading as belonging to the number beside it
/// rather than to the column next door.
const COL_GAP: f32 = 7.0;
/// The margin at both ends of the pill, which is NOT the couple gap: the ends
/// are bounded by the round caps, so they need air the middle does not. Tying
/// the two together is what made the last reading sit on the cap (Max: *"BT is
/// too close to the end of the pill"*) — tightening the row would have brought
/// that straight back, since a 15px margin barely clears a ~12px cap radius.
const END_PAD: f32 = 14.0;

// --- The box (Max, 2026-09-13: "scroll down or click the gear opens a box") --
/// The open panel's height is [`crate::options::BOX_DRAWER_H`] — the ONE height
/// every box on this bar opens to, never a number of its own. A height of its
/// own (540) is what cut this panel's bottom corners off against the fixed
/// surface (Max: *"why does the botton of the gear box have sharp corners?"*)
/// and what made it taller than its neighbours (*"gear, clipboard and notis
/// should be the same height"*). See [`App::stats_box_h`].
/// The open panel's corner radius: the CLIPBOARD's, imported rather than
/// chosen again — it is the other box on this edge, and a second opinion about
/// the same corner is exactly the inconsistency the bar cannot afford.
use crate::clipboard::BOX_RADIUS;
/// How far a wheel has to travel before the box opens. The same idea as the
/// clipboard's notch: one deliberate push, not a graze.
const NOTCH: f32 = 1.0;
/// The page number's type: big enough to read as the page's identity rather than
/// as another reading. A placeholder — it goes when the pages get content.
const PAGE_PX: f32 = 44.0;

// --- Page 1: Golem's settings ---
/// Inset of a setting row from the panel edge, and the row's own height.
const SETTING_PAD: f32 = 16.0;
const SETTING_ROW_H: f32 = 26.0;
/// A setting's label type — the box's reading size, not the row's gauge size.
const SETTING_PX: f32 = 14.0;
/// The switch, the SUNSET panel's exactly (`module_box.rs`): one toggle shape
/// on this bar, not one per box.
const SWITCH_W: f32 = 40.0;
const SWITCH_H: f32 = 20.0;

/// A symbol and the size it has to be drawn at to LOOK the size of its
/// neighbours.
///
/// Nerd Font packs several icon sets into one font and they are not drawn to a
/// common optical size: at one font size the Material bluetooth towered over the
/// Material aerial, which sat smaller than every Font Awesome glyph (Max,
/// 2026-09-13: *"BT looks bigger than the others, and WIFI looks smaller than
/// all of them"*). So each symbol carries the factor that makes its INK match
/// the others — measured off a screenshot, not guessed (see `docs/`-less note in
/// this module's tests).
#[derive(Clone, Copy)]
struct Icon {
    glyph: &'static str,
    /// Multiplier on [`ICON_PX`].
    scale: f32,
    /// How wide this glyph's INK is, as a fraction of the size it is drawn at.
    ///
    /// The layout spaces couples by ink, not by a common cell or by the font's
    /// advance — that is what makes every symbol-to-number distance exactly
    /// [`PAIR_GAP`] and every couple-to-couple distance exactly [`COL_GAP`],
    /// instead of each varying with how wide its own glyph happens to be
    /// (measured at 8.8–14.4px and 27.5–33px respectively before this).
    ///
    /// A fraction rather than a width so it survives a change of size: ink
    /// scales linearly with the font. Measured by connected-component analysis
    /// of a live screenshot; the glyphs all draw centred on their anchor, which
    /// is what lets the layout place ink precisely.
    ink: f32,
}

/// One readout: the thing's own symbol, where it stands, and how much room its
/// number needs at its widest.
struct Reading {
    icon: Icon,
    value: String,
    /// The widest form this number can take — what the layout RESERVES for it,
    /// rather than the current text. Empty means "as wide as it is".
    ///
    /// Otherwise the panel breathes: CPU crossing 9% → 10% is a digit wider, so
    /// the pill and every couple right of it would shift a few pixels every few
    /// seconds while the pointer rests on it.
    reserve: &'static str,
}

/// Every percentage is reserved at its widest; so is a transfer rate, which
/// swings between nothing and whatever the link can carry, several times a
/// second.
const RESERVE_PCT: &str = "100%";
const RESERVE_RATE: &str = "99.9MB";
/// A device count reserves TWO digits. Not because ten headsets are likely, but
/// because a column with no reservation has no slack to give: every percentage
/// here is left-aligned in a `100%` box and spills what it does not use into the
/// gap after it, so an unreserved single digit ends up with a trailing gap half
/// the size of its neighbours — which, once Bluetooth moved into the middle of
/// the row, read as the count belonging to the disk beside it (Max, 2026-09-13:
/// *"bt is touching disk"*). It also means a second device arriving does not
/// shove the rest of the row sideways.
const RESERVE_COUNT: &str = "10";

impl Reading {
    /// What the layout measures for this reading.
    fn template(&self) -> &str {
        if self.reserve.is_empty() {
            &self.value
        } else {
            self.reserve
        }
    }
}

/// A transfer rate in **bytes per second**, short enough for a column:
/// `0`, `12K`, `1.4M`, `1.1G`.
///
/// Kilo is 1024 here, not 1000: this is a quantity of bytes, and every other
/// place a person meets bytes on this machine (the file manager, `du`, the
/// trash's own "reclaim 200 MB") counts them the same way.
///
/// An unknown rate prints NOTHING rather than `0` — the aerial stands alone —
/// because a rate is unknown for the first three seconds after a link appears,
/// and a confident `0` there would say "connected but dead" about a link that
/// is merely young.
fn rate_text(bps: Option<u64>) -> String {
    const K: f64 = 1024.0;
    let Some(b) = bps else {
        return String::new();
    };
    let b = b as f64;
    if b < K {
        // Whole bytes: below a kilobyte the number IS the detail, and "0B" is a
        // true and useful reading — the link is up and idle.
        return format!("{}B", b as u64);
    }
    for (unit, scale) in [("KB", K), ("MB", K * K), ("GB", K * K * K)] {
        let scaled = b / scale;
        if scaled < K || unit == "GB" {
            // One decimal only while it buys precision: 1.4MB says more than
            // 1MB, 140MB says as much as 140.3MB and is a character shorter.
            return if scaled < 10.0 {
                format!("{scaled:.1}{unit}")
            } else {
                format!("{:.0}{unit}", scaled)
            };
        }
    }
    String::new()
}

// Nerd Font glyphs. Max, 2026-09-13: *"use icons instead of words like BAT"*.
//
// ⚠️ **Check a codepoint before trusting it** — a glyph the font lacks renders
// as tofu, and it looks like a layout bug rather than a missing character.
// `fc-list ":charset=f538:family=JetBrainsMono Nerd Font" family` answers in a
// second; fa-memory (`f538`) is NOT in this build, which one screenshot round
// found the slow way.
const fn icon(glyph: &'static str, scale: f32, ink: f32) -> Icon {
    Icon { glyph, scale, ink }
}

// The optical-size factors, MEASURED rather than guessed: the row was drawn at
// one size, screenshotted, each symbol's cell cropped and `-trim`med to its ink,
// and the factor computed from what came back (physical px, `ICON_PX` = 19):
//
//     microchip 17×18   database 18×20   harddisk 18×22
//     battery   18×10   aerial   16×13   bluetooth 16×28
//
// which is exactly what Max saw — the bluetooth rune's ink is nearly three times
// the battery's height, and the aerial is the smallest thing in the row.
//
// **Normalised on AREA, not height.** A battery and an aerial are *drawn* short
// and wide; matching their heights to the drive's would have made them enormous
// sideways. `sqrt(reference_area / own_area)` equalises visual mass, which is
// what the eye actually compares. The drive is the reference because Max
// approved it by eye ("disk looks big and good").
//
// The bluetooth rune gets a further nudge down: it is narrow, so its height
// dominates how big it reads, and pure area leaves it towering.
const SCALE_CPU: f32 = 1.14;
const SCALE_RAM: f32 = 1.05;
const SCALE_DISK: f32 = 1.00; // the reference
const SCALE_BATTERY: f32 = 1.35;
const SCALE_WIFI: f32 = 1.35;
const SCALE_BT: f32 = 0.86;
/// The graphics card is the FLATTEST glyph in the set (ink 60×30 at a common
/// size, against the drive's 60×76), so area-normalising alone would blow it up
/// to 1.59 — the same trap the battery and the aerial fell into, and both were
/// settled by eye at 1.35. Landed instead on the BATTERY, the other wide-flat
/// symbol and the one already approved: measured live off the bar, the card's
/// ink was 26×14 against the battery's 26×15, so this is 1.40 (the offline
/// estimate) times that difference.
const SCALE_GPU: f32 = 1.45;
/// It fills its cell horizontally, like the chip and the drive do.
const INK_GPU: f32 = 0.59;
/// fa-bolt is tall and narrow like the bluetooth rune, so it is held back too.
const SCALE_BOLT: f32 = 0.95;
/// The other Material link symbols sit with the drive.
const SCALE_MD_LINK: f32 = 1.05;

// Ink widths, as a fraction of the size each glyph is drawn at. The six on the
// bar were measured; the variants that only appear in another state inherit
// their sibling's, which is right to within a pixel because a battery at 40% is
// the same drawing as a battery at 90%.
const INK_BATTERY: f32 = 0.585;
/// The aerial is the one glyph whose ink does NOT sit centred on its anchor —
/// it hangs left — so its reserved ink is corrected to bring its number to the
/// same bond as the rest. The correction is ABSOLUTE, not proportional, so it
/// has to be re-checked whenever [`PAIR_GAP`] moves: trimmed to 0.435 it suited
/// a 6px bond, and at 3px that same trim ate the whole gap — the aerial's
/// number sat 1.25px from the glyph while every other column had 3 (measured
/// live, 2026-09-13). 0.505 puts it back in line.
const INK_WIFI: f32 = 0.505;
const INK_BT: f32 = 0.535;
/// Unmeasured: the bolt only shows while charging, and a lightning stroke is
/// the narrowest thing in the set.
const INK_BOLT: f32 = 0.40;
/// Unmeasured: the cable and the broken-link, which only show off wifi.
const INK_MD_LINK: f32 = 0.60;

/// fa-microchip.
const ICON_CPU: Icon = icon("\u{f2db}", SCALE_CPU, 0.604);
/// fa-database — stacked banks. md-memory (`f035b`) exists and is the literal
/// answer, but it draws as a chip and sat next to the microchip above looking
/// like the same icon twice: **a symbol that cannot be told from its neighbour
/// is not a symbol**, so the pair is chip-then-stack instead.
const ICON_RAM: Icon = icon("\u{f1c0}", SCALE_RAM, 0.596);
/// md-harddisk — a drive, with a platter on it. fa-hdd-o (`f0a0`) renders as a
/// flat featureless box at this size and says nothing.
const ICON_DISK: Icon = icon("\u{f02ca}", SCALE_DISK, 0.592);
/// md-expansion-card — a board with a fan on it, i.e. a graphics card. Picked
/// by rendering the candidates and looking: every chip-shaped glyph in this
/// font (`f4bc` oct-cpu, `f035b` md-memory, `eabe` cod-circuit-board) is a
/// second microchip beside the CPU's, and a symbol that cannot be told from its
/// neighbour is not a symbol. This one is unmistakably a card.
const ICON_GPU: Icon = icon("\u{f08ae}", SCALE_GPU, INK_GPU);
/// fa-bolt: a charging battery is a bolt everywhere, so charging needs no word
/// and no arrow — the symbol IS the state.
const ICON_CHARGING: Icon = icon("\u{f0e7}", SCALE_BOLT, INK_BOLT);
/// md-wifi-strength-1 → -4: the wireless link, drawn at its own strength, so
/// the arc says what the number beside it does not.
const ICON_WIFI: [Icon; 4] = [
    icon("\u{f091f}", SCALE_WIFI, INK_WIFI), // 1 bar
    icon("\u{f0922}", SCALE_WIFI, INK_WIFI), // 2
    icon("\u{f0925}", SCALE_WIFI, INK_WIFI), // 3
    icon("\u{f0928}", SCALE_WIFI, INK_WIFI), // 4
];
/// md-wifi-strength-off-outline — a radio that is up but has no signal to
/// report, which is not the same as being disconnected.
const ICON_WIFI_NONE: Icon = icon("\u{f092b}", SCALE_WIFI, INK_WIFI);
/// md-ethernet.
const ICON_WIRED: Icon = icon("\u{f0200}", SCALE_MD_LINK, INK_MD_LINK);
/// md-lan-disconnect: nothing is up.
const ICON_NO_LINK: Icon = icon("\u{f0318}", SCALE_MD_LINK, INK_MD_LINK);
/// md-bluetooth, and its two other states: connected (with the link), and off.
const ICON_BT: Icon = icon("\u{f00af}", SCALE_BT, INK_BT);
const ICON_BT_CONNECTED: Icon = icon("\u{f00b1}", SCALE_BT, INK_BT);
const ICON_BT_OFF: Icon = icon("\u{f00b2}", SCALE_BT, INK_BT);

/// One couple's width: its symbol, plus its number and the breath between them
/// when it has one. A symbol-only reading (a wired link, Bluetooth with nothing
/// connected) is exactly its symbol wide — no gap reserved for a number that
/// isn't there.
fn pair_w(icon_w: f32, value_w: f32, scale: f32) -> f32 {
    if value_w <= 0.0 {
        icon_w
    } else {
        icon_w + PAIR_GAP * scale + value_w
    }
}

/// The wireless symbol for a signal quality — four arcs, so each covers 25%.
fn wifi_glyph(pct: u8) -> Icon {
    let step = (pct as usize * ICON_WIFI.len()) / 101;
    ICON_WIFI[step.min(ICON_WIFI.len() - 1)]
}
/// fa-battery-full → -empty. The battery's own symbol says roughly how full it
/// is before the number is read, which is what an icon is for.
const ICON_BATTERY: [Icon; 5] = [
    icon("\u{f244}", SCALE_BATTERY, INK_BATTERY), // empty
    icon("\u{f243}", SCALE_BATTERY, INK_BATTERY), // quarter
    icon("\u{f242}", SCALE_BATTERY, INK_BATTERY), // half
    icon("\u{f241}", SCALE_BATTERY, INK_BATTERY), // three quarters
    icon("\u{f240}", SCALE_BATTERY, INK_BATTERY), // full
];

/// The battery symbol for a charge level — five steps, so each covers 20%.
fn battery_glyph(pct: u8) -> Icon {
    let step = (pct as usize * ICON_BATTERY.len()) / 101;
    ICON_BATTERY[step.min(ICON_BATTERY.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rate's whole job is to be read at a glance, in a column the layout
    /// has already reserved room for.
    #[test]
    fn a_rate_reads_in_four_characters() {
        assert_eq!(rate_text(None), "", "unknown says nothing, not zero");
        assert_eq!(
            rate_text(Some(0)),
            "0B",
            "a live idle link truthfully reads zero"
        );
        assert_eq!(rate_text(Some(940)), "940B");
        assert_eq!(rate_text(Some(1024)), "1.0KB");
        assert_eq!(rate_text(Some(12_000)), "12KB");
        assert_eq!(
            rate_text(Some(1_500_000)),
            "1.4MB",
            "1024-based, like every other byte count"
        );
        assert_eq!(rate_text(Some(140_000_000)), "134MB");
        assert_eq!(rate_text(Some(2_000_000_000)), "1.9GB");
        // Nothing may outgrow the width the layout reserves for it.
        for bps in [0, 1, 999, 1024, 999_999, 50_000_000, 9_000_000_000] {
            assert!(
                rate_text(Some(bps)).len() <= RESERVE_RATE.len(),
                "{bps} rendered wider than the reserved column"
            );
        }
    }

    /// The symbol has to agree with the number beside it — a full glyph next to
    /// "9%" is the surface contradicting itself in the same breath. Pure, so it
    /// is checked here rather than by waiting for a battery to drain.
    #[test]
    fn the_battery_symbol_follows_the_charge() {
        assert_eq!(battery_glyph(0).glyph, ICON_BATTERY[0].glyph, "empty");
        assert_eq!(battery_glyph(100).glyph, ICON_BATTERY[4].glyph, "full");
        // Every step is reachable, and it only ever climbs.
        let mut seen: Vec<&str> = Vec::new();
        for pct in 0..=100u8 {
            let g = battery_glyph(pct).glyph;
            if seen.last() != Some(&g) {
                seen.push(g);
            }
        }
        let ladder: Vec<&str> = ICON_BATTERY.iter().map(|i| i.glyph).collect();
        assert_eq!(
            seen, ladder,
            "the ladder must climb once through every step, in order"
        );
    }
}

/// The peek's animation state. Mirrors `ClipState`'s peek fields exactly.
#[derive(Debug, Default)]
pub(crate) struct StatsState {
    /// Whether the readout should be out (the pointer is on the gear or on it).
    pub(crate) reveal: bool,
    /// Slide/width progress, 0 (hidden behind the gear) → 1 (fully out).
    pub(crate) t: f32,
    last: Option<Instant>,
    frame_pending: bool,
    /// Set when the pointer leaves: the readout holds briefly, so crossing a
    /// small gap does not snap it shut.
    hold_deadline: Option<Instant>,
    /// Measured `(symbol, number)` widths, one per reading, in the order
    /// [`App::stats_readings`] builds them. Refreshed by
    /// [`App::measure_stats`]; the draw pass cannot measure (it holds `&self`
    /// and the renderer measures through `&mut`).
    widths: Vec<(f32, f32)>,
    /// Whether the panel is open (intent), and how far it has grown into one
    /// (0 = the row, 1 = the full panel). The row IS the panel collapsed —
    /// one shape, like every other box on this bar.
    pub(crate) open: bool,
    e: f32,
    /// Wheel travel accumulated while the box is shut, so it takes a real push
    /// to open rather than a graze.
    scroll_accum: f32,
    /// Which page the panel is showing, 1-based: **1 is the gear's own**, and
    /// every reading after it takes the next number in the order they are drawn
    /// (Max, 2026-09-13: *"make each a button, everyone have a page on the box…
    /// click on the gear, the box show a 1, click on wifi, the box show a 2"*).
    ///
    /// Numbered rather than named because the pages have no content yet; when
    /// they do, this becomes the page's identity and the number goes away.
    page: usize,
}

impl App {
    /// The readout's rect: at rest exactly the gear's circle (hidden behind it),
    /// sliding right and widening as `t` rises. The single source for drawing
    /// and hit-testing, like `clip_geom`.
    pub(crate) fn stats_geom(&self) -> Rect {
        let ph = self.options_pill_h();
        let t = self.stats.t;
        let left = EDGE_PAD + (ph + BOND_GAP) * t;
        let w = lerp(ph, self.stats_full_w(), t).max(ph);
        // Opening grows the row DOWNWARD and does nothing else: same width, same
        // left edge, the readings still sitting in the band at the top. The box
        // is the row grown, not a different object that replaced it.
        let h = lerp(ph, self.stats_box_h(), self.stats.e);
        Rect::new(left, PILL_MARGIN_Y, w, h)
    }

    /// The right edge the readout can EVER reach: open, at its measured width.
    /// The left-edge twin of [`App::clip_span_full_right`] and used for the same
    /// reason — the frost sampler must treat it as ours whether it is open or
    /// not, or the colour every left box wears would jump when it opens.
    pub(crate) fn stats_span_full_right(&self) -> f32 {
        let ph = self.options_pill_h();
        EDGE_PAD + (ph + BOND_GAP) + self.stats_full_w()
    }

    /// The panel's full height: the bar's one drawer height. Empty for now —
    /// Max is choosing what goes in it.
    fn stats_box_h(&self) -> f32 {
        self.options_box_drawer_h().max(self.options_pill_h())
    }

    /// How far the panel has grown, 0 (the row) → 1 (open). Read by the pill
    /// draw for the corner radius and the surface.
    pub(crate) fn stats_open_t(&self) -> f32 {
        self.stats.e
    }

    /// The readout's corner radius: the row's stadium, easing to the panel's
    /// rounded rectangle as it opens. Measured against the BAND height so it
    /// cannot balloon with the box.
    pub(crate) fn stats_radius(&self) -> f32 {
        let s = self.options_scale();
        lerp(
            self.options_pill_h() / 2.0,
            BOX_RADIUS * s,
            self.stats.e.clamp(0.0, 1.0),
        )
    }

    /// Whether the panel is painting below the bar line — the left-edge twin of
    /// `NotifState::occludes_below_bar` and `clip_occludes_below_bar`.
    ///
    /// The bar's colour-match samples the window just under itself; an open box
    /// paints over exactly those rows, so the sampler must skip its columns or
    /// the bar ends up matching ITSELF. The other two boxes already declare
    /// this; a third that did not would reintroduce a bug they each had once.
    pub(crate) fn stats_occludes_below_bar(&self) -> bool {
        self.stats.e > 0.01
    }

    /// The fully-open bottom, for the input region — the settled height, not the
    /// animating one, so the region is stable the instant it opens.
    pub(crate) fn stats_box_input_bottom(&self) -> f32 {
        PILL_MARGIN_Y + self.stats_box_h()
    }

    /// Which page the panel is showing. Never zero: a panel opened by the wheel
    /// rather than by a button shows the gear's own page, which is page 1.
    pub(crate) fn stats_page(&self) -> usize {
        self.stats.page.max(1)
    }

    /// Every button on this element and the page it opens: the **gear is 1**,
    /// then one per reading in the order they are drawn. Built from the live
    /// readings, so a machine with no battery has seven pages rather than eight
    /// with a dead one.
    ///
    /// Each reading's target is widened to the midpoints between its neighbours,
    /// so the row has no dead strips between its buttons — a click anywhere on
    /// it lands on the reading it looks like it landed on.
    fn stats_buttons(&self) -> Vec<(usize, Rect)> {
        let rect = self.stats_geom();
        let cols = self.stats_columns(rect);
        let mut out = Vec::with_capacity(cols.len());
        let right_edge = rect.x + rect.w;
        for (i, c) in cols.iter().enumerate() {
            let left = match i {
                0 => rect.x,
                _ => (cols[i - 1].x + cols[i - 1].w + c.x) / 2.0,
            };
            let right = match cols.get(i + 1) {
                Some(n) => (c.x + c.w + n.x) / 2.0,
                None => right_edge,
            };
            out.push((i + 2, Rect::new(left, rect.y, right - left, rect.h)));
        }
        out
    }

    /// The page a press at `(x, y)` opens, if it landed on a button.
    ///
    /// Only the BAND at the top holds buttons. The panel below it is a page, not
    /// a row of controls: a press there must not turn the page you are reading
    /// just because the reading it grew from is above that x.
    pub(crate) fn stats_page_at(&self, x: f32, y: f32) -> Option<usize> {
        if y > self.stats_geom().y + self.options_pill_h() {
            return None;
        }
        self.stats_buttons()
            .into_iter()
            .find(|(_, r)| x >= r.x && x < r.x + r.w)
            .map(|(p, _)| p)
    }

    /// A button was pressed: show its page. Pressing the button of the page
    /// already showing closes the panel — the gear's own toggle, generalised, so
    /// every button behaves the way the one that came first does.
    pub(crate) fn stats_open_page(&mut self, page: usize) {
        if self.stats.open && self.stats_page() == page {
            self.set_stats_box(false);
            return;
        }
        self.stats.page = page;
        if self.stats.open {
            // Already open: just turn the page. No animation to run, but the
            // panel has to be redrawn with its new contents.
            self.draw_options();
        } else {
            self.set_stats_box(true);
        }
    }

    /// Open or close the panel. The page is left alone: the wheel and the debug
    /// verb are not buttons, so the panel comes back showing whatever was last
    /// chosen (page 1 until something is).
    pub(crate) fn set_stats_box(&mut self, open: bool) {
        if self.stats.open == open {
            return;
        }
        self.stats.open = open;
        self.stats.scroll_accum = 0.0;
        // The row has to be out for the panel to grow from it — opening from a
        // click on the gear while the readout is still tucked away would have
        // the panel appear from nothing.
        if open {
            self.stats.reveal = true;
        }
        self.stats.last = None;
        self.measure_stats();
        self.schedule_stats_frame();
        self.sync_options_input();
    }

    /// The wheel over the gear or its readout: down opens the panel, up closes
    /// it. Direction honours `input.natural_scroll` like every other scroll on
    /// this bar, so all of them agree about which way is down.
    pub(crate) fn stats_axis(&mut self, value: f32) {
        // A scroll's END arrives as an axis event carrying ZERO, and a kinetic
        // flick trails a run of ever-smaller ones; reading those as a push is
        // what once closed the playing box the instant a finger stopped. Only a
        // real push counts.
        if value.abs() < SCROLL_DEADZONE {
            return;
        }
        let delta = if self.config.input.natural_scroll {
            value
        } else {
            -value
        };
        // **Pull UP to open** (Max, 2026-09-13). The box grows downward out of
        // the row, so the gesture that brings it out is the one that drags its
        // contents up into view — and pushing back down puts it away.
        let opening = delta < 0.0;
        if self.stats.open {
            if !opening {
                self.set_stats_box(false);
            }
            return;
        }
        self.stats.scroll_accum += -delta;
        if self.stats.scroll_accum >= NOTCH {
            self.set_stats_box(true);
        } else if self.stats.scroll_accum < -NOTCH {
            self.stats.scroll_accum = 0.0;
        }
    }

    /// How wide the readout is when fully out: **what it is actually holding**,
    /// not a constant.
    ///
    /// The couples are already measured for their own layout, so the panel can
    /// be their sum plus the gaps and the round caps. That is the difference
    /// between a readout and a box with things in it: adding network and
    /// Bluetooth to a fixed 280px panel put six couples in four couples' worth
    /// of room and they collided (live, 2026-09-13), and a machine with no
    /// battery would have been left with a stretch of empty pill.
    fn stats_full_w(&self) -> f32 {
        let s = self.options_scale();
        if self.stats.widths.is_empty() {
            return STATS_W * s; // nothing measured yet (first frame)
        }
        let ink: f32 = self
            .stats
            .widths
            .iter()
            .map(|(icon_w, value_w)| pair_w(*icon_w, *value_w, s))
            .sum();
        let gaps = COL_GAP * s * (self.stats.widths.len() - 1) as f32;
        // The SAME margin the draw lays out from, at both ends — get this wrong
        // and the row is off-centre by the difference. It was `options_pill_h()`
        // (the round caps) while the draw used the gap, which left a right
        // margin of about one pixel: the arithmetic behind "BT is too close to
        // the end of the pill".
        ink + gaps + 2.0 * END_PAD * s
    }

    /// Whether the readout is out far enough to be worth drawing or touching.
    pub(crate) fn stats_out(&self) -> bool {
        self.stats.t > 0.001
    }

    /// Whether the readout has claimed the left edge, so the clipboard cluster
    /// steps out of the layout for as long as it is there.
    ///
    /// It has to LEAVE rather than fade: those three pills draw themselves and
    /// `continue` before the per-pill fade exists, and glyphs are drawn in one
    /// late pass — so a covered clipboard icon lands on top of the readout
    /// sliding over it, which reads as a rendering fault (seen live,
    /// 2026-09-13). The threshold is small but not zero: by the time it trips,
    /// the readout is already spilling out from under the gear, so the swap
    /// happens beneath something rather than in the open.
    pub(crate) fn stats_covering(&self) -> bool {
        self.stats.t > 0.12
    }

    /// Measure each reading's symbol and number, so the draw can lay the pair
    /// out at its true width. Cheap, and called wherever the readings or the
    /// scale can have changed: when the readout is raised, on every Brain
    /// snapshot while it is out, and from the bar's own measuring pass.
    pub(crate) fn measure_stats(&mut self) {
        let templates: Vec<String> = self
            .stats_readings()
            .iter()
            .map(|r| r.template().to_owned())
            .collect();
        let inks: Vec<f32> = self
            .stats_readings()
            .iter()
            .map(|r| ICON_PX * r.icon.scale * r.icon.ink)
            .collect();
        let s = self.options_scale();
        let value_px = VALUE_PX * s;
        let Some(r) = self.options_renderer.as_mut() else {
            return;
        };
        // Only the NUMBERS are measured; a symbol's width is its INK
        // ([`Icon::ink`]), never the font's advance — the advance varies by icon
        // set and has nothing to do with how wide the glyph looks, which is what
        // made the gaps wobble from column to column.
        self.stats.widths = templates
            .into_iter()
            .zip(inks)
            .map(|(value, ink)| (ink * s, r.measure_text(&value, value_px, Some(NERD))))
            .collect();
    }

    /// Recompute whether the readout should show. Hovering the gear OR the
    /// readout it revealed holds it open — crossing from one to the other is
    /// transit, not departure (the clipboard's rule, same words).
    pub(crate) fn update_stats_reveal(&mut self) {
        let on = matches!(
            self.options_hover,
            Some(PillId::Settings | PillId::SettingsStats)
        );
        if on {
            self.stats.hold_deadline = None;
            if !self.stats.reveal {
                self.stats.reveal = true;
                self.stats.last = None;
                self.measure_stats();
                self.schedule_stats_frame();
            }
        } else if self.stats.reveal && self.stats.hold_deadline.is_none() {
            // The panel folds with the row when the hand leaves, like every
            // other box on this bar — a visit ends when you go.
            self.schedule_stats_collapse(LEAVE_HOLD);
        }
    }

    /// Slide the readout out (or back) without a pointer — `debug-stats`.
    /// Returns whether it is now out. It holds until the verb is sent again or
    /// the pointer visits the gear and leaves, which is the same rule a real
    /// hover follows.
    pub(crate) fn toggle_stats_debug(&mut self) -> &'static str {
        // A three-state cycle, mirroring `debug-sunset`'s "already up ⇒ toggle
        // the box": row out → panel open → all the way back.
        match (self.stats.reveal, self.stats.open) {
            (false, _) => {
                self.stats.hold_deadline = None;
                self.stats.reveal = true;
                self.stats.last = None;
                self.measure_stats();
                self.schedule_stats_frame();
                "row"
            }
            (true, false) => {
                self.set_stats_box(true);
                "box"
            }
            (true, true) => {
                self.set_stats_box(false);
                self.stats.reveal = false;
                self.stats.last = None;
                self.schedule_stats_frame();
                "shut"
            }
        }
    }

    /// Retract after `delay` unless the pointer comes back in the meantime.
    fn schedule_stats_collapse(&mut self, delay: Duration) {
        let at = Instant::now() + delay;
        self.stats.hold_deadline = Some(at);
        let timer = Timer::from_duration(delay);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                // Only the deadline this timer armed may act: a later hover
                // clears it, and a stale timer must not close what the hand
                // came back to.
                if app.stats.hold_deadline == Some(at) {
                    app.stats.hold_deadline = None;
                    app.stats.reveal = false;
                    app.stats.open = false;
                    app.stats.scroll_accum = 0.0;
                    app.stats.last = None;
                    app.schedule_stats_frame();
                    app.sync_options_input();
                }
                TimeoutAction::Drop
            });
    }

    fn schedule_stats_frame(&mut self) {
        if self.stats.frame_pending {
            return;
        }
        self.stats.frame_pending = true;
        if self.stats.last.is_none() {
            self.stats.last = Some(Instant::now());
        }
        let timer = Timer::from_duration(Duration::from_millis(8));
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.stats.frame_pending = false;
                app.tick_stats();
                TimeoutAction::Drop
            });
    }

    /// Advance the slide one frame — the same rate, and a settle measured
    /// against the span this particular morph carries (§3).
    fn tick_stats(&mut self) {
        let now = Instant::now();
        let dt = self
            .stats
            .last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32())
            .min(0.05);
        self.stats.last = Some(now);
        let target = if self.stats.reveal { 1.0 } else { 0.0 };
        // Settled against the span this morph actually carries — the row's own
        // width, not the fallback constant (§3: one rate, and a settle measured
        // in real pixels).
        let (t, mut moving) = ease_toward(
            self.stats.t,
            target,
            dt,
            MORPH_RATE,
            settle_t(self.stats_full_w()),
        );
        self.stats.t = t;
        // The panel's own growth, settled against the span IT carries (§3: the
        // rate is shared, the distance is not).
        let box_target = if self.stats.open { 1.0 } else { 0.0 };
        let span = (self.stats_box_h() - self.options_pill_h()).max(1.0);
        let (e, box_moving) = ease_toward(
            self.stats.e,
            box_target,
            dt,
            MORPH_RATE,
            settle_t(span).max(SETTLE_PX / span),
        );
        self.stats.e = e;
        moving |= box_moving;
        self.draw_options();
        if moving {
            self.schedule_stats_frame();
        } else {
            self.stats.last = None;
            // Fully shut: the input region can come back off the box's area.
            if !self.stats.open && self.stats.e <= 0.0 {
                self.sync_options_input();
            }
        }
    }

    /// What the Brain currently knows about the machine, in the order Max named
    /// them (2026-09-13): **wifi · bt · disk · cpu · ram · gpu · battery** —
    /// the links first, then what is full, then what is busy, and the battery
    /// last, beside the bar's other right-hand furniture. A reading the engine
    /// does not have is left out rather than shown as a dash — the columns
    /// divide the width by how many there actually are.
    fn stats_readings(&self) -> Vec<Reading> {
        let Some(ctx) = self.brain.as_ref() else {
            return Vec::new();
        };
        let m = &ctx.metrics;
        let mut out: Vec<Reading> = Vec::new();
        // The network. **The symbol carries strength, the number carries
        // speed** — two different facts, so neither repeats the other.
        //
        // The number used to be the signal percentage, and Max asked what it
        // meant (2026-09-13). The honest answer was "RSSI + 110, scaled by 70" —
        // a flattering, compressed figure that says nothing about how fast the
        // link is. So the aerial keeps drawing the strength (which is what an
        // aerial is for) and the figure is the negotiated bitrate.
        let net = &ctx.network;
        out.push(match net.link {
            NetworkLink::Wireless => Reading {
                icon: net.signal_pct.map_or(ICON_WIFI_NONE, wifi_glyph),
                value: rate_text(net.throughput_bps),
                reserve: RESERVE_RATE,
            },
            NetworkLink::Wired => Reading {
                icon: ICON_WIRED,
                value: rate_text(net.throughput_bps),
                reserve: RESERVE_RATE,
            },
            NetworkLink::Down => Reading {
                icon: ICON_NO_LINK,
                value: String::new(),
                reserve: "",
            },
        });
        // Bluetooth, but ONLY when the engine can actually see it: an absent
        // adapter or a stopped bluetoothd is not "nothing connected", and a
        // column that quietly means "we cannot tell" is the surface lying.
        //
        // The count is ALWAYS drawn, `0` included (Max, 2026-09-13: *"add a 0
        // after BT, (counter) its 1 when 1 connected, 0 = (disconnected)"*). It
        // used to disappear at zero, on the reasoning that a bare symbol says
        // "on, nothing paired" — but then this column is the only one that
        // sometimes has a number and sometimes does not, and a missing figure
        // reads as a missing reading rather than as a zero.
        let bt = &ctx.bluetooth;
        if bt.present {
            out.push(Reading {
                icon: match (bt.powered, bt.connected) {
                    (false, _) => ICON_BT_OFF,
                    (true, 0) => ICON_BT,
                    (true, _) => ICON_BT_CONNECTED,
                },
                value: bt.connected.to_string(),
                reserve: RESERVE_COUNT,
            });
        }
        if let Some(disk) = m.disk_usage_pct {
            out.push(Reading {
                icon: ICON_DISK,
                value: format!("{disk:.0}%"),
                reserve: RESERVE_PCT,
            });
        }
        out.push(Reading {
            icon: ICON_CPU,
            value: format!("{:.0}%", m.cpu_usage_pct),
            reserve: RESERVE_PCT,
        });
        out.push(Reading {
            icon: ICON_RAM,
            value: format!("{:.0}%", m.ram_usage_pct),
            reserve: RESERVE_PCT,
        });
        // Shown only when the Brain can actually read it: no Intel GPU, or a
        // kernel that will not hand out perf counters, means we cannot tell, and
        // a column that quietly means "cannot tell" is the surface lying (see
        // `options-engine`'s `collectors::gpu`).
        if let Some(gpu) = m.gpu_usage_pct {
            out.push(Reading {
                icon: ICON_GPU,
                value: format!("{gpu:.0}%"),
                reserve: RESERVE_PCT,
            });
        }
        if let Some(bat) = m.battery_pct {
            out.push(Reading {
                // Charging is the one thing a percentage cannot say on its own:
                // 40% climbing and 40% falling are different situations, so the
                // symbol carries it.
                icon: if m.is_charging {
                    ICON_CHARGING
                } else {
                    battery_glyph(bat)
                },
                value: format!("{bat}%"),
                reserve: RESERVE_PCT,
            });
        }
        out
    }

    /// Draw the readout: the pill itself, then a column per reading. Nothing
    /// while it rests behind the gear, which draws over it.
    pub(crate) fn push_stats_child(&self, scene: &mut Scene) {
        let t = self.stats.t;
        if t < 0.001 {
            return;
        }
        let rect = self.stats_geom();
        let s = self.options_scale();
        let readings = self.stats_readings();
        if readings.is_empty() {
            return;
        }
        // The text fades in on the BACK half of the slide, so it appears in a
        // pill that is already most of the way out instead of being squeezed
        // out of the gear (the clipboard's preview does the same).
        // Content fades in on the BACK HALF of the movement — the same split the
        // module box and the clipboard drawer use, so every box on this bar
        // reveals its contents at the same moment in its own morph.
        let a = ((t - 0.5) / 0.5).clamp(0.0, 1.0);
        if a < 0.01 {
            return;
        }
        let ink = self.options_text_color();
        // The couples are laid out one after another from the left margin, each
        // at its own width with `COL_GAP` between — the same arithmetic
        // `stats_full_w` sized the pill with, so the row lands centred by
        // construction rather than by dividing a width that may not fit. The
        // ends get `END_PAD` instead of the gap: they are bounded by the caps.
        let font = VALUE_PX * s;
        let line = LINE_PX * s;
        // ONE line per column. A label stacked over its value is the obvious
        // layout and it does not fit: the pill band is a bar-height minus its
        // margins, and two lines of readable type overflow it — the values came
        // out sliced in half on the first live look.
        // The readings stay in the BAND at the top, not centred in the rect —
        // the box grows downward beneath them and they must not drift into it
        // (Max, 2026-09-13: "the buttons stay on top").
        let band_h = self.options_pill_h();
        let ty = rect.y + (band_h - line) / 2.0;
        // The symbol is its own label so it can be its own size — one label
        // carries one font, and each symbol needs its own optical correction.
        // Every one is centred in a cell of the same width, so the distance from
        // symbol to number is identical in every couple.
        let gap = PAIR_GAP * s;
        let columns = self.stats_columns(rect);
        for (i, r) in readings.iter().enumerate() {
            let Some(col) = columns.get(i) else { break };
            let pair_x = col.x;
            let (icon_w, value_w) = self.stats_pair_w(i, r, s);
            let icon_px = ICON_PX * r.icon.scale * s;
            scene.labels.push(Label {
                text: r.icon.glyph.to_owned(),
                pos: (
                    pair_x + icon_w / 2.0,
                    rect.y + (band_h - icon_px * 1.2) / 2.0,
                ),
                // Room for the glyph at its own size, whatever the cell is —
                // the cell governs the LAYOUT, never the drawing, or an
                // enlarged symbol would be clipped by its own column.
                max_w: icon_px * 2.0,
                font_px: icon_px,
                line_px: icon_px * 1.2,
                centered: true,
                dim: false,
                cache: false,
                family: Some(NERD),
                color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
                clip: Some(rect),
            });
            if !r.value.is_empty() {
                scene.labels.push(Label {
                    // LEFT-aligned in the width it reserves, not centred in it.
                    // The reservation is the widest form the number can take, so
                    // centring spends the leftover on both sides — and half of it
                    // lands between the symbol and its own number, which is why
                    // "6%" sat 18px from its chip while "84%" sat 10px from its
                    // battery (measured, 2026-09-13). Left-aligned, the bond is
                    // exactly `PAIR_GAP` in every couple and the slack falls into
                    // the wide gap between couples, where nobody is comparing.
                    text: r.value.clone(),
                    pos: (pair_x + icon_w + gap, ty),
                    max_w: value_w + 1.0,
                    font_px: font,
                    line_px: line,
                    centered: false,
                    dim: false,
                    cache: false,
                    // The Nerd Font for the figures too: it is monospaced, so
                    // they are tabular and a column stops shifting sideways as
                    // its value changes under the pointer.
                    family: Some(NERD),
                    color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
                    clip: Some(rect),
                });
            }
        }
        let e = self.stats_open_t();
        // Page 1 is the gear's own, and it has its first real setting on it;
        // the rest still say only their number.
        if e > 0.01 && self.stats_page() == 1 {
            self.push_settings_page(scene, rect, ink, a * e);
            return;
        }
        // The page, under the row. A number for now — Max is choosing what each
        // page actually holds, and a number says "this is page 3 of the thing
        // you pressed" without pretending to be content.
        if e > 0.01 {
            let page_px = PAGE_PX * s;
            scene.labels.push(Label {
                text: self.stats_page().to_string(),
                pos: (rect.x + rect.w / 2.0, rect.y + band_h + (rect.h - band_h - page_px * 1.2) / 2.0),
                max_w: rect.w,
                font_px: page_px,
                line_px: page_px * 1.2,
                centered: true,
                dim: false,
                cache: false,
                family: Some(NERD),
                // Fades in with the panel it is written on, like every other
                // box's contents.
                color: Some([ink[0], ink[1], ink[2], ink[3] * a * e]),
                clip: Some(rect),
            });
        }
    }

    /// The rect of page 1's floating switch row — label on the left, switch on
    /// the right. The ONE source for its draw and its hit-test.
    fn stats_floating_row(&self, rect: Rect) -> Rect {
        let s = self.options_scale();
        let pad = SETTING_PAD * s;
        Rect::new(
            rect.x + pad,
            rect.y + self.options_pill_h() + pad,
            (rect.w - 2.0 * pad).max(0.0),
            SETTING_ROW_H * s,
        )
    }

    /// Page 1: the gear's own page, and Golem's settings.
    ///
    /// The switch is the SUNSET panel's switch — a stadium track with a knob
    /// that slides on, label to its left — not a second opinion about what a
    /// toggle looks like on this bar (`options-design-patterns`).
    fn push_settings_page(&self, scene: &mut Scene, rect: Rect, ink: [f32; 4], a: f32) {
        let s = self.options_scale();
        let row = self.stats_floating_row(rect);
        let line = LINE_PX * s;
        scene.labels.push(Label {
            text: "Floating windows".to_owned(),
            pos: (row.x, row.y + (row.h - line) / 2.0),
            max_w: row.w - SWITCH_W * s - 8.0 * s,
            font_px: SETTING_PX * s,
            line_px: line,
            centered: false,
            dim: false,
            cache: false,
            family: None,
            color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            clip: Some(rect),
        });
        let sw_w = SWITCH_W * s;
        let sw_h = SWITCH_H * s;
        let sw = Rect::new(
            row.x + row.w - sw_w,
            row.y + (row.h - sw_h) / 2.0,
            sw_w,
            sw_h,
        );
        let on = self.floating_mode();
        scene.rects.push(RectInst {
            rect: sw,
            radius: sw_h / 2.0,
            color: [ink[0], ink[1], ink[2], if on { 0.35 } else { 0.14 } * a],
            glass: 0.0,
            border: 0.0,
        });
        let knob_d = sw_h - 4.0 * s;
        let kx = if on {
            sw.x + sw.w - knob_d - 2.0 * s
        } else {
            sw.x + 2.0 * s
        };
        scene.rects.push(RectInst {
            rect: Rect::new(kx, sw.y + 2.0 * s, knob_d, knob_d),
            radius: knob_d / 2.0,
            color: [ink[0], ink[1], ink[2], ink[3] * a],
            glass: 0.0,
            border: 0.0,
        });
        // What the switch actually does, under it — this one reaches past the
        // shell and re-shapes the whole desktop, which is worth a sentence.
        scene.labels.push(Label {
            text: if on {
                "Every window floats, and new ones open floating.".to_owned()
            } else {
                "Windows tile. Golem places them.".to_owned()
            },
            pos: (row.x, row.y + row.h + 4.0 * s),
            max_w: row.w,
            font_px: 13.0 * s,
            line_px: 16.0 * s,
            centered: false,
            dim: false,
            cache: false,
            family: None,
            color: Some([ink[0], ink[1], ink[2], ink[3] * 0.55 * a]),
            clip: Some(rect),
        });
    }

    /// A press inside the open panel: page 1's switch is the only thing on any
    /// page that takes one. Returns whether it was consumed.
    pub(crate) fn stats_panel_click(&mut self, px: f32, py: f32) -> bool {
        if self.stats_open_t() < 0.5 || self.stats_page() != 1 {
            return false;
        }
        let row = self.stats_floating_row(self.stats_geom());
        // The whole ROW is the target, not just the little stadium: a switch
        // with a label is one control, and hunting for a 40px pill is not what
        // the row looks like it is asking for.
        if row.contains((px, py)) {
            self.toggle_floating_mode();
            return true;
        }
        false
    }

    /// One reading's measured `(symbol, number)` widths, with the rough guess
    /// that stands in until the first measuring pass lands — which only ever
    /// shows on the frame a reading first appears.
    fn stats_pair_w(&self, i: usize, r: &Reading, s: f32) -> (f32, f32) {
        self.stats.widths.get(i).copied().unwrap_or((
            ICON_PX * r.icon.scale * r.icon.ink * s,
            VALUE_PX * s * r.value.len() as f32 * 0.6,
        ))
    }

    /// Where each reading's couple sits inside the readout — the ONE source for
    /// the draw and for the hit-test, the way `stats_geom` is for the element as
    /// a whole. Two spellings of this arithmetic is how a button ends up
    /// somewhere other than where its symbol is drawn.
    fn stats_columns(&self, rect: Rect) -> Vec<Rect> {
        let s = self.options_scale();
        let band_h = self.options_pill_h();
        let readings = self.stats_readings();
        let mut out = Vec::with_capacity(readings.len());
        let mut pair_x = rect.x + END_PAD * s;
        for (i, r) in readings.iter().enumerate() {
            let (icon_w, value_w) = self.stats_pair_w(i, r, s);
            let w = pair_w(icon_w, value_w, s);
            out.push(Rect::new(pair_x, rect.y, w, band_h));
            pair_x += w + COL_GAP * s;
        }
        out
    }
}

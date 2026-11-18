//! The settings gear's stats child: CPU / RAM / DISK / BATTERY.
//!
//! Hovering the gear at the bar's left end slides a readout out from behind it —
//! **the clipboard's peek, mirrored** (Max, 2026-09-13: *"on hover, a child opens
//! and show CPU/RAM/DISK/BATTERY, same animation of clipboard"*). Same geometry
//! (`left + (ph + READING_GAP) * t`, width easing `ph → STATS_W`), the same
//! `MORPH_RATE` and `settle_t` the whole surface shares (`UXRules.md` §3),
//! and the same leave-hold before it retracts, so the two edges of the bar
//! behave identically.
//!
//! ONE STRIP OF SYMBOLS (Max, 2026-11-18, "C" of the mockup
//! https://claude.ai/artifact/WcfLMZ9odYsXr8Pkfk6Sgn: "c pero con los colores
//! reales"). The readings were each a pill of their own from 2026-11-05, then a
//! round pill with a state dot (2026-11-17); seven bubbles and seven dots read
//! as a load, so they share ONE pill again, `READING_GAP` after the gear: a slot
//! a reading (`stats_pills`), each SYMBOL tinted in how its reading stands
//! (`Signal`). The slots fan out from behind the first as the readout slides
//! out. Opening the box, the strip gives way to the box's one ground, which
//! grows down from that row as before (`push_stats_grounds`).
//!
//! The numbers come from the Brain's own `ContextState.metrics` — the daemon
//! reads no `/proc` of its own (method.md §1, "the Brain is the only sense").
//! A reading the engine does not have is simply not shown: a desktop with no
//! battery gets three readouts, not a fourth saying nothing.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};

use crate::animation::{ease_toward, lerp, settle_t, LEAVE_HOLD, MORPH_RATE, SETTLE_PX};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::gear::PageKind;
use crate::i18n::tr;
use crate::options::{push_neumorph, PillId, EDGE_PAD, LINE_PX, NERD, PILL_MARGIN_Y, SCROLL_DEADZONE};
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
/// The air at both ends of a reading's own pill, between its round caps and
/// its couple (a bar pill's words get `PILL_PAD_X`; a couple of a symbol and a
/// figure wants a little less).
const READING_PAD: f32 = 10.0;
/// THE OPEN BOX is narrower than the row of pills (Max, 2026-11-05: "for me it
/// is too wide"): opened, the readings are one band of the box's one ground, so
/// they drop their pills' air and stand this far apart, `OPEN_PAD` in from its
/// sides; the box is as wide as that band.
const OPEN_GAP: f32 = 10.0;
const OPEN_PAD: f32 = 12.0;

// --- THE SYMBOL AND ITS COLOUR (Max, 2026-11-17: "quiero que tengan colores
// (semáforo + azul)… y que en las píldoras se vea solo símbolo, la información
// solo en hover"; 2026-11-18: the colour on the symbol itself, the readings in
// one strip). Shut, a reading is a SLOT of the strip with its symbol, tinted in
// how it stands. Resting on the SYMBOL opens its slot to the right (its words
// after the symbol, the slots after it pushed along); leaving the symbol,
// either way, closes it.

/// Between the gear and the strip: wider than the power pills' bond (Max,
/// 2026-11-17: "separá un poco las pills, están muy juntas, se ve muy cargado
/// todo"; then "un poco más").
const READING_GAP: f32 = 10.0;
/// A slot shut is the pill's height and this much more (the mockup's 28 at a
/// 23 px pill): the symbols' pitch along the strip.
const SLOT_AIR: f32 = 5.0;
/// The strip's own air before its first slot and after its last.
const STRIP_PAD: f32 = 2.0;
/// An open slot's words begin this far past its symbol's ink.
const WORDS_GAP: f32 = 6.0;
/// How long the pointer rests on a symbol before its pill opens (Max's pick).
const OPEN_AFTER: Duration = Duration::from_millis(300);
/// Opening and closing tempos, 95 % of the way in 120 ms and in 80 ms (Max's
/// picks, off the mockup's sliders). A deliberate exception to law 3's one
/// tempo (`MORPH_RATE`, ~230 ms): a reading glanced at answers at once.
const OPEN_RATE: f32 = 3.0 / 0.120;
const CLOSE_RATE: f32 = 3.0 / 0.080;
/// A colour goes back to a better one only once its value is this many points
/// past the limit, so a reading that hovers round a limit does not blink.
const SETTLE_POINTS: f32 = 3.0;

/// How a reading stands, as its symbol's colour says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    /// Connected or plugged in.
    Blue,
    /// On, and fine.
    Green,
    /// Worth a look: a weak signal, a middling use, a battery running down.
    Yellow,
    /// Off, or at the limit.
    Red,
}

impl Signal {
    /// The traffic light's own colours, fixed whatever the palette (Max's
    /// "semáforo puro"): bright on a dark bar, deep on a light one. sRGB.
    fn srgb(self, bright: bool) -> [f32; 4] {
        let hex: u32 = match (self, bright) {
            (Signal::Blue, false) => 0x4aa3ff,
            (Signal::Green, false) => 0x35d06a,
            (Signal::Yellow, false) => 0xffd23f,
            (Signal::Red, false) => 0xff5b52,
            (Signal::Blue, true) => 0x0a5fd0,
            (Signal::Green, true) => 0x1b8a3c,
            (Signal::Yellow, true) => 0x9a5b00,
            (Signal::Red, true) => 0xd0021b,
        };
        let c = |shift: u32| ((hex >> shift) & 255) as f32 / 255.0;
        [c(16), c(8), c(0), 1.0]
    }
}

/// What a reading's colour is decided from.
#[derive(Debug, Clone, Copy)]
enum Measure {
    /// A state with no number: its colour as it is.
    Is(Signal),
    /// Nothing to say (a graphics card that does not tell): the bar's ink.
    Unknown,
    /// A number against its limits: `colors[n]` where `n` limits are passed —
    /// reached going up when `worse_up` (a use), gone under going down when
    /// not (a charge, a signal).
    Scale {
        value: f32,
        limits: &'static [f32],
        worse_up: bool,
        colors: &'static [Signal],
    },
}

const USE: [Signal; 3] = [Signal::Green, Signal::Yellow, Signal::Red];

/// The colour for `m`, given the one shown `before`: worse at once, better only
/// [`SETTLE_POINTS`] past the limit.
fn decide(m: Measure, before: Option<Signal>) -> Option<Signal> {
    match m {
        Measure::Is(s) => Some(s),
        Measure::Unknown => None,
        Measure::Scale { value, limits, worse_up, colors } => {
            let rank = |v: f32| limits.iter().filter(|&&l| if worse_up { v >= l } else { v < l }).count();
            let now = rank(value);
            let held = before.and_then(|b| colors.iter().position(|&c| c == b));
            let r = match held {
                Some(p) if now < p => {
                    let toward_worse = if worse_up { value + SETTLE_POINTS } else { value - SETTLE_POINTS };
                    rank(toward_worse).min(p)
                }
                _ => now,
            };
            colors.get(r.min(colors.len().saturating_sub(1))).copied()
        }
    }
}

/// Where a figure begins in the open box's band, from its symbol's centre:
/// past the symbol's ink by the couple's bond.
fn val_off(ink: f32, s: f32) -> f32 {
    ink / 2.0 + PAIR_GAP * s
}

/// Where an open slot's words begin, from its symbol's centre.
fn words_off(ink: f32, s: f32) -> f32 {
    ink / 2.0 + WORDS_GAP * s
}

/// The wash of a slot whose symbol is under the pointer: LIGHTER on any
/// ground, a dark bar's or a light one's.
fn stats_lit_wash(bright: bool) -> [f32; 4] {
    crate::options::wash(true, if bright { 0.66 } else { 0.32 })
}

/// A couple in the open box's band, from its symbol's left ink to the end of
/// its figure (or of its symbol, when it has none).
fn band_w(ink: f32, value_w: f32, s: f32) -> f32 {
    ink / 2.0 + if value_w > 0.0 { val_off(ink, s) + value_w } else { ink / 2.0 }
}

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
    /// [`PAIR_GAP`] and every couple centred in its own pill,
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
    /// The page this reading opens.
    kind: PageKind,
    icon: Icon,
    value: String,
    /// The widest form this number can take — what the layout RESERVES for it,
    /// rather than the current text. Empty means "as wide as it is".
    ///
    /// Otherwise the panel breathes: CPU crossing 9% → 10% is a digit wider, so
    /// the pill and every couple right of it would shift a few pixels every few
    /// seconds while the pointer rests on it.
    reserve: &'static str,
    /// What the pill says when it opens: the number, or a word where there is
    /// no number ("Not connected", "Off", the headphones' name). Empty: it
    /// does not open.
    hover: String,
    /// What its dot is decided from.
    measure: Measure,
}

/// Every percentage is reserved at its widest; so is a transfer rate, which
/// swings between nothing and whatever the link can carry, several times a
/// second.
const RESERVE_PCT: &str = "100%";
const RESERVE_RATE: &str = "99.9MB/s";
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
        // (Per second, said: THE KIT — a rate reads as one.)
        return format!("{}B/s", b as u64);
    }
    for (unit, scale) in [("KB", K), ("MB", K * K), ("GB", K * K * K)] {
        let scaled = b / scale;
        if scaled < K || unit == "GB" {
            // One decimal only while it buys precision: 1.4MB says more than
            // 1MB, 140MB says as much as 140.3MB and is a character shorter.
            return if scaled < 10.0 {
                format!("{scaled:.1}{unit}/s")
            } else {
                format!("{:.0}{unit}/s", scaled)
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
            "0B/s",
            "a live idle link truthfully reads zero"
        );
        assert_eq!(rate_text(Some(940)), "940B/s");
        assert_eq!(rate_text(Some(1024)), "1.0KB/s");
        assert_eq!(rate_text(Some(12_000)), "12KB/s");
        assert_eq!(
            rate_text(Some(1_500_000)),
            "1.4MB/s",
            "1024-based, like every other byte count"
        );
        assert_eq!(rate_text(Some(140_000_000)), "134MB/s");
        assert_eq!(rate_text(Some(2_000_000_000)), "1.9GB/s");
        // Nothing may outgrow the width the layout reserves for it.
        for bps in [0, 1, 999, 1024, 999_999, 50_000_000, 9_000_000_000] {
            assert!(
                rate_text(Some(bps)).len() <= RESERVE_RATE.len(),
                "{bps} rendered wider than the reserved column"
            );
        }
    }

    /// The dot by the table Max approved: a use goes green → yellow → red at its
    /// limits at once, and back only once it is SETTLE_POINTS under them.
    #[test]
    fn a_use_reddens_at_its_limit_and_greens_three_points_under() {
        let cpu = |v: f32| Measure::Scale { value: v, limits: &[50.0, 80.0], worse_up: true, colors: &USE };
        assert_eq!(decide(cpu(12.0), None), Some(Signal::Green));
        assert_eq!(decide(cpu(50.0), Some(Signal::Green)), Some(Signal::Yellow), "worse at once");
        assert_eq!(decide(cpu(80.0), Some(Signal::Yellow)), Some(Signal::Red));
        assert_eq!(decide(cpu(78.0), Some(Signal::Red)), Some(Signal::Red), "two points under: held");
        assert_eq!(decide(cpu(76.9), Some(Signal::Red)), Some(Signal::Yellow), "past three: back");
        assert_eq!(decide(cpu(10.0), Some(Signal::Red)), Some(Signal::Green), "far under: all the way");
        assert_eq!(decide(cpu(48.0), Some(Signal::Yellow)), Some(Signal::Yellow));
        assert_eq!(decide(cpu(46.0), Some(Signal::Yellow)), Some(Signal::Green));
    }

    /// A charge and a signal go the other way: worse as they fall.
    #[test]
    fn a_battery_and_a_signal_worsen_as_they_fall() {
        let bat = |v: f32| Measure::Scale { value: v, limits: &[40.0, 15.0], worse_up: false, colors: &USE };
        assert_eq!(decide(bat(84.0), None), Some(Signal::Green));
        assert_eq!(decide(bat(39.0), Some(Signal::Green)), Some(Signal::Yellow));
        assert_eq!(decide(bat(14.0), Some(Signal::Yellow)), Some(Signal::Red));
        assert_eq!(decide(bat(16.0), Some(Signal::Red)), Some(Signal::Red), "charging back: held");
        assert_eq!(decide(bat(18.0), Some(Signal::Red)), Some(Signal::Yellow));
        assert_eq!(decide(bat(43.0), Some(Signal::Yellow)), Some(Signal::Green));
        let wifi = |v: f32| Measure::Scale { value: v, limits: &[40.0], worse_up: false, colors: &[Signal::Blue, Signal::Yellow] };
        assert_eq!(decide(wifi(78.0), None), Some(Signal::Blue));
        assert_eq!(decide(wifi(30.0), Some(Signal::Blue)), Some(Signal::Yellow));
        assert_eq!(decide(wifi(41.0), Some(Signal::Yellow)), Some(Signal::Yellow));
        assert_eq!(decide(Measure::Unknown, Some(Signal::Green)), None, "nothing to say: no dot");
        assert_eq!(decide(Measure::Is(Signal::Red), Some(Signal::Blue)), Some(Signal::Red));
    }

    /// The words never start on the symbol, and a couple in the band is at
    /// least its symbol wide.
    #[test]
    fn the_words_start_after_the_symbol() {
        for s in [0.82, 1.0, 1.5] {
            for ink in [8.0, 11.0, 16.0] {
                let ink = ink * s;
                assert!(words_off(ink, s) >= ink / 2.0 + WORDS_GAP * s - 0.01, "on the symbol at ink {ink}");
                assert!(val_off(ink, s) >= ink / 2.0 + PAIR_GAP * s - 0.01);
                assert!(band_w(ink, 0.0, s) >= ink - 0.01);
                assert!(band_w(ink, 20.0, s) >= ink / 2.0 + val_off(ink, s) + 20.0 - 0.01);
            }
        }
        // Each colour reads apart from the others on both grounds.
        for bright in [false, true] {
            let all = [Signal::Blue, Signal::Green, Signal::Yellow, Signal::Red].map(|g| g.srgb(bright));
            for (i, a) in all.iter().enumerate() {
                for b in &all[i + 1..] {
                    assert!(a != b);
                }
            }
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
    /// Each reading's open words, measured (`measure_stats`), as `widths`.
    hover_w: Vec<f32>,
    /// The reading whose SYMBOL the pointer is on, and since when.
    sym: Option<(usize, Instant)>,
    /// The reading rested on long enough: its pill is open (or opening).
    opened: Option<usize>,
    /// How far each pill is open, 0 (its round symbol) → 1 (its words out).
    g: Vec<f32>,
    /// The dot each reading shows, kept so a better one waits
    /// [`SETTLE_POINTS`] (`measure_stats` decides them).
    signals: Vec<(PageKind, Option<Signal>)>,
    /// What the network pill names when it opens (`net::pill_facts`): the
    /// Wi-Fi network in use, and whether the radio is on — asked off the loop
    /// while the readout is out, again when the link changes or after
    /// `NET_ASK_AGAIN`.
    net_facts: Option<(Option<String>, Option<bool>)>,
    net_asked: Option<(String, Instant)>,
    net_busy: bool,
}

/// How long a network's name is taken as still true without the link changing.
const NET_ASK_AGAIN: Duration = Duration::from_secs(30);

impl App {
    /// The readout's rect: at rest exactly the gear's circle (hidden behind it),
    /// sliding right and widening as `t` rises. The single source for drawing
    /// and hit-testing, like `clip_geom`.
    pub(crate) fn stats_geom(&self) -> Rect {
        let ph = self.options_pill_h();
        let t = self.stats.t;
        let left = EDGE_PAD + (ph + READING_GAP * self.options_scale()) * t;
        // (Opened, it narrows to the readings' tight band: `stats_open_w`.)
        let w = lerp(lerp(ph, self.stats_full_w(), t), self.stats_open_w(), self.stats.e).max(ph);
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
        EDGE_PAD + (ph + READING_GAP * self.options_scale()) + self.stats_full_w_max()
    }

    /// The panel's full height: the bar's one drawer height. Empty for now —
    /// Max is choosing what goes in it.
    fn stats_box_h(&self) -> f32 {
        // A page with nothing to list (a radio switched off) is a shorter
        // box; the page eases that height itself.
        let page_h = self.gear.box_h();
        let h = if page_h > 0.0 {
            page_h
        } else {
            self.options_box_drawer_h()
        };
        h.max(self.options_pill_h())
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

    /// What the page numbered `page` is: the gear's own, or the page of the
    /// reading in that place (the numbers shift with what a machine has).
    pub(crate) fn stats_kind_of_page(&self, page: usize) -> PageKind {
        match page {
            0 | 1 => PageKind::Gear,
            n => self
                .stats_readings()
                .get(n - 2)
                .map_or(PageKind::Other, |r| r.kind),
        }
    }

    /// The number of the page of `kind`, if this machine has it.
    pub(crate) fn stats_page_for(&self, kind: PageKind) -> Option<usize> {
        if kind == PageKind::Gear {
            return Some(1);
        }
        self.stats_readings().iter().position(|r| r.kind == kind).map(|i| i + 2)
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
            // Already open: just turn the page. The page may need things of
            // its own (its worker, the keyboard), and the panel has to be
            // redrawn with its new contents.
            self.gear_sync();
            self.schedule_stats_frame();
            self.draw_options();
        } else {
            self.set_stats_box(true);
        }
    }

    /// How long a page opened from elsewhere waits for the hand.
    const ARRIVE_HOLD: Duration = Duration::from_secs(4);

    /// Show the page of `kind` from somewhere else on the shell (the control
    /// panel's "More…"): out and open, and held long enough for the hand to
    /// arrive. `false`: this machine has no such page.
    pub(crate) fn stats_show_page(&mut self, kind: PageKind) -> bool {
        let Some(page) = self.stats_page_for(kind) else {
            return false;
        };
        if !self.stats.reveal {
            self.stats.reveal = true;
            self.stats.last = None;
            self.measure_stats();
            self.schedule_stats_frame();
        }
        if !(self.stats.open && self.stats_page() == page) {
            self.stats_open_page(page);
        }
        self.schedule_stats_collapse(Self::ARRIVE_HOLD);
        true
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
        self.stats.sym = None;
        self.stats.opened = None;
        // The row has to be out for the panel to grow from it — opening from a
        // click on the gear while the readout is still tucked away would have
        // the panel appear from nothing.
        if open {
            self.stats.reveal = true;
            // (One page at a time on the engine: a page the control panel
            // hosts folds away as the box opens.)
            if self.gear_host.take().is_some() {
                self.panel_escape();
            }
        }
        self.stats.last = None;
        self.gear_sync();
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
        // A page with a list in it travels that list; it is put away by
        // leaving, by Escape, or by its own reading's button.
        if self.stats.open
            && self.stats_page_kind() != PageKind::Other
        {
            self.gear_axis(delta);
            return;
        }
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
        // Every reading's slot and the strip's air at its ends: the SAME
        // arithmetic `stats_pills` lays them out with.
        let n = self.stats.widths.len();
        let slots: f32 = (0..n).map(|i| self.stats_pill_w(i)).sum();
        slots + 2.0 * STRIP_PAD * s
    }

    /// The widest the row ever gets: every slot shut but the widest one open.
    fn stats_full_w_max(&self) -> f32 {
        let n = self.stats.widths.len();
        if n == 0 {
            return STATS_W * self.options_scale();
        }
        let slot = self.stats_slot_w();
        let extra = (0..n).map(|i| self.stats_open_pill_w(i) - slot).fold(0.0, f32::max);
        slot * n as f32 + 2.0 * STRIP_PAD * self.options_scale() + extra
    }

    /// A slot of the strip shut: its symbol's pitch.
    fn stats_slot_w(&self) -> f32 {
        self.options_pill_h() + SLOT_AIR * self.options_scale()
    }

    /// The open box's width: every couple (symbol and number), `OPEN_GAP`
    /// apart, `OPEN_PAD` in from both sides.
    pub(crate) fn stats_open_w(&self) -> f32 {
        let s = self.options_scale();
        if self.stats.widths.is_empty() {
            return self.stats_full_w();
        }
        let n = self.stats.widths.len() as f32;
        let couples: f32 = self.stats.widths.iter().map(|(i, v)| band_w(*i, *v, s)).sum();
        couples + (n - 1.0) * OPEN_GAP * s + 2.0 * OPEN_PAD * s
    }

    /// A reading's slot now: its symbol's pitch shut, its words after the
    /// symbol as it opens (`g`).
    fn stats_pill_w(&self, i: usize) -> f32 {
        lerp(self.stats_slot_w(), self.stats_open_pill_w(i), self.stats.g.get(i).copied().unwrap_or(0.0))
    }

    /// A reading's slot wide open: its symbol's half, the words after the
    /// symbol, `READING_PAD` after them.
    fn stats_open_pill_w(&self, i: usize) -> f32 {
        let s = self.options_scale();
        let slot = self.stats_slot_w();
        let ink = self.stats.widths.get(i).map_or(0.0, |w| w.0);
        let words = self.stats.hover_w.get(i).copied().unwrap_or(0.0);
        if words <= 0.0 {
            return slot;
        }
        (slot / 2.0 + words_off(ink, s) + words + READING_PAD * s).max(slot)
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
        self.stats_net_ask();
        let readings = self.stats_readings();
        // The colours, decided here (every snapshot while the readout is out, and
        // as it comes out): worse at once, better only past the limit.
        let signals: Vec<(PageKind, Option<Signal>)> = readings
            .iter()
            .map(|r| {
                let before = self.stats.signals.iter().find(|(k, _)| *k == r.kind).and_then(|(_, s)| *s);
                (r.kind, decide(r.measure, before))
            })
            .collect();
        self.stats.signals = signals;
        self.stats.g.resize(readings.len(), 0.0);
        if self.stats.opened.is_some_and(|i| i >= readings.len()) {
            self.stats.opened = None;
        }
        let templates: Vec<String> = readings.iter().map(|r| r.template().to_owned()).collect();
        // The open words: a number takes its reserved width (it must not
        // breathe while it is read), a word its own.
        let hovers: Vec<String> = readings
            .iter()
            .map(|r| if r.hover == r.value { r.template().to_owned() } else { r.hover.clone() })
            .collect();
        let inks: Vec<f32> = readings.iter().map(|r| ICON_PX * r.icon.scale * r.icon.ink).collect();
        let s = self.options_scale();
        let value_px = VALUE_PX * s;
        let Some(rd) = self.options_renderer.as_mut() else {
            return;
        };
        // Only the NUMBERS are measured; a symbol's width is its INK
        // ([`Icon::ink`]), never the font's advance — the advance varies by icon
        // set and has nothing to do with how wide the glyph looks, which is what
        // made the gaps wobble from column to column.
        self.stats.widths = templates
            .into_iter()
            .zip(inks)
            .map(|(value, ink)| (ink * s, if value.is_empty() { 0.0 } else { rd.measure_text(&value, value_px, Some(NERD)) }))
            .collect();
        self.stats.hover_w = hovers
            .iter()
            .map(|h| if h.is_empty() { 0.0 } else { rd.measure_text(h, value_px, Some(NERD)) })
            .collect();
    }

    /// Ask NetworkManager what the network pill should name, off the loop:
    /// while the readout is out, when the link changed or the answer is old.
    fn stats_net_ask(&mut self) {
        if !self.stats.reveal || self.stats.net_busy {
            return;
        }
        let Some(ctx) = self.brain.as_ref() else { return };
        let key = format!("{:?} {}", ctx.network.link, ctx.network.interface);
        if self.stats.net_asked.as_ref().is_some_and(|(k, at)| *k == key && at.elapsed() < NET_ASK_AGAIN) {
            return;
        }
        self.stats.net_asked = Some((key, Instant::now()));
        self.stats.net_busy = true;
        self.desktop_off_loop(crate::net::pill_facts, |app, facts| {
            app.stats.net_busy = false;
            if app.stats.net_facts.as_ref() != Some(&facts) {
                app.stats.net_facts = Some(facts);
                app.measure_stats();
                app.schedule_stats_frame();
            }
        });
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
        } else if self.stats.reveal && self.stats.hold_deadline.is_none() && !self.gear_pins_box() {
            // The panel folds with the row when the hand leaves, like every
            // other box on this bar — a visit ends when you go.
            self.schedule_stats_collapse(LEAVE_HOLD);
        }
        self.update_stats_symbol();
    }

    /// The reading whose SYMBOL — a pill's height square round it — is at
    /// `(x, y)`, while the row is out and the box shut. Its words, the air
    /// between symbols and the rest are not a symbol.
    fn stats_symbol_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.stats.e > 0.01 || self.stats.t < 0.99 {
            return None;
        }
        let ph = self.options_pill_h();
        let half = self.stats_slot_w() / 2.0;
        self.stats_pills(self.stats_geom())
            .iter()
            .position(|p| (x - (p.x + half)).abs() < ph / 2.0 && y >= p.y && y < p.y + ph)
    }

    /// Follow the pointer over the symbols (Max, 2026-11-17: "que la píldora se
    /// cierre cuando el puntero abandone el símbolo, no la píldora"): the one it
    /// is on lights at once; rested on [`OPEN_AFTER`], its pill opens; leaving
    /// the symbol, whichever way, closes it — onto its own number too.
    fn update_stats_symbol(&mut self) {
        let on = if self.options_hover == Some(PillId::SettingsStats) {
            self.options_ptr.and_then(|(x, y)| self.stats_symbol_at(x, y))
        } else {
            None
        };
        if on == self.stats.sym.map(|(i, _)| i) {
            return;
        }
        let now = Instant::now();
        self.stats.sym = on.map(|i| (i, now));
        self.stats.opened = None;
        if let Some(i) = on {
            let _ = self.loop_handle.insert_source(Timer::from_duration(OPEN_AFTER), move |_, _, app: &mut App| {
                // Only if the hand is still on that symbol, since then.
                if app.stats.sym == Some((i, now)) && app.stats.e < 0.01 {
                    app.stats.opened = Some(i);
                    app.schedule_stats_frame();
                }
                TimeoutAction::Drop
            });
        }
        self.schedule_stats_frame();
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
    /// The box and its row away now, as a hand that left would have them.
    pub(crate) fn stats_put_away(&mut self) {
        self.stats.hold_deadline = None;
        self.stats.sym = None;
        self.stats.opened = None;
        self.set_stats_box(false);
        self.stats.reveal = false;
        self.stats.last = None;
        self.schedule_stats_frame();
        self.sync_options_input();
    }

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
                    // A password half typed, a pairing under way: the page
                    // re-arms this itself when that is over.
                    if app.gear_pins_box() {
                        return TimeoutAction::Drop;
                    }
                    app.stats.reveal = false;
                    app.stats.open = false;
                    app.stats.scroll_accum = 0.0;
                    app.stats.sym = None;
                    app.stats.opened = None;
                    app.stats.last = None;
                    app.gear_sync();
                    app.schedule_stats_frame();
                    app.sync_options_input();
                }
                TimeoutAction::Drop
            });
    }

    pub(crate) fn schedule_stats_frame(&mut self) {
        // (A page hosted in the control panel moves with the dock's frames.)
        if self.gear_host.is_some() {
            self.schedule_frame();
        }
        self.schedule_tick(|a| (&mut a.stats.frame_pending, &mut a.stats.last), App::tick_stats);
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
        // Each pill opening or closing, at its own tempo.
        let ph = self.options_pill_h();
        for i in 0..self.stats.g.len() {
            let target = if self.stats.opened == Some(i) { 1.0 } else { 0.0 };
            let rate = if target > self.stats.g[i] { OPEN_RATE } else { CLOSE_RATE };
            let span = (self.stats_open_pill_w(i) - ph).max(1.0);
            let (g, m) = ease_toward(self.stats.g[i], target, dt, rate, settle_t(span));
            self.stats.g[i] = g;
            moving |= m;
        }
        // The pills moved under a pointer that did not: the symbol it is on
        // now is the one it rests on (the next one slid under it as the open
        // one closed).
        if moving {
            self.update_stats_symbol();
        }
        moving |= self.gear_tick(dt);
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
            // Blue while the link is up; yellow on a weak wireless signal.
            NetworkLink::Wireless => Reading {
                kind: PageKind::Net,
                icon: net.signal_pct.map_or(ICON_WIFI_NONE, wifi_glyph),
                value: rate_text(net.throughput_bps),
                reserve: RESERVE_RATE,
                // The network's name and its signal (Max, 2026-11-17: "quiero
                // que wifi muestre el nombre de la red"; 2026-11-18: "agrega el
                // porcentaje de signal al wifi"); the signal alone until the name
                // is in, the speed with neither.
                hover: match (&self.stats.net_facts, net.signal_pct) {
                    (Some((Some(name), _)), Some(p)) if !name.is_empty() => format!("{name} · {p}%"),
                    (Some((Some(name), _)), None) if !name.is_empty() => name.clone(),
                    (_, Some(p)) => format!("{p}%"),
                    _ => rate_text(net.throughput_bps),
                },
                measure: net.signal_pct.map_or(Measure::Is(Signal::Blue), |p| Measure::Scale {
                    value: f32::from(p),
                    limits: &[40.0],
                    worse_up: false,
                    colors: &[Signal::Blue, Signal::Yellow],
                }),
            },
            NetworkLink::Wired => Reading {
                kind: PageKind::Net,
                icon: ICON_WIRED,
                value: rate_text(net.throughput_bps),
                reserve: RESERVE_RATE,
                hover: tr("Wired").to_owned(),
                measure: Measure::Is(Signal::Blue),
            },
            NetworkLink::Down => Reading {
                kind: PageKind::Net,
                icon: ICON_NO_LINK,
                value: String::new(),
                reserve: "",
                // Off when the Wi-Fi radio is off; else disconnected.
                hover: match &self.stats.net_facts {
                    Some((_, Some(false))) => tr("Off").to_owned(),
                    _ => tr("Disconnected").to_owned(),
                },
                measure: Measure::Is(Signal::Red),
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
                kind: PageKind::Bt,
                icon: match (bt.powered, bt.connected) {
                    (false, _) => ICON_BT_OFF,
                    (true, 0) => ICON_BT,
                    (true, _) => ICON_BT_CONNECTED,
                },
                value: bt.connected.to_string(),
                reserve: RESERVE_COUNT,
                // Off, on, or what is connected, by its name.
                hover: match (bt.powered, bt.connected) {
                    (false, _) => tr("Off").to_owned(),
                    (true, 0) => tr("On").to_owned(),
                    (true, _) if bt.device.is_empty() => bt.connected.to_string(),
                    (true, 1) => bt.device.clone(),
                    (true, n) => format!("{} +{}", bt.device, n - 1),
                },
                measure: Measure::Is(match (bt.powered, bt.connected) {
                    (false, _) => Signal::Red,
                    (true, 0) => Signal::Green,
                    (true, _) => Signal::Blue,
                }),
            });
        }
        if let Some(disk) = m.disk_usage_pct {
            out.push(Reading {
                kind: PageKind::Disk,
                icon: ICON_DISK,
                value: format!("{disk:.0}%"),
                reserve: RESERVE_PCT,
                hover: format!("{disk:.0}%"),
                measure: Measure::Scale { value: disk, limits: &[75.0, 90.0], worse_up: true, colors: &USE },
            });
        }
        out.push(Reading {
            kind: PageKind::Cpu,
            icon: ICON_CPU,
            value: format!("{:.0}%", m.cpu_usage_pct),
            reserve: RESERVE_PCT,
            hover: format!("{:.0}%", m.cpu_usage_pct),
            measure: Measure::Scale { value: m.cpu_usage_pct, limits: &[50.0, 80.0], worse_up: true, colors: &USE },
        });
        out.push(Reading {
            kind: PageKind::Ram,
            icon: ICON_RAM,
            value: format!("{:.0}%", m.ram_usage_pct),
            reserve: RESERVE_PCT,
            hover: format!("{:.0}%", m.ram_usage_pct),
            measure: Measure::Scale { value: m.ram_usage_pct, limits: &[60.0, 85.0], worse_up: true, colors: &USE },
        });
        // Shown only when the Brain can actually read it: no Intel GPU, or a
        // kernel that will not hand out perf counters, means we cannot tell, and
        // a column that quietly means "cannot tell" is the surface lying (see
        // `options-engine`'s `collectors::gpu`).
        // The graphics page is on every machine (its Effects are the
        // look's, whatever the card): with no number, its symbol alone, as a
        // network that is down.
        out.push(match m.gpu_usage_pct {
            Some(gpu) => Reading {
                kind: PageKind::Gpu,
                icon: ICON_GPU,
                value: format!("{gpu:.0}%"),
                reserve: RESERVE_PCT,
                hover: format!("{gpu:.0}%"),
                measure: Measure::Scale { value: gpu, limits: &[50.0, 80.0], worse_up: true, colors: &USE },
            },
            None => Reading {
                kind: PageKind::Gpu,
                icon: ICON_GPU,
                value: String::new(),
                reserve: "",
                hover: tr("No reading").to_owned(),
                measure: Measure::Unknown,
            },
        });
        if let Some(bat) = m.battery_pct {
            out.push(Reading {
                kind: PageKind::Battery,
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
                hover: format!("{bat}%"),
                // Blue on the charger; else by the charge left.
                measure: if m.is_charging {
                    Measure::Is(Signal::Blue)
                } else {
                    Measure::Scale { value: f32::from(bat), limits: &[40.0, 15.0], worse_up: false, colors: &USE }
                },
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
        // The bar's ink on the bar; the box's as it opens under the row (the
        // pill's fill makes the same turn — a light box kept the bar's white).
        let ink = crate::animation::lerp4(self.options_text_color(), self.box_surface_at(rect).1, self.stats_open_t());
        // Each couple is centred in its own pill (`stats_columns` from
        // `stats_pills` — the same arithmetic `stats_full_w` sized the row
        // with).
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
        // Shut, it stands centred in its round pill and never moves as the
        // pill opens; open, it eases to its place in the box's band.
        let centres = self.stats_centres(rect);
        let pills = self.stats_pills(rect);
        // With the box open, the reading whose page is showing stays at full
        // strength and the others step back, so the row says which page this is.
        let open_e = self.stats.e.clamp(0.0, 1.0);
        let page = self.stats_page();
        let bright = self.options_bar_is_bright();
        // Which of the two colour sets reads on the ground: the bar's, or the
        // box's once it is more than half open (a light box has dark ink).
        let (_, box_ink) = self.box_surface_at(rect);
        let light_ground = if open_e < 0.5 { bright } else { box_ink[0] < 0.5 };
        for (i, r) in readings.iter().enumerate() {
            let (Some(&c), Some(pill)) = (centres.get(i), pills.get(i)) else { break };
            let a = if page == i + 2 { a } else { a * lerp(1.0, 0.6, open_e) };
            let ink_w = self.stats_ink(i, r, s);
            let icon_px = ICON_PX * r.icon.scale * s;
            // The symbol in how its reading stands; the bar's ink when there
            // is nothing to say.
            let signal = self.stats.signals.get(i).and_then(|(k, sig)| (*k == r.kind).then_some(*sig)).flatten();
            let tint = signal.map_or(ink, |sig| {
                let c = crate::kit::linear(sig.srgb(light_ground));
                [c[0], c[1], c[2], ink[3]]
            });
            scene.labels.push(Label {
                text: r.icon.glyph.to_owned(),
                pos: (c, rect.y + (band_h - icon_px * 1.2) / 2.0),
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
                color: Some([tint[0], tint[1], tint[2], tint[3] * a]),
                clip: Some(rect),
            });
            // Open, in its slot: what it says, uncovered as the slot grows.
            let g = self.stats.g.get(i).copied().unwrap_or(0.0);
            if g > 0.001 && open_e < 0.99 && !r.hover.is_empty() {
                scene.labels.push(Label {
                    text: r.hover.clone(),
                    pos: (c + words_off(ink_w, s), ty),
                    max_w: self.stats.hover_w.get(i).copied().unwrap_or(0.0) + 1.0,
                    font_px: font,
                    line_px: line,
                    centered: false,
                    dim: false,
                    cache: false,
                    family: Some(NERD),
                    color: Some([ink[0], ink[1], ink[2], ink[3] * a * g * (1.0 - open_e)]),
                    clip: Some(*pill),
                });
            }
            // In the open box's band: today's figure, as it always was —
            // LEFT-aligned in the width it reserves, so the slack falls into
            // the gap between couples where nobody is comparing.
            if !r.value.is_empty() && open_e > 0.001 {
                scene.labels.push(Label {
                    text: r.value.clone(),
                    pos: (c + val_off(ink_w, s), ty),
                    max_w: self.stats.widths.get(i).map_or(0.0, |w| w.1) + 1.0,
                    font_px: font,
                    line_px: line,
                    centered: false,
                    dim: false,
                    cache: false,
                    // The Nerd Font for the figures too: it is monospaced, so
                    // they are tabular and a column stops shifting sideways as
                    // its value changes under the pointer.
                    family: Some(NERD),
                    color: Some([ink[0], ink[1], ink[2], ink[3] * a * open_e]),
                    clip: Some(rect),
                });
            }
        }
        let e = self.stats_open_t();
        // The pages with real content draw themselves (`gear.rs`).
        if e > 0.01 && self.stats_page_kind() != PageKind::Other {
            self.push_gear_page(scene, a * e);
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

    /// A press inside the open panel: the page's own controls get it first.
    /// Returns whether it was consumed.
    pub(crate) fn stats_panel_click(&mut self, _px: f32, _py: f32) -> bool {
        self.gear_press()
    }

    /// One reading's symbol ink, with the rough guess that stands in until the
    /// first measuring pass lands.
    fn stats_ink(&self, i: usize, r: &Reading, s: f32) -> f32 {
        self.stats.widths.get(i).map_or(ICON_PX * r.icon.scale * r.icon.ink * s, |w| w.0)
    }

    /// Where each reading's SYMBOL is centred now: in its slot's first pitch
    /// shut, at its place in the box's tight band as the box opens.
    fn stats_centres(&self, rect: Rect) -> Vec<f32> {
        let s = self.options_scale();
        let e = self.stats.e.clamp(0.0, 1.0);
        let half = self.stats_slot_w() / 2.0;
        let readings = self.stats_readings();
        let mut open_x = rect.x + OPEN_PAD * s;
        self.stats_pills(rect)
            .into_iter()
            .zip(readings.iter().enumerate())
            .map(|(p, (i, r))| {
                let ink = self.stats_ink(i, r, s);
                let value_w = self.stats.widths.get(i).map_or(0.0, |w| w.1);
                let open = open_x + ink / 2.0;
                open_x += band_w(ink, value_w, s) + OPEN_GAP * s;
                lerp(p.x + half, open, e)
            })
            .collect()
    }

    /// Where each reading stands for the pointer: its pill shut, its couple in
    /// the band as the box opens — the ONE source the buttons are made from.
    fn stats_columns(&self, rect: Rect) -> Vec<Rect> {
        let s = self.options_scale();
        let e = self.stats.e.clamp(0.0, 1.0);
        let readings = self.stats_readings();
        let mut open_x = rect.x + OPEN_PAD * s;
        self.stats_pills(rect)
            .into_iter()
            .zip(readings.iter().enumerate())
            .map(|(p, (i, r))| {
                let ink = self.stats_ink(i, r, s);
                let w = band_w(ink, self.stats.widths.get(i).map_or(0.0, |w| w.1), s);
                let x = lerp(p.x, open_x, e);
                let col_w = lerp(p.w, w, e);
                open_x += w + OPEN_GAP * s;
                Rect::new(x, p.y, col_w, p.h)
            })
            .collect()
    }

    /// Where each reading's SLOT of the strip stands now: side by side after
    /// the strip's air, fanned out from behind the first as the readout slides
    /// out (`t`). A symbol's pitch shut; the one open is as long as its words,
    /// pushing the rest along.
    pub(crate) fn stats_pills(&self, rect: Rect) -> Vec<Rect> {
        let band_h = self.options_pill_h();
        let t = self.stats.t.clamp(0.0, 1.0);
        let n = self.stats_readings().len();
        let mut out = Vec::with_capacity(n);
        let mut offset = 0.0;
        let x0 = rect.x + STRIP_PAD * self.options_scale();
        let slot = self.stats_slot_w();
        for i in 0..n {
            let w = if self.stats.widths.is_empty() { slot } else { self.stats_pill_w(i) };
            out.push(Rect::new(x0 + offset * t, rect.y, w, band_h));
            offset += w;
        }
        out
    }

    /// The readout's grounds: the strip while the box is shut (the slot whose
    /// symbol is under the pointer lighter), and as the box opens the box's one
    /// ground growing in under them while the strip gives way to it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_stats_grounds(
        &self,
        scene: &mut Scene,
        rect: Rect,
        bright: bool,
        a: f32,
        shadow_a: f32,
        _hover_wash: [f32; 4],
        rest_wash: [f32; 4],
    ) {
        let e = self.stats_open_t().clamp(0.0, 1.0);
        // The strip, while the box is shut — gone a third of the way open.
        let own = (1.0 - e * 3.0).clamp(0.0, 1.0);
        if own > 0.001 {
            let band = Rect::new(rect.x, rect.y, rect.w, self.options_pill_h());
            let radius = band.h / 2.0;
            push_neumorph(scene, band, radius, bright, shadow_a * own);
            scene.rects.push(RectInst {
                rect: band,
                radius,
                color: [rest_wash[0], rest_wash[1], rest_wash[2], rest_wash[3] * a * own],
                glass: 0.0,
                border: 0.0,
            });
            // Lit is the slot whose SYMBOL the pointer is on, at once, lighter
            // on any ground (Max, 2026-11-17: "un efecto hover en cada hover,
            // la píldora más clara").
            if let Some(p) = self.stats.sym.and_then(|(i, _)| self.stats_pills(rect).get(i).copied()) {
                if e < 0.01 {
                    let wash = stats_lit_wash(bright);
                    scene.rects.push(RectInst {
                        rect: p,
                        radius: p.h / 2.0,
                        color: [wash[0], wash[1], wash[2], wash[3] * a * own],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
            }
        }
        // The box's ground, as it opens: the frosted surface every box wears,
        // in from nothing (the pills were the row).
        if e > 0.001 {
            let radius = self.stats_radius();
            push_neumorph(scene, rect, radius, bright, shadow_a * e);
            let (fill, _) = self.box_surface_at(rect);
            scene.rects.push(RectInst {
                rect,
                radius,
                color: [fill[0], fill[1], fill[2], self.box_panel_alpha() * e],
                glass: 0.0,
                border: 0.0,
            });
        }
    }
}

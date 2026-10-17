//! The main card's two doors on the dock: the Apps button, which opens it on
//! the apps grid (macOS's Launchpad), and Golem's **control panel** (its
//! name since 2026-09-30), which opens it as a field of settings — also on
//! Super+Ctrl (`waverunner-ctl control-panel`).
//!
//! OPTIONS offers the right control at the right moment, but it can't guess
//! every one (a screen resolution, a scale). The panel is the place to go for
//! the rest: a gear among the apps ([`crate::apps::CONTROL_PANEL_ID`]) that opens
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
//! - **The game: use brings a setting closer** (Max, 2026-09-30). Every use
//!   of a setting (a click, or Enter on a search) scores it, and old uses fade
//!   (a two-week half-life). When a pill outscores the weakest pill one layer
//!   nearer, the two swap: it steps one layer closer, the other steps back.
//!   The layers keep their sizes (5 · 8 · 11). The swap happens while the
//!   setting is open, so the field is rearranged when it folds back — nothing
//!   re-flows under the eye. The arrangement and the scores are saved
//!   (`control-panel.json`), starting from [`SETTINGS`]' order.
//! - **The first real settings: Scale and Resolution** (2026-10-01). Open,
//!   they show the screen's choices as floating pills, the one in use lit
//!   (the settings boxes' preset idiom); a click sets it live (see
//!   [`crate::display`]). The rest still show the placeholder controls.

use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use smithay_client_toolkit::seat::keyboard::Keysym;

use tracing::info;

use crate::content::{self, Label, Layout, Rect, RectInst, ShadowInst};
use crate::controls::{ControlCommand, ControlEvent, ControlView, UiFacts};
use crate::gear::PageKind;
use crate::display::DisplayView;
use crate::options::{FONT_PX, LINE_PX, PILL_PAD_X, TEXT_FONT};
use crate::state::Target;
use crate::{apps, groups, App};
use waverunner_proto::Command;
use crate::i18n::tr;

/// One pill of a field: its name, the layer it starts on (0 near, 1
/// middle, 2 far), and its search keywords.
type Item = (&'static str, u8, &'static str);

/// The settings and the layer each starts on (0 near, 1 middle, 2 far):
/// the ones people reach for float nearest. Placeholders until each becomes
/// a real setting.
const SETTINGS: [Item; 24] = [
    // (name, first layer, search keywords: what else people call it or
    // look for it by). 5 near, 9 middle, 11 far.
    ("Resolution", 0, "display screen monitor size pixels"),
    ("Scale", 0, "display screen zoom size hidpi text bigger smaller"),
    ("Wi-Fi", 0, "wifi wireless network internet connection"),
    ("Sound output", 0, "audio speakers speaker headphones output"),
    ("Colours", 0, "dark mode light theme appearance night colors colour color accent palette highlight dynamic websites web apps"),
    ("Brightness", 1, "display screen backlight dim"),
    ("Bluetooth", 1, "wireless devices headphones pairing"),
    ("Volume", 1, "audio sound loud quiet mute"),
    ("Wallpaper", 1, "background desktop picture image appearance"),
    ("Night light", 1, "display screen warm blue light sunset eyes"),
    // How the windows sit: where they open, how they stick, the side column…
    // (Max, 2026-10-14: "a control called Tiles… we will start dumping
    // controls in there").
    ("Tiles", 1, "windows tiling tile floating snap magnets magnetism layout arrange side column"),
    ("Notifications", 1, "alerts do not disturb dnd messages"),
    ("Keyboard layout", 1, "input language typing keys"),
    ("Power mode", 1, "battery performance energy saver"),
    ("Touchpad speed", 2, "trackpad mouse pointer cursor gestures"),
    ("Language", 2, "region locale translation input"),
    ("Time zone", 2, "clock date time region"),
    ("Natural scrolling", 2, "touchpad trackpad mouse scroll direction"),
    ("Updates", 2, "upgrade system software version"),
    ("About Golem", 2, "system version info hardware computer"),
    ("Microphone", 2, "audio input mic recording sound"),
    ("Battery", 2, "power charge energy"),
    ("Default apps", 2, "applications browser open with"),
    ("Privacy", 2, "security permissions camera location"),
];

/// The setting whose name, without spaces or capitals, is `name`.
pub(crate) fn setting_named(name: &str) -> Option<&'static str> {
    let flat = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    // (Dark mode was a page of its own until 2026-10-15: it is Colours' now.)
    if flat(name) == "darkmode" {
        return Some("Colours");
    }
    SETTINGS.iter().map(|s| s.0).find(|label| flat(label) == flat(name))
}

/// A pill's look on layer `layer` (0 near, 1 middle, 2 far) of a card that
/// reads bright or dark: its wash, and how strong its name is. For the
/// settings drawn as pills elsewhere (the quick controls' card).
pub(crate) fn pill_look(bright: bool, layer: usize) -> ([f32; 4], f32) {
    let wash = if bright { LAYER_WASH_BRIGHT } else { LAYER_WASH_DARK };
    (crate::options::wash(!bright, wash[layer.min(2)]), LAYER_INK[layer.min(2)])
}

/// A picture's preview in a setting (the wallpapers): its height for its
/// width, the gap between two, its corner, and the widest one gets.
const TILE_ASPECT: f32 = 0.625;
const TILE_GAP: f32 = 9.0;
/// The most pictures on one page of a wall; the room under the wall for its
/// pages' dots, and a dot's size.
/// (63 = seven rows of nine: Max, 2026-10-14, after a first 45: "agregá 18
/// más por página (dos líneas más)".)
pub(crate) const TILES_PER_PAGE: usize = 63;
const TILE_DOTS_H: f32 = 22.0;
const TILE_DOT: f32 = 6.0;
const TILE_RADIUS: f32 = 8.0;
const TILE_MAX_W: f32 = 420.0;
/// The palettes stand in two lines (the rest turn as pages).
const PALETTE_ROWS: usize = 2;
/// The quick Colours box: wide and short, a share of the screen's width.
const QUICK_COLOURS_W: f32 = 0.72;

/// The quick box (Super+H): its width, the room under its content for the
/// closing hint, and its least height — design pixels.
const QUICK_W: f32 = 620.0;
const QUICK_FOOT: f32 = 44.0;
const QUICK_MIN_H: f32 = 170.0;
/// The share of the screen a setting made of pictures takes.
const QUICK_WHOLE: f32 = 0.955;
/// How far its foot stays off the dock.
const QUICK_OFF_DOCK: f32 = 16.0;

/// How long the settings and the modules take to trade places.
const PANEL_SWAP_SECS: f32 = 0.30;

/// A whole field drawn `scale` times about `centre`, at `alpha`.
fn transform_draw(d: &mut PanelDraw, centre: (f32, f32), scale: f32, alpha: f32) {
    let still = (scale - 1.0).abs() < 0.0005;
    let at = |x: f32, y: f32| (centre.0 + (x - centre.0) * scale, centre.1 + (y - centre.1) * scale);
    let rect = |r: Rect| {
        let (x, y) = at(r.x, r.y);
        Rect::new(x, y, r.w * scale, r.h * scale)
    };
    let fix_rect = |r: &mut RectInst| {
        r.rect = rect(r.rect);
        r.radius *= scale;
        r.color[3] *= alpha;
    };
    let fix_label = |l: &mut Label| {
        l.pos = at(l.pos.0, l.pos.1);
        l.font_px *= scale;
        l.line_px *= scale;
        l.max_w *= scale;
        l.cache &= still;
        if let Some(c) = &mut l.color {
            c[3] *= alpha;
        }
        if let Some(c) = &mut l.clip {
            *c = rect(*c);
        }
    };
    d.rects.iter_mut().for_each(fix_rect);
    d.labels.iter_mut().for_each(fix_label);
    for g in &mut d.glows {
        g.rect = rect(g.rect);
        g.radius *= scale;
        g.color[3] *= alpha;
    }
    for (clip, rects, labels) in &mut d.clipped {
        *clip = rect(*clip);
        rects.iter_mut().for_each(fix_rect);
        labels.iter_mut().for_each(fix_label);
    }
}

// MODULES' pills come from the catalog (`modules.rs`, `assets/modules.json`).

// ---- An open module (Max's approved design, 2026-10-03) -------------------
/// A program's name in the list (px at bar scale 1; "a little bigger", Max 2026-10-03).
const MOD_NAME_PX: f32 = 20.0;
/// List pixels per wheel unit.
const MOD_WHEEL: f32 = 2.0;

/// An open module's switches and confirmation.
#[derive(Default)]
struct ModView {
    /// The programs switched on (catalog names).
    want: std::collections::BTreeSet<&'static str>,
    /// The button asked once: the next click confirms.
    sure: bool,
    /// The list's scroll, px from its top.
    scroll: f32,
}

/// What the one button at the foot says, by state.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ModButton {
    /// Nothing of the module in this computer: switch on our picks, then confirm.
    InstallRecommended,
    /// Some of it in, no change made: confirm, then everything goes.
    Uninstall,
    /// Switches flipped: confirm, then apply them.
    Apply,
    /// The confirmation (adding only).
    Confirm,
    /// The confirmation, something leaving (soft red).
    ConfirmRemove,
}

/// The small link under the button.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ModLink {
    Undo,
    KeepChoosing,
    KeepThem,
}

struct ModRow {
    prog: &'static crate::modules::Program,
    /// Its top inside the list's content.
    y: f32,
}

/// Where everything of an open module sits (field coordinates): drawing and
/// clicks read the same layout.
struct ModLayout {
    /// The visible list, cut halfway through a pill when there is more.
    list: Rect,
    max_scroll: f32,
    captions: Vec<(String, f32)>,
    rows: Vec<ModRow>,
    row_x: f32,
    row_w: f32,
    row_h: f32,
    button: Rect,
    button_text: String,
    kind: ModButton,
    sub: Vec<(String, f32)>,
    link: Option<(Rect, ModLink, &'static str)>,
    hint_y: f32,
    add: Vec<&'static crate::modules::Program>,
    remove: Vec<&'static crate::modules::Program>,
}

/// "A, B and C" — at most `n` names, then "and N more".
fn names(progs: &[&crate::modules::Program], n: usize) -> String {
    let shown: Vec<&str> = progs.iter().take(n).map(|p| p.name.as_str()).collect();
    let more = progs.len().saturating_sub(n);
    match (shown.len(), more) {
        (0, _) => String::new(),
        (1, 0) => shown[0].to_owned(),
        (_, 0) => crate::i18n::fill(tr("{0} and {1}"), &[("0", &(shown[..shown.len() - 1].join(", "))), ("1", &(shown[shown.len() - 1]))]),
        (_, m) => crate::i18n::fill(tr("{0} and {m} more"), &[("0", &(shown.join(", "))), ("m", &m)]),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { tr("program") } else { tr("programs") }
}

// ---- The layers --------------------------------------------------------
/// Size of each layer against the OPTIONS bar's own pill (the middle layer
/// IS the bar's pill).
pub(crate) const LAYER_SCALE: [f32; 3] = [1.53, 1.0, 0.8];
/// Resting wash alpha per layer, on a dark card (white wash) and a bright
/// one (black wash, which reads stronger at equal alpha).
pub(crate) const LAYER_WASH_DARK: [f32; 3] = [0.19, 0.10, 0.055];
pub(crate) const LAYER_WASH_BRIGHT: [f32; 3] = [0.16, 0.085, 0.045];
/// The OPTIONS hover wash, the same on every layer.
pub(crate) const HOVER_WASH_DARK: f32 = 0.27;
pub(crate) const HOVER_WASH_BRIGHT: f32 = 0.30;
/// Ink strength per layer.
pub(crate) const LAYER_INK: [f32; 3] = [1.0, 0.84, 0.62];
/// Soft glow per layer: (blur px, alpha). The far layer has none.
pub(crate) const LAYER_GLOW: [(f32, f32); 3] = [(7.0, 0.17), (3.5, 0.08), (0.0, 0.0)];
/// How "near" each layer is: sets the orbit radius.
const LAYER_Z: [f32; 3] = [1.0, 0.62, 0.35];

// ---- Layout (px at bar scale 1) -----------------------------------------
/// Air kept off the card's sides.
const MARGIN: f32 = 22.0;
/// The field's largest size (px at bar scale 1): the size it had on Max's
/// card when he set it as the maximum (2026-09-30: "i dont want it wider or
/// taller no matter how big the box gets"). Smaller cards shrink it.
const FIELD_MAX: (f32, f32) = (960.0, 845.0);
/// Air under the dock, and above the card's floor as a share of the field:
/// the heap sits up under the dock with more room below.
const GAP_TOP: f32 = 32.0;
const GAP_BOTTOM_FRAC: f32 = 0.21;

// ---- Motion (rates are 1/s for exponential approach) --------------------
const GLIDE_RATE: f32 = 24.0;
const SIZE_RATE: f32 = 26.0;
const FADE_IN_RATE: f32 = 22.0;
/// Seconds for a layer to leave from its end.
const EXIT_SECS: f32 = 0.10;
/// Wheel travel for one layer step, and the shortest gap between steps.
const WHEEL_STEP: f64 = 10.0;
const SHIFT_COOLDOWN: Duration = Duration::from_millis(150);
/// Hover: how fast a pill settles, and comes closer (and by how much).
const SETTLE_RATE: f32 = 6.0;
pub(crate) const LIFT_RATE: f32 = 12.0;
pub(crate) const LIFT: f32 = 0.07;
/// Search: how fast pills rise or sink as the query changes, how much a
/// miss shrinks, and how far it dims.
const SEARCH_RATE: f32 = 14.0;
const SEARCH_SHRINK: f32 = 0.15;
const SEARCH_DIM: f32 = 0.85;
/// The open-box morph's rate.
const OPEN_RATE: f32 = 14.0;

// ---- The game ------------------------------------------------------------
/// Uses fade: one counts half after this many days.
const USE_HALF_LIFE_DAYS: f64 = 14.0;
/// The score each layer starts with, so the first order has a little
/// weight: a far setting needs two uses to pass a middle one that is never
/// used, and three to reach the front. It fades like any use.
const SEED_SCORE: [f64; 3] = [2.0, 1.0, 0.0];

/// A setting's place in the game: its layer and its fading score.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Standing {
    layer: u8,
    score: f64,
    /// When `score` was last brought up to date (unix seconds).
    at: u64,
}

impl Standing {
    /// The score as of `now`, faded.
    fn score_at(&self, now: u64) -> f64 {
        let days = now.saturating_sub(self.at) as f64 / 86_400.0;
        self.score * 0.5f64.powf(days / USE_HALF_LIFE_DAYS)
    }
}

/// Every setting's standing, by name: the saved arrangement.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Usage {
    settings: BTreeMap<String, Standing>,
}

impl Usage {
    /// The saved layer of every item in `items`' order, if the save covers
    /// them all and keeps the layers' sizes; otherwise (first run, or the
    /// items changed) `None`.
    fn arrangement(&self, items: &[Item]) -> Option<Vec<u8>> {
        let layers: Vec<u8> = items
            .iter()
            .map(|(name, ..)| self.settings.get(*name).map(|st| st.layer).filter(|&l| l < 3))
            .collect::<Option<_>>()?;
        let count = |ls: &mut dyn Iterator<Item = u8>, l: u8| ls.filter(|&x| x == l).count();
        (0..3u8)
            .all(|l| count(&mut layers.iter().copied(), l) == count(&mut items.iter().map(|s| s.1), l))
            .then_some(layers)
    }
}

/// How well a setting matches a (lowercased, trimmed) search: 0 its name
/// starts with it, 1 its name contains it, 2 a keyword does; `None` no match.
fn match_rank(label: &'static str, keywords: &'static str, q: &str) -> Option<u8> {
    // Its name and keywords in English, and in the shell's language.
    let name = label.to_lowercase();
    let shown = crate::i18n::tr(label).to_lowercase();
    if name.starts_with(q) || shown.starts_with(q) {
        Some(0)
    } else if name.contains(q) || shown.contains(q) {
        Some(1)
    } else if keywords.contains(q) || crate::i18n::tr(keywords).contains(q) {
        Some(2)
    } else {
        None
    }
}

/// The most controls the apps grid's search shows in its row.
pub(crate) const MAX_SEARCH_CONTROLS: usize = 6;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

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
    /// Its layer in the arrangement (the game moves it); scrolling cycles
    /// the layers from here.
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
        // In English and in the shell's language: either finds it.
        self.label.to_lowercase().contains(query)
            || self.keywords.contains(query)
            || crate::i18n::tr(self.label).to_lowercase().contains(query)
            || crate::i18n::tr(self.keywords).contains(query)
    }
}

// ---- An open setting's choices ------------------------------------------
/// Where an open setting's controls start under its title and subtitle.
const CONTENT_TOP: f32 = 114.0;
/// A wall of pictures: where it starts (under a title and subtitle drawn
/// higher than a setting's), how far under it its hint is, and how far the
/// box's edge is under that.
const WALL_TOP: f32 = 92.0;
const WALL_HINT_GAP: f32 = 8.0;
const WALL_FOOT: f32 = 26.0;
/// A choice pill: its height, the gap to the next, and between rows.
const CHIP_H: f32 = 40.0;
const CHIP_GAP: f32 = 12.0;
const CHIP_ROW_GAP: f32 = 14.0;
/// Between one group of choices and the next caption.
const GROUP_GAP: f32 = 26.0;
/// The most resolutions shown (largest first; the one in use always is).
const MAX_SIZES: usize = 8;

/// What a click on an open setting asks for; `App` runs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PanelAction {
    Scale(f64),
    /// A resolution, at this refresh rate or the one nearest the current.
    Mode { w: u32, h: u32, hz: Option<f64> },
    /// Keep, or give up, the resolution being tried.
    Keep,
    Revert,
    /// A choice of one of the other settings ([`crate::controls`]): the
    /// group it is in and its place there.
    Pick { group: u8, index: u8 },
}

/// One choice of an open setting: a floating pill, lit when it is the one
/// in use.
struct Chip {
    /// At rest, in field coordinates.
    rect: Rect,
    text: String,
    on: bool,
    action: PanelAction,
    /// A picture's file: the choice is a preview of it, not a pill.
    image: Option<String>,
    /// A palette: its swatch's colours (ground, then its soul).
    swatch: Vec<[u8; 3]>,
    /// A stop of a slider: not drawn by itself (the slider is), only clicked.
    seg: bool,
    /// A palette's swatch (Dynamic's has no colours: it is drawn as a wheel).
    palette: bool,
    /// A colour cell of the palette editor (its colour = `swatch[0]`).
    cell: bool,
    /// How it is drawn in the hairline language.
    style: Style,
}

/// THE HAIRLINE LANGUAGE (Max, 2026-10-17, https://claude.ai/artifact/FmEgQWQLWxMxHQhpSDJmgb):
/// what a chip is drawn as.
#[derive(Clone, Copy, PartialEq)]
enum Style {
    /// One of several: a word, bright and underlined when it is the one in use
    /// (`big`: the box's main choice, bold, on the title's line).
    Word { big: bool },
    /// On or off: the whole row is the chip; a slim switch at its right.
    Switch { on: bool },
    /// An action: a hairline capsule (`main`: filled, one per box).
    Button { main: bool },
    /// A text field: the words over a hairline.
    Field,
}

/// A line of text the layout placed (a row's name, a slider's value).
struct Text {
    text: String,
    x: f32,
    y: f32,
    px: f32,
    alpha: f32,
    centered: bool,
}

/// A slider as drawn: its track, the lit stop's place on it (0..=1), and
/// which chips are its stops.
struct Slider {
    track: Rect,
    value: f32,
    stops: std::ops::Range<usize>,
}

/// A real setting's content at rest, in field coordinates: what
/// [`Panel::draw_open`] draws and a click is tested against.
struct OpenContent {
    subtitle: String,
    /// A line of small text over a group of choices, and its top.
    captions: Vec<(String, f32)>,
    chips: Vec<Chip>,
    /// Top of the closing hint.
    foot_y: f32,
    /// A wall of pictures in pages: how far the page that is up still has
    /// to slide (added to its tiles' x when drawn), the tiles of the page
    /// it is taking the place of with theirs, and the dots under it — their
    /// top, how many pages, which is up.
    slide: f32,
    ghosts: Vec<Chip>,
    ghost_slide: f32,
    dots: Option<(f32, usize, usize)>,
    sliders: Vec<Slider>,
    texts: Vec<Text>,
}

/// The pill that became the whole card.
struct OpenBox {
    pill: usize,
    /// 0 = the pill, 1 = the whole card; eases toward `want`.
    k: f32,
    want: f32,
    /// The pill's drawn rect when clicked (field-relative).
    from: Rect,
    /// A box of its own (solo) comes and goes as a WINDOW does: `pop` is
    /// how grown it is (0..1, sprung — it may pass 1 a little), `vel` that
    /// spring's speed, `shown` how far its fade has run (0..1, linear).
    pop: f32,
    vel: f32,
    shown: f32,
}

/// THE LEFT HAND (Max, 2026-10-13: "en todos los lugares que no haya type
/// to search se pueda navegar con las flechas pero también con A S W D,
/// E (enter) y Q (back)"): where no letter is ever typed, W A S D walk as
/// the arrows do, E is Enter and Q is the way back (Escape) — the mouse
/// hand stays on the mouse. Where letters ARE typed (a search, a name)
/// this is not used; the file windows take the same keys with Alt held.
pub(crate) fn left_hand(keysym: Keysym) -> Keysym {
    match keysym {
        Keysym::w | Keysym::W => Keysym::Up,
        Keysym::a | Keysym::A => Keysym::Left,
        Keysym::s | Keysym::S => Keysym::Down,
        Keysym::d | Keysym::D => Keysym::Right,
        Keysym::e | Keysym::E => Keysym::Return,
        Keysym::q | Keysym::Q => Keysym::Escape,
        other => other,
    }
}

/// THE COMPOSITOR'S OWN WINDOW MOTION, for a box that stands mid-screen
/// like a window (Max, 2026-10-13: "las animaciones de las cajas son
/// diferentes a las de hyprland. se ve raro. necesitamos que se vea
/// nativo, unificado"). The numbers are Golem's `hyprland.lua`, and must
/// follow it: a window pops in from 1 % of its size on the spring "easy"
/// (`windowsIn`) while it fades in over 173 ms (`fadeIn`); it pops out
/// straight in 149 ms (`windowsOut`) while it fades over 146 ms
/// (`fadeOut`); both fades on the curve "almostLinear".
const WINDOW_POP_FROM: f32 = 0.01;
const WINDOW_SPRING: (f32, f32) = (322.8714, 33.6891); // stiffness, dampening (mass 1)
const WINDOW_FADE_IN: f32 = 0.173;
const WINDOW_FADE_OUT: f32 = 0.146;
const WINDOW_POP_OUT: f32 = 0.149;
const ALMOST_LINEAR: [f32; 4] = [0.5, 0.5, 0.75, 1.0];

/// A CSS-style cubic bezier (from 0,0 to 1,1 through the two points of
/// `c`): its height where it is `x` along.
fn bezier(c: [f32; 4], x: f32) -> f32 {
    let at = |a: f32, b: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    let x = x.clamp(0.0, 1.0);
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..20 {
        let mid = (lo + hi) / 2.0;
        if at(c[0], c[2], mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(c[1], c[3], (lo + hi) / 2.0)
}

/// What the panel draws this frame, in surface coordinates. `content` puts
/// the rects and labels in a grid clipped to the card and the glows among
/// the overlay shadows.
#[derive(Debug, Default)]
pub(crate) struct PanelDraw {
    pub rects: Vec<RectInst>,
    pub labels: Vec<Label>,
    pub glows: Vec<ShadowInst>,
    /// Drawn clipped to their rect (an open module's scrolling list: the
    /// pill at its foot is cut in half, which is what says "more below").
    pub clipped: Vec<(Rect, Vec<RectInst>, Vec<Label>)>,
    /// Pictures to draw (a wallpaper's preview): where, and which file —
    /// whoever puts the panel on the screen finds each its texture.
    pub images: Vec<(Rect, String)>,
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
    /// What the field holds: the settings ([`SETTINGS`]) or the modules
    /// (the catalog's, `modules.rs`).
    items: &'static [Item],
    modules: bool,
    /// An open module: its switches and confirmation (MODULES only).
    mods: ModView,
    /// The catalog programs in this computer (lowercased names).
    installed: std::collections::HashSet<String>,
    /// Each program name's shaped width at the list's type size.
    name_w: std::collections::HashMap<&'static str, f32>,
    /// An Apply asked for: (module, to add, to remove); `App` takes it.
    pub(crate) pending_apply: Option<(&'static str, Vec<&'static crate::modules::Program>, Vec<&'static crate::modules::Program>)>,
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
    /// The game's standings, and where they are saved (`None`: not saved,
    /// as in tests).
    usage: Usage,
    store: Option<PathBuf>,
    /// A setting to open as soon as the pills exist (asked for from the
    /// apps grid's search before the panel was ever laid out).
    pending_open: Option<&'static str>,
    /// The pointer this frame, in surface coordinates.
    pointer: Option<(f32, f32)>,
    /// The screen an open display setting (Scale, Resolution) is about, as
    /// last read; `display_at` is when to read it again.
    display: Option<DisplayView>,
    display_at: Option<Instant>,
    /// Seconds a tried resolution still has before it goes back.
    keep_left: Option<f32>,
    /// What the last click asked for, until `App` takes it.
    action: Option<PanelAction>,
    /// The open setting as last read, when it is one of
    /// [`crate::controls`]'; `control_ask`: it wants reading.
    control: Option<ControlView>,
    control_ask: bool,
    /// ONE setting in a box of its own (the quick controls' box, Super+H):
    /// no field of pills, only the open setting.
    solo: bool,
    /// THE KEYBOARD in an open setting (Max, 2026-10-13: "manejar todo con
    /// el teclado"): the ONE cursor on its choices — which of the content's
    /// pills, in their drawn order. The pointer moving puts it under the
    /// pointer, a key moves it from there (`step`); `None`: nowhere.
    focus: Option<usize>,
    /// PAGES of pictures (the wallpapers past one page's worth): which page
    /// is up and its slide, the dock's box's own pager.
    tiles: crate::pager::Pager,
    /// The room a wall of pictures may take top to foot, where the box is
    /// made to hug it (the quick box): 0 = the field's own height.
    tile_room_h: f32,
    /// How wide a wallpaper's preview is in the wallpaper box: a palette's
    /// tile is made that size (Max, 2026-10-16). 0 = not known yet.
    pub(crate) wall_tile: f32,
    /// How many pictures the page as last laid out holds — its whole grid,
    /// every row full (`flow_tiles`).
    tile_cap: std::cell::Cell<usize>,
    /// The wall's height as last laid out: the box opens at it, before the
    /// setting has been read again.
    wall_h: Option<f32>,
}

impl Panel {
    /// A panel that only ever shows one open setting, in a box of its own
    /// (see `fast_launch.rs`, the quick controls). Its standings are the
    /// settings panel's: it keeps none.
    pub(crate) fn solo() -> Self {
        Self { items: &SETTINGS, solo: true, ..Self::default() }
    }

    /// Open `label` in the solo box, growing out of `from` (field-relative).
    pub(crate) fn open_solo(&mut self, label: &str, from: Rect) -> bool {
        let Some(i) = self.pills.iter().position(|p| p.label == label) else {
            return false;
        };
        self.open = Some(OpenBox { pill: i, k: 1.0, want: 1.0, from, pop: 0.0, vel: 0.0, shown: 0.0 });
        self.focus = None;
        self.tiles.reset();
        self.control = None;
        self.control_ask = crate::controls::is_control(self.pills[i].label);
        if is_display_setting(self.pills[i].label) {
            self.display_at = Some(Instant::now());
        }
        true
    }

    /// The open setting's choices as the keys walk them: (rect, lit, what a
    /// pick asks), in drawn order. Empty: nothing open, or nothing to pick.
    fn key_chips(&self) -> Vec<(Rect, bool, PanelAction)> {
        let Some(open) = self.open.as_ref().filter(|o| o.want > 0.5) else {
            return Vec::new();
        };
        if self.modules {
            return Vec::new();
        }
        self.open_content(self.pills[open.pill].label)
            .map(|c| c.chips.into_iter().map(|c| (c.rect, c.on, c.action)).collect())
            .unwrap_or_default()
    }

    /// Where the keys start: the choice in use, else the first.
    fn key_home(chips: &[(Rect, bool, PanelAction)]) -> usize {
        chips.iter().position(|c| c.1).unwrap_or(0)
    }

    /// An arrow: `(dx, dy)` one step. Sideways walks the choices in order
    /// (round the ends); up and down go to the nearest choice of the next
    /// row. Returns whether there was anything to move on.
    pub(crate) fn key_move(&mut self, dx: i32, dy: i32) -> bool {
        let chips = self.key_chips();
        if chips.is_empty() {
            return false;
        }
        let n = chips.len();
        let Some(at) = self.focus.map(|f| f.min(n - 1)) else {
            // The first key shows where the keys are; it moves nothing.
            self.focus = Some(Self::key_home(&chips));
            return true;
        };
        // Past either end of a page of pictures: the next page, the keys on
        // its first; the one before, on its last.
        if dx != 0 && self.tile_pages() > 1 && ((dx > 0 && at == n - 1) || (dx < 0 && at == 0)) {
            if self.tiles_turn(i64::from(dx.signum())) {
                let now = self.key_chips().len();
                self.focus = Some(if dx > 0 { 0 } else { now.saturating_sub(1) });
            }
            return true;
        }
        let to = if dx != 0 {
            (at as i32 + dx).rem_euclid(n as i32) as usize
        } else {
            let here = chips[at].0;
            let mid = |r: &Rect| r.x + r.w / 2.0;
            // The rows beyond this one, the nearest first.
            let mut rows: Vec<f32> = chips
                .iter()
                .map(|c| c.0.y)
                .filter(|y| if dy > 0 { *y > here.y + 1.0 } else { *y < here.y - 1.0 })
                .collect();
            rows.sort_by(|a, b| if dy > 0 { a.total_cmp(b) } else { b.total_cmp(a) });
            match rows.first() {
                Some(&row) => chips
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| (c.0.y - row).abs() < 1.0)
                    .min_by(|a, b| (mid(&a.1 .0) - mid(&here)).abs().total_cmp(&(mid(&b.1 .0) - mid(&here)).abs()))
                    .map_or(at, |(i, _)| i),
                None => at,
            }
        };
        self.focus = Some(to);
        true
    }

    /// Enter or Space: the choice the keys are on is picked (the one in use
    /// when no key has moved yet). Returns whether a choice was there.
    pub(crate) fn key_pick(&mut self) -> bool {
        let chips = self.key_chips();
        if chips.is_empty() {
            return false;
        }
        let at = self.focus.map_or_else(|| Self::key_home(&chips), |f| f.min(chips.len() - 1));
        self.focus = Some(at);
        self.action = Some(chips[at].2);
        true
    }

    /// The content changed under the keys (a setting chosen in steps grew a
    /// new group): they go to the newest group, on its choice in use.
    fn key_follow(&mut self, before: usize) {
        if self.focus.is_none() {
            return;
        }
        let chips = self.key_chips();
        if chips.is_empty() {
            self.focus = None;
        } else if chips.len() != before {
            let last_group = chips
                .iter()
                .filter_map(|c| if let PanelAction::Pick { group, .. } = c.2 { Some(group) } else { None })
                .max();
            let in_last = |c: &(Rect, bool, PanelAction)| matches!(c.2, PanelAction::Pick { group, .. } if Some(group) == last_group);
            let first = chips.iter().position(in_last).unwrap_or(0);
            self.focus = Some(chips.iter().enumerate().position(|(i, c)| i >= first && in_last(c) && c.1).unwrap_or(first));
        }
    }

    /// Fold the solo box away.
    pub(crate) fn close_solo(&mut self) {
        if let Some(open) = &mut self.open {
            open.want = 0.0;
        }
    }

    /// Whether a setting is (still) on show, and whether it is folding.
    pub(crate) fn solo_up(&self) -> bool {
        self.open.is_some()
    }

    /// Whether the open setting, as last read, shows pictures. (Not read
    /// yet: taken to — the wall is what it will be, and a box that opened
    /// small would jump.)
    pub(crate) fn open_has_tiles(&self) -> bool {
        self.control.as_ref().is_none_or(|v| v.groups.iter().any(|g| g.tiles && !g.choices.is_empty()))
    }

    /// Whether the open setting is `label`.
    pub(crate) fn open_is(&self, label: &str) -> bool {
        self.open.as_ref().is_some_and(|o| self.pills[o.pill].label == label)
    }

    /// The solo box this frame, as a window would be: how big (of its
    /// size at rest, about its centre) and how present (0..1).
    pub(crate) fn solo_pose(&self) -> (f32, f32) {
        self.open.as_ref().map_or((1.0, 0.0), |o| {
            (WINDOW_POP_FROM + (1.0 - WINDOW_POP_FROM) * o.pop, bezier(ALMOST_LINEAR, o.shown))
        })
    }

    /// How tall the open setting's content is, from the field's top.
    pub(crate) fn open_height(&self) -> Option<f32> {
        let label = self.pills[self.open.as_ref()?.pill].label;
        Some(self.open_content(label)?.foot_y)
    }

    /// How much a setting is used (the game's score, fading with time), as
    /// a whole number to rank by; and one more use of it.
    pub(crate) fn use_count(&self, label: &str) -> u32 {
        self.usage.settings.get(label).map_or(0, |st| (st.score_at(now_secs()) * 100.0) as u32)
    }

    pub(crate) fn note_use(&mut self, label: &'static str) {
        match self.pills.iter().position(|p| p.label == label) {
            Some(i) => self.record_use(i, now_secs()),
            None => {
                let now = now_secs();
                let st = self.usage.settings.entry(label.to_owned()).or_default();
                *st = Standing { layer: st.layer, score: st.score_at(now) + 1.0, at: now };
                if let Some(path) = &self.store {
                    crate::persist::write_json("control panel", path, &self.usage);
                }
            }
        }
    }

    /// The panel with its saved arrangement (`control-panel.json`).
    pub(crate) fn load() -> Self {
        Self::load_with(&SETTINGS, false, "control-panel.json")
    }

    /// The modules' field, with its own saved arrangement
    /// (`modules-panel.json`).
    pub(crate) fn load_modules() -> Self {
        Self::load_with(crate::modules::items(), true, "modules-panel.json")
    }

    fn load_with(items: &'static [Item], modules: bool, file: &str) -> Self {
        let path = crate::persist::data_path(file);
        Self {
            items,
            modules,
            usage: crate::persist::read_json(&path).unwrap_or_default(),
            store: Some(path),
            ..Self::default()
        }
    }

    /// Whether this is the modules' field.
    pub(crate) fn is_modules(&self) -> bool {
        self.modules
    }

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
            "control panel: field laid out for {:.0}x{:.0} at scale {:.2} (fresh {fresh})",
            key.0, key.1, key.2
        );
        self.key = key;
        self.scale = scale;
        if fresh {
            // The saved arrangement, or (first run, or the settings changed)
            // the first order, each setting seeded with its layer's score
            // (scores already earned are kept).
            let items = self.items;
            let groups = self.usage.arrangement(items).unwrap_or_else(|| {
                let now = now_secs();
                for &(name, layer, _) in items {
                    let st = self.usage.settings.entry(name.to_owned()).or_default();
                    let earned = st.score_at(now);
                    *st = Standing { layer, score: earned.max(SEED_SCORE[layer as usize]), at: now };
                }
                items.iter().map(|s| s.1).collect()
            });
            let mut rng = Rng(0x85EB_CA6B);
            self.pills = items
                .iter()
                .zip(groups)
                .map(|(&(label, _, keywords), group)| Pill {
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
                p.w[l] = (measure(crate::i18n::tr(p.label), FONT_PX * scale * f) + 2.0 * PILL_PAD_X * scale * f).max(h);
            }
        }
        if self.modules {
            self.name_w.clear();
            for m in &crate::modules::catalog().modules {
                for prog in m.groups.iter().flat_map(|g| &g.programs) {
                    self.name_w.insert(prog.name.as_str(), measure(&prog.name, MOD_NAME_PX * scale));
                }
            }
        }
        self.compose(size, true);
        if let Some(label) = self.pending_open.take() {
            self.open_named(label);
        }
    }

    /// The settings matching `query` (the apps grid's search), best first:
    /// name prefix, name, keyword; then nearest layer; at most
    /// [`MAX_SEARCH_CONTROLS`].
    pub(crate) fn matching_controls(&self, query: &str) -> Vec<&'static str> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let items = self.items;
        let groups: Vec<u8> = if self.pills.len() == items.len() {
            self.pills.iter().map(|p| p.group).collect()
        } else {
            self.usage.arrangement(items).unwrap_or_else(|| items.iter().map(|s| s.1).collect())
        };
        let mut hits: Vec<(u8, u8, usize)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, &(label, _, keywords))| Some((match_rank(label, keywords, &q)?, groups[i], i)))
            .collect();
        hits.sort();
        hits.into_iter().take(MAX_SEARCH_CONTROLS).map(|(.., i)| items[i].0).collect()
    }

    /// Open the setting named `label` (from the apps grid's search): now if
    /// the pills exist, else as soon as they do.
    pub(crate) fn open_named(&mut self, label: &'static str) {
        if self.pills.is_empty() {
            self.pending_open = Some(label);
            return;
        }
        if let Some(i) = self.pills.iter().position(|p| p.label == label) {
            if self.open.is_none() {
                self.open_pill(i);
            }
        }
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
        if self.modules {
            if let Some(m) = self.open.as_ref().filter(|o| o.want > 0.5).and_then(|o| crate::modules::module(self.pills[o.pill].label)) {
                let max = self.mod_layout(m).max_scroll;
                self.mods.scroll = (self.mods.scroll + value as f32 * MOD_WHEEL).clamp(0.0, max);
                return;
            }
        }
        let now = Instant::now();
        if self.wheel_at.is_none_or(|t| now - t > Duration::from_millis(250)) {
            self.wheel = 0.0;
        }
        self.wheel_at = Some(now);
        self.wheel += value;
        if self.wheel.abs() >= WHEEL_STEP && self.last_shift.is_none_or(|t| now - t > SHIFT_COOLDOWN) {
            // Inverted (Max, 2026-09-30): scrolling up brings the next layer
            // forward, the near one falling away; down goes back.
            self.shift_layers(if self.wheel > 0.0 { -1 } else { 1 });
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
        self.focus = None;
        if let Some((pill, want)) = self.open.as_ref().map(|o| (o.pill, o.want)) {
            // Open, the setting is the whole card: a click on its floating
            // controls is theirs; anywhere else folds it back.
            let label = self.pills[pill].label;
            if want > 0.5 && self.modules {
                if !self.mod_click(label, pos) {
                    if let Some(open) = &mut self.open {
                        open.want = 0.0;
                    }
                    info!("modules: closing {label}");
                }
                return true;
            }
            if want > 0.5 {
                match self.open_hit(label, pos) {
                    Some(action) => self.action = action,
                    None => {
                        if let Some(open) = &mut self.open {
                            open.want = 0.0;
                        }
                        info!("control panel: closing {label}");
                    }
                }
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
        info!("control panel: opening {}", p.label);
        self.focus = None;
        self.open = Some(OpenBox {
            pill: i,
            k: 0.0,
            want: 1.0,
            from: Rect::new(p.pos.0 - w / 2.0, p.pos.1 - h / 2.0, w, h),
            pop: 0.0,
            vel: 0.0,
            shown: 0.0,
        });
        self.hot = None;
        self.tiles.reset();
        if self.modules {
            let name = self.pills[i].label;
            self.mods = ModView::default();
            if let Some(m) = crate::modules::module(name) {
                self.mods.want = m.programs().filter(|p| self.installed.contains(&p.name.to_lowercase())).map(|p| p.name.as_str()).collect();
            }
        }
        if is_display_setting(self.pills[i].label) {
            self.display_at = Some(Instant::now());
        }
        if !self.modules && crate::controls::is_control(self.pills[i].label) {
            self.control = None;
            self.control_ask = true;
        }
        // The swap happens now, behind the setting growing over the field.
        self.record_use(i, now_secs());
    }

    /// Whether the open setting is one about the screen, and it is time to
    /// read the screen (again).
    fn display_due(&self) -> bool {
        self.display_at.is_some_and(|at| Instant::now() >= at)
            && self.open.as_ref().is_some_and(|o| is_display_setting(self.pills[o.pill].label))
    }

    /// The screen as just read.
    fn set_display(&mut self, view: Option<DisplayView>) {
        self.display = view;
        self.display_at = None;
    }

    /// Read the screen again at `at`: a change was asked for, and the
    /// compositor needs a moment to make it.
    pub(crate) fn display_refresh_at(&mut self, at: Instant) {
        self.display_at = Some(at);
    }

    /// A scale was just asked for: its pill is the lit one from now, before
    /// the screen has been read back (the read confirms or corrects it).
    pub(crate) fn display_expect_scale(&mut self, scale: f64) {
        if let Some(view) = &mut self.display {
            view.scale = scale;
        }
    }

    /// The language changed: the pills are measured and laid out again.
    pub(crate) fn relabel(&mut self) {
        self.key = (0.0, 0.0, 0.0);
    }

    /// The open setting, when it is one of [`crate::controls`]'.
    pub(crate) fn open_control_label(&self) -> Option<&'static str> {
        let label = self.pills[self.open.as_ref()?.pill].label;
        (!self.modules && crate::controls::is_control(label)).then_some(label)
    }

    /// The names of the settings that have real content, in the panel's
    /// order: what the quick controls search.
    pub(crate) fn real_settings() -> Vec<(&'static str, &'static str)> {
        SETTINGS
            .iter()
            .filter(|s| is_display_setting(s.0) || crate::controls::is_control(s.0))
            .map(|s| (s.0, s.2))
            .collect()
    }

    /// The open setting as last read.
    pub(crate) fn control_view(&self) -> Option<&ControlView> {
        self.control.as_ref()
    }

    /// The setting that wants reading, once.
    pub(crate) fn control_due(&mut self) -> Option<&'static str> {
        if !std::mem::take(&mut self.control_ask) {
            return None;
        }
        // A setting opened afresh starts on its own page (Colours: not its apps list).
        crate::controls::APPS_LIST.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut e) = crate::controls::EDITOR.lock() {
            *e = None;
        }
        if let Ok(mut t) = crate::controls::TYPING.lock() {
            *t = None;
        }
        crate::controls::field_typed("colors-apps-find", "", false);
        self.open_control_label()
    }

    /// The open setting as just read. A view of a setting that is no
    /// longer the open one is dropped.
    pub(crate) fn set_control(&mut self, view: ControlView) -> bool {
        if self.open_control_label() != Some(view.label) {
            return false;
        }
        let before = self.key_chips().len();
        self.control = Some(view);
        self.key_follow(before);
        true
    }

    /// The choice at (`group`, `index`) of the open setting: its setting,
    /// its group's key and its value. It is lit from now, before the
    /// machine has been read back (the read confirms or corrects it).
    pub(crate) fn control_pick(&mut self, group: u8, index: u8) -> Option<(&'static str, &'static str, String)> {
        let label = self.open_control_label()?;
        let view = self.control.as_mut().filter(|v| v.label == label)?;
        let g = view.groups.get_mut(group as usize)?;
        let value = g.choices.get(index as usize)?.value.clone();
        // One of several: the pick is the lit one. A lone pill is a button.
        if g.choices.len() > 1 {
            for (i, c) in g.choices.iter_mut().enumerate() {
                c.on = i == index as usize;
            }
        }
        Some((label, g.key, value))
    }

    /// A press at `from` that began on a SLIDER, now at `pos`: the stop
    /// under the pointer's x (the slider's own, wherever the pointer went
    /// up or down), when it is not the lit one.
    pub(crate) fn slide(&self, from: (f32, f32), pos: (f32, f32)) -> Option<(u8, u8)> {
        let label = self.open_control_label()?;
        let content = self.open_content(label)?;
        let f = self.field;
        let inside = |c: &Chip| Rect::new(f.x + c.rect.x, f.y + c.rect.y, c.rect.w, c.rect.h).contains(from);
        let sl = content.sliders.iter().find(|sl| content.chips[sl.stops.clone()].iter().any(inside))?;
        let stops = &content.chips[sl.stops.clone()];
        let n = stops.len().max(2) - 1;
        let k = (((pos.0 - f.x - sl.track.x) / sl.track.w).clamp(0.0, 1.0) * n as f32).round() as usize;
        let chip = stops.get(k)?;
        match chip.action {
            PanelAction::Pick { group, index } if !chip.on => Some((group, index)),
            _ => None,
        }
    }

    /// Whether a press at `pos` is on a slider (its whole row).
    pub(crate) fn on_slider(&self, pos: (f32, f32)) -> bool {
        let Some(label) = self.open_control_label() else { return false };
        let Some(content) = self.open_content(label) else { return false };
        let f = self.field;
        content.sliders.iter().any(|sl| {
            content.chips[sl.stops.clone()]
                .iter()
                .any(|c| Rect::new(f.x + c.rect.x, f.y + c.rect.y, c.rect.w, c.rect.h).contains(pos))
        })
    }

    /// What the last click asked for, once.
    fn take_action(&mut self) -> Option<PanelAction> {
        self.action.take()
    }

    /// Lay a group of choices out in rows of `w`-wide pills from `y` down,
    /// centred, wrapping to the field. Returns the top of what comes next.
    fn flow(&self, out: &mut OpenContent, y: f32, w: f32, chips: Vec<(String, bool, PanelAction)>) -> f32 {
        if chips.is_empty() {
            return y;
        }
        let s = self.scale;
        let fw = self.key.0;
        let (w, h, gap, row_gap) = (w * s, CHIP_H * s, CHIP_GAP * s, CHIP_ROW_GAP * s);
        let room = (fw - 2.0 * MARGIN * s).max(w);
        let fit = (((room + gap) / (w + gap)).floor() as usize).max(1);
        let total = chips.len();
        // When they wrap, the rows share them evenly (seven as 4 + 3, not
        // 6 + 1).
        let rows = total.div_ceil(fit);
        let per_row = total.div_ceil(rows);
        for (i, (text, on, action)) in chips.into_iter().enumerate() {
            let (row, col) = (i / per_row, i % per_row);
            // Each row is centred by itself: the last may be short.
            let in_row = (total - row * per_row).min(per_row);
            let row_w = in_row as f32 * w + (in_row - 1) as f32 * gap;
            let x = (fw - row_w) / 2.0 + col as f32 * (w + gap);
            out.chips.push(Chip { rect: Rect::new(x, y + row as f32 * (h + row_gap), w, h), text, on, action, image: None, swatch: Vec::new(), seg: false, palette: false, cell: false, style: Style::Word { big: false } });
        }
        y + rows as f32 * h + (rows - 1) as f32 * row_gap + GROUP_GAP * s
    }

    /// Lay WORDS out from `y` down, centred, wrapping: each as wide as its
    /// letters (`big`: the main choice's size).
    fn flow_words(&self, out: &mut OpenContent, y: f32, chips: Vec<(String, bool, PanelAction)>, big: bool) -> f32 {
        if chips.is_empty() {
            return y;
        }
        let s = self.scale;
        let fw = self.key.0;
        let (letter, gap, h) = if big { (8.6, 28.0, 30.0) } else { (7.4, 22.0, 26.0) };
        let (letter, gap, h, row_gap) = (letter * s, gap * s, h * s, 8.0 * s);
        let room = fw - 4.0 * MARGIN * s;
        let widths: Vec<f32> = chips.iter().map(|(t, _, _)| t.chars().count().max(1) as f32 * letter + 4.0 * s).collect();
        // Rows as full as they go.
        let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
        let mut used = 0.0;
        for (i, w) in widths.iter().enumerate() {
            let need = if rows.last().is_some_and(|r| r.is_empty()) { *w } else { used + gap + *w };
            if need > room && !rows.last().is_some_and(|r| r.is_empty()) {
                rows.push(Vec::new());
                used = *w;
            } else {
                used = need;
            }
            if let Some(r) = rows.last_mut() {
                r.push(i);
            }
        }
        let mut chips: Vec<Option<(String, bool, PanelAction)>> = chips.into_iter().map(Some).collect();
        for (ri, row) in rows.iter().enumerate() {
            let row_w: f32 = row.iter().map(|&i| widths[i]).sum::<f32>() + gap * (row.len().saturating_sub(1)) as f32;
            let mut x = (fw - row_w) / 2.0;
            for &i in row {
                if let Some((text, on, action)) = chips[i].take() {
                    out.chips.push(Chip {
                        rect: Rect::new(x - gap / 2.0, y + ri as f32 * (h + row_gap), widths[i] + gap, h),
                        text,
                        on,
                        action,
                        image: None,
                        swatch: Vec::new(),
                        seg: false,
                        palette: false,
                        cell: false,
                        style: Style::Word { big },
                    });
                }
                x += widths[i] + gap;
            }
        }
        y + rows.len() as f32 * h + (rows.len() - 1) as f32 * row_gap + GROUP_GAP * s
    }

    /// A SWITCH ROW from `y`: the name at the left, the switch at the
    /// right; the whole row turns it.
    fn switch_row(&self, out: &mut OpenContent, y: f32, group: u8, name: &str, on: bool) -> f32 {
        let s = self.scale;
        let fw = self.key.0;
        let (left, right, h) = (2.0 * MARGIN * s, fw - 2.0 * MARGIN * s, 34.0 * s);
        out.texts.push(Text { text: name.to_owned(), x: left, y: y + (h - 18.0 * s) / 2.0, px: 14.0, alpha: 0.85, centered: false });
        out.chips.push(Chip {
            rect: Rect::new(left - 6.0 * s, y, right - left + 12.0 * s, h),
            text: String::new(),
            on,
            // (A click picks the other one: On ↔ Off.)
            action: PanelAction::Pick { group, index: if on { 1 } else { 0 } },
            image: None,
            swatch: Vec::new(),
            seg: false,
            palette: false,
            cell: false,
            style: Style::Switch { on },
        });
        y + h + 10.0 * s
    }

    /// Lay COLOUR CELLS out from `y` down, `cols` to a row, square, across
    /// the field: the palette editor's field and its hue strip.
    fn flow_cells(&self, out: &mut OpenContent, y: f32, group: u8, cols: usize, cells: &[crate::controls::Choice]) -> f32 {
        let s = self.scale;
        let fw = self.key.0;
        let room = fw - 4.0 * MARGIN * s;
        let gap = 3.0 * s;
        let w = ((room - (cols - 1) as f32 * gap) / cols as f32).min(40.0 * s);
        // Squat cells: the editor must fit a 900 px screen.
        let h = if cells.len() <= cols { (w * 0.6).min(18.0 * s) } else { w.min(24.0 * s) };
        let x0 = (fw - cols as f32 * w - (cols - 1) as f32 * gap) / 2.0;
        for (i, c) in cells.iter().enumerate() {
            let (row, col) = (i / cols, i % cols);
            out.chips.push(Chip {
                rect: Rect::new(x0 + col as f32 * (w + gap), y + row as f32 * (h + gap), w, h),
                text: String::new(),
                on: c.on,
                action: PanelAction::Pick { group, index: i as u8 },
                image: None,
                swatch: c.swatch.clone(),
                seg: false,
                palette: false,
                cell: true,
                style: Style::Word { big: false },
            });
        }
        let rows = cells.len().div_ceil(cols);
        y + rows as f32 * h + (rows.saturating_sub(1)) as f32 * gap + GROUP_GAP * s * 0.7
    }

    /// Lay a SLIDER out from `y` down, across the field, with its own pair
    /// of pills (`pair`, the group laid beside it) at its right end. Its
    /// stops are chips that are not drawn: a click lands on the nearest, and
    /// the keys walk them a stop at a time.
    fn flow_slider(
        &self,
        out: &mut OpenContent,
        y: f32,
        group: u8,
        stops: &[crate::controls::Choice],
        pair: Option<(u8, &crate::controls::Group)>,
        (name, value): (&str, &str),
    ) -> f32 {
        let s = self.scale;
        let fw = self.key.0;
        let h = 30.0 * s;
        let (mut left, mut right) = (MARGIN * 2.0 * s, fw - MARGIN * 2.0 * s);
        // The row's name, at the left.
        out.texts.push(Text { text: name.to_owned(), x: left, y: y + (h - 18.0 * s) / 2.0, px: 13.5, alpha: 0.6, centered: false });
        left += 74.0 * s;
        if let Some((pg, g)) = pair {
            // Its look: small words at the right.
            let letter = 7.0 * s;
            let gap = 12.0 * s;
            let widths: Vec<f32> = g.choices.iter().map(|c| c.text.chars().count().max(1) as f32 * letter + 4.0 * s).collect();
            let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len().saturating_sub(1)) as f32;
            let x0 = right - total;
            let mut x = x0;
            for (ci, c) in g.choices.iter().enumerate() {
                let w = widths[ci];
                out.chips.push(Chip {
                    rect: Rect::new(x - gap / 2.0, y + 2.0 * s, w + gap, h - 4.0 * s),
                    text: c.text.clone(),
                    on: c.on,
                    action: PanelAction::Pick { group: pg, index: ci as u8 },
                    image: None,
                    swatch: Vec::new(),
                    seg: false,
                    palette: false,
                    cell: false,
                    style: Style::Word { big: false },
                });
                x += w + gap;
            }
            right = x0 - 18.0 * s;
        }
        // Its value, in words, before the look.
        out.texts.push(Text { text: value.to_owned(), x: right - 36.0 * s, y: y + (h - 16.0 * s) / 2.0, px: 12.0, alpha: 0.55, centered: true });
        right -= 84.0 * s;
        let n = stops.len().max(2);
        let track = Rect::new(left, y + h / 2.0 - 1.0 * s, (right - left).max(40.0 * s), 2.0 * s);
        let step = track.w / (n - 1) as f32;
        let first = out.chips.len();
        let lit = stops.iter().position(|c| c.on).unwrap_or(0);
        for (ci, c) in stops.iter().enumerate() {
            // Each stop takes its share of the WHOLE row — up into the caption
            // and down to the next row, the ends out to the field's edge and
            // to the look pair — so a click near the slider catches it
            // instead of landing on empty space (Max, 2026-10-16: "i miss and
            // close the box all the time").
            let x0 = if ci == 0 { track.x - 12.0 * s } else { track.x + (ci as f32 - 0.5) * step };
            let x1 = if ci + 1 == stops.len() { right + 14.0 * s } else { track.x + (ci as f32 + 0.5) * step };
            let (top, bottom) = (y - 6.0 * s, y + h + 6.0 * s);
            out.chips.push(Chip {
                rect: Rect::new(x0, top, x1 - x0, bottom - top),
                text: String::new(),
                on: c.on,
                action: PanelAction::Pick { group, index: ci as u8 },
                image: None,
                swatch: Vec::new(),
                seg: true,
                palette: false,
                cell: false,
                style: Style::Word { big: false },
            });
        }
        out.sliders.push(Slider { track, value: lit as f32 / (n - 1) as f32, stops: first..out.chips.len() });
        y + h + 10.0 * s
    }

    /// Lay PICTURES out as a grid of previews from `y` down: as many columns
    /// as make them the biggest they can be with a page of them in the box.
    /// A PAGE is at most [`TILES_PER_PAGE`]; past that the rest are on
    /// further pages to the right, turned as the dock's box's are (Max,
    /// 2026-10-14: "que llegue a 45 y de ahí que aparezca otra página a la
    /// derecha, por desborde, como hacemos en la caja del dock"). Returns
    /// the top of what comes next — the same on every page.
    fn flow_tiles(&self, out: &mut OpenContent, y: f32, group: u8, choices: &[crate::controls::Choice]) -> f32 {
        if choices.is_empty() {
            return y;
        }
        let s = self.scale;
        let fw = self.key.0;
        let gap = TILE_GAP * s;
        let room_w = (fw - 2.0 * MARGIN * s).max(100.0);
        // The dots' room is kept whether or not there are pages: the size
        // of the previews must not depend on it.
        let dots_h = TILE_DOTS_H * s;
        // The room top to foot: the box's own height (a box made to hug the
        // wall says how much it may take), less what is above the grid, the
        // dots and the closing hint under it.
        let whole = if self.tile_room_h > 0.0 { self.tile_room_h } else { self.field.h.max(self.key.1) };
        let room_h = (whole - y - 40.0 * s - dots_h).max(TILE_ASPECT * 80.0 * s);
        // The biggest previews whose grid — as many rows as fit, each one
        // FULL — holds them all, or a page's worth ([`TILES_PER_PAGE`]) when
        // there are more. A page holds its whole grid, so every page but the
        // last is full to the last row (Max, 2026-10-14: "lo que yo quiero es
        // que todas las filas estén llenas … la página siguiente tiene que
        // estar por si se desborda la primera" — 63 to a page on a grid of
        // eight left the eighth row with seven, and the rest on page two).
        let want = choices.len().min(TILES_PER_PAGE);
        let mut best: Option<(usize, usize, f32)> = None;
        let swatches = choices.iter().any(|c| !c.swatch.is_empty());
        for cols in 2..=12usize {
            let w = ((room_w - (cols - 1) as f32 * gap) / cols as f32).min(TILE_MAX_W * s);
            let h = w * TILE_ASPECT;
            let rows = (((room_h + gap) / (h + gap)).floor() as usize).max(1);
            if cols * rows >= want && best.is_none_or(|(_, _, bw)| w > bw) {
                best = Some((cols, rows, w));
            }
        }
        // PALETTES AS BIG AS THE WALLPAPERS' PREVIEWS (Max, 2026-10-16: "the
        // palette rectangles, not the whole box"): the wallpaper box's tile
        // width, in whatever box this is — as many across as that fits.
        // Two lines of them (Max, 2026-10-16: "i want two lines … a horizontal
        // rectangle"), the rest on pages.
        let best = if swatches && self.wall_tile > 0.0 {
            Some(self.wall_tile.min(room_w)).map(|w| {
                let cols = (((room_w + gap) / (w + gap)).floor() as usize).max(1);
                (cols, PALETTE_ROWS, w)
            })
        } else {
            best
        };
        // (Too many for the box at any size: the smallest, a page of them.)
        let (cols, rows_fit, w) = best.unwrap_or_else(|| {
            let w = (room_w - 11.0 * gap) / 12.0;
            (12, (((room_h + gap) / (w * TILE_ASPECT + gap)).floor() as usize).max(1), w)
        });
        let h = w * TILE_ASPECT;
        let cap = (cols * rows_fit).max(1);
        self.tile_cap.set(cap);
        let pages = choices.len().div_ceil(cap);
        // The grid is as tall as the fullest page.
        let rows = choices.len().min(cap).div_ceil(cols);
        let page_of = |page: usize| -> Vec<Chip> {
            let first = page * cap;
            let on_page = choices.len().saturating_sub(first).min(cap);
            (0..on_page)
                .map(|k| {
                    let (row, col) = (k / cols, k % cols);
                    let in_row = (on_page - row * cols).min(cols);
                    let row_w = in_row as f32 * w + (in_row - 1) as f32 * gap;
                    let x = (fw - row_w) / 2.0 + col as f32 * (w + gap);
                    let c = &choices[first + k];
                    Chip {
                        rect: Rect::new(x, y + row as f32 * (h + gap), w, h),
                        text: c.text.clone(),
                        on: c.on,
                        action: PanelAction::Pick { group, index: (first + k) as u8 },
                        // A palette's card is a picture too, once drawn.
                        image: if swatches {
                            let card = if c.value == crate::palette::DYNAMIC_ID {
                                crate::palette_card::wheel(crate::look::dark())
                            } else {
                                crate::palette_card::path(&c.swatch).filter(|p| !c.swatch.is_empty() && p.is_file())
                            };
                            card.map(|p| p.to_string_lossy().into_owned())
                        } else {
                            Some(c.value.clone())
                        },
                        swatch: c.swatch.clone(),
                        seg: false,
                        palette: swatches,
                        cell: false,
                        style: Style::Word { big: false },
                    }
                })
                .collect()
        };
        let page = self.tiles.page(fw).min(pages - 1);
        out.chips.extend(page_of(page));
        // Mid-turn: the page slides in from its side, the one it replaces
        // out by the other (the pager's position against where it is going).
        let behind = self.tiles.pos - self.tiles.target;
        if pages > 1 && behind.abs() > 0.5 {
            let forward = behind < 0.0;
            let from = if forward { (page + pages - 1) % pages } else { (page + 1) % pages };
            out.slide = -behind;
            out.ghosts = page_of(from);
            out.ghost_slide = -behind + if forward { -fw } else { fw };
        }
        let foot = y + rows as f32 * h + (rows - 1) as f32 * gap;
        if pages > 1 {
            out.dots = Some((foot + (dots_h - TILE_DOT * s) / 2.0, pages, page));
        }
        foot + if pages > 1 { dots_h } else { 0.0 } + GROUP_GAP * s
    }

    /// How many pages the open setting's pictures make.
    fn tile_pages(&self) -> usize {
        self.control
            .as_ref()
            .and_then(|v| v.groups.iter().find(|g| g.tiles))
            .map_or(1, |g| g.choices.len().div_ceil(self.tile_cap.get().max(1)).max(1))
    }

    /// Turn the pictures' page: `dir` +1 the next, -1 the one before, round
    /// the ends. Returns whether there was a page to turn to.
    pub(crate) fn tiles_turn(&mut self, dir: i64) -> bool {
        let pages = self.tile_pages();
        let turned = self.open.is_some() && self.tiles.turn(dir, true, pages, self.key.0);
        if turned {
            self.focus = None;
        }
        turned
    }

    /// The wheel over the pictures: enough of it turns a page.
    pub(crate) fn tiles_wheel(&mut self, value: f64) -> bool {
        if self.tile_pages() < 2 {
            return false;
        }
        match self.tiles.wheel(value) {
            Some(dir) => self.tiles_turn(dir),
            None => false,
        }
    }

    /// A caption over the next group of choices. Returns the group's top.
    fn caption(&self, out: &mut OpenContent, y: f32, text: String) -> f32 {
        out.captions.push((text, y));
        y + 27.0 * self.scale
    }

    /// The content of the open setting `label`, if it is a real one: the
    /// screen's choices for Scale and Resolution. `None`: a placeholder.
    fn open_content(&self, label: &str) -> Option<OpenContent> {
        let s = self.scale;
        let top = CONTENT_TOP * s;
        let mut out = OpenContent {
            subtitle: String::new(),
            captions: Vec::new(),
            chips: Vec::new(),
            foot_y: top,
            slide: 0.0,
            ghosts: Vec::new(),
            ghost_slide: 0.0,
            dots: None,
            sliders: Vec::new(),
            texts: Vec::new(),
        };
        if !self.modules && crate::controls::is_control(label) {
            // Not read yet: the bare card, for the moment the read takes.
            let Some(view) = self.control.as_ref().filter(|v| v.label == label) else {
                return Some(out);
            };
            out.subtitle = view.subtitle.clone();
            // A WALL of pictures sits tight under its title and over its
            // hint (Max, 2026-10-14: "para mí hay mucho aire abajo y arriba
            // de las imágenes").
            let wall = view.groups.iter().any(|g| g.tiles && !g.choices.is_empty());
            let mut y = if wall { WALL_TOP * s } else { top };
            let title_y = if wall { 20.0 } else { 30.0 } * s;
            for (gi, g) in view.groups.iter().enumerate() {
                if g.beside {
                    continue; // (laid out with the slider before it)
                }
                // THE MAIN CHOICE: bold words centred on the title's line.
                if g.main {
                    let chips = g.choices.iter().enumerate().map(|(ci, c)| (c.text.clone(), c.on, PanelAction::Pick { group: gi as u8, index: ci as u8 })).collect();
                    self.flow_words(&mut out, title_y + 2.0 * s, chips, true);
                    continue;
                }
                // A SLIDER is one row: its name, the track, its value, its look.
                if g.slider {
                    let (name, value) = match g.caption.as_deref().and_then(|c| c.split_once(" · ")) {
                        Some((n, v)) => (n.to_owned(), v.to_owned()),
                        None => (g.caption.clone().unwrap_or_default(), String::new()),
                    };
                    let pair = view.groups.get(gi + 1).filter(|n| n.beside).map(|n| (gi as u8 + 1, n));
                    y = self.flow_slider(&mut out, y, gi as u8, &g.choices, pair, (&name, &value));
                    continue;
                }
                // ON OR OFF is a switch row: its name left, the switch right.
                let values: Vec<&str> = g.choices.iter().map(|c| c.value.as_str()).collect();
                if values == ["on", "off"] {
                    let on = g.choices.first().is_some_and(|c| c.on);
                    let name = g.caption.clone().unwrap_or_else(|| crate::i18n::tr("On").to_owned());
                    y = self.switch_row(&mut out, y, gi as u8, &name, on);
                    continue;
                }
                if let Some(c) = &g.caption {
                    y = self.caption(&mut out, y, c.clone());
                }
                if g.cells > 0 {
                    y = self.flow_cells(&mut out, y, gi as u8, g.cells, &g.choices);
                    continue;
                }
                if g.tiles {
                    y = self.flow_tiles(&mut out, y, gi as u8, &g.choices);
                    continue;
                }
                let chips = g
                    .choices
                    .iter()
                    .enumerate()
                    .map(|(ci, c)| (c.text.clone(), c.on, PanelAction::Pick { group: gi as u8, index: ci as u8 }))
                    .collect();
                let first = out.chips.len();
                let coloured = g.choices.iter().any(|c| !c.swatch.is_empty());
                // (Pills stay for what is not a word: a coloured place, an
                // action, a field; one lone choice is an action too.)
                y = if g.buttons || g.field || coloured || g.choices.len() == 1 { self.flow(&mut out, y, g.width, chips) } else { self.flow_words(&mut out, y, chips, false) };
                let n = out.chips.len() - first;
                for (k, (chip, c)) in out.chips[first..].iter_mut().zip(&g.choices).enumerate() {
                    chip.swatch = c.swatch.clone();
                    if g.field {
                        chip.style = Style::Field;
                    } else if g.buttons || (n == 1 && !coloured) {
                        chip.style = Style::Button { main: g.buttons && k + 1 == n };
                    }
                }
            }
            out.foot_y = if wall { y - (GROUP_GAP - WALL_HINT_GAP) * s } else { (y - (GROUP_GAP - 12.0) * s).max(top) };
            return Some(out);
        }
        if !is_display_setting(label) {
            return None;
        }
        let Some(v) = &self.display else {
            out.subtitle = tr("No screen to set.").to_owned();
            return Some(out);
        };
        let mut y = top;
        if label == "Scale" {
            let (lw, lh) = v.looks_like();
            out.subtitle = crate::i18n::fill(tr("{0} · looks like {lw} × {lh}"), &[("0", &v.screen), ("lw", &lw), ("lh", &lh)]);
            let chips = v
                .stops
                .iter()
                .map(|&stop| {
                    // Rounded as people round (112.5 reads 113).
                    let text = format!("{}%", (stop * 100.0).round() as u32);
                    (text, (stop - v.scale).abs() < 1e-4, PanelAction::Scale(stop))
                })
                .collect();
            y = self.flow(&mut out, y, 76.0, chips);
        } else {
            out.subtitle = format!("{} · {}", v.screen, hz_text(v.mode.hz));
            let now = (v.mode.w, v.mode.h);
            let mut sizes: Vec<(u32, u32)> = v.sizes.iter().copied().take(MAX_SIZES).collect();
            if !sizes.contains(&now) {
                sizes.pop();
                sizes.push(now);
            }
            let chips = sizes
                .into_iter()
                .map(|(w, h)| (format!("{w} × {h}"), (w, h) == now, PanelAction::Mode { w, h, hz: None }))
                .collect();
            y = self.flow(&mut out, y, 150.0, chips);
            // A panel with ONE mode (the MacBook Air, the ASUS X550LC): say
            // so, rather than leave a lone lit pill that looks broken.
            if v.sizes.len() < 2 {
                y = self.caption(&mut out, y, tr("This screen has one resolution.").to_owned());
            }
            if v.rates.len() > 1 {
                y = self.caption(&mut out, y, tr("Refresh rate").to_owned());
                let chips = v
                    .rates
                    .iter()
                    .map(|&hz| {
                        let action = PanelAction::Mode { w: now.0, h: now.1, hz: Some(hz) };
                        (hz_text(hz), (hz - v.mode.hz).abs() < 0.5, action)
                    })
                    .collect();
                y = self.flow(&mut out, y, 96.0, chips);
            }
            if let Some(left) = self.keep_left {
                let text = crate::i18n::fill(tr("Keep this resolution? Going back in {0} s"), &[("0", &format!("{:.0}", left.ceil().max(1.0)))]);
                y = self.caption(&mut out, y, text);
                let chips = vec![
                    (tr("Keep").to_owned(), true, PanelAction::Keep),
                    (tr("Go back").to_owned(), false, PanelAction::Revert),
                ];
                y = self.flow(&mut out, y, 120.0, chips);
            }
        }
        out.foot_y = y - (GROUP_GAP - 12.0) * s;
        Some(out)
    }

    /// A click on the open setting `label`: `None` on the empty space,
    /// else what the control it landed on asks for (a placeholder's
    /// controls ask for nothing).
    fn open_hit(&self, label: &str, pos: (f32, f32)) -> Option<Option<PanelAction>> {
        let f = self.field;
        match self.open_content(label) {
            Some(content) => content
                .chips
                .iter()
                .find(|c| Rect::new(f.x + c.rect.x, f.y + c.rect.y, c.rect.w, c.rect.h).contains(pos))
                .map(|c| Some(c.action)),
            None => content_hit(self.key, self.scale, pos, f).then_some(None),
        }
    }

    /// The game: score a use of pill `i`, and if it now outscores the
    /// weakest pill one layer nearer, swap the two. Saves the standings.
    fn record_use(&mut self, i: usize, now: u64) {
        let st = self.usage.settings.entry(self.pills[i].label.to_owned()).or_default();
        *st = Standing { layer: st.layer, score: st.score_at(now) + 1.0, at: now };
        let score = st.score;
        let group = self.pills[i].group;
        if group > 0 {
            let score_of = |p: &Pill| self.usage.settings.get(p.label).map_or(0.0, |st| st.score_at(now));
            let weakest = (0..self.pills.len())
                .filter(|&j| self.pills[j].group == group - 1)
                .map(|j| (j, score_of(&self.pills[j])))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((j, weakest_score)) = weakest.filter(|&(_, sc)| score > sc) {
                self.pills[i].group = group - 1;
                self.pills[j].group = group;
                info!(
                    "control panel: {} ({score:.2}) steps nearer, {} ({weakest_score:.2}) steps back",
                    self.pills[i].label, self.pills[j].label
                );
                self.regroup();
            }
        }
        for p in &self.pills {
            if let Some(st) = self.usage.settings.get_mut(p.label) {
                st.layer = p.group;
            }
        }
        if let Some(path) = &self.store {
            crate::persist::write_json("control panel", path, &self.usage);
        }
    }

    /// Put every pill on its group's layer at the current scroll, and let
    /// them glide there.
    fn regroup(&mut self) {
        for p in &mut self.pills {
            let layer = (p.group as i32 - self.shift).rem_euclid(3) as u8;
            if layer == p.layer {
                continue;
            }
            p.layer = layer;
            if p.exit.is_none() {
                p.sc *= p.h[p.drawn as usize] / p.h[layer as usize];
                p.drawn = layer;
            }
        }
        let size = (self.key.0, self.key.1);
        self.compose(size, false);
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
        let best = (0..self.pills.len())
            .filter(|&i| self.pills[i].exit.is_none())
            .filter_map(|i| Some((match_rank(self.pills[i].label, self.pills[i].keywords, q)?, self.pills[i].layer, i)))
            .min()
            .map(|(.., i)| i);
        match best {
            Some(i) => {
                self.open_pill(i);
                true
            }
            None => false,
        }
    }

    /// Escape: fold an open setting back. Returns whether one was open.
    /// The catalog programs in this computer (lowercased names), from `App`.
    pub(crate) fn set_installed(&mut self, installed: std::collections::HashSet<String>) {
        self.installed = installed;
    }

    /// Lay an open module out for its state (see [`ModLayout`]).
    fn mod_layout(&self, m: &'static crate::modules::Module) -> ModLayout {
        let s = self.scale.max(0.01);
        let (fw, fh) = (self.key.0, self.key.1);
        let est = crate::options::est_text_w;
        let have: Vec<&'static crate::modules::Program> =
            m.programs().filter(|p| self.installed.contains(&p.name.to_lowercase())).collect();
        let want = &self.mods.want;
        let add: Vec<&'static crate::modules::Program> =
            m.programs().filter(|p| want.contains(p.name.as_str()) && !have.iter().any(|h| h.name == p.name)).collect();
        let remove: Vec<&'static crate::modules::Program> =
            have.iter().copied().filter(|p| !want.contains(p.name.as_str())).collect();
        let changed = !add.is_empty() || !remove.is_empty();
        let (kind, button_text, sub, link): (ModButton, String, Vec<String>, Option<(ModLink, &'static str)>) =
            match (changed, self.mods.sure, have.is_empty()) {
                (true, false, _) => {
                    let mut parts = Vec::new();
                    if !add.is_empty() {
                        parts.push(crate::i18n::fill(tr("{0} to add"), &[("0", &add.len())]));
                    }
                    if !remove.is_empty() {
                        parts.push(crate::i18n::fill(tr("{0} to remove"), &[("0", &remove.len())]));
                    }
                    (ModButton::Apply, tr("Apply changes").to_owned(), vec![parts.join(" · ")], Some((ModLink::Undo, tr("undo"))))
                }
                (true, true, _) => {
                    let mut say = Vec::new();
                    if !add.is_empty() {
                        say.push(format!("install {}", add.len()));
                    }
                    if !remove.is_empty() {
                        say.push(format!("remove {}", remove.len()));
                    }
                    let n = add.len() + remove.len();
                    let mut sub = Vec::new();
                    if !remove.is_empty() {
                        sub.push(crate::i18n::fill(tr("{0} will be removed."), &[("0", &(names(&remove, 3)))]));
                        sub.push(tr("Your files stay where they are.").to_owned());
                    } else {
                        sub.push(format!("{}.", names(&add, 4)));
                    }
                    let kind = if remove.is_empty() { ModButton::Confirm } else { ModButton::ConfirmRemove };
                    (kind, crate::i18n::fill(tr("Yes, {0} {1}"), &[("0", &(say.join(" and "))), ("1", &(plural(n)))]), sub, Some((ModLink::KeepChoosing, tr("keep choosing"))))
                }
                (false, _, true) => (
                    ModButton::InstallRecommended,
                    tr("Install recommended").to_owned(),
                    vec![tr("or switch on the programs you want").to_owned()],
                    None,
                ),
                (false, false, false) => (
                    ModButton::Uninstall,
                    tr("Uninstall all").to_owned(),
                    vec![tr("or switch programs on and off").to_owned()],
                    None,
                ),
                (false, true, false) => (
                    ModButton::ConfirmRemove,
                    crate::i18n::fill(tr("Yes, uninstall {0} {1}"), &[("0", &have.len()), ("1", &(plural(have.len())))]),
                    vec![tr("Your files stay where they are.").to_owned()],
                    Some((ModLink::KeepThem, tr("keep them"))),
                ),
            };
        // From the foot up: the hint, the link, the lines, the button.
        let line = 21.0 * s;
        // Clear of the card's Search pill, which sits in the field's foot.
        let hint_y = fh - 64.0 * s;
        let mut y = hint_y - 6.0 * s;
        let link = link.map(|(what, t)| {
            y -= line;
            let w = est(t, 15.5 * s) + 8.0 * s;
            (Rect::new((fw - w) / 2.0, y, w, line), what, t)
        });
        y -= sub.len() as f32 * line + 4.0 * s;
        let sub: Vec<(String, f32)> = sub.into_iter().enumerate().map(|(i, t)| (t, y + i as f32 * line)).collect();
        let bh = 36.0 * s;
        y -= 8.0 * s + bh;
        let bw = est(&button_text, 18.0 * s) + 42.0 * s;
        let button = Rect::new((fw - bw) / 2.0, y, bw, bh);
        // The list: captions over groups of rows.
        let row_h = 46.0 * s;
        let row_w = (580.0 * s).min(fw - 32.0 * s);
        let (mut captions, mut rows, mut cy) = (Vec::new(), Vec::new(), 0.0);
        for g in &m.groups {
            let progs: Vec<&'static crate::modules::Program> =
                g.programs.iter().filter(|p| crate::modules::machine().can(p.needs)).collect();
            if progs.is_empty() {
                continue;
            }
            captions.push((g.name.clone(), cy));
            cy += line + 8.0 * s;
            for prog in progs {
                rows.push(ModRow { prog, y: cy });
                cy += row_h + 9.0 * s;
            }
            cy += 10.0 * s;
        }
        let content_h = (cy - 16.0 * s).max(0.0);
        let top = 82.0 * s;
        let mut h = (y - 14.0 * s - top).max(row_h);
        if content_h > h {
            // Cut halfway through the last pill that fits: the cut says "more".
            if let Some(r) = rows.iter().rfind(|r| r.y + row_h / 2.0 <= h) {
                h = r.y + row_h / 2.0;
            }
        }
        ModLayout {
            list: Rect::new(0.0, top, fw, h),
            max_scroll: (content_h - h).max(0.0),
            captions,
            rows,
            row_x: (fw - row_w) / 2.0,
            row_w,
            row_h,
            button,
            button_text,
            kind,
            sub,
            link,
            hint_y,
            add,
            remove,
        }
    }

    /// A click on an open module: a switch, the button, the link. `false`:
    /// none of them (the empty space closes the module).
    fn mod_click(&mut self, label: &str, pos: (f32, f32)) -> bool {
        let Some(m) = crate::modules::module(label) else {
            return false;
        };
        let l = self.mod_layout(m);
        let f = self.field;
        let p = (pos.0 - f.x, pos.1 - f.y);
        if l.button.contains(p) {
            match l.kind {
                ModButton::InstallRecommended => {
                    // Switch our picks on so they show, then ask.
                    self.mods.want = m.programs().filter(|p| p.in_recommended()).map(|p| p.name.as_str()).collect();
                    self.mods.sure = true;
                }
                ModButton::Uninstall | ModButton::Apply => self.mods.sure = true,
                ModButton::Confirm | ModButton::ConfirmRemove => {
                    let remove = if l.add.is_empty() && l.remove.is_empty() {
                        // "Yes, uninstall": everything of it that is in.
                        m.programs().filter(|p| self.installed.contains(&p.name.to_lowercase())).collect()
                    } else {
                        l.remove
                    };
                    info!("modules: {label} — apply");
                    self.pending_apply = Some((m.name.as_str(), l.add, remove));
                    self.mods = ModView::default();
                    if let Some(open) = &mut self.open {
                        open.want = 0.0;
                    }
                }
            }
            return true;
        }
        if let Some((r, what, _)) = l.link {
            if r.contains(p) {
                match what {
                    ModLink::Undo => {
                        self.mods.want = m
                            .programs()
                            .filter(|p| self.installed.contains(&p.name.to_lowercase()))
                            .map(|p| p.name.as_str())
                            .collect();
                    }
                    ModLink::KeepChoosing | ModLink::KeepThem => self.mods.sure = false,
                }
                return true;
            }
        }
        if l.list.contains(p) {
            let y = p.1 - l.list.y + self.mods.scroll;
            if let Some(row) = l
                .rows
                .iter()
                .find(|r| y >= r.y && y < r.y + l.row_h && p.0 >= l.row_x && p.0 < l.row_x + l.row_w)
            {
                let name = row.prog.name.as_str();
                if !self.mods.want.remove(name) {
                    self.mods.want.insert(name);
                }
                self.mods.sure = false;
                return true;
            }
        }
        false
    }

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
    ///
    /// Every visit starts from the beginning (Max, 2026-09-30): the layers
    /// back in their first order, every pill home and at rest, no search,
    /// nothing open. Run out of sight (see `App::panel_reset_due`).
    pub(crate) fn reset(&mut self) {
        self.open = None;
        self.hot = None;
        self.query.clear();
        if self.pills.is_empty() {
            return;
        }
        self.shift = 0;
        for p in &mut self.pills {
            p.focus = 0.0;
            p.layer = p.group;
            p.drawn = p.group;
            p.exit = None;
            p.sc = 1.0;
            p.op = 1.0;
            p.lift = 0.0;
            p.rate = 1.0;
        }
        let size = (self.key.0, self.key.1);
        self.compose(size, true);
        info!("control panel: panel reset to its first layer order");
    }

    /// Advance the field by `dt`; `pointer` is in surface coordinates.
    fn step(&mut self, dt: f32, pointer: Option<(f32, f32)>) {
        self.clock += dt;
        // ONE CURSOR on an open setting's choices (Max, 2026-10-13: three
        // tiles looked chosen — the pointer's, the keys', the one in use —
        // "hacé que haya una sola, la del puntero, y se mueve con el
        // puntero o con las teclas"): the pointer MOVING puts it where the
        // pointer is (off every choice: nowhere); a key moves it from
        // there. A pointer lying still is not a second one.
        let moved = match (self.pointer, pointer) {
            (Some(a), Some(b)) => (a.0 - b.0).hypot(a.1 - b.1) > 1.5,
            (None, Some(_)) => true,
            _ => false,
        };
        self.pointer = pointer;
        self.tiles.ease(dt);
        if moved && !self.tiles.is_settling() && self.open.as_ref().is_some_and(|o| o.want > 0.5) {
            let chips = self.key_chips();
            if !chips.is_empty() {
                let f = self.field;
                self.focus = pointer
                    .and_then(|pos| chips.iter().position(|c| Rect::new(f.x + c.0.x, f.y + c.0.y, c.0.w, c.0.h).contains(pos)));
            }
        }
        if let Some(open) = &mut self.open {
            if open.want == 0.0 && !self.solo {
                // Fold back into where the pill is now: the game may have
                // moved it to another layer while it was open.
                let p = &self.pills[open.pill];
                let d = p.drawn as usize;
                let k = p.sc * p.focus_scale();
                let (w, h) = (p.w[d] * k, p.h[d] * k);
                open.from = Rect::new(p.pos.0 - w / 2.0, p.pos.1 - h / 2.0, w, h);
            }
            if self.solo {
                // A box of its own: the content stands as at rest; the whole
                // box comes and goes the window's way (`solo_pose`).
                open.k = 1.0;
                if open.want > 0.0 {
                    open.shown = (open.shown + dt / WINDOW_FADE_IN).min(1.0);
                    if crate::animation::reduce_motion() {
                        (open.pop, open.vel) = (1.0, 0.0);
                    }
                    let mut left = dt.clamp(0.0, 0.1);
                    while left > 0.0 {
                        let h = left.min(1.0 / 240.0);
                        open.vel += (-WINDOW_SPRING.0 * (open.pop - 1.0) - WINDOW_SPRING.1 * open.vel) * h;
                        open.pop += open.vel * h;
                        left -= h;
                    }
                } else {
                    open.vel = 0.0;
                    open.pop = (open.pop - dt / WINDOW_POP_OUT).max(0.0);
                    open.shown = (open.shown - dt / WINDOW_FADE_OUT).max(0.0);
                }
            } else {
                open.k = approach(open.k, open.want, OPEN_RATE, dt);
            }
            let gone = if self.solo { open.shown <= 0.0 } else { open.k < 0.01 };
            if open.want == 0.0 && gone {
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

        // (One setting in a box of its own: no field behind it.)
        if self.solo {
            order.clear();
        }
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
            let a = p.op.max(0.0) * fade * (1.0 - SEARCH_DIM * (-p.focus).max(0.0));
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
                text: crate::i18n::tr(p.label).to_owned(),
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
                    text: crate::i18n::tr(p.label).to_owned(),
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
        let wall = self.open_has_tiles() && self.control.is_some();
        let mut y = if wall { 20.0 } else { 30.0 } * s;
        // (A line of text from its left edge.)
        let text_l = |out: &mut PanelDraw, t: String, x: f32, y: f32, px: f32, a: f32| {
            let (mx, my) = map(x, y);
            out.labels.push(Label {
                text: t,
                pos: (mx, my),
                max_w: (fw - x - 2.0 * MARGIN * s).max(40.0) * cs,
                font_px: px * s * cs,
                line_px: px * 1.3 * s * cs,
                centered: false,
                dim: false,
                cache: resting,
                family: TEXT_FONT,
                color: ink(a),
                clip: None,
            });
        };
        if let Some(content) = self.open_content(p.label) {
            // THE HAIRLINE LANGUAGE: the box's name at the left of its header
            // (its main choice, if it has one, centred on the same line), a
            // quiet line under it; the rows below.
            let left = 2.0 * MARGIN * s;
            text_l(out, crate::i18n::tr(p.label).to_owned(), left, y, 20.0, 1.0);
            y += if wall { 30.0 } else { 34.0 } * s;
            text_l(out, content.subtitle, left, y, 12.5, 0.5);
            for (caption, top) in content.captions {
                text(out, caption, cx, top, 12.5, 16.0, 0.5);
            }
            for t in content.texts {
                if t.centered {
                    text(out, t.text, t.x, t.y, t.px, t.px * 1.3, t.alpha);
                } else {
                    text_l(out, t.text, t.x, t.y, t.px, t.alpha);
                }
            }
            let layer_wash = if paint.bright { LAYER_WASH_BRIGHT } else { LAYER_WASH_DARK };
            let hover_a = if paint.bright { HOVER_WASH_BRIGHT } else { HOVER_WASH_DARK };
            let line = 20.0 * s;
            // A PICTURE: its preview in a still frame — a wash until the
            // picture is in, and ONE ring where the cursor is (never a second
            // ring, and no mark, for the one in use). The picture fills its
            // tile, and nothing is written on it (Max, 2026-10-14, of a foot
            // strip with ▶ and ✓: "the wallpaper box is screwed … it was
            // good before"). `dx`: a page of them mid-turn is drawn that far
            // aside, and a tile not wholly in the box is not drawn at all
            // (a picture cannot be cut at the box's edge).
            let tile_at = |out: &mut PanelDraw, r: Rect, dx: f32, file: String, keyed: bool| {
                if r.x + dx < 0.0 || r.x + dx + r.w > fw {
                    return;
                }
                let (mx, my) = map(r.x + dx, r.y);
                let tile = Rect::new(mx, my, r.w * cs, r.h * cs);
                let radius = TILE_RADIUS * s * cs;
                let wash = crate::options::wash(!paint.bright, layer_wash[2]);
                out.rects.push(RectInst { rect: tile, radius, color: [wash[0], wash[1], wash[2], wash[3] * ca], glass: 0.0, border: 0.0 });
                if resting {
                    out.images.push((tile, file));
                }
                if keyed {
                    let (grow, alpha, border) = (3.0 * s, 0.95, 2.4 * s);
                    out.rects.push(RectInst {
                        rect: Rect::new(tile.x - grow, tile.y - grow, tile.w + 2.0 * grow, tile.h + 2.0 * grow),
                        radius: radius + grow,
                        color: [paint.ink[0], paint.ink[1], paint.ink[2], alpha * ca],
                        glass: 0.0,
                        border: border.max(1.5),
                    });
                }
            };
            // A PALETTE: its swatch, the ground with its soul as a band of
            // stripes along the foot; Dynamic (no colours: it is made from
            // the wallpaper) as a wheel of hues. The one in use, ringed.
            let swatch_at = |out: &mut PanelDraw, r: Rect, dx: f32, colours: &[[u8; 3]], on: bool, keyed: bool| {
                if r.x + dx < 0.0 || r.x + dx + r.w > fw {
                    return;
                }
                let lin = |c: [u8; 3], a: f32| {
                    let f = |v: u8| crate::options::srgb_to_linear(f32::from(v) / 255.0);
                    [f(c[0]), f(c[1]), f(c[2]), a * ca]
                };
                let (x, y0) = (r.x + dx, r.y);
                let radius = TILE_RADIUS * s;
                if colours.is_empty() {
                    let n = 12;
                    for k in 0..n {
                        let c = crate::palette::tint::hue(k as f32 / n as f32);
                        let w = r.w / n as f32;
                        rect(out, x + k as f32 * w, y0, w + 0.5, r.h, if k == 0 || k == n - 1 { radius } else { 0.0 }, lin(c, 1.0));
                    }
                } else {
                    // PAPER (the approved card, ~/Golem/work/palettes): the
                    // ground, lit from above; the soul as tall capsules
                    // fanned like a deck — the first on top — each with a
                    // sheen and its shadow cast to the left; a soft oval
                    // shadow under the fan. The renderer has no gradients:
                    // each is a stack of thin strips, every strip cut to the
                    // shape it lies in.
                    let ground = colours[0];
                    let bright = crate::palette::rel_lum(ground) > 0.35;
                    rect(out, x, y0, r.w, r.h, radius, lin(ground, 1.0));
                    // (The panel's blend only DARKENS through alpha: a light is
                    // painted solid, the colour under it already mixed toward
                    // white; a shade can stay translucent black.)
                    let lit = |c: [u8; 3], a: f32| {
                        let f = |v: u8| crate::options::srgb_to_linear(f32::from(v) / 255.0);
                        let m = |v: u8| f(v) + (1.0 - f(v)) * a;
                        [m(c[0]), m(c[1]), m(c[2]), ca]
                    };
                    let dim = |c: [u8; 3], a: f32| {
                        let f = |v: u8| crate::options::srgb_to_linear(f32::from(v) / 255.0) * (1.0 - a);
                        [f(c[0]), f(c[1]), f(c[2]), ca]
                    };
                    let shade = |a: f32| [0.0, 0.0, 0.0, a * ca];
                    const STRIPS: usize = 24;
                    // The light from above: a dome of it, fading down to 45 %.
                    // (In linear light: a dark ground takes very little to show it.)
                    let lift = if bright { 0.30 } else { 0.006 };
                    let dome = r.h * 0.45;
                    for k in 0..STRIPS {
                        let t = (k as f32 + 0.5) / STRIPS as f32;
                        let half = r.w * 0.45 * (1.0 - t * t).sqrt().max(0.0);
                        let inset = if k == 0 { radius * 0.6 } else { 0.0 };
                        let (sx, sw) = (x + r.w / 2.0 - half + inset, 2.0 * (half - inset));
                        let a = lift * (1.0 - t).powf(1.6);
                        rect(out, sx, y0 + t * dome - dome / STRIPS as f32 / 2.0, sw, dome / STRIPS as f32 + 0.5, 0.0, lit(ground, a));
                    }
                    let soul = &colours[1..];
                    let n = soul.len().max(1) as f32;
                    let (pw, ph) = (r.w * 0.15, r.h * 0.62);
                    let step = pw * 0.5;
                    let span = pw + (n - 1.0) * step;
                    let (px0, py) = (x + (r.w - span) / 2.0, y0 + (r.h - ph) / 2.0);
                    let cap = pw / 2.0;
                    // A capsule's width at `y` down it (its ends are half circles).
                    let width_at = |y: f32| -> f32 {
                        let d = if y < cap { cap - y } else if y > ph - cap { y - (ph - cap) } else { 0.0 };
                        2.0 * (cap * cap - d * d).max(0.0).sqrt()
                    };
                    // The shadow the fan leaves on the paper: an oval, soft.
                    for k in 0..STRIPS {
                        let t = (k as f32 + 0.5) / STRIPS as f32 * 2.0 - 1.0;
                        let half = r.w * 0.26 * (1.0 - t * t).sqrt();
                        let h = r.h * 0.09 / STRIPS as f32;
                        rect(out, x + r.w / 2.0 - half, y0 + r.h * 0.86 + t * r.h * 0.045, 2.0 * half, h + 0.5, 0.0, shade(0.30 * (1.0 - t.abs())));
                    }
                    // The last first, so the first lies on top.
                    for (k, c) in soul.iter().enumerate().rev() {
                        let cx = px0 + k as f32 * step;
                        // Its shadow, cast to the left (soft: three passes).
                        for (o, a) in [(5.0f32, 0.08f32), (3.4, 0.10), (1.8, 0.14)] {
                            rect(out, cx - o * s, py + 0.4 * o * s, pw, ph, cap, shade(a));
                        }
                        rect(out, cx, py, pw, ph, cap, lin(*c, 1.0));
                        // Its sheen: light down the top third, shade at the foot.
                        for k in 0..STRIPS {
                            let t = (k as f32 + 0.5) / STRIPS as f32;
                            let y = t * ph;
                            let w = width_at(y);
                            if w < 0.5 {
                                continue;
                            }
                            let a = if t < 0.35 { 0.10 * (1.0 - t / 0.35) } else if t > 0.7 { 0.10 * (t - 0.7) / 0.3 } else { 0.0 };
                            if a <= 0.002 {
                                continue;
                            }
                            let col = if t < 0.5 { lit(*c, a) } else { dim(*c, a) };
                            rect(out, cx + (pw - w) / 2.0, py + y - ph / STRIPS as f32 / 2.0, w, ph / STRIPS as f32 + 0.5, 0.0, col);
                        }
                    }
                }
                if on || keyed {
                    let grow = 3.0 * s;
                    let (mx, my) = map(x - grow, y0 - grow);
                    out.rects.push(RectInst {
                        rect: Rect::new(mx, my, (r.w + 2.0 * grow) * cs, (r.h + 2.0 * grow) * cs),
                        radius: (radius + grow) * cs,
                        color: [paint.ink[0], paint.ink[1], paint.ink[2], if keyed { 0.95 } else { 0.6 } * ca],
                        glass: 0.0,
                        border: (if keyed { 2.4 } else { 1.6 } * s * cs).max(1.2),
                    });
                }
            };
            for chip in content.ghosts {
                if let Some(file) = chip.image {
                    tile_at(out, chip.rect, content.ghost_slide, file, false);
                } else if chip.palette {
                    swatch_at(out, chip.rect, content.ghost_slide, &chip.swatch, chip.on, false);
                }
            }
            // The SLIDERS: a quiet track, filled to the knob, the knob ringed
            // when the keys are on it.
            for sl in &content.sliders {
                let t = sl.track;
                let ink_c = |a: f32| [paint.ink[0], paint.ink[1], paint.ink[2], a];
                rect(out, t.x, t.y, t.w, t.h, t.h / 2.0, ink_c(0.16));
                let kx = t.x + t.w * sl.value;
                rect(out, t.x, t.y, (kx - t.x).max(t.h), t.h, t.h / 2.0, ink_c(0.9));
                let knob = 12.0 * s;
                rect(out, kx - knob / 2.0, t.y + t.h / 2.0 - knob / 2.0, knob, knob, knob / 2.0, ink_c(0.95));
                if resting && self.focus.is_some_and(|f| sl.stops.contains(&f)) {
                    let g = knob + 7.0 * s;
                    let (mx, my) = map(kx - g / 2.0, t.y + t.h / 2.0 - g / 2.0);
                    out.rects.push(RectInst {
                        rect: Rect::new(mx, my, g * cs, g * cs),
                        radius: g / 2.0 * cs,
                        color: ink_c(0.9 * ca),
                        glass: 0.0,
                        border: (2.0 * s * cs).max(1.5),
                    });
                }
            }
            // The pages' dots under the wall: the one that is up, brighter.
            if let Some((top, pages, page)) = content.dots {
                let (d, step) = (TILE_DOT * s, TILE_DOT * s * 2.2);
                let x0 = (fw - (pages - 1) as f32 * step - d) / 2.0;
                for k in 0..pages {
                    let a = if k == page { 0.9 } else { 0.3 };
                    rect(out, x0 + k as f32 * step, top, d, d, d / 2.0, [paint.ink[0], paint.ink[1], paint.ink[2], a]);
                }
            }
            for (i, chip) in content.chips.into_iter().enumerate() {
                let r = chip.rect;
                // (The one cursor — the pointer's or the keys': `step`.)
                let keyed = resting && self.focus == Some(i);
                if let Some(file) = chip.image {
                    tile_at(out, r, content.slide, file, keyed && content.slide.abs() < 0.5);
                    // The palette in use, ringed (a wallpaper has no mark).
                    if chip.palette && chip.on && !keyed && r.x + content.slide >= 0.0 && r.x + content.slide + r.w <= fw {
                        let grow = 3.0 * s;
                        let (mx, my) = map(r.x + content.slide - grow, r.y - grow);
                        out.rects.push(RectInst {
                            rect: Rect::new(mx, my, (r.w + 2.0 * grow) * cs, (r.h + 2.0 * grow) * cs),
                            radius: (TILE_RADIUS * s + grow) * cs,
                            color: [paint.ink[0], paint.ink[1], paint.ink[2], 0.6 * ca],
                            glass: 0.0,
                            border: (1.6 * s * cs).max(1.2),
                        });
                    }
                    continue;
                }
                if chip.seg {
                    continue;
                }
                let lin = |c: [u8; 3], a: f32| {
                    let f = |v: u8| crate::options::srgb_to_linear(f32::from(v) / 255.0);
                    [f(c[0]), f(c[1]), f(c[2]), a * ca]
                };
                // A COLOUR CELL: its colour, ringed when it is the one chosen
                // or the keys are on it.
                if chip.cell {
                    let c = chip.swatch.first().copied().unwrap_or([0, 0, 0]);
                    rect(out, r.x, r.y, r.w, r.h, 3.0 * s, lin(c, 1.0));
                    if chip.on || keyed {
                        let g = 2.5 * s;
                        let (mx, my) = map(r.x - g, r.y - g);
                        out.rects.push(RectInst {
                            rect: Rect::new(mx, my, (r.w + 2.0 * g) * cs, (r.h + 2.0 * g) * cs),
                            radius: (3.0 * s + g) * cs,
                            color: [paint.ink[0], paint.ink[1], paint.ink[2], if keyed { 0.95 } else { 0.8 } * ca],
                            glass: 0.0,
                            border: (2.0 * s * cs).max(1.5),
                        });
                    }
                    continue;
                }
                // The "+" tile after the palettes: make one's own.
                if chip.palette && chip.text == "+" {
                    rect(out, r.x, r.y, r.w, r.h, TILE_RADIUS * s, crate::options::wash(!paint.bright, if keyed { hover_a } else { layer_wash[1] }));
                    text(out, "+".to_owned(), r.x + r.w / 2.0, r.y + (r.h - 30.0 * s) / 2.0, 26.0, 30.0, 0.9);
                    continue;
                }
                if chip.palette {
                    swatch_at(out, r, content.slide, &chip.swatch, chip.on, keyed && content.slide.abs() < 0.5);
                    continue;
                }
                // A PILL IN A COLOUR (the editor's places): filled with it,
                // its words in whichever ink reads on it.
                if let Some(&c) = chip.swatch.first() {
                    let a = self.clock * TAU / (13.0 + 1.7 * (i % 5) as f32) + 1.9 * i as f32;
                    let (x0, y0) = (r.x + 1.1 * s * a.cos(), r.y + 1.1 * s * a.sin());
                    rect(out, x0, y0, r.w, r.h, r.h / 2.0, lin(c, 1.0));
                    if chip.on || keyed {
                        let g = if chip.on { 3.0 } else { 2.5 } * s;
                        let (mx, my) = map(x0 - g, y0 - g);
                        out.rects.push(RectInst {
                            rect: Rect::new(mx, my, (r.w + 2.0 * g) * cs, (r.h + 2.0 * g) * cs),
                            radius: (r.h / 2.0 + g) * cs,
                            color: [paint.ink[0], paint.ink[1], paint.ink[2], 0.95 * ca],
                            glass: 0.0,
                            border: (2.2 * s * cs).max(1.5),
                        });
                    }
                    let light = crate::palette::rel_lum(c) > 0.25;
                    let ink_c = if light { [0.02, 0.02, 0.02, ca] } else { [1.0, 1.0, 1.0, ca] };
                    let (mx, my) = map(x0 + r.w / 2.0, y0 + (r.h - line) / 2.0);
                    out.labels.push(Label {
                        text: chip.text,
                        pos: (mx, my),
                        max_w: r.w * cs,
                        font_px: 16.0 * s * cs,
                        line_px: 20.0 * s * cs,
                        centered: true,
                        dim: false,
                        cache: resting,
                        family: TEXT_FONT,
                        color: Some(ink_c),
                        clip: None,
                    });
                    continue;
                }
                let ink_c = |a: f32| [paint.ink[0], paint.ink[1], paint.ink[2], a * ca];
                // (What reads on the ink: the ground's tone.)
                let on_ink = if paint.bright { [0.93, 0.93, 0.92, ca] } else { [0.02, 0.02, 0.025, ca] };
                let label_in = |out: &mut PanelDraw, t: String, x: f32, y: f32, w: f32, px: f32, col: [f32; 4]| {
                    let (mx, my) = map(x, y);
                    out.labels.push(Label { text: t, pos: (mx, my), max_w: w * cs, font_px: px * s * cs, line_px: px * 1.3 * s * cs, centered: true, dim: false, cache: resting, family: TEXT_FONT, color: Some(col), clip: None });
                };
                match chip.style {
                    // A WORD: bright and underlined when it is the one in use.
                    Style::Word { big } => {
                        let (px, gap) = if big { (15.5, 28.0 * s) } else { (13.0, 22.0 * s) };
                        let a = if chip.on || keyed { 1.0 } else { 0.55 };
                        label_in(out, chip.text, r.x + r.w / 2.0, r.y + (r.h - px * 1.3 * s) / 2.0, r.w, px, ink_c(a));
                        let uw = (r.w - gap).max(6.0 * s);
                        let ux = r.x + (r.w - uw) / 2.0;
                        let uy = r.y + (r.h + px * 1.3 * s) / 2.0 + 2.0 * s;
                        if chip.on {
                            rect(out, ux, uy, uw, if big { 1.5 } else { 1.0 } * s, 0.0, ink_c(1.0));
                        } else if keyed {
                            rect(out, ux, uy, uw, 1.0 * s, 0.0, ink_c(0.35));
                        }
                    }
                    // A SWITCH: a slim outlined capsule; on, it fills and the knob crosses.
                    Style::Switch { on } => {
                        let (tw, th) = (36.0 * s, 20.0 * s);
                        let (tx, ty) = (r.x + r.w - 6.0 * s - tw, r.y + (r.h - th) / 2.0);
                        let (mx, my) = map(tx, ty);
                        let knob = 12.0 * s;
                        if on {
                            rect(out, tx, ty, tw, th, th / 2.0, ink_c(0.92));
                            rect(out, tx + tw - 4.0 * s - knob, ty + 4.0 * s, knob, knob, knob / 2.0, on_ink);
                        } else {
                            out.rects.push(RectInst { rect: Rect::new(mx, my, tw * cs, th * cs), radius: th / 2.0 * cs, color: ink_c(0.38), glass: 0.0, border: (1.0 * s * cs).max(1.0) });
                            rect(out, tx + 4.0 * s, ty + 4.0 * s, knob, knob, knob / 2.0, ink_c(0.75));
                        }
                        if keyed {
                            let g = 3.0 * s;
                            let (mx, my) = map(tx - g, ty - g);
                            out.rects.push(RectInst { rect: Rect::new(mx, my, (tw + 2.0 * g) * cs, (th + 2.0 * g) * cs), radius: (th / 2.0 + g) * cs, color: ink_c(0.7), glass: 0.0, border: (1.0 * s * cs).max(1.0) });
                        }
                    }
                    // A BUTTON: a hairline capsule; the main one filled.
                    Style::Button { main } => {
                        let (mx, my) = map(r.x, r.y);
                        if main {
                            rect(out, r.x, r.y, r.w, r.h, r.h / 2.0, ink_c(if keyed { 1.0 } else { 0.92 }));
                            label_in(out, chip.text, r.x + r.w / 2.0, r.y + (r.h - line) / 2.0, r.w, 14.0, on_ink);
                        } else {
                            if keyed {
                                rect(out, r.x, r.y, r.w, r.h, r.h / 2.0, crate::options::wash(!paint.bright, hover_a));
                            }
                            out.rects.push(RectInst { rect: Rect::new(mx, my, r.w * cs, r.h * cs), radius: r.h / 2.0 * cs, color: ink_c(0.3), glass: 0.0, border: (1.0 * s * cs).max(1.0) });
                            label_in(out, chip.text, r.x + r.w / 2.0, r.y + (r.h - line) / 2.0, r.w, 14.0, ink_c(if chip.on || keyed { 1.0 } else { 0.85 }));
                        }
                    }
                    // A FIELD: its words over a hairline.
                    Style::Field => {
                        let a = if chip.on { 1.0 } else { 0.6 };
                        label_in(out, chip.text, r.x + r.w / 2.0, r.y + (r.h - line) / 2.0, r.w, 14.0, ink_c(a));
                        rect(out, r.x, r.y + r.h - 2.0 * s, r.w, 1.0 * s, 0.0, ink_c(if chip.on || keyed { 0.8 } else { 0.3 }));
                    }
                }
            }
            text(out, tr("Esc or a click on the empty space closes").to_owned(), cx, content.foot_y, 12.0, 16.0, 0.4);
            return;
        }
        text(out, crate::i18n::tr(p.label).to_owned(), cx, y, 30.0, 36.0, 1.0);
        y += if wall { 40.0 } else { 44.0 } * s;
        // A module: its programs as a list of switches, ONE button at the foot.
        if let Some(m) = self.modules.then(|| crate::modules::module(p.label)).flatten() {
            let l = self.mod_layout(m);
            let wash = |a: f32| crate::options::wash(!paint.bright, a);
            let clip = {
                let (x, y) = map(l.list.x, l.list.y);
                Rect::new(x, y, l.list.w * cs, l.list.h * cs)
            };
            let mut inside = PanelDraw::default();
            let top = l.list.y - self.mods.scroll;
            for (cap, cy) in &l.captions {
                text(&mut inside, cap.clone(), cx, top + cy, 17.0, 21.0, 0.62);
            }
            let hover_a = if paint.bright { HOVER_WASH_BRIGHT } else { HOVER_WASH_DARK };
            for row in &l.rows {
                let (rx, ry, rw, rh) = (l.row_x, top + row.y, l.row_w, l.row_h);
                if ry + rh < l.list.y || ry > l.list.y + l.list.h {
                    continue;
                }
                let on = self.mods.want.contains(row.prog.name.as_str());
                let was = self.installed.contains(&row.prog.name.to_lowercase());
                let hot = resting
                    && self.pointer.is_some_and(|pos| clip.contains(pos) && Rect::new(f.x + rx, f.y + ry, rw, rh).contains(pos));
                let fill = if hot { hover_a } else if on { 0.11 } else { 0.07 };
                rect(&mut inside, rx, ry, rw, rh, rh / 2.0, wash(fill));
                if on {
                    let (mx, my) = map(rx, ry);
                    inside.rects.push(RectInst {
                        rect: Rect::new(mx, my, rw * cs, rh * cs),
                        radius: rh / 2.0 * cs,
                        color: [paint.ink[0], paint.ink[1], paint.ink[2], 0.55 * ca],
                        glass: 0.0,
                        border: 1.0,
                    });
                }
                // The name, then what it is (or what will happen to it).
                let left = |t: String, x: f32, px: f32, a: f32, max: f32| {
                    let (mx, my) = map(rx + x, ry + (rh - px * 1.3 * s) / 2.0);
                    Label {
                        text: t,
                        pos: (mx, my),
                        max_w: max * cs,
                        font_px: px * s * cs,
                        line_px: px * 1.3 * s * cs,
                        centered: false,
                        dim: false,
                        cache: resting,
                        family: TEXT_FONT,
                        color: ink(a),
                        clip: Some(clip),
                    }
                };
                let pad = 20.0 * s;
                let sw = (44.0 * s, 26.0 * s);
                let name_w = self.name_w.get(row.prog.name.as_str()).copied().unwrap_or(0.0);
                inside.labels.push(left(row.prog.name.clone(), pad, MOD_NAME_PX, 1.0, rw - 2.0 * pad - sw.0));
                let what = match (on, was) {
                    (true, false) => crate::i18n::fill(tr("will be added · {0}"), &[("0", &row.prog.what)]),
                    (false, true) => crate::i18n::fill(tr("will be removed · {0}"), &[("0", &row.prog.what)]),
                    _ => row.prog.what.clone(),
                };
                let dx = pad + name_w + 10.0 * s;
                let room = rw - dx - pad - sw.0 - 8.0 * s;
                if room > 20.0 * s {
                    inside.labels.push(left(what, dx, 16.0, 0.65, room));
                }
                // The switch: on, white with a dark knob; off, a quiet wash.
                let (tx, ty) = (rx + rw - 9.0 * s - sw.0, ry + (rh - sw.1) / 2.0);
                rect(&mut inside, tx, ty, sw.0, sw.1, sw.1 / 2.0, if on { [paint.ink[0], paint.ink[1], paint.ink[2], 0.92] } else { wash(0.16) });
                let knob = 19.0 * s;
                let kx = if on { tx + sw.0 - 3.0 * s - knob } else { tx + 3.0 * s };
                let kc = if on { [0.02, 0.025, 0.035, 1.0] } else { [paint.ink[0], paint.ink[1], paint.ink[2], 0.8] };
                rect(&mut inside, kx, ty + (sw.1 - knob) / 2.0, knob, knob, knob / 2.0, kc);
            }
            out.clipped.push((clip, inside.rects, inside.labels));
            // The one button and what it means.
            let b = l.button;
            let hot = resting && self.pointer.is_some_and(|pos| Rect::new(f.x + b.x, f.y + b.y, b.w, b.h).contains(pos));
            let (fill, ring) = match l.kind {
                ModButton::ConfirmRemove => ([1.0, 0.18, 0.18, 0.35], [1.0, 0.55, 0.55, 0.7]),
                ModButton::Apply | ModButton::Confirm => (wash(if hot { 0.30 } else { 0.22 }), [paint.ink[0], paint.ink[1], paint.ink[2], 0.9]),
                _ => (wash(if hot { 0.27 } else { 0.14 }), [paint.ink[0], paint.ink[1], paint.ink[2], 0.6]),
            };
            rect(out, b.x, b.y, b.w, b.h, b.h / 2.0, fill);
            let (mx, my) = map(b.x, b.y);
            out.rects.push(RectInst {
                rect: Rect::new(mx, my, b.w * cs, b.h * cs),
                radius: b.h / 2.0 * cs,
                color: [ring[0], ring[1], ring[2], ring[3] * ca],
                glass: 0.0,
                border: 1.0,
            });
            text(out, l.button_text.clone(), cx, b.y + (b.h - 23.0 * s) / 2.0, 18.0, 23.0, 1.0);
            for (t, ty) in &l.sub {
                text(out, t.clone(), cx, *ty, 15.5, 20.0, 0.65);
            }
            if let Some((r, _, t)) = &l.link {
                let hot = resting && self.pointer.is_some_and(|pos| Rect::new(f.x + r.x, f.y + r.y, r.w, r.h).contains(pos));
                text(out, t.to_string(), cx, r.y, 15.5, 20.0, if hot { 1.0 } else { 0.85 });
                rect(out, cx - r.w / 2.0 + 4.0 * s, r.y + 19.0 * s, r.w - 8.0 * s, 1.0 * s, 0.0, [paint.ink[0], paint.ink[1], paint.ink[2], 0.5]);
            }
            text(out, tr("Esc or a click on the empty space closes").to_owned(), cx, l.hint_y, 12.0, 16.0, 0.4);
            return;
        }
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
        let rows: [(&str, u8); 3] = [(tr("Level"), 0), (tr("Automatic"), 1), (tr("Apply to"), 2)];
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
                    text(out, tr("this screen").to_owned(), x0 + rw - 75.0 * s, y0 + (rh - 17.0 * s) / 2.0, 13.0, 17.0, 0.7);
                }
            }
            y += rh + 22.0 * s;
        }
        text(out, tr("Esc or a click on the empty space closes").to_owned(), cx, y + 6.0 * s, 12.0, 16.0, 0.4);
    }
}

/// The settings about the screen, fed by [`crate::display`].
fn is_display_setting(label: &str) -> bool {
    matches!(label, "Scale" | "Resolution")
}

/// A refresh rate as people read it: `60 Hz`, `59.95 Hz`.
fn hz_text(hz: f64) -> String {
    if (hz - hz.round()).abs() < 0.01 {
        format!("{hz:.0} Hz")
    } else {
        format!("{hz:.2} Hz")
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
    pub(crate) fn pin_control_panel_once(&mut self) {
        let trash = format!("group:{}", groups::TRASH_ID);
        let slot = self
            .pins
            .pins()
            .iter()
            .position(|p| *p == trash)
            .unwrap_or(self.pins.pins().len());
        self.pin_once(apps::CONTROL_PANEL_ID, "settings-pinned", slot);
    }

    /// One-shot: pin the Apps button first on the dock, where Launchpad sits
    /// on a Mac. Same marker rule as [`Self::pin_control_panel_once`].
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
    /// it if the apps are what's showing. Over the control panel the apps
    /// come back in place.
    pub(crate) fn toggle_apps_grid(&mut self) {
        self.close_group();
        if self.ui.target() == Target::Open {
            if self.control_panel {
                info!("apps: in place of the control panel");
                self.control_panel = false;
                self.control_panel_from_apps = false;
                self.panel_reset_due = true;
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

    /// The settings' field, wherever it is (on show or parked).
    pub(crate) fn settings_panel(&self) -> &Panel {
        if self.panel.is_modules() { &self.panel_parked } else { &self.panel }
    }

    /// The modules' field, wherever it is.
    pub(crate) fn modules_panel(&self) -> &Panel {
        if self.panel.is_modules() { &self.panel } else { &self.panel_parked }
    }

    pub(crate) fn settings_panel_mut(&mut self) -> &mut Panel {
        if self.panel.is_modules() { &mut self.panel_parked } else { &mut self.panel }
    }

    /// Put the modules' field (`modules`) or the settings' on show; the one
    /// coming in starts fresh, on its first layer.
    fn panel_switch(&mut self, modules: bool) {
        if self.panel.is_modules() != modules {
            std::mem::swap(&mut self.panel, &mut self.panel_parked);
            self.panel.reset();
            // On show already (the card open on the other field): the old one
            // falls away while this one rises in.
            if self.ui.target() == Target::Open && self.control_panel {
                self.panel_parked.open = None;
                self.panel_swap_t = 0.0;
            }
        }
        if modules {
            let installed = self.modules_installed();
            self.panel.set_installed(installed);
        }
    }

    /// The catalog programs this computer has (their names, lowercased): a
    /// package the dock installed or is installing, a webapp installed from
    /// the catalog — and, for what came with the system, an app in the grid
    /// by the program's name.
    pub(crate) fn modules_installed(&self) -> std::collections::HashSet<String> {
        let have: std::collections::HashSet<String> = (0..self.base_len.min(self.entries.len()))
            .filter(|&i| self.kinds.get(i) == Some(&apps::EntryKind::App) && !self.is_catalog_webapp(i))
            .map(|i| self.entries[i].name.to_lowercase())
            .collect();
        let listed: std::collections::HashSet<String> = crate::applier::list_attrs().into_iter().collect();
        let webapps = crate::webapps::load_catalog();
        crate::modules::catalog()
            .modules
            .iter()
            .flat_map(|m| m.groups.iter().flat_map(|g| &g.programs))
            .filter(|p| match (&p.pkg, &p.web) {
                (Some(attr), _) => listed.contains(attr) || self.managed.contains(attr) || have.contains(&p.name.to_lowercase()),
                (None, Some(url)) => crate::modules::webapp_slug(url, &webapps).is_some_and(|s| self.managed_webapps.contains(&s)),
                _ => false,
            })
            .map(|p| p.name.to_lowercase())
            .collect()
    }

    pub(crate) fn modules_panel_mut(&mut self) -> &mut Panel {
        if self.panel.is_modules() { &mut self.panel } else { &mut self.panel_parked }
    }

    /// The gear was clicked: the control panel's settings (see
    /// [`Self::toggle_panel`]).
    pub(crate) fn toggle_control_panel(&mut self) {
        self.toggle_panel(false);
    }

    /// Open the modules' field on module `name` (the `modules <name>` verb:
    /// a key's way straight to one, and the pointer-free way to look at it).
    pub(crate) fn show_module(&mut self, name: &str) {
        let Some(label) = crate::modules::items().iter().map(|i| i.0).find(|n| n.eq_ignore_ascii_case(name)) else {
            tracing::warn!("modules: no module named {name:?}");
            return;
        };
        if !(self.ui.target() == Target::Open && self.control_panel && self.panel.is_modules()) {
            self.toggle_modules();
        }
        self.panel.open_named(label);
        self.schedule_frame();
    }

    /// The Modules tile was clicked: the same field, holding the modules.
    pub(crate) fn toggle_modules(&mut self) {
        self.toggle_panel(true);
    }

    /// Open the card as the field (`modules`: the modules', else the
    /// settings'), or close it if that field is what's showing. With the
    /// card already open on the apps, the sections clear in place; on the
    /// other field, this one takes its place.
    fn toggle_panel(&mut self, modules: bool) {
        let what = if modules { "modules" } else { "control panel" };
        self.close_group();
        if self.ui.target() == Target::Open && self.control_panel && self.panel.is_modules() != modules {
            info!("{what}: in place of the other field");
            self.panel_switch(modules);
            self.search.open = false;
            self.search.query.clear();
            self.panel_search();
            return;
        }
        if self.ui.target() != Target::Open || !self.control_panel {
            self.panel_switch(modules);
        }
        if self.ui.target() == Target::Open {
            if self.control_panel && self.control_panel_from_apps {
                // Entered from the apps: the gear goes back to them.
                info!("{what}: back to the apps");
                self.control_panel = false;
                self.control_panel_from_apps = false;
                self.panel_reset_due = true;
                self.schedule_frame();
            } else if self.control_panel {
                info!("{what}: closing the panel");
                self.handle_command(Command::Collapse);
            } else {
                info!("{what}: panel in place of the apps");
                self.control_panel = true;
                self.control_panel_from_apps = true;
                self.search.open = false;
                self.search.query.clear();
                self.panel_search();
                self.schedule_frame();
            }
            return;
        }
        info!("{what}: opening the panel");
        self.control_panel_opening = true;
        self.handle_command(Command::Toggle);
        // Refused (e.g. the dock is suppressed): don't let a later open
        // come up as the panel.
        self.control_panel_opening = false;
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
        // Never larger than the size Max settled on (at the bar's scale):
        // a bigger card (a longer dock) keeps it centred under the dock; a
        // smaller card shrinks it with the card.
        let s = self.options_scale();
        // Sized for the untrimmed card: while it grows back from an apps
        // page's height the field keeps its size (no re-lay every frame).
        let card_h = (settled.card_h + self.ui.open_trim() - dock_h).max(1.0);
        let w = settled.card_w.min(FIELD_MAX.0 * s);
        let h = card_h.min(FIELD_MAX.1 * s);
        Rect::new(
            settled.card_x + (settled.card_w - w) / 2.0,
            settled.card_top + dock_h + ride,
            w,
            h,
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
        // An open display setting shows the screen as it is: read it when
        // the setting opens and again after each change.
        if self.panel.display_due() {
            let view = self.display_view();
            self.panel.set_display(view);
        }
        if let Some(label) = self.panel.control_due() {
            let ui = self.ui_facts();
            self.controls_send(ControlCommand::Read(label, ui));
        }
        self.colours_preview_guard();
        self.panel.wall_tile = self.wallpaper_tile_w();
        self.panel.keep_left = self.display_keep_left();
        let pointer = if self.ui.target() == Target::Open { self.pointer_pos } else { None };
        self.panel.step(dt, pointer);
        let mut draw = self.panel.draw(paint);
        // Settings <-> modules (Max, 2026-10-03: the first motion "sucks"):
        // the two fields cross as wholes — the old one sinks back a little
        // and fades, the new one settles in from a touch closer and fades up.
        if self.panel_swap_t < 1.0 {
            self.panel_swap_t = (self.panel_swap_t + dt / PANEL_SWAP_SECS).min(1.0);
            let t = self.panel_swap_t;
            let ease = |x: f32| 1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3);
            let centre = (field.x + field.w / 2.0, field.y + field.h * 0.45);
            let k_in = ease((t - 0.18) / 0.82);
            transform_draw(&mut draw, centre, 1.04 - 0.04 * k_in, k_in);
            self.panel_parked.field = field;
            self.panel_parked.step(dt, None);
            let mut old = self.panel_parked.draw(paint);
            let k_out = ease(t / 0.62);
            transform_draw(&mut old, centre, 1.0 - 0.06 * k_out, 1.0 - k_out);
            old.rects.append(&mut draw.rects);
            old.labels.append(&mut draw.labels);
            old.glows.append(&mut draw.glows);
            old.clipped.append(&mut draw.clipped);
            draw = old;
            self.schedule_frame();
        }
        draw
    }

    /// Wheel over the open panel: move through its layers.
    pub(crate) fn panel_wheel(&mut self, value: f64) {
        self.panel.wheel(value);
        self.schedule_frame();
    }

    /// A click on the panel (below the dock band, inside the card).
    /// A control from the apps grid's search: the card becomes the control
    /// panel with that setting opening out of its pill (a use in the game).
    pub(crate) fn open_control(&mut self, label: &'static str) {
        info!("control panel: {label} from the apps search");
        self.close_group();
        // The Controls row holds settings, never modules.
        self.panel_switch(false);
        if self.panel_reset_due {
            // Out of sight until now: start it fresh before opening.
            self.panel.reset();
            self.panel_reset_due = false;
        }
        self.control_panel = true;
        self.control_panel_from_apps = true;
        self.search.open = false;
        self.search.query.clear();
        self.refilter();
        self.panel_search();
        self.panel.open_named(label);
        self.schedule_frame();
    }

    /// The pointer moved with the button held: a press that began on a
    /// slider drags its knob.
    pub(crate) fn panel_drag(&mut self, pos: (f32, f32)) {
        let Some(from) = self.gesture.press_pos else { return };
        self.slider_to(from, pos, false);
    }

    /// A press that began on a slider at `from`, now at `pos`: the knob
    /// goes to the stop under the pointer's x (the control panel's or,
    /// `quick`, the quick box's).
    pub(crate) fn slider_to(&mut self, from: (f32, f32), pos: (f32, f32), quick: bool) {
        let panel = if quick { &self.quick } else { &self.panel };
        if let Some((group, index)) = panel.slide(from, pos) {
            self.control_pick(group, index, quick);
            self.schedule_frame();
        }
    }

    pub(crate) fn panel_click(&mut self, pos: (f32, f32)) {
        if self.panel.click(pos) {
            // A module's Apply: the wanted set goes to the system side, and
            // the card folds away.
            if let Some((module, add, remove)) = self.panel.pending_apply.take() {
                self.modules_apply(module, &add, &remove);
                self.handle_command(Command::Collapse);
                return;
            }
            match self.panel.take_action() {
                Some(PanelAction::Scale(scale)) => self.set_display_scale(scale),
                Some(PanelAction::Mode { w, h, hz }) => self.set_display_mode(w, h, hz),
                Some(PanelAction::Keep) => self.keep_display(),
                Some(PanelAction::Revert) => self.revert_display(),
                Some(PanelAction::Pick { group, index }) => self.control_pick(group, index, false),
                None => {}
            }
            self.schedule_frame();
        }
    }

    /// Hand the settings' worker a command, starting it at the first one.
    fn controls_send(&mut self, cmd: ControlCommand) {
        if self.controls.is_none() {
            let (tx, rx) = calloop::channel::channel::<ControlEvent>();
            let _ = self.loop_handle.insert_source(rx, |ev, _, app: &mut App| {
                if let calloop::channel::Event::Msg(e) = ev {
                    app.on_control_event(e);
                }
            });
            self.controls = Some(crate::controls::spawn(tx));
        }
        if let Some(h) = &self.controls {
            h.send(cmd);
        }
    }

    /// A text field of Colours is being typed in (and Colours is up).
    pub(crate) fn colours_typing(&self) -> bool {
        let on = |p: &Panel| p.open_control_label() == Some("Colours");
        crate::controls::TYPING.lock().is_ok_and(|t| t.is_some()) && (on(&self.panel) || on(&self.quick))
    }

    /// A key while a field of Colours has the keys: letters go in,
    /// Backspace takes one out, Enter keeps it, Escape leaves it.
    pub(crate) fn colours_type(&mut self, keysym: Keysym, utf8: Option<&str>) {
        let Some((key, mut text)) = crate::controls::TYPING.lock().ok().and_then(|t| t.clone()) else {
            return;
        };
        let mut done = false;
        let mut leave = false;
        match keysym {
            Keysym::Escape => leave = true,
            Keysym::Return | Keysym::KP_Enter => {
                done = true;
                leave = true;
            }
            Keysym::BackSpace => {
                text.pop();
            }
            _ => match utf8 {
                Some(t) if !t.is_empty() && t.chars().all(|c| !c.is_control()) && text.chars().count() < 40 => text.push_str(t),
                _ => return,
            },
        }
        crate::controls::field_typed(key, &text, done);
        if let Ok(mut t) = crate::controls::TYPING.lock() {
            *t = if leave { None } else { Some((key, text)) };
        }
        if done && key != "colors-apps-find" {
            crate::palette::set_preview(crate::controls::editor_draft());
            self.colors_apply(false);
        }
        let ui = self.ui_facts();
        self.controls_send(ControlCommand::Read("Colours", ui));
        self.schedule_frame();
    }

    /// The editor's preview lasts only while the editor is up: a panel
    /// closed (or another setting opened) mid-edit puts the palette in use
    /// back.
    pub(crate) fn colours_preview_guard(&mut self) {
        self.colours_preview_check(false);
    }

    /// `closing`: the card is going away, whatever it shows.
    pub(crate) fn colours_preview_check(&mut self, closing: bool) {
        if !crate::palette::previewing() {
            return;
        }
        let editing = crate::controls::EDITOR.lock().is_ok_and(|g| g.is_some());
        let shown = self.ui.target() == Target::Open || self.fast.boxed.is_some();
        let on = |p: &Panel| p.open_control_label() == Some("Colours");
        if closing || !(editing && shown && (on(&self.panel) || on(&self.quick))) {
            crate::palette::set_preview(None);
            if let Ok(mut e) = crate::controls::EDITOR.lock() {
                *e = None;
            }
            self.colors_apply(false);
        }
    }

    /// What only this thread knows about the settings.
    fn ui_facts(&self) -> UiFacts {
        let stale = self.brain.as_ref().is_some_and(|c| c.deploy.stale_generation);
        UiFacts {
            dnd: self.notif_muted(),
            night_k: self.screen.temperature_k.unwrap_or(crate::screen::NEUTRAL_K),
            updates: crate::i18n::tr(if stale { tr("Restart to finish updating") } else { tr("Up to date") }).to_owned(),
            has_bt: self.stats_page_for(PageKind::Bt).is_some(),
            has_battery: self.stats_page_for(PageKind::Battery).is_some(),
            apps: self.colours_apps(),
        }
    }

    /// The grid's apps for the Colours list, most used first (a webapp only
    /// once it is installed).
    fn colours_apps(&self) -> Vec<(String, String)> {
        let mut apps: Vec<(u32, String, String)> = self
            .entries
            .iter()
            .enumerate()
            // Real apps only: a .desktop of its own (not a home folder, an
            // icon asset or the shell's own tiles), with a name.
            .filter(|(i, e)| self.kinds.get(*i) == Some(&crate::apps::EntryKind::App) && e.path.is_some() && !e.name.trim().is_empty())
            .map(|(_, e)| e)
            .filter(|e| !e.id.starts_with("golem-") && !e.id.starts_with("waverunner"))
            .filter(|e| !e.id.starts_with("webapp-") || self.managed_webapps.contains(&e.id["webapp-".len()..]))
            .map(|e| (self.usage.count(&e.id), e.id.clone(), e.name.clone()))
            .collect();
        apps.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase())));
        apps.dedup_by(|a, b| a.1 == b.1);
        apps.into_iter().map(|(_, id, name)| (id, name)).collect()
    }

    fn on_control_event(&mut self, ev: ControlEvent) {
        match ev {
            ControlEvent::View(view) => {
                // The settings field's open setting, or the quick box's.
                if self.settings_panel_mut().set_control(view.clone()) || self.quick.set_control(view) {
                    self.schedule_frame();
                }
            }
            ControlEvent::Zone(zone) => {
                crate::controls::adopt_zone(Some(&zone));
                self.schedule_frame();
                self.draw_options();
            }
            ControlEvent::Language => {
                crate::i18n::init();
                info!("control panel: the shell speaks another language from now");
                self.panel.relabel();
                self.panel_parked.relabel();
                self.quick.relabel();
                self.quick.control_ask = true;
                // The open setting was read in the old language.
                self.settings_panel_mut().control_ask = true;
                self.schedule_frame();
                self.draw_options();
            }
        }
    }

    /// [`Self::control_pick`] for the `display pick` verb: says what is
    /// there when the place is empty.
    pub(crate) fn control_pick_at(&mut self, group: u8, index: u8) {
        if let Some(view) = self.panel.control_view() {
            let shape: Vec<String> =
                view.groups.iter().map(|g| format!("{}×{}", g.key, g.choices.len())).collect();
            info!("control panel: {} · {:?} · groups {shape:?}", view.label, view.subtitle);
        }
        // (The quick box's setting, when that is what is up.)
        let quick = self.fast.boxed.is_some();
        if quick {
            if let Some(view) = self.quick.control_view() {
                info!("quick controls: {} · {:?}", view.label, view.subtitle);
            }
        }
        self.control_pick(group, index, quick);
        self.schedule_frame();
    }

    /// A choice of an open setting was clicked: the ones that are this
    /// thread's to make are made here, the rest go to the worker; either
    /// way the setting is read again.
    fn control_pick(&mut self, group: u8, index: u8, quick: bool) {
        let panel = if quick { &mut self.quick } else { &mut self.panel };
        let Some((label, key, value)) = panel.control_pick(group, index) else {
            return;
        };
        info!("control panel: {label}: {key} = {value}");
        if key == "wallpaper" {
            self.wallpaper_changing(&value);
        }
        if key == "dark" {
            // (The worker writes it for the apps; the dock's own look is here.)
            self.look_set(value == "on");
        }
        if !crate::controls::ui_key(key) {
            let ui = self.ui_facts();
            self.controls_send(ControlCommand::Set { label, key, value, ui });
            return;
        }
        match key {
            "night" => {
                if let Ok(k) = value.parse::<u32>() {
                    self.set_screen_temperature(k.clamp(1000, crate::screen::NEUTRAL_K));
                }
            }
            // A click on a text field: it takes the keys (`colours_type`).
            "colors-edit-hex" | "colors-edit-name" | "colors-apps-find" => {
                if let Ok(mut t) = crate::controls::TYPING.lock() {
                    *t = crate::controls::field_start(key);
                }
            }
            // The editor's two fields on one row: the value names the field.
            "colors-edit-text" => {
                if let Ok(mut t) = crate::controls::TYPING.lock() {
                    *t = crate::controls::field_start(&value);
                }
            }
            // The "+" tile after the palettes opens the editor.
            "colors-palette" if value == "new" => {
                crate::controls::editor_open();
                crate::palette::set_preview(crate::controls::editor_draft());
                self.colors_apply(false);
            }
            "colors-edit" | "colors-edit-role" | "colors-edit-colour" | "colors-edit-hue" => {
                let mut saved = None;
                crate::controls::editor_pick(key, &value, &mut saved);
                // Worn while it is made; Save or Cancel takes the preview off.
                crate::palette::set_preview(crate::controls::editor_draft());
                if let Some(id) = saved {
                    self.colors_pick("colors-palette", &id);
                } else {
                    self.colors_apply(false);
                }
            }
            // The "+" beside Apps, and the list's Back.
            "colors-apps-look" if value == "list" => {
                crate::controls::APPS_LIST.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            "colors-apps-list" => {
                crate::controls::APPS_LIST.store(false, std::sync::atomic::Ordering::Relaxed);
                crate::controls::field_typed("colors-apps-find", "", false);
                if let Ok(mut t) = crate::controls::TYPING.lock() {
                    *t = None;
                }
            }
            "colors-skip" | "colors-apps" | "colors-palette" | "colors-web" | "colors-icons" | "colors-apps-look" | "colors-web-look" | "colors-icons-look" => {
                self.colors_pick(key, &value);
            }
            "dnd" => {
                if self.notif_muted() != (value == "on") {
                    self.toggle_notif_mute();
                }
            }
            "open" => {
                let kind = match value.as_str() {
                    "net" => PageKind::Net,
                    "bt" => PageKind::Bt,
                    "bat" => PageKind::Battery,
                    _ => PageKind::Gear,
                };
                // The card (or the quick box) folds away and the gear box
                // takes over.
                if quick {
                    self.close_fast_launch();
                } else {
                    self.handle_command(Command::Collapse);
                }
                if !self.stats_show_page(kind) {
                    info!("control panel: this machine has no {value} page");
                }
                return;
            }
            _ => {}
        }
        let ui = self.ui_facts();
        self.controls_send(ControlCommand::Read(label, ui));
    }

    /// THE QUICK BOX (Super+H, see `fast_launch.rs`): one setting in a box
    /// of its own mid-screen. Opens `label` growing out of `from` (the quick
    /// controls' card, in surface coordinates).
    pub(crate) fn quick_open(&mut self, label: &'static str, from: Rect) {
        let rect = self.quick_rect();
        self.quick_ensure(rect);
        let from = Rect::new(from.x - rect.x, from.y - rect.y, from.w, from.h);
        if self.quick.open_solo(label, from) {
            self.settings_panel_mut().note_use(label);
            info!("quick controls: opening {label}");
        }
        self.schedule_frame();
    }

    /// Where the quick box is: centred where the quick controls' card is, as
    /// wide as a setting needs and as tall as the one that is open.
    pub(crate) fn quick_rect(&self) -> Rect {
        let s = self.options_scale() * self.fast_zoom();
        let g = self.fast_geometry();
        let (sw, sh) = (self.buffer_size.0 as f32, self.buffer_size.1 as f32);
        // A setting made of pictures takes nearly the whole screen (Max,
        // 2026-10-13: "casi tan grande como la pantalla completa").
        // (With no picture to show it is a box like any other: one line of
        // words in a box the screen's size read as something broken.)
        let whole = self.quick.open_is("Wallpaper") && self.quick.open_has_tiles();
        if whole {
            // From under the bar to clear of the dock, nearly the screen's
            // width (Max, 2026-10-14: "make the box a little wider and move
            // it a few pixels up (i dont want it to touch the dock)" — it
            // was 90 % of the screen each way, centred, and its foot lay on
            // the dock's top edge).
            let (top, room) = self.quick_wall_room();
            let w = (sw * QUICK_WHOLE).max(200.0);
            // As tall as its wall and no taller, in the middle of that room
            // (Max, 2026-10-14: "hay mucho real estate que estamos
            // desperdiciando en la caja, sobre todo abajo" — it ran to the
            // dock whatever was in it). Not laid out yet: as it last was.
            let wall = self.quick.open_height().filter(|h| *h > WALL_TOP * s + 1.0).or(self.quick.wall_h);
            let h = wall.map_or(room, |h| (h + WALL_FOOT * s).min(room)).max(QUICK_MIN_H * s);
            let cx = g.card.x + g.card.w / 2.0;
            return Rect::new(cx - w / 2.0, top + (room - h) / 2.0, w, h);
        }
        // Colours with its palettes up: a wide, short box (a horizontal
        // rectangle — Max, 2026-10-16).
        let wide = self.quick.open_is("Colours") && self.quick.open_has_tiles();
        let (w, h) = {
            (
                if wide { (sw * QUICK_COLOURS_W).max(QUICK_W * s).min(sw - 40.0 * s) } else { (QUICK_W * s).min(sw - 40.0 * s).max(200.0) },
                (self.quick.open_height().unwrap_or(0.0) + QUICK_FOOT * s).max(QUICK_MIN_H * s).min(sh - 20.0 * s),
            )
        };
        let (cx, cy) = (g.card.x + g.card.w / 2.0, g.card.y + g.card.h / 2.0);
        let y = (cy - h / 2.0).clamp(10.0 * s, (sh - h - 10.0 * s).max(10.0 * s));
        Rect::new(cx - w / 2.0, y, w, h)
    }

    /// Where a wall of pictures may stand: from under the bar to clear of
    /// the dock — its top, and how tall that is.
    /// How wide a wallpaper's preview is in the wallpaper box (as
    /// `flow_tiles` makes it there: the biggest a page of them fits).
    pub(crate) fn wallpaper_tile_w(&self) -> f32 {
        let s = self.options_scale() * self.fast_zoom();
        let sw = self.buffer_size.0 as f32;
        let room_w = ((sw * QUICK_WHOLE).max(200.0) - 2.0 * MARGIN * s).max(100.0);
        let (_, room) = self.quick_wall_room();
        let gap = TILE_GAP * s;
        let room_h = (room - WALL_TOP * s - 40.0 * s - TILE_DOTS_H * s).max(TILE_ASPECT * 80.0 * s);
        let mut best = 0.0f32;
        for cols in 2..=12usize {
            let w = ((room_w - (cols - 1) as f32 * gap) / cols as f32).min(TILE_MAX_W * s);
            let rows = (((room_h + gap) / (w * TILE_ASPECT + gap)).floor() as usize).max(1);
            if cols * rows >= TILES_PER_PAGE && w > best {
                best = w;
            }
        }
        best
    }

    fn quick_wall_room(&self) -> (f32, f32) {
        let s = self.options_scale() * self.fast_zoom();
        let sh = self.buffer_size.1 as f32;
        let screen_h = self.output_logical_height().unwrap_or(sh).min(sh);
        let (dock_h, _) = self.scaled_extents();
        let top = self.options_pill_h() + 16.0 * s;
        (top, (screen_h - dock_h - QUICK_OFF_DOCK * s - top).max(QUICK_MIN_H * s))
    }

    fn quick_ensure(&mut self, rect: Rect) {
        // (The wall is laid out for the room it MAY take; the box then hugs
        // what it took.)
        self.quick.tile_room_h = if self.quick.open_is("Wallpaper") { self.quick_wall_room().1 } else { 0.0 };
        self.quick.wall_tile = self.wallpaper_tile_w();
        if let Some(h) = self.quick.open_height().filter(|_| self.quick.open_is("Wallpaper") && self.quick.open_has_tiles()) {
            self.quick.wall_h = Some(h);
        }
        let scale = self.options_scale() * self.fast_zoom();
        let pill_h = self.options_pill_h() * self.fast_zoom();
        if let Some(r) = self.renderer.as_mut() {
            let mut measure = |t: &str, px: f32| r.measure_text(t, px, TEXT_FONT);
            // (Laid out for the box's WIDTH only: its height follows the
            // setting, and a new height must not lay the field out again.)
            self.quick.ensure((rect.w, QUICK_MIN_H * scale), scale, pill_h, &mut measure);
        }
        self.quick.field = rect;
    }

    /// The quick box of `label` was left.
    pub(crate) fn quick_left(&mut self, label: &'static str) {
        if label == "Wallpaper" {
            self.pictures_rest();
            let ui = self.ui_facts();
            self.controls_send(ControlCommand::Set { label, key: "wallpaper-leave", value: String::new(), ui });
        }
    }

    /// One frame of the quick box: its rect at rest, how present it is
    /// (0..1), how big of that rect it is drawn (about its centre) and what
    /// it draws. `None`: no box.
    pub(crate) fn quick_frame(&mut self, dt: f32, paint: PanelPaint) -> Option<(Rect, f32, f32, PanelDraw)> {
        if !self.quick.solo_up() {
            return None;
        }
        let rect = self.quick_rect();
        self.quick_ensure(rect);
        if self.quick.display_due() {
            let view = self.display_view();
            self.quick.set_display(view);
        }
        self.quick.keep_left = self.display_keep_left();
        if let Some(label) = self.quick.control_due() {
            let ui = self.ui_facts();
            self.controls_send(ControlCommand::Read(label, ui));
        }
        let pointer = self.pointer_pos;
        self.quick.step(dt, pointer);
        let (scale, a) = self.quick.solo_pose();
        // Gone: the quick controls are over.
        if !self.quick.solo_up() {
            if let Some(label) = self.fast.boxed.take() {
                self.quick_left(label);
                if self.fast.open {
                    self.close_fast_launch();
                }
            }
            return None;
        }
        let draw = self.quick.draw(paint);
        Some((rect, a, scale, draw))
    }

    /// A key while the quick box is up: the arrows and Tab walk its
    /// choices, Enter or Space picks, Escape closes it — the box and the
    /// quick controls under it, at once (Max, 2026-10-13: "esc debería
    /// cerrar todo"). Nothing is typed here, so the left hand's keys are
    /// keys too (`left_hand`).
    pub(crate) fn quick_key(&mut self, keysym: Keysym) {
        let keysym = if self.modifiers.ctrl { keysym } else { left_hand(keysym) };
        if !self.panel_keys(keysym, true) && keysym == Keysym::Escape {
            self.close_fast_launch();
        }
        self.schedule_frame();
    }

    /// The keys of an open setting — the control panel's (`quick` false) or
    /// the quick box's: arrows and Tab walk the choices, Enter or Space
    /// picks the one they are on. Returns whether the key was one of these
    /// and there was a choice to act on.
    pub(crate) fn panel_keys(&mut self, keysym: Keysym, quick: bool) -> bool {
        let panel = if quick { &mut self.quick } else { &mut self.panel };
        let used = match keysym {
            Keysym::Left => panel.key_move(-1, 0),
            Keysym::Right | Keysym::Tab => panel.key_move(1, 0),
            Keysym::ISO_Left_Tab => panel.key_move(-1, 0),
            Keysym::Up => panel.key_move(0, -1),
            Keysym::Down => panel.key_move(0, 1),
            Keysym::Return | Keysym::KP_Enter | Keysym::space => panel.key_pick(),
            _ => false,
        };
        if used {
            match panel.take_action() {
                Some(PanelAction::Scale(scale)) => self.set_display_scale(scale),
                Some(PanelAction::Mode { w, h, hz }) => self.set_display_mode(w, h, hz),
                Some(PanelAction::Keep) => self.keep_display(),
                Some(PanelAction::Revert) => self.revert_display(),
                Some(PanelAction::Pick { group, index }) => self.control_pick(group, index, quick),
                None => {}
            }
            self.schedule_frame();
        }
        used
    }

    /// A click while the quick box is up: a choice is picked; anywhere
    /// else — the box's own empty space or outside it — closes it.
    pub(crate) fn quick_click(&mut self, pos: (f32, f32)) {
        let rect = self.quick_rect();
        if !rect.contains(pos) {
            self.close_fast_launch();
            return;
        }
        let Some(label) = self.quick.open.as_ref().map(|o| self.quick.pills[o.pill].label) else {
            return;
        };
        match self.quick.open_hit(label, pos) {
            Some(Some(action)) => match action {
                PanelAction::Scale(scale) => self.set_display_scale(scale),
                PanelAction::Mode { w, h, hz } => self.set_display_mode(w, h, hz),
                PanelAction::Keep => self.keep_display(),
                PanelAction::Revert => self.revert_display(),
                PanelAction::Pick { group, index } => self.control_pick(group, index, true),
            },
            // On no choice: the box's own empty space closes it, as its
            // last line says (Max, 2026-10-14: "the box says click empty
            // space closes it and it does not" — only a click OUTSIDE the
            // box did).
            _ => {
                self.close_fast_launch();
                return;
            }
        }
        self.schedule_frame();
    }

    /// Open the control panel on the setting `label`, from wherever the
    /// card is (the `display show …` verb: a key binding's way straight to
    /// a setting, and the pointer-free way to look at one).
    pub(crate) fn show_control(&mut self, label: &'static str) {
        if self.ui.target() == Target::Open && self.control_panel && self.panel.is_modules() {
            self.panel_switch(false);
        }
        if self.ui.target() != Target::Open {
            self.toggle_control_panel();
        } else if !self.control_panel {
            self.open_control(label);
            return;
        }
        // Another setting open: this one takes its place.
        if self.panel.open.as_ref().is_some_and(|o| self.panel.pills[o.pill].label != label) {
            self.panel.open = None;
        }
        self.panel.open_named(label);
        self.schedule_frame();
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

    #[test]
    fn the_left_hand_walks_opens_and_goes_back() {
        assert_eq!(left_hand(Keysym::w), Keysym::Up);
        assert_eq!(left_hand(Keysym::A), Keysym::Left);
        assert_eq!(left_hand(Keysym::s), Keysym::Down);
        assert_eq!(left_hand(Keysym::D), Keysym::Right);
        assert_eq!(left_hand(Keysym::e), Keysym::Return);
        assert_eq!(left_hand(Keysym::q), Keysym::Escape);
        // Every other key is itself.
        assert_eq!(left_hand(Keysym::Tab), Keysym::Tab);
        assert_eq!(left_hand(Keysym::x), Keysym::x);
    }

    /// A panel laid out at bar scale 1 over a typical card field, with
    /// estimated label widths (no text shaper in tests).
    fn panel() -> Panel {
        let mut p = Panel { items: &SETTINGS, ..Panel::default() };
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
        assert_eq!(p.pills.len(), 24);
        let per_layer = [0, 1, 2].map(|l| p.pills.iter().filter(|q| q.layer == l).count());
        assert_eq!(per_layer, [5, 9, 10]);
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
        assert_eq!(counts(&p), [9, 10, 5], "down: the near group goes to the back");
        assert_clean(&p, "one step down");
        p.shift_layers(1);
        assert_eq!(counts(&p), [10, 5, 9]);
        assert_clean(&p, "two steps down");
        p.shift_layers(-1);
        p.shift_layers(-1);
        assert_eq!(counts(&p), [5, 9, 10], "up undoes it");
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
    fn a_reset_brings_back_the_first_layer_order() {
        let mut p = panel();
        let first: Vec<(u8, (f32, f32))> = p.pills.iter().map(|q| (q.layer, q.home)).collect();
        p.shift_layers(1);
        p.set_query("wifi");
        p.reset();
        assert_eq!(p.shift, 0);
        for (q, (layer, home)) in p.pills.iter().zip(&first) {
            assert_eq!(q.layer, *layer, "{} back on its first layer", q.label);
            assert!((q.home.0 - home.0).abs() < 0.5 && (q.home.1 - home.1).abs() < 0.5, "{} back home", q.label);
            assert!(q.exit.is_none() && q.focus == 0.0);
        }
    }

    fn layer_sizes(p: &Panel) -> [usize; 3] {
        let mut n = [0; 3];
        for q in &p.pills {
            n[q.group as usize] += 1;
        }
        n
    }

    #[test]
    fn the_apps_search_finds_controls_best_first() {
        let p = Panel { items: &SETTINGS, ..Panel::default() };
        let audio = p.matching_controls("audio");
        for want in ["Sound output", "Volume", "Microphone"] {
            assert!(audio.contains(&want), "{want} in {audio:?}");
        }
        assert_eq!(p.matching_controls("res").first(), Some(&"Resolution"), "a name prefix first");
        assert!(p.matching_controls("  ").is_empty());
        assert!(p.matching_controls("e").len() <= MAX_SEARCH_CONTROLS);
    }

    #[test]
    fn the_modules_field_lays_out_clean_and_searches_its_own() {
        let mut p = Panel { items: crate::modules::items(), modules: true, ..Panel::default() };
        let mut est = |t: &str, px: f32| crate::options::est_text_w(t, px);
        p.ensure((960.0, 560.0), 1.0, 25.0, &mut est);
        assert!(p.is_modules());
        assert_eq!(p.pills.len(), crate::modules::items().len());
        assert_clean(&p, "the modules field");
        assert_eq!(p.matching_controls("steam").first(), Some(&"Gaming"), "a keyword finds its module");
        assert!(p.matching_controls("resolution").is_empty(), "no settings in the modules");
    }

    #[test]
    fn use_brings_a_setting_closer_one_layer_at_a_time() {
        let mut p = panel();
        let now = 1_000_000;
        let far = p.pills.iter().position(|q| q.group == 2).unwrap();
        // Seeded: a never-used middle setting holds 1, a near one 2.
        p.record_use(far, now);
        assert_eq!(p.pills[far].group, 2, "one use does not yet pass a middle setting");
        p.record_use(far, now);
        assert_eq!(p.pills[far].group, 1, "two uses step it to the middle");
        assert_eq!(layer_sizes(&p), [5, 9, 10], "the layers keep their sizes");
        p.record_use(far, now);
        assert_eq!(p.pills[far].group, 0, "a third steps it to the front");
        assert_eq!(layer_sizes(&p), [5, 9, 10]);
        // Whoever it passed stepped back exactly one layer.
        let back: Vec<(&str, u8, u8)> = p
            .pills
            .iter()
            .zip(SETTINGS)
            .filter(|(q, s)| q.group != s.1 && q.label != p.pills[far].label)
            .map(|(q, s)| (q.label, s.1, q.group))
            .collect();
        assert_eq!(back.len(), 2, "two settings stepped back: {back:?}");
        assert!(back.iter().all(|&(_, from, to)| to == from + 1), "{back:?}");
    }

    #[test]
    fn old_uses_fade_and_the_arrangement_is_kept() {
        let st = Standing { layer: 1, score: 4.0, at: 0 };
        let two_weeks = (USE_HALF_LIFE_DAYS * 86_400.0) as u64;
        assert!((st.score_at(two_weeks) - 2.0).abs() < 1e-9, "a use counts half after the half-life");

        let mut p = panel();
        let far = p.pills.iter().position(|q| q.group == 2).unwrap();
        p.record_use(far, 1_000);
        p.record_use(far, 1_000);
        let json = serde_json::to_string(&p.usage).unwrap();
        let mut again = Panel { items: &SETTINGS, usage: serde_json::from_str(&json).unwrap(), ..Panel::default() };
        let mut est = |t: &str, px: f32| crate::options::est_text_w(t, px);
        again.ensure((960.0, 560.0), 1.0, 25.0, &mut est);
        let groups = |p: &Panel| p.pills.iter().map(|q| q.group).collect::<Vec<_>>();
        assert_eq!(groups(&again), groups(&p), "the saved arrangement comes back");
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

    /// The panel with `label` open and the MacBook's screen read into it.
    fn display_open(label: &'static str, scale: f64) -> Panel {
        let mut p = panel();
        p.field = Rect::new(40.0, 90.0, 960.0, 560.0);
        p.step(0.0, None);
        p.open_named(label);
        assert!(p.display_due(), "an opened display setting asks for the screen");
        let mode = crate::display::Mode { w: 1440, h: 900, hz: 60.0 };
        p.set_display(Some(DisplayView {
            screen: "Built-in screen".to_owned(),
            mode,
            scale,
            stops: crate::display::scale_stops(1440, 900, scale),
            sizes: vec![(1440, 900), (1280, 800), (1024, 640)],
            rates: vec![60.0],
        }));
        for _ in 0..40 {
            p.step(1.0 / 60.0, None);
        }
        p
    }

    fn centre(p: &Panel, chip: &Chip) -> (f32, f32) {
        (p.field.x + chip.rect.x + chip.rect.w / 2.0, p.field.y + chip.rect.y + chip.rect.h / 2.0)
    }

    #[test]
    fn scale_offers_the_screens_stops_with_the_one_in_use_lit() {
        let mut p = display_open("Scale", 1.25);
        let content = p.open_content("Scale").expect("Scale is a real setting");
        assert_eq!(content.subtitle, "Built-in screen · looks like 1152 × 720");
        let texts: Vec<&str> = content.chips.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["75%", "83%", "90%", "100%", "113%", "125%", "150%"]);
        let lit: Vec<&str> = content.chips.iter().filter(|c| c.on).map(|c| c.text.as_str()).collect();
        assert_eq!(lit, ["125%"]);
        for (i, a) in content.chips.iter().enumerate() {
            assert!(a.rect.x >= MARGIN && a.rect.x + a.rect.w <= p.key.0 - MARGIN, "{} leaves the field", a.text);
            for b in content.chips.iter().skip(i + 1) {
                let apart = a.rect.x + a.rect.w <= b.rect.x || b.rect.x + b.rect.w <= a.rect.x || a.rect.y != b.rect.y;
                assert!(apart, "{} and {} overlap", a.text, b.text);
            }
        }
        // A click on a choice asks for it and leaves the setting open.
        let chip = &content.chips[6];
        assert!(p.click(centre(&p, chip)));
        assert_eq!(p.take_action(), Some(PanelAction::Scale(1.5)));
        assert_eq!(p.take_action(), None, "asked once");
        assert!(p.open.as_ref().is_some_and(|o| o.want > 0.5));
        // A click on the empty space folds it back, asking for nothing.
        assert!(p.click((p.field.x + 5.0, p.field.y + p.field.h - 5.0)));
        assert_eq!(p.take_action(), None);
        assert!(p.open.as_ref().is_some_and(|o| o.want == 0.0));
    }

    #[test]
    fn a_narrow_field_wraps_the_choices_and_keeps_them_inside() {
        let mut p = display_open("Scale", 1.0);
        p.key = (400.0, 560.0, 1.0);
        let content = p.open_content("Scale").unwrap();
        let rows: std::collections::BTreeSet<i32> = content.chips.iter().map(|c| c.rect.y as i32).collect();
        assert!(rows.len() > 1, "seven pills do not fit one 400px row");
        let per_row: Vec<usize> = rows.iter().map(|y| content.chips.iter().filter(|c| c.rect.y as i32 == *y).count()).collect();
        assert!(per_row.iter().max().unwrap() - per_row.iter().min().unwrap() <= 1, "rows share the pills evenly: {per_row:?}");
        for c in &content.chips {
            assert!(c.rect.x >= MARGIN - 0.5 && c.rect.x + c.rect.w <= 400.0 - MARGIN + 0.5, "{} leaves the field", c.text);
        }
        assert!(content.foot_y > content.chips.last().unwrap().rect.y + CHIP_H);
    }

    #[test]
    fn resolution_offers_the_sizes_and_a_tried_one_asks_to_be_kept() {
        let mut p = display_open("Resolution", 1.0);
        let content = p.open_content("Resolution").unwrap();
        assert_eq!(content.subtitle, "Built-in screen · 60 Hz");
        let texts: Vec<&str> = content.chips.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["1440 × 900", "1280 × 800", "1024 × 640"], "one rate: no refresh row");
        assert!(content.chips[0].on && !content.chips[1].on);
        assert!(p.click(centre(&p, &content.chips[1])));
        assert_eq!(p.take_action(), Some(PanelAction::Mode { w: 1280, h: 800, hz: None }));
        // While it is being tried: the question, Keep and Go back.
        p.keep_left = Some(11.2);
        let content = p.open_content("Resolution").unwrap();
        assert_eq!(content.captions.len(), 1);
        assert_eq!(content.captions[0].0, "Keep this resolution? Going back in 12 s");
        let (keep, back) = (&content.chips[3], &content.chips[4]);
        assert_eq!((keep.text.as_str(), back.text.as_str()), ("Keep", "Go back"));
        assert!(content.captions[0].1 > content.chips[2].rect.y + CHIP_H && keep.rect.y > content.captions[0].1);
        assert!(p.click(centre(&p, keep)));
        assert_eq!(p.take_action(), Some(PanelAction::Keep));
        assert!(p.click(centre(&p, back)));
        assert_eq!(p.take_action(), Some(PanelAction::Revert));
        // Two rates: a refresh row for the size in use.
        if let Some(v) = &mut p.display {
            v.rates = vec![60.0, 165.0];
        }
        p.keep_left = None;
        let content = p.open_content("Resolution").unwrap();
        assert_eq!(content.captions[0].0, "Refresh rate");
        let rate = content.chips.iter().find(|c| c.text == "165 Hz").expect("the other rate is offered");
        assert_eq!(rate.action, PanelAction::Mode { w: 1440, h: 900, hz: Some(165.0) });
    }

    #[test]
    fn a_screen_with_one_resolution_says_so() {
        let mut p = display_open("Resolution", 1.0);
        if let Some(v) = &mut p.display {
            v.sizes = vec![(1440, 900)];
        }
        let content = p.open_content("Resolution").unwrap();
        assert_eq!(content.chips.len(), 1);
        assert!(content.chips[0].on);
        assert_eq!(content.captions.len(), 1);
        assert!(content.captions[0].0.starts_with("This screen has one resolution"));
        assert!(content.captions[0].1 > content.chips[0].rect.y);
    }

    #[test]
    fn an_open_display_setting_draws_its_choices() {
        let p = display_open("Scale", 1.0);
        let draw = p.draw(PanelPaint { ink: [1.0; 4], bright: false });
        for want in ["Scale", "Built-in screen · looks like 1440 × 900", "100%", "150%"] {
            assert!(draw.labels.iter().any(|l| l.text == want), "{want:?} is drawn");
        }
        // The one in use is underlined (the hairline language: no rings).
        assert_eq!(draw.rects.iter().filter(|r| r.border > 0.0).count(), 0);
        assert_eq!(draw.rects.iter().filter(|r| r.rect.h <= 2.0 && r.rect.w > 4.0).count(), 1);
    }
}

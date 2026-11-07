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
//! THE GRID (Max, 2026-11-06: "cambiar el look de controls por iconos como
//! el grid de apps"; 2026-11-07: "que se parezca y comporte como el grid de
//! apps… todo el mismo tamaño y disposición y comportamiento"): the field
//! is THE APPS GRID — its cells, its plates, its names, its box under the
//! pointer, its keys (`content.rs`'s own numbers at the box's icon scale),
//! each setting's symbol on the dock's plate.
//!
//! IN SECTIONS (Max, 2026-11-07: "que las configuraciones se separen en
//! secciones, como apps, install, files. pero: estilo, energía, seguridad"):
//! the settings stand in [`SECTIONS`], each a titled strip as Apps · Install
//! · Files are (the apps' title word, air and dot room), every setting in
//! its FIXED place ("no más el juego de más usado sube, poné todo en su
//! lugar y queda así": the standings below still count uses for the quick
//! controls' three most used, they move nothing here). The rows grow
//! sideways with the card as the apps' do ("que las líneas se expandan
//! hacia los lados"): a wider dock gives every section more columns and
//! fewer rows. The CARD GROWS to hold every section (`App::apps_trim_target`,
//! as far as the screen over it lets it); where the screen is too short,
//! the field scrolls under the wheel and the keys bring their tile into
//! view. A click grows the tile's plate into the setting; a search leaves
//! only the matches, in their sections, the best one lit for Enter.
//!
//! The settings themselves: Scale and Resolution show the screen's choices
//! (see [`crate::display`]); every other one is `controls.rs`'s.

use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use smithay_client_toolkit::seat::keyboard::Keysym;

use tracing::info;

use crate::content::{self, Label, Layout, Rect, RectInst, ShadowInst};
use crate::controls::{ControlCommand, ControlEvent, ControlView, UiFacts};
use crate::gear::PageKind;
use crate::display::DisplayView;
use crate::options::{FONT_PX, LINE_PX, TEXT_FONT};
use crate::state::Target;
use crate::{apps, groups, App};
use waverunner_proto::Command;
use crate::i18n::tr;

/// One pill of a field: its name, its section (an index into [`SECTIONS`];
/// the modules' field has one section, whatever the number), and its
/// search keywords.
type Item = (&'static str, u8, &'static str);

/// THE SECTIONS (Max, 2026-11-07: "estilo, energía, seguridad, etc."): the
/// strips the settings stand in, top to bottom, each titled as the box's
/// Apps · Install · Files are. Translated where drawn.
const SECTIONS: [&str; 6] = ["Style", "Screen and sound", "Connections", "Power", "Security", "Keys and system"];

/// The settings, each in its section, in the order they stand there — the
/// most reached for first. Fixed: use moves nothing.
const SETTINGS: [Item; 34] = [
    // (name, section, search keywords: what else people call it or look
    // for it by).
    // STYLE: how Golem looks.
    ("Colours", 0, "dark mode light theme appearance night colors colour color accent palette highlight dynamic websites web apps"),
    ("Wallpaper", 0, "background desktop picture image appearance"),
    // How the windows sit: where they open, how they stick, the side column…
    // (Max, 2026-10-14: "a control called Tiles… we will start dumping
    // controls in there").
    ("Tiles", 0, "windows tiling tile floating snap magnets magnetism layout arrange side column"),
    // The dock and its box (Max, 2026-10-20: "we will call it Launcher").
    ("Launcher", 0, "dock launcher apps box hide hidden stay away position left right bottom side monitors screens displays animated icons motion arrange sort order"),
    // SCREEN AND SOUND.
    ("Resolution", 1, "display screen monitor size pixels"),
    ("Scale", 1, "display screen zoom size hidpi text bigger smaller"),
    // The screens together (Max, 2026-10-21): arranged, mirrored, what goes where.
    ("Monitors", 1, "monitors screens displays external tv projector arrange mirror extend isolated rotate turn resolution scale lid"),
    ("Brightness", 1, "display screen backlight dim auto sensor light"),
    ("Night light", 1, "display screen warm blue light sunset eyes night automatic schedule"),
    ("Sound output", 1, "audio speakers speaker headphones output"),
    ("Volume", 1, "audio sound loud quiet mute apps mixer per app"),
    ("Microphone", 1, "audio input mic recording sound"),
    // CONNECTIONS.
    ("Wi-Fi", 2, "wifi wireless network internet connection hotspot share airplane ip address dns wired ethernet"),
    ("Bluetooth", 2, "wireless devices headphones pairing send receive files"),
    ("VPN", 2, "vpn wireguard openvpn tunnel private network connection"),
    // Printing (2026-10-21): the printers, their prints and what a print gets.
    ("Printers", 2, "printers printer print printing prints queue cancel paper ink toner colour color black white quality draft test page pause resume default scanner"),
    // POWER.
    ("Power mode", 3, "battery performance energy saver fans fast slow"),
    ("Battery", 3, "power charge energy health"),
    ("Sleep", 3, "sleep suspend screen off idle lid close minutes power saving"),
    // SECURITY.
    ("Lock screen", 4, "lock screen password sleep idle minutes security away"),
    // THE CONTROLS GAP AUDIT (Max, 2026-11-06, "quiero todo lo necesario, en
    // Controls": what every settings app has and this one lacked): the
    // owner's account, the mouse and touchpad whole, the keys, what starts
    // with the session, the lock screen, accessibility, VPNs; and the
    // choices that lived only in the gear box found from here by their words.
    ("Your account", 4, "user account password name photo picture avatar fingerprint finger login sign in"),
    ("Privacy", 4, "security permissions camera location"),
    ("Updates", 4, "upgrade system software version"),
    // KEYS AND SYSTEM.
    ("Keyboard layout", 5, "input language typing keys layouts switch repeat delay caps lock compose"),
    ("Mouse and touchpad", 5, "trackpad mouse pointer cursor speed tap click right click left handed scrolling natural size"),
    ("Natural scrolling", 5, "touchpad trackpad mouse scroll direction"),
    ("Shortcuts", 5, "keyboard shortcuts keys hotkeys super combinations binds"),
    ("Accessibility", 5, "accessibility text size bigger larger magnifier zoom reduce motion animations contrast pointer cursor screen reader"),
    ("Language", 5, "region locale translation input"),
    ("Time zone", 5, "clock date time region 24 12 hour am pm format"),
    ("Notifications", 5, "alerts do not disturb dnd messages mute silence app quiet"),
    ("Default apps", 5, "applications browser open with"),
    ("Startup apps", 5, "autostart startup login start boot open at start apps run automatically"),
    ("About Golem", 5, "system version info hardware computer name"),
];

/// The setting whose name, without spaces or capitals, is `name`.
pub(crate) fn setting_named(name: &str) -> Option<&'static str> {
    let flat = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    // (Dark mode was a page of its own until 2026-10-15: it is Colours' now.)
    if flat(name) == "darkmode" {
        return Some("Colours");
    }
    // (Touchpad speed was a page of its own until 2026-11-06: the mouse and
    // touchpad have one page now.)
    if flat(name) == "touchpadspeed" {
        return Some("Mouse and touchpad");
    }
    SETTINGS.iter().map(|s| s.0).find(|label| flat(label) == flat(name))
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
/// A palette card's colour at Palette 100 ("full colour"), against as made.
const CARD_FULL: f32 = 0.6;
const TILE_MAX_W: f32 = 420.0;
/// THE CARDS' GRID (the Colours model, Max 2026-10-20: "7×4 cards"): the
/// palettes — and the apps' tiles, the same card — stand as many across as
/// fit at a card at least this wide, four to seven of them (a 960 field
/// falls to four, the hairline mockup's four by three), and four rows, or
/// three when four would run the box past its height.
const PALETTE_TILE_MIN: f32 = 170.0;
const PALETTE_COLS: (usize, usize) = (4, 7);
const PALETTE_ROWS: usize = 4;
const PALETTE_ROWS_FEWER: usize = 3;
/// How long a slider's "cannot" is said in its words' place.
const NOTE_MS: u64 = 2400;
/// …and two, when three still run past (the Apps section's own line of apps
/// under its sliders, at 1440×900).
const PALETTE_ROWS_FEWEST: usize = 2;
/// An app's icon on its card, logical px.
const TILE_ICON: f32 = 72.0;
/// A ROW OF SLIDERS: the air between two columns, the label line's height
/// and the track line's under it.
const SLIDER_COL_GAP: f32 = 44.0;
const SLIDER_LABEL_H: f32 = 26.0;
const SLIDER_TRACK_H: f32 = 32.0;
/// A switch as drawn (`Style::Switch`): its width.
const SWITCH_W: f32 = 36.0;
/// A SPLIT TRACK: the gap at its middle, a notch's width, and where the
/// notches of its two halves stand (each half's "as designed" point).
const SPLIT_GAP: f32 = 18.0;
const NOTCH_W: f32 = 2.0;
const SPLIT_NOTCHES: [f32; 2] = [0.225, 0.775];

/// The quick box (Super+H): its width, the room under its content for the
/// closing hint, and its least height — design pixels.
const QUICK_W: f32 = 620.0;
const QUICK_FOOT: f32 = 44.0;
const QUICK_MIN_H: f32 = 170.0;
/// The share of the screen a setting made of pictures takes.
const QUICK_WHOLE: f32 = 0.955;
/// The Colours or the Launcher box called a hidden dock up (`quick_open`):
/// it hides again when the box is left.
static DOCK_CALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The settings whose box calls a hidden dock up while it is open: what
/// they set is seen on the dock.
pub(crate) fn calls_dock(label: &str) -> bool {
    matches!(label, "Colours" | "Launcher")
}

/// The Colours box's width: a little less than the wallpapers' (Max,
/// 2026-10-20: "make the box a little smaller").
const QUICK_COLOURS: f32 = 0.88;
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
    let fix_symbol = |s: &mut PanelSymbol| {
        s.rect = rect(s.rect);
        s.color[3] *= alpha;
    };
    d.symbols.iter_mut().for_each(fix_symbol);
    for (clip, rects, labels, symbols) in &mut d.clipped {
        *clip = rect(*clip);
        rects.iter_mut().for_each(fix_rect);
        labels.iter_mut().for_each(fix_label);
        symbols.iter_mut().for_each(fix_symbol);
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

// ---- Layout (px at bar scale 1) -----------------------------------------
/// Air kept off the card's sides.
const MARGIN: f32 = 22.0;
/// The Monitors page's drawing: its height, the turning corners' size, and
/// how near (field px) a screen's side comes before it sticks to another's.
const SCREENS_H: f32 = 210.0;
const SCREENS_MIN_H: f32 = 110.0;
const CORNER: f32 = 22.0;
const STICK: f32 = 10.0;
/// The field's largest size (px at bar scale 1): the size it had on Max's
/// card when he set it as the maximum (2026-09-30: "i dont want it wider or
/// taller no matter how big the box gets"). Smaller cards shrink it.
const FIELD_MAX: (f32, f32) = (960.0, 845.0);

// ---- The grid (2026-11-06/07): THE APPS GRID'S OWN -----------------------
// Max, 2026-11-07: "que se parezca y comporte como el grid de apps, los
// iconos los platos, todo el mismo tamaño y disposición y comportamiento":
// the cells, the icons and their plates, the names, the box under the
// pointer, the magnifying, the pages and their dots are `content.rs`'s
// (`GRID_CELL_W`, `GRID_ICON`, `LABEL_FONT_PX`…), at the box's icon scale as
// the apps are — nothing of its own but the symbol on each plate.
/// The symbol's share of its plate (an app's icon fills it; a line symbol
/// wants air round it).
const SYMBOL_SHARE: f32 = 0.6;
/// Air under the last section's dot room: the Search pill's room at the
/// card's foot.
const GRID_BOTTOM: f32 = 56.0;
/// A field too short for every section scrolls: pixels a wheel unit moves
/// it, and how fast it follows.
const SCROLL_WHEEL: f32 = 3.0;
const SCROLL_RATE: f32 = 14.0;
/// The box under the pointer: its inset in the cell and its corner.
const CELL_INSET: f32 = 4.0;
const CELL_RADIUS: f32 = 14.0;
/// The plate's corner as a share of its side (what a setting's box grows
/// out of).
const PLATE_CORNER: f32 = 0.22;

/// The symbol a setting wears on its tile: Golem's own line symbols
/// (`app_actions::SYMBOLS`, the `ctl-*` ones and the menu's), by name; a
/// module, or a setting not listed, the package.
fn symbol_of(label: &str) -> &'static str {
    match label {
        "Resolution" => "ctl-resolution",
        "Scale" => "ctl-scale",
        "Wi-Fi" => "ctl-wifi",
        "Sound output" => "ctl-sound",
        "Colours" => "ctl-colours",
        "Brightness" => "ctl-brightness",
        "Bluetooth" => "ctl-bluetooth",
        "Volume" => "ctl-volume",
        "Wallpaper" => "ctl-wallpaper",
        "Night light" => "ctl-night",
        "Tiles" => "ctl-tiles",
        "Launcher" => "ctl-launcher",
        "Monitors" => "ctl-monitors",
        "Printers" => "ctl-printers",
        "Notifications" => "bell",
        "Keyboard layout" => "ctl-keyboard",
        "Power mode" => "ctl-power",
        "Your account" => "person",
        "Mouse and touchpad" => "ctl-mouse",
        "Shortcuts" => "ctl-shortcuts",
        "Accessibility" => "ctl-access",
        "Startup apps" => "play",
        "Lock screen" => "ctl-lock",
        "Sleep" => "ctl-sleep",
        "VPN" => "ctl-vpn",
        "Language" => "ctl-language",
        "Time zone" => "clock",
        "Natural scrolling" => "ctl-scroll",
        "Updates" => "ctl-updates",
        "About Golem" => "ctl-about",
        "Microphone" => "ctl-mic",
        "Battery" => "ctl-battery",
        "Default apps" => "ctl-apps",
        "Privacy" => "ctl-privacy",
        _ => "package",
    }
}

// ---- Motion (rates are 1/s for exponential approach) --------------------
const GLIDE_RATE: f32 = 24.0;
const SIZE_RATE: f32 = 26.0;
const FADE_IN_RATE: f32 = 22.0;
/// How fast a pill comes closer under the pointer, and by how much: the
/// search pill's and fast launch's (`frame.rs`, `content.rs`).
pub(crate) const LIFT_RATE: f32 = 12.0;
pub(crate) const LIFT: f32 = 0.07;
/// The open-box morph's rate.
const OPEN_RATE: f32 = 14.0;

// ---- The game ------------------------------------------------------------
/// Uses fade: one counts half after this many days.
const USE_HALF_LIFE_DAYS: f64 = 14.0;
/// A setting's standing: its fading score (the quick controls' "three used
/// most"), and the layer it had when the field was layers (kept in the
/// file, read by nothing).
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


fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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

struct Pill {
    label: &'static str,
    /// What else people call it or look for it by (search).
    keywords: &'static str,
    /// In the grid: every tile with no search on, the matches of one (a
    /// miss fades out where it stands, as the apps grid leaves out what a
    /// search does not find).
    shown: bool,
    /// Its slot in the grid, counted across the pages (`Grid::cell_of`).
    slot: usize,
    /// Its name fitted to the cell at the names' type size: on one line (cut
    /// with …) and, when it needs them, on two (the second cut) — drawn on
    /// two only where the cell under it is free, as the apps' names are.
    one: String,
    two: Option<(String, String)>,
    /// The cell's centre the grid wants it at (on its page, as if that page
    /// were the first); `anchor` glides there, `pos` is the drawn centre
    /// this frame.
    home: (f32, f32),
    anchor: (f32, f32),
    pos: (f32, f32),
    /// Size multiplier easing home, and presence (0 while not shown).
    sc: f32,
    op: f32,
}

/// One section of the grid as laid out: its title (none on the modules'
/// field), where its title's line stands, its cells' window, the slot of
/// its first tile and how many it holds.
#[derive(Clone, Debug, Default)]
struct Section {
    title: Option<&'static str>,
    title_y: f32,
    vp: Rect,
    first: usize,
    n: usize,
}

/// The grid as laid out: the apps grid's own numbers (`content::GRID_*`) at
/// the box's icon scale, in the grid field's coordinates (unscrolled).
#[derive(Clone, Debug, Default)]
struct Grid {
    cols: usize,
    /// A cell; the icon's side and how far under the cell's top it sits.
    cell_w: f32,
    cell_h: f32,
    icon: f32,
    icon_top: f32,
    /// The sections, top to bottom — only those with a tile.
    sections: Vec<Section>,
    /// How tall the whole is, the Search pill's room included.
    total_h: f32,
}

impl Grid {
    /// The section slot `k` is in.
    fn section_of(&self, k: usize) -> Option<&Section> {
        self.sections.iter().find(|s| k >= s.first && k < s.first + s.n)
    }

    /// The cell of slot `k`.
    fn cell_of(&self, k: usize) -> Rect {
        let cols = self.cols.max(1);
        let Some(s) = self.section_of(k) else {
            return Rect::new(0.0, 0.0, self.cell_w, self.cell_h);
        };
        let within = k - s.first;
        let (row, col) = (within / cols, within % cols);
        Rect::new(s.vp.x + col as f32 * self.cell_w, s.vp.y + row as f32 * self.cell_h, self.cell_w, self.cell_h)
    }

    /// Whether nothing of the grid stands under slot `k`'s cell (its
    /// section's last row, or no tile in the slot below): a second line
    /// of name has room there.
    fn free_under(&self, k: usize) -> bool {
        let cols = self.cols.max(1);
        let Some(s) = self.section_of(k) else {
            return true;
        };
        let within = k - s.first;
        within / cols == (s.n - 1) / cols || within + cols >= s.n
    }
}

/// The grid for sections of `counts` tiles (each with its title, `None` =
/// no word over it) in a field `w` wide at the box's icon scale: as many
/// cells across as the apps grid puts in a card this wide, each section as
/// many rows as its tiles need at that, stacked with the air the box's
/// sections have (the title's line, the dot room, the gap), centred.
fn grid_metrics(counts: &[(Option<&'static str>, usize)], w: f32, icon_scale: f32) -> Grid {
    let (cell_w, cell_h) = (content::GRID_CELL_W * icon_scale, content::GRID_CELL_H * icon_scale);
    let inner_w = (w - 2.0 * content::GRID_PAD_X).max(cell_w);
    let cols = ((inner_w / cell_w).floor() as usize).max(1);
    let vp_w = cols as f32 * cell_w;
    let x0 = (w - vp_w) / 2.0;
    let mut y = content::GRID_TOP_GAP;
    let mut sections = Vec::new();
    let mut first = 0;
    for &(title, n) in counts {
        if n == 0 {
            continue;
        }
        let rows = n.div_ceil(cols);
        let vp = Rect::new(x0, y + content::SECTION_TITLE_H, vp_w, rows as f32 * cell_h);
        sections.push(Section { title, title_y: y, vp, first, n });
        first += n;
        y = vp.y + vp.h + content::DOT_ROOM + content::SECTION_GAP;
    }
    let total_h = if sections.is_empty() { 0.0 } else { y - content::SECTION_GAP + GRID_BOTTOM };
    Grid { cols, cell_w, cell_h, icon: content::GRID_ICON * icon_scale, icon_top: content::GRID_ICON_TOP * icon_scale, sections, total_h }
}

/// `text` on one line of `room`: whole when it fits, else cut with "…".
fn fit_line(text: &str, room: f32, width: &mut dyn FnMut(&str) -> f32) -> String {
    if width(text) <= room {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let cut = format!("{}…", chars[..n].iter().collect::<String>().trim_end());
        if width(&cut) <= room {
            return cut;
        }
    }
    "…".to_owned()
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
    /// The screen a Scale or Resolution page sets: its place among them.
    Target(u8),
    /// Colours (or Wallpaper, its tab) asked for on the card: it opens in its
    /// box mid-screen, as Super+H opens it (`Panel::boxed`).
    Boxed(&'static str),
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
    /// An app's card: the entry whose icon stands on it, and the small
    /// line at its foot (`controls::Choice::icon`, `hint`).
    icon: Option<usize>,
    hint: Option<String>,
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
    /// A SCREEN in the Monitors page's drawing: a hairline box, bright when
    /// picked, with its marks — the Options bar on its top edge, the dock on
    /// its bottom, turned with it; `held` while it is dragged.
    Screen { rot: u16, bar: bool, dock: bool, held: bool },
    /// One of the picked screen's two turning corners (`back`: the other way).
    Corner { back: bool },
}

/// Where the Monitors page's drawing puts the screens: a logical pixel is
/// `k` of the field's, the logical origin at `(ox, oy)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScreenView {
    k: f32,
    ox: f32,
    oy: f32,
}

/// A screen of the drawing in the hand (`Panel::screen_press`).
#[derive(Clone, Debug)]
pub(crate) struct ScreenDrag {
    /// The group it is of, and which screen.
    group: u8,
    index: usize,
    /// Where the press was (surface px) and the screen's corner then.
    from: (f32, f32),
    origin: (f64, f64),
    /// The drawing as it was at the press: it holds still under the hand.
    view: ScreenView,
    /// Where it is now (logical px), and the side it sticks to.
    at: (f64, f64),
    side: Option<(crate::monitors_page::Side, usize)>,
    /// It has gone further than a click's wobble.
    moved: bool,
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
    /// The choices' group (what a [`Knob`] is of) and how many stops it has.
    group: u8,
    count: usize,
    /// A gap at the middle the knob leaps across, no fill
    /// (`controls::Group::split`); a notch at this fraction (`mark`).
    split: bool,
    mark: Option<f32>,
}

/// The spans of a track that are drawn, as fractions of its length: the
/// whole line, less the middle gap of a split track and a notch's width
/// at each mark.
fn track_spans(w: f32, split: bool, mark: Option<f32>) -> Vec<(f32, f32)> {
    let mut cuts: Vec<(f32, f32)> = Vec::new();
    if split {
        cuts.push((0.5 - SPLIT_GAP / 2.0 / w.max(1.0), 0.5 + SPLIT_GAP / 2.0 / w.max(1.0)));
        cuts.extend(SPLIT_NOTCHES.iter().map(|&m| (m - NOTCH_W / 2.0 / w.max(1.0), m + NOTCH_W / 2.0 / w.max(1.0))));
    } else if let Some(m) = mark {
        cuts.push((m - NOTCH_W / 2.0 / w.max(1.0), m + NOTCH_W / 2.0 / w.max(1.0)));
    }
    cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut spans = Vec::with_capacity(cuts.len() + 1);
    let mut from = 0.0f32;
    for (a, b) in cuts {
        if a > from {
            spans.push((from, a.min(1.0)));
        }
        from = from.max(b);
    }
    if from < 1.0 {
        spans.push((from, 1.0));
    }
    spans
}

/// A slider of a ROW (`Panel::flow_slider_row`): its group's index, the
/// group, and the one laid beside it with its index.
type RowSlider<'a> = (u8, &'a crate::controls::Group, Option<(u8, &'a crate::controls::Group)>);

/// `text` in lines no wider than `room` (by `width`), broken between words.
fn wrap_words(text: &str, room: f32, mut width: impl FnMut(&str) -> f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if width(&format!("{line} {word}")) <= room => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_owned()),
        }
    }
    lines
}

/// Whether a group is a SWITCH: on or off, nothing else.
fn is_switch(g: &crate::controls::Group) -> bool {
    g.choices.len() == 2 && g.choices[0].value == "on" && g.choices[1].value == "off"
}

/// A slider's caption as its name and its value ("Lighting · Dark 10").
fn slider_words(g: &crate::controls::Group) -> (String, String) {
    match g.caption.as_deref().and_then(|c| c.split_once(" · ")) {
        Some((n, v)) => (n.to_owned(), v.to_owned()),
        None => (g.caption.clone().unwrap_or_default(), String::new()),
    }
}

/// THE KNOB under the pointer (Max, 2026-10-18: "the slider jumps"): where a
/// slider was last dragged to, continuous, drawn from here and not from the
/// stop the setting reports — so it never lags behind the pointer nor
/// steps. It is drawn only while it names the lit stop (`knob_shown`); a
/// setting that moved on another road puts the knob on its stop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Knob {
    pub(crate) group: u8,
    pub(crate) key: &'static str,
    /// 0..=1 along the track.
    pub(crate) v: f32,
    /// The stop it is nearest.
    pub(crate) stop: u8,
    /// The pointer is still down.
    pub(crate) held: bool,
}

/// The key of the wallpapers' Colours slider (see `App::slider_to`).
const WALL_SLIDER: &str = "colors-wallpaper";
/// The wallpapers' Saturation: live on the tiles while dragged, like Colours.
const WALL_SAT: &str = "colors-wall-sat";

/// A knob's place 0..=1 as a level 0..=100.
fn knob_level(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// A pointer at `x` over a track from `track_x`, `track_w` wide (all in one
/// frame): the knob's place 0..=1, continuous, kept on the track.
pub(crate) fn knob_value(x: f32, track_x: f32, track_w: f32) -> f32 {
    ((x - track_x) / track_w.max(1.0)).clamp(0.0, 1.0)
}

/// Where a slider's knob is drawn: the dragged place while it is the lit
/// stop's (or the pointer is down), else the lit stop's.
fn knob_shown(lit: f32, count: usize, knob: Option<Knob>) -> f32 {
    let n = count.max(2) - 1;
    match knob {
        Some(k) if k.held || (k.v * n as f32).round() == (lit * n as f32).round() => k.v,
        _ => lit,
    }
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
    /// Thin filled lines (the edge two screens touch along, in the drawing).
    lines: Vec<Rect>,
    /// The Monitors page's drawing: its group and where it put the screens.
    screens_view: Option<(u8, ScreenView)>,
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
    /// Drawn clipped to their rect (the grid, cut at its page's edges as
    /// the apps' sections are; an open module's scrolling list: the pill at
    /// its foot is cut in half, which is what says "more below").
    pub clipped: Vec<Clipped>,
    /// Pictures to draw (a wallpaper's preview): where, and which file —
    /// whoever puts the panel on the screen finds each its texture.
    pub images: Vec<(Rect, String)>,
    /// The corner the pictures are cut to (their tiles').
    pub picture_radius: f32,
    /// The palettes' cards as the Palette and Saturation sliders would make
    /// them: their colour scaled by this (0 grey, 1 as made).
    pub card_chroma: Option<f32>,
    /// Apps' icons to draw (the Colours Apps tiles): the app entry and
    /// where — drawn through the dock's icon layers, as fast launch's are.
    pub icons: Vec<(usize, Rect)>,
    /// Symbols drawn through the dock's figure layers (none of the grid's:
    /// those ride `clipped`).
    pub symbols: Vec<PanelSymbol>,
}

/// A part of the panel cut at its own window's edges: the window, its
/// rects, its words and its symbols.
pub(crate) type Clipped = (Rect, Vec<RectInst>, Vec<Label>, Vec<PanelSymbol>);

/// A symbol of `app_actions::SYMBOLS` on the panel: where, and its colour —
/// washed with it, its alpha how much it shows (the menus' symbols' way);
/// or, `plate`, drawn as an app's icon is: the layer over a plate of that
/// colour (the tiles' plates, under their symbols).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PanelSymbol {
    pub sym: usize,
    pub rect: Rect,
    pub color: [f32; 4],
    pub plate: bool,
}

/// The card's colours the panel draws in.
#[derive(Clone, Copy)]
pub(crate) struct PanelPaint {
    pub ink: [f32; 4],
    /// The card reads bright (dark ink): black washes and shadows.
    pub bright: bool,
    /// The box under the pointer and the page dots: the apps grid's own
    /// (`dock_highlight`).
    pub highlight: [f32; 4],
    /// The plate under every app's icon in the box: the tiles' too.
    pub plate: [f32; 4],
    /// The sections' words: the box's section titles' ink.
    pub title_ink: [f32; 4],
}

/// The settings field: its pills, their layout and motion, the layer cycle
/// and the open setting.
#[derive(Default)]
pub(crate) struct Panel {
    /// What the field holds: the settings ([`SETTINGS`]) or the modules
    /// (the catalog's, `modules.rs`).
    items: &'static [Item],
    /// A slider moved where it cannot change anything: its key, what is said
    /// in its words' place, until when (`App::colours_note`).
    pub(crate) note: Option<(String, &'static str, std::time::Instant)>,
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
    /// Field size, bar scale and icon scale the pills were measured and laid
    /// out for.
    key: (f32, f32, f32, f32),
    /// The field this frame, in surface coordinates.
    field: Rect,
    scale: f32,
    /// Seconds the panel has been up (what breathes in an open setting
    /// reads it).
    clock: f32,
    /// The grid as laid out (`grid_metrics`) for a field `grid_w` wide —
    /// `grid_field` is that field this frame, in surface coordinates: the
    /// card's whole width (the open setting's `field` is narrower, centred).
    grid: Grid,
    grid_w: f32,
    grid_field: Rect,
    /// How far the grid is scrolled up when the field is too short for its
    /// sections (`scroll` eases toward `scroll_to`).
    scroll: f32,
    scroll_to: f32,
    /// The tile under the pointer.
    hot: Option<usize>,
    /// THE SELECTION on the grid, the apps grid's (`SearchState::selected`):
    /// the tile under the pointer (softly), or the one the keys walked to
    /// (`keyed`: stronger); with a search on, its best match until the hand
    /// or the keys move it. Enter opens it.
    sel: Option<usize>,
    keyed: bool,
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
    /// …and it is a screen's TURN, not a resolution (the Monitors page asks
    /// under its drawing).
    pub(crate) keep_turn: bool,
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
    /// The slider last dragged: see [`Knob`].
    knob: Option<Knob>,
    /// A screen of the Monitors page's drawing in the hand.
    screen_drag: Option<ScreenDrag>,
    /// How tall the drawing is laid out (design px; 0: `SCREENS_H`): less
    /// in a box too short for the whole page (`open_content`).
    screens_h: std::cell::Cell<f32>,
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
        if self.open.is_none() && !self.solo {
            return self.grid_key_move(dx, dy);
        }
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
            // (A stop with no place — a split track's middle — is passed over.)
            let mut to = at;
            loop {
                to = (to as i32 + dx).rem_euclid(n as i32) as usize;
                if chips[to].0.w > 0.0 || to == at {
                    break to;
                }
            }
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
                    .filter(|(_, c)| (c.0.y - row).abs() < 1.0 && c.0.w > 0.0)
                    .min_by(|a, b| (mid(&a.1 .0) - mid(&here)).abs().total_cmp(&(mid(&b.1 .0) - mid(&here)).abs()))
                    .map_or(at, |(i, _)| i),
                None => at,
            }
        };
        self.focus = Some(to);
        true
    }

    /// An arrow on the grid (nothing open): the selection walks the tiles as
    /// the apps grid's does — a step sideways (on into the next section,
    /// held at the ends), up or down to the nearest tile of the row above
    /// or below (the next section's when this one ends); the first key
    /// lands on the first tile whatever it is — and the field scrolls to
    /// show it. Returns whether there was a tile to walk.
    fn grid_key_move(&mut self, dx: i32, dy: i32) -> bool {
        let shown = self.shown();
        if shown == 0 {
            return false;
        }
        let last = shown - 1;
        let at_slot = |slot: usize| self.pills.iter().position(|p| p.shown && p.slot == slot);
        let to = match self.sel.filter(|&i| self.pills[i].shown) {
            None => at_slot(0),
            Some(i) if dx != 0 => {
                let at = self.pills[i].slot;
                at_slot(if dx < 0 { at.saturating_sub(1) } else { (at + 1).min(last) })
            }
            Some(i) => {
                let here = self.pills[i].home;
                // The rows beyond this one, the nearest first; on it, the
                // tile nearest across.
                let rows: Vec<f32> = self
                    .pills
                    .iter()
                    .filter(|p| p.shown)
                    .map(|p| p.home.1)
                    .filter(|&y| if dy > 0 { y > here.1 + 1.0 } else { y < here.1 - 1.0 })
                    .collect();
                let row = rows.iter().copied().min_by(|a, b| (a - here.1).abs().total_cmp(&(b - here.1).abs()));
                match row {
                    Some(row) => self
                        .pills
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.shown && (p.home.1 - row).abs() < 1.0)
                        .min_by(|a, b| (a.1.home.0 - here.0).abs().total_cmp(&(b.1.home.0 - here.0).abs()))
                        .map(|(j, _)| j),
                    None => Some(i),
                }
            }
        };
        let Some(to) = to else {
            return false;
        };
        self.sel = Some(to);
        self.keyed = true;
        self.reveal(to);
        true
    }

    /// Scroll so tile `i` — with its section's word over it and its name
    /// under it — is in the field's window.
    fn reveal(&mut self, i: usize) {
        let c = self.grid.cell_of(self.pills[i].slot);
        let view = (self.grid_field.h - GRID_BOTTOM + content::DOT_ROOM).max(1.0);
        let top = c.y - content::SECTION_TITLE_H - content::TITLE_LINE_PX;
        let bottom = c.y + c.h + content::LABEL_LINE_PX;
        if top < self.scroll_to {
            self.scroll_to = top.max(0.0);
        } else if bottom > self.scroll_to + view {
            self.scroll_to = bottom - view;
        }
        self.scroll_to = self.scroll_to.clamp(0.0, self.max_scroll());
    }

    /// Whether the keys drive the grid's selection (an open setting's keys
    /// are its own): then Space, too, opens the tile they are on.
    pub(crate) fn grid_keyed(&self) -> bool {
        self.open.is_none() && !self.solo && self.keyed && self.sel.is_some()
    }

    /// Enter or Space: the choice the keys are on is picked (the one in use
    /// when no key has moved yet); on the grid, the tile the keys or the
    /// hand are on opens — with a search on and neither, its best match.
    /// Returns whether a choice was there.
    pub(crate) fn key_pick(&mut self) -> bool {
        if self.open.is_none() && !self.solo {
            if let Some(i) = self.sel.filter(|&i| self.pills.get(i).is_some_and(|p| p.shown)) {
                match self.boxed(i) {
                    Some(label) => self.action = Some(PanelAction::Boxed(label)),
                    None => self.open_pill(i),
                }
                return true;
            }
            return self.open_best();
        }
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
    fn ensure(&mut self, size: (f32, f32), grid_w: f32, scale: f32, icon_scale: f32, measure: &mut dyn FnMut(&str, f32) -> f32) {
        let key = (size.0, size.1, scale, icon_scale);
        let fresh = self.pills.is_empty();
        if !fresh
            && (self.key.0 - key.0).abs() < 0.5
            && (self.key.1 - key.1).abs() < 0.5
            && (self.key.2 - key.2).abs() < 0.001
            && (self.key.3 - key.3).abs() < 0.001
            && (self.grid_w - grid_w).abs() < 0.5
        {
            return;
        }
        tracing::debug!(
            "control panel: field laid out for {:.0}x{:.0} (grid {grid_w:.0} wide) at scale {:.2}, icons {:.2} (fresh {fresh})",
            key.0, key.1, key.2, key.3
        );
        // (The names are measured for the cell's width: once per icon size.)
        let remeasure = fresh || (self.key.3 - key.3).abs() >= 0.001;
        self.key = key;
        self.grid_w = grid_w;
        self.scale = scale;
        if fresh {
            self.pills = self
                .items
                .iter()
                .map(|&(label, _, keywords)| Pill {
                    label,
                    keywords,
                    shown: true,
                    slot: 0,
                    one: String::new(),
                    two: None,
                    home: (0.0, 0.0),
                    anchor: (0.0, 0.0),
                    pos: (0.0, 0.0),
                    sc: 1.0,
                    op: 1.0,
                })
                .collect();
        }
        // Each name fitted to the apps grid's cell at the box's icon scale,
        // at the names' type size (the apps' own, not scaled with the icons).
        if remeasure {
            let room = content::GRID_CELL_W * icon_scale - 12.0;
            for p in &mut self.pills {
                let name = crate::i18n::tr(p.label);
                let mut width = |s: &str| measure(s, content::LABEL_FONT_PX);
                let lines = wrap_words(name, room, &mut width);
                p.one = fit_line(name, room, &mut width);
                p.two = (lines.len() > 1).then(|| (lines[0].clone(), fit_line(&lines[1..].join(" "), room, &mut width)));
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
        self.compose(true);
        if let Some(label) = self.pending_open.take() {
            self.open_named(label);
        }
    }

    /// Every one of this field's pills matching `query` (the apps grid's
    /// search), best first — name prefix, name, keyword; then nearest layer —
    /// each
    /// with how well it matched (0 = its name begins so, 1 = its name holds
    /// it, 2 = a keyword does) — no cap: the dock box's System row takes as
    /// many as its width holds.
    pub(crate) fn matching_ranked(&self, query: &str) -> Vec<(u8, &'static str)> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let items = self.items;
        // Among equal matches, the order use brings them (the grid's own).
        let now = now_secs();
        let score = |label: &str| self.usage.settings.get(label).map_or(0.0, |st| st.score_at(now));
        let mut by_use: Vec<usize> = (0..items.len()).collect();
        by_use.sort_by(|&a, &b| score(items[b].0).total_cmp(&score(items[a].0)).then(a.cmp(&b)));
        let mut place = vec![0u8; items.len()];
        for (k, &i) in by_use.iter().enumerate() {
            place[i] = k.min(255) as u8;
        }
        let mut hits: Vec<(u8, u8, usize)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, &(label, _, keywords))| Some((match_rank(label, keywords, &q)?, place[i], i)))
            .collect();
        hits.sort();
        hits.into_iter().map(|(rank, _, i)| (rank, items[i].0)).collect()
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

    /// The section of item `i`: its own on the settings' field; the one
    /// section of the modules'.
    fn section_index(&self, i: usize) -> usize {
        if self.modules {
            0
        } else {
            usize::from(self.items[i].1)
        }
    }

    /// Each section's title and how many of its tiles `shown` keeps, in
    /// order (the modules' field: one section, no title).
    fn section_counts(&self, shown: &dyn Fn(usize) -> bool) -> Vec<(Option<&'static str>, usize)> {
        let n = if self.modules { 1 } else { SECTIONS.len() };
        (0..n)
            .map(|s| {
                let title = (!self.modules).then(|| SECTIONS[s]);
                (title, (0..self.items.len()).filter(|&i| self.section_index(i) == s && shown(i)).count())
            })
            .collect()
    }

    /// How tall the grid of every section is in a field `w` wide (the
    /// card grows to it: `App::apps_trim_target`), the Search pill's room
    /// included — a search changes nothing of it.
    pub(crate) fn wanted_h(&self, w: f32, icon_scale: f32) -> f32 {
        grid_metrics(&self.section_counts(&|_| true), w, icon_scale).total_h
    }

    /// Lay the grid out: every section in its place, every tile in its
    /// fixed order inside it, as many columns as the field holds; with a
    /// search on, only its matches, in their sections — the rest leave.
    /// `snap` puts every tile there at once; otherwise they glide.
    fn compose(&mut self, snap: bool) {
        let shown: Vec<bool> = (0..self.items.len())
            .map(|i| self.query.is_empty() || match_rank(self.items[i].0, self.items[i].2, &self.query).is_some())
            .collect();
        let counts = self.section_counts(&|i| shown[i]);
        self.grid = grid_metrics(&counts, self.grid_w, self.key.3);
        let secs: Vec<usize> = (0..self.pills.len()).map(|i| self.section_index(i)).collect();
        for p in &mut self.pills {
            p.shown = false;
        }
        let mut k = 0;
        for s in 0..counts.len() {
            for (i, p) in self.pills.iter_mut().enumerate() {
                if secs[i] != s || !shown[i] {
                    continue;
                }
                let c = self.grid.cell_of(k);
                p.shown = true;
                p.slot = k;
                p.home = (c.x + c.w / 2.0, c.y + c.h / 2.0);
                if snap {
                    p.anchor = p.home;
                    p.pos = p.home;
                }
                k += 1;
            }
        }
        // (Scrolled past what there is now: back up to the end.)
        self.scroll_to = self.scroll_to.min(self.max_scroll());
        if snap {
            self.scroll = self.scroll_to;
        }
    }

    /// How many tiles the grid shows (the slots are `0..shown`).
    fn shown(&self) -> usize {
        self.pills.iter().filter(|p| p.shown).count()
    }

    /// How far the grid can scroll: what of it the field does not hold.
    fn max_scroll(&self) -> f32 {
        (self.grid.total_h - self.grid_field.h).max(0.0)
    }

    /// The grid's window in the field (surface coordinates): from under the
    /// dock to the Search pill's room, what the sections are cut to.
    fn grid_clip(&self) -> Rect {
        let g = self.grid_field;
        Rect::new(g.x, g.y, g.w, (g.h - GRID_BOTTOM + content::DOT_ROOM).max(0.0))
    }

    /// The cell of tile `p` as drawn now (grid field coordinates): where it
    /// stands, scrolled.
    fn cell_now(&self, p: &Pill) -> Rect {
        let g = &self.grid;
        Rect::new(p.pos.0 - g.cell_w / 2.0, p.pos.1 - g.cell_h / 2.0 - self.scroll, g.cell_w, g.cell_h)
    }

    /// Its icon's place — the plate an app's icon would fill — in the open
    /// setting's field coordinates: what its setting opens out of.
    fn icon_rect(&self, p: &Pill) -> Rect {
        let c = self.cell_now(p);
        let g = &self.grid;
        let (dx, dy) = (self.grid_field.x - self.field.x, self.grid_field.y - self.field.y);
        Rect::new(c.x + (c.w - g.icon) / 2.0 + dx, c.y + g.icon_top + dy, g.icon, g.icon)
    }

    /// Wheel travel over the panel: an open module's list scrolls; the grid
    /// scrolls where the field is too short for its sections.
    fn wheel(&mut self, value: f64) {
        if self.open.is_some() {
            if self.modules {
                if let Some(m) = self.open.as_ref().filter(|o| o.want > 0.5).and_then(|o| crate::modules::module(self.pills[o.pill].label)) {
                    let max = self.mod_layout(m).max_scroll;
                    self.mods.scroll = (self.mods.scroll + value as f32 * MOD_WHEEL).clamp(0.0, max);
                }
            }
            return;
        }
        self.scroll_to = (self.scroll_to + value as f32 * SCROLL_WHEEL).clamp(0.0, self.max_scroll());
    }

    /// The tile whose cell is under `pos` (surface coordinates).
    fn pill_at(&self, pos: (f32, f32)) -> Option<usize> {
        if !self.grid_clip().contains(pos) {
            return None;
        }
        let fp = (pos.0 - self.grid_field.x, pos.1 - self.grid_field.y);
        self.pills.iter().position(|p| p.shown && p.op >= 0.5 && self.cell_now(p).contains(fp))
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
        if let Some(label) = self.boxed(i) {
            self.action = Some(PanelAction::Boxed(label));
            return true;
        }
        self.open_pill(i);
        true
    }

    /// The setting of pill `i` when it opens in its own box, not on the card:
    /// Colours and Wallpaper. On the card its sliders fell under the screen's
    /// edge and the dock's Search pill covered its tabs (the pass of
    /// 2026-10-27); the box mid-screen fits it on every screen.
    fn boxed(&self, i: usize) -> Option<&'static str> {
        let label = self.pills.get(i)?.label;
        (!self.solo && !self.modules && crate::controls::box_of(label) == "Colours").then_some(label)
    }

    /// A box asked for (`PanelAction::Boxed`), taken — and nothing else.
    pub(crate) fn take_boxed(&mut self) -> Option<&'static str> {
        match self.action {
            Some(PanelAction::Boxed(label)) => {
                self.action = None;
                Some(label)
            }
            _ => None,
        }
    }

    /// Grow pill `i` into its setting.
    fn open_pill(&mut self, clicked: usize) {
        let from = self.icon_rect(&self.pills[clicked]);
        // (Wallpaper's pill opens the Colours box on its Wallpaper tab,
        // grown out of its own pill: `controls::box_of`.)
        let i = if self.modules { clicked } else { self.box_pill(clicked) };
        info!("control panel: opening {}", self.pills[i].label);
        self.focus = None;
        self.open = Some(OpenBox {
            pill: i,
            k: 0.0,
            want: 1.0,
            from,
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
        // The order changes now, behind the setting growing over the field.
        self.record_use(clicked, now_secs());
    }

    /// The pill whose box pill `i` opens (`controls::box_of`), the tab it
    /// asks for set.
    fn box_pill(&self, i: usize) -> usize {
        let label = self.pills[i].label;
        crate::controls::start_tab_for(label);
        let to = crate::controls::box_of(label);
        self.pills.iter().position(|q| q.label == to).unwrap_or(i)
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
        self.key = (0.0, 0.0, 0.0, 0.0);
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
        // A setting opened afresh starts on its own page (Colours: the
        // Colours tab, Shell, not its apps page).
        crate::controls::APPS_LIST.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::controls::COLOURS_TAB.store(crate::controls::COLOURS_START.swap(0, std::sync::atomic::Ordering::Relaxed), std::sync::atomic::Ordering::Relaxed);
        crate::controls::COLOURS_SECTION.store(0, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut a) = crate::controls::APP_CHOSEN.lock() {
            *a = None;
        }
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
    pub(crate) fn slide(&mut self, from: (f32, f32), pos: (f32, f32)) -> Option<(u8, u8)> {
        let label = self.open_control_label()?;
        let content = self.open_content(label)?;
        let f = self.field;
        let inside = |c: &Chip| Rect::new(f.x + c.rect.x, f.y + c.rect.y, c.rect.w, c.rect.h).contains(from);
        let sl = content.sliders.iter().find(|sl| content.chips[sl.stops.clone()].iter().any(inside))?;
        let stops = &content.chips[sl.stops.clone()];
        let n = stops.len().max(2) - 1;
        let mut v = knob_value(pos.0 - f.x, sl.track.x, sl.track.w);
        let mut k = (v * n as f32).round() as usize;
        // A SPLIT track: its middle stop is never rested on — the knob leaps
        // the gap to the stop on the pointer's side (Dark 45 or Light 55).
        if sl.split && stops.len() % 2 == 1 && k == n / 2 {
            k = if v < 0.5 { n / 2 - 1 } else { n / 2 + 1 };
            v = k as f32 / n as f32;
        }
        let chip = stops.get(k)?;
        if let PanelAction::Pick { group, index } = chip.action {
            let key = self.control.as_ref().and_then(|c| c.groups.get(group as usize)).map_or("", |g| g.key);
            self.knob = Some(Knob { group, key, v, stop: index, held: true });
        }
        match chip.action {
            PanelAction::Pick { group, index } if !chip.on => Some((group, index)),
            _ => None,
        }
    }

    /// The pointer went up: the knob stays where it was left. Gives it back
    /// when it was held.
    pub(crate) fn knob_let_go(&mut self) -> Option<Knob> {
        let k = self.knob.as_mut().filter(|k| k.held)?;
        k.held = false;
        Some(*k)
    }

    /// The knob that is of slider `sl` (the same group of the same setting).
    fn knob_of(&self, sl: &Slider) -> Option<Knob> {
        let key = self.control.as_ref().and_then(|c| c.groups.get(sl.group as usize)).map(|g| g.key)?;
        self.knob.filter(|k| k.group == sl.group && k.key == key)
    }

    /// The dragged knob, while the pointer is down on it.
    pub(crate) fn knob_held(&self) -> Option<Knob> {
        self.knob.filter(|k| k.held)
    }

    /// Whether a press at `pos` is on a slider (its whole row).
    pub(crate) fn on_slider(&self, pos: (f32, f32)) -> bool {
        let Some(label) = self.open_control_label() else { return false };
        let Some(content) = self.open_content(label) else { return false };
        let f = self.field;
        let at = |c: &Chip| Rect::new(f.x + c.rect.x, f.y + c.rect.y, c.rect.w, c.rect.h).contains(pos);
        // (Not when something drawn lies under the pointer — the switch
        // beside a row slider, whose line the stops reach up into: that is
        // its click, as `open_hit` takes it.)
        !content.chips.iter().any(|c| !c.seg && at(c)) && content.sliders.iter().any(|sl| content.chips[sl.stops.clone()].iter().any(at))
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
            out.chips.push(Chip { rect: Rect::new(x, y + row as f32 * (h + row_gap), w, h), text, on, action, image: None, swatch: Vec::new(), seg: false, palette: false, cell: false, icon: None, hint: None, style: Style::Word { big: false } });
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
                        icon: None,
                        hint: None,
                        style: Style::Word { big },
                    });
                }
                x += widths[i] + gap;
            }
        }
        y + rows.len() as f32 * h + (rows.len() - 1) as f32 * row_gap + GROUP_GAP * s
    }

    /// A SWITCH ROW from `y`: the name at the left, the switch at the
    /// right; the whole row turns it. A `hint` is a muted line under the
    /// name (what the choice means: the Launcher's Stay away).
    fn switch_row(&self, out: &mut OpenContent, y: f32, group: u8, name: &str, on: bool, hint: Option<&str>) -> f32 {
        let s = self.scale;
        let fw = self.key.0;
        let (left, right) = (2.0 * MARGIN * s, fw - 2.0 * MARGIN * s);
        // The hint keeps clear of the switch: in as many lines as it needs
        // (a narrow box — beside a side dock — takes two).
        let room = right - left - (SWITCH_W + 18.0) * s;
        let lines = hint.map(|h| wrap_words(h, room, |t| crate::options::est_text_w(t, 12.0 * s))).unwrap_or_default();
        let h = if lines.is_empty() { 34.0 * s } else { (30.0 + 16.0 * lines.len() as f32) * s };
        if lines.is_empty() {
            out.texts.push(Text { text: name.to_owned(), x: left, y: y + (h - 18.0 * s) / 2.0, px: 14.0, alpha: 0.85, centered: false });
        } else {
            out.texts.push(Text { text: name.to_owned(), x: left, y: y + 4.0 * s, px: 14.0, alpha: 0.85, centered: false });
            for (k, line) in lines.into_iter().enumerate() {
                out.texts.push(Text { text: line, x: left, y: y + (25.0 + 16.0 * k as f32) * s, px: 12.0, alpha: 0.55, centered: false });
            }
        }
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
            icon: None,
            hint: None,
            style: Style::Switch { on },
        });
        y + h + 10.0 * s
    }

    /// A WORD ROW from `y` (`controls::Group::labelled`): the caption at
    /// the left as a switch row's name, the words at the right end of the
    /// same line, each lit by its own `on`. Words too long for the line go
    /// under their name, centred, as any group's do.
    fn word_row(&self, out: &mut OpenContent, y: f32, group: u8, g: &crate::controls::Group) -> f32 {
        let s = self.scale;
        let fw = self.key.0;
        let (left, right, h) = (2.0 * MARGIN * s, fw - 2.0 * MARGIN * s, 34.0 * s);
        let (letter, gap, word_h) = (7.4 * s, 22.0 * s, 26.0 * s);
        let name = g.caption.clone().unwrap_or_default();
        let widths: Vec<f32> = g.choices.iter().map(|c| c.text.chars().count().max(1) as f32 * letter + 4.0 * s).collect();
        let words_w = widths.iter().sum::<f32>() + gap * (widths.len().saturating_sub(1)) as f32;
        let chips = || g.choices.iter().enumerate().map(|(ci, c)| (c.text.clone(), c.on, PanelAction::Pick { group, index: ci as u8 })).collect();
        if left + crate::options::est_text_w(&name, 14.0 * s) + 28.0 * s + words_w > right {
            let y = self.caption(out, y, name);
            return self.flow_words(out, y, chips(), false);
        }
        out.texts.push(Text { text: name, x: left, y: y + (h - 18.0 * s) / 2.0, px: 14.0, alpha: 0.85, centered: false });
        let mut x = right - words_w;
        for ((text, on, action), w) in chips().into_iter().zip(widths) {
            out.chips.push(Chip {
                rect: Rect::new(x - gap / 2.0, y + (h - word_h) / 2.0, w + gap, word_h),
                text,
                on,
                action,
                image: None,
                swatch: Vec::new(),
                seg: false,
                palette: false,
                cell: false,
                icon: None,
                hint: None,
                style: Style::Word { big: false },
            });
            x += w + gap;
        }
        y + h + 10.0 * s
    }

    /// THE SCREENS, DRAWN (the Monitors page, `monitors_page.rs`): each a
    /// hairline box in its own proportions and place, the whole fitted to
    /// the box's width; the picked one bright, its two turning corners on
    /// its top corners; the one in the hand where the hand has it, the edge
    /// it touches lit. Mirrored, they stand one over the other.
    fn flow_screens(&self, out: &mut OpenContent, y: f32, group: u8, g: &crate::controls::Group) -> f32 {
        use crate::monitors_page::{Box2, Side};
        let s = self.scale;
        let fw = self.key.0;
        let tall = if self.screens_h.get() > 0.0 { self.screens_h.get() } else { SCREENS_H };
        let area = Rect::new(2.0 * MARGIN * s, y, fw - 4.0 * MARGIN * s, tall * s);
        let held = self.screen_drag.as_ref().filter(|d| d.group == group && d.moved);
        let boxes: Vec<Box2> = g
            .screens
            .iter()
            .enumerate()
            .map(|(i, sh)| match held.filter(|d| d.index == i) {
                Some(d) => Box2 { x: d.at.0, y: d.at.1, w: sh.w, h: sh.h },
                None => Box2 { x: sh.x, y: sh.y, w: sh.w, h: sh.h },
            })
            .collect();
        let mirrored = g.screens.iter().any(|sh| sh.mirrored);
        // The drawing holds still while a screen is in the hand.
        let view = match held {
            Some(d) => d.view,
            None => screens_fit(&g.screens, area, mirrored),
        };
        out.screens_view = Some((group, view));
        let cx = area.x + area.w / 2.0;
        let cy = area.y + area.h / 2.0;
        let mut picked = None;
        for (i, (sh, b)) in g.screens.iter().zip(&boxes).enumerate() {
            let (w, h) = (b.w as f32 * view.k, b.h as f32 * view.k);
            let (x, y) = if mirrored { (cx - w / 2.0, cy - h / 2.0) } else { (view.ox + b.x as f32 * view.k, view.oy + b.y as f32 * view.k) };
            let r = Rect::new(x, y, w, h);
            let in_hand = held.is_some_and(|d| d.index == i);
            out.chips.push(Chip {
                rect: r,
                text: if mirrored && !sh.picked { String::new() } else { sh.short.clone() },
                on: sh.picked,
                action: PanelAction::Pick { group, index: i as u8 },
                image: None,
                swatch: Vec::new(),
                seg: false,
                palette: false,
                cell: false,
                icon: None,
                hint: None,
                style: Style::Screen { rot: sh.rot, bar: sh.bar, dock: sh.dock, held: in_hand },
            });
            if sh.picked {
                picked = Some((r, in_hand));
            }
        }
        // The picked one's corners: back on the left, on (clockwise) on the
        // right — not while it is in the hand.
        let n = g.screens.len();
        if let Some((r, false)) = picked.filter(|_| g.choices.len() >= n + 2) {
            let d = CORNER * s;
            for (k, back) in [(0, true), (1, false)] {
                let x = if back { r.x - d / 2.0 } else { r.x + r.w - d / 2.0 };
                out.chips.push(Chip {
                    rect: Rect::new(x, r.y - d / 2.0, d, d),
                    text: String::new(),
                    on: false,
                    action: PanelAction::Pick { group, index: (n + k) as u8 },
                    image: None,
                    swatch: Vec::new(),
                    seg: false,
                    palette: false,
                    cell: false,
                    icon: None,
                    hint: None,
                    style: Style::Corner { back },
                });
            }
        }
        // The edge the one in the hand touches, lit.
        if let Some(d) = held.filter(|_| !mirrored && !g.screens.iter().any(|sh| sh.isolated)) {
            if let Some((side, oi)) = d.side {
                let others: Vec<Box2> = boxes.iter().enumerate().filter(|(i, _)| *i != d.index).map(|(_, b)| *b).collect();
                if let (Some(o), Some(b)) = (others.get(oi), boxes.get(d.index)) {
                    let t = 2.0 * s;
                    let line = match side {
                        Side::Right | Side::Left => {
                            let x = if side == Side::Right { o.x + o.w } else { o.x };
                            let (y0, y1) = (b.y.max(o.y), (b.y + b.h).min(o.y + o.h));
                            (y1 > y0).then(|| Rect::new(view.ox + x as f32 * view.k - t / 2.0, view.oy + y0 as f32 * view.k, t, (y1 - y0) as f32 * view.k))
                        }
                        Side::Below | Side::Above => {
                            let y = if side == Side::Below { o.y + o.h } else { o.y };
                            let (x0, x1) = (b.x.max(o.x), (b.x + b.w).min(o.x + o.w));
                            (x1 > x0).then(|| Rect::new(view.ox + x0 as f32 * view.k, view.oy + y as f32 * view.k - t / 2.0, (x1 - x0) as f32 * view.k, t))
                        }
                    };
                    out.lines.extend(line);
                }
            }
        }
        y + area.h + 10.0 * s
    }

    /// A press at `pos` on a screen of the drawing (not on a corner): it is
    /// taken in the hand (it moves only once the pointer has gone further
    /// than a click's wobble). True when it was.
    pub(crate) fn screen_press(&mut self, pos: (f32, f32)) -> bool {
        self.screen_drag = None;
        let Some(label) = self.open_control_label() else { return false };
        let Some(content) = self.open_content(label) else { return false };
        let Some((group, view)) = content.screens_view else { return false };
        let f = self.field;
        let at = |c: &Chip| Rect::new(f.x + c.rect.x, f.y + c.rect.y, c.rect.w, c.rect.h).contains(pos);
        if content.chips.iter().any(|c| matches!(c.style, Style::Corner { .. }) && at(c)) {
            return false;
        }
        let Some(PanelAction::Pick { index, .. }) = content.chips.iter().find(|c| matches!(c.style, Style::Screen { .. }) && at(c)).map(|c| c.action) else {
            return false;
        };
        let Some(sh) = self.control.as_ref().and_then(|v| v.groups.get(group as usize)).and_then(|g| g.screens.get(index as usize)) else {
            return false;
        };
        if sh.mirrored {
            return false;
        }
        self.screen_drag = Some(ScreenDrag {
            group,
            index: index as usize,
            from: pos,
            origin: (sh.x, sh.y),
            view,
            at: (sh.x, sh.y),
            side: None,
            moved: false,
        });
        true
    }

    /// Whether a screen is in the hand.
    pub(crate) fn screen_dragging(&self) -> bool {
        self.screen_drag.is_some()
    }

    /// The pointer moved with a screen in the hand: it follows, its sides
    /// sticking to the others' and its ends lining up with theirs (the
    /// magnet, `monitors_page::magnet`).
    pub(crate) fn screen_drag_to(&mut self, pos: (f32, f32)) {
        use crate::monitors_page::{magnet, Box2, ISOLATED_GAP};
        let s = self.scale;
        let Some(d) = self.screen_drag.as_mut() else { return };
        let Some(g) = self.control.as_ref().and_then(|v| v.groups.get(d.group as usize)) else { return };
        let Some(sh) = g.screens.get(d.index) else { return };
        let (dx, dy) = (pos.0 - d.from.0, pos.1 - d.from.1);
        if !d.moved && dx.hypot(dy) < 4.0 * s {
            return;
        }
        d.moved = true;
        let k = f64::from(d.view.k.max(1e-6));
        let raw = Box2 { x: d.origin.0 + f64::from(dx) / k, y: d.origin.1 + f64::from(dy) / k, w: sh.w, h: sh.h };
        let others: Vec<Box2> = g.screens.iter().enumerate().filter(|(i, _)| *i != d.index).map(|(_, o)| Box2 { x: o.x, y: o.y, w: o.w, h: o.h }).collect();
        let gap = if sh.isolated { ISOLATED_GAP } else { 0.0 };
        let (x, y, side) = magnet(raw, &others, f64::from(STICK * s) / k, gap);
        d.at = (x, y);
        d.side = side;
    }

    /// The pointer went up: a screen that was moved is let go where it
    /// rests (laid against the others, `monitors_page::settle`) — drawn
    /// there at once, ahead of the page being read again — and its name and
    /// place given back to be saved. `None`: it was a click (or nothing).
    pub(crate) fn screen_release(&mut self) -> Option<(String, f64, f64)> {
        use crate::monitors_page::{settle, Box2};
        let d = self.screen_drag.take().filter(|d| d.moved)?;
        let g = self.control.as_mut().and_then(|v| v.groups.get_mut(d.group as usize))?;
        let others: Vec<Box2> = g.screens.iter().enumerate().filter(|(i, _)| *i != d.index).map(|(_, o)| Box2 { x: o.x, y: o.y, w: o.w, h: o.h }).collect();
        let sh = g.screens.get_mut(d.index)?;
        let (x, y) = settle(Box2 { x: d.at.0, y: d.at.1, w: sh.w, h: sh.h }, &others, sh.isolated);
        sh.x = x;
        sh.y = y;
        // (Picked as it was taken.)
        let picked = sh.sel.clone();
        for o in &mut g.screens {
            o.picked = o.sel == picked;
        }
        Some((picked, x, y))
    }

    /// What a group laid INLINE on a line takes: a switch (`on`/`off`) as
    /// its name and a small switch, anything else as its choices in small
    /// words (a slider's look pair).
    fn inline_w(&self, g: &crate::controls::Group) -> f32 {
        let s = self.scale;
        if is_switch(g) {
            crate::options::est_text_w(g.caption.as_deref().unwrap_or_default(), 12.0 * s) + (SWITCH_W + 14.0) * s
        } else {
            g.choices.iter().map(|c| c.text.chars().count().max(1) as f32 * 7.0 * s + 16.0 * s).sum()
        }
    }

    /// Lay `g` inline from `x`, on a line `h` tall at `y` (see [`inline_w`]
    /// for what it takes).
    fn inline(&self, out: &mut OpenContent, x: f32, y: f32, h: f32, group: u8, g: &crate::controls::Group) {
        let s = self.scale;
        let chip = |rect: Rect, text: String, on: bool, index: u8, style: Style| Chip {
            rect,
            text,
            on,
            action: PanelAction::Pick { group, index },
            image: None,
            swatch: Vec::new(),
            seg: false,
            palette: false,
            cell: false,
            icon: None,
            hint: None,
            style,
        };
        if is_switch(g) {
            let on = g.choices.first().is_some_and(|c| c.on);
            let w = self.inline_w(g);
            out.texts.push(Text { text: g.caption.clone().unwrap_or_default(), x, y: y + (h - 16.0 * s) / 2.0, px: 12.0, alpha: 0.6, centered: false });
            // (The name and the switch are one chip: a click on either turns it.)
            out.chips.push(chip(Rect::new(x - 4.0 * s, y, w + 8.0 * s, h), String::new(), on, if on { 1 } else { 0 }, Style::Switch { on }));
        } else {
            let (letter, gap) = (7.0 * s, 12.0 * s);
            let mut x = x;
            for (ci, c) in g.choices.iter().enumerate() {
                let w = c.text.chars().count().max(1) as f32 * letter + 4.0 * s;
                out.chips.push(chip(Rect::new(x, y + 2.0 * s, w + gap, h - 4.0 * s), c.text.clone(), c.on, ci as u8, Style::Word { big: false }));
                x += w + gap;
            }
        }
    }

    /// A SWITCH ON ITS OWN LINE, CENTRED (the Colours model's Auto): its
    /// name and the switch together in the middle of the box.
    fn switch_centred(&self, out: &mut OpenContent, y: f32, group: u8, g: &crate::controls::Group) -> f32 {
        let s = self.scale;
        let h = 34.0 * s;
        let w = self.inline_w(g);
        self.inline(out, (self.key.0 - w) / 2.0, y, h, group, g);
        // (A group's air under it: alone under the tabs — Auto on — the
        // closing hint touched the switch; rig, 2026-10-20.)
        y + h + GROUP_GAP * s
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
                icon: None,
                hint: None,
                style: Style::Word { big: false },
            });
        }
        let rows = cells.len().div_ceil(cols);
        y + rows as f32 * h + (rows.saturating_sub(1)) as f32 * gap + GROUP_GAP * s * 0.7
    }

    /// Lay a SLIDER (`sl`) out from `y` down, across the field, with its own
    /// pair of pills (`pair`, the group laid beside it) at its right end.
    /// Its stops are chips that are not drawn: a click lands on the nearest,
    /// and the keys walk them a stop at a time.
    fn flow_slider(&self, out: &mut OpenContent, y: f32, group: u8, sl: &crate::controls::Group, pair: Option<(u8, &crate::controls::Group)>) -> f32 {
        let s = self.scale;
        let fw = self.key.0;
        let h = 30.0 * s;
        let (stops, (split, mark)) = (&sl.choices[..], (sl.split, sl.mark));
        let (name, value) = slider_words(sl);
        let (mut left, mut right) = (MARGIN * 2.0 * s, fw - MARGIN * 2.0 * s);
        // The row's name, at the left.
        out.texts.push(Text { text: name, x: left, y: y + (h - 18.0 * s) / 2.0, px: 13.5, alpha: 0.6, centered: false });
        left += 74.0 * s;
        if let Some((pg, g)) = pair {
            // Its look: small words at the right.
            let letter = 7.0 * s;
            let gap = 12.0 * s;
            let widths: Vec<f32> = g.choices.iter().map(|c| c.text.chars().count().max(1) as f32 * letter + 4.0 * s).collect();
            let total: f32 = widths.iter().sum::<f32>() + gap * (widths.len().saturating_sub(1)) as f32;
            let x0 = right - total;
            // (Every row keeps the same room for its look, so the tracks line up.)
            let lane = right - 116.0 * s;
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
                    icon: None,
                    hint: None,
                    style: Style::Word { big: false },
                });
                x += w + gap;
            }
            right = x0.min(lane) - 18.0 * s;
        }
        // Its value, in words, before the look.
        out.texts.push(Text { text: value.to_owned(), x: right - 36.0 * s, y: y + (h - 16.0 * s) / 2.0, px: 12.0, alpha: 0.55, centered: true });
        right -= 84.0 * s;
        let n = stops.len().max(2);
        let track = Rect::new(left, y + h / 2.0 - 1.0 * s, (right - left).max(40.0 * s), 2.0 * s);
        let lit = stops.iter().position(|c| c.on).unwrap_or(0);
        let range = self.slider_stops(out, group, stops, track, (track.x - 12.0 * s, right + 14.0 * s), (y - 6.0 * s, y + h + 6.0 * s), split);
        out.sliders.push(Slider { track, value: lit as f32 / (n - 1) as f32, stops: range, group, count: n, split, mark });
        y + h + 10.0 * s
    }

    /// A slider's STOPS: chips that are not drawn (the track is), each
    /// taking its share of the WHOLE row — from `top` to `bottom`, the
    /// first out to `x_first` and the last to `x_last` — so a click near
    /// the slider catches a stop instead of landing on empty space (Max,
    /// 2026-10-16: "i miss and close the box all the time"). On a SPLIT
    /// track the middle stop has no place at all: its share goes to its two
    /// neighbours, and the knob leaps the gap (`slide`). Returns which
    /// chips they are.
    #[allow(clippy::too_many_arguments)]
    fn slider_stops(
        &self,
        out: &mut OpenContent,
        group: u8,
        stops: &[crate::controls::Choice],
        track: Rect,
        (x_first, x_last): (f32, f32),
        (top, bottom): (f32, f32),
        split: bool,
    ) -> std::ops::Range<usize> {
        let n = stops.len().max(2);
        let step = track.w / (n - 1) as f32;
        let mid = (stops.len() - 1) / 2;
        let gone = split && stops.len() % 2 == 1;
        let first = out.chips.len();
        for (ci, c) in stops.iter().enumerate() {
            let mut x0 = if ci == 0 { x_first } else { track.x + (ci as f32 - 0.5) * step };
            let mut x1 = if ci + 1 == stops.len() { x_last } else { track.x + (ci as f32 + 0.5) * step };
            if gone && ci == mid {
                x1 = x0;
            } else if gone && ci + 1 == mid {
                x1 = track.x + mid as f32 * step;
            } else if gone && ci == mid + 1 {
                x0 = track.x + mid as f32 * step;
            }
            out.chips.push(Chip {
                rect: Rect::new(x0, top, (x1 - x0).max(0.0), bottom - top),
                text: String::new(),
                on: c.on,
                action: PanelAction::Pick { group, index: ci as u8 },
                image: None,
                swatch: Vec::new(),
                seg: true,
                palette: false,
                cell: false,
                icon: None,
                hint: None,
                style: Style::Word { big: false },
            });
        }
        first..out.chips.len()
    }

    /// Lay a ROW OF SLIDERS out from `y` down (the Colours model, Max
    /// 2026-10-20: Lighting · Palette · Saturation side by side): a column
    /// each, as wide as a third of the field — one or two alone stand
    /// centred at that width, more than three share it — and each as two
    /// lines: its name at the left of the label line, its value at the
    /// right, the group laid `beside` it (a small switch, or words) before
    /// the value; the track the column's whole width under them.
    fn flow_slider_row(&self, out: &mut OpenContent, y: f32, sliders: &[RowSlider<'_>]) -> f32 {
        let s = self.scale;
        let fw = self.key.0;
        let (lh, th) = (SLIDER_LABEL_H * s, SLIDER_TRACK_H * s);
        let gap = SLIDER_COL_GAP * s;
        let room = fw - 4.0 * MARGIN * s;
        let n = sliders.len().max(1);
        let cols = n.max(3);
        let col_w = (room - (cols - 1) as f32 * gap) / cols as f32;
        let x0 = (fw - (n as f32 * col_w + (n - 1) as f32 * gap)) / 2.0;
        for (k, (group, g, beside)) in sliders.iter().enumerate() {
            let left = x0 + k as f32 * (col_w + gap);
            let right = left + col_w;
            // WHAT CANNOT MOVE IS SAID in the slider's words' place, a moment
            // (Max, 2026-10-27), and the knob goes back to its stop.
            let note = self.note.as_ref().filter(|(key, _, until)| key == g.key && std::time::Instant::now() < *until);
            if let Some((_, words, _)) = note {
                out.texts.push(Text { text: (*words).to_owned(), x: left + col_w / 2.0, y: y + (lh - 18.0 * s) / 2.0, px: 13.5, alpha: 0.95, centered: true });
                let track = Rect::new(left, y + lh + th / 2.0 - 1.0 * s, col_w, 2.0 * s);
                let count = g.choices.len().max(2);
                let lit = g.choices.iter().position(|c| c.on).unwrap_or(0);
                let stops = self.slider_stops(out, *group, &g.choices, track, (left - 14.0 * s, right + 14.0 * s), (y - 4.0 * s, y + lh + th + 4.0 * s), g.split);
                out.sliders.push(Slider { track, value: lit as f32 / (count - 1) as f32, stops, group: *group, count, split: g.split, mark: g.mark });
                continue;
            }
            let (name, value) = slider_words(g);
            out.texts.push(Text { text: name, x: left, y: y + (lh - 18.0 * s) / 2.0, px: 13.5, alpha: 0.6, centered: false });
            // Its value at the right end, what stands beside it before that.
            let mut end = right;
            if !value.is_empty() {
                let vw = crate::options::est_text_w(&value, 12.0 * s);
                out.texts.push(Text { text: value, x: right - vw / 2.0, y: y + (lh - 16.0 * s) / 2.0, px: 12.0, alpha: 0.55, centered: true });
                end = right - vw - 14.0 * s;
            }
            if let Some((pg, b)) = beside {
                let w = self.inline_w(b);
                self.inline(out, end - w, y, lh, *pg, b);
            }
            let track = Rect::new(left, y + lh + th / 2.0 - 1.0 * s, col_w, 2.0 * s);
            let count = g.choices.len().max(2);
            let lit = g.choices.iter().position(|c| c.on).unwrap_or(0);
            // (The ends reach into the column gap, never across it.)
            let stops = self.slider_stops(out, *group, &g.choices, track, (left - 14.0 * s, right + 14.0 * s), (y - 4.0 * s, y + lh + th + 4.0 * s), g.split);
            out.sliders.push(Slider { track, value: lit as f32 / (count - 1) as f32, stops, group: *group, count, split: g.split, mark: g.mark });
        }
        // (A group's air under it, like every other group: the closing hint
        // sat on the tracks — rig, 2026-10-20.)
        y + lh + th + GROUP_GAP * s
    }

    /// Lay PICTURES out as a grid of previews from `y` down: as many columns
    /// as make them the biggest they can be with a page of them in the box.
    /// A PAGE is at most [`TILES_PER_PAGE`]; past that the rest are on
    /// further pages to the right, turned as the dock's box's are (Max,
    /// 2026-10-14: "que llegue a 45 y de ahí que aparezca otra página a la
    /// derecha, por desborde, como hacemos en la caja del dock"). Returns
    /// the top of what comes next — the same on every page.
    /// CARDS (the palettes, the apps) take the cards' grid instead: `rows`
    /// a page at most, when the group asks for fewer.
    fn flow_tiles(&self, out: &mut OpenContent, y: f32, group: u8, choices: &[crate::controls::Choice], rows: Option<usize>) -> f32 {
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
        // THE FIRST TILE WITH ITS WORDS (Max, 2026-10-18: the wallpaper box's
        // "Follows your colours"): a line of room under the first row for them,
        // and under no other (a wallpaper has no words on it).
        let swatches = choices.iter().any(|c| !c.swatch.is_empty());
        let icons = choices.iter().any(|c| c.icon.is_some());
        // (Pictures asked for as CARDS — the Colours box's Backgrounds, rows
        // given — stand in the cards' grid, a name under each.)
        let cards = rows.is_some();
        // (A palette's card and an app's: a name under every row.)
        let named = swatches || icons || cards;
        let captioned = !named && choices.first().is_some_and(|c| c.caption);
        let nh = if named || captioned { 20.0 * s } else { 0.0 };
        let cap_h = if captioned { nh } else { 0.0 };
        // The room top to foot: the box's own height (a box made to hug the
        // wall says how much it may take), less what is above the grid, the
        // dots and the closing hint under it.
        let whole = if self.tile_room_h > 0.0 { self.tile_room_h } else { self.field.h.max(self.key.1) };
        let room_h = (whole - y - 40.0 * s - dots_h - cap_h).max(TILE_ASPECT * 80.0 * s);
        // The biggest previews whose grid — as many rows as fit, each one
        // FULL — holds them all, or a page's worth ([`TILES_PER_PAGE`]) when
        // there are more. A page holds its whole grid, so every page but the
        // last is full to the last row (Max, 2026-10-14: "lo que yo quiero es
        // que todas las filas estén llenas … la página siguiente tiene que
        // estar por si se desborda la primera" — 63 to a page on a grid of
        // eight left the eighth row with seven, and the rest on page two).
        let want = choices.len().min(TILES_PER_PAGE);
        let mut best: Option<(usize, usize, f32)> = None;
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
        // THE CARDS' GRID (the Colours model, 2026-10-20): as wide as the
        // rows above (the same side margins), as many across as fit at
        // `PALETTE_TILE_MIN`, four to seven — a 960 field falls to four —
        // and four rows a page, or fewer when asked (`open_content` asks
        // for three when four run the box past its height).
        let best = if icons || cards || (swatches && self.wall_tile > 0.0) {
            let room = fw - 4.0 * MARGIN * s;
            let cols = (((room + gap) / (PALETTE_TILE_MIN * s + gap)).floor() as usize).clamp(PALETTE_COLS.0, PALETTE_COLS.1);
            let w = (room - (cols - 1) as f32 * gap) / cols as f32;
            Some((cols, rows.unwrap_or(PALETTE_ROWS).clamp(1, PALETTE_ROWS), w))
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
        // (A palette's name is written under its card: room for it — under
        // every row of palettes, under the first row only of a wall.)
        let row_y = |row: usize| {
            y + row as f32 * (h + gap) + if named { row as f32 * nh } else if row > 0 { cap_h } else { 0.0 }
        };
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
                        rect: Rect::new(x, row_y(row), w, h),
                        text: c.text.clone(),
                        on: c.on,
                        action: PanelAction::Pick { group, index: (first + k) as u8 },
                        // A palette's card is a picture too, once drawn. (An
                        // app's card is drawn from its icon.)
                        image: if c.icon.is_some() {
                            None
                        } else if swatches {
                            let card = if c.value == crate::palette::DYNAMIC_ID {
                                crate::palette_card::wheel(crate::palette::shell_dark())
                            } else {
                                crate::palette_card::path(&c.swatch).filter(|p| !c.swatch.is_empty() && p.is_file())
                            };
                            card.map(|p| p.to_string_lossy().into_owned())
                        } else {
                            Some(c.value.clone())
                        },
                        swatch: c.swatch.clone(),
                        seg: false,
                        palette: named || c.caption,
                        cell: false,
                        icon: c.icon,
                        hint: c.hint.clone(),
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
        let foot = y + rows as f32 * h + if named { rows as f32 * nh } else { cap_h } + (rows - 1) as f32 * gap;
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

    /// Whether a grid of cards laid with more than `rows` rows runs the box
    /// past the room it has — the box's own height where it is made to hug
    /// its content (`tile_room_h`), else the field's — with the closing hint
    /// under the content.
    fn cards_overflow(&self, view: &ControlView, content: &OpenContent, rows: usize) -> bool {
        let four = view.groups.iter().any(|g| {
            g.tiles && g.grid_rows.is_none_or(|r| r > rows) && (g.grid_rows.is_some() || g.choices.iter().any(|c| !c.swatch.is_empty() || c.icon.is_some()))
        });
        let whole = if self.tile_room_h > 0.0 { self.tile_room_h } else { self.field.h.max(self.key.1) };
        four && content.foot_y + QUICK_FOOT * self.scale > whole
    }

    /// A control's content (`open_content`): the view's groups laid out
    /// from the top down, into `out`. `fewer`: a grid of cards takes at most
    /// that many rows a page, not four.
    fn control_content(&self, view: &ControlView, mut out: OpenContent, fewer: Option<usize>) -> OpenContent {
        let s = self.scale;
        let top = CONTENT_TOP * s;
        out.subtitle = view.subtitle.clone();
        // A WALL of pictures sits tight under its title and over its
        // hint (Max, 2026-10-14: "para mí hay mucho aire abajo y arriba
        // de las imágenes").
        let wall = view.groups.iter().any(|g| g.tiles && !g.choices.is_empty());
        let mut y = if wall { WALL_TOP * s } else { top };
        let title_y = if wall { 20.0 } else { 30.0 } * s;
        // (The groups a row of sliders took along: up to here they are laid.)
        let mut laid = 0;
        for (gi, g) in view.groups.iter().enumerate() {
            if g.beside || gi < laid {
                continue; // (laid out with the group before it)
            }
            // (As tall as one line of words, `flow_words`: 26 and its gap.)
            if g.spacer {
                y += (26.0 + GROUP_GAP) * s;
                continue;
            }
            // THE MAIN CHOICE: bold words centred on the title's line.
            if g.main {
                let chips = g.choices.iter().enumerate().map(|(ci, c)| (c.text.clone(), c.on, PanelAction::Pick { group: gi as u8, index: ci as u8 })).collect();
                self.flow_words(&mut out, title_y + 2.0 * s, chips, true);
                continue;
            }
            let pair = view.groups.get(gi + 1).filter(|n| n.beside).map(|n| (gi as u8 + 1, n));
            // A SLIDER is one row: its name, the track, its value, its look.
            if g.slider {
                // A ROW OF THEM: this one and every row slider right after
                // it, each with what is laid beside it, on one line.
                if g.row {
                    let mut run = Vec::new();
                    let mut j = gi;
                    while let Some(h) = view.groups.get(j).filter(|h| h.slider && h.row) {
                        let pair = view.groups.get(j + 1).filter(|n| n.beside).map(|n| (j as u8 + 1, n));
                        run.push((j as u8, h, pair));
                        j += 1 + usize::from(pair.is_some());
                    }
                    laid = j;
                    y = self.flow_slider_row(&mut out, y, &run);
                    continue;
                }
                y = self.flow_slider(&mut out, y, gi as u8, g, pair);
                continue;
            }
            // THE SCREENS, DRAWN (the Monitors page).
            if !g.screens.is_empty() {
                y = self.flow_screens(&mut out, y, gi as u8, g);
                // (A turn being tried: kept, or it goes back by itself.)
                if let Some(left) = self.keep_left.filter(|_| self.keep_turn) {
                    let text = crate::i18n::fill(tr("Keep the screen turned? Going back in {0} s"), &[("0", &format!("{:.0}", left.ceil().max(1.0)))]);
                    y = self.caption(&mut out, y, text);
                    let chips = vec![
                        (tr("Keep").to_owned(), true, PanelAction::Keep),
                        (tr("Go back").to_owned(), false, PanelAction::Revert),
                    ];
                    y = self.flow(&mut out, y, 120.0, chips);
                }
                continue;
            }
            // ON OR OFF is a switch row: its name left, the switch right —
            // or, centred, the two together in the middle.
            if is_switch(g) {
                if g.centred {
                    y = self.switch_centred(&mut out, y, gi as u8, g);
                    continue;
                }
                let on = g.choices.first().is_some_and(|c| c.on);
                let name = g.caption.clone().unwrap_or_else(|| crate::i18n::tr("On").to_owned());
                let hint = g.choices.first().and_then(|c| c.hint.as_deref());
                y = self.switch_row(&mut out, y, gi as u8, &name, on, hint);
                continue;
            }
            // A WORD ROW: its name at the left, the words at the right end.
            if g.labelled && !g.choices.is_empty() {
                y = self.word_row(&mut out, y, gi as u8, g);
                // (A screen's resolution being tried, on the Monitors page:
                // kept, or it goes back by itself.)
                if g.key == "monitors-mode" && !self.keep_turn {
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
                continue;
            }
            // A CENTRED LINE TO READ (a section's reach, the chosen app's name):
            // muted words in the middle.
            if g.centred && g.choices.is_empty() && !g.slider && g.caption.is_some() {
                let text = g.caption.clone().unwrap_or_default();
                out.texts.push(Text { text, x: self.key.0 / 2.0, y, px: 13.0, alpha: 0.6, centered: true });
                y += 24.0 * s;
                // (Last of all — the Launcher's Ctrl +/− line — it keeps a
                // group's air under it: the closing hint sat on it.)
                if gi + 1 == view.groups.len() {
                    y += (GROUP_GAP - 8.0) * s;
                }
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
                let rows = match fewer {
                    Some(n) => Some(g.grid_rows.map_or(n, |r| r.min(n))),
                    None => g.grid_rows,
                };
                y = self.flow_tiles(&mut out, y, gi as u8, &g.choices, rows);
                continue;
            }
            let chips: Vec<(String, bool, PanelAction)> = g
                .choices
                .iter()
                .enumerate()
                .map(|(ci, c)| (c.text.clone(), c.on, PanelAction::Pick { group: gi as u8, index: ci as u8 }))
                .collect();
            let first = out.chips.len();
            let coloured = g.choices.iter().any(|c| !c.swatch.is_empty());
            // WORDS ON A LINE OF THEIR OWN, CENTRED, with what is laid
            // beside them at the line's left end (the Auto switch by the
            // section tabs).
            if g.centred && !g.buttons && !g.field && !coloured {
                let line = y;
                // (No words — the Backgrounds tab's line — is still a line, for what sits on it.)
                // (A hugging line with no words — an app with only its Colours
                // switch — is as tall as one with them: picking another app
                // moves nothing.)
                y = if chips.is_empty() {
                    y + if g.hug { (26.0 + GROUP_GAP) * s } else { 40.0 * s }
                } else {
                    self.flow_words(&mut out, y, chips, false)
                };
                // Every group beside it, left to right (Auto, then Ambient while on):
                // at the box's left, or — hugging — just left of the words,
                // the whole centred.
                let besides: Vec<(usize, &crate::controls::Group)> = (gi + 1..).map_while(|j| view.groups.get(j).filter(|b| b.beside).map(|b| (j, b))).collect();
                let air = 28.0 * s;
                let mut x = 2.0 * MARGIN * s;
                if g.hug && !besides.is_empty() && out.chips.len() > first {
                    let bw: f32 = besides.iter().map(|(_, b)| self.inline_w(b) + air).sum();
                    let shift = bw / 2.0;
                    for c in &mut out.chips[first..] {
                        c.rect.x += shift;
                    }
                    let left = out.chips[first..].iter().map(|c| c.rect.x).fold(f32::MAX, f32::min);
                    x = left - bw;
                } else if g.hug && !besides.is_empty() {
                    // (No words: what is beside them, alone, centred.)
                    let bw: f32 = besides.iter().map(|(_, b)| self.inline_w(b)).sum::<f32>() + air * (besides.len() - 1) as f32;
                    x = (self.key.0 - bw) / 2.0;
                }
                for (j, b) in besides {
                    self.inline(&mut out, x, line, 26.0 * s, j as u8, b);
                    x += self.inline_w(b) + air;
                }
                let _ = pair;
                continue;
            }
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
        out.foot_y = if wall { y - (GROUP_GAP - WALL_HINT_GAP) * s } else { (y + 8.0 * s - (GROUP_GAP - 12.0) * s).max(top) };
        out
    }

    /// The content of the open setting `label`, if it is a real one: the
    /// screen's choices for Scale and Resolution. `None`: a placeholder.
    fn open_content(&self, label: &str) -> Option<OpenContent> {
        let s = self.scale;
        let top = CONTENT_TOP * s;
        let blank = || OpenContent {
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
            lines: Vec::new(),
            screens_view: None,
        };
        let mut out = blank();
        if !self.modules && crate::controls::is_control(label) {
            // Not read yet: the bare card, for the moment the read takes.
            let Some(view) = self.control.as_ref().filter(|v| v.label == label) else {
                return Some(out);
            };
            self.screens_h.set(0.0);
            let content = self.control_content(view, blank(), None);
            // THE SCREENS' DRAWING gives up height to a box too short for the
            // whole page (the rows under it stay on it), down to a floor.
            if view.groups.iter().any(|g| !g.screens.is_empty()) {
                let whole = self.field.h.max(self.key.1);
                let over = content.foot_y + QUICK_FOOT * self.scale - whole;
                if over > 0.0 {
                    self.screens_h.set((SCREENS_H - over / self.scale.max(0.01)).max(SCREENS_MIN_H));
                    return Some(self.control_content(view, blank(), None));
                }
            }
            // FOUR ROWS OF CARDS, OR THREE (the Colours model, 2026-10-20):
            // laid out with four, a box that would run past its height is
            // laid out again with three — and two, if three still do.
            let mut content = content;
            for rows in [PALETTE_ROWS_FEWER, PALETTE_ROWS_FEWEST] {
                if !self.cards_overflow(view, &content, rows) {
                    break;
                }
                content = self.control_content(view, blank(), Some(rows));
            }
            return Some(content);
        }
        if !is_display_setting(label) {
            return None;
        }
        let Some(v) = &self.display else {
            out.subtitle = tr("No screen to set.").to_owned();
            return Some(out);
        };
        let mut y = top;
        // THE SCREENS (Max, 2026-10-21: "add monitors to the scale and
        // resolution pages"): with more than one, their names first, the one
        // the page sets lit — the Monitors page's picked screen.
        if v.screens.len() > 1 {
            let chips = v
                .screens
                .iter()
                .enumerate()
                .map(|(i, (sel, title))| (title.clone(), *sel == v.target, PanelAction::Target(i as u8)))
                .collect();
            y = self.flow_words(&mut out, y, chips, false) + 6.0 * s;
        }
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
            if let Some(left) = self.keep_left.filter(|_| !self.keep_turn) {
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
            Some(content) => {
                let at = |c: &&Chip| Rect::new(f.x + c.rect.x, f.y + c.rect.y, c.rect.w, c.rect.h).contains(pos);
                // (A screen's turning corner is over its box: it comes first.)
                let corner = content.chips.iter().filter(|c| matches!(c.style, Style::Corner { .. })).find(at);
                corner.or_else(|| content.chips.iter().find(at)).map(|c| Some(c.action))
            }
            None => content_hit((self.key.0, self.key.1, self.key.2), self.scale, pos, f).then_some(None),
        }
    }

    /// The game: score a use of pill `i`, and if it now outscores the
    /// weakest pill one layer nearer, swap the two. Saves the standings.
    fn record_use(&mut self, i: usize, now: u64) {
        let st = self.usage.settings.entry(self.pills[i].label.to_owned()).or_default();
        *st = Standing { layer: st.layer, score: st.score_at(now) + 1.0, at: now };
        if let Some(path) = &self.store {
            crate::persist::write_json("control panel", path, &self.usage);
        }
        // (A use moves nothing on the grid: every setting keeps its place.)
    }

    /// Set the search (from the card's search pill): the grid shows its
    /// matches only, each in its section, the rest leave; its best match
    /// is lit, softly, for Enter — the apps grid's way.
    pub(crate) fn set_query(&mut self, query: &str) {
        let q = query.trim().to_lowercase();
        if q == self.query {
            return;
        }
        self.query = q;
        if self.pills.is_empty() {
            return;
        }
        self.compose(false);
        // Entering or leaving a search: back to the top.
        self.scroll_to = 0.0;
        self.keyed = false;
        self.sel = if self.query.is_empty() { None } else { self.best_match() };
    }

    /// The best match of the search among the tiles shown: a name that
    /// starts with it, then a name that contains it, then a keyword match;
    /// the first in the grid among equals.
    fn best_match(&self) -> Option<usize> {
        self.pills
            .iter()
            .enumerate()
            .filter(|(_, p)| p.shown)
            .filter_map(|(i, p)| Some((match_rank(p.label, p.keywords, &self.query)?, p.slot, i)))
            .min()
            .map(|(.., i)| i)
    }

    /// Open the best match of the search (`best_match`). Returns whether
    /// there was one.
    fn open_best(&mut self) -> bool {
        if self.query.is_empty() || self.open.is_some() {
            return false;
        }
        match self.best_match() {
            Some(i) => {
                match self.boxed(i) {
                    Some(label) => self.action = Some(PanelAction::Boxed(label)),
                    None => self.open_pill(i),
                }
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
        self.knob = None;
        // (A screen of the Monitors page let go outside: not still in hand.)
        self.screen_drag = None;
        self.hot = None;
        self.sel = None;
        self.keyed = false;
        self.query.clear();
        self.scroll_to = 0.0;
        self.scroll = 0.0;
        if self.pills.is_empty() {
            return;
        }
        self.compose(true);
        for p in &mut self.pills {
            p.sc = 1.0;
            p.op = 1.0;
        }
        info!("control panel: panel reset, every tile home");
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
        self.scroll = approach(self.scroll, self.scroll_to, SCROLL_RATE, dt);
        if (self.scroll - self.scroll_to).abs() < 0.3 {
            self.scroll = self.scroll_to;
        }
        if moved && !self.tiles.is_settling() && self.open.as_ref().is_some_and(|o| o.want > 0.5) {
            let chips = self.key_chips();
            if !chips.is_empty() {
                let f = self.field;
                self.focus = pointer
                    .and_then(|pos| chips.iter().position(|c| Rect::new(f.x + c.0.x, f.y + c.0.y, c.0.w, c.0.h).contains(pos)));
            }
        }
        // Folding back: into where the tile's plate is now — the order may
        // have moved it while it was open.
        let fold_to = self.open.as_ref().filter(|o| o.want == 0.0 && !self.solo).map(|o| self.icon_rect(&self.pills[o.pill]));
        if let Some(open) = &mut self.open {
            if let Some(from) = fold_to {
                open.from = from;
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
        // THE SELECTION follows the hand as the apps grid's does
        // (`pointer_takes_selection`): on a tile it is that tile, softly;
        // off every tile with no search on, nothing — the keys' choice stays
        // until the pointer moves.
        if moved && self.open.is_none() {
            match self.hot {
                Some(i) => {
                    self.sel = Some(i);
                    self.keyed = false;
                }
                None if self.query.is_empty() => self.sel = None,
                None => {}
            }
        }
        for p in &mut self.pills {
            p.anchor.0 = approach(p.anchor.0, p.home.0, GLIDE_RATE, dt);
            p.anchor.1 = approach(p.anchor.1, p.home.1, GLIDE_RATE, dt);
            p.sc = approach(p.sc, 1.0, SIZE_RATE, dt);
            p.op = approach(p.op, if p.shown { 1.0 } else { 0.0 }, FADE_IN_RATE, dt);
            p.pos = p.anchor;
        }
    }

    /// Whether nothing of the panel moves — then it needs no frame until
    /// something does (`App::orbit_soon` asks). A tile gliding, growing,
    /// fading, under the pointer, a setting opening: not calm.
    pub(crate) fn calm(&self) -> bool {
        const E: f32 = 0.002;
        let still = |a: f32, b: f32| (a - b).abs() < E;
        let open_still = self.open.as_ref().is_none_or(|o| {
            if self.solo {
                o.want > 0.0 && still(o.shown, 1.0) && still(o.pop, 1.0) && o.vel.abs() < E
            } else {
                still(o.k, o.want)
            }
        });
        !self.tiles.is_settling()
            && (self.scroll - self.scroll_to).abs() < E
            && open_still
            && self.pills.iter().all(|p| {
                (p.anchor.0 - p.home.0).abs() < 0.05
                    && (p.anchor.1 - p.home.1).abs() < 0.05
                    && still(p.sc, 1.0)
                    && still(p.op, if p.shown { 1.0 } else { 0.0 })
            })
    }

    /// Everything to draw this frame.
    fn draw(&self, paint: PanelPaint) -> PanelDraw {
        let mut out = PanelDraw::default();
        let s = self.scale;
        // (The grid's field: the card's width; the open setting's is centred in it.)
        let f = self.grid_field;
        let g = &self.grid;
        let open_k = self.open.as_ref().map_or(0.0, |o| o.k);
        let open_pill = self.open.as_ref().map(|o| o.pill);
        // (The open box's start, in the grid field's coordinates.)
        let open_from = self.open.as_ref().map(|o| Rect::new(o.from.x + self.field.x - f.x, o.from.y + self.field.y - f.y, o.from.w, o.from.h));
        let ink = paint.ink;
        let hl = paint.highlight;
        let blank = crate::app_actions::symbol("blank");
        // (The pointer in field coordinates, for the magnifying.)
        let pointer = self.pointer.map(|(x, y)| (x - f.x, y - f.y));
        let clip = self.grid_clip();

        // THE GRID, cut at the field's window as the apps' sections are.
        let (mut rects, mut labels, mut symbols) = (Vec::new(), Vec::new(), Vec::new());
        // (One setting in a box of its own: no field behind it.)
        let order: Vec<usize> = if self.solo { Vec::new() } else { (0..self.pills.len()).collect() };
        // The sections' words over their strips, the apps' title word
        // (fading with the tiles as a setting opens over them).
        if !self.solo {
            for sec in &g.sections {
                let Some(title) = sec.title else { continue };
                let y = f.y + sec.title_y - self.scroll + content::TITLE_DROP;
                if y + content::TITLE_LINE_PX < clip.y || y > clip.y + clip.h {
                    continue;
                }
                let t = paint.title_ink;
                labels.push(Label {
                    text: crate::i18n::tr(title).to_owned(),
                    pos: (f.x + sec.vp.x + sec.vp.w / 2.0, y),
                    max_w: sec.vp.w,
                    font_px: content::TITLE_FONT_PX,
                    line_px: content::TITLE_LINE_PX,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: Some(content::FONT_BOLD),
                    color: Some([t[0], t[1], t[2], t[3] * (1.0 - open_k)]),
                    clip: None,
                });
            }
        }
        for i in order {
            let p = &self.pills[i];
            let (mut dx, mut dy, mut fade, mut shrink) = (0.0, 0.0, 1.0, 1.0);
            if let (Some(op), Some(from)) = (open_pill, open_from) {
                if op == i {
                    fade = 0.0; // the box has taken its place
                } else {
                    // Opened out of the clicked tile: pushed away along the
                    // line from it, fading, the nearest furthest.
                    let (cx, cy) = (from.x + from.w / 2.0, from.y + from.h / 2.0);
                    let (ox, oy) = (p.pos.0 - cx, p.pos.1 - self.scroll - cy);
                    let dist = ox.hypot(oy).max(1.0);
                    let push = 140.0 * s * open_k * (0.6 + 0.4 * (260.0 * s / dist).min(1.0));
                    dx += ox / dist * push;
                    dy += oy / dist * push;
                    fade = 1.0 - open_k;
                    shrink = 1.0 - 0.12 * open_k;
                }
            }
            let a = p.op.max(0.0) * fade;
            if a < 0.004 {
                continue;
            }
            let c0 = self.cell_now(p);
            let cell = Rect::new(c0.x + dx, c0.y + dy, c0.w, c0.h);
            // (Scrolled out of the window: not drawn.)
            if f.y + cell.y + cell.h + content::LABEL_LINE_PX < clip.y || f.y + cell.y > clip.y + clip.h {
                continue;
            }
            let k = p.sc * shrink;
            let cx = cell.x + cell.w / 2.0;
            let icon_cy = cell.y + g.icon_top + g.icon / 2.0;
            // The box under the pointer, or where the keys are (stronger),
            // or on a search's best match: the apps grid's own.
            if self.sel == Some(i) || (self.sel.is_none() && self.hot == Some(i)) {
                let strong = self.sel == Some(i) && self.keyed;
                let alpha = if strong { (hl[3] * 1.8).min(0.4) } else { hl[3] };
                rects.push(RectInst {
                    rect: Rect::new(f.x + cell.x + CELL_INSET, f.y + cell.y + CELL_INSET, cell.w - 2.0 * CELL_INSET, cell.h - 2.0 * CELL_INSET),
                    radius: CELL_RADIUS,
                    color: [hl[0], hl[1], hl[2], alpha * a],
                    glass: 0.0,
                    border: 0.0,
                });
            }
            // The icon grows a little under the pointer, as the apps do.
            let mag = pointer.map_or(1.0, |(px, py)| {
                let d = (px - cx).hypot(py - icon_cy);
                1.0 + (content::GRID_MAGNIFY - 1.0) * content::falloff(d, content::GRID_MAG_RADIUS)
            });
            let size = g.icon * k * mag;
            let icon = Rect::new(f.x + cx - size / 2.0, f.y + icon_cy - size / 2.0, size, size);
            // Its plate — the dock's, under every app's icon — and the
            // setting's symbol on it, in the card's ink.
            symbols.push(PanelSymbol { sym: blank, rect: icon, color: [paint.plate[0], paint.plate[1], paint.plate[2], paint.plate[3] * a], plate: true });
            let inner = size * SYMBOL_SHARE;
            symbols.push(PanelSymbol {
                sym: crate::app_actions::symbol(symbol_of(p.label)),
                rect: Rect::new(icon.x + (size - inner) / 2.0, icon.y + (size - inner) / 2.0, inner, inner),
                color: [ink[0], ink[1], ink[2], ink[3] * a],
                plate: false,
            });
            // Its name: on two lines where nothing stands under its cell
            // (its section's last row, or an empty slot below), else one.
            let (first, second) = match (&p.two, g.free_under(p.slot)) {
                (Some((a, b)), true) => (a.as_str(), Some(b.as_str())),
                _ => (p.one.as_str(), None),
            };
            let ly = f.y + cell.y + g.icon_top + g.icon + content::GRID_LABEL_GAP;
            let colour = Some([ink[0], ink[1], ink[2], ink[3] * a]);
            let name = |text: &str, y: f32| Label {
                text: text.to_owned(),
                pos: (f.x + cx, y),
                max_w: cell.w - 12.0,
                font_px: content::LABEL_FONT_PX,
                line_px: content::LABEL_LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                family: None,
                color: colour,
                clip: None,
            };
            labels.push(name(first, ly));
            if let Some(second) = second {
                labels.push(name(second, ly + content::SECOND_LINE));
            }
        }
        if !(rects.is_empty() && labels.is_empty() && symbols.is_empty()) {
            out.clipped.push((clip, rects, labels, symbols));
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
        // (The one layer the pills ever wear now: the bar's own pill size.)
        let d = 1usize;
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
            let radius = lerp(open.from.w * PLATE_CORNER, content::BOX_CORNER_RADIUS, k);
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
            y += if wall { 34.0 } else { 38.0 } * s;
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
                // The ground is the picture's stand-in while it is not up
                // (the box on its way, a page sliding): under the picture
                // itself nothing is drawn — its own rounded edge is the only
                // one seen (Max, 2026-10-21: "the rectangles behind the items
                // show behind … weird corners, and like shinny stuff").
                if !resting {
                    out.rects.push(RectInst { rect: tile, radius, color: [wash[0], wash[1], wash[2], wash[3] * ca], glass: 0.0, border: 0.0 });
                }
                if resting {
                    // (The wallpapers wear the Colours slider: `wall_tint`.)
                    out.images.push((tile, file));
                    out.picture_radius = radius;
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
            // AN APP'S CARD (the Colours model, 2026-10-20: the Apps
            // section's exceptions as tiles): a card's ground, the app's
            // icon centred on it — through the dock's icon layers, as fast
            // launch's are, once the box rests — its exception in a small
            // line at the foot; its name under the card like a palette's.
            let card_at = |out: &mut PanelDraw, r: Rect, dx: f32, entry: usize, hint: Option<&str>, on: bool, keyed: bool| {
                if r.x + dx < 0.0 || r.x + dx + r.w > fw {
                    return;
                }
                let (x, y0) = (r.x + dx, r.y);
                let radius = TILE_RADIUS * s;
                rect(out, x, y0, r.w, r.h, radius, crate::options::wash(!paint.bright, layer_wash[2]));
                if resting {
                    let side = (TILE_ICON * s).min(r.h * 0.7);
                    let lift = if hint.is_some() { 6.0 * s } else { 0.0 };
                    let (mx, my) = map(x + (r.w - side) / 2.0, y0 + (r.h - side) / 2.0 - lift);
                    out.icons.push((entry, Rect::new(mx, my, side * cs, side * cs)));
                }
                if let Some(h) = hint {
                    text(out, h.to_owned(), x + r.w / 2.0, y0 + r.h - 18.0 * s, 10.5, 13.0, 0.5);
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
                let r = chip.rect;
                let whole = r.x + content.ghost_slide >= 0.0 && r.x + content.ghost_slide + r.w <= fw;
                if let Some(entry) = chip.icon {
                    card_at(out, r, content.ghost_slide, entry, chip.hint.as_deref(), chip.on, false);
                    if whole {
                        text(out, chip.text.clone(), r.x + content.ghost_slide + r.w / 2.0, r.y + r.h + 4.0 * s, 12.0, 15.0, 0.55);
                    }
                } else if let Some(file) = chip.image {
                    tile_at(out, chip.rect, content.ghost_slide, file, false);
                    if chip.palette && whole {
                        text(out, chip.text.clone(), r.x + content.ghost_slide + r.w / 2.0, r.y + r.h + 4.0 * s, 12.0, 15.0, 0.55);
                    }
                } else if chip.palette {
                    swatch_at(out, chip.rect, content.ghost_slide, &chip.swatch, chip.on, false);
                }
            }
            // The SLIDERS: a quiet track, filled to the knob, the knob ringed
            // when the keys are on it. The track is drawn in its spans — a
            // split track's middle gap and its notches are where nothing is
            // — and a split track has no fill: its halves are two scales
            // (Dark, Light), not one amount.
            for sl in &content.sliders {
                let t = sl.track;
                let ink_c = |a: f32| [paint.ink[0], paint.ink[1], paint.ink[2], a];
                let spans = track_spans(t.w, sl.split, sl.mark);
                for &(a, b) in &spans {
                    rect(out, t.x + a * t.w, t.y, (b - a) * t.w, t.h, t.h / 2.0, ink_c(0.16));
                }
                let kx = t.x + t.w * knob_shown(sl.value, sl.count, self.knob_of(sl));
                if !sl.split {
                    // (A knob at the start still shows a dot of fill.)
                    let to = kx.max(t.x + t.h);
                    for &(a, b) in &spans {
                        let (x0, x1) = (t.x + a * t.w, (t.x + b * t.w).min(to));
                        if x1 > x0 {
                            rect(out, x0, t.y, x1 - x0, t.h, t.h / 2.0, ink_c(0.9));
                        }
                    }
                }
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
            // THE CARDS SHOW THE SLIDERS (Max, 2026-10-21: "the pallete and
            // the saturation slider have to show the prev on the cards, like
            // the palette on BG cards"): a section's Palette and Saturation,
            // at the knobs as they are this frame, scale the palettes' colour —
            // Palette from grey (0) through as made (its notch) to full
            // colour, Saturation from grey to vivid, as `tint::saturate` does.
            let mut grade = (None, None);
            for sl in &content.sliders {
                let key = self.control.as_ref().and_then(|c| c.groups.get(sl.group as usize)).map(|g| g.key);
                let t = knob_shown(sl.value, sl.count, self.knob_of(sl));
                match key {
                    Some("colors-shell" | "colors-apps" | "colors-icons" | "colors-web") => {
                        grade.0 = Some(if t <= 0.5 { t / 0.5 } else { 1.0 + (t - 0.5) / 0.5 * CARD_FULL });
                    }
                    Some("colors-shell-sat" | "colors-apps-sat" | "colors-icons-sat" | "colors-web-sat") => grade.1 = Some(t / 0.5),
                    _ => {}
                }
            }
            if grade.0.is_some() || grade.1.is_some() {
                out.card_chroma = Some(grade.0.unwrap_or(1.0) * grade.1.unwrap_or(1.0));
            }
            for (i, chip) in content.chips.into_iter().enumerate() {
                let r = chip.rect;
                // (The one cursor — the pointer's or the keys': `step`.)
                let keyed = resting && self.focus == Some(i);
                let whole = r.x + content.slide >= 0.0 && r.x + content.slide + r.w <= fw;
                if let Some(entry) = chip.icon {
                    card_at(out, r, content.slide, entry, chip.hint.as_deref(), chip.on, keyed && content.slide.abs() < 0.5);
                    if whole {
                        let a = if chip.on || keyed { 1.0 } else { 0.55 };
                        text(out, chip.text.clone(), r.x + content.slide + r.w / 2.0, r.y + r.h + 4.0 * s, 12.0, 15.0, a);
                    }
                    continue;
                }
                if let Some(file) = chip.image {
                    tile_at(out, r, content.slide, file, keyed && content.slide.abs() < 0.5);
                    if chip.palette && r.x + content.slide >= 0.0 && r.x + content.slide + r.w <= fw {
                        let a = if chip.on || keyed { 1.0 } else { 0.55 };
                        // (Centred under its tile — unless its words are wider than
                        // the tile and the tile stands at the box's side: then they
                        // are kept inside the margin, not cut by the box's edge.)
                        let half = chip.text.chars().count() as f32 * 3.0 * s;
                        let lo = MARGIN * s + half;
                        let hi = (fw - MARGIN * s - half).max(lo);
                        let mid = (r.x + content.slide + r.w / 2.0).clamp(lo, hi);
                        text(out, chip.text.clone(), mid, r.y + r.h + 4.0 * s, 12.0, 15.0, a);
                    }
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
                        let (tw, th) = (SWITCH_W * s, 20.0 * s);
                        let (tx, ty) = (r.x + r.w - 6.0 * s - tw, r.y + (r.h - th) / 2.0);
                        let (mx, my) = map(tx, ty);
                        let knob = 12.0 * s;
                        if on {
                            // (THE KIT's switch: on = the accent, its knob in the
                            // accent's contrast.)
                            let (acc, k) = (crate::kit::linear(crate::kit::accent()), crate::kit::linear(crate::kit::on_accent()));
                            rect(out, tx, ty, tw, th, th / 2.0, [acc[0], acc[1], acc[2], ca]);
                            rect(out, tx + tw - 4.0 * s - knob, ty + 4.0 * s, knob, knob, knob / 2.0, [k[0], k[1], k[2], ca]);
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
                    // A BUTTON (THE KIT's word buttons): a faint fill, stronger
                    // under the pointer or the keys; the main one the accent's
                    // words on its tint.
                    Style::Button { main } => {
                        if main {
                            let dark = !paint.bright;
                            let t = crate::kit::main_tint(dark, keyed);
                            let l = crate::kit::linear(t);
                            rect(out, r.x, r.y, r.w, r.h, r.h / 2.0, [l[0], l[1], l[2], t[3] * ca]);
                            let w = crate::kit::accent_on(dark);
                            label_in(out, chip.text, r.x + r.w / 2.0, r.y + (r.h - line) / 2.0, r.w, 14.0, [w[0], w[1], w[2], ca]);
                        } else {
                            rect(out, r.x, r.y, r.w, r.h, r.h / 2.0, crate::options::wash(!paint.bright, if keyed { hover_a } else { 0.08 }));
                            label_in(out, chip.text, r.x + r.w / 2.0, r.y + (r.h - line) / 2.0, r.w, 14.0, ink_c(if chip.on || keyed { 1.0 } else { 0.85 }));
                        }
                    }
                    // A FIELD: its words over a hairline.
                    Style::Field => {
                        let a = if chip.on { 1.0 } else { 0.6 };
                        label_in(out, chip.text, r.x + r.w / 2.0, r.y + (r.h - line) / 2.0, r.w, 14.0, ink_c(a));
                        rect(out, r.x, r.y + r.h - 2.0 * s, r.w, 1.0 * s, 0.0, ink_c(if chip.on || keyed { 0.8 } else { 0.3 }));
                    }
                    // A SCREEN of the Monitors page's drawing: a hairline box,
                    // a faint wash when picked (more while in the hand), the
                    // Options bar along its top edge and the dock on its
                    // bottom — its top turned with it (a quarter turn
                    // clockwise puts it on the right).
                    Style::Screen { rot, bar, dock, held } => {
                        if held || chip.on {
                            rect(out, r.x, r.y, r.w, r.h, 3.0 * s, crate::options::wash(!paint.bright, if held { 0.10 } else { 0.05 }));
                        }
                        let (mx, my) = map(r.x, r.y);
                        let a = if chip.on || held || keyed { 1.0 } else { 0.5 };
                        out.rects.push(RectInst { rect: Rect::new(mx, my, r.w * cs, r.h * cs), radius: 3.0 * s * cs, color: ink_c(a), glass: 0.0, border: (1.0 * s * cs).max(1.0) });
                        // An edge's strip: `t` thick, `pad` in from the edge,
                        // from `from` to `to` (fractions) along it.
                        let strip = |edge: u16, t: f32, pad: f32, from: f32, to: f32| -> (f32, f32, f32, f32) {
                            match edge {
                                90 => (r.x + r.w - pad - t, r.y + r.h * from, t, r.h * (to - from)),
                                180 => (r.x + r.w * from, r.y + r.h - pad - t, r.w * (to - from), t),
                                270 => (r.x + pad, r.y + r.h * from, t, r.h * (to - from)),
                                _ => (r.x + r.w * from, r.y + pad, r.w * (to - from), t),
                            }
                        };
                        if bar {
                            let (x, y, w, h) = strip(rot % 360, 3.0 * s, 0.0, 0.0, 1.0);
                            rect(out, x, y, w, h, 0.0, ink_c(0.4));
                        }
                        if dock {
                            let (x, y, w, h) = strip((rot + 180) % 360, 2.5 * s, 3.0 * s, 0.28, 0.72);
                            rect(out, x, y, w, h, 1.2 * s, ink_c(0.4));
                        }
                        if !chip.text.is_empty() {
                            label_in(out, chip.text, r.x + r.w / 2.0, r.y + (r.h - 11.0 * 1.3 * s) / 2.0, (r.w - 8.0 * s).max(1.0), 11.0, ink_c(if chip.on || held { 0.9 } else { 0.65 }));
                        }
                    }
                    // A TURNING CORNER of the picked screen: a small round
                    // button over its corner, the way it turns on it.
                    Style::Corner { back } => {
                        let ground = if paint.bright { [0.96, 0.96, 0.95, ca] } else { [0.12, 0.12, 0.13, ca] };
                        rect(out, r.x, r.y, r.w, r.h, r.h / 2.0, ground);
                        let (mx, my) = map(r.x, r.y);
                        out.rects.push(RectInst { rect: Rect::new(mx, my, r.w * cs, r.h * cs), radius: r.h / 2.0 * cs, color: ink_c(if keyed { 0.95 } else { 0.55 }), glass: 0.0, border: (1.0 * s * cs).max(1.0) });
                        let glyph = if back { "↺" } else { "↻" };
                        label_in(out, glyph.to_owned(), r.x + r.w / 2.0, r.y + (r.h - 13.0 * 1.3 * s) / 2.0, r.w, 13.0, ink_c(0.9));
                    }
                }
            }
            // The edge two screens touch along while one is in the hand.
            for l in &content.lines {
                rect(out, l.x, l.y, l.w, l.h, l.w.min(l.h) / 2.0, [paint.ink[0], paint.ink[1], paint.ink[2], 0.95 * ca]);
            }
            text(out, crate::controls::closing_hint().to_owned(), cx, content.foot_y, 12.0, 16.0, 0.4);
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
                // (Quiet, 2026-10-16: a program is its words and its switch;
                // the wash under the pointer only — the switch says on.)
                if hot {
                    rect(&mut inside, rx, ry, rw, rh, rh / 2.0, wash(hover_a));
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
                // The slim switch (the panel's own): on, filled with the ink
                // and a dark knob; off, an outlined capsule.
                let (tw, th) = (36.0 * s, 20.0 * s);
                let (tx, ty) = (rx + rw - 9.0 * s - tw, ry + (rh - th) / 2.0);
                let knob = 12.0 * s;
                if on {
                    // (THE KIT's switch: on = the accent.)
                    let (acc, k) = (crate::kit::linear(crate::kit::accent()), crate::kit::linear(crate::kit::on_accent()));
                    rect(&mut inside, tx, ty, tw, th, th / 2.0, acc);
                    rect(&mut inside, tx + tw - 4.0 * s - knob, ty + 4.0 * s, knob, knob, knob / 2.0, k);
                } else {
                    let (mx, my) = map(tx, ty);
                    inside.rects.push(RectInst {
                        rect: Rect::new(mx, my, tw * cs, th * cs),
                        radius: th / 2.0 * cs,
                        color: [paint.ink[0], paint.ink[1], paint.ink[2], 0.38 * ca],
                        glass: 0.0,
                        border: (1.0 * s * cs).max(1.0),
                    });
                    rect(&mut inside, tx + 4.0 * s, ty + 4.0 * s, knob, knob, knob / 2.0, [paint.ink[0], paint.ink[1], paint.ink[2], 0.75]);
                }
            }
            out.clipped.push((clip, inside.rects, inside.labels, Vec::new()));
            // The one button and what it means.
            let b = l.button;
            let hot = resting && self.pointer.is_some_and(|pos| Rect::new(f.x + b.x, f.y + b.y, b.w, b.h).contains(pos));
            let (fill, ring) = match l.kind {
                ModButton::ConfirmRemove => ([1.0, 0.18, 0.18, 0.35], [1.0, 0.55, 0.55, 0.7]),
                ModButton::Apply | ModButton::Confirm => (wash(if hot { 0.30 } else { 0.22 }), [paint.ink[0], paint.ink[1], paint.ink[2], 0.9]),
                // (Quiet: the plain button a hairline, filled only under the pointer.)
                _ => (wash(if hot { 0.27 } else { 0.0 }), [paint.ink[0], paint.ink[1], paint.ink[2], 0.3]),
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
            text(out, crate::controls::closing_hint().to_owned(), cx, l.hint_y, 12.0, 16.0, 0.4);
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
        text(out, crate::controls::closing_hint().to_owned(), cx, y + 6.0 * s, 12.0, 16.0, 0.4);
    }
}

/// The settings about the screen, fed by [`crate::display`].
/// Where the drawing puts the screens: their bounds fitted to `area` with
/// air around, centred (mirrored: each centred, the biggest fitted).
fn screens_fit(screens: &[crate::controls::ScreenShape], area: Rect, mirrored: bool) -> ScreenView {
    if screens.is_empty() {
        return ScreenView { k: 1.0, ox: area.x, oy: area.y };
    }
    let (x0, y0, x1, y1) = if mirrored {
        let w = screens.iter().map(|s| s.w).fold(1.0_f64, f64::max);
        let h = screens.iter().map(|s| s.h).fold(1.0_f64, f64::max);
        (0.0, 0.0, w, h)
    } else {
        (
            screens.iter().map(|s| s.x).fold(f64::MAX, f64::min),
            screens.iter().map(|s| s.y).fold(f64::MAX, f64::min),
            screens.iter().map(|s| s.x + s.w).fold(f64::MIN, f64::max),
            screens.iter().map(|s| s.y + s.h).fold(f64::MIN, f64::max),
        )
    };
    let (bw, bh) = ((x1 - x0).max(1.0) as f32, (y1 - y0).max(1.0) as f32);
    let k = (area.w * 0.86 / bw).min(area.h * 0.8 / bh);
    ScreenView {
        k,
        ox: area.x + (area.w - bw * k) / 2.0 - x0 as f32 * k,
        oy: area.y + (area.h - bh * k) / 2.0 - y0 as f32 * k,
    }
}

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
        // (An app's page goes with the results it was opened from: the panel
        // took every click while the page stood under it — Max, 2026-10-30.)
        self.page_close();
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
                self.search.deep = None;
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
        // Beside a side dock: the box's room (the card less the band), centred.
        if settled.sideways() {
            let room = settled.box_area(dock_h);
            let (w, h) = (room.w.min(FIELD_MAX.0 * s), room.h.min(FIELD_MAX.1 * s));
            return Rect::new(room.x + (room.w - w) / 2.0, room.y + (room.h - h) / 2.0, w, h);
        }
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

    /// THE GRID'S field: the card's whole width under the dock, as tall as
    /// the card is (grown for the sections: `apps_trim_target`) — the
    /// sections gain columns with the card as the apps' do (Max,
    /// 2026-11-07: "que las líneas se expandan hacia los lados").
    fn panel_grid_rect(&self, layout: &Layout) -> Rect {
        let settled = self.layout_at_rest(self.ui.extent_of(Target::Open));
        let dock_h = self.config.window.input_bar_height as f32 * self.icon_scale();
        let ride = layout.sections[content::SECTION_APPS].title_pos.1
            - settled.sections[content::SECTION_APPS].title_pos.1;
        if settled.sideways() {
            return settled.box_area(dock_h);
        }
        // (The card as it stands this frame — grown, or on its way: the
        // layout lays the sections out by the width, so only the window
        // onto them follows the height.)
        let card_h = (settled.card_h - dock_h).max(1.0);
        Rect::new(settled.card_x, settled.card_top + dock_h + ride, settled.card_w, card_h)
    }

    /// Advance the panel by `dt` and return what it draws this frame.
    pub(crate) fn panel_frame(&mut self, layout: &Layout, dt: f32, paint: PanelPaint) -> PanelDraw {
        let field = self.panel_field(layout);
        let grid = self.panel_grid_rect(layout);
        let scale = self.options_scale();
        // (The grid at the box's icon size, as the apps are: Ctrl +/−.)
        let icon_scale = self.icon_scale();
        self.panel.grid_field = grid;
        if let Some(r) = self.renderer.as_mut() {
            let mut measure = |t: &str, px: f32| r.measure_text(t, px, TEXT_FONT);
            self.panel.ensure((field.w, field.h), grid.w, scale, icon_scale, &mut measure);
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
        self.panel.keep_turn = self.monitors_turn.is_some();
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
            old.icons.append(&mut draw.icons);
            old.symbols.append(&mut draw.symbols);
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
        if crate::controls::box_of(label) == "Colours" {
            self.settings_box(label);
            return;
        }
        info!("control panel: {label} from the apps search");
        self.close_group();
        self.page_close();
        // The Controls row holds settings, never modules.
        self.panel_switch(false);
        if self.panel_reset_due {
            // Out of sight until now: start it fresh before opening.
            self.panel.reset();
            self.panel_reset_due = false;
        }
        self.search.deep = None;
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
        // …or a screen of the Monitors page's drawing.
        if self.panel.screen_dragging() {
            self.panel.screen_drag_to(pos);
            self.schedule_frame();
            return;
        }
        self.slider_to(from, pos, false);
    }

    /// A press that began on a slider at `from`, now at `pos`: the knob
    /// goes under the pointer (continuous, drawn at once from the panel's
    /// own state) and the stop nearest to it is picked (the control panel's
    /// or, `quick`, the quick box's). The wallpapers' Colours is not picked
    /// stop by stop: its level is only SHOWN, live, on the card — the tiles
    /// are graded with it every frame — and kept when the pointer goes up
    /// (`slider_end`), so the wallpaper itself is made once.
    pub(crate) fn slider_to(&mut self, from: (f32, f32), pos: (f32, f32), quick: bool) {
        let panel = if quick { &mut self.quick } else { &mut self.panel };
        let pick = panel.slide(from, pos);
        let knob = panel.knob_held();
        if let Some(k) = knob.filter(|k| k.key == WALL_SLIDER || k.key == WALL_SAT) {
            crate::wall_tint::drag_begin();
            crate::palette::set_wall_live(Some((k.key == WALL_SAT, knob_level(k.v))));
        } else if let Some((group, index)) = pick {
            // (A colour slider in hand: the programs are written as it is let go.)
            if knob.is_some() {
                crate::palette::drag_begin();
            }
            self.control_pick(group, index, quick);
        }
        if knob.is_some() {
            self.schedule_frame();
        }
    }

    /// The pointer went up after a drag on a slider: the knob stays where it
    /// was left. The wallpapers' Colours is kept now, at the level the knob
    /// was left at, and the wallpaper is made once.
    pub(crate) fn slider_end(&mut self, quick: bool) {
        let panel = if quick { &mut self.quick } else { &mut self.panel };
        let Some(k) = panel.knob_let_go() else { return };
        if k.key != WALL_SLIDER && k.key != WALL_SAT {
            crate::palette::drag_end();
            return;
        }
        // (The lit stop first, as a pick does, then the exact level.)
        let label = panel.control_pick(k.group, k.stop).map(|(label, ..)| label);
        self.colors_pick(k.key, &knob_level(k.v).to_string());
        crate::palette::set_wall_live(None);
        crate::wall_tint::drag_end();
        if let Some(label) = label {
            let ui = self.ui_facts();
            self.controls_send(ControlCommand::Read(label, ui));
        }
        self.schedule_frame();
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
                Some(PanelAction::Target(i)) => self.display_pick_screen(usize::from(i)),
                Some(PanelAction::Boxed(label)) => self.settings_box(label),
                None => {}
            }
            self.schedule_frame();
        }
    }

    /// Hand the settings' worker a command, starting it at the first one.
    pub(crate) fn controls_send(&mut self, cmd: ControlCommand) {
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

    /// A text field of Colours (or of Printers: an address) is being typed in,
    /// and its page is up.
    pub(crate) fn colours_typing(&self) -> bool {
        let on = |p: &Panel| matches!(p.open_control_label(), Some("Colours" | "Printers"));
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
                // (A printer's address can be a whole URI: longer than a name.)
                Some(t) if !t.is_empty() && t.chars().all(|c| !c.is_control()) && text.chars().count() < if key == "printers-add" { 200 } else { 40 } => text.push_str(t),
                _ => return,
            },
        }
        crate::controls::field_typed(key, &text, done);
        if let Ok(mut t) = crate::controls::TYPING.lock() {
            *t = if leave { None } else { Some((key, text)) };
        }
        if done && key.starts_with("colors-") && key != "colors-apps-find" {
            crate::palette::set_preview(crate::controls::editor_draft());
            self.colors_apply(false);
        }
        let ui = self.ui_facts();
        let label = if key.starts_with("printers-") { "Printers" } else { "Colours" };
        self.controls_send(ControlCommand::Read(label, ui));
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
    pub(crate) fn ui_facts(&self) -> UiFacts {
        let stale = self.brain.as_ref().is_some_and(|c| c.deploy.stale_generation);
        UiFacts {
            dnd: self.notif_muted(),
            night_k: self.screen.temperature_k.unwrap_or(crate::screen::NEUTRAL_K),
            updates: crate::i18n::tr(if stale { tr("Restart to finish updating") } else { tr("Up to date") }).to_owned(),
            has_bt: self.stats_page_for(PageKind::Bt).is_some(),
            has_battery: self.stats_page_for(PageKind::Battery).is_some(),
            apps: self.colours_apps(),
            notif_apps: self.notif_apps(),
        }
    }

    /// The grid's apps for the Colours list, most used first (a webapp only
    /// once it is installed).
    fn colours_apps(&self) -> Vec<(String, String, usize)> {
        let picked = crate::palette::choice();
        let mut apps: Vec<(u32, String, String, usize)> = self
            .entries
            .iter()
            .enumerate()
            // Real apps only: a .desktop of its own (not a home folder, an
            // icon asset or the shell's own tiles), with a name.
            .filter(|(i, e)| self.kinds.get(*i) == Some(&crate::apps::EntryKind::App) && e.path.is_some() && !e.name.trim().is_empty())
            .filter(|(_, e)| !e.id.starts_with("golem-") && !e.id.starts_with("waverunner"))
            .filter(|(_, e)| !e.id.starts_with("webapp-") || self.managed_webapps.contains(&e.id["webapp-".len()..]))
            // Only an app an exception can reach (`app_caps`) — or one that
            // has an exception already, so it can be taken back.
            .filter(|(_, e)| crate::app_caps::of(&e.id) != Some(crate::app_caps::Caps::None) || picked.skip.contains(&e.id) || picked.app_look.contains_key(&e.id))
            .map(|(i, e)| (self.usage.count(&e.id), e.id.clone(), e.name.clone(), i))
            .collect();
        apps.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase())));
        apps.dedup_by(|a, b| a.1 == b.1);
        apps.into_iter().map(|(_, id, name, i)| (id, name, i)).collect()
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
        // (The first tile with Dynamic in use puts no picture up: nothing is
        // changing, so the dock is not told to watch for it.)
        if key == "wallpaper" && !crate::wall_follow::is_idle_tile(&value) {
            self.wallpaper_changing(&value);
        }
        // The account's buttons take what is still being typed in a field
        // (Change the password pressed with the caret in the last field).
        if key.starts_with("account-") && key != "account-fields" {
            crate::controls::typing_commit();
        }
        if !crate::controls::ui_key(key) {
            let ui = self.ui_facts();
            self.controls_send(ControlCommand::Set { label, key, value, ui });
            return;
        }
        match key {
            "night" => {
                if let Ok(k) = value.parse::<u32>() {
                    let k = k.clamp(1000, crate::screen::NEUTRAL_K);
                    self.set_screen_temperature(k);
                    // (A warmth chosen is the night's warmth from then on.)
                    if k != crate::screen::NEUTRAL_K {
                        crate::display::GolemSettings::change(|s| {
                            s.night_k = Some(k);
                            s.store();
                        });
                    }
                }
            }
            "night-auto" => {
                crate::display::GolemSettings::change(|s| {
                    s.night_auto = Some(value == "on");
                    s.store();
                });
                self.night_auto_changed();
            }
            "notif-hours" => {
                self.settings.notif_quiet_hours = value == "on";
                self.settings.save();
            }
            "notif-hours-from" | "notif-hours-to" => {
                if let Some(h) = value.parse::<u8>().ok().filter(|h| *h < 24) {
                    if key == "notif-hours-from" {
                        self.settings.notif_quiet_from = Some(h);
                    } else {
                        self.settings.notif_quiet_to = Some(h);
                    }
                    self.settings.save();
                }
            }
            // A click on a text field: it takes the keys (`colours_type`).
            "colors-edit-hex" | "colors-edit-name" | "colors-apps-find" | "printers-add" => {
                let kept = crate::controls::typing_commit();
                if let Ok(mut t) = crate::controls::TYPING.lock() {
                    *t = crate::controls::field_start(key);
                }
                if kept {
                    crate::palette::set_preview(crate::controls::editor_draft());
                    self.colors_apply(false);
                }
            }
            // The editor's two fields on one row: the value names the field.
            // The account's three fields (2026-11-06): the same way.
            "account-fields" => {
                crate::controls::typing_commit();
                if let Ok(mut t) = crate::controls::TYPING.lock() {
                    *t = crate::controls::field_start(&value);
                }
            }
            "colors-edit-text" => {
                let kept = crate::controls::typing_commit();
                if let Ok(mut t) = crate::controls::TYPING.lock() {
                    *t = crate::controls::field_start(&value);
                }
                if kept {
                    crate::palette::set_preview(crate::controls::editor_draft());
                    self.colors_apply(false);
                }
            }
            // The "+" tile after the palettes opens the editor.
            "colors-palette" if value == "new" => {
                crate::controls::editor_open();
                crate::palette::set_preview(crate::controls::editor_draft());
                self.colors_apply(false);
            }
            "colors-edit" | "colors-edit-which" | "colors-edit-role" | "colors-edit-colour" | "colors-edit-hue" => {
                // (What a field held, kept first: the click then has its say.)
                crate::controls::typing_commit();
                let done = crate::controls::editor_pick(key, &value);
                // Worn while it is made; Save, Cancel or the Bin takes the preview off.
                crate::palette::set_preview(crate::controls::editor_draft());
                match done {
                    crate::controls::EditorDone::Saved(id) => self.colors_pick("colors-palette", &id),
                    // To the Bin (restored from there, it is back in the grid):
                    // the one in use gives way to Golem's as it comes.
                    crate::controls::EditorDone::Binned { name, in_use } => {
                        let body = crate::i18n::fill(tr("Moved “{name}” to the Bin"), &[("name", &name)]);
                        let _ = std::thread::Builder::new()
                            .name("palette-binned".into())
                            .spawn(move || crate::desktop_send_notify_open(tr("Colours"), "preferences-desktop-color", &body, None));
                        if in_use {
                            self.colors_pick("colors-palette", crate::palette::DYNAMIC_ID);
                        } else {
                            self.colors_apply(false);
                        }
                    }
                    crate::controls::EditorDone::Nothing => self.colors_apply(false),
                }
            }
            // THE COLOURS BOX'S PLACE (the Colours model, 2026-10-20): its
            // tab, its section, the Apps page and the app chosen there.
            "colors-tab" => {
                crate::controls::typing_commit();
                crate::controls::COLOURS_TAB.store(u8::from(value == "backgrounds"), std::sync::atomic::Ordering::Relaxed);
                crate::controls::APPS_LIST.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            "colors-section" => {
                if let Ok(i) = value.parse::<u8>() {
                    crate::controls::COLOURS_SECTION.store(i, std::sync::atomic::Ordering::Relaxed);
                }
            }
            // The Apps page: "list" in, "back" out (the old "+" beside Apps too).
            "colors-apps-list" | "colors-apps-look" if value == "list" || key == "colors-apps-list" => {
                crate::controls::APPS_LIST.store(value == "list", std::sync::atomic::Ordering::Relaxed);
                if let Ok(mut a) = crate::controls::APP_CHOSEN.lock() {
                    *a = None;
                }
                crate::controls::field_typed("colors-apps-find", "", false);
                if let Ok(mut t) = crate::controls::TYPING.lock() {
                    *t = None;
                }
            }
            "colors-app" => {
                if let Ok(mut a) = crate::controls::APP_CHOSEN.lock() {
                    *a = Some(value.clone());
                }
            }
            // The chosen app's Colours switch: on, the palette; off, its own.
            "colors-app-colours" => {
                if let Some(id) = crate::controls::APP_CHOSEN.lock().ok().and_then(|a| a.clone()) {
                    let own = crate::palette::choice().skip.contains(&id);
                    if own == (value == "on") {
                        self.colors_pick("colors-skip", &id);
                    }
                }
            }
            k if k.starts_with("colors-") => self.colors_pick(key, &value),
            k if k.starts_with("launcher-") => self.launcher_pick(key, &value),
            k if k.starts_with("monitors-") => self.monitors_pick(key, &value),
            "dnd" => {
                if self.notif_muted() != (value == "on") {
                    self.toggle_notif_mute();
                }
            }
            // THE CONTROLS GAP AUDIT'S PAGES (2026-11-06), the keys that are
            // this thread's: the idle times (the gear's own machinery), the
            // text size (the shell's zooms), less motion (the shell's flag),
            // the reader's install, an app added to the startup folder, a
            // quiet app.
            "idle-lock" | "idle-screen" | "idle-sleep" => {
                if let Ok(secs) = value.parse::<u32>() {
                    match key {
                        "idle-lock" => self.settings.lock_secs = Some(secs),
                        "idle-screen" => self.settings.screen_off_secs = Some(secs),
                        _ => self.settings.sleep_secs = Some(secs),
                    }
                    self.settings.save();
                    self.apply_idle();
                }
            }
            "idle-lid" => {
                let nothing = value == "on";
                self.settings.lid_nothing = nothing;
                self.settings.save();
                self.hold_lid(nothing);
            }
            "access-text" => {
                if let Ok(f) = value.parse::<f64>().map(|f| f.clamp(0.7, 2.0)) {
                    for what in ["bar", "menu", "files", "fast", "desktop"] {
                        self.zoom(what, &format!("{f:.2}"));
                    }
                    crate::display::GolemSettings::change(|s| {
                        s.text_scale = Some(f);
                        s.store();
                    });
                    let _ = std::thread::Builder::new().name("access-text".into()).spawn(move || {
                        crate::controls::dconf_write("/org/gnome/desktop/interface/text-scaling-factor", &format!("{f:.2}"));
                    });
                }
            }
            "access-motion" => {
                let on = value == "on";
                crate::animation::set_reduce_motion(on || self.config.accessibility.reduce_motion);
                crate::display::GolemSettings::change(|s| {
                    s.reduce_motion = Some(on);
                    s.store_input();
                });
                if !on {
                    crate::hypr::reload_config();
                }
                let _ = std::thread::Builder::new().name("access-motion".into()).spawn(move || {
                    crate::controls::dconf_write("/org/gnome/desktop/interface/enable-animations", if on { "false" } else { "true" });
                });
            }
            "access-reader-install" => {
                crate::access_page::INSTALLING.store(true, std::sync::atomic::Ordering::Relaxed);
                self.start_managed_install(crate::access_page::READER_ATTR, vec![crate::access_page::READER_ID.to_owned()]);
            }
            "startup-add" => {
                if let Some(e) = self.entries.iter().find(|e| e.id == value) {
                    crate::startup_page::add(&e.id, &e.name, &e.exec, e.icon.as_deref(), e.needs_terminal);
                    let exec = e.exec.clone();
                    let terminal = e.needs_terminal;
                    if let Err(err) = crate::launch::launch(&exec, terminal, &self.config.launch.terminal) {
                        info!("startup apps: {value} added, not started now: {err:#}");
                    }
                }
            }
            "notif-app" => {
                let list = &mut self.settings.notif_quiet;
                match list.iter().position(|k| *k == value) {
                    Some(i) => {
                        list.remove(i);
                    }
                    None => list.push(value.clone()),
                }
                self.settings.save();
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
            // A scanner and nothing to scan with (the Printers page): Simple
            // Scan, the way any app is installed.
            "printers-scan-app" => {
                self.start_managed_install(crate::scanners::APP_ATTR, vec![crate::scanners::APP_ID.to_owned()]);
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
        // (What the Transparency is as Colours opens: said as it closes if it rose.)
        if crate::controls::box_of(label) == "Colours" {
            crate::palette::clear_remember();
        }
        let rect = self.quick_rect();
        self.quick_ensure(rect);
        let from = Rect::new(from.x - rect.x, from.y - rect.y, from.w, from.h);
        if self.quick.open_solo(label, from) {
            self.settings_panel_mut().note_use(label);
            info!("quick controls: opening {label}");
            // The colours are set looking at the dock: a hidden one comes up
            // for as long as the box is open (Max, 2026-10-20). So is the
            // dock itself, in the Launcher box (Max, 2026-10-21: "so I see
            // it when I'm controlling it").
            if calls_dock(label) && self.call_dock() {
                DOCK_CALLED.store(true, std::sync::atomic::Ordering::Relaxed);
            }
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
            let w = self.quick_beside_dock((sw * QUICK_WHOLE).max(200.0), g.card.x + g.card.w / 2.0);
            // As tall as its wall and no taller, in the middle of that room
            // (Max, 2026-10-14: "hay mucho real estate que estamos
            // desperdiciando en la caja, sobre todo abajo" — it ran to the
            // dock whatever was in it). Not laid out yet: as it last was.
            let wall = self.quick.open_height().filter(|h| *h > WALL_TOP * s + 1.0).or(self.quick.wall_h);
            let h = wall.map_or(room, |h| (h + WALL_FOOT * s).min(room)).max(QUICK_MIN_H * s);
            let cx = g.card.x + g.card.w / 2.0;
            return Rect::new(cx - w / 2.0, top + (room - h) / 2.0, w, h);
        }
        // Colours with its cards up: as wide as the wallpapers' box (the
        // Colours model, 2026-10-20: seven cards across; it was a 680 box
        // of four by three), as tall as its content and never taller than
        // the screen less a margin — past that its cards go four rows to
        // three (`cards_overflow`).
        // (Always, cards or none: its three sliders side by side need the
        // width, and switching tabs must not change the box's — rig, 2026-10-20.)
        let wide = self.quick.open_is("Colours");
        // THE COLOURS BOX stands where the wallpapers' does: under the bar and
        // clear of the dock, a little narrower (Max, 2026-10-20: "i want to
        // see the dock when im setting up") — past that room its cards go
        // four rows to three (`cards_overflow`).
        if wide {
            let (top, room) = self.quick_wall_room();
            let w = self.quick_beside_dock((sw * QUICK_COLOURS).max(200.0), g.card.x + g.card.w / 2.0);
            let h = (self.quick.open_height().unwrap_or(0.0) + QUICK_FOOT * s).max(QUICK_MIN_H * s).min(room);
            let cx = g.card.x + g.card.w / 2.0;
            return Rect::new(cx - w / 2.0, top + (room - h) / 2.0, w, h);
        }
        let (w, h) = {
            (
                if wide { (sw * QUICK_WHOLE).max(200.0) } else { (QUICK_W * s).min(sw - 40.0 * s).max(200.0) },
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
        let screen_h = self.dock_screen_h().unwrap_or(sh).min(sh);
        // The dock's band comes off the bottom only where the dock is: beside
        // a side dock the box keeps the screen's height, and its width keeps
        // clear of the dock instead (`quick_beside_dock`).
        let dock_h = if self.dock_side() == crate::launcher_prefs::Side::Bottom { self.scaled_extents().0 } else { 0.0 };
        let top = self.options_pill_h() + 16.0 * s;
        (top, (screen_h - dock_h - QUICK_OFF_DOCK * s - top).max(QUICK_MIN_H * s))
    }

    /// A wide box's width `w` about `cx`, kept clear of a side dock (the
    /// same air as above a bottom one), still centred: on a small screen
    /// with big icons it would touch the dock.
    fn quick_beside_dock(&self, w: f32, cx: f32) -> f32 {
        let s = self.options_scale() * self.fast_zoom();
        let sw = self.buffer_size.0 as f32;
        let band = self.scaled_extents().0 + QUICK_OFF_DOCK * s;
        let half = match self.dock_side() {
            crate::launcher_prefs::Side::Left => cx - band,
            crate::launcher_prefs::Side::Right => sw - band - cx,
            crate::launcher_prefs::Side::Bottom => return w,
        };
        w.min(2.0 * half).max(200.0)
    }

    fn quick_ensure(&mut self, rect: Rect) {
        // (The wall is laid out for the room it MAY take; the box then hugs
        // what it took.)
        // (Colours' cards: the box may grow to the screen less a margin,
        // `quick_rect`'s cap, and four rows give way to three past that.)
        self.quick.tile_room_h = if self.quick.open_is("Wallpaper") || self.quick.open_is("Colours") {
            self.quick_wall_room().1
        } else {
            0.0
        };
        self.quick.wall_tile = self.wallpaper_tile_w();
        if let Some(h) = self.quick.open_height().filter(|_| self.quick.open_is("Wallpaper") && self.quick.open_has_tiles()) {
            self.quick.wall_h = Some(h);
        }
        let scale = self.options_scale() * self.fast_zoom();
        let icon_scale = self.icon_scale();
        if let Some(r) = self.renderer.as_mut() {
            let mut measure = |t: &str, px: f32| r.measure_text(t, px, TEXT_FONT);
            // (Laid out for the box's WIDTH only: its height follows the
            // setting, and a new height must not lay the field out again.)
            self.quick.ensure((rect.w, QUICK_MIN_H * scale), rect.w, scale, icon_scale, &mut measure);
        }
        self.quick.field = rect;
        self.quick.grid_field = rect;
    }

    /// The quick box of `label` was left.
    pub(crate) fn quick_left(&mut self, label: &'static str) {
        // The dock the Colours or Launcher box called goes back to hiding (as
        // it does when the pointer leaves it: unless the pointer is on it) —
        // so does one parked in view when Stay away was turned on in it.
        if calls_dock(label) {
            let called = DOCK_CALLED.swap(false, std::sync::atomic::Ordering::Relaxed);
            if called || self.settings.launcher_away {
                self.schedule_autohide();
            }
        }
        // A Transparency rose in this visit: what it costs is said once, as
        // the box closes (Max, 2026-10-27).
        if (label == "Wallpaper" || label == "Colours") && crate::palette::clear_rose() {
            let (title, body) = (tr("Colours").to_owned(), tr("Transparency can make things feel slower. Solid is the lightest.").to_owned());
            let _ = std::thread::Builder::new()
                .name("clear-notice".into())
                .spawn(move || crate::desktop_send_notify_open(&title, "preferences-desktop-color", &body, None));
        }
        // (The wallpapers are on the Colours box's Wallpaper tab.)
        if label == "Wallpaper" || label == "Colours" {
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
        self.quick.keep_turn = self.monitors_turn.is_some();
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
            self.quick_leave();
        }
        self.schedule_frame();
    }

    /// A slider moved where it cannot change anything (`palette::refusal`):
    /// said in its words' place for a moment, nothing saved, the knob back at
    /// its stop.
    pub(crate) fn colours_note(&mut self, key: &str, words: &'static str) {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(NOTE_MS);
        for p in [&mut self.panel, &mut self.quick] {
            p.note = Some((key.to_owned(), words, until));
        }
        // (A frame as it ends, for the words to come back.)
        let timer = calloop::timer::Timer::from_duration(std::time::Duration::from_millis(NOTE_MS + 30));
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            app.schedule_frame();
            calloop::timer::TimeoutAction::Drop
        });
        let ui = self.ui_facts();
        self.controls_send(ControlCommand::Read("Colours", ui));
        self.schedule_frame();
    }

    /// Escape, a click on the empty space or outside, Super+H again: the
    /// box goes — unless a palette being made has changes not saved, which
    /// asks once (its foot says so; `controls::editor_may_leave`).
    pub(crate) fn quick_leave(&mut self) {
        if crate::controls::editor_may_leave() {
            self.close_fast_launch();
        } else {
            let ui = self.ui_facts();
            self.controls_send(ControlCommand::Read("Colours", ui));
            self.schedule_frame();
        }
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
            Keysym::Return | Keysym::KP_Enter => panel.key_pick(),
            // (On the grid a space is typed into the search unless the keys
            // are walking the tiles.)
            Keysym::space => (panel.open.is_some() || panel.grid_keyed()) && panel.key_pick(),
            _ => false,
        };
        if used {
            match panel.take_action() {
                Some(PanelAction::Scale(scale)) => self.set_display_scale(scale),
                Some(PanelAction::Mode { w, h, hz }) => self.set_display_mode(w, h, hz),
                Some(PanelAction::Keep) => self.keep_display(),
                Some(PanelAction::Revert) => self.revert_display(),
                Some(PanelAction::Pick { group, index }) => self.control_pick(group, index, quick),
                Some(PanelAction::Target(i)) => self.display_pick_screen(usize::from(i)),
                Some(PanelAction::Boxed(label)) => self.settings_box(label),
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
            self.quick_leave();
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
                PanelAction::Target(i) => self.display_pick_screen(usize::from(i)),
                PanelAction::Boxed(label) => self.settings_box(label),
            },
            // On no choice: the box's own empty space closes it, as its
            // last line says (Max, 2026-10-14: "the box says click empty
            // space closes it and it does not" — only a click OUTSIDE the
            // box did).
            _ => {
                self.quick_leave();
                return;
            }
        }
        self.schedule_frame();
    }

    /// Open the control panel on the setting `label`, from wherever the
    /// card is (the `display show …` verb: a key binding's way straight to
    /// a setting, and the pointer-free way to look at one).
    pub(crate) fn show_control(&mut self, label: &'static str) {
        // (Colours has its box mid-screen: `Panel::boxed`.)
        if crate::controls::box_of(label) == "Colours" {
            self.settings_box(label);
            return;
        }
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
        if let Some(label) = self.panel.take_boxed() {
            self.settings_box(label);
        }
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
        panel_in(960.0, 560.0)
    }

    /// …over a field `w` × `h` (the grid's field the same, at the origin).
    fn panel_in(w: f32, h: f32) -> Panel {
        let mut p = Panel { items: &SETTINGS, ..Panel::default() };
        let mut est = |t: &str, px: f32| crate::options::est_text_w(t, px);
        p.field = Rect::new(0.0, 0.0, w, h);
        p.grid_field = p.field;
        p.ensure((w, h), w, 1.0, 1.0, &mut est);
        p
    }

    /// A tile's cell, from its home (on its page, as if it were the first).
    fn cell(p: &Panel, q: &Pill) -> Rect {
        let g = &p.grid;
        Rect::new(q.home.0 - g.cell_w / 2.0, q.home.1 - g.cell_h / 2.0, g.cell_w, g.cell_h)
    }

    /// Every shown tile in its own slot, inside the field, over the Search
    /// pill's room.
    fn assert_clean(p: &Panel, what: &str) {
        let (w, h) = (p.key.0, p.key.1);
        let shown: Vec<&Pill> = p.pills.iter().filter(|q| q.shown).collect();
        let _ = h;
        let top = content::GRID_TOP_GAP + content::SECTION_TITLE_H;
        for (i, a) in shown.iter().enumerate() {
            let c = cell(p, a);
            assert!(
                c.x >= -0.5 && c.x + c.w <= w + 0.5 && c.y >= top - 0.5 && c.y + c.h <= p.grid.total_h - GRID_BOTTOM + 0.5,
                "{what}: {} leaves the grid at {:?}",
                a.label,
                a.home
            );
            for b in shown.iter().skip(i + 1) {
                assert_ne!(a.slot, b.slot, "{what}: {} and {} share a slot", a.label, b.label);
            }
        }
        let slots: Vec<usize> = shown.iter().map(|q| q.slot).collect();
        assert!((0..shown.len()).all(|k| slots.contains(&k)), "{what}: the slots are not 0..{}: {slots:?}", shown.len());
    }

    /// The tiles in the order the grid shows them (its slots).
    fn shown_order(p: &Panel) -> Vec<&'static str> {
        let mut v: Vec<&Pill> = p.pills.iter().filter(|q| q.shown).collect();
        v.sort_by_key(|q| q.slot);
        v.iter().map(|q| q.label).collect()
    }

    #[test]
    fn every_setting_has_a_symbol_of_its_own() {
        for (label, ..) in SETTINGS {
            let name = symbol_of(label);
            assert!(crate::app_actions::has_symbol(name), "{label}: no symbol {name:?}");
            assert_ne!(name, "package", "{label} wears the fallback");
        }
        assert_eq!(symbol_of("Gaming"), "package", "a module wears the package");
    }

    #[test]
    fn the_grid_is_the_apps_grids_cells_in_sections() {
        let p = panel();
        assert_eq!(p.pills.len(), 34);
        assert_clean(&p, "at rest");
        let g = &p.grid;
        // A 960 card: as many across as the apps grid puts there; every
        // section a titled strip, as many rows as its tiles need.
        assert_eq!((g.cell_w, g.cell_h, g.icon), (content::GRID_CELL_W, content::GRID_CELL_H, content::GRID_ICON));
        assert_eq!(g.cols, ((960.0 - 2.0 * content::GRID_PAD_X) / content::GRID_CELL_W) as usize);
        assert_eq!(g.sections.len(), SECTIONS.len());
        let counts: Vec<usize> = g.sections.iter().map(|s| s.n).collect();
        assert_eq!(counts, [4, 8, 4, 3, 4, 11]);
        assert_eq!(g.sections.iter().map(|s| s.title).collect::<Vec<_>>(), SECTIONS.iter().map(|t| Some(*t)).collect::<Vec<_>>());
        // Up under the dock: the first row where the apps grid's stands;
        // then each strip after the apps' air (dot room, gap, title line).
        let first = &g.sections[0];
        assert_eq!(first.title_y, content::GRID_TOP_GAP);
        assert_eq!(first.vp.y, content::GRID_TOP_GAP + content::SECTION_TITLE_H);
        let second = &g.sections[1];
        assert_eq!(second.title_y, first.vp.y + first.vp.h + content::DOT_ROOM + content::SECTION_GAP);
        assert_eq!(g.sections[5].vp.h, 2.0 * content::GRID_CELL_H, "eleven tiles at eight across: two rows");
        assert_eq!(g.total_h, g.sections[5].vp.y + g.sections[5].vp.h + content::DOT_ROOM + GRID_BOTTOM);
        assert!((first.vp.x + first.vp.w / 2.0 - 480.0).abs() < 0.5, "the strips are centred");
        // The fixed order: Style's four first, then the screen's.
        assert_eq!(&shown_order(&p)[..5], &["Colours", "Wallpaper", "Tiles", "Launcher", "Resolution"]);
        // A wider card: more columns, fewer rows, the same places.
        let wide = panel_in(1400.0, 560.0);
        assert!(wide.grid.cols > g.cols);
        assert_eq!(wide.grid.sections[5].vp.h, content::GRID_CELL_H, "eleven tiles at thirteen across: one row");
        assert_eq!(shown_order(&wide), shown_order(&p));
        assert!(wide.grid.total_h < g.total_h);
        // Every name fits its cell on one line, cut if it must; the long
        // ones have their two lines too.
        let room = g.cell_w - 12.0;
        assert!(p.pills.iter().all(|q| crate::options::est_text_w(&q.one, content::LABEL_FONT_PX) <= room + 0.5));
        let long = p.pills.iter().find(|q| q.label == "Mouse and touchpad").unwrap();
        assert!(long.one.ends_with('…') && long.two.is_some(), "{:?} / {:?}", long.one, long.two);
        // A section's last row has room for a second line; a row with a
        // row under it has not.
        assert!(g.free_under(0) && !g.free_under(g.sections[5].first));
        // The card is asked for the whole grid's height, searching or not.
        assert_eq!(p.wanted_h(960.0, 1.0), g.total_h);
    }

    #[test]
    fn a_short_field_scrolls_under_the_wheel_and_the_keys_bring_their_tile_in() {
        // A field too short for every section (the screen over the card
        // let it grow no further).
        let mut p = panel_in(960.0, 400.0);
        assert!(p.grid.total_h > 400.0 && p.max_scroll() > 0.0, "{}", p.grid.total_h);
        assert_clean(&p, "short");
        // A tile below the fold is neither drawn nor under the pointer.
        let last = p.pills.iter().position(|q| q.slot == 33).unwrap();
        let home = p.pills[last].home;
        assert!(p.pill_at(home).is_none(), "below the fold");
        // The wheel scrolls, eased; never past the end.
        p.wheel(30.0);
        assert!((p.scroll_to - 90.0).abs() < 0.01);
        for _ in 0..90 {
            p.step(1.0 / 60.0, None);
        }
        assert_eq!(p.scroll, p.scroll_to);
        p.wheel(10_000.0);
        assert_eq!(p.scroll_to, p.max_scroll());
        p.wheel(-20_000.0);
        assert_eq!(p.scroll_to, 0.0);
        // The keys: the first arrow lands on the first tile; Right walks on
        // through the sections, held at the last; the field follows.
        assert!(p.key_move(1, 0));
        assert_eq!(p.sel.map(|i| p.pills[i].slot), Some(0));
        assert!(p.keyed && p.grid_keyed());
        for _ in 0..40 {
            p.key_move(1, 0);
        }
        assert_eq!(p.sel, Some(last));
        assert_eq!(p.scroll_to, p.max_scroll());
        for _ in 0..90 {
            p.step(1.0 / 60.0, None);
        }
        let shown_at = (home.0, home.1 - p.scroll);
        assert_eq!(p.pill_at(shown_at), Some(last), "brought into view");
        // Up: the nearest tile of the row above — the row above the last
        // section's second row is its first.
        assert!(p.key_move(0, -1));
        let up = p.sel.unwrap();
        assert_eq!(p.grid.section_of(p.pills[up].slot).map(|s| s.first), p.grid.section_of(33).map(|s| s.first));
        assert!(p.pills[up].home.1 < home.1);
        // Enter opens the tile the keys are on.
        let expect = p.pills[up].label;
        assert!(p.key_pick());
        assert_eq!(p.open.as_ref().map(|o| p.pills[o.pill].label), Some(expect));
    }

    #[test]
    fn a_search_shows_the_matches_only_in_their_sections_the_best_lit() {
        let mut p = panel();
        p.set_query("night");
        for _ in 0..90 {
            p.step(1.0 / 60.0, None);
        }
        // Colours (a keyword) in Style, Night light (its name) in the
        // screen's: two strips, the rest gone.
        assert_eq!(shown_order(&p), ["Colours", "Night light"]);
        assert_eq!(p.grid.sections.iter().map(|s| s.title).collect::<Vec<_>>(), [Some("Style"), Some("Screen and sound")]);
        for q in &p.pills {
            if q.shown {
                assert!(q.op > 0.95, "{} stays", q.label);
            } else {
                assert!(q.op < 0.05, "{} should fade out", q.label);
            }
        }
        assert_clean(&p, "searched");
        // The best match is lit, softly, for Enter: the name over the
        // keyword, whatever stands first.
        assert_eq!(p.sel.map(|i| p.pills[i].label), Some("Night light"));
        assert!(!p.keyed);
        assert!(p.open_best());
        assert_eq!(p.open.as_ref().map(|o| p.pills[o.pill].label), Some("Night light"));
        p.open = None;
        // "sc" matches Scale by name, Resolution by a keyword: Scale.
        p.set_query("sc");
        assert_eq!(p.best_match().map(|i| p.pills[i].label), Some("Scale"));
        p.set_query("");
        for _ in 0..90 {
            p.step(1.0 / 60.0, None);
        }
        assert!(p.pills.iter().all(|q| q.shown && q.op > 0.95), "clearing brings every tile back");
        assert_eq!(p.sel, None);
    }

    #[test]
    fn a_line_is_cut_to_its_room() {
        let mut est = |t: &str| crate::options::est_text_w(t, 12.0);
        assert_eq!(fit_line("Scale", 200.0, &mut est), "Scale");
        let cut = fit_line("Mouse and touchpad", 60.0, &mut est);
        assert!(cut.ends_with('…') && cut.len() < "Mouse and touchpad".len(), "{cut:?}");
        assert!(est(&cut) <= 60.0);
        assert_eq!(fit_line("Mouse", 1.0, &mut est), "…");
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
    fn a_reset_puts_every_tile_home_with_no_search() {
        let mut p = panel();
        let first: Vec<(f32, f32)> = p.pills.iter().map(|q| q.home).collect();
        p.set_query("wifi");
        for _ in 0..30 {
            p.step(1.0 / 60.0, None);
        }
        p.reset();
        for (q, home) in p.pills.iter().zip(&first) {
            assert!((q.home.0 - home.0).abs() < 0.5 && (q.home.1 - home.1).abs() < 0.5, "{} back home", q.label);
            assert!(q.shown && q.op == 1.0 && q.pos == q.home);
        }
        assert!(p.query.is_empty() && p.sel.is_none());
    }

    #[test]
    fn the_apps_search_finds_controls_best_first() {
        let p = Panel { items: &SETTINGS, ..Panel::default() };
        let names = |q: &str| -> Vec<&'static str> { p.matching_ranked(q).into_iter().map(|(_, l)| l).collect() };
        let audio = names("audio");
        for want in ["Sound output", "Volume", "Microphone"] {
            assert!(audio.contains(&want), "{want} in {audio:?}");
        }
        assert_eq!(names("res").first(), Some(&"Resolution"), "a name prefix first");
        assert_eq!(p.matching_ranked("res").first().map(|&(rank, _)| rank), Some(0), "ranked as a name prefix");
        assert!(names("  ").is_empty());
    }

    #[test]
    fn the_modules_field_lays_out_clean_and_searches_its_own() {
        let mut p = Panel { items: crate::modules::items(), modules: true, ..Panel::default() };
        let mut est = |t: &str, px: f32| crate::options::est_text_w(t, px);
        p.ensure((960.0, 560.0), 960.0, 1.0, 1.0, &mut est);
        assert!(p.is_modules());
        assert_eq!(p.pills.len(), crate::modules::items().len());
        assert_clean(&p, "the modules field");
        // One strip, no word over it.
        assert_eq!(p.grid.sections.len(), 1);
        assert_eq!(p.grid.sections[0].title, None);
        assert_eq!(p.matching_ranked("steam").first(), Some(&(2, "Gaming")), "a keyword finds its module");
        assert!(p.matching_ranked("resolution").is_empty(), "no settings in the modules");
    }

    #[test]
    fn a_use_counts_for_the_quick_controls_and_moves_nothing() {
        let mut p = panel();
        let before = shown_order(&p);
        let now = now_secs();
        let last = p.pills.len() - 1;
        let label = p.pills[last].label;
        let count = p.use_count(label);
        p.record_use(last, now);
        p.record_use(last, now);
        p.record_use(last, now);
        assert!(p.use_count(label) > count, "{} counts", p.use_count(label));
        assert_eq!(shown_order(&p), before, "every setting keeps its place");
        assert_clean(&p, "after uses");
    }

    #[test]
    fn old_uses_fade_and_the_arrangement_is_kept() {
        let st = Standing { layer: 1, score: 4.0, at: 0 };
        let two_weeks = (USE_HALF_LIFE_DAYS * 86_400.0) as u64;
        assert!((st.score_at(two_weeks) - 2.0).abs() < 1e-9, "a use counts half after the half-life");

        let mut p = panel();
        let last = p.pills.len() - 1;
        let now = now_secs();
        p.record_use(last, now);
        p.record_use(last, now);
        let json = serde_json::to_string(&p.usage).unwrap();
        let mut again = Panel { items: &SETTINGS, usage: serde_json::from_str(&json).unwrap(), ..Panel::default() };
        let mut est = |t: &str, px: f32| crate::options::est_text_w(t, px);
        again.ensure((960.0, 560.0), 960.0, 1.0, 1.0, &mut est);
        assert_eq!(shown_order(&again), shown_order(&p), "the saved order comes back");
    }

    #[test]
    fn a_click_opens_the_pill_and_escape_folds_it_back() {
        let mut p = panel();
        p.step(0.0, None);
        // (Not Colours, the first tile: it opens in a box of its own.)
        let i = p.pills.iter().position(|q| q.label == "Resolution").unwrap();
        let target = (p.grid_field.x + p.pills[i].pos.0, p.grid_field.y + p.pills[i].pos.1);
        assert!(p.click(target), "a click on a pill opens it");
        assert_eq!(p.open.as_ref().map(|o| o.pill), Some(i));
        for _ in 0..30 {
            p.step(1.0 / 60.0, None);
        }
        assert!(p.open.as_ref().is_some_and(|o| o.k > 0.95));
        // The wheel over an open setting moves no tile.
        p.wheel(30.0);
        assert!(p.pills.iter().all(|q| q.pos == q.home));
        assert!(p.escape());
        for _ in 0..60 {
            p.step(1.0 / 60.0, None);
        }
        assert!(p.open.is_none(), "folded back and gone");
    }

    #[test]
    fn with_two_screens_the_scale_page_names_them_the_one_it_sets_lit() {
        let mut p = display_open("Scale", 1.0);
        if let Some(v) = p.display.as_mut() {
            v.screens = vec![("eDP-1".to_owned(), "Built-in screen".to_owned()), ("desc:TV".to_owned(), "TV MONITOR".to_owned())];
            v.target = "desc:TV".to_owned();
        }
        let c = p.open_content("Scale").unwrap();
        let names: Vec<&Chip> = c.chips.iter().filter(|ch| matches!(ch.action, PanelAction::Target(_))).collect();
        assert_eq!(names.len(), 2);
        assert!(!names[0].on && names[1].on, "the TV is the one set");
        assert_eq!(names[1].action, PanelAction::Target(1));
        let scales: Vec<&Chip> = c.chips.iter().filter(|ch| matches!(ch.action, PanelAction::Scale(_))).collect();
        assert!(scales.iter().all(|ch| ch.rect.y > names[0].rect.y + names[0].rect.h), "the screens come first");
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
            screens: Vec::new(),
            target: String::new(),
        }));
        for _ in 0..40 {
            p.step(1.0 / 60.0, None);
        }
        p
    }

    #[test]
    fn colours_asked_for_on_the_card_opens_in_its_own_box() {
        // (The pass of 2026-10-27: on the card its sliders fell under the
        // screen's edge.)
        let mut p = panel();
        p.field = Rect::new(40.0, 90.0, 960.0, 560.0);
        p.step(0.0, None);
        p.set_query("colours");
        assert!(p.open_best());
        assert_eq!(p.take_boxed(), Some("Colours"), "the box is asked for");
        assert!(p.open.is_none(), "nothing opens on the card");
        assert_eq!(p.take_boxed(), None, "asked once");
        p.set_query("scale");
        assert!(p.open_best());
        assert_eq!(p.take_boxed(), None, "another setting opens on the card");
        assert!(p.open.is_some());
    }

    #[test]
    fn the_wallpaper_opens_the_colours_box() {
        let mut p = panel();
        p.field = Rect::new(40.0, 90.0, 960.0, 560.0);
        p.step(0.0, None);
        p.open_named("Wallpaper");
        assert_eq!(p.open.as_ref().map(|o| p.pills[o.pill].label), Some("Colours"), "Wallpaper's pill opens the Colours box");
        assert_eq!(crate::controls::box_of("Wallpaper"), "Colours");
        assert_eq!(crate::controls::box_of("Scale"), "Scale");
    }

    /// The wallpaper box open on `n` pictures, the first of them (when
    /// `caption`) the palette's card with its words.
    fn wall_open(n: usize, caption: bool) -> Panel {
        let mut p = panel();
        p.field = Rect::new(40.0, 90.0, 960.0, 560.0);
        p.step(0.0, None);
        // (Its own box, as it was laid out: a pill now opens the Colours
        // box's Wallpaper tab instead, `the_wallpaper_opens_the_colours_box`.)
        assert!(p.open_solo("Wallpaper", Rect::new(0.0, 0.0, 10.0, 10.0)));
        for _ in 0..40 {
            p.step(1.0 / 60.0, None);
        }
        let choices = (0..n)
            .map(|i| crate::controls::Choice {
                text: if i == 0 && caption { "Follows your colours".into() } else { format!("p{i}") },
                on: i == 0 && caption,
                value: format!("/pictures/{i}.png"),
                swatch: Vec::new(),
                caption: i == 0 && caption,
                icon: None,
                hint: None,
            })
            .collect();
        let group = crate::controls::Group { choices, tiles: true, width: 170.0, ..plain_group("wallpaper") };
        assert!(p.set_control(ControlView { label: "Wallpaper", subtitle: String::new(), groups: vec![group] }));
        p
    }

    /// A group with nothing set but its key (every flag off).
    fn plain_group(key: &'static str) -> crate::controls::Group {
        crate::controls::Group {
            caption: None,
            key,
            width: 0.0,
            choices: Vec::new(),
            tiles: false,
            slider: false,
            beside: false,
            field: false,
            main: false,
            buttons: false,
            cells: 0,
            row: false,
            split: false,
            mark: None,
            centred: false,
            grid_rows: None,
            spacer: false,
            hug: false,
            labelled: false,
            screens: Vec::new(),
        }
    }

    /// A choice of `text` (its value the same, lowercased), lit or not.
    fn plain_choice(text: &str, on: bool) -> crate::controls::Choice {
        crate::controls::Choice { text: text.to_owned(), on, value: text.to_lowercase(), swatch: Vec::new(), caption: false, icon: None, hint: None }
    }

    /// A slider from 0 to 100 by fives, `level` lit, captioned `name · value`.
    fn slider_group(key: &'static str, name: &str, value: &str, level: u8) -> crate::controls::Group {
        let choices = (0..=20u8).map(|i| crate::controls::Choice { value: (i * 5).to_string(), ..plain_choice("", i * 5 == level) }).collect();
        crate::controls::Group { caption: Some(format!("{name} · {value}")), slider: true, choices, ..plain_group(key) }
    }

    /// On / Off, named.
    fn switch_group(key: &'static str, name: &str, on: bool) -> crate::controls::Group {
        crate::controls::Group { caption: Some(name.to_owned()), choices: vec![plain_choice("On", on), plain_choice("Off", !on)], ..plain_group(key) }
    }

    /// The Colours box open on `groups`, in a field `w` × `h` (the cards'
    /// grid takes its geometry, as it does once the wallpaper box's tile
    /// is known).
    fn colours_open(groups: Vec<crate::controls::Group>, (w, h): (f32, f32)) -> Panel {
        let mut p = Panel { items: &SETTINGS, ..Panel::default() };
        let mut est = |t: &str, px: f32| crate::options::est_text_w(t, px);
        p.ensure((w, h), w, 1.0, 1.0, &mut est);
        p.field = Rect::new(40.0, 90.0, w, h);
        p.step(0.0, None);
        p.open_named("Colours");
        for _ in 0..300 {
            p.step(1.0 / 60.0, None);
        }
        p.wall_tile = 300.0;
        assert!(p.set_control(ControlView { label: "Colours", subtitle: String::new(), groups }));
        p
    }

    #[test]
    fn a_split_track_has_a_gap_two_notches_and_no_stop_fifty() {
        // The spans drawn: round the gap and the two notches; one notch; whole.
        let spans = track_spans(1000.0, true, None);
        assert_eq!(spans.len(), 4, "{spans:?}");
        assert!((spans[1].1 - (0.5 - 9.0 / 1000.0)).abs() < 1e-4 && (spans[2].0 - (0.5 + 9.0 / 1000.0)).abs() < 1e-4, "the gap is 18 wide: {spans:?}");
        assert!((spans[0].1 - 0.224).abs() < 1e-3 && (spans[3].0 - 0.776).abs() < 1e-3, "the notches at 22.5 and 77.5 %: {spans:?}");
        assert_eq!(track_spans(1000.0, false, Some(0.5)).len(), 2);
        assert_eq!(track_spans(1000.0, false, None), vec![(0.0, 1.0)]);

        let lighting = crate::controls::Group { split: true, ..slider_group("colors-lighting", "Lighting", "Dark 10", 20) };
        let mut p = colours_open(vec![lighting], (960.0, 560.0));
        let content = p.open_content("Colours").unwrap();
        let sl = &content.sliders[0];
        assert!(sl.split && sl.count == 21);
        let first = sl.stops.start;
        let mid = sl.track.x + sl.track.w / 2.0;
        // Stop 50 has no place; 45 and 55 meet at the middle.
        assert_eq!(content.chips[first + 10].rect.w, 0.0);
        let (c45, c55) = (&content.chips[first + 9].rect, &content.chips[first + 11].rect);
        assert!((c45.x + c45.w - mid).abs() < 0.01 && (c55.x - mid).abs() < 0.01, "{c45:?} {c55:?} about {mid}");
        // A drag: the pointer on the gap's dark side rests on 45, on its
        // light side on 55 — the knob with it, never between.
        let f = p.field;
        let from = (f.x + c45.x + c45.w / 2.0, f.y + c45.y + c45.h / 2.0);
        let at = |frac: f32| (f.x + sl.track.x + frac * sl.track.w, from.1);
        assert_eq!(p.slide(from, at(0.48)), Some((0, 9)));
        assert_eq!(p.knob_held().map(|k| k.v), Some(0.45));
        assert_eq!(p.slide(from, at(0.52)), Some((0, 11)));
        assert_eq!(p.knob_held().map(|k| k.v), Some(0.55));
        assert_eq!(p.slide(from, at(0.30)), Some((0, 6)));
        // The keys pass over it too.
        p.focus = Some(first + 9);
        assert!(p.key_move(1, 0));
        assert_eq!(p.focus, Some(first + 11));
        assert!(p.key_move(-1, 0));
        assert_eq!(p.focus, Some(first + 9));
    }

    #[test]
    fn a_row_of_three_sliders_shares_one_line_in_three_columns() {
        let groups = vec![
            crate::controls::Group { row: true, split: true, ..slider_group("colors-lighting", "Lighting", "Dark 10", 20) },
            crate::controls::Group { beside: true, ..switch_group("colors-ambient", "Ambient", false) },
            crate::controls::Group { row: true, mark: Some(0.5), ..slider_group("colors-palette", "Palette", "50%", 50) },
            crate::controls::Group { row: true, mark: Some(0.5), ..slider_group("colors-sat", "Saturation", "as made", 50) },
        ];
        let p = colours_open(groups, (960.0, 560.0));
        let c = p.open_content("Colours").unwrap();
        assert_eq!(c.sliders.len(), 3);
        let t: Vec<Rect> = c.sliders.iter().map(|s| s.track).collect();
        assert!(t.iter().all(|r| (r.y - t[0].y).abs() < 0.01), "one line: {t:?}");
        assert!(t.iter().all(|r| (r.w - t[0].w).abs() < 0.01), "equal columns: {t:?}");
        assert!((t[1].x - (t[0].x + t[0].w) - SLIDER_COL_GAP).abs() < 0.01, "{t:?}");
        assert!((t[0].x - 2.0 * MARGIN).abs() < 0.01 && (t[2].x + t[2].w - (960.0 - 2.0 * MARGIN)).abs() < 0.01, "the row spans the field: {t:?}");
        assert_eq!(c.sliders.iter().map(|s| (s.split, s.mark)).collect::<Vec<_>>(), vec![(true, None), (false, Some(0.5)), (false, Some(0.5))]);
        // The name on the label line over the track, the value at the
        // column's right end, the switch between them.
        let name = c.texts.iter().find(|x| x.text == "Lighting").unwrap();
        assert!((name.x - t[0].x).abs() < 0.01 && name.y < t[0].y);
        let value = c.texts.iter().find(|x| x.text == "Dark 10").unwrap();
        assert!(value.x < t[0].x + t[0].w && value.x > t[0].x + t[0].w / 2.0 && value.y < t[0].y, "the value at {}, {}", value.x, value.y);
        let sw = c.chips.iter().find(|ch| matches!(ch.style, Style::Switch { .. })).expect("the switch beside Lighting");
        assert!(sw.rect.y < t[0].y && sw.rect.x > t[0].x && sw.rect.x + sw.rect.w < value.x, "{:?} before the value at {}", sw.rect, value.x);
        assert_eq!(sw.action, PanelAction::Pick { group: 1, index: 0 }, "off: a click asks for On");
        assert!(c.texts.iter().any(|x| x.text == "Ambient"));
        // A press on the switch is the switch's, one on the track the slider's.
        let f = p.field;
        assert!(!p.on_slider((f.x + sw.rect.x + sw.rect.w / 2.0, f.y + sw.rect.y + sw.rect.h / 2.0)));
        assert!(p.on_slider((f.x + t[0].x + t[0].w / 2.0, f.y + t[0].y)));
        assert!(p.on_slider((f.x + t[0].x + 10.0, f.y + name.y)), "the name's line is the slider's where nothing else is");
        // Two alone: each a third wide, the pair centred.
        let two = vec![
            crate::controls::Group { row: true, ..slider_group("colors-palette", "Palette", "50%", 50) },
            crate::controls::Group { row: true, ..slider_group("colors-sat", "Saturation", "as made", 50) },
        ];
        let c2 = colours_open(two, (960.0, 560.0)).open_content("Colours").unwrap();
        let t2: Vec<Rect> = c2.sliders.iter().map(|s| s.track).collect();
        assert_eq!(t2.len(), 2);
        assert!((t2[0].w - t[0].w).abs() < 0.01, "a third still: {} against {}", t2[0].w, t[0].w);
        assert!((t2[0].x - 2.0 * MARGIN - (960.0 - 2.0 * MARGIN - (t2[1].x + t2[1].w))).abs() < 0.01, "centred: {t2:?}");
        // A slider not in a row keeps its own line.
        let one = vec![slider_group("colors-palette", "Palette", "50%", 50)];
        let c1 = colours_open(one, (960.0, 560.0)).open_content("Colours").unwrap();
        assert!(c1.sliders[0].track.w > t[0].w * 2.0);
    }

    #[test]
    fn a_centred_line_holds_its_words_in_the_middle_with_the_switch_at_the_left() {
        let tabs = crate::controls::Group {
            centred: true,
            choices: vec![plain_choice("Shell", true), plain_choice("Apps", false), plain_choice("Icons", false), plain_choice("Web", false)],
            ..plain_group("colors-section")
        };
        let auto = crate::controls::Group { beside: true, ..switch_group("colors-auto", "Auto", false) };
        let alone = crate::controls::Group { centred: true, ..switch_group("colors-auto", "Auto", true) };
        let p = colours_open(vec![tabs, auto, alone], (960.0, 560.0));
        let c = p.open_content("Colours").unwrap();
        let words: Vec<&Chip> = c.chips.iter().filter(|ch| matches!(ch.style, Style::Word { .. })).collect();
        assert_eq!(words.len(), 4);
        let (first, last) = (words[0].rect, words[3].rect);
        assert!(words.iter().all(|w| (w.rect.y - first.y).abs() < 0.01), "one line");
        assert!(((first.x + last.x + last.w) / 2.0 - 480.0).abs() < 1.0, "centred: {first:?} … {last:?}");
        let switches: Vec<&Chip> = c.chips.iter().filter(|ch| matches!(ch.style, Style::Switch { .. })).collect();
        assert_eq!(switches.len(), 2);
        // Auto at the left end of the tabs' line…
        assert!((switches[0].rect.y - first.y).abs() < 0.01, "the same line");
        assert!((switches[0].rect.x - (2.0 * MARGIN - 4.0)).abs() < 0.01 && switches[0].rect.x + switches[0].rect.w < first.x, "{:?}", switches[0].rect);
        assert!(c.texts.iter().any(|x| x.text == "Auto" && (x.x - 2.0 * MARGIN).abs() < 0.01));
        // …and alone on a line of its own, name and switch together in the middle.
        let alone = switches[1].rect;
        assert!(alone.y > first.y + first.h);
        assert!((alone.x + alone.w / 2.0 - 480.0).abs() < 1.0, "centred: {alone:?}");
    }

    #[test]
    fn a_word_row_has_its_name_at_the_left_and_its_words_at_the_right_end() {
        let mut away = switch_group("launcher-away", "Stay away", false);
        away.choices[0].hint = Some("Off the screen until you call it.".to_owned());
        let side = crate::controls::Group {
            labelled: true,
            caption: Some("Position".to_owned()),
            choices: vec![plain_choice("Left", false), plain_choice("Bottom", true), plain_choice("Right", false)],
            ..plain_group("launcher-side")
        };
        let p = colours_open(vec![away, side], (960.0, 560.0));
        let c = p.open_content("Colours").unwrap();
        // The switch row's hint under its name.
        let name = c.texts.iter().find(|x| x.text == "Stay away").unwrap();
        let hint = c.texts.iter().find(|x| x.text.starts_with("Off the screen")).unwrap();
        assert!(hint.y > name.y && (hint.x - name.x).abs() < 0.01);
        // The words on one line, ending at the right margin; the name at the left.
        let words: Vec<&Chip> = c.chips.iter().filter(|ch| matches!(ch.style, Style::Word { .. })).collect();
        assert_eq!(words.len(), 3);
        assert!(words.iter().all(|w| (w.rect.y - words[0].rect.y).abs() < 0.01), "one line");
        let last = words[2].rect;
        assert!((last.x + last.w - 11.0 - (960.0 - 2.0 * MARGIN)).abs() < 0.01, "ends at the right margin: {last:?}");
        let label = c.texts.iter().find(|x| x.text == "Position").unwrap();
        assert!((label.x - 2.0 * MARGIN).abs() < 0.01 && label.y > hint.y && label.y < last.y + last.h);
        assert!(words[1].on && !words[0].on && !words[2].on);
        assert_eq!(words[1].action, PanelAction::Pick { group: 1, index: 1 });
    }

    #[test]
    fn the_screens_are_drawn_picked_turned_and_dragged_to_a_side() {
        use crate::controls::ScreenShape;
        let shape = |sel: &str, x: f64, w: f64, h: f64, picked: bool| ScreenShape {
            sel: sel.to_owned(),
            short: sel.to_owned(),
            x,
            y: 0.0,
            w,
            h,
            rot: 0,
            bar: sel == "laptop",
            dock: sel == "laptop",
            picked,
            mirrored: false,
            isolated: false,
        };
        let screens = vec![shape("laptop", 0.0, 2000.0, 1250.0, true), shape("tv", 2000.0, 3840.0, 2160.0, false)];
        let choices = vec![plain_choice("laptop", true), plain_choice("tv", false), plain_choice("back", false), plain_choice("on", false)];
        let g = crate::controls::Group { screens, choices, ..plain_group("monitors-arrange") };
        let mut p = colours_open(vec![g], (900.0, 760.0));
        let c = p.open_content("Colours").unwrap();
        let boxes: Vec<&Chip> = c.chips.iter().filter(|ch| matches!(ch.style, Style::Screen { .. })).collect();
        let corners: Vec<&Chip> = c.chips.iter().filter(|ch| matches!(ch.style, Style::Corner { .. })).collect();
        assert_eq!(boxes.len(), 2);
        let (laptop, tv) = (boxes[0].rect, boxes[1].rect);
        assert!((laptop.x + laptop.w - tv.x).abs() < 0.01 && (laptop.y - tv.y).abs() < 0.01, "side by side, tops lined up");
        assert!((tv.w / laptop.w - 3840.0 / 2000.0).abs() < 1e-3, "in their own sizes");
        // The picked one's corners: back on its top left, on on its top right.
        assert_eq!(corners.len(), 2);
        assert!(matches!(corners[0].style, Style::Corner { back: true }) && (corners[0].rect.x + corners[0].rect.w / 2.0 - laptop.x).abs() < 0.01);
        assert_eq!(corners[1].action, PanelAction::Pick { group: 0, index: 3 });
        // A click on a corner is the corner's, not the box's under it.
        let f = p.field;
        let at = |r: Rect| (f.x + r.x + r.w / 2.0, f.y + r.y + r.h / 2.0);
        assert_eq!(p.open_hit("Colours", at(corners[1].rect)), Some(Some(PanelAction::Pick { group: 0, index: 3 })));
        // THE TV TAKEN AND MOVED UNDER THE LAPTOP: near enough, it sticks to
        // its bottom, left ends lined up; let go, it rests there.
        let from = at(tv);
        assert!(p.screen_press(from));
        let k = laptop.w / 2000.0;
        let to = (from.0 - (2000.0 * k) - 3.0, from.1 + 1250.0 * k + 4.0);
        p.screen_drag_to(to);
        let c = p.open_content("Colours").unwrap();
        assert_eq!(c.lines.len(), 1, "the edge they touch is lit");
        let (sel, x, y) = p.screen_release().expect("it was moved");
        assert_eq!((sel.as_str(), x, y), ("tv", 0.0, 1250.0));
        // A press and let go without moving is a click: no move.
        assert!(p.screen_press(from));
        assert_eq!(p.screen_release(), None);
    }

    #[test]
    fn cards_stand_seven_across_in_a_wide_field_and_four_in_a_narrow_one() {
        let apps = |n: usize, rows: Option<usize>| {
            let choices = (0..n)
                .map(|i| crate::controls::Choice { icon: Some(i), hint: (i == 0).then(|| "Own colours · Light".to_owned()), ..plain_choice(&format!("App {i}"), i == 0) })
                .collect();
            vec![crate::controls::Group { tiles: true, choices, grid_rows: rows, ..plain_group("colors-app") }]
        };
        let row_len = |c: &OpenContent| c.chips.iter().take_while(|t| (t.rect.y - c.chips[0].rect.y).abs() < 0.5).count();
        // Wide (a 2560 screen's quick box): seven across, four rows, 28 a page.
        let mut wide = colours_open(apps(40, None), (2400.0, 1400.0));
        let c = wide.open_content("Colours").unwrap();
        assert_eq!(row_len(&c), 7);
        assert_eq!(c.chips.len(), 28, "a page");
        assert!(c.chips[0].rect.w >= PALETTE_TILE_MIN && c.chips[0].icon == Some(0) && c.chips[0].image.is_none() && c.chips[0].palette);
        assert!((c.chips[0].rect.x - 2.0 * MARGIN).abs() < 0.01 && (c.chips[6].rect.x + c.chips[6].rect.w - (2400.0 - 2.0 * MARGIN)).abs() < 0.01, "as wide as the rows");
        assert_eq!(c.dots.map(|d| (d.1, d.2)), Some((2, 0)), "two pages, the first up");
        // Drawn at rest: the icons through the dock's layers, the hint on its card.
        let draw = wide.draw(PanelPaint { ink: [1.0; 4], bright: false, highlight: [1.0, 1.0, 1.0, 0.12], plate: [0.5, 0.5, 0.5, 1.0], title_ink: [1.0, 1.0, 1.0, 0.3] });
        assert_eq!(draw.icons.len(), 28);
        assert!((draw.icons[0].1.w - TILE_ICON).abs() < 0.01 && draw.icons[0].0 == 0);
        assert!(draw.labels.iter().any(|l| l.text == "Own colours · Light"));
        assert!(draw.labels.iter().any(|l| l.text == "App 3"));
        // The next page turns as the wall's do.
        assert!(wide.tiles_turn(1));
        assert_eq!(wide.open_content("Colours").unwrap().chips.len(), 12);
        // Fewer rows when asked; few tiles take only their rows.
        assert_eq!(colours_open(apps(40, Some(2)), (2400.0, 1400.0)).open_content("Colours").unwrap().chips.len(), 14);
        let few = colours_open(apps(5, None), (2400.0, 1400.0)).open_content("Colours").unwrap();
        assert_eq!(few.chips.len(), 5);
        assert!(few.foot_y < few.chips[0].rect.y + 2.0 * few.chips[0].rect.h, "the box shrinks to one row");
        // Narrow (the control panel's field): four across — and three rows,
        // since four would run past the field's height.
        let c = colours_open(apps(40, None), (960.0, 700.0)).open_content("Colours").unwrap();
        assert_eq!(row_len(&c), 4);
        assert_eq!(c.chips.len(), 12, "three rows");
        assert!(c.foot_y + QUICK_FOOT <= 700.0, "it fits: {}", c.foot_y);
        // A tall field keeps the four.
        let c = colours_open(apps(40, None), (960.0, 1400.0)).open_content("Colours").unwrap();
        assert_eq!(c.chips.len(), 16);
    }

    #[test]
    fn the_first_tile_of_a_wall_has_a_line_for_its_words_and_no_other_does() {
        let plain = wall_open(30, false).open_content("Wallpaper").unwrap();
        let worded = wall_open(30, true).open_content("Wallpaper").unwrap();
        assert_eq!(plain.chips.len(), worded.chips.len());
        // Only the first tile is a captioned picture (its words under it, a ring when lit).
        assert!(worded.chips[0].palette && worded.chips[0].image.is_some());
        assert!(worded.chips.iter().skip(1).all(|c| !c.palette));
        assert!(plain.chips.iter().all(|c| !c.palette));
        // The tiles of a row: where the next row starts.
        let row_len = |c: &OpenContent| c.chips.iter().take_while(|t| (t.rect.y - c.chips[0].rect.y).abs() < 0.5).count();
        let (n_plain, n_worded) = (row_len(&plain), row_len(&worded));
        assert!(n_worded >= 2 && n_worded < worded.chips.len(), "{n_worded} to a row");
        // Under the first row there is a line more room than between plain rows…
        let under = |c: &OpenContent, n: usize| c.chips[n].rect.y - (c.chips[0].rect.y + c.chips[0].rect.h);
        assert!(under(&worded, n_worded) > under(&plain, n_plain) + 10.0, "{} against {}", under(&worded, n_worded), under(&plain, n_plain));
        // …and between the rows below it the plain gap again.
        if let Some(i) = (n_worded..worded.chips.len()).find(|&i| worded.chips[i].rect.y > worded.chips[n_worded].rect.y + 0.5) {
            let gap = worded.chips[i].rect.y - (worded.chips[n_worded].rect.y + worded.chips[n_worded].rect.h);
            assert!((gap - TILE_GAP).abs() < 0.5, "{gap}");
        }
        // The grid ends where the next thing is told to start: below its last row.
        let last = worded.chips.last().unwrap().rect;
        assert!(worded.foot_y >= last.y + last.h, "{} under {}", worded.foot_y, last.y + last.h);
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
        p.key = (400.0, 560.0, 1.0, 1.0);
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
        let draw = p.draw(PanelPaint { ink: [1.0; 4], bright: false, highlight: [1.0, 1.0, 1.0, 0.12], plate: [0.5, 0.5, 0.5, 1.0], title_ink: [1.0, 1.0, 1.0, 0.3] });
        for want in ["Scale", "Built-in screen · looks like 1440 × 900", "100%", "150%"] {
            assert!(draw.labels.iter().any(|l| l.text == want), "{want:?} is drawn");
        }
        // The one in use is underlined (the hairline language: no rings).
        assert_eq!(draw.rects.iter().filter(|r| r.border > 0.0).count(), 0);
        assert_eq!(draw.rects.iter().filter(|r| r.rect.h <= 2.0 && r.rect.w > 4.0).count(), 1);
    }

    #[test]
    fn the_knob_follows_the_pointer_within_the_track() {
        // Continuous (not stepped), kept on the track.
        assert_eq!(knob_value(150.0, 100.0, 200.0), 0.25);
        assert_eq!(knob_value(101.0, 100.0, 200.0), 0.005);
        assert_eq!(knob_value(40.0, 100.0, 200.0), 0.0);
        assert_eq!(knob_value(900.0, 100.0, 200.0), 1.0);
        assert_eq!(knob_value(5.0, 0.0, 0.0), 1.0, "a track with no width does not divide by zero");
        assert_eq!(knob_level(0.2749), 27);
        assert_eq!(knob_level(2.0), 100);
    }

    #[test]
    fn the_knob_is_drawn_where_it_was_left_while_it_names_the_lit_stop() {
        let knob = |v: f32, held: bool| Some(Knob { group: 0, key: "k", v, stop: 0, held });
        // 21 stops: stop k at k / 20. Held: always the pointer's place.
        assert_eq!(knob_shown(0.0, 21, knob(0.123, true)), 0.123);
        // Let go on the lit stop's side of it: kept (no snap at the release).
        assert_eq!(knob_shown(0.25, 21, knob(0.263, false)), 0.263);
        // The setting moved on another road (the keys): the lit stop's.
        assert_eq!(knob_shown(0.5, 21, knob(0.263, false)), 0.5);
        assert_eq!(knob_shown(0.5, 21, None), 0.5);
    }
}

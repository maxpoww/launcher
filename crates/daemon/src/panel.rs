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

use tracing::info;

use crate::content::{self, Label, Layout, Rect, RectInst, ShadowInst};
use crate::display::DisplayView;
use crate::options::{FONT_PX, LINE_PX, PILL_PAD_X, TEXT_FONT};
use crate::state::Target;
use crate::{apps, groups, App};
use waverunner_proto::Command;

/// One pill of a field: its name, the layer it starts on (0 near, 1
/// middle, 2 far), and its search keywords.
type Item = (&'static str, u8, &'static str);

/// The settings and the layer each starts on (0 near, 1 middle, 2 far):
/// the ones people reach for float nearest. Placeholders until each becomes
/// a real setting.
const SETTINGS: [Item; 24] = [
    // (name, first layer, search keywords: what else people call it or
    // look for it by). 5 near, 8 middle, 11 far.
    ("Resolution", 0, "display screen monitor size pixels"),
    ("Scale", 0, "display screen zoom size hidpi text bigger smaller"),
    ("Wi-Fi", 0, "wifi wireless network internet connection"),
    ("Sound output", 0, "audio speakers speaker headphones output"),
    ("Dark mode", 0, "theme appearance light night colours colors"),
    ("Brightness", 1, "display screen backlight dim"),
    ("Bluetooth", 1, "wireless devices headphones pairing"),
    ("Volume", 1, "audio sound loud quiet mute"),
    ("Wallpaper", 1, "background desktop picture image appearance"),
    ("Night light", 1, "display screen warm blue light sunset eyes"),
    ("Notifications", 1, "alerts do not disturb dnd messages"),
    ("Keyboard layout", 1, "input language typing keys"),
    ("Power mode", 1, "battery performance energy saver"),
    ("Accent colour", 2, "color theme appearance highlight"),
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
const MOD_NAME_PX: f32 = 17.5;
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
        (_, 0) => format!("{} and {}", shown[..shown.len() - 1].join(", "), shown[shown.len() - 1]),
        (_, m) => format!("{} and {m} more", shown.join(", ")),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "program" } else { "programs" }
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
fn match_rank(label: &str, keywords: &str, q: &str) -> Option<u8> {
    let name = label.to_lowercase();
    if name.starts_with(q) {
        Some(0)
    } else if name.contains(q) {
        Some(1)
    } else if keywords.contains(q) {
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
        self.label.to_lowercase().contains(query) || self.keywords.contains(query)
    }
}

// ---- An open setting's choices ------------------------------------------
/// Where an open setting's controls start under its title and subtitle.
const CONTENT_TOP: f32 = 114.0;
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
}

/// One choice of an open setting: a floating pill, lit when it is the one
/// in use.
struct Chip {
    /// At rest, in field coordinates.
    rect: Rect,
    text: String,
    on: bool,
    action: PanelAction,
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
    /// Drawn clipped to their rect (an open module's scrolling list: the
    /// pill at its foot is cut in half, which is what says "more below").
    pub clipped: Vec<(Rect, Vec<RectInst>, Vec<Label>)>,
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
}

impl Panel {
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
                p.w[l] = (measure(p.label, FONT_PX * scale * f) + 2.0 * PILL_PAD_X * scale * f).max(h);
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
        self.open = Some(OpenBox {
            pill: i,
            k: 0.0,
            want: 1.0,
            from: Rect::new(p.pos.0 - w / 2.0, p.pos.1 - h / 2.0, w, h),
        });
        self.hot = None;
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
            out.chips.push(Chip { rect: Rect::new(x, y + row as f32 * (h + row_gap), w, h), text, on, action });
        }
        y + rows as f32 * h + (rows - 1) as f32 * row_gap + GROUP_GAP * s
    }

    /// A caption over the next group of choices. Returns the group's top.
    fn caption(&self, out: &mut OpenContent, y: f32, text: String) -> f32 {
        out.captions.push((text, y));
        y + 27.0 * self.scale
    }

    /// The content of the open setting `label`, if it is a real one: the
    /// screen's choices for Scale and Resolution. `None`: a placeholder.
    fn open_content(&self, label: &str) -> Option<OpenContent> {
        if !is_display_setting(label) {
            return None;
        }
        let s = self.scale;
        let top = CONTENT_TOP * s;
        let mut out = OpenContent { subtitle: String::new(), captions: Vec::new(), chips: Vec::new(), foot_y: top };
        let Some(v) = &self.display else {
            out.subtitle = "No screen to set.".to_owned();
            return Some(out);
        };
        let mut y = top;
        if label == "Scale" {
            let (lw, lh) = v.looks_like();
            out.subtitle = format!("{} · looks like {lw} × {lh}", v.screen);
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
                y = self.caption(&mut out, y, "This screen has one resolution.".to_owned());
            }
            if v.rates.len() > 1 {
                y = self.caption(&mut out, y, "Refresh rate".to_owned());
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
                let text = format!("Keep this resolution? Going back in {:.0} s", left.ceil().max(1.0));
                y = self.caption(&mut out, y, text);
                let chips = vec![
                    ("Keep".to_owned(), true, PanelAction::Keep),
                    ("Go back".to_owned(), false, PanelAction::Revert),
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
                        parts.push(format!("{} to add", add.len()));
                    }
                    if !remove.is_empty() {
                        parts.push(format!("{} to remove", remove.len()));
                    }
                    (ModButton::Apply, "Apply changes".to_owned(), vec![parts.join(" · ")], Some((ModLink::Undo, "undo")))
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
                        sub.push(format!("{} will be removed.", names(&remove, 3)));
                        sub.push("Your files stay where they are.".to_owned());
                    } else {
                        sub.push(format!("{}.", names(&add, 4)));
                    }
                    let kind = if remove.is_empty() { ModButton::Confirm } else { ModButton::ConfirmRemove };
                    (kind, format!("Yes, {} {}", say.join(" and "), plural(n)), sub, Some((ModLink::KeepChoosing, "keep choosing")))
                }
                (false, _, true) => (
                    ModButton::InstallRecommended,
                    "Install recommended".to_owned(),
                    vec!["or switch on the programs you want".to_owned()],
                    None,
                ),
                (false, false, false) => (
                    ModButton::Uninstall,
                    "Uninstall all".to_owned(),
                    vec!["or switch programs on and off".to_owned()],
                    None,
                ),
                (false, true, false) => (
                    ModButton::ConfirmRemove,
                    format!("Yes, uninstall {} {}", have.len(), plural(have.len())),
                    vec!["Your files stay where they are.".to_owned()],
                    Some((ModLink::KeepThem, "keep them")),
                ),
            };
        // From the foot up: the hint, the link, the lines, the button.
        let line = 19.0 * s;
        // Clear of the card's Search pill, which sits in the field's foot.
        let hint_y = fh - 64.0 * s;
        let mut y = hint_y - 6.0 * s;
        let link = link.map(|(what, t)| {
            y -= line;
            let w = est(t, 14.0 * s) + 8.0 * s;
            (Rect::new((fw - w) / 2.0, y, w, line), what, t)
        });
        y -= sub.len() as f32 * line + 4.0 * s;
        let sub: Vec<(String, f32)> = sub.into_iter().enumerate().map(|(i, t)| (t, y + i as f32 * line)).collect();
        let bh = 32.0 * s;
        y -= 8.0 * s + bh;
        let bw = est(&button_text, 16.5 * s) + 38.0 * s;
        let button = Rect::new((fw - bw) / 2.0, y, bw, bh);
        // The list: captions over groups of rows.
        let row_h = 40.0 * s;
        let row_w = (510.0 * s).min(fw - 36.0 * s);
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
                cy += row_h + 8.0 * s;
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
        self.pointer = pointer;
        if let Some(open) = &mut self.open {
            if open.want == 0.0 {
                // Fold back into where the pill is now: the game may have
                // moved it to another layer while it was open.
                let p = &self.pills[open.pill];
                let d = p.drawn as usize;
                let k = p.sc * p.focus_scale();
                let (w, h) = (p.w[d] * k, p.h[d] * k);
                open.from = Rect::new(p.pos.0 - w / 2.0, p.pos.1 - h / 2.0, w, h);
            }
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
        if let Some(content) = self.open_content(p.label) {
            // A real setting: its choices as floating pills, the one in use
            // lit and ringed (the settings boxes' preset idiom), each on its
            // own tiny orbit like the field's pills.
            text(out, content.subtitle, cx, y, 14.0, 18.0, 0.55);
            for (caption, top) in content.captions {
                text(out, caption, cx, top, 13.0, 17.0, 0.55);
            }
            let layer_wash = if paint.bright { LAYER_WASH_BRIGHT } else { LAYER_WASH_DARK };
            let hover_a = if paint.bright { HOVER_WASH_BRIGHT } else { HOVER_WASH_DARK };
            let line = 20.0 * s;
            for (i, chip) in content.chips.into_iter().enumerate() {
                let r = chip.rect;
                let hot = resting
                    && self.pointer.is_some_and(|pos| Rect::new(f.x + r.x, f.y + r.y, r.w, r.h).contains(pos));
                let a = self.clock * TAU / (13.0 + 1.7 * (i % 5) as f32) + 1.9 * i as f32;
                let (x0, y0) = (r.x + 1.1 * s * a.cos(), r.y + 1.1 * s * a.sin());
                let wa = if hot {
                    hover_a
                } else if chip.on {
                    layer_wash[0]
                } else {
                    layer_wash[1]
                };
                rect(out, x0, y0, r.w, r.h, r.h / 2.0, crate::options::wash(!paint.bright, wa));
                if chip.on {
                    let (mx, my) = map(x0, y0);
                    out.rects.push(RectInst {
                        rect: Rect::new(mx, my, r.w * cs, r.h * cs),
                        radius: r.h / 2.0 * cs,
                        color: [paint.ink[0], paint.ink[1], paint.ink[2], 0.5 * ca],
                        glass: 0.0,
                        border: (1.4 * s * cs).max(1.0),
                    });
                }
                let ink_a = if chip.on || hot { 1.0 } else { LAYER_INK[1] };
                text(out, chip.text, x0 + r.w / 2.0, y0 + (r.h - line) / 2.0, 16.0, 20.0, ink_a);
            }
            text(out, "Esc or a click on the empty space closes".to_owned(), cx, content.foot_y, 12.0, 16.0, 0.4);
            return;
        }
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
                text(&mut inside, cap.clone(), cx, top + cy, 15.5, 19.0, 0.62);
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
                let pad = 18.0 * s;
                let sw = (38.0 * s, 22.0 * s);
                let name_w = self.name_w.get(row.prog.name.as_str()).copied().unwrap_or(0.0);
                inside.labels.push(left(row.prog.name.clone(), pad, MOD_NAME_PX, 1.0, rw - 2.0 * pad - sw.0));
                let what = match (on, was) {
                    (true, false) => format!("will be added · {}", row.prog.what),
                    (false, true) => format!("will be removed · {}", row.prog.what),
                    _ => row.prog.what.clone(),
                };
                let dx = pad + name_w + 10.0 * s;
                let room = rw - dx - pad - sw.0 - 8.0 * s;
                if room > 20.0 * s {
                    inside.labels.push(left(what, dx, 14.5, 0.65, room));
                }
                // The switch: on, white with a dark knob; off, a quiet wash.
                let (tx, ty) = (rx + rw - 9.0 * s - sw.0, ry + (rh - sw.1) / 2.0);
                rect(&mut inside, tx, ty, sw.0, sw.1, sw.1 / 2.0, if on { [paint.ink[0], paint.ink[1], paint.ink[2], 0.92] } else { wash(0.16) });
                let knob = 16.0 * s;
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
            text(out, l.button_text.clone(), cx, b.y + (b.h - 21.0 * s) / 2.0, 16.5, 21.0, 1.0);
            for (t, ty) in &l.sub {
                text(out, t.clone(), cx, *ty, 14.0, 18.0, 0.65);
            }
            if let Some((r, _, t)) = &l.link {
                let hot = resting && self.pointer.is_some_and(|pos| Rect::new(f.x + r.x, f.y + r.y, r.w, r.h).contains(pos));
                text(out, t.to_string(), cx, r.y, 14.0, 18.0, if hot { 1.0 } else { 0.85 });
                rect(out, cx - r.w / 2.0 + 4.0 * s, r.y + 17.0 * s, r.w - 8.0 * s, 1.0 * s, 0.0, [paint.ink[0], paint.ink[1], paint.ink[2], 0.5]);
            }
            text(out, "Esc or a click on the empty space closes".to_owned(), cx, l.hint_y, 12.0, 16.0, 0.4);
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

    /// The catalog programs this computer has: an app in the grid by the
    /// program's name (lowercased).
    fn modules_installed(&self) -> std::collections::HashSet<String> {
        let have: std::collections::HashSet<String> = (0..self.base_len.min(self.entries.len()))
            .filter(|&i| self.kinds.get(i) == Some(&apps::EntryKind::App) && !self.is_catalog_webapp(i))
            .map(|i| self.entries[i].name.to_lowercase())
            .collect();
        crate::modules::catalog()
            .modules
            .iter()
            .flat_map(|m| m.groups.iter().flat_map(|g| &g.programs))
            .map(|p| p.name.to_lowercase())
            .filter(|n| have.contains(n))
            .collect()
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

    pub(crate) fn panel_click(&mut self, pos: (f32, f32)) {
        if self.panel.click(pos) {
            // A module's Apply: the wanted set goes to the system side, and
            // the card folds away.
            if let Some((module, add, remove)) = self.panel.pending_apply.take() {
                crate::modules::apply(module, &add, &remove);
                self.handle_command(Command::Collapse);
                return;
            }
            match self.panel.take_action() {
                Some(PanelAction::Scale(scale)) => self.set_display_scale(scale),
                Some(PanelAction::Mode { w, h, hz }) => self.set_display_mode(w, h, hz),
                Some(PanelAction::Keep) => self.keep_display(),
                Some(PanelAction::Revert) => self.revert_display(),
                None => {}
            }
            self.schedule_frame();
        }
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
        assert_eq!(per_layer, [5, 8, 11]);
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
        assert_eq!(counts(&p), [8, 11, 5], "down: the near group goes to the back");
        assert_clean(&p, "one step down");
        p.shift_layers(1);
        assert_eq!(counts(&p), [11, 5, 8]);
        assert_clean(&p, "two steps down");
        p.shift_layers(-1);
        p.shift_layers(-1);
        assert_eq!(counts(&p), [5, 8, 11], "up undoes it");
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
        assert_eq!(layer_sizes(&p), [5, 8, 11], "the layers keep their sizes");
        p.record_use(far, now);
        assert_eq!(p.pills[far].group, 0, "a third steps it to the front");
        assert_eq!(layer_sizes(&p), [5, 8, 11]);
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
    fn a_placeholder_setting_still_shows_its_sample_controls() {
        let mut p = panel();
        p.field = Rect::new(0.0, 0.0, 960.0, 560.0);
        p.step(0.0, None);
        p.open_named("Wi-Fi");
        assert!(!p.display_due());
        assert!(p.open_content("Wi-Fi").is_none());
        let draw = p.draw(PanelPaint { ink: [1.0; 4], bright: false });
        assert!(!draw.rects.is_empty());
        // Its sample rows take a click without asking for anything.
        for _ in 0..40 {
            p.step(1.0 / 60.0, None);
        }
        assert!(p.click((480.0, 114.0 + 23.0)));
        assert_eq!(p.take_action(), None);
        assert!(p.open.as_ref().is_some_and(|o| o.want > 0.5), "a click on a control keeps it open");
    }

    #[test]
    fn an_open_display_setting_draws_its_choices() {
        let p = display_open("Scale", 1.0);
        let draw = p.draw(PanelPaint { ink: [1.0; 4], bright: false });
        for want in ["Scale", "Built-in screen · looks like 1440 × 900", "100%", "150%"] {
            assert!(draw.labels.iter().any(|l| l.text == want), "{want:?} is drawn");
        }
        // The one in use wears a ring.
        assert_eq!(draw.rects.iter().filter(|r| r.border > 0.0).count(), 1);
    }
}

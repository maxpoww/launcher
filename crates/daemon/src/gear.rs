//! The gear box's pages that hold real content: **Wi-Fi** and **Bluetooth**.
//!
//! Built from the notification and clipboard boxes' vocabulary rather than a
//! new one: edge-to-edge rows with zebra, a hairline frame and firmer ink on
//! hover, floating footer circles, and a footer that stretches into a field
//! when something has to be typed (a password, a search, a new name).
//!
//! A page is a [`View`]: a plain list of [`Item`]s plus the round buttons
//! above and below. One layout ([`App::gear_layout`]) places them, and both
//! the draw and the hit-test walk that same layout, so a control can never be
//! somewhere other than where it is drawn. The pages themselves
//! ([`App::net_view`], [`App::bt_view`]) only describe what is there.

use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::{debug, info};

use crate::animation::{ease_toward, lerp, settle_t, MORPH_RATE, SCROLL_RATE, SETTLE_ALPHA};
use crate::bt::{BtAudio, BtCommand, BtDevice, BtEvent, BtHandle, BtSnapshot};
use crate::bt_files::{FileCommand, FileEvent, FileHandle};
use crate::content::{GridContent, Label, Rect, RectInst, Scene};
use crate::net::{self, Ap, NetCommand, NetDetail, NetEvent, NetHandle, NetSnapshot};
use crate::options::{
    hover_grow, hover_ink_for, push_neumorph, FONT_PX,
    LINE_PX, NERD, PILL_MARGIN_Y, PILL_PAD_X,
};
use crate::App;
use crate::i18n::tr;

// --- geometry, in logical px at scale 1 (the notification card's numbers) ---
const ROW_PAD_X: f32 = 14.0;
const ROW_PAD_Y: f32 = 11.0;
const TILE: f32 = 40.0;
const TILE_GAP: f32 = 11.0;
const SMALL_TILE: f32 = 28.0;
const SUB_GAP: f32 = 3.0;
const TRAIL_PAD: f32 = 8.0;
const MORE_SZ: f32 = 28.0;
const HEAD_H: f32 = 30.0;
const KV_H: f32 = 30.0;
/// A level's row; its symbol's circle; the room for its words at the right.
const LEVEL_H: f32 = 44.0;
const LEVEL_GLYPH: f32 = 30.0;
const LEVEL_WORD_W: f32 = 52.0;
/// The › of a row with a page of its own.
const GO_SZ: f32 = 24.0;
/// "‹ Back"'s air at its ends.
const BACK_PAD: f32 = 12.0;
/// How far in from the box's sides a row's tint stands.
const ROW_INSET: f32 = 6.0;
const KV_KEY_W: f32 = 118.0;
const CHOICE_H: f32 = 38.0;
const CHOICE_PILL_H: f32 = 26.0;
const STRIP_H: f32 = 52.0;
const STRIP_GAP: f32 = 6.0;
const STRIP_X: f32 = 12.0;
/// A footer button's words: their margin at its left end, and how far into
/// the symbol's circle they may reach (its empty side), in circles.
const WORDS_PAD: f32 = 0.15;
const WORDS_INTO: f32 = 0.12;
const FOOTER_GAP: f32 = 26.0;
const FIELD_PAD: f32 = 12.0;
const CARD_TILE: f32 = 56.0;
const QR_SZ: f32 = 124.0;
const QR_PAD: f32 = 8.0;
const SMALL_PX: f32 = 14.0;
const SMALL_LINE: f32 = 17.0;
const CODE_PX: f32 = 38.0;
use crate::kit::{SWITCH_H, SWITCH_W};
/// The box's height when the page has nothing to list (radio off, airplane
/// mode): the switch row and one line under it.
const COMPACT_H: f32 = 150.0;
const GRAPH_H: f32 = 56.0;
const CORES_H: f32 = 30.0;
/// Pixels of list travel per wheel unit — the other boxes' figure.
const SCROLL_SPEED: f32 = 3.0;
/// What a sign-in page is asked for: a plain-http address every portal
/// intercepts, and the one Seam's own check uses.
const PORTAL_URL: &str = "http://detectportal.firefox.com/canonical.html";

// --- glyphs (JetBrainsMono Nerd Font Mono; each checked against the font) ---
const G_WIFI: [&str; 4] = ["\u{f091f}", "\u{f0922}", "\u{f0925}", "\u{f0928}"];
const G_ETH: &str = "\u{f0200}";
const G_LOCK: &str = "\u{f023}";
const G_CHECK: &str = "\u{f00c}";
/// A row with a page of its own ends in this, always (THE KIT: the details
/// are a ›, never a hidden ⋯).
pub(crate) const G_GO: &str = "\u{f0142}";
const G_REFRESH: &str = "\u{f021}";
const G_PLUS: &str = "\u{f067}";
const G_EYE: &str = "\u{f06e}";
/// Being visible to others (Bluetooth): a broadcast.
const G_BROADCAST: &str = "\u{f1720}";
const G_QR: &str = "\u{f029}";
const G_LINK: &str = "\u{f0c1}";
const G_UNLINK: &str = "\u{f127}";
const G_TRASH: &str = "\u{f014}";
pub(crate) const G_BACK: &str = "\u{f053}";
const G_PENCIL: &str = "\u{f040}";
const G_TIMES: &str = "\u{f00d}";
const G_SEARCH: &str = "\u{f002}";
const G_HOTSPOT: &str = "\u{f0003}";
const G_BT: &str = "\u{f00af}";
const G_HEADPHONES: &str = "\u{f025}";
const G_SPEAKER: &str = "\u{f04c3}";
const G_MOUSE: &str = "\u{f037d}";
const G_KEYBOARD: &str = "\u{f11c}";
const G_PHONE: &str = "\u{f10b}";
const G_GAMEPAD: &str = "\u{f11b}";
const G_LAPTOP: &str = "\u{f109}";
const G_SEND: &str = "\u{f093}";

/// Which page of the gear box is showing, by what it is rather than by its
/// number (the numbers shift with the readings a machine has).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageKind {
    Gear,
    Net,
    Bt,
    Disk,
    Cpu,
    Ram,
    Gpu,
    Battery,
    /// This computer (the machine's details): a page of the control panel
    /// only, hosted there (`App::gear_host`); the box has no button for it.
    About,
    Other,
}

/// Everything a press can land on.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum Hit {
    #[default]
    None,
    Back,
    Airplane,
    Field,
    FieldEye,
    // Wi-Fi
    WifiPower,
    Net(String),
    NetMore(String),
    WifiScan,
    Hidden,
    Hotspot,
    NetToggle,
    NetShare,
    NetShowPw,
    NetForget,
    NetForgetNow,
    /// A saved network out of range: forget it (asked), then for good.
    NetForgetSaved(String),
    NetForgetSavedNow(String),
    NetAuto,
    NetMetered,
    NetPrivate,
    NetIp(bool),
    NetEdit(EditKey),
    HsPower,
    HsName,
    HsPass,
    HsBand(bool),
    HsEye,
    BtShare,
    // Bluetooth
    BtPower,
    BtName,
    Dev(String),
    DevMore(String),
    BtScan,
    BtVisible,
    DevToggle,
    DevForget,
    DevForgetNow,
    DevAuto,
    DevRename,
    DevQuality(bool),
    DevOutput,
    PairYes,
    PairNo,
    BtFiles,
    SendTo(String),
    BtReceive,
    /// The footer's "click again" line: do what was asked.
    Confirm,
    /// The machine pages' own targets (`gear_pages.rs`).
    Sys(crate::gear_pages::SysHit),
    /// The quick settings' (`quick.rs`).
    Quick(crate::quick::QuickHit),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditKey {
    Ip,
    Gateway,
    Dns,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tone {
    #[default]
    Normal,
    /// Something is in progress ("Connecting…").
    Busy,
    /// It needs the person (wrong password, sign-in page).
    Warn,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum Trail {
    #[default]
    None,
    Glyph(&'static str),
    Switch(bool),
    Battery(u8),
    /// A figure at the row's end ("12%", "3.2 GB").
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum Extra {
    #[default]
    None,
    /// A QR code (rows of dark modules) and the line under it.
    Qr(Vec<Vec<bool>>, String),
    /// A pairing number, large.
    Code(String),
    Battery(u8),
    /// A filled bar (0–100) and the line under it.
    Meter(u8, String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Item {
    Row {
        hit: Hit,
        /// The corner control that replaces the trail on hover.
        more: Option<(Hit, &'static str)>,
        tile: Option<&'static str>,
        title: String,
        sub: String,
        tone: Tone,
        trail: Trail,
        /// Held lit (the row a field is about).
        sel: bool,
    },
    Heading {
        text: String,
        busy: bool,
    },
    Card {
        glyph: &'static str,
        title: String,
        sub: String,
        extra: Extra,
        rename: Option<Hit>,
    },
    Kv {
        hit: Option<Hit>,
        key: String,
        value: String,
    },
    Toggle {
        hit: Hit,
        label: String,
        hint: String,
        on: bool,
    },
    Choice {
        key: String,
        opts: Vec<(Hit, String, bool)>,
    },
    Empty(String),
    /// A reading's recent past, as a row of bars (0–100 each).
    Graph(Vec<f32>),
    /// One short bar per processor core (0–100 each).
    Cores(Vec<f32>),
    /// A segmented bar and its legend: `(label, weight, opacity)`.
    Bar(Vec<(String, f32, f32)>),
    /// A small dim line under what it explains.
    Note(String),
    /// A level set by hand (THE KIT's slider: a 2 px track, the accent up to
    /// the value, a 12 px knob; a press, a drag or the wheel set it): its
    /// symbol at the left — a click there does `glyph_hit` (mute) —, at the
    /// right the value in words, or a word a click turns (`word_hit`: Auto,
    /// lit by `word_on`).
    Level {
        hit: Hit,
        glyph: &'static str,
        glyph_hit: Hit,
        value: f32,
        word: String,
        word_hit: Hit,
        word_on: bool,
        /// Drawn quieter (muted: the level is kept, nothing is heard).
        quiet: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Btn {
    pub(crate) hit: Hit,
    pub(crate) glyph: &'static str,
    /// Drawn pressed-in: the state it switches is on.
    pub(crate) lit: bool,
    /// Buttons tucked behind this one, out to its LEFT while the pointer is on
    /// it, over what stands there — which steps away meanwhile (the power
    /// button: sleep, restart, over the lock; Max, 2026-11-03).
    pub(crate) behind: Vec<Btn>,
    /// Words beside the symbol (the sleep timer's countdown): the button grows
    /// to the LEFT, its symbol where it was. With no symbol, the words alone
    /// in the circle ("+15").
    pub(crate) label: Option<String>,
    /// The words beside the symbol answer a click of their own (the running
    /// timer's minutes: edited), underlined under the pointer.
    pub(crate) label_hit: Option<Hit>,
}

pub(crate) fn btn(hit: Hit, glyph: &'static str) -> Btn {
    Btn {
        hit,
        glyph,
        ..Default::default()
    }
}

/// One page, described: what is on it, top to bottom.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct View {
    /// Round buttons under the band (a details view's way out and actions).
    pub(crate) strip: Vec<Btn>,
    pub(crate) items: Vec<Item>,
    /// Round buttons floating at the bottom.
    pub(crate) footer: Vec<Btn>,
    /// Nothing to list: the box shrinks to [`COMPACT_H`].
    pub(crate) compact: bool,
    pub(crate) zebra: bool,
    /// Items from this index on sit on the second tone (a details sheet).
    pub(crate) sheet_from: Option<usize>,
}

/// Where a [`View`] lands inside the box.
pub(crate) struct Layout {
    content: Rect,
    items: Vec<Rect>,
    strip: Vec<Rect>,
    footer: Vec<Rect>,
    /// Each footer button's buttons behind it, as far out as they are.
    footer_behind: Vec<Vec<Rect>>,
    /// The footer buttons those come out over (they step away meanwhile): the
    /// index of the button whose buttons cover each.
    footer_covered: Vec<Option<usize>>,
    field: Option<Rect>,
    total_h: f32,
}

#[derive(Debug, Clone, PartialEq)]
enum FieldKind {
    Search,
    /// Something a machine page asked for (`gear_pages.rs`).
    Page(crate::gear_pages::PField),
    Password {
        ssid: String,
        hidden: bool,
    },
    HiddenSsid,
    Edit(EditKey),
    Rename(String),
    AdapterName,
    HsName,
    HsPass,
}

#[derive(Debug, Clone, PartialEq)]
struct Field {
    kind: FieldKind,
    text: String,
    prompt: String,
    glyph: &'static str,
    secret: bool,
    show: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
enum NetView {
    #[default]
    List,
    Detail(String),
    Hotspot,
}

#[derive(Debug, Clone, PartialEq, Default)]
enum BtView {
    #[default]
    List,
    Detail(String),
    Pair(String),
    Send,
    Share,
}

#[derive(Default)]
pub(crate) struct GearState {
    net: Option<NetHandle>,
    bt: Option<BtHandle>,
    files: Option<FileHandle>,
    /// The device a file is going to, and what its row says meanwhile.
    sending: Option<(String, String)>,
    net_snap: NetSnapshot,
    bt_snap: BtSnapshot,
    net_detail: Option<NetDetail>,
    bt_audio: Option<BtAudio>,
    net_view: NetView,
    bt_view: BtView,
    /// The network a connection attempt is running for, and the one whose
    /// last attempt was refused for its password.
    net_busy: Option<String>,
    net_err: Option<String>,
    /// Whether that failure was the password (else: it could not connect),
    /// and whether it was refused here for being too short.
    net_err_password: bool,
    net_err_short: bool,
    net_scanning: bool,
    hs_busy: bool,
    /// Why sharing did not start, as a title and a line under it.
    share_err: Option<(&'static str, &'static str)>,
    share: bool,
    show_pw: bool,
    qr: Option<Vec<Vec<bool>>>,
    bt_busy: Option<String>,
    /// The device whose connecting failed last: said on its row until it is
    /// tried again.
    bt_err: Option<String>,
    /// The network being left (its row says "Disconnecting…" until it is).
    net_leaving: Option<String>,
    /// Whether that device was connected when the action began: what the
    /// action is called, and how its end is recognised.
    bt_busy_was: bool,
    /// The number a pairing shows, and whether it waits for a yes.
    pair: Option<(String, bool)>,
    field: Option<Field>,
    field_t: f32,
    pub(crate) keyboard_held: bool,
    hit: Hit,
    scroll: f32,
    scroll_target: f32,
    /// The box's eased height (0 = not seeded yet).
    box_h: f32,
    /// A freshly shown view fades in.
    view_t: f32,
    watching: Option<PageKind>,
    /// `debug-gear open`: keep the box up with no pointer on it (the rig has
    /// none), until `debug-gear close`.
    debug_hold: bool,
    /// The machine pages' state (`gear_pages.rs`).
    pub(crate) pages: crate::gear_pages::PagesState,
    /// A thing that cannot be undone, asked once: what the footer says, and
    /// what a second click does.
    pub(crate) arm: Option<(String, Hit)>,
    /// The footer button (its index) the pointer is on, with buttons behind
    /// it, or on those; the one whose buttons are out or going (`behind_ix`),
    /// and how far, 0 → 1. One at a time.
    behind_open: Option<usize>,
    behind_ix: Option<usize>,
    behind_out: f32,
    /// The quick settings' (`quick.rs`).
    pub(crate) quick: crate::quick::QuickState,
    /// The strip's and the footer's buttons say their names (THE KIT).
    names: crate::kit::Names<Hit>,
}

impl GearState {
    pub(crate) fn box_h(&self) -> f32 {
        self.box_h
    }
}

/// A width for text the draw pass cannot measure (it holds `&self`; the
/// renderer measures through `&mut`). Proportional sans averages just over
/// half an em per character, which is close enough to place a caret or cut a
/// long name — the same estimate the clipboard box's rows use.
fn est_w(text: &str, px: f32) -> f32 {
    text.chars().count() as f32 * px * 0.54
}

/// `text` cut to fit `max_w`, with an ellipsis when it had to be.
fn fit(text: &str, max_w: f32, px: f32) -> String {
    if est_w(text, px) <= max_w {
        return text.to_owned();
    }
    let keep = ((max_w / (px * 0.54)) as usize).saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    out
}

fn wifi_glyph(signal: u8) -> &'static str {
    G_WIFI[((signal as usize * G_WIFI.len()) / 101).min(G_WIFI.len() - 1)]
}

fn signal_word(signal: u8) -> &'static str {
    match signal {
        75..=u8::MAX => tr("Excellent"),
        50..=74 => tr("Good"),
        25..=49 => tr("Fair"),
        _ => tr("Weak"),
    }
}

/// A Bluetooth device's symbol and what to call its kind, from BlueZ's icon
/// name.
fn bt_kind(icon: &str) -> (&'static str, &'static str) {
    match icon {
        "audio-headphones" | "audio-headset" => (G_HEADPHONES, tr("Headphones")),
        "audio-card" | "audio-speakers" => (G_SPEAKER, tr("Speaker")),
        "input-mouse" | "input-tablet" => (G_MOUSE, tr("Mouse")),
        "input-keyboard" => (G_KEYBOARD, tr("Keyboard")),
        "phone" => (G_PHONE, tr("Phone")),
        "input-gaming" => (G_GAMEPAD, tr("Game controller")),
        "computer" => (G_LAPTOP, tr("Computer")),
        _ => (G_BT, tr("Device")),
    }
}

/// The dark modules of a QR code for `payload`, row by row.
fn qr_matrix(payload: &str) -> Option<Vec<Vec<bool>>> {
    let code = qrcode::QrCode::new(payload.as_bytes()).ok()?;
    let w = code.width();
    let colors = code.to_colors();
    Some(
        colors
            .chunks(w)
            .map(|row| row.iter().map(|c| *c == qrcode::Color::Dark).collect())
            .collect(),
    )
}

/// A password for a new hotspot: ten characters with no look-alikes, from the
/// kernel's randomness.
fn random_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut bytes = [0u8; 10];
    let read = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes));
    if read.is_err() {
        // No randomness to read: fall back on the clock, which is at least
        // different on every machine and every run.
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (n >> (i * 5)) as u8;
        }
    }
    bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_owned())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "golem".to_owned())
}

fn item_h(item: &Item, s: f32, box_w: f32) -> f32 {
    match item {
        Item::Row { sub, .. } => {
            let text = if sub.is_empty() {
                LINE_PX
            } else {
                2.0 * LINE_PX + SUB_GAP
            };
            (2.0 * ROW_PAD_Y + text) * s
        }
        Item::Heading { .. } => HEAD_H * s,
        Item::Level { .. } => LEVEL_H * s,
        Item::Kv { .. } => KV_H * s,
        // (THE KIT: a switch's row is a row — the list's height and words.)
        Item::Toggle { hint, .. } => {
            let text = if hint.is_empty() { LINE_PX } else { 2.0 * LINE_PX + SUB_GAP };
            (2.0 * ROW_PAD_Y + text) * s
        }
        Item::Choice { .. } => CHOICE_H * s,
        Item::Card { extra, .. } => {
            let extra = match extra {
                Extra::None => 0.0,
                Extra::Qr(..) => 8.0 + QR_SZ + 2.0 * QR_PAD + 6.0 + SMALL_LINE,
                Extra::Code(_) => 8.0 + CODE_PX * 1.2,
                Extra::Battery(_) | Extra::Meter(..) => 10.0 + 5.0 + 6.0 + SMALL_LINE,
            };
            (6.0 + CARD_TILE + 8.0 + LINE_PX + SUB_GAP + LINE_PX + extra + 16.0) * s
        }
        // Takes whatever is left; this is only its floor.
        Item::Empty(_) => 60.0 * s,
        Item::Graph(_) => GRAPH_H * s,
        Item::Cores(_) => CORES_H * s,
        Item::Bar(parts) => {
            // The legend wraps: count its lines the way the draw lays them.
            let w = box_w - 2.0 * ROW_PAD_X * s;
            let mut lines = 1.0;
            let mut x = 0.0;
            for (label, _, _) in parts {
                let lw = est_w(label, SMALL_PX * s) * 1.14 + 26.0 * s;
                if x > 0.0 && x + lw > w {
                    lines += 1.0;
                    x = 0.0;
                }
                x += lw;
            }
            (6.0 + 8.0 + 8.0 + lines * SMALL_LINE + 8.0) * s
        }
        Item::Note(_) => (SMALL_LINE + 8.0) * s,
    }
}

/// The pressable parts of one item, first match wins.
fn item_zones(item: &Item, r: Rect, s: f32) -> Vec<(Hit, Rect)> {
    match item {
        Item::Row { hit, more, trail, .. } => {
            let mut z = Vec::new();
            if let Some((m, g)) = more {
                z.push((m.clone(), row_more_rect(r, s, trail, g)));
            }
            if *hit != Hit::None {
                z.push((hit.clone(), r));
            }
            z
        }
        Item::Kv { hit: Some(h), .. } => vec![(h.clone(), r)],
        Item::Level { hit, glyph_hit, word_hit, .. } => {
            let (g, (x0, x1), word) = level_parts(r, s);
            let mut z = Vec::new();
            if *glyph_hit != Hit::None {
                z.push((glyph_hit.clone(), g));
            }
            if *word_hit != Hit::None {
                z.push((word_hit.clone(), word));
            }
            // (The track's zone is the row's height and a knob's width past
            // each end: a press near the end is the end.)
            z.push((hit.clone(), Rect::new(x0 - 8.0 * s, r.y, x1 - x0 + 16.0 * s, r.h)));
            z
        }
        Item::Toggle { hit, .. } => vec![(hit.clone(), r)],
        Item::Choice { key, opts } => {
            // The key takes the room its word needs, not a fixed column: on a
            // narrow box that is what lets every pill stay on the one line.
            let mut x = r.x + ROW_PAD_X * s + choice_key_w(key, s);
            let h = CHOICE_PILL_H * s;
            opts.iter()
                .map(|(hit, label, _)| {
                    let w = est_w(label, 15.0 * s) + 22.0 * s;
                    let pill = Rect::new(x, r.y + (r.h - h) / 2.0, w, h);
                    x += w + 6.0 * s;
                    (hit.clone(), pill)
                })
                .collect()
        }
        Item::Card {
            rename: Some(h),
            title,
            ..
        } => {
            let tw = est_w(title, FONT_PX * s);
            let y = r.y + (6.0 + CARD_TILE + 8.0) * s;
            vec![(
                h.clone(),
                Rect::new(
                    r.x + r.w / 2.0 + tw / 2.0 + 4.0 * s,
                    y - 3.0 * s,
                    26.0 * s,
                    26.0 * s,
                ),
            )]
        }
        _ => Vec::new(),
    }
}

/// How much of a choice row its key takes, with the gap after it.
fn choice_key_w(key: &str, s: f32) -> f32 {
    (est_w(key, FONT_PX * s) * 1.1 + 14.0 * s).min((KV_KEY_W + 10.0) * s)
}

/// How much of a row's end its trail takes (none: nothing).
fn trail_w(trail: &Trail, s: f32) -> f32 {
    match trail {
        Trail::None => 0.0,
        Trail::Glyph(_) => 18.0 * s,
        Trail::Switch(_) => (SWITCH_W + 6.0) * s,
        Trail::Text(t) => est_w(t, 15.0 * s) * 1.25 + 6.0 * s,
        Trail::Battery(b) => est_w(&format!("{b}%"), 15.0 * s) + 4.0 * s,
    }
}

/// A row's corner control: at the row's end where it has no trail, else just
/// LEFT of the trail — which stays, and stays what a click there does (Max,
/// 2026-11-05: on hover the Bluetooth switch became a pencil, "si hago click
/// estoy trabajando en el toggle"). A row's way to its own page (`G_GO`) is
/// always at the very end, the trail left of it.
fn row_more_rect(r: Rect, s: f32, trail: &Trail, glyph: &str) -> Rect {
    if glyph == G_GO {
        return row_go_rect(r, s);
    }
    let d = MORE_SZ * s;
    let end = r.x + r.w - TRAIL_PAD * s + 4.0 * s;
    let tw = trail_w(trail, s);
    let x = if tw > 0.0 { end - 4.0 * s - tw - 6.0 * s - d } else { end - d };
    // Beside the trail, on its line: a switch stands in the row's middle.
    let cy = if matches!(trail, Trail::Switch(_)) {
        r.y + r.h / 2.0
    } else {
        r.y + ROW_PAD_Y * s + LINE_PX * s / 2.0
    };
    Rect::new(x, cy - d / 2.0, d, d)
}

/// The › of a row with a page of its own: at its end, in its middle.
fn row_go_rect(r: Rect, s: f32) -> Rect {
    let d = GO_SZ * s;
    Rect::new(r.x + r.w - TRAIL_PAD * s + 4.0 * s - d, r.y + (r.h - d) / 2.0, d, d)
}

/// "‹ Back"'s width (its words, estimated as the rows' are, and their air).
fn back_w(s: f32) -> f32 {
    est_w(tr("‹ Back"), FONT_PX * s) * 1.1 + 2.0 * BACK_PAD * s
}

/// A level's three parts: its symbol's circle (where a row's tile stands), the
/// track's run `(x0, x1)`, and the word's place at the right.
fn level_parts(r: Rect, s: f32) -> (Rect, (f32, f32), Rect) {
    let d = LEVEL_GLYPH * s;
    let cx = r.x + ROW_PAD_X * s + TILE * s / 2.0;
    let g = Rect::new(cx - d / 2.0, r.y + (r.h - d) / 2.0, d, d);
    let wh = CHOICE_PILL_H * s;
    let word = Rect::new(r.x + r.w - (TRAIL_PAD + LEVEL_WORD_W) * s, r.y + (r.h - wh) / 2.0, LEVEL_WORD_W * s, wh);
    let x0 = r.x + (ROW_PAD_X + TILE + TILE_GAP) * s;
    let x1 = word.x - 12.0 * s;
    (g, (x0, x1.max(x0 + 1.0)), word)
}

/// The content-scissored grid this page's rects ride, so a half-scrolled row's
/// tile or stripe is cut at the list's edge instead of drawn over the footer.
fn gear_grid(scene: &mut Scene, content: Rect) -> &mut GridContent {
    let at = match scene.grids.iter().position(|g| g.clip == content) {
        Some(i) => i,
        None => {
            scene.grids.push(GridContent {
                clip: content,
                ..Default::default()
            });
            scene.grids.len() - 1
        }
    };
    &mut scene.grids[at]
}

struct Pen {
    s: f32,
    a: f32,
    ink: [f32; 4],
    dim: [f32; 4],
    hot: [f32; 4],
    /// The row under the pointer: the kit's accent tint (`kit::row_tint`).
    tint: [f32; 4],
    content: Rect,
}

impl Pen {
    #[allow(clippy::too_many_arguments)]
    fn text(
        &self,
        scene: &mut Scene,
        text: String,
        at: (f32, f32),
        max_w: f32,
        px: (f32, f32),
        color: [f32; 4],
        centered: bool,
        family: Option<&'static str>,
    ) {
        scene.labels.push(Label {
            text,
            pos: at,
            max_w,
            font_px: px.0,
            line_px: px.1,
            centered,
            dim: false,
            cache: false,
            family,
            color: Some([color[0], color[1], color[2], color[3] * self.a]),
            clip: Some(self.content),
        });
    }

    fn glyph(&self, scene: &mut Scene, g: &str, center: (f32, f32), px: f32, color: [f32; 4]) {
        self.text(
            scene,
            g.to_owned(),
            (center.0, center.1 - px * 0.6),
            px * 2.0,
            (px, px * 1.2),
            color,
            true,
            Some(NERD),
        );
    }

    fn rect(&self, scene: &mut Scene, rect: Rect, radius: f32, color: [f32; 4], border: f32) {
        gear_grid(scene, self.content).rects.push(RectInst {
            rect,
            radius,
            color: [color[0], color[1], color[2], color[3] * self.a],
            glass: 0.0,
            border,
        });
    }

    /// THE KIT's switch (`kit::SWITCH_*`): off, an outlined capsule with its
    /// knob at the left; on, filled with the accent and its knob at the right
    /// in the accent's own contrast.
    fn switch(&self, scene: &mut Scene, right: f32, cy: f32, on: bool) {
        let s = self.s;
        let (w, h) = (SWITCH_W * s, SWITCH_H * s);
        let sw = Rect::new(right - w, cy - h / 2.0, w, h);
        let i = self.ink;
        let d = crate::kit::SWITCH_KNOB * s;
        let inset = (h - d) / 2.0;
        if on {
            self.rect(scene, sw, h / 2.0, crate::kit::accent(), 0.0);
            self.rect(scene, Rect::new(sw.x + w - d - inset, sw.y + inset, d, d), d / 2.0, crate::kit::on_accent(), 0.0);
        } else {
            self.rect(scene, sw, h / 2.0, [i[0], i[1], i[2], crate::kit::SWITCH_LINE], 1.0);
            self.rect(
                scene,
                Rect::new(sw.x + inset, sw.y + inset, d, d),
                d / 2.0,
                [i[0], i[1], i[2], crate::kit::SWITCH_KNOB_OFF],
                0.0,
            );
        }
    }

    /// The row under the pointer or the keys, tinted (the kit's one hover),
    /// a little in from the box's sides as a menu's row is from its panel.
    fn lit(&self, scene: &mut Scene, r: Rect) {
        let s = self.s;
        let inset = if r.w > 120.0 * s { ROW_INSET * s } else { 0.0 };
        let r = Rect::new(r.x + inset, r.y, r.w - 2.0 * inset, r.h);
        self.rect(scene, r, crate::kit::ROW_RADIUS * s, self.tint, 0.0);
    }

}

impl App {
    /// What the page under the open box is, or `Other` for the ones that still
    /// only show their number.
    pub(crate) fn stats_page_kind(&self) -> PageKind {
        self.stats_kind_of_page(self.stats_page())
    }

    /// The page with content that is on screen, if one is: the one the
    /// CONTROL PANEL hosts (Max, 2026-11-07: "the banner box should be the
    /// quick settings, the more should be on controls" — the Wi-Fi, Bluetooth
    /// and machine pages open as Controls' own settings, on the same engine),
    /// else the box's.
    pub(crate) fn gear_page(&self) -> Option<PageKind> {
        if let Some(kind) = self.gear_host {
            return Some(kind);
        }
        let kind = self.stats_page_kind();
        (self.stats.open && kind != PageKind::Other).then_some(kind)
    }

    /// The pointer the page answers to: the bar's, or the dock's while the
    /// control panel hosts the page.
    fn gear_ptr(&self) -> Option<(f32, f32)> {
        if self.gear_host.is_some() {
            self.pointer_pos
        } else {
            self.options_ptr
        }
    }

    /// Whether the page is far enough out to be touched: the box half open,
    /// or the control panel's setting up.
    fn gear_shown(&self) -> bool {
        if self.gear_host.is_some() {
            self.panel_host_up()
        } else {
            self.stats_open_t() >= 0.5
        }
    }

    /// Leave the page: fold the control panel's setting it is hosted in, or
    /// shut the box.
    fn gear_leave(&mut self) {
        if self.gear_host.is_some() {
            self.panel_escape();
        } else {
            self.set_stats_box(false);
        }
    }

    /// How tall a hosted page is in a window `rect` wide: its strip, its
    /// items and its footer or field (the control panel cuts its card to it).
    pub(crate) fn gear_host_h(&self, rect: Rect) -> f32 {
        let s = self.options_scale();
        let view = self.gear_view();
        // (Each item its own height — an `Empty` its line, not the room
        // left, which is what the card is being sized to.)
        let items: f32 = view.items.iter().map(|i| item_h(i, s, rect.w)).sum();
        let strip_h = if view.strip.is_empty() { 0.0 } else { STRIP_H * s };
        let foot_h = if view.footer.is_empty() && self.gear.field.is_none() && self.gear.arm.is_none() {
            0.0
        } else {
            self.gear_footer_h()
        };
        strip_h + items + foot_h
    }

    /// Whether the open page must not fold when the pointer wanders off: a
    /// pairing in progress, a drive being worked on, a question asked once.
    /// A FIELD does not hold it (Max, 2026-11-05: "cuando estoy escribiendo y
    /// me arrepiento y salgo de la caja o hasta hago click afuera, no se
    /// cierra la caja"): leaving it, or a click off it, puts what was being
    /// typed away with the box.
    pub(crate) fn gear_pins_box(&self) -> bool {
        self.gear_page().is_some()
            && (self.gear.debug_hold
                || self.gear.arm.is_some()
                || self.gear.pages.pins_box()
                || matches!(self.gear.bt_view, BtView::Pair(_)))
    }

    /// The box was opened, closed, or turned to another page: start or stop
    /// what the page needs (its worker's polling, the keyboard) and put the
    /// page back at its list.
    pub(crate) fn gear_sync(&mut self) {
        let page = self.gear_page();
        if page == self.gear.watching {
            return;
        }
        if self.gear.watching == Some(PageKind::Bt) {
            // A search left running would keep the radio busy for nothing.
            if self.gear.bt_snap.discovering {
                self.bt_send(BtCommand::Scan(false));
            }
        }
        self.gear.watching = page;
        self.gear.arm = None;
        self.pages_sync(page);
        self.gear.field = None;
        self.gear.field_t = 0.0;
        self.gear.net_view = NetView::List;
        self.gear.bt_view = BtView::List;
        self.gear.share = false;
        self.gear.show_pw = false;
        self.gear.hit = Hit::None;
        self.gear.scroll = 0.0;
        self.gear.scroll_target = 0.0;
        self.gear.view_t = 0.0;
        let radio = matches!(page, Some(PageKind::Net | PageKind::Bt));
        if radio {
            self.ensure_net();
        }
        if page == Some(PageKind::Bt) {
            self.ensure_bt();
        }
        if let Some(h) = &self.gear.net {
            // Watched from the Bluetooth page too: sharing internet over
            // Bluetooth is NetworkManager's, and its state shows there.
            h.send(NetCommand::Watch(radio));
        }
        if let Some(h) = &self.gear.bt {
            h.send(BtCommand::Watch(page == Some(PageKind::Bt)));
        }
        self.sync_gear_keyboard();
        // The sleep timer stands in the box while it is open, on the banner
        // once it is shut (`timer_pill.rs`).
        self.timer_pill_sync();
    }

    /// The box holds the keyboard on the pages where typing means something
    /// by itself (a search), and anywhere while a field is out.
    fn sync_gear_keyboard(&mut self) {
        let typing = matches!(
            self.gear_page(),
            Some(PageKind::Net | PageKind::Bt | PageKind::Cpu | PageKind::Ram)
        );
        self.set_gear_keyboard(typing || (self.gear_page().is_some() && self.gear.field.is_some()));
    }

    fn ensure_net(&mut self) {
        if self.gear.net.is_some() {
            return;
        }
        let (tx, rx) = calloop::channel::channel::<NetEvent>();
        let _ = self.loop_handle.insert_source(rx, |ev, _, app: &mut App| {
            if let calloop::channel::Event::Msg(e) = ev {
                app.on_net_event(e);
            }
        });
        self.gear.net = Some(net::spawn(tx));
    }

    fn ensure_bt(&mut self) {
        if self.gear.bt.is_some() {
            return;
        }
        let (tx, rx) = calloop::channel::channel::<BtEvent>();
        let _ = self.loop_handle.insert_source(rx, |ev, _, app: &mut App| {
            if let calloop::channel::Event::Msg(e) = ev {
                app.on_bt_event(e);
            }
        });
        self.gear.bt = Some(crate::bt::spawn(tx));
    }

    fn files_send(&mut self, cmd: FileCommand) {
        if self.gear.files.is_none() {
            let (tx, rx) = calloop::channel::channel::<FileEvent>();
            let _ = self.loop_handle.insert_source(rx, |ev, _, app: &mut App| {
                if let calloop::channel::Event::Msg(e) = ev {
                    match e {
                        FileEvent::Progress { address, text } => {
                            app.gear.sending = Some((address, text));
                        }
                        FileEvent::ReceiveFailed => {
                            app.settings.bt_receive = false;
                            app.settings.save();
                        }
                        FileEvent::Done { address } => {
                            if app
                                .gear
                                .sending
                                .as_ref()
                                .is_some_and(|(a, _)| *a == address)
                            {
                                app.gear.sending = None;
                            }
                        }
                    }
                    app.gear_changed();
                }
            });
            self.gear.files = Some(crate::bt_files::spawn(tx));
        }
        if let Some(h) = &self.gear.files {
            h.send(cmd);
        }
    }

    /// At start: what the owner switched on stays on without the page ever
    /// being opened (receiving files is an agent that has to exist).
    pub(crate) fn gear_startup(&mut self) {
        self.pages_startup();
        // The pairing agent exists from the start: a pairing begun on a phone
        // or another computer has to find someone to ask, page open or not.
        // (The worker only waits; it polls nothing until its page is shown.)
        self.ensure_bt();
        if self.settings.bt_receive {
            self.files_send(FileCommand::Receive(true));
        }
    }

    fn net_send(&mut self, cmd: NetCommand) {
        self.ensure_net();
        if let Some(h) = &self.gear.net {
            h.send(cmd);
        }
    }

    fn bt_send(&mut self, cmd: BtCommand) {
        self.ensure_bt();
        if let Some(h) = &self.gear.bt {
            h.send(cmd);
        }
    }

    fn on_net_event(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Snapshot(s) => {
                if let Some(left) = &self.gear.net_leaving {
                    if !s.aps.iter().any(|a| a.active && &a.ssid == left) {
                        self.gear.net_leaving = None;
                    }
                }
                if let Some(busy) = &self.gear.net_busy {
                    if s.aps.iter().any(|a| a.active && &a.ssid == busy) {
                        self.gear.net_busy = None;
                    }
                }
                // Wi-Fi came back by some other hand (a terminal, a key):
                // airplane mode is over, whatever the store still says.
                if self.settings.airplane && s.wifi_on {
                    self.settings.airplane = false;
                    self.settings.save();
                }
                if s == self.gear.net_snap {
                    return;
                }
                self.gear.net_snap = s;
            }
            NetEvent::Detail(d) => {
                if let Some(pw) = &d.password {
                    let secured = self.net_ap(&d.ssid).is_none_or(Ap::secured);
                    self.gear.qr = qr_matrix(&net::wifi_qr_payload(&d.ssid, pw, secured));
                }
                // A read without the secret must not forget one already shown.
                let password = d.password.clone().or_else(|| {
                    self.gear
                        .net_detail
                        .as_ref()
                        .and_then(|o| o.password.clone())
                });
                self.gear.net_detail = Some(NetDetail { password, ..d });
            }
            NetEvent::Failed {
                ssid,
                wrong_password,
            } => {
                info!("net: could not join {ssid:?} (wrong password: {wrong_password})");
                self.gear.net_busy = None;
                self.gear.net_err = Some(ssid.clone());
                self.gear.net_err_password = wrong_password;
                self.gear.net_err_short = false;
                if wrong_password && self.gear_page() == Some(PageKind::Net) {
                    self.gear.net_view = NetView::List;
                    self.open_gear_field(password_field(ssid, false));
                }
            }
            NetEvent::ShareFailed { wifi } => {
                self.gear.hs_busy = false;
                self.gear.share_err = Some(if wifi {
                    (
                        tr("The Wi-Fi hotspot did not start"),
                        tr("Use Bluetooth sharing instead"),
                    )
                } else {
                    (
                        tr("Sharing over Bluetooth did not start"),
                        tr("Check that Bluetooth is on"),
                    )
                });
            }
            NetEvent::Idle => {
                self.gear.net_busy = None;
                self.gear.net_scanning = false;
                self.gear.hs_busy = false;
            }
        }
        self.gear_changed();
    }

    fn on_bt_event(&mut self, ev: BtEvent) {
        match ev {
            BtEvent::Snapshot(s) => {
                if s == self.gear.bt_snap {
                    return;
                }
                self.gear.bt_snap = s;
                // The link is what was asked for: the action is over, whether
                // or not BlueZ has answered the call yet (a Connect to a
                // computer stays unanswered long after it is connected, and
                // the page read "Disconnecting…" all that time).
                if let Some(path) = self.gear.bt_busy.clone() {
                    let now = self.bt_dev(&path).map(|d| d.connected);
                    if self.gear.pair.is_none() && now.is_some_and(|c| c != self.gear.bt_busy_was) {
                        self.gear.bt_busy = None;
                    }
                }
            }
            // (The page is brought up first: opening it starts at its list.)
            BtEvent::PairConfirm { path, code } => {
                self.show_pairing();
                self.gear.bt_view = BtView::Pair(path);
                self.gear.pair = Some((code, true));
                self.gear.view_t = 0.0;
                self.gear_show_view();
            }
            BtEvent::PairShow { path, code } => {
                self.show_pairing();
                self.gear.bt_view = BtView::Pair(path);
                self.gear.pair = Some((code, false));
                self.gear_show_view();
            }
            BtEvent::Done { path, ok } => {
                debug!("bt: {path} done (ok: {ok})");
                if self.gear.bt_busy.as_deref() == Some(path.as_str()) {
                    self.gear.bt_busy = None;
                    // A connect that failed says so on its row.
                    if !ok && !self.gear.bt_busy_was {
                        self.gear.bt_err = Some(path.clone());
                    }
                }
                if self.gear.bt_view == BtView::Pair(path) {
                    self.gear.bt_view = BtView::List;
                    self.gear.pair = None;
                    // The pairing held the box open; with it over, the usual
                    // rule is back (leave, and it leaves).
                    self.update_stats_reveal();
                }
            }
            BtEvent::Audio(a) => self.gear.bt_audio = Some(a),
        }
        self.gear_changed();
    }

    /// A pairing asks something of the person: the Bluetooth page comes up
    /// with the question, wherever they are — the other device started it,
    /// so nobody opened this page (it stays up while the question stands).
    fn show_pairing(&mut self) {
        // (Hosted in Controls the page is up too: a pairing asked there is
        // answered there, not pulled into the banner's box — 2026-11-09.)
        if self.gear_page() == Some(PageKind::Bt) && (self.stats.open || self.gear_host == Some(PageKind::Bt)) {
            return;
        }
        if let Some(page) = self.stats_page_for(PageKind::Bt) {
            self.stats.reveal = true;
            self.stats_open_page(page);
        }
    }

    /// Something the page shows has changed: redraw it if it is on screen.
    pub(crate) fn gear_changed(&mut self) {
        if self.gear_page().is_none() {
            return;
        }
        self.clamp_gear_scroll();
        self.gear.hit = self.gear_hit_at();
        self.schedule_stats_frame();
        self.draw_options();
    }

    fn net_ap(&self, ssid: &str) -> Option<&Ap> {
        self.gear.net_snap.aps.iter().find(|a| a.ssid == ssid)
    }

    fn bt_dev(&self, path: &str) -> Option<&BtDevice> {
        self.gear.bt_snap.devices.iter().find(|d| d.path == path)
    }

    pub(crate) fn search_query(&self) -> String {
        match &self.gear.field {
            Some(f) if f.kind == FieldKind::Search => f.text.trim().to_lowercase(),
            _ => String::new(),
        }
    }

    fn hotspot_name(&self) -> String {
        if self.settings.hotspot_name.is_empty() {
            host_name()
        } else {
            self.settings.hotspot_name.clone()
        }
    }

    // --- the pages -----------------------------------------------------------

    pub(crate) fn gear_view(&self) -> View {
        let mut view = match self.gear_page() {
            Some(PageKind::Net) => self.net_view(),
            Some(PageKind::Bt) => self.bt_view(),
            Some(kind) => self.pages_view(kind),
            None => View::default(),
        };
        // A hosted page at its root: its setting is the way back (the panel's
        // own Escape), not a "‹ Back" that leads nowhere.
        // (Each page knows its root: Wi-Fi's and Bluetooth's own views, not
        // the machine pages' — a network's or a device's details lost their
        // Back, and Connect stood where it was.)
        let at_root = match self.gear_page() {
            Some(PageKind::Net) => self.gear.net_view == NetView::List,
            Some(PageKind::Bt) => self.gear.bt_view == BtView::List,
            _ => self.pages_at_root(),
        };
        if self.gear_host.is_some() && at_root && view.strip.first().is_some_and(|b| b.hit == Hit::Back) {
            view.strip.remove(0);
        }
        view
    }

    /// The compact page both radios share while airplane mode is on.
    fn airplane_view(what: &str) -> View {
        View {
            items: vec![
                Item::Row {
                    hit: Hit::Airplane,
                    more: None,
                    tile: None,
                    title: tr("Airplane mode").into(),
                    sub: tr("Wi-Fi and Bluetooth off").into(),
                    tone: Tone::Normal,
                    trail: Trail::Switch(true),
                    sel: false,
                },
                Item::Empty(crate::i18n::fill(tr("Turn it off to see {what}"), &[("what", &what)])),
            ],
            compact: true,
            ..View::default()
        }
    }

    fn net_view(&self) -> View {
        let g = &self.gear;
        let snap = &g.net_snap;
        match &g.net_view {
            NetView::Detail(ssid) => return self.net_detail_view(ssid),
            NetView::Hotspot => return self.hotspot_view(),
            NetView::List => {}
        }
        if self.settings.airplane {
            return Self::airplane_view(tr("networks"));
        }
        let power = |sub: &str, on: bool| Item::Row {
            hit: Hit::WifiPower,
            more: None,
            tile: None,
            title: tr("Wi-Fi").into(),
            sub: sub.into(),
            tone: Tone::Normal,
            trail: Trail::Switch(on),
            sel: false,
        };
        // (A wired link is told even with no Wi-Fi or Wi-Fi off: a desktop on
        // its cable read "This computer has no Wi-Fi" and nothing more.)
        let wired = snap.wired.as_ref().map(|speed| Item::Row {
            hit: Hit::None,
            more: None,
            tile: Some(G_ETH),
            title: tr("Wired").into(),
            sub: if speed.is_empty() {
                tr("Connected").into()
            } else {
                crate::i18n::fill(tr("Connected · {speed}"), &[("speed", speed)])
            },
            tone: Tone::Normal,
            trail: Trail::Glyph(G_CHECK),
            sel: false,
        });
        let compact = |sub: &str, on: bool, why: &str| {
            let mut items = vec![power(sub, on)];
            items.extend(wired.clone());
            items.push(Item::Empty(why.into()));
            View { items, compact: wired.is_none(), ..View::default() }
        };
        if !snap.reachable {
            return compact(
                tr("Not available"),
                false,
                tr("The network service is not answering"),
            );
        }
        if snap.wifi_dev.is_none() {
            return compact(tr("Not available"), false, tr("This computer has no Wi-Fi"));
        }
        if !snap.wifi_on {
            return compact(tr("Off"), false, tr("Wi-Fi is off"));
        }
        let active = snap.aps.iter().find(|a| a.active);
        let sub = if g.net_busy.is_some() {
            tr("Connecting…").to_owned()
        } else if let Some(a) = active {
            crate::i18n::fill(tr("Connected to {0}"), &[("0", &a.ssid)])
        } else if snap.hotspot_on {
            tr("Paused while the hotspot is on").to_owned()
        } else {
            tr("Not connected").to_owned()
        };
        let mut items = vec![power(&sub, true)];
        let query = self.search_query();
        if query.is_empty() {
            items.extend(wired);
        }
        let mut shown = 0;
        for ap in snap
            .aps
            .iter()
            .filter(|a| query.is_empty() || a.ssid.to_lowercase().contains(&query))
        {
            shown += 1;
            let (sub, tone) = if g.net_leaving.as_deref() == Some(ap.ssid.as_str()) && ap.active {
                (tr("Disconnecting…").to_owned(), Tone::Busy)
            } else if g.net_busy.as_deref() == Some(ap.ssid.as_str()) {
                (tr("Connecting…").to_owned(), Tone::Busy)
            } else if g.net_err.as_deref() == Some(ap.ssid.as_str()) && !ap.active {
                (
                    if g.net_err_short {
                        tr("A password has 8 characters or more")
                    } else if g.net_err_password {
                        tr("Wrong password. Try again")
                    } else {
                        tr("Could not connect. Try again")
                    }
                    .to_owned(),
                    Tone::Warn,
                )
            } else if ap.active && snap.portal {
                (
                    tr("Sign-in needed. Click to open the page").to_owned(),
                    Tone::Warn,
                )
            } else if ap.active {
                (
                    [tr("Connected"), ap.band(), ap.rate.as_str()]
                        .iter()
                        .filter(|p| !p.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" · "),
                    Tone::Normal,
                )
            } else if ap.saved.is_some() {
                (tr("Saved").to_owned(), Tone::Normal)
            } else if ap.secured() {
                (tr("Secured").to_owned(), Tone::Normal)
            } else {
                (tr("Open").to_owned(), Tone::Normal)
            };
            items.push(Item::Row {
                hit: Hit::Net(ap.ssid.clone()),
                more: Some((Hit::NetMore(ap.ssid.clone()), G_GO)),
                tile: Some(wifi_glyph(ap.signal)),
                title: ap.ssid.clone(),
                sub,
                tone,
                trail: if ap.active {
                    Trail::Glyph(G_CHECK)
                } else if ap.secured() {
                    Trail::Glyph(G_LOCK)
                } else {
                    Trail::None
                },
                sel: matches!(&g.field, Some(Field { kind: FieldKind::Password { ssid, .. }, .. }) if *ssid == ap.ssid),
            });
        }
        // The saved ones out of range: there, to be forgotten (THE KIT: what
        // this machine keeps is seen).
        let away: Vec<&String> = snap.away.iter().filter(|s| query.is_empty() || s.to_lowercase().contains(&query)).collect();
        if !away.is_empty() {
            items.push(Item::Heading { text: tr("Saved, not in range").into(), busy: false });
            for ssid in away {
                items.push(Item::Row {
                    hit: Hit::None,
                    more: Some((Hit::NetForgetSaved(ssid.clone()), G_TRASH)),
                    tile: Some(G_WIFI[0]),
                    title: ssid.clone(),
                    sub: tr("Saved").into(),
                    tone: Tone::Normal,
                    trail: Trail::None,
                    sel: false,
                });
            }
        }
        if g.net_scanning {
            items.push(Item::Heading {
                text: tr("Searching…").into(),
                busy: true,
            });
        }
        if shown == 0 && !g.net_scanning {
            items.push(Item::Empty(
                if query.is_empty() {
                    if snap.hotspot_on {
                        tr("The hotspot is using the Wi-Fi radio")
                    } else {
                        tr("No networks in range")
                    }
                } else {
                    tr("No networks match")
                }
                .into(),
            ));
        }
        View {
            items,
            footer: vec![
                // (Lit while it searches, as Bluetooth's: a fill is a state.)
                Btn {
                    hit: Hit::WifiScan,
                    glyph: G_REFRESH,
                    lit: g.net_scanning,
                    ..Default::default()
                },
                btn(Hit::Hidden, G_PLUS),
                Btn {
                    hit: Hit::Hotspot,
                    glyph: G_HOTSPOT,
                    lit: snap.hotspot_on || snap.bt_share_on,
                    ..Default::default()
                },
            ],
            zebra: true,
            ..View::default()
        }
    }

    fn net_detail_view(&self, ssid: &str) -> View {
        let g = &self.gear;
        let detail = g.net_detail.as_ref().filter(|d| d.ssid == ssid);
        let Some(ap) = self.net_ap(ssid) else {
            // It went out of range while its details were open.
            return View {
                strip: vec![btn(Hit::Back, G_BACK)],
                items: vec![Item::Card {
                    glyph: G_WIFI[0],
                    title: ssid.to_owned(),
                    sub: tr("Out of range").into(),
                    extra: Extra::None,
                    rename: None,
                }],
                ..View::default()
            };
        };
        let saved = ap.saved.is_some();
        let mut strip = vec![
            btn(Hit::Back, G_BACK),
            btn(Hit::NetToggle, if ap.active { G_UNLINK } else { G_LINK }),
        ];
        if saved && ap.secured() {
            strip.push(Btn {
                hit: Hit::NetShare,
                glyph: G_QR,
                lit: g.share,
                ..Default::default()
            });
            if g.share {
                strip.push(Btn {
                    hit: Hit::NetShowPw,
                    glyph: G_EYE,
                    lit: g.show_pw,
                    ..Default::default()
                });
            }
        }
        if saved {
            strip.push(btn(Hit::NetForget, G_TRASH));
        }
        let state = if g.net_busy.as_deref() == Some(ssid) {
            tr("Connecting…")
        } else if ap.active {
            tr("Connected")
        } else if saved {
            tr("Saved, not connected")
        } else {
            tr("Not connected")
        };
        let extra = match (&g.qr, g.share) {
            (Some(m), true) => Extra::Qr(m.clone(), tr("Scan with a phone camera to join").into()),
            _ => Extra::None,
        };
        let mut items = vec![Item::Card {
            glyph: wifi_glyph(ap.signal),
            title: ssid.to_owned(),
            sub: state.into(),
            extra,
            rename: None,
        }];
        let kv = |k: &str, v: String| Item::Kv {
            hit: None,
            key: k.into(),
            value: v,
        };
        if g.share {
            let pw = detail.and_then(|d| d.password.clone()).unwrap_or_default();
            items.push(kv(
                tr("Password"),
                if g.show_pw {
                    pw
                } else {
                    "•".repeat(pw.chars().count().max(8))
                },
            ));
        }
        items.push(kv(tr("Signal"), signal_word(ap.signal).into()));
        items.push(kv(
            tr("Security"),
            if ap.secured() {
                ap.security.clone()
            } else {
                tr("None (open)").into()
            },
        ));
        if ap.freq_mhz > 0 {
            items.push(kv(tr("Band"), crate::i18n::fill(tr("{0} · channel {1}"), &[("0", &ap.band()), ("1", &ap.chan)])));
        }
        if ap.active && !ap.rate.is_empty() {
            items.push(kv(tr("Speed"), ap.rate.clone()));
        }
        let manual = detail.is_some_and(|d| d.manual);
        if saved {
            items.push(Item::Toggle {
                hit: Hit::NetAuto,
                label: tr("Connect automatically").into(),
                hint: String::new(),
                on: ap.autoconnect,
            });
            items.push(Item::Toggle {
                hit: Hit::NetMetered,
                label: tr("Metered").into(),
                hint: tr("Golem holds big downloads on this network").into(),
                on: detail.is_some_and(|d| d.metered),
            });
            items.push(Item::Toggle {
                hit: Hit::NetPrivate,
                label: tr("Private address").into(),
                hint: tr("Hides this computer from tracking on public networks").into(),
                on: detail.is_some_and(|d| d.private_mac),
            });
            items.push(Item::Choice {
                key: tr("Address").into(),
                opts: vec![
                    (Hit::NetIp(false), tr("Automatic").into(), !manual),
                    (Hit::NetIp(true), tr("Manual").into(), manual),
                ],
            });
        }
        if let Some(d) = detail.filter(|_| ap.active || manual) {
            for (key, label, value) in [
                (EditKey::Ip, tr("IP address"), &d.ip),
                (EditKey::Gateway, tr("Router"), &d.gateway),
                (EditKey::Dns, "DNS", &d.dns),
            ] {
                if value.is_empty() && !manual {
                    continue;
                }
                items.push(Item::Kv {
                    hit: manual.then_some(Hit::NetEdit(key)),
                    key: label.into(),
                    // "192.168.1.37/24": the part after the slash is the
                    // network's size, which nobody asked about.
                    value: value.split('/').next().unwrap_or("").to_owned(),
                });
            }
            if !d.mac.is_empty() {
                items.push(kv(tr("This computer"), d.mac.clone()));
            }
        }
        View {
            strip,
            items,
            sheet_from: Some(1),
            ..View::default()
        }
    }

    /// Share this computer's internet: as a Wi-Fi hotspot, over Bluetooth, or
    /// both. One page, reached from the hotspot circle of either list.
    fn hotspot_view(&self) -> View {
        let g = &self.gear;
        let wifi_on = g.net_snap.hotspot_on;
        let bt_on = g.net_snap.bt_share_on;
        let name = self.hotspot_name();
        let pass = &self.settings.hotspot_pass;
        let mut items = vec![Item::Card {
            glyph: G_HOTSPOT,
            title: tr("Share internet").into(),
            sub: match (wifi_on, bt_on) {
                (true, true) => crate::i18n::fill(tr("Wi-Fi “{name}” and Bluetooth"), &[("name", &name)]),
                (true, false) => crate::i18n::fill(tr("Wi-Fi hotspot “{name}” is on"), &[("name", &name)]),
                (false, true) => tr("Sharing over Bluetooth").into(),
                (false, false) => tr("Lets other devices online").into(),
            },
            extra: match (&g.qr, wifi_on) {
                (Some(m), true) => Extra::Qr(m.clone(), tr("Scan with a phone camera to join").into()),
                _ => Extra::None,
            },
            rename: None,
        }];
        if let Some((title, sub)) = g.share_err {
            items.push(Item::Row {
                hit: Hit::None,
                more: None,
                tile: None,
                title: title.into(),
                sub: sub.into(),
                tone: Tone::Warn,
                trail: Trail::None,
                sel: false,
            });
        }
        let busy = |hint: &str| {
            if g.hs_busy {
                tr("Working…").to_owned()
            } else {
                hint.to_owned()
            }
        };
        items.push(Item::Toggle {
            hit: Hit::HsPower,
            label: tr("Wi-Fi hotspot").into(),
            hint: busy(if g.net_snap.wifi_dev.is_none() {
                tr("This computer has no Wi-Fi")
            } else if g.net_snap.wired.is_some() {
                tr("Shares the wired connection")
            } else {
                tr("Needs a cable to share from")
            }),
            on: wifi_on,
        });
        items.push(Item::Kv {
            hit: Some(Hit::HsName),
            key: tr("Name").into(),
            value: name,
        });
        items.push(Item::Kv {
            hit: Some(Hit::HsPass),
            key: tr("Password").into(),
            value: if g.show_pw {
                pass.clone()
            } else {
                "•".repeat(pass.chars().count())
            },
        });
        items.push(Item::Choice {
            key: tr("Band").into(),
            opts: vec![
                (
                    Hit::HsBand(false),
                    tr("2.4 GHz").into(),
                    !self.settings.hotspot_5ghz,
                ),
                (
                    Hit::HsBand(true),
                    tr("5 GHz").into(),
                    self.settings.hotspot_5ghz,
                ),
            ],
        });
        let bt = &g.bt_snap;
        items.push(Item::Toggle {
            hit: Hit::BtShare,
            label: tr("Over Bluetooth").into(),
            hint: busy(if !bt.present {
                tr("This computer has no Bluetooth")
            } else if !bt.powered {
                tr("Turn Bluetooth on first")
            } else if bt_on {
                tr("Paired devices can connect now")
            } else {
                tr("Paired devices use this internet")
            }),
            on: bt_on,
        });
        if bt_on {
            items.push(Item::Heading {
                text: crate::i18n::fill(tr("On the phone: Bluetooth, “{0}”, Internet access"), &[("0", &bt.alias)]),
                busy: false,
            });
        }
        View {
            strip: vec![
                btn(Hit::Back, G_BACK),
                Btn {
                    hit: Hit::HsEye,
                    glyph: G_EYE,
                    lit: g.show_pw,
                    ..Default::default()
                },
            ],
            items,
            sheet_from: Some(1),
            ..View::default()
        }
    }

    fn bt_view(&self) -> View {
        let g = &self.gear;
        let snap = &g.bt_snap;
        match &g.bt_view {
            BtView::Detail(path) => return self.bt_detail_view(path),
            BtView::Pair(path) => return self.bt_pair_view(path),
            BtView::Send => return self.bt_send_view(),
            BtView::Share => return self.hotspot_view(),
            BtView::List => {}
        }
        if self.settings.airplane {
            return Self::airplane_view(tr("devices"));
        }
        let power = |sub: String, on: bool, rename: bool| Item::Row {
            hit: Hit::BtPower,
            more: rename.then_some((Hit::BtName, G_PENCIL)),
            tile: None,
            title: tr("Bluetooth").into(),
            sub,
            tone: Tone::Normal,
            trail: Trail::Switch(on),
            sel: false,
        };
        let compact = |sub: &str, why: &str| View {
            items: vec![power(sub.into(), false, false), Item::Empty(why.into())],
            compact: true,
            ..View::default()
        };
        if !snap.reachable {
            return compact(tr("Not available"), tr("The Bluetooth service is not running"));
        }
        if !snap.present {
            return compact(tr("Not available"), tr("This computer has no Bluetooth"));
        }
        if !snap.powered {
            return compact(tr("Off"), tr("Bluetooth is off"));
        }
        let connected = snap.devices.iter().filter(|d| d.connected).count();
        let sub = if snap.discoverable {
            crate::i18n::fill(tr("Visible to others as “{0}”"), &[("0", &snap.alias)])
        } else if connected > 0 {
            crate::i18n::fill(tr("{connected} connected · this computer is “{0}”"), &[("connected", &connected), ("0", &snap.alias)])
        } else {
            crate::i18n::fill(tr("This computer is “{0}”"), &[("0", &snap.alias)])
        };
        let mut items = vec![power(sub, true, true)];
        let query = self.search_query();
        let matches = |d: &&BtDevice| query.is_empty() || d.name.to_lowercase().contains(&query);
        let busy = |d: &BtDevice| g.bt_busy.as_deref() == Some(d.path.as_str());
        for d in snap.devices.iter().filter(|d| d.paired).filter(matches) {
            let (glyph, _) = bt_kind(&d.icon);
            let (sub, tone) = if busy(d) {
                (
                    if g.bt_busy_was {
                        tr("Disconnecting…")
                    } else {
                        tr("Connecting…")
                    }
                    .to_owned(),
                    Tone::Busy,
                )
            } else if d.connected {
                (tr("Connected").to_owned(), Tone::Normal)
            } else if g.bt_err.as_deref() == Some(d.path.as_str()) {
                (tr("Could not connect. Try again").to_owned(), Tone::Warn)
            } else {
                (tr("Paired").to_owned(), Tone::Normal)
            };
            items.push(Item::Row {
                hit: Hit::Dev(d.path.clone()),
                more: Some((Hit::DevMore(d.path.clone()), G_GO)),
                tile: Some(glyph),
                title: d.name.clone(),
                sub,
                tone,
                trail: match (d.connected, d.battery) {
                    (true, Some(b)) => Trail::Battery(b),
                    (true, None) => Trail::Glyph(G_CHECK),
                    _ => Trail::None,
                },
                sel: false,
            });
        }
        let near: Vec<&BtDevice> = snap
            .devices
            .iter()
            .filter(|d| !d.paired && d.nearby)
            .filter(matches)
            .collect();
        if snap.discovering || !near.is_empty() {
            items.push(Item::Heading {
                text: if snap.discovering {
                    tr("Nearby · searching…").into()
                } else {
                    tr("Nearby").into()
                },
                busy: snap.discovering,
            });
        }
        for d in near {
            let (glyph, kind) = bt_kind(&d.icon);
            items.push(Item::Row {
                hit: Hit::Dev(d.path.clone()),
                more: None,
                tile: Some(glyph),
                title: d.name.clone(),
                sub: if busy(d) {
                    tr("Pairing…").into()
                } else {
                    crate::i18n::fill(tr("{kind} · click to pair"), &[("kind", &kind)])
                },
                tone: if busy(d) { Tone::Busy } else { Tone::Normal },
                trail: Trail::None,
                sel: false,
            });
        }
        if items.len() == 1 {
            items.push(Item::Empty(
                if query.is_empty() {
                    tr("No devices yet. Search to find one nearby")
                } else {
                    tr("No devices match")
                }
                .into(),
            ));
        }
        View {
            items,
            footer: vec![
                Btn {
                    hit: Hit::BtScan,
                    glyph: G_REFRESH,
                    lit: snap.discovering,
                    ..Default::default()
                },
                Btn {
                    hit: Hit::BtVisible,
                    // (Not the eye: the eye shows a password. THE KIT: one
                    // symbol, one meaning.)
                    glyph: G_BROADCAST,
                    lit: snap.discoverable,
                    ..Default::default()
                },
                btn(Hit::BtFiles, G_SEND),
            ],
            zebra: true,
            ..View::default()
        }
    }

    fn bt_detail_view(&self, path: &str) -> View {
        let g = &self.gear;
        let Some(d) = self.bt_dev(path) else {
            return View {
                strip: vec![btn(Hit::Back, G_BACK)],
                items: vec![Item::Empty(tr("This device is gone").into())],
                ..View::default()
            };
        };
        let (glyph, kind) = bt_kind(&d.icon);
        let state = if g.bt_busy.as_deref() == Some(path) {
            if g.bt_busy_was {
                tr("Disconnecting…")
            } else {
                tr("Connecting…")
            }
        } else if d.connected {
            tr("Connected")
        } else {
            tr("Paired, not connected")
        };
        let mut items = vec![Item::Card {
            glyph,
            title: d.name.clone(),
            sub: state.into(),
            extra: match (d.connected, d.battery) {
                (true, Some(b)) => Extra::Battery(b),
                _ => Extra::None,
            },
            rename: Some(Hit::DevRename),
        }];
        items.push(Item::Kv {
            hit: None,
            key: tr("Type").into(),
            value: kind.into(),
        });
        if let Some(a) = g.bt_audio.as_ref().filter(|a| a.address == d.address) {
            if a.can_switch() {
                items.push(Item::Choice {
                    key: tr("Sound").into(),
                    opts: vec![
                        (Hit::DevQuality(true), tr("High quality").into(), a.high_quality),
                        (
                            Hit::DevQuality(false),
                            tr("Calls + mic").into(),
                            !a.high_quality,
                        ),
                    ],
                });
            }
            if !a.codec.is_empty() {
                items.push(Item::Kv {
                    hit: None,
                    key: tr("Codec").into(),
                    value: a.codec.clone(),
                });
            }
            // Where the sound goes is a fact with a ✓, not a switch (turning
            // it "off" would say nowhere): the quick settings choose another.
            if a.has_sink() {
                items.push(Item::Row {
                    hit: if a.is_output { Hit::None } else { Hit::DevOutput },
                    more: None,
                    tile: None,
                    title: tr(if a.is_output { "Sound plays here" } else { "Play sound here" }).into(),
                    sub: if a.is_output { String::new() } else { tr("Click to send this computer's sound to it").into() },
                    tone: Tone::Normal,
                    trail: if a.is_output { Trail::Glyph(G_CHECK) } else { Trail::None },
                    sel: false,
                });
            }
        }
        items.push(Item::Toggle {
            hit: Hit::DevAuto,
            label: tr("Connect automatically").into(),
            hint: tr("When it is switched on and in range").into(),
            on: d.trusted,
        });
        items.push(Item::Kv {
            hit: None,
            key: tr("Address").into(),
            value: d.address.clone(),
        });
        View {
            strip: vec![
                btn(Hit::Back, G_BACK),
                btn(Hit::DevToggle, if d.connected { G_UNLINK } else { G_LINK }),
                btn(Hit::DevForget, G_TRASH),
            ],
            items,
            sheet_from: Some(1),
            ..View::default()
        }
    }

    /// Files: take them from paired devices, and send one to a phone or a
    /// computer.
    fn bt_send_view(&self) -> View {
        let g = &self.gear;
        let mut items = vec![
            Item::Card {
                glyph: G_SEND,
                title: tr("Files").into(),
                sub: tr("Between this computer and paired devices").into(),
                extra: Extra::None,
                rename: None,
            },
            Item::Toggle {
                hit: Hit::BtReceive,
                label: tr("Receive files").into(),
                hint: tr("From paired devices, into Downloads").into(),
                on: self.settings.bt_receive,
            },
            Item::Heading {
                text: tr("Send a file to").into(),
                busy: false,
            },
        ];
        let mut any = false;
        for d in g.bt_snap.devices.iter().filter(|d| d.paired && d.files) {
            any = true;
            let sending = g.sending.as_ref().filter(|(a, _)| *a == d.address);
            items.push(Item::Row {
                hit: Hit::SendTo(d.path.clone()),
                more: None,
                tile: Some(bt_kind(&d.icon).0),
                title: d.name.clone(),
                sub: sending.map_or_else(|| tr("Choose a file…").to_owned(), |(_, t)| t.clone()),
                tone: if sending.is_some() {
                    Tone::Busy
                } else {
                    Tone::Normal
                },
                trail: Trail::Glyph(G_SEND),
                sel: false,
            });
        }
        if !any {
            items.push(Item::Empty(tr("No paired phone or computer").into()));
        }
        View {
            strip: vec![btn(Hit::Back, G_BACK)],
            items,
            sheet_from: Some(1),
            ..View::default()
        }
    }

    fn bt_pair_view(&self, path: &str) -> View {
        let (name, glyph) = self
            .bt_dev(path)
            .map_or((tr("the device").to_owned(), G_BT), |d| {
                (d.name.clone(), bt_kind(&d.icon).0)
            });
        let (code, confirm) = self.gear.pair.clone().unwrap_or_default();
        // Read in two groups of three, the way the other screen shows it.
        let spaced = if code.len() == 6 {
            format!("{} {}", &code[..3], &code[3..])
        } else {
            code
        };
        let mut strip = vec![btn(Hit::PairNo, G_TIMES)];
        if confirm {
            strip.push(btn(Hit::PairYes, G_CHECK));
        }
        View {
            strip,
            items: vec![Item::Card {
                glyph,
                sub: if confirm {
                    tr("Does it show the same number?").into()
                } else {
                    tr("Type this on it, then press Enter").into()
                },
                title: name,
                extra: Extra::Code(spaced),
                rename: None,
            }],
            ..View::default()
        }
    }

    // --- layout --------------------------------------------------------------

    fn gear_footer_d(&self) -> f32 {
        self.options_pill_h() * 1.4
    }

    fn gear_footer_h(&self) -> f32 {
        self.gear_footer_d() + 3.0 * PILL_MARGIN_Y
    }

    pub(crate) fn gear_layout(&self, view: &View, rect: Rect) -> Layout {
        let s = self.options_scale();
        // (The band is the box's readout; a hosted page has the setting's
        // title over it instead.)
        let band = if self.gear_host.is_some() { 0.0 } else { self.options_pill_h() };
        let d = self.gear_footer_d();
        let has_field = self.gear.field.is_some();
        let armed = self.gear.arm.is_some();
        let foot_h = if view.footer.is_empty() && !has_field && !armed {
            0.0
        } else {
            self.gear_footer_h()
        };
        let strip_h = if view.strip.is_empty() {
            0.0
        } else {
            STRIP_H * s
        };
        let top = rect.y + band + strip_h;
        let content = Rect::new(
            rect.x,
            top,
            rect.w,
            (rect.y + rect.h - foot_h - top).max(0.0),
        );
        // (Back is words, "‹ Back" — THE KIT's one Back —, as wide as they
        // are; the round buttons follow it.)
        let mut x = rect.x + STRIP_X * s;
        let strip = view
            .strip
            .iter()
            .map(|b| {
                let w = if b.hit == Hit::Back { back_w(s) } else { d };
                let r = Rect::new(x, rect.y + band + (strip_h - d) / 2.0, w, d);
                x += w + STRIP_GAP * s;
                r
            })
            .collect();
        let fy = rect.y + rect.h - foot_h + (foot_h - d) / 2.0;
        let n = view.footer.len() as f32;
        let row_w = n * d + (n - 1.0).max(0.0) * FOOTER_GAP * s;
        // The row is centred on the round buttons; one with words grows to the
        // left from its circle, which stays where it was (under the hand).
        let footer: Vec<Rect> = view
            .footer
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let x = rect.x + (rect.w - row_w) / 2.0 + i as f32 * (d + FOOTER_GAP * s);
                let more = self.gear_btn_words_w(b, d);
                Rect::new(x - more, fy, d + more, d)
            })
            .collect();
        // (Out to the LEFT of their button, which keeps its place, into the
        // places of the buttons there, which step away: nothing moves under
        // the hand as they come.)
        let step = d + FOOTER_GAP * s;
        let footer_behind: Vec<Vec<Rect>> = view
            .footer
            .iter()
            .zip(&footer)
            .enumerate()
            .map(|(i, (b, r))| {
                let out = if self.gear.behind_ix == Some(i) { self.gear.behind_out.clamp(0.0, 1.0) } else { 0.0 };
                (0..b.behind.len())
                    .map(|k| Rect::new(lerp(r.x, r.x - (k as f32 + 1.0) * step, out), r.y, d, d))
                    .collect()
            })
            .collect();
        let footer_covered = footer
            .iter()
            .map(|q: &Rect| {
                view.footer.iter().zip(&footer).position(|(b, r)| {
                    let reach = r.x - b.behind.len() as f32 * step;
                    !b.behind.is_empty() && q.x < r.x && q.x + q.w > reach + 0.5
                })
            })
            .collect();
        let field = (has_field || armed).then(|| {
            let full = Rect::new(
                rect.x + FIELD_PAD * s,
                fy,
                (rect.w - 2.0 * FIELD_PAD * s).max(d),
                d,
            );
            let seat = Rect::new(rect.x + (rect.w - d) / 2.0, fy, d, d);
            let t = if armed { 1.0 } else { self.gear.field_t };
            Rect::new(lerp(seat.x, full.x, t), fy, lerp(seat.w, full.w, t), d)
        });
        // Stack the items; an `Empty` takes whatever height is left.
        let fixed: f32 = view
            .items
            .iter()
            .filter(|i| !matches!(i, Item::Empty(_)))
            .map(|i| item_h(i, s, rect.w))
            .sum();
        let mut y = content.y - self.gear.scroll;
        let mut items = Vec::with_capacity(view.items.len());
        for item in &view.items {
            let h = match item {
                Item::Empty(_) => (content.h - fixed).max(item_h(item, s, rect.w)),
                _ => item_h(item, s, rect.w),
            };
            items.push(Rect::new(rect.x, y, rect.w, h));
            y += h;
        }
        Layout {
            content,
            items,
            strip,
            footer,
            footer_behind,
            footer_covered,
            field,
            total_h: y + self.gear.scroll - content.y,
        }
    }

    /// The box's rect as the page lays out in it: always the settled height,
    /// so nothing inside shifts while the box is still growing.
    fn gear_rect(&self) -> Rect {
        if self.gear_host.is_some() {
            return self.panel_host_rect();
        }
        let r = self.stats_geom();
        // (The settled width too: the box narrows as it opens — `stats_open_w`.)
        Rect::new(r.x, r.y, self.stats_open_w(), self.gear_target_h())
    }

    fn gear_target_h(&self) -> f32 {
        if self.gear_host.is_some() {
            return self.panel_host_rect().h;
        }
        if self.gear_page().is_some() && self.gear_view().compact {
            COMPACT_H * self.options_scale()
        } else {
            self.options_box_drawer_h()
        }
    }

    fn gear_scroll_span(&self) -> f32 {
        let view = self.gear_view();
        let lay = self.gear_layout(&view, self.gear_rect());
        (lay.total_h + 6.0 * self.options_scale() - lay.content.h).max(0.0)
    }

    fn clamp_gear_scroll(&mut self) {
        let span = self.gear_scroll_span();
        self.gear.scroll_target = self.gear.scroll_target.clamp(0.0, span);
        self.gear.scroll = self.gear.scroll.clamp(0.0, span);
    }

    // --- hit-testing ---------------------------------------------------------

    /// What the pointer is on in the open page, now.
    pub(crate) fn gear_hit_now(&self) -> Hit {
        self.gear_hit_at()
    }

    /// Where the pointer stands along the track of the level `hit`, 0..1
    /// (`None`: no such level on the page).
    pub(crate) fn gear_level_at_ptr(&self, hit: &Hit) -> Option<f32> {
        let p = self.gear_ptr()?;
        let view = self.gear_view();
        let lay = self.gear_layout(&view, self.gear_rect());
        let s = self.options_scale();
        view.items.iter().zip(&lay.items).find_map(|(item, r)| match item {
            Item::Level { hit: h, .. } if h == hit => {
                let (_, (x0, x1), _) = level_parts(*r, s);
                Some(((p.0 - x0) / (x1 - x0)).clamp(0.0, 1.0))
            }
            _ => None,
        })
    }

    fn gear_hit_at(&self) -> Hit {
        let (Some(p), Some(_)) = (self.gear_ptr(), self.gear_page()) else {
            return Hit::None;
        };
        if !self.gear_shown() {
            return Hit::None;
        }
        let s = self.options_scale();
        let view = self.gear_view();
        let lay = self.gear_layout(&view, self.gear_rect());
        if let (Some(fr), true) = (lay.field, self.gear.arm.is_some()) {
            if fr.contains(p) {
                return Hit::Confirm;
            }
        } else if let Some(fr) = lay.field {
            if self.gear.field.as_ref().is_some_and(|f| f.secret) && field_eye_rect(fr).contains(p)
            {
                return Hit::FieldEye;
            }
            if fr.contains(p) {
                return Hit::Field;
            }
        } else {
            let out = self.gear.behind_ix.filter(|_| self.gear.behind_out > 0.5);
            for (i, (((b, r), behind), covered)) in view
                .footer
                .iter()
                .zip(&lay.footer)
                .zip(&lay.footer_behind)
                .zip(&lay.footer_covered)
                .enumerate()
            {
                if out == Some(i) {
                    if let Some((bb, _)) = b.behind.iter().zip(behind).find(|(_, br)| br.contains(p)) {
                        return bb.hit.clone();
                    }
                }
                if r.contains(p) && !(out.is_some() && *covered == out) {
                    if let Some(words) = b.label_hit.as_ref().filter(|_| p.0 < Self::words_end(*r)) {
                        return words.clone();
                    }
                    return b.hit.clone();
                }
            }
        }
        for (b, r) in view.strip.iter().zip(&lay.strip) {
            if r.contains(p) {
                return b.hit.clone();
            }
        }
        if !lay.content.contains(p) {
            return Hit::None;
        }
        for (item, r) in view.items.iter().zip(&lay.items) {
            if !r.contains(p) {
                continue;
            }
            return item_zones(item, *r, s)
                .into_iter()
                .find(|(_, z)| z.contains(p))
                .map_or(Hit::None, |(h, _)| h);
        }
        Hit::None
    }

    /// Recompute what the pointer is on; returns whether it changed.
    pub(crate) fn update_gear_hit(&mut self) -> bool {
        let hit = self.gear_hit_at();
        let changed = hit != self.gear.hit;
        // An icon button rested on says its name (THE KIT): a frame is asked
        // for when its wait is over.
        let button = self.gear_button_rect(&hit).map(|_| hit.clone());
        let among = self.gear_ptr().is_some() && self.gear_page().is_some();
        let named = self.gear.names.shown().cloned();
        if let Some(wait) = self.gear.names.follow(button, among) {
            let timer = calloop::timer::Timer::from_duration(wait);
            let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
                app.schedule_stats_frame();
                calloop::timer::TimeoutAction::Drop
            });
        }
        let changed = changed || named != self.gear.names.shown().cloned();
        self.gear.hit = hit;
        let open = self.gear_behind_ptr_on();
        if open != self.gear.behind_open {
            self.gear.behind_open = open;
            // (Another button's: its own come out from nothing.)
            if open.is_some() && open != self.gear.behind_ix {
                self.gear.behind_ix = open;
                self.gear.behind_out = 0.0;
            }
            self.schedule_stats_frame();
            return true;
        }
        changed
    }

    /// Which footer button with buttons behind it the pointer is on, or on
    /// those — the gaps between included, so crossing them is not leaving.
    fn gear_behind_ptr_on(&self) -> Option<usize> {
        let (Some(p), Some(_)) = (self.gear_ptr(), self.gear_page()) else {
            return None;
        };
        if self.gear.field.is_some() || self.gear.arm.is_some() || !self.gear_shown() {
            return None;
        }
        let view = self.gear_view();
        let lay = self.gear_layout(&view, self.gear_rect());
        view.footer.iter().zip(&lay.footer).zip(&lay.footer_behind).position(|((b, r), behind)| {
            let left = behind.iter().map(|x| x.x).fold(r.x, f32::min);
            !b.behind.is_empty() && p.1 >= r.y && p.1 <= r.y + r.h && p.0 >= left && p.0 <= r.x + r.w
        })
    }

    /// How much wider than its circle a footer button is for its words.
    fn gear_btn_words_w(&self, b: &Btn, d: f32) -> f32 {
        match (&b.label, b.glyph.is_empty()) {
            (Some(words), false) => {
                // (Estimated, as the page's rows are: the layout has no
                // renderer to measure with. Close to the symbol: the words
                // end just inside its circle, short of the symbol itself —
                // Max, 2026-11-04: "los minutos un poco más cerca".)
                est_w(words, FONT_PX * self.options_scale()) + WORDS_PAD * d - WORDS_INTO * d
            }
            _ => 0.0,
        }
    }

    /// Where a button's words end and its symbol's circle begins answering
    /// (a little into the circle, where it is empty).
    fn words_end(r: Rect) -> f32 {
        r.x + r.w - r.h + WORDS_INTO * r.h
    }

    pub(crate) fn gear_hit_clickable(&self) -> bool {
        self.gear.hit != Hit::None
    }

    // --- drawing -------------------------------------------------------------

    /// Draw the open page under the band. `a` is the box's own content fade.
    pub(crate) fn push_gear_page(&self, scene: &mut Scene, a: f32) {
        let rect = self.gear_rect();
        let (fill, ink) = self.box_surface_at(rect);
        // The box is still growing: nothing below its current edge is drawn.
        let live = self.stats_geom();
        self.push_gear_page_at(scene, a, rect, live.y + live.h, (fill, ink, self.options_bar_is_bright()));
    }

    /// Draw the page laid out in `rect`, cut at `bottom`, on a `surface` of
    /// (fill, ink, bright — dark ink) — the box's, or the control panel's
    /// card for a hosted page.
    pub(crate) fn push_gear_page_at(&self, scene: &mut Scene, a: f32, rect: Rect, bottom: f32, surface: ([f32; 4], [f32; 4], bool)) {
        let (fill, ink, bright) = surface;
        let s = self.options_scale();
        let view = self.gear_view();
        let lay = self.gear_layout(&view, rect);
        let stripe = self.zebra_stripe(fill);
        let content = Rect::new(
            lay.content.x,
            lay.content.y,
            lay.content.w,
            (lay.content.h).min((bottom - lay.content.y).max(0.0)),
        );
        let pen = Pen {
            s,
            a: a * self.gear.view_t.clamp(0.0, 1.0),
            ink,
            dim: self.dim_ink(ink),
            hot: hover_ink_for(ink),
            tint: crate::kit::row_tint(bright),
            content,
        };
        let hit = &self.gear.hit;

        // The details sheet's second tone, from its first item to the bottom.
        if let Some(r) = view.sheet_from.and_then(|i| lay.items.get(i)) {
            // The zebra's own tone, the way the clipboard's details sheet takes
            // it, and rounded with the box at the bottom (two rects: the upper
            // one squares the top off).
            let top = r.y.max(content.y);
            let bot = (rect.y + rect.h).min(bottom);
            let radius = self.box_radius();
            // (Quiet, 2026-10-16: a breath off the box, as Golem Files' side
            // panel is off its content — not the zebra's full step.)
            let color = [stripe[0], stripe[1], stripe[2], stripe[3] * pen.a * QUIET_SHEET];
            if bot > top {
                scene.rects.push(RectInst {
                    rect: Rect::new(rect.x, top, rect.w, bot - top),
                    radius,
                    color,
                    glass: 0.0,
                    border: 0.0,
                });
                if bot - top > radius {
                    scene.rects.push(RectInst {
                        rect: Rect::new(rect.x, top, rect.w, bot - top - radius),
                        radius: 0.0,
                        color,
                        glass: 0.0,
                        border: 0.0,
                    });
                }
            }
        }

        let mut list_pos = 0usize;
        for (item, r) in view.items.iter().zip(&lay.items) {
            let r = *r;
            if r.y + r.h <= content.y || r.y >= content.y + content.h {
                if matches!(item, Item::Row { .. }) {
                    list_pos += 1;
                }
                continue;
            }
            let zones = item_zones(item, r, s);
            let hovered = zones.iter().any(|(h, _)| h == hit);
            match item {
                Item::Row {
                    more,
                    tile,
                    title,
                    sub,
                    tone,
                    trail,
                    sel,
                    hit: row_hit,
                } => {
                    if crate::options::BOX_ZEBRA && view.zebra && list_pos % 2 == 1 {
                        pen.rect(scene, r, 0.0, stripe, 0.0);
                    }
                    list_pos += 1;
                    let lit = (hovered && (*row_hit != Hit::None || more.is_some())) || *sel;
                    if lit {
                        pen.lit(scene, r);
                    }
                    let col = if lit { pen.hot } else { pen.dim };
                    let mut tx = r.x + ROW_PAD_X * s;
                    if let Some(g) = tile {
                        // A one-line row wears a smaller tile, so it fits the
                        // row's own height — in the same column, so every row's
                        // words begin at one line (an empty tile: only that).
                        let d = if sub.is_empty() { SMALL_TILE } else { TILE } * s;
                        let t = Rect::new(tx + (TILE * s - d) / 2.0, r.y + (r.h - d) / 2.0, d, d);
                        // (The icon alone: nothing behind it at rest.)
                        if !g.is_empty() {
                            pen.glyph(scene, g, (t.x + t.w / 2.0, t.y + t.h / 2.0), d * 0.52, col);
                        }
                        tx += (TILE + TILE_GAP) * s;
                    }
                    // The trail always (what the row IS: on, connected, its
                    // battery…); the corner control beside it, left of it,
                    // while the pointer is on the row; a page of its own: a ›
                    // at the very end, always, the trail left of it.
                    let go = more.as_ref().is_some_and(|(_, g)| *g == G_GO);
                    let right = if go {
                        row_go_rect(r, s).x - 2.0 * s
                    } else {
                        r.x + r.w - TRAIL_PAD * s
                    };
                    let line_cy = r.y + ROW_PAD_Y * s + LINE_PX * s / 2.0;
                    let mut text_right = right;
                    let mut more_left = None;
                    if let (Some((m, _)), true) = (more, go) {
                        let z = row_go_rect(r, s);
                        let on = m == hit;
                        if on {
                            pen.rect(scene, z, z.h / 2.0, [ink[0], ink[1], ink[2], 0.10], 0.0);
                        }
                        pen.glyph(scene, G_GO, (z.x + z.w / 2.0, z.y + z.h / 2.0), FONT_PX * s, if on || lit { pen.hot } else { pen.dim });
                    } else if let (Some((m, g)), true) = (more, hovered) {
                        let z = row_more_rect(r, s, trail, g);
                        let on = m == hit;
                        if on {
                            pen.rect(scene, z, z.h / 2.0, [ink[0], ink[1], ink[2], 0.14], 0.0);
                        }
                        pen.glyph(
                            scene,
                            g,
                            (z.x + z.w / 2.0, z.y + z.h / 2.0),
                            FONT_PX * s,
                            if on { pen.hot } else { pen.dim },
                        );
                        more_left = Some(z.x - 4.0 * s);
                    }
                    {
                        match trail {
                            Trail::None => {}
                            Trail::Glyph(g) => {
                                pen.glyph(scene, g, (right - 9.0 * s, line_cy), 16.0 * s, col);
                                text_right = right - 22.0 * s;
                            }
                            Trail::Switch(on) => {
                                // (Centred on the row, as every switch is.)
                                pen.switch(scene, right - 6.0 * s, r.y + r.h / 2.0, *on);
                                text_right = right - (SWITCH_W + 14.0) * s;
                            }
                            Trail::Text(t) => {
                                let w = est_w(t, 15.0 * s) * 1.25 + 6.0 * s;
                                pen.text(
                                    scene,
                                    t.clone(),
                                    (right - w, r.y + ROW_PAD_Y * s),
                                    w + 4.0,
                                    (15.0 * s, LINE_PX * s),
                                    col,
                                    false,
                                    None,
                                );
                                text_right = right - w - 6.0 * s;
                            }
                            Trail::Battery(b) => {
                                let t = format!("{b}%");
                                let w = est_w(&t, 15.0 * s) + 4.0 * s;
                                pen.text(
                                    scene,
                                    t,
                                    (right - w, r.y + ROW_PAD_Y * s),
                                    w + 4.0,
                                    (15.0 * s, LINE_PX * s),
                                    col,
                                    false,
                                    None,
                                );
                                text_right = right - w - 6.0 * s;
                            }
                        }
                    }
                    // (The words end before the corner control while it shows.)
                    if let Some(l) = more_left {
                        text_right = text_right.min(l);
                    }
                    let max_w = (text_right - tx).max(0.0);
                    let px = (FONT_PX * s, LINE_PX * s);
                    pen.text(
                        scene,
                        fit(title, max_w, px.0),
                        (tx, r.y + ROW_PAD_Y * s),
                        max_w + 4.0,
                        px,
                        col,
                        false,
                        None,
                    );
                    if !sub.is_empty() {
                        let sub_w = (r.x + r.w - ROW_PAD_X * s - tx).max(0.0);
                        let c = match tone {
                            Tone::Warn => crate::kit::warn_on(!bright),
                            _ => [col[0], col[1], col[2], col[3] * 0.72],
                        };
                        pen.text(
                            scene,
                            fit(sub, sub_w, px.0),
                            (tx, r.y + (ROW_PAD_Y + LINE_PX + SUB_GAP) * s),
                            sub_w + 4.0,
                            px,
                            c,
                            false,
                            None,
                        );
                    }
                }
                Item::Graph(vals) => {
                    // A bar per sample, the newest at the right edge.
                    let (x0, w) = (r.x + ROW_PAD_X * s, r.w - 2.0 * ROW_PAD_X * s);
                    let (top, h) = (r.y + 4.0 * s, r.h - 10.0 * s);
                    let n = crate::gear_pages::HIST_LEN as f32;
                    let step = w / n;
                    pen.rect(
                        scene,
                        Rect::new(x0, top + h, w, 1.0),
                        0.0,
                        [ink[0], ink[1], ink[2], 0.18],
                        0.0,
                    );
                    let skip = crate::gear_pages::HIST_LEN.saturating_sub(vals.len());
                    for (i, v) in vals.iter().enumerate() {
                        let bh = (h * (v / 100.0).clamp(0.0, 1.0)).max(1.0);
                        pen.rect(
                            scene,
                            Rect::new(
                                x0 + (skip + i) as f32 * step,
                                top + h - bh,
                                (step - 1.5 * s).max(1.0),
                                bh,
                            ),
                            1.0,
                            [ink[0], ink[1], ink[2], 0.62],
                            0.0,
                        );
                    }
                }
                Item::Cores(vals) => {
                    let (x0, w) = (r.x + ROW_PAD_X * s, r.w - 2.0 * ROW_PAD_X * s);
                    let (top, h) = (r.y + 2.0 * s, r.h - 10.0 * s);
                    let step = w / vals.len().max(1) as f32;
                    for (i, v) in vals.iter().enumerate() {
                        let slot =
                            Rect::new(x0 + i as f32 * step, top, (step - 3.0 * s).max(2.0), h);
                        pen.rect(scene, slot, 2.0, [ink[0], ink[1], ink[2], 0.10], 0.0);
                        let bh = (h * (v / 100.0).clamp(0.0, 1.0)).max(1.0);
                        pen.rect(
                            scene,
                            Rect::new(slot.x, top + h - bh, slot.w, bh),
                            2.0,
                            [ink[0], ink[1], ink[2], 0.55],
                            0.0,
                        );
                    }
                }
                Item::Bar(parts) => {
                    let (x0, w) = (r.x + ROW_PAD_X * s, r.w - 2.0 * ROW_PAD_X * s);
                    let total: f32 = parts.iter().map(|p| p.1.max(0.0)).sum::<f32>().max(1.0);
                    let top = r.y + 6.0 * s;
                    let mut x = x0;
                    for (_, weight, opacity) in parts {
                        let pw = w * weight.max(0.0) / total;
                        if pw >= 1.0 {
                            // (The quiet track: 2 px where it was 8, in the
                            // middle of the same band.)
                            pen.rect(
                                scene,
                                Rect::new(x, top + 3.0 * s, (pw - 2.0 * s).max(1.0), 2.0 * s),
                                1.0 * s,
                                [ink[0], ink[1], ink[2], *opacity],
                                0.0,
                            );
                        }
                        x += pw;
                    }
                    // The legend, wrapping the same way `item_h` counted it.
                    let (mut lx, mut ly) = (0.0, top + 16.0 * s);
                    for (label, _, opacity) in parts {
                        let lw = est_w(label, SMALL_PX * s) * 1.14 + 26.0 * s;
                        if lx > 0.0 && lx + lw > w {
                            lx = 0.0;
                            ly += SMALL_LINE * s;
                        }
                        pen.rect(
                            scene,
                            Rect::new(x0 + lx, ly + 4.5 * s, 8.0 * s, 8.0 * s),
                            2.0 * s,
                            [ink[0], ink[1], ink[2], *opacity],
                            0.0,
                        );
                        pen.text(
                            scene,
                            label.clone(),
                            (x0 + lx + 13.0 * s, ly),
                            lw,
                            (SMALL_PX * s, SMALL_LINE * s),
                            [ink[0], ink[1], ink[2], ink[3] * 0.62],
                            false,
                            None,
                        );
                        lx += lw;
                    }
                }
                Item::Note(text) => pen.text(
                    scene,
                    fit(text, r.w - 2.0 * ROW_PAD_X * s, SMALL_PX * s),
                    (r.x + ROW_PAD_X * s, r.y),
                    r.w - 2.0 * ROW_PAD_X * s + 4.0,
                    (SMALL_PX * s, SMALL_LINE * s),
                    [ink[0], ink[1], ink[2], ink[3] * 0.5],
                    false,
                    None,
                ),
                // (THE KIT's heading: 12.5 px at half the ink.)
                Item::Heading { text, .. } => pen.text(
                    scene,
                    text.clone(),
                    (
                        r.x + ROW_PAD_X * s,
                        r.y + (r.h - SMALL_LINE * s) / 2.0 + 2.0 * s,
                    ),
                    r.w,
                    (crate::kit::HEADING_PX * s, SMALL_LINE * s),
                    [ink[0], ink[1], ink[2], ink[3] * 0.5],
                    false,
                    None,
                ),
                Item::Empty(text) => pen.text(
                    scene,
                    text.clone(),
                    (r.x + r.w / 2.0, r.y + (r.h - LINE_PX * s) / 2.0),
                    r.w - 2.0 * ROW_PAD_X * s,
                    (FONT_PX * s, LINE_PX * s),
                    [ink[0], ink[1], ink[2], ink[3] * 0.55],
                    true,
                    None,
                ),
                Item::Level {
                    hit: track_hit,
                    glyph,
                    glyph_hit,
                    value,
                    word,
                    word_hit,
                    word_on,
                    quiet,
                } => {
                    let (g, (x0, x1), wr) = level_parts(r, s);
                    // The symbol: a button where it does something (mute), a
                    // round wash under the pointer as every icon button.
                    let on_glyph = *glyph_hit != Hit::None && glyph_hit == hit;
                    if on_glyph {
                        pen.rect(scene, g, g.h / 2.0, [ink[0], ink[1], ink[2], 0.10], 0.0);
                    }
                    pen.glyph(
                        scene,
                        glyph,
                        (g.x + g.w / 2.0, g.y + g.h / 2.0),
                        TILE * s * 0.52,
                        if on_glyph { pen.hot } else { pen.dim },
                    );
                    // The track, the accent up to the value, the knob.
                    let cy = r.y + r.h / 2.0;
                    let v = value.clamp(0.0, 1.0);
                    let kx = x0 + (x1 - x0) * v;
                    let held = self.gear.quick.drag.is_some_and(|l| l.hit() == *track_hit);
                    let over = held || track_hit == hit;
                    let line = 2.0 * s;
                    pen.rect(
                        scene,
                        Rect::new(x0, cy - line / 2.0, x1 - x0, line),
                        line / 2.0,
                        [ink[0], ink[1], ink[2], crate::kit::SWITCH_LINE * 0.7],
                        0.0,
                    );
                    let fill = if *quiet { [ink[0], ink[1], ink[2], 0.45] } else { crate::kit::accent() };
                    pen.rect(scene, Rect::new(x0, cy - line / 2.0, kx - x0, line), line / 2.0, fill, 0.0);
                    let k = if over { 14.0 } else { crate::kit::SWITCH_KNOB } * s;
                    let knob = if *quiet { [ink[0], ink[1], ink[2], 0.6] } else { [ink[0], ink[1], ink[2], 0.95] };
                    pen.rect(scene, Rect::new(kx - k / 2.0, cy - k / 2.0, k, k), k / 2.0, knob, 0.0);
                    // The words at the right: the value, or a word that turns.
                    let px = (15.0 * s, LINE_PX * s);
                    let ty = wr.y + (wr.h - px.1) / 2.0;
                    if *word_hit != Hit::None {
                        let over_word = word_hit == hit;
                        if over_word {
                            pen.lit(scene, wr);
                        }
                        if *word_on {
                            let uw = (est_w(word, px.0) * 1.1).min(wr.w - 8.0 * s);
                            pen.rect(
                                scene,
                                Rect::new(wr.x + (wr.w - uw) / 2.0, wr.y + wr.h - 3.0 * s, uw, 1.5 * s),
                                0.0,
                                crate::kit::accent(),
                                0.0,
                            );
                        }
                        let c = if *word_on || over_word { pen.hot } else { [ink[0], ink[1], ink[2], ink[3] * 0.6] };
                        pen.text(scene, word.clone(), (wr.x + wr.w / 2.0, ty), wr.w, px, c, true, None);
                    } else {
                        pen.text(
                            scene,
                            word.clone(),
                            (wr.x + wr.w / 2.0, ty),
                            wr.w + 8.0 * s,
                            px,
                            [ink[0], ink[1], ink[2], ink[3] * 0.72],
                            true,
                            None,
                        );
                    }
                }
                Item::Kv {
                    hit: kv_hit,
                    key,
                    value,
                } => {
                    if hovered {
                        pen.lit(scene, r);
                    }
                    let px = (FONT_PX * s, LINE_PX * s);
                    let y = r.y + (r.h - px.1) / 2.0;
                    let vx = r.x + (ROW_PAD_X + KV_KEY_W + 10.0) * s;
                    let vw = (r.x + r.w
                        - ROW_PAD_X * s
                        - vx
                        - if kv_hit.is_some() { 22.0 * s } else { 0.0 })
                    .max(0.0);
                    pen.text(
                        scene,
                        key.clone(),
                        (r.x + ROW_PAD_X * s, y),
                        KV_KEY_W * s,
                        px,
                        [ink[0], ink[1], ink[2], ink[3] * 0.55],
                        false,
                        None,
                    );
                    pen.text(
                        scene,
                        fit(value, vw, px.0),
                        (vx, y),
                        vw + 4.0,
                        px,
                        if hovered {
                            pen.hot
                        } else {
                            [ink[0], ink[1], ink[2], ink[3] * 0.85]
                        },
                        false,
                        None,
                    );
                    if kv_hit.is_some() {
                        pen.glyph(
                            scene,
                            G_PENCIL,
                            (r.x + r.w - (ROW_PAD_X + 7.0) * s, r.y + r.h / 2.0),
                            15.0 * s,
                            [ink[0], ink[1], ink[2], ink[3] * 0.55],
                        );
                    }
                }
                Item::Toggle {
                    label, hint, on, ..
                } => {
                    // The list's row with its switch (THE KIT: one idiom): its
                    // words where a row's are, the switch where a row's is.
                    if hovered {
                        pen.lit(scene, r);
                    }
                    let col = if hovered { pen.hot } else { pen.dim };
                    let right = r.x + r.w - TRAIL_PAD * s;
                    let tx = r.x + ROW_PAD_X * s;
                    let w = (right - (SWITCH_W + 14.0) * s - tx).max(0.0);
                    let px = (FONT_PX * s, LINE_PX * s);
                    pen.text(scene, fit(label, w, px.0), (tx, r.y + ROW_PAD_Y * s), w + 4.0, px, col, false, None);
                    if !hint.is_empty() {
                        pen.text(
                            scene,
                            fit(hint, w, px.0),
                            (tx, r.y + (ROW_PAD_Y + LINE_PX + SUB_GAP) * s),
                            w + 4.0,
                            px,
                            [col[0], col[1], col[2], col[3] * 0.72],
                            false,
                            None,
                        );
                    }
                    pen.switch(scene, right - 6.0 * s, r.y + r.h / 2.0, *on);
                }
                Item::Choice { key, opts } => {
                    let px = (FONT_PX * s, LINE_PX * s);
                    pen.text(
                        scene,
                        key.clone(),
                        (r.x + ROW_PAD_X * s, r.y + (r.h - px.1) / 2.0),
                        choice_key_w(key, s),
                        px,
                        [ink[0], ink[1], ink[2], ink[3] * 0.55],
                        false,
                        None,
                    );
                    for ((h, label, on), (_, z)) in opts.iter().zip(&zones) {
                        let over = h == hit;
                        // The control panel's words idiom (quiet, 2026-10-16):
                        // a choice is its word; the one in use bright and
                        // underlined, the wash under the pointer.
                        if over {
                            pen.lit(scene, *z);
                        }
                        if *on {
                            // (The kit: the word in use underlined in the accent.)
                            let uw = (est_w(label, 15.0 * s) * 1.1).min(z.w - 8.0 * s);
                            pen.rect(
                                scene,
                                Rect::new(z.x + (z.w - uw) / 2.0, z.y + z.h - 3.0 * s, uw, 1.5 * s),
                                0.0,
                                crate::kit::accent(),
                                0.0,
                            );
                        }
                        pen.text(
                            scene,
                            label.clone(),
                            (z.x + z.w / 2.0, z.y + (z.h - LINE_PX * s) / 2.0),
                            z.w,
                            (15.0 * s, LINE_PX * s),
                            if *on {
                                pen.hot
                            } else {
                                [ink[0], ink[1], ink[2], ink[3] * 0.75]
                            },
                            true,
                            None,
                        );
                    }
                }
                Item::Card {
                    glyph,
                    title,
                    sub,
                    extra,
                    rename,
                } => {
                    let cx = r.x + r.w / 2.0;
                    let mut y = r.y + 6.0 * s;
                    let t = Rect::new(cx - CARD_TILE * s / 2.0, y, CARD_TILE * s, CARD_TILE * s);
                    pen.glyph(scene, glyph, (cx, t.y + t.h / 2.0), 28.0 * s, ink);
                    y += (CARD_TILE + 8.0) * s;
                    let px = (FONT_PX * s, LINE_PX * s);
                    let w = r.w - 2.0 * ROW_PAD_X * s;
                    pen.text(
                        scene,
                        fit(title, w - 60.0 * s, px.0),
                        (cx, y),
                        w,
                        px,
                        ink,
                        true,
                        None,
                    );
                    if rename.is_some() {
                        if let Some((h, z)) = zones.first() {
                            pen.glyph(
                                scene,
                                G_PENCIL,
                                (z.x + z.w / 2.0, z.y + z.h / 2.0),
                                15.0 * s,
                                if h == hit {
                                    pen.hot
                                } else {
                                    [ink[0], ink[1], ink[2], ink[3] * 0.5]
                                },
                            );
                        }
                    }
                    y += (LINE_PX + SUB_GAP) * s;
                    pen.text(
                        scene,
                        fit(sub, w, px.0),
                        (cx, y),
                        w,
                        px,
                        pen.dim,
                        true,
                        None,
                    );
                    y += LINE_PX * s;
                    match extra {
                        Extra::None => {}
                        Extra::Code(code) => pen.text(
                            scene,
                            code.clone(),
                            (cx, y + 8.0 * s),
                            w,
                            (CODE_PX * s, CODE_PX * 1.2 * s),
                            ink,
                            true,
                            Some(NERD),
                        ),
                        Extra::Battery(_) | Extra::Meter(..) => {
                            let (b, line) = match extra {
                                Extra::Meter(b, line) => (b, line.clone()),
                                Extra::Battery(b) => (b, crate::i18n::fill(tr("Battery {b}%"), &[("b", &b)])),
                                _ => unreachable!("matched above"),
                            };
                            let bw = 150.0 * s;
                            let bar = Rect::new(cx - bw / 2.0, y + 11.5 * s, bw, 2.0 * s);
                            pen.rect(scene, bar, bar.h / 2.0, [ink[0], ink[1], ink[2], 0.14], 0.0);
                            let fill_w = bw * f32::from((*b).min(100)) / 100.0;
                            pen.rect(
                                scene,
                                Rect::new(bar.x, bar.y, fill_w.max(bar.h), bar.h),
                                bar.h / 2.0,
                                [ink[0], ink[1], ink[2], 0.8],
                                0.0,
                            );
                            pen.text(
                                scene,
                                line,
                                (cx, bar.y + bar.h + 6.0 * s),
                                w,
                                (SMALL_PX * s, SMALL_LINE * s),
                                [ink[0], ink[1], ink[2], ink[3] * 0.55],
                                true,
                                None,
                            );
                        }
                        Extra::Qr(m, note) => {
                            let side = (QR_SZ + 2.0 * QR_PAD) * s;
                            let plate = Rect::new(cx - side / 2.0, y + 8.0 * s, side, side);
                            // Always dark on white, whatever the box wears: a
                            // camera reads contrast, not the theme.
                            pen.rect(scene, plate, 8.0 * s, [1.0, 1.0, 1.0, 1.0], 0.0);
                            let n = m.len().max(1) as f32;
                            let cell = QR_SZ * s / n;
                            for (ry, row) in m.iter().enumerate() {
                                // One rect per run of dark modules, not per module.
                                let mut x = 0usize;
                                while x < row.len() {
                                    if !row[x] {
                                        x += 1;
                                        continue;
                                    }
                                    let start = x;
                                    while x < row.len() && row[x] {
                                        x += 1;
                                    }
                                    pen.rect(
                                        scene,
                                        Rect::new(
                                            plate.x + QR_PAD * s + start as f32 * cell,
                                            plate.y + QR_PAD * s + ry as f32 * cell,
                                            (x - start) as f32 * cell + 0.3,
                                            cell + 0.3,
                                        ),
                                        0.0,
                                        [0.02, 0.02, 0.02, 1.0],
                                        0.0,
                                    );
                                }
                            }
                            pen.text(
                                scene,
                                note.clone(),
                                (cx, plate.y + side + 6.0 * s),
                                w,
                                (SMALL_PX * s, SMALL_LINE * s),
                                [ink[0], ink[1], ink[2], ink[3] * 0.55],
                                true,
                                None,
                            );
                        }
                    }
                }
            }
        }

        // The round buttons: bar pills, the way the other boxes' footers are —
        // in the BOX's ink: they sit on it, and a shell in its own look (the
        // palette, or Off in Light) over a wallpaper of the other kind left
        // them white on a light box (the Acer, 2026-10-17).
        let glyph_ink = pen.ink;
        // `upto`: a button coming out from behind another (leftward) keeps its
        // symbol off that one (symbols are drawn in one late pass, over every
        // fill).
        let round = |scene: &mut Scene, b: &Btn, r: Rect, alpha: f32, upto: Option<f32>| {
            if r.y + r.h > bottom || alpha <= 0.01 {
                return;
            }
            let over = b.hit == *hit;
            let br = if over { hover_grow(r) } else { r };
            let radius = br.h / 2.0;
            // (Quiet, 2026-10-16: a round button is its icon; the wash under
            // the pointer, or while it is the one that is on.)
            if over || b.lit {
                let mut base = self.options_hover_wash();
                base[3] *= alpha;
                // (Words beside a symbol are not part of the button's ground:
                // the wash is the symbol's circle alone — no pill; Max,
                // 2026-11-04.)
                let ground = if b.label.is_some() && !b.glyph.is_empty() {
                    Rect::new(br.x + br.w - br.h, br.y, br.h, br.h)
                } else {
                    br
                };
                scene.rects.push(RectInst { rect: ground, radius, color: base, glass: 0.0, border: 0.0 });
            }
            let end = upto.unwrap_or(f32::MAX).min(br.x + br.w + 4.0);
            let clip = Rect::new(br.x - 4.0, br.y - 4.0, (end - br.x + 4.0).max(0.0), br.h + 8.0);
            let ink = [glyph_ink[0], glyph_ink[1], glyph_ink[2], glyph_ink[3] * alpha];
            // The words: beside the symbol, which keeps the circle at the
            // right end; alone, centred and a size down.
            if let Some(words) = &b.label {
                let alone = b.glyph.is_empty();
                let px = if alone { r.h * 0.36 } else { FONT_PX * s };
                let at = if alone { br.x + br.w / 2.0 } else { br.x + WORDS_PAD * r.h };
                // Words that answer a click: underlined under the pointer (the
                // quiet language's value, a word underlined).
                if !alone && b.label_hit.as_ref() == Some(hit) {
                    let line = Rect::new(at, br.y + br.h / 2.0 + px * 0.62, est_w(words, px), 1.5 * s);
                    scene.rects.push(RectInst {
                        rect: line,
                        radius: 0.75 * s,
                        color: [ink[0], ink[1], ink[2], ink[3] * 0.55],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                scene.labels.push(Label {
                    text: words.clone(),
                    pos: (at, br.y + (br.h - px * 1.2) / 2.0),
                    max_w: br.w,
                    font_px: px,
                    line_px: px * 1.2,
                    centered: alone,
                    dim: false,
                    cache: false,
                    family: None,
                    color: Some(ink),
                    clip: Some(clip),
                });
                if alone {
                    return;
                }
            }
            let circle = Rect::new(br.x + br.w - br.h, br.y, br.h, br.h);
            let gpx = r.h * 0.56;
            scene.labels.push(Label {
                text: b.glyph.to_owned(),
                pos: (circle.x + circle.w / 2.0, circle.y + (circle.h - gpx * 1.2) / 2.0),
                max_w: br.w + 16.0,
                font_px: gpx,
                line_px: gpx * 1.2,
                centered: true,
                dim: false,
                cache: true,
                family: Some(NERD),
                color: Some(ink),
                clip: Some(clip),
            });
        };
        for (b, r) in view.strip.iter().zip(&lay.strip) {
            if b.hit == Hit::Back {
                // "‹ Back": its words, the wash under the pointer.
                if r.y + r.h <= bottom {
                    if b.hit == *hit {
                        let mut base = self.options_hover_wash();
                        base[3] *= pen.a;
                        scene.rects.push(RectInst { rect: *r, radius: r.h / 2.0, color: base, glass: 0.0, border: 0.0 });
                    }
                    let px = FONT_PX * s;
                    let c = if b.hit == *hit { pen.hot } else { pen.dim };
                    // (Over the list, not in it: the pen's clip is the list's.)
                    scene.labels.push(Label {
                        text: tr("‹ Back").to_owned(),
                        pos: (r.x + BACK_PAD * s, r.y + (r.h - LINE_PX * s) / 2.0),
                        max_w: r.w,
                        font_px: px,
                        line_px: LINE_PX * s,
                        centered: false,
                        dim: false,
                        cache: true,
                        family: None,
                        color: Some([c[0], c[1], c[2], c[3] * pen.a]),
                        clip: Some(Rect::new(r.x - 4.0, r.y - 4.0, r.w + 8.0, r.h + 8.0)),
                    });
                }
                continue;
            }
            round(scene, b, *r, pen.a, None);
        }
        let t = if self.gear.arm.is_some() {
            1.0
        } else if self.gear.field.is_some() {
            self.gear.field_t
        } else {
            0.0
        };
        let out = ((self.gear.behind_out - 0.15) / 0.6).clamp(0.0, 1.0);
        for (i, (((b, r), behind), covered)) in view
            .footer
            .iter()
            .zip(&lay.footer)
            .zip(&lay.footer_behind)
            .zip(&lay.footer_covered)
            .enumerate()
        {
            let mine = self.gear.behind_ix == Some(i);
            // What they come out over steps away as they come.
            let own = if covered.is_some() && *covered == self.gear.behind_ix { 1.0 - out } else { 1.0 };
            // The ones behind first: their button covers them as they come out.
            let mut edge = r.x;
            for (bb, br) in b.behind.iter().zip(behind) {
                let a = if mine { out } else { 0.0 };
                round(scene, bb, *br, pen.a * (1.0 - t) * a, Some(edge));
                edge = br.x;
            }
            round(scene, b, *r, pen.a * (1.0 - t) * own, None);
        }
        if let (Some(fr), Some((text, _))) = (lay.field, &self.gear.arm) {
            if fr.y + fr.h <= bottom {
                // The footer as a question (THE KIT: what cannot be undone is
                // asked in place, in red): the field's own stadium on the red's
                // quiet ground, saying what a second click does.
                let over = self.gear.hit == Hit::Confirm;
                let radius = fr.h / 2.0;
                let red = crate::kit::danger(!bright);
                let mut wash = crate::kit::danger_tint(!bright);
                if over {
                    wash[3] *= 1.6;
                }
                wash[3] *= a;
                scene.rects.push(RectInst {
                    rect: fr,
                    radius,
                    color: wash,
                    glass: 0.0,
                    border: 0.0,
                });
                let px = 15.0 * s;
                scene.labels.push(Label {
                    text: fit(text, fr.w - 2.0 * PILL_PAD_X, px),
                    pos: (fr.x + fr.w / 2.0, fr.y + (fr.h - LINE_PX * s) / 2.0),
                    max_w: fr.w - PILL_PAD_X,
                    font_px: px,
                    line_px: LINE_PX * s,
                    centered: true,
                    dim: false,
                    cache: false,
                    family: None,
                    color: Some([red[0], red[1], red[2], a]),
                    clip: Some(fr),
                });
            }
        } else if let (Some(fr), Some(f)) = (lay.field, &self.gear.field) {
            if fr.y + fr.h <= bottom {
                self.push_gear_field(scene, fr, f, a, bright);
            }
        }
        // The name of the icon button rested on (THE KIT): under a strip's,
        // over a footer's.
        if let Some(h) = self.gear.names.shown() {
            if let (Some((at, above)), Some(words)) = (self.gear_button_rect(h), self.gear_hit_name(h)) {
                let r = crate::kit::name_rect(at, &words, above, rect, s);
                crate::kit::push_name(scene, r, &words, a, s);
            }
        }
    }

    /// Where the icon button `hit` stands on the open page — the strip's (its
    /// name goes under it) or the footer's, those behind included (over it).
    fn gear_button_rect(&self, hit: &Hit) -> Option<(Rect, bool)> {
        if *hit == Hit::None {
            return None;
        }
        let view = self.gear_view();
        let lay = self.gear_layout(&view, self.gear_rect());
        if let Some((_, r)) = view.strip.iter().zip(&lay.strip).find(|(b, _)| b.hit == *hit) {
            return Some((*r, false));
        }
        // (While the footer is a field or a question, it has no buttons.)
        if lay.field.is_some() {
            return None;
        }
        for ((b, r), behind) in view.footer.iter().zip(&lay.footer).zip(&lay.footer_behind) {
            // A button with words beside it (the timer's countdown) is named by them.
            if b.hit == *hit && b.label.is_none() {
                return Some((*r, true));
            }
            if let Some((_, br)) = b.behind.iter().zip(behind).find(|(bb, _)| bb.hit == *hit) {
                return Some((*br, true));
            }
        }
        None
    }

    /// What an icon button of the box does, in words (its name).
    fn gear_hit_name(&self, hit: &Hit) -> Option<String> {
        use crate::gear_pages::SysHit as S;
        let g = &self.gear;
        let ap_on = |s: &str| self.net_ap(s).is_some_and(|a| a.active);
        let words = match hit {
            Hit::NetToggle => match &g.net_view {
                NetView::Detail(ssid) if ap_on(ssid) => tr("Disconnect"),
                _ => tr("Connect"),
            },
            Hit::NetShare => tr("Share this network"),
            Hit::NetShowPw | Hit::HsEye if g.show_pw => tr("Hide the password"),
            Hit::NetShowPw | Hit::HsEye => tr("Show the password"),
            Hit::NetForget | Hit::DevForget => tr("Forget"),
            Hit::DevToggle => match &g.bt_view {
                BtView::Detail(path) if self.bt_dev(path).is_some_and(|d| d.connected) => tr("Disconnect"),
                _ => tr("Connect"),
            },
            Hit::PairNo => tr("Cancel"),
            Hit::PairYes => tr("It matches: pair"),
            Hit::WifiScan => tr("Search again"),
            Hit::Hidden => tr("Join a hidden network…"),
            Hit::Hotspot => tr("Share internet"),
            Hit::BtScan if g.bt_snap.discovering => tr("Stop searching"),
            Hit::BtScan => tr("Search for devices"),
            Hit::BtVisible if g.bt_snap.discoverable => tr("Stop being visible"),
            Hit::BtVisible => tr("Be visible to others"),
            Hit::BtFiles => tr("Send or receive files"),
            Hit::Sys(S::SleepTimer) if self.sleep_timer_left().is_some() => tr("Cancel the timer"),
            Hit::Sys(S::SleepTimer) => tr("Sleep timer"),
            Hit::Sys(S::Lock) => tr("Lock"),
            Hit::Sys(S::AskHibernate) => tr("Hibernate"),
            Hit::Sys(S::Sleep) => tr("Sleep"),
            Hit::Sys(S::AskRestart) => tr("Restart"),
            Hit::Sys(S::AppClose(_)) => tr("Close"),
            Hit::Sys(S::AskKill(_)) => tr("Force quit"),
            Hit::Sys(S::OpenFolder(_)) => tr("Show in Files"),
            Hit::Sys(S::OpenDrive(_)) => tr("Open"),
            Hit::Sys(S::Eject(_)) => tr("Eject"),
            _ => return None,
        };
        Some(words.to_owned())
    }

    /// The footer stretched into a field: the clipboard's search field, with a
    /// leading symbol that says what is being typed.
    fn push_gear_field(&self, scene: &mut Scene, fr: Rect, f: &Field, a: f32, bright: bool) {
        let s = self.options_scale();
        let t = self.gear.field_t.clamp(0.0, 1.0);
        let radius = fr.h / 2.0;
        push_neumorph(scene, fr, radius, bright, a);
        let mut wash = self.options_rest_wash();
        wash[3] *= a;
        scene.rects.push(RectInst {
            rect: fr,
            radius,
            color: wash,
            glass: 0.0,
            border: 0.0,
        });
        // The contents arrive once the stretch is mostly done.
        let ta = ((t - 0.45) / 0.55).clamp(0.0, 1.0) * a;
        if ta <= 0.01 {
            return;
        }
        let (_, ink) = self.box_surface_at(fr);
        let dim = self.dim_ink(ink);
        let font = FONT_PX * s;
        let cy = fr.y + (fr.h - LINE_PX * s) / 2.0;
        let gx = fr.x + PILL_PAD_X;
        let mut label = |text: String, x: f32, w: f32, col: [f32; 4], family, px: f32| {
            scene.labels.push(Label {
                text,
                pos: (x, cy),
                max_w: w,
                font_px: px,
                line_px: LINE_PX * s,
                centered: false,
                dim: false,
                cache: false,
                family,
                color: Some([col[0], col[1], col[2], col[3] * ta]),
                clip: Some(fr),
            });
        };
        label(
            f.glyph.to_owned(),
            gx,
            font * 2.0,
            dim,
            Some(NERD),
            font * 0.95,
        );
        let tx = gx + font * 1.7;
        let right = if f.secret {
            field_eye_rect(fr).x - 4.0 * s
        } else {
            fr.x + fr.w - PILL_PAD_X
        };
        let shown = if f.secret && !f.show {
            "•".repeat(f.text.chars().count())
        } else {
            f.text.clone()
        };
        let max_w = (right - tx).max(0.0);
        // A long entry keeps its END in view: that is where the caret is.
        let mut visible = shown.clone();
        while est_w(&visible, font) > max_w && !visible.is_empty() {
            visible.remove(0);
        }
        if f.text.is_empty() {
            label(
                crate::i18n::tr_dyn(&f.prompt).to_owned(),
                tx,
                max_w,
                dim,
                None,
                font,
            );
        } else {
            label(visible.clone(), tx, max_w + 4.0, ink, None, font);
        }
        if f.secret {
            let e = field_eye_rect(fr);
            let over = self.gear.hit == Hit::FieldEye;
            scene.labels.push(Label {
                text: G_EYE.to_owned(),
                pos: (e.x + e.w / 2.0, cy),
                max_w: e.w + 8.0,
                font_px: font * 0.95,
                line_px: LINE_PX * s,
                centered: true,
                dim: false,
                cache: true,
                family: Some(NERD),
                color: Some(if over || f.show {
                    [ink[0], ink[1], ink[2], ta]
                } else {
                    [dim[0], dim[1], dim[2], dim[3] * ta]
                }),
                clip: Some(fr),
            });
        }
        if self.gear.keyboard_held {
            // A bullet is wider than the average letter the estimate assumes.
            let wide = if f.secret && !f.show { 1.16 } else { 1.0 };
            let cw = if f.text.is_empty() {
                0.0
            } else {
                est_w(&visible, font) * wide
            };
            scene.rects.push(RectInst {
                rect: Rect::new(
                    (tx + cw + 2.0).min(right),
                    fr.y + fr.h * 0.26,
                    2.0,
                    fr.h * 0.48,
                ),
                radius: 1.0,
                color: [ink[0], ink[1], ink[2], ink[3] * ta * 0.8],
                glass: 0.0,
                border: 0.0,
            });
        }
    }

    // --- animation -----------------------------------------------------------

    /// Advance the page's own motion one frame; returns whether it still moves.
    pub(crate) fn gear_tick(&mut self, dt: f32) -> bool {
        if self.gear_page().is_none() {
            self.gear.box_h = 0.0;
            return false;
        }
        let mut moving = false;
        let target = self.gear_target_h();
        if self.gear.box_h <= 0.0 {
            self.gear.box_h = target;
        } else {
            let span = (target - self.gear.box_h).abs().max(1.0);
            let (h, m) = ease_toward(self.gear.box_h, target, dt, MORPH_RATE, 0.5);
            self.gear.box_h = h;
            moving |= m && span > 0.5;
        }
        let (v, m) = ease_toward(self.gear.view_t, 1.0, dt, MORPH_RATE, SETTLE_ALPHA);
        self.gear.view_t = v;
        moving |= m;
        if self.gear.field.is_some() {
            let w = self.gear_rect().w;
            let (t, m) = ease_toward(self.gear.field_t, 1.0, dt, MORPH_RATE, settle_t(w));
            self.gear.field_t = t;
            moving |= m;
        }
        let behind_to = f32::from(u8::from(self.gear.behind_open.is_some() && self.gear.behind_open == self.gear.behind_ix));
        let (o, m) = ease_toward(self.gear.behind_out, behind_to, dt, MORPH_RATE, settle_t(2.0 * self.gear_footer_d()));
        self.gear.behind_out = o;
        moving |= m;
        let (sc, m) = ease_toward(
            self.gear.scroll,
            self.gear.scroll_target,
            dt,
            SCROLL_RATE,
            0.5,
        );
        self.gear.scroll = sc;
        moving |= m;
        if m {
            self.gear.hit = self.gear_hit_at();
        }
        moving
    }

    /// The wheel inside an open page: a level under the pointer is set by it,
    /// else the list travels.
    pub(crate) fn gear_axis(&mut self, value: f32) {
        if self.quick_wheel(value) {
            return;
        }
        let span = self.gear_scroll_span();
        self.gear.scroll_target = (self.gear.scroll_target + value * SCROLL_SPEED).clamp(0.0, span);
        self.schedule_stats_frame();
    }

    // --- the keyboard --------------------------------------------------------

    /// Take or release the keyboard for the page — the clipboard box's grab,
    /// for the same reason: a layer surface only gets keys while it holds it.
    fn set_gear_keyboard(&mut self, want: bool) {
        if want == self.gear.keyboard_held {
            return;
        }
        // Hosted in the control panel: the dock's box holds the keyboard
        // already; only who answers the keys changes (`gear_wants_keys`).
        if self.gear_host.is_some() {
            self.gear.keyboard_held = want;
            return;
        }
        // (The bar's surface is shared: the clipboard box may hold it too.)
        let others = self.clip.keyboard_held
            || self.phone_box_keys()
            || self.timer_pill_keys()
            || self
                .desktop
                .config
                .as_ref()
                .is_some_and(|c| c.hosted && c.editing);
        if want {
            self.cancel_keyboard_handback(crate::KbSurface::Options);
        } else if !others && !self.keyboard_handback_armed(crate::KbSurface::Options) {
            self.begin_keyboard_handback(crate::KbSurface::Options, None);
        }
        self.gear.keyboard_held = want;
        debug!("gear: keyboard grab {}", if want { "on" } else { "off" });
        let any = self.options_keys_wanted();
        if let Some(layer) = &self.options_layer {
            crate::surface::set_interactive(layer, any);
        }
    }

    /// A field of the box waits for typing: a click off the box puts the box
    /// away at once (the field holds the keyboard exclusively, so the click
    /// comes to the bar wherever it lands). Whether it did.
    pub(crate) fn gear_click_off(&mut self, on_box: bool) -> bool {
        if on_box || self.gear_host.is_some() || self.gear.field.is_none() || self.gear_page().is_none() || self.gear.debug_hold {
            return false;
        }
        self.stats_put_away();
        true
    }

    /// Whether keys belong to the gear page right now.
    pub(crate) fn gear_wants_keys(&self) -> bool {
        self.gear.keyboard_held && self.gear_page().is_some()
    }

    fn open_gear_field(&mut self, f: Field) {
        self.gear.field = Some(f);
        self.gear.field_t = 0.0;
        self.gear.arm = None;
        self.sync_gear_keyboard();
        self.schedule_stats_frame();
        self.draw_options();
    }

    fn close_gear_field(&mut self) {
        self.gear.field = None;
        self.gear.field_t = 0.0;
        self.sync_gear_keyboard();
        self.clamp_gear_scroll();
        // The field held the box open; the usual leave rule is back.
        self.update_stats_reveal();
        self.draw_options();
    }

    pub(crate) fn gear_key(&mut self, keysym: Keysym, utf8: Option<&str>) {
        match keysym {
            Keysym::Escape => {
                // One layer at a time: the question, the field, the details,
                // then the box.
                if self.gear.arm.take().is_some() {
                    self.gear_changed();
                } else if self.gear.field.is_some() {
                    self.close_gear_field();
                } else if self.pages_back() {
                    self.gear_changed();
                } else if self.gear_page() == Some(PageKind::Net)
                    && self.gear.net_view != NetView::List
                {
                    self.gear_click(Hit::Back);
                } else if self.gear_page() == Some(PageKind::Bt)
                    && self.gear.bt_view != BtView::List
                {
                    let hit = if matches!(self.gear.bt_view, BtView::Pair(_)) {
                        Hit::PairNo
                    } else {
                        Hit::Back
                    };
                    self.gear_click(hit);
                } else {
                    self.gear_leave();
                }
            }
            Keysym::Return | Keysym::KP_Enter => {
                if self.gear.field.is_some() {
                    self.submit_gear_field();
                } else if matches!(self.gear.pair, Some((_, true))) {
                    self.gear_click(Hit::PairYes);
                }
            }
            Keysym::BackSpace => {
                if let Some(f) = &mut self.gear.field {
                    f.text.pop();
                    self.after_gear_edit();
                }
            }
            _ => {
                let Some(text) = utf8.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
                else {
                    return;
                };
                self.gear_type(text);
            }
        }
    }

    /// Typed or pasted text: into the field, or — on a list — the start of a
    /// search, the way typing over the clipboard list is.
    pub(crate) fn gear_type(&mut self, text: &str) {
        if self.gear.field.is_none() {
            let on_list = match self.gear_page() {
                Some(PageKind::Net) => self.gear.net_view == NetView::List,
                Some(PageKind::Bt) => self.gear.bt_view == BtView::List,
                _ => self.pages_can_search(),
            };
            if !on_list || self.gear_view().compact || text.trim().is_empty() {
                return;
            }
            let prompt = match self.gear_page() {
                Some(PageKind::Net) => tr("Search networks…"),
                Some(PageKind::Bt) => tr("Search devices…"),
                _ => tr("Search apps…"),
            };
            self.open_gear_field(Field {
                kind: FieldKind::Search,
                text: String::new(),
                prompt: prompt.into(),
                glyph: G_SEARCH,
                secret: false,
                show: false,
            });
        }
        if let Some(f) = &mut self.gear.field {
            f.text.push_str(text);
        }
        self.after_gear_edit();
    }

    fn after_gear_edit(&mut self) {
        if matches!(&self.gear.field, Some(f) if f.kind == FieldKind::Search) {
            self.gear.scroll = 0.0;
            self.gear.scroll_target = 0.0;
        }
        self.gear.net_err = None;
        self.draw_options();
    }

    fn submit_gear_field(&mut self) {
        let Some(f) = self.gear.field.clone() else {
            return;
        };
        let text = f.text.trim().to_owned();
        match f.kind {
            FieldKind::Search => {
                // Enter takes the first match, like the clipboard's search —
                // with nothing typed, nothing (it was the first row: the
                // network in use, disconnected).
                if text.is_empty() {
                    self.close_gear_field();
                    return;
                }
                let first =
                    self.gear_view().items.into_iter().find_map(|i| match i {
                        Item::Row {
                            hit:
                                h @ (Hit::Net(_)
                                | Hit::Dev(_)
                                | Hit::Sys(crate::gear_pages::SysHit::App(_))),
                            ..
                        } => Some(h),
                        _ => None,
                    });
                self.close_gear_field();
                if let Some(h) = first {
                    self.gear_click(h);
                }
            }
            FieldKind::Password { ssid, hidden } => {
                // WPA passwords are 8 characters at least; anything shorter
                // cannot be right, so it is refused here without a round trip.
                if f.text.chars().count() < 8 {
                    self.gear.net_err = Some(ssid);
                    self.gear.net_err_password = true;
                    self.gear.net_err_short = true;
                    if let Some(f) = &mut self.gear.field {
                        f.text.clear();
                    }
                    self.draw_options();
                    return;
                }
                self.gear.net_err = None;
                self.gear.net_busy = Some(ssid.clone());
                self.net_send(NetCommand::Connect {
                    ssid,
                    password: Some(f.text),
                    hidden,
                });
                self.close_gear_field();
            }
            FieldKind::Page(what) => {
                self.close_gear_field();
                self.pages_field_submit(what, f.text);
            }
            FieldKind::HiddenSsid => {
                if text.is_empty() {
                    self.close_gear_field();
                } else {
                    self.open_gear_field(password_field(text, true));
                }
            }
            FieldKind::Edit(key) => {
                if let (NetView::Detail(ssid), Some(d), false) = (
                    self.gear.net_view.clone(),
                    self.gear.net_detail.clone(),
                    text.is_empty(),
                ) {
                    let pick = |k: EditKey, old: &str| {
                        if k == key {
                            text.clone()
                        } else {
                            old.to_owned()
                        }
                    };
                    self.net_send(NetCommand::Manual {
                        ssid,
                        ip: pick(EditKey::Ip, &d.ip),
                        gateway: pick(EditKey::Gateway, &d.gateway),
                        dns: pick(EditKey::Dns, &d.dns),
                    });
                }
                self.close_gear_field();
            }
            FieldKind::Rename(path) => {
                if !text.is_empty() {
                    self.bt_send(BtCommand::Rename(path, text));
                }
                self.close_gear_field();
            }
            FieldKind::AdapterName => {
                if !text.is_empty() {
                    self.bt_send(BtCommand::AdapterAlias(text));
                }
                self.close_gear_field();
            }
            FieldKind::HsName => {
                if !text.is_empty() {
                    self.settings.hotspot_name = text;
                    self.settings.save();
                }
                self.close_gear_field();
            }
            FieldKind::HsPass => {
                if f.text.chars().count() >= 8 {
                    self.settings.hotspot_pass = f.text;
                    self.settings.save();
                    self.close_gear_field();
                } else if let Some(f) = &mut self.gear.field {
                    // (Too short: said where it is typed, the field kept.)
                    f.prompt = tr("A password has 8 characters or more").to_owned();
                    f.text.clear();
                    self.schedule_stats_frame();
                }
            }
        }
    }

    // --- acting --------------------------------------------------------------

    /// Ask once before something that cannot be undone: the footer says
    /// `text`, and a click on it does `then`.
    pub(crate) fn gear_arm(&mut self, text: &str, then: Hit) {
        self.gear.field = None;
        self.gear.arm = Some((text.to_owned(), then));
        // (The field is gone: the keys it held go back.)
        self.sync_gear_keyboard();
        self.schedule_stats_frame();
        // THE KIT: a question asked in place runs out by itself.
        let asked = text.to_owned();
        let timer = calloop::timer::Timer::from_duration(crate::kit::ASK_FOR);
        let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            if app.gear.arm.as_ref().is_some_and(|(t, _)| *t == asked) {
                app.gear.arm = None;
                app.update_stats_reveal();
                app.schedule_stats_frame();
            }
            calloop::timer::TimeoutAction::Drop
        });
    }

    /// Ask a machine page's question in the footer field.
    pub(crate) fn gear_ask(
        &mut self,
        what: crate::gear_pages::PField,
        prompt: &str,
        glyph: &'static str,
        secret: bool,
        text: String,
    ) {
        self.open_gear_field(Field {
            kind: FieldKind::Page(what),
            text,
            prompt: prompt.to_owned(),
            glyph,
            secret,
            show: false,
        });
    }

    /// A press inside the open page. Returns whether it was the page's.
    pub(crate) fn gear_press(&mut self) -> bool {
        if self.gear_page().is_none() {
            return false;
        }
        let hit = self.gear_hit_at();
        if hit == Hit::None {
            // A press on the page's empty space is still the page's: it must
            // not fall through and turn the page.
            let p = self.gear_ptr().unwrap_or((0.0, 0.0));
            return p.1 > self.gear_rect().y + if self.gear_host.is_some() { 0.0 } else { self.options_pill_h() };
        }
        self.gear_click(hit);
        true
    }

    /// A right-click: the row's details, the clipboard list's other way in.
    pub(crate) fn gear_right_click(&mut self) -> bool {
        match self.gear_hit_at() {
            Hit::Net(ssid) | Hit::NetMore(ssid) => self.gear_click(Hit::NetMore(ssid)),
            Hit::Dev(path) | Hit::DevMore(path) => {
                if self.bt_dev(&path).is_some_and(|d| d.paired) {
                    self.gear_click(Hit::DevMore(path));
                }
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn gear_show_view(&mut self) {
        self.gear.view_t = 0.0;
        self.gear.scroll = 0.0;
        self.gear.scroll_target = 0.0;
        self.gear.field = None;
        self.gear.field_t = 0.0;
        self.sync_gear_keyboard();
        self.schedule_stats_frame();
    }

    fn connect_net(&mut self, ssid: String) {
        let Some(ap) = self.net_ap(&ssid).cloned() else {
            return;
        };
        self.gear.net_err = None;
        if ap.active {
            if self.gear.net_snap.portal {
                // The network is up but wants its sign-in page first.
                if let Err(e) = crate::launch::launch(&format!("xdg-open {PORTAL_URL}"), false, "")
                {
                    info!("net: cannot open the sign-in page: {e}");
                }
                self.set_stats_box(false);
            } else {
                self.gear.net_leaving = Some(ssid);
                self.net_send(NetCommand::Disconnect);
            }
        } else if ap.saved.is_some() || !ap.secured() {
            self.gear.net_busy = Some(ssid.clone());
            self.net_send(NetCommand::Connect {
                ssid,
                password: None,
                hidden: false,
            });
        } else {
            self.gear.net_view = NetView::List;
            self.open_gear_field(password_field(ssid, false));
        }
    }

    fn set_airplane(&mut self, on: bool) {
        self.settings.airplane = on;
        self.settings.save();
        self.net_send(NetCommand::Radio(!on));
        if self.gear.bt_snap.present || self.gear.bt.is_none() {
            self.bt_send(BtCommand::Power(!on));
        }
    }

    fn refresh_hotspot_qr(&mut self) {
        let payload = net::wifi_qr_payload(&self.hotspot_name(), &self.settings.hotspot_pass, true);
        self.gear.qr = qr_matrix(&payload);
    }

    pub(crate) fn gear_click(&mut self, hit: Hit) {
        debug!("gear: {hit:?}");
        // Any click but the confirming one withdraws a pending question.
        let armed = self.gear.arm.take();
        match hit {
            Hit::Confirm => {
                if let Some((_, then)) = armed {
                    self.gear_click(then);
                }
                self.update_stats_reveal();
                return;
            }
            Hit::Sys(h) => self.sys_click(h),
            Hit::None | Hit::Field => {}
            Hit::FieldEye => {
                if let Some(f) = &mut self.gear.field {
                    f.show = !f.show;
                }
            }
            Hit::Back => {
                if !self.quick_back() {
                    self.pages_back();
                }
                self.gear.net_view = NetView::List;
                self.gear.bt_view = BtView::List;
                self.gear.share = false;
                self.gear.show_pw = false;
                self.gear_show_view();
            }
            Hit::Airplane => self.set_airplane(!self.settings.airplane),
            Hit::Quick(q) => self.quick_settings_click(q),
            Hit::WifiPower => {
                let on = self.gear.net_snap.wifi_on;
                self.net_send(NetCommand::Radio(!on));
            }
            Hit::Net(ssid) => self.connect_net(ssid),
            Hit::NetToggle => {
                if let NetView::Detail(ssid) = self.gear.net_view.clone() {
                    self.connect_net(ssid);
                }
            }
            Hit::NetMore(ssid) => {
                self.gear.net_detail = None;
                self.gear.qr = None;
                self.gear.share = false;
                self.gear.show_pw = false;
                self.net_send(NetCommand::Detail(ssid.clone()));
                self.gear.net_view = NetView::Detail(ssid);
                self.gear_show_view();
            }
            Hit::WifiScan => {
                self.gear.net_scanning = true;
                self.net_send(NetCommand::Rescan);
            }
            Hit::Hidden => self.open_gear_field(Field {
                kind: FieldKind::HiddenSsid,
                text: String::new(),
                prompt: tr("Name of the hidden network").into(),
                glyph: G_WIFI[3],
                secret: false,
                show: false,
            }),
            Hit::Hotspot => {
                if self.settings.hotspot_pass.is_empty() {
                    self.settings.hotspot_pass = random_password();
                    self.settings.save();
                }
                self.refresh_hotspot_qr();
                self.gear.show_pw = false;
                self.gear.share_err = None;
                // The same page from either list; Back returns to the one it
                // was opened from.
                if self.gear_page() == Some(PageKind::Bt) {
                    self.gear.bt_view = BtView::Share;
                } else {
                    self.gear.net_view = NetView::Hotspot;
                }
                self.gear_show_view();
            }
            Hit::NetShare => {
                self.gear.share = !self.gear.share;
                self.gear.show_pw = false;
                if let (true, NetView::Detail(ssid)) = (self.gear.share, self.gear.net_view.clone())
                {
                    self.net_send(NetCommand::Secret(ssid));
                }
            }
            Hit::NetShowPw | Hit::HsEye => self.gear.show_pw = !self.gear.show_pw,
            // Forgetting a network loses its password: asked once, as every
            // thing in the box that cannot be undone.
            Hit::NetForget => {
                if let NetView::Detail(ssid) = self.gear.net_view.clone() {
                    let ask = crate::i18n::fill(tr("Click again to forget {name}"), &[("name", &ssid)]);
                    self.gear_arm(&ask, Hit::NetForgetNow);
                }
            }
            Hit::NetForgetSaved(ssid) => {
                let ask = crate::i18n::fill(tr("Click again to forget {name}"), &[("name", &ssid)]);
                self.gear_arm(&ask, Hit::NetForgetSavedNow(ssid));
            }
            Hit::NetForgetSavedNow(ssid) => self.net_send(NetCommand::Forget(ssid)),
            Hit::NetForgetNow => {
                if let NetView::Detail(ssid) = self.gear.net_view.clone() {
                    self.net_send(NetCommand::Forget(ssid));
                    self.gear.net_view = NetView::List;
                    self.gear_show_view();
                }
            }
            Hit::NetAuto | Hit::NetMetered | Hit::NetPrivate | Hit::NetIp(_) | Hit::NetEdit(_) => {
                self.net_detail_click(hit);
            }
            Hit::BtShare => {
                let on = !self.gear.net_snap.bt_share_on;
                if on && !self.gear.bt_snap.powered {
                    return;
                }
                self.gear.share_err = None;
                self.gear.hs_busy = true;
                self.net_send(NetCommand::BtShare(on));
            }
            Hit::HsPower => {
                let on = !self.gear.net_snap.hotspot_on;
                self.gear.share_err = None;
                self.gear.hs_busy = true;
                self.refresh_hotspot_qr();
                let cmd = NetCommand::Hotspot {
                    on,
                    name: self.hotspot_name(),
                    password: self.settings.hotspot_pass.clone(),
                    band5: self.settings.hotspot_5ghz,
                };
                self.net_send(cmd);
            }
            Hit::HsName => {
                let text = self.hotspot_name();
                self.open_gear_field(Field {
                    kind: FieldKind::HsName,
                    text,
                    prompt: tr("Hotspot name").into(),
                    glyph: G_PENCIL,
                    secret: false,
                    show: false,
                });
            }
            Hit::HsPass => self.open_gear_field(Field {
                kind: FieldKind::HsPass,
                text: String::new(),
                prompt: tr("New password, 8 characters or more").into(),
                glyph: G_LOCK,
                secret: true,
                show: true,
            }),
            Hit::HsBand(five) => {
                self.settings.hotspot_5ghz = five;
                self.settings.save();
            }
            Hit::BtPower => {
                let on = self.gear.bt_snap.powered;
                self.bt_send(BtCommand::Power(!on));
            }
            Hit::BtName => {
                let text = self.gear.bt_snap.alias.clone();
                self.open_gear_field(Field {
                    kind: FieldKind::AdapterName,
                    text,
                    prompt: tr("Name of this computer").into(),
                    glyph: G_PENCIL,
                    secret: false,
                    show: false,
                });
            }
            Hit::Dev(path) => self.toggle_dev(path),
            Hit::DevToggle => {
                if let BtView::Detail(path) = self.gear.bt_view.clone() {
                    self.toggle_dev(path);
                }
            }
            Hit::DevMore(path) => {
                if let Some(address) = self.bt_dev(&path).map(|d| d.address.clone()) {
                    self.gear.bt_audio = None;
                    self.bt_send(BtCommand::Audio(address));
                }
                self.gear.bt_view = BtView::Detail(path);
                self.gear_show_view();
            }
            Hit::BtScan => {
                let on = self.gear.bt_snap.discovering;
                self.bt_send(BtCommand::Scan(!on));
            }
            Hit::BtVisible => {
                let on = self.gear.bt_snap.discoverable;
                self.bt_send(BtCommand::Discoverable(!on));
            }
            Hit::DevForget
            | Hit::DevForgetNow
            | Hit::DevAuto
            | Hit::DevRename
            | Hit::DevQuality(_)
            | Hit::DevOutput => {
                self.bt_detail_click(hit);
            }
            Hit::BtFiles => {
                self.gear.bt_view = BtView::Send;
                self.gear_show_view();
            }
            Hit::BtReceive => {
                self.settings.bt_receive = !self.settings.bt_receive;
                self.settings.save();
                let on = self.settings.bt_receive;
                self.files_send(FileCommand::Receive(on));
            }
            Hit::SendTo(path) => {
                if let Some(d) = self.bt_dev(&path).cloned() {
                    self.files_send(FileCommand::Send {
                        address: d.address,
                        name: d.name,
                    });
                    // The file picker is a window: the box (which holds the
                    // keyboard and sits above windows) gets out of its way.
                    self.gear.debug_hold = false;
                    self.set_stats_box(false);
                    return;
                }
            }
            Hit::PairYes => {
                self.bt_send(BtCommand::PairAnswer(true));
                // A pairing the OTHER device began has no call of ours to end
                // it: the question is answered, so it goes (it stayed up, box
                // held open, on the MacBook that accepted, 2026-10-06).
                if self.gear.bt_busy.is_none() {
                    self.gear.pair = None;
                    self.gear.bt_view = BtView::List;
                    self.gear_show_view();
                    self.update_stats_reveal();
                }
            }
            Hit::PairNo => {
                self.bt_send(BtCommand::PairAnswer(false));
                self.gear.bt_busy = None;
                self.gear.pair = None;
                self.gear.bt_view = BtView::List;
                self.gear_show_view();
                self.update_stats_reveal();
            }
        }
        self.gear_changed();
    }

    fn net_detail_click(&mut self, hit: Hit) {
        let NetView::Detail(ssid) = self.gear.net_view.clone() else {
            return;
        };
        let detail = self.gear.net_detail.clone().unwrap_or_default();
        match hit {
            Hit::NetAuto => {
                let on = self.net_ap(&ssid).is_some_and(|a| a.autoconnect);
                self.net_send(NetCommand::Auto(ssid, !on));
            }
            Hit::NetMetered => self.net_send(NetCommand::Metered(ssid, !detail.metered)),
            Hit::NetPrivate => self.net_send(NetCommand::Private(ssid, !detail.private_mac)),
            Hit::NetIp(true) if !detail.manual => {
                // Start from what the network handed out: the address stays
                // the one in use until it is edited.
                self.net_send(NetCommand::Manual {
                    ssid,
                    ip: detail.ip,
                    gateway: detail.gateway,
                    dns: detail.dns.split(',').next().unwrap_or("").trim().to_owned(),
                });
            }
            Hit::NetIp(false) if detail.manual => self.net_send(NetCommand::AutoIp(ssid)),
            Hit::NetEdit(key) => {
                let (text, prompt) = match key {
                    EditKey::Ip => (detail.ip, tr("IP address")),
                    EditKey::Gateway => (detail.gateway, tr("Router address")),
                    EditKey::Dns => (detail.dns, tr("DNS server")),
                };
                self.open_gear_field(Field {
                    kind: FieldKind::Edit(key),
                    text,
                    prompt: prompt.into(),
                    glyph: G_PENCIL,
                    secret: false,
                    show: false,
                });
            }
            _ => {}
        }
    }

    fn toggle_dev(&mut self, path: String) {
        let Some(d) = self.bt_dev(&path).cloned() else {
            return;
        };
        self.gear.bt_busy = Some(path.clone());
        self.gear.bt_busy_was = d.connected;
        self.gear.bt_err = None;
        self.bt_send(if d.connected {
            BtCommand::Disconnect(path)
        } else if d.paired {
            BtCommand::Connect(path)
        } else {
            BtCommand::Pair(path)
        });
    }

    fn bt_detail_click(&mut self, hit: Hit) {
        let BtView::Detail(path) = self.gear.bt_view.clone() else {
            return;
        };
        let Some(d) = self.bt_dev(&path).cloned() else {
            return;
        };
        match hit {
            // (Asked once: it must be paired again to come back.)
            Hit::DevForget => {
                let ask = crate::i18n::fill(tr("Click again to forget {name}"), &[("name", &d.name)]);
                self.gear_arm(&ask, Hit::DevForgetNow);
            }
            Hit::DevForgetNow => {
                self.bt_send(BtCommand::Forget(path));
                self.gear.bt_view = BtView::List;
                self.gear_show_view();
            }
            Hit::DevAuto => self.bt_send(BtCommand::Trust(path, !d.trusted)),
            Hit::DevRename => self.open_gear_field(Field {
                kind: FieldKind::Rename(path),
                text: d.name,
                prompt: tr("New name for this device").into(),
                glyph: G_PENCIL,
                secret: false,
                show: false,
            }),
            Hit::DevQuality(high) => self.bt_send(BtCommand::SoundQuality {
                address: d.address,
                high,
            }),
            Hit::DevOutput => self.bt_send(BtCommand::UseForSound(d.address)),
            _ => {}
        }
    }

    /// `debug-gear <what>`: reach a page state without a pointer (the nested
    /// rig has none). Returns what it did, for the log.
    pub(crate) fn gear_debug(&mut self, what: &str) -> String {
        let (verb, arg) = what.split_once(' ').unwrap_or((what, ""));
        match verb {
            "open" => {
                let kind = match arg {
                    "gear" => PageKind::Gear,
                    "bt" => PageKind::Bt,
                    "disk" => PageKind::Disk,
                    "cpu" => PageKind::Cpu,
                    "ram" => PageKind::Ram,
                    "gpu" => PageKind::Gpu,
                    "bat" => PageKind::Battery,
                    _ => PageKind::Net,
                };
                let Some(page) = self.stats_page_for(kind) else {
                    return format!("this machine has no {arg} page");
                };
                self.gear.debug_hold = true;
                self.stats.reveal = true;
                if !(self.stats.open && self.stats_page() == page) {
                    self.stats_open_page(page);
                }
            }
            "close" => {
                self.gear.debug_hold = false;
                self.set_stats_box(false);
                self.update_stats_reveal();
            }
            // The sleep timer's minute-before notice, now (`sleep_timer.rs`).
            "timer-notice" => self.sleep_timer_notice_now(),
            // The banner's timer pill (`timer_pill.rs`): its X, its time.
            "timer-x" => self.sleep_timer_cancel(),
            "timer-time" => self.timer_pill_edit(),
            // Keys to the pill being edited: "timer-key 45", "timer-key enter|esc|bs".
            "timer-key" => match arg {
                "enter" => self.timer_pill_key(Keysym::Return, None),
                "esc" => self.timer_pill_key(Keysym::Escape, None),
                "bs" => self.timer_pill_key(Keysym::BackSpace, None),
                typed => {
                    for c in typed.chars() {
                        self.timer_pill_key(Keysym::from_char(c), Some(&c.to_string()));
                    }
                }
            },
            // The footer's buttons behind a button out (`behind off`: in), as
            // the pointer on it would have them; the next pointer move decides.
            // (`behind timer`: the first footer button with buttons behind it;
            // alone, the last — the power button.)
            "behind" => {
                let view = self.gear_view();
                let with: Vec<usize> =
                    view.footer.iter().enumerate().filter(|(_, b)| !b.behind.is_empty()).map(|(i, _)| i).collect();
                self.gear.behind_open = match arg {
                    "off" => None,
                    "timer" => with.first().copied(),
                    _ => with.last().copied(),
                };
                if self.gear.behind_open.is_some() {
                    self.gear.behind_ix = self.gear.behind_open;
                }
                self.schedule_stats_frame();
            }
            "fake-off" => {
                // A picture of the compact page; the next poll puts it right.
                self.gear.net_snap.wifi_on = false;
                self.gear.bt_snap.powered = false;
                self.gear_changed();
            }
            "state" => {
                let n = self.gear.net_snap.aps.len();
                let d = self.gear.bt_snap.devices.len();
                return format!(
                    "{n} networks, {d} devices, page {:?}, apps {}",
                    self.gear_page(),
                    self.pages_debug_apps()
                );
            }
            "detail" => {
                let hit = match self.gear_page() {
                    Some(PageKind::Net) => self
                        .gear
                        .net_snap
                        .aps
                        .iter()
                        .find(|a| arg.is_empty() || a.ssid == arg)
                        .map(|a| Hit::NetMore(a.ssid.clone())),
                    Some(PageKind::Bt) => self
                        .gear
                        .bt_snap
                        .devices
                        .iter()
                        .find(|d| d.paired && (arg.is_empty() || d.name == arg))
                        .map(|d| Hit::DevMore(d.path.clone())),
                    _ => None,
                };
                match hit {
                    Some(h) => self.gear_click(h),
                    None => return "nothing to show details for".into(),
                }
            }
            "page" => {
                if !self.pages_debug(arg) {
                    return format!("no such view: {arg}");
                }
                self.gear_changed();
            }
            "confirm" => self.gear_click(Hit::Confirm),
            "enter" => self.submit_gear_field(),
            "do" => {
                if !self.pages_debug_do(arg) {
                    return format!("cannot do: {arg}");
                }
                self.gear_changed();
            }
            "share" => self.gear_click(Hit::NetShare),
            "files" => self.gear_click(Hit::BtFiles),
            "hotspot" => self.gear_click(Hit::Hotspot),
            "back" => self.gear_click(Hit::Back),
            "password" => {
                self.open_gear_field(password_field(arg.to_owned(), false));
                self.gear_changed();
            }
            "type" => self.gear_type(arg),
            "scroll" => self.gear_axis(arg.parse().unwrap_or(40.0)),
            "pair" => {
                // A picture of the pairing view only: nothing is paired.
                if let Some(d) = self.gear.bt_snap.devices.first().cloned() {
                    self.gear.bt_view = BtView::Pair(d.path);
                    self.gear.pair = Some(("482913".into(), true));
                    self.gear_show_view();
                    self.gear_changed();
                }
            }
            // What the open page says, as text: every item with its index,
            // so a script can check a page and `click` any control on it.
            "dump" => return self.gear_dump(),
            // `click 3` / `click 3.1` (a choice's option) / `click strip 0` /
            // `click footer 1` / `click more 3` (a row's corner control).
            "click" => {
                let view = self.gear_view();
                let (kind, rest) = arg.split_once(' ').unwrap_or(("item", arg));
                let (a, b) = rest.split_once('.').unwrap_or((rest, ""));
                let (i, j) = (
                    a.parse::<usize>().unwrap_or(usize::MAX),
                    b.parse::<usize>().unwrap_or(0),
                );
                let hit = match kind {
                    "strip" => view.strip.get(i).map(|b| b.hit.clone()),
                    "footer" => view.footer.get(i).map(|b| b.hit.clone()),
                    "more" => match view.items.get(i) {
                        Some(Item::Row {
                            more: Some((h, _)), ..
                        }) => Some(h.clone()),
                        _ => None,
                    },
                    _ => match view.items.get(i) {
                        Some(Item::Row { hit, .. } | Item::Toggle { hit, .. }) => Some(hit.clone()),
                        Some(Item::Kv { hit, .. }) => hit.clone(),
                        Some(Item::Card { rename, .. }) => rename.clone(),
                        Some(Item::Choice { opts, .. }) => opts.get(j).map(|o| o.0.clone()),
                        _ => None,
                    },
                };
                match hit {
                    Some(h) if h != Hit::None => {
                        let said = format!("click {h:?}");
                        self.gear_click(h);
                        return said;
                    }
                    _ => return format!("nothing to click at: {arg}"),
                }
            }
            _ => return format!("unknown: {what}"),
        }
        format!("{verb} done")
    }

    /// `debug-gear dump`: the open page as lines of text.
    fn gear_dump(&self) -> String {
        let view = self.gear_view();
        let mut out = format!("page {:?}", self.gear_page());
        let btns = |bs: &[Btn]| {
            bs.iter()
                .map(|b| format!("{:?}{}", b.hit, if b.lit { "*" } else { "" }))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if !view.strip.is_empty() {
            out += &format!("\n  strip: {}", btns(&view.strip));
        }
        for (i, it) in view.items.iter().enumerate() {
            let line = match it {
                Item::Row {
                    hit,
                    more,
                    title,
                    sub,
                    tone,
                    trail,
                    sel,
                    ..
                } => format!(
                    "row [{title}] [{sub}] {trail:?} {tone:?}{} -> {hit:?}{}",
                    if *sel { " sel" } else { "" },
                    more.as_ref()
                        .map(|m| format!(" (more {:?})", m.0))
                        .unwrap_or_default()
                ),
                Item::Heading { text, busy } => {
                    format!("heading [{text}]{}", if *busy { " busy" } else { "" })
                }
                Item::Level { hit, value, word, word_on, quiet, .. } => format!(
                    "level {:.0}% [{word}]{}{} -> {hit:?}",
                    value * 100.0,
                    if *word_on { " on" } else { "" },
                    if *quiet { " quiet" } else { "" }
                ),
                Item::Card {
                    title,
                    sub,
                    extra,
                    rename,
                    ..
                } => {
                    let extra = match extra {
                        Extra::Qr(_, line) => crate::i18n::fill(tr("Qr({line})"), &[("line", &line)]),
                        other => format!("{other:?}"),
                    };
                    format!("card [{title}] [{sub}] {extra} rename={rename:?}")
                }
                Item::Kv { hit, key, value } => format!("kv [{key}] = [{value}] -> {hit:?}"),
                Item::Toggle {
                    hit,
                    label,
                    hint,
                    on,
                } => format!("toggle [{label}] [{hint}] on={on} -> {hit:?}"),
                Item::Choice { key, opts } => format!(
                    "choice [{key}] {}",
                    opts.iter()
                        .map(|(h, l, on)| format!("{l}{}={h:?}", if *on { "*" } else { "" }))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ),
                Item::Empty(t) => format!("empty [{t}]"),
                Item::Note(t) => format!("note [{t}]"),
                Item::Graph(v) => crate::i18n::fill(tr("graph {0} points"), &[("0", &v.len())]),
                Item::Cores(v) => format!("cores {}", v.len()),
                Item::Bar(v) => format!(
                    "bar {}",
                    v.iter()
                        .map(|(l, w, _)| format!("{l}:{w:.0}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
            };
            out += &format!("\n  {i}: {line}");
        }
        if !view.footer.is_empty() {
            out += &format!("\n  footer: {}", btns(&view.footer));
        }
        if let Some(f) = &self.gear.field {
            out += &format!(
                "\n  field [{}] text [{}]",
                f.prompt,
                if f.secret && !f.show {
                    "•".repeat(f.text.chars().count())
                } else {
                    f.text.clone()
                }
            );
        }
        if let Some((line, hit)) = &self.gear.arm {
            out += &format!("\n  armed [{line}] -> {hit:?}");
        }
        out
    }
}

fn password_field(ssid: String, hidden: bool) -> Field {
    Field {
        prompt: crate::i18n::fill(tr("Password for {ssid}"), &[("ssid", &ssid)]),
        kind: FieldKind::Password { ssid, hidden },
        text: String::new(),
        glyph: G_LOCK,
        secret: true,
        show: false,
    }
}

/// The show/hide eye at a secret field's right end.
fn field_eye_rect(fr: Rect) -> Rect {
    let d = fr.h - 6.0;
    Rect::new(fr.x + fr.w - d - 5.0, fr.y + 3.0, d, d)
}

/// How much of the zebra's tone a details sheet keeps.
const QUIET_SHEET: f32 = 0.12;


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_are_cut_with_an_ellipsis() {
        assert_eq!(fit("Casa", 200.0, 17.0), "Casa");
        let cut = fit("A very long network name indeed", 100.0, 17.0);
        assert!(cut.ends_with('…'));
        assert!(est_w(&cut, 17.0) <= 100.0 + 17.0);
    }

    #[test]
    fn the_signal_symbol_and_word_follow_the_strength() {
        assert_eq!(wifi_glyph(0), G_WIFI[0]);
        assert_eq!(wifi_glyph(100), G_WIFI[3]);
        assert_eq!(signal_word(80), "Excellent");
        assert_eq!(signal_word(10), "Weak");
    }

    #[test]
    fn a_qr_code_is_square_and_has_its_finder_corner() {
        let m = qr_matrix("WIFI:T:WPA;S:Casa;P:secret123;;").expect("encodes");
        assert!(m.len() >= 21);
        assert!(m.iter().all(|r| r.len() == m.len()));
        assert!(
            m[0][..7].iter().all(|d| *d),
            "the top-left finder's top edge"
        );
    }

    #[test]
    fn a_row_presses_its_corner_before_itself() {
        let row = Item::Row {
            hit: Hit::Net("a".into()),
            more: Some((Hit::NetMore("a".into()), G_GO)),
            tile: None,
            title: "a".into(),
            sub: "Saved".into(),
            tone: Tone::Normal,
            trail: Trail::None,
            sel: false,
        };
        let r = Rect::new(0.0, 0.0, 400.0, item_h(&row, 1.0, 400.0));
        let zones = item_zones(&row, r, 1.0);
        let at = |p: (f32, f32)| {
            zones
                .iter()
                .find(|(_, z)| z.contains(p))
                .map(|(h, _)| h.clone())
        };
        // (The › stands at the end, in the row's middle.)
        assert_eq!(at((384.0, r.h / 2.0)), Some(Hit::NetMore("a".into())));
        assert_eq!(at((100.0, 20.0)), Some(Hit::Net("a".into())));
    }

    #[test]
    fn a_generated_password_is_long_enough_for_wpa() {
        let p = random_password();
        assert_eq!(p.chars().count(), 10);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}

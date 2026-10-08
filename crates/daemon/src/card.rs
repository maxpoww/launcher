//! THE CARD: a shelf that rides the windows (the mockup at
//! `~/terminal-mockup/new`).
//!
//! One card, one list. It sits on a window — inside it, under the title
//! bar, on the right — and whatever is dragged into it (text, files,
//! pictures) stays there as an item that can be dragged back out into any
//! window. It follows the focus, but only onto windows it was turned on
//! for; on any other window it stays where it was.
//!
//! One layer-shell surface of its own covering the output
//! ([`crate::surface::create_card_surface`]): the card is drawn where its
//! window is and only its box takes the pointer, so a change of window is a
//! redraw and never a resize of the surface. Where the window is comes from
//! the compositor ([`crate::hypr::window_spot`]), read on focus and layout
//! events and on a slow poll while the card is up — the plugin will carry
//! it frame by frame and put it in the window's own place in the stack;
//! until then it trails a window that is being dragged, and draws over
//! whatever overlaps its window.
//!
//! Items: a text is kept whole; a file or a folder is kept as its path (the
//! card points at it, it does not copy it); a picture that came as pixels
//! (out of a web page) is saved beside the list. A drag out is a real
//! Wayland drag of ours, always a COPY: the item stays.
//!
//! Pointer-free: `waverunner-ctl card [toggle [addr]|all|add text <…>|add file
//! <path>|remove <n>|clear|state]`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use smithay_client_toolkit::data_device_manager::data_offer::DragOffer;
use smithay_client_toolkit::data_device_manager::data_source::DragSource;
use smithay_client_toolkit::data_device_manager::WritePipe;
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{debug, error, info, warn};
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_pointer;
use wayland_client::WEnum;

use crate::content::{GridContent, IconInst, Label, Rect, RectInst, Scene, ShadowInst, NO_PLATE};
use crate::desktop::DragIcon;
use crate::hypr::WindowSpot;
use crate::App;

/// The card's width, and how far it sits in from its window's top, right
/// and bottom edges (the mockup's 320 and 10).
pub(crate) const WIDTH: f32 = 320.0;
/// The card's width can be changed by its side edges (Max, 2026-10-08: *"i
/// want to resize the card width"*): a press within [`GRIP`] of either side
/// takes that edge, between these limits (the first mockup's were 240–520).
/// The width is the WINDOW's, like the place the card was slid to: each
/// window has its own, and turning the card off there forgets it (*"the size
/// is also per window and reset when i close it, as the position"*).
const WIDTH_MIN: f32 = 240.0;
const WIDTH_MAX: f32 = 640.0;
const GRIP: f32 = 7.0;
const INSET: f32 = 10.0;
const RADIUS: f32 = 10.0;
/// How far past either side of its window the card may be slid.
const OVERHANG: f32 = 12.0;
/// How far the card slides per unit of scroll on its window's title bar.
const SLIDE_PER_SCROLL: f32 = 2.5;
/// How fast the card unrolls (the ease's rate, per second): 11 at first,
/// then 22 (Max, 2026-10-08: *"the reveal animation, make it snappier"*), then
/// **40** (*"snappier, and also the hide animation"*) — down, or back up, in
/// well under a tenth of a second. One rate for both ways.
const UNROLL_RATE: f32 = 40.0;
/// How long a scroll that moves a window (on the OPTIONS pill) must be
/// quiet before the window counts as put down.
const NUDGE_QUIET: std::time::Duration = std::time::Duration::from_millis(280);
/// A scroll over the card is one thing at a time: the list (up and down) or
/// the card itself (sideways). Whichever way has travelled this far first
/// takes the gesture, and keeps it until the scroll has been quiet this long.
const SCROLL_CLAIM: f32 = 5.0;
const SCROLL_QUIET: std::time::Duration = std::time::Duration::from_millis(220);
/// A THROW: a short, fast scroll that ends sends the card all the way to
/// that side, outside its window (Max, 2026-10-08: *"a fast scroll sends the
/// card to the end… just with a short fast scroll on the bar or on the
/// card"*). It is judged when the scroll STOPS, by how brief it was — a flick
/// is over in a moment, while moving the card fast by hand is a longer scroll
/// however quick. (The first cut threw on speed alone, mid-scroll: *"too hard
/// now to move without throw it"*, then *"i want to be able to move the card
/// fast without throwing it"*.) So: the whole scroll lasted no longer than
/// [`FLING_BRIEF`], covered at least [`FLING_SCROLL`], one way, and then went
/// quiet for [`FLING_QUIET`].
const FLING_SCROLL: f32 = 40.0;
const FLING_BRIEF: std::time::Duration = std::time::Duration::from_millis(170);
// 70 at first: the card slid with the flick, then stood still for that long
// before it took off — it read as getting stuck half way (Max, 2026-10-08:
// *"it feels like it gets stuck (slower) on the middle of the window"*). Now
// as short as the gaps between scroll steps allow, and no wait at all where
// the touchpad says the fingers lifted (`AxisStop`, over the card).
const FLING_QUIET: std::time::Duration = std::time::Duration::from_millis(30);
/// HOW THE CARD TRAVELS sideways — every way it is moved by scroll, and a
/// throw: it does not jump to where the scroll says, it GOES there, never
/// faster than [`TRAVEL_SPEED`] and never gaining speed faster than
/// [`TRAVEL_ACCEL`] (logical px/s and px/s²). A slow scroll it simply keeps
/// up with. A flick it falls behind, and the throw that follows carries on
/// at the same pace to the end — one motion, where the first cut slid at the
/// fingers' speed and then switched to a glide of its own (Max, 2026-10-08:
/// *"the sliding is not constant, like the first slide is fast because it
/// catch the speed of my fingers… maybe setting a max acceleration"*).
/// `card rate <speed> [accel]` changes both on the running dock.
// 7000 / 50000 at first; then a higher top speed reached about as gently
// (Max: *"make it faster but not snappy"*) — the trip is shorter, its start
// and its landing are not sharper.
const TRAVEL_SPEED: f32 = 12000.0;
const TRAVEL_ACCEL: f32 = 60000.0;
/// How it settles: its speed is at most this many times the distance left
/// (per second), so the last stretch eases in — and never less than
/// [`TRAVEL_CREEP`], so the ease has an end.
const TRAVEL_BRAKE: f32 = 28.0;
const TRAVEL_CREEP: f32 = 40.0;
/// A window shorter than this has no room for a card.
const MIN_HEIGHT: f32 = 90.0;

/// The list: its padding, the gap between items, an item's own padding
/// and corner.
const LIST_PAD: f32 = 12.0;
const GAP: f32 = 8.0;
const TILE_PAD_X: f32 = 10.0;
const TILE_PAD_Y: f32 = 9.0;
const TILE_RADIUS: f32 = 8.0;
const TEXT_PX: f32 = 12.0;
const TEXT_LINE: f32 = 17.0;
/// The small capital word over a file, a folder or a picture.
const KIND_PX: f32 = 10.0;
const KIND_LINE: f32 = 16.0;
/// A picture's box, and the air under it.
const PIC_H: f32 = 120.0;
const PIC_GAP: f32 = 6.0;
const PIC_RADIUS: f32 = 6.0;
/// The × that takes an item off, at the item's top-right corner.
const CLOSE: f32 = 22.0;
const CLOSE_INSET: f32 = 4.0;
/// A text keeps this many lines on the card; the whole of it still leaves
/// with a drag.
const MAX_LINES: usize = 12;
/// A press that travels this far takes the item (or the card) along.
const DRAG_START: f32 = 6.0;

/// The mockup's orange: the kind word, and the rim while a drag is over.
const ACCENT: [f32; 3] = [0.910, 0.576, 0.353];
const DANGER: [f32; 3] = [0.878, 0.322, 0.322];

/// Picture layers in the card's texture array; past it the oldest is
/// reused.
const PIC_LAYERS: u32 = 32;
/// The largest drop read into memory (a picture out of a web page).
const DROP_MAX: u64 = 48 * 1024 * 1024;
/// How often a card that is up asks where its window is.
const POLL: std::time::Duration = std::time::Duration::from_millis(250);

const URI_LIST: &str = "text/uri-list";
const TEXT_MIMES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];
const IMAGE_MIMES: [(&str, &str); 6] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
    ("image/bmp", "bmp"),
    ("image/svg+xml", "svg"),
];

const ITEMS_FILE: &str = "card.json";
/// Where pictures that came as pixels are kept (under the data directory).
const PICTURES_DIR: &str = "card";

/// What an item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Text,
    Image,
    File,
    Folder,
}

impl Kind {
    /// The word over the item (a text has none).
    fn word(self) -> Option<&'static str> {
        match self {
            Kind::Text => None,
            Kind::Image => Some("IMAGE"),
            Kind::File => Some("FILE"),
            Kind::Folder => Some("FOLDER"),
        }
    }
}

/// One thing on the card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Item {
    pub id: u64,
    pub kind: Kind,
    /// A text: the text. Anything else: the line shown under the kind word
    /// (the name and the size).
    pub body: String,
    /// The file, folder or picture on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A picture's width over its height (0 = not known).
    #[serde(default)]
    pub aspect: f32,
    /// The picture is the card's own copy (it came as pixels): it goes
    /// with the item.
    #[serde(default)]
    pub owned: bool,
}

/// The list as kept on disk.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    next_id: u64,
    items: Vec<Item>,
}

/// An item's box as last drawn, in surface coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Tile {
    pub id: u64,
    pub rect: Rect,
}

impl Tile {
    /// Where its × is.
    fn close(&self) -> Rect {
        Rect::new(
            self.rect.x + self.rect.w - CLOSE - CLOSE_INSET,
            self.rect.y + CLOSE_INSET,
            CLOSE,
            CLOSE,
        )
    }
}

/// The left button, held.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Press {
    /// On an item's ×: let go there and the item is off.
    Close(u64),
    /// On an item: travel takes it into a drag.
    Item {
        id: u64,
        at: (f32, f32),
        serial: u32,
    },
    /// On the card itself: it slides sideways with the pointer. `left` is
    /// where its left edge was, from its window's.
    Slide { from_x: f32, left: f32, moved: bool },
    /// On one of its side edges: that edge follows the pointer and the
    /// other stays. `left`/`width` are what they were at the press.
    Resize {
        left_edge: bool,
        from_x: f32,
        left: f32,
        width: f32,
    },
}

/// One run of sliding scroll: when it began, when its last step came, what
/// it adds up to, and whether it ever turned back.
#[derive(Debug, Clone, Copy)]
struct Swipe {
    began: std::time::Instant,
    last: std::time::Instant,
    sum: f32,
    turned: bool,
}

impl Swipe {
    /// Whether, now that it has stopped, it was a throw — and which way
    /// (`true`: the card goes left, as scroll down/right slides it).
    fn thrown(&self) -> Option<bool> {
        (!self.turned && self.last - self.began <= FLING_BRIEF && self.sum.abs() >= FLING_SCROLL)
            .then_some(self.sum > 0.0)
    }
}

/// A scroll gesture over the card.
#[derive(Debug, Default)]
struct Wheel {
    /// `Some(true)`: it slides the card; `Some(false)`: it scrolls the list.
    sideways: Option<bool>,
    across: f32,
    along: f32,
    until: Option<std::time::Instant>,
    /// The quiet timer is running.
    waiting: bool,
}

/// An item in hand: a Wayland drag of ours.
pub(crate) struct Drag {
    pub id: u64,
    pub source: DragSource,
    _icon: Option<DragIcon>,
}

/// What a drag out hands over for one type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Payload {
    Bytes(Vec<u8>),
    /// The file's own bytes, read when asked for.
    File(PathBuf),
}

/// The card's state on the loop.
#[derive(Default)]
pub(crate) struct Card {
    pub items: Vec<Item>,
    next_id: u64,
    loaded: bool,
    /// Windows the card was turned on for, by address.
    armed: HashSet<String>,
    /// Turned on for every window (the master switch)…
    all: bool,
    /// …but for these, turned off one by one since.
    off: HashSet<String>,
    /// The window it is on.
    pub host: Option<String>,
    /// Where that window is; `None` until it has been found.
    spot: Option<WindowSpot>,
    /// Where the card was slid to on a window: its left edge as a fraction
    /// of the window's width. A window without one has it at the right.
    geom: HashMap<String, f32>,
    /// The card's box on the surface.
    rect: Option<Rect>,
    /// The width it was given on a window, by address; a window without
    /// one has it [`WIDTH`] wide. Kept and forgotten with `geom`.
    widths: HashMap<String, f32>,
    /// The unroll, 0 (away) to 1 (down).
    shown: f32,
    /// Rolling up for good: the window is let go when it is up.
    leaving: bool,
    /// Its window is in hand (being moved or resized), since when: the
    /// card is away until the window is put down.
    lifted: Option<std::time::Instant>,
    /// The scroll that is sliding the card, measured for a throw.
    swipe: Option<Swipe>,
    /// The timer that judges the swipe once it stops is running.
    swipe_waiting: bool,
    /// The card on its way sideways: where its left edge (from its
    /// window's) is right now, and how fast it is going. `None` at rest —
    /// it is where `geom` says. See [`TRAVEL_SPEED`].
    at: Option<f32>,
    speed: f32,
    /// The pace, when `card rate` has set one for this run: (speed, accel).
    pace: Option<(f32, f32)>,
    /// The scroll gesture over the card: which way took it (see
    /// [`SCROLL_CLAIM`]), what each way has travelled before one did, and
    /// until when it lasts.
    wheel: Wheel,
    /// A window moved by scrolling on the OPTIONS pill has no moment it is
    /// let go: it counts as put down once the scroll has been quiet until
    /// this instant (`card_window_nudged`).
    nudge_until: Option<std::time::Instant>,
    scroll: f32,
    max_scroll: f32,
    /// Show the newest item on the next draw.
    to_bottom: bool,
    tiles: Vec<Tile>,
    /// Each item's wrapped lines (the width never changes).
    lines: HashMap<u64, Vec<String>>,
    ptr: Option<(f32, f32)>,
    press: Option<Press>,
    pub drag: Option<Drag>,
    /// Another app's drag is over the card.
    dnd_over: bool,
    dnd_mimes: Vec<String>,
    /// A name for a picture about to arrive as pixels (the end of its URL).
    dnd_hint: Option<String>,
    /// Pictures in hand, by path, and the texture layer each is in.
    chains: HashMap<String, Vec<u8>>,
    slots: HashMap<String, u32>,
    slot_next: u32,
    asked: HashSet<String>,
    array_ready: bool,
}

impl Card {
    /// How wide the card is on the window it is on.
    fn width(&self) -> f32 {
        self.host
            .as_ref()
            .and_then(|h| self.widths.get(h))
            .map_or(WIDTH, |w| w.clamp(WIDTH_MIN, WIDTH_MAX))
    }

    /// The side edge under `pos` (`true`: the left one), if any.
    fn grip(&self, pos: (f32, f32)) -> Option<bool> {
        let r = self.rect?;
        if pos.1 < r.y || pos.1 >= r.y + r.h {
            return None;
        }
        if (pos.0 - r.x).abs() <= GRIP && pos.0 >= r.x {
            Some(true)
        } else if (r.x + r.w - pos.0).abs() <= GRIP && pos.0 < r.x + r.w {
            Some(false)
        } else {
            None
        }
    }

    fn armed(&self, addr: &str) -> bool {
        self.armed.contains(addr) || (self.all && !self.off.contains(addr))
    }

    fn item(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|it| it.id == id)
    }

    /// The item under `pos`, and whether the pointer is on its ×.
    fn hit(&self, pos: (f32, f32)) -> Option<(u64, bool)> {
        let rect = self.rect?;
        if !rect.contains(pos) {
            return None;
        }
        self.tiles
            .iter()
            .find(|t| t.rect.contains(pos))
            .map(|t| (t.id, t.close().contains(pos)))
    }
}

/// The card's box on a window at `spot`: inside it, under the bar, on the
/// right — or with its left edge at `fx` of the window's width, where it
/// was slid to.
///
/// Its edges land on whole pixels OF THE SCREEN (`scale` physical per
/// logical), not on whole logical ones: at Golem's 1.6× a card travelling
/// sideways stepped 1.6, 1.6, 3.2 pixels where it now steps evenly, and
/// that unevenness was most of what made a slide look rough (Max,
/// 2026-10-08: *"see if you can make the sliding smoother"*).
pub(crate) fn card_rect(spot: &WindowSpot, fx: Option<f32>, scale: f32, width: f32) -> Rect {
    let left = match fx {
        Some(fx) => clamp_left(fx * spot.w, spot.w, width),
        None => spot.w - INSET - width,
    };
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let snap = |v: f32| (v * scale).round() / scale;
    Rect::new(
        snap(spot.x + left),
        snap(spot.y + INSET),
        snap(width),
        snap(spot.h - 2.0 * INSET),
    )
}

/// Where a thrown card comes to rest: OUTSIDE its window, beside it with a
/// little air, on the left (`to_left`) or the right — as far as a slide can
/// take it (Max, 2026-10-08: *"i meant to the end outside the window"*; the
/// first cut stopped at the inside edges).
pub(crate) fn fling_end(window_w: f32, to_left: bool, width: f32) -> f32 {
    if to_left {
        -(width + OVERHANG)
    } else {
        window_w + OVERHANG
    }
}

/// One frame of the card's travel from `at` toward `to`. Its speed gains at
/// most `accel`, never passes `top`, and comes down as the place nears —
/// [`TRAVEL_BRAKE`] times the distance left — so it sets off and settles
/// without a jolt, and a scroll it is following (a place that keeps moving a
/// little ahead of it) is one even motion, not a string of starts and stops.
/// It arrives exactly. Returns where it is and its speed after `dt`.
pub(crate) fn travel(at: f32, to: f32, speed: f32, dt: f32, top: f32, accel: f32) -> (f32, f32) {
    let left = to - at;
    if left.abs() < 0.3 {
        return (to, 0.0);
    }
    let want = (left.abs() * TRAVEL_BRAKE).clamp(TRAVEL_CREEP, top.max(TRAVEL_CREEP));
    let speed = if want > speed {
        (speed + accel * dt).min(want)
    } else {
        want
    };
    let step = speed * dt;
    if step >= left.abs() {
        (to, 0.0)
    } else {
        (at + step * left.signum(), speed)
    }
}

/// How far the card's left edge may go from its window's: fully out on
/// the left with a little air, to fully out on the right.
pub(crate) fn clamp_left(left: f32, window_w: f32, width: f32) -> f32 {
    left.clamp(-(width + OVERHANG), window_w + OVERHANG)
}

/// Break `text` into lines of at most `cols` characters: its own line
/// breaks are kept, a long line breaks at its last space (mid-word where
/// there is none), a line's indentation survives. At most `max` lines; the
/// last ends in "…" when more was left.
pub(crate) fn wrap(text: &str, cols: usize, max: usize) -> Vec<String> {
    let cols = cols.max(4);
    let text = text.replace('\t', "    ");
    let mut out: Vec<String> = Vec::new();
    let mut cut = false;
    'lines: for raw in text.trim_end().split('\n') {
        let chars: Vec<char> = raw.trim_end().chars().collect();
        if chars.is_empty() {
            if out.len() >= max {
                cut = true;
                break;
            }
            out.push(String::new());
            continue;
        }
        let mut start = 0;
        while start < chars.len() {
            if out.len() >= max {
                cut = true;
                break 'lines;
            }
            let mut end = (start + cols).min(chars.len());
            if end < chars.len() && chars[end] != ' ' {
                if let Some(space) = (start + 1..end).rev().find(|&i| chars[i] == ' ') {
                    end = space;
                }
            }
            out.push(
                chars[start..end]
                    .iter()
                    .collect::<String>()
                    .trim_end()
                    .to_owned(),
            );
            start = end;
            while start < chars.len() && chars[start] == ' ' {
                start += 1;
            }
        }
    }
    if cut {
        if let Some(last) = out.last_mut() {
            let mut kept: String = last.chars().take(cols.saturating_sub(1)).collect();
            kept.push('…');
            *last = kept;
        }
    }
    out
}

/// How tall an item with `lines` lines of text is.
pub(crate) fn tile_height(kind: Kind, lines: usize) -> f32 {
    let text = lines.max(1) as f32 * TEXT_LINE;
    2.0 * TILE_PAD_Y
        + match kind {
            Kind::Text => text,
            Kind::Image => KIND_LINE + PIC_H + PIC_GAP + text,
            Kind::File | Kind::Folder => KIND_LINE + text,
        }
}

/// What a local path is to the card.
pub(crate) fn kind_of(path: &Path) -> Kind {
    if path.is_dir() {
        Kind::Folder
    } else if crate::files::file_asset_name(&path.to_string_lossy()) == "asset-image" {
        Kind::Image
    } else {
        Kind::File
    }
}

/// The line under a file's kind word: its name, and its size (a folder:
/// how many things are in it).
fn file_body(path: &Path) -> String {
    let name = path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let Ok(meta) = std::fs::metadata(path) else {
        return name;
    };
    if meta.is_dir() {
        match std::fs::read_dir(path) {
            Ok(dir) => {
                let n = dir.count();
                format!("{name}/ · {n} item{}", if n == 1 { "" } else { "s" })
            }
            Err(_) => format!("{name}/"),
        }
    } else {
        format!("{name} · {}", size_text(meta.len()))
    }
}

/// A file's size as the mockup writes it: "9.8 KB", "2.4 MB".
pub(crate) fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A picture's width over its height, from the file's header.
fn aspect_of(path: &Path) -> f32 {
    match image::image_dimensions(path) {
        Ok((w, h)) if w > 0 && h > 0 => w as f32 / h as f32,
        _ => 0.0,
    }
}

/// The first of our text types among `mimes`.
pub(crate) fn text_mime(mimes: &[String]) -> Option<&'static str> {
    TEXT_MIMES
        .into_iter()
        .find(|t| mimes.iter().any(|m| m == t))
}

/// The first of our picture types among `mimes`, and its file extension.
pub(crate) fn image_mime(mimes: &[String]) -> Option<(&'static str, &'static str)> {
    IMAGE_MIMES
        .into_iter()
        .find(|(t, _)| mimes.iter().any(|m| m == t))
}

/// The picture type a file's name says it is.
fn mime_of(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    let ext = if ext == "jpeg" { "jpg".to_owned() } else { ext };
    IMAGE_MIMES
        .into_iter()
        .find(|(_, e)| *e == ext)
        .map(|(t, _)| t)
}

/// The types a drag of `item` offers, the richest first: a file as a list
/// of URIs (and a picture as its own pixels), then its path as text; a
/// text as text.
pub(crate) fn out_mimes(item: &Item) -> Vec<&'static str> {
    let mut mimes = Vec::new();
    if let Some(path) = item.path.as_deref() {
        mimes.push(URI_LIST);
        if item.kind == Kind::Image {
            if let Some(mime) = mime_of(path) {
                mimes.push(mime);
            }
        }
    }
    mimes.extend(TEXT_MIMES);
    mimes
}

/// What `item` hands over as `mime` (`None`: a type it never offered).
pub(crate) fn payload(item: &Item, mime: &str) -> Option<Payload> {
    match item.path.as_deref() {
        Some(path) if mime == URI_LIST => Some(Payload::Bytes(
            format!("{}\r\n", crate::desktop::file_uri(path)).into_bytes(),
        )),
        Some(path) if item.kind == Kind::Image && mime_of(path) == Some(mime) => {
            Some(Payload::File(PathBuf::from(path)))
        }
        Some(path) if TEXT_MIMES.contains(&mime) => Some(Payload::Bytes(path.as_bytes().to_vec())),
        None if TEXT_MIMES.contains(&mime) => Some(Payload::Bytes(item.body.as_bytes().to_vec())),
        _ => None,
    }
}

/// The name at the end of the first remote address in a URI list (a
/// picture dragged out of a web page says where it came from).
fn remote_name(list: &str) -> Option<String> {
    let url = list
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("http://") || l.starts_with("https://"))?;
    let path = url.split(['?', '#']).next()?;
    let name = path.rsplit('/').next().filter(|n| !n.is_empty())?;
    Some(
        crate::trash::decode_path(name)
            .to_string_lossy()
            .into_owned(),
    )
}

/// The colours a card is drawn in: the box's fill and ink (the OPTIONS
/// boxes' own, read on the card's side of the screen).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Paint {
    pub fill: [f32; 4],
    pub ink: [f32; 4],
}

impl Paint {
    /// A light box (its ink is dark): the insets darken less, the rims are
    /// dark instead of light.
    fn bright(&self) -> bool {
        0.2126 * self.ink[0] + 0.7152 * self.ink[1] + 0.0722 * self.ink[2] < 0.5
    }

    fn ink_at(&self, a: f32) -> [f32; 4] {
        [self.ink[0], self.ink[1], self.ink[2], self.ink[3] * a]
    }
}

/// Everything one frame of the card is drawn from.
pub(crate) struct View<'a> {
    pub rect: Rect,
    pub shown: f32,
    pub items: &'a [Item],
    pub lines: &'a HashMap<u64, Vec<String>>,
    pub scroll: f32,
    /// The item under the pointer, and whether the pointer is on its ×.
    pub hover: Option<(u64, bool)>,
    pub dnd_over: bool,
    /// The pictures that have arrived: path → texture layer.
    pub slots: &'a HashMap<String, u32>,
    pub paint: Paint,
}

/// The card as a scene, the boxes its items were drawn in, and how far the
/// list can be scrolled. The unroll is a clip from the top: the card is
/// whole underneath and `shown` of its height is let through.
pub(crate) fn scene(view: &View) -> (Scene, Vec<Tile>, f32) {
    let mut scene = Scene {
        alpha: 1.0,
        ..Default::default()
    };
    let rect = view.rect;
    let shown = view.shown.clamp(0.0, 1.0);
    if shown <= 0.0 || rect.h <= 0.0 {
        return (scene, Vec::new(), 0.0);
    }
    let paint = &view.paint;
    let bright = paint.bright();
    let window = Rect::new(rect.x, rect.y, rect.w, (rect.h * shown).round());

    scene.shadows.push(ShadowInst {
        rect: window,
        radius: RADIUS,
        blur: 26.0,
        color: [0.0, 0.0, 0.0, shown],
        edges: [0.18, 0.42, 0.34, 0.34],
    });

    let mut panel = GridContent {
        clip: window,
        ..Default::default()
    };
    panel.rects.push(RectInst {
        rect,
        radius: RADIUS,
        color: paint.fill,
        glass: 0.0,
        border: 0.0,
    });
    let rim = if view.dnd_over {
        [ACCENT[0], ACCENT[1], ACCENT[2], 0.7]
    } else if bright {
        [0.0, 0.0, 0.0, 0.12]
    } else {
        [1.0, 1.0, 1.0, 0.09]
    };
    panel.rects.push(RectInst {
        rect,
        radius: RADIUS,
        color: rim,
        glass: 0.0,
        border: if view.dnd_over { 1.5 } else { 1.0 },
    });
    scene.grids.push(panel);

    // The list, clipped a hair inside the card so nothing rides over its
    // rim, and to what the unroll has let through.
    let inner = Rect::new(rect.x + 1.0, rect.y + 1.0, rect.w - 2.0, rect.h - 2.0);
    let clip = Rect::new(
        inner.x,
        inner.y,
        inner.w,
        (window.y + window.h - inner.y).min(inner.h).max(0.0),
    );
    let mut list = GridContent {
        clip,
        ..Default::default()
    };
    let tile_w = rect.w - 2.0 * LIST_PAD;
    let text_w = tile_w - 2.0 * TILE_PAD_X;
    let heights: Vec<f32> = view
        .items
        .iter()
        .map(|it| tile_height(it.kind, view.lines.get(&it.id).map_or(1, Vec::len)))
        .collect();
    let total =
        2.0 * LIST_PAD + heights.iter().sum::<f32>() + GAP * heights.len().saturating_sub(1) as f32;
    let max_scroll = (total - rect.h).max(0.0);
    let scroll = view.scroll.clamp(0.0, max_scroll);

    let tile_fill = if bright {
        [0.0, 0.0, 0.0, 0.07]
    } else {
        [0.0, 0.0, 0.0, 0.28]
    };
    let tile_rim = if bright {
        [0.0, 0.0, 0.0, 0.10]
    } else {
        [1.0, 1.0, 1.0, 0.06]
    };
    let pic_fill = if bright {
        [0.0, 0.0, 0.0, 0.08]
    } else {
        [0.0, 0.0, 0.0, 0.30]
    };
    let mut tiles = Vec::with_capacity(view.items.len());
    let mut y = rect.y + LIST_PAD - scroll;
    for (item, h) in view.items.iter().zip(heights) {
        let tile = Tile {
            id: item.id,
            rect: Rect::new(rect.x + LIST_PAD, y.round(), tile_w, h),
        };
        y += h + GAP;
        tiles.push(tile);
        let t = tile.rect;
        if t.y > clip.y + clip.h || t.y + t.h < clip.y {
            continue;
        }
        list.rects.push(RectInst {
            rect: t,
            radius: TILE_RADIUS,
            color: tile_fill,
            glass: 0.0,
            border: 0.0,
        });
        list.rects.push(RectInst {
            rect: t,
            radius: TILE_RADIUS,
            color: tile_rim,
            glass: 0.0,
            border: 1.0,
        });
        let x = t.x + TILE_PAD_X;
        let mut line_y = t.y + TILE_PAD_Y;
        if let Some(word) = item.kind.word() {
            list.labels.push(Label {
                text: word.to_owned(),
                pos: (x, line_y),
                max_w: text_w,
                font_px: KIND_PX,
                line_px: KIND_LINE,
                centered: false,
                dim: false,
                cache: true,
                clip: Some(clip),
                family: None,
                color: Some([ACCENT[0], ACCENT[1], ACCENT[2], 0.85]),
            });
            line_y += KIND_LINE;
        }
        if item.kind == Kind::Image {
            let frame = Rect::new(x, line_y, text_w, PIC_H);
            list.rects.push(RectInst {
                rect: frame,
                radius: PIC_RADIUS,
                color: pic_fill,
                glass: 0.0,
                border: 0.0,
            });
            let layer = item.path.as_ref().and_then(|p| view.slots.get(p));
            if let Some(&layer) = layer {
                // The picture is kept whole on a square: the square is
                // sized so the picture itself fills the frame's height, or
                // its width where it is wider than that allows.
                let side = if item.aspect > 1.0 {
                    (PIC_H * item.aspect).min(frame.w)
                } else {
                    PIC_H
                };
                list.icons.push(IconInst {
                    rect: Rect::new(
                        // (Not rounded: the card's own edge is on a screen pixel,
                        // and a picture rounded apart from it would jitter
                        // against the card as it travels.)
                        frame.x + (frame.w - side) / 2.0,
                        (frame.y + (frame.h - side) / 2.0).round(),
                        side,
                        side,
                    ),
                    layer,
                    tint: [0.0; 4],
                    ring: -1.0,
                    plate: NO_PLATE,
                });
            }
            line_y += PIC_H + PIC_GAP;
        }
        let family = (item.kind == Kind::Text).then_some(crate::options::NERD);
        for line in view.lines.get(&item.id).into_iter().flatten() {
            if !line.is_empty() {
                list.labels.push(Label {
                    text: line.clone(),
                    pos: (x, line_y),
                    max_w: text_w,
                    font_px: TEXT_PX,
                    line_px: TEXT_LINE,
                    centered: false,
                    dim: false,
                    cache: true,
                    clip: Some(clip),
                    family,
                    color: Some(paint.ink_at(0.92)),
                });
            }
            line_y += TEXT_LINE;
        }
        // The × shows on the item under the pointer only.
        if let Some((id, on_close)) = view.hover {
            if id == item.id {
                let close = tile.close();
                if on_close {
                    list.rects.push(RectInst {
                        rect: close,
                        radius: 6.0,
                        color: [DANGER[0], DANGER[1], DANGER[2], 0.2],
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                list.labels.push(Label {
                    text: "×".to_owned(),
                    pos: (close.x + close.w / 2.0, close.y + 1.0),
                    max_w: close.w,
                    font_px: 15.0,
                    line_px: CLOSE - 2.0,
                    centered: true,
                    dim: false,
                    cache: true,
                    clip: Some(clip),
                    family: None,
                    color: Some(if on_close {
                        [DANGER[0], DANGER[1], DANGER[2], 1.0]
                    } else {
                        paint.ink_at(0.45)
                    }),
                });
            }
        }
    }
    scene.grids.push(list);
    (scene, tiles, max_scroll)
}

fn load() -> Saved {
    crate::persist::read_json(&crate::persist::data_path(ITEMS_FILE)).unwrap_or_default()
}

impl App {
    /// A `configure` for the card's surface: its size (the output's). The
    /// renderer is not built here but when a card is first drawn — a
    /// surface with nothing attached costs nothing.
    pub(crate) fn configure_card(
        &mut self,
        configure: smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure,
    ) {
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        // A dock that has just started has the card on for no window: any
        // title bar still showing it on (from the dock before) is put right.
        if self.card_size == (0, 0) {
            self.card_tell_bar("*", false);
        }
        self.card_size = (width, height);
        if let Some(fs) = &self.card_fscale {
            fs.set_logical_size(width, height);
        }
        let scale = self.surface_scale(crate::fractional::SurfaceKind::Card);
        if let Some(renderer) = self.card_renderer.as_mut() {
            renderer.set_scale(scale);
            renderer.resize(
                crate::fractional::physical(width, scale),
                crate::fractional::physical(height, scale),
            );
        }
        self.sync_card();
    }

    /// The card's renderer, built the first time there is a card to draw.
    fn card_ensure_renderer(&mut self) -> bool {
        if self.card_renderer.is_some() {
            return true;
        }
        let (width, height) = self.card_size;
        if width == 0 || height == 0 {
            return false;
        }
        let scale = self.surface_scale(crate::fractional::SurfaceKind::Card);
        let built = {
            let Some(layer) = self.card_layer.as_ref() else {
                return false;
            };
            crate::renderer::Renderer::new(
                &self.conn,
                layer.wl_surface(),
                crate::fractional::physical(width, scale),
                crate::fractional::physical(height, scale),
                scale,
            )
        };
        match built {
            Ok(mut renderer) => {
                renderer.alloc_icon_array(PIC_LAYERS);
                for (path, &layer) in &self.card.slots {
                    if let Some(chain) = self.card.chains.get(path) {
                        renderer.update_icon_layer(layer, chain);
                    }
                }
                self.card.array_ready = true;
                self.card_renderer = Some(renderer);
                true
            }
            Err(e) => {
                // Never fatal: the shell outlives a card that cannot draw.
                error!("card renderer init failed: {e:#}");
                false
            }
        }
    }

    /// Read the list kept on disk, once.
    fn card_load(&mut self) {
        if self.card.loaded {
            return;
        }
        self.card.loaded = true;
        let saved = load();
        self.card.next_id = saved
            .next_id
            .max(saved.items.iter().map(|it| it.id + 1).max().unwrap_or(1));
        self.card.items = saved.items;
        self.card.to_bottom = true;
        for path in self.card_pictures() {
            self.card_ask_picture(&path);
        }
    }

    fn card_save(&self) {
        crate::persist::write_json(
            "card",
            &crate::persist::data_path(ITEMS_FILE),
            &Saved {
                next_id: self.card.next_id,
                items: self.card.items.clone(),
            },
        );
    }

    fn card_pictures(&self) -> Vec<String> {
        self.card
            .items
            .iter()
            .filter(|it| it.kind == Kind::Image)
            .filter_map(|it| it.path.clone())
            .collect()
    }

    /// Ask the thumbnailer for a picture the card has none of yet.
    fn card_ask_picture(&mut self, path: &str) {
        if self.card.chains.contains_key(path) || !self.card.asked.insert(path.to_owned()) {
            return;
        }
        self.thumbs.request(path);
    }

    /// A thumbnail arrived: if it is of a picture on the card, keep it and
    /// put it in a layer.
    pub(crate) fn card_on_thumb(&mut self, path: &str, pixels: &[u8]) {
        if !self
            .card
            .items
            .iter()
            .any(|it| it.path.as_deref() == Some(path))
        {
            return;
        }
        self.card.chains.insert(path.to_owned(), pixels.to_vec());
        let layer = match self.card.slots.get(path) {
            Some(&layer) => layer,
            None => {
                let layer = self.card.slot_next % PIC_LAYERS;
                self.card.slot_next += 1;
                // Reused: whichever picture was in it shows its frame again.
                self.card.slots.retain(|_, l| *l != layer);
                self.card.slots.insert(path.to_owned(), layer);
                layer
            }
        };
        if let Some(renderer) = self.card_renderer.as_mut() {
            renderer.update_icon_layer(layer, pixels);
        }
        self.request_card_draw();
    }

    /// Put `item` at the end of the list (the newest is at the bottom).
    fn card_push(&mut self, kind: Kind, body: String, path: Option<String>, owned: bool) {
        self.card_load();
        let id = self.card.next_id.max(1);
        self.card.next_id = id + 1;
        let aspect = match (&path, kind) {
            (Some(p), Kind::Image) => aspect_of(Path::new(p)),
            _ => 0.0,
        };
        if let (Some(p), Kind::Image) = (&path, kind) {
            self.card_ask_picture(&p.clone());
        }
        info!(
            "card: + {kind:?} {:?}",
            body.chars().take(60).collect::<String>()
        );
        self.card.items.push(Item {
            id,
            kind,
            body,
            path,
            aspect,
            owned,
        });
        self.card.to_bottom = true;
        self.card_save();
        self.request_card_draw();
    }

    /// Put a text on the card.
    pub(crate) fn card_add_text(&mut self, text: &str) {
        // A run of blank lines is one (the mockup's rule).
        let mut body = String::new();
        let mut blank = 0;
        for line in text.trim().lines() {
            if line.trim().is_empty() {
                blank += 1;
                if blank > 1 {
                    continue;
                }
            } else {
                blank = 0;
            }
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line.trim_end());
        }
        if !body.is_empty() {
            self.card_push(Kind::Text, body, None, false);
        }
    }

    /// Put a file, a folder or a picture on the card, by its path.
    pub(crate) fn card_add_path(&mut self, path: &Path) {
        if !path.exists() {
            warn!("card: {} is not there; not added", path.display());
            return;
        }
        self.card_push(
            kind_of(path),
            file_body(path),
            Some(path.to_string_lossy().into_owned()),
            false,
        );
    }

    /// Take an item off the card (a picture of the card's own goes too).
    fn card_remove(&mut self, id: u64) {
        let Some(at) = self.card.items.iter().position(|it| it.id == id) else {
            return;
        };
        let item = self.card.items.remove(at);
        info!(
            "card: − {:?} {:?}",
            item.kind,
            item.body.chars().take(60).collect::<String>()
        );
        self.card.lines.remove(&id);
        if let Some(path) = item.path.as_deref() {
            if !self
                .card
                .items
                .iter()
                .any(|it| it.path.as_deref() == Some(path))
            {
                self.card.chains.remove(path);
                self.card.slots.remove(path);
                self.card.asked.remove(path);
            }
            if item.owned {
                let _ = std::fs::remove_file(path);
            }
        }
        self.card_save();
        self.request_card_draw();
    }

    /// Whether there is a card to show right now: on a window that can be
    /// seen, and not under the overview.
    fn card_present(&self) -> bool {
        self.card.host.is_some()
            && !self.card.leaving
            && self.card.lifted.is_none()
            && !self.overview_active
            && self
                .card
                .spot
                .is_some_and(|s| s.visible && s.h >= MIN_HEIGHT)
    }

    /// Tell the title bars which windows the card is on for (`addr`: one
    /// window, or "*" for all): the plugin draws the button, we own the
    /// answer. Nothing hears it without the plugin, which is fine.
    fn card_tell_bar(&self, addr: &str, on: bool) {
        crate::hypr::eval(&format!("hl.plugin.waveview.card(\"{addr}\", {on})"));
    }

    /// The button on a window's title bar: the card on for that window (and
    /// here), or off for it (and away, if it was here). Without an address:
    /// the focused window.
    pub(crate) fn card_toggle(&mut self, addr: Option<&str>) -> String {
        let addr = addr
            .map(str::to_owned)
            .or_else(|| self.options_active_addr.clone())
            .or_else(crate::hypr::active_window);
        let Some(addr) = addr else {
            return "no window is focused".to_owned();
        };
        self.card_tell_bar(&addr, !self.card.armed(&addr));
        if self.card.armed(&addr) {
            self.card.armed.remove(&addr);
            if self.card.all {
                self.card.off.insert(addr.clone());
            }
            // Turning it off here forgets where it was put here.
            self.card.geom.remove(&addr);
            self.card.widths.remove(&addr);
            if self.card.host.as_deref() == Some(addr.as_str()) {
                self.card_dismiss();
            }
            format!("off for {addr}")
        } else {
            self.card.armed.insert(addr.clone());
            self.card.off.remove(&addr);
            self.card_summon(&addr);
            format!("on for {addr}")
        }
    }

    /// The master switch: the card on for every window, or off for all.
    pub(crate) fn card_toggle_all(&mut self) -> String {
        if self.card.all {
            self.card.all = false;
            self.card.armed.clear();
            self.card.off.clear();
            self.card.geom.clear();
            self.card.widths.clear();
            self.card_tell_bar("*", false);
            self.card_dismiss();
            "off for every window".to_owned()
        } else {
            self.card.all = true;
            self.card.off.clear();
            self.card_tell_bar("*", true);
            if let Some(addr) = self
                .options_active_addr
                .clone()
                .or_else(crate::hypr::active_window)
            {
                self.card_summon(&addr);
            }
            "on for every window".to_owned()
        }
    }

    /// The focus moved: the card comes along onto a window it is on for,
    /// and stays where it is otherwise.
    pub(crate) fn card_focus_changed(&mut self, addr: Option<&str>) {
        let Some(addr) = addr else {
            return;
        };
        if self.card.host.as_deref() == Some(addr) && !self.card.leaving {
            return;
        }
        if self.card.armed(addr) {
            // (A window opened since the master switch has not heard yet.)
            if self.card.all {
                self.card_tell_bar(addr, true);
            }
            self.card_summon(addr);
        }
    }

    /// Bring the card onto `addr`: it unrolls there from the top.
    fn card_summon(&mut self, addr: &str) {
        self.card_load();
        // The text is wrapped to the card's width, which is the window's.
        let before = self.card.width();
        self.card.host = Some(addr.to_owned());
        if self.card.width() != before {
            self.card.lines.clear();
        }
        self.card.leaving = false;
        self.card.shown = 0.0;
        self.card.spot = None;
        self.card.press = None;
        self.card_last_frame = None;
        self.sync_card();
    }

    /// Roll the card up and let its window go.
    fn card_dismiss(&mut self) {
        if self.card.host.is_some() {
            self.card.leaving = true;
            self.card.press = None;
            self.request_card_draw();
        }
    }

    /// Find the card's window again and put the card where it is now. Runs
    /// on focus and layout events, and on the poll while a card is up.
    pub(crate) fn sync_card(&mut self) {
        let Some(host) = self.card.host.clone() else {
            return;
        };
        match crate::hypr::window_spot(&host) {
            None => {
                // The window is gone, and the card with it (an address is
                // reused: nothing of it may be kept for the next window).
                debug!("card: its window {host} is gone");
                self.card.armed.remove(&host);
                self.card.off.remove(&host);
                self.card.geom.remove(&host);
                self.card.widths.remove(&host);
                self.card.host = None;
                self.card.spot = None;
                self.card.rect = None;
                self.card.shown = 0.0;
                self.card.leaving = false;
                self.card.press = None;
            }
            Some(spot) => {
                self.card.spot = Some(spot);
                // A card being slid keeps to the pointer, not to memory.
                if !matches!(
                    self.card.press,
                    Some(Press::Slide { .. } | Press::Resize { .. })
                ) {
                    let fx = match self.card.at {
                        Some(at) if spot.w > 0.0 => Some(at / spot.w),
                        _ => self.card.geom.get(&host).copied(),
                    };
                    let scale = self.surface_scale(crate::fractional::SurfaceKind::Card);
                    self.card.rect = Some(card_rect(&spot, fx, scale, self.card.width()));
                }
            }
        }
        self.sync_card_input();
        self.request_card_draw();
        self.card_poll_arm();
    }

    /// Only the card's box takes the pointer; with no card up the surface
    /// takes nothing.
    fn sync_card_input(&mut self) {
        let Some(layer) = self.card_layer.as_ref() else {
            return;
        };
        // While a sideways scroll is moving the card, the whole surface
        // takes the pointer: the card slides out from under it, and the
        // scroll must keep reaching us until it stops.
        let (w, h) = self.card_size;
        let rects: Vec<(i32, i32, i32, i32)> = match self.card.rect.filter(|_| self.card_present())
        {
            Some(_) if self.card.wheel.sideways == Some(true) => vec![(0, 0, w as i32, h as i32)],
            Some(r) => vec![(r.x as i32, r.y as i32, r.w as i32, r.h as i32)],
            None => Vec::new(),
        };
        if self.card_input != rects {
            crate::surface::set_input_rects(&self.compositor, layer, &rects);
            self.card_input = rects;
        }
    }

    /// While a card is up, ask where its window is every so often: nothing
    /// tells us a window is being dragged or resized. (Scaffolding — the
    /// plugin will carry the card with its window.)
    fn card_poll_arm(&mut self) {
        if self.card_poll_timer || self.card.host.is_none() {
            return;
        }
        let timer = calloop::timer::Timer::from_duration(POLL);
        let armed = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.card_poll_timer = false;
                let Some(host) = app.card.host.clone() else {
                    return calloop::timer::TimeoutAction::Drop;
                };
                let now = crate::hypr::window_spot(&host);
                // A window "in hand" that has not moved for a long while:
                // its being put down was never heard (the plugin went away
                // mid-drag). The card comes back rather than staying lost.
                if now == app.card.spot
                    && app
                        .card
                        .lifted
                        .is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(6))
                {
                    app.card_window_placed(&host);
                    return calloop::timer::TimeoutAction::Drop;
                }
                if now != app.card.spot || now.is_none() {
                    app.sync_card();
                } else {
                    app.card_poll_arm();
                }
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
        if armed {
            self.card_poll_timer = true;
        }
    }

    /// Draw now, or once the frame in flight has been shown.
    pub(crate) fn request_card_draw(&mut self) {
        self.card_cursor();
        if self.card_frame_pending {
            self.card_dirty = true;
        } else {
            self.draw_card();
        }
    }

    /// Draw one frame of the card.
    pub(crate) fn draw_card(&mut self) {
        self.card_dirty = false;
        let present = self.card_present();
        // Nothing was ever drawn and nothing is to be: no renderer yet.
        if !present && self.card_renderer.is_none() {
            return;
        }
        if !self.card_ensure_renderer() {
            return;
        }
        let now = std::time::Instant::now();
        let dt = self
            .card_last_frame
            .map(|l| now.duration_since(l).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        let target = if present { 1.0 } else { 0.0 };
        // A card that cannot be seen at all (its window's workspace left)
        // does not roll up: it is simply not there.
        let (shown, moving) = if !present && !self.card.leaving {
            (0.0, false)
        } else {
            crate::animation::ease_toward(self.card.shown, target, dt, UNROLL_RATE, 0.004)
        };
        self.card.shown = shown;
        // The card on its way sideways (see `TRAVEL_SPEED`).
        let mut moving = moving;
        if let (Some(at), Some(spot), Some(host)) =
            (self.card.at, self.card.spot, self.card.host.clone())
        {
            let to = match self.card.geom.get(&host) {
                Some(fx) => fx * spot.w,
                None => spot.w - INSET - self.card.width(),
            };
            let (top, accel) = self.card.pace.unwrap_or((TRAVEL_SPEED, TRAVEL_ACCEL));
            let (at, speed) = travel(at, to, self.card.speed, dt, top, accel);
            self.card.speed = speed;
            if at == to || !present || spot.w <= 0.0 {
                self.card.at = None;
                self.card.speed = 0.0;
                self.card.rect = Some(card_rect(
                    &spot,
                    self.card.geom.get(&host).copied(),
                    self.surface_scale(crate::fractional::SurfaceKind::Card),
                    self.card.width(),
                ));
                self.sync_card_input();
            } else {
                self.card.at = Some(at);
                self.card.rect = Some(card_rect(
                    &spot,
                    Some(at / spot.w),
                    self.surface_scale(crate::fractional::SurfaceKind::Card),
                    self.card.width(),
                ));
                moving = true;
            }
        }
        self.card_last_frame = moving.then_some(now);
        if self.card.leaving && shown <= 0.0 {
            self.card.leaving = false;
            self.card.host = None;
            self.card.spot = None;
            self.card.rect = None;
            self.sync_card_input();
        }
        let rect = self.card.rect.unwrap_or_default();
        // The box's colour is read on one side of the screen or the other
        // (`box_surface_at`). By the WINDOW's place, not the card's own: a
        // card crossing the screen's middle changed colour in mid-flight,
        // which read as a hole in the glide (Max, 2026-10-08: *"i see a
        // 'hole' on the animation when the card cross the middle of the
        // window"*). Its window does not move while it slides.
        let paint = {
            let side = match self.card.spot {
                Some(s) => Rect::new(s.x, s.y, s.w, s.h),
                None => rect,
            };
            let (fill, ink) = self.box_surface_at(side);
            Paint { fill, ink }
        };
        let Some(renderer) = self.card_renderer.as_mut() else {
            return;
        };
        // Wrap what has not been wrapped yet: a text by the column (its
        // font is fixed-pitch), a name by its own average glyph.
        let text_w = self.card.width() - 2.0 * LIST_PAD - 2.0 * TILE_PAD_X;
        let mono = renderer.measure_text("MMMMMMMMMM", TEXT_PX, Some(crate::options::NERD)) / 10.0;
        for item in &self.card.items {
            if self.card.lines.contains_key(&item.id) {
                continue;
            }
            let lines = if item.kind == Kind::Text {
                let cols = if mono > 0.0 {
                    (text_w / mono).floor() as usize
                } else {
                    40
                };
                wrap(&item.body, cols, MAX_LINES)
            } else {
                let n = item.body.chars().count().max(1);
                let w = renderer.measure_text(&item.body, TEXT_PX, None);
                let cols = if w > 0.0 {
                    (text_w * 0.94 / (w / n as f32)).floor() as usize
                } else {
                    40
                };
                wrap(&item.body, cols, 3)
            };
            self.card.lines.insert(item.id, lines);
        }
        let hover = self.card.ptr.and_then(|p| self.card.hit(p));
        // A list that was showing its newest item keeps showing it when
        // the card's height changes under it (its window was resized).
        if self.card.max_scroll > 0.0 && self.card.scroll >= self.card.max_scroll - 0.5 {
            self.card.to_bottom = true;
        }
        let view = View {
            rect,
            shown,
            items: &self.card.items,
            lines: &self.card.lines,
            scroll: if self.card.to_bottom {
                f32::MAX
            } else {
                self.card.scroll
            },
            hover: if self.card.drag.is_some() {
                None
            } else {
                hover
            },
            dnd_over: self.card.dnd_over,
            slots: &self.card.slots,
            paint,
        };
        let (scene, tiles, max_scroll) = scene(&view);
        if shown > 0.0 {
            self.card.max_scroll = max_scroll;
            self.card.scroll = if self.card.to_bottom {
                max_scroll
            } else {
                self.card.scroll.clamp(0.0, max_scroll)
            };
            self.card.to_bottom = false;
            self.card.tiles = tiles;
        } else {
            self.card.tiles.clear();
        }
        let (layer, qh, pending) = (
            self.card_layer.as_ref(),
            &self.qh,
            &mut self.card_frame_pending,
        );
        let mut presented = false;
        match renderer.render(
            &scene,
            paint.ink,
            None,
            0.0,
            0,
            self.card_visible.as_mut(),
            &mut || {
                if let (Some(layer), false) = (layer, *pending) {
                    let surface = layer.wl_surface();
                    surface.frame(qh, surface.clone());
                    *pending = true;
                }
            },
        ) {
            Ok(crate::renderer::Frame::Presented) => presented = true,
            Ok(crate::renderer::Frame::Unchanged) => {}
            Err(e) => warn!("card render failed: {e:#}"),
        }
        if moving {
            // The unroll's next frame: on the frame callback when this one
            // was presented, on a timer when it changed nothing (the first
            // frame of an ease has dt = 0 — the desktop's lesson).
            self.card_dirty = true;
            if !presented && !self.card_frame_pending {
                self.schedule_card_tick();
            }
        }
    }

    fn schedule_card_tick(&mut self) {
        if self.card_tick_timer {
            return;
        }
        let timer = calloop::timer::Timer::from_duration(std::time::Duration::from_millis(8));
        let armed = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.card_tick_timer = false;
                app.draw_card();
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
        if armed {
            self.card_tick_timer = true;
        }
    }

    /// Route a pointer event on the card's surface.
    pub(crate) fn card_pointer(&mut self, event: wl_pointer::Event) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                self.enter_serial = serial;
                self.cursor_now = None;
                self.card_motion(surface_x as f32, surface_y as f32);
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => self.card_motion(surface_x as f32, surface_y as f32),
            wl_pointer::Event::Leave { .. } => {
                // (Also what the compositor sends the moment a drag of
                // ours starts: the pointer is its from then on.)
                self.pointer_surface = crate::options::PointerSurface::Dock;
                self.card.ptr = None;
                match self.card.press.take() {
                    Some(Press::Slide { moved: true, .. }) => self.card_settle_slide(),
                    Some(Press::Resize { .. }) => self.card_settle_resize(),
                    _ => {}
                }
                self.request_card_draw();
            }
            wl_pointer::Event::Button {
                serial,
                button,
                state: WEnum::Value(state),
                ..
            } if button == crate::BTN_LEFT => match state {
                wl_pointer::ButtonState::Pressed => {
                    let Some(at) = self.card.ptr else {
                        return;
                    };
                    let edge = self.card.grip(at).zip(self.card.rect).zip(self.card.spot);
                    self.card.press = match self.card.hit(at) {
                        // A side edge first: it is the card's, whatever
                        // lies just inside it.
                        _ if edge.is_some() => edge.map(|((left_edge, r), s)| Press::Resize {
                            left_edge,
                            from_x: at.0,
                            left: r.x - s.x,
                            width: r.w,
                        }),
                        Some((id, true)) => Some(Press::Close(id)),
                        Some((id, false)) => Some(Press::Item { id, at, serial }),
                        None => self
                            .card
                            .rect
                            .zip(self.card.spot)
                            .map(|(r, s)| Press::Slide {
                                from_x: at.0,
                                left: r.x - s.x,
                                moved: false,
                            }),
                    };
                    self.card_cursor();
                }
                wl_pointer::ButtonState::Released => {
                    match self.card.press.take() {
                        Some(Press::Close(id)) => {
                            let still = self.card.ptr.and_then(|p| self.card.hit(p));
                            if still == Some((id, true)) {
                                self.card_remove(id);
                            }
                        }
                        Some(Press::Slide { moved: true, .. }) => self.card_settle_slide(),
                        Some(Press::Resize { .. }) => self.card_settle_resize(),
                        _ => {}
                    }
                    self.card_cursor();
                }
                _ => {}
            },
            // The fingers left the touchpad: the sideways scroll is over
            // now, with nothing to wait out.
            wl_pointer::Event::AxisStop {
                axis: WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                ..
            } => self.card_swipe_judge(),
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => match axis {
                wl_pointer::Axis::VerticalScroll => self.card_wheel(0.0, value as f32),
                wl_pointer::Axis::HorizontalScroll => self.card_wheel(value as f32, 0.0),
                _ => {}
            },
            _ => {}
        }
    }

    /// A scroll over the card: up and down moves the list, sideways moves
    /// the card itself (Max, 2026-10-08: *"im on top of the card, i scroll to
    /// the sides, the card moves, until i let go"*). One or the other for the
    /// length of a gesture — the way that travels [`SCROLL_CLAIM`] first.
    fn card_wheel(&mut self, across: f32, along: f32) {
        let now = std::time::Instant::now();
        if self.card.wheel.until.is_some_and(|t| t < now) {
            self.card_wheel_end();
        }
        self.card.wheel.until = Some(now + SCROLL_QUIET);
        if !self.card.wheel.waiting {
            self.card_wheel_wait(SCROLL_QUIET);
        }
        if self.card.wheel.sideways.is_none() {
            self.card.wheel.across += across;
            self.card.wheel.along += along;
            let (a, l) = (self.card.wheel.across.abs(), self.card.wheel.along.abs());
            if a.max(l) < SCROLL_CLAIM {
                return;
            }
            self.card.wheel.sideways = Some(a > l);
            // What was gathered while deciding is not thrown away.
            let (across, along) = (self.card.wheel.across, self.card.wheel.along);
            if a > l {
                self.sync_card_input();
                self.card_wheel_apply(across, 0.0);
            } else {
                self.card_wheel_apply(0.0, along);
            }
            return;
        }
        self.card_wheel_apply(across, along);
    }

    fn card_wheel_apply(&mut self, across: f32, along: f32) {
        match self.card.wheel.sideways {
            Some(true) if across != 0.0 => {
                if let Some(host) = self.card.host.clone() {
                    self.card_slide(&host, across);
                }
            }
            Some(false) if along != 0.0 => {
                let to = (self.card.scroll + along * 2.4).clamp(0.0, self.card.max_scroll);
                if to != self.card.scroll {
                    self.card.scroll = to;
                    self.request_card_draw();
                }
            }
            _ => {}
        }
    }

    /// The scroll gesture is over: the next one chooses its way afresh, and
    /// the pointer is the card's box's alone again.
    fn card_wheel_end(&mut self) {
        let was_sideways = self.card.wheel.sideways == Some(true);
        let waiting = self.card.wheel.waiting;
        self.card.wheel = Wheel {
            waiting,
            ..Default::default()
        };
        if was_sideways {
            self.sync_card_input();
        }
    }

    fn card_wheel_wait(&mut self, wait: std::time::Duration) {
        let timer = calloop::timer::Timer::from_duration(wait);
        self.card.wheel.waiting = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.card.wheel.waiting = false;
                let now = std::time::Instant::now();
                match app.card.wheel.until {
                    Some(until) if until > now => app.card_wheel_wait(until - now),
                    _ => app.card_wheel_end(),
                }
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
    }

    fn card_motion(&mut self, x: f32, y: f32) {
        let before = self.card.ptr.and_then(|p| self.card.hit(p));
        self.card.ptr = Some((x, y));
        match self.card.press {
            Some(Press::Item { id, at, serial }) if (x - at.0).hypot(y - at.1) >= DRAG_START => {
                self.card_lift(id, serial);
                return;
            }
            Some(Press::Resize {
                left_edge,
                from_x,
                left,
                width,
            }) => {
                // The edge in hand follows the pointer; the other stays.
                let dx = x - from_x;
                let to = (if left_edge { width - dx } else { width + dx })
                    .clamp(WIDTH_MIN, WIDTH_MAX)
                    .round();
                if let (Some(rect), Some(spot)) = (self.card.rect.as_mut(), self.card.spot) {
                    if to != rect.w {
                        rect.w = to;
                        if left_edge {
                            rect.x = spot.x + left + (width - to);
                        }
                        if let Some(host) = self.card.host.clone() {
                            self.card.widths.insert(host, to);
                        }
                        // The text is wrapped to the width: wrap it again.
                        self.card.lines.clear();
                        self.card.at = None;
                        self.request_card_draw();
                    }
                }
                return;
            }
            Some(Press::Slide {
                from_x,
                left,
                moved,
            }) => {
                let travelled = moved || (x - from_x).abs() >= DRAG_START;
                if travelled {
                    if let (Some(rect), Some(spot)) = (self.card.rect.as_mut(), self.card.spot) {
                        rect.x = (spot.x + clamp_left(left + x - from_x, spot.w, rect.w)).round();
                    }
                    self.card.press = Some(Press::Slide {
                        from_x,
                        left,
                        moved: true,
                    });
                    self.request_card_draw();
                }
                return;
            }
            _ => {}
        }
        if before != self.card.hit((x, y)) {
            self.request_card_draw();
        } else {
            self.card_cursor();
        }
    }

    /// The card's window was taken in hand — a move or a resize began (the
    /// plugin watches the compositor's drag and says so): the card is gone
    /// at once, with no roll-up (Max, 2026-10-08: *"make it hide instantly
    /// when i move the window, and then come back when i drop the window…
    /// we don't need it following the window"*).
    pub(crate) fn card_window_lifted(&mut self, addr: &str) {
        if self.card.host.as_deref() != Some(addr) || self.card.leaving {
            return;
        }
        self.card.lifted = Some(std::time::Instant::now());
        self.card.shown = 0.0;
        self.card.press = None;
        self.sync_card_input();
        self.request_card_draw();
    }

    /// The window was put down: the card comes back on it, at the same
    /// place relative to the window, unrolling as when it is summoned. At
    /// once: a 220 ms wait before the reveal was tried and taken back the
    /// same day (Max, 2026-10-08: *"i dont think we need the delay"*).
    pub(crate) fn card_window_placed(&mut self, addr: &str) {
        if self.card.host.as_deref() != Some(addr) || self.card.lifted.take().is_none() {
            return;
        }
        self.card.shown = 0.0;
        self.card_last_frame = None;
        self.sync_card();
    }

    /// The card's window was moved a step by a gesture with no end of its
    /// own (the scroll on the OPTIONS pill): in hand from the first step,
    /// put down when the steps have stopped for [`NUDGE_QUIET`].
    pub(crate) fn card_window_nudged(&mut self, addr: &str) {
        if self.card.host.as_deref() != Some(addr) {
            return;
        }
        let waiting = self.card.nudge_until.is_some();
        self.card.nudge_until = Some(std::time::Instant::now() + NUDGE_QUIET);
        if self.card.lifted.is_none() {
            self.card_window_lifted(addr);
        }
        if !waiting {
            self.card_nudge_wait(NUDGE_QUIET);
        }
    }

    fn card_nudge_wait(&mut self, wait: std::time::Duration) {
        let timer = calloop::timer::Timer::from_duration(wait);
        let armed = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                let now = std::time::Instant::now();
                match app.card.nudge_until {
                    // Scrolled again since: wait out the rest.
                    Some(until) if until > now => app.card_nudge_wait(until - now),
                    _ => {
                        app.card.nudge_until = None;
                        if let Some(host) = app.card.host.clone() {
                            app.card_window_placed(&host);
                        }
                    }
                }
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
        if !armed {
            self.card.nudge_until = None;
        }
    }

    /// A scroll on the title bar of the card's window (the plugin hears it
    /// and sends it on): the card slides sideways by it, down or right
    /// taking it left as in the mockup, and stays where it is left —
    /// remembered for this window like a slide by hand.
    pub(crate) fn card_slide(&mut self, addr: &str, delta: f32) {
        if self.card.host.as_deref() != Some(addr) || !self.card_present() {
            return;
        }
        let (Some(rect), Some(spot)) = (self.card.rect, self.card.spot) else {
            return;
        };
        if spot.w <= 0.0
            || matches!(
                self.card.press,
                Some(Press::Slide { .. } | Press::Resize { .. })
            )
        {
            return;
        }
        // Measure the run for a throw: judged when it stops (`card_swipe_wait`).
        let now = std::time::Instant::now();
        self.card.swipe = Some(match self.card.swipe {
            Some(sw) if now - sw.last <= FLING_QUIET => Swipe {
                last: now,
                sum: sw.sum + delta,
                turned: sw.turned || sw.sum * delta < 0.0,
                ..sw
            },
            _ => Swipe {
                began: now,
                last: now,
                sum: delta,
                turned: false,
            },
        });
        if !self.card.swipe_waiting {
            self.card_swipe_wait(FLING_QUIET);
        }
        // Where the scroll says the card should be (from the remembered
        // fraction, so small scrolls add up) — and the card sets off for it
        // from where it is, at its own pace (`TRAVEL_SPEED`).
        let left = match self.card.geom.get(addr) {
            Some(fx) => fx * spot.w,
            None => rect.x - spot.x,
        };
        let fx = clamp_left(left - delta * SLIDE_PER_SCROLL, spot.w, rect.w) / spot.w;
        self.card.geom.insert(addr.to_owned(), fx);
        self.card_set_off(rect.x - spot.x);
    }

    /// The card's place changed: it travels there from `from` (where it is
    /// drawn now), unless it is already on its way.
    fn card_set_off(&mut self, from: f32) {
        if self.card.at.is_none() {
            self.card.at = Some(from);
            self.card.speed = 0.0;
            self.card_last_frame = None;
        }
        self.request_card_draw();
    }

    /// The sliding scroll has stopped: a brief one was a throw, and the
    /// card glides to that side.
    fn card_swipe_judge(&mut self) {
        let Some(sw) = self.card.swipe.take() else {
            return;
        };
        if let (Some(to_left), Some(spot), Some(host), Some(rect)) = (
            sw.thrown(),
            self.card.spot,
            self.card.host.clone(),
            self.card.rect,
        ) {
            if self.card_present() && spot.w > 0.0 {
                // Its place is the far side from now on; it carries on
                // there at the pace it was already travelling.
                self.card
                    .geom
                    .insert(host, fling_end(spot.w, to_left, rect.w) / spot.w);
                self.card_set_off(rect.x - spot.x);
            }
        }
    }

    /// Wait for the sliding scroll to stop, then judge it: a brief one was
    /// a throw, and the card glides to that side.
    fn card_swipe_wait(&mut self, wait: std::time::Duration) {
        let timer = calloop::timer::Timer::from_duration(wait);
        self.card.swipe_waiting = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.card.swipe_waiting = false;
                let now = std::time::Instant::now();
                match app.card.swipe {
                    Some(sw) if now - sw.last < FLING_QUIET => {
                        app.card_swipe_wait(FLING_QUIET - (now - sw.last));
                    }
                    Some(_) => app.card_swipe_judge(),
                    None => {}
                }
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
    }

    /// A side edge was let go: the card keeps its new width on this window
    /// and stays where it is.
    fn card_settle_resize(&mut self) {
        if let (Some(host), Some(rect), Some(spot)) =
            (self.card.host.clone(), self.card.rect, self.card.spot)
        {
            if spot.w > 0.0 {
                self.card.geom.insert(host, (rect.x - spot.x) / spot.w);
            }
        }
        self.sync_card_input();
        self.request_card_draw();
    }

    /// The card was let go after a slide: it stays exactly there (no
    /// snapping — the mockup's rule), remembered for this window.
    fn card_settle_slide(&mut self) {
        if let (Some(host), Some(rect), Some(spot)) =
            (self.card.host.clone(), self.card.rect, self.card.spot)
        {
            if spot.w > 0.0 {
                self.card.geom.insert(host, (rect.x - spot.x) / spot.w);
            }
        }
        self.sync_card_input();
        self.request_card_draw();
    }

    /// The pointer's shape over the card: a hand on the card itself (it
    /// can be slid), closed while it is; a finger on a ×.
    fn card_cursor(&mut self) {
        if self.pointer_surface != crate::options::PointerSurface::Card || self.card.ptr.is_none() {
            return;
        }
        let Some(device) = &self.cursor_device else {
            return;
        };
        let hit = self.card.ptr.and_then(|p| self.card.hit(p));
        let grip = self.card.ptr.and_then(|p| self.card.grip(p));
        let shape = match (self.card.press, hit) {
            (Some(Press::Resize { .. }), _) => Shape::EwResize,
            (Some(Press::Slide { .. }), _) => Shape::Grabbing,
            (None, _) if grip.is_some() => Shape::EwResize,
            (_, Some((_, true))) => Shape::Pointer,
            (_, Some((_, false))) => Shape::Default,
            (_, None) => Shape::Grab,
        };
        if self.cursor_now != Some(shape) {
            device.set_shape(self.enter_serial, shape);
            self.cursor_now = Some(shape);
        }
    }

    /// Take item `id` into a Wayland drag: any app it is let go on gets a
    /// copy of it.
    fn card_lift(&mut self, id: u64, serial: u32) {
        self.card.press = None;
        let Some(item) = self.card.item(id).cloned() else {
            return;
        };
        let (Some(manager), Some(device), Some(layer)) = (
            self.data_device_manager.as_ref(),
            self.data_device.as_ref(),
            self.card_layer.as_ref(),
        ) else {
            warn!("card: no data device; items cannot be dragged out");
            return;
        };
        let source =
            manager.create_drag_and_drop_source(&self.qh, out_mimes(&item), DndAction::Copy);
        // A picture travels as itself; anything else under the bare pointer.
        let image = item
            .path
            .as_ref()
            .and_then(|p| self.card.chains.get(p))
            .and_then(|chain| self.drag_image(chain, (24.0, 24.0), self.icon_scale()));
        source.start_drag(
            device,
            layer.wl_surface(),
            image.as_ref().map(DragIcon::surface),
            serial,
        );
        if let Some(image) = image.as_ref() {
            image.surface().commit();
        }
        info!(
            "card: {:?} in hand",
            item.body.chars().take(60).collect::<String>()
        );
        self.card.drag = Some(Drag {
            id,
            source,
            _icon: image,
        });
        self.request_card_draw();
    }

    /// Whether `source` is the card's drag.
    pub(crate) fn is_card_drag_source(
        &self,
        source: &wayland_client::protocol::wl_data_source::WlDataSource,
    ) -> bool {
        self.card
            .drag
            .as_ref()
            .is_some_and(|d| d.source.inner() == source)
    }

    /// Another app asked for the item in hand, as `mime`. Written off the
    /// loop (the other side reads when it pleases).
    pub(crate) fn card_send_drag(&mut self, mime: &str, pipe: WritePipe) {
        let item = self.card.drag.as_ref().and_then(|d| self.card.item(d.id));
        let Some(payload) = item.and_then(|it| payload(it, mime)) else {
            warn!("card: {mime} asked of a drag that never offered it");
            return;
        };
        let fd: std::os::fd::OwnedFd = pipe.into();
        let mime = mime.to_owned();
        std::thread::spawn(move || {
            use std::io::Write;
            let mut file = std::fs::File::from(fd);
            let bytes = match payload {
                Payload::Bytes(bytes) => Ok(bytes),
                Payload::File(path) => std::fs::read(path),
            };
            match bytes {
                Ok(bytes) => {
                    if let Err(e) = file.write_all(&bytes) {
                        warn!("card: writing the dragged item's {mime} failed: {e}");
                    }
                }
                Err(e) => warn!("card: the dragged file cannot be read: {e}"),
            }
        });
    }

    /// The card's drag is over, dropped or not: the item never left.
    pub(crate) fn card_drag_end(&mut self, done: bool) {
        if self.card.drag.take().is_some() {
            info!("card: {}", if done { "dropped" } else { "drag cancelled" });
            self.request_card_draw();
        }
    }

    /// A drag came over the card. Another app's (or a desktop icon's) is
    /// taken if it carries anything the card keeps (files, a picture, text)
    /// — as a copy; the card's own is refused (an item does not land on
    /// itself).
    pub(crate) fn card_dnd_enter(&mut self, offer: DragOffer) {
        let mimes = offer.with_mime_types(|m| m.to_vec());
        let own = self.card.drag.is_some();
        let first = if mimes.iter().any(|m| m == URI_LIST) {
            Some(URI_LIST)
        } else {
            image_mime(&mimes)
                .map(|(m, _)| m)
                .or_else(|| text_mime(&mimes))
        };
        if !own {
            info!(
                "card: a drag came over offering {mimes:?}, actions {:?}",
                offer.source_actions
            );
        }
        let Some(first) = first.filter(|_| !own) else {
            offer.accept_mime_type(offer.serial, None);
            return;
        };
        offer.accept_mime_type(offer.serial, Some(first.to_owned()));
        offer.set_actions(DndAction::Copy | DndAction::Move, DndAction::Copy);
        self.card.dnd_over = true;
        self.card.dnd_mimes = mimes;
        self.card.dnd_hint = None;
        self.card_dnd_offer = Some(offer);
        self.request_card_draw();
    }

    /// Whether a drag is over the card (the data device's events are the
    /// card's then, not the desktop's).
    pub(crate) fn card_dnd_active(&self) -> bool {
        self.card_dnd_offer.is_some()
    }

    /// The drag left — unless it has just been DROPPED here: Hyprland
    /// sends `leave` right after `drop`, while what was dropped is still
    /// on its way through the pipe (see `desktop_dnd_leave`).
    pub(crate) fn card_dnd_leave(&mut self) {
        let dropped = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
            .is_some_and(|o| o.dropped);
        if dropped {
            return;
        }
        if self.card_dnd_offer.take().is_some() {
            self.card.dnd_over = false;
            self.request_card_draw();
        }
    }

    /// Let go on the card: read what it carries, the richest first — the
    /// list of files; a picture's pixels; the text.
    pub(crate) fn card_dnd_drop(&mut self) {
        if self.card_dnd_offer.is_none() {
            return;
        }
        self.card.dnd_over = false;
        self.request_card_draw();
        let first = if self.card.dnd_mimes.iter().any(|m| m == URI_LIST) {
            Some(URI_LIST)
        } else {
            image_mime(&self.card.dnd_mimes)
                .map(|(m, _)| m)
                .or_else(|| text_mime(&self.card.dnd_mimes))
        };
        match first {
            Some(mime) => self.card_dnd_read(mime),
            None => self.card_dnd_end(false),
        }
    }

    /// Ask the dragging app for `mime` and read it off the loop.
    fn card_dnd_read(&mut self, mime: &'static str) {
        // The live offer where there is one (the one kept since `enter` is
        // a snapshot).
        let live = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
            .filter(|o| o.dropped);
        let Some(offer) = live.or_else(|| self.card_dnd_offer.clone()) else {
            return;
        };
        let pipe = match offer.receive(mime.to_owned()) {
            Ok(pipe) => pipe,
            Err(e) => {
                warn!("card: cannot receive the drop as {mime}: {e}");
                self.card_dnd_end(false);
                return;
            }
        };
        // Through `OwnedFd`, never `into_raw_fd` (SCTK closes the pipe).
        let fd: std::os::fd::OwnedFd = pipe.into();
        let (tx, rx) = calloop::channel::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut bytes = Vec::new();
            if let Err(e) = std::fs::File::from(fd)
                .take(DROP_MAX)
                .read_to_end(&mut bytes)
            {
                warn!("card: reading the drop's {mime} failed: {e}");
            }
            let _ = tx.send(bytes);
        });
        if self
            .loop_handle
            .insert_source(rx, move |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(bytes) = event {
                    app.card_dnd_received(mime, &bytes);
                }
            })
            .is_err()
        {
            warn!("card: cannot wait for the drop");
            self.card_dnd_end(false);
        }
    }

    /// One type of the drop arrived: keep it, or ask for the next.
    fn card_dnd_received(&mut self, mime: &'static str, bytes: &[u8]) {
        if self.card_dnd_offer.is_none() {
            warn!("card: the drop's {mime} arrived after its drag was gone");
            return;
        }
        info!("card: the drop's {mime} is {} bytes", bytes.len());
        if mime == URI_LIST {
            let list = String::from_utf8_lossy(bytes).into_owned();
            let paths = crate::desktop::uri_list_paths(&list);
            if !paths.is_empty() {
                for path in &paths {
                    self.card_add_path(path);
                }
                self.card_dnd_end(true);
                return;
            }
            // No file of this machine in it: a picture or a link out of a
            // web page. Its pixels if they are offered, else its text —
            // else the addresses themselves.
            self.card.dnd_hint = remote_name(&list);
            if let Some((next, _)) = image_mime(&self.card.dnd_mimes) {
                self.card_dnd_read(next);
            } else if let Some(next) = text_mime(&self.card.dnd_mimes) {
                self.card_dnd_read(next);
            } else {
                self.card_add_text(&list);
                self.card_dnd_end(true);
            }
            return;
        }
        if let Some((_, ext)) = IMAGE_MIMES.into_iter().find(|(m, _)| *m == mime) {
            if bytes.is_empty() {
                // Offered and not delivered: the text, if there is one.
                if let Some(next) = text_mime(&self.card.dnd_mimes) {
                    self.card_dnd_read(next);
                    return;
                }
                self.card_dnd_end(false);
                return;
            }
            self.card_keep_picture(bytes, ext);
            self.card_dnd_end(true);
            return;
        }
        self.card_add_text(&String::from_utf8_lossy(bytes));
        self.card_dnd_end(true);
    }

    /// A picture that came as pixels: saved beside the list, as the card's
    /// own.
    fn card_keep_picture(&mut self, bytes: &[u8], ext: &str) {
        self.card_load();
        let dir = crate::persist::data_path(PICTURES_DIR);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("card: cannot keep the picture ({e})");
            return;
        }
        let path = dir.join(format!("{}.{ext}", self.card.next_id.max(1)));
        if let Err(e) = std::fs::write(&path, bytes) {
            warn!("card: cannot keep the picture ({e})");
            return;
        }
        let name = self
            .card
            .dnd_hint
            .take()
            .unwrap_or_else(|| "picture".to_owned());
        let body = format!("{name} · {}", size_text(bytes.len() as u64));
        self.card_push(
            Kind::Image,
            body,
            Some(path.to_string_lossy().into_owned()),
            true,
        );
    }

    /// The drop is over: tell the dragging app (it stays mid-drag until
    /// it hears), taken or not.
    fn card_dnd_end(&mut self, taken: bool) {
        if let Some(offer) = self.card_dnd_offer.take() {
            if taken {
                offer.finish();
            } else {
                offer.destroy();
            }
        }
        self.card.dnd_over = false;
        self.card.dnd_mimes.clear();
        self.card.dnd_hint = None;
        self.request_card_draw();
    }

    /// `waverunner-ctl card …`: the card without a pointer.
    pub(crate) fn card_command(&mut self, what: &str) -> String {
        let what = what.trim();
        let (verb, rest) = what.split_once(' ').unwrap_or((what, ""));
        let rest = rest.trim();
        match verb {
            "" | "toggle" => self.card_toggle((!rest.is_empty()).then_some(rest)),
            "all" => self.card_toggle_all(),
            "rate" => {
                let mut parts = rest.split_whitespace().map(|p| p.parse::<f32>().ok());
                self.card.pace = match (parts.next().flatten(), parts.next().flatten()) {
                    (Some(speed), accel) if speed > 0.0 => {
                        Some((speed, accel.filter(|a| *a > 0.0).unwrap_or(TRAVEL_ACCEL)))
                    }
                    _ => None,
                };
                let (speed, accel) = self.card.pace.unwrap_or((TRAVEL_SPEED, TRAVEL_ACCEL));
                format!("the card travels at {speed} px/s, gaining {accel} px/s²")
            }
            "lifted" => {
                self.card_window_lifted(rest);
                String::new()
            }
            "slide" => {
                let mut parts = rest.split_whitespace();
                match (
                    parts.next(),
                    parts.next().and_then(|d| d.parse::<f32>().ok()),
                ) {
                    (Some(addr), Some(delta)) if delta.is_finite() => {
                        self.card_slide(addr, delta);
                        String::new()
                    }
                    _ => "slide <addr> <delta>".to_owned(),
                }
            }
            "add" => match rest.split_once(' ') {
                Some(("text", text)) => {
                    self.card_add_text(&text.replace("\\n", "\n"));
                    format!("{} items", self.card.items.len())
                }
                Some(("file", path)) => {
                    self.card_add_path(Path::new(path.trim()));
                    format!("{} items", self.card.items.len())
                }
                _ => "add text <…> | add file <path>".to_owned(),
            },
            "remove" => {
                self.card_load();
                match rest
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| self.card.items.get(n))
                    .map(|it| it.id)
                {
                    Some(id) => {
                        self.card_remove(id);
                        format!("{} items", self.card.items.len())
                    }
                    None => "no such item".to_owned(),
                }
            }
            "clear" => {
                self.card_load();
                let ids: Vec<u64> = self.card.items.iter().map(|it| it.id).collect();
                for id in ids {
                    self.card_remove(id);
                }
                "empty".to_owned()
            }
            "state" => {
                self.card_load();
                let items: Vec<String> = self
                    .card
                    .items
                    .iter()
                    .enumerate()
                    .map(|(n, it)| {
                        format!(
                            "{n}:{:?}:{:?}",
                            it.kind,
                            it.body.chars().take(32).collect::<String>()
                        )
                    })
                    .collect();
                format!(
                    "host {:?} spot {:?} rect {:?} shown {:.2} all {} armed {:?} scroll {:.0}/{:.0} pictures {} items [{}]",
                    self.card.host,
                    self.card.spot,
                    self.card.rect,
                    self.card.shown,
                    self.card.all,
                    self.card.armed,
                    self.card.scroll,
                    self.card.max_scroll,
                    self.card.slots.len(),
                    items.join(", ")
                )
            }
            _ => "card [toggle|all|add text <…>|add file <path>|remove <n>|clear|state]".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot() -> WindowSpot {
        WindowSpot {
            x: 100.0,
            y: 50.0,
            w: 900.0,
            h: 600.0,
            visible: true,
        }
    }

    fn text(id: u64, body: &str) -> Item {
        Item {
            id,
            kind: Kind::Text,
            body: body.to_owned(),
            path: None,
            aspect: 0.0,
            owned: false,
        }
    }

    fn file(id: u64, kind: Kind, path: &str) -> Item {
        Item {
            id,
            kind,
            body: path.rsplit('/').next().unwrap_or(path).to_owned(),
            path: Some(path.to_owned()),
            aspect: 1.5,
            owned: false,
        }
    }

    const PAINT: Paint = Paint {
        fill: [0.16, 0.14, 0.19, 1.0],
        ink: [0.95, 0.94, 0.96, 1.0],
    };

    #[test]
    fn the_card_rests_inside_its_window_on_the_right() {
        let r = card_rect(&spot(), None, 1.0, WIDTH);
        assert_eq!(
            (r.x, r.y, r.w, r.h),
            (100.0 + 900.0 - 10.0 - 320.0, 60.0, 320.0, 580.0)
        );
    }

    #[test]
    fn a_slid_card_keeps_its_fraction_and_may_hang_off_either_side() {
        let s = spot();
        assert_eq!(card_rect(&s, Some(0.5), 1.0, WIDTH).x, 100.0 + 450.0);
        // Fully out on the left with a little air, and on the right.
        assert_eq!(
            card_rect(&s, Some(-5.0), 1.0, WIDTH).x,
            100.0 - 320.0 - 12.0
        );
        assert_eq!(card_rect(&s, Some(5.0), 1.0, WIDTH).x, 100.0 + 900.0 + 12.0);
    }

    #[test]
    fn a_thrown_card_rests_beside_its_window_on_either_side() {
        assert_eq!(fling_end(900.0, true, WIDTH), -(320.0 + 12.0));
        assert_eq!(fling_end(900.0, false, WIDTH), 900.0 + 12.0);
        // Exactly as far as a slide by hand can take it.
        assert_eq!(
            fling_end(900.0, true, WIDTH),
            clamp_left(-9999.0, 900.0, WIDTH)
        );
        assert_eq!(
            fling_end(900.0, false, WIDTH),
            clamp_left(9999.0, 900.0, WIDTH)
        );
    }

    #[test]
    fn the_card_travels_at_a_capped_pace_eases_in_and_arrives_exactly() {
        // From rest it gains speed, no faster than the acceleration allows.
        let (at, speed) = travel(0.0, 5000.0, 0.0, 0.01, 6000.0, 50_000.0);
        assert_eq!(speed, 500.0);
        assert_eq!(at, 5.0);
        // At full pace it never goes faster, however far the place is.
        let (at, speed) = travel(0.0, 100_000.0, 6000.0, 0.01, 6000.0, 50_000.0);
        assert_eq!((at, speed), (60.0, 6000.0));
        // Near its place it slows: its speed is tied to the distance left.
        let (_, speed) = travel(0.0, 50.0, 6000.0, 0.001, 6000.0, 50_000.0);
        assert_eq!(speed, 50.0 * TRAVEL_BRAKE);
        // …but never to nothing, and it stops dead on arrival, either way.
        let (_, speed) = travel(0.0, 0.5, 0.0, 0.001, 6000.0, 1e9);
        assert_eq!(speed, TRAVEL_CREEP);
        assert_eq!(travel(0.2, 0.0, 6000.0, 0.01, 6000.0, 50_000.0), (0.0, 0.0));
        let (at, _) = travel(500.0, 0.0, 6000.0, 0.01, 6000.0, 50_000.0);
        assert!(at < 500.0 && at > 0.0);
    }

    #[test]
    fn the_cards_edges_land_on_whole_screen_pixels() {
        let s = WindowSpot {
            x: 100.3,
            y: 50.0,
            w: 900.0,
            h: 600.0,
            visible: true,
        };
        let r = card_rect(&s, Some(0.5), 1.6, WIDTH);
        assert!(((r.x * 1.6) - (r.x * 1.6).round()).abs() < 1e-3);
        // At 1× that is the whole logical pixel, as before.
        assert_eq!(card_rect(&s, Some(0.5), 1.0, WIDTH).x, 550.0);
    }

    #[test]
    fn a_wider_card_rests_and_is_thrown_by_its_own_width() {
        let r = card_rect(&spot(), None, 1.0, 500.0);
        assert_eq!((r.x, r.w), (100.0 + 900.0 - 10.0 - 500.0, 500.0));
        assert_eq!(fling_end(900.0, true, 500.0), -(500.0 + 12.0));
        // The side edges are grips, inside the card only.
        let card = Card {
            rect: Some(Rect::new(100.0, 50.0, 400.0, 300.0)),
            ..Default::default()
        };
        assert_eq!(card.grip((103.0, 200.0)), Some(true));
        assert_eq!(card.grip((497.0, 200.0)), Some(false));
        assert_eq!(card.grip((300.0, 200.0)), None);
        assert_eq!(card.grip((98.0, 200.0)), None);
        assert_eq!(card.grip((103.0, 20.0)), None);
        // Never changed: the mockup's width; out of range: the limits.
        assert_eq!(card.width(), WIDTH);
        // Its width is the window's it is on, within the limits.
        let mut wide = Card {
            host: Some("0x1".into()),
            ..Default::default()
        };
        wide.widths.insert("0x1".into(), 9999.0);
        wide.widths.insert("0x2".into(), 400.0);
        assert_eq!(wide.width(), WIDTH_MAX);
        wide.host = Some("0x2".into());
        assert_eq!(wide.width(), 400.0);
        wide.host = Some("0x3".into());
        assert_eq!(wide.width(), WIDTH);
    }

    #[test]
    fn only_a_brief_one_way_scroll_is_a_throw() {
        let t0 = std::time::Instant::now();
        let ms = std::time::Duration::from_millis;
        let swipe = |lasted: u64, sum: f32, turned: bool| Swipe {
            began: t0,
            last: t0 + ms(lasted),
            sum,
            turned,
        };
        // A flick: over in a moment.
        assert_eq!(swipe(90, 80.0, false).thrown(), Some(true));
        assert_eq!(swipe(90, -80.0, false).thrown(), Some(false));
        // Moving the card fast by hand: much more scroll, but it lasts.
        assert_eq!(swipe(600, 900.0, false).thrown(), None);
        // A small nudge, and a scroll that turned back.
        assert_eq!(swipe(60, 12.0, false).thrown(), None);
        assert_eq!(swipe(90, 80.0, true).thrown(), None);
    }

    #[test]
    fn wrap_breaks_at_spaces_keeps_line_breaks_and_indentation() {
        assert_eq!(wrap("one two three", 7, 9), ["one two", "three"]);
        assert_eq!(wrap("a\n\n  b", 10, 9), ["a", "", "  b"]);
        // No space to break at: mid-word.
        assert_eq!(wrap("abcdefghij", 4, 9), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_stops_at_the_cap_with_an_ellipsis() {
        let lines = wrap("1\n2\n3\n4", 10, 2);
        assert_eq!(lines, ["1", "2…"]);
        assert_eq!(wrap("1\n2", 10, 2), ["1", "2"]);
    }

    #[test]
    fn a_text_goes_out_as_text_and_a_picture_as_file_pixels_and_path() {
        let t = text(1, "hello");
        assert_eq!(out_mimes(&t), TEXT_MIMES.to_vec());
        assert_eq!(
            payload(&t, "text/plain"),
            Some(Payload::Bytes(b"hello".to_vec()))
        );
        assert_eq!(payload(&t, URI_LIST), None);

        let p = file(2, Kind::Image, "/tmp/a b.JPEG");
        assert_eq!(out_mimes(&p)[..2], [URI_LIST, "image/jpeg"]);
        assert_eq!(
            payload(&p, URI_LIST),
            Some(Payload::Bytes(b"file:///tmp/a%20b.JPEG\r\n".to_vec()))
        );
        assert_eq!(
            payload(&p, "image/jpeg"),
            Some(Payload::File(PathBuf::from("/tmp/a b.JPEG")))
        );
        assert_eq!(payload(&p, "image/png"), None);
        assert_eq!(
            payload(&p, "UTF8_STRING"),
            Some(Payload::Bytes(b"/tmp/a b.JPEG".to_vec()))
        );

        // A plain file never offers pixels.
        let f = file(3, Kind::File, "/tmp/notes.png.txt");
        assert_eq!(out_mimes(&f)[0], URI_LIST);
        assert!(!out_mimes(&f).contains(&"image/png"));
    }

    #[test]
    fn sizes_read_as_the_mockup_writes_them() {
        assert_eq!(size_text(6), "6 B");
        assert_eq!(size_text(10_035), "9.8 KB");
        assert_eq!(size_text(2_516_582), "2.4 MB");
        assert_eq!(size_text(29_360_128), "28.0 MB");
        assert_eq!(size_text(4_402_341_478), "4.1 GB");
    }

    #[test]
    fn a_drop_is_read_as_its_richest_type() {
        let m = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            text_mime(&m(&["STRING", "text/plain;charset=utf-8"])),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(text_mime(&m(&["text/html"])), None);
        assert_eq!(
            image_mime(&m(&["text/html", "image/jpeg"])),
            Some(("image/jpeg", "jpg"))
        );
    }

    #[test]
    fn a_web_picture_is_named_after_the_end_of_its_address() {
        assert_eq!(
            remote_name("https://upload.example.org/a/Nokota%20Horses.jpg?w=500\r\n").as_deref(),
            Some("Nokota Horses.jpg")
        );
        assert_eq!(remote_name("file:///home/x/a.png"), None);
    }

    #[test]
    fn the_scene_lists_items_top_down_and_scrolls_to_the_newest() {
        let items = vec![
            text(1, "a"),
            file(2, Kind::File, "/tmp/x.txt"),
            file(3, Kind::Image, "/tmp/p.png"),
        ];
        let lines: HashMap<u64, Vec<String>> = items
            .iter()
            .map(|it| (it.id, vec![it.body.clone()]))
            .collect();
        let slots = HashMap::from([("/tmp/p.png".to_owned(), 4u32)]);
        let rect = Rect::new(600.0, 60.0, WIDTH, 200.0);
        let view = |scroll: f32, shown: f32| View {
            rect,
            shown,
            items: &items,
            lines: &lines,
            scroll,
            hover: Some((2, true)),
            dnd_over: false,
            slots: &slots,
            paint: PAINT,
        };
        let (scene, tiles, max_scroll) = scene(&view(0.0, 1.0));
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[0].rect.y, 60.0 + LIST_PAD);
        assert!(tiles[1].rect.y > tiles[0].rect.y && tiles[2].rect.y > tiles[1].rect.y);
        // Taller than the card: it scrolls, by exactly the overflow.
        let total = 2.0 * LIST_PAD
            + tile_height(Kind::Text, 1)
            + tile_height(Kind::File, 1)
            + tile_height(Kind::Image, 1)
            + 2.0 * GAP;
        assert_eq!(max_scroll, total - 200.0);
        // The picture is drawn from its layer, the × on the hovered item.
        assert!(scene.grids[1].icons.iter().any(|i| i.layer == 4));
        assert!(scene.grids[1].labels.iter().any(|l| l.text == "×"));
        // Scrolled past the end shows the newest at the bottom edge.
        let (_, tiles, _) = super::scene(&view(f32::MAX, 1.0));
        let last = tiles[2].rect;
        assert!((last.y + last.h - (rect.y + rect.h - LIST_PAD)).abs() <= 1.0);
        // Rolled up: nothing at all.
        let (scene, tiles, _) = super::scene(&view(0.0, 0.0));
        assert!(scene.grids.is_empty() && tiles.is_empty());
    }

    #[test]
    fn the_unroll_lets_the_card_through_from_the_top() {
        let items = vec![text(1, "a")];
        let lines = HashMap::from([(1, vec!["a".to_owned()])]);
        let slots = HashMap::new();
        let rect = Rect::new(0.0, 0.0, WIDTH, 400.0);
        let (scene, _, _) = scene(&View {
            rect,
            shown: 0.25,
            items: &items,
            lines: &lines,
            scroll: 0.0,
            hover: None,
            dnd_over: true,
            slots: &slots,
            paint: PAINT,
        });
        assert_eq!(scene.grids[0].clip.h, 100.0);
        // The card under the clip is whole, and wears the drag's rim.
        assert_eq!(scene.grids[0].rects[0].rect.h, 400.0);
        assert_eq!(scene.grids[0].rects[1].color[..3], ACCENT);
    }

    #[test]
    fn the_card_is_on_for_a_window_one_by_one_or_for_all_but() {
        let mut card = Card::default();
        assert!(!card.armed("0x1"));
        card.armed.insert("0x1".into());
        assert!(card.armed("0x1") && !card.armed("0x2"));
        card.all = true;
        card.off.insert("0x3".into());
        assert!(card.armed("0x2") && !card.armed("0x3"));
    }

    #[test]
    fn the_list_survives_the_disk() {
        let saved = Saved {
            next_id: 9,
            items: vec![text(1, "a\nb"), file(2, Kind::Folder, "/tmp/d")],
        };
        let json = serde_json::to_string(&saved).unwrap();
        let back: Saved = serde_json::from_str(&json).unwrap();
        assert_eq!(back.items, saved.items);
        assert_eq!(back.next_id, 9);
        // (A list saved while the width was kept in it still reads.)
        let old: Saved = serde_json::from_str(r#"{"next_id":1,"items":[],"width":400.0}"#).unwrap();
        assert_eq!(old.next_id, 1);
        // A text has no path on disk at all.
        assert!(!json.contains("\"path\":null"));
    }
}

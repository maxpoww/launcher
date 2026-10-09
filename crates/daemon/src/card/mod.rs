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
//! the compositor ([`crate::hypr::window_spot`]), read when something says
//! it may have changed. The card does not FOLLOW a window that is moving:
//! while its window is in hand, or the workspaces are being swiped, it is
//! simply away, and comes back where the window ends up (`Away`). Being a
//! layer, it draws over a window that overlaps its own.
//!
//! The code is in four parts: `model` (the items), `view` (one frame as a
//! scene), `place` (where on the window, and the pace it travels at), and
//! this file — the card's state and everything that touches the loop.
//!
//! Items: a text is kept whole; a file or a folder is kept as its path (the
//! card points at it, it does not copy it); a picture that came as pixels
//! (out of a web page) is saved beside the list. A drag out is a real
//! Wayland drag of ours, always a COPY: the item stays.
//!
//! Pointer-free: `waverunner-ctl card [toggle [addr]|all|add text <…>|add file
//! <path>|remove <n>|clear|state|rate <speed> [accel]]`. The plugin's own
//! words to us ride the same verb: `slide <addr> <delta>`, `lifted <addr>`,
//! `away`, `back`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use smithay_client_toolkit::data_device_manager::data_source::DragSource;
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{debug, error, info, warn};
use wayland_client::protocol::wl_pointer;
use wayland_client::WEnum;

use crate::content::Rect;
use crate::desktop::DragIcon;
use crate::hypr::WindowSpot;
use crate::App;

mod bar;
mod dnd;
mod model;
mod place;
mod view;
mod voice;

use bar::*;
use model::*;
use place::*;
use view::*;

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

/// How long after a workspace swipe's fingers lift the card stays away: the
/// compositor is still sliding the workspaces into place.
// 320 at first: the card came a beat after the workspace had landed (Max:
// *"the reveal should be done when i land on the workspace"*) → 130, so its
// own unroll finishes about as the slide does.
const SWIPE_SETTLE: std::time::Duration = std::time::Duration::from_millis(130);

/// A window shorter than this has no room for a card.
const MIN_HEIGHT: f32 = 90.0;

/// How tall a place the list opens for something dragged in from outside
/// (an item of the card's own gets a place its own height).
const DROP_OPENING: f32 = 44.0;
/// How fast the items move aside for it (the ease's rate, per second).
const SHIFT_RATE: f32 = 26.0;
/// A press that travels this far takes the item (or the card) along.
const DRAG_START: f32 = 6.0;
/// A click's paste waits this long for the clipboard to be the item's (and
/// longer when the card's window has to be given the keyboard first).
const PASTE_SETTLE: u64 = 70;
const PASTE_FOCUS: u64 = 160;
/// Apps that paste with Ctrl+Shift+V (Ctrl+V is theirs to pass on). NOT
/// foot: Golem's own terminal is set to paste with Ctrl+V
/// (`clipboard-paste=Control+v`), and Ctrl+Shift+V does nothing there.
const SHIFT_PASTERS: [&str; 6] = [
    "kitty",
    "alacritty",
    "org.wezfurlong.wezterm",
    "com.mitchellh.ghostty",
    "org.gnome.console",
    "xterm",
];

/// The items' zoom: how far it goes and how much a key press changes it.
const ZOOM_MIN: f32 = 0.7;
const ZOOM_MAX: f32 = 2.2;
const ZOOM_STEP: f32 = 0.1;
/// The keys that zoom while the pointer is on the card, as the compositor
/// names them, and what each does (`card zoom <…>`). Plus is its own key on
/// some layouts and Shift+= on others; both, and the keypad.
const ZOOM_KEYS: [(&str, &str); 8] = [
    ("CTRL + plus", "in"),
    ("CTRL + SHIFT + plus", "in"),
    ("CTRL + equal", "in"),
    ("CTRL + KP_Add", "in"),
    ("CTRL + minus", "out"),
    ("CTRL + KP_Subtract", "out"),
    ("CTRL + 0", "reset"),
    ("CTRL + KP_0", "reset"),
];

/// The ids of the clipboard page's rows start here (a clip's own id is
/// added): far from the card's own.
const CLIP_IDS: u64 = 1 << 62;

/// How many window titles are remembered with their session.
const TITLES_MAX: usize = 300;

/// Picture layers in the card's texture array; past it the oldest is
/// reused.
const PIC_LAYERS: u32 = 32;

/// The safety net: how often a card that is SHOWING asks where its window
/// is. Everything that moves a window says so (the compositor's events, the
/// plugin's drag and swipe notices, the pill's gestures) — this catches the
/// rest (a key binding that moves or resizes), and is what brings the card
/// back if a window "in hand" is never reported put down. It was 250 ms,
/// from when the card trailed a dragged window: two round trips to the
/// compositor four times a second for as long as a card was on any window.
const POLL: Duration = Duration::from_secs(2);
/// A window "in hand" this long without moving was put down unheard.
const LOST_DRAG: Duration = Duration::from_secs(6);
/// The size the card's renderer is shrunk to while there is no card
/// anywhere: its surface is the whole output's, and holding that much video
/// memory for an empty picture is a waste.
const PARKED: u32 = 8;

/// The left button, held.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Press {
    /// On a button of any kind (a pin, a tab, the search, a button of the
    /// input box…): let go there and it acts.
    Tap(Hover),
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

/// A scroll gesture over the card: which way took it (see [`SCROLL_CLAIM`])
/// and what each way has travelled before one did. It lasts until the scroll
/// has been quiet for [`SCROLL_QUIET`] (`Wait::Wheel`).
#[derive(Debug, Default)]
struct Wheel {
    /// `Some(true)`: it slides the card; `Some(false)`: it scrolls the list.
    sideways: Option<bool>,
    across: f32,
    along: f32,
}

/// An item in hand: a Wayland drag of ours.
pub(crate) struct Drag {
    pub id: u64,
    /// How far down the surface it was picked up: where the list's
    /// opening is until the drag's first motion says better.
    pub from_y: f32,
    pub source: DragSource,
    _icon: Option<DragIcon>,
}

/// What the card keeps for ONE window (by its address) for as long as that
/// window is open: whether it was turned on or off there, where it was put,
/// how wide it was made. `None` everywhere: the defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
struct Win {
    /// Turned on (or off) for this window by hand; `None` follows the
    /// master switch.
    on: Option<bool>,
    place: Option<Place>,
    width: Option<f32>,
    /// The session this window works with (`Past::id`); `None` until one
    /// is found for it or picked.
    #[serde(default)]
    session: Option<u64>,
}

/// Why a card that has a window is not showing on it. Each cause comes and
/// goes on its own; the card is back when none is left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Away {
    /// The window is in the compositor's hand: being moved or resized.
    Drag = 1,
    /// The window is being moved by the OPTIONS pill's scroll.
    Nudge = 2,
    /// The workspaces are being swiped.
    Swipe = 4,
}

/// The moments the card waits for: each is "a while after the LAST time
/// something happened", so each is a deadline that is pushed back, and ONE
/// kind of timer serves them all (`App::card_wait`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// The scroll over the card went quiet: the gesture is over.
    Wheel,
    /// The pill's scroll went quiet: the window is put down.
    Nudge,
    /// The sliding scroll stopped: was it a throw?
    Throw,
    /// The fingers left a workspace swipe: the slide has settled.
    SwipeBack,
}
const WAITS: usize = 4;

/// What is kept across a restart of the DOCK (not of the compositor: the
/// addresses are its): which windows the card is on for, where, how wide.
/// In the runtime dir, so it is gone at logout with the windows it names.
#[derive(Default, Serialize, Deserialize)]
struct Session {
    all: bool,
    wins: HashMap<String, Win>,
}
const SESSION_FILE: &str = "waverunner-card.json";

/// The card's state on the loop.
#[derive(Default)]
pub(crate) struct Card {
    pub items: Vec<Item>,
    next_id: u64,
    loaded: bool,
    /// Turned on for every window (the master switch); each window may say
    /// otherwise (`Win::on`).
    all: bool,
    wins: HashMap<String, Win>,
    /// The window it is on.
    pub host: Option<String>,
    /// Where that window is; `None` until it has been found.
    spot: Option<WindowSpot>,
    /// The card's box on the surface.
    rect: Option<Rect>,
    /// The unroll, 0 (away) to 1 (down).
    shown: f32,
    /// Rolling up for good: the window is let go when it is up.
    leaving: bool,
    /// Why it is away (`Away` bits), and since when.
    away: u8,
    away_since: Option<Instant>,
    /// Each [`Wait`]'s deadline, while it is being waited for.
    waits: [Option<Instant>; WAITS],
    /// The scroll that is sliding the card, measured for a throw.
    swipe: Option<Swipe>,
    /// The card on its way sideways: where its left edge (from its
    /// window's) is right now, and how fast it is going. `None` at rest —
    /// it is where its `Place` says. See [`TRAVEL_SPEED`].
    at: Option<f32>,
    speed: f32,
    /// The pace, when `card rate` has set one for this run: (speed, accel).
    pace: Option<(f32, f32)>,
    wheel: Wheel,
    scroll: f32,
    max_scroll: f32,
    /// Show the newest item on the next draw.
    to_bottom: bool,
    tiles: Vec<Tile>,
    /// Each item's wrapped lines, at the width the card has now.
    lines: HashMap<u64, Vec<String>>,
    ptr: Option<(f32, f32)>,
    press: Option<Press>,
    pub drag: Option<Drag>,
    /// Which page is up. `items` is ALWAYS the list on screen — a
    /// session's on its page, the pinned ones on theirs, one row a session
    /// on Memory's — so everything that works on the list (order, drags,
    /// the ×, a click) works on every page. The real lists are `memory`
    /// (every session, newest first) and `pinned`; the one on screen is
    /// OUT of there meanwhile (`Card::put_back` / `take_out`).
    page: Page,
    /// The session whose items are the card's on the Session page: the
    /// one of the window the card is on. `None`: that window has none yet
    /// (Memory's page is up for one to be picked).
    open: Option<u64>,
    pinned: Vec<Item>,
    memory: Vec<Past>,
    /// The session each window title was given (see `model::Saved`).
    titles: HashMap<String, u64>,
    /// How big the items are drawn (0 = never set; read with `zoom()`).
    zoom: f32,
    /// The card has the keyboard, and the typing goes into this (the
    /// input box, the search, the memory's name). `None`: its window has
    /// the keyboard, as always.
    field: Option<Field>,
    /// What is written in the input box, and that wrapped to the box.
    draft: String,
    draft_lines: Vec<String>,
    /// What is being searched for on the page that is up.
    query: String,
    /// The memory's new name, while it is typed.
    naming: String,
    /// A recording in progress, a voice note being played, and whether
    /// what was said is being written out right now (`voice.rs`).
    rec: Option<voice::Rec>,
    playing: Option<voice::Playing>,
    writing: bool,
    /// The emoji picker is open over the input box.
    picking: bool,
    /// Something to say in the line under the input box (a tool that is
    /// missing, nothing made out), until the next thing is done.
    say: Option<&'static str>,
    /// Ctrl +/− are the card's right now (the pointer is on it).
    keys: bool,
    /// A drag is over the card (another app's, or an item of ours).
    dnd_over: bool,
    /// …and it is one of the card's own items, being moved to a new place.
    drop_own: bool,
    /// Where that drag's pointer is, down the surface (`None` until its
    /// first motion: the compositor's `enter` does not say).
    drop_y: Option<f32>,
    /// The place in the list a drop just made is filling: what arrives
    /// from it lands there, in order.
    drop_index: Option<usize>,
    /// How far each item has moved down to open a place for the drag,
    /// by id, eased frame by frame (`view::View::shifts`).
    shifts: HashMap<u64, f32>,
    dnd_mimes: Vec<String>,
    /// A name for a picture about to arrive as pixels (the end of its URL).
    dnd_hint: Option<String>,
    /// Pictures in hand, by path, and the texture layer each is in. Both
    /// hold at most [`PIC_LAYERS`]: a picture pushed out of its layer is
    /// forgotten, and asked for again when its item is next on screen.
    chains: HashMap<String, Vec<u8>>,
    slots: HashMap<String, u32>,
    slot_next: u32,
    asked: HashSet<String>,
    /// The renderer is shrunk to [`PARKED`]: there is no card anywhere.
    parked: bool,
}

/// The paste keystroke, into the window that has the keyboard: Ctrl+V, or
/// Ctrl+Shift+V where that is what pastes.
fn card_paste_key() {
    let shifted = crate::hypr::active_window_where()
        .is_some_and(|(class, _)| SHIFT_PASTERS.contains(&class.to_lowercase().as_str()));
    crate::hypr::send_shortcut_active(if shifted { "CTRL SHIFT" } else { "CTRL" }, "v");
}

/// Whether `addr` is a window's address as the compositor writes it
/// (`0x` and hex). Addresses arrive over the dock's socket and go into
/// commands the compositor evaluates: anything else is refused.
pub(crate) fn valid_addr(addr: &str) -> bool {
    addr.strip_prefix("0x")
        .is_some_and(|h| !h.is_empty() && h.len() <= 16 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Where the session's record is kept (see [`Session`]).
fn session_path() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned()))
        .join(SESSION_FILE)
}

impl Card {
    /// What is kept for the window the card is on.
    fn win(&self) -> Win {
        self.host
            .as_ref()
            .and_then(|h| self.wins.get(h))
            .copied()
            .unwrap_or_default()
    }

    /// How big the items are drawn: 1 as designed.
    fn zoom(&self) -> f32 {
        if self.zoom > 0.0 {
            self.zoom.clamp(ZOOM_MIN, ZOOM_MAX)
        } else {
            1.0
        }
    }

    /// How wide the card is on the window it is on.
    fn width(&self) -> f32 {
        self.win()
            .width
            .map_or(WIDTH, |w| w.clamp(WIDTH_MIN, WIDTH_MAX))
    }

    /// The card's left edge from its window's, where it rests on a window
    /// `window_w` wide.
    fn left(&self, window_w: f32) -> f32 {
        self.win()
            .place
            .unwrap_or(Place::REST)
            .left(window_w, self.width())
    }

    /// Put the card with its left edge `left` from its window's (kept by
    /// the side it is nearer to).
    fn put(&mut self, left: f32, window_w: f32) {
        let width = self.width();
        if let Some(host) = self.host.clone() {
            self.wins.entry(host).or_default().place = Some(Place::of(left, window_w, width));
        }
    }

    /// Whether the card is on for the window at `addr`.
    fn armed(&self, addr: &str) -> bool {
        self.wins.get(addr).and_then(|w| w.on).unwrap_or(self.all)
    }

    /// Turn the card on for `addr` if it was off, off if it was on; which
    /// it is now. (Where it was put there and how wide stay — until the
    /// window closes: Max, 2026-10-08, *"if i close and open the card it
    /// remembers the position and size until i close that window."*)
    fn toggle(&mut self, addr: &str) -> bool {
        let on = !self.armed(addr);
        self.wins.entry(addr.to_owned()).or_default().on = Some(on);
        on
    }

    /// The master switch: on for every window, or off for all — whatever
    /// each was told by hand before. Which it is now.
    fn toggle_all(&mut self) -> bool {
        self.all = !self.all;
        for win in self.wins.values_mut() {
            win.on = None;
        }
        self.all
    }

    /// Whether a focus arriving on `addr` brings the card there.
    fn follows(&self, addr: &str) -> bool {
        (self.host.as_deref() != Some(addr) || self.leaving) && self.armed(addr)
    }

    /// A window is gone: nothing of it is kept (its address will be reused).
    fn forget(&mut self, addr: &str) {
        self.wins.remove(addr);
    }

    fn hidden(&self) -> bool {
        self.away != 0
    }

    /// Where the drag over the card would land in the list, and how tall
    /// a place the list opens for it there.
    fn opening(&self) -> Option<(usize, f32)> {
        let y = self.drop_y.filter(|_| self.dnd_over)?;
        let carried = self.drag.as_ref().filter(|_| self.drop_own).map(|d| d.id);
        let height = carried
            .and_then(|id| self.item(id))
            .map_or(DROP_OPENING, |it| {
                tile_height(
                    it.kind,
                    self.lines.get(&it.id).map_or(1, Vec::len),
                    self.zoom(),
                )
            });
        Some((insert_index(&self.tiles, y), height + GAP))
    }

    /// Move item `id` to place `index` of the list as it reads WITHOUT it
    /// (which is how it is shown while the item is carried).
    fn move_to(&mut self, id: u64, index: usize) -> bool {
        let Some(from) = self.items.iter().position(|it| it.id == id) else {
            return false;
        };
        let item = self.items.remove(from);
        let to = index.min(self.items.len());
        self.items.insert(to, item);
        from != to
    }

    /// The card goes away for `why`. Whether it was showing until now.
    fn hide(&mut self, why: Away, now: Instant) -> bool {
        let was_showing = self.away == 0;
        self.away |= why as u8;
        if was_showing {
            self.away_since = Some(now);
        }
        was_showing
    }

    /// `why` is over. Whether the card is back (no other cause holds it).
    fn unhide(&mut self, why: Away) -> bool {
        let was_hidden = self.away != 0;
        self.away &= !(why as u8);
        if self.away == 0 {
            self.away_since = None;
        }
        was_hidden && self.away == 0
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

    fn item(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|it| it.id == id)
    }

    /// Whether the search is open (it has the cursor, or holds a query).
    fn seeking(&self) -> bool {
        self.field == Some(Field::Seek) || !self.query.is_empty()
    }

    /// The memory the card is working with.
    fn memory_open(&self) -> Option<&Past> {
        let open = self.open?;
        self.memory.iter().find(|p| p.id == open)
    }

    /// Its name as the bubble shows it.
    fn open_name(&self) -> Option<String> {
        self.memory_open()
            .map(|p| p.name.clone().unwrap_or_else(|| "Untitled".to_owned()))
    }

    /// What is under `pos`: the bottom (the search, the input box, New), a
    /// button of the head, the memory's name, or an item — its ×, its pin
    /// (a memory's items; on the clipboard's page it means "keep"), a
    /// voice note's play button, or the item itself.
    fn hit(&self, pos: (f32, f32)) -> Option<Hover> {
        let rect = self.rect?;
        if !rect.contains(pos) {
            return None;
        }
        let low = bottom(
            rect,
            self.page,
            self.draft_lines.len(),
            self.seeking(),
            self.picking,
        );
        if let Some(hit) = low.hit(pos, self.seeking()) {
            return Some(hit);
        }
        let list = list_rect(rect, true, low.h);
        if pos.1 < list.y {
            return foot_buttons(rect)
                .into_iter()
                .find(|(_, r)| r.contains(pos))
                .map(|(f, _)| Hover::Foot(f));
        }
        if pos.1 >= list.y + list.h {
            return None;
        }
        if self.page == Page::Session {
            let name = match self.field {
                Some(Field::Name) => Some(format!("{}|", self.naming)),
                _ => self.open_name(),
            };
            if name.is_some_and(|n| name_rect(list, &n).contains(pos)) {
                return Some(Hover::Name);
            }
        }
        let tile = self.tiles.iter().find(|t| t.rect.contains(pos))?;
        let pins = matches!(self.page, Page::Session | Page::Clipboard);
        Some(
            if self.page != Page::Clipboard && tile.close().contains(pos) {
                Hover::Close(tile.id)
            } else if pins && tile.pin().contains(pos) {
                Hover::Pin(tile.id)
            } else if tile.play().contains(pos)
                && self.item(tile.id).is_some_and(|it| it.kind == Kind::Voice)
            {
                Hover::Play(tile.id)
            } else {
                Hover::Item(tile.id)
            },
        )
    }

    /// Whether `item` (of the page that is up) matches what is searched.
    fn matches(&self, item: &Item) -> bool {
        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let has = |it: &Item| {
            it.body.to_lowercase().contains(&query)
                || it
                    .from
                    .as_ref()
                    .is_some_and(|f| f.to_lowercase().contains(&query))
                || (it.kind == Kind::Voice && "voice note".contains(&query))
        };
        // A memory is found by its name or by anything in it.
        match (self.page, self.memory.iter().find(|p| p.id == item.id)) {
            (Page::Memory, Some(past)) => has(item) || past.items.iter().any(has),
            _ => has(item),
        }
    }

    /// Keep a copy of `item` in the memory the card is working with, while
    /// another page is up (the clipboard's "keep"). Whether there was one.
    fn keep(&mut self, item: &Item, now: u64) -> bool {
        if self.page == Page::Session {
            return false;
        }
        let Some(open) = self.open else {
            return false;
        };
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        let Some(past) = self.memory.iter_mut().find(|p| p.id == open) else {
            return false;
        };
        past.items.push(Item {
            id,
            at: now,
            ..item.clone()
        });
        true
    }

    /// The items of the session that is out on the card (none while
    /// another page is up: it is back in `memory` then), and the pinned
    /// ones wherever they are.
    fn session_items(&self) -> &[Item] {
        if self.page == Page::Session {
            &self.items
        } else {
            &[]
        }
    }

    fn pinned_items(&self) -> &[Item] {
        if self.page == Page::Pinned {
            &self.items
        } else {
            &self.pinned
        }
    }

    /// Every session as it is right now (the one on screen included).
    fn sessions(&self) -> Vec<Past> {
        let mut all = self.memory.clone();
        if let (Page::Session, Some(id)) = (self.page, self.open) {
            if let Some(past) = all.iter_mut().find(|p| p.id == id) {
                past.items = self.items.clone();
            }
        }
        all
    }

    /// The list on screen goes back where it is kept.
    fn put_back(&mut self) {
        let shown = std::mem::take(&mut self.items);
        match self.page {
            Page::Session => {
                let open = self.open;
                if let Some(past) = self.memory.iter_mut().find(|p| Some(p.id) == open) {
                    past.items = shown;
                }
            }
            Page::Pinned => self.pinned = shown,
            Page::Memory | Page::Clipboard => {}
        }
    }

    /// The page's list comes out onto the screen.
    fn take_out(&mut self) {
        self.items = match self.page {
            Page::Session => {
                let open = self.open;
                self.memory
                    .iter_mut()
                    .find(|p| Some(p.id) == open)
                    .map(|p| std::mem::take(&mut p.items))
                    .unwrap_or_default()
            }
            Page::Pinned => std::mem::take(&mut self.pinned),
            Page::Memory => self.memory.iter().map(session_row).collect(),
            // (The clipboard's rows are the dock's to list: `card_show`.)
            Page::Clipboard => Vec::new(),
        };
        // (A session's row is wrapped afresh: it may have changed.)
        for past in &self.memory {
            self.lines.remove(&past.id);
        }
        self.shifts.clear();
        self.scroll = 0.0;
        self.to_bottom = self.page == Page::Session;
    }

    /// Put page `page` up. Whether anything changed.
    fn show(&mut self, page: Page) -> bool {
        if page == self.page {
            return false;
        }
        self.put_back();
        self.page = page;
        // (A search belongs to the page it was typed on.)
        self.query.clear();
        self.take_out();
        true
    }

    /// The card works with session `id` from now on, and shows it; with
    /// none, Memory's page is up for one to be picked.
    fn turn_to(&mut self, id: Option<u64>) {
        self.put_back();
        self.open = id.filter(|id| self.memory.iter().any(|p| p.id == *id));
        self.page = if self.open.is_some() {
            Page::Session
        } else {
            Page::Memory
        };
        self.take_out();
    }

    /// Start a session called `name`: it is in memory at once, first, with
    /// nothing in it yet. Its id.
    fn begin(&mut self, name: Option<String>, now: u64) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        // (Memory's page, if it is up, is listed again by whoever shows it.)
        self.memory.insert(
            0,
            Past {
                id,
                at: now,
                name,
                items: Vec::new(),
            },
        );
        id
    }

    /// The session window `addr`, titled `title`, should show: the one it
    /// was given by hand; else the one a window of that title was given
    /// before; else the only one there is. `None`: there are several and
    /// nothing says which — it is picked on Memory's page.
    fn session_for(&self, addr: &str, title: Option<&str>) -> Option<u64> {
        let exists = |id: &u64| self.memory.iter().any(|p| p.id == *id);
        self.wins
            .get(addr)
            .and_then(|w| w.session)
            .filter(exists)
            .or_else(|| {
                title
                    .and_then(|t| self.titles.get(t).copied())
                    .filter(exists)
            })
            .or_else(|| (self.memory.len() == 1).then(|| self.memory[0].id))
    }

    /// Window `addr` works with session `id` until it closes. With `title`
    /// (the choice was made by hand), windows of that title find it again.
    fn give(&mut self, addr: &str, title: Option<&str>, id: u64) {
        self.wins.entry(addr.to_owned()).or_default().session = Some(id);
        if let Some(title) = title.filter(|t| !t.is_empty()) {
            if self.titles.len() >= TITLES_MAX && !self.titles.contains_key(title) {
                self.titles.clear();
            }
            self.titles.insert(title.to_owned(), id);
        }
    }

    /// Session `id` is forgotten: no window works with it any more. What
    /// it was, for its pictures to be cleared away.
    fn lose(&mut self, id: u64) -> Option<Past> {
        let at = self.memory.iter().position(|p| p.id == id)?;
        if self.open == Some(id) {
            self.put_back();
            self.open = None;
            self.take_out();
        }
        let past = self.memory.remove(at);
        for win in self.wins.values_mut() {
            if win.session == Some(id) {
                win.session = None;
            }
        }
        self.titles.retain(|_, s| *s != id);
        if self.page == Page::Memory {
            self.items.retain(|row| row.id != id);
        }
        self.lines.remove(&id);
        Some(past)
    }

    /// Pin item `id` of the session, or take its pin off. Whether it is
    /// pinned now. (The pinned one is a copy: it stays when the session
    /// goes — Max, 2026-10-09.)
    fn pin(&mut self, id: u64) -> Option<bool> {
        if self.page != Page::Session {
            return None;
        }
        let item = self.item(id)?.clone();
        if let Some(at) = self.pinned.iter().position(|p| p.same(&item)) {
            self.pinned.remove(at);
            return Some(false);
        }
        let new = self.next_id.max(1);
        self.next_id = new + 1;
        self.pinned.push(Item { id: new, ..item });
        Some(true)
    }

    /// Whether anything the card keeps — on it, pinned, or in memory — is
    /// the file at `path` (a picture of the card's own is deleted only
    /// with the LAST item that shows it).
    fn keeps(&self, path: &str) -> bool {
        let is = |it: &Item| it.path.as_deref() == Some(path);
        self.session_items().iter().any(is)
            || self.pinned_items().iter().any(is)
            || self.memory.iter().any(|p| p.items.iter().any(is))
    }
}

/// The card goes UNDER the dock (Max, 2026-10-08: *"i want the card to stay
/// under the dock, now it stays ontop"*). Both are on the Top layer, where
/// the compositor stacks by its `order` rule and then by who came first; the
/// card's surface, mapped at its first summon, came last and so lay over the
/// dock. A layer with the HIGHER order is drawn first — underneath (the
/// renderer sorts each layer's surfaces by order, descending, and paints
/// them in that sequence) — so the card gets an order above the dock's 0.
/// A runtime rule: declared before the surface exists, and again whenever
/// the compositor re-reads its config and forgets it (`hypr.rs`).
pub(crate) fn declare_layer_rule() {
    crate::hypr::eval(
        "hl.layer_rule({ name = \"golem-card-under-dock\", match = { namespace = \"waverunner-card\" }, order = 1 })",
    );
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
        let first = self.card_size == (0, 0);
        self.card_size = (width, height);
        if let Some(fs) = &self.card_fscale {
            fs.set_logical_size(width, height);
        }
        if !self.card.parked {
            let scale = self.surface_scale(crate::fractional::SurfaceKind::Card);
            if let Some(renderer) = self.card_renderer.as_mut() {
                renderer.set_scale(scale);
                renderer.resize(
                    crate::fractional::physical(width, scale),
                    crate::fractional::physical(height, scale),
                );
            }
        }
        if first {
            self.card_restore_session();
        }
        self.sync_card();
    }

    /// A dock that has just started picks up where the last one left off in
    /// this session: the windows the card was on for (those still open),
    /// where it was on each and how wide — and the title bars are told, so
    /// their buttons say the same as we do.
    fn card_restore_session(&mut self) {
        let saved: Session = crate::persist::read_json(&session_path()).unwrap_or_default();
        self.card.all = saved.all;
        self.card.wins = saved
            .wins
            .into_iter()
            .filter(|(addr, _)| valid_addr(addr) && crate::hypr::window_exists(addr))
            .collect();
        self.card_tell_bars();
        if let Some(addr) = crate::hypr::active_window() {
            self.card_focus_changed(Some(&addr));
        }
    }

    /// Tell every title bar where the card is on: all of them what the
    /// master switch says, then each window that was told otherwise by
    /// hand. At the dock's start, and when the plugin says it has just
    /// loaded (`card bars`) — a reloaded plugin's buttons all read "off".
    fn card_tell_bars(&self) {
        self.card_tell_bar("*", self.card.all);
        for (addr, win) in &self.card.wins {
            if let Some(on) = win.on.filter(|on| *on != self.card.all) {
                self.card_tell_bar(addr, on);
            }
        }
    }

    /// Keep what a restarted dock must find again (see `Session`).
    fn card_save_session(&self) {
        crate::persist::write_json(
            "card session",
            &session_path(),
            &Session {
                all: self.card.all,
                wins: self.card.wins.clone(),
            },
        );
    }

    /// The card's renderer, built the first time there is a card to draw —
    /// and brought back to the output's size if it was parked.
    fn card_ensure_renderer(&mut self) -> bool {
        let (width, height) = self.card_size;
        if width == 0 || height == 0 {
            return false;
        }
        let scale = self.surface_scale(crate::fractional::SurfaceKind::Card);
        let (pw, ph) = (
            crate::fractional::physical(width, scale),
            crate::fractional::physical(height, scale),
        );
        if let Some(renderer) = self.card_renderer.as_mut() {
            if self.card.parked {
                renderer.set_scale(scale);
                renderer.resize(pw, ph);
                self.card.parked = false;
            }
            return true;
        }
        let built = {
            let Some(layer) = self.card_layer.as_ref() else {
                return false;
            };
            crate::renderer::Renderer::new(&self.conn, layer.wl_surface(), pw, ph, scale)
        };
        match built {
            Ok(mut renderer) => {
                renderer.alloc_icon_array(PIC_LAYERS);
                for (path, &layer) in &self.card.slots {
                    if let Some(chain) = self.card.chains.get(path) {
                        renderer.update_icon_layer(layer, chain);
                    }
                }
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

    /// There is no card anywhere: shrink the renderer (its surface is the
    /// whole output's) and show one empty frame at that size, so the big
    /// one is let go of. Only where the compositor stretches a buffer to
    /// the surface for us (the fractional-scale path).
    fn card_park(&mut self) {
        if self.card.parked || self.card_fscale.is_none() {
            return;
        }
        let Some(renderer) = self.card_renderer.as_mut() else {
            return;
        };
        renderer.resize(PARKED, PARKED);
        self.card.parked = true;
        let empty = crate::content::Scene::default();
        if let Err(e) = renderer.render(&empty, [0.0; 4], None, 0.0, 0, None, &mut || {}) {
            warn!("card: parking its renderer failed: {e:#}");
        }
    }

    /// Read the list kept on disk, once.
    fn card_load(&mut self) {
        if self.card.loaded {
            return;
        }
        self.card.loaded = true;
        let saved = load();
        let ids = saved
            .items
            .iter()
            .chain(&saved.pinned)
            .chain(saved.memory.iter().flat_map(|p| &p.items))
            .map(|it| it.id)
            .chain(saved.memory.iter().map(|p| p.id));
        self.card.next_id = saved.next_id.max(ids.max().map_or(1, |id| id + 1));
        self.card.page = Page::Session;
        self.card.open = None;
        self.card.items = Vec::new();
        self.card.pinned = saved.pinned;
        self.card.memory = saved.memory;
        self.card.titles = saved.titles;
        self.card.zoom = saved.zoom;
        // The one list of before there were sessions is a session now.
        if !saved.items.is_empty() {
            let id = self.card.begin(None, now_secs());
            self.card.memory[0].items = saved.items;
            self.card.open = Some(id);
            self.card.take_out();
        }
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
                items: Vec::new(),
                pinned: self.card.pinned_items().to_vec(),
                memory: self.card.sessions(),
                titles: self.card.titles.clone(),
                zoom: self.card.zoom,
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
        let layer = match self.card.slots.get(path) {
            Some(&layer) => layer,
            None => {
                let layer = self.card.slot_next % PIC_LAYERS;
                self.card.slot_next += 1;
                // Reused: whichever picture was in it is forgotten whole
                // (its pixels too), and asked for again when its item is
                // next on screen (`draw_card`).
                let evicted: Vec<String> = self
                    .card
                    .slots
                    .iter()
                    .filter(|(_, l)| **l == layer)
                    .map(|(p, _)| p.clone())
                    .collect();
                for old in evicted {
                    self.card.slots.remove(&old);
                    self.card.chains.remove(&old);
                    self.card.asked.remove(&old);
                }
                self.card.slots.insert(path.to_owned(), layer);
                layer
            }
        };
        self.card.chains.insert(path.to_owned(), pixels.to_vec());
        if let Some(renderer) = self.card_renderer.as_mut() {
            renderer.update_icon_layer(layer, pixels);
        }
        self.request_card_draw();
    }

    /// Put an item at the end of the list (the newest is at the bottom).
    fn card_push(
        &mut self,
        kind: Kind,
        body: String,
        path: Option<String>,
        owned: bool,
        aspect: f32,
        at: Option<usize>,
    ) {
        self.card_load();
        // What arrives is a session's: the window's own, or — where none
        // was picked yet — a new one of its name.
        if self.card.page != Page::Pinned {
            if self.card.open.is_none() {
                self.card_begin();
            }
            self.card_show(Page::Session);
        }
        let id = self.card.next_id.max(1);
        self.card.next_id = id + 1;
        if let (Some(p), Kind::Image) = (&path, kind) {
            self.card_ask_picture(&p.clone());
        }
        info!(
            "card: + {kind:?} {:?}",
            body.chars().take(60).collect::<String>()
        );
        // At the place a drop chose, or at the end (the newest is at the
        // bottom, and is scrolled to).
        let end = self.card.items.len();
        let at = at.map_or(end, |i| i.min(end));
        self.card.items.insert(
            at,
            Item {
                id,
                kind,
                body,
                path,
                aspect,
                owned,
                at: now_secs(),
                from: crate::hypr::active_window_where().map(|(class, _)| class),
            },
        );
        self.card.to_bottom = at == end;
        self.card_save();
        self.request_card_draw();
    }

    /// Put a text on the card.
    pub(crate) fn card_add_text(&mut self, text: &str) {
        self.card_add_text_at(text, None);
    }

    /// …at place `at` of the list (`None`: the end).
    fn card_add_text_at(&mut self, text: &str, at: Option<usize>) {
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
            self.card_push(Kind::Text, body, None, false, 0.0, at);
        }
    }

    /// Put files, folders or pictures on the card, by their paths. What
    /// each one is is read off the loop (`model::facts`: a folder is
    /// counted, and a phone's storage answers when it pleases), so they
    /// land a moment later, in order.
    pub(crate) fn card_add_paths(&mut self, paths: Vec<PathBuf>) {
        self.card_add_paths_at(paths, None);
    }

    /// …from place `at` of the list on, in order (`None`: the end).
    fn card_add_paths_at(&mut self, paths: Vec<PathBuf>, at: Option<usize>) {
        let (tx, rx) = calloop::channel::channel::<Vec<Facts>>();
        std::thread::spawn(move || {
            let found: Vec<Facts> = paths
                .iter()
                .filter_map(|p| {
                    let facts = facts(p);
                    if facts.is_none() {
                        warn!("card: {} is not there; not added", p.display());
                    }
                    facts
                })
                .collect();
            let _ = tx.send(found);
        });
        let waiting = self
            .loop_handle
            .insert_source(rx, move |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(found) = event {
                    for (n, f) in found.into_iter().enumerate() {
                        app.card_push(
                            f.kind,
                            f.body,
                            Some(f.path),
                            false,
                            f.aspect,
                            at.map(|i| i + n),
                        );
                    }
                }
            });
        if waiting.is_err() {
            warn!("card: cannot wait for the files' facts; not added");
        }
    }

    /// Take an item off the card (a picture of the card's own goes too).
    /// A click on an item: it is copied and pasted into the card's window,
    /// as Copy then Paste would do (Max, 2026-10-08: *"short click (not
    /// grab) to paste the item as copy and paste will do"*). A text goes as
    /// text, anything else as its file. It stays on the clipboard.
    fn card_paste(&mut self, id: u64) {
        if self.card.page == Page::Memory {
            self.card_recall(id);
            return;
        }
        let Some(item) = self.card.item(id).cloned() else {
            return;
        };
        // (A voice note is listened to: there is nothing of it to paste.)
        if item.kind == Kind::Voice {
            self.card_play(id);
            return;
        }
        match &item.path {
            Some(path) => self.serve_files(std::slice::from_ref(path), false),
            None => self.serve_transient_text(&item.body),
        }
        // The paste is the keyboard's window's: the card's own first. (A
        // card that has the keyboard hands it back before the keys go.)
        let host = self.card.host.clone();
        let focused = crate::hypr::active_window();
        let typing = self.card.field.is_some();
        self.card_drop_keys();
        let wait = match host {
            _ if typing => PASTE_FOCUS + 60,
            Some(host) if focused.as_deref() != Some(host.as_str()) => {
                crate::hypr::focus_window(&host);
                PASTE_FOCUS
            }
            _ => PASTE_SETTLE,
        };
        info!(
            "card: {:?} pasted",
            item.body.chars().take(60).collect::<String>()
        );
        self.after_ms(wait, |_| card_paste_key());
    }

    /// Put page `page` up.
    fn card_show(&mut self, page: Page) {
        self.card_load();
        if self.card.show(page) {
            // (The input box and the name are a memory's.)
            if page != Page::Session && matches!(self.card.field, Some(Field::Box | Field::Name)) {
                self.card_drop_keys();
            }
            if page == Page::Clipboard {
                self.card.items = self.card_clip_rows();
            }
            for path in self.card_pictures() {
                self.card_ask_picture(&path);
            }
            self.request_card_draw();
        }
    }

    /// The plain clipboard's history as the rows of its page, newest first
    /// (Max, 2026-10-09: *"the plain clipboard should be reachable from the
    /// card"*): a text as a text, copied files and pictures as their file.
    /// A row's id is the clip's, out of the way of the card's own.
    fn card_clip_rows(&self) -> Vec<Item> {
        self.clip
            .history
            .iter()
            .filter_map(|clip| {
                let (kind, body, path) = match clip.kind {
                    crate::clipboard::ClipKind::Text => (Kind::Text, clip.text.clone(), None),
                    crate::clipboard::ClipKind::Files => {
                        let at = crate::desktop::uri_list_paths(&clip.text)
                            .into_iter()
                            .next()?;
                        let path = at.to_string_lossy().into_owned();
                        (kind_of(&at), file_body(&at), Some(path))
                    }
                    crate::clipboard::ClipKind::Image => {
                        let path = clip.image_path.as_ref()?.to_string_lossy().into_owned();
                        (Kind::Image, clip.preview.clone(), Some(path))
                    }
                };
                Some(Item {
                    id: CLIP_IDS + clip.id,
                    kind,
                    body,
                    path,
                    aspect: if clip.height > 0 {
                        clip.width as f32 / clip.height as f32
                    } else {
                        0.0
                    },
                    owned: false,
                    at: clip.timestamp_ms / 1000,
                    from: (!clip.source.is_empty()).then(|| clip.source.clone()),
                })
            })
            .collect()
    }

    /// The clipboard's history changed: its page, if it is up, lists it anew.
    pub(crate) fn card_clip_changed(&mut self) {
        if self.card.page == Page::Clipboard && self.card.host.is_some() {
            self.card.items = self.card_clip_rows();
            self.request_card_draw();
        }
    }

    /// The pin of a clipboard row: a copy of it is kept in the memory the
    /// card is working with (one is started if the window has none).
    fn card_keep_clip(&mut self, id: u64) {
        let Some(item) = self.card.item(id).cloned() else {
            return;
        };
        if self.card.open.is_none() {
            let title = self.card_host_title();
            let new = self.card.begin(title.clone(), now_secs());
            if let Some(host) = self.card.host.clone() {
                self.card.give(&host, title.as_deref(), new);
            }
            self.card.open = Some(new);
        }
        if self.card.keep(&item, now_secs()) {
            info!("card: kept from the clipboard");
            self.card_save();
            self.card_show(Page::Session);
        }
    }

    // ---- the keyboard: the input box, the search, the name ----

    /// The card takes the keyboard for `field` (a click on the input box,
    /// the search, the name). As the desktop does it (`desktop_take_keys`):
    /// an EXCLUSIVE grab fetches it at once, and the moment it arrives
    /// (`card_keys_arrived`) the grab is let go for "on demand" — the
    /// keyboard stays until a window is clicked, and the pointer is free.
    fn card_take_keys(&mut self, field: Field) {
        let had = self.card.field.replace(field);
        if field == Field::Name {
            self.card.naming = self.card.open_name().unwrap_or_default();
        }
        if had.is_none() {
            if let Some(layer) = self.card_layer.as_ref() {
                crate::surface::set_interactive(layer, true);
                let _ = self.conn.flush();
            }
            self.cancel_keyboard_handback(crate::KbSurface::Card);
            debug!("card: has the keyboard ({field:?})");
        }
        self.sync_card_input();
        self.request_card_draw();
    }

    /// The keyboard has arrived on the card.
    pub(crate) fn card_keys_arrived(&mut self) {
        if self.card.field.is_none() {
            return;
        }
        if let Some(layer) = self.card_layer.as_ref() {
            crate::surface::set_on_demand(layer);
        }
        let _ = self.conn.flush();
    }

    /// The keyboard left the card (a window was clicked): what was being
    /// typed stays where it is, the cursor is the window's again.
    pub(crate) fn card_keys_left(&mut self) {
        if self.card.field.take().is_some() {
            debug!("card: the keyboard left");
            if let Some(layer) = self.card_layer.as_ref() {
                crate::surface::set_interactive(layer, false);
            }
            let _ = self.conn.flush();
            self.request_card_draw();
        }
        self.complete_keyboard_handback(crate::KbSurface::Card);
    }

    /// Give the keyboard back to the card's window.
    fn card_drop_keys(&mut self) {
        if self.card.field.take().is_none() {
            return;
        }
        debug!("card: gives the keyboard back");
        self.begin_keyboard_handback(crate::KbSurface::Card, self.card.host.clone());
        if let Some(layer) = self.card_layer.as_ref() {
            crate::surface::set_interactive(layer, false);
        }
        let _ = self.conn.flush();
        self.request_card_draw();
    }

    /// Whether the card takes the keys right now.
    pub(crate) fn card_typing(&self) -> bool {
        self.card.field.is_some()
    }

    /// A key, while the card has the keyboard.
    pub(crate) fn card_key(
        &mut self,
        keysym: smithay_client_toolkit::seat::keyboard::Keysym,
        utf8: Option<&str>,
    ) {
        use smithay_client_toolkit::seat::keyboard::Keysym;
        let Some(field) = self.card.field else {
            return;
        };
        let (ctrl, shift) = (self.modifiers.ctrl, self.modifiers.shift);
        let typed = utf8
            .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
            .filter(|_| !ctrl);
        match (field, keysym) {
            (_, Keysym::Escape) => {
                // One layer at a time: the search's words, then the cursor.
                match field {
                    Field::Seek if !self.card.query.is_empty() => self.card.query.clear(),
                    _ => self.card_drop_keys(),
                }
            }
            // The input box: Enter keeps it in the memory, Ctrl+Enter sends
            // it to the window, Shift+Enter breaks the line.
            (Field::Box, Keysym::Return | Keysym::KP_Enter) if shift => self.card.draft.push('\n'),
            (Field::Box, Keysym::Return | Keysym::KP_Enter) if ctrl => self.card_send_draft(),
            (Field::Box, Keysym::Return | Keysym::KP_Enter) => self.card_keep_draft(),
            (Field::Box, Keysym::v | Keysym::V) if ctrl => {
                // Paste, as any text field does.
                if let Some(clip) = self.clip.history.first() {
                    if clip.kind == crate::clipboard::ClipKind::Text {
                        let text = clip.text.clone();
                        self.card.draft.push_str(&text);
                    }
                }
            }
            (Field::Box, Keysym::BackSpace) => {
                self.card.draft.pop();
            }
            (Field::Seek, Keysym::BackSpace) => {
                self.card.query.pop();
            }
            (Field::Seek, Keysym::Return | Keysym::KP_Enter) => {}
            (Field::Name, Keysym::BackSpace) => {
                self.card.naming.pop();
            }
            (Field::Name, Keysym::Return | Keysym::KP_Enter) => {
                let name = self.card.naming.trim().to_owned();
                self.card_rename(&name);
                self.card.field = Some(Field::Box);
            }
            _ => match (field, typed) {
                (Field::Box, Some(s)) => self.card.draft.push_str(s),
                (Field::Seek, Some(s)) => self.card.query.push_str(s),
                (Field::Name, Some(s)) => self.card.naming.push_str(s),
                _ => return,
            },
        }
        self.card.scroll = if self.card.field == Some(Field::Seek) {
            0.0
        } else {
            self.card.scroll
        };
        self.sync_card_input();
        self.request_card_draw();
    }

    /// The open memory is called `name` from now on (an empty one keeps
    /// what it had).
    fn card_rename(&mut self, name: &str) {
        let Some(open) = self.card.open else {
            return;
        };
        if name.is_empty() {
            return;
        }
        if let Some(past) = self.card.memory.iter_mut().find(|p| p.id == open) {
            past.name = Some(name.to_owned());
            info!("card: a memory renamed {name:?}");
            self.card_save();
        }
    }

    /// Enter in the input box: what is written is kept in the memory, and
    /// the cursor stays for the next thing.
    fn card_keep_draft(&mut self) {
        let text = std::mem::take(&mut self.card.draft);
        if text.trim().is_empty() {
            return;
        }
        self.card_push(
            Kind::Text,
            text.trim_end().to_owned(),
            None,
            false,
            0.0,
            None,
        );
    }

    /// Ctrl+Enter in the input box: what is written goes into the card's
    /// window, and the cursor goes back there with it.
    fn card_send_draft(&mut self) {
        let text = std::mem::take(&mut self.card.draft);
        if text.trim().is_empty() {
            return;
        }
        self.serve_transient_text(text.trim_end());
        self.card_drop_keys();
        self.after_ms(PASTE_FOCUS + 60, |_| card_paste_key());
    }

    /// One of the input box's buttons.
    fn card_box_button(&mut self, which: BoxBtn) {
        match which {
            BoxBtn::Mic => self.card_voice_button(false),
            BoxBtn::Talk => self.card_voice_button(true),
            BoxBtn::Emoji => {
                self.card.picking = !self.card.picking;
                self.sync_card_input();
                self.request_card_draw();
            }
            BoxBtn::Clip => self.card_pick_file(),
        }
    }

    /// An emoji of the picker goes WHERE THE CURSOR IS: into the input box
    /// while the card has the keyboard there; straight into the window
    /// while the window has it — as a phone's keyboard does, and nothing
    /// is left on the card (Max, 2026-10-09: *"i dont want to have one
    /// emoji on the card for each time i want to send an emoji"*). The
    /// picker stays open for the next one.
    fn card_emoji(&mut self, n: usize) {
        let Some(emoji) = EMOJI.get(n) else {
            return;
        };
        if self.card.field == Some(Field::Box) {
            self.card.draft.push_str(emoji);
            self.request_card_draw();
        } else if self.card.field.is_none() {
            self.serve_transient_text(emoji);
            self.after_ms(PASTE_SETTLE, |_| card_paste_key());
        }
    }

    /// The paperclip: the desktop's file picker, and what is picked is
    /// kept in the memory.
    fn card_pick_file(&mut self) {
        let (tx, rx) = calloop::channel::channel::<Option<PathBuf>>();
        std::thread::spawn(move || {
            let _ = tx.send(crate::bt_files::pick_file_waiting());
        });
        let waiting = self
            .loop_handle
            .insert_source(rx, |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(Some(path)) = event {
                    app.card_add_paths(vec![path]);
                }
            });
        if waiting.is_err() {
            warn!("card: cannot wait for the file picker");
        }
    }

    /// A button, let go where it was pressed.
    fn card_tap(&mut self, what: Hover) {
        self.card.say = None;
        match what {
            Hover::Close(id) => self.card_remove(id),
            Hover::Pin(id) if self.card.page == Page::Clipboard => self.card_keep_clip(id),
            Hover::Pin(id) => self.card_pin(id),
            Hover::Foot(which) => self.card_foot(which),
            Hover::Seek => self.card_take_keys(Field::Seek),
            Hover::SeekClear => {
                self.card.query.clear();
                if self.card.field == Some(Field::Seek) {
                    self.card_drop_keys();
                }
                self.sync_card_input();
                self.request_card_draw();
            }
            Hover::Input => self.card_take_keys(Field::Box),
            Hover::Btn(which) => self.card_box_button(which),
            // New, in the input box's place: the new memory opens with the
            // cursor already in its box (Max: *"so i click on it and i have
            // my pointer on the input"*).
            Hover::New => {
                self.card_begin();
                self.card_take_keys(Field::Box);
            }
            Hover::Name => self.card_take_keys(Field::Name),
            Hover::Play(id) => self.card_play(id),
            Hover::Emoji(n) => self.card_emoji(n),
            Hover::Item(_) => {}
        }
    }

    /// The title of the window the card is on, as the bar's pill reads it
    /// (`task_title::groom`): what a session started there is called, and
    /// how a window of that task finds it again.
    fn card_host_title(&self) -> Option<String> {
        let host = self.card.host.as_deref()?;
        let (class, title) = crate::hypr::window_class_title(host)?;
        let home = std::env::var("HOME").ok();
        let name = crate::task_title::groom(&title, Some(&class), home.as_deref());
        (!name.is_empty()).then_some(name)
    }

    /// The card has come to a window: it shows that window's session — the
    /// one it was given, or the one its title was given before, or the
    /// only one there is. With several and nothing to go by it opens on
    /// MEMORY, to be picked (Max, 2026-10-09: *"which session should a new
    /// window start on? i think it should open on memory"*); with none at
    /// all, a first one is started.
    fn card_attach(&mut self) {
        let Some(host) = self.card.host.clone() else {
            return;
        };
        let title = self.card_host_title();
        let found = self.card.session_for(&host, title.as_deref());
        let id = match found {
            None if self.card.memory.is_empty() => {
                let id = self.card.begin(title.clone(), now_secs());
                self.card.give(&host, title.as_deref(), id);
                self.card_save();
                Some(id)
            }
            found => found,
        };
        if let Some(id) = id {
            // (Not remembered by title: nobody chose it.)
            self.card.give(&host, None, id);
        }
        self.card.turn_to(id);
        for path in self.card_pictures() {
            self.card_ask_picture(&path);
        }
    }

    /// Start a session for the window the card is on, called after it. It
    /// is in memory at once, even with nothing in it — it can be picked
    /// from another window straight away (Max, 2026-10-09: *"i can start a
    /// session here and go to the browser, open it and drop some stuff
    /// there"*).
    fn card_begin(&mut self) {
        self.card_load();
        let title = self.card_host_title();
        let id = self.card.begin(title.clone(), now_secs());
        if let Some(host) = self.card.host.clone() {
            self.card.give(&host, title.as_deref(), id);
        }
        self.card.turn_to(Some(id));
        info!(
            "card: a new session {title:?}; {} in memory",
            self.card.memory.len()
        );
        self.card_save();
        self.card_save_session();
        self.request_card_draw();
    }

    /// A button of the head: its page — or back to the session when it is
    /// the one already up (if the window has one).
    fn card_foot(&mut self, which: Foot) {
        self.card_load();
        match which.page() {
            page if page != self.card.page => self.card_show(page),
            _ if self.card.open.is_some() => self.card_show(Page::Session),
            _ => self.card_show(Page::Memory),
        }
    }

    /// A click on a session in memory: this window works with it from now
    /// on (and so will a window of the same title, another day).
    fn card_recall(&mut self, id: u64) {
        if !self.card.memory.iter().any(|p| p.id == id) {
            return;
        }
        let title = self.card_host_title();
        if let Some(host) = self.card.host.clone() {
            self.card.give(&host, title.as_deref(), id);
        }
        self.card.turn_to(Some(id));
        info!("card: session {id} picked for {title:?}");
        for path in self.card_pictures() {
            self.card_ask_picture(&path);
        }
        self.card_save();
        self.card_save_session();
        self.request_card_draw();
    }

    /// The × of a session in memory: it is gone for good, with the
    /// pictures that were only its.
    fn card_forget(&mut self, id: u64) {
        let Some(past) = self.card.lose(id) else {
            return;
        };
        info!("card: a session of {} forgotten", past.items.len());
        for item in &past.items {
            self.card.lines.remove(&item.id);
            if let (true, Some(path)) = (item.owned, item.path.as_deref()) {
                if !self.card.keeps(path) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
        self.card_save();
        self.card_save_session();
        self.request_card_draw();
    }

    /// The pin of an item: pinned, or not any more.
    fn card_pin(&mut self, id: u64) {
        if let Some(on) = self.card.pin(id) {
            info!("card: an item {}", if on { "pinned" } else { "unpinned" });
            self.card_save();
            self.request_card_draw();
        }
    }

    fn card_remove(&mut self, id: u64) {
        if self.card.page == Page::Memory {
            self.card_forget(id);
            return;
        }
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
            if item.owned && !self.card.keeps(path) {
                let _ = std::fs::remove_file(path);
            }
        }
        self.card_save();
        self.request_card_draw();
    }

    /// Whether there is a card to show right now: on a window that can be
    /// seen, not away for any reason, and not under the overview.
    fn card_present(&self) -> bool {
        self.card.host.is_some()
            && !self.card.leaving
            && !self.card.hidden()
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
        if addr != "*" && !valid_addr(addr) {
            return;
        }
        crate::hypr::eval(&format!("hl.plugin.waveview.card(\"{addr}\", {on})"));
    }

    /// The focused window's address.
    fn card_focused(&self) -> Option<String> {
        self.options_active_addr
            .clone()
            .or_else(crate::hypr::active_window)
    }

    /// The button on a window's title bar: the card on for that window (and
    /// here), or off for it (and away, if it was here). Without an address:
    /// the focused window.
    pub(crate) fn card_toggle(&mut self, addr: Option<&str>) -> String {
        let Some(addr) = addr.map(str::to_owned).or_else(|| self.card_focused()) else {
            return "no window is focused".to_owned();
        };
        if !valid_addr(&addr) {
            return format!("{addr:?} is not a window's address");
        }
        let on = self.card.toggle(&addr);
        self.card_tell_bar(&addr, on);
        self.card_save_session();
        if on {
            self.card_summon(&addr);
            format!("on for {addr}")
        } else {
            if self.card.host.as_deref() == Some(addr.as_str()) {
                self.card_dismiss();
            }
            format!("off for {addr}")
        }
    }

    /// The master switch: the card on for every window, or off for all.
    pub(crate) fn card_toggle_all(&mut self) -> String {
        let on = self.card.toggle_all();
        self.card_tell_bar("*", on);
        self.card_save_session();
        if on {
            if let Some(addr) = self.card_focused() {
                self.card_summon(&addr);
            }
            "on for every window".to_owned()
        } else {
            self.card_dismiss();
            "off for every window".to_owned()
        }
    }

    /// The focus moved: the card comes along onto a window it is on for,
    /// and stays where it is otherwise.
    pub(crate) fn card_focus_changed(&mut self, addr: Option<&str>) {
        let Some(addr) = addr else {
            return;
        };
        if self.card.follows(addr) {
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
        self.card_attach();
        self.card.leaving = false;
        self.card.shown = 0.0;
        self.card.spot = None;
        self.card.press = None;
        self.card.at = None;
        self.card.swipe = None;
        // Whatever kept it away belonged to the window it was on.
        self.card.away = 0;
        self.card.away_since = None;
        self.card.waits = [None; WAITS];
        self.card.wheel = Wheel::default();
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

    /// The card has no window any more: nothing is drawn, nothing takes the
    /// pointer, and the renderer is shrunk until it is wanted again.
    fn card_let_go(&mut self) {
        self.card_keys(false);
        self.card_drop_keys();
        self.card_voice_quiet();
        self.card.host = None;
        self.card.spot = None;
        self.card.rect = None;
        self.card.shown = 0.0;
        self.card.leaving = false;
        self.card.press = None;
        self.card.at = None;
        self.card.away = 0;
        self.card.away_since = None;
        self.card.waits = [None; WAITS];
        self.card.wheel = Wheel::default();
        self.card.swipe = None;
        self.card.tiles.clear();
        self.sync_card_input();
    }

    /// A window closed: everything the card kept for it goes — whether it
    /// was on for it, where it was put on it, how wide. (An address is
    /// reused: the next window to get it must find nothing.)
    pub(crate) fn card_window_closed(&mut self, addr: &str) {
        if self.card.wins.contains_key(addr) {
            self.card.forget(addr);
            self.card_save_session();
        }
    }

    /// Find the card's window again and put the card where it is now. Runs
    /// when something says the window may have moved (focus and layout
    /// events, a window put down, a swipe settled) and on the slow poll.
    pub(crate) fn sync_card(&mut self) {
        let Some(host) = self.card.host.clone() else {
            return;
        };
        match crate::hypr::window_spot(&host) {
            None => {
                debug!("card: its window {host} is gone");
                self.card_window_closed(&host);
                self.card_let_go();
            }
            Some(spot) => {
                self.card.spot = Some(spot);
                // A card in the hand keeps to the pointer, not to memory.
                if !matches!(
                    self.card.press,
                    Some(Press::Slide { .. } | Press::Resize { .. })
                ) {
                    let left = self.card.at.unwrap_or_else(|| self.card.left(spot.w));
                    let scale = self.surface_scale(crate::fractional::SurfaceKind::Card);
                    self.card.rect = Some(card_rect(&spot, left, scale, self.card.width()));
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

    /// The safety net (see [`POLL`]): armed only while there is something
    /// it could change — a card showing, or one away for a window in hand.
    /// A card whose window is on another workspace waits for the
    /// compositor's own news instead, which costs nothing.
    fn card_poll_arm(&mut self) {
        let worth = self.card.spot.is_some_and(|s| s.visible)
            && (self.card.away == 0 || self.card.away & Away::Drag as u8 != 0);
        if self.card_poll_timer || self.card.host.is_none() || !worth {
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
                let lost = app.card.away & Away::Drag as u8 != 0
                    && app.card.away_since.is_some_and(|t| t.elapsed() > LOST_DRAG);
                if now == app.card.spot && lost {
                    app.card_back(Away::Drag);
                } else if now != app.card.spot {
                    app.sync_card();
                }
                app.card_poll_arm();
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
        if armed {
            self.card_poll_timer = true;
        }
    }

    /// Wait until `wait` after now for `what` — or, if it is already being
    /// waited for, push its moment back to then. ONE kind of timer for
    /// every "a while after the last…" the card has (see [`Wait`]).
    fn card_wait(&mut self, what: Wait, wait: Duration) {
        let pending = self.card.waits[what as usize].is_some();
        self.card.waits[what as usize] = Some(Instant::now() + wait);
        if !pending {
            self.card_wait_arm(what, wait);
        }
    }

    fn card_wait_arm(&mut self, what: Wait, wait: Duration) {
        let timer = calloop::timer::Timer::from_duration(wait);
        let armed = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                let now = Instant::now();
                match app.card.waits[what as usize] {
                    // Pushed back since: wait out the rest.
                    Some(until) if until > now => app.card_wait_arm(what, until - now),
                    Some(_) => {
                        app.card.waits[what as usize] = None;
                        app.card_waited(what);
                    }
                    // Called off meanwhile.
                    None => {}
                }
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
        if !armed {
            // No timer to be had: what was waited for happens now.
            self.card.waits[what as usize] = None;
            self.card_waited(what);
        }
    }

    /// The moment `what` was waiting for has come.
    fn card_waited(&mut self, what: Wait) {
        match what {
            Wait::Wheel => self.card_wheel_end(),
            Wait::Nudge => self.card_back(Away::Nudge),
            Wait::Throw => self.card_swipe_judge(),
            Wait::SwipeBack => self.card_back(Away::Swipe),
        }
    }

    /// The card goes away at once, with no roll-up, for `why` (see
    /// [`Away`]) — if it has a window to be away from.
    fn card_away(&mut self, why: Away) {
        if self.card.host.is_none() || self.card.leaving {
            return;
        }
        if self.card.hide(why, Instant::now()) {
            self.card_keys(false);
            self.card_drop_keys();
            self.card.shown = 0.0;
            self.card.press = None;
            self.card.at = None;
            self.sync_card_input();
            self.request_card_draw();
        }
        self.card_poll_arm();
    }

    /// `why` is over: if nothing else keeps it away, the card comes back
    /// where its window is now, unrolling as when it is summoned.
    fn card_back(&mut self, why: Away) {
        if self.card.unhide(why) {
            self.card.shown = 0.0;
            self.card_last_frame = None;
            self.sync_card();
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
        // No card anywhere and its renderer put away: nothing to draw, and
        // drawing would only bring the renderer back to full size.
        if self.card.host.is_none() && self.card.parked {
            return;
        }
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
        if let (Some(at), Some(spot)) = (self.card.at, self.card.spot) {
            let to = self.card.left(spot.w);
            let (top, accel) = self.card.pace.unwrap_or((TRAVEL_SPEED, TRAVEL_ACCEL));
            let (at, speed) = travel(at, to, self.card.speed, dt, top, accel);
            let arrived = at == to || !present;
            self.card.at = (!arrived).then_some(at);
            self.card.speed = if arrived { 0.0 } else { speed };
            self.card.rect = Some(card_rect(
                &spot,
                if arrived { to } else { at },
                self.surface_scale(crate::fractional::SurfaceKind::Card),
                self.card.width(),
            ));
            if arrived {
                // Where it came to rest is worth keeping; where it passed
                // through on the way was not.
                self.sync_card_input();
                self.card_save_session();
            } else {
                moving = true;
            }
        }
        self.card_last_frame = moving.then_some(now);
        if self.card.leaving && shown <= 0.0 {
            self.card_let_go();
        }
        let rect = self.card.rect.unwrap_or_default();
        // The box's colour is read on one side of the screen or the other
        // (`box_surface_at`). By the WINDOW's place, not the card's own: a
        // card crossing the screen's middle changed colour in mid-flight,
        // which read as a hole in the glide (Max, 2026-10-08: *"i see a
        // 'hole' on the animation when the card cross the middle of the
        // window"*). Its window does not move while it slides.
        let paint = self.card_paint();
        let Some(renderer) = self.card_renderer.as_mut() else {
            return;
        };
        // Wrap what has not been wrapped yet: a text by the column (its
        // font is fixed-pitch), a name by its own average glyph.
        let text_w = self.card.width() - 2.0 * LIST_PAD - 2.0 * TILE_PAD_X;
        let text_px = TEXT_PX * self.card.zoom();
        let mono = renderer.measure_text("MMMMMMMMMM", text_px, Some(crate::options::NERD)) / 10.0;
        for item in &self.card.items {
            if self.card.lines.contains_key(&item.id) {
                continue;
            }
            let lines = if self.card.page == Page::Memory {
                // (A memory's row: its name, its last thing — one line each.)
                item.body.lines().take(2).map(str::to_owned).collect()
            } else if item.kind == Kind::Voice {
                Vec::new()
            } else if item.kind == Kind::Text {
                let cols = if mono > 0.0 {
                    (text_w / mono).floor() as usize
                } else {
                    40
                };
                wrap(&item.body, cols, MAX_LINES)
            } else {
                let n = item.body.chars().count().max(1);
                let w = renderer.measure_text(&item.body, text_px, None);
                let cols = if w > 0.0 {
                    (text_w * 0.94 / (w / n as f32)).floor() as usize
                } else {
                    40
                };
                wrap(&item.body, cols, 3)
            };
            self.card.lines.insert(item.id, lines);
        }
        // The items move aside for a drag over the card: those from where
        // it would land on go down by the place it needs, the rest stay —
        // each easing to where it should be (Max, 2026-10-08: *"the items
        // react to create spots"*).
        let opening = self.card.opening();
        let hidden = self
            .card
            .drag
            .as_ref()
            .filter(|_| self.card.dnd_over && self.card.drop_own)
            .map(|d| d.id);
        let mut shifting = false;
        let ids: Vec<u64> = self
            .card
            .items
            .iter()
            .map(|it| it.id)
            .filter(|id| Some(*id) != hidden)
            .collect();
        self.card.shifts.retain(|id, _| ids.contains(id));
        for (n, id) in ids.iter().enumerate() {
            let target = match opening {
                Some((index, height)) if n >= index => height,
                _ => 0.0,
            };
            let now_at = self.card.shifts.get(id).copied().unwrap_or(0.0);
            let (to, going) = crate::animation::ease_toward(now_at, target, dt, SHIFT_RATE, 0.4);
            shifting |= going;
            if to == 0.0 {
                self.card.shifts.remove(id);
            } else {
                self.card.shifts.insert(*id, to);
            }
        }
        if shifting {
            moving = true;
            self.card_last_frame = Some(now);
        }
        let hover = self.card.ptr.and_then(|p| self.card.hit(p));
        // The items of the session that are pinned too: their pin is lit.
        let pinned: HashSet<u64> = match self.card.page {
            Page::Session => self
                .card
                .items
                .iter()
                .filter(|it| self.card.pinned.iter().any(|p| p.same(it)))
                .map(|it| it.id)
                .collect(),
            _ => HashSet::new(),
        };
        // A list that was showing its newest item keeps showing it when
        // the card's height changes under it (its window was resized).
        if self.card.max_scroll > 0.0 && self.card.scroll >= self.card.max_scroll - 0.5 {
            self.card.to_bottom = true;
        }
        // What is written in the input box, wrapped to it.
        let low = bottom(rect, self.card.page, 1, self.card.seeking(), false);
        let box_w = low.text().map_or(200.0, |r| r.w);
        let per = renderer.measure_text("nnnnnnnnnn", TEXT_PX + 0.5, None) / 10.0;
        let cols = if per > 0.0 {
            (box_w / per).floor() as usize
        } else {
            30
        };
        self.card.draft_lines = if self.card.draft.is_empty() {
            Vec::new()
        } else {
            let mut lines = wrap(&self.card.draft, cols.max(8), 200);
            if self.card.draft.ends_with('\n') {
                lines.push(String::new());
            }
            lines
        };
        // The search narrows the list to what matches.
        let found: Vec<Item>;
        let listed: &[Item] = if self.card.query.trim().is_empty() {
            &self.card.items
        } else {
            found = self
                .card
                .items
                .iter()
                .filter(|it| self.card.matches(it))
                .cloned()
                .collect();
            &found
        };
        // The line at the foot of each item: when it is from (and, on the
        // clipboard's page, which app).
        let now_s = now_secs();
        let pinned_too = &pinned;
        let notes: HashMap<u64, String> = listed
            .iter()
            .map(|it| {
                let when = when_text(it.at, now_s);
                let note = match (self.card.page, &it.from) {
                    (Page::Clipboard, Some(from)) => {
                        let from: String = from.chars().take(26).collect();
                        format!("{from} · {when}")
                    }
                    _ if pinned_too.contains(&it.id) => format!("pinned · {when}"),
                    _ => when,
                };
                (it.id, note)
            })
            .collect();
        let name = self.card.open_name();
        let recording = self.card.rec.as_ref().map(|r| (r.seconds(), r.talk()));
        let talk = match (recording, self.card.writing) {
            (Some((_, true)), _) => Some("Listening…  press again to write it"),
            (_, true) => Some("Writing what you said…"),
            _ => None,
        };
        let playing = self
            .card
            .playing
            .as_ref()
            .map(|p| (p.id, p.started.elapsed().as_secs_f32()));
        let hint = match hover {
            _ if self.card.say.is_some() => self.card.say.unwrap_or(""),
            Some(Hover::Btn(which)) => which.hint(),
            _ if self.card.draft.trim().is_empty() => "",
            _ => "Enter keeps it in this memory  ·  Ctrl+Enter sends it to the window",
        };
        let view = View {
            rect,
            shown,
            items: listed,
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
            foot: Some(FootView {
                page: self.card.page,
                pinned: &pinned,
                current: self
                    .card
                    .host
                    .as_ref()
                    .and_then(|h| self.card.wins.get(h))
                    .and_then(|w| w.session),
                bar: BarView {
                    name: name.as_deref(),
                    naming: Some(&self.card.naming),
                    draft: &self.card.draft_lines,
                    query: &self.card.query,
                    found: listed.len(),
                    field: self.card.field,
                    rec: recording.filter(|(_, talk)| !talk).map(|(secs, _)| secs),
                    talk,
                    hint,
                    picking: self.card.picking,
                },
                notes: &notes,
                playing,
            }),
            dnd_over: self.card.dnd_over,
            hidden,
            shifts: &self.card.shifts,
            slots: &self.card.slots,
            paint,
            zoom: self.card.zoom(),
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
        // A picture on screen that has no pixels (never asked for, or pushed
        // out of its layer by newer ones) is asked for.
        let wanted: Vec<String> = self
            .card
            .tiles
            .iter()
            .filter(|t| t.rect.y < rect.y + rect.h && t.rect.y + t.rect.h > rect.y)
            .filter_map(|t| self.card.item(t.id))
            .filter(|it| it.kind == Kind::Image)
            .filter_map(|it| it.path.clone())
            .filter(|p| !self.card.slots.contains_key(p))
            .collect();
        for path in wanted {
            self.card_ask_picture(&path);
        }
        if self.card.host.is_none() {
            // That was the last frame of a card that is gone.
            self.card_park();
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

    /// The card's colours right now (the OPTIONS boxes' own, read on its
    /// window's side of the screen).
    pub(super) fn card_paint(&self) -> Paint {
        let side = match self.card.spot {
            Some(s) => Rect::new(s.x, s.y, s.w, s.h),
            None => self.card.rect.unwrap_or_default(),
        };
        let (fill, ink) = self.box_surface_at(side);
        Paint { fill, ink }
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
                self.card_keys(true);
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
                self.card_keys(false);
                match self.card.press.take() {
                    Some(Press::Slide { moved: true, .. }) => self.card_settle(),
                    Some(Press::Resize { .. }) => self.card_settle(),
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
                        Some(Hover::Item(id)) => Some(Press::Item { id, at, serial }),
                        Some(button) => Some(Press::Tap(button)),
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
                        // Pressed and let go without carrying it off: a click.
                        Some(Press::Item { id, .. }) => self.card_paste(id),
                        Some(Press::Tap(what)) => {
                            if self.card_still_on(what) {
                                self.card_tap(what);
                            }
                        }
                        Some(Press::Slide { moved: true, .. }) => self.card_settle(),
                        Some(Press::Resize { .. }) => self.card_settle(),
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
        self.card_wait(Wait::Wheel, SCROLL_QUIET);
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
        self.card.wheel = Wheel::default();
        if was_sideways {
            self.sync_card_input();
        }
    }

    /// Ctrl +/− (and Ctrl 0) are the card's while the pointer is on it, and
    /// the window's the rest of the time (Max, 2026-10-09: *"Ctrl + +- to
    /// make the items bigger as the window. but only when i hover the
    /// card"*). The card never has the keyboard — its window keeps typing
    /// — so the keys are taken as compositor BINDS for exactly as long as
    /// the pointer is on it: bound on its enter, unbound on its leave.
    /// (`hl.bind` adds: always unbound first, or one press would act twice.)
    pub(crate) fn card_keys(&mut self, on: bool) {
        if on == self.card.keys {
            return;
        }
        self.card.keys = on;
        let ctl = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("waverunner-ctl")))
            .filter(|ctl| ctl.exists());
        let mut lua = String::new();
        for (key, what) in ZOOM_KEYS {
            lua.push_str(&format!("hl.unbind(\"{key}\") "));
            if let (true, Some(ctl)) = (on, &ctl) {
                lua.push_str(&format!(
                    "hl.bind(\"{key}\", hl.dsp.exec_cmd(\"{} card zoom {what}\")) ",
                    ctl.display()
                ));
            }
        }
        let ok = crate::hypr::eval_ok(&lua);
        info!(
            "card: Ctrl +/- {} (ctl {}, compositor said {})",
            if on { "taken" } else { "given back" },
            if ctl.is_some() { "found" } else { "MISSING" },
            if ok { "ok" } else { "NO" }
        );
    }

    /// Make the items bigger or smaller, or as designed again.
    fn card_zoom(&mut self, what: &str) -> String {
        self.card_load();
        let now = self.card.zoom();
        let to = match what {
            "in" => now + ZOOM_STEP,
            "out" => now - ZOOM_STEP,
            "reset" | "" => 1.0,
            n => n.parse().unwrap_or(now),
        };
        // (On a tenth, so ten steps out and ten back end where they began.)
        let to = ((to * 10.0).round() / 10.0).clamp(ZOOM_MIN, ZOOM_MAX);
        if to != now {
            self.card.zoom = to;
            self.card.lines.clear();
            self.card.shifts.clear();
            self.card_save();
            self.request_card_draw();
        }
        format!("zoom {to:.1}")
    }

    /// Whether the pointer is (still) on `what`: a button acts when it is
    /// let go where it was pressed.
    fn card_still_on(&self, what: Hover) -> bool {
        self.card.ptr.and_then(|p| self.card.hit(p)) == Some(what)
    }

    fn card_motion(&mut self, x: f32, y: f32) {
        let before = self.card.ptr.and_then(|p| self.card.hit(p));
        self.card.ptr = Some((x, y));
        match self.card.press {
            // (A session in memory is not carried anywhere: it is clicked.)
            Some(Press::Item { id, at, serial })
                if self.card.page != Page::Memory && (x - at.0).hypot(y - at.1) >= DRAG_START =>
            {
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
                            self.card.wins.entry(host).or_default().width = Some(to);
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
        if self.card.host.as_deref() == Some(addr) {
            self.card_away(Away::Drag);
        }
    }

    /// The window was put down: the card comes back on it, at the same
    /// place relative to the window, unrolling as when it is summoned. At
    /// once: a 220 ms wait before the reveal was tried and taken back the
    /// same day (Max, 2026-10-08: *"i dont think we need the delay"*).
    pub(crate) fn card_window_placed(&mut self, addr: &str) {
        if self.card.host.as_deref() == Some(addr) {
            self.card_back(Away::Drag);
        }
    }

    /// The card's window was moved a step by a gesture with no end of its
    /// own (the scroll on the OPTIONS pill): in hand from the first step,
    /// put down when the steps have stopped for [`NUDGE_QUIET`].
    pub(crate) fn card_window_nudged(&mut self, addr: &str) {
        if self.card.host.as_deref() == Some(addr) {
            self.card_away(Away::Nudge);
            self.card_wait(Wait::Nudge, NUDGE_QUIET);
        }
    }

    /// A workspace swipe began (the plugin hears the fingers land): the
    /// card is gone at once — it is a layer, and would stand still over
    /// windows sliding away under it.
    pub(crate) fn card_swipe_away(&mut self) {
        // (A return still waiting on the last swipe is called off.)
        self.card.waits[Wait::SwipeBack as usize] = None;
        self.card_away(Away::Swipe);
    }

    /// The fingers left: once the slide has settled the card comes back,
    /// if its window is still the one showing (`sync_card` decides).
    pub(crate) fn card_swipe_back(&mut self) {
        if self.card.away & Away::Swipe as u8 != 0 {
            self.card_wait(Wait::SwipeBack, SWIPE_SETTLE);
        }
    }

    /// A scroll on the title bar of the card's window (the plugin hears it
    /// and sends it on), or sideways over the card: the card slides by it,
    /// down or right taking it left as in the mockup, and stays where it is
    /// left — remembered for this window like a slide by hand.
    pub(crate) fn card_slide(&mut self, addr: &str, delta: f32) {
        if self.card.host.as_deref() != Some(addr) || !self.card_present() {
            return;
        }
        let (Some(rect), Some(spot)) = (self.card.rect, self.card.spot) else {
            return;
        };
        if matches!(
            self.card.press,
            Some(Press::Slide { .. } | Press::Resize { .. })
        ) {
            return;
        }
        // Measured as a run, for a throw: judged once it stops.
        self.card.swipe = Some(Swipe::step(self.card.swipe, Instant::now(), delta));
        self.card_wait(Wait::Throw, FLING_QUIET);
        // Where the scroll says the card should be — from where it is
        // GOING, so small scrolls add up — and it sets off for it from
        // where it is drawn, at its own pace (`TRAVEL_SPEED`).
        let to = clamp_left(
            self.card.left(spot.w) - delta * SLIDE_PER_SCROLL,
            spot.w,
            rect.w,
        );
        self.card.put(to, spot.w);
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
    /// card goes on to that side, outside its window.
    fn card_swipe_judge(&mut self) {
        self.card.waits[Wait::Throw as usize] = None;
        let Some(sw) = self.card.swipe.take() else {
            return;
        };
        let (Some(to_left), Some(spot), Some(rect), Some(host)) = (
            sw.thrown(),
            self.card.spot,
            self.card.rect,
            self.card.host.clone(),
        ) else {
            return;
        };
        if self.card_present() {
            // Its place is the far side from now on; it carries on there
            // at the pace it was already travelling.
            self.card.wins.entry(host).or_default().place = Some(Place::thrown(to_left, rect.w));
            self.card_set_off(rect.x - spot.x);
        }
    }

    /// The card was let go after being slid or resized by hand: it stays
    /// exactly there (no snapping — the mockup's rule), kept for this
    /// window.
    fn card_settle(&mut self) {
        if let (Some(rect), Some(spot)) = (self.card.rect, self.card.spot) {
            self.card.put(rect.x - spot.x, spot.w);
        }
        self.card_save_session();
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
            (_, Some(Hover::Item(_))) if self.card.page != Page::Memory => Shape::Default,
            (_, Some(Hover::Input | Hover::Name)) => Shape::Text,
            (_, Some(Hover::Seek)) if self.card.seeking() => Shape::Text,
            (_, Some(_)) => Shape::Pointer,
            (_, None) => Shape::Grab,
        };
        if self.cursor_now != Some(shape) {
            device.set_shape(self.enter_serial, shape);
            self.cursor_now = Some(shape);
        }
    }

    /// Where the card stands, in a line (the answer to the page verbs).
    fn card_words(&self) -> String {
        let sessions: Vec<String> = self
            .card
            .sessions()
            .iter()
            .map(|p| {
                format!(
                    "{}:{:?}:{}",
                    p.id,
                    p.name.as_deref().unwrap_or(""),
                    p.items.len()
                )
            })
            .collect();
        format!(
            "{:?}, session {:?}, {} pinned, sessions [{}]",
            self.card.page,
            self.card.open,
            self.card.pinned_items().len(),
            sessions.join(", ")
        )
    }

    /// `waverunner-ctl card …`: the card without a pointer.
    pub(crate) fn card_command(&mut self, what: &str) -> String {
        let what = what.trim();
        let (verb, rest) = what.split_once(' ').unwrap_or((what, ""));
        let rest = rest.trim();
        match verb {
            "" | "toggle" => self.card_toggle((!rest.is_empty()).then_some(rest)),
            // The input box, the search and the name, without a keyboard:
            // `write <text>` · `keep` · `send` · `find <words>` · `rename <name>`
            // · `clipboard` (its page).
            "write" => {
                self.card.draft = rest.replace("\\n", "\n");
                self.request_card_draw();
                format!("draft {:?}", self.card.draft)
            }
            "keep" => {
                self.card_keep_draft();
                self.card_words()
            }
            "send" => {
                self.card_send_draft();
                "sent".to_owned()
            }
            "find" => {
                self.card.query = rest.to_owned();
                self.request_card_draw();
                format!("find {:?}", self.card.query)
            }
            "rename" => {
                self.card_rename(rest);
                self.card.lines.clear();
                self.request_card_draw();
                self.card_words()
            }
            "clipboard" => {
                self.card_show(Page::Clipboard);
                format!("{} rows", self.card.items.len())
            }
            // `zoom in|out|reset|<n>`: the items' size (Ctrl +/− over the card).
            "zoom" => self.card_zoom(rest),
            // The foot's buttons, and what the pages do, without a pointer:
            // `new` · `memory` · `pinned` · `session` · `pin <n>` · `open <n>`
            // (a session of memory) · `forget <n>`.
            "new" | "memory" | "pinned" | "session" => {
                match verb {
                    "new" => self.card_begin(),
                    "memory" => self.card_show(Page::Memory),
                    "pinned" => self.card_show(Page::Pinned),
                    _ => self.card_show(Page::Session),
                }
                self.card_words()
            }
            "pin" | "open" | "forget" => {
                self.card_load();
                let n = rest.parse::<usize>().ok();
                let done = match verb {
                    "pin" => {
                        self.card_show(Page::Session);
                        n.and_then(|n| self.card.items.get(n))
                            .map(|it| it.id)
                            .map(|id| self.card_pin(id))
                    }
                    "open" => n
                        .and_then(|n| self.card.memory.get(n))
                        .map(|p| p.id)
                        .map(|id| self.card_recall(id)),
                    _ => n
                        .and_then(|n| self.card.memory.get(n))
                        .map(|p| p.id)
                        .map(|id| self.card_forget(id)),
                };
                match done {
                    Some(()) => self.card_words(),
                    None => "no such one".to_owned(),
                }
            }
            // (These work on the SESSION's items.)
            "add" | "remove" | "clear" | "move" | "paste" if self.card.page != Page::Session => {
                self.card_show(Page::Session);
                self.card_command(what)
            }
            "all" => self.card_toggle_all(),
            // `move <n> <place>`: item n to that place of the list, as a
            // drag within the card does.
            "move" => {
                self.card_load();
                let mut parts = rest.split_whitespace().map(|p| p.parse::<usize>().ok());
                let (from, to) = (parts.next().flatten(), parts.next().flatten());
                match from
                    .and_then(|n| self.card.items.get(n))
                    .map(|it| it.id)
                    .zip(to)
                {
                    Some((id, to)) => {
                        self.card.move_to(id, to);
                        self.card_save();
                        self.request_card_draw();
                        "moved".to_owned()
                    }
                    None => "move <n> <place>".to_owned(),
                }
            }
            // The plugin has just (re)loaded: its bars know nothing.
            "bars" => {
                self.card_tell_bars();
                String::new()
            }
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
            "away" => {
                self.card_swipe_away();
                String::new()
            }
            "back" => {
                self.card_swipe_back();
                String::new()
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
                    self.card_add_paths(vec![PathBuf::from(path.trim())]);
                    "adding".to_owned()
                }
                _ => "add text <…> | add file <path>".to_owned(),
            },
            "paste" => {
                self.card_load();
                match rest
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| self.card.items.get(n))
                    .map(|it| it.id)
                {
                    Some(id) => {
                        self.card_paste(id);
                        "pasted".to_owned()
                    }
                    None => "no such item".to_owned(),
                }
            }
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
                    "host {:?} spot {:?} rect {:?} shown {:.2} away {} parked {} all {} windows {:?} scroll {:.0}/{:.0} pictures {} page {:?} pinned {} memory {} items [{}]",
                    self.card.host,
                    self.card.spot,
                    self.card.rect,
                    self.card.shown,
                    self.card.away,
                    self.card.parked,
                    self.card.all,
                    self.card.wins,
                    self.card.scroll,
                    self.card.max_scroll,
                    self.card.slots.len(),
                    self.card.page,
                    self.card.pinned_items().len(),
                    self.card.memory.len(),
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

    #[test]
    fn each_window_keeps_its_own_width_and_place_until_it_closes() {
        let mut card = Card {
            host: Some("0x1".into()),
            ..Default::default()
        };
        // Never touched: the default width, resting on the right.
        assert_eq!(card.width(), WIDTH);
        assert_eq!(card.left(900.0), 900.0 - 10.0 - WIDTH);
        // Made wider and slid left on this window…
        card.wins.entry("0x1".into()).or_default().width = Some(9999.0);
        assert_eq!(card.width(), WIDTH_MAX, "within the limits");
        card.wins.entry("0x1".into()).or_default().width = Some(500.0);
        card.put(30.0, 900.0);
        assert_eq!((card.width(), card.left(900.0)), (500.0, 30.0));
        // …turned off and on again there: still so.
        assert!(card.toggle("0x1"));
        assert!(!card.toggle("0x1"));
        assert_eq!((card.width(), card.left(900.0)), (500.0, 30.0));
        // Another window has its own.
        card.host = Some("0x2".into());
        assert_eq!(
            (card.width(), card.left(900.0)),
            (WIDTH, 900.0 - 10.0 - WIDTH)
        );
        // The first one closes: the next window at that address finds nothing.
        card.forget("0x1");
        card.host = Some("0x1".into());
        assert_eq!(card.width(), WIDTH);
        assert!(!card.armed("0x1"));
    }

    #[test]
    fn an_item_moves_to_a_place_of_the_list_as_it_reads_without_it() {
        let item = |id: u64| Item {
            id,
            kind: Kind::Text,
            body: id.to_string(),
            path: None,
            aspect: 0.0,
            owned: false,
            at: 0,
            from: None,
        };
        let order = |c: &Card| c.items.iter().map(|it| it.id).collect::<Vec<_>>();
        let mut card = Card {
            items: (1..=4).map(item).collect(),
            ..Default::default()
        };
        // Down: 1 carried, dropped after what reads as [2, 3, 4]'s second.
        assert!(card.move_to(1, 2));
        assert_eq!(order(&card), [2, 3, 1, 4]);
        // Up, to the very top.
        assert!(card.move_to(4, 0));
        assert_eq!(order(&card), [4, 2, 3, 1]);
        // Past the end is the end; back where it was is no move at all.
        assert!(card.move_to(4, 99));
        assert_eq!(order(&card), [2, 3, 1, 4]);
        assert!(!card.move_to(3, 1));
        assert!(!card.move_to(77, 0));
    }

    #[test]
    fn the_side_edges_are_grips_inside_the_card_only() {
        let card = Card {
            rect: Some(Rect::new(100.0, 50.0, 400.0, 300.0)),
            ..Default::default()
        };
        assert_eq!(card.grip((103.0, 200.0)), Some(true));
        assert_eq!(card.grip((497.0, 200.0)), Some(false));
        assert_eq!(card.grip((300.0, 200.0)), None);
        assert_eq!(card.grip((98.0, 200.0)), None);
        assert_eq!(card.grip((103.0, 20.0)), None);
    }

    #[test]
    fn the_card_follows_the_focus_only_where_it_is_on() {
        let mut card = Card::default();
        assert!(!card.follows("0x1"));
        assert!(card.toggle("0x1"), "on for this window");
        assert!(card.follows("0x1") && !card.follows("0x2"));
        // It is there already: a focus on its own window brings nothing…
        card.host = Some("0x1".into());
        assert!(!card.follows("0x1"));
        // …unless it was on its way out.
        card.leaving = true;
        assert!(card.follows("0x1"));
        card.leaving = false;
        // The master switch: every window, but the ones told otherwise since.
        assert!(card.toggle_all());
        assert!(card.follows("0x2") && card.armed("0x9"));
        assert!(!card.toggle("0x2"));
        assert!(!card.follows("0x2") && card.armed("0x9"));
        // Off for all: what each was told by hand goes with it.
        assert!(!card.toggle_all());
        assert!(!card.armed("0x1") && !card.armed("0x2"));
    }

    #[test]
    fn the_card_is_back_only_when_nothing_keeps_it_away() {
        let mut card = Card::default();
        let now = Instant::now();
        assert!(card.hide(Away::Drag, now), "it was showing");
        assert!(card.hidden());
        // A second cause on top: it was already away.
        assert!(!card.hide(Away::Swipe, now));
        // The drag ends while the swipe goes on: still away.
        assert!(!card.unhide(Away::Drag));
        assert!(card.hidden());
        // The swipe settles: back.
        assert!(card.unhide(Away::Swipe));
        assert!(!card.hidden() && card.away_since.is_none());
        // Ending something that never began brings nothing back.
        assert!(!card.unhide(Away::Nudge));
    }

    #[test]
    fn only_a_windows_address_is_taken_for_one() {
        assert!(valid_addr("0x61ba51997770"));
        assert!(!valid_addr("61ba51997770"));
        assert!(!valid_addr("0x"));
        assert!(!valid_addr("0x12\"); os.execute(\"x\") --"));
        assert!(!valid_addr("*"));
    }

    #[test]
    fn the_session_reads_back_what_was_kept_for_each_window() {
        let mut wins = HashMap::new();
        wins.insert(
            "0x1".to_owned(),
            Win {
                on: Some(true),
                place: Some(Place::FromLeft(-432.0)),
                width: Some(500.0),
                session: Some(7),
            },
        );
        wins.insert("0x2".to_owned(), Win::default());
        let json = serde_json::to_string(&Session {
            all: true,
            wins: wins.clone(),
        })
        .unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert!(back.all);
        assert_eq!(back.wins, wins);
    }

    fn note(id: u64, body: &str) -> Item {
        Item {
            id,
            kind: Kind::Text,
            body: body.to_owned(),
            path: None,
            aspect: 0.0,
            owned: false,
            at: 0,
            from: None,
        }
    }

    #[test]
    fn each_window_works_with_its_own_session() {
        let bodies = |items: &[Item]| items.iter().map(|it| it.body.clone()).collect::<Vec<_>>();
        let mut card = Card::default();
        // A session started in one window is in memory at once, empty.
        let a = card.begin(Some("Golem new feature".into()), 100);
        card.give("0x1", Some("Golem new feature"), a);
        card.turn_to(Some(a));
        assert_eq!((card.page, card.memory.len()), (Page::Session, 1));
        card.items.push(note(10, "for the feature"));

        // Another window, another session.
        let b = card.begin(Some("Desktop".into()), 200);
        card.give("0x2", Some("Desktop"), b);
        card.turn_to(Some(b));
        assert!(card.items.is_empty());
        card.items.push(note(11, "for the desktop"));
        assert_eq!(card.memory[0].name.as_deref(), Some("Desktop"));

        // Each window finds its own; what was on the card went back.
        assert_eq!(card.session_for("0x1", None), Some(a));
        card.turn_to(Some(a));
        assert_eq!(bodies(&card.items), ["for the feature"]);
        assert_eq!(bodies(&card.sessions()[0].items), ["for the desktop"]);

        // A new window of a known title finds that session; an unknown one
        // has to pick (there are two) — Memory's page, a row a session.
        assert_eq!(card.session_for("0x9", Some("Desktop")), Some(b));
        assert_eq!(card.session_for("0x9", Some("Seam")), None);
        card.turn_to(None);
        assert_eq!((card.page, card.items.len()), (Page::Memory, 2));
        assert_eq!(bodies(&card.sessions()[1].items), ["for the feature"]);

        // Forgotten: its windows and its title have nothing any more, and
        // with one session left there is nothing to pick.
        assert!(card.lose(b).is_some());
        assert_eq!(card.items.len(), 1);
        assert_eq!(card.session_for("0x2", Some("Desktop")), Some(a));
        assert!(card.titles.get("Desktop").is_none());
    }

    #[test]
    fn a_pin_is_a_copy_that_outlives_the_session() {
        let mut card = Card::default();
        let a = card.begin(None, 1);
        card.turn_to(Some(a));
        card.items = vec![note(1, "me@example.org"), note(2, "b")];
        card.next_id = 3;
        assert_eq!(card.pin(1), Some(true));
        assert_eq!(card.pinned.len(), 1);
        assert_ne!(card.pinned[0].id, 1);
        assert!(card.pinned[0].same(&card.items[0]));
        // The session goes; the pin stays, and is the list of its page.
        card.lose(a);
        assert!(card.show(Page::Pinned));
        assert_eq!(card.items.len(), 1);
        assert_eq!(card.pinned_items().len(), 1);
        // (No pinning from the pinned page itself.)
        let pid = card.items[0].id;
        assert_eq!(card.pin(pid), None);
        // On a session that has the same thing: its pin comes off.
        let b = card.begin(None, 2);
        card.turn_to(Some(b));
        card.items.push(note(7, "me@example.org"));
        assert_eq!(card.pin(7), Some(false));
        assert!(card.pinned.is_empty());
    }

    #[test]
    fn a_picture_of_the_cards_own_is_kept_while_anything_shows_it() {
        let pic = |id: u64| Item {
            kind: Kind::Image,
            path: Some("/tmp/card/p.png".to_owned()),
            owned: true,
            ..note(id, "p.png")
        };
        let mut card = Card::default();
        let a = card.begin(None, 1);
        card.turn_to(Some(a));
        card.items = vec![pic(1)];
        card.next_id = 5;
        card.pin(1);
        // Off the screen (Memory's page is up), but pinned and in memory.
        card.turn_to(None);
        assert!(card.keeps("/tmp/card/p.png"));
        card.pinned.clear();
        assert!(card.keeps("/tmp/card/p.png"));
        card.lose(a);
        assert!(!card.keeps("/tmp/card/p.png"));
    }
}

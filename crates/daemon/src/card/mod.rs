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
pub(crate) mod dictate;
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
/// A press that travels this far takes the item along (a short way: it is
/// easy to grab), or the card.
const DRAG_START: f32 = 3.0;
const SLIDE_START: f32 = 6.0;
/// A press on an item held this long and let go without carrying it was a
/// grab that did not take, not a click: nothing is pasted (Max, 2026-10-11:
/// *"i click a lot on the items when i'm trying to grab them"*).
const CLICK_MS: u128 = 300;
/// A click on an item acts this long after it — if the pointer has not
/// gone this far from it meanwhile (`card_clicked`).
const CLICK_WAIT_MS: u64 = 180;
const CLICK_AWAY: f32 = 8.0;
/// A press on an item held this long takes it in hand where it is, with no
/// travel at all: to grab is to press and hold a moment.
const HOLD_MS_GRAB: u64 = 160;
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

/// How many emoji the picker remembers as used lately (two rows).
const RECENT_EMOJI: usize = 16;

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
    /// In the input box's words: travel selects them.
    Select,
    /// On a button of any kind (a pin, a tab, the search, a button of the
    /// input box…): let go there and it acts.
    Tap(Hover),
    /// On an item: travel takes it into a drag.
    Item {
        id: u64,
        at: (f32, f32),
        serial: u32,
        since: Instant,
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

/// The card hung under the bar (see `Card::banner`).
#[derive(Debug, Clone, Copy)]
struct Banner {
    x: f32,
    y: f32,
    /// It stands IN THE BAR'S ROW, where the long pill was — its top at
    /// the pill's top (Max, 2026-10-12: *"i want the long pill to become
    /// the card"*, then *"the scroll does not hide the old content of the
    /// long pill"*): the bar draws no long pill then and gives that stretch
    /// of its row to the card (`sync_options_input`). Not so when the bar
    /// is painted (a maximized window's): it would cover the card's top —
    /// there the card hangs from the bar's foot.
    in_bar: bool,
    was_on: bool,
    /// It is rolling up to go away: it keeps its place under the bar till
    /// it is gone.
    closing: bool,
}

/// How tall the card from the bar is at most.
const BANNER_H: f32 = 640.0;

/// What a second of a drag was like (see `draw_card`).
#[derive(Debug, Default, Clone, Copy)]
struct DragPace {
    since: Option<Instant>,
    draws: u32,
    worst_draw: f32,
    all_draw: f32,
    moves: u32,
    worst_wait: f32,
    last_move: Option<Instant>,
}

/// An item in hand: a Wayland drag of ours.
pub(crate) struct Drag {
    pub id: u64,
    /// The item itself, as it was when it was picked up: it may be let go
    /// on ANOTHER page of the card (held over a button of the head, the
    /// card turns to that page), where its list no longer has it.
    pub item: Item,
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
    /// The page the card was on, on this window (Max, 2026-10-11: *"the
    /// tab should stay where i left it, along with the size and position;
    /// it resets only when i close the window"*). Not written to disk: a
    /// dock that starts again opens on the window's session.
    #[serde(skip)]
    page: Option<Page>,
}

/// What was just removed, kept for as long as the hint offers it back
/// ("Removed · Undo"): an item with its place in its list, or a whole
/// session with its place in memory.
enum Undone {
    Item {
        item: Item,
        at: usize,
        page: Page,
        open: Option<u64>,
    },
    Session {
        past: Past,
        at: usize,
        was_open: bool,
    },
}

/// Which language `text` is in, told from its own common words (the one
/// most of them belong to; English when none says otherwise). A guess from
/// a few words — good for a sentence, not for a single word.
fn language_of(text: &str) -> &'static str {
    const WORDS: [(&str, &str); 6] = [
        ("es", " el la los las de que y en un una es no por con para como está esto hola qué más pero muy "),
        ("en", " the of and to in is you that it for with as this are how what hello need thanks your not "),
        ("fr", " le la les des et est un une que pour dans avec vous je bonjour comment ça merci pas très "),
        ("de", " der die das und ist ein eine nicht ich du wie guten morgen danke mit für auch sehr "),
        ("pt", " o os as um uma que não com para você como está obrigado olá isso muito mas também "),
        ("it", " il lo gli le un una che non per con come stai grazie ciao questo sono molto anche "),
    ];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .collect();
    WORDS
        .iter()
        .map(|(code, list)| {
            let n = words
                .iter()
                .filter(|w| list.contains(&format!(" {w} ")))
                .count();
            (*code, n)
        })
        .fold(
            ("en", 0),
            |best, now| if now.1 > best.1 { now } else { best },
        )
        .0
}

/// The rows of a translation's menu of languages, and the code each stands
/// for (the first six: what it goes into; the last six: what it is in).
const LANG_ROWS: [&str; 12] = [
    "To English",
    "To Spanish",
    "To French",
    "To German",
    "To Portuguese",
    "To Italian",
    "It is English",
    "It is Spanish",
    "It is French",
    "It is German",
    "It is Portuguese",
    "It is Italian",
];
const LANG_CODES: [&str; 6] = ["en", "es", "fr", "de", "pt", "it"];

/// The two languages of a translation: the ones picked for it by hand (its
/// `from` keeps them, "es>en"), else the ones its words are told to be in.
fn pair_of(item: &Item) -> (&'static str, &'static str) {
    let known = |code: &str| {
        ["en", "es", "fr", "de", "pt", "it"]
            .into_iter()
            .find(|c| *c == code)
    };
    item.from
        .as_deref()
        .and_then(|f| f.split_once('>'))
        .and_then(|(a, b)| known(a).zip(known(b)))
        .unwrap_or_else(|| {
            let from = language_of(&item.body);
            (from, if from == "en" { "es" } else { "en" })
        })
}

/// A language's name, from its code.
fn language_name(code: &str) -> &'static str {
    match code {
        "es" => "Spanish",
        "en" => "English",
        "fr" => "French",
        "de" => "German",
        "pt" => "Portuguese",
        "it" => "Italian",
        _ => "?",
    }
}

/// Translate `text` from one language into another with the engine on this
/// computer: `translateLocally` (offline — marian with the Bergamot models;
/// the flake's `translate` package, pointed at by `WAVERUNNER_TRANSLATE`),
/// given the words on its input and the pair as its model ("es-en-tiny").
/// What comes back is the translation — or, in brackets, why there is
/// none: a reason, not words.
fn translate(text: &str, from: &str, to: &str) -> String {
    // (The pairs it has all go through English: one that has none of it on
    // either side is done in two steps.)
    if from != "en" && to != "en" {
        let english = translate_pair(text, from, "en");
        if english.starts_with('(') {
            return english;
        }
        return translate_pair(&english, "en", to);
    }
    translate_pair(text, from, to)
}

fn translate_pair(text: &str, from: &str, to: &str) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let Some(engine) = voice::tool("WAVERUNNER_TRANSLATE", "translateLocally") else {
        return "(No translation engine on this computer yet)".to_owned();
    };
    let child = Command::new(engine)
        .args(["-m", &format!("{from}-{to}-tiny")])
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return "(The translation engine did not start)".to_owned();
    };
    if let Some(mut input) = child.stdin.take() {
        let _ = input.write_all(text.as_bytes());
        let _ = input.write_all(b"\n");
    }
    match child.wait_with_output() {
        Ok(out) if out.status.success() => {
            let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if said.is_empty() {
                "(Nothing came back)".to_owned()
            } else {
                said
            }
        }
        // (It has no model for that pair.)
        Ok(_) => format!(
            "({} → {} is not on this computer yet)",
            language_name(from),
            language_name(to)
        ),
        Err(_) => "(The translation engine did not finish)".to_owned(),
    }
}

/// The id of the row "Pinned" among what a search of Memory's page finds.
const PINNED_ROW: u64 = u64::MAX - 9;

/// How many lines a text opened whole ("Show all") shows at the most.
const ALL_LINES: usize = 400;

/// How long Space is held before it is a voice note and not a tap (ms).
const HOLD_MS: u64 = 350;

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
    /// Its window is not the focused one: the card is its own window's, and
    /// is not drawn over the others (Max, 2026-10-10). It is back when its
    /// window is focused again.
    Focus = 8,
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
    /// Memory's page, SEARCHED: what is listed instead of its rows — each
    /// session that has something of what is typed (its row, then those
    /// things), then "Pinned" and its own; and which of them are rows.
    finds: Vec<Item>,
    find_rows: HashSet<u64>,
    /// The words looked up in the dictionary (its page's list), where they
    /// are kept while another page is up.
    looked: Vec<Item>,
    /// The things translated (the translator's list), where they are kept
    /// while another page is up.
    translated: Vec<Item>,
    /// The words whose entries could not be read yet (the dictionary was
    /// still being opened): looked up again once it is.
    looking: Vec<u64>,
    memory: Vec<Past>,
    /// The session each window title was given (see `model::Saved`).
    titles: HashMap<String, u64>,
    /// How big the items are drawn (0 = never set; read with `zoom()`).
    zoom: f32,
    /// The card has the keyboard, and the typing goes into this (the
    /// input box, the search, the memory's name). `None`: its window has
    /// the keyboard, as always.
    field: Option<Field>,
    /// Tab (or a paste) handed the keys to the window with the pointer still
    /// on the card: they come back on a click on it, or when the pointer
    /// leaves and comes in again — not on a mere movement.
    tabbed: bool,
    /// The window that has the focus, as last heard from the compositor.
    active: Option<String>,
    /// The width the items' words were last wrapped to (`lines`): another
    /// one, and they are wrapped again.
    wrapped_w: f32,
    /// What is in the writing box is this item, being changed (its pencil);
    /// `None`: a new thing.
    edit: Option<u64>,
    /// Where the writing box is in the list this frame (as the tiles are:
    /// in the space the card is laid out in).
    writer: Option<Rect>,
    /// The items that are OPEN (a click on one made of parts): their voice
    /// notes show what they say, and their pieces answer by themselves —
    /// played, pasted or dragged alone.
    wide: HashSet<u64>,
    /// The parts already put in what is being written: they stand in the
    /// box, over the line being written, in the order they were put in.
    sealed: Vec<Item>,
    /// The +'s menu (of the writing box) is open.
    plus: bool,
    /// What is written is being kept right now (so what `card_push` is
    /// given then is the thing itself, not one more part of it).
    keeping: bool,
    /// The item whose disk's menu is open.
    menu: Option<u64>,
    /// The translation whose menu of languages is open.
    lang: Option<u64>,
    /// SMOOTH SCROLLING THAT KEEPS GOING (Max, 2026-10-11): where the list is
    /// easing to (a wheel step adds to it; the list follows on the frames),
    /// how fast the wheel was going (px/s) and when its last step was — a
    /// fast scroll let go of glides on from there (`gliding`), slowing down.
    scroll_to: Option<f32>,
    scroll_vel: f32,
    scroll_last: Option<Instant>,
    gliding: bool,
    /// The scroll comes from fingers on a touchpad (not a wheel's notches):
    /// the list then FOLLOWS them, step for step — easing after them felt
    /// elastic on a short scroll (Max, 2026-10-11).
    finger: bool,
    /// How far this scroll has gone since it began (px): a short one does
    /// not glide.
    scroll_run: f32,
    /// A drag is held over this button of the head, since then: held long
    /// enough, the card turns to its page (so a thing can be carried from
    /// any page to any other).
    dnd_foot: Option<(Foot, Instant)>,
    /// A drag was let go on this session's row (Memory's page): what it
    /// brought is kept in that session.
    drop_session: Option<u64>,
    /// A session is being CHOSEN on Memory's page, to keep this in; and the
    /// page to go back to once it is (or once it is given up).
    picking: Option<(Item, Page)>,
    /// What was just done, said where the hint is for a moment ("Saved").
    flash: Option<(String, Instant)>,
    /// What was just removed, while the hint still offers it back.
    undo: Option<Undone>,
    /// The session whose × was clicked once: a second click, soon, forgets it.
    arming: Option<(u64, Instant)>,
    /// The session being renamed in its row on Memory's page (`None`: the
    /// name being typed is the open session's own).
    renaming: Option<u64>,
    /// Voice to text's page: the last thing said (its one item), kept here
    /// while another page is up. It is gone with the next one, or when the
    /// card is closed.
    said_last: Option<Item>,
    /// The voice note being recorded (Space held) asks for its title the
    /// moment it is kept.
    name_next: bool,
    /// Space is down since then (nothing being written): held long enough
    /// it is a voice note, recorded until it is let go (`held`).
    space: Option<Instant>,
    held: bool,
    /// The dictation key is down (`dictate.rs`: talk written straight into
    /// the window, no card).
    dictation: Option<dictate::Dictation>,
    /// …or it went down with the card up here, and is the CARD's talk
    /// key: since when, and whether the card was on voice-to-text's page
    /// already (a tap then puts the card away).
    talk_key: Option<(Instant, bool)>,
    /// …and what the bar's card pill shows of it.
    dictate_show: Option<dictate::Show>,
    /// The pill is growing to it, or going back: how much wider than round
    /// it was (px) when that began, and when.
    dictate_from: Option<(f32, Instant)>,
    /// Its frames are running; Escape is taken (`dictate.rs`).
    dictate_ticking: bool,
    dictate_esc: bool,
    /// What is written in the input box, and that wrapped to the box.
    draft: String,
    draft_lines: Vec<String>,
    /// …the same lines as ranges of its characters, and where the
    /// writing cursor is among them (0 = before the first).
    draft_spans: Vec<(usize, usize)>,
    caret: usize,
    /// The first line of the input box in view when the WHEEL chose it
    /// (`None`: it follows the cursor), and the wheel's travel not yet
    /// worth a line.
    draft_first: Option<usize>,
    /// …and how far that line is scrolled out at the top (px, less than a
    /// line): what is written scrolls smoothly, not a line at a time.
    draft_off: f32,
    /// The scroll that is eased, or glides (`scroll_to`), is the EMOJI
    /// grid's, not the list's.
    scroll_pick: bool,
    /// The other end of what is SELECTED in the input box (the cursor is
    /// one end); `None`, or the cursor's own place: nothing is.
    anchor: Option<usize>,
    /// How wide each letter met in the input box is (measured once).
    letter_w: HashMap<char, f32>,
    /// What is being searched for on the page that is up.
    query: String,
    /// The memory's new name, while it is typed.
    naming: String,
    /// A recording in progress, a voice note being played, and whether
    /// what was said is being written out right now (`voice.rs`).
    rec: Option<voice::Rec>,
    playing: Option<voice::Playing>,
    /// The voice note that is paused, and how many seconds in it stopped.
    paused: Option<(u64, f32)>,
    /// THE CARD FROM THE BAR (Max, 2026-10-12: *"one circle that i can
    /// hover and scroll down to open the card from the banner, as the card
    /// we have now. same"*): while this is set the card does not stand on
    /// its window but hangs under the bar's card pill — the same card, its
    /// window's session and pages. `Some`: where its top left corner is,
    /// and whether it was showing on its window before (it goes back there
    /// when this is over; else it goes away).
    banner: Option<Banner>,
    /// A drag (anyone's) is over the BAR: held on the card's pill, it
    /// brings the card out (`card_bar_drag_at`).
    bar_drag: bool,
    /// The watch on a pointer that leaves without a word is running
    /// (`card_keys_watch`).
    keys_watch: bool,
    /// The item clicked once, and when: a second click on it soon after
    /// pastes it (`card_click`).
    clicked: Option<(u64, Instant, (f32, f32))>,
    /// How the drag in hand is going, this second (`draw_card`).
    drag_pace: Option<DragPace>,
    /// The timer that redraws the card while something records or plays
    /// is running (`card_voice_tick`: one at a time).
    voice_ticking: bool,
    /// How fast voice notes play (its place in `voice::SPEEDS`).
    voice_speed: usize,
    writing: bool,
    /// The emoji page's picker: what its search holds, the emoji it shows
    /// for that (`emoji_table::EMOJI` by place), how far its grid is
    /// scrolled.
    pick_query: String,
    picked: Vec<usize>,
    pick_scroll: f32,
    /// The emoji used lately (their place in the table), the latest
    /// first: the picker's first rows.
    recent: Vec<usize>,
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
/// THE DESKTOP, as a card's "window": a workspace with no window on it has
/// nothing for the card to stand on, so it hangs under the bar's pill
/// (the banner) and this is the address it is kept under — no window ever
/// has it (Max, 2026-10-12: *"if I'm on an empty workspace and I press
/// Super+. I want the card to open from the banner"*).
const DESK: &str = "0x0";

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

    /// The card's box as its contents are laid out in: `1/zoom` its size,
    /// about its top-left corner (`view::virt`).
    fn vrect(&self) -> Option<Rect> {
        self.rect.map(|r| virt(r, self.zoom()))
    }

    /// `pos` on the surface, in the space the contents are laid out in.
    fn inside(&self, pos: (f32, f32)) -> (f32, f32) {
        match self.rect {
            Some(r) => {
                let z = self.zoom();
                (r.x + (pos.0 - r.x) / z, r.y + (pos.1 - r.y) / z)
            }
            None => pos,
        }
    }

    /// A point of the laid-out space, on the surface.
    fn outside(&self, pos: (f32, f32)) -> (f32, f32) {
        match self.rect {
            Some(r) => {
                let z = self.zoom();
                (r.x + (pos.0 - r.x) * z, r.y + (pos.1 - r.y) * z)
            }
            None => pos,
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
        // (Under the bar it hangs from its corner: no place of its own.)
        if self.banner.is_some() {
            return 0.0;
        }
        self.win()
            .place
            .unwrap_or(Place::REST)
            .left(window_w, self.width())
    }

    /// Put the card with its left edge `left` from its window's (kept by
    /// the side it is nearer to).
    fn put(&mut self, left: f32, window_w: f32) {
        // (Slid under the bar, nothing is kept: that is not its window.)
        if self.banner.is_some() {
            return;
        }
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
                tile_height(it.kind, self.lines.get(&it.id).map_or(1, Vec::len), 1.0)
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
        // (An item of the list, or a part of one.)
        // (…or a thing a search of Memory's page found.)
        self.items
            .iter()
            .chain(self.finds.iter())
            .flat_map(|it| std::iter::once(it).chain(it.parts.iter()))
            .find(|it| it.id == id)
    }

    /// The same, to be changed.
    fn item_mut(&mut self, id: u64) -> Option<&mut Item> {
        for it in &mut self.items {
            if it.id == id {
                return Some(it);
            }
            if let Some(part) = it.parts.iter_mut().find(|p| p.id == id) {
                return Some(part);
            }
        }
        None
    }

    /// The emoji picker, when it is open: how many it shows, how far down.
    fn pick(&self) -> Option<(usize, f32)> {
        (self.page == Page::Emoji).then_some((self.picked.len(), self.pick_scroll))
    }

    /// Which emoji the picker shows: all, or those its search names.
    fn repick(&mut self) {
        let needle = self.pick_query.trim().to_lowercase();
        let found = crate::emoji_table::EMOJI
            .iter()
            .enumerate()
            .filter(|(_, def)| needle.is_empty() || crate::emoji::emoji_matches(def, &needle))
            .map(|(n, _)| n);
        // (With nothing searched: the ones used lately first, then all.)
        self.picked = match needle.is_empty() {
            true => self.recent.iter().copied().chain(found).collect(),
            false => found.collect(),
        };
        self.pick_scroll = 0.0;
    }

    /// An emoji was used: it is the first of the recent ones now.
    fn used(&mut self, at: usize) {
        self.recent.retain(|n| *n != at);
        self.recent.insert(0, at);
        self.recent.truncate(RECENT_EMOJI);
    }

    // ---- the input box's text and its writing cursor ----

    fn draft_len(&self) -> usize {
        self.draft.chars().count()
    }

    /// Where character `at` of the draft starts, in bytes.
    fn draft_byte(&self, at: usize) -> usize {
        self.draft
            .char_indices()
            .nth(at)
            .map_or(self.draft.len(), |(b, _)| b)
    }

    /// What is selected in the input box: from which character to which.
    fn selected(&self) -> Option<(usize, usize)> {
        let (a, b) = (
            self.anchor?.min(self.draft_len()),
            self.caret.min(self.draft_len()),
        );
        (a != b).then_some((a.min(b), a.max(b)))
    }

    /// The selected words themselves.
    fn selection(&self) -> Option<String> {
        let (from, to) = self.selected()?;
        Some(self.draft.chars().skip(from).take(to - from).collect())
    }

    /// Take what is selected out of the text; the cursor is where it was.
    /// Whether there was anything.
    fn cut(&mut self) -> bool {
        let Some((from, to)) = self.selected() else {
            self.anchor = None;
            return false;
        };
        let (a, b) = (self.draft_byte(from), self.draft_byte(to));
        self.draft.replace_range(a..b, "");
        self.caret = from;
        self.anchor = None;
        true
    }

    /// Write `text` in at the cursor (over what is selected, if anything is).
    fn type_in(&mut self, text: &str) {
        self.cut();
        self.caret = self.caret.min(self.draft_len());
        let at = self.draft_byte(self.caret);
        self.draft.insert_str(at, text);
        self.caret += text.chars().count();
    }

    /// Rub out the character before the cursor, or (`ahead`) the one after.
    fn rub(&mut self, ahead: bool) {
        // (With words selected, either key rubs those out.)
        if self.cut() {
            return;
        }
        self.caret = self.caret.min(self.draft_len());
        if !ahead {
            if self.caret == 0 {
                return;
            }
            self.caret -= 1;
        }
        if self.caret < self.draft_len() {
            let at = self.draft_byte(self.caret);
            self.draft.remove(at);
        }
    }

    /// The whole draft, taken (the box is empty again).
    fn take_draft(&mut self) -> String {
        self.caret = 0;
        self.anchor = None;
        self.draft_first = None;
        self.draft_spans.clear();
        std::mem::take(&mut self.draft)
    }

    /// The lines of the draft as the box breaks them (at least one).
    fn spans(&self) -> Vec<(usize, usize)> {
        match self.draft_spans.is_empty() {
            true => vec![(0, self.draft_len())],
            false => self.draft_spans.clone(),
        }
    }

    /// The first line of the input box that is in view: where the wheel
    /// left it, or wherever keeps the cursor in sight.
    fn draft_top(&self) -> usize {
        let lines = self.spans().len();
        let most = lines.saturating_sub(BOX_LINES);
        match self.draft_first {
            Some(first) => first.min(most),
            None => first_line(lines, Some(self.caret_line())),
        }
    }

    /// Which of those lines the cursor is on: the one it is inside — and
    /// at a line's very end, that line, unless the next one starts right
    /// there (a long line broken in two: the cursor is at its second part).
    fn caret_line(&self) -> usize {
        let spans = self.spans();
        let at = self.caret;
        spans
            .iter()
            .enumerate()
            .position(|(n, (start, end))| {
                at >= *start
                    && (at < *end
                        || (at == *end && spans.get(n + 1).is_none_or(|next| next.0 > *end)))
            })
            .unwrap_or(spans.len() - 1)
    }

    /// Move the cursor a line up (`-1`) or down (`1`), keeping how far
    /// along the line it is; to the line's start or end with `edge`.
    fn caret_line_move(&mut self, by: i32, edge: Option<bool>) {
        let spans = self.spans();
        let line = self.caret_line();
        let (start, end) = spans[line];
        match edge {
            Some(false) => self.caret = start,
            Some(true) => self.caret = end,
            None => {
                let to = line as i32 + by;
                if to < 0 || to as usize >= spans.len() {
                    return;
                }
                let (s, e) = spans[to as usize];
                self.caret = (s + (self.caret - start)).min(e);
            }
        }
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
        if !self.rect?.contains(pos) {
            return None;
        }
        // (Everything is tested where it was laid out: see `view::virt`.)
        let rect = self.vrect()?;
        let pos = self.inside(pos);
        if let Some(menu) = self.menu_rect().filter(|r| r.contains(pos)) {
            let row = ((pos.1 - menu.y - MENU_PAD) / MENU_ROW).floor().max(0.0) as usize;
            return Some(Hover::Menu(row.min(self.menu_rows().len() - 1)));
        }
        let low = bottom(rect, self.page, self.seeking(), self.pick())
            .plain_if(self.titling())
            .with_input(self.writer);
        if self.undo.is_some() && self.flash.is_some() && low.hint_rect(rect).contains(pos) {
            return Some(Hover::Undo);
        }
        if let Some(hit) = low.hit(pos, self.seeking()) {
            return Some(hit);
        }
        let list = list_rect(rect, Some(self.page), low.h);
        if pos.1 < list.y {
            return foot_buttons(rect)
                .into_iter()
                .find(|(_, r)| r.contains(pos))
                .map(|(f, _)| Hover::Foot(f));
        }
        if pos.1 >= list.y + list.h {
            return None;
        }
        if self.page == Page::Talk && talk_mic(list).contains(pos) {
            return Some(Hover::Mic);
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
        // (Memory's rows are sessions: no disk; and while one is being
        // chosen, no × either — a row only chooses.)
        let pins = !self.is_row(tile.id);
        // (A session's row has a pencil too, in the disk's place: its name.)
        if self.is_row(tile.id)
            && tile.id != PINNED_ROW
            && self.picking.is_none()
            && tile.pin().contains(pos)
        {
            return Some(Hover::Edit(tile.id));
        }
        let pencil = matches!(
            self.page,
            Page::Session | Page::Pinned | Page::Dictionary | Page::Translate
        ) && self.item(tile.id).is_some();
        // (Every list's items have a ×; a session's row has none while a
        // session is being chosen.)
        let closes =
            !(self.page == Page::Memory && self.picking.is_some()) && tile.id != PINNED_ROW;
        Some(if closes && tile.close().contains(pos) {
            Hover::Close(tile.id)
        } else if pencil && tile.edit(true).contains(pos) {
            Hover::Edit(tile.id)
        } else if pins && tile.pin().contains(pos) {
            Hover::Pin(tile.id)
        } else if self.lang_strip(tile.id).is_some_and(|r| r.contains(pos)) {
            Hover::Lang(tile.id)
        } else if tile.line().contains(pos)
            && self.item(tile.id).is_some_and(|it| it.kind == Kind::Voice)
        {
            Hover::Line(tile.id)
        } else if tile.speed().contains(pos)
            && self.item(tile.id).is_some_and(|it| it.kind == Kind::Voice)
        {
            Hover::Speed(tile.id)
        } else if self.item(tile.id).is_some_and(|it| {
            // (A voice note's "Transcribe"; a long text's "Show all".)
            if it.kind == Kind::Voice {
                return tile.voice_more().contains(pos);
            }
            tile.more().contains(pos)
                && it.kind == Kind::Text
                && it.parts.is_empty()
                && has_more(
                    self.lines.get(&it.id).map_or(&[][..], Vec::as_slice),
                    self.wide.contains(&it.id),
                )
        }) {
            Hover::More(tile.id)
        } else if tile.play().contains(pos)
            && self.item(tile.id).is_some_and(|it| it.kind == Kind::Voice)
        {
            Hover::Play(tile.id)
        } else if let Some((piece, r, voice)) = self
            .item(tile.id)
            .filter(|it| !it.parts.is_empty() && self.wide.contains(&it.id))
            .and_then(|it| {
                // An OPEN item: the piece under the pointer is what is
                // played, pasted or carried off — not the whole item.
                piece_rects(
                    &it.parts,
                    &self.lines,
                    1.0,
                    true,
                    (tile.rect.x + TILE_PAD_X, tile.rect.y + TILE_PAD_Y),
                    tile.rect.w - 2.0 * TILE_PAD_X,
                )
                .into_iter()
                .find(|(_, r, _)| r.contains(pos))
            })
        {
            // (A voice note: its play button, at the left of its row.)
            if voice && pos.0 < r.x + 30.0 && pos.1 < r.y + 30.0 {
                Hover::Play(piece)
            } else {
                Hover::Item(piece)
            }
        } else {
            Hover::Item(tile.id)
        })
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

    /// What is being written is a voice note's TITLE (its pencil, or just
    /// recorded): words alone — the box has no + then.
    fn titling(&self) -> bool {
        self.field == Some(Field::Box)
            && self
                .edit
                .and_then(|id| self.item(id))
                .is_some_and(|it| it.kind == Kind::Voice)
    }

    /// Where the line of languages of translation `id` is on the card: the
    /// rule over its last part (the translator's page only).
    fn lang_strip(&self, id: u64) -> Option<Rect> {
        if self.page != Page::Translate {
            return None;
        }
        let tile = self.tiles.iter().find(|t| t.id == id)?;
        let item = self.items.iter().find(|it| it.id == id)?;
        let answer = item.parts.last().filter(|p| p.from.is_some())?.id;
        piece_rects(
            &item.parts,
            &self.lines,
            1.0,
            self.wide.contains(&id),
            (tile.rect.x + TILE_PAD_X, tile.rect.y + TILE_PAD_Y),
            tile.rect.w - 2.0 * TILE_PAD_X,
        )
        .into_iter()
        .find(|(pid, ..)| *pid == answer)
        .map(|(_, r, _)| Rect::new(r.x, r.y, r.w, ANSWER_HEAD))
    }

    /// Whether item `id` of Memory's page is a ROW (a session, "Pinned") and
    /// not a thing a search found.
    fn is_row(&self, id: u64) -> bool {
        self.page == Page::Memory && (self.finds.is_empty() || self.find_rows.contains(&id))
    }

    /// MEMORY'S PAGE, SEARCHED (Max, 2026-10-10: *"that address i saved
    /// somewhere"*): the THINGS that match, each group under the row of
    /// where it is kept — every session, then Pinned. (A session whose
    /// name matches is listed too, with nothing under it.)
    fn search_all(&self) -> (Vec<Item>, HashSet<u64>) {
        let query = self.query.trim().to_lowercase();
        let has = |it: &Item| {
            it.body.to_lowercase().contains(&query)
                || it
                    .from
                    .as_ref()
                    .is_some_and(|f| f.to_lowercase().contains(&query))
                || (it.kind == Kind::Voice && "voice note".contains(&query))
        };
        let (mut out, mut rows) = (Vec::new(), HashSet::new());
        for past in &self.memory {
            let named = past
                .name
                .as_deref()
                .is_some_and(|n| n.to_lowercase().contains(&query));
            let hits: Vec<Item> = past.items.iter().filter(|it| has(it)).cloned().collect();
            if named || !hits.is_empty() {
                rows.insert(past.id);
                out.push(session_row(past));
                out.extend(hits);
            }
        }
        let pins: Vec<Item> = self.pinned.iter().filter(|it| has(it)).cloned().collect();
        if !pins.is_empty() {
            rows.insert(PINNED_ROW);
            out.push(Item {
                id: PINNED_ROW,
                kind: Kind::Text,
                body: "Pinned".to_owned(),
                path: None,
                aspect: 0.0,
                owned: false,
                at: pins.iter().map(|p| p.at).max().unwrap_or(0),
                from: None,
                parts: Vec::new(),
                said: None,
            });
            out.extend(pins);
        }
        (out, rows)
    }

    /// The rows of the disk's menu on the page that is up: Pin (not for
    /// what is pinned already), Memory, New session.
    fn menu_rows(&self) -> &'static [&'static str] {
        // (The +'s, in the writing box: what can be put in.)
        if self.plus {
            return &["Image", "File", "Voice note", "Voice to text"];
        }
        // (A translation's languages: what it goes into, what it is in.)
        if self.lang.is_some() {
            return &LANG_ROWS;
        }
        match self.page {
            Page::Pinned => &["Memory", "New session"],
            _ => &["Pin", "Memory", "New session"],
        }
    }

    /// Where the disk's menu is: under the disk of its item, its right edge
    /// on the disk's (over it where there is no room under).
    fn menu_rect(&self) -> Option<Rect> {
        let card = self.vrect()?;
        // (Under the + of the writing box, or under an item's disk.)
        let disk = if self.plus {
            bottom(card, self.page, self.seeking(), self.pick())
                .plain_if(self.titling())
                .with_input(self.writer)
                .btns[0]
                .1
        } else if let Some(strip) = self.lang.and_then(|id| self.lang_strip(id)) {
            // (Under the middle of the line of languages.)
            Rect::new(
                strip.x + strip.w / 2.0 - 10.0,
                strip.y,
                MENU_W / 2.0 + 10.0,
                strip.h,
            )
        } else {
            self.tiles.iter().find(|t| Some(t.id) == self.menu)?.pin()
        };
        if disk.w == 0.0 {
            return None;
        }
        let h = self.menu_rows().len() as f32 * MENU_ROW + 2.0 * MENU_PAD;
        let x = (disk.x + disk.w - MENU_W).max(card.x + 6.0);
        let under = disk.y + disk.h + 4.0;
        let y = if under + h > card.y + card.h - 26.0 {
            disk.y - 4.0 - h
        } else {
            under
        };
        // (A long menu — the languages — stays inside the card all the same.)
        let y = y.min(card.y + card.h - 8.0 - h).max(card.y + 8.0);
        Some(Rect::new(x.round(), y.round(), MENU_W, h))
    }

    /// A copy of `item` of its own (a new id, made now): what is kept
    /// somewhere else is never tied to where it came from.
    fn copy_of(&mut self, item: &Item, now: u64) -> Item {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        let mut copy = Item {
            id,
            at: now,
            ..item.clone()
        };
        for part in &mut copy.parts {
            part.id = self.next_id;
            self.next_id += 1;
        }
        copy
    }

    /// Pin a copy of `item`.
    fn pin_copy(&mut self, item: &Item, now: u64) {
        let copy = self.copy_of(item, now);
        // (On Pinned's own page the pins are the list on screen.)
        if self.page == Page::Pinned {
            self.items.push(copy);
        } else {
            self.pinned.push(copy);
        }
    }

    /// Keep a copy of `item` in session `id`. Whether there is such a one.
    fn keep_in(&mut self, id: u64, item: &Item, now: u64) -> bool {
        if !self.memory.iter().any(|p| p.id == id) {
            return false;
        }
        let copy = self.copy_of(item, now);
        // (The open session's items are out on the card while its page is up.)
        if self.page == Page::Session && self.open == Some(id) {
            self.items.push(copy);
        } else if let Some(past) = self.memory.iter_mut().find(|p| p.id == id) {
            past.items.push(copy);
        }
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
            Page::Dictionary => self.looked = shown,
            Page::Translate => self.translated = shown,
            Page::Talk => self.said_last = shown.into_iter().next(),
            Page::Memory | Page::Clipboard | Page::Emoji => {}
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
            Page::Talk => self.said_last.take().into_iter().collect(),
            Page::Dictionary => std::mem::take(&mut self.looked),
            Page::Translate => std::mem::take(&mut self.translated),
            Page::Clipboard | Page::Emoji => Vec::new(),
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
        self.prune();
        true
    }

    /// EMPTY SESSIONS DISAPPEAR (Max, 2026-10-10): a session with nothing
    /// in it is in memory only while it is the one open on screen. Left
    /// empty, it is gone — its windows and its title have nothing then, as
    /// when it is forgotten.
    fn prune(&mut self) {
        let showing = (self.page == Page::Session).then_some(self.open).flatten();
        let empty: Vec<u64> = self
            .memory
            .iter()
            .filter(|p| p.items.is_empty() && Some(p.id) != showing)
            .map(|p| p.id)
            .collect();
        for id in empty {
            self.lose(id);
        }
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
        self.prune();
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

    /// Whether anything the card keeps — on it, pinned, or in memory — is
    /// the file at `path` (a picture of the card's own is deleted only
    /// with the LAST item that shows it).
    fn keeps(&self, path: &str) -> bool {
        // (An item's own file, or one of its parts'.)
        let is = |it: &Item| {
            it.path.as_deref() == Some(path)
                || it.parts.iter().any(|p| p.path.as_deref() == Some(path))
        };
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
    // Its ground is see-through (`view::AIR`): what shows through it is
    // frosted, as behind the dock — and only where the card is (the rest
    // of its surface is clear, under the threshold).
    crate::hypr::eval(
        "hl.layer_rule({ name = \"golem-card-blur\", match = { namespace = \"waverunner-card\" }, blur = true })",
    );
    crate::hypr::eval(
        "hl.layer_rule({ name = \"golem-card-blur-edge\", match = { namespace = \"waverunner-card\" }, ignore_alpha = 0.5 })",
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
            .chain(&saved.looked)
            .chain(&saved.translated)
            .chain(saved.memory.iter().flat_map(|p| &p.items))
            .map(|it| it.id)
            .chain(saved.memory.iter().map(|p| p.id));
        self.card.next_id = saved.next_id.max(ids.max().map_or(1, |id| id + 1));
        self.card.page = Page::Session;
        self.card.open = None;
        self.card.items = Vec::new();
        self.card.pinned = saved.pinned;
        self.card.looked = saved.looked;
        self.card.translated = saved.translated;
        self.card.memory = saved.memory;
        self.card.titles = saved.titles;
        self.card.zoom = saved.zoom;
        self.card.recent = saved
            .emoji
            .iter()
            .filter_map(|ch| crate::emoji_table::EMOJI.iter().position(|d| d.ch == ch))
            .collect();
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
                looked: if self.card.page == Page::Dictionary {
                    self.card.items.clone()
                } else {
                    self.card.looked.clone()
                },
                translated: if self.card.page == Page::Translate {
                    self.card.items.clone()
                } else {
                    self.card.translated.clone()
                },
                memory: self.card.sessions(),
                titles: self.card.titles.clone(),
                zoom: self.card.zoom,
                emoji: self
                    .card
                    .recent
                    .iter()
                    .filter_map(|n| crate::emoji_table::EMOJI.get(*n).map(|d| d.ch.to_owned()))
                    .collect(),
            },
        );
    }

    fn card_pictures(&self) -> Vec<String> {
        self.card
            .items
            .iter()
            .flat_map(|it| std::iter::once(it).chain(it.parts.iter()))
            .chain(self.card.sealed.iter())
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
        // (An item's own picture, a part's, or one in what is being written.)
        if !self.card_pictures().iter().any(|p| p == path) {
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
        // While something is being WRITTEN, what arrives is a part of it: a
        // file dropped on the card, a voice note recorded from the +.
        if self.card.field == Some(Field::Box) && !self.card.keeping {
            let part = self.card_part(kind, body, path, owned, aspect);
            self.card_seal(part);
            return;
        }
        // What arrives on a page that DOES something with it (Max,
        // 2026-10-10: drag and drop from everywhere to everywhere): words
        // on the translator's are translated, on the dictionary's looked
        // up; anything let go on a session's row is kept in that session.
        match self.card.page {
            Page::Translate if kind == Kind::Text => {
                self.card_keep_translation(&body, None);
                return;
            }
            Page::Dictionary if kind == Kind::Text => {
                self.card_keep_word(&body, None);
                return;
            }
            // (The clipboard's page: it is COPIED — on the clipboard, and
            // so in its history, as anything copied is.)
            Page::Clipboard => {
                match path {
                    Some(path) => self.serve_files(std::slice::from_ref(&path), false),
                    None => self.copy_text(&body),
                }
                self.card_flash("Copied");
                return;
            }
            Page::Memory => {
                if let Some(session) = self.card.drop_session {
                    let item = self.card_part(kind, body, path, owned, aspect);
                    if self.card.keep_in(session, &item, now_secs()) {
                        self.card.items = self.card.memory.iter().map(session_row).collect();
                        self.card.lines.remove(&session);
                        self.card_flash("Saved");
                        self.card_save();
                    }
                    return;
                }
            }
            _ => {}
        }
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
                parts: Vec::new(),
                said: None,
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
        if id == PINNED_ROW {
            self.card_show(Page::Pinned);
            return;
        }
        if self.card.is_row(id) {
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
        // (…and they stay its: Enter after a paste must run it there, not
        // make an item here — until the pointer leaves and comes back.)
        self.card.tabbed = true;
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
        self.card.menu = None;
        self.card.plus = false;
        self.card.lang = None;
        // (Choosing a session is Memory's page's: leaving it gives it up.)
        if page != Page::Memory {
            self.card.picking = None;
        }
        if self.card.show(page) {
            // (The input box and the name are a memory's.)
            // (What was being written was the other page's.)
            if matches!(self.card.field, Some(Field::Box | Field::Name)) {
                self.card.take_draft();
                self.card_unseal();
                self.card.edit = None;
                self.card_rest();
            }
            if page == Page::Emoji {
                self.card.pick_query.clear();
                self.card.repick();
            }
            // (The translator's page: a thing that could not be translated —
            // no engine then, or not that pair — is tried again now.)
            if page == Page::Translate {
                let again: Vec<(u64, String)> = self
                    .card
                    .items
                    .iter()
                    .filter(|it| {
                        it.parts
                            .last()
                            .is_some_and(|p| p.from.is_some() && p.body.starts_with('('))
                    })
                    .map(|it| (it.id, it.body.clone()))
                    .collect();
                for (id, words) in again {
                    self.card_keep_translation(&words, Some(id));
                }
            }
            // (The dictionary's page: a word left waiting for the dictionary
            // — the dock was closed meanwhile — is looked up now.)
            if page == Page::Dictionary {
                let waiting: Vec<u64> = self
                    .card
                    .items
                    .iter()
                    .filter(|it| {
                        it.parts
                            .iter()
                            .any(|p| p.from.as_deref() == Some("Dictionary"))
                    })
                    .map(|it| it.id)
                    .collect();
                if !waiting.is_empty() {
                    self.card.looking = waiting;
                    self.card_dict_loaded();
                }
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
                    parts: Vec::new(),
                    said: None,
                })
            })
            .collect()
    }

    /// THE TRANSLATOR works as Pinned does, with the translation under a
    /// rule (Max, 2026-10-10). What is written is kept as an item of two
    /// parts: the words, and their translation — an ANSWER whose name is
    /// the two languages ("Spanish → English"). The language is told from
    /// the words themselves (`language_of`); anything but English goes
    /// into English, English into Spanish. The translating is the work of
    /// an engine on this computer (`translate`: translateLocally, offline)
    /// on a thread; where there is none, or it has not that pair of
    /// languages, the answer says so.
    /// A row of a translation's menu of languages: "To …" is the language
    /// it goes into, "It is …" the one it is in (where the guess was wrong).
    /// It is translated again.
    fn card_lang(&mut self, id: u64, row: usize) {
        let Some(item) = self.card.items.iter().find(|it| it.id == id) else {
            return;
        };
        let words = item.body.clone();
        let (mut from, mut to) = pair_of(item);
        let code = LANG_CODES[row % LANG_CODES.len()];
        if row < LANG_CODES.len() {
            to = code;
        } else {
            from = code;
        }
        // (Into itself is nothing: the other way round, then.)
        if from == to {
            if row < LANG_CODES.len() {
                from = if to == "en" { "es" } else { "en" };
            } else {
                to = if from == "en" { "es" } else { "en" };
            }
        }
        self.card_translate_as(&words, Some(id), Some((from, to)));
    }

    fn card_keep_translation(&mut self, text: &str, edit: Option<u64>) {
        // (One translated before keeps the languages it was given.)
        let pair = edit
            .and_then(|id| self.card.items.iter().find(|it| it.id == id))
            .filter(|it| it.from.is_some())
            .map(pair_of);
        self.card_translate_as(text, edit, pair);
    }

    fn card_translate_as(
        &mut self,
        text: &str,
        edit: Option<u64>,
        pair: Option<(&'static str, &'static str)>,
    ) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let chosen = pair.is_some();
        let (from, to) = pair.unwrap_or_else(|| {
            let from = language_of(text);
            (from, if from == "en" { "es" } else { "en" })
        });
        let mut words = self.card_part(Kind::Text, text.to_owned(), None, false, 0.0);
        words.from = None;
        let mut answer = self.card_part(Kind::Text, "Translating…".to_owned(), None, false, 0.0);
        answer.from = Some(format!("{} → {}", language_name(from), language_name(to)));
        let answer_id = answer.id;
        let parts = vec![words, answer];
        // (Languages picked by hand are the item's own from then on: its
        // `from` keeps them, "es>en".)
        let kept = chosen.then(|| format!("{from}>{to}"));
        let id = match edit.and_then(|id| self.card.items.iter_mut().find(|it| it.id == id)) {
            Some(item) => {
                item.body = text.to_owned();
                item.parts = parts;
                if kept.is_some() {
                    item.from = kept;
                }
                item.id
            }
            None => {
                let mut item = self.card_part(Kind::Text, text.to_owned(), None, false, 0.0);
                item.parts = parts;
                item.from = kept;
                let id = item.id;
                self.card.items.push(item);
                self.card.to_bottom = true;
                id
            }
        };
        self.card.wide.insert(id);
        self.card.lines.remove(&id);
        let (tx, rx) = calloop::channel::channel::<String>();
        let (text, from, to) = (text.to_owned(), from.to_owned(), to.to_owned());
        std::thread::spawn(move || {
            let _ = tx.send(translate(&text, &from, &to));
        });
        let waiting = self
            .loop_handle
            .insert_source(rx, move |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(said) = event {
                    // (Wherever it is by now: on the card, or put away.)
                    let part = app
                        .card
                        .items
                        .iter_mut()
                        .chain(app.card.translated.iter_mut())
                        .flat_map(|it| it.parts.iter_mut())
                        .find(|p| p.id == answer_id);
                    if let Some(part) = part {
                        part.body = said;
                    }
                    app.card.lines.remove(&answer_id);
                    app.card_save();
                    app.request_card_draw();
                }
            });
        if waiting.is_err() {
            warn!("card: cannot wait for a translation");
        }
        self.card_save();
        self.request_card_draw();
    }

    /// THE DICTIONARY works as Pinned does (Max, 2026-10-10: *"the
    /// dictionary: same flow"*): a word is written in the box and kept — as
    /// an item whose first part is the word and whose other parts are its
    /// entries, one a language, each under a rule with that language on it.
    /// The entries of `word`, as parts.
    fn card_look_up(&mut self, word: &str) -> Vec<Item> {
        let word = word.trim();
        let mut parts = vec![self.card_part(Kind::Text, word.to_owned(), None, false, 0.0)];
        let mut answer = |app: &mut Self, from: &str, body: String| {
            let mut part = app.card_part(Kind::Text, body, None, false, 0.0);
            part.from = Some(from.to_owned());
            parts.push(part);
        };
        if word.split_whitespace().count() > 1 {
            answer(
                self,
                "One word",
                "The dictionary can define one word at a time.".to_owned(),
            );
            return parts;
        }
        match self.dict_define(word) {
            // (Still being opened: `card_dict_loaded` looks it up again.)
            None => answer(self, "Dictionary", "Opening the dictionary…".to_owned()),
            Some(found) if found.is_empty() => {
                answer(self, "Not found", format!("No entry for “{word}”."))
            }
            Some(found) => {
                for (lang, definition, origin) in found {
                    let body = match origin {
                        Some(origin) => format!("{origin}\n{definition}"),
                        None => definition,
                    };
                    answer(self, &lang, body);
                }
            }
        }
        parts
    }

    /// Enter in the box on the dictionary's page: `word` is looked up and
    /// kept (or, with its pencil, the item `edit` is looked up anew).
    fn card_keep_word(&mut self, word: &str, edit: Option<u64>) {
        let word = word.trim();
        if word.is_empty() {
            return;
        }
        let parts = self.card_look_up(word);
        let waiting = parts
            .iter()
            .any(|p| p.from.as_deref() == Some("Dictionary"));
        let id = match edit.and_then(|id| self.card.items.iter_mut().find(|it| it.id == id)) {
            Some(item) => {
                item.body = word.to_owned();
                item.parts = parts;
                item.id
            }
            None => {
                let mut item = self.card_part(Kind::Text, word.to_owned(), None, false, 0.0);
                item.parts = parts;
                let id = item.id;
                self.card.items.push(item);
                self.card.to_bottom = true;
                id
            }
        };
        if waiting {
            self.card.looking.push(id);
        }
        // (Its answers are to be read: it opens at once.)
        self.card.wide.insert(id);
        self.card.lines.remove(&id);
        self.card_save();
        self.request_card_draw();
    }

    /// The dictionary is in memory now: its page, if it is up, answers.
    pub(crate) fn card_dict_loaded(&mut self) {
        // (The words that were waiting for it are looked up now.)
        for id in std::mem::take(&mut self.card.looking) {
            let word = self
                .card
                .items
                .iter()
                .chain(self.card.looked.iter())
                .find(|it| it.id == id)
                .map(|it| it.body.clone());
            let Some(word) = word else { continue };
            let parts = self.card_look_up(&word);
            // (Still not open: it goes on waiting.)
            if parts
                .iter()
                .any(|p| p.from.as_deref() == Some("Dictionary"))
            {
                self.card.looking.push(id);
            }
            let item = self
                .card
                .items
                .iter_mut()
                .chain(self.card.looked.iter_mut())
                .find(|it| it.id == id);
            if let Some(item) = item {
                item.parts = parts;
            }
            self.card.lines.remove(&id);
        }
        self.card_save();
        self.request_card_draw();
    }

    /// The clipboard's history changed: its page, if it is up, lists it anew.
    pub(crate) fn card_clip_changed(&mut self) {
        if self.card.page == Page::Clipboard && self.card.host.is_some() {
            self.card.items = self.card_clip_rows();
            self.request_card_draw();
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
            info!("card: has the keyboard ({field:?})");
        }
        self.card_keys_watch();
        self.sync_card_input();
        self.request_card_draw();
    }

    /// THE KEYS THAT FOLLOWED THE POINTER ONTO THE CARD go back when it
    /// leaves — and the compositor does not always say that it left (Max,
    /// 2026-10-12, once the card's top touched the title bar: *"the card
    /// won't give back the kb focus now"* — a pointer that goes from the
    /// card onto a title bar is on no surface, and nothing is sent). So
    /// while the card has them for that reason alone, it asks a few times
    /// a second where the pointer really is.
    fn card_keys_watch(&mut self) {
        if self.card.field != Some(Field::Card)
            || std::mem::replace(&mut self.card.keys_watch, true)
        {
            return;
        }
        self.after_ms(250, |app| {
            app.card.keys_watch = false;
            if app.card.field != Some(Field::Card) {
                return;
            }
            let off = crate::hypr::cursor_on_monitor()
                .zip(app.card.rect)
                .is_some_and(|((x, y), r)| {
                    x < r.x - 1.0 || x > r.x + r.w + 1.0 || y < r.y - 1.0 || y > r.y + r.h + 1.0
                });
            if off {
                info!("card: the pointer is off the card and nothing said so: the keys go back");
                app.card.ptr = None;
                app.card_keys(false);
                app.card_drop_keys();
            } else {
                app.card_keys_watch();
            }
        });
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
        // (Space was held for a voice note: it ends here, and is kept.)
        self.card.space = None;
        if std::mem::take(&mut self.card.held) {
            self.card_record_stop(true);
        }
        if self.card.field.take().is_some() {
            debug!("card: the keyboard left");
            if let Some(layer) = self.card_layer.as_ref() {
                crate::surface::set_interactive(layer, false);
            }
            let _ = self.conn.flush();
            self.sync_card_input();
            self.request_card_draw();
        }
        self.complete_keyboard_handback(crate::KbSurface::Card);
    }

    /// Give the keyboard back to the card's window.
    fn card_drop_keys(&mut self) {
        if self.card.field.take().is_none() {
            return;
        }
        info!("card: gives the keyboard back");
        self.begin_keyboard_handback(crate::KbSurface::Card, self.card.host.clone());
        if let Some(layer) = self.card_layer.as_ref() {
            crate::surface::set_interactive(layer, false);
        }
        let _ = self.conn.flush();
        self.sync_card_input();
        self.request_card_draw();
    }

    /// THE KEYS FOLLOW THE POINTER (Max, 2026-10-10): on the card, they are
    /// the card's — a letter searches, Enter makes something new — with no
    /// click; off it, the window's again. (Not after a Tab: see `tabbed`.)
    fn card_hover_keys(&mut self) {
        let on = self
            .card
            .ptr
            .zip(self.card.rect)
            .is_some_and(|(at, r)| r.contains(at));
        // ONLY WHILE ITS WINDOW IS THE FOCUSED ONE (Max, 2026-10-10: *"the card
        // should not get focus by mouse by itself, it should only work when
        // the window is focused"* — by a click on it, or just opened): the
        // pointer passing over the card of a window one is not working in
        // takes nothing.
        if self.card.active.is_none() {
            self.card.active = crate::hypr::active_window();
        }
        let focused = self.card.host.is_some() && self.card.active == self.card.host;
        if on && focused && self.card.field.is_none() && !self.card.tabbed && self.card_present() {
            self.card_take_keys(Field::Card);
        }
    }

    /// What was being written is done: the keys stay the card's while the
    /// pointer is on it, and go back to the window otherwise.
    fn card_rest(&mut self) {
        let on = self
            .card
            .ptr
            .zip(self.card.rect)
            .is_some_and(|(at, r)| r.contains(at));
        if on && !self.card.tabbed && self.card.field.is_some() {
            self.card.field = Some(Field::Card);
            self.card_keys_watch();
            self.sync_card_input();
            self.request_card_draw();
        } else {
            self.card_drop_keys();
        }
    }

    /// A key let go. Space, after a hold: the voice note ends there and is
    /// kept, in the list that is up.
    pub(crate) fn card_key_release(
        &mut self,
        keysym: smithay_client_toolkit::seat::keyboard::Keysym,
    ) {
        if keysym != smithay_client_toolkit::seat::keyboard::Keysym::space {
            return;
        }
        self.card.space = None;
        if std::mem::take(&mut self.card.held) {
            // (A voice note asks for its title; talk is just written.)
            self.card.name_next = self.card.rec.as_ref().is_some_and(|r| !r.talk());
            self.card_record_stop(true);
            self.request_card_draw();
        }
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
        if field == Field::Card {
            // Nothing is being written. TAB (or Escape) hands the keys to
            // the window with the pointer where it is; ENTER starts
            // something new; a letter starts a search.
            let letter = utf8
                .filter(|s| !s.is_empty() && !s.chars().any(|c| c.is_control() || c == ' '))
                .filter(|_| !ctrl);
            // SPACE HELD is a voice note, for as long as it is held (Max,
            // 2026-10-10: *"hold Space to talk"*); a tap of it is nothing —
            // a search does not start with a space.
            if keysym == Keysym::space && !ctrl {
                if self.card.space.is_none()
                    && matches!(self.card.page, Page::Session | Page::Pinned | Page::Talk)
                    && self.card.picking.is_none()
                {
                    let since = Instant::now();
                    self.card.space = Some(since);
                    // (On voice-to-text's page it is talk, written when let go.)
                    let talk = self.card.page == Page::Talk;
                    self.after_ms(HOLD_MS, move |app| {
                        if app.card.space == Some(since) && app.card.rec.is_none() {
                            app.card.held = true;
                            app.card_record_start(talk);
                            app.request_card_draw();
                        }
                    });
                }
                return;
            }
            match (keysym, self.card.page) {
                // A session being chosen: Escape goes back with nothing
                // kept; Enter keeps the thing in a new one.
                (Keysym::Escape, _) if self.card.picking.is_some() => {
                    self.card_picked(None);
                    return;
                }
                (Keysym::Return | Keysym::KP_Enter, _) if self.card.picking.is_some() => {
                    let title = self.card_host_title();
                    let new = self.card.begin(title, now_secs());
                    self.card_picked(Some(new));
                    return;
                }
                (Keysym::Tab | Keysym::Escape, _) => {
                    self.card.tabbed = true;
                    self.card_drop_keys();
                    return;
                }
                (Keysym::Return | Keysym::KP_Enter, Page::Session) if self.card.open.is_some() => {
                    self.card_write_new("");
                }
                (
                    Keysym::Return | Keysym::KP_Enter,
                    Page::Pinned | Page::Dictionary | Page::Translate,
                ) => self.card_write_new(""),
                // (A new session starts EMPTY — the frame that says "Drag
                // here to keep", not a box to write in: Max, 2026-10-11.
                // Enter there opens the box.)
                (Keysym::Return | Keysym::KP_Enter, Page::Memory) => self.card_begin(),
                _ => match letter {
                    // (An empty list: there is nothing to search — the hint
                    // says so for a moment and is back. At rest it still
                    // says "type to search": that is there to teach.)
                    Some(_)
                        if self.card.items.is_empty()
                            && matches!(
                                self.card.page,
                                Page::Session | Page::Pinned | Page::Clipboard | Page::Memory
                            ) =>
                    {
                        self.card_flash("Nothing to search");
                        return;
                    }
                    Some(s) if self.card.page != Page::Emoji => {
                        self.card.field = Some(Field::Seek);
                        self.card.query.push_str(s);
                        self.card.scroll = 0.0;
                        self.card.scroll_to = None;
                    }
                    Some(s) => {
                        self.card.field = Some(Field::Pick);
                        self.card.pick_query.push_str(s);
                        self.card.repick();
                    }
                    None => return,
                },
            }
            self.sync_card_input();
            self.request_card_draw();
            return;
        }
        if field == Field::Box {
            // (Typing or moving the cursor: the box shows where it is again.)
            self.card.draft_first = None;
            // A move of the cursor with Shift held selects from where it
            // was; without, whatever was selected is let go.
            let moves = matches!(
                keysym,
                Keysym::Left
                    | Keysym::Right
                    | Keysym::Up
                    | Keysym::Down
                    | Keysym::Home
                    | Keysym::End
                    | Keysym::KP_Left
                    | Keysym::KP_Right
                    | Keysym::KP_Up
                    | Keysym::KP_Down
                    | Keysym::KP_Home
                    | Keysym::KP_End
            );
            if moves && shift {
                let at = self.card.caret;
                self.card.anchor.get_or_insert(at);
            } else if moves {
                self.card.anchor = None;
            }
        }
        let typed = utf8
            .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
            .filter(|_| !ctrl);
        match (field, keysym) {
            (_, Keysym::Escape) => {
                // One layer at a time: the search's words, then the cursor.
                match field {
                    Field::Seek if !self.card.query.is_empty() => self.card.query.clear(),
                    // (Escape drops what was being written.)
                    Field::Box => {
                        // (…a recording into it too.)
                        if self.card.rec.is_some() {
                            self.card_record_stop(false);
                        }
                        self.card.take_draft();
                        self.card_unseal();
                        self.card.edit = None;
                        self.card_rest();
                    }
                    Field::Name => {
                        self.card.renaming = None;
                        self.card_rest();
                    }
                    _ => self.card_rest(),
                }
            }
            // The input box: Enter keeps it in the memory, Ctrl+Enter sends
            // it to the window, Shift+Enter breaks the line.
            (Field::Box, Keysym::Return | Keysym::KP_Enter) if shift => self.card.type_in("\n"),
            (Field::Box, Keysym::Return | Keysym::KP_Enter) if ctrl => self.card_send_draft(),
            // (Kept, and the box is gone again: the hint is back.)
            // (While something is recorded into the box, Enter ends that:
            // the note becomes a part, what was said is written in.)
            (Field::Box, Keysym::Return | Keysym::KP_Enter) if self.card.rec.is_some() => {
                self.card_record_stop(true);
            }
            (Field::Box, Keysym::Return | Keysym::KP_Enter) => {
                self.card_keep_draft();
                self.card_rest();
            }
            // Select all, copy, cut — as any text field (Max, 2026-10-09:
            // *"so i can select all and erase it"*).
            (Field::Box, Keysym::a | Keysym::A) if ctrl => {
                self.card.anchor = Some(0);
                self.card.caret = self.card.draft_len();
            }
            (Field::Box, Keysym::c | Keysym::C | Keysym::x | Keysym::X) if ctrl => {
                if let Some(text) = self.card.selection() {
                    self.serve_transient_text(&text);
                    if matches!(keysym, Keysym::x | Keysym::X) {
                        self.card.cut();
                    }
                }
            }
            (Field::Box, Keysym::v | Keysym::V) if ctrl => {
                // Paste, as any text field does.
                if let Some(clip) = self.clip.history.first() {
                    if clip.kind == crate::clipboard::ClipKind::Text {
                        let text = clip.text.clone();
                        self.card.type_in(&text);
                    }
                }
            }
            // Backspace with nothing written: the last part put in goes
            // back — words into the line to be written again, anything
            // else out of the box.
            (Field::Box, Keysym::BackSpace)
                if self.card.draft.is_empty() && !self.card.sealed.is_empty() =>
            {
                if let Some(part) = self.card.sealed.pop() {
                    if part.kind == Kind::Text {
                        self.card.type_in(&part.body);
                    } else if let (true, Some(path)) = (part.owned, part.path.as_deref()) {
                        if !self.card.keeps(path) {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
            }
            (Field::Box, Keysym::BackSpace) => self.card.rub(false),
            (Field::Box, Keysym::Delete | Keysym::KP_Delete) => self.card.rub(true),
            // The writing cursor: along the text, to a line's ends, up and
            // down its lines (Max, 2026-10-09: *"i cant use the arrows to
            // jump on the text"*).
            (Field::Box, Keysym::Left | Keysym::KP_Left) => {
                self.card.caret = self.card.caret.saturating_sub(1);
            }
            (Field::Box, Keysym::Right | Keysym::KP_Right) => {
                self.card.caret = (self.card.caret + 1).min(self.card.draft_len());
            }
            (Field::Box, Keysym::Home | Keysym::KP_Home) => {
                self.card.caret_line_move(0, Some(false))
            }
            (Field::Box, Keysym::End | Keysym::KP_End) => self.card.caret_line_move(0, Some(true)),
            (Field::Box, Keysym::Up | Keysym::KP_Up) => self.card.caret_line_move(-1, None),
            (Field::Box, Keysym::Down | Keysym::KP_Down) => self.card.caret_line_move(1, None),
            (Field::Seek, Keysym::BackSpace) => {
                self.card.query.pop();
                // (Nothing left of it: the hint is back in its place.)
                if self.card.query.is_empty() {
                    self.card_rest();
                }
            }
            (Field::Pick, Keysym::BackSpace) => {
                self.card.pick_query.pop();
                self.card.repick();
            }
            (Field::Pick, Keysym::Return | Keysym::KP_Enter) => {}
            // Enter in the search of an open session: what was typed
            // becomes a new thing, to be finished in the box.
            (Field::Seek, Keysym::Return | Keysym::KP_Enter)
                if matches!(
                    self.card.page,
                    Page::Pinned | Page::Dictionary | Page::Translate
                ) || (self.card.page == Page::Session && self.card.open.is_some()) =>
            {
                let words = std::mem::take(&mut self.card.query);
                self.card_write_new(&words);
            }
            (Field::Seek, Keysym::Return | Keysym::KP_Enter) => {}
            (Field::Name, Keysym::BackSpace) => {
                self.card.naming.pop();
            }
            (Field::Name, Keysym::Return | Keysym::KP_Enter) => {
                let name = self.card.naming.trim().to_owned();
                self.card_rename(&name);
                self.card_rest();
            }
            _ => match (field, typed) {
                (Field::Box, Some(s)) => self.card.type_in(s),
                (Field::Seek, Some(s)) => self.card.query.push_str(s),
                (Field::Name, Some(s)) => self.card.naming.push_str(s),
                (Field::Pick, Some(s)) => {
                    self.card.pick_query.push_str(s);
                    self.card.repick();
                }
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
        // (The session being renamed in its row, else the open one.)
        let Some(which) = self.card.renaming.take().or(self.card.open) else {
            return;
        };
        if name.is_empty() {
            return;
        }
        if let Some(past) = self.card.memory.iter_mut().find(|p| p.id == which) {
            past.name = Some(name.to_owned());
            info!("card: a session renamed {name:?}");
            // (Memory's rows are made from the sessions: this one again.)
            if self.card.page == Page::Memory {
                self.card.items = self.card.memory.iter().map(session_row).collect();
                self.card.lines.remove(&which);
            }
            self.card_save();
            self.card_flash("Renamed");
        }
    }

    /// Enter (on a session, on Pinned): a NEW thing is written, in the list,
    /// after the last item — starting with `words` (what the search held).
    fn card_write_new(&mut self, words: &str) {
        self.card.take_draft();
        self.card_unseal();
        self.card.edit = None;
        self.card.field = Some(Field::Box);
        self.card.type_in(words);
        self.card.to_bottom = true;
        self.sync_card_input();
        self.request_card_draw();
    }

    /// An item's pencil: its words are in the writing box, in its own place
    /// in the list, to be changed. Enter keeps the changes, Escape leaves it
    /// as it was.
    fn card_edit(&mut self, id: u64) {
        // (A session's row: it is its NAME that is written again, there.)
        if self.card.page == Page::Memory {
            let Some(name) = self
                .card
                .memory
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone().unwrap_or_default())
            else {
                return;
            };
            if self.card.field.is_none() {
                self.card_take_keys(Field::Name);
            } else {
                self.card.field = Some(Field::Name);
            }
            self.card.renaming = Some(id);
            self.card.naming = name;
            self.sync_card_input();
            self.request_card_draw();
            return;
        }
        let Some(item) = self.card.item(id).cloned() else {
            return;
        };
        self.card.take_draft();
        self.card_unseal();
        // (A word of the dictionary, a thing translated: what was written
        // is written again.)
        if matches!(self.card.page, Page::Dictionary | Page::Translate) {
            self.card.type_in(&item.body);
            self.card.edit = Some(id);
            if self.card.field.is_none() {
                self.card_take_keys(Field::Box);
            } else {
                self.card.field = Some(Field::Box);
            }
            self.sync_card_input();
            self.request_card_draw();
            return;
        }
        // (A voice note: it is its TITLE that is written — none yet, an
        // empty line.)
        if item.kind == Kind::Voice {
            if !item.body.starts_with(VOICE_NAME) {
                self.card.type_in(&item.body);
            }
            self.card.edit = Some(id);
            if self.card.field.is_none() {
                self.card_take_keys(Field::Box);
            } else {
                self.card.field = Some(Field::Box);
            }
            self.sync_card_input();
            self.request_card_draw();
            return;
        }
        // (Made of parts: they stand in the box as they were put in, and
        // its last words — if it ends in words — are the line being written.)
        let mut parts = item.parts.clone();
        match parts.last() {
            Some(last) if last.kind == Kind::Text => {
                let words = parts.pop().map(|p| p.body).unwrap_or_default();
                self.card.type_in(&words);
            }
            Some(_) => {}
            // (A file, a folder, a picture of its own: it is the first part,
            // and words can be written round it — Max, 2026-10-10.)
            None if item.kind != Kind::Text => parts.push(item.clone()),
            None => self.card.type_in(&item.body),
        }
        self.card.sealed = parts;
        self.card.edit = Some(id);
        if self.card.field.is_none() {
            self.card_take_keys(Field::Box);
        } else {
            self.card.field = Some(Field::Box);
        }
        self.sync_card_input();
        self.request_card_draw();
    }

    /// Enter in the writing box: what is written is kept — as a new item of
    /// the list that is up, or as the new words of the item being changed.
    fn card_keep_draft(&mut self) {
        let text = self.card.take_draft();
        // (On the dictionary's page what is written is a word to look up;
        // on the translator's, something to translate.)
        if self.card.page == Page::Dictionary {
            let edit = self.card.edit.take();
            self.card_keep_word(&text, edit);
            return;
        }
        if self.card.page == Page::Translate {
            let edit = self.card.edit.take();
            self.card_keep_translation(&text, edit);
            return;
        }
        let mut parts = std::mem::take(&mut self.card.sealed);
        if !text.trim().is_empty() {
            let words = self.card_part(Kind::Text, text.trim_end().to_owned(), None, false, 0.0);
            parts.push(words);
        }
        let edit = self.card.edit.take();
        // (A voice note's title: the words alone, the note stays a note.)
        if let Some(note) = edit
            .and_then(|id| self.card.item_mut(id))
            .filter(|it| it.kind == Kind::Voice)
        {
            if !text.trim().is_empty() {
                note.body = text.trim().lines().next().unwrap_or("").to_owned();
                self.card_save();
            }
            self.request_card_draw();
            return;
        }
        // (Nothing in it: nothing is kept — an item being changed stays as
        // it was; its × is what removes it.)
        if parts.is_empty() {
            return;
        }
        // ONE thing is an item of that kind, as ever; SEVERAL are one item
        // made of them, in their order.
        let (kind, body, path, owned, aspect, parts) = if parts.len() == 1 {
            let p = parts.remove(0);
            (p.kind, p.body, p.path, p.owned, p.aspect, Vec::new())
        } else {
            (Kind::Text, Item::words_of(&parts), None, false, 0.0, parts)
        };
        if let Some(id) = edit {
            if let Some(item) = self.card.items.iter_mut().find(|it| it.id == id) {
                item.kind = kind;
                item.body = body;
                item.path = path;
                item.owned = owned;
                item.aspect = aspect;
                item.parts = parts;
            }
            self.card.lines.remove(&id);
            for path in self.card_pictures() {
                self.card_ask_picture(&path);
            }
            self.card_save();
            self.request_card_draw();
            return;
        }
        self.card.keeping = true;
        self.card_push(kind, body, path, owned, aspect, None);
        self.card.keeping = false;
        if !parts.is_empty() {
            if let Some(last) = self.card.items.last_mut() {
                last.parts = parts;
                let id = last.id;
                self.card.lines.remove(&id);
            }
            self.card_save();
        }
    }

    /// Ctrl+Enter in the input box: what is written goes into the card's
    /// window, and the cursor goes back there with it.
    fn card_send_draft(&mut self) {
        let text = self.card.take_draft();
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
            // The +: its menu — or, while something is being recorded into
            // the box, the end of that.
            BoxBtn::Clip if self.card.rec.is_some() => self.card_record_stop(true),
            BoxBtn::Clip => {
                self.card.menu = None;
                self.card.plus = !self.card.plus;
                self.request_card_draw();
            }
        }
    }

    /// An emoji of the picker goes WHERE THE CURSOR IS: into the input box
    /// while the card has the keyboard there; straight into the window
    /// while the window has it — as a phone's keyboard does, and nothing
    /// is left on the card (Max, 2026-10-09: *"i dont want to have one
    /// emoji on the card for each time i want to send an emoji"*). The
    /// picker stays open for the next one.
    fn card_emoji(&mut self, n: usize) {
        let Some((at, emoji)) = self
            .card
            .picked
            .get(n)
            .and_then(|at| crate::emoji_table::EMOJI.get(*at).map(|def| (*at, def.ch)))
        else {
            return;
        };
        // (Remembered as used — shown first the NEXT time the picker
        // opens: the grid does not jump under the pointer now.)
        self.card.used(at);
        self.card_save();
        // Straight into the window, whatever has the cursor (the picker's
        // search gives the keyboard back first).
        let typing = self.card.field.is_some();
        self.serve_transient_text(emoji);
        self.card_drop_keys();
        let wait = if typing {
            PASTE_FOCUS + 60
        } else {
            PASTE_SETTLE
        };
        self.after_ms(wait, |_| card_paste_key());
    }

    /// The paperclip: the desktop's file picker, and what is picked is
    /// kept in the memory.
    fn card_plus(&mut self, row: usize) {
        match row {
            // A picture, a file: picked from this computer.
            0 | 1 => self.card_pick_file(),
            // A voice note: recorded until Enter (or the +) ends it.
            2 => self.card_record_start(false),
            // Voice to text: what is said is written where the cursor is.
            _ => self.card_record_start(true),
        }
        self.request_card_draw();
    }

    /// A new item of the card's, not yet in any list.
    fn card_part(
        &mut self,
        kind: Kind,
        body: String,
        path: Option<String>,
        owned: bool,
        aspect: f32,
    ) -> Item {
        let id = self.card.next_id.max(1);
        self.card.next_id = id + 1;
        Item {
            id,
            kind,
            body,
            path,
            aspect,
            owned,
            at: now_secs(),
            from: None,
            parts: Vec::new(),
            said: None,
        }
    }

    /// `part` goes INTO what is being written, after what is there: the
    /// words written so far become a part of their own first, so the order
    /// is the order things were put in (words, a picture, more words…).
    fn card_seal(&mut self, part: Item) {
        let text = self.card.take_draft();
        if !text.trim().is_empty() {
            let words = self.card_part(Kind::Text, text.trim_end().to_owned(), None, false, 0.0);
            self.card.sealed.push(words);
        }
        if let (Kind::Image, Some(path)) = (part.kind, part.path.clone()) {
            self.card_ask_picture(&path);
        }
        self.card.sealed.push(part);
        self.card.to_bottom = self.card.edit.is_none();
        self.sync_card_input();
        self.request_card_draw();
    }

    /// What was being written is given up: the parts put in it go — a
    /// recording of the card's own with them, unless an item still has it.
    fn card_unseal(&mut self) {
        for part in std::mem::take(&mut self.card.sealed) {
            if let (true, Some(path)) = (part.owned, part.path.as_deref()) {
                if !self.card.keeps(path) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    /// The file picker (the +'s Image and File): what is picked goes into
    /// what is being written.
    fn card_pick_file(&mut self) {
        let (tx, rx) = calloop::channel::channel::<Option<Facts>>();
        std::thread::spawn(move || {
            let _ = tx.send(crate::bt_files::pick_file_waiting().and_then(|p| facts(&p)));
        });
        let waiting = self
            .loop_handle
            .insert_source(rx, |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(Some(f)) = event {
                    // (The picker had the keyboard meanwhile: the box has
                    // it again, to go on writing.)
                    if app.card.field != Some(Field::Box) {
                        app.card_take_keys(Field::Box);
                    }
                    let part = app.card_part(f.kind, f.body, Some(f.path), false, f.aspect);
                    app.card_seal(part);
                }
            });
        if waiting.is_err() {
            warn!("card: cannot wait for the file picker");
        }
    }

    /// What was said, written in where the cursor is (a space before it
    /// if it follows a word).
    pub(super) fn card_say_in(&mut self, text: &str) {
        let at = self.card.caret.min(self.card.draft_len());
        let before = self.card.draft.chars().nth(at.wrapping_sub(1));
        if at > 0 && before.is_some_and(|c| !c.is_whitespace()) {
            self.card.type_in(" ");
        }
        self.card.type_in(text);
    }

    /// Put the writing cursor where the pointer is in the input box: on
    /// the line under it, between the two letters it is nearest to.
    fn card_place_caret(&mut self) {
        let (Some(ptr), Some(rect)) = (self.card.ptr, self.card.vrect()) else {
            return;
        };
        let pos = self.card.inside(ptr);
        let low = bottom(rect, self.card.page, self.card.seeking(), self.card.pick())
            .plain_if(self.card.titling())
            .with_input(self.card.writer);
        let spans = self.card.spans();
        let first = self.card.draft_top();
        let off = if self.card.draft_first.is_some() {
            self.card.draft_off
        } else {
            0.0
        };
        let Some((line, x)) = low.text_at(pos, spans.len(), first, off) else {
            return;
        };
        let Some(renderer) = self.card_renderer.as_mut() else {
            return;
        };
        let letters: Vec<char> = self.card.draft.chars().collect();
        let (start, end) = spans[line.min(spans.len() - 1)];
        // The nearest gap between letters: where the width of what is
        // before it comes closest to the pointer.
        let mut best = (start, f32::MAX);
        let mut before = String::new();
        for at in start..=end {
            let far = (renderer.measure_text(&before, TEXT_PX + 0.5, None) - x).abs();
            if far < best.1 {
                best = (at, far);
            }
            if let Some(c) = letters.get(at) {
                before.push(*c);
            }
        }
        self.card.caret = best.0;
        self.request_card_draw();
    }

    /// A button, let go where it was pressed.
    fn card_tap(&mut self, what: Hover) {
        self.card.say = None;
        match what {
            Hover::Close(id) => self.card_remove(id),
            // The disk: its menu opens under it (a second click closes it).
            Hover::Pin(id) => {
                self.card.menu = (self.card.menu != Some(id)).then_some(id);
                self.request_card_draw();
            }
            Hover::Menu(row) => self.card_menu(row),
            // The line of languages over a translation: their menu.
            Hover::Lang(id) => {
                let open = self.card.lang == Some(id);
                self.card.menu = None;
                self.card.plus = false;
                self.card.lang = (!open).then_some(id);
                self.request_card_draw();
            }
            Hover::Undo => self.card_undo(),
            // The big microphone: talk; again, and it is written.
            Hover::Mic => self.card_voice_button(true),
            // "Show all" / "Show less": the text is wrapped again, whole or cut.
            // (On a voice note it is "Transcribe" / "Hide text": what is
            // said in it is written out the first time, and kept.)
            Hover::More(id) => {
                if !self.card.wide.remove(&id) {
                    self.card.wide.insert(id);
                    self.card_write_out(id);
                }
                self.card.lines.remove(&id);
                self.request_card_draw();
            }
            Hover::Edit(id) => self.card_edit(id),
            Hover::Foot(which) => self.card_foot(which),
            Hover::Seek => self.card_take_keys(Field::Seek),
            Hover::SeekClear => {
                self.card.query.clear();
                if self.card.field == Some(Field::Seek) {
                    self.card_rest();
                }
                self.sync_card_input();
                self.request_card_draw();
            }
            // A click in the input box: the cursor goes there, and where in
            // the text it was clicked (Max: *"or click on a spot to place
            // my writing cursor"*).
            Hover::Input => {
                self.card_take_keys(Field::Box);
                self.card_place_caret();
            }
            Hover::Btn(which) => self.card_box_button(which),
            // New: the new session opens empty, on its frame (Max,
            // 2026-10-11: *"it should start with the ----drag----, not
            // with an input"*).
            Hover::New => self.card_begin(),
            Hover::Name => self.card_take_keys(Field::Name),
            Hover::Play(id) => self.card_play(id),
            Hover::Speed(_) => self.card_speed(),
            // A voice note's line: it plays from where it was clicked.
            Hover::Line(id) => {
                let along = self
                    .card
                    .ptr
                    .map(|p| self.card.inside(p))
                    .zip(self.card.tiles.iter().find(|t| t.id == id).map(Tile::line))
                    .map_or(0.0, |(p, line)| (p.0 - line.x) / line.w.max(1.0));
                self.card_play_from(id, along);
            }
            Hover::Emoji(n) => self.card_emoji(n),
            Hover::PickSeek => self.card_take_keys(Field::Pick),
            Hover::Tray => {}
            // A kind of the picker: its grid jumps to where that kind starts.
            Hover::Kind(kind) => {
                self.card.pick_query.clear();
                self.card.repick();
                let first = crate::emoji_table::EMOJI
                    .iter()
                    .position(|d| d.group as usize == kind)
                    .unwrap_or(0);
                // (Past the recent ones, which fill whole rows of their own.)
                let recent = self.card.recent.len();
                self.card.pick_scroll = Bottom::pick_row(recent + first).min(self.card_pick_span());
                self.request_card_draw();
            }
            Hover::Item(_) | Hover::Corner(_) => {}
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
        // (The page it was on, on this window, before it went away — its
        // window's workspace left, another window's card was called.)
        let left_on = self.card.wins.get(&host).and_then(|w| w.page);
        self.card.turn_to(id);
        match left_on {
            Some(Page::Session) | None => {}
            Some(page) if page != self.card.page => self.card_show(page),
            Some(_) => {}
        }
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
        // (The arrow, while a session is being chosen: back, nothing kept.)
        if self.card.picking.is_some() && which.page() == self.card.page {
            self.card_picked(None);
            return;
        }
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
        // (While a session is being chosen to keep something in, a click
        // on one keeps it there — the window's own session stays.)
        if self.card.picking.is_some() {
            self.card_picked(Some(id));
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
        // A whole session goes with it: the first click only ASKS, for a
        // few seconds (its row says so); the second forgets (Max,
        // 2026-10-10). And the hint then offers it back for a moment.
        let now = Instant::now();
        let armed = self
            .card
            .arming
            .is_some_and(|(which, at)| which == id && at.elapsed() < Duration::from_millis(3500));
        if !armed {
            self.card.arming = Some((id, now));
            self.after_ms(3500, move |app| {
                if app.card.arming.is_some_and(|a| a.1 == now) {
                    app.card.arming = None;
                    app.request_card_draw();
                }
            });
            self.request_card_draw();
            return;
        }
        self.card.arming = None;
        self.card_undo_over();
        let at = self.card.memory.iter().position(|p| p.id == id);
        let was_open = self.card.open == Some(id);
        let Some(past) = self.card.lose(id) else {
            return;
        };
        info!("card: a session of {} forgotten", past.items.len());
        for item in &past.items {
            self.card.lines.remove(&item.id);
        }
        self.card.undo = Some(Undone::Session {
            past,
            at: at.unwrap_or(0),
            was_open,
        });
        self.card_flash_long("Session forgotten · Undo");
        self.card_save();
        self.card_save_session();
        self.request_card_draw();
    }

    /// A click on an item. One MADE OF PARTS opens (Max, 2026-10-10: *"click
    /// expand (transcribe audios, etc), when an item is expanded, hover on
    /// the pieces show a border as the individual items so i know what i
    /// can drag or click to paste"*) — its voice notes are written out, its
    /// pieces answer by themselves — and a click on its ground folds it
    /// again. Anything else (a plain item, a piece of an open one) is pasted
    /// into the window.
    fn card_click(&mut self, id: u64) {
        let composed = self
            .card
            .items
            .iter()
            .any(|it| it.id == id && !it.parts.is_empty());
        // (On voice to text's page what was said is COPIED: it is in the
        // window already — Max, 2026-10-11: *"i want to copy it if i need
        // it"*.)
        let said = self.card.page == Page::Talk
            && !composed
            && self.card.item(id).is_some_and(|it| it.kind != Kind::Voice);
        if said {
            if let Some(body) = self.card.item(id).map(|it| it.body.clone()) {
                self.copy_text(&body);
                self.card_flash("Copied");
            }
        } else if !composed {
            self.card_paste(id);
        } else if !self.card.wide.remove(&id) {
            self.card.wide.insert(id);
            self.card_write_out(id);
        }
        self.request_card_draw();
    }

    /// AN ITEM WAS CLICKED — pressed and let go at once, where it was. ONE
    /// click acts (Max, 2026-10-12: *"i don't like the double click… i want
    /// to manage with single click, and somehow make the grab a little
    /// easier"*), but not before a moment has passed with the pointer still
    /// there: a click the pointer then runs away from was a grab that
    /// missed (the log of 2026-10-11: quick clean clicks between his
    /// drags), and does nothing. A session's row, which cannot be carried,
    /// answers at once.
    fn card_clicked(&mut self, id: u64) {
        let Some(at) = self.card.ptr else {
            return;
        };
        if self.card.page == Page::Memory {
            self.card_click(id);
            return;
        }
        let now = Instant::now();
        self.card.clicked = Some((id, now, at));
        self.after_ms(CLICK_WAIT_MS, move |app| {
            if app.card.clicked.is_some_and(|c| c.0 == id && c.1 == now) {
                app.card.clicked = None;
                app.card_click(id);
            }
        });
    }

    /// Write out what the voice notes of item `id` say (those not written
    /// out yet): each on a thread, as talk-to-text does it; its words stand
    /// under its row once they are in.
    fn card_write_out(&mut self, id: u64) {
        const BUSY: &str = WRITING_OUT;
        // (The item itself, when it is a voice note; else its parts.)
        let todo: Vec<(u64, PathBuf)> = self
            .card
            .items
            .iter()
            .filter(|it| it.id == id)
            .flat_map(|it| std::iter::once(it).chain(it.parts.iter()))
            .filter(|p| p.kind == Kind::Voice)
            // (Not yet, or it could not be: "(…)" is a reason, not its words —
            // it is tried again the next time the item is opened.)
            .filter(|p| {
                p.said
                    .as_deref()
                    .is_none_or(|s| s == BUSY || s.starts_with('('))
            })
            .filter_map(|p| p.path.as_ref().map(|path| (p.id, PathBuf::from(path))))
            .collect();
        for (part, wav) in todo {
            if let Some(p) = self.card.item_mut(part) {
                p.said = Some(BUSY.to_owned());
            }
            self.card.lines.remove(&part);
            let (tx, rx) = calloop::channel::channel::<String>();
            std::thread::spawn(move || {
                let said = match voice::engine() {
                    Some((cli, model)) => match voice::transcribe(&cli, &model, &wav) {
                        Ok(text) if text.trim().is_empty() => "(Nothing was made out)".to_owned(),
                        Ok(text) => text,
                        Err(why) => format!("({why})"),
                    },
                    None => "(The speech engine is not installed)".to_owned(),
                };
                let _ = tx.send(said);
            });
            let waiting = self
                .loop_handle
                .insert_source(rx, move |event, _, app: &mut App| {
                    if let calloop::channel::Event::Msg(said) = event {
                        if let Some(p) = app.card.item_mut(part) {
                            p.said = Some(said);
                        }
                        app.card.lines.remove(&part);
                        app.card_save();
                        app.request_card_draw();
                    }
                });
            if waiting.is_err() {
                warn!("card: cannot wait for a voice note being written out");
            }
        }
    }

    /// Voice to text's page: what was just said is WRITTEN INTO THE WINDOW
    /// (pasted there, the keys handed to it first), and is the page's one
    /// item from now — gone with the next thing said, unless its disk keeps
    /// it somewhere.
    pub(super) fn card_said(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            self.card.say = Some("Nothing was made out");
            return;
        }
        let item = self.card_part(Kind::Text, text.to_owned(), None, false, 0.0);
        self.card.lines.clear();
        self.card.items = vec![item];
        self.serve_transient_text(text);
        self.card.tabbed = true;
        self.card_drop_keys();
        self.after_ms(PASTE_FOCUS + 60, |_| card_paste_key());
        self.request_card_draw();
    }

    /// A row of the disk's menu (Max, 2026-10-10): PIN — a copy of the item
    /// is pinned; MEMORY — the card turns to Memory's page for a session to
    /// be CHOSEN (`picking`), and comes back once one is; NEW SESSION — a
    /// session is started with a copy of it in it. Always a copy of its own.
    fn card_menu(&mut self, row: usize) {
        if std::mem::take(&mut self.card.plus) {
            self.card_plus(row);
            return;
        }
        if let Some(id) = self.card.lang.take() {
            self.card_lang(id, row);
            return;
        }
        let Some(id) = self.card.menu.take() else {
            return;
        };
        let (Some(item), Some(what)) = (
            self.card.item(id).cloned(),
            self.card.menu_rows().get(row).copied(),
        ) else {
            return;
        };
        match what {
            "Pin" => {
                self.card.pin_copy(&item, now_secs());
                self.card_flash("Pinned");
            }
            "Memory" => {
                let back = self.card.page;
                self.card_show(Page::Memory);
                self.card.picking = Some((item, back));
            }
            _ => {
                let title = self.card_host_title();
                let new = self.card.begin(title, now_secs());
                self.card.keep_in(new, &item, now_secs());
                self.card_flash("Saved");
            }
        }
        self.card_save();
        self.request_card_draw();
    }

    /// Say `words` where the hint is, for a moment: what was just done.
    fn card_flash(&mut self, words: &str) {
        self.card_undo_over();
        let since = Instant::now();
        self.card.flash = Some((words.to_owned(), since));
        self.after_ms(1600, move |app| {
            if app.card.flash.as_ref().is_some_and(|f| f.1 == since) {
                app.card.flash = None;
                app.request_card_draw();
            }
        });
        self.request_card_draw();
    }

    /// A session was being chosen and is not any more: back to the page the
    /// card was on, with `into` (if any) holding a copy of the thing.
    fn card_picked(&mut self, into: Option<u64>) {
        let Some((item, back)) = self.card.picking.take() else {
            return;
        };
        let kept = into.is_some_and(|id| self.card.keep_in(id, &item, now_secs()));
        self.card_show(back);
        if kept {
            self.card_save();
            self.card_flash("Saved");
        }
        self.request_card_draw();
    }

    /// An item's ×: it is gone from the list that is up.
    fn card_remove(&mut self, id: u64) {
        // (Memory's page: a session's row — or, while it is searched, a
        // thing found: that goes from where it is kept.)
        if self.card.page == Page::Memory && !self.card.is_row(id) {
            for past in &mut self.card.memory {
                past.items.retain(|it| it.id != id);
            }
            self.card.pinned.retain(|it| it.id != id);
            self.card.lines.remove(&id);
            self.card_flash("Removed");
            self.card_save();
            return;
        }
        if self.card.page == Page::Memory {
            self.card_forget(id);
            return;
        }
        // (A row of the clipboard's page: the clip itself goes, from the
        // clipboard's own history — there is no taking that back.)
        if self.card.page == Page::Clipboard {
            let clip = id.wrapping_sub(CLIP_IDS);
            if let Some(idx) = self.clip.history.iter().position(|c| c.id == clip) {
                self.delete_clip(idx);
            }
            self.card.items = self.card_clip_rows();
            self.card.lines.remove(&id);
            self.card_flash("Removed");
            return;
        }
        let Some(at) = self.card.items.iter().position(|it| it.id == id) else {
            return;
        };
        self.card_undo_over();
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
        }
        // It is kept for a moment: the hint says "Removed · Undo", and a
        // click on that puts it back where it was (Max, 2026-10-10). A
        // picture of the card's own is deleted only once that is over.
        self.card.undo = Some(Undone::Item {
            item,
            at,
            page: self.card.page,
            open: self.card.open,
        });
        self.card_flash_long("Removed · Undo");
        self.card_save();
        self.request_card_draw();
    }

    /// What the hint offered back is taken back: the item is where it was,
    /// the session is in memory again.
    fn card_undo(&mut self) {
        let Some(undone) = self.card.undo.take() else {
            return;
        };
        self.card.flash = None;
        match undone {
            Undone::Item {
                item,
                at,
                page,
                open,
            } => {
                let path = item.path.clone();
                let here =
                    self.card.page == page && (page != Page::Session || self.card.open == open);
                let list = if here {
                    Some(&mut self.card.items)
                } else {
                    match page {
                        Page::Pinned => Some(&mut self.card.pinned),
                        Page::Dictionary => Some(&mut self.card.looked),
                        Page::Translate => Some(&mut self.card.translated),
                        Page::Session => self
                            .card
                            .memory
                            .iter_mut()
                            .find(|p| Some(p.id) == open)
                            .map(|p| &mut p.items),
                        _ => None,
                    }
                };
                if let Some(list) = list {
                    list.insert(at.min(list.len()), item);
                }
                if let Some(path) = path {
                    self.card_ask_picture(&path);
                }
            }
            Undone::Session { past, at, was_open } => {
                let id = past.id;
                let at = at.min(self.card.memory.len());
                self.card.memory.insert(at, past);
                if was_open {
                    let title = self.card_host_title();
                    if let Some(host) = self.card.host.clone() {
                        self.card.give(&host, title.as_deref(), id);
                    }
                    self.card.open = Some(id);
                }
                if self.card.page == Page::Memory {
                    self.card.items = self.card.memory.iter().map(session_row).collect();
                    self.card.lines.remove(&id);
                }
                self.card_save_session();
            }
        }
        info!("card: taken back");
        self.card_save();
        self.request_card_draw();
    }

    /// The hint's offer is over (its time ran out, or something else was
    /// removed): what was removed is gone for good — a picture of the card's
    /// own with the last thing that showed it.
    fn card_undo_over(&mut self) {
        let gone: Vec<Item> = match self.card.undo.take() {
            Some(Undone::Item { item, .. }) => vec![item],
            Some(Undone::Session { past, .. }) => past.items,
            None => return,
        };
        for item in gone {
            if let (true, Some(path)) = (item.owned, item.path.as_deref()) {
                if !self.card.keeps(path) {
                    let _ = std::fs::remove_file(path);
                }
            }
        }
    }

    /// As `card_flash`, for longer, and ending whatever it offered back.
    fn card_flash_long(&mut self, words: &str) {
        let since = Instant::now();
        self.card.flash = Some((words.to_owned(), since));
        self.after_ms(4500, move |app| {
            if app.card.flash.as_ref().is_some_and(|f| f.1 == since) {
                app.card.flash = None;
                app.card_undo_over();
                app.request_card_draw();
            }
        });
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
            .filter(|a| !a.is_empty() && a != DESK)
    }

    /// Where the card's "window" is: a real one's place — or, for the
    /// desktop's card, its place under the bar while it hangs there.
    fn card_host_spot(&self, host: &str) -> Option<crate::hypr::WindowSpot> {
        if host == DESK {
            self.card_banner_spot()
        } else {
            crate::hypr::window_spot(host)
        }
    }

    /// Whether the card is UP where the owner is: showing on the focused
    /// window (standing on it, or hung under the bar for it) — or, on a
    /// workspace with no window, hung under the bar for the desktop. Then
    /// the card's keys are for THAT card: they do not call another.
    pub(super) fn card_up_here(&self) -> bool {
        let here = self.card_focused().filter(|a| valid_addr(a));
        self.card_present() && self.card.host.as_deref() == Some(here.as_deref().unwrap_or(DESK))
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

    /// The stretch of the bar's row the card stands in right now (from,
    /// to): the bar draws no long pill there and takes no pointer there.
    pub(crate) fn card_banner_in_bar(&self) -> Option<(f32, f32)> {
        let b = self.card.banner.filter(|b| b.in_bar)?;
        Some((b.x, b.x + self.card.width()))
    }

    /// …the same stretch, and how far down the row the card's top is.
    pub(crate) fn card_banner_in_bar_at(&self) -> Option<(f32, f32, f32)> {
        let b = self.card.banner.filter(|b| b.in_bar)?;
        Some((b.x, b.x + self.card.width(), b.y))
    }

    /// Whether the card hangs under the bar right now.
    pub(crate) fn card_banner_open(&self) -> bool {
        self.card.banner.is_some_and(|b| !b.closing)
    }

    /// Where the card hangs under the bar, as the window it would stand on.
    fn card_banner_spot(&self) -> Option<crate::hypr::WindowSpot> {
        let b = self.card.banner?;
        let room = (self.card_size.1 as f32 - b.y - 48.0).max(MIN_HEIGHT);
        Some(crate::hypr::WindowSpot {
            x: b.x,
            y: b.y,
            w: self.card.width(),
            // (`card_rect` leaves a title bar's height under it.)
            h: room.min(BANNER_H) + 25.0,
            visible: true,
        })
    }

    /// THE BAR'S CARD PILL, scrolled down on: the card of the focused window
    /// comes out under the pill (`open`), or goes back where it was — onto
    /// its window if it was showing there, away if it was not. Nothing is
    /// turned on or off by this: that is the pill's click.
    pub(crate) fn card_banner(&mut self, open: bool) {
        if open == self.card_banner_open() {
            return;
        }
        if !open {
            let Some(b) = self.card.banner else {
                return;
            };
            if b.was_on {
                self.card.banner = None;
                self.card.lines.clear();
                self.sync_card();
                self.sync_options_input();
            } else {
                self.card.banner = Some(Banner { closing: true, ..b });
                self.card_dismiss();
            }
            // (The long pill is a pill again, and folds when it is left.)
            self.update_clip_reveal();
            self.draw_options();
            return;
        }
        // (No window here: it is the desktop's card that comes out.)
        let addr = self.card_focused().filter(|a| valid_addr(a)).unwrap_or_else(|| DESK.to_owned());
        // THE LONG PILL BECOMES THE CARD: the card stands where the long
        // pill was — its top left corner the pill's — at the size it has on
        // a window, and unrolls down from there.
        let pill = self.clip_long_rect();
        let in_bar = self.options_bar_matched.is_none();
        let was_on = self.card.host.as_deref() == Some(addr.as_str()) && self.card_present();
        self.card.banner = Some(Banner {
            x: pill.x,
            y: if in_bar { pill.y } else { self.options_bar_h() },
            in_bar,
            was_on,
            closing: false,
        });
        self.card.at = None;
        self.card.press = None;
        self.card.lines.clear();
        self.sync_options_input();
        self.draw_options();
        if was_on {
            self.sync_card();
        } else {
            self.card.leaving = false;
            self.card_summon(&addr);
        }
        info!("card: out under the bar for {addr}");
    }

    /// A drag came over the bar (`on`), or left it.
    pub(crate) fn card_bar_drag(&mut self, on: bool) {
        if self.card.bar_drag && !on {
            // (Off the bar without the card having taken it: it folds.)
            self.card_banner_check();
        }
        self.card.bar_drag = on;
    }

    /// A drag over the bar moved. ON THE CARD'S PILL the card comes out, to
    /// be dropped into (Max, 2026-10-12: *"i want to drag an item on top of
    /// the small button and the card to open so i can drop inside the
    /// card"*). Whether the move was the bar's (not the desktop's).
    pub(crate) fn card_bar_drag_at(&mut self, x: f32, _y: f32) -> bool {
        if !self.card.bar_drag {
            return false;
        }
        let ph = self.options_pill_h();
        let from = self.options_left_start();
        // (The round pill, and a little round it.)
        if x >= from - 6.0 && x <= from + ph + 6.0 && !self.card_banner_open() {
            self.card_banner(true);
        }
        true
    }

    /// The pointer left the pill, or the card: the card from the bar folds
    /// a moment later unless the pointer is on one of them again, or
    /// something is being written or carried there.
    pub(crate) fn card_banner_check(&mut self) {
        if !self.card_banner_open() {
            return;
        }
        self.after_ms(380, |app| {
            let on_pill = matches!(
                app.options_hover,
                Some(crate::options::PillId::Clipboard | crate::options::PillId::ClipboardBox)
            );
            let busy = app.card.ptr.is_some()
                || app.card.dnd_over
                || app.card.menu.is_some()
                || app.card.plus
                || app.card.field.is_some_and(|f| f != Field::Card);
            if app.card_banner_open() && !on_pill && !busy {
                app.card_banner(false);
            }
        });
    }

    /// THE BAR'S CARD PILL, clicked: the card on or off for the focused
    /// window (Max: *"super useful for windows with no titlebar"*). With the
    /// card out under the bar, a click puts it ON its window and there.
    pub(crate) fn card_pill_click(&mut self) {
        // (The desktop's card has no window to be put on: it goes back.)
        if self.card_banner_open() && self.card.host.as_deref() == Some(DESK) {
            self.card_banner(false);
            return;
        }
        if let Some(b) = self.card.banner.filter(|b| !b.closing) {
            if let Some(addr) = self.card.host.clone() {
                if !self.card.armed(&addr) {
                    let on = self.card.toggle(&addr);
                    self.card_tell_bar(&addr, on);
                    self.card_save_session();
                }
            }
            let _ = b;
            self.card.banner = None;
            self.card.lines.clear();
            self.sync_card();
            self.sync_options_input();
            self.update_clip_reveal();
            self.draw_options();
            return;
        }
        // (No window has the focus: there is none to put it on — it comes
        // out under the bar, the desktop's. Max, 2026-10-12: "clicking the
        // pill should open it on the bar when there is no window focused".)
        if self.card_focused().filter(|a| valid_addr(a)).is_none() {
            self.card_banner(true);
            info!("card: the bar's pill — out under the bar, no window");
            return;
        }
        let said = self.card_toggle(None);
        info!("card: the bar's pill — {said}");
    }

    /// What the bar's long card pill says: the last thing kept in the
    /// focused window's session, or what the pill is for.
    pub(crate) fn card_pill_words(&self) -> String {
        let addr = self.card.active.as_deref().or(self.card.host.as_deref());
        let session = addr
            .and_then(|a| self.card.wins.get(a))
            .and_then(|w| w.session);
        let showing = self.card.page == Page::Session && self.card.open == session;
        let last = if showing && session.is_some() {
            self.card.items.last()
        } else {
            self.card
                .memory
                .iter()
                .find(|p| Some(p.id) == session)
                .and_then(|p| p.items.last())
        };
        match last {
            Some(it) if it.kind == Kind::Voice => "Voice note".to_owned(),
            Some(it) => it
                .body
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .chars()
                .take(90)
                .collect(),
            None => "Nothing kept yet · scroll down for the card".to_owned(),
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
        // (The desktop's card: a window has the focus now, it is over.)
        if self.card.host.as_deref() == Some(DESK) {
            self.card_let_go();
        }
        // (The card from the bar was another window's: it is over.)
        if self.card.banner.is_some() && self.card.host.as_deref() != Some(addr) {
            self.card.banner = None;
        }
        self.card.active = Some(addr.to_owned());
        if self.card.follows(addr) {
            // (A window opened since the master switch has not heard yet.)
            if self.card.all {
                self.card_tell_bar(addr, true);
            }
            self.card_summon(addr);
        } else if self.card.host.as_deref() == Some(addr) {
            // Its own window again: it is back.
            self.card_back(Away::Focus);
        } else {
            // A window it is not on for: the card is HIDDEN for as long as
            // its own window is not the focused one (Max, 2026-10-10: *"we
            // keep it always hidden when i abandon the window, if i want it
            // i call it from the other one. that is simple, easy to
            // understand"*). It is a layer, drawn over every window: left
            // showing, it stood on top of whatever was moved over it.
            // (Staying unless covered, and not drawing its covered part,
            // were both tried that day and dropped.)
            self.card_away(Away::Focus);
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
        // (The long pill it hung from has its own ground again: the bar is
        // drawn afresh once the card is gone.)
        let hung = self.card.banner.take().is_some();
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
        if hung {
            self.sync_options_input();
            self.update_clip_reveal();
            self.draw_options();
        }
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
        match self.card_host_spot(&host) {
            None => {
                debug!("card: its window {host} is gone");
                self.card_window_closed(&host);
                self.card_let_go();
            }
            Some(spot) => {
                // (Hung under the bar, "its window" is the place there.)
                let spot = self.card_banner_spot().unwrap_or(spot);
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
            // …and while the card has the keyboard: a click anywhere off the
            // card must reach us, for it is how the keyboard is given back
            // (see `card_drop_keys`).
            // (Not while they are only the pointer's — `Field::Card`: then
            // the pointer leaving the card is what gives them back.)
            Some(_) if self.card.field.is_some_and(|f| f != Field::Card) => {
                vec![(0, 0, w as i32, h as i32)]
            }
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
                let now = app.card_host_spot(&host).map(|s| app.card_banner_spot().unwrap_or(s));
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

    /// Draw one frame of the card — and, WHILE AN ITEM IS CARRIED, measure
    /// it: once a second the log says how many frames were drawn, how long
    /// the longest took, how many moves of the drag came and the longest
    /// wait between two (Max, 2026-10-11: a drag that lags — which side is
    /// slow, the card's drawing or the moves it is sent?).
    pub(crate) fn draw_card(&mut self) {
        if self.card.drag.is_none() {
            self.card.drag_pace = None;
            return self.draw_card_frame();
        }
        let began = Instant::now();
        self.draw_card_frame();
        let took = began.elapsed().as_secs_f32() * 1000.0;
        let pace = self.card.drag_pace.get_or_insert(DragPace {
            since: Some(began),
            ..Default::default()
        });
        pace.draws += 1;
        pace.worst_draw = pace.worst_draw.max(took);
        pace.all_draw += took;
        if began
            .duration_since(pace.since.unwrap_or(began))
            .as_secs_f32()
            >= 1.0
        {
            debug!(
                "card: drag pace — {} frames (drawing: {:.1} ms each, longest {:.1}), {} moves (longest wait {:.0} ms), {} items, over the card {}",
                pace.draws,
                pace.all_draw / pace.draws.max(1) as f32,
                pace.worst_draw,
                pace.moves,
                pace.worst_wait,
                self.card.items.len(),
                self.card.dnd_over,
            );
            let last_move = self.card.drag_pace.as_ref().and_then(|p| p.last_move);
            self.card.drag_pace = Some(DragPace {
                since: Some(began),
                last_move,
                ..Default::default()
            });
        }
    }

    fn draw_card_frame(&mut self) {
        // (The page that is up is kept for this window: see `Win::page`.)
        if let Some(host) = self.card.host.as_deref() {
            let page = self.card.page;
            match self.card.wins.get_mut(host) {
                Some(win) => win.page = Some(page),
                None => {
                    self.card.wins.insert(
                        host.to_owned(),
                        Win {
                            page: Some(page),
                            ..Default::default()
                        },
                    );
                }
            }
        }
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
        // The list on its way to where the wheel sent it: quickly after a
        // step of the wheel, slowly through a glide.
        if self.card.to_bottom && !self.card.scroll_pick {
            self.card.scroll_to = None;
            self.card.gliding = false;
        }
        if let Some(to) = self.card.scroll_to {
            // (The list's scroll, or the emoji grid's.)
            let pick = self.card.scroll_pick;
            let (from, most) = if pick {
                (self.card.pick_scroll, self.card_pick_span())
            } else {
                (self.card.scroll, self.card.max_scroll)
            };
            let to = to.clamp(0.0, most);
            let rate = if self.card.gliding { 4.5 } else { 24.0 };
            let (at, going) = crate::animation::ease_toward(from, to, dt, rate, 0.4);
            if pick {
                self.card.pick_scroll = at;
            } else {
                self.card.scroll = at;
            }
            if going {
                moving = true;
            } else {
                self.card.scroll_to = None;
                self.card.gliding = false;
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
        // (Hung under the bar: the bar's boxes' own fill.)
        let boxed = self.card.banner.is_some().then(|| {
            let (fill, _) = self.clip_box_surface();
            ([fill[0], fill[1], fill[2], self.box_panel_alpha()], self.options_bar_is_bright())
        });
        let Some(renderer) = self.card_renderer.as_mut() else {
            return;
        };
        // Wrap what has not been wrapped yet: a text by the column (its
        // font is fixed-pitch), a name by its own average glyph.
        // (All of it in the space the contents are laid out in: the card
        // `1/zoom` its size. The scene is scaled up at the end.)
        let zoom = self.card.zoom();
        let real = rect;
        let rect = virt(rect, zoom);
        let text_w = rect.w - 2.0 * LIST_PAD - 2.0 * TILE_PAD_X;
        // (Wrapped for another width — a frame drawn before the card had
        // its place, a window resized: all of it again.)
        if (text_w - self.card.wrapped_w).abs() > 0.5 {
            self.card.lines.clear();
            self.card.wrapped_w = text_w;
        }
        let text_px = TEXT_PX;
        let mono = renderer.measure_text("MMMMMMMMMM", text_px, Some(crate::options::NERD)) / 10.0;
        // (`whole`: a part of an item, or of what is being written — its
        // words are never cut short; an item's own are, unless it is open.)
        // Memory's page, searched: the things found, under their rows.
        let searching = self.card.page == Page::Memory
            && !self.card.query.trim().is_empty()
            && self.card.picking.is_none();
        if searching {
            let (finds, rows) = self.card.search_all();
            self.card.finds = finds;
            self.card.find_rows = rows;
        } else {
            self.card.finds.clear();
            self.card.find_rows.clear();
        }
        let all = self
            .card
            .items
            .iter()
            .chain(self.card.finds.iter())
            .flat_map(|it| std::iter::once((it, false)).chain(it.parts.iter().map(|p| (p, true))))
            .chain(self.card.sealed.iter().map(|p| (p, true)));
        for (item, whole) in all {
            if self.card.lines.contains_key(&item.id) {
                continue;
            }
            let lines = if !item.parts.is_empty() {
                // (Made of parts: it has no lines of its own.)
                Vec::new()
            } else if self.card.page == Page::Memory
                && (!searching || self.card.find_rows.contains(&item.id))
            {
                // (A memory's row: its name, its last thing — one line each.)
                item.body.lines().take(2).map(str::to_owned).collect()
            } else if item.kind == Kind::Voice {
                // (A voice note that was written out: what it says.)
                // (A note of its own shows it only once asked — "Transcribe";
                // a part's is always wrapped, and shown when its item is open.)
                match item
                    .said
                    .as_deref()
                    .filter(|_| whole || self.card.wide.contains(&item.id))
                {
                    Some(said) => {
                        let n = said.chars().count().max(1);
                        let w = renderer.measure_text(said, text_px, None);
                        let cols = if w > 0.0 {
                            ((text_w - 6.0) * 0.94 / (w / n as f32)).floor() as usize
                        } else {
                            40
                        };
                        wrap(said, cols, 30)
                    }
                    None => Vec::new(),
                }
            } else if item.kind == Kind::Text {
                let cols = if mono > 0.0 {
                    (text_w / mono).floor() as usize
                } else {
                    40
                };
                // (A word's answer is read whole; a text on the card keeps
                // its first lines.)
                let most = if whole || self.card.wide.contains(&item.id) {
                    ALL_LINES
                } else {
                    MAX_LINES
                };
                wrap(&item.body, cols, most)
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
        // A list that was showing its newest item keeps showing it when
        // the card's height changes under it (its window was resized).
        if self.card.max_scroll > 0.0 && self.card.scroll >= self.card.max_scroll - 0.5 {
            self.card.to_bottom = true;
        }
        // What is written in the input box, wrapped to it.
        // (The box is as wide as an item: it stands among them.)
        let low = bottom(rect, self.card.page, self.card.seeking(), None).with_input(Some(
            Rect::new(rect.x + LIST_PAD, rect.y, rect.w - 2.0 * LIST_PAD, 48.0),
        ));
        let box_w = low.text().map_or(200.0, |r| r.w);
        // (As ranges of its characters, so the cursor has a place in it;
        // by the real width of each letter — measured once and kept — so a
        // line is as long as the box lets it be.)
        self.card.caret = self.card.caret.min(self.card.draft_len());
        for c in self.card.draft.chars() {
            if c != '\n' && !self.card.letter_w.contains_key(&c) {
                // (Between two others, so a space has its width too.)
                let both = renderer.measure_text(&format!("n{c}n"), TEXT_PX + 0.5, None);
                let bare = renderer.measure_text("nn", TEXT_PX + 0.5, None);
                self.card.letter_w.insert(c, (both - bare).max(0.0));
            }
        }
        let widths = &self.card.letter_w;
        self.card.draft_spans = wrap_spans_by(&self.card.draft, (box_w - 3.0).max(20.0), |c| {
            widths.get(&c).copied().unwrap_or(7.0)
        });
        let letters: Vec<char> = self.card.draft.chars().collect();
        self.card.draft_lines = match self.card.draft.is_empty() {
            true => Vec::new(),
            false => self
                .card
                .draft_spans
                .iter()
                .map(|(s, e)| letters[*s..*e].iter().collect())
                .collect(),
        };
        // Where the writing cursor is: its line, and how far along it.
        let caret = (self.card.field == Some(Field::Box)).then(|| {
            let line = self.card.caret_line();
            let start = self.card.spans()[line].0;
            let before: String = letters[start..self.card.caret.max(start)].iter().collect();
            (line, renderer.measure_text(&before, TEXT_PX + 0.5, None))
        });
        // What is selected in it: for each line it touches, from where to
        // where along it.
        let mut select: Vec<(usize, f32, f32)> = Vec::new();
        if let (Some(Field::Box), Some((from, to))) = (self.card.field, self.card.selected()) {
            for (line, (start, end)) in self.card.spans().into_iter().enumerate() {
                let (a, b) = (from.max(start), to.min(end));
                if a > b || (a == b && !(from <= start && to > end)) {
                    continue;
                }
                let upto = |at: usize, r: &mut crate::renderer::Renderer| {
                    let before: String = letters[start..at.max(start)].iter().collect();
                    r.measure_text(&before, TEXT_PX + 0.5, None)
                };
                let (x0, x1) = (upto(a, renderer), upto(b, renderer));
                select.push((line, x0, x1));
            }
        }
        // The search narrows the list to what matches.
        let found: Vec<Item>;
        let listed: &[Item] = if searching {
            &self.card.finds
        } else if self.card.query.trim().is_empty() {
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
        let notes: HashMap<u64, String> = listed
            .iter()
            .map(|it| {
                let when = when_text(it.at, now_s);
                let note = match (self.card.page, &it.from) {
                    (Page::Dictionary, Some(from)) => from.clone(),
                    (Page::Clipboard, Some(from)) => {
                        let from: String = from.chars().take(26).collect();
                        format!("{from} · {when}")
                    }
                    _ => when,
                };
                (it.id, note)
            })
            .collect();
        let picking = self.card.pick().is_some();
        let emoji: Vec<&'static str> = match picking {
            true => self
                .card
                .picked
                .iter()
                .filter_map(|n| crate::emoji_table::EMOJI.get(*n).map(|d| d.ch))
                .collect(),
            false => Vec::new(),
        };
        // One emoji to stand for each kind: its first.
        let kinds: Vec<&'static str> = match picking {
            true => (0..crate::emoji_table::GROUPS.len())
                .filter_map(|g| {
                    crate::emoji_table::EMOJI
                        .iter()
                        .find(|d| d.group as usize == g)
                        .map(|d| d.ch)
                })
                .collect(),
            false => Vec::new(),
        };
        let name = self.card.open_name();
        let recording = self.card.rec.as_ref().map(|r| (r.seconds(), r.talk()));
        let talk = match (recording, self.card.writing) {
            (Some((_, true)), _) => Some("Listening…  press again to write it"),
            (_, true) => Some("Writing what you said…"),
            _ => None,
        };
        let playing = self.card.playing.as_ref().map(|p| (p.id, p.at()));
        // THE HINT: what the keys do here, for as long as the card has them
        // (without them it says nothing — Max, 2026-10-10: *"they are not
        // useful when it is unfocused"*).
        let flash = self.card.flash.as_ref().map(|f| f.0.clone());
        // (A voice note being recorded with Space held: its seconds.)
        let held = recording
            .filter(|(_, talk)| !talk && self.card.held)
            .map(|(secs, _)| {
                format!(
                    "●  {}:{:02}   recording — let go to keep it",
                    secs / 60,
                    secs % 60
                )
            });
        let hint = match (hover, self.card.field, self.card.page) {
            _ if held.is_some() => held.as_deref().unwrap_or(""),
            _ if flash.is_some() => flash.as_deref().unwrap_or(""),
            _ if self.card.say.is_some() => self.card.say.unwrap_or(""),
            (_, Some(_), _) if self.card.picking.is_some() => {
                "Click a session · Enter for a new one · Escape goes back"
            }
            (Some(Hover::Btn(which)), ..) => which.hint(),
            (_, None, _) => "",
            (_, Some(Field::Box), _)
                if self
                    .card
                    .edit
                    .and_then(|id| self.card.item(id))
                    .is_some_and(|it| it.kind == Kind::Voice) =>
            {
                "Type a title and press Enter"
            }
            (_, Some(Field::Box), _) if recording.is_some_and(|r| r.1) => {
                "Listening… Enter writes what you said"
            }
            (_, Some(Field::Box), _) if recording.is_some() => "Recording… Enter puts it in",
            (_, Some(Field::Box), _) => {
                "Enter keeps it · Shift+Enter for a new line · Escape drops it"
            }
            (_, Some(Field::Name), _) => "Enter keeps the name · Escape leaves it",
            (_, _, Page::Session) if self.card.open.is_some() => {
                "Type to search · press Enter to create · hold Space to talk"
            }
            (_, _, Page::Memory) => "Enter to create · type to search",
            (_, _, Page::Clipboard) => "Click to paste · type to search",
            (_, _, Page::Pinned) => "Type to search · press Enter to create · hold Space to talk",
            (_, _, Page::Dictionary) => "Type to search · press Enter to look a word up",
            (_, _, Page::Translate) => "Type to search · press Enter to translate",
            (_, _, Page::Talk) => "Press the microphone, or hold Space to talk",
            _ => "",
        };
        let view = View {
            rect,
            shown,
            boxed,
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
                // (On an item, near its top right corner: its buttons show.)
                match (hover, self.card.ptr.map(|p| self.card.inside(p))) {
                    (Some(Hover::Item(id)), Some(at))
                        if self
                            .card
                            .tiles
                            .iter()
                            .any(|t| t.id == id && t.corner().contains(at)) =>
                    {
                        Some(Hover::Corner(id))
                    }
                    _ => hover,
                }
            },
            foot: Some(FootView {
                page: self.card.page,
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
                    caret,
                    first: self.card.draft_top(),
                    // (Only while the wheel chose the line: following the
                    // cursor, a line is whole.)
                    first_off: if self.card.draft_first.is_some() {
                        self.card.draft_off
                    } else {
                        0.0
                    },
                    select: &select,
                    rec: recording.filter(|(_, talk)| !talk).map(|(secs, _)| secs),
                    talk,
                    hint,
                    hint_lit: flash.is_some(),
                    hint_red: held.is_some(),
                    plain: self.card.titling(),
                    // (The kind of the first emoji in view, past the ones
                    // used lately, which are of no kind.)
                    kind_now: Bottom::first_in_view(self.card.pick_scroll)
                        .checked_sub(self.card.recent.len())
                        .and_then(|n| crate::emoji_table::EMOJI.get(n))
                        .map(|d| d.group as usize),
                    picking: self.card.pick(),
                    emoji: &emoji,
                    pick_query: &self.card.pick_query,
                    kinds: &kinds,
                    recent: if self.card.pick_query.trim().is_empty() {
                        self.card.recent.len()
                    } else {
                        0
                    },
                },
                notes: &notes,
                playing,
                paused: self.card.paused,
                speed: voice::speed_name(self.card.voice_speed),
                writing: (self.card.field == Some(Field::Box)).then_some(self.card.edit),
                title: self
                    .card
                    .picking
                    .is_some()
                    .then_some("Keep in which session?"),
                choosing: self.card.picking.is_some(),
                sealed: &self.card.sealed,
                wide: &self.card.wide,
                rows: searching.then_some(&self.card.find_rows),
                arming: self.card.arming.map(|a| a.0),
                renaming: self
                    .card
                    .renaming
                    .filter(|_| self.card.field == Some(Field::Name))
                    .map(|id| (id, self.card.naming.as_str())),
                menu: self.card.menu_rect().map(|rect| MenuView {
                    rect,
                    rows: self.card.menu_rows(),
                    hot: match hover {
                        Some(Hover::Menu(row)) => Some(row),
                        _ => None,
                    },
                }),
            }),
            dnd_over: self.card.dnd_over,
            hidden,
            opening: opening.map_or(0.0, |o| o.1),
            rise: 0.0,
            shifts: &self.card.shifts,
            slots: &self.card.slots,
            paint,
            zoom: 1.0,
        };
        let (mut scene, tiles, max_scroll) = scene(&view);
        zoom_scene(&mut scene, (real.x, real.y), zoom);
        if shown > 0.0 {
            self.card.max_scroll = max_scroll;
            self.card.scroll = if self.card.to_bottom {
                max_scroll
            } else {
                self.card.scroll.clamp(0.0, max_scroll)
            };
            self.card.to_bottom = false;
            // (The writing box's place is not an item's: it is kept apart.)
            // (Its lower part: the line being written. Over it, the parts
            // already put in.)
            let draft_h = box_h(self.card.draft_lines.len());
            self.card.writer = tiles
                .iter()
                .find(|t| t.id == WRITER)
                .map(|t| Rect::new(t.rect.x, t.rect.y + t.rect.h - draft_h, t.rect.w, draft_h));
            self.card.tiles = tiles.into_iter().filter(|t| t.id != WRITER).collect();
        } else {
            self.card.tiles.clear();
            self.card.writer = None;
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
                self.card_hover_keys();
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
                self.card_banner_check();
                self.card_keys(false);
                // The keys go with the pointer — unless something is being
                // written: that keeps them until it is done.
                self.card.tabbed = false;
                self.card.menu = None;
                self.card.plus = false;
                self.card.lang = None;
                self.card.plus = false;
                self.card.lang = None;
                if self.card.field == Some(Field::Card) {
                    self.card_drop_keys();
                } else if let Some(field) = self.card.field {
                    info!("card: the pointer left; the keys stay ({field:?})");
                }
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
                    // A click off the card while it has the keyboard: the
                    // typing is over, the keyboard is its window's again.
                    // (It has to be taken here: the window under the card
                    // is still the compositor's "active" one, so a click on
                    // it changes nothing there and the keyboard would stay
                    // on the card — Max, 2026-10-09: *"it wont let my KB go…
                    // it got stuck until i closed the card"*.)
                    if self.card.field.is_some() && !self.card.rect.is_some_and(|r| r.contains(at))
                    {
                        self.card_drop_keys();
                        return;
                    }
                    // (A press anywhere but on the disk's menu closes it.)
                    if (self.card.menu.is_some() || self.card.plus || self.card.lang.is_some())
                        && !matches!(
                            self.card.hit(at),
                            Some(Hover::Menu(_) | Hover::Pin(_) | Hover::Btn(_) | Hover::Lang(_))
                        )
                    {
                        self.card.menu = None;
                        self.card.plus = false;
                        self.card.lang = None;
                        self.card.plus = false;
                        self.card.lang = None;
                        self.card.plus = false;
                        self.card.lang = None;
                        self.request_card_draw();
                    }
                    // A click on the card of a window that is not the focused
                    // one FOCUSES that window (Max, 2026-10-10: *"clicking the
                    // card from another window activates the card on the
                    // calling window"*): the card is its window's, and from
                    // then on the pointer gives it the keys.
                    if let Some(host) = self.card.host.clone() {
                        if self.card.active.as_deref() != Some(host.as_str()) {
                            crate::hypr::focus_window(&host);
                            self.card.active = Some(host);
                        }
                    }
                    // A click on the card brings the keys back after a Tab.
                    self.card.tabbed = false;
                    self.card_hover_keys();
                    // In the input box's words: the cursor goes there, and
                    // travel from here selects (`Press::Select`).
                    if self.card.hit(at) == Some(Hover::Input) && self.card.grip(at).is_none() {
                        self.card.say = None;
                        self.card_take_keys(Field::Box);
                        self.card_place_caret();
                        self.card.anchor = Some(self.card.caret);
                        self.card.press = Some(Press::Select);
                        self.card_cursor();
                        return;
                    }
                    let edge = self.card.grip(at).zip(self.card.rect).zip(self.card.spot);
                    // (A press cancels a click that was waiting to act.)
                    self.card.clicked = None;
                    self.card.press = match self.card.hit(at) {
                        // A side edge first: it is the card's, whatever
                        // lies just inside it.
                        _ if edge.is_some() => edge.map(|((left_edge, r), s)| Press::Resize {
                            left_edge,
                            from_x: at.0,
                            left: r.x - s.x,
                            width: r.w,
                        }),
                        Some(Hover::Item(id)) => Some(Press::Item {
                            id,
                            at,
                            serial,
                            since: Instant::now(),
                        }),
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
                    // PRESS AND HOLD takes an item in hand where it is, with
                    // no travel needed (Max, 2026-10-12: *"make the grab a
                    // little easier"*): a click is quick, a grab is not.
                    if let Some(Press::Item { id, since, .. }) = self.card.press {
                        if self.card.page != Page::Memory {
                            self.after_ms(HOLD_MS_GRAB, move |app| {
                                if let Some(Press::Item {
                                    id: held,
                                    since: from,
                                    serial,
                                    ..
                                }) = app.card.press
                                {
                                    if held == id && from == since {
                                        app.card_lift(id, serial);
                                    }
                                }
                            });
                        }
                    }
                    self.card_cursor();
                }
                wl_pointer::ButtonState::Released => {
                    match self.card.press.take() {
                        // Pressed and let go without carrying it off: a click.
                        // (Held a while first: a grab that did not take.)
                        Some(Press::Item { id, since, .. }) => {
                            if since.elapsed().as_millis() < CLICK_MS {
                                self.card_clicked(id);
                            }
                        }
                        Some(Press::Tap(what)) => {
                            if self.card_still_on(what) {
                                self.card_tap(what);
                            }
                        }
                        // (Let go where it was pressed: a click, nothing
                        // selected.)
                        Some(Press::Select) => {
                            if self.card.anchor == Some(self.card.caret) {
                                self.card.anchor = None;
                            }
                            self.request_card_draw();
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
            wl_pointer::Event::AxisStop {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                ..
            } => self.card_fling(),
            // (What the scroll that follows comes from: fingers, or a wheel.)
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(source),
            } => self.card.finger = source == wl_pointer::AxisSource::Finger,
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
            // Over the input box it is what is written that scrolls, a line
            // at a time (Max, 2026-10-09: *"i need to scroll on the text
            // when im over the input box too"*).
            // Over the input box it is what is written that scrolls (Max,
            // 2026-10-09: *"i need to scroll on the text when im over the
            // input box too"*) — by the pixel, not a line at a time.
            Some(false) if along != 0.0 && self.card_over_box() => {
                let lines = self.card.spans().len();
                let most = lines.saturating_sub(BOX_LINES) as f32 * TEXT_LINE;
                if most > 0.0 {
                    let at = self.card.draft_top() as f32 * TEXT_LINE
                        + if self.card.draft_first.is_some() {
                            self.card.draft_off
                        } else {
                            0.0
                        };
                    let to = (at + along * 2.4).clamp(0.0, most);
                    let first = (to / TEXT_LINE).floor();
                    self.card.draft_first = Some(first as usize);
                    self.card.draft_off = to - first * TEXT_LINE;
                    self.request_card_draw();
                }
            }
            // Over the emoji picker it is the emoji that scroll; anywhere
            // else, the list. Both the same way (`card_scroll_by`).
            Some(false) if along != 0.0 && self.card_over_tray() => {
                self.card_scroll_by(along, true)
            }
            Some(false) if along != 0.0 => self.card_scroll_by(along, false),
            _ => {}
        }
    }

    /// How far the emoji picker's grid can be scrolled.
    fn card_pick_span(&self) -> f32 {
        self.card.vrect().map_or(0.0, |rect| {
            bottom(rect, self.card.page, self.card.seeking(), self.card.pick()).pick_span()
        })
    }

    /// Whether the pointer is on the input box.
    fn card_over_box(&self) -> bool {
        matches!(
            self.card.ptr.and_then(|p| self.card.hit(p)),
            Some(Hover::Input | Hover::Btn(_))
        )
    }

    /// Whether the pointer is on the emoji picker.
    fn card_over_tray(&self) -> bool {
        matches!(
            self.card.ptr.and_then(|p| self.card.hit(p)),
            Some(Hover::Emoji(_) | Hover::Tray | Hover::PickSeek)
        )
    }

    /// The scroll gesture is over: the next one chooses its way afresh, and
    /// the pointer is the card's box's alone again.
    /// A step of the wheel over the list (or, `pick`, over the emoji grid).
    /// Under FINGERS what scrolls moves exactly as they do; a wheel's notch
    /// is eased to; and how fast it goes is kept, for the glide when it is
    /// let go (`card_fling`).
    fn card_scroll_by(&mut self, along: f32, pick: bool) {
        // (Another thing than the last scroll moved: nothing is carried over.)
        if self.card.scroll_pick != pick {
            self.card.scroll_pick = pick;
            self.card.scroll_to = None;
            self.card.gliding = false;
            self.card.scroll_vel = 0.0;
            self.card.scroll_last = None;
            self.card.scroll_run = 0.0;
        }
        let (at, most) = if pick {
            (self.card.pick_scroll, self.card_pick_span())
        } else {
            (self.card.scroll, self.card.max_scroll)
        };
        let step = along * 2.4;
        // (A glide under way is taken over from where it is.)
        let from = match (self.card.gliding, self.card.scroll_to) {
            (false, Some(to)) => to,
            _ => at,
        };
        self.card.gliding = false;
        self.card.scroll_run += step.abs();
        let now = Instant::now();
        let dt = self
            .card
            .scroll_last
            .map_or(0.1, |last| (now - last).as_secs_f32())
            .clamp(0.004, 0.1);
        self.card.scroll_vel = 0.55 * self.card.scroll_vel + 0.45 * (step / dt);
        self.card.scroll_last = Some(now);
        let to = (from + step).clamp(0.0, most);
        if self.card.finger {
            if pick {
                self.card.pick_scroll = to;
            } else {
                self.card.scroll = to;
            }
            self.card.scroll_to = None;
        } else {
            self.card.scroll_to = Some(to);
        }
        self.request_card_draw();
    }

    /// The fingers left the touchpad on an up-and-down scroll: one that was
    /// going FAST keeps going — the list glides on in the same direction and
    /// comes to rest, as a page does under a flick. (A wheel that had
    /// already stopped under the fingers does not.)
    fn card_fling(&mut self) {
        // (Fast, and after some travel: a short scroll, however quick, just
        // stops where it was let go.)
        const FAST: f32 = 900.0;
        const FAR: f32 = 90.0;
        let far = std::mem::take(&mut self.card.scroll_run) >= FAR;
        let fresh = self
            .card
            .scroll_last
            .is_some_and(|last| last.elapsed() < Duration::from_millis(90));
        let v = self.card.scroll_vel;
        self.card.scroll_vel = 0.0;
        self.card.scroll_last = None;
        if !fresh || !far || v.abs() < FAST || self.card.wheel.sideways == Some(true) {
            return;
        }
        // (How far it carries: its speed for about a third of a second.)
        let (at, most) = if self.card.scroll_pick {
            (self.card.pick_scroll, self.card_pick_span())
        } else {
            (self.card.scroll, self.card.max_scroll)
        };
        let from = self.card.scroll_to.unwrap_or(at);
        self.card.scroll_to = Some((from + v * 0.32).clamp(0.0, most));
        self.card.gliding = true;
        self.request_card_draw();
    }

    fn card_wheel_end(&mut self) {
        // (A wheel with no "fingers left" of its own — a mouse's: its speed
        // is not carried into the next scroll.)
        if !self.card.gliding {
            self.card.scroll_vel = 0.0;
            self.card.scroll_last = None;
        }
        self.card.scroll_run = 0.0;
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
        let ctl = crate::hypr::ctl_program();
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
        // (A click the pointer runs away from was a grab that missed.)
        if self
            .card
            .clicked
            .is_some_and(|(_, _, at)| (x - at.0).hypot(y - at.1) >= CLICK_AWAY)
        {
            self.card.clicked = None;
        }
        match self.card.press {
            // Travel in the input box's words selects them, from where the
            // press was to where the pointer is.
            Some(Press::Select) => {
                self.card_place_caret();
                return;
            }
            // (A session in memory is not carried anywhere: it is clicked.)
            Some(Press::Item { id, at, serial, .. })
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
                let travelled = moved || (x - from_x).abs() >= SLIDE_START;
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
        // (Max, 2026-10-11: the pointing hand on the items as on the
        // buttons — an item is clicked and dragged — and the closed hand
        // while one is carried.)
        let shape = match (self.card.press, hit) {
            _ if self.card.drag.is_some() => Shape::Grabbing,
            (Some(Press::Resize { .. }), _) => Shape::EwResize,
            (Some(Press::Slide { .. }), _) => Shape::Grabbing,
            (None, _) if grip.is_some() => Shape::EwResize,
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
            // `dictate down|up`: the key that is voice-to-text's — a tap
            // brings the card up at that page, held it is dictation with no
            // card at all (`dictate.rs`).
            "dictate" => match rest {
                "down" => self.dictate_down().to_owned(),
                "up" => self.dictate_up(),
                "cancel" => self.dictate_cancel().to_string(),
                _ => "card dictate down|up|cancel".to_owned(),
            },
            // `at clipboard|emoji|memory|voice`: the card on the focused window,
            // open AT that page — what a key does (Max, 2026-10-12: Super+.
            // the clipboard, Super+, the emoji, Super+/ memory). The same
            // key again, on that page, puts the card away.
            "at" => {
                let page = match rest {
                    "clipboard" => Page::Clipboard,
                    "emoji" | "emojis" => Page::Emoji,
                    "memory" => Page::Memory,
                    "voice" => Page::Talk,
                    _ => return "card at clipboard|emoji|memory|voice".to_owned(),
                };
                // (THE CARD THAT IS UP HERE is the one the key is for: it
                // turns to that page where it is — on its window or under
                // the bar — and the same key again puts it away. Max,
                // 2026-10-12: "the card should know when it's open… all of
                // the keybinds should go to the card because it's open".)
                let addr = self.card_focused().filter(|a| valid_addr(a));
                if self.card_up_here() {
                    if self.card.page != page {
                        self.card_show(page);
                        return format!("at {rest}");
                    }
                    if self.card_banner_open() {
                        self.card_banner(false);
                        return "away".to_owned();
                    }
                    return self.card_toggle(addr.as_deref());
                }
                match &addr {
                    Some(addr) if !self.card.armed(addr) => {
                        self.card_toggle(Some(addr));
                    }
                    Some(addr) => self.card_summon(addr),
                    // (A workspace with no window: from the bar's pill.)
                    None => self.card_banner(true),
                }
                self.card_show(page);
                format!("at {rest} on {}", addr.as_deref().unwrap_or("the desktop"))
            }
            // The input box, the search and the name, without a keyboard:
            // `write <text>` · `keep` · `send` · `find <words>` · `rename <name>`
            // · `clipboard` (its page).
            "write" => {
                self.card.draft = rest.replace("\\n", "\n");
                self.card.caret = self.card.draft_len();
                self.request_card_draw();
                format!("draft {:?}", self.card.draft)
            }
            // `select <from> <to>`: those characters of the box, selected
            // (the box is given the cursor so it shows) · `type <text>`:
            // written in at the cursor, over what is selected.
            "select" => {
                let mut ends = rest
                    .split_whitespace()
                    .filter_map(|n| n.parse::<usize>().ok());
                let (from, to) = (ends.next().unwrap_or(0), ends.next().unwrap_or(usize::MAX));
                self.card.field = Some(Field::Box);
                self.card.anchor = Some(from.min(self.card.draft_len()));
                self.card.caret = to.min(self.card.draft_len());
                self.request_card_draw();
                format!("selected {:?}", self.card.selection())
            }
            "type" => {
                self.card.type_in(rest);
                self.request_card_draw();
                format!("draft {:?}", self.card.draft)
            }
            // `caret <n>`: the writing cursor before character n of the box.
            "caret" => {
                self.card.caret = rest
                    .parse::<usize>()
                    .unwrap_or(0)
                    .min(self.card.draft_len());
                self.card.draft_first = None;
                self.request_card_draw();
                format!("caret {}", self.card.caret)
            }
            "keep" => {
                self.card_keep_draft();
                self.card_words()
            }
            // `disk <n>`: the disk's menu of item n of the list that is up,
            // open · `menu <row>`: that row of it (from 1).
            "disk" => {
                self.card.menu = rest
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| self.card.items.get(n.saturating_sub(1)))
                    .map(|it| it.id);
                self.request_card_draw();
                format!("menu {:?}", self.card.menu_rows())
            }
            "menu" => {
                self.card_menu(rest.parse::<usize>().unwrap_or(1).saturating_sub(1));
                format!("page {:?}", self.card.page)
            }
            // `click <n>`: a click on item n of the list that is up (one
            // made of parts opens or folds; another is pasted).
            "click" => {
                match rest
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| self.card.items.get(n.saturating_sub(1)))
                    .map(|it| it.id)
                {
                    Some(id) => {
                        self.card_click(id);
                        format!("open {:?}", self.card.wide)
                    }
                    None => "click <n>".to_owned(),
                }
            }
            // `plus`: the +'s menu of the writing box, open (then `menu <row>`).
            "plus" => {
                self.card.plus = true;
                self.request_card_draw();
                format!("menu {:?}", self.card.menu_rows())
            }
            // `undo`: what the hint offers back, taken back.
            "undo" => {
                self.card_undo();
                self.card_words()
            }
            // `edit <n>`: the pencil of item n of the list that is up.
            "edit" => {
                let id = rest
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| self.card.items.get(n.saturating_sub(1)))
                    .map(|it| it.id);
                match id {
                    Some(id) => {
                        self.card_edit(id);
                        format!("editing {:?}", self.card.draft)
                    }
                    None => "edit <n>".to_owned(),
                }
            }
            // `key <enter|escape|tab|backspace>`: that key, as the keyboard
            // would give it (the card must have the keys: the pointer on it).
            "key" => {
                use smithay_client_toolkit::seat::keyboard::Keysym;
                let key = match rest {
                    "enter" => Keysym::Return,
                    "escape" => Keysym::Escape,
                    "tab" => Keysym::Tab,
                    "backspace" => Keysym::BackSpace,
                    // (Space held: `space` is the press, `space-up` the letting go.)
                    "space" => Keysym::space,
                    "space-up" => {
                        self.card_key_release(Keysym::space);
                        return self.card_words();
                    }
                    _ => return "key enter|escape|tab|backspace|space|space-up".to_owned(),
                };
                self.card_key(key, None);
                format!("field {:?}", self.card.field)
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
            // `lang <n> <row>`: row of the menu of languages of translation n.
            "lang" => {
                let mut n = rest
                    .split_whitespace()
                    .filter_map(|x| x.parse::<usize>().ok());
                let id = n
                    .next()
                    .and_then(|i| self.card.items.get(i.saturating_sub(1)))
                    .map(|it| it.id);
                match (id, n.next()) {
                    (Some(id), Some(row)) => {
                        self.card_lang(id, row.saturating_sub(1));
                        "translating".to_owned()
                    }
                    _ => "lang <n> <row>".to_owned(),
                }
            }
            // `translate [words]`: the translator's page (and those words kept).
            "translate" => {
                self.card_show(Page::Translate);
                if !rest.trim().is_empty() {
                    self.card_keep_translation(rest, None);
                }
                format!("{} things", self.card.items.len())
            }
            // `talk`: voice-to-text's page · `said <words>`: as if that had
            // just been said there (written into the window, kept as its last).
            "talk" => {
                self.card_show(Page::Talk);
                format!("{:?}", self.card.page)
            }
            "said" => {
                self.card_show(Page::Talk);
                self.card_said(rest);
                self.card_words()
            }
            "clipboard" => {
                self.card_show(Page::Clipboard);
                format!("{} rows", self.card.items.len())
            }
            // `dictionary [word]` · `emojis`: their pages.
            "dictionary" => {
                self.card_show(Page::Dictionary);
                if !rest.trim().is_empty() {
                    self.card_keep_word(rest, None);
                }
                format!("{} words", self.card.items.len())
            }
            "emojis" => {
                self.card_show(Page::Emoji);
                format!("{} emoji", self.card.picked.len())
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
                    // `pin <n>`: a copy of item n of the session, pinned.
                    "pin" => {
                        self.card_show(Page::Session);
                        n.and_then(|n| self.card.items.get(n)).cloned().map(|it| {
                            self.card.pin_copy(&it, now_secs());
                            self.card_save();
                        })
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
            "carry" => self.card_carry(rest),
            // `banner open|close|click|words`: the bar's card pill, without
            // a pointer.
            "banner" => match rest {
                "open" => {
                    self.card_banner(true);
                    format!("{:?}", self.card.banner)
                }
                "close" => {
                    self.card_banner(false);
                    format!("{:?}", self.card.banner)
                }
                "click" => {
                    self.card_pill_click();
                    String::new()
                }
                _ => self.card_pill_words(),
            },
            // `icons <factor> <dx> <dy>` · `icons` (as they are): see
            // `view::ICON_TRY`.
            "icons" => {
                let n: Vec<f32> = rest
                    .split_whitespace()
                    .filter_map(|x| x.parse().ok())
                    .collect();
                if let Ok(mut t) = view::ICON_TRY.lock() {
                    *t = match n[..] {
                        [factor, dx, dy] => Some((factor, dx, dy)),
                        _ => None,
                    };
                }
                self.request_card_draw();
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
            parts: Vec::new(),
            said: None,
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
                page: None,
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
            parts: Vec::new(),
            said: None,
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
    fn a_sentences_language_is_told_from_its_common_words() {
        assert_eq!(
            language_of("hola, ¿cómo estás? necesito ayuda con esto"),
            "es"
        );
        assert_eq!(
            language_of("the card follows you through the windows"),
            "en"
        );
        assert_eq!(
            language_of("bonjour, comment ça va ? merci pour tout"),
            "fr"
        );
        assert_eq!(language_of("guten morgen, wie geht es dir? danke"), "de");
        // (Nothing to go by: English.)
        assert_eq!(language_of("xyzzy"), "en");
        assert_eq!(language_name("pt"), "Portuguese");
    }

    #[test]
    fn an_empty_session_is_gone_once_it_is_left() {
        let mut card = Card::default();
        let a = card.begin(Some("A".to_owned()), 1);
        card.turn_to(Some(a));
        // Open on screen, with nothing in it yet: it is there.
        assert_eq!((card.memory.len(), card.page), (1, Page::Session));
        // Left empty (Memory's page is put up): gone, and nothing is open.
        assert!(card.show(Page::Memory));
        assert!(card.memory.is_empty() && card.open.is_none() && card.items.is_empty());
        // With a thing in it, it stays when it is left.
        let b = card.begin(Some("B".to_owned()), 2);
        card.turn_to(Some(b));
        card.items.push(note(1, "kept"));
        assert!(card.show(Page::Memory));
        assert_eq!((card.memory.len(), card.items.len()), (1, 1));
    }

    #[test]
    fn what_the_disk_keeps_is_a_copy_of_its_own() {
        let mut card = Card::default();
        let a = card.begin(Some("A".to_owned()), 1);
        let b = card.begin(Some("B".to_owned()), 1);
        // (A session with nothing in it does not stay: this one has a thing.)
        card.memory[0].items.push(note(50, "in b"));
        card.turn_to(Some(a));
        card.items = vec![note(1, "me@example.org"), note(2, "b")];
        card.next_id = 100;
        // PINNED: a copy, with an id of its own; again, another copy (it
        // is not a switch any more).
        let item = card.items[0].clone();
        card.pin_copy(&item, 5);
        card.pin_copy(&item, 6);
        assert_eq!(card.pinned.len(), 2);
        assert!(card.pinned.iter().all(|p| p.id != 1 && p.body == item.body));
        // Kept in ANOTHER SESSION: there, not here.
        assert!(card.keep_in(b, &item, 7));
        assert_eq!(card.items.len(), 2);
        let in_b: Vec<&str> = card.memory[0]
            .items
            .iter()
            .map(|i| i.body.as_str())
            .collect();
        assert_eq!(
            (card.memory[0].id, in_b),
            (b, vec!["in b", "me@example.org"])
        );
        // …in the OPEN one: on the card, where its items are.
        assert!(card.keep_in(a, &item, 8));
        assert_eq!(card.items.len(), 3);
        // (No such session: nothing.)
        assert!(!card.keep_in(999, &item, 9));
        // The session goes; the pins stay, and are the list of their page —
        // where the disk has no "Pin" row.
        card.lose(a);
        assert!(card.show(Page::Pinned));
        assert_eq!(card.items.len(), 2);
        assert_eq!(card.menu_rows(), ["Memory", "New session"]);
        // The copies are apart: changing one leaves the other.
        card.items[0].body = "changed".to_owned();
        assert_eq!(card.items[1].body, "me@example.org");
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
        let own = card.items[0].clone();
        card.pin_copy(&own, 2);
        // Off the screen (Memory's page is up), but pinned and in memory.
        card.turn_to(None);
        assert!(card.keeps("/tmp/card/p.png"));
        card.pinned.clear();
        assert!(card.keeps("/tmp/card/p.png"));
        card.lose(a);
        assert!(!card.keeps("/tmp/card/p.png"));
    }
}

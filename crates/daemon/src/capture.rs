//! THE CAPTURE TOOL: what Print Screen opens (Max, 2026-11-16: "Print
//! Screen should be an implementation like macOS's or Windows': a TOOL, with
//! everything a user needs and some plus").
//!
//! One verb, `waverunner-ctl capture` (Golem binds Print to it; the card's
//! Captures page opens it from its camera): the screen the pointer is on
//! stays live, dimmed lightly outside what is chosen (`capture_view.rs`
//! draws it), and its controls come to the BANNER as pills — a round parent
//! and its children (`capture_pill.rs`; Max, the same day, on seeing a
//! toolbar at the bottom of the screen: "a parent pill and a few children"):
//!
//! - WHAT: Region · Window · Screen (one lit; Tab walks them). A region is
//!   dragged, then resized by its eight handles, moved by a drag inside and
//!   nudged by the arrows (Shift: ten at a time), its size written by it; the
//!   last one is there again at the next opening. A window is outlined under
//!   the pointer and taken by a click; so is the screen.
//! - Video (V): lit, a film; not lit, a picture. With it, Sound.
//! - Timer: off · 3 s · 10 s, a click the next.
//! - THE PLUS, Copy text: the words in the selection are READ (`tesseract`)
//!   and put on the clipboard. Not shown at all without `tesseract`.
//! - the round pill closes the tool (Escape).
//! - by the selection itself, one small button: Capture, or Record (Enter,
//!   or a double click inside a region).
//!
//! A picture is taken without the pointer and a film with it (`capture
//! pointer on` puts it in a picture: a verb, no control).
//!
//! THE SCREEN SIDE is drawn on the desktop's TOP surface (`desktop_top.rs`:
//! Overlay, above every window), made again on the pointer's screen when it
//! is on another (`capture_follow`). At rest that surface stops short of the
//! bar's strip (its exclusive zone); while the tool is up it is asked to
//! cover the screen whole (`capture_cover`), so the banner can be in a
//! region — but it takes no pointer on that strip, where the pills are, and
//! does not dim it. The keyboard is FETCHED with an exclusive grab and kept
//! on demand once it is here (`capture_keys_arrived`, the desktop's and the
//! card's way): an exclusive layer has the pointer pinned to it too, and the
//! pills must take their clicks. During a countdown the surface takes no
//! pointer at all, so a menu can be opened for the picture.
//!
//! TAKING IT. Nothing of the tool may be in the picture: its surface is
//! drawn EMPTY and its input region cleared, the banner's pills go in the
//! same step, at once (`capture_pills_off_now`; a banner called over a
//! fullscreen window goes whole), and only when the compositor says BOTH
//! frames were shown (their frame callbacks: `capture_frame_shown`,
//! `capture_bar_shown`; `HIDE_WITHIN` later at the latest) does a thread run
//! `grim`. A picture then does what Golem's `golem-screenshot` does: the
//! clipboard first (`wl-copy`), a PNG in Pictures → Screenshots, a
//! notification that carries the picture and opens it on a click. A video
//! starts the screen recording (`record.rs`) on the same rectangle, with its
//! task pill and Stop as they are. Nothing slow is on the loop, and closed
//! it costs nothing: no timer, no surface of its own, no state but what was
//! chosen last (`Prefs`).
//!
//! THE PRINT KEY itself no longer opens this tool (the third round, the same
//! night: `capture_shot.rs`): it takes the whole screen at once and shows it
//! on the card's pill; a click there opens this tool over the FROZEN shot,
//! as its editor (`Tool::still`: no film, no timer, nothing to hide before
//! taking, the button says Save). `capture` / `capture open` — the card's
//! camera, a film of a region or a window — stay the live tool described
//! here.
//!
//! Programs: `grim`, `wl-copy`, `wf-recorder`, `tesseract`, `busctl`; each
//! can be named by `WAVERUNNER_GRIM`, `WAVERUNNER_WL_COPY`,
//! `WAVERUNNER_WF_RECORDER`, `WAVERUNNER_TESSERACT`, `WAVERUNNER_BUSCTL` (a
//! rig's stand-ins).
//!
//! Pointer-free: `capture [toggle|open|close|state|pills]`, `capture what
//! region|window|screen`, `capture kind picture|video`, `capture timer
//! <0|3|10>`, `capture pointer on|off`, `capture sound on|off`, `capture rect
//! <x> <y> <w> <h>`, `capture take`, `capture text`. `state` says everything
//! in one line and `pills` where the banner's pills stand (the log's
//! `capture: …`).

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::seat::keyboard::Keysym;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{debug, info, warn};
use wayland_client::protocol::wl_pointer;
use wayland_client::WEnum;

use crate::content::Rect;
use crate::i18n::{fill, tr};
use crate::record::Aim;
use crate::App;

/// How near a handle or an edge the pointer takes it.
const REACH: f32 = 9.0;
/// A press that travels less than this draws no new region.
const DRAG_FROM: i32 = 3;
/// Two presses inside the region within this take it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// The surface's new size is waited for at most this long (ms).
const COVER_WITHIN: u64 = 120;
/// The keyboard is given this long to arrive before its grab is let go (ms).
const GRAB_WITHIN: u64 = 350;
/// After a picture the keyboard goes back to its window this much later
/// (ms): once `grim` has what it came for.
const KEYS_BACK_AFTER: u64 = 400;
/// The empty frame is waited for at most this long.
const HIDE_WITHIN: u64 = 150;
/// `grim` gets this long (a screen that is not being drawn never answers).
const GRIM_WITHIN: Duration = Duration::from_secs(10);
/// The timer's steps, in seconds.
pub(crate) const TIMERS: [u32; 3] = [0, 3, 10];

/// What is captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum What {
    Region,
    Window,
    Screen,
}

impl What {
    pub(crate) const ALL: [What; 3] = [What::Region, What::Window, What::Screen];

    fn name(self) -> &'static str {
        match self {
            What::Region => "region",
            What::Window => "window",
            What::Screen => "screen",
        }
    }

    /// The next (or the one before), round the ends: Tab's walk.
    fn step(self, back: bool) -> What {
        let i = What::ALL.iter().position(|w| *w == self).unwrap_or(0);
        What::ALL[(i + if back { 2 } else { 1 }) % 3]
    }
}

/// As a picture or as a film.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Picture,
    Video,
}

/// A rectangle of the screen, in the surface's own logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub(crate) fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub(crate) fn rect(self) -> Rect {
        Rect::new(self.x as f32, self.y as f32, self.w as f32, self.h as f32)
    }

    fn contains(self, (px, py): (f32, f32)) -> bool {
        px >= self.x as f32
            && px < (self.x + self.w) as f32
            && py >= self.y as f32
            && py < (self.y + self.h) as f32
    }

    /// Between two corners, whichever way they were dragged.
    fn between((ax, ay): (i32, i32), (bx, by): (i32, i32)) -> Self {
        Self::new(ax.min(bx), ay.min(by), (ax - bx).abs(), (ay - by).abs())
    }

    /// The part of it on a screen `bounds` big (`None`: none of it is).
    pub(crate) fn clipped(self, (bw, bh): (i32, i32)) -> Option<Self> {
        let (x0, y0) = (self.x.max(0), self.y.max(0));
        let (x1, y1) = ((self.x + self.w).min(bw), (self.y + self.h).min(bh));
        (x1 > x0 && y1 > y0).then(|| Self::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Moved back onto the screen whole (its size kept; one bigger than the
    /// screen is cut to it).
    fn kept_on(self, (bw, bh): (i32, i32)) -> Self {
        let (w, h) = (self.w.clamp(1, bw.max(1)), self.h.clamp(1, bh.max(1)));
        Self::new(
            self.x.clamp(0, (bw - w).max(0)),
            self.y.clamp(0, (bh - h).max(0)),
            w,
            h,
        )
    }

    /// A film's sides are even numbers (the encoder refuses others).
    fn even(self) -> Self {
        Self::new(self.x, self.y, (self.w & !1).max(2), (self.h & !1).max(2))
    }
}

/// Where a region is held: a corner, an edge, its inside (it moves), or
/// outside it (a new one is drawn).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grip {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
    Move,
    New,
}

impl Grip {
    /// The pointer's shape over it.
    fn shape(self) -> Shape {
        match self {
            Grip::N | Grip::S => Shape::NsResize,
            Grip::E | Grip::W => Shape::EwResize,
            Grip::NE | Grip::SW => Shape::NeswResize,
            Grip::NW | Grip::SE => Shape::NwseResize,
            Grip::Move => Shape::Move,
            Grip::New => Shape::Crosshair,
        }
    }
}

/// The eight handles of `a`: its corners and the middles of its sides.
pub(crate) fn handles(a: Area) -> [(Grip, (f32, f32)); 8] {
    let (x0, y0, x1, y1) = (
        a.x as f32,
        a.y as f32,
        (a.x + a.w) as f32,
        (a.y + a.h) as f32,
    );
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    [
        (Grip::NW, (x0, y0)),
        (Grip::NE, (x1, y0)),
        (Grip::SW, (x0, y1)),
        (Grip::SE, (x1, y1)),
        (Grip::N, (cx, y0)),
        (Grip::S, (cx, y1)),
        (Grip::W, (x0, cy)),
        (Grip::E, (x1, cy)),
    ]
}

/// What of region `a` is at `p`: a corner before a side (the whole side
/// resizes, not only its handle), a side before the inside.
pub(crate) fn grip_at(a: Area, p: (f32, f32), reach: f32) -> Grip {
    let (x0, y0, x1, y1) = (
        a.x as f32,
        a.y as f32,
        (a.x + a.w) as f32,
        (a.y + a.h) as f32,
    );
    let near = |v: f32, edge: f32| (v - edge).abs() <= reach;
    let (in_x, in_y) = (
        p.0 >= x0 - reach && p.0 <= x1 + reach,
        p.1 >= y0 - reach && p.1 <= y1 + reach,
    );
    if !in_x || !in_y {
        return Grip::New;
    }
    // (A small region: the nearer side of the two.)
    let west = near(p.0, x0) && (p.0 - x0).abs() <= (p.0 - x1).abs();
    let east = near(p.0, x1) && !west;
    let north = near(p.1, y0) && (p.1 - y0).abs() <= (p.1 - y1).abs();
    let south = near(p.1, y1) && !north;
    match (north, south, west, east) {
        (true, _, true, _) => Grip::NW,
        (true, _, _, true) => Grip::NE,
        (_, true, true, _) => Grip::SW,
        (_, true, _, true) => Grip::SE,
        (true, ..) => Grip::N,
        (_, true, ..) => Grip::S,
        (_, _, true, _) => Grip::W,
        (_, _, _, true) => Grip::E,
        _ if a.contains(p) => Grip::Move,
        _ => Grip::New,
    }
}

/// Region `start`, held by `grip` at `from`, with the pointer now at `to`,
/// on a screen `bounds` big: a side or a corner follows the pointer (and may
/// cross the opposite one), the inside moves it whole, outside a new one
/// spans the two points. Never off the screen, never empty.
pub(crate) fn dragged(
    start: Area,
    grip: Grip,
    from: (i32, i32),
    to: (i32, i32),
    bounds: (i32, i32),
) -> Area {
    let (bw, bh) = (bounds.0.max(1), bounds.1.max(1));
    let to = (to.0.clamp(0, bw), to.1.clamp(0, bh));
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let (mut x0, mut y0, mut x1, mut y1) = (start.x, start.y, start.x + start.w, start.y + start.h);
    match grip {
        Grip::New => return whole(Area::between(from, to), (bw, bh)),
        Grip::Move => {
            return Area::new(start.x + dx, start.y + dy, start.w, start.h).kept_on((bw, bh))
        }
        Grip::N => y0 += dy,
        Grip::S => y1 += dy,
        Grip::W => x0 += dx,
        Grip::E => x1 += dx,
        Grip::NW => (x0, y0) = (x0 + dx, y0 + dy),
        Grip::NE => (x1, y0) = (x1 + dx, y0 + dy),
        Grip::SW => (x0, y1) = (x0 + dx, y1 + dy),
        Grip::SE => (x1, y1) = (x1 + dx, y1 + dy),
    }
    whole(Area::between((x0, y0), (x1, y1)), (bw, bh))
}

/// `a` cut to the screen, and at least one pixel each way.
fn whole(a: Area, (bw, bh): (i32, i32)) -> Area {
    let (x0, y0) = (a.x.clamp(0, bw - 1), a.y.clamp(0, bh - 1));
    let (x1, y1) = ((a.x + a.w).clamp(x0 + 1, bw), (a.y + a.h).clamp(y0 + 1, bh));
    Area::new(x0, y0, x1 - x0, y1 - y0)
}

/// The screen the tool is on: its connector, its corner in the layout and
/// its size, logical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Screen {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// `"x,y wxh"` of `a` in the layout: what `grim -g` and `wf-recorder -g` take.
pub(crate) fn geometry(screen: &Screen, a: Area) -> String {
    format!("{},{} {}x{}", screen.x + a.x, screen.y + a.y, a.w, a.h)
}

/// A window as the tool needs it: where it is, and how it is stacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stacked {
    pub area: Area,
    pub floating: bool,
    pub fullscreen: bool,
    /// The compositor's focus history (0: the focused one).
    pub rank: i64,
}

/// The windows topmost first, as well as the compositor lets it be known:
/// a fullscreen one over everything, floating ones over tiled ones, and
/// among equals the one used last (its true stacking order is not told).
pub(crate) fn stack(mut wins: Vec<Stacked>) -> Vec<Area> {
    wins.sort_by_key(|w| (!w.fullscreen, !w.floating, w.rank));
    wins.into_iter().map(|w| w.area).collect()
}

/// What was chosen last: the next opening starts from it (kept while the
/// dock runs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prefs {
    pub what: What,
    pub kind: Kind,
    /// Seconds before it is taken (one of [`TIMERS`]).
    pub timer: u32,
    /// The pointer in the picture.
    pub pointer: bool,
    /// The sound in a film.
    pub sound: bool,
    /// The last region, and the screen it was on.
    pub region: Option<(String, Area)>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            what: What::Region,
            kind: Kind::Picture,
            timer: 0,
            pointer: false,
            sound: false,
            region: None,
        }
    }
}

/// What the main action, or Copy text, is about to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Job {
    Picture,
    Video,
    Text,
}

/// Where the tool is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Something is being chosen.
    Choosing,
    /// The timer counts: `left` seconds to go.
    Counting { job: Job, left: u32 },
    /// Drawn empty; `aim` is taken when that frame has been shown.
    Hiding { job: Job, aim: Aim },
}

/// A control of the tool: the banner's pills (`capture_pill.rs`), and the
/// one button by the selection (`Main`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ctl {
    What(What),
    /// Video: lit = a film, not lit = a picture.
    Video,
    Timer,
    Sound,
    Text,
    Main,
}

/// A region being dragged.
#[derive(Debug, Clone, Copy)]
struct Drag {
    grip: Grip,
    from: (i32, i32),
    start: Area,
    /// What was there before (a press that never travels leaves it).
    before: Option<Area>,
    moved: bool,
}

/// The tool, while it is up.
pub(crate) struct Tool {
    pub screen: Screen,
    /// The windows of the workspace on show there, topmost first.
    pub wins: Vec<Area>,
    /// The one outlined.
    pub win: Option<usize>,
    pub region: Option<Area>,
    drag: Option<Drag>,
    ptr: Option<(f32, f32)>,
    pub hover: Option<Ctl>,
    pressed: Option<Ctl>,
    last_press: Option<Instant>,
    pub phase: Phase,
    /// Whether text can be read here (`tesseract` is there).
    pub ocr: bool,
    /// The one button by the selection, Capture or Record, as it was last
    /// laid out (`capture_view.rs`); nothing wide while there is none.
    pub bar_rect: Rect,
    /// The banner's strip at the top of this screen (0: it is on another, or
    /// the surface begins under it): its pills take the pointer there, and
    /// it is not dimmed.
    pub strip: i32,
    /// The empty frame was drawn, and then shown; and the banner's frame
    /// without the pills likewise. The picture waits for both.
    pub cleared: bool,
    top_shown: bool,
    bar_drawn: bool,
    bar_shown: bool,
    /// THE EDITOR (`capture_shot.rs`): the screenshot this tool is open
    /// over, frozen (`None`: the live screen).
    pub still: Option<std::sync::Arc<crate::capture_shot::Kept>>,
}

/// The capture tool's place in [`App`].
#[derive(Default)]
pub(crate) struct Capture {
    pub tool: Option<Tool>,
    pub prefs: Prefs,
    /// Counts the tool's turns: a timer armed for an earlier one does nothing.
    epoch: u64,
    /// How the surface holds the keyboard and the pointer for it now.
    held: Held,
    /// The keyboard is being fetched (exclusive) and has not arrived yet.
    grab: bool,
    /// The surface was asked to cover the whole screen, the bar's strip too.
    whole: bool,
    /// The screen the tool opens on once the surface has its new size.
    opening: Option<String>,
    /// The Print key, its shot and its preview (`capture_shot.rs`).
    pub(crate) shots: crate::capture_shot::Shots,
    /// A still is in the surface's big picture (and goes with the tool).
    still_up: bool,
}

/// The top surface's hold for the tool.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Held {
    /// Nothing: the tool is closed, or drawn empty.
    #[default]
    Nothing,
    /// The keyboard, and the pointer everywhere under the banner's strip
    /// (this many pixels at the top are the banner's pills').
    All(i32),
    /// The keyboard while no window is clicked, and no pointer at all (a
    /// countdown: a menu can be opened for the picture).
    Keys,
}

impl Tool {
    fn bounds(&self) -> (i32, i32) {
        (self.screen.w, self.screen.h)
    }

    /// What is chosen now, on the surface.
    pub(crate) fn selection(&self, what: What) -> Option<Area> {
        match what {
            What::Region => self.region,
            What::Window => self.win.and_then(|i| self.wins.get(i).copied()),
            What::Screen => Some(Area::new(0, 0, self.screen.w, self.screen.h)),
        }
    }

    /// What `job` would take: the screen by its name, anything else by its
    /// rectangle in the layout (a film's sides made even).
    pub(crate) fn aim(&self, what: What, job: Job) -> Option<Aim> {
        let area = self.selection(what)?;
        Some(match what {
            What::Screen => Aim::Screen(self.screen.name.clone()),
            _ if job == Job::Video => Aim::Area(geometry(&self.screen, area.even())),
            _ => Aim::Area(geometry(&self.screen, area)),
        })
    }

    /// The topmost window at `p`.
    fn window_at(&self, p: (f32, f32)) -> Option<usize> {
        self.wins.iter().position(|w| w.contains(p))
    }

    /// The control at `p`.
    fn ctl_at(&self, p: (f32, f32)) -> Option<Ctl> {
        (self.bar_rect.w > 0.0 && self.bar_rect.contains(p)).then_some(Ctl::Main)
    }
}

/// A program by the name the environment gives it, else its own.
fn program(var: &str, name: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from(name), PathBuf::from)
}

fn grim() -> PathBuf {
    program("WAVERUNNER_GRIM", "grim")
}

fn wl_copy() -> PathBuf {
    program("WAVERUNNER_WL_COPY", "wl-copy")
}

fn tesseract() -> PathBuf {
    program("WAVERUNNER_TESSERACT", "tesseract")
}

/// Whether text can be read on this machine at all.
fn can_read_text() -> bool {
    crate::launch::on_path(&tesseract().to_string_lossy())
}

/// `grim`'s arguments for `aim` into `file`, the pointer in it or not.
pub(crate) fn grim_args(aim: &Aim, pointer: bool, file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    if pointer {
        args.push("-c".into());
    }
    match aim {
        Aim::Screen(name) => args.extend(["-o".into(), name.into()]),
        Aim::Area(area) => args.extend(["-g".into(), area.into()]),
    }
    args.push(file.into());
    args
}

/// `tesseract`'s arguments to read `picture` in `langs` onto its output.
pub(crate) fn tesseract_args(picture: &Path, langs: &[String]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![picture.into(), "stdout".into()];
    if !langs.is_empty() {
        args.extend(["-l".into(), langs.join("+").into()]);
    }
    args
}

/// Tesseract's name for a language of the shell (`es_ES` → `spa`).
fn tesseract_lang(shell: &str) -> Option<&'static str> {
    Some(match shell.split(['_', '-']).next().unwrap_or(shell) {
        "en" => "eng",
        "es" => "spa",
        "pt" => "por",
        "fr" => "fra",
        "de" => "deu",
        "it" => "ita",
        _ => return None,
    })
}

/// The languages to read in, from what `tesseract --list-langs` said: the
/// shell's and English where their data are there, else every one there is
/// (but the two that are not languages), else none named.
pub(crate) fn pick_langs(shell: Option<&str>, listed: &str) -> Vec<String> {
    let have: Vec<&str> = listed
        .lines()
        .map(str::trim)
        // (Its first line is a sentence: "List of available languages in …".)
        .filter(|l| !l.is_empty() && !l.contains(' ') && !l.contains(':'))
        .collect();
    let mut want: Vec<String> = Vec::new();
    for lang in shell.and_then(tesseract_lang).into_iter().chain(["eng"]) {
        if have.contains(&lang) && !want.iter().any(|w| w == lang) {
            want.push(lang.to_owned());
        }
    }
    if want.is_empty() {
        want = have
            .iter()
            .filter(|l| !matches!(**l, "osd" | "equ"))
            .map(|l| (*l).to_owned())
            .collect();
    }
    want
}

/// The text as it goes on the clipboard: no bare lines at its ends, no
/// spaces at a line's end (tesseract ends with a form feed).
pub(crate) fn tidy(read: &str) -> String {
    let lines: Vec<&str> = read
        .lines()
        .map(|l| l.trim_end_matches([' ', '\t', '\u{c}']))
        .collect();
    let first = lines.iter().position(|l| !l.trim().is_empty());
    let last = lines.iter().rposition(|l| !l.trim().is_empty());
    match (first, last) {
        (Some(a), Some(b)) => lines[a..=b].join("\n"),
        _ => String::new(),
    }
}

/// The first words of `text`, for its notification.
pub(crate) fn first_words(text: &str, most: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= most {
        return flat;
    }
    let cut: String = flat.chars().take(most).collect();
    format!("{}…", cut.trim_end())
}

/// The notification's arguments for `busctl`: `picture` is shown in it, and
/// a click on it opens that file.
pub(crate) fn notify_args(title: &str, body: &str, picture: Option<&Path>) -> Vec<String> {
    let mut args: Vec<String> = [
        "--user",
        "call",
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        "susssasa{sv}i",
        "waverunner",
        "0",
    ]
    .map(str::to_owned)
    .to_vec();
    let file = picture.map(|p| p.display().to_string());
    args.push(file.clone().unwrap_or_else(|| "camera-photo".to_owned()));
    args.extend([title.to_owned(), body.to_owned()]);
    match &file {
        // (Its action, as a pair of key and label; its picture as a hint.)
        Some(file) => args.extend([
            "2".to_owned(),
            format!("{}{file}", crate::NOTIFY_OPEN),
            tr("Open").to_owned(),
            "1".to_owned(),
            "image-path".to_owned(),
            "s".to_owned(),
            file.clone(),
        ]),
        None => args.extend(["0".to_owned(), "0".to_owned()]),
    }
    args.push("6000".to_owned());
    args
}

pub(crate) fn notify(title: &str, body: &str, picture: Option<&Path>) {
    let sent = Command::new(program("WAVERUNNER_BUSCTL", "busctl"))
        .args(notify_args(title, body, picture))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Err(e) = sent {
        debug!("capture: cannot notify ({e})");
    }
}

/// Run `cmd` to its end, or end it after `limit`. Whether it ended well.
fn run_within(mut cmd: Command, limit: Duration) -> bool {
    let Ok(mut child) = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let until = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// A place for a picture on its way (the session's own folder, in memory).
pub(crate) fn scratch_png() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    dir.join(format!("waverunner-capture-{}-{n}.png", std::process::id()))
}

/// `aim` as a PNG in `file`. Whether there is one.
pub(crate) fn grab(aim: &Aim, pointer: bool, file: &Path) -> bool {
    let mut cmd = Command::new(grim());
    cmd.args(grim_args(aim, pointer, file));
    run_within(cmd, GRIM_WITHIN) && std::fs::metadata(file).is_ok_and(|m| m.len() > 0)
}

/// `bytes` onto the clipboard as `kind`. Whether it went.
pub(crate) fn copy(kind: &str, bytes: &[u8]) -> bool {
    let spawned = Command::new(wl_copy())
        .args(["--type", kind])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawned else {
        return false;
    };
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut input| input.write_all(bytes).is_ok());
    child.wait().is_ok_and(|s| s.success()) && wrote
}

/// The file `picture` is kept in, in `dir`: `Screenshot <when>.png`, the
/// second inside one second `… (2).png`; it arrives whole under its name.
pub(crate) fn keep(picture: &[u8], dir: &Path, at: SystemTime) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let name = crate::record::stamped("Screenshot", "png", at);
    let stem = name.strip_suffix(".png").unwrap_or(&name);
    for n in 1..=50 {
        let file = if n == 1 {
            dir.join(&name)
        } else {
            dir.join(format!("{stem} ({n}).png"))
        };
        if file.exists() {
            continue;
        }
        let part = dir.join(format!(".{stem} ({n}).png.part"));
        if std::fs::write(&part, picture).is_err() {
            let _ = std::fs::remove_file(&part);
            return None;
        }
        return match std::fs::rename(&part, &file) {
            Ok(()) => Some(file),
            Err(_) => {
                let _ = std::fs::remove_file(&part);
                None
            }
        };
    }
    None
}

/// A folder as it is read: `~/Pictures/Screenshots`.
fn shown(dir: &Path) -> String {
    match dir.strip_prefix(crate::filer::home()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => dir.display().to_string(),
    }
}

/// A PICTURE of `aim` (on a thread): the clipboard first — it is what a
/// hand reaches for next, and it never waits for the file —, then the file,
/// then the notification that carries it.
fn shoot(aim: &Aim, pointer: bool) {
    let tmp = scratch_png();
    let grabbed = grab(aim, pointer, &tmp);
    let picture = if grabbed {
        std::fs::read(&tmp).ok()
    } else {
        None
    };
    let _ = std::fs::remove_file(&tmp);
    let Some(picture) = picture else {
        warn!("capture: no picture of {aim:?}");
        notify(tr("The screenshot could not be taken"), "", None);
        return;
    };
    let copied = copy("image/png", &picture);
    let dir = crate::record::shots_folder();
    let file = keep(&picture, &dir, SystemTime::now());
    let place = shown(&dir);
    info!("capture: a picture of {aim:?} → {file:?} (clipboard: {copied})");
    match (&file, copied) {
        (Some(file), true) => notify(
            tr("Screenshot saved"),
            &fill(tr("It is in {0} and on the clipboard."), &[("0", &place)]),
            Some(file),
        ),
        (Some(file), false) => notify(
            tr("Screenshot saved"),
            &fill(tr("It is in {0}."), &[("0", &place)]),
            Some(file),
        ),
        (None, true) => notify(
            tr("The screenshot is on the clipboard"),
            &fill(tr("It could not be saved in {0}."), &[("0", &place)]),
            None,
        ),
        (None, false) => notify(tr("The screenshot could not be taken"), "", None),
    }
}

/// THE PLUS (on a thread): the words in `aim`, read and put on the
/// clipboard.
fn read_text(aim: &Aim) {
    let tmp = scratch_png();
    if !grab(aim, false, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        warn!("capture: no picture of {aim:?} to read");
        notify(tr("The text could not be read"), "", None);
        return;
    }
    read_picture(&tmp, &format!("{aim:?}"));
    let _ = std::fs::remove_file(&tmp);
}

/// The words in the picture `tmp` (a PNG; `of` names it in the log), read
/// and put on the clipboard: the live tool's and the editor's Copy text.
pub(crate) fn read_picture(tmp: &Path, of: &str) {
    let listed = Command::new(tesseract())
        .arg("--list-langs")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let listed = listed
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let langs = pick_langs(crate::i18n::lang().as_deref(), &listed);
    let read = Command::new(tesseract())
        .args(tesseract_args(tmp, &langs))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let text = match read {
        Ok(out) if out.status.success() => tidy(&String::from_utf8_lossy(&out.stdout)),
        other => {
            warn!(
                "capture: the text was not read ({:?})",
                other.map(|o| o.status)
            );
            notify(tr("The text could not be read"), "", None);
            return;
        }
    };
    info!(
        "capture: {} characters read in {of} ({})",
        text.chars().count(),
        langs.join("+")
    );
    if text.is_empty() {
        notify(tr("No text was found there"), "", None);
    } else if copy("text/plain;charset=utf-8", text.as_bytes()) {
        notify(tr("Text copied"), &first_words(&text, 90), None);
    } else {
        notify(tr("The text could not be copied"), "", None);
    }
}

impl App {
    /// Whether the tool is up at all (a countdown and the empty frame too).
    pub(crate) fn capture_up(&self) -> bool {
        self.capture_tool.tool.is_some()
    }

    /// Whether it draws anything now.
    pub(crate) fn capture_shows(&self) -> bool {
        self.capture_tool
            .tool
            .as_ref()
            .is_some_and(|t| !matches!(t.phase, Phase::Hiding { .. }))
    }

    /// Whether the banner's pills stand (`capture_pill.rs`): while the tool
    /// shows, a countdown included; never while it waits to be off the screen.
    pub(crate) fn capture_pills_on(&self) -> bool {
        self.capture_shows()
    }

    /// A frame of the top surface was drawn: the EMPTY one, when the tool
    /// waits to be off the screen (its showing is what the picture waits for).
    pub(crate) fn capture_drawn_away(&mut self) {
        if let Some(tool) = self
            .capture_tool
            .tool
            .as_mut()
            .filter(|t| matches!(t.phase, Phase::Hiding { .. }))
        {
            tool.cleared = true;
        }
    }

    /// Whether the keys are the tool's now: all of them while something is
    /// chosen (its surface holds the keyboard), Escape during a countdown.
    pub(crate) fn capture_takes_key(&self, keysym: Keysym) -> bool {
        match self.capture_tool.tool.as_ref().map(|t| &t.phase) {
            Some(Phase::Choosing) => true,
            Some(Phase::Counting { .. }) => keysym == Keysym::Escape,
            _ => false,
        }
    }

    /// `waverunner-ctl capture …` (the module's head lists the verbs).
    pub(crate) fn capture_command(&mut self, what: &str) -> String {
        let mut words = what.split_whitespace();
        let (verb, arg) = (words.next().unwrap_or("toggle"), words.next().unwrap_or(""));
        let on_off = |arg: &str| match arg {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        };
        match (verb, arg) {
            ("toggle", _) => {
                if self.capture_up() {
                    self.capture_close();
                } else {
                    self.capture_open();
                }
            }
            ("open", _) => {
                if !self.capture_up() {
                    self.capture_open();
                }
            }
            ("close", _) => self.capture_close(),
            ("state", _) => {}
            // THE KEY and its screenshot (`capture_shot.rs`).
            ("press", _) => self.capture_key_press(),
            ("release", _) => self.capture_key_release(),
            ("shot", _) => {
                self.capture_key_press();
                self.capture_key_release();
            }
            ("preview", "click") | ("edit", _) => {
                if !self.capture_edit() {
                    return format!("no preview is out ({})", self.capture_state());
                }
            }
            ("preview", "hover") => self.capture_preview_hover(true),
            ("preview", "unhover") => self.capture_preview_hover(false),
            ("preview", "close") => {
                if !self.capture_preview_close() {
                    return format!("no preview is out ({})", self.capture_state());
                }
            }
            ("preview", _) => {}
            ("pills", _) => return self.capture_pill_state(),
            ("what", "region") => self.capture_set_what(What::Region),
            ("what", "window") => self.capture_set_what(What::Window),
            ("what", "screen") => self.capture_set_what(What::Screen),
            ("kind", "picture") => self.capture_set_kind(Kind::Picture),
            ("kind", "video") => self.capture_set_kind(Kind::Video),
            ("timer", n) if n.parse().is_ok_and(|n: u32| TIMERS.contains(&n)) => {
                self.capture_tool.prefs.timer = n.parse().unwrap_or(0);
                self.capture_changed();
            }
            ("pointer", arg) if on_off(arg).is_some() => {
                self.capture_tool.prefs.pointer = arg == "on";
                self.capture_changed();
            }
            ("sound", arg) if on_off(arg).is_some() => {
                self.capture_tool.prefs.sound = arg == "on";
                self.capture_changed();
            }
            ("rect", _) => {
                let n: Vec<i32> = what.split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
                let (Some(tool), &[x, y, w, h]) = (self.capture_tool.tool.as_mut(), n.as_slice()) else {
                    return "capture rect <x> <y> <w> <h> (with the tool open)".to_owned();
                };
                if w < 1 || h < 1 {
                    return "capture rect: nothing in it".to_owned();
                }
                let Some(area) = Area::new(x, y, w, h).clipped(tool.bounds()) else {
                    return "capture rect: off the screen".to_owned();
                };
                tool.region = Some(area);
                self.capture_tool.prefs.region = Some((tool.screen.name.clone(), area));
                self.capture_set_what(What::Region);
            }
            ("take", _) => {
                let job = if self.capture_tool.prefs.kind == Kind::Video { Job::Video } else { Job::Picture };
                if !self.capture_go(job) {
                    return format!("nothing to take ({})", self.capture_state());
                }
            }
            ("text", _) => {
                if !self.capture_tool.tool.as_ref().is_some_and(|t| t.ocr) {
                    return "no tesseract (or the tool is closed)".to_owned();
                }
                if !self.capture_go(Job::Text) {
                    return format!("nothing to read ({})", self.capture_state());
                }
            }
            _ => return "capture [toggle|open|close|state|what region|window|screen|kind picture|video|timer 0|3|10|pointer on|off|sound on|off|rect <x> <y> <w> <h>|take|text|pills]".to_owned(),
        }
        self.capture_state()
    }

    /// Everything about the tool, in one line.
    fn capture_state(&self) -> String {
        let p = &self.capture_tool.prefs;
        let kind = if p.kind == Kind::Video {
            "video"
        } else {
            "picture"
        };
        let on = |b: bool| if b { "on" } else { "off" };
        let rest = format!(
            "what {} kind {kind} timer {} pointer {} sound {}",
            p.what.name(),
            p.timer,
            on(p.pointer),
            on(p.sound)
        );
        let rest = format!("{rest} {}", self.capture_shot_state());
        let Some(tool) = self.capture_tool.tool.as_ref() else {
            return format!("closed {rest}");
        };
        let rest = format!(
            "mode {} {rest}",
            if tool.still.is_some() {
                "still"
            } else {
                "live"
            }
        );
        let phase = match &tool.phase {
            Phase::Choosing => "open".to_owned(),
            Phase::Counting { left, .. } => format!("counting {left}"),
            Phase::Hiding { .. } => "taking".to_owned(),
        };
        let rect = tool.selection(p.what).map_or_else(
            || "none".to_owned(),
            |a| format!("{} {} {} {}", a.x, a.y, a.w, a.h),
        );
        format!(
            "{phase} on {} {}x{} {rest} rect {rect} windows {} text {}",
            tool.screen.name,
            tool.screen.w,
            tool.screen.h,
            tool.wins.len(),
            on(tool.ocr)
        )
    }

    /// Open the tool on the screen the pointer is on.
    pub(crate) fn capture_open(&mut self) {
        let pointer = crate::hypr::cursor_pos();
        let monitors = crate::hypr::all_monitors();
        let under = pointer.and_then(|(x, y)| {
            monitors
                .iter()
                .find(|m| x >= m.x && x < m.x + m.w && y >= m.y && y < m.y + m.h)
                .cloned()
        });
        let Some(monitor) = under.or_else(|| crate::hypr::focused_monitor().ok()) else {
            warn!("capture: no screen to open on");
            return;
        };
        // (On another screen than the menus' surface: made again there first.)
        if self.capture_follow(&monitor.name) {
            return;
        }
        self.capture_open_on(&monitor.name);
    }

    /// Open it on screen `name`, where the top surface is: once that
    /// surface covers the screen whole.
    pub(crate) fn capture_open_on(&mut self, name: &str) {
        if !self.desktop_top_ready() {
            warn!("capture: no top surface to draw the tool on");
            return;
        }
        if self.capture_tool.opening.is_some() {
            return;
        }
        if self.capture_cover(true) {
            // (Its new size comes with a configure — `capture_surface_sized`;
            // none comes where nothing is reserved: then a moment later.)
            self.capture_tool.opening = Some(name.to_owned());
            self.capture_tool.epoch += 1;
            let epoch = self.capture_tool.epoch;
            self.after_ms(COVER_WITHIN, move |app| {
                if app.capture_tool.epoch == epoch {
                    app.capture_surface_sized();
                }
            });
            return;
        }
        self.capture_open_now(name);
    }

    /// The top surface over the WHOLE screen (the bar's strip too) while the
    /// tool is up, back inside what the bar leaves after. Whether it changed.
    fn capture_cover(&mut self, whole: bool) -> bool {
        if self.capture_tool.whole == whole {
            return false;
        }
        let Some(layer) = self.desktop_top.layer.as_ref() else {
            return false;
        };
        layer.set_exclusive_zone(if whole { -1 } else { 0 });
        layer.commit();
        let _ = self.conn.flush();
        self.capture_tool.whole = whole;
        true
    }

    /// The top surface has its size (again): the tool that waited for it
    /// opens; one that is up is laid out for it.
    pub(crate) fn capture_surface_sized(&mut self) {
        if let Some(name) = self.capture_tool.opening.take() {
            self.capture_open_now(&name);
        } else if let Some(tool) = self.capture_tool.tool.as_mut() {
            let (sw, sh) = self.desktop_top.size;
            (tool.screen.w, tool.screen.h) = (sw as i32, sh as i32);
            tool.region = tool.region.and_then(|a| a.clipped((sw as i32, sh as i32)));
            self.capture_changed();
        }
    }

    fn capture_open_now(&mut self, name: &str) {
        if self.capture_up() {
            return;
        }
        let Some(monitor) = crate::hypr::monitor_info(Some(name))
            .ok()
            .filter(|m| m.name == name)
        else {
            warn!("capture: screen {name} is gone");
            return;
        };
        // (The surface's own size and corner: the screen's when it covers it
        // whole, else what the bar and the like leave of it.)
        let (sw, sh) = self.desktop_top.size;
        let covers = (sw as f64 - monitor.w).abs() < 2.0 && (sh as f64 - monitor.h).abs() < 2.0;
        let (ox, oy) = if covers {
            (monitor.x, monitor.y)
        } else {
            (
                monitor.x + monitor.reserved.0,
                monitor.y + monitor.reserved.1,
            )
        };
        let screen = Screen {
            name: monitor.name.clone(),
            x: ox.round() as i32,
            y: oy.round() as i32,
            w: sw as i32,
            h: sh as i32,
        };
        let bounds = (screen.w, screen.h);
        // THE EDITOR: the windows are the ones that were on the screen when
        // the shot was taken, where they stood then (on this surface).
        let still = self.capture_tool.shots.opening.take();
        let wins = if let Some(kept) = still.as_ref() {
            let (dx, dy) = (kept.screen.x - screen.x, kept.screen.y - screen.y);
            kept.wins
                .iter()
                .filter_map(|a| Area::new(a.x + dx, a.y + dy, a.w, a.h).clipped(bounds))
                .collect()
        } else {
            Vec::new()
        };
        let live = still.is_none();
        let wins = if !live {
            wins
        } else {
            stack(
                crate::hypr::client_geometries()
                    .into_iter()
                    .filter(|c| !c.special && c.workspace == monitor.active_ws)
                    .filter_map(|c| {
                        let area = Area::new(
                            (c.x - ox).round() as i32,
                            (c.y - oy).round() as i32,
                            c.w.round() as i32,
                            c.h.round() as i32,
                        );
                        Some(Stacked {
                            area: area.clipped(bounds)?,
                            floating: c.floating,
                            fullscreen: c.fullscreen,
                            rank: c.focus_rank,
                        })
                    })
                    .collect(),
            )
        };
        // (Its pixels into the surface's one big picture, once.)
        if let (Some(kept), Some(renderer)) = (still.as_ref(), self.desktop_top.renderer.as_mut()) {
            renderer.set_video_frame(kept.px.0, kept.px.1, &kept.rgba);
            self.capture_tool.still_up = true;
        }
        let region = self
            .capture_tool
            .prefs
            .region
            .as_ref()
            .filter(|(on, _)| *on == screen.name)
            .and_then(|(_, a)| a.clipped(bounds));
        // (A screenshot's preview and the picture hanging from it go at
        // once: nothing of ours may be in what the tool takes.)
        self.capture_preview_off_now();
        // (A menu up would be under it, and its keys: it goes.)
        self.desktop.menu = None;
        self.desktop.props = None;
        self.capture_tool.epoch += 1;
        let ptr = pointer_on(&screen);
        let mut tool = Tool {
            screen,
            wins,
            win: None,
            region,
            drag: None,
            ptr: None,
            hover: None,
            pressed: None,
            last_press: None,
            phase: Phase::Choosing,
            ocr: can_read_text(),
            bar_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            strip: 0,
            cleared: false,
            top_shown: false,
            bar_drawn: false,
            bar_shown: false,
            still,
        };
        // (The banner's strip, where this surface covers it: the bar's own
        // height where it stands on this screen, else what is kept at the top.)
        let bar_here = self.bar_screen_name().is_none_or(|n| n == tool.screen.name);
        tool.strip = if covers && bar_here {
            self.options_bar_h().ceil() as i32
        } else {
            0
        };
        // (The window under the pointer as it opens, else the topmost.)
        tool.win = ptr
            .and_then(|p| tool.window_at(p))
            .or((!tool.wins.is_empty()).then_some(0));
        info!(
            "capture: open {} on {} ({} windows, text: {})",
            if tool.still.is_some() {
                "over the shot"
            } else {
                "live"
            },
            tool.screen.name,
            tool.wins.len(),
            tool.ocr
        );
        self.capture_tool.tool = Some(tool);
        self.capture_changed();
    }

    /// Close it, whatever it was doing (a countdown is given up).
    pub(crate) fn capture_close(&mut self) {
        if self.capture_tool.tool.take().is_some() {
            info!("capture: closed");
            self.capture_tool.epoch += 1;
            self.capture_changed();
        }
    }

    fn capture_set_what(&mut self, what: What) {
        self.capture_tool.prefs.what = what;
        if let Some(tool) = self.capture_tool.tool.as_mut() {
            tool.drag = None;
        }
        self.capture_changed();
    }

    fn capture_set_kind(&mut self, kind: Kind) {
        // (A still is a picture: nothing of a film in the editor.)
        if self
            .capture_tool
            .tool
            .as_ref()
            .is_some_and(|t| t.still.is_some())
        {
            return;
        }
        self.capture_tool.prefs.kind = kind;
        self.capture_changed();
    }

    /// Something of the tool changed: its hold on the keyboard and the
    /// pointer follows, and it is drawn.
    pub(crate) fn capture_changed(&mut self) {
        if self.capture_tool.tool.is_none() && self.capture_tool.opening.is_none() {
            self.capture_cover(false);
        }
        // (The editor's still goes with it.)
        if self.capture_tool.tool.is_none() && std::mem::take(&mut self.capture_tool.still_up) {
            if let Some(renderer) = self.desktop_top.renderer.as_mut() {
                renderer.drop_video();
            }
        }
        self.capture_layout();
        self.capture_hold();
        self.capture_cursor();
        self.capture_pill_sync();
        self.request_desktop_top_draw();
    }

    /// The surface's hold on the keyboard and the pointer, as the tool's
    /// phase wants it.
    fn capture_hold(&mut self) {
        let want = match self.capture_tool.tool.as_ref() {
            Some(Tool {
                phase: Phase::Choosing,
                strip,
                ..
            }) => Held::All(*strip),
            Some(Tool {
                phase: Phase::Counting { .. },
                ..
            }) => Held::Keys,
            _ => Held::Nothing,
        };
        let had = std::mem::replace(&mut self.capture_tool.held, want);
        if had == want {
            return;
        }
        let (w, h) = self.desktop_top.size;
        let Some(layer) = self.desktop_top.layer.as_ref() else {
            return;
        };
        let rects: Vec<(i32, i32, i32, i32)> = match want {
            Held::All(strip) => vec![(0, strip, w as i32, h as i32 - strip)],
            _ => Vec::new(),
        };
        crate::surface::set_input_rects(&self.compositor, layer, &rects);
        self.desktop_top.input_on = false;
        match (want, had) {
            // The keyboard is fetched with an exclusive grab and kept on
            // demand once it is here (`capture_keys_arrived`): an exclusive
            // layer has the pointer pinned to it too, and the banner's pills
            // must take their clicks.
            (Held::All(_), Held::Nothing) => {
                crate::surface::set_interactive(layer, true);
                self.capture_tool.grab = true;
                self.cancel_keyboard_handback(crate::KbSurface::Capture);
                self.after_ms(GRAB_WITHIN, |app| app.capture_keys_arrived());
            }
            (Held::Nothing, _) => {
                crate::surface::set_interactive(layer, false);
                self.capture_tool.grab = false;
                // The keyboard goes back to the window it was taken from —
                // but only AFTER a picture: giving it back is a focus call,
                // and where that moves the focus (no plugin: a bounce through
                // a neighbour) a fullscreen window would leave fullscreen
                // under the picture being taken. Nothing has the keyboard
                // meanwhile, which changes nothing on the screen.
                let picture = self.capture_tool.tool.as_ref().is_some_and(|t| {
                    matches!(
                        t.phase,
                        Phase::Hiding {
                            job: Job::Picture | Job::Text,
                            ..
                        }
                    )
                });
                if picture {
                    self.after_ms(KEYS_BACK_AFTER, |app| {
                        if app.capture_tool.held == Held::Nothing {
                            app.kb_worker.hand_back(None);
                        }
                    });
                } else {
                    self.begin_keyboard_handback(crate::KbSurface::Capture, None);
                }
            }
            // (Between choosing and counting the keyboard stays as it is.)
            _ => {}
        }
        let _ = self.conn.flush();
        debug!("capture: holds {want:?}");
    }

    /// The keyboard has arrived on the tool's surface (or was given long
    /// enough to): the exclusive grab that fetched it is let go, and it is
    /// kept on demand — until a window is clicked.
    pub(crate) fn capture_keys_arrived(&mut self) {
        if !std::mem::take(&mut self.capture_tool.grab) || self.capture_tool.held == Held::Nothing {
            return;
        }
        if let Some(layer) = self.desktop_top.layer.as_ref() {
            crate::surface::set_on_demand(layer);
        }
        let _ = self.conn.flush();
        debug!("capture: has the keyboard; the grab is let go");
    }

    /// The main action (`job`: a picture or a film), or Copy text: after
    /// the timer when one is set. False when nothing is chosen.
    fn capture_go(&mut self, job: Job) -> bool {
        let (what, timer) = (self.capture_tool.prefs.what, self.capture_tool.prefs.timer);
        let Some(tool) = self
            .capture_tool
            .tool
            .as_mut()
            .filter(|t| t.phase == Phase::Choosing)
        else {
            return false;
        };
        // THE EDITOR: nothing to wait for and nothing to hide — the picture
        // is in hand. Save cuts it (the whole screen: it stays as it is);
        // Copy text reads the selection of the still.
        if let Some(kept) = tool.still.clone() {
            let Some(area) = tool.selection(what) else {
                return false;
            };
            let (dx, dy) = (tool.screen.x - kept.screen.x, tool.screen.y - kept.screen.y);
            let area = Area::new(area.x + dx, area.y + dy, area.w, area.h);
            let whole = what == What::Screen;
            self.capture_tool.tool = None;
            self.capture_tool.epoch += 1;
            match job {
                Job::Text => self.capture_still_text(kept, area),
                _ if whole => info!("capture: the shot stays as it was taken"),
                _ => self.capture_still_save(kept, area),
            }
            self.capture_changed();
            return true;
        }
        let Some(aim) = tool.aim(what, job) else {
            return false;
        };
        tool.drag = None;
        if timer > 0 {
            tool.phase = Phase::Counting { job, left: timer };
            self.capture_tool.epoch += 1;
            self.capture_count_arm();
        } else {
            self.capture_hide(job, aim);
        }
        self.capture_changed();
        true
    }

    /// A second of the countdown, a second from now (only while it counts).
    fn capture_count_arm(&mut self) {
        let epoch = self.capture_tool.epoch;
        self.after_ms(1000, move |app| {
            if app.capture_tool.epoch != epoch {
                return;
            }
            let what = app.capture_tool.prefs.what;
            let Some(tool) = app.capture_tool.tool.as_mut() else {
                return;
            };
            let Phase::Counting { job, left } = tool.phase else {
                return;
            };
            if left > 1 {
                tool.phase = Phase::Counting {
                    job,
                    left: left - 1,
                };
                app.capture_count_arm();
            } else if let Some(aim) = tool.aim(what, job) {
                app.capture_hide(job, aim);
            } else {
                app.capture_tool.tool = None;
            }
            app.capture_changed();
        });
    }

    /// Draw the tool away; `aim` is taken once that frame was shown.
    fn capture_hide(&mut self, job: Job, aim: Aim) {
        let Some(tool) = self.capture_tool.tool.as_mut() else {
            return;
        };
        tool.phase = Phase::Hiding { job, aim };
        (tool.cleared, tool.top_shown, tool.bar_drawn, tool.bar_shown) =
            (false, false, false, false);
        self.capture_tool.epoch += 1;
        let epoch = self.capture_tool.epoch;
        // The banner's pills go in the same step, at once (no easing: a
        // region or a screen can have the banner in it), and a banner that
        // was called over a fullscreen window goes whole.
        self.capture_pills_off_now();
        // (No frame callback comes for a surface nobody sees: then anyway.)
        self.after_ms(HIDE_WITHIN, move |app| {
            if app.capture_tool.epoch == epoch {
                app.capture_fire();
            }
        });
    }

    /// The banner drew a frame (`presented`: it went to the screen, and its
    /// callback will say when it was shown; else nothing of it had changed).
    pub(crate) fn capture_bar_drawn(&mut self, presented: bool) {
        let Some(tool) = self
            .capture_tool
            .tool
            .as_mut()
            .filter(|t| matches!(t.phase, Phase::Hiding { .. }))
        else {
            return;
        };
        tool.bar_drawn = true;
        if !presented {
            tool.bar_shown = true;
            self.capture_fire_when_gone();
        }
    }

    /// The banner's frame was shown: the one without the pills, when the tool
    /// is waiting for it.
    pub(crate) fn capture_bar_shown(&mut self) {
        if let Some(tool) = self
            .capture_tool
            .tool
            .as_mut()
            .filter(|t| t.bar_drawn && matches!(t.phase, Phase::Hiding { .. }))
        {
            tool.bar_shown = true;
            self.capture_fire_when_gone();
        }
    }

    /// Take it once BOTH the tool's surface and the banner were shown without
    /// anything of the tool.
    fn capture_fire_when_gone(&mut self) {
        if self
            .capture_tool
            .tool
            .as_ref()
            .is_some_and(|t| t.top_shown && t.bar_shown)
        {
            self.capture_fire();
        }
    }

    /// The top surface's frame was shown: the empty one, when the tool is
    /// waiting for it.
    pub(crate) fn capture_frame_shown(&mut self) {
        if let Some(tool) = self
            .capture_tool
            .tool
            .as_mut()
            .filter(|t| t.cleared && matches!(t.phase, Phase::Hiding { .. }))
        {
            tool.top_shown = true;
            self.capture_fire_when_gone();
        }
    }

    /// The top surface went (its screen did), or was made again: what the
    /// tool held of it went with it.
    pub(crate) fn capture_surface_gone(&mut self, close: bool) {
        self.capture_tool.held = Held::Nothing;
        self.capture_tool.grab = false;
        // (A surface made again is inside the bar's strip, as made.)
        self.capture_tool.whole = false;
        if close && self.capture_tool.tool.take().is_some() {
            self.capture_tool.epoch += 1;
        }
    }

    /// The tool is off the screen: take what it aimed at, off the loop.
    fn capture_fire(&mut self) {
        let Some(Tool {
            phase: Phase::Hiding { job, aim },
            ..
        }) = self.capture_tool.tool.take()
        else {
            return;
        };
        self.capture_tool.epoch += 1;
        let (pointer, sound) = (
            self.capture_tool.prefs.pointer,
            self.capture_tool.prefs.sound,
        );
        info!("capture: taking {aim:?} as {job:?}");
        match job {
            Job::Video => self.record_start_at(aim, sound),
            Job::Picture | Job::Text => {
                let spawned = std::thread::Builder::new()
                    .name("capture".into())
                    .spawn(move || match job {
                        Job::Text => read_text(&aim),
                        _ => shoot(&aim, pointer),
                    });
                if let Err(e) = spawned {
                    warn!("capture: cannot take it ({e})");
                }
            }
        }
        self.capture_changed();
    }

    /// A control of the tool, clicked: a pill of the banner, or the button by
    /// the selection.
    pub(crate) fn capture_press_ctl(&mut self, ctl: Ctl) {
        // (During a countdown nothing but the round pill answers.)
        if self
            .capture_tool
            .tool
            .as_ref()
            .is_some_and(|t| matches!(t.phase, Phase::Counting { .. }))
        {
            return;
        }
        let prefs = &mut self.capture_tool.prefs;
        match ctl {
            Ctl::What(what) => return self.capture_set_what(what),
            Ctl::Video => {
                let kind = if prefs.kind == Kind::Video {
                    Kind::Picture
                } else {
                    Kind::Video
                };
                return self.capture_set_kind(kind);
            }
            Ctl::Timer => {
                let i = TIMERS.iter().position(|t| *t == prefs.timer).unwrap_or(0);
                prefs.timer = TIMERS[(i + 1) % TIMERS.len()];
            }
            Ctl::Sound => prefs.sound = !prefs.sound,
            Ctl::Text => {
                self.capture_go(Job::Text);
                return;
            }
            Ctl::Main => {
                let job = if prefs.kind == Kind::Video {
                    Job::Video
                } else {
                    Job::Picture
                };
                self.capture_go(job);
                return;
            }
        }
        self.capture_changed();
    }

    /// A key while the tool is up.
    pub(crate) fn capture_key(&mut self, keysym: Keysym) {
        let Some(tool) = self.capture_tool.tool.as_mut() else {
            return;
        };
        // (Escape during a countdown gives the countdown up, not the tool.)
        if let Phase::Counting { .. } = tool.phase {
            if keysym == Keysym::Escape {
                tool.phase = Phase::Choosing;
                self.capture_tool.epoch += 1;
                self.capture_changed();
            }
            return;
        }
        let what = self.capture_tool.prefs.what;
        let step = if self.modifiers.shift { 10 } else { 1 };
        let nudge = match keysym {
            Keysym::Left | Keysym::KP_Left => Some((-step, 0)),
            Keysym::Right | Keysym::KP_Right => Some((step, 0)),
            Keysym::Up | Keysym::KP_Up => Some((0, -step)),
            Keysym::Down | Keysym::KP_Down => Some((0, step)),
            _ => None,
        };
        match keysym {
            Keysym::Escape => self.capture_close(),
            Keysym::Tab | Keysym::ISO_Left_Tab => self.capture_set_what(
                what.step(keysym == Keysym::ISO_Left_Tab || self.modifiers.shift),
            ),
            Keysym::v | Keysym::V => {
                let kind = if self.capture_tool.prefs.kind == Kind::Picture {
                    Kind::Video
                } else {
                    Kind::Picture
                };
                self.capture_set_kind(kind);
            }
            Keysym::Return | Keysym::KP_Enter => self.capture_press_ctl(Ctl::Main),
            _ => {
                let (Some((dx, dy)), What::Region, Some(area)) = (nudge, what, tool.region) else {
                    return;
                };
                let moved =
                    Area::new(area.x + dx, area.y + dy, area.w, area.h).kept_on(tool.bounds());
                tool.region = Some(moved);
                self.capture_tool.prefs.region = Some((tool.screen.name.clone(), moved));
                self.capture_changed();
            }
        }
    }

    /// The pointer on the top surface while the tool is up.
    pub(crate) fn capture_pointer(&mut self, event: &wl_pointer::Event) {
        match *event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                // (The compositor forgot our cursor on the last leave.)
                self.enter_serial = serial;
                self.cursor_now = None;
                self.capture_motion((surface_x as f32, surface_y as f32));
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => self.capture_motion((surface_x as f32, surface_y as f32)),
            wl_pointer::Event::Leave { .. } => {
                self.pointer_surface = crate::options::PointerSurface::Dock;
                if let Some(tool) = self.capture_tool.tool.as_mut() {
                    tool.ptr = None;
                    tool.pressed = None;
                    if tool.hover.take().is_some() {
                        self.request_desktop_top_draw();
                    }
                }
            }
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(state),
                ..
            } if button == crate::BTN_LEFT => {
                if state == wl_pointer::ButtonState::Pressed {
                    self.capture_press();
                } else {
                    self.capture_release();
                }
            }
            _ => {}
        }
    }

    fn capture_motion(&mut self, p: (f32, f32)) {
        let what = self.capture_tool.prefs.what;
        let Some(tool) = self.capture_tool.tool.as_mut() else {
            return;
        };
        tool.ptr = Some(p);
        let mut changed = false;
        if let Some(drag) = tool.drag.as_mut() {
            let to = (p.0.round() as i32, p.1.round() as i32);
            drag.moved |=
                (to.0 - drag.from.0).abs() >= DRAG_FROM || (to.1 - drag.from.1).abs() >= DRAG_FROM;
            if drag.moved {
                let (start, grip, from) = (drag.start, drag.grip, drag.from);
                tool.region = Some(dragged(start, grip, from, to, tool.bounds()));
                changed = true;
            }
        } else {
            let hover = tool.ctl_at(p);
            changed |= std::mem::replace(&mut tool.hover, hover) != hover;
            // (The window under the pointer; over the toolbar or bare
            // wallpaper the last one stays outlined.)
            if what == What::Window && tool.phase == Phase::Choosing && !tool.bar_rect.contains(p) {
                if let Some(win) = tool.window_at(p) {
                    changed |= tool.win.replace(win) != Some(win);
                }
            }
        }
        self.capture_cursor();
        if changed {
            self.request_desktop_top_draw();
        }
    }

    fn capture_press(&mut self) {
        let what = self.capture_tool.prefs.what;
        let Some(tool) = self.capture_tool.tool.as_mut() else {
            return;
        };
        let Some(p) = tool.ptr else {
            return;
        };
        if tool.bar_rect.contains(p) || tool.phase != Phase::Choosing {
            tool.pressed = tool.ctl_at(p);
            return;
        }
        match what {
            What::Region => {
                let at = (p.0.round() as i32, p.1.round() as i32);
                let grip = tool.region.map_or(Grip::New, |a| grip_at(a, p, REACH));
                // A double click inside takes it.
                let again = tool
                    .last_press
                    .replace(Instant::now())
                    .is_some_and(|t| t.elapsed() < DOUBLE_CLICK);
                if again && grip == Grip::Move {
                    tool.last_press = None;
                    return self.capture_press_ctl(Ctl::Main);
                }
                let start = tool.region.unwrap_or(Area::new(at.0, at.1, 1, 1));
                tool.drag = Some(Drag {
                    grip,
                    from: at,
                    start,
                    before: tool.region,
                    moved: false,
                });
            }
            What::Window => {
                if let Some(win) = tool.window_at(p) {
                    tool.win = Some(win);
                    self.capture_press_ctl(Ctl::Main);
                }
            }
            What::Screen => self.capture_press_ctl(Ctl::Main),
        }
    }

    fn capture_release(&mut self) {
        let Some(tool) = self.capture_tool.tool.as_mut() else {
            return;
        };
        if let Some(drag) = tool.drag.take() {
            if !drag.moved {
                tool.region = drag.before;
            }
            if let Some(area) = tool.region {
                self.capture_tool.prefs.region = Some((tool.screen.name.clone(), area));
            }
            return self.capture_changed();
        }
        let pressed = tool.pressed.take();
        let over = tool.ptr.and_then(|p| tool.ctl_at(p));
        if let Some(ctl) = pressed.filter(|c| Some(*c) == over) {
            self.capture_press_ctl(ctl);
        }
    }

    /// The pointer's shape over the tool: the index over a control, a
    /// region's own arrows over its handles, the cross where one is drawn.
    fn capture_cursor(&mut self) {
        if self.pointer_surface != crate::options::PointerSurface::DesktopTop {
            return;
        }
        let what = self.capture_tool.prefs.what;
        let Some(tool) = self.capture_tool.tool.as_ref() else {
            return;
        };
        let Some(p) = tool.ptr else {
            return;
        };
        let shape = if let Some(drag) = tool.drag.as_ref() {
            drag.grip.shape()
        } else if tool.bar_rect.contains(p) || tool.phase != Phase::Choosing {
            if tool.ctl_at(p).is_some() {
                Shape::Pointer
            } else {
                Shape::Default
            }
        } else {
            match what {
                What::Region => tool
                    .region
                    .map_or(Shape::Crosshair, |a| grip_at(a, p, REACH).shape()),
                What::Window | What::Screen => Shape::Pointer,
            }
        };
        if self.cursor_now != Some(shape) {
            if let Some(device) = &self.cursor_device {
                device.set_shape(self.enter_serial, shape);
            }
            self.cursor_now = Some(shape);
        }
    }
}

/// Where the pointer is on `screen`, when it is on it.
fn pointer_on(screen: &Screen) -> Option<(f32, f32)> {
    let (x, y) = crate::hypr::cursor_pos()?;
    let p = ((x - screen.x as f64) as f32, (y - screen.y as f64) as f32);
    Area::new(0, 0, screen.w, screen.h).contains(p).then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: (i32, i32) = (1440, 900);

    fn screen() -> Screen {
        Screen {
            name: "MB".into(),
            x: 1920,
            y: 0,
            w: 1440,
            h: 900,
        }
    }

    fn tool() -> Tool {
        Tool {
            screen: screen(),
            wins: vec![Area::new(100, 100, 301, 201), Area::new(0, 28, 1440, 872)],
            win: Some(0),
            region: Some(Area::new(10, 20, 641, 481)),
            drag: None,
            ptr: None,
            hover: None,
            pressed: None,
            last_press: None,
            phase: Phase::Choosing,
            ocr: false,
            bar_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            strip: 0,
            cleared: false,
            top_shown: false,
            bar_drawn: false,
            bar_shown: false,
            still: None,
        }
    }

    #[test]
    fn a_region_is_held_by_its_corners_its_sides_and_its_inside() {
        let a = Area::new(100, 100, 400, 300);
        assert_eq!(grip_at(a, (100.0, 100.0), REACH), Grip::NW);
        assert_eq!(grip_at(a, (503.0, 98.0), REACH), Grip::NE);
        assert_eq!(grip_at(a, (96.0, 404.0), REACH), Grip::SW);
        assert_eq!(grip_at(a, (500.0, 400.0), REACH), Grip::SE);
        assert_eq!(grip_at(a, (300.0, 102.0), REACH), Grip::N);
        assert_eq!(grip_at(a, (300.0, 396.0), REACH), Grip::S);
        assert_eq!(grip_at(a, (104.0, 250.0), REACH), Grip::W);
        assert_eq!(grip_at(a, (497.0, 250.0), REACH), Grip::E);
        assert_eq!(grip_at(a, (300.0, 250.0), REACH), Grip::Move);
        assert_eq!(grip_at(a, (50.0, 250.0), REACH), Grip::New);
        assert_eq!(grip_at(a, (300.0, 450.0), REACH), Grip::New);
        // Every handle is where its grip is found.
        for (grip, at) in handles(a) {
            assert_eq!(grip_at(a, at, REACH), grip);
        }
        // A tiny region is still held by both its ends.
        let small = Area::new(100, 100, 6, 6);
        assert_eq!(grip_at(small, (100.0, 100.0), REACH), Grip::NW);
        assert_eq!(grip_at(small, (106.0, 106.0), REACH), Grip::SE);
    }

    #[test]
    fn a_drag_resizes_moves_or_draws_and_never_leaves_the_screen() {
        let a = Area::new(100, 100, 400, 300);
        // A side follows the pointer; a corner both ways.
        assert_eq!(
            dragged(a, Grip::E, (500, 250), (560, 300), BOUNDS),
            Area::new(100, 100, 460, 300)
        );
        assert_eq!(
            dragged(a, Grip::N, (300, 100), (300, 60), BOUNDS),
            Area::new(100, 60, 400, 340)
        );
        assert_eq!(
            dragged(a, Grip::SE, (500, 400), (520, 450), BOUNDS),
            Area::new(100, 100, 420, 350)
        );
        assert_eq!(
            dragged(a, Grip::NW, (100, 100), (150, 120), BOUNDS),
            Area::new(150, 120, 350, 280)
        );
        // Past the opposite side it turns over, never negative.
        assert_eq!(
            dragged(a, Grip::W, (100, 250), (600, 250), BOUNDS),
            Area::new(500, 100, 100, 300)
        );
        // Onto the opposite side: one pixel is left, not none.
        assert_eq!(dragged(a, Grip::E, (500, 250), (100, 250), BOUNDS).w, 1);
        // The inside moves it whole and stops at the screen's edges.
        assert_eq!(
            dragged(a, Grip::Move, (300, 250), (310, 240), BOUNDS),
            Area::new(110, 90, 400, 300)
        );
        assert_eq!(
            dragged(a, Grip::Move, (300, 250), (-900, 5000), BOUNDS),
            Area::new(0, 600, 400, 300)
        );
        // Outside a new one spans the two points, whichever way.
        assert_eq!(
            dragged(a, Grip::New, (700, 500), (600, 450), BOUNDS),
            Area::new(600, 450, 100, 50)
        );
        // The pointer off the screen: cut to it.
        assert_eq!(
            dragged(a, Grip::SE, (500, 400), (5000, 5000), BOUNDS),
            Area::new(100, 100, 1340, 800)
        );
        assert_eq!(
            dragged(a, Grip::New, (1400, 880), (2000, 2000), BOUNDS),
            Area::new(1400, 880, 40, 20)
        );
    }

    #[test]
    fn a_rectangle_is_cut_to_the_screen_or_kept_on_it() {
        assert_eq!(
            Area::new(-50, -20, 200, 100).clipped(BOUNDS),
            Some(Area::new(0, 0, 150, 80))
        );
        assert_eq!(
            Area::new(1400, 880, 200, 100).clipped(BOUNDS),
            Some(Area::new(1400, 880, 40, 20))
        );
        assert_eq!(Area::new(2000, 0, 200, 100).clipped(BOUNDS), None);
        assert_eq!(
            Area::new(1400, -5, 200, 100).kept_on(BOUNDS),
            Area::new(1240, 0, 200, 100)
        );
        assert_eq!(
            Area::new(0, 0, 3000, 100).kept_on(BOUNDS),
            Area::new(0, 0, 1440, 100)
        );
        assert_eq!(Area::new(3, 5, 641, 481).even(), Area::new(3, 5, 640, 480));
        assert_eq!(Area::new(3, 5, 1, 1).even(), Area::new(3, 5, 2, 2));
    }

    #[test]
    fn a_rectangle_is_said_in_the_layouts_own_place() {
        assert_eq!(
            geometry(&screen(), Area::new(10, 20, 640, 480)),
            "1930,20 640x480"
        );
        let left = Screen {
            name: "TV".into(),
            x: -1920,
            y: 120,
            w: 1920,
            h: 1080,
        };
        assert_eq!(
            geometry(&left, Area::new(0, 0, 1920, 1080)),
            "-1920,120 1920x1080"
        );
    }

    #[test]
    fn what_is_taken_is_the_region_the_window_or_the_screen_by_its_name() {
        let t = tool();
        assert_eq!(
            t.aim(What::Region, Job::Picture),
            Some(Aim::Area("1930,20 641x481".into()))
        );
        // (A film's sides are even.)
        assert_eq!(
            t.aim(What::Region, Job::Video),
            Some(Aim::Area("1930,20 640x480".into()))
        );
        assert_eq!(
            t.aim(What::Window, Job::Text),
            Some(Aim::Area("2020,100 301x201".into()))
        );
        assert_eq!(
            t.aim(What::Screen, Job::Video),
            Some(Aim::Screen("MB".into()))
        );
        let none = Tool {
            region: None,
            win: None,
            ..tool()
        };
        assert_eq!(none.aim(What::Region, Job::Picture), None);
        assert_eq!(none.aim(What::Window, Job::Picture), None);
        assert!(none.aim(What::Screen, Job::Picture).is_some());
    }

    #[test]
    fn the_window_under_the_pointer_is_the_topmost_there() {
        let wins = stack(vec![
            Stacked {
                area: Area::new(0, 28, 720, 872),
                floating: false,
                fullscreen: false,
                rank: 0,
            },
            Stacked {
                area: Area::new(200, 200, 300, 200),
                floating: true,
                fullscreen: false,
                rank: 2,
            },
            Stacked {
                area: Area::new(250, 250, 300, 200),
                floating: true,
                fullscreen: false,
                rank: 1,
            },
        ]);
        assert_eq!(
            wins[0],
            Area::new(250, 250, 300, 200),
            "floating over tiled, the one used last first"
        );
        assert_eq!(wins[2], Area::new(0, 28, 720, 872));
        let t = Tool { wins, ..tool() };
        assert_eq!(t.window_at((260.0, 260.0)), Some(0));
        assert_eq!(t.window_at((210.0, 210.0)), Some(1));
        assert_eq!(t.window_at((20.0, 500.0)), Some(2));
        assert_eq!(t.window_at((1000.0, 500.0)), None);
        let full = stack(vec![
            Stacked {
                area: Area::new(200, 200, 300, 200),
                floating: true,
                fullscreen: false,
                rank: 0,
            },
            Stacked {
                area: Area::new(0, 0, 1440, 900),
                floating: false,
                fullscreen: true,
                rank: 1,
            },
        ]);
        assert_eq!(
            full[0],
            Area::new(0, 0, 1440, 900),
            "a fullscreen window is over everything"
        );
    }

    #[test]
    fn tab_walks_region_window_screen_round() {
        assert_eq!(What::Region.step(false), What::Window);
        assert_eq!(What::Window.step(false), What::Screen);
        assert_eq!(What::Screen.step(false), What::Region);
        assert_eq!(What::Region.step(true), What::Screen);
        assert_eq!(What::Window.step(true), What::Region);
    }

    #[test]
    fn grim_is_asked_for_a_rectangle_or_a_screen_and_the_pointer_when_wanted() {
        let args = |aim: &Aim, pointer| {
            grim_args(aim, pointer, Path::new("/tmp/x.png"))
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            args(&Aim::Area("1930,20 640x480".into()), false),
            ["-g", "1930,20 640x480", "/tmp/x.png"]
        );
        assert_eq!(
            args(&Aim::Area("0,0 10x10".into()), true),
            ["-c", "-g", "0,0 10x10", "/tmp/x.png"]
        );
        assert_eq!(
            args(&Aim::Screen("HDMI-A-1".into()), true),
            ["-c", "-o", "HDMI-A-1", "/tmp/x.png"]
        );
    }

    #[test]
    fn text_is_read_in_the_shells_language_and_english_when_their_data_are_there() {
        let listed =
            "List of available languages in \"/usr/share/tessdata/\" (4):\neng\nosd\nspa\ndeu\n";
        assert_eq!(pick_langs(Some("es_ES"), listed), ["spa", "eng"]);
        assert_eq!(pick_langs(Some("en"), listed), ["eng"]);
        assert_eq!(
            pick_langs(Some("fr"), listed),
            ["eng"],
            "no French data: English alone"
        );
        assert_eq!(pick_langs(None, listed), ["eng"]);
        // Neither the shell's nor English: whatever there is, but not osd.
        assert_eq!(
            pick_langs(Some("es"), "List of available languages (2):\nosd\nita\n"),
            ["ita"]
        );
        assert!(pick_langs(Some("es"), "").is_empty());
        let args = |langs: &[String]| {
            tesseract_args(Path::new("/tmp/x.png"), langs)
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            args(&["spa".to_owned(), "eng".to_owned()]),
            ["/tmp/x.png", "stdout", "-l", "spa+eng"]
        );
        assert_eq!(args(&[]), ["/tmp/x.png", "stdout"]);
    }

    #[test]
    fn the_text_read_is_tidied_and_said_by_its_first_words() {
        assert_eq!(
            tidy("\n\n  Hello there  \nworld\t\n\n\u{c}"),
            "  Hello there\nworld"
        );
        assert_eq!(tidy(" \n\u{c}"), "");
        assert_eq!(
            first_words("Hello\n  there   world", 90),
            "Hello there world"
        );
        assert_eq!(first_words("one two three four", 9), "one two t…");
    }

    #[test]
    fn the_notification_carries_the_picture_and_opens_it() {
        let with = notify_args(
            "Screenshot saved",
            "It is there.",
            Some(Path::new("/home/u/Pictures/Screenshots/a b.png")),
        );
        let tail = &with[9..];
        assert_eq!(
            tail[0], "/home/u/Pictures/Screenshots/a b.png",
            "its picture as the icon"
        );
        assert_eq!(&tail[1..3], ["Screenshot saved", "It is there."]);
        assert_eq!(
            &tail[3..5],
            ["2", "golem-open:/home/u/Pictures/Screenshots/a b.png"]
        );
        assert_eq!(
            &tail[6..],
            [
                "1",
                "image-path",
                "s",
                "/home/u/Pictures/Screenshots/a b.png",
                "6000"
            ]
        );
        let without = notify_args("The screenshot could not be taken", "", None);
        assert_eq!(&without[12..], ["0", "0", "6000"]);
        assert_eq!(without[6], "susssasa{sv}i");
    }

    #[test]
    fn a_second_picture_inside_one_second_is_kept_beside_the_first() {
        let dir = std::env::temp_dir().join(format!("wr-capture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_800_000);
        let first = keep(b"one", &dir, at).expect("kept");
        let second = keep(b"two", &dir, at).expect("kept");
        let name = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name(&first).starts_with("Screenshot 20") && name(&first).ends_with(".png"));
        assert!(name(&second).ends_with(" (2).png"), "{second:?}");
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        // Nothing half-written is left beside them.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

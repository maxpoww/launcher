//! FILES: Golem's file manager — a folder as a real window.
//!
//! Each window is an `xdg_toplevel` of the daemon's own (class
//! [`APP_ID`]), so it is tiled, minimised and carried between workspaces
//! like any app's, with its own renderer and icon array as the desktop has.
//! It shows ONE folder the way the desktop shows `~/Desktop` — the same
//! cell, carrier or thumbnail, and one-line name — flowing row by row and
//! scrolling, with the places down its left side and the way here along its
//! top.
//!
//! This is the first cut: looking and going. A click selects, a double
//! click opens (a folder in this window, a file in its app), the side
//! buttons of the mouse / Backspace / Alt+arrows go back, forward and up,
//! the arrows walk the icons and typing jumps to a name. The folder is
//! listed off the loop and followed with inotify. Not here yet: the menu,
//! drags, rename, copy and paste, the bin — the desktop's own, to be shared.
//!
//! Pointer-free: `waverunner-ctl files [<folder>]` opens a window;
//! `files go <folder>|back|forward|up|select <n>|activate <n>|key
//! <name>|scroll <px>|hidden|close|state` drive the newest one.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::seat::keyboard::Keysym;
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{error, info, warn};
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle, WEnum};

use crate::content::{
    GridContent, IconInst, Label, Rect, RectInst, Scene, GRID_CELL_W, GRID_ICON, GRID_ICON_TOP,
    LABEL_FONT_PX, LABEL_LINE_PX, NO_PLATE, PLATE_STATIC,
};
use crate::desktop::{Item, Kind};
use crate::fractional::SurfaceKind;
use crate::i18n::tr;
use crate::{launch, App};

/// The windows' class: what the compositor, the dock and window memory
/// know them by.
pub(crate) const APP_ID: &str = "golem-files";

/// A window's size until the compositor gives it one (floating), and the
/// least it can be made.
const DEFAULT_SIZE: (u32, u32) = (920, 600);
const MIN_SIZE: (u32, u32) = (380, 260);

/// The strip along the top: back, forward, the way here.
const HEAD_H: f32 = 46.0;
/// The places' column, and the window width under which it is left out.
const SIDE_W: f32 = 176.0;
const SIDE_FROM: f32 = 560.0;
/// One place's row, its icon, and the space over the plugged-in ones.
const ROW_H: f32 = 30.0;
const ROW_ICON: f32 = 18.0;
const ROW_GAP: f32 = 12.0;
/// Space around the icons' field.
const PAD: f32 = 10.0;
/// A cell is the desktop's with a second line of name: here a name is read
/// to tell files apart, and one line cuts most of them.
const CELL_H: f32 = crate::content::GRID_CELL_H + 6.0 + LABEL_LINE_PX;
const LABEL_GAP: f32 = 5.0;
/// Text in the head and the places.
const TEXT_PX: f32 = 13.0;
const TEXT_LINE: f32 = 17.0;

/// The most a folder shows (a listing past it is cut, and said so in the
/// log): every item is laid out each frame.
const ITEMS_MAX: usize = 20_000;
/// Thumbnails kept in hand per window (~350 KB each); past it, only those
/// on screen stay.
const CHAINS_MAX: usize = 400;
const LAYER_HEADROOM: u32 = 16;

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// Typing a name: how long between letters still counts as one word.
const TYPE_PAUSE: Duration = Duration::from_millis(900);
/// A wheel's notch (15 units from the compositor) in pixels of scroll.
const WHEEL_STEP: f32 = 4.0;

const WASH_HOVER: [f32; 4] = [1.0, 1.0, 1.0, 0.07];
const WASH_HERE: [f32; 4] = [1.0, 1.0, 1.0, 0.12];
const WASH_SELECTED: [f32; 4] = [1.0, 1.0, 1.0, 0.16];
const SCROLL_THUMB: [f32; 4] = [1.0, 1.0, 1.0, 0.22];

/// One of the places down the side.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Place {
    pub name: String,
    pub path: PathBuf,
    /// Its picture's key (an `asset:` one — see `desktop::icon_request`).
    pub icon: String,
    /// The first of a group: a little space over it.
    pub gap: bool,
    /// The way to a folder under it starts here (Home, a stick, a phone);
    /// the others are steps under Home.
    pub start: bool,
}

/// What the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Spot {
    #[default]
    Nothing,
    Item(usize),
    Place(usize),
    Crumb(usize),
    Back,
    Forward,
}

/// Where a window's parts are: its places' column, the icons' field and
/// the cells in it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Layout {
    pub side_w: f32,
    pub body: Rect,
    pub cols: usize,
    /// From one column's left edge to the next's.
    pub pitch: f32,
    pub cw: f32,
    pub ch: f32,
}

impl Layout {
    pub fn new((w, h): (u32, u32), icon_scale: f32) -> Self {
        let (w, h) = (w as f32, h as f32);
        let side_w = if w >= SIDE_FROM { SIDE_W } else { 0.0 };
        let body = Rect::new(side_w, HEAD_H, (w - side_w).max(1.0), (h - HEAD_H).max(1.0));
        let (cw, ch) = (GRID_CELL_W * icon_scale, CELL_H * icon_scale);
        let room = (body.w - 2.0 * PAD).max(cw);
        let cols = ((room / cw).floor() as usize).max(1);
        Self {
            side_w,
            body,
            cols,
            pitch: room / cols as f32,
            cw,
            ch,
        }
    }

    /// Item `i`'s cell, before any scroll.
    pub fn cell(&self, i: usize) -> Rect {
        let (col, row) = (i % self.cols, i / self.cols);
        Rect::new(
            self.body.x + PAD + col as f32 * self.pitch + (self.pitch - self.cw) / 2.0,
            self.body.y + PAD + row as f32 * self.ch,
            self.cw,
            self.ch,
        )
    }

    /// How tall `n` items are, pads included.
    pub fn content_h(&self, n: usize) -> f32 {
        n.div_ceil(self.cols) as f32 * self.ch + 2.0 * PAD
    }

    pub fn max_scroll(&self, n: usize) -> f32 {
        (self.content_h(n) - self.body.h).max(0.0)
    }

    /// The item whose cell is under `pos`, the field scrolled by `scroll`.
    pub fn index_at(&self, pos: (f32, f32), scroll: f32, n: usize) -> Option<usize> {
        if !self.body.contains(pos) {
            return None;
        }
        let (x, y) = (pos.0 - self.body.x - PAD, pos.1 - self.body.y - PAD + scroll);
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let (col, row) = ((x / self.pitch) as usize, (y / self.ch) as usize);
        if col >= self.cols {
            return None;
        }
        Some(row * self.cols + col).filter(|&i| i < n)
    }

    /// The items with any part on screen at `scroll`.
    pub fn in_view(&self, scroll: f32, n: usize) -> std::ops::Range<usize> {
        let first = ((scroll - PAD) / self.ch).floor().max(0.0) as usize;
        let last = ((scroll - PAD + self.body.h) / self.ch).ceil().max(0.0) as usize;
        (first * self.cols).min(n)..(last * self.cols).min(n)
    }

    /// The scroll that brings item `i` wholly into view from `scroll` (the
    /// least move).
    pub fn reveal(&self, i: usize, scroll: f32, n: usize) -> f32 {
        let cell = self.cell(i);
        let top = cell.y - self.body.y - PAD;
        let bottom = cell.y + cell.h - self.body.y + PAD;
        let to = if top < scroll {
            top
        } else if bottom > scroll + self.body.h {
            bottom - self.body.h
        } else {
            scroll
        };
        to.clamp(0.0, self.max_scroll(n))
    }
}

/// The clickable parts whose places are only known once drawn.
#[derive(Debug, Default)]
struct Hits {
    back: Rect,
    forward: Rect,
    crumbs: Vec<(Rect, PathBuf)>,
    places: Vec<Rect>,
}

/// One window.
// (Field order is drop order: the renderer's surface is made from the
// window's, so it goes first.)
struct Win {
    renderer: Option<crate::renderer::Renderer>,
    fscale: Option<crate::fractional::SurfaceScale>,
    window: Window,
    id: u32,
    size: (u32, u32),
    frame_pending: bool,
    dirty: bool,
    last_frame: Option<Instant>,
    tick: bool,

    dir: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    /// Which listing is awaited: an answer for an older one is dropped.
    generation: u64,
    loading: bool,
    failed: bool,
    hidden: bool,
    items: Vec<Item>,
    /// Each item's name as it is written under it (two lines), shaped once
    /// per listing and icon size: `(icon scale, names)`.
    names: (f32, Vec<Option<(String, String)>>),
    selected: HashSet<String>,
    /// Where the keys are (the last item clicked or walked to).
    cursor: Option<usize>,
    /// A path to select and bring into view once it is listed (the folder
    /// just come out of).
    reveal: Option<String>,
    scroll: f32,
    scroll_to: f32,
    places: Vec<Place>,

    chains: HashMap<String, Vec<u8>>,
    uploaded: Vec<Option<String>>,
    capacity: u32,
    pending: HashSet<String>,
    missing: HashSet<String>,

    ptr: Option<(f32, f32)>,
    hover: Spot,
    /// The scroll being reported comes from a wheel (notches), not fingers.
    wheel: bool,
    click: Option<(Instant, String)>,
    typed: String,
    typed_at: Option<Instant>,
    hits: Hits,
    watch: Option<calloop::RegistrationToken>,
    reload_pending: bool,
}

impl Win {
    fn layout(&self, icon_scale: f32) -> Layout {
        Layout::new(self.size, icon_scale)
    }

    fn layer_of(&self, key: &str) -> Option<u32> {
        self.uploaded
            .iter()
            .position(|u| u.as_deref() == Some(key))
            .map(|l| l as u32)
    }

    fn spot(&self, pos: (f32, f32), layout: &Layout) -> Spot {
        if self.hits.back.contains(pos) {
            return Spot::Back;
        }
        if self.hits.forward.contains(pos) {
            return Spot::Forward;
        }
        if let Some(i) = self.hits.crumbs.iter().position(|(r, _)| r.contains(pos)) {
            return Spot::Crumb(i);
        }
        if let Some(i) = self.hits.places.iter().position(|r| r.contains(pos)) {
            return Spot::Place(i);
        }
        layout
            .index_at(pos, self.scroll, self.items.len())
            .map_or(Spot::Nothing, Spot::Item)
    }

    fn select_only(&mut self, i: usize) {
        self.selected.clear();
        if let Some(item) = self.items.get(i) {
            self.selected.insert(item.path.clone());
            self.cursor = Some(i);
        }
    }
}

/// Every file window, and what they share.
pub(crate) struct Filer {
    xdg: Option<XdgShell>,
    fractional: Option<crate::fractional::Fractional>,
    wins: Vec<Win>,
    next: u32,
    /// The window the keyboard is on.
    pub keys: Option<u32>,
    icons: Option<crate::notif_icons::NotifIcons>,
}

impl Filer {
    pub fn new(xdg: Option<XdgShell>, fractional: Option<crate::fractional::Fractional>) -> Self {
        Self {
            xdg,
            fractional,
            wins: Vec::new(),
            next: 1,
            keys: None,
            icons: None,
        }
    }

    fn win(&self, id: u32) -> Option<&Win> {
        self.wins.iter().find(|w| w.id == id)
    }

    fn win_mut(&mut self, id: u32) -> Option<&mut Win> {
        self.wins.iter_mut().find(|w| w.id == id)
    }

    /// Take a window out to work on it with the rest of the app in hand;
    /// it goes back with `put`.
    fn take(&mut self, id: u32) -> Option<Win> {
        let at = self.wins.iter().position(|w| w.id == id)?;
        Some(self.wins.swap_remove(at))
    }

    fn put(&mut self, win: Win) {
        self.wins.push(win);
    }

    /// Whose surface this is.
    pub fn id_of(&self, surface: &WlSurface) -> Option<u32> {
        self.wins
            .iter()
            .find(|w| w.window.wl_surface() == surface)
            .map(|w| w.id)
    }

    fn newest(&self) -> Option<u32> {
        self.wins.iter().map(|w| w.id).max()
    }

    /// A window's fractional scale state and logical size (`main.rs`'s
    /// scale plumbing).
    pub fn scale_slot(&mut self, id: u32) -> Option<&mut crate::fractional::SurfaceScale> {
        self.win_mut(id).and_then(|w| w.fscale.as_mut())
    }

    pub fn scale_of(&self, id: u32, fallback: f32) -> (f32, (u32, u32)) {
        self.win(id).map_or((fallback, (0, 0)), |w| {
            (
                w.fscale.as_ref().map_or(fallback, |f| f.scale_or(fallback)),
                w.size,
            )
        })
    }
}

/// One of the owner's folders as `user-dirs.dirs` names it
/// (`XDG_<KEY>_DIR="$HOME/…"`), if it is there and is not Home itself.
fn user_dir(text: &str, home: &Path, key: &str) -> Option<PathBuf> {
    let wanted = format!("XDG_{key}_DIR=");
    let value = text
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix(wanted.as_str()))?
        .trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    (path != home && path.is_absolute()).then_some(path)
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
}

/// The places down the side: Home, the owner's folders that exist (under
/// the names they have on disk), the phones' folder, then what is plugged
/// in and open.
pub(crate) fn places(home: &Path, user_dirs: &str, volumes: &[crate::mounts::Mounted]) -> Vec<Place> {
    let mut out = vec![Place {
        name: tr("Home").to_owned(),
        path: home.to_path_buf(),
        icon: "asset:user-home".to_owned(),
        gap: false,
        start: true,
    }];
    for (key, fallback, icon) in [
        ("DESKTOP", "Desktop", "user-desktop"),
        ("DOCUMENTS", "Documents", "folder-documents"),
        ("DOWNLOAD", "Downloads", "folder-download"),
        ("PICTURES", "Pictures", "folder-pictures"),
        ("MUSIC", "Music", "folder-music"),
        ("VIDEOS", "Videos", "folder-videos"),
    ] {
        let path = user_dir(user_dirs, home, key).unwrap_or_else(|| home.join(fallback));
        if path.is_dir() && !out.iter().any(|p| p.path == path) {
            out.push(Place {
                name: file_name(&path),
                path,
                icon: format!("asset:{icon}"),
                gap: false,
                start: false,
            });
        }
    }
    let phones = home.join("Phones");
    if phones.is_dir() {
        out.push(Place {
            name: file_name(&phones),
            path: phones,
            icon: "asset:phone".to_owned(),
            gap: false,
            start: false,
        });
    }
    let mut first = true;
    for v in volumes.iter().filter(|v| !v.closed) {
        out.push(Place {
            name: v.name.clone(),
            path: v.path.clone(),
            icon: format!("asset:{}", if v.phone { "phone" } else { "drive-removable-media-usb" }),
            gap: first,
            start: true,
        });
        first = false;
    }
    out
}

/// The way to `dir` as steps to click: from the place it is deepest under
/// (Home, a stick, a phone), else from the root.
pub(crate) fn crumbs(dir: &Path, places: &[Place]) -> Vec<(String, PathBuf)> {
    let start = places
        .iter()
        .filter(|p| p.start && dir.starts_with(&p.path))
        .max_by_key(|p| p.path.components().count());
    let (mut out, mut at, rest) = match start {
        Some(p) => (
            vec![(p.name.clone(), p.path.clone())],
            p.path.clone(),
            dir.strip_prefix(&p.path).unwrap_or(Path::new("")).to_path_buf(),
        ),
        None => (
            vec![("/".to_owned(), PathBuf::from("/"))],
            PathBuf::from("/"),
            dir.strip_prefix("/").unwrap_or(dir).to_path_buf(),
        ),
    };
    for part in rest.components() {
        at.push(part);
        out.push((part.as_os_str().to_string_lossy().into_owned(), at.clone()));
    }
    out
}

/// A folder's items: folders first, then by name whatever the case; dot
/// files only when `hidden`. `Err` when the folder cannot be read.
pub(crate) fn list(dir: &Path, hidden: bool) -> Result<Vec<Item>, std::io::Error> {
    let mut items: Vec<Item> = std::fs::read_dir(dir)?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                return None;
            }
            let path = e.path();
            // Through the link, so a linked folder is a folder; a link to
            // nothing is still shown, as a file.
            let is_dir = std::fs::metadata(&path).is_ok_and(|m| m.is_dir());
            Some(crate::desktop::item_for(path, name, is_dir))
        })
        .collect();
    items.sort_by(|a, b| {
        (b.kind == Kind::Folder)
            .cmp(&(a.kind == Kind::Folder))
            .then_with(|| by_name(&a.name, &b.name))
    });
    if items.len() > ITEMS_MAX {
        warn!("files: {} holds {} things, showing {ITEMS_MAX}", dir.display(), items.len());
        items.truncate(ITEMS_MAX);
    }
    Ok(items)
}

/// Names in the order a person files them: whatever the case, and a run
/// of digits by its number ("file 2" before "file 10").
pub(crate) fn by_name(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (la, lb) = (a.to_lowercase(), b.to_lowercase());
    let (mut x, mut y) = (la.chars().peekable(), lb.chars().peekable());
    loop {
        let (Some(&cx), Some(&cy)) = (x.peek(), y.peek()) else {
            // One ran out: the shorter first; the same but for case, as written.
            return x.peek().is_some().cmp(&y.peek().is_some()).then_with(|| a.cmp(b));
        };
        if cx.is_ascii_digit() && cy.is_ascii_digit() {
            let run = |it: &mut std::iter::Peekable<std::str::Chars>| -> String {
                let mut digits = String::new();
                while let Some(d) = it.next_if(char::is_ascii_digit) {
                    digits.push(d);
                }
                digits
            };
            let (nx, ny) = (run(&mut x), run(&mut y));
            let (tx, ty) = (nx.trim_start_matches('0'), ny.trim_start_matches('0'));
            // (More digits is the bigger number; the same count compares as text.)
            match tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty)) {
                Ordering::Equal => {}
                other => return other,
            }
        } else {
            match cx.cmp(&cy) {
                Ordering::Equal => {
                    x.next();
                    y.next();
                }
                other => return other,
            }
        }
    }
}

/// A name on two lines under its icon: the first as much as fits (broken
/// after a space or a mark when one is near its end), the second the rest,
/// cut with an ellipsis if that is still too long.
pub(crate) fn two_lines(name: &str, max_w: f32, measure: &mut dyn FnMut(&str) -> f32) -> (String, String) {
    if measure(name) <= max_w {
        return (name.to_owned(), String::new());
    }
    let chars: Vec<char> = name.chars().collect();
    // The longest head that fits: binary search over measured widths.
    let (mut lo, mut hi) = (1usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(&chars[..mid].iter().collect::<String>()) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    // A break after a space, dash, underscore or dot in the head's second
    // half reads better than one mid-word.
    let cut = (lo / 2..lo)
        .rev()
        .find(|&i| matches!(chars[i], ' ' | '-' | '_' | '.'))
        .map_or(lo, |i| i + 1);
    let first: String = chars[..cut].iter().collect();
    let rest: String = chars[cut..].iter().collect();
    (
        first.trim_end().to_owned(),
        crate::desktop::fit_label(rest.trim_start(), max_w, measure),
    )
}

/// The first item whose name starts with `typed`, whatever the case.
pub(crate) fn typed_match(items: &[Item], typed: &str) -> Option<usize> {
    let typed = typed.to_lowercase();
    items.iter().position(|it| it.name.to_lowercase().starts_with(&typed))
}

/// A colour brought `k` of the way to white, solid: the window's ground is
/// the dock's own colour without its glass. (Colours here are linear light:
/// a little goes a long way.)
fn lift(c: [f32; 4], k: f32) -> [f32; 4] {
    [c[0] + (1.0 - c[0]) * k, c[1] + (1.0 - c[1]) * k, c[2] + (1.0 - c[2]) * k, 1.0]
}

fn rect(rect: Rect, radius: f32, color: [f32; 4]) -> RectInst {
    RectInst {
        rect,
        radius,
        color,
        glass: 0.0,
        border: 0.0,
    }
}

fn label(text: &str, pos: (f32, f32), max_w: f32, centered: bool, dim: bool) -> Label {
    Label {
        text: text.to_owned(),
        pos,
        max_w,
        font_px: TEXT_PX,
        line_px: TEXT_LINE,
        centered,
        dim,
        cache: true,
        clip: None,
        family: None,
        color: None,
    }
}

impl App {
    /// Open a window on `dir` (Home when it is not a folder).
    pub(crate) fn filer_open(&mut self, dir: PathBuf) {
        let dir = if dir.is_dir() { dir } else { home() };
        let Some(xdg) = self.filer.xdg.as_ref() else {
            warn!("files: the compositor has no xdg-shell; handing {} to the system", dir.display());
            let exec = format!("xdg-open {}", launch::shell_quote(&dir.to_string_lossy()));
            if let Err(e) = launch::launch(&exec, false, &self.config.launch.terminal) {
                error!("files: opening {} failed: {e:#}", dir.display());
            }
            return;
        };
        let id = self.filer.next;
        self.filer.next += 1;
        let surface = self.compositor.create_surface(&self.qh);
        let window = xdg.create_window(surface, WindowDecorations::RequestServer, &self.qh);
        window.set_app_id(APP_ID);
        window.set_title(file_name(&dir));
        window.set_min_size(Some(MIN_SIZE));
        let fscale = self
            .filer
            .fractional
            .as_ref()
            .map(|f| f.attach(window.wl_surface(), SurfaceKind::Files(id), &self.qh));
        if fscale.is_none() {
            window
                .wl_surface()
                .set_buffer_scale(self.config.desktop.render_scale.max(1) as i32);
        }
        // The first commit has no buffer: the compositor answers with a
        // configure, and the window is drawn then.
        window.commit();
        // One resolver for every window's carriers, started with the first.
        if self.filer.icons.is_none() {
            let (tx, rx) = calloop::channel::channel::<crate::notif_icons::Resolved>();
            let waiting = self.loop_handle.insert_source(rx, |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(res) = event {
                    app.filer_on_icon(res);
                }
            });
            if waiting.is_ok() {
                self.filer.icons = Some(crate::notif_icons::spawn(
                    "waverunner-files-icons",
                    self.config.theme.icon_theme.clone(),
                    tx,
                ));
            } else {
                warn!("files: no icon resolver, names only");
            }
        }
        info!("files: window {id} on {}", dir.display());
        self.filer.put(Win {
            renderer: None,
            fscale,
            window,
            id,
            size: (0, 0),
            frame_pending: false,
            dirty: false,
            last_frame: None,
            tick: false,
            dir: PathBuf::new(),
            back: Vec::new(),
            forward: Vec::new(),
            generation: 0,
            loading: false,
            failed: false,
            hidden: false,
            items: Vec::new(),
            names: (0.0, Vec::new()),
            selected: HashSet::new(),
            cursor: None,
            reveal: None,
            scroll: 0.0,
            scroll_to: 0.0,
            places: Vec::new(),
            chains: HashMap::new(),
            uploaded: Vec::new(),
            capacity: 0,
            pending: HashSet::new(),
            missing: HashSet::new(),
            ptr: None,
            hover: Spot::Nothing,
            wheel: false,
            click: None,
            typed: String::new(),
            typed_at: None,
            hits: Hits::default(),
            watch: None,
            reload_pending: false,
        });
        self.filer_places();
        self.filer_show(id, dir);
    }

    /// The owner closed a window (or `files close`).
    fn filer_close(&mut self, id: u32) {
        let Some(win) = self.filer.take(id) else {
            return;
        };
        if let Some(token) = win.watch {
            self.loop_handle.remove(token);
        }
        if self.filer.keys == Some(id) {
            self.filer.keys = None;
        }
        info!("files: window {id} closed");
    }

    /// The places, read again for every window (something was plugged in or
    /// taken out, a window opened).
    pub(crate) fn filer_places(&mut self) {
        if self.filer.wins.is_empty() {
            return;
        }
        let home = home();
        let user_dirs = std::fs::read_to_string(
            std::env::var_os("XDG_CONFIG_HOME")
                .map_or_else(|| home.join(".config"), PathBuf::from)
                .join("user-dirs.dirs"),
        )
        .unwrap_or_default();
        let list = places(&home, &user_dirs, &self.desktop.volumes);
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            if let Some(win) = self.filer.win_mut(id) {
                if win.places != list {
                    win.places = list.clone();
                    self.filer_request_draw(id);
                }
            }
        }
    }

    /// Go to `dir`, the folder shown now becoming the way back.
    fn filer_go(&mut self, id: u32, dir: PathBuf) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if win.dir == dir {
            return;
        }
        let from = std::mem::take(&mut win.dir);
        win.back.push(from);
        win.forward.clear();
        self.filer_show(id, dir);
    }

    fn filer_back(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(to) = win.back.pop() else {
            return;
        };
        let from = std::mem::take(&mut win.dir);
        win.reveal = Some(from.to_string_lossy().into_owned());
        win.forward.push(from);
        self.filer_show(id, to);
    }

    fn filer_forward(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(to) = win.forward.pop() else {
            return;
        };
        let from = std::mem::take(&mut win.dir);
        win.back.push(from);
        self.filer_show(id, to);
    }

    /// To the folder this one is in, which comes up with this one selected.
    fn filer_up(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(parent) = win.dir.parent().map(Path::to_path_buf) else {
            return;
        };
        win.reveal = Some(win.dir.to_string_lossy().into_owned());
        self.filer_go(id, parent);
    }

    /// Show `dir` in window `id`: empty at once, listed off the loop,
    /// watched from now on. History is the caller's.
    fn filer_show(&mut self, id: u32, dir: PathBuf) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.dir = dir.clone();
        win.items.clear();
        win.names.1.clear();
        win.selected.clear();
        win.cursor = None;
        win.click = None;
        win.typed.clear();
        win.scroll = 0.0;
        win.scroll_to = 0.0;
        win.failed = false;
        win.window.set_title(file_name(&dir));
        if let Some(token) = win.watch.take() {
            self.loop_handle.remove(token);
        }
        if let Some(fd) = crate::apps::watch_dirs(std::slice::from_ref(&dir)) {
            use std::os::fd::AsFd;
            let source = calloop::generic::Generic::new(fd, calloop::Interest::READ, calloop::Mode::Level);
            let token = self.loop_handle.insert_source(source, move |_, fd, app: &mut App| {
                crate::apps::drain_inotify(fd.as_fd());
                app.filer_changed(id);
                Ok(calloop::PostAction::Continue)
            });
            match token {
                Ok(token) => {
                    if let Some(win) = self.filer.win_mut(id) {
                        win.watch = Some(token);
                    }
                }
                Err(e) => warn!("files: cannot follow {}: {e}", dir.display()),
            }
        }
        self.filer_list(id);
        self.filer_request_draw(id);
    }

    /// The folder changed on disk: read it again in a moment, once for a
    /// burst.
    fn filer_changed(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if win.reload_pending {
            return;
        }
        win.reload_pending = true;
        let soon = calloop::timer::Timer::from_duration(Duration::from_millis(60));
        let _ = self.loop_handle.insert_source(soon, move |_, _, app: &mut App| {
            if let Some(win) = app.filer.win_mut(id) {
                win.reload_pending = false;
            }
            app.filer_list(id);
            calloop::timer::TimeoutAction::Drop
        });
    }

    /// List the window's folder off the loop (a phone's storage takes its
    /// time) and take the answer if it is still the one awaited.
    fn filer_list(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.generation += 1;
        win.loading = true;
        let (generation, dir, hidden) = (win.generation, win.dir.clone(), win.hidden);
        self.desktop_off_loop(
            move || list(&dir, hidden),
            move |app, listed| app.filer_listed(id, generation, listed),
        );
    }

    fn filer_listed(&mut self, id: u32, generation: u64, listed: Result<Vec<Item>, std::io::Error>) {
        let icon_scale = self.icon_scale();
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if win.generation != generation {
            return;
        }
        win.loading = false;
        let cursor_path = win.cursor.and_then(|i| win.items.get(i)).map(|it| it.path.clone());
        match listed {
            Ok(items) => {
                win.items = items;
                win.failed = false;
            }
            Err(e) => {
                warn!("files: cannot read {}: {e}", win.dir.display());
                win.items.clear();
                win.failed = true;
            }
        }
        win.names.1.clear();
        // The selection follows the files; the keys stay on theirs.
        let present: HashSet<&str> = win.items.iter().map(|it| it.path.as_str()).collect();
        win.selected.retain(|p| present.contains(p.as_str()));
        win.cursor = cursor_path.and_then(|p| win.items.iter().position(|it| it.path == p));
        let layout = win.layout(icon_scale);
        if let Some(i) = win
            .reveal
            .take()
            .and_then(|p| win.items.iter().position(|it| it.path == p))
        {
            win.select_only(i);
            win.scroll_to = layout.reveal(i, win.scroll_to, win.items.len());
            win.scroll = win.scroll_to;
        }
        let max = layout.max_scroll(win.items.len());
        win.scroll_to = win.scroll_to.min(max);
        win.scroll = win.scroll.min(max);
        self.filer_request_draw(id);
    }

    /// Open item `i`: a folder in this window, a shortcut runs, anything
    /// else in its app.
    fn filer_activate(&mut self, id: u32, i: usize) {
        let Some(item) = self.filer.win(id).and_then(|w| w.items.get(i)).cloned() else {
            return;
        };
        if item.kind == Kind::Folder || item.kind == Kind::Volume {
            self.filer_go(id, PathBuf::from(&item.path));
            return;
        }
        let (exec, terminal) = match &item.exec {
            Some((exec, terminal)) => (exec.clone(), *terminal),
            None => (format!("xdg-open {}", launch::shell_quote(&item.path)), false),
        };
        info!("files: open {}", item.path);
        if let Err(e) = launch::launch(&exec, terminal, &self.config.launch.terminal) {
            error!("files: opening {} failed: {e:#}", item.path);
        }
    }

    // ── the window ──────────────────────────────────────────────────────

    /// A `configure`: learn the size, build or resize the renderer, draw.
    fn filer_configure(&mut self, window: &Window, configure: WindowConfigure) {
        let Some(id) = self.filer.id_of(window.wl_surface()) else {
            return;
        };
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let pick = |new: Option<std::num::NonZeroU32>, old: u32, default: u32| {
            new.map_or(if old > 0 { old } else { default }, std::num::NonZeroU32::get)
        };
        let width = pick(configure.new_size.0, win.size.0, DEFAULT_SIZE.0);
        let height = pick(configure.new_size.1, win.size.1, DEFAULT_SIZE.1);
        win.size = (width, height);
        let scale = self.surface_scale(SurfaceKind::Files(id));
        let (pw, ph) = (
            crate::fractional::physical(width, scale),
            crate::fractional::physical(height, scale),
        );
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if let Some(fs) = &win.fscale {
            fs.set_logical_size(width, height);
        }
        if let Some(renderer) = win.renderer.as_mut() {
            renderer.set_scale(scale);
            renderer.resize(pw, ph);
        } else {
            match crate::renderer::Renderer::new(&self.conn, win.window.wl_surface(), pw, ph, scale) {
                Ok(mut renderer) => {
                    if win.capacity > 0 {
                        renderer.alloc_icon_array(win.capacity);
                        win.uploaded.iter_mut().for_each(|u| *u = None);
                    }
                    win.renderer = Some(renderer);
                }
                Err(e) => {
                    // Never fatal: the shell outlives a window that cannot draw.
                    error!("files: renderer init failed: {e:#}");
                    return;
                }
            }
        }
        self.filer_relayout(id);
    }

    /// The size or the scale changed: keep the scroll inside and draw.
    pub(crate) fn filer_relayout(&mut self, id: u32) {
        let icon_scale = self.icon_scale();
        if let Some(win) = self.filer.win_mut(id) {
            let max = win.layout(icon_scale).max_scroll(win.items.len());
            win.scroll_to = win.scroll_to.min(max);
            win.scroll = win.scroll.min(max);
        }
        self.filer_request_draw(id);
    }

    /// A window's renderer, for a change of scale (`rescale_surface`).
    pub(crate) fn filer_renderer(&mut self, id: u32) -> Option<&mut crate::renderer::Renderer> {
        self.filer.win_mut(id).and_then(|w| w.renderer.as_mut())
    }

    /// The compositor showed a frame of this window's.
    pub(crate) fn filer_frame(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.frame_pending = false;
        if win.dirty {
            self.filer_draw(id);
        }
    }

    /// Draw now, or once the frame in flight has been shown.
    pub(crate) fn filer_request_draw(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if win.frame_pending {
            win.dirty = true;
        } else {
            self.filer_draw(id);
        }
    }

    /// The pictures on screen into the window's texture layers: uploaded
    /// where the pixels are in hand, asked for where not. A file that can
    /// have a picture of its own gets it from the Files thumbnailer.
    fn filer_sync_icons(&mut self, win: &mut Win, layout: &Layout) {
        let view = layout.in_view(win.scroll, win.items.len());
        for item in win.items[view.clone()].iter_mut().filter(|it| it.kind == Kind::File) {
            if self.audio_paths.contains(&item.path) {
                item.icon = crate::desktop::asset_key("asset-audio");
                continue;
            }
            if !crate::thumbs::thumbable(&item.name) {
                continue;
            }
            let key = crate::desktop::thumb_key(&item.path);
            if !win.chains.contains_key(&key) {
                if let Some((_, pixels)) = self.thumb_map.get(&item.path) {
                    win.chains.insert(key.clone(), pixels.clone());
                }
            }
            if win.chains.contains_key(&key) {
                item.icon = key;
            } else {
                // (Its picture was let go while it was off screen, or never
                // came: the carrier meanwhile.)
                item.icon = crate::desktop::asset_key(crate::files::file_asset_name(&item.name));
                if self.thumb_pending.insert(item.path.clone()) {
                    self.thumbs.request(&item.path);
                }
            }
        }
        let mut worn: Vec<&str> = Vec::new();
        for key in win
            .places
            .iter()
            .map(|p| p.icon.as_str())
            .chain(win.items[view].iter().map(|it| it.icon.as_str()))
        {
            if !worn.contains(&key) {
                worn.push(key);
            }
        }
        // Thumbnails are big: past the cap only those on screen are kept.
        if win.chains.len() > CHAINS_MAX {
            win.chains.retain(|k, _| worn.contains(&k.as_str()));
        }
        for held in win.uploaded.iter_mut() {
            if held.as_ref().is_some_and(|k| !worn.contains(&k.as_str())) {
                *held = None;
            }
        }
        // (Growing the array empties it, so what was uploaded is forgotten.)
        if worn.len() as u32 > win.capacity {
            win.capacity = worn.len() as u32 + LAYER_HEADROOM;
            if let Some(r) = win.renderer.as_mut() {
                r.alloc_icon_array(win.capacity);
            }
            win.uploaded.clear();
        }
        win.uploaded.resize(win.capacity as usize, None);
        let mut asked = Vec::new();
        for key in worn {
            if win.uploaded.iter().any(|u| u.as_deref() == Some(key)) {
                continue;
            }
            if let Some(chain) = win.chains.get(key) {
                let free = win.uploaded.iter().position(Option::is_none);
                if let (Some(layer), Some(r)) = (free, win.renderer.as_mut()) {
                    r.update_icon_layer(layer as u32, chain);
                    win.uploaded[layer] = Some(key.to_owned());
                }
            } else if !win.pending.contains(key) && !win.missing.contains(key) {
                if let Some(req) = crate::desktop::icon_request(key) {
                    asked.push(req);
                }
            }
        }
        if let Some(icons) = self.filer.icons.as_ref() {
            for req in asked {
                win.pending.insert(req.key.clone());
                icons.request(req);
            }
        }
    }

    /// The resolver answered: every window that asked for it files the
    /// pixels and draws.
    fn filer_on_icon(&mut self, res: crate::notif_icons::Resolved) {
        let mut redraw = Vec::new();
        for win in self.filer.wins.iter_mut() {
            if !win.pending.remove(&res.key) {
                continue;
            }
            match &res.chain {
                Some(chain) => {
                    win.chains.insert(res.key.clone(), chain.clone());
                }
                None => {
                    win.missing.insert(res.key.clone());
                }
            }
            redraw.push(win.id);
        }
        for id in redraw {
            self.filer_request_draw(id);
        }
    }

    /// The thumbnailer finished a file: the windows showing it wear it.
    pub(crate) fn filer_on_thumb(&mut self, path: &str, pixels: &[u8]) {
        let key = crate::desktop::thumb_key(path);
        let mut redraw = Vec::new();
        for win in self.filer.wins.iter_mut() {
            if !win.items.iter().any(|it| it.path == path) {
                continue;
            }
            win.chains.insert(key.clone(), pixels.to_vec());
            // (A picture taken anew: its layer is written again.)
            for held in win.uploaded.iter_mut() {
                if held.as_deref() == Some(key.as_str()) {
                    *held = None;
                }
            }
            redraw.push(win.id);
        }
        for id in redraw {
            self.filer_request_draw(id);
        }
    }

    /// Draw one frame of window `id`.
    fn filer_draw(&mut self, id: u32) {
        let icon_scale = self.icon_scale();
        let ground = lift(self.config.theme.background_rgba(), 0.0);
        let side_ground = lift(self.config.theme.background_rgba(), 0.012);
        let ink = self.config.theme.text_rgba();
        let squircle = self.config.theme.icon_squircle;
        let Some(mut win) = self.filer.take(id) else {
            return;
        };
        if win.size.0 == 0 || win.renderer.is_none() {
            self.filer.put(win);
            return;
        }
        win.dirty = false;
        let layout = win.layout(icon_scale);
        // The scroll eases to where the wheel sent it.
        let now = Instant::now();
        let dt = win
            .last_frame
            .map(|l| now.duration_since(l).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        let (scroll, moving) = crate::animation::ease_toward(win.scroll, win.scroll_to, dt, 18.0, 0.5);
        win.scroll = scroll;
        win.last_frame = moving.then_some(now);
        self.filer_sync_icons(&mut win, &layout);

        let (w, h) = (win.size.0 as f32, win.size.1 as f32);
        let mut scene = Scene {
            alpha: 1.0,
            ..Default::default()
        };
        scene.rects.push(rect(Rect::new(0.0, 0.0, w, h), 0.0, ground));
        if layout.side_w > 0.0 {
            scene
                .rects
                .push(rect(Rect::new(0.0, HEAD_H, layout.side_w, h - HEAD_H), 0.0, side_ground));
        }
        let mut hits = Hits::default();

        // ── the head: back, forward, the way here ──
        let mid = HEAD_H / 2.0;
        hits.back = Rect::new(PAD, mid - 14.0, 28.0, 28.0);
        hits.forward = Rect::new(PAD + 32.0, mid - 14.0, 28.0, 28.0);
        for (spot, r, glyph, live) in [
            (Spot::Back, hits.back, "‹", !win.back.is_empty()),
            (Spot::Forward, hits.forward, "›", !win.forward.is_empty()),
        ] {
            if live && win.hover == spot {
                scene.rects.push(rect(r, 14.0, WASH_HOVER));
            }
            scene.labels.push(Label {
                font_px: 18.0,
                line_px: 22.0,
                ..label(glyph, (r.x + r.w / 2.0, mid - 12.0), r.w, true, !live)
            });
        }
        let steps = crumbs(&win.dir, &win.places);
        if let Some(renderer) = win.renderer.as_mut() {
            let widths: Vec<f32> = steps
                .iter()
                .map(|(name, _)| renderer.measure_text(name, TEXT_PX, None))
                .collect();
            const STEP_GAP: f32 = 22.0;
            let (left, room) = (PAD + 76.0, w - PAD - 76.0 - PAD);
            // The steps that fit, counted from the folder shown: the first
            // ones give way when the window is narrow.
            let mut from = steps.len().saturating_sub(1);
            let mut used = widths.last().copied().unwrap_or(0.0);
            while from > 0 && used + STEP_GAP + widths[from - 1] <= room {
                from -= 1;
                used += STEP_GAP + widths[from];
            }
            let mut x = left;
            for (n, ((name, path), tw)) in steps.iter().zip(&widths).enumerate().skip(from) {
                if n > from {
                    scene
                        .labels
                        .push(label("›", (x - STEP_GAP / 2.0, mid - TEXT_LINE / 2.0), STEP_GAP, true, true));
                }
                let last = n + 1 == steps.len();
                let tw = tw.min(room);
                let hit = Rect::new(x - 7.0, mid - 13.0, tw + 14.0, 26.0);
                if !last && win.hover == Spot::Crumb(hits.crumbs.len()) {
                    scene.rects.push(rect(hit, 8.0, WASH_HOVER));
                }
                scene
                    .labels
                    .push(label(name, (x, mid - TEXT_LINE / 2.0), tw + 2.0, false, !last));
                hits.crumbs.push((hit, path.clone()));
                x += tw + STEP_GAP;
            }
        }

        // ── the places ──
        if layout.side_w > 0.0 {
            let mut y = HEAD_H + 6.0;
            for (n, place) in win.places.iter().enumerate() {
                if place.gap {
                    y += ROW_GAP;
                }
                let row = Rect::new(6.0, y, layout.side_w - 12.0, ROW_H);
                if win.dir == place.path {
                    scene.rects.push(rect(row, 8.0, WASH_HERE));
                } else if win.hover == Spot::Place(n) {
                    scene.rects.push(rect(row, 8.0, WASH_HOVER));
                }
                if let Some(layer) = win.layer_of(&place.icon) {
                    scene.icons.push(IconInst {
                        rect: Rect::new(row.x + 10.0, y + (ROW_H - ROW_ICON) / 2.0, ROW_ICON, ROW_ICON),
                        layer,
                        tint: [0.0; 4],
                        ring: -1.0,
                        plate: NO_PLATE,
                    });
                }
                scene.labels.push(label(
                    &place.name,
                    (row.x + 10.0 + ROW_ICON + 9.0, y + (ROW_H - TEXT_LINE) / 2.0),
                    row.w - ROW_ICON - 26.0,
                    false,
                    false,
                ));
                hits.places.push(row);
                y += ROW_H;
            }
        }

        // ── the icons ──
        let mut grid = GridContent {
            clip: layout.body,
            ..Default::default()
        };
        let view = layout.in_view(win.scroll, win.items.len());
        let label_w = layout.cw - 12.0;
        if win.names.0 != icon_scale || win.names.1.len() != win.items.len() {
            win.names = (icon_scale, vec![None; win.items.len()]);
        }
        for i in view {
            let item = &win.items[i];
            let mut cell = layout.cell(i);
            cell.y -= win.scroll;
            if win.selected.contains(&item.path) {
                grid.rects.push(rect(
                    Rect::new(cell.x + 3.0, cell.y + 3.0, cell.w - 6.0, cell.h - 6.0),
                    10.0,
                    WASH_SELECTED,
                ));
            }
            let size = GRID_ICON * icon_scale;
            let icon = Rect::new(cell.x + (cell.w - size) / 2.0, cell.y + GRID_ICON_TOP * icon_scale, size, size);
            if let Some(layer) = win.layer_of(&item.icon) {
                grid.icons.push(IconInst {
                    rect: icon,
                    layer,
                    tint: [0.0; 4],
                    ring: -1.0,
                    plate: if item.kind == Kind::Launcher { PLATE_STATIC } else { NO_PLATE },
                });
            }
            if win.names.1[i].is_none() {
                if let Some(renderer) = win.renderer.as_mut() {
                    win.names.1[i] = Some(two_lines(&item.name, label_w, &mut |t| {
                        renderer.measure_text(t, LABEL_FONT_PX, None)
                    }));
                }
            }
            let Some((first, second)) = win.names.1[i].as_ref() else {
                continue;
            };
            for (n, line) in [first, second].into_iter().enumerate().filter(|(_, l)| !l.is_empty()) {
                grid.labels.push(Label {
                    font_px: LABEL_FONT_PX,
                    line_px: LABEL_LINE_PX,
                    ..label(
                        line,
                        (cell.x + cell.w / 2.0, icon.y + icon.h + LABEL_GAP + n as f32 * LABEL_LINE_PX),
                        label_w,
                        true,
                        false,
                    )
                });
            }
        }
        if win.items.is_empty() && !win.loading {
            let words = if win.failed { tr("This folder can't be opened") } else { tr("Empty folder") };
            grid.labels.push(label(
                words,
                (layout.body.x + layout.body.w / 2.0, layout.body.y + layout.body.h / 2.0 - TEXT_LINE),
                layout.body.w - 2.0 * PAD,
                true,
                true,
            ));
        }
        // How far down the folder this is, when it does not all fit.
        let (content, max) = (layout.content_h(win.items.len()), layout.max_scroll(win.items.len()));
        if max > 0.0 {
            let bar_h = (layout.body.h * layout.body.h / content).max(28.0);
            let y = layout.body.y + 3.0 + (layout.body.h - 6.0 - bar_h) * (win.scroll / max).clamp(0.0, 1.0);
            grid.rects.push(rect(
                Rect::new(layout.body.x + layout.body.w - 7.0, y, 4.0, bar_h),
                2.0,
                SCROLL_THUMB,
            ));
        }
        scene.grids.push(grid);
        win.hits = hits;

        let mut presented = false;
        {
            let (surface, qh, pending) = (win.window.wl_surface(), &self.qh, &mut win.frame_pending);
            if let Some(renderer) = win.renderer.as_mut() {
                match renderer.render(&scene, ink, None, squircle, u32::MAX, None, &mut || {
                    if !*pending {
                        surface.frame(qh, surface.clone());
                        *pending = true;
                    }
                }) {
                    Ok(crate::renderer::Frame::Presented) => presented = true,
                    Ok(crate::renderer::Frame::Unchanged) => {}
                    Err(e) => warn!("files: render failed: {e:#}"),
                }
            }
        }
        if moving {
            // The ease's next frame: on the frame callback, or — when this
            // one changed nothing and so none is coming — on a timer (the
            // desktop's lesson).
            win.dirty = true;
            if !presented && !win.frame_pending && !win.tick {
                let timer = calloop::timer::Timer::from_duration(Duration::from_millis(8));
                win.tick = self
                    .loop_handle
                    .insert_source(timer, move |_, _, app: &mut App| {
                        if let Some(win) = app.filer.win_mut(id) {
                            win.tick = false;
                        }
                        app.filer_draw(id);
                        calloop::timer::TimeoutAction::Drop
                    })
                    .is_ok();
            }
        }
        self.filer.put(win);
    }

    // ── the pointer ─────────────────────────────────────────────────────

    /// The pointer's shape for what it is on: the index on what a click
    /// goes to, the arrow elsewhere.
    fn filer_cursor(&mut self, id: u32) {
        let Some(win) = self.filer.win(id) else {
            return;
        };
        let goes = match win.hover {
            Spot::Nothing => false,
            Spot::Item(i) => win
                .items
                .get(i)
                .is_some_and(|it| matches!(it.kind, Kind::Folder | Kind::Volume)),
            Spot::Back => !win.back.is_empty(),
            Spot::Forward => !win.forward.is_empty(),
            Spot::Crumb(i) => i + 1 < win.hits.crumbs.len(),
            Spot::Place(_) => true,
        };
        let shape = if goes { Shape::Pointer } else { Shape::Default };
        if self.cursor_now != Some(shape) {
            if let Some(device) = &self.cursor_device {
                device.set_shape(self.enter_serial, shape);
                self.cursor_now = Some(shape);
            }
        }
    }

    fn filer_hover(&mut self, id: u32) {
        let icon_scale = self.icon_scale();
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let layout = win.layout(icon_scale);
        let spot = win.ptr.map_or(Spot::Nothing, |p| win.spot(p, &layout));
        let changed = spot != win.hover;
        win.hover = spot;
        self.filer_cursor(id);
        // (An icon under the pointer looks no different: only the head's
        // and the places' rows answer it.)
        if changed {
            self.filer_request_draw(id);
        }
    }

    /// Route a pointer event on window `id`.
    pub(crate) fn filer_pointer(&mut self, id: u32, event: wl_pointer::Event) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                // A fresh enter: the compositor forgot our shape on leave.
                self.enter_serial = serial;
                self.cursor_now = None;
                if let Some(win) = self.filer.win_mut(id) {
                    win.ptr = Some((surface_x as f32, surface_y as f32));
                }
                self.filer_hover(id);
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.ptr = None;
                }
                self.filer_hover(id);
            }
            wl_pointer::Event::Motion {
                surface_x, surface_y, ..
            } => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.ptr = Some((surface_x as f32, surface_y as f32));
                }
                self.filer_hover(id);
            }
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => match button {
                0x110 => self.filer_click(id),
                // The mouse's side buttons.
                0x113 => self.filer_back(id),
                0x114 => self.filer_forward(id),
                _ => {}
            },
            wl_pointer::Event::AxisSource { axis_source } => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.wheel = axis_source == WEnum::Value(wl_pointer::AxisSource::Wheel);
                }
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } => {
                let step = if self.filer.win(id).is_some_and(|w| w.wheel) {
                    value as f32 * WHEEL_STEP
                } else {
                    value as f32
                };
                self.filer_scroll(id, step, true);
            }
            wl_pointer::Event::Frame => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.wheel = false;
                }
            }
            _ => {}
        }
    }

    /// Scroll by `by` pixels: fingers move the folder with them, a wheel's
    /// notch (or a key) is eased to.
    fn filer_scroll(&mut self, id: u32, by: f32, from_pointer: bool) {
        let icon_scale = self.icon_scale();
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let max = win.layout(icon_scale).max_scroll(win.items.len());
        win.scroll_to = (win.scroll_to + by).clamp(0.0, max);
        if from_pointer && !win.wheel {
            win.scroll = win.scroll_to;
        }
        self.filer_hover(id);
        self.filer_request_draw(id);
    }

    /// A left press.
    fn filer_click(&mut self, id: u32) {
        let ctrl = self.modifiers.ctrl;
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        match win.hover {
            Spot::Back => self.filer_back(id),
            Spot::Forward => self.filer_forward(id),
            Spot::Crumb(n) => {
                if let Some((_, path)) = win.hits.crumbs.get(n).cloned() {
                    self.filer_go(id, path);
                }
            }
            Spot::Place(n) => {
                if let Some(path) = win.places.get(n).map(|p| p.path.clone()) {
                    self.filer_go(id, path);
                }
            }
            Spot::Item(i) => {
                let Some(path) = win.items.get(i).map(|it| it.path.clone()) else {
                    return;
                };
                let now = Instant::now();
                let twice = win
                    .click
                    .take()
                    .is_some_and(|(at, p)| p == path && now.duration_since(at) <= DOUBLE_CLICK);
                if twice && !ctrl {
                    self.filer_activate(id, i);
                    return;
                }
                win.click = Some((now, path.clone()));
                if ctrl {
                    // One more, or one fewer.
                    if !win.selected.remove(&path) {
                        win.selected.insert(path);
                    }
                    win.cursor = Some(i);
                } else {
                    win.select_only(i);
                }
                self.filer_request_draw(id);
            }
            Spot::Nothing => {
                win.click = None;
                if !win.selected.is_empty() {
                    win.selected.clear();
                    self.filer_request_draw(id);
                }
            }
        }
    }

    // ── the keyboard ────────────────────────────────────────────────────

    /// Put the keys on item `i` (it alone selected) and bring it into view.
    fn filer_walk_to(&mut self, id: u32, i: usize) {
        let icon_scale = self.icon_scale();
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if i >= win.items.len() {
            return;
        }
        win.select_only(i);
        win.scroll_to = win.layout(icon_scale).reveal(i, win.scroll_to, win.items.len());
        self.filer_request_draw(id);
    }

    /// A key while window `id` has the keyboard.
    pub(crate) fn filer_key(&mut self, id: u32, keysym: Keysym, utf8: Option<&str>) {
        let icon_scale = self.icon_scale();
        let (ctrl, alt) = (self.modifiers.ctrl, self.modifiers.alt);
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let (n, cols) = (win.items.len(), win.layout(icon_scale).cols);
        let page = win.layout(icon_scale).body.h * 0.9;
        let at = win.cursor;
        let step = |by: isize| -> Option<usize> {
            match at {
                None => (n > 0).then_some(0),
                Some(i) => Some((i as isize + by).clamp(0, n as isize - 1) as usize).filter(|_| n > 0),
            }
        };
        match keysym {
            Keysym::Left if alt => self.filer_back(id),
            Keysym::Right if alt => self.filer_forward(id),
            Keysym::Up if alt => self.filer_up(id),
            Keysym::BackSpace => self.filer_back(id),
            Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down => {
                let by = match keysym {
                    Keysym::Left => -1,
                    Keysym::Right => 1,
                    Keysym::Up => -(cols as isize),
                    _ => cols as isize,
                };
                // (Up from the first row and down from the last stay put.)
                let to = match at {
                    Some(i) if (i as isize + by) < 0 || (i as isize + by) >= n as isize => Some(i),
                    _ => step(by),
                };
                if let Some(to) = to {
                    self.filer_walk_to(id, to);
                }
            }
            Keysym::Home => self.filer_walk_to(id, 0),
            Keysym::End => self.filer_walk_to(id, n.saturating_sub(1)),
            Keysym::Page_Up => self.filer_scroll(id, -page, false),
            Keysym::Page_Down => self.filer_scroll(id, page, false),
            Keysym::Return | Keysym::KP_Enter => {
                if let Some(i) = at.filter(|&i| win.items.get(i).is_some_and(|it| win.selected.contains(&it.path))) {
                    self.filer_activate(id, i);
                }
            }
            Keysym::Escape => {
                win.typed.clear();
                if !win.selected.is_empty() {
                    win.selected.clear();
                    self.filer_request_draw(id);
                }
            }
            Keysym::F5 => self.filer_list(id),
            Keysym::a if ctrl => {
                win.selected = win.items.iter().map(|it| it.path.clone()).collect();
                self.filer_request_draw(id);
            }
            Keysym::h if ctrl => {
                win.hidden = !win.hidden;
                self.filer_list(id);
            }
            Keysym::r if ctrl => self.filer_list(id),
            Keysym::n if ctrl => {
                let dir = win.dir.clone();
                self.filer_open(dir);
            }
            Keysym::w if ctrl => self.filer_close(id),
            _ => {
                // Typing a name goes to the first thing called that.
                let Some(text) = utf8.filter(|t| !ctrl && !alt && t.chars().all(|c| !c.is_control())) else {
                    return;
                };
                if text.is_empty() {
                    return;
                }
                let now = Instant::now();
                if win.typed_at.is_none_or(|t| now.duration_since(t) > TYPE_PAUSE) {
                    win.typed.clear();
                }
                win.typed.push_str(text);
                win.typed_at = Some(now);
                if let Some(i) = typed_match(&win.items, &win.typed) {
                    self.filer_walk_to(id, i);
                }
            }
        }
    }

    // ── without a pointer ───────────────────────────────────────────────

    /// The `files` verb: alone or with a folder, a new window; else a word
    /// for the newest window (see the top of this file).
    pub(crate) fn filer_command(&mut self, what: &str) -> String {
        let what = what.trim();
        let (verb, rest) = what.split_once(' ').unwrap_or((what, ""));
        let rest = rest.trim();
        let as_path = |text: &str| -> PathBuf {
            let text = text.strip_prefix("file://").unwrap_or(text);
            match text.strip_prefix('~') {
                Some(under) => home().join(under.trim_start_matches('/')),
                None => PathBuf::from(text),
            }
        };
        if what.is_empty() {
            self.filer_open(home());
            return "opened Home".to_owned();
        }
        if verb == "open" || what.starts_with(['/', '~']) || what.starts_with("file://") {
            let dir = as_path(if verb == "open" { rest } else { what });
            self.filer_open(dir.clone());
            return format!("opened {}", dir.display());
        }
        let Some(id) = self.filer.newest() else {
            return "no window".to_owned();
        };
        match verb {
            "go" => {
                let dir = as_path(rest);
                if !dir.is_dir() {
                    return format!("not a folder: {}", dir.display());
                }
                self.filer_go(id, dir);
            }
            "back" => self.filer_back(id),
            "forward" => self.filer_forward(id),
            "up" => self.filer_up(id),
            "close" => self.filer_close(id),
            "hidden" => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.hidden = !win.hidden;
                }
                self.filer_list(id);
            }
            "select" => {
                if let Ok(i) = rest.parse::<usize>() {
                    self.filer_walk_to(id, i);
                }
            }
            "activate" => {
                if let Ok(i) = rest.parse::<usize>() {
                    self.filer_activate(id, i);
                }
            }
            "scroll" => {
                if let Ok(by) = rest.parse::<f32>() {
                    self.filer_scroll(id, by, false);
                }
            }
            "key" => {
                let key = match rest {
                    "left" => Keysym::Left,
                    "right" => Keysym::Right,
                    "up" => Keysym::Up,
                    "down" => Keysym::Down,
                    "enter" => Keysym::Return,
                    "back" => Keysym::BackSpace,
                    "escape" => Keysym::Escape,
                    "home" => Keysym::Home,
                    "end" => Keysym::End,
                    _ => return format!("no key {rest}"),
                };
                self.filer_key(id, key, None);
            }
            "type" => self.filer_key(id, Keysym::NoSymbol, Some(rest)),
            "state" => {}
            _ => return format!("unknown: {what}"),
        }
        let icon_scale = self.icon_scale();
        let Some(win) = self.filer.win(id) else {
            return "closed".to_owned();
        };
        let layout = win.layout(icon_scale);
        format!(
            "window {id} {}x{} on {} — {} items{}, {} cols, scroll {:.0}/{:.0}, selected {}, back {}, forward {}, places {}, layers {}/{}",
            win.size.0,
            win.size.1,
            win.dir.display(),
            win.items.len(),
            if win.loading { " (listing)" } else { "" },
            layout.cols,
            win.scroll_to,
            layout.max_scroll(win.items.len()),
            win.selected.len(),
            win.back.len(),
            win.forward.len(),
            win.places.len(),
            win.uploaded.iter().flatten().count(),
            win.capacity,
        )
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, window: &Window) {
        if let Some(id) = self.filer.id_of(window.wl_surface()) {
            self.filer_close(id);
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        self.filer_configure(window, configure);
    }
}

smithay_client_toolkit::delegate_xdg_shell!(App);
smithay_client_toolkit::delegate_xdg_window!(App);

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> Item {
        Item {
            path: format!("/x/{name}"),
            name: name.to_owned(),
            kind: Kind::File,
            icon: String::new(),
            exec: None,
        }
    }

    #[test]
    fn cells_flow_row_by_row_and_share_the_spare_width() {
        let l = Layout::new((920, 600), 1.0);
        assert_eq!(l.side_w, SIDE_W);
        assert_eq!(l.cols, 6); // (920 − 176 − 20) / 104 = 6.96
        let (a, b, next_row) = (l.cell(0), l.cell(1), l.cell(l.cols));
        assert!((b.x - a.x - l.pitch).abs() < 0.01 && l.pitch > l.cw);
        assert!((next_row.x - a.x).abs() < 0.01 && (next_row.y - a.y - l.ch).abs() < 0.01);
        // As far from the right edge as from the left.
        let last = l.cell(l.cols - 1);
        let (left, right) = (a.x - l.body.x, l.body.x + l.body.w - (last.x + last.w));
        assert!((left - right).abs() < 0.01);
    }

    #[test]
    fn a_narrow_window_has_no_places_and_never_zero_columns() {
        let l = Layout::new((400, 300), 1.0);
        assert_eq!(l.side_w, 0.0);
        assert_eq!(l.cols, 3);
        assert_eq!(Layout::new((60, 60), 2.0).cols, 1);
    }

    #[test]
    fn the_item_under_a_point_follows_the_scroll() {
        let l = Layout::new((920, 600), 1.0);
        let c = l.cell(7);
        let mid = (c.x + c.w / 2.0, c.y + c.h / 2.0);
        assert_eq!(l.index_at(mid, 0.0, 20), Some(7));
        assert_eq!(l.index_at(mid, l.ch, 20), Some(7 + l.cols));
        assert_eq!(l.index_at((mid.0, mid.1 - l.ch), l.ch, 20), Some(7));
        // Past the last item, in the head and in the places: nothing.
        assert_eq!(l.index_at(mid, 0.0, 7), None);
        assert_eq!(l.index_at((mid.0, 10.0), 0.0, 20), None);
        assert_eq!(l.index_at((20.0, mid.1), 0.0, 20), None);
    }

    #[test]
    fn only_the_rows_on_screen_are_in_view_and_a_far_item_is_brought_in() {
        let l = Layout::new((920, 600), 1.0);
        let n = 600;
        let view = l.in_view(0.0, n);
        assert_eq!(view.start, 0);
        assert!(view.end >= l.cols * 6 && view.end <= l.cols * 8);
        let deep = l.in_view(l.ch * 50.0 + PAD, n);
        assert_eq!(deep.start, l.cols * 50);
        // Already whole on screen: the scroll stays; below: the least move.
        assert_eq!(l.reveal(0, 0.0, n), 0.0);
        let to = l.reveal(l.cols * 30, 0.0, n);
        let cell = l.cell(l.cols * 30);
        assert!((cell.y + cell.h - to - (l.body.y + l.body.h) + PAD).abs() < 0.01);
        assert!(l.reveal(n - 1, 0.0, n) <= l.max_scroll(n));
    }

    #[test]
    fn the_owners_folders_are_read_from_user_dirs() {
        let home = Path::new("/home/max");
        let text = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Escritorio\"\nXDG_MUSIC_DIR=\"$HOME/\"\nXDG_VIDEOS_DIR=\"/data/Videos\"\n";
        assert_eq!(user_dir(text, home, "DESKTOP"), Some(PathBuf::from("/home/max/Escritorio")));
        // Set to Home itself (how a folder is switched off): not a place.
        assert_eq!(user_dir(text, home, "MUSIC"), None);
        assert_eq!(user_dir(text, home, "VIDEOS"), Some(PathBuf::from("/data/Videos")));
        assert_eq!(user_dir(text, home, "PICTURES"), None);
    }

    #[test]
    fn the_way_here_starts_at_home_a_stick_or_the_root() {
        let place = |name: &str, path: &str, start: bool| Place {
            name: name.to_owned(),
            path: PathBuf::from(path),
            icon: String::new(),
            gap: false,
            start,
        };
        let places = vec![
            place("Home", "/home/max", true),
            place("Documents", "/home/max/Documents", false),
            place("KINGSTON", "/run/media/max/KINGSTON", true),
        ];
        let names = |dir: &str| -> Vec<String> {
            crumbs(Path::new(dir), &places).into_iter().map(|(n, _)| n).collect()
        };
        assert_eq!(names("/home/max"), ["Home"]);
        // Documents is a step under Home, not a start of its own.
        assert_eq!(names("/home/max/Documents/Work"), ["Home", "Documents", "Work"]);
        assert_eq!(names("/run/media/max/KINGSTON/a"), ["KINGSTON", "a"]);
        assert_eq!(names("/etc/nixos"), ["/", "etc", "nixos"]);
        let steps = crumbs(Path::new("/home/max/a/b"), &places);
        assert_eq!(steps[1].1, PathBuf::from("/home/max/a"));
    }

    #[test]
    fn a_listing_puts_folders_first_and_hides_dot_files_unless_asked() {
        let dir = std::env::temp_dir().join(format!("waverunner-filer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zeta")).unwrap();
        for f in ["b.txt", "A.txt", ".secret"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let names = |hidden| -> Vec<String> {
            list(&dir, hidden).unwrap().into_iter().map(|it| it.name).collect()
        };
        assert_eq!(names(false), ["zeta", "A.txt", "b.txt"]);
        for f in ["b 10.txt", "b 9.txt"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        assert_eq!(names(false), ["zeta", "A.txt", "b 9.txt", "b 10.txt", "b.txt"]);
        for f in ["b 10.txt", "b 9.txt"] {
            std::fs::remove_file(dir.join(f)).unwrap();
        }
        assert_eq!(names(true), ["zeta", ".secret", "A.txt", "b.txt"]);
        assert!(list(&dir.join("nowhere"), false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_sort_by_their_numbers_whatever_the_case() {
        use std::cmp::Ordering::*;
        assert_eq!(by_name("file 2", "file 10"), Less);
        assert_eq!(by_name("File 10", "file 9"), Greater);
        assert_eq!(by_name("a007", "a7b"), Less);
        assert_eq!(by_name("abc", "ABD"), Less);
        assert_eq!(by_name("x", "x1"), Less);
        assert_eq!(by_name("same", "same"), Equal);
        assert_ne!(by_name("Same", "same"), Equal);
    }

    #[test]
    fn a_long_name_takes_two_lines_and_breaks_at_a_mark() {
        // Ten wide a letter: 100 holds ten letters.
        let mut measure = |t: &str| t.chars().count() as f32 * 10.0;
        assert_eq!(two_lines("short.txt", 100.0, &mut measure), ("short.txt".to_owned(), String::new()));
        let (a, b) = two_lines("holiday photos.zip", 100.0, &mut measure);
        assert_eq!((a.as_str(), b.as_str()), ("holiday", "photos.zip"));
        let (a, b) = two_lines("abcdefghijklmnopqrstuvwxyz0123456789", 100.0, &mut measure);
        assert_eq!(a, "abcdefghij");
        assert!(b.ends_with('…') && b.chars().count() <= 10);
    }

    #[test]
    fn typing_goes_to_the_first_name_that_starts_so() {
        let items = [item("Alpha"), item("beta"), item("Bravo")];
        assert_eq!(typed_match(&items, "b"), Some(1));
        assert_eq!(typed_match(&items, "BR"), Some(2));
        assert_eq!(typed_match(&items, "z"), None);
    }
}

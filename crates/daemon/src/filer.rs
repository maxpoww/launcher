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
//! A click selects, a double click opens (a folder in this window, a file
//! in its app), the side buttons of the mouse / Backspace / Alt+arrows go
//! back, forward and up, the arrows walk the icons and typing jumps to a
//! name. The folder is listed off the loop and followed with inotify.
//!
//! A right click opens the desktop's own menu (`desktop_menu::Menu`, with
//! this window's rows — `menu_rows`), drawn in the window: Open · Open in
//! terminal · Cut · Copy · Rename · Move to bin · Properties on a thing,
//! Paste · New folder · Open in terminal on the folder itself. A name is
//! typed in place (`Rename`; the desktop's rules for the ending). The keys
//! a file manager has: Ctrl+C/X/V, Delete, F2, Ctrl+Shift+N.
//! A press that travels lifts the thing (a real Wayland drag, as the
//! desktop's) or, from bare ground, draws a rubber band; a drag let go on a
//! window lands in the folder under it, else in the folder shown.
//! Not here yet: the bin as a place.
//!
//! Pointer-free: `waverunner-ctl files [<folder>]` opens a window;
//! `files go <folder>|back|forward|up|select <n>|add <n>|activate <n>|key
//! <name>|type <letters>|scroll <px>|hidden|menu [<n>]|pick <row>|copy|cut|
//! paste|newfolder|rename <n>|bin|props <n>|close|state` drive the newest
//! one (⚠ `bin`, `paste`, `pick` and a rename's Enter are REAL).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use smithay_client_toolkit::data_device_manager::data_offer::DragOffer;
use smithay_client_toolkit::data_device_manager::data_source::DragSource;
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::seat::keyboard::Keysym;
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{error, info, warn};
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle, WEnum};

use crate::content::{
    GridContent, IconInst, Label, Rect, RectInst, Scene, GRID_CELL_W, GRID_ICON, GRID_ICON_TOP,
    LABEL_FONT_PX, LABEL_LINE_PX, NO_PLATE, PLATE_STATIC,
};
use crate::desktop::{Item, Kind, PLAIN_TEXT, URI_LIST};
use crate::desktop_menu::{Action, Menu, MenuPaint, Row};
use crate::desktop_props::Props;
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

/// THE LIST: a row a thing, under a strip naming its columns.
const LIST_ROW: f32 = 30.0;
const LIST_HEAD: f32 = 26.0;
const LIST_ICON: f32 = 20.0;
/// Where a row's name begins, and its columns' widths (each left out of a
/// window too narrow for it).
const LIST_NAME_X: f32 = 38.0;
const LIST_SIZE_W: f32 = 84.0;
const LIST_DATE_W: f32 = 150.0;

/// The search field at the head's right end.
const FIND_W: f32 = 210.0;

/// The things of a folder whose names hold `find` (whatever the case);
/// all of them for no search.
pub(crate) fn found(all: &[Item], find: Option<&str>) -> Vec<Item> {
    match find.map(str::to_lowercase).filter(|f| !f.is_empty()) {
        Some(find) => all.iter().filter(|it| it.name.to_lowercase().contains(&find)).cloned().collect(),
        None => all.to_vec(),
    }
}

/// What a folder's things are ordered by in a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SortKey {
    /// The folder's own order: by name, or as arranged by hand.
    #[default]
    Name,
    Size,
    Modified,
}

/// What the list says of a thing beside its name.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Meta {
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// What the disk says of each of `items` (asked off the loop, with the
/// listing).
fn meta_of(items: &[Item]) -> HashMap<String, Meta> {
    items
        .iter()
        .filter_map(|it| {
            let m = std::fs::metadata(&it.path).ok()?;
            Some((
                it.path.clone(),
                Meta {
                    size: m.len(),
                    modified: m.modified().ok(),
                },
            ))
        })
        .collect()
}

/// `items` by size or date (folders first, as ever), biggest or newest
/// first when `desc`. By name they stay as they are: the folder's own order.
pub(crate) fn sorted(mut items: Vec<Item>, meta: &HashMap<String, Meta>, key: SortKey, desc: bool) -> Vec<Item> {
    if key == SortKey::Name {
        return items;
    }
    let of = |it: &Item| meta.get(&it.path).copied().unwrap_or_default();
    items.sort_by(|a, b| {
        let by = match key {
            SortKey::Size => of(a).size.cmp(&of(b).size),
            _ => of(a).modified.cmp(&of(b).modified),
        };
        (b.kind == Kind::Folder)
            .cmp(&(a.kind == Kind::Folder))
            .then(if desc { by.reverse() } else { by })
    });
    items
}

/// A drag held this near the field's top or bottom scrolls the folder, at
/// most this many pixels a tick (faster the nearer the edge).
const EDGE_ZONE: f32 = 44.0;
const EDGE_SPEED: f32 = 18.0;
const EDGE_TICK: Duration = Duration::from_millis(16);

/// How far a press travels before it is a drag (or a rubber band).
const DRAG_START: f32 = 6.0;
/// The rubber band: a faint fill and a hairline (the desktop's).
const BAND_FILL: [f32; 4] = [1.0, 1.0, 1.0, 0.07];
const BAND_LINE: [f32; 4] = [1.0, 1.0, 1.0, 0.38];

const WASH_HOVER: [f32; 4] = [1.0, 1.0, 1.0, 0.07];
const WASH_HERE: [f32; 4] = [1.0, 1.0, 1.0, 0.12];
const WASH_SELECTED: [f32; 4] = [1.0, 1.0, 1.0, 0.16];
const SCROLL_THUMB: [f32; 4] = [1.0, 1.0, 1.0, 0.22];

/// The app's own launcher, written where the dock's index finds it: a tile
/// in the grid that opens a window on Home (`waverunner-ctl files`, the ctl
/// that sits beside this daemon), and the class by which the dock knows
/// the windows for it. It claims no file type: opening folders from other
/// apps stays the system file manager's until this one takes that over.
/// Written only when it changes, as the webapps' launchers are.
pub(crate) fn materialize_entry() {
    let dir = crate::webapps::applications_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let ctl = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("waverunner-ctl")))
        .filter(|p| p.exists())
        .map_or_else(|| "waverunner-ctl".to_owned(), |p| p.to_string_lossy().into_owned());
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName=Golem Files\nName[es]=Archivos de Golem\n\
         Comment=Your folders, in a window\nComment[es]=Tus carpetas, en una ventana\n\
         Exec={ctl} files\nIcon=system-file-manager\nStartupWMClass={APP_ID}\n\
         Categories=System;FileManager;\nTerminal=false\n"
    );
    let path = dir.join(format!("{APP_ID}.desktop"));
    if std::fs::read_to_string(&path).is_ok_and(|c| c == contents) {
        return;
    }
    if let Err(e) = std::fs::write(&path, contents) {
        warn!("files: cannot write {}: {e}", path.display());
    }
}

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
    /// The button that turns the folder between icons and a list.
    View,
    /// The magnifier: it opens the search.
    Find,
    /// A column's name in the list's strip (0 name, 1 size, 2 date).
    Col(usize),
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
    /// A list: one column of rows, under a strip naming the columns.
    pub list: bool,
}

impl Layout {
    #[cfg(test)]
    pub fn new((w, h): (u32, u32), icon_scale: f32) -> Self {
        Self::of((w as f32, h as f32), icon_scale, false)
    }

    /// The same for a size that is not whole (a zoomed window's — `Win::virt`).
    pub fn of((w, h): (f32, f32), icon_scale: f32, list: bool) -> Self {
        let side_w = if w >= SIDE_FROM { SIDE_W } else { 0.0 };
        if list {
            // One column as wide as the field: a row is a cell, so all that
            // knows cells (the pointer, the keys, a band, a drag's room)
            // knows rows.
            let top = HEAD_H + LIST_HEAD;
            let body = Rect::new(side_w, top, (w - side_w).max(1.0), (h - top).max(1.0));
            let room = (body.w - 2.0 * PAD).max(1.0);
            return Self {
                side_w,
                body,
                cols: 1,
                pitch: room,
                cw: room,
                ch: LIST_ROW,
                list,
            };
        }
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
            list,
        }
    }

    /// A thing's picture in its cell: the grid's icon, centred under the
    /// cell's top; a row's small one, at its left.
    pub fn icon_in(&self, cell: &Rect, icon_scale: f32) -> Rect {
        if self.list {
            return Rect::new(cell.x + 9.0, cell.y + (cell.h - LIST_ICON) / 2.0, LIST_ICON, LIST_ICON);
        }
        let size = GRID_ICON * icon_scale;
        Rect::new(cell.x + (cell.w - size) / 2.0, cell.y + GRID_ICON_TOP * icon_scale, size, size)
    }

    /// The list's columns in a row `cell`: where the size ends (right
    /// aligned) and where the date begins, each only where there is room.
    pub fn columns(&self, cell: &Rect) -> (Option<f32>, Option<f32>) {
        let date = (cell.w > 470.0).then_some(cell.x + cell.w - LIST_DATE_W);
        let size = (cell.w > 300.0).then(|| date.unwrap_or(cell.x + cell.w) - 14.0);
        (size, date)
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

    /// Where a thing stands at display slot `idx` — between two cells while
    /// it glides from one to the next (the Apps grid's `cell_pos`).
    pub fn cell_at(&self, idx: f32) -> Rect {
        let i0 = idx.max(0.0).floor() as usize;
        let frac = (idx - i0 as f32).clamp(0.0, 1.0);
        let a = self.cell(i0);
        if frac <= f32::EPSILON {
            return a;
        }
        let b = self.cell(i0 + 1);
        Rect::new(a.x + (b.x - a.x) * frac, a.y + (b.y - a.y) * frac, a.w, a.h)
    }

    /// The display slot under `pos` (past the last one too) and how far
    /// across it the pointer is, the field scrolled by `scroll`.
    pub fn slot_at(&self, pos: (f32, f32), scroll: f32) -> (usize, f32) {
        let x = ((pos.0 - self.body.x - PAD) / self.pitch).clamp(0.0, self.cols as f32 - 0.001);
        let y = ((pos.1 - self.body.y - PAD + scroll) / self.ch).max(0.0);
        // (Things follow one another across a row of icons, down a list.)
        (y as usize * self.cols + x as usize, if self.list { y.fract() } else { x.fract() })
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

/// A rubber band: where it began, where the pointer is, what was selected
/// before it.
type Band = ((f32, f32), (f32, f32), HashSet<String>);

/// A press of the left button in the icons' field, until it is a click, a
/// drag of the thing under it, or a rubber band from bare ground.
#[derive(Debug, Clone, PartialEq)]
struct Press {
    /// The thing pressed on (its path), if any.
    path: Option<String>,
    at: (f32, f32),
    /// The press's serial: a drag is started with it.
    serial: u32,
    /// Pressed on one of SEVERAL selected things: the group is kept for a
    /// drag, and only a plain click (no travel) leaves this one alone.
    of_group: bool,
}

/// Files of a window in hand: a real Wayland drag of ours, as the desktop's
/// is. Where it goes comes back as `wl_data_device` events on whichever of
/// our surfaces it crosses; another app reads the files from the source.
pub(crate) struct Lifted {
    win: u32,
    /// The grabbed one first.
    paths: Vec<String>,
    source: DragSource,
    /// The drag image, held for the drag's life.
    _icon: Option<crate::desktop::DragIcon>,
    action: DndAction,
}

/// A drag hovering a file window (anyone's: another app's, the desktop's,
/// a file window's own).
struct Hovering {
    win: u32,
    offer: DragOffer,
}

/// The items a rubber band from `a` to `b` touches (both in the field's
/// own, unscrolled coordinates): those whose cell, less its margins, it
/// crosses.
pub(crate) fn band_hits(layout: &Layout, a: (f32, f32), b: (f32, f32), n: usize) -> Vec<usize> {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    (0..n)
        .filter(|&i| {
            let c = layout.cell(i);
            let (cx0, cx1) = (c.x + 10.0, c.x + c.w - 10.0);
            let (cy0, cy1) = (c.y + 6.0, c.y + c.h - 6.0);
            x0 < cx1 && cx0 < x1 && y0 < cy1 && cy0 < y1
        })
        .collect()
}

/// MAKE ROOM (Max, 2026-10-13: "que hagan espacio como en el grid de
/// aplicaciones"): the display slot of the thing `rank`th among those NOT in
/// hand, while a gap `n` slots wide stands open at `gap` — the Apps grid's
/// reflow (`pages::shifted_slot`, which this equals for one icon on one
/// page), for a group as well.
pub(crate) fn parted(rank: usize, gap: usize, n: usize) -> usize {
    if rank >= gap {
        rank + n
    } else {
        rank
    }
}

/// Where the gap goes for a pointer over display slot `d`, `fx` of the way
/// across it: before or after the thing standing there, by which half of
/// it the pointer is on (the grid's rule for an icon coming in); over the
/// gap itself it stays. `m` things are not in hand.
pub(crate) fn gap_at(d: usize, fx: f32, gap_now: usize, n: usize, m: usize) -> usize {
    let rank = if d < gap_now {
        d
    } else if d >= gap_now + n {
        d - n
    } else {
        return gap_now.min(m);
    };
    (rank + usize::from(fx >= 0.5)).min(m)
}

/// A folder's things in the order its owner gave them: those named in
/// `order` first, as it says, then whatever came since, as listed.
pub(crate) fn arranged(items: Vec<Item>, order: &[String]) -> Vec<Item> {
    let place: HashMap<&str, usize> = order.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    let mut keyed: Vec<(usize, usize, Item)> = items
        .into_iter()
        .enumerate()
        .map(|(i, it)| {
            let at = place.get(file_name(Path::new(&it.path)).as_str()).copied().unwrap_or(usize::MAX);
            (at, i, it)
        })
        .collect();
    keyed.sort_by_key(|(at, i, _)| (*at, *i));
    keyed.into_iter().map(|(_, _, it)| it).collect()
}

/// The folders arranged by hand: each one's names in its owner's order
/// (`files-order.json` in the data dir), read at first use.
#[derive(Default)]
struct Orders {
    loaded: bool,
    map: HashMap<String, Vec<String>>,
}

impl Orders {
    /// Folders remembered at most: past it, those that are gone go.
    const MAX: usize = 400;

    fn load(&mut self) {
        if !self.loaded {
            self.loaded = true;
            self.map = crate::persist::read_json(&crate::persist::data_path("files-order.json")).unwrap_or_default();
        }
    }

    fn of(&mut self, dir: &Path) -> Option<&Vec<String>> {
        self.load();
        self.map.get(dir.to_string_lossy().as_ref())
    }

    /// Give `dir` an order, or (`None`) take it back to by-name.
    fn set(&mut self, dir: &Path, order: Option<Vec<String>>) {
        self.load();
        let key = dir.to_string_lossy().into_owned();
        match order {
            Some(order) => {
                self.map.insert(key, order);
            }
            None => {
                self.map.remove(&key);
            }
        }
        if self.map.len() > Self::MAX {
            self.map.retain(|dir, _| Path::new(dir).is_dir());
        }
        crate::persist::write_json("files-order", &crate::persist::data_path("files-order.json"), &self.map);
    }
}

/// A name being typed in place.
#[derive(Debug, Clone, PartialEq)]
struct Rename {
    path: String,
    text: String,
    /// The whole name is selected (as it opens): the first key replaces
    /// it, a Backspace clears it.
    all: bool,
}

/// What a right click was made on: what its menu is made from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Asked {
    /// On a thing (else on the folder's own ground)…
    pub on_item: bool,
    /// …that is one of several selected.
    pub many: bool,
    /// Files are on the clipboard.
    pub paste: bool,
    pub bin: InBin,
    /// The folder is arranged by hand.
    pub by_hand: bool,
    /// The thing is an archive (a zip, a tarball…).
    pub archive: bool,
}

/// Whether `name` is an archive the system's archiver opens.
pub(crate) fn is_archive(name: &str) -> bool {
    let name = name.to_lowercase();
    [".zip", ".tar", ".tgz", ".gz", ".xz", ".txz", ".zst", ".bz2", ".tbz2", ".7z", ".rar"]
        .iter()
        .any(|ext| name.ends_with(ext))
}

/// Where a window is with respect to the bin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InBin {
    /// An ordinary folder.
    No,
    /// The bin itself: what was thrown away, each thing able to go back.
    Top,
    /// A folder that was thrown away, opened: only looking and copying.
    Deeper,
}

/// The bin's folder (`Trash/files` — what a window on the bin shows).
pub(crate) fn bin_root() -> PathBuf {
    crate::trash::Trash::home().files_root()
}

/// What is in the bin, newest first, each under the name it had.
fn list_bin() -> Vec<Item> {
    let trash = crate::trash::Trash::home();
    trash
        .list()
        .into_iter()
        .map(|t| {
            let path = trash.file_path(&t.name);
            let mut item = crate::desktop::item_for(path, t.display_name().to_owned(), t.is_dir);
            // (A thrown-away shortcut is a file: nothing runs from the bin.)
            if item.kind == Kind::Launcher {
                item.kind = Kind::File;
                item.exec = None;
                item.name = t.display_name().to_owned();
                item.icon = crate::desktop::asset_key("asset-file");
            }
            item
        })
        .collect()
}

/// The rows of a right click: on one thing, on one of `many` selected, or
/// on the folder itself (`paste`: files are on the clipboard) — and in the
/// bin, what the bin is for.
pub(crate) fn menu_rows(asked: Asked) -> Vec<Row> {
    let Asked {
        on_item,
        many,
        paste,
        bin,
        by_hand,
        archive,
    } = asked;
    let row = |label: &'static str, action: Action| Row::Item {
        label,
        action,
        danger: false,
    };
    let danger = |label: &'static str, action: Action| Row::Item {
        label,
        action,
        danger: true,
    };
    match (bin, on_item) {
        (InBin::Top, true) => {
            return vec![
                row(tr("Open"), Action::Open),
                Row::Sep,
                row(tr("Put back"), Action::PutBack),
                Row::Sep,
                danger(tr("Delete for good"), Action::Erase),
            ]
        }
        (InBin::Top, false) => return vec![danger(tr("Empty the bin"), Action::EmptyBin)],
        (InBin::Deeper, true) => return vec![row(tr("Open"), Action::Open), Row::Sep, row(tr("Copy"), Action::Copy)],
        (InBin::Deeper, false) => return Vec::new(),
        (InBin::No, _) => {}
    }
    let bin = Row::Item {
        label: tr("Move to bin"),
        action: Action::MoveToBin,
        danger: true,
    };
    if on_item && many {
        vec![
            row(tr("Open"), Action::Open),
            Row::Sep,
            row(tr("Cut"), Action::Cut),
            row(tr("Copy"), Action::Copy),
            Row::Sep,
            row(tr("Compress"), Action::Compress),
            Row::Sep,
            bin,
        ]
    } else if on_item {
        let mut rows = vec![row(tr("Open"), Action::Open), row(tr("Open in terminal"), Action::OpenTerminal)];
        if archive {
            rows.push(row(tr("Extract here"), Action::Extract));
        }
        rows.extend([
            Row::Sep,
            row(tr("Cut"), Action::Cut),
            row(tr("Copy"), Action::Copy),
            Row::Sep,
            row(tr("Rename"), Action::Rename),
            row(tr("Compress"), Action::Compress),
            Row::Sep,
            bin,
            Row::Sep,
            // Last, and opened by HOVER, as on the desktop.
            row(tr("Properties"), Action::Properties),
        ]);
        rows
    } else {
        let mut rows = Vec::new();
        if paste {
            rows.extend([row(tr("Paste"), Action::Paste), Row::Sep]);
        }
        rows.extend([
            row(tr("New folder"), Action::NewFolder),
            Row::Sep,
            row(tr("Open in terminal"), Action::OpenTerminal),
        ]);
        // A folder arranged by hand can go back to being in order of name
        // (the desktop's own word for it).
        if by_hand {
            rows.extend([Row::Sep, row(tr("Clean up"), Action::CleanUp)]);
        }
        rows
    }
}

/// The clickable parts whose places are only known once drawn.
#[derive(Debug, Default)]
struct Hits {
    back: Rect,
    forward: Rect,
    crumbs: Vec<(Rect, PathBuf)>,
    places: Vec<Rect>,
    view: Rect,
    find: Rect,
    /// The list's column names.
    cols: Vec<Rect>,
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
    /// The owner's zoom of the file windows (`App::files_zoom`), kept here
    /// for the layout.
    zoom: f32,
    /// Shown as a list (`settings.files_list`, one choice for every window).
    list: bool,
    /// What the things are ordered by here, biggest or newest first when
    /// the flag is set; and what the disk says of each.
    sort: (SortKey, bool),
    meta: HashMap<String, Meta>,
    /// A search of this folder: what is typed (the field is out while this
    /// is `Some`), and everything the folder holds — `items` is then only
    /// what was found.
    find: Option<String>,
    all: Vec<Item>,
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

    /// The menu that is up, and the thing it was opened on (its place in
    /// the list moves when the folder is read again).
    menu: Option<Menu>,
    menu_path: Option<String>,
    props: Option<Props>,
    rename: Option<Rename>,
    /// A thing whose name is to be typed as soon as it is listed (a folder
    /// just made).
    rename_next: Option<String>,

    /// A row that cannot be undone was clicked once: the second click does it.
    armed: Option<Action>,
    /// The place a hovering drag would be let go onto.
    drop_place: Option<usize>,
    /// A hovering drag is near the field's top (< 0) or bottom (> 0) edge:
    /// how fast the folder scrolls for it, −1..1; where the drag is (real
    /// pixels), to read again what is under it as the folder moves; and
    /// whether the timer that does it is running.
    edge: f32,
    edge_at: (f32, f32),
    edge_timer: bool,
    /// Where each thing is drawn, as a display slot (its own place in the
    /// list at rest; easing to make room while a drag is over the window).
    slide: Vec<f32>,
    /// The gap a hovering drag has open, counted among the things not in
    /// hand: where what it carries would land.
    gap: Option<usize>,
    press: Option<Press>,
    /// A rubber band being drawn: where it began and where the pointer is,
    /// in the field's unscrolled coordinates, and what was selected before
    /// it (kept when Ctrl is held).
    band: Option<Band>,
    /// The folder a hovering drag would be let go into.
    drop_on: Option<usize>,
}

impl Win {
    /// The room made is where what comes will stay: the names not in hand
    /// (`carried`), in order, before and after the gap — for a drop into
    /// the folder shown. The gap is closed.
    fn arrangement(&mut self, dir: &Path, carried: &[String]) -> Option<(Vec<String>, Vec<String>)> {
        let g = self.gap.take().filter(|_| dir == self.dir)?;
        let names: Vec<String> = self
            .items
            .iter()
            .filter(|it| !carried.contains(&it.path))
            .map(|it| file_name(Path::new(&it.path)))
            .collect();
        let g = g.min(names.len());
        Some((names[..g].to_vec(), names[g..].to_vec()))
    }

    /// Whether this window is on the bin, or in a folder thrown away.
    fn in_bin(&self) -> InBin {
        let bin = bin_root();
        if self.dir == bin {
            InBin::Top
        } else if self.dir.starts_with(&bin) {
            InBin::Deeper
        } else {
            InBin::No
        }
    }

    fn layout(&self, icon_scale: f32) -> Layout {
        Layout::of(self.virt(), icon_scale, self.list)
    }

    /// The size everything is laid out in: the window's, as many times
    /// smaller as it is zoomed — the renderer draws a logical pixel that
    /// much bigger, so no layout code knows about the zoom. The pointer is
    /// brought into it the same way (`filer_pointer`).
    fn virt(&self) -> (f32, f32) {
        (self.size.0 as f32 / self.zoom, self.size.1 as f32 / self.zoom)
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
        if self.hits.view.contains(pos) {
            return Spot::View;
        }
        if self.hits.find.contains(pos) {
            return Spot::Find;
        }
        if let Some(i) = self.hits.cols.iter().position(|r| r.contains(pos)) {
            return Spot::Col(i);
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
    /// Files in hand, lifted from one of the windows.
    drag: Option<Lifted>,
    /// A drag over one of the windows.
    dnd: Option<Hovering>,
    orders: Orders,
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
            drag: None,
            dnd: None,
            orders: Orders::default(),
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

    /// Files of a window are in hand.
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The files in hand.
    pub fn drag_paths(&self) -> Vec<String> {
        self.drag.as_ref().map_or_else(Vec::new, |d| d.paths.clone())
    }

    pub fn is_drag_source(&self, source: &wayland_client::protocol::wl_data_source::WlDataSource) -> bool {
        self.drag.as_ref().is_some_and(|d| d.source.inner() == source)
    }

    /// A drag is over one of the windows.
    pub fn dnd_active(&self) -> bool {
        self.dnd.is_some()
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
pub(crate) fn places(home: &Path, user_dirs: &str, bin: &Path, volumes: &[crate::mounts::Mounted]) -> Vec<Place> {
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
    out.push(Place {
        name: tr("Recycle Bin").to_owned(),
        path: bin.to_path_buf(),
        icon: "asset:user-trash".to_owned(),
        gap: false,
        start: true,
    });
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
            zoom: self.files_zoom(),
            list: self.settings.files_list,
            sort: (SortKey::Name, false),
            meta: HashMap::new(),
            find: None,
            all: Vec::new(),
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
            menu: None,
            menu_path: None,
            props: None,
            rename: None,
            rename_next: None,
            armed: None,
            drop_place: None,
            edge: 0.0,
            edge_at: (0.0, 0.0),
            edge_timer: false,
            slide: Vec::new(),
            gap: None,
            press: None,
            band: None,
            drop_on: None,
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
        let list = places(&home, &user_dirs, &bin_root(), &self.desktop.volumes);
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
        win.menu = None;
        win.menu_path = None;
        win.props = None;
        win.rename = None;
        win.press = None;
        win.band = None;
        win.drop_on = None;
        win.gap = None;
        win.edge = 0.0;
        win.slide.clear();
        win.find = None;
        win.all.clear();
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
            // (The bin is read through its own records: names, newest first.)
            move || {
                let items = if dir == bin_root() { list_bin() } else { list(&dir, hidden)? };
                let meta = meta_of(&items);
                Ok((items, meta))
            },
            move |app, listed| app.filer_listed(id, generation, listed),
        );
    }

    fn filer_listed(
        &mut self,
        id: u32,
        generation: u64,
        listed: Result<(Vec<Item>, HashMap<String, Meta>), std::io::Error>,
    ) {
        let icon_scale = self.icon_scale();
        let (listed, meta) = match listed {
            Ok((items, meta)) => (Ok(items), meta),
            Err(e) => (Err(e), HashMap::new()),
        };
        // (By size or by date, in this window, over whatever order the
        // folder has.)
        let sort = self.filer.win(id).map_or((SortKey::Name, false), |w| w.sort);
        if let Some(win) = self.filer.win_mut(id).filter(|w| w.generation == generation) {
            win.meta = meta.clone();
        }
        // A folder arranged by hand is shown as it was left.
        let order = self
            .filer
            .win(id)
            .map(|w| w.dir.clone())
            .and_then(|dir| self.filer.orders.of(&dir).cloned());
        let listed = listed.map(|items| match &order {
            Some(order) => arranged(items, order),
            None => items,
        });
        let listed = listed.map(|items| sorted(items, &meta, sort.0, sort.1));
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
                win.items = found(&items, win.find.as_deref());
                win.all = items;
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
        // What is up holds a thing by its path: carried over, or let go of
        // if the thing went — Enter in a name must never name another.
        if win.rename.as_ref().is_some_and(|r| !present.contains(r.path.as_str())) {
            win.rename = None;
        }
        if let Some(path) = win.menu_path.clone() {
            match win.items.iter().position(|it| it.path == path) {
                Some(i) => {
                    if let Some(menu) = win.menu.as_mut() {
                        menu.item = Some(i);
                    }
                }
                None => {
                    win.menu = None;
                    win.menu_path = None;
                }
            }
        }
        if win.props.as_ref().is_some_and(|p| !present.contains(p.path.as_str())) {
            win.props = None;
        }
        win.drop_on = None;
        if win.press.as_ref().is_some_and(|p| p.path.as_ref().is_some_and(|p| !present.contains(p.as_str()))) {
            win.press = None;
        }
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
        // A folder just made: its name is typed at once.
        let named = win
            .rename_next
            .take()
            .and_then(|p| win.items.iter().position(|it| it.path == p));
        self.filer_request_draw(id);
        if let Some(i) = named {
            self.filer_walk_to(id, i);
            self.filer_begin_rename(id, i);
        }
    }

    /// The selected things' paths, in the order they are shown.
    fn filer_selected(&self, id: u32) -> Vec<String> {
        self.filer.win(id).map_or_else(Vec::new, |w| {
            w.items
                .iter()
                .filter(|it| w.selected.contains(&it.path))
                .map(|it| it.path.clone())
                .collect()
        })
    }

    /// A right click at `at`: the menu for the thing under it (which is
    /// selected if it was not — a group stays a group), or the folder's.
    fn filer_open_menu(&mut self, id: u32, at: (f32, f32), item: Option<usize>) {
        let paste = self.clipboard_files().is_some();
        let by_hand = self
            .filer
            .win(id)
            .map(|w| w.dir.clone())
            .is_some_and(|dir| self.filer.orders.of(&dir).is_some());
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let (w, h) = win.virt();
        let path = item.and_then(|i| win.items.get(i)).map(|it| it.path.clone());
        match (&path, item) {
            (Some(path), Some(i)) => {
                if !win.selected.contains(path) {
                    win.select_only(i);
                }
            }
            _ => win.selected.clear(),
        }
        let many = path.is_some() && win.selected.len() > 1;
        let archive = item
            .and_then(|i| win.items.get(i))
            .is_some_and(|it| it.kind == Kind::File && is_archive(&it.name));
        let rows = menu_rows(Asked {
            on_item: path.is_some(),
            many,
            paste,
            bin: win.in_bin(),
            by_hand,
            archive,
        });
        if rows.is_empty() {
            return;
        }
        win.armed = None;
        win.menu = Some(Menu::open(item.filter(|_| path.is_some()), many, at, w, h).with_rows(rows, h));
        win.menu_path = path;
        win.props = None;
        self.filer_request_draw(id);
    }

    /// A row of the menu was chosen (or its key pressed): on the thing it
    /// was opened on, else on what is selected.
    fn filer_act(&mut self, id: u32, action: Action, item: Option<usize>, at: (f32, f32)) {
        let selected = self.filer_selected(id);
        let Some(win) = self.filer.win(id) else {
            return;
        };
        let dir = win.dir.clone();
        let bin = win.in_bin();
        // In the bin nothing is made, named, moved in or thrown away again.
        if bin != InBin::No
            && matches!(
                action,
                Action::Cut
                    | Action::Paste
                    | Action::Rename
                    | Action::MoveToBin
                    | Action::NewFolder
                    | Action::Extract
                    | Action::Compress
            )
        {
            return;
        }
        match action {
            Action::PutBack | Action::Erase if bin == InBin::Top => {
                let trash = crate::trash::Trash::home();
                let known = trash.list();
                for path in selected {
                    let name = file_name(Path::new(&path));
                    if action == Action::Erase {
                        info!("files: {name} deleted for good");
                        if let Err(e) = trash.erase(&name) {
                            warn!("files: cannot delete {name}: {e}");
                        }
                        continue;
                    }
                    // Back where it was — never over something that has
                    // taken its place since.
                    let Some(was) = known.iter().find(|t| t.name == name) else {
                        continue;
                    };
                    if was.original_path.exists() {
                        let body = format!("{} {}", was.display_name(), tr("is already there: it stays in the bin"));
                        std::thread::spawn(move || crate::desktop_send_notify(&body));
                        continue;
                    }
                    match trash.restore(&name) {
                        Ok(to) => info!("files: {name} put back at {}", to.display()),
                        Err(e) => warn!("files: cannot put {name} back: {e}"),
                    }
                }
                self.filer_list(id);
            }
            Action::EmptyBin if bin == InBin::Top => {
                info!("files: the bin is emptied");
                if let Err(e) = crate::trash::Trash::home().empty() {
                    warn!("files: the bin could not be emptied whole: {e}");
                }
                self.filer_list(id);
                self.schedule_frame();
            }
            Action::Open => {
                let all: Vec<usize> = win
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, it)| win.selected.contains(&it.path))
                    .map(|(i, _)| i)
                    .collect();
                match (all.as_slice(), item) {
                    ([one], _) => self.filer_activate(id, *one),
                    ([], Some(i)) => self.filer_activate(id, i),
                    // Several: each file in its app; this window stays here.
                    (many, _) => {
                        let files: Vec<usize> = many
                            .iter()
                            .copied()
                            .filter(|&i| win.items.get(i).is_some_and(|it| !matches!(it.kind, Kind::Folder | Kind::Volume)))
                            .collect();
                        for i in files {
                            self.filer_activate(id, i);
                        }
                    }
                }
            }
            Action::OpenTerminal => {
                // In the folder clicked, else in the one shown.
                let at_dir = item
                    .and_then(|i| win.items.get(i))
                    .filter(|it| matches!(it.kind, Kind::Folder | Kind::Volume))
                    .map_or_else(|| dir.to_string_lossy().into_owned(), |it| it.path.clone());
                let exec = format!("cd {} && exec {}", launch::shell_quote(&at_dir), self.config.launch.terminal);
                info!("files: terminal at {at_dir}");
                if let Err(e) = launch::launch(&exec, false, &self.config.launch.terminal) {
                    error!("files: terminal launch failed: {e:#}");
                }
            }
            Action::Cut | Action::Copy => {
                if selected.is_empty() {
                    return;
                }
                let cut = action == Action::Cut;
                info!("files: {} {}", if cut { "cut" } else { "copied" }, selected.join(", "));
                self.serve_files(&selected, cut);
            }
            Action::Paste => self.filer_paste(id),
            Action::Rename => {
                let i = item.or_else(|| win.items.iter().position(|it| selected.first() == Some(&it.path)));
                if let Some(i) = i {
                    self.filer_begin_rename(id, i);
                }
            }
            Action::MoveToBin => {
                for path in selected {
                    info!("files: {path} → Recycle Bin");
                    self.trash_file(&path); // the folder's watch takes it off the window
                }
            }
            // Archives are the system archiver's work (File Roller, which
            // Golem ships): it shows how far it is and asks what it must.
            Action::Extract | Action::Compress => {
                if selected.is_empty() {
                    return;
                }
                let files: Vec<String> = selected.iter().map(|p| launch::shell_quote(p)).collect();
                let exec = if action == Action::Extract {
                    format!("file-roller --extract-here {}", files.join(" "))
                } else {
                    format!("file-roller --add {}", files.join(" "))
                };
                info!("files: {exec}");
                if let Err(e) = launch::launch(&exec, false, &self.config.launch.terminal) {
                    error!("files: the archiver could not be started: {e:#}");
                }
            }
            Action::NewFolder => self.filer_new_folder(id),
            Action::CleanUp => {
                info!("files: {} is in order of name again", dir.display());
                self.filer.orders.set(&dir, None);
                self.filer_list(id);
            }
            Action::Properties => {
                if let Some(i) = item {
                    self.filer_open_props(id, i, at, None);
                }
            }
            _ => {}
        }
    }

    /// The clipboard's files into the folder shown: copied, or moved if
    /// they were cut. Off the loop; what arrived comes up selected.
    fn filer_paste(&mut self, id: u32) {
        let Some((paths, cut)) = self.clipboard_files() else {
            return;
        };
        let Some(dir) = self.filer.win(id).map(|w| w.dir.clone()) else {
            return;
        };
        let into = dir.clone();
        let task = self.filer_copy_task(&paths, &dir, cut, cut);
        self.desktop_off_loop(
            move || {
                use crate::desktop_send::{put_metered, weigh, Meter};
                let total = task.as_ref().map_or(0, |_| weigh(&paths));
                let tell = |done, total| {
                    if let Some(task) = task.as_ref() {
                        task.set(done, total);
                    }
                };
                let stopped = || task.as_ref().is_some_and(|t| t.cancelled());
                let mut meter = Meter::new(total, &tell, &stopped);
                put_metered(&paths, &into, cut, task.is_some().then_some(&mut meter))
            },
            move |app, (brought, failed)| {
                app.filer_say_failed(failed);
                info!(
                    "files: pasted {} into {} ({}; {failed} failed)",
                    brought.len(),
                    dir.display(),
                    if cut { "moved" } else { "copied" }
                );
                let now: Vec<String> = brought.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                if cut {
                    // The same files where they are now, as a plain copy: a
                    // second paste must not look for what has moved.
                    app.serve_files(&now, false);
                }
                if let Some(win) = app.filer.win_mut(id).filter(|w| w.dir == dir) {
                    win.reveal = now.first().cloned();
                    app.filer_list(id);
                }
            },
        );
    }

    /// A task on the bar for bringing `paths` into `dir`, when bytes will be
    /// copied (`moving`: a move is asked for — on one disk that is a rename
    /// and no task; `says_move`: the words say "Moving"). The worker
    /// reports to it and reads its Cancel.
    fn filer_copy_task(
        &mut self,
        paths: &[PathBuf],
        dir: &Path,
        moving: bool,
        says_move: bool,
    ) -> Option<crate::tasks::TaskHandle> {
        if !crate::desktop_send::will_copy(paths, dir, moving) {
            return None;
        }
        let what = match paths {
            [one] => file_name(one),
            many => crate::i18n::fill(tr("{n} things"), &[("n", &many.len())]),
        };
        let place = self
            .filer
            .wins
            .iter()
            .flat_map(|w| w.places.iter())
            .find(|p| p.path == dir)
            .map_or_else(|| file_name(dir), |p| p.name.clone());
        let words = if says_move {
            tr("Moving {what} to {where}")
        } else {
            tr("Copying {what} to {where}")
        };
        Some(self.task_begin(&crate::i18n::fill(words, &[("what", &what), ("where", &place)])))
    }

    /// Some things could not be copied: said once, as a notification.
    fn filer_say_failed(&mut self, failed: usize) {
        if failed > 0 {
            let body = crate::i18n::fill(tr("{n} could not be copied"), &[("n", &failed)]);
            std::thread::spawn(move || crate::desktop_send_notify(&body));
        }
    }

    /// A new, empty folder here, named `untitled folder` (then `untitled
    /// folder 2`…), its name opened for typing once it is listed.
    fn filer_new_folder(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let dir = win.dir.clone();
        let path = (1..)
            .map(|n| {
                if n == 1 {
                    dir.join("untitled folder")
                } else {
                    dir.join(format!("untitled folder {n}"))
                }
            })
            .find(|p| !p.exists())
            .unwrap_or_else(|| dir.join("untitled folder"));
        if let Err(e) = std::fs::create_dir(&path) {
            error!("files: cannot make {}: {e}", path.display());
            return;
        }
        info!("files: new folder {}", path.display());
        win.rename_next = Some(path.to_string_lossy().into_owned());
        self.filer_list(id);
    }

    /// Open item `i`'s name for typing, the whole of it selected. (The
    /// window has the keyboard already: it is a window.)
    fn filer_begin_rename(&mut self, id: u32, i: usize) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(item) = win.items.get(i) else {
            return;
        };
        if item.kind == Kind::Volume {
            return;
        }
        // (A shortcut is renamed by its file, not its `Name=`.)
        let (path, text) = (item.path.clone(), file_name(Path::new(&item.path)));
        info!("files: renaming {text}");
        win.select_only(i);
        win.rename = Some(Rename { path, text, all: true });
        self.filer_request_draw(id);
    }

    /// The name is settled: kept (`commit`, if it changed and is free) or
    /// left as it was.
    pub(crate) fn filer_end_rename(&mut self, id: u32, commit: bool) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(rename) = win.rename.take() else {
            return;
        };
        let old = PathBuf::from(&rename.path);
        let is_file = win
            .items
            .iter()
            .find(|it| it.path == rename.path)
            .is_some_and(|it| it.kind == Kind::File);
        let name = crate::desktop::typed_name(&rename.text, &old, is_file);
        if let Some(new) = commit.then(|| crate::desktop::rename_file(&old, &name)).flatten() {
            // In a folder arranged by hand it keeps its place under its new name.
            let dir = win.dir.clone();
            let (was, now) = (file_name(&old), file_name(&new));
            if let Some(mut order) = self.filer.orders.of(&dir).cloned() {
                for n in order.iter_mut().filter(|n| **n == was) {
                    *n = now.clone();
                }
                self.filer.orders.set(&dir, Some(order));
            }
            let Some(win) = self.filer.win_mut(id) else {
                return;
            };
            let new = new.to_string_lossy().into_owned();
            win.selected.clear();
            win.reveal = Some(new);
            self.filer_list(id);
        }
        self.filer_request_draw(id);
    }

    /// A key while a name is being typed: text goes in (the first key
    /// replaces the selected name), Backspace takes out, Enter keeps,
    /// Escape leaves the old name.
    fn filer_rename_key(&mut self, id: u32, keysym: Keysym, utf8: Option<&str>) {
        let ctrl = self.modifiers.ctrl;
        let Some(rename) = self.filer.win_mut(id).and_then(|w| w.rename.as_mut()) else {
            return;
        };
        match keysym {
            Keysym::Return | Keysym::KP_Enter => return self.filer_end_rename(id, true),
            Keysym::Escape => return self.filer_end_rename(id, false),
            Keysym::BackSpace => {
                if rename.all {
                    rename.text.clear();
                    rename.all = false;
                } else {
                    rename.text.pop();
                }
            }
            _ => {
                let Some(text) = utf8.filter(|t| !ctrl && !t.is_empty() && !t.chars().any(char::is_control)) else {
                    return;
                };
                if rename.all {
                    rename.text.clear();
                    rename.all = false;
                }
                rename.text.push_str(text);
            }
        }
        self.filer_request_draw(id);
    }

    /// The Properties box for item `i`: grown out of `menu` when it comes
    /// from its last row, by the pointer otherwise. A folder is weighed on
    /// a thread.
    fn filer_open_props(&mut self, id: u32, i: usize, at: (f32, f32), menu: Option<Menu>) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(item) = win.items.get(i) else {
            return;
        };
        let (w, h) = win.virt();
        let props = Props::open(item, at, menu, w, h, &home());
        if item.kind == Kind::Folder {
            let path = item.path.clone();
            let (tx, rx) = calloop::channel::channel::<(String, u64, u64)>();
            std::thread::spawn(move || {
                let (bytes, files) = crate::desktop_props::folder_size(Path::new(&path));
                let _ = tx.send((path, bytes, files));
            });
            let _ = self.loop_handle.insert_source(rx, move |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg((path, bytes, files)) = event {
                    if let Some(p) = app.filer.win_mut(id).and_then(|w| w.props.as_mut()).filter(|p| p.path == path) {
                        p.set_size(bytes, files);
                        app.filer_request_draw(id);
                    }
                }
            });
        }
        info!("files: properties of {}", props.title);
        if let Some(win) = self.filer.win_mut(id) {
            win.menu = None;
            win.props = Some(props);
        }
        self.filer_request_draw(id);
    }

    /// The menu's and the box's paint in a window: the boxes' own surface,
    /// laid solid on the window's ground (there is no glass inside it).
    fn filer_panel_paint(&self, rect: Rect, ground: [f32; 4]) -> MenuPaint {
        let mut paint = self.desktop_panel_paint(rect);
        let a = paint.fill[3];
        for (c, g) in paint.fill.iter_mut().zip(ground).take(3) {
            *c = *c * a + g * (1.0 - a);
        }
        paint.fill[3] = 1.0;
        paint
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
        let drawn = scale * win.zoom;
        if let Some(renderer) = win.renderer.as_mut() {
            renderer.set_scale(drawn);
            renderer.resize(pw, ph);
        } else {
            match crate::renderer::Renderer::new(&self.conn, win.window.wl_surface(), pw, ph, drawn) {
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

    /// Search window `id`'s folder for `find` (`Some("")`: the field out and
    /// empty; `None`: put away, everything shown again).
    fn filer_find(&mut self, id: u32, find: Option<String>) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.find = find;
        win.items = found(&win.all, win.find.as_deref());
        // What is shown changed under everything that held a place in it.
        let present: HashSet<&str> = win.items.iter().map(|it| it.path.as_str()).collect();
        win.selected.retain(|p| present.contains(p.as_str()));
        win.cursor = None;
        win.names.1.clear();
        win.slide.clear();
        win.menu = None;
        win.menu_path = None;
        win.props = None;
        win.rename = None;
        win.press = None;
        win.band = None;
        win.scroll = 0.0;
        win.scroll_to = 0.0;
        self.filer_hover(id);
        self.filer_request_draw(id);
    }

    /// Icons, or a list: the owner's standing choice for every window.
    pub(crate) fn filer_set_list(&mut self, list: bool) {
        if self.settings.files_list != list {
            self.settings.files_list = list;
            self.settings.save();
            info!("files: shown as {}", if list { "a list" } else { "icons" });
        }
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            if let Some(win) = self.filer.win_mut(id) {
                win.list = list;
                win.menu = None;
                win.menu_path = None;
                win.props = None;
                win.slide.clear();
            }
            self.filer_relayout(id);
            self.filer_hover(id);
        }
    }

    /// The owner's zoom of the file windows changed: every window is laid
    /// out and drawn anew at it.
    pub(crate) fn filer_zoomed(&mut self) {
        let zoom = self.files_zoom();
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            let scale = self.surface_scale(SurfaceKind::Files(id));
            if let Some(win) = self.filer.win_mut(id) {
                win.zoom = zoom;
                // What is up was placed for the other size.
                win.menu = None;
                win.menu_path = None;
                win.props = None;
                if let Some(renderer) = win.renderer.as_mut() {
                    renderer.set_scale(scale * zoom);
                }
            }
            self.filer_relayout(id);
            self.filer_hover(id);
        }
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
        let (scroll, mut moving) = crate::animation::ease_toward(win.scroll, win.scroll_to, dt, 18.0, 0.5);
        win.scroll = scroll;
        // The menu's and the box's entrance, at the desktop's rates.
        if let Some(menu) = win.menu.as_mut() {
            let (t, m) = crate::animation::ease_toward(menu.t, 1.0, dt, 20.0, 0.004);
            menu.t = t;
            moving |= m;
        }
        if let Some(props) = win.props.as_mut() {
            let (t, m) = crate::animation::ease_toward(props.t, 1.0, dt, 16.0, 0.004);
            props.t = t;
            moving |= m;
        }
        win.last_frame = moving.then_some(now);
        self.filer_sync_icons(&mut win, &layout);

        let (w, h) = win.virt();
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
        // Icons or a list: the button shows the one it turns to.
        hits.view = Rect::new(w - PAD - 28.0, mid - 14.0, 28.0, 28.0);
        if win.hover == Spot::View {
            scene.rects.push(rect(hits.view, 14.0, WASH_HOVER));
        }
        scene.labels.push(Label {
            font_px: 14.0,
            line_px: 18.0,
            family: Some(crate::options::NERD),
            ..label(
                // (The bar's icon font: a grid of squares, a list of lines.)
                if win.list { "\u{f00a}" } else { "\u{f03a}" },
                (hits.view.x + hits.view.w / 2.0, mid - 9.0),
                hits.view.w,
                true,
                false,
            )
        });
        // The magnifier, and — once it is clicked or Ctrl+F pressed — the
        // field what is typed goes in: the folder shows what it finds.
        hits.find = Rect::new(hits.view.x - 32.0, mid - 14.0, 28.0, 28.0);
        if win.hover == Spot::Find || win.find.is_some() {
            scene.rects.push(rect(hits.find, 14.0, WASH_HOVER));
        }
        scene.labels.push(Label {
            font_px: 14.0,
            line_px: 18.0,
            family: Some(crate::options::NERD),
            ..label("\u{f002}", (hits.find.x + hits.find.w / 2.0, mid - 9.0), hits.find.w, true, false)
        });
        if let Some(find) = win.find.as_ref() {
            let field = Rect::new(hits.find.x - 8.0 - FIND_W, mid - 13.0, FIND_W, 26.0);
            scene.rects.push(rect(field, 13.0, [0.0, 0.0, 0.0, 0.35]));
            scene.rects.push(RectInst {
                border: 1.0,
                ..rect(field, 13.0, [1.0, 1.0, 1.0, 0.16])
            });
            let (x, y) = (field.x + 12.0, mid - TEXT_LINE / 2.0);
            let tw = win
                .renderer
                .as_mut()
                .map_or(0.0, |r| r.measure_text(find, TEXT_PX, None))
                .min(FIND_W - 26.0);
            if find.is_empty() {
                scene.labels.push(label(tr("Search"), (x, y), FIND_W - 24.0, false, true));
            } else {
                scene.labels.push(Label {
                    cache: false,
                    clip: Some(field),
                    ..label(find, (x, y), FIND_W - 24.0, false, false)
                });
            }
            scene.rects.push(rect(Rect::new(x + tw + 1.0, y + 2.0, 1.0, TEXT_LINE - 4.0), 0.0, ink));
        }
        // The list's strip: what each column is, the one the things are
        // ordered by said with its arrow; a click orders by it.
        if layout.list {
            let row = Rect::new(layout.body.x + PAD, HEAD_H, layout.pitch, LIST_HEAD);
            let (size_end, date_x) = layout.columns(&row);
            let y = HEAD_H + (LIST_HEAD - TEXT_LINE) / 2.0;
            let name_of = |key: SortKey, words: &'static str| -> String {
                match win.sort {
                    (k, desc) if k == key && key != SortKey::Name => format!("{words} {}", if desc { "↓" } else { "↑" }),
                    _ => words.to_owned(),
                }
            };
            let name_end = size_end.map_or(row.x + row.w, |x| x - LIST_SIZE_W);
            let mut cols = vec![(SortKey::Name, row.x + LIST_NAME_X, name_end, name_of(SortKey::Name, tr("Name")))];
            if let Some(end) = size_end {
                cols.push((SortKey::Size, end - LIST_SIZE_W, end, name_of(SortKey::Size, tr("Size"))));
            }
            if let Some(x) = date_x {
                cols.push((SortKey::Modified, x, row.x + row.w, name_of(SortKey::Modified, tr("Modified"))));
            }
            for (n, (key, x0, x1, words)) in cols.into_iter().enumerate() {
                let hit = Rect::new(x0 - 6.0, HEAD_H + 2.0, x1 - x0 + 6.0, LIST_HEAD - 4.0);
                if win.hover == Spot::Col(n) {
                    scene.rects.push(rect(hit, 7.0, WASH_HOVER));
                }
                // (Size is read from its right end, as its numbers are.)
                let tw = if key == SortKey::Size {
                    win.renderer.as_mut().map_or(0.0, |r| r.measure_text(&words, TEXT_PX, None))
                } else {
                    0.0
                };
                let x = if key == SortKey::Size { x1 - tw } else { x0 };
                scene.labels.push(label(&words, (x, y), x1 - x0 + 4.0, false, win.sort.0 != key));
                hits.cols.push(hit);
            }
        }
        let steps = crumbs(&win.dir, &win.places);
        if let Some(renderer) = win.renderer.as_mut() {
            let widths: Vec<f32> = steps
                .iter()
                .map(|(name, _)| renderer.measure_text(name, TEXT_PX, None))
                .collect();
            const STEP_GAP: f32 = 22.0;
            // (The view's button, the magnifier and the search field stand
            // at the head's right end.)
            let taken = 68.0 + if win.find.is_some() { FIND_W + 8.0 } else { 0.0 };
            let (left, room) = (PAD + 76.0, (w - PAD - 76.0 - PAD - taken).max(40.0));
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
                if win.dir == place.path || win.drop_place == Some(n) {
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
        // Names are painted last of all: one under the menu or the box is
        // left out, or it would show through (the desktop's lesson).
        let cover = win.menu.as_ref().map(|m| m.rect).or(win.props.as_ref().map(|p| p.rect));
        let covered = |r: Rect| {
            cover.is_some_and(|c| r.x < c.x + c.w && c.x < r.x + r.w && r.y < c.y + c.h && c.y < r.y + r.h)
        };
        // MAKE ROOM: what is in hand leaves its place, and while a drag is
        // over the window the rest part around where it would land — each
        // easing to its display slot at the Apps grid's rate.
        let in_hand: HashSet<&str> = self
            .filer
            .drag
            .as_ref()
            .map(|d| d.paths.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let width = self.filer.drag.as_ref().map_or(1, |d| d.paths.len());
        if win.slide.len() != win.items.len() {
            win.slide = (0..win.items.len()).map(|i| i as f32).collect();
        }
        let mut gliding = false;
        let mut rank = 0usize;
        for (i, item) in win.items.iter().enumerate() {
            if in_hand.contains(item.path.as_str()) {
                continue;
            }
            let target = win.gap.map_or(rank, |g| parted(rank, g, width)) as f32;
            rank += 1;
            if win.slide[i] != target {
                let (v, m) = crate::animation::ease_toward(win.slide[i], target, dt, crate::frame::MAKEROOM_RATE, 0.005);
                win.slide[i] = v;
                gliding |= m;
            }
        }
        if gliding {
            moving = true;
            win.last_frame = Some(now);
        }
        // (While things are out of their places, a few more rows are drawn
        // than those on screen at rest.)
        let view = if win.gap.is_some() || gliding || !in_hand.is_empty() {
            let spare = width + 2 * layout.cols;
            view.start.saturating_sub(spare)..(view.end + spare).min(win.items.len())
        } else {
            view
        };
        for i in view {
            let item = &win.items[i];
            if in_hand.contains(item.path.as_str()) {
                continue;
            }
            let mut cell = layout.cell_at(win.slide[i]);
            cell.y -= win.scroll;
            if win.selected.contains(&item.path) || win.drop_on == Some(i) {
                grid.rects.push(rect(
                    Rect::new(cell.x + 3.0, cell.y + 3.0, cell.w - 6.0, cell.h - 6.0),
                    10.0,
                    WASH_SELECTED,
                ));
            }
            let icon = layout.icon_in(&cell, icon_scale);
            if let Some(layer) = win.layer_of(&item.icon) {
                grid.icons.push(IconInst {
                    rect: icon,
                    layer,
                    tint: [0.0; 4],
                    ring: -1.0,
                    plate: if item.kind == Kind::Launcher { PLATE_STATIC } else { NO_PLATE },
                });
            }
            if layout.list {
                // A row: the name, its size (a file's) and when it was last
                // changed, each in its column.
                let top = cell.y + (cell.h - TEXT_LINE) / 2.0;
                let (size_end, date_x) = layout.columns(&cell);
                let name_x = cell.x + LIST_NAME_X;
                let name_w = size_end.map_or(cell.x + cell.w, |x| x - LIST_SIZE_W) - name_x - 10.0;
                if covered(cell) {
                    continue;
                }
                let meta = win.meta.get(&item.path).copied().unwrap_or_default();
                if let Some(rename) = win.rename.as_ref().filter(|r| r.path == item.path) {
                    let tw = win
                        .renderer
                        .as_mut()
                        .map_or(0.0, |r| r.measure_text(&rename.text, TEXT_PX, None));
                    let inner = tw.max(24.0);
                    let field = Rect::new(name_x - 7.0, top - 2.0, inner + 14.0, TEXT_LINE + 4.0);
                    grid.rects.push(rect(field, 7.0, [0.0, 0.0, 0.0, 0.6]));
                    grid.rects.push(RectInst {
                        border: 1.0,
                        ..rect(field, 7.0, [1.0, 1.0, 1.0, 0.18])
                    });
                    if rename.all && tw > 0.0 {
                        grid.rects.push(rect(Rect::new(name_x, top + 1.0, tw, TEXT_LINE - 2.0), 2.0, [1.0, 1.0, 1.0, 0.22]));
                    }
                    grid.rects.push(rect(Rect::new(name_x + tw + 1.0, top + 2.0, 1.0, TEXT_LINE - 4.0), 0.0, ink));
                    grid.labels.push(Label {
                        cache: false,
                        ..label(&rename.text, (name_x, top), inner + 2.0, false, false)
                    });
                } else if let Some(renderer) = win.renderer.as_mut() {
                    let name = crate::desktop::fit_label(&item.name, name_w, &mut |t| {
                        renderer.measure_text(t, TEXT_PX, None)
                    });
                    grid.labels.push(label(&name, (name_x, top), name_w + 2.0, false, false));
                }
                if let Some(end) = size_end {
                    let words = if item.kind == Kind::File || item.kind == Kind::Launcher {
                        crate::desktop_props::size_text(meta.size)
                    } else {
                        "—".to_owned()
                    };
                    let tw = win.renderer.as_mut().map_or(0.0, |r| r.measure_text(&words, TEXT_PX, None));
                    grid.labels.push(label(&words, (end - tw, top), tw + 4.0, false, true));
                }
                if let (Some(x), Some(when)) = (date_x, crate::desktop_props::date_text(meta.modified)) {
                    grid.labels.push(label(&when, (x, top), LIST_DATE_W, false, true));
                }
                continue;
            }
            if win.names.1[i].is_none() {
                if let Some(renderer) = win.renderer.as_mut() {
                    win.names.1[i] = Some(two_lines(&item.name, label_w, &mut |t| {
                        renderer.measure_text(t, LABEL_FONT_PX, None)
                    }));
                }
            }
            if let Some(rename) = win.rename.as_ref().filter(|r| r.path == item.path) {
                // The field: a dark pill around what is typed, washed while
                // all of it is selected, a caret after it (the desktop's).
                let (cx, top) = (cell.x + cell.w / 2.0, icon.y + icon.h + LABEL_GAP);
                let tw = win
                    .renderer
                    .as_mut()
                    .map_or(0.0, |r| r.measure_text(&rename.text, LABEL_FONT_PX, None));
                let inner = tw.max(24.0);
                let field = Rect::new(cx - inner / 2.0 - 7.0, top - 2.0, inner + 14.0, LABEL_LINE_PX + 4.0);
                grid.rects.push(rect(field, 7.0, [0.0, 0.0, 0.0, 0.6]));
                grid.rects.push(RectInst {
                    border: 1.0,
                    ..rect(field, 7.0, [1.0, 1.0, 1.0, 0.18])
                });
                if rename.all && tw > 0.0 {
                    grid.rects.push(rect(
                        Rect::new(cx - tw / 2.0, top + 1.0, tw, LABEL_LINE_PX - 2.0),
                        2.0,
                        [1.0, 1.0, 1.0, 0.22],
                    ));
                }
                grid.rects.push(rect(
                    Rect::new(cx + tw / 2.0 + 1.0, top + 2.0, 1.0, LABEL_LINE_PX - 4.0),
                    0.0,
                    ink,
                ));
                grid.labels.push(Label {
                    font_px: LABEL_FONT_PX,
                    line_px: LABEL_LINE_PX,
                    cache: false,
                    ..label(&rename.text, (cx, top), inner + 2.0, true, false)
                });
                continue;
            }
            let Some((first, second)) = win.names.1[i].as_ref() else {
                continue;
            };
            if covered(Rect::new(
                cell.x,
                icon.y + icon.h + LABEL_GAP,
                cell.w,
                2.0 * LABEL_LINE_PX,
            )) {
                continue;
            }
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
            let words = if win.failed {
                tr("This folder can't be opened")
            } else if win.find.as_ref().is_some_and(|f| !f.is_empty()) {
                tr("Nothing found")
            } else if win.in_bin() == InBin::Top {
                tr("The bin is empty")
            } else {
                tr("Empty folder")
            };
            grid.labels.push(label(
                words,
                (layout.body.x + layout.body.w / 2.0, layout.body.y + layout.body.h / 2.0 - TEXT_LINE),
                layout.body.w - 2.0 * PAD,
                true,
                true,
            ));
        }
        if let Some((a, b, _)) = win.band.as_ref() {
            let band = Rect::new(
                a.0.min(b.0),
                a.1.min(b.1) - win.scroll,
                (a.0 - b.0).abs(),
                (a.1 - b.1).abs(),
            );
            grid.rects.push(rect(band, 3.0, BAND_FILL));
            grid.rects.push(RectInst {
                border: 1.0,
                ..rect(band, 3.0, BAND_LINE)
            });
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
        // The menu and the box, over everything.
        if let Some(menu) = win.menu.as_ref() {
            menu.push(&mut scene, &self.filer_panel_paint(menu.rect, ground));
        }
        if let Some(props) = win.props.as_ref() {
            props.push(&mut scene, &self.filer_panel_paint(props.rect, ground));
        }

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
            Spot::Place(_) | Spot::View | Spot::Find | Spot::Col(_) => true,
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
        if let (Some(menu), Some(at)) = (win.menu.as_mut(), win.ptr) {
            // Only its rows answer; coming onto its last one, Properties,
            // turns the menu into that box (the desktop's way).
            let over = menu.hit(at);
            if over != menu.hover {
                menu.hover = over;
                let props = over.and_then(|r| menu.action(r)) == Some(Action::Properties);
                if let (true, Some(i)) = (props, menu.item) {
                    let menu = win.menu.take();
                    self.filer_open_props(id, i, at, menu);
                } else {
                    self.filer_request_draw(id);
                }
            }
            return;
        }
        if let (Some(props), Some(at)) = (win.props.as_mut(), win.ptr) {
            // Its Back row, on hover: the box turns back into the menu.
            if props.back_rect().is_some_and(|r| r.contains(at)) {
                if let Some(mut menu) = win.props.take().and_then(|p| p.back.map(|m| (m, p.rect))).map(|(mut m, from)| {
                    m.grow_from = Some(from);
                    m.t = 0.0;
                    m
                }) {
                    menu.hover = menu.hit(at);
                    win.menu = Some(menu);
                }
                self.filer_request_draw(id);
            }
            return;
        }
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
                    win.ptr = Some((surface_x as f32 / win.zoom, surface_y as f32 / win.zoom));
                }
                self.filer_hover(id);
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.ptr = None;
                    // (A drag of ours takes the pointer away like this.)
                    win.press = None;
                    win.band = None;
                }
                self.filer_hover(id);
                self.filer_request_draw(id);
            }
            wl_pointer::Event::Motion {
                surface_x, surface_y, ..
            } => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.ptr = Some((surface_x as f32 / win.zoom, surface_y as f32 / win.zoom));
                }
                if !self.filer_travel(id) {
                    self.filer_hover(id);
                }
            }
            wl_pointer::Event::Button {
                button, state, serial, ..
            } => {
                let pressed = state == WEnum::Value(wl_pointer::ButtonState::Pressed);
                self.filer_button(id, button, pressed, serial);
            }
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
                // (Fingers move the folder with them whatever the zoom.)
                let step = match self.filer.win(id) {
                    Some(w) if w.wheel => value as f32 * WHEEL_STEP,
                    Some(w) => value as f32 / w.zoom,
                    None => return,
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

    /// A button went down or up.
    fn filer_button(&mut self, id: u32, button: u32, pressed: bool, serial: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let at = win.ptr.unwrap_or_default();
        // A menu that is up takes the click: a left press on a row acts
        // when it comes back up on the same row; any other press closes it.
        if let Some(menu) = win.menu.as_mut() {
            let over = menu.hit(at);
            if button == 0x110 && !pressed {
                let chosen = menu.pressed.take().filter(|&r| Some(r) == over);
                if let Some(action) = chosen.and_then(|r| menu.action(r)) {
                    // What cannot be undone is asked once: the row says so
                    // and the menu stays for the second click.
                    if matches!(action, Action::Erase | Action::EmptyBin) && win.armed != Some(action) {
                        win.armed = Some(action);
                        if let Some(Row::Item { label, .. }) = chosen.and_then(|r| menu.rows.get_mut(r)) {
                            *label = tr("Click again to delete");
                        }
                        self.filer_request_draw(id);
                        return;
                    }
                    let item = menu.item;
                    win.menu = None;
                    win.menu_path = None;
                    self.filer_request_draw(id);
                    self.filer_act(id, action, item, at);
                }
                return;
            }
            if button == 0x110 && over.is_some() {
                menu.pressed = over;
                return;
            }
            if pressed {
                win.menu = None;
                win.menu_path = None;
                self.filer_request_draw(id);
                // A right press elsewhere opens the menu there instead.
                if button != 0x111 {
                    self.filer_hover(id);
                    return;
                }
            } else {
                return;
            }
        }
        if !pressed {
            // The left button came up with nothing carried off: a band
            // ends where it is; a click on one of a group leaves that one.
            if button == 0x110 {
                if let Some(win) = self.filer.win_mut(id) {
                    let press = win.press.take();
                    let banded = win.band.take().is_some();
                    if let Some(i) = press
                        .filter(|p| p.of_group && !banded)
                        .and_then(|p| p.path)
                        .and_then(|p| win.items.iter().position(|it| it.path == p))
                    {
                        win.select_only(i);
                    }
                    self.filer_request_draw(id);
                }
            }
            return;
        }
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        // A press anywhere puts the Properties box away and keeps a name
        // being typed.
        if win.props.take().is_some() {
            self.filer_hover(id);
            self.filer_request_draw(id);
            return;
        }
        if win.rename.is_some() {
            self.filer_end_rename(id, true);
        }
        match button {
            0x110 => self.filer_click(id, serial),
            0x111 => {
                let icon_scale = self.icon_scale();
                let Some(win) = self.filer.win(id) else {
                    return;
                };
                let layout = win.layout(icon_scale);
                // The menu belongs to the icons' field: the head and the
                // places have none.
                if layout.body.contains(at) {
                    let item = layout.index_at(at, win.scroll, win.items.len());
                    self.filer_open_menu(id, at, item);
                }
            }
            // The mouse's side buttons.
            0x113 => self.filer_back(id),
            0x114 => self.filer_forward(id),
            _ => {}
        }
    }

    /// The pointer moved with the left button down: past `DRAG_START` a
    /// press on a thing takes it (and its group) in hand, one on bare
    /// ground draws a rubber band. Whether the motion was that.
    fn filer_travel(&mut self, id: u32) -> bool {
        let icon_scale = self.icon_scale();
        let ctrl = self.modifiers.ctrl;
        let Some(win) = self.filer.win_mut(id) else {
            return false;
        };
        let Some(at) = win.ptr else {
            return false;
        };
        let layout = win.layout(icon_scale);
        if let Some((_, to, kept)) = win.band.as_mut() {
            *to = (at.0, at.1 + win.scroll);
            let kept = kept.clone();
            let (a, b) = win.band.as_ref().map_or(((0.0, 0.0), (0.0, 0.0)), |(a, b, _)| (*a, *b));
            let mut selected = kept;
            selected.extend(
                band_hits(&layout, a, b, win.items.len())
                    .into_iter()
                    .map(|i| win.items[i].path.clone()),
            );
            win.selected = selected;
            self.filer_request_draw(id);
            return true;
        }
        let Some(press) = win.press.clone() else {
            return false;
        };
        if (at.0 - press.at.0).hypot(at.1 - press.at.1) < DRAG_START {
            return true;
        }
        match press.path.and_then(|p| win.items.iter().position(|it| it.path == p)) {
            Some(i) => {
                win.press = None;
                self.filer_lift(id, i, press.at, press.serial);
            }
            None => {
                let from = (press.at.0, press.at.1 + win.scroll);
                let kept = if ctrl { win.selected.clone() } else { HashSet::new() };
                win.band = Some((from, (at.0, at.1 + win.scroll), kept));
                win.press = None;
                self.filer_request_draw(id);
            }
        }
        true
    }

    /// Take item `i` and what is selected with it in hand: a Wayland drag
    /// of ours offering the files, the grabbed one's picture as its image.
    fn filer_lift(&mut self, id: u32, i: usize, at: (f32, f32), serial: u32) {
        let icon_scale = self.icon_scale();
        let (Some(manager), Some(device), Some(win)) = (
            self.data_device_manager.as_ref(),
            self.data_device.as_ref(),
            self.filer.win(id),
        ) else {
            warn!("files: no data device; nothing can be dragged");
            return;
        };
        let Some(item) = win.items.get(i) else {
            return;
        };
        // The grabbed one first, then the rest of its selection.
        let mut paths = vec![item.path.clone()];
        let mut volume = item.kind == Kind::Volume;
        for it in win.items.iter().filter(|it| it.path != item.path && win.selected.contains(&it.path)) {
            paths.push(it.path.clone());
            volume |= it.kind == Kind::Volume;
        }
        // (A plugged-in volume is only ever copied from.)
        let source = manager.create_drag_and_drop_source(
            &self.qh,
            [URI_LIST, PLAIN_TEXT],
            if volume { DndAction::Copy } else { DndAction::Move | DndAction::Copy },
        );
        // Its picture, held where it was grabbed (in real pixels: the
        // window's zoom is in how big it is drawn).
        let layout = win.layout(icon_scale);
        let mut cell = layout.cell(i);
        cell.y -= win.scroll;
        let icon = layout.icon_in(&cell, icon_scale);
        // (A row's small picture travels full size, held by its middle.)
        let grip = if layout.list {
            let half = GRID_ICON * icon_scale * win.zoom / 2.0;
            (half, half)
        } else {
            ((at.0 - icon.x) * win.zoom, (at.1 - icon.y) * win.zoom)
        };
        let image = win
            .chains
            .get(&item.icon)
            .and_then(|chain| self.drag_image(chain, grip, icon_scale * win.zoom));
        source.start_drag(device, win.window.wl_surface(), image.as_ref().map(|d| d.surface()), serial);
        // (Hyprland maps the image on the first commit that has a texture.)
        if let Some(image) = image.as_ref() {
            image.surface().commit();
        }
        info!("files: {} in hand ({} in all)", item.name, paths.len());
        // Its place stays open until the pointer says otherwise.
        let hole = (win.in_bin() == InBin::No)
            .then(|| win.items[..i].iter().filter(|it| !paths.contains(&it.path)).count());
        self.filer.drag = Some(Lifted {
            win: id,
            paths,
            source,
            _icon: image,
            action: DndAction::empty(),
        });
        if let Some(win) = self.filer.win_mut(id) {
            win.gap = hole;
        }
        self.filer_request_draw(id);
    }

    /// Another app (or one of our own surfaces) asked for the files in
    /// hand. Written off the loop.
    pub(crate) fn filer_send_drag(&mut self, mime: &str, pipe: smithay_client_toolkit::data_device_manager::WritePipe) {
        let Some(drag) = self.filer.drag.as_ref() else {
            return;
        };
        let paths: Vec<&str> = drag.paths.iter().map(String::as_str).collect();
        let Some(payload) = crate::desktop::drag_payload(&paths, mime) else {
            warn!("files: {mime} asked of a drag that never offered it");
            return;
        };
        let fd: std::os::fd::OwnedFd = pipe.into();
        std::thread::spawn(move || {
            use std::io::Write;
            let mut file = std::fs::File::from(fd);
            if let Err(e) = file.write_all(payload.as_bytes()) {
                warn!("files: writing the dragged files failed: {e}");
            }
        });
    }

    /// The drop target chose what to do with the files in hand.
    pub(crate) fn filer_drag_action(&mut self, action: DndAction) {
        if let Some(drag) = self.filer.drag.as_mut() {
            drag.action = action;
        }
    }

    /// Our drag is over: dropped and done, or cancelled. The window it came
    /// from reads its folder again (a move took the files).
    pub(crate) fn filer_drag_end(&mut self, done: bool) {
        let Some(drag) = self.filer.drag.take() else {
            return;
        };
        info!(
            "files: {} file(s) {} (action {:?})",
            drag.paths.len(),
            if done { "dropped" } else { "let go nowhere" },
            drag.action
        );
        let win = drag.win;
        drop(drag); // the source and the image go with it
        self.schedule_frame();
        // Every window closes the room it had made.
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            if let Some(w) = self.filer.win_mut(id) {
                w.gap = None;
                w.edge = 0.0;
            }
            self.filer_request_draw(id);
        }
        if done {
            self.filer_list(win);
        }
    }

    /// A drag came over window `id`: taken if it carries files, as a move
    /// (or a copy, where a move is not possible).
    pub(crate) fn filer_dnd_enter(&mut self, id: u32, offer: DragOffer) {
        let has_files = offer.with_mime_types(|m| m.iter().any(|t| t == URI_LIST));
        if !has_files {
            offer.accept_mime_type(offer.serial, None);
            return;
        }
        offer.accept_mime_type(offer.serial, Some(URI_LIST.to_owned()));
        offer.set_actions(DndAction::Move | DndAction::Copy, DndAction::Move);
        // (The enter's position is the surface's centre on Hyprland, not
        // the pointer's: where it is comes with the first motion.)
        self.filer.dnd = Some(Hovering { win: id, offer });
    }

    /// The hovering drag moved: over a folder, that folder is where it
    /// would go (and is washed to say so).
    pub(crate) fn filer_dnd_motion(&mut self, x: f32, y: f32) {
        let Some(id) = self.filer.dnd.as_ref().map(|d| d.win) else {
            return;
        };
        self.filer_drag_over(id, x, y);
    }

    /// A drag is at (`x`, `y`) on window `id`: what it would be let go
    /// onto, and the room made for it.
    fn filer_drag_over(&mut self, id: u32, x: f32, y: f32) {
        let icon_scale = self.icon_scale();
        let carried = self.filer.drag_paths();
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let at = (x / win.zoom, y / win.zoom);
        let layout = win.layout(icon_scale);
        // Over the places: onto that place (the bin among them).
        let place = win.hits.places.iter().position(|r| r.contains(at));
        // In the field: the things not in hand, and which of them — if any
        // — stands where the pointer is, the room made counted in.
        let rest: Vec<usize> = (0..win.items.len())
            .filter(|&i| !carried.contains(&win.items[i].path))
            .collect();
        let (n, m) = (carried.len().max(1), rest.len());
        // (Room is made only where a place can be kept: in the folder's own
        // order, not while it is shown by size or by date.)
        let in_field = place.is_none()
            && layout.body.contains(at)
            && win.in_bin() == InBin::No
            && win.sort.0 == SortKey::Name
            && win.find.as_ref().is_none_or(|f| f.is_empty());
        let (d, fx) = layout.slot_at(at, win.scroll);
        let gap_now = win.gap.unwrap_or(m);
        let under = if d < gap_now {
            rest.get(d).copied()
        } else if d >= gap_now + n {
            rest.get(d - n).copied()
        } else {
            None
        };
        // On a folder's middle: into it, and nothing moves (the grid's
        // "a neighbour holds still so it can be dropped onto"). Anywhere
        // else in the field: the gap goes where the pointer is.
        let onto = under.filter(|&i| {
            layout.body.contains(at)
                && place.is_none()
                && matches!(win.items[i].kind, Kind::Folder | Kind::Volume)
                && (0.22..0.78).contains(&fx)
        });
        let gap = if !in_field {
            None
        } else if onto.is_some() {
            win.gap
        } else {
            Some(gap_at(d, fx, gap_now, n, m))
        };
        // Near the top or the bottom of the field: the folder scrolls.
        win.edge_at = (x, y);
        win.edge = if place.is_some() || !layout.body.contains(at) {
            0.0
        } else if at.1 < layout.body.y + EDGE_ZONE {
            -((layout.body.y + EDGE_ZONE - at.1) / EDGE_ZONE).min(1.0)
        } else if at.1 > layout.body.y + layout.body.h - EDGE_ZONE {
            ((at.1 - (layout.body.y + layout.body.h - EDGE_ZONE)) / EDGE_ZONE).min(1.0)
        } else {
            0.0
        };
        let scrolls = win.edge != 0.0 && layout.max_scroll(win.items.len()) > 0.0 && !win.edge_timer;
        if onto != win.drop_on || place != win.drop_place || gap != win.gap {
            win.drop_on = onto;
            win.drop_place = place;
            win.gap = gap;
            self.filer_request_draw(id);
        }
        if scrolls {
            self.filer_edge_tick(id);
        }
    }

    /// Scroll window `id` a step for the drag held at its edge, and again
    /// in a moment for as long as it is held there.
    fn filer_edge_tick(&mut self, id: u32) {
        let icon_scale = self.icon_scale();
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.edge_timer = false;
        if win.edge == 0.0 {
            return;
        }
        let max = win.layout(icon_scale).max_scroll(win.items.len());
        let to = (win.scroll_to + win.edge * EDGE_SPEED).clamp(0.0, max);
        if to == win.scroll_to {
            return; // the end of the folder
        }
        win.scroll_to = to;
        win.scroll = to;
        let at = win.edge_at;
        let timer = calloop::timer::Timer::from_duration(EDGE_TICK);
        win.edge_timer = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                // (What is under the drag changed as the folder moved.)
                if app.filer.win(id).is_some_and(|w| w.edge != 0.0) {
                    app.filer_drag_over(id, at.0, at.1);
                    app.filer_edge_tick(id);
                } else if let Some(win) = app.filer.win_mut(id) {
                    win.edge_timer = false;
                }
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
        self.filer_request_draw(id);
    }

    /// The drag left for elsewhere — unless it has just been dropped here:
    /// Hyprland sends `leave` right after `drop` (the desktop's lesson).
    pub(crate) fn filer_dnd_leave(&mut self) {
        let dropped = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
            .is_some_and(|o| o.dropped);
        if dropped {
            return;
        }
        if let Some(dnd) = self.filer.dnd.take() {
            if let Some(win) = self.filer.win_mut(dnd.win) {
                win.drop_on = None;
                win.drop_place = None;
                win.gap = None;
        win.edge = 0.0;
            }
            self.filer_request_draw(dnd.win);
        }
    }

    /// Let go on a window: into the folder under the pointer, else into
    /// the folder shown. The list of files is asked of the source and read
    /// off the loop — whoever the source is, a window of ours included.
    pub(crate) fn filer_dnd_drop(&mut self) {
        let Some(dnd) = self.filer.dnd.take() else {
            return;
        };
        let id = dnd.win;
        let carried = self.filer.drag_paths();
        let Some(win) = self.filer.win_mut(id) else {
            dnd.offer.destroy();
            return;
        };
        let dir = match win.drop_place.take().and_then(|n| win.places.get(n)) {
            Some(place) => place.path.clone(),
            None => win
                .drop_on
                .and_then(|i| win.items.get(i))
                .map_or_else(|| win.dir.clone(), |it| PathBuf::from(&it.path)),
        };
        // Let go in the room made for it: that is where it stays — the
        // folder is arranged by hand from now on (its names, in order, with
        // the place what comes goes to).
        let arrange = win.arrangement(&dir, &carried);
        win.edge = 0.0;
        win.drop_on = None;
        self.filer_request_draw(id);
        // The live offer (the one kept since `enter` is a snapshot).
        let offer = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
            .filter(|o| o.dropped)
            .unwrap_or(dnd.offer);
        info!("files: dropped on window {id}, into {}", dir.display());
        let pipe = match offer.receive(URI_LIST.to_owned()) {
            Ok(pipe) => pipe,
            Err(e) => {
                warn!("files: cannot receive the drop: {e}");
                offer.destroy();
                return;
            }
        };
        // (Through `OwnedFd`, never `into_raw_fd`: SCTK closes the pipe.)
        let fd: std::os::fd::OwnedFd = pipe.into();
        let (tx, rx) = calloop::channel::channel::<String>();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut file = std::fs::File::from(fd);
            let mut text = String::new();
            if let Err(e) = file.read_to_string(&mut text) {
                warn!("files: reading the drop's files failed: {e}");
            }
            let _ = tx.send(text);
        });
        let dropped = offer.clone();
        if self
            .loop_handle
            .insert_source(rx, move |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(text) = event {
                    // A move, unless the source only lets its files be copied.
                    let may_move =
                        dropped.source_actions.is_empty() || dropped.source_actions.contains(DndAction::Move);
                    let offer = dropped.clone();
                    let paths = crate::desktop::uri_list_paths(&text);
                    app.filer_bring(id, paths, dir.clone(), may_move, arrange.clone(), move || {
                        // Done, whatever came of the files: the source must
                        // always hear the end of its drag.
                        offer.finish();
                    });
                }
            })
            .is_err()
        {
            warn!("files: cannot wait for the drop's files");
            offer.destroy();
        }
    }

    /// Bring `paths` into `dir` off the loop — a move where one disk
    /// allows it, else a copy (`desktop::import`) — then `then`, and window
    /// `id` shows what came if that is the folder it is on.
    fn filer_bring(
        &mut self,
        id: u32,
        paths: Vec<PathBuf>,
        dir: PathBuf,
        may_move: bool,
        arrange: Option<(Vec<String>, Vec<String>)>,
        then: impl FnOnce() + 'static,
    ) {
        // Into the bin (its window, or its place): thrown away, not moved
        // into its folder — the bin keeps a record of where each came from.
        if dir.starts_with(bin_root()) {
            for path in paths.iter().filter(|p| !p.starts_with(bin_root())) {
                info!("files: {} → Recycle Bin", path.display());
                self.trash_file(&path.to_string_lossy());
            }
            then();
            self.filer_list(id);
            return;
        }
        let into = dir.clone();
        let mut then = Some(then);
        // (Across disks a drop is a copy: what was dragged stays where it was.)
        let task = self.filer_copy_task(&paths, &dir, may_move, false);
        self.desktop_off_loop(
            move || {
                use crate::desktop_send::{weigh, Meter};
                let total = task.as_ref().map_or(0, |_| weigh(&paths));
                let tell = |done, total| {
                    if let Some(task) = task.as_ref() {
                        task.set(done, total);
                    }
                };
                let stopped = || task.as_ref().is_some_and(|t| t.cancelled());
                let mut meter = Meter::new(total, &tell, &stopped);
                let brought =
                    crate::desktop::import_metered(&paths, &into, may_move, task.is_some().then_some(&mut meter));
                let failed = if stopped() { 0 } else { paths.len() - brought.len().min(paths.len()) };
                (brought, failed)
            },
            move |app, (brought, failed)| {
                app.filer_say_failed(failed);
                info!("files: {} thing(s) brought into {}", brought.len(), dir.display());
                if let Some(then) = then.take() {
                    then();
                }
                // What came stands where it was let go.
                if let Some((before, after)) = arrange.clone() {
                    let came: Vec<String> = brought.iter().map(|p| file_name(p)).collect();
                    let mut order: Vec<String> = before.into_iter().filter(|n| !came.contains(n)).collect();
                    order.extend(came.iter().cloned());
                    order.extend(after.into_iter().filter(|n| !came.contains(n)));
                    app.filer.orders.set(&dir, Some(order));
                }
                if let Some(win) = app.filer.win_mut(id).filter(|w| w.dir == dir) {
                    win.selected = brought.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                    app.filer_list(id);
                }
            },
        );
    }

    /// A left press.
    fn filer_click(&mut self, id: u32, serial: u32) {
        let ctrl = self.modifiers.ctrl;
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let at = win.ptr.unwrap_or_default();
        let layout_top = if win.list { HEAD_H + LIST_HEAD } else { HEAD_H };
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
                // One of several selected: the group stays for a drag, and
                // the release decides (see `filer_button`).
                let of_group = !ctrl && win.selected.len() > 1 && win.selected.contains(&path);
                if ctrl {
                    // One more, or one fewer.
                    if !win.selected.remove(&path) {
                        win.selected.insert(path.clone());
                    }
                    win.cursor = Some(i);
                } else if !of_group {
                    win.select_only(i);
                }
                win.press = Some(Press {
                    path: Some(path),
                    at,
                    serial,
                    of_group,
                });
                self.filer_request_draw(id);
            }
            Spot::View => {
                let list = !win.list;
                self.filer_set_list(list);
            }
            Spot::Find => {
                let open = win.find.is_none();
                self.filer_find(id, open.then(String::new));
            }
            Spot::Col(n) => {
                let key = [SortKey::Name, SortKey::Size, SortKey::Modified][n.min(2)];
                // The same column again turns it round; a new one starts
                // with the biggest, the newest.
                win.sort = match win.sort {
                    (k, desc) if k == key && key != SortKey::Name => (key, !desc),
                    _ => (key, key != SortKey::Name),
                };
                self.filer_list(id);
            }
            Spot::Nothing => {
                win.click = None;
                // Bare ground in the field: a band may start here.
                if at.1 > layout_top && at.0 > win.hits.places.first().map_or(0.0, |r| r.x + r.w) {
                    win.press = Some(Press {
                        path: None,
                        at,
                        serial,
                        of_group: false,
                    });
                }
                if !ctrl && !win.selected.is_empty() {
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
        // A name being typed takes every key.
        if win.rename.is_some() {
            return self.filer_rename_key(id, keysym, utf8);
        }
        // Escape puts away what is up before anything else.
        if win.menu.is_some() || win.props.is_some() {
            if keysym == Keysym::Escape {
                win.menu = None;
                win.menu_path = None;
                win.props = None;
                self.filer_hover(id);
                self.filer_request_draw(id);
            }
            return;
        }
        // A search that is open takes what is typed; the keys that walk
        // and open go on to the things found.
        if let Some(find) = win.find.clone() {
            let typed = utf8.filter(|t| !ctrl && !alt && !t.is_empty() && !t.chars().any(char::is_control));
            match keysym {
                Keysym::Escape => return self.filer_find(id, None),
                Keysym::BackSpace => {
                    let mut find = find;
                    // (On an empty field it puts the field away.)
                    let left = find.pop().map(|_| find);
                    return self.filer_find(id, left);
                }
                Keysym::Return | Keysym::KP_Enter if win.cursor.is_none() && !win.items.is_empty() => {
                    // Enter with nothing walked to: the first thing found.
                    self.filer_walk_to(id, 0);
                    return self.filer_activate(id, 0);
                }
                _ => {
                    if let Some(text) = typed {
                        return self.filer_find(id, Some(format!("{find}{text}")));
                    }
                }
            }
        }
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
            Keysym::f | Keysym::F if ctrl => {
                let open = win.find.is_none();
                self.filer_find(id, open.then(String::new));
            }
            Keysym::_1 if ctrl => self.filer_set_list(false),
            Keysym::_2 if ctrl => self.filer_set_list(true),
            Keysym::plus | Keysym::equal | Keysym::KP_Add if ctrl => {
                self.zoom("files", "in");
            }
            Keysym::minus | Keysym::KP_Subtract if ctrl => {
                self.zoom("files", "out");
            }
            Keysym::_0 | Keysym::KP_0 if ctrl => {
                self.zoom("files", "reset");
            }
            Keysym::F2 => self.filer_act(id, Action::Rename, None, (0.0, 0.0)),
            Keysym::Delete => self.filer_act(id, Action::MoveToBin, None, (0.0, 0.0)),
            Keysym::c | Keysym::C if ctrl => self.filer_act(id, Action::Copy, None, (0.0, 0.0)),
            Keysym::x | Keysym::X if ctrl => self.filer_act(id, Action::Cut, None, (0.0, 0.0)),
            Keysym::v | Keysym::V if ctrl => self.filer_act(id, Action::Paste, None, (0.0, 0.0)),
            // (With Shift: Ctrl+N alone is another window.)
            Keysym::N if ctrl => self.filer_act(id, Action::NewFolder, None, (0.0, 0.0)),
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
            "add" => {
                if let (Ok(i), Some(win)) = (rest.parse::<usize>(), self.filer.win_mut(id)) {
                    if let Some(path) = win.items.get(i).map(|it| it.path.clone()) {
                        win.selected.insert(path);
                    }
                    self.filer_request_draw(id);
                }
            }
            "menu" => {
                let icon_scale = self.icon_scale();
                let Some(win) = self.filer.win(id) else {
                    return "closed".to_owned();
                };
                let layout = win.layout(icon_scale);
                let item = rest.parse::<usize>().ok().filter(|&i| i < win.items.len());
                // Where a click would have been: the thing's middle, or the
                // middle of the field.
                let at = match item {
                    Some(i) => {
                        let c = layout.cell(i);
                        (c.x + c.w / 2.0, c.y + c.h / 2.0 - win.scroll)
                    }
                    None => (layout.body.x + layout.body.w / 2.0, layout.body.y + layout.body.h / 2.0),
                };
                self.filer_open_menu(id, at, item);
            }
            "pick" => {
                let Some((action, item, at)) = self.filer.win_mut(id).and_then(|w| {
                    let menu = w.menu.take()?;
                    w.menu_path = None;
                    Some((menu.action(rest.parse().ok()?)?, menu.item, menu.at))
                }) else {
                    return "no such row".to_owned();
                };
                if action == Action::Properties {
                    if let Some(i) = item {
                        self.filer_open_props(id, i, at, None);
                    }
                } else {
                    self.filer_act(id, action, item, at);
                }
            }
            "copy" => self.filer_act(id, Action::Copy, None, (0.0, 0.0)),
            "cut" => self.filer_act(id, Action::Cut, None, (0.0, 0.0)),
            "paste" => self.filer_act(id, Action::Paste, None, (0.0, 0.0)),
            "bin" => self.filer_act(id, Action::MoveToBin, None, (0.0, 0.0)),
            "newfolder" => self.filer_act(id, Action::NewFolder, None, (0.0, 0.0)),
            "rename" => {
                if let Ok(i) = rest.parse::<usize>() {
                    self.filer_begin_rename(id, i);
                }
            }
            "props" => {
                if let Ok(i) = rest.parse::<usize>() {
                    let at = self.filer.win(id).map_or((0.0, 0.0), |w| (w.virt().0 / 2.0, w.virt().1 / 3.0));
                    self.filer_open_props(id, i, at, None);
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
                    "f2" => Keysym::F2,
                    _ => return format!("no key {rest}"),
                };
                self.filer_key(id, key, None);
            }
            "type" => self.filer_key(id, Keysym::NoSymbol, Some(rest)),
            "zoom" => return self.zoom("files", rest),
            "view" => self.filer_set_list(rest == "list"),
            "find" => self.filer_find(id, (rest != "-").then(|| rest.to_owned())),
            "sort" => {
                // `sort name|size|date [up]`
                let mut words = rest.split_whitespace();
                let key = match words.next() {
                    Some("size") => SortKey::Size,
                    Some("date") => SortKey::Modified,
                    _ => SortKey::Name,
                };
                if let Some(win) = self.filer.win_mut(id) {
                    win.sort = (key, key != SortKey::Name && words.next() != Some("up"));
                }
                self.filer_list(id);
            }
            "band" => {
                // `band <x0> <y0> <x1> <y1>`: as a press and a travel.
                let n: Vec<f32> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                let [x0, y0, x1, y1] = n[..] else {
                    return "band <x0> <y0> <x1> <y1>".to_owned();
                };
                if let Some(win) = self.filer.win_mut(id) {
                    win.press = Some(Press {
                        path: None,
                        at: (x0, y0),
                        serial: 0,
                        of_group: false,
                    });
                    win.ptr = Some((x1, y1));
                }
                self.filer_travel(id);
                self.filer_travel(id);
            }
            "unband" => self.filer_button(id, 0x110, false, 0),
            "over" => {
                // `over <x> <y>`: as a drag hovering there; `over` alone: gone.
                let n: Vec<f32> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                match n[..] {
                    [x, y] => {
                        let zoom = self.filer.win(id).map_or(1.0, |w| w.zoom);
                        self.filer_drag_over(id, x * zoom, y * zoom);
                    }
                    _ => {
                        if let Some(win) = self.filer.win_mut(id) {
                            win.gap = None;
        win.edge = 0.0;
                            win.drop_on = None;
                            win.drop_place = None;
                        }
                        self.filer_request_draw(id);
                    }
                }
            }
            "drop" => {
                // `drop <n|-> <path…>`: as files let go on folder n, or on
                // the folder shown.
                let mut words = rest.split_whitespace();
                let onto = words.next().and_then(|w| w.parse::<usize>().ok());
                let paths: Vec<PathBuf> = words.map(&as_path).collect();
                let Some(win) = self.filer.win_mut(id) else {
                    return "closed".to_owned();
                };
                let dir = onto
                    .and_then(|i| win.items.get(i))
                    .map_or_else(|| win.dir.clone(), |it| PathBuf::from(&it.path));
                // (After `over`: let go in the room made there.)
                let arrange = win.arrangement(&dir, &[]);
                self.filer_bring(id, paths, dir, true, arrange, || {});
            }
            "state" => {}
            _ => return format!("unknown: {what}"),
        }
        let icon_scale = self.icon_scale();
        let Some(win) = self.filer.win(id) else {
            return "closed".to_owned();
        };
        let layout = win.layout(icon_scale);
        format!(
            "window {id} {}x{} on {} — {} items{}, {} cols, scroll {:.0}/{:.0}, selected {}, back {}, forward {}, places {}, layers {}/{}, menu {}, props {}, rename {:?}",
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
            win.menu.as_ref().map_or(0, |m| m.rows.len()),
            win.props.as_ref().map_or("-", |p| p.title.as_str()),
            win.rename.as_ref().map(|r| r.text.as_str()),
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
    fn the_menu_offers_what_makes_sense_for_what_was_clicked() {
        let asked = |on_item, many| Asked {
            on_item,
            many,
            paste: false,
            bin: InBin::No,
            by_hand: false,
            archive: false,
        };
        let actions = |rows: Vec<Row>| -> Vec<Action> {
            rows.into_iter()
                .filter_map(|r| match r {
                    Row::Item { action, .. } => Some(action),
                    _ => None,
                })
                .collect()
        };
        let one = actions(menu_rows(Asked { on_item: true, many: false, paste: true, bin: InBin::No, by_hand: false, archive: false }));
        assert_eq!(one.first(), Some(&Action::Open));
        // Properties is last: it is the row the menu turns into a box from.
        assert_eq!(one.last(), Some(&Action::Properties));
        assert!(one.contains(&Action::Rename) && !one.contains(&Action::Paste));
        // A group: nothing that is about ONE thing.
        let many = actions(menu_rows(Asked { on_item: true, many: true, paste: false, bin: InBin::No, by_hand: false, archive: false }));
        assert!(!many.contains(&Action::Rename) && !many.contains(&Action::Properties));
        assert!(many.contains(&Action::Cut) && many.contains(&Action::MoveToBin));
        // The folder itself: Paste only with files on the clipboard.
        assert_eq!(actions(menu_rows(Asked { on_item: false, many: false, paste: true, bin: InBin::No, by_hand: false, archive: false })).first(), Some(&Action::Paste));
        assert_eq!(actions(menu_rows(Asked { on_item: false, many: false, paste: false, bin: InBin::No, by_hand: false, archive: false })).first(), Some(&Action::NewFolder));
        // An archive can be opened out; anything can be packed.
        let zip = actions(menu_rows(Asked { archive: true, ..asked(true, false) }));
        assert!(zip.contains(&Action::Extract) && zip.contains(&Action::Compress));
        assert!(!one.contains(&Action::Extract) && many.contains(&Action::Compress));
        assert!(is_archive("Fotos.ZIP") && is_archive("a.tar.gz") && !is_archive("zip.txt"));
        // In the bin: back or gone, and the bin emptied; never a new thing.
        let binned = actions(menu_rows(Asked { on_item: true, many: false, paste: true, bin: InBin::Top, by_hand: false, archive: false }));
        assert!(binned.contains(&Action::PutBack) && binned.contains(&Action::Erase));
        assert!(!binned.contains(&Action::Rename) && !binned.contains(&Action::MoveToBin));
        assert_eq!(actions(menu_rows(Asked { on_item: false, many: false, paste: true, bin: InBin::Top, by_hand: false, archive: false })), [Action::EmptyBin]);
        assert!(menu_rows(Asked { on_item: false, many: false, paste: true, bin: InBin::Deeper, by_hand: false, archive: false }).is_empty());
    }

    #[test]
    fn a_rubber_band_selects_what_it_crosses_either_way_round() {
        let l = Layout::new((920, 600), 1.0);
        let (a, b) = (l.cell(1), l.cell(l.cols + 2));
        let from = (a.x + a.w / 2.0, a.y + a.h / 2.0);
        let to = (b.x + b.w / 2.0, b.y + b.h / 2.0);
        let want = vec![1, 2, l.cols + 1, l.cols + 2];
        assert_eq!(band_hits(&l, from, to, 40), want);
        assert_eq!(band_hits(&l, to, from, 40), want);
        // Only as far as there are things; a band in a cell's margin touches nothing.
        assert_eq!(band_hits(&l, from, to, 3), vec![1, 2]);
        assert!(band_hits(&l, (a.x, a.y), (a.x + 6.0, a.y + 4.0), 40).is_empty());
    }

    #[test]
    fn making_room_is_the_apps_grids_reflow_and_takes_a_group() {
        // One icon on one page: the same answer as the grid's own function,
        // for a thing of this window (it has a place to close) and for one
        // coming in (it has none).
        for origin in [Some(4usize), None] {
            for gap in 0..10 {
                let mut rank = 0;
                for s in 0..10 {
                    if Some(s) == origin {
                        continue;
                    }
                    assert_eq!(
                        parted(rank, gap.min(if origin.is_some() { 9 } else { 10 }), 1),
                        crate::pages::shifted_slot(s, origin, gap.min(if origin.is_some() { 9 } else { 10 }), 1000),
                        "slot {s}, origin {origin:?}, gap {gap}"
                    );
                    rank += 1;
                }
            }
        }
        // Three in hand: three slots stand open.
        assert_eq!((0..5).map(|r| parted(r, 2, 3)).collect::<Vec<_>>(), [0, 1, 5, 6, 7]);
    }

    #[test]
    fn the_gap_goes_to_the_side_of_a_thing_the_pointer_is_on() {
        // Ten things at rest, nothing open yet (the gap rests past the end).
        assert_eq!(gap_at(3, 0.2, 10, 1, 10), 3);
        assert_eq!(gap_at(3, 0.7, 10, 1, 10), 4);
        // Past the last thing: the end.
        assert_eq!(gap_at(40, 0.1, 10, 1, 10), 10);
        // Over the room already made, it stays; beyond it, slots count from after it.
        assert_eq!(gap_at(4, 0.9, 4, 2, 10), 4);
        assert_eq!(gap_at(6, 0.2, 4, 2, 10), 4);
        assert_eq!(gap_at(6, 0.8, 4, 2, 10), 5);
    }

    #[test]
    fn a_folder_arranged_by_hand_keeps_its_order_and_newcomers_go_last() {
        let items = vec![item("a"), item("b"), item("c"), item("new")];
        let order = ["c".to_owned(), "gone".to_owned(), "a".to_owned(), "b".to_owned()];
        let names: Vec<String> = arranged(items, &order).into_iter().map(|it| it.name).collect();
        assert_eq!(names, ["c", "a", "b", "new"]);
    }

    #[test]
    fn a_thing_glides_between_two_cells() {
        let l = Layout::new((920, 600), 1.0);
        let (a, b) = (l.cell(2), l.cell(3));
        let mid = l.cell_at(2.5);
        assert!((mid.x - (a.x + b.x) / 2.0).abs() < 0.01 && (mid.y - a.y).abs() < 0.01);
        assert_eq!(l.cell_at(3.0), b);
        // The slot under a point, and how far across it.
        let (d, fx) = l.slot_at((b.x + b.w / 2.0, b.y + 10.0), 0.0);
        assert_eq!(d, 3);
        assert!(fx > 0.3 && fx < 0.7);
    }

    #[test]
    fn a_list_is_one_column_of_rows_under_its_strip() {
        let l = Layout::of((920.0, 600.0), 1.0, true);
        assert_eq!((l.cols, l.ch), (1, LIST_ROW));
        assert_eq!(l.body.y, HEAD_H + LIST_HEAD);
        let (a, b) = (l.cell(0), l.cell(1));
        assert!((b.y - a.y - LIST_ROW).abs() < 0.01 && (a.w - (l.body.w - 2.0 * PAD)).abs() < 0.01);
        // The row under a point, and how far DOWN it the pointer is.
        assert_eq!(l.index_at((a.x + 200.0, b.y + 4.0), 0.0, 5), Some(1));
        let (d, f) = l.slot_at((a.x + 200.0, b.y + 24.0), 0.0);
        assert_eq!(d, 1);
        assert!(f > 0.7);
        // Both columns in a wide window, none in a narrow one.
        assert!(matches!(l.columns(&a), (Some(_), Some(_))));
        let narrow = Layout::of((320.0, 400.0), 1.0, true);
        assert_eq!(narrow.columns(&narrow.cell(0)).1, None);
    }

    #[test]
    fn by_size_or_date_folders_stay_first_and_by_name_nothing_moves() {
        let mut folder = item("zdir");
        folder.kind = Kind::Folder;
        let items = vec![item("small"), folder, item("big")];
        let meta: HashMap<String, Meta> = [("/x/small", 10u64), ("/x/zdir", 4096), ("/x/big", 900)]
            .into_iter()
            .map(|(p, size)| (p.to_owned(), Meta { size, modified: None }))
            .collect();
        let names = |key, desc| -> Vec<String> {
            sorted(items.clone(), &meta, key, desc).into_iter().map(|it| it.name).collect()
        };
        assert_eq!(names(SortKey::Size, true), ["zdir", "big", "small"]);
        assert_eq!(names(SortKey::Size, false), ["zdir", "small", "big"]);
        assert_eq!(names(SortKey::Name, true), ["small", "zdir", "big"]);
    }

    #[test]
    fn a_search_keeps_what_holds_the_words_whatever_the_case() {
        let all = [item("Informe.pdf"), item("notas.txt"), item("INFORME viejo.odt")];
        let names = |find| -> Vec<String> { found(&all, find).into_iter().map(|it| it.name).collect() };
        assert_eq!(names(Some("inf")), ["Informe.pdf", "INFORME viejo.odt"]);
        assert_eq!(names(Some("")).len(), 3);
        assert_eq!(names(None).len(), 3);
        assert!(names(Some("zzz")).is_empty());
    }

    #[test]
    fn typing_goes_to_the_first_name_that_starts_so() {
        let items = [item("Alpha"), item("beta"), item("Bravo")];
        assert_eq!(typed_match(&items, "b"), Some(1));
        assert_eq!(typed_match(&items, "BR"), Some(2));
        assert_eq!(typed_match(&items, "z"), None);
    }
}

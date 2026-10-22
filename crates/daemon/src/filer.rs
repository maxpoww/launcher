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
//! back, forward and up, the arrows walk the icons and typing searches
//! the folder. The folder is listed off the loop and followed with inotify.
//!
//! A right click opens THE ONE MENU (`desktop_menu::rows_for`, the same rows
//! in the same order as everywhere in Golem), drawn in the window. A name is
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
use wayland_protocols::xdg::toplevel_tag::v1::client::xdg_toplevel_tag_manager_v1::XdgToplevelTagManagerV1;
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle, WEnum};

use crate::content::{
    GridContent, IconInst, Label, Rect, RectInst, Scene, GRID_ICON, GRID_ICON_TOP,
    LABEL_FONT_PX, LABEL_LINE_PX, NO_PLATE, PLATE_STATIC,
};
use crate::desktop::{Item, Kind, PLAIN_TEXT, URI_LIST};
use crate::desktop_menu::{Action, Menu, MenuKey, MenuPaint, Row};
use crate::desktop_props::Props;
use crate::fractional::SurfaceKind;
use crate::i18n::{fill, tr};
use crate::{launch, App};

mod chrome;
mod history;
mod print;
mod recent;
mod smart;

pub(crate) use print::Can;
pub(crate) use smart::{slow_mount, smart_for};

pub(crate) use history::say_quietly;

/// The windows' class: what the compositor, the dock and window memory
/// know them by.
pub(crate) const APP_ID: &str = "golem-files";

/// A window's size until the compositor gives it one (floating), and the
/// least it can be made.
const DEFAULT_SIZE: (u32, u32) = (920, 600);
const MIN_SIZE: (u32, u32) = (380, 260);

/// THE TOP LINE (Max, 2026-10-21, "A · Unified"): the tabs and the way here
/// on its left, sort · i · grid/list · search at its right end, on the
/// page's own colour with no line under it (`filer/chrome.rs`).
const HEAD_H: f32 = 32.0;
/// The room the selection pill takes under the last row (the mockup's
/// 60 px of padding).
const PILL_ROOM: f32 = 60.0;
/// The places' column at rest (the tests' places).
#[cfg(test)]
const SIDE_W: f32 = PLACE_ICON + 2.0 * RAIL_AIR;
/// THE SIDEBAR at rest is a column of the places' SYMBOLS, 48 px; the
/// pointer on it opens it to 200 with their names, OVER the files (or it
/// is pinned open beside them). Its icon is `PLACE_ICON` times the panel's
/// zoom, with `RAIL_AIR` on either side — the zoom is of the icons alone,
/// not their words (Max, 2026-10-14).
const PLACE_ICON: f32 = 16.0;
const RAIL_AIR: f32 = 16.0;
/// How far the words of an open place reach past the rail (200 in all).
const SIDE_WORDS: f32 = 152.0;
/// A place's icon this small or smaller is drawn as its symbol alone.
const SYMBOL_FROM: f32 = 30.0;
/// How fast the sidebar opens and closes, and how narrow a window is left
/// without one.
const SIDE_RATE: f32 = 18.0;
const SIDE_FROM: f32 = 560.0;
/// One place's row.
const ROW_H: f32 = 28.0;
/// Space around the icons' field.
const PAD: f32 = 10.0;
/// A cell is the desktop's with a second line of name: here a name is read
/// to tell files apart, and one line cuts most of them.
const CELL_H: f32 = crate::content::GRID_CELL_H - 8.0 + LABEL_LINE_PX;
/// A cell's width: narrower than the Apps grid's (Max, 2026-10-14: "make the
/// icons in the content be closer to each other") — the things stand nearer,
/// more to a row; a long name takes its second line sooner.
const FILES_CELL_W: f32 = 80.0;
/// From the bottom of what a picture shows to its name's words: about 3 px
/// to its name's box, and the box's own pixel (Max, 2026-10-21).
const LABEL_GAP: f32 = 4.0;
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
/// THE CARD'S SCROLLING (Max, 2026-10-14: "hacé que el scrolling sea como
/// el de la card" — `card/mod.rs::card_scroll_by` / `card_fling`, its
/// numbers): a step of the wheel or the fingers in pixels; how fast and
/// after how far a scroll let go of glides on; how far it carries (its
/// speed for this long); and how quickly the folder follows a wheel's
/// notch, and a glide.
const SCROLL_STEP: f32 = 2.4;
const FLING_FAST: f32 = 900.0;
const FLING_FAR: f32 = 90.0;
const FLING_CARRY: f32 = 0.32;
const RATE_NOTCH: f32 = 24.0;
const RATE_GLIDE: f32 = 4.5;

/// THE LIST: a row a thing, every other one a stripe.
const LIST_ROW: f32 = 32.0;
const LIST_ICON: f32 = 20.0;
/// Where a row's name begins, and its columns' widths (each left out of a
/// window too narrow for it).
const LIST_NAME_X: f32 = 38.0;
const LIST_SIZE_W: f32 = 84.0;
const LIST_DATE_W: f32 = 150.0;
/// A selected thing's soft box (the mockup's): its air at the sides and
/// under the name, and over the picture; the name's own box's (sideways, up
/// and down).
const SEL_PAD: f32 = 4.0;
const SEL_TOP: f32 = 6.0;
const NAME_PAD: (f32, f32) = (5.0, 1.0);
/// The air beside the grid (inside `PAD`) and beside a list's rows, and
/// where a list's first row begins.
const GRID_AIR: f32 = 10.0;
const LIST_AIR: f32 = 8.0;
const LIST_TOP: f32 = HEAD_H + 4.0;

/// The things of a folder whose names hold `find` (whatever the case);
/// all of them for no search.
pub(crate) fn found(all: &[Item], find: Option<&str>) -> Vec<Item> {
    match find.map(str::to_lowercase).filter(|f| !f.is_empty()) {
        Some(find) => all.iter().filter(|it| it.name.to_lowercase().contains(&find)).cloned().collect(),
        None => all.to_vec(),
    }
}

/// A search goes on into the folders below, a moment after the typing
/// stops: this deep, for this long, for at most this many things.
const DEEP_WAIT: Duration = Duration::from_millis(250);
const DEEP_DEPTH: usize = 7;
const DEEP_TIME: Duration = Duration::from_secs(3);
const DEEP_MAX: usize = 300;

/// The things called something with `find` in it in the folders BELOW
/// `dir` (its own are the listing's), nearest first. Links to folders are
/// not followed, and dot folders only when `hidden`.
pub(crate) fn search_below(dir: &Path, find: &str, hidden: bool) -> Vec<Item> {
    let find = find.to_lowercase();
    let started = Instant::now();
    let mut out = Vec::new();
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = std::collections::VecDeque::new();
    queue.push_back((dir.to_path_buf(), 0));
    while let Some((at, depth)) = queue.pop_front() {
        if out.len() >= DEEP_MAX || started.elapsed() > DEEP_TIME {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        let mut here: Vec<Item> = Vec::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            // (The entry's own kind: a link is not gone into.)
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            if is_dir && depth + 1 < DEEP_DEPTH {
                queue.push_back((e.path(), depth + 1));
            }
            if depth > 0 && name.to_lowercase().contains(&find) {
                here.push(crate::desktop::item_for(e.path(), name, is_dir));
            }
        }
        here.sort_by(|a, b| by_name(&a.name, &b.name));
        out.extend(here);
    }
    out.truncate(DEEP_MAX);
    out
}

/// Where under `dir` a thing found below it is: its folder, from `dir`.
pub(crate) fn below(path: &str, dir: &Path) -> String {
    Path::new(path)
        .parent()
        .and_then(|p| p.strip_prefix(dir).ok())
        .map_or_else(String::new, |p| p.to_string_lossy().into_owned())
}

/// What a folder's things are ordered by in a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SortKey {
    /// The folder's own order: by name, or as arranged by hand.
    #[default]
    Name,
    Size,
    Modified,
    /// By kind: the ending, then the name.
    Kind,
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

/// `items` by size, date or kind (folders first, as ever), biggest, newest
/// or last first when `desc`. By name they stay in the folder's own order
/// (by name, or as arranged by hand) — turned round when `desc`.
pub(crate) fn sorted(mut items: Vec<Item>, meta: &HashMap<String, Meta>, key: SortKey, desc: bool) -> Vec<Item> {
    if key == SortKey::Name {
        // (The folder's own order — by name, or as arranged by hand — and
        // turned round, folders still first.)
        if desc {
            let split = items.iter().position(|it| it.kind != Kind::Folder).unwrap_or(items.len());
            items[..split].reverse();
            items[split..].reverse();
        }
        return items;
    }
    let of = |it: &Item| meta.get(&it.path).copied().unwrap_or_default();
    let ending = |it: &Item| it.name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    items.sort_by(|a, b| {
        let by = match key {
            SortKey::Size => of(a).size.cmp(&of(b).size),
            SortKey::Kind => ending(a).cmp(&ending(b)).then_with(|| by_name(&a.name, &b.name)),
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
/// How long a drag is held on a folder before it opens (the dock's own).
const SPRING: Duration = Duration::from_millis(650);

/// How far a press travels before it is a drag (or a rubber band).
const DRAG_START: f32 = 6.0;
// (The band's and the washes' colours are the look's:
// `Palette`.)

/// The app's own launcher, written where the dock's index finds it: a tile
/// in the grid that opens a window on Home (`waverunner-ctl files`, the ctl
/// that sits beside this daemon; with a folder after it, on that folder),
/// and the class by which the dock knows the windows for it. Written only
/// when it changes, as the webapps' launchers are.
///
/// IT IS THE SYSTEM'S FILE MANAGER (Max, 2026-10-13: "hacé Golem Files
/// default"): the entry says it opens folders (`MimeType=inode/directory`),
/// and — ONCE, so a later choice of the owner's in the control panel stands
/// — it is made the default for them (`xdg-mime default`, the way the
/// control panel's own default apps are set). Golem's `home.nix` names it
/// first for new installs.
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
         Exec={ctl} files %f\nIcon=golem-files\nStartupWMClass={APP_ID}\n\
         MimeType=inode/directory;\nCategories=System;FileManager;\nTerminal=false\n"
    );
    let path = dir.join(format!("{APP_ID}.desktop"));
    if !std::fs::read_to_string(&path).is_ok_and(|c| c == contents) {
        if let Err(e) = std::fs::write(&path, contents) {
            warn!("files: cannot write {}: {e}", path.display());
            return;
        }
    }
    // The default for folders, once.
    let marker = crate::persist::data_path("files-default-set");
    if marker.exists() {
        return;
    }
    std::thread::spawn(move || {
        let set = std::process::Command::new("xdg-mime")
            .args(["default", &format!("{APP_ID}.desktop"), "inode/directory"])
            .status()
            .is_ok_and(|s| s.success());
        if set {
            info!("files: Golem Files is the system's file manager from now on");
            crate::persist::write_text("files-default", &marker, "1\n");
        } else {
            warn!("files: could not be made the default for folders (xdg-mime)");
        }
    });
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
    /// The head's `..` (only where the places, and theirs, are left out).
    Up,
    /// The sidebar's two modes at its top: 0 Files, 1 Media.
    Mode(usize),
    /// The pin in the sidebar's corner.
    Pin,
    /// A tab of the top line that is not the current one, a tab's ×, the +.
    Tab(usize),
    TabX(usize),
    TabAdd,
    /// The current tab's pencil: its path as a field to type in.
    Pencil,
    /// The top line's right end: the sort menu, the i, the view (a list?),
    /// the magnifier.
    Sort,
    Info,
    View(bool),
    Find,
    /// The ↶ (left of sort, while something can be taken back); a row of
    /// its history, that row's Undo (by the row's number in the history).
    Undo,
    HistRow(usize),
    HistUndo(usize),
    /// The selection pill's `n`th button; the folder's own action.
    Pill(usize),
    Here,
    /// A part of the chrome that takes the pointer and does nothing (the
    /// sidebar's ground, the search field, the i card).
    Chrome,
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
    /// The CONTENT's zoom (Max, 2026-10-14: "ctrl +/-/0 independent on the
    /// content than the panel"): the field is laid out `1/zoom` its size
    /// from its top-left corner — `body` is that laid-out field — and drawn
    /// `zoom` times bigger from there (`zoom_grid`); the places and the head
    /// keep their size. A point is brought into the field by [`Self::inward`]
    /// and out of it by [`Self::outward`].
    pub zoom: f32,
    /// Room under the last row (the selection pill floats there), in the
    /// field's own size.
    pub foot: f32,
}

impl Layout {
    #[cfg(test)]
    pub fn new((w, h): (u32, u32), icon_scale: f32) -> Self {
        Self::of((w as f32, h as f32), icon_scale, false, 1.0, SIDE_W)
    }

    /// A window of `w`×`h`; its field at `zoom`, beside places `rail` wide.
    pub fn of((w, h): (f32, f32), icon_scale: f32, list: bool, zoom: f32, left: f32) -> Self {
        let zoom = zoom.max(0.1);
        // The field begins where the sidebar ends (at rest, or pinned
        // open), under the top line. (Its own size, as it is laid out: the
        // room it has, `zoom` times smaller.)
        let side_w = left;
        let air = if list { LIST_AIR } else { GRID_AIR };
        let room_w = ((w - left - 2.0 * air) / zoom).max(1.0);
        if list {
            // One column as wide as the field: a row is a cell, so all that
            // knows cells (the pointer, the keys, a band, a drag's room)
            // knows rows.
            let top = LIST_TOP;
            let body = Rect::new(left + air, top, room_w, ((h - top) / zoom).max(1.0));
            let room = (body.w - 2.0 * PAD).max(1.0);
            return Self {
                side_w,
                body,
                cols: 1,
                pitch: room,
                cw: room,
                ch: LIST_ROW,
                list,
                zoom,
                foot: 0.0,
            };
        }
        // As many columns as fit, the room left shared between them: the
        // same air on both sides.
        let body = Rect::new(left + air, HEAD_H, room_w, ((h - HEAD_H) / zoom).max(1.0));
        let (cw, ch) = (FILES_CELL_W * icon_scale, CELL_H * icon_scale);
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
            zoom,
            foot: 0.0,
        }
    }

    /// The field as it is drawn: `body` at the zoom.
    pub fn shown_body(&self) -> Rect {
        Rect::new(self.body.x, self.body.y, self.body.w * self.zoom, self.body.h * self.zoom)
    }

    /// A point of the window brought into the field's own space — when it is
    /// over the field; a point over the places or the head stays as it is.
    pub fn inward(&self, p: (f32, f32)) -> (f32, f32) {
        if !self.shown_body().contains(p) {
            return p;
        }
        (self.body.x + (p.0 - self.body.x) / self.zoom, self.body.y + (p.1 - self.body.y) / self.zoom)
    }

    /// A point of the field's own space where it is drawn in the window (a
    /// point that is not in the field stays as it is).
    pub fn outward(&self, p: (f32, f32)) -> (f32, f32) {
        if !self.body.contains(p) {
            return p;
        }
        (self.out_x(p.0), self.body.y + (p.1 - self.body.y) * self.zoom)
    }

    /// An x of the field's own space where it is drawn.
    pub fn out_x(&self, x: f32) -> f32 {
        self.body.x + (x - self.body.x) * self.zoom
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
        (self.content_h(n) + self.foot - self.body.h).max(0.0)
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

/// A drag held on `..` (see `Win::up_hold`).
#[derive(Debug, Clone, Copy)]
struct UpHold {
    since: Instant,
    due: Duration,
    ups: u32,
}

/// How long after the first step up the second comes, then each after it.
const UP_SECOND: Duration = Duration::from_secs(2);
const UP_NEXT: Duration = Duration::from_secs(1);

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
    /// What its release does, with no travel (a SECOND click on the one
    /// thing selected — Finder's way, Max 2026-10-21): on its picture it
    /// opens, on its name the name is typed.
    then: Then,
}

/// What a second click on the one thing selected does when let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Then {
    #[default]
    Nothing,
    Open,
    Rename,
}

/// Which part of a thing the pointer is on: its picture, its name, or
/// neither (a list row's size or date).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Picture,
    Name,
    Neither,
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
    /// It has been let go: the source lives on until the target has read
    /// the files (a long copy), but nothing is in hand any more — the
    /// things are drawn in their places again and no later drag is "ours".
    dropped: bool,
    /// The dock was hidden and came up for this drag (its bin takes
    /// things): it goes again when they are let go.
    dock_raised: bool,
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

/// Things of this very folder let go in the room made for them: `items`
/// with those `carried` (paths) standing at `gap` among the rest, and each
/// thing's display slot — the rest keep theirs (they are already where the
/// room put them), the carried take their new places at once. So what is
/// let go LANDS: nothing goes back to where it was first.
pub(crate) fn settled(items: Vec<Item>, slide: &[f32], carried: &[String], gap: usize) -> (Vec<Item>, Vec<f32>) {
    let (mut rest, mut moved) = (Vec::new(), Vec::new());
    for (i, item) in items.into_iter().enumerate() {
        if carried.contains(&item.path) {
            moved.push(item);
        } else {
            rest.push((item, slide.get(i).copied().unwrap_or(i as f32)));
        }
    }
    let gap = gap.min(rest.len());
    let after = rest.split_off(gap);
    let mut out = Vec::with_capacity(rest.len() + moved.len() + after.len());
    let mut at = Vec::with_capacity(out.capacity());
    for (item, slot) in rest {
        out.push(item);
        at.push(slot);
    }
    for item in moved {
        at.push(out.len() as f32);
        out.push(item);
    }
    for (item, slot) in after {
        out.push(item);
        at.push(slot);
    }
    (out, at)
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

/// Where the open windows' folders are noted, so a dock that is started
/// again (an update, a crash) brings them back: the runtime dir, which a
/// new login starts empty.
fn nav_path() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned()))
        .join("waverunner-files-nav")
}

fn session_path() -> PathBuf {
    PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_owned()))
        .join("waverunner-files-session.json")
}

/// The things from `anchor` to `to`, whichever comes first: what Shift
/// with a click or an arrow selects.
pub(crate) fn span(anchor: usize, to: usize) -> std::ops::RangeInclusive<usize> {
    anchor.min(to)..=anchor.max(to)
}

/// Something done in a file window that Ctrl+Z takes back.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Undo {
    Renamed { from: PathBuf, to: PathBuf },
    /// Each thing: where it was, where it is now.
    Moved(Vec<(PathBuf, PathBuf)>),
    /// The copies made.
    Copied(Vec<PathBuf>),
    /// Thrown away: their names in the bin.
    Binned(Vec<String>),
    /// A folder made.
    Made(PathBuf),
    /// Things put together in a new folder ("New folder with these"):
    /// the folder, and each thing's place before and in it.
    Grouped { folder: PathBuf, moved: Vec<(PathBuf, PathBuf)> },
    /// A photo made the wallpaper: the one before, and it.
    Wallpaper { was: String, now: String },
    /// Photos turned a quarter clockwise.
    Rotated(Vec<PathBuf>),
    /// An app made the one for a kind of file ("Always use for…"): the
    /// kind, in words, the app's name, the one before (none: the system's
    /// own), the one now (`<id>.desktop`).
    Default {
        mime: String,
        kind: String,
        app: String,
        was: Option<String>,
        now: String,
    },
}

/// What bringing `sources` in did, as things to take back: those that are
/// no longer where they were moved, the others were copied. (Only when
/// every one arrived — else which is which cannot be told.)
pub(crate) fn undo_of(sources: &[PathBuf], brought: &[PathBuf]) -> Vec<Undo> {
    if sources.len() != brought.len() {
        return Vec::new();
    }
    let (mut moved, mut copied) = (Vec::new(), Vec::new());
    for (from, now) in sources.iter().zip(brought).filter(|(a, b)| a != b) {
        if from.exists() {
            copied.push(now.clone());
        } else {
            moved.push((from.clone(), now.clone()));
        }
    }
    let mut out = Vec::new();
    if !moved.is_empty() {
        out.push(Undo::Moved(moved));
    }
    if !copied.is_empty() {
        out.push(Undo::Copied(copied));
    }
    out
}

/// Where a copy of `name` beside it goes: "name copy", then "name copy 2"…
/// — before a file's ending (`desktop::ending`), after a folder's whole name.
pub(crate) fn copy_name(dir: &Path, name: &str, folder: bool) -> PathBuf {
    let (stem, end) = match crate::desktop::ending(name).filter(|_| !folder) {
        Some(end) => (&name[..name.len() - end.len() - 1], format!(".{end}")),
        None => (name, String::new()),
    };
    let copy = tr("copy");
    (1..)
        .map(|n| {
            let mark = if n == 1 { copy.to_owned() } else { format!("{copy} {n}") };
            dir.join(format!("{stem} {mark}{end}"))
        })
        .find(|p| !p.exists())
        .unwrap_or_else(|| dir.join(format!("{stem} {copy}{end}")))
}

/// Take `undo` back; what to say of it (`None`: nothing could be done).
pub(crate) fn take_back(undo: Undo) -> Option<String> {
    // One at a time: each runs on a thread of its own, and two quick Ctrl+Z
    // both found a place free, then both moved into it (the first put back
    // was lost under the second). Now the second looks after the first.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let trash = crate::trash::Trash::home();
    match undo {
        Undo::Renamed { from, to } => (to.exists() && !from.exists() && std::fs::rename(&to, &from).is_ok())
            .then(|| tr("Undone: it has its old name again").to_owned()),
        Undo::Moved(pairs) => {
            // Each back where it was — never over something there now.
            let n = pairs
                .iter()
                .filter(|(from, now)| {
                    now.exists()
                        && !from.exists()
                        && (std::fs::rename(now, from).is_ok()
                            || (crate::desktop_send::copy_all_metered(now, from, None).is_ok()
                                && std::process::Command::new("sync")
                                    .arg("-f")
                                    .arg(from)
                                    .status()
                                    .is_ok_and(|s| s.success())
                                && (if now.is_dir() {
                                    std::fs::remove_dir_all(now)
                                } else {
                                    std::fs::remove_file(now)
                                })
                                .is_ok()))
                })
                .count();
            (n > 0).then(|| fill(tr("Undone: {n} moved back"), &[("n", &n)]))
        }
        Undo::Copied(copies) => {
            let n = copies.iter().filter(|c| c.exists() && trash.trash(c).is_ok()).count();
            (n > 0).then(|| fill(tr("Undone: {n} copies moved to the bin"), &[("n", &n)]))
        }
        Undo::Binned(names) => {
            let known = trash.list();
            let n = names
                .iter()
                .filter(|name| {
                    known
                        .iter()
                        .find(|t| &t.name == *name)
                        .is_some_and(|t| !t.original_path.exists() && trash.restore(name).is_ok())
                })
                .count();
            (n > 0).then(|| fill(tr("Undone: {n} put back from the bin"), &[("n", &n)]))
        }
        // (Only while it is still empty: what was put in it since is not ours to take.)
        Undo::Made(path) => std::fs::remove_dir(&path)
            .is_ok()
            .then(|| tr("Undone: the new folder is gone").to_owned()),
        // Each thing back where it was, then the folder — only if that left
        // it empty.
        Undo::Grouped { folder, moved } => {
            let n = moved
                .iter()
                .filter(|(from, now)| now.exists() && !from.exists() && std::fs::rename(now, from).is_ok())
                .count();
            let _ = std::fs::remove_dir(&folder);
            (n > 0).then(|| fill(tr("Undone: {n} moved back"), &[("n", &n)]))
        }
        // The wallpaper before, put up again (through the box's own pick).
        Undo::Wallpaper { was, .. } => {
            crate::wall_follow::picked_other(move || crate::controls::pick_wallpaper(was));
            Some(tr("Undone: the wallpaper before is back").to_owned())
        }
        // Turned back: three quarters more, as losslessly.
        Undo::Rotated(paths) => {
            let n = paths.iter().filter(|p| p.exists() && smart::rotate(p, 3)).count();
            (n > 0).then(|| fill(tr("Undone: {n} turned back"), &[("n", &n)]))
        }
        // The app before, the one again (or none of the owner's own).
        Undo::Default { mime, kind, was, .. } => {
            let back = match &was {
                Some(was) => crate::other_apps::set_default(was, &mime),
                None => crate::other_apps::unset_default(&mime),
            };
            back.then(|| fill(tr("Undone: {kind} open as before"), &[("kind", &kind)]))
        }
    }
}

/// Whether `undo` can still be taken back, the disk as it is now.
pub(crate) fn can_take_back(undo: &Undo) -> bool {
    let bin = if matches!(undo, Undo::Binned(_)) { crate::trash::Trash::home().list() } else { Vec::new() };
    history::can_undo(undo, &bin)
}

/// The folder a typed path stands for: `~` is Home; a file's is the one
/// it is in (and the file, to be shown there).
pub(crate) fn typed_place(text: &str, home: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    let text = text.trim();
    let path = tidy(match text.strip_prefix('~') {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(text),
    });
    if !path.is_absolute() {
        return None;
    }
    if path.is_dir() {
        Some((path, None))
    } else if path.exists() {
        Some((path.parent()?.to_path_buf(), Some(path)))
    } else {
        None
    }
}

/// A path as one writes it: no trailing `/`, no `.` steps (a test round,
/// 2026-10-14: a path finished with Tab came in as `…/Alfa/`, and the window's
/// folder then never equalled the same folder reached another way).
pub(crate) fn tidy(path: PathBuf) -> PathBuf {
    path.components().collect()
}

/// What a window's title says for `dir`: the places' own names for Home and
/// the bin (they read "max" and "files" as folders), else the folder's.
fn window_title(dir: &Path) -> String {
    if dir == home() {
        tr("Home").to_owned()
    } else if dir == bin_root() {
        tr("Recycle Bin").to_owned()
    } else if dir == recent::root() {
        tr("Recents").to_owned()
    } else {
        file_name(dir)
    }
}

/// Tab in a typed path: as much of the name being typed as only one thing
/// in that folder can be (a folder gets its `/`); the text as it was when
/// nothing there starts so.
pub(crate) fn complete_path(text: &str, home: &Path) -> String {
    let Some((head, part)) = text.rsplit_once('/') else {
        return text.to_owned();
    };
    let dir = match head.strip_prefix('~') {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None if head.is_empty() => PathBuf::from("/"),
        None => PathBuf::from(head),
    };
    let mut names: Vec<(String, bool)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name.starts_with(part) && (part.starts_with('.') || !name.starts_with('.')))
                .then(|| (name, e.path().is_dir()))
        })
        .collect();
    names.sort();
    let Some((first, is_dir)) = names.first().cloned() else {
        return text.to_owned();
    };
    // As far as they all agree.
    let mut common: String = first.clone();
    for (name, _) in &names[1..] {
        let same = common.chars().zip(name.chars()).take_while(|(a, b)| a == b).count();
        common = common.chars().take(same).collect();
    }
    let tail = if names.len() == 1 && is_dir { "/" } else { "" };
    format!("{head}/{common}{tail}")
}

/// "SHOW IN FOLDER": what another app asks of the system's file manager
/// over `org.freedesktop.FileManager1` (a browser's downloads, a mail
/// attachment) — these folders opened, or these things shown in theirs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Shown {
    Folders(Vec<String>),
    Items(Vec<String>),
}

/// The D-Bus face of the file manager: it only hands what is asked to the
/// loop, where the windows are.
struct FileManager1 {
    asked: calloop::channel::Sender<Shown>,
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager1 {
    fn show_folders(&self, uris: Vec<String>, _startup_id: String) {
        let _ = self.asked.send(Shown::Folders(uris));
    }

    fn show_items(&self, uris: Vec<String>, _startup_id: String) {
        let _ = self.asked.send(Shown::Items(uris));
    }

    /// (No box of its own from here: the thing is shown, its menu has it.)
    fn show_item_properties(&self, uris: Vec<String>, _startup_id: String) {
        let _ = self.asked.send(Shown::Items(uris));
    }
}

/// Be `org.freedesktop.FileManager1` on the session bus for as long as the
/// dock lives. The name is asked for over whoever has it (a file manager
/// that lets go gives it at once; one that does not, when it ends — we
/// wait in line). On a thread of its own.
fn serve_file_manager(asked: calloop::channel::Sender<Shown>) {
    let spawned = std::thread::Builder::new().name("files-dbus".into()).spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                warn!("files: cannot build the D-Bus runtime: {e}");
                return;
            }
        };
        rt.block_on(async move {
            let served: zbus::Result<zbus::Connection> = async {
                let conn = zbus::connection::Builder::session()?
                    .serve_at("/org/freedesktop/FileManager1", FileManager1 { asked })?
                    .build()
                    .await?;
                let got = conn
                    .request_name_with_flags(
                        "org.freedesktop.FileManager1",
                        zbus::fdo::RequestNameFlags::ReplaceExisting.into(),
                    )
                    .await?;
                info!("files: org.freedesktop.FileManager1 on the session bus ({got:?})");
                Ok(conn)
            }
            .await;
            match served {
                // (The connection is what serves: it is held for good.)
                Ok(_conn) => std::future::pending::<()>().await,
                Err(e) => warn!("files: \"show in folder\" stays another's (D-Bus: {e})"),
            }
        });
    });
    if let Err(e) = spawned {
        warn!("files: cannot spawn the D-Bus thread: {e}");
    }
}

/// The folders to open for what was asked, each with the thing to show
/// selected in it (the first asked of that folder), in the order asked.
pub(crate) fn shown_places(asked: &Shown) -> Vec<(PathBuf, Option<PathBuf>)> {
    let paths = |uris: &[String]| crate::desktop::uri_list_paths(&uris.join("\n"));
    match asked {
        Shown::Folders(uris) => paths(uris).into_iter().map(|dir| (dir, None)).collect(),
        Shown::Items(uris) => {
            let mut out: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
            for item in paths(uris) {
                let Some(dir) = item.parent().map(Path::to_path_buf) else {
                    continue;
                };
                if !out.iter().any(|(d, _)| *d == dir) {
                    out.push((dir, Some(item)));
                }
            }
            out
        }
    }
}

/// A name being typed in place.
#[derive(Debug, Clone, PartialEq)]
struct Rename {
    path: String,
    text: String,
    /// How much of the name is selected from its start, in bytes: as it
    /// opens, the STEM (the ending stays: Max, 2026-10-21, the mockup's
    /// rename) — the first key replaces it, a Backspace clears it.
    picked: usize,
    /// Where the typing goes (a byte index of `text`).
    caret: usize,
}

/// The apps `gio mime <type>` names for a kind of file, as desktop ids
/// (no `.desktop`): the default first, then the others, each once.
pub(crate) fn parse_gio_mime(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        // "Default application for “x/y”: id.desktop", then tab-set lists.
        let id = match line.strip_prefix('\t') {
            Some(id) => id,
            None => match line.split_once("”: ").or_else(|| line.split_once("': ")) {
                Some((head, id)) if head.starts_with("Default application") => id,
                _ => continue,
            },
        };
        let id = id.trim().trim_end_matches(".desktop").to_owned();
        if !id.is_empty() && !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// The apps that say they open the file at `path` (see `parse_gio_mime`).
/// Asks two programs: call it off the loop.
pub(crate) fn apps_for(path: &str) -> Vec<String> {
    let said = |program: &str, args: &[&str]| -> String {
        std::process::Command::new(program)
            .args(args)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let kind = said("xdg-mime", &["query", "filetype", path]);
    let kind = kind.trim();
    if kind.is_empty() {
        return Vec::new();
    }
    parse_gio_mime(&said("gio", &["mime", kind]))
}

/// The owner's own places: the bookmarks GTK's file dialogs keep
/// (`gtk-3.0/bookmarks`, a line a folder: its `file://` address, then a name
/// if it was given one) — so a folder kept here is there when a file is
/// opened or saved from any app, and the other way round.
fn marks_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map_or_else(|| home().join(".config"), PathBuf::from)
        .join("gtk-3.0/bookmarks")
}

/// Each bookmark of `text` that is a folder on this computer: its name
/// (the one given, else the folder's) and where it is.
pub(crate) fn parse_marks(text: &str) -> Vec<(String, PathBuf)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (uri, label) = line.split_once(' ').unwrap_or((line, ""));
            let path = crate::desktop::uri_list_paths(uri).into_iter().next()?;
            let name = if label.trim().is_empty() { file_name(&path) } else { label.trim().to_owned() };
            Some((name, path))
        })
        .collect()
}

/// `text` with `path` kept (`keep`) or taken out: its line added at the
/// end if it is not there, or every line that is it removed; the others as
/// they are (those of other kinds too — a network place stays).
pub(crate) fn marks_with(text: &str, path: &Path, keep: bool) -> String {
    let is_it = |line: &str| {
        let uri = line.trim().split(' ').next().unwrap_or("");
        crate::desktop::uri_list_paths(uri).first().is_some_and(|p| p == path)
    };
    let mut lines: Vec<String> = text.lines().filter(|l| !l.trim().is_empty()).map(str::to_owned).collect();
    let there = lines.iter().any(|l| is_it(l));
    if keep && !there {
        lines.push(crate::desktop::file_uri(&path.to_string_lossy()));
    } else if !keep {
        lines.retain(|l| !is_it(l));
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
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

/// The clickable parts whose places are only known once drawn.
#[derive(Debug, Default)]
struct Hits {
    up: Rect,
    crumbs: Vec<(Rect, PathBuf)>,
    places: Vec<Rect>,
    find: Rect,
    /// The list's column names.
    cols: Vec<Rect>,
    /// THE CHROME as drawn (`filer/chrome.rs`): the sidebar, its pin and
    /// its two modes; the tabs (the others, each ×, the +), the current
    /// one and its pencil; the right end's buttons and the search field;
    /// the i card.
    side: Rect,
    pin: Rect,
    mode: [Rect; 2],
    tabs: Vec<(Rect, usize)>,
    tab_x: Vec<(Rect, usize)>,
    tab_add: Rect,
    cur_tab: Rect,
    pencil: Rect,
    sort: Rect,
    info: Rect,
    grid: Rect,
    list: Rect,
    seek: Rect,
    card: Rect,
    /// The ↶, the history under it, and each row's Undo (the row's number
    /// in the history) and the row itself.
    undo: Rect,
    hist: Rect,
    hist_undo: Vec<(Rect, usize)>,
    hist_rows: Vec<(Rect, usize)>,
    /// The folder's own action by the ↶ ("Play all", "Slideshow").
    here: Option<(Rect, Action)>,
    /// THE SELECTION PILL, and its buttons.
    pill: Rect,
    pill_btns: Vec<(Rect, chrome::PillAct)>,
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
    /// The owner's zoom of the file windows' field (`App::files_zoom`),
    /// kept here for the layout…
    zoom: f32,
    /// …and of their places' ICONS (`App::files_panel_zoom`): the rail and
    /// the rows grow with them, the words do not.
    panel: f32,
    /// The pointer is over the field (not the panel): what Ctrl +/−/0 zoom.
    over_field: bool,
    /// The pointer as it is in the laid-out window (not brought into the
    /// field): what opens the places.
    at_win: Option<(f32, f32)>,
    /// The sidebar: open over the files (wanted), and how open it is
    /// (0 = the rail, 1 = open; eased).
    side_open: bool,
    side_t: f32,
    /// The places scrolled (Max, 2026-10-14: "add scroll to the sidebar"):
    /// where they are and where the wheel sent them.
    side_scroll: f32,
    side_scroll_to: f32,
    /// The pointer came onto the sidebar at rest: it opens after a breath
    /// (`chrome::SIDE_WAIT`) if it stays. Pinned, it stays open beside the
    /// files (`settings.files_pinned`, one choice for every window).
    side_wait: Option<Instant>,
    pinned: bool,
    /// The mode: MEDIA shows only pictures and videos.
    media: bool,
    /// THE TABS (Max, 2026-10-21): each its own folder and its own way back
    /// and forward. The current one's live in `dir`, `back` and `forward`;
    /// its entry here is brought up to date when another is taken.
    tabs: Vec<chrome::Tab>,
    tab: usize,
    tab_seq: u64,
    /// Each tab's width as drawn, easing to its own (a tab folds into its
    /// name and unfolds into its path).
    tab_w: HashMap<u64, f32>,
    /// The current tab's width when its pencil was taken: the field's.
    edit_w: f32,
    /// The search field out (eased to `find.is_some()`).
    find_t: f32,
    /// The i card is up, and what it says (asked for off the loop when the
    /// selection changes).
    info_on: bool,
    info: Option<chrome::Info>,
    info_asked: Option<String>,
    /// Shown as a list (`settings.files_list`, one choice for every window).
    list: bool,
    /// What the things are ordered by here, biggest or newest first when
    /// the flag is set; and what the disk says of each.
    sort: (SortKey, bool),
    meta: HashMap<String, Meta>,
    /// A path being typed in the head (Ctrl+L, or a `/` or `~` typed):
    /// Enter goes there.
    go_to: Option<String>,
    /// The typed path was nothing there: its field is red until the next key.
    go_bad: bool,
    /// A search of this folder: what is typed (the field is out while this
    /// is `Some`), and everything the folder holds — `items` is then only
    /// what was found.
    find: Option<String>,
    all: Vec<Item>,
    /// What the search found in the folders below (they follow the
    /// folder's own in `items`, from `deep_from` on); which search is
    /// awaited; and whether one is still walking.
    deep: Vec<Item>,
    deep_from: usize,
    deep_gen: u64,
    deep_busy: bool,
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
    /// Where a selection began: Shift selects from here to the keys.
    anchor: Option<usize>,
    /// A drag held on a folder: which, and since when — held long enough,
    /// the folder opens under it.
    spring: Option<(usize, Instant)>,
    /// …and on a place of the sidebar (its number); the place blinking
    /// just before it opens.
    place_spring: Option<(usize, Instant)>,
    place_blink: Option<usize>,
    /// THE HISTORY (`filer/history.rs`): what was done in this window, the
    /// newest last, for as long as it is open; the ↶'s slide (0 away, 1
    /// out); its menu up, and how far down its rows it is scrolled (in rows).
    history: Vec<history::Did>,
    undo_t: f32,
    hist_open: bool,
    hist_first: f32,
    /// When the history up was last read against the disk.
    hist_read: Option<Instant>,
    /// What the folder shown is mostly (the button by the ↶), for which
    /// listing.
    here_kind: Option<(u64, Option<smart::FolderKind>)>,
    /// THE SELECTION PILL (`chrome::filer_pill`): how far it is in (0..1),
    /// what it shows (kept while it goes), whether it is wanted (the files
    /// then get room under their last row); a line it says for a moment.
    pill_t: f32,
    pill_view: Option<chrome::PillView>,
    pill_on: bool,
    flash: Option<(String, Instant)>,
    /// Where this window was before a drag held on folders (or on `..`) took
    /// it elsewhere, and how long its way back was then. Let go here, it STAYS
    /// where the things were let go and its way back is that one folder (Max,
    /// 2026-10-15: "I want to stay where I dropped… and have / on back"); the
    /// drag leaving without a drop takes it back there.
    sprung_from: Option<(PathBuf, usize)>,
    /// A drag held on `..`: it climbs (Max, 2026-10-15: "hovering a file on
    /// .. goes back… held another 2 secs it goes another level up and then a
    /// level up every second") — since when, after how long the next step,
    /// how many steps so far.
    up_hold: Option<UpHold>,
    /// A path to select and bring into view once it is listed (the folder
    /// just come out of).
    reveal: Option<String>,
    scroll: f32,
    scroll_to: f32,
    places: Vec<Place>,
    /// Its own root, when it was opened from outside on a folder under none
    /// of the others (`floor`).
    root: Option<PathBuf>,

    chains: HashMap<String, Vec<u8>>,
    /// How much of each picture's square is empty above and under what it
    /// shows (`ink_rows`), for it to sit on the bottom of its space.
    ink: HashMap<String, (f32, f32)>,
    uploaded: Vec<Option<String>>,
    capacity: u32,
    pending: HashSet<String>,
    missing: HashSet<String>,

    ptr: Option<(f32, f32)>,
    hover: Spot,
    /// The scroll being reported comes from a wheel (notches), not fingers.
    /// The scroll comes from FINGERS on a touchpad (the folder then follows
    /// them step for step); how fast it was going (px/s) and when its last
    /// step was; how far this scroll has gone; and whether the folder is
    /// GLIDING on after a fast one was let go.
    finger: bool,
    scroll_vel: f32,
    scroll_last: Option<Instant>,
    scroll_run: f32,
    gliding: bool,
    click: Option<(Instant, String)>,
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
    /// A second click on the selected thing's name: its name is typed
    /// once the double click's time is out, unless another click came
    /// (`filer_rename_soon`; the counter tells the waits apart).
    rename_due: Option<u64>,
    rename_seq: u64,
    /// The thing a second click on its picture opened, and when (the next
    /// click of a double click does not open it again).
    opened: Option<(Instant, String)>,

    /// A row that cannot be undone was clicked once: the second click does it.
    armed: Option<Action>,
    /// The apps of the menu's "Open with" page: name, command, in a
    /// terminal.
    open_with: Vec<(String, String, bool)>,
    /// The "Open with" page is up and its apps are being asked for.
    asking_apps: bool,
    /// The folders that are the owner's own places (bookmarks), and the one
    /// a menu opened on a place's row is about.
    marks: Vec<PathBuf>,
    menu_place: Option<PathBuf>,
    /// The folder changed while it was being read: read once more after.
    again: bool,
    /// The place a hovering drag would be let go onto.
    drop_place: Option<usize>,
    /// A hovering drag is near the field's top (< 0) or bottom (> 0) edge:
    /// how fast the folder scrolls for it, −1..1; where the drag is (real
    /// pixels), to read again what is under it as the folder moves; and
    /// whether the timer that does it is running.
    edge: f32,
    edge_at: (f32, f32),
    edge_timer: bool,
    /// How far the wheel has turned with Ctrl held since the last step of
    /// zoom it made.
    zoom_run: f32,
    /// Where each thing is drawn, as a display slot (its own place in the
    /// list at rest; easing to make room while a drag is over the window).
    slide: Vec<f32>,
    /// The gap a hovering drag has open, counted among the things not in
    /// hand: where what it carries would land.
    gap: Option<usize>,
    /// Something was let go in the gap and is on its way (being copied,
    /// listed): the gap stays open for it until it is there.
    gap_keep: bool,
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
    /// the folder shown. (The gap stays open: what comes arrives into it.)
    fn arrangement(&mut self, dir: &Path, carried: &[String]) -> Option<(Vec<String>, Vec<String>)> {
        let g = self.gap.filter(|_| dir == self.dir)?;
        let names: Vec<String> = self
            .items
            .iter()
            .filter(|it| !carried.contains(&it.path))
            .map(|it| file_name(Path::new(&it.path)))
            .collect();
        let g = g.min(names.len());
        Some((names[..g].to_vec(), names[g..].to_vec()))
    }

    /// Put in `items` what is to be shown: all the folder holds, or — while
    /// a search has words — what it found here, then what it found below.
    fn show_found(&mut self) {
        self.items = found(&self.all, self.find.as_deref());
        // (MEDIA: only the pictures and the videos.)
        if self.media {
            self.items.retain(|it| it.kind == Kind::File && is_media(&it.name));
        }
        self.deep_from = usize::MAX;
        if self.find.as_ref().is_some_and(|f| !f.is_empty()) {
            self.deep_from = self.items.len();
            self.items.extend(self.deep.iter().cloned());
        }
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
        let mut layout = Layout::of(self.virt(), icon_scale, self.list, self.zoom, self.field_left());
        if self.pill_on {
            layout.foot = PILL_ROOM / layout.zoom;
        }
        layout
    }

    /// The window's size (the field has a zoom of its own: `Layout::zoom`).
    fn virt(&self) -> (f32, f32) {
        (self.size.0 as f32, self.size.1 as f32)
    }

    /// A place's icon, at the panel's zoom.
    fn place_icon(&self) -> f32 {
        (PLACE_ICON * self.panel).round()
    }

    /// The places at rest: their icon and the same air on either side.
    fn rail(&self) -> f32 {
        self.place_icon() + 2.0 * RAIL_AIR
    }

    /// The places open: the rail and their words.
    fn side_open_w(&self) -> f32 {
        self.rail() + SIDE_WORDS
    }

    /// The icon a place wears: its folder — or, once the places' icons are
    /// made small enough that a folder could not carry its symbol, the
    /// SYMBOL alone (`<name>-symbolic`; Max, 2026-10-14: "the sidebar dirs
    /// become the symbols if I make them too small"). A place with no
    /// symbol of its own keeps its icon.
    fn place_key(&self, place: &Place) -> String {
        let icon = place.icon.as_str();
        // (The chrome's own symbols — Recents' clock — are symbols already.)
        if icon.ends_with(".svg") {
            return icon.to_owned();
        }
        if self.place_icon() > SYMBOL_FROM {
            return icon.to_owned();
        }
        // (`..` is an arrow up then: Max, 2026-10-14.)
        let symbol = if place.name == UP { "asset:go-up-symbolic".to_owned() } else { format!("{icon}-symbolic") };
        if self.missing.contains(&symbol) {
            icon.to_owned()
        } else {
            symbol
        }
    }

    /// The way here as a path to type over (Recents, a place of no folder:
    /// Home).
    fn typed_here(&self) -> String {
        let dir = if self.dir == recent::root() { home() } else { self.dir.clone() };
        dir.to_string_lossy().into_owned()
    }

    /// Whether the window is wide enough for a sidebar.
    fn has_side(&self) -> bool {
        self.virt().0 >= SIDE_FROM
    }

    /// Where the files begin: beside the sidebar at rest, or pinned open.
    fn field_left(&self) -> f32 {
        match (self.has_side(), self.pinned) {
            (false, _) => 0.0,
            (true, false) => self.rail(),
            (true, true) => self.side_open_w(),
        }
    }

    /// How wide the sidebar is drawn now (open over the files, or not).
    fn side_now(&self) -> f32 {
        if !self.has_side() {
            return 0.0;
        }
        if self.pinned {
            return self.side_open_w();
        }
        let t = self.side_t;
        let ease = t * t * (3.0 - 2.0 * t);
        self.rail() + (self.side_open_w() - self.rail()) * ease
    }

    /// How far the places can be scrolled: what they need past what the
    /// window shows of them.
    fn side_max(&self) -> f32 {
        (chrome::PLACES_TOP + self.places_tall() + 10.0 - self.virt().1).max(0.0)
    }

    /// How tall the places are (their sections' headings counted in).
    fn places_tall(&self) -> f32 {
        let heads = self.places.iter().filter(|p| p.gap).count() + 1;
        self.places.len() as f32 * self.place_row() + heads as f32 * chrome::SEC_H
    }

    /// A place's row: tall enough for its icon.
    fn place_row(&self) -> f32 {
        ROW_H.max(self.place_icon() + 12.0)
    }

    /// Where the pointer is, for what it is over: brought into the laid-out
    /// window, then into the field's own space over the field
    /// (`Layout::inward`) — except while a menu or a box is up, which stand
    /// in the window's, and over the chrome.
    fn point(&self, raw: (f32, f32), icon_scale: f32) -> (f32, f32) {
        let at = raw;
        if self.menu.is_some() || self.props.is_some() || self.on_chrome(at) {
            return at;
        }
        self.layout(icon_scale).inward(at)
    }

    /// Whether things can be made or put in what this window shows (not in
    /// Recents, the bin, or the media view's library).
    fn put_here(&self) -> bool {
        self.dir != recent::root() && self.in_bin() == InBin::No && !self.media
    }

    /// Whether a point of the window is on the sidebar as it is drawn now.
    fn in_side(&self, at: (f32, f32)) -> bool {
        self.has_side() && at.0 < self.side_now()
    }

    /// Whether a point of the window is on the chrome: the sidebar, the top
    /// line, the i card, the history.
    fn on_chrome(&self, at: (f32, f32)) -> bool {
        self.in_side(at)
            || at.1 < HEAD_H
            || (self.info_on && self.hits.card.contains(at))
            || (self.hist_open && self.hits.hist.contains(at))
            || self.hits.pill.contains(at)
    }

    /// Whether a point the pointer gave is over the field.
    fn on_field(&self, raw: (f32, f32), icon_scale: f32) -> bool {
        !self.on_chrome(raw) && self.layout(icon_scale).shown_body().contains(raw)
    }

    /// Open the sidebar while the pointer is on it — after a breath
    /// (`side_wait`, the caller's timer); leaving closes it at once. Whether
    /// that changed what is drawn.
    fn side_follow(&mut self) -> bool {
        let reach = if self.side_open { self.side_open_w() } else { self.rail() };
        let want = self.has_side() && !self.pinned && self.at_win.is_some_and(|p| p.0 < reach);
        if !want {
            self.side_wait = None;
            return std::mem::replace(&mut self.side_open, false);
        }
        if !self.side_open && self.side_wait.is_none() {
            self.side_wait = Some(Instant::now());
        }
        false
    }

    /// What Ctrl +/−/0 zoom now: the field, or the panel under the pointer.
    fn zoom_target(&self) -> &'static str {
        // (The places' icons while the pointer is on them; else the field.)
        if self.at_win.is_some_and(|p| self.in_side(p)) {
            "files-panel"
        } else {
            "files"
        }
    }

    fn layer_of(&self, key: &str) -> Option<u32> {
        self.uploaded
            .iter()
            .position(|u| u.as_deref() == Some(key))
            .map(|l| l as u32)
    }

    fn spot(&self, pos: (f32, f32), layout: &Layout) -> Spot {
        let h = &self.hits;
        if h.up.contains(pos) {
            return Spot::Up;
        }
        // (The selection pill floats over the files.)
        if h.pill.contains(pos) {
            return h
                .pill_btns
                .iter()
                .position(|(r, _)| r.contains(pos))
                .map_or(Spot::Chrome, Spot::Pill);
        }
        // (The history stands over everything but the menus.)
        if self.hist_open && h.hist.contains(pos) {
            let at = |list: &[(Rect, usize)]| list.iter().find(|(r, _)| r.contains(pos)).map(|(_, k)| *k);
            return at(&h.hist_undo)
                .map(Spot::HistUndo)
                .or_else(|| at(&h.hist_rows).map(Spot::HistRow))
                .unwrap_or(Spot::Chrome);
        }
        // (The card stands over the files: what is under it is not pointed at.)
        if self.info_on && h.card.contains(pos) {
            return Spot::Chrome;
        }
        if self.in_side(pos) {
            if h.pin.contains(pos) {
                return Spot::Pin;
            }
            if let Some(n) = h.mode.iter().position(|r| r.w > 0.0 && r.contains(pos)) {
                return Spot::Mode(n);
            }
            if let Some(i) = h.places.iter().position(|r| r.contains(pos)) {
                return Spot::Place(i);
            }
            return Spot::Chrome;
        }
        if pos.1 < HEAD_H {
            let at = |list: &[(Rect, usize)]| list.iter().find(|(r, _)| r.contains(pos)).map(|(_, n)| *n);
            if let Some(n) = at(&h.tab_x) {
                return Spot::TabX(n);
            }
            if let Some(n) = at(&h.tabs) {
                return Spot::Tab(n);
            }
            for (r, spot) in [
                (h.tab_add, Spot::TabAdd),
                (h.pencil, Spot::Pencil),
                (h.undo, Spot::Undo),
                (h.here.map_or(Rect::default(), |(r, _)| r), Spot::Here),
                (h.sort, Spot::Sort),
                (h.info, Spot::Info),
                (h.grid, Spot::View(false)),
                (h.list, Spot::View(true)),
                (h.find, Spot::Find),
            ] {
                if r.contains(pos) {
                    return spot;
                }
            }
            if let Some(i) = h.crumbs.iter().position(|(r, _)| r.contains(pos)) {
                return Spot::Crumb(i);
            }
            return Spot::Chrome;
        }
        if let Some(i) = h.cols.iter().position(|r| r.contains(pos)) {
            return Spot::Col(i);
        }
        layout
            .index_at(pos, self.scroll, self.items.len())
            .map_or(Spot::Nothing, Spot::Item)
    }

    /// Which part of thing `i` is at `at` (a point of the field): its
    /// picture or its name — in a list, the icon or the name's words.
    fn part_at(&mut self, i: usize, at: (f32, f32), layout: &Layout, icon_scale: f32) -> Part {
        let mut cell = layout.cell(i);
        cell.y -= self.scroll;
        let icon = layout.icon_in(&cell, icon_scale);
        if !layout.list {
            return if at.1 <= icon.y + icon.h + 2.0 { Part::Picture } else { Part::Name };
        }
        let name_x = cell.x + LIST_NAME_X;
        if at.0 < name_x - 4.0 {
            return Part::Picture;
        }
        let name = self.items.get(i).map(|it| it.name.clone()).unwrap_or_default();
        let w = self.renderer.as_mut().map_or(0.0, |r| r.measure_text(&name, TEXT_PX, None));
        if at.0 <= name_x + w.min(cell.w - LIST_NAME_X) + 6.0 {
            Part::Name
        } else {
            Part::Neither
        }
    }

    fn select_only(&mut self, i: usize) {
        self.selected.clear();
        if let Some(item) = self.items.get(i) {
            self.selected.insert(item.path.clone());
            self.cursor = Some(i);
            self.anchor = Some(i);
        }
    }

    /// Shift: everything from where the selection began to `i` (added to
    /// what is selected when `keep`); the keys go to `i`.
    fn select_to(&mut self, i: usize, keep: bool) {
        if i >= self.items.len() {
            return;
        }
        let anchor = self.anchor.or(self.cursor).unwrap_or(i).min(self.items.len() - 1);
        if !keep {
            self.selected.clear();
        }
        for it in &self.items[span(anchor, i)] {
            self.selected.insert(it.path.clone());
        }
        self.anchor = Some(anchor);
        self.cursor = Some(i);
    }
}

/// Every file window, and what they share.
pub(crate) struct Filer {
    xdg: Option<XdgShell>,
    /// `xdg-toplevel-tag`: each window is tagged `golem-files-<id>`, which
    /// the compositor (and the waveview bar) sees as the window's own.
    tags: Option<XdgToplevelTagManagerV1>,
    fractional: Option<crate::fractional::Fractional>,
    wins: Vec<Win>,
    next: u32,
    /// The window the keyboard is on.
    pub keys: Option<u32>,
    icons: Option<crate::notif_icons::NotifIcons>,
    /// THE SYMBOLS (`glyphs.rs`) every window may wear — a menu's, the
    /// pill's — read ONCE for all of them as the first window opens, so a
    /// menu has every symbol the moment it is up (asked for when first worn,
    /// they came a frame or many too late).
    glyphs: HashMap<String, Vec<u8>>,
    glyphs_asked: bool,
    /// What can be printed, as last asked (`print.rs`).
    printers: print::Printers,
    /// Files in hand, lifted from one of the windows.
    drag: Option<Lifted>,
    /// A drag over one of the windows.
    dnd: Option<Hovering>,
    /// The rig's `files over`: a drag that is no Wayland one (the springs
    /// take it for real).
    rig_drag: bool,
    orders: Orders,
    /// The files on the clipboard the pill was told to forget (its ×):
    /// neither offered here nor drawn faded while they are what is there.
    pub clip_forgot: Option<Vec<String>>,
    /// A drag of ours came over the DOCK while still in hand (set at its
    /// enter there, taken at the drop): the bin may take it.
    pub over_dock: bool,
    /// The video or GIF under the pointer, playing in its tile (`filer_live`).
    pub(crate) live: crate::filer_live::Live,
    /// The screen each window is on (its surface's enter): Quick Look comes
    /// out there.
    pub(crate) screens: HashMap<u32, wayland_client::protocol::wl_output::WlOutput>,
}

impl Filer {
    pub fn new(
        xdg: Option<XdgShell>,
        fractional: Option<crate::fractional::Fractional>,
        tags: Option<XdgToplevelTagManagerV1>,
    ) -> Self {
        Self {
            xdg,
            fractional,
            tags,
            wins: Vec::new(),
            next: 1,
            keys: None,
            icons: None,
            glyphs: HashMap::new(),
            glyphs_asked: false,
            printers: Default::default(),
            drag: None,
            dnd: None,
            rig_drag: false,
            orders: Orders::default(),
            clip_forgot: None,
            live: Default::default(),
            over_dock: false,
            screens: HashMap::new(),
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

    /// A file window is in the bin (the dock's bin stands open meanwhile).
    pub(crate) fn showing_bin(&self) -> bool {
        self.wins.iter().any(|w| w.in_bin() != InBin::No)
    }

    /// Files of a window are in hand.
    pub fn dragging(&self) -> bool {
        self.drag.as_ref().is_some_and(|d| !d.dropped)
    }

    /// The files in hand.
    pub fn drag_paths(&self) -> Vec<String> {
        self.drag.as_ref().map_or_else(Vec::new, |d| d.paths.clone())
    }

    /// The window a drag of ours came from.
    pub fn drag_win(&self) -> Option<u32> {
        self.drag.as_ref().map(|d| d.win)
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
pub(crate) fn user_dir(text: &str, home: &Path, key: &str) -> Option<PathBuf> {
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

pub(crate) fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
}

/// The sidebar's places in its two sections (Max, 2026-10-21, "A ·
/// Unified"): FAVOURITES — Recents (`recent.rs`), Home, under the owner's name as the way here
/// starts with it, the owner's folders from `user-dirs.dirs`, `~/Phones`,
/// then the owner's own (GTK's bookmarks) — and LOCATIONS: the owner's other
/// disks, what is plugged in, the bin. `gap` marks the first of Locations
/// (its heading stands over it).
pub(crate) fn places(
    home: &Path,
    user_dirs: &str,
    bin: &Path,
    marks: &[(String, PathBuf)],
    volumes: &[crate::mounts::Mounted],
) -> Vec<Place> {
    let mut out = vec![
        Place {
            name: tr("Recents").to_owned(),
            path: recent::root(),
            icon: chrome::glyph("clock"),
            gap: false,
            start: true,
        },
        Place {
            name: file_name(home),
            path: home.to_path_buf(),
            icon: "asset:user-home".to_owned(),
            gap: false,
            start: true,
        },
    ];
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
            icon: "asset:folder-phone".to_owned(),
            gap: false,
            start: false,
        });
    }
    // The owner's own, where they still are (and are not up there already).
    for (name, path) in marks {
        if path.is_dir() && !out.iter().any(|p| p.path == *path) {
            out.push(Place {
                name: name.clone(),
                path: path.clone(),
                icon: "asset:folder".to_owned(),
                gap: false,
                start: false,
            });
        }
    }
    // LOCATIONS.
    let first = out.len();
    for disk in crate::mounts::own_disks() {
        out.push(Place {
            name: file_name(&disk),
            path: disk,
            icon: "asset:drive-harddisk".to_owned(),
            gap: false,
            start: true,
        });
    }
    for v in volumes.iter().filter(|v| !v.closed) {
        out.push(Place {
            name: v.name.clone(),
            path: v.path.clone(),
            icon: format!("asset:{}", crate::mounts::volume_icon(v)),
            gap: false,
            start: true,
        });
    }
    out.push(Place {
        name: tr("Bin").to_owned(),
        path: bin.to_path_buf(),
        icon: "asset:user-trash".to_owned(),
        gap: false,
        start: true,
    });
    out[first].gap = true;
    out
}

/// THE FLOOR (Max, 2026-10-15: "Files should go from ~/ onwards… nobody
/// should go into /; a superuser goes from the terminal"): a file window
/// lives under a ROOT — Home, the bin, a stick, a phone, a disk of the
/// owner's — and goes no higher than the one it is under. A folder under
/// none is reached only when it is ASKED FOR FROM OUTSIDE (another app's
/// "show in folder", `xdg-open`): the window opened on it has it as its own
/// root (`Win::root`), and goes down from there only.
pub(crate) fn floor<'a>(dir: &Path, roots: &'a [PathBuf]) -> Option<&'a Path> {
    roots
        .iter()
        .filter(|r| dir.starts_with(r))
        .max_by_key(|r| r.components().count())
        .map(PathBuf::as_path)
}

/// `..` — the folder this one is in — as the first of the places (Max,
/// 2026-10-14): a click goes up, and it is where things are dragged OUT of
/// a folder to.
pub(crate) const UP: &str = "..";

/// `places` for a window on `dir`: `..` — the folder this one is in —
/// first, a place like the others (Max, 2026-10-14: "tiene que ser otra
/// carpeta arriba de Home"; brought back to the sidebar of 2026-10-21: "add
/// the '..' place too"). At the root there is nowhere up, and nothing is
/// above a root (`floor`): no `..` at all there (Max: hidden, not grey).
pub(crate) fn with_up(places: &[Place], dir: &Path, roots: Option<&[PathBuf]>) -> Vec<Place> {
    let Some(parent) = dir.parent() else {
        return places.to_vec();
    };
    if roots.is_some_and(|roots| floor(dir, roots).is_none_or(|f| f == dir)) {
        return places.to_vec();
    }
    let mut out = vec![Place {
        name: UP.to_owned(),
        path: parent.to_path_buf(),
        icon: "asset:folder-up".to_owned(),
        gap: false,
        start: false,
    }];
    out.extend(places.iter().cloned());
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
    // (Clear of the edge: see `desktop::FIT_SLACK`.)
    let room = max_w - crate::desktop::FIT_SLACK;
    if measure(name) <= room {
        return (name.to_owned(), String::new());
    }
    let chars: Vec<char> = name.chars().collect();
    // The longest head that fits: binary search over measured widths.
    let (mut lo, mut hi) = (1usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(&chars[..mid].iter().collect::<String>()) <= room {
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

/// The window's colours in the present look (Max, 2026-10-14: "do the dark
/// light mode of files"). DARK: the dock's own colour made solid, its ink,
/// light washes — or, with Dynamic colour off, THE PALETTE's surface and ink.
/// LIGHT: the palette's light greys (Golem's ARE the ones tuned here on
/// 2026-10-14: ground base01 #dcd9d1, side base02 #cecbc2, fields base00
/// #eae7e0, ink base05 #27251f), dark washes — the same weights, turned
/// over. Colours here are LINEAR light.
pub(crate) struct Palette {
    /// The dark look (the chrome's symbols follow it: `chrome::tones`).
    pub dark: bool,
    pub accent: [f32; 4],
    pub ground: [f32; 4],
    pub side: [f32; 4],
    pub ink: [f32; 4],
    pub band_fill: [f32; 4],
    pub band_line: [f32; 4],
}

fn palette(dark: bool, dock: [f32; 4], dock_ink: [f32; 4], theme: &crate::palette::Theme) -> Palette {
    let lin = crate::palette::linear;
    if dark {
        let (dock, dock_ink) = if theme.dynamic { (dock, dock_ink) } else { (lin(theme.surface()), lin(theme.ink())) };
        return Palette {
            dark: true,
            accent: lin(theme.accent()),
            ground: lift(dock, 0.0),
            side: lift(dock, 0.012),
            ink: dock_ink,
            band_fill: [1.0, 1.0, 1.0, 0.07],
            band_line: [1.0, 1.0, 1.0, 0.38],
        };
    }
    Palette {
        dark: false,
        accent: lin(theme.accent()),
        // A BRIGHT GREY, a touch warm — not white, and not blue (Max,
        // 2026-10-14: "too white… reduce a little the blue light", then
        // "make it warmer", "little darker and warmer", "too warm" — half of
        // that warmth back): Golem's light palette carries exactly those.
        ground: lin(theme.surface()),
        side: lin(theme.raised()),
        ink: lin(theme.ink()),
        band_fill: [0.0, 0.0, 0.0, 0.05],
        band_line: [0.0, 0.0, 0.0, 0.32],
    }
}

/// `c` darkened toward black by `k` (a CARVED symbol, a SUNKEN square: the
/// pill's own colour pressed in — Max, 2026-10-17, as the plugged-in
/// devices stood in the places).
/// `k` is the mockup's black at that alpha over the colour in sRGB; the
/// colours here are linear.
fn carve(c: [f32; 4], k: f32) -> [f32; 4] {
    let f = (1.0 - k).powf(2.2);
    [c[0] * f, c[1] * f, c[2] * f, 1.0]
}

/// The field drawn `zoom` times bigger from `origin`, its top-left corner
/// (the card's `view::zoom_scene`, for one grid).
fn zoom_grid(grid: &mut GridContent, origin: (f32, f32), zoom: f32) {
    if (zoom - 1.0).abs() < 0.001 {
        return;
    }
    let r = |rect: &mut Rect| {
        rect.x = origin.0 + (rect.x - origin.0) * zoom;
        rect.y = origin.1 + (rect.y - origin.1) * zoom;
        rect.w *= zoom;
        rect.h *= zoom;
    };
    r(&mut grid.clip);
    for rect in &mut grid.rects {
        r(&mut rect.rect);
        rect.radius *= zoom;
        rect.border *= zoom;
    }
    for icon in &mut grid.icons {
        r(&mut icon.rect);
    }
    for label in &mut grid.labels {
        label.pos.0 = origin.0 + (label.pos.0 - origin.0) * zoom;
        label.pos.1 = origin.1 + (label.pos.1 - origin.1) * zoom;
        label.max_w *= zoom;
        label.font_px *= zoom;
        label.line_px *= zoom;
        if let Some(clip) = label.clip.as_mut() {
            r(clip);
        }
    }
}

/// A colour brought `k` of the way to white, solid: the window's ground is
/// the dock's own colour without its glass. (Colours here are linear light:
/// a little goes a long way.)
fn lift(c: [f32; 4], k: f32) -> [f32; 4] {
    [c[0] + (1.0 - c[0]) * k, c[1] + (1.0 - c[1]) * k, c[2] + (1.0 - c[2]) * k, 1.0]
}

/// Whether a file of this name is a picture or a video (what MEDIA shows).
fn is_media(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tif", ".tiff", ".heic", ".heif", ".avif", ".jxl", ".svg",
        ".raw", ".cr2", ".nef", ".arw", ".dng", ".mp4", ".m4v", ".mkv", ".webm", ".mov", ".avi", ".ogv", ".3gp",
    ]
    .iter()
    .any(|e| lower.ends_with(e))
        || crate::thumb_more::mime_of(&lower).is_some_and(|m| m.starts_with("image/") || m.starts_with("video/"))
}

/// Where a typed name's field begins (`w` its words' width): centred under
/// its cell, but kept inside the field.
fn name_field_x(layout: &Layout, cx: f32, w: f32) -> f32 {
    let (lo, hi) = (layout.body.x + 2.0, layout.body.x + layout.body.w - 2.0);
    (cx - w / 2.0 - NAME_PAD.0).min(hi - w - 2.0 * NAME_PAD.0).max(lo)
}

/// A name being typed, measured: its words, the selected part (from its
/// start), the caret.
struct Typed {
    text: String,
    picked: usize,
    tw: f32,
    mark_w: f32,
    caret_x: f32,
}

impl Typed {
    fn of(r: &Rename, measure: &mut dyn FnMut(&str) -> f32) -> Self {
        Self {
            tw: measure(&r.text),
            mark_w: measure(&r.text[..r.picked]),
            caret_x: measure(&r.text[..r.caret]),
            text: r.text.clone(),
            picked: r.picked,
        }
    }

    /// The field's words' width.
    fn width(&self) -> f32 {
        self.tw.max(24.0)
    }

    /// Drawn with its words beginning at `x` (`top` their line's top): the
    /// field — the page's colour, a hairline round it, radius 4 — the part
    /// selected in the ACCENT with white words on it (the accent shows
    /// nowhere else: Max, 2026-10-21), else the caret.
    fn draw(
        &self,
        g: &mut GridContent,
        x: f32,
        top: f32,
        (px, line): (f32, f32),
        (ground, hair, accent, ink): ([f32; 4], [f32; 4], [f32; 4], [f32; 4]),
    ) {
        let field = Rect::new(x - NAME_PAD.0, top - NAME_PAD.1, self.width() + 2.0 * NAME_PAD.0, line + 2.0 * NAME_PAD.1);
        g.rects.push(rect(field, 4.0, ground));
        g.rects.push(RectInst {
            border: 1.0,
            ..rect(field, 4.0, hair)
        });
        if self.mark_w > 0.0 {
            g.rects.push(rect(Rect::new(x, top, self.mark_w, line), 2.0, accent));
        } else {
            g.rects.push(rect(Rect::new(x + self.caret_x, top + 2.0, 1.0, line - 4.0), 0.0, ink));
        }
        let (head, rest) = self.text.split_at(self.picked);
        for (words, at, color) in [(head, x, [1.0, 1.0, 1.0, 1.0]), (rest, x + self.mark_w, ink)] {
            if !words.is_empty() {
                g.labels.push(Label {
                    font_px: px,
                    line_px: line,
                    cache: false,
                    color: Some(color),
                    ..label(words, (at, top), self.tw + 8.0, false, false)
                });
            }
        }
    }
}

/// How much of a picture's square is empty above and under what it shows
/// (shares of its height): rows of its largest image with nothing more
/// than a faint shadow in them.
pub(crate) fn ink_rows(chain: &[u8]) -> (f32, f32) {
    let n = crate::apps::ICON_SIZE as usize;
    if chain.len() < n * n * 4 {
        return (0.0, 0.0);
    }
    let inked = |y: usize| chain[y * n * 4..(y + 1) * n * 4].chunks_exact(4).any(|px| px[3] > 40);
    let (Some(first), Some(last)) = ((0..n).find(|&y| inked(y)), (0..n).rev().find(|&y| inked(y))) else {
        return (0.0, 0.0);
    };
    (first as f32 / n as f32, (n - 1 - last) as f32 / n as f32)
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
        // (The chrome's symbols, written where the icon loader reads them, and
        // read once for every window.)
        crate::glyphs::write_all();
        // Asked for from outside, under none of the roots (another app's
        // "show in folder"): this window's own root, down from there only.
        let root = floor(&dir, &self.filer_roots(None)).is_none().then(|| dir.clone());
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
        // Its tag: how the bar tells this window from the others of its class
        // (its back and forward arrows — `filer_note_nav`).
        if let Some(tags) = self.filer.tags.as_ref() {
            tags.set_toplevel_tag(window.xdg_toplevel(), format!("{APP_ID}-{id}"));
        }
        window.set_title(window_title(&dir));
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
        self.filer_glyphs_warm();
        // (Is there a printer now: asked again as a window opens.)
        self.filer_printers(true);
        info!("files: window {id} on {}", dir.display());
        self.filer.put(Win {
            renderer: None,
            fscale,
            window,
            id,
            size: (0, 0),
            zoom: self.files_zoom(),
            panel: self.files_panel_zoom(),
            over_field: false,
            at_win: None,
            side_open: false,
            side_t: 0.0,
            side_scroll: 0.0,
            side_scroll_to: 0.0,
            side_wait: None,
            pinned: self.settings.files_pinned,
            media: false,
            tabs: vec![chrome::Tab::new(0, dir.clone())],
            tab: 0,
            tab_seq: 1,
            tab_w: HashMap::new(),
            edit_w: 0.0,
            find_t: 0.0,
            info_on: false,
            info: None,
            info_asked: None,
            list: self.settings.files_list,
            sort: (SortKey::Name, false),
            meta: HashMap::new(),
            go_to: None,
            go_bad: false,
            find: None,
            all: Vec::new(),
            deep: Vec::new(),
            deep_from: usize::MAX,
            deep_gen: 0,
            deep_busy: false,
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
            anchor: None,
            spring: None,
            place_spring: None,
            place_blink: None,
            history: Vec::new(),
            undo_t: 0.0,
            hist_open: false,
            hist_first: 0.0,
            hist_read: None,
            here_kind: None,
            pill_t: 0.0,
            pill_view: None,
            pill_on: false,
            flash: None,
            sprung_from: None,
            up_hold: None,
            reveal: None,
            scroll: 0.0,
            scroll_to: 0.0,
            places: Vec::new(),
            root,
            chains: HashMap::new(),
            ink: HashMap::new(),
            uploaded: Vec::new(),
            capacity: 0,
            pending: HashSet::new(),
            missing: HashSet::new(),
            ptr: None,
            hover: Spot::Nothing,
            finger: false,
            scroll_vel: 0.0,
            scroll_last: None,
            scroll_run: 0.0,
            gliding: false,
            click: None,
            hits: Hits::default(),
            watch: None,
            reload_pending: false,
            menu: None,
            menu_path: None,
            props: None,
            rename: None,
            rename_next: None,
            rename_due: None,
            rename_seq: 0,
            opened: None,
            armed: None,
            open_with: Vec::new(),
            asking_apps: false,
            marks: Vec::new(),
            menu_place: None,
            again: false,
            drop_place: None,
            edge: 0.0,
            edge_at: (0.0, 0.0),
            edge_timer: false,
            zoom_run: 0.0,
            slide: Vec::new(),
            gap: None,
            gap_keep: false,
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
        self.peek.forget_window(id);
        if self.peek.up.as_ref().is_some_and(|u| u.win == id) {
            self.peek_gone();
        }
        self.filer_live_closed(id);
        self.filer.screens.remove(&id);
        info!("files: window {id} closed");
        self.filer_note_session();
        self.filer_note_nav();
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
        let marks = parse_marks(&std::fs::read_to_string(marks_path()).unwrap_or_default());
        let list = places(&home, &user_dirs, &bin_root(), &marks, &self.desktop.volumes);
        let marked: Vec<PathBuf> = marks.into_iter().map(|(_, p)| p).collect();
        let base = self.filer_roots(None);
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            if let Some(win) = self.filer.win_mut(id) {
                win.marks = marked.clone();
                // (Each window's own: `..` is the folder ITS folder is in.)
                let mut roots = base.clone();
                roots.extend(win.root.clone());
                let list = with_up(&list, &win.dir, Some(&roots));
                if win.places != list {
                    win.places = list;
                    // (A drag over them held a place by its number.)
                    win.drop_place = None;
                    self.filer_request_draw(id);
                }
            }
        }
    }

    /// The roots a file window lives under (`floor`): Home, the bin, and
    /// what is plugged in and open.
    /// (A window's own root too, when `id` names one opened from outside.)
    pub(crate) fn filer_roots(&self, id: Option<u32>) -> Vec<PathBuf> {
        let mut roots = vec![home(), bin_root(), recent::root()];
        roots.extend(self.desktop.volumes.iter().filter(|v| !v.closed).map(|v| v.path.clone()));
        roots.extend(crate::mounts::own_disks());
        roots.extend(id.and_then(|id| self.filer.win(id)).and_then(|w| w.root.clone()));
        roots
    }

    /// Whether window `id` may show `dir`: under one of its roots.
    pub(crate) fn filer_may_show(&self, id: u32, dir: &Path) -> bool {
        floor(dir, &self.filer_roots(Some(id))).is_some()
    }

    /// Go to `dir`, the folder shown now becoming the way back.
    fn filer_go(&mut self, id: u32, dir: PathBuf) {
        let dir = tidy(dir);
        // (Above the floor: nowhere to go, and nothing for the way back.)
        if !self.filer_may_show(id, &dir) {
            info!("files: {} is the system's: staying", dir.display());
            return;
        }
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
        let roots = self.filer_roots(Some(id));
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(parent) = win.dir.parent().map(Path::to_path_buf) else {
            return;
        };
        // (No higher than its root.)
        if floor(&win.dir, &roots).is_none_or(|f| f == win.dir) {
            return;
        }
        win.reveal = Some(win.dir.to_string_lossy().into_owned());
        self.filer_go(id, parent);
    }

    /// Show `dir` in window `id`: empty at once, listed off the loop,
    /// watched from now on. History is the caller's.
    fn filer_show(&mut self, id: u32, dir: PathBuf) {
        // Above the floor (an old step back to a stick since ejected):
        // Home instead.
        let dir = if self.filer_may_show(id, &dir) {
            dir
        } else {
            info!("files: {} is the system's: Home instead", dir.display());
            home()
        };
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
        win.go_to = None;
        win.find = None;
        win.all.clear();
        win.deep.clear();
        win.deep_from = usize::MAX;
        win.deep_gen += 1;
        win.deep_busy = false;
        win.selected.clear();
        win.cursor = None;
        win.anchor = None;
        win.spring = None;
        win.click = None;
        win.scroll = 0.0;
        win.scroll_to = 0.0;
        win.failed = false;
        win.window.set_title(window_title(&dir));
        if let Some(token) = win.watch.take() {
            self.loop_handle.remove(token);
        }
        // (Recents is no folder: it is read again when Files opens a file.)
        let watched = (dir != recent::root()).then(|| crate::apps::watch_dirs(std::slice::from_ref(&dir))).flatten();
        if let Some(fd) = watched {
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
        self.filer_note_session();
        self.filer_note_nav();
        // (`..` is another folder now.)
        self.filer_places();
    }

    /// BACK AND FORWARD ON THE WINDOW'S BAR (Max, 2026-10-14: "los botones
    /// < > en la barra de título, como las webapps"). The waveview bar draws
    /// them, and only while there is somewhere to go, from what each window
    /// says here — a line a window, `<its tag> <can go back 0|1> <can go
    /// forward 0|1>` (the webapps' own format, `seam/nav`) — and a click on
    /// one sends the window Alt+← / Alt+→, which are its back and forward.
    fn filer_note_nav(&self) {
        let text: String = self
            .filer
            .wins
            .iter()
            .map(|w| format!("{APP_ID}-{} {} {}\n", w.id, u8::from(!w.back.is_empty()), u8::from(!w.forward.is_empty())))
            .collect();
        crate::persist::write_text("files-nav", &nav_path(), &text);
    }

    /// Note which folders stand open (see `session_path`).
    fn filer_note_session(&self) {
        let mut wins: Vec<(u32, String)> = self
            .filer
            .wins
            .iter()
            .map(|w| (w.id, w.dir.to_string_lossy().into_owned()))
            .collect();
        wins.sort();
        let dirs: Vec<String> = wins.into_iter().map(|(_, d)| d).collect();
        crate::persist::write_json("files-session", &session_path(), &dirs);
    }

    /// Start answering "show in folder" (`serve_file_manager`): what is
    /// asked comes to the loop and opens windows.
    pub(crate) fn filer_serve_dbus(&mut self) {
        let (tx, rx) = calloop::channel::channel::<Shown>();
        let heard = self.loop_handle.insert_source(rx, |event, _, app: &mut App| {
            if let calloop::channel::Event::Msg(asked) = event {
                info!("files: asked over D-Bus: {asked:?}");
                for (dir, item) in shown_places(&asked).into_iter().take(8) {
                    if !dir.is_dir() {
                        continue;
                    }
                    app.filer_open(dir);
                    // (Its folder is being listed: it comes up selected.)
                    if let (Some(item), Some(id)) = (item, app.filer.newest()) {
                        if let Some(win) = app.filer.win_mut(id) {
                            win.reveal = Some(item.to_string_lossy().into_owned());
                        }
                    }
                }
            }
        });
        if heard.is_ok() {
            serve_file_manager(tx);
        }
    }

    /// The dock was started again within one login: the windows that stood
    /// open come back on their folders. (Where each stood on the screen is
    /// the compositor's to give; a new login starts with none.)
    pub(crate) fn filer_restore_session(&mut self) {
        let dirs: Vec<String> = crate::persist::read_json(&session_path()).unwrap_or_default();
        for dir in dirs.into_iter().take(12) {
            let dir = PathBuf::from(dir);
            if dir.is_dir() {
                info!("files: back on {}", dir.display());
                self.filer_open(dir);
            }
        }
    }

    /// The folder changed on disk: read it again in a moment, once for a
    /// burst.
    fn filer_changed(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        // (While it is being read, changes only ask for one more reading
        // after — else every answer is dropped for the next and a folder
        // that keeps changing never shows.)
        if win.loading {
            win.again = true;
            return;
        }
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
                let items = if dir == bin_root() {
                    list_bin()
                } else if dir == recent::root() {
                    recent::list()
                } else {
                    list(&dir, hidden)?
                };
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
        // (The folder changed: what can still be taken back is read again.)
        self.filer_history_check(id);
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
        let again = std::mem::take(&mut win.again);
        let cursor_path = win.cursor.and_then(|i| win.items.get(i)).map(|it| it.path.clone());
        let anchor_path = win.anchor.and_then(|i| win.items.get(i)).map(|it| it.path.clone());
        // (What the gap was kept open for has arrived: it stands in it.)
        if win.gap_keep && listed.as_ref().is_ok_and(|items| items.len() != win.all.len()) {
            win.gap = None;
            win.gap_keep = false;
        }
        match listed {
            Ok(items) => {
                win.all = items;
                win.show_found();
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
        win.anchor = anchor_path.and_then(|p| win.items.iter().position(|it| it.path == p));
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
        // It changed again while this reading was on its way.
        if again {
            self.filer_changed(id);
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

    /// The one stick or memory card plugged in, by its label and folder —
    /// "Copy to <it>" (not while this window is on it; not with two in).
    pub(crate) fn one_stick(&self, inside: Option<&Path>) -> Option<(String, PathBuf)> {
        let mut sticks = self.desktop.volumes.iter().filter(|v| !v.phone && !v.closed);
        let (Some(v), None) = (sticks.next(), sticks.next()) else {
            return None;
        };
        if inside.is_some_and(|d| d.starts_with(&v.path)) {
            return None;
        }
        Some((v.name.clone(), v.path.clone()))
    }

    /// What a menu on a plugged-in volume is made from.
    pub(crate) fn volume_ctx(&self, v: &crate::mounts::Mounted) -> crate::desktop_menu::Volume {
        let path = v.path.to_string_lossy();
        let camera = self.desktop.camera_on.as_ref().is_some_and(|c| c.path == path)
            || self.desktop.cameras.lock().is_ok_and(|c| c.contains_key(path.as_ref()));
        crate::desktop_menu::Volume {
            phone: v.phone,
            closed: v.closed,
            camera,
            tether: v.tether,
            limits: if v.phone { self.phone_limits(v) } else { Default::default() },
        }
    }

    /// A right click at `at`: the menu for the thing under it (which is
    /// selected if it was not — a group stays a group), or the folder's.
    fn filer_open_menu(&mut self, id: u32, at: (f32, f32), item: Option<usize>) {
        use crate::desktop_menu::{Ctx, On};
        let icon_scale = self.icon_scale();
        let clip = self.clipboard_files().is_some();
        let by_hand = self
            .filer
            .win(id)
            .map(|w| w.dir.clone())
            .is_some_and(|dir| self.filer.orders.of(&dir).is_some());
        let stick = self.filer.win(id).and_then(|w| self.one_stick(Some(&w.dir))).map(|(name, _)| name);
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
        let thing = item.and_then(|i| win.items.get(i));
        let here = win.put_here();
        let on = match (win.in_bin(), thing) {
            (InBin::Top, Some(_)) => On::BinItem,
            (InBin::Top, None) => On::BinGround,
            (InBin::Deeper, Some(_)) => On::BinDeep,
            (InBin::Deeper, None) => return,
            (InBin::No, None) => On::Ground,
            (InBin::No, Some(_)) if many => On::Many,
            (InBin::No, Some(it)) => match it.kind {
                Kind::Folder | Kind::Volume => On::Folder,
                Kind::Launcher => On::App,
                _ if is_archive(&it.name) => On::Archive,
                _ => On::File,
            },
        };
        let ctx = Ctx {
            on,
            n: win.selected.len(),
            stick,
            here,
            clip: clip && here,
            untrusted: thing.is_some_and(|it| crate::desktop::locked_shortcut(it).is_some()),
            placed: path.as_ref().is_some_and(|p| win.marks.iter().any(|m| m.as_os_str() == p.as_str())),
            system: win.selected.iter().any(|p| crate::trash::protected(Path::new(p))),
            hidden: win.hidden,
            by_hand,
            terminal: win.dir != recent::root() && win.in_bin() == InBin::No,
            bin_n: win.all.len(),
            ..Default::default()
        };
        win.menu_place = None;
        win.open_with.clear();
        win.asking_apps = false;
        win.armed = None;
        // (A point of the field: the menu stands where it is drawn.)
        let at = win.layout(icon_scale).outward(at);
        // What this kind of thing is for, right after Open (not in the bin).
        let mut ctx = ctx;
        if let Some(win) = self.filer.win(id).filter(|w| w.in_bin() == InBin::No) {
            let sel: Vec<&Item> = win.items.iter().filter(|it| win.selected.contains(&it.path)).collect();
            ctx.smart = self.filer_smart(win, &sel).menu;
        }
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.menu = Some(Menu::new(ctx, item.filter(|_| path.is_some()), many, at, w, h));
        win.menu_path = path;
        win.props = None;
        win.hist_open = false;
        self.filer_request_draw(id);
    }

    /// A right click on place `n` of the sidebar: a place's menu — a stick's
    /// or a phone's when it is one, the bin's when it is the bin.
    fn filer_place_menu(&mut self, id: u32, n: usize, at: (f32, f32)) {
        use crate::desktop_menu::{Ctx, On};
        let Some(place) = self.filer.win(id).and_then(|w| w.places.get(n)).filter(|p| p.name != UP).cloned() else {
            return;
        };
        let volume = self.desktop.volumes.iter().find(|v| v.path == place.path);
        let mut ctx = match volume {
            Some(v) => Ctx {
                on: if v.phone { On::Phone } else { On::Stick },
                volume: self.volume_ctx(v),
                ..Default::default()
            },
            None if place.path == bin_root() => Ctx {
                on: On::BinPlace,
                bin_n: crate::trash::Trash::home().list().len(),
                ..Default::default()
            },
            None => Ctx { on: On::Place, ..Default::default() },
        };
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        ctx.fixed = !win.marks.contains(&place.path);
        let (w, h) = win.virt();
        win.selected.clear();
        win.open_with.clear();
        win.asking_apps = false;
        win.armed = None;
        win.menu = Some(Menu::new(ctx, None, false, at, w, h));
        win.menu_path = None;
        win.menu_place = Some(place.path);
        win.props = None;
        win.hist_open = false;
        self.filer_request_draw(id);
    }

    /// Row `row` of the menu that is up was chosen: a page turns (the menu
    /// stays), what cannot be undone asks once more, anything else is done
    /// and the menu goes. What it says, for the rig.
    fn filer_menu_row(&mut self, id: u32, row: usize) -> String {
        use crate::desktop_menu::Page;
        let Some(win) = self.filer.win_mut(id) else {
            return "closed".to_owned();
        };
        let Some(menu) = win.menu.as_mut() else {
            return "no menu is up".to_owned();
        };
        let Some(action) = menu.action(row) else {
            return "no such row".to_owned();
        };
        // What cannot be undone is asked once: the row says so and the menu
        // stays for the second click.
        if matches!(action, Action::Erase | Action::EmptyBin) && win.armed != Some(action) {
            win.armed = Some(action);
            if let Some(Row::Item { label, .. }) = menu.rows.get_mut(row) {
                *label = tr("Click again to delete").to_owned();
            }
            self.filer_request_draw(id);
            return format!("{action:?} armed");
        }
        // Turning a page keeps the menu up.
        match action {
            Action::OpenWith => {
                self.filer_menu_apps(id);
                return "Open with".to_owned();
            }
            Action::SendPage => {
                self.send_targets_ask(crate::desktop::SendFor::Files(id));
                return "Send to".to_owned();
            }
            Action::SortPage => {
                let (key, desc) = win.sort;
                let key = [SortKey::Name, SortKey::Modified, SortKey::Size, SortKey::Kind]
                    .iter()
                    .position(|k| *k == key)
                    .unwrap_or(0);
                menu.turn(Page::Sort, crate::desktop_menu::sort_page(key as u8, desc));
                self.filer_hover(id);
                self.filer_request_draw(id);
                return "Sort by".to_owned();
            }
            Action::Back => {
                self.filer_menu_back(id);
                return "Back".to_owned();
            }
            Action::OtherApp => {
                self.filer_other_apps(id);
                return "Another app".to_owned();
            }
            Action::AlwaysUse => {
                menu.toggle_always();
                self.filer_request_draw(id);
                return "Always use".to_owned();
            }
            Action::OtherAppPick(n) => {
                let chosen = menu.other.as_ref().and_then(|o| Some((o.apps.get(n)?.clone(), o.always, o.mime.clone(), o.kind.clone())));
                let path = win.menu_path.take();
                win.menu = None;
                self.filer_request_draw(id);
                let (Some((other, always, mime, kind)), Some(path)) = (chosen, path) else {
                    return "nothing to open".to_owned();
                };
                let path = PathBuf::from(path);
                if self.open_with_other(&other, &path) {
                    self.filer_used(&path);
                }
                if let (true, Some(mime), Some(kind)) = (always, mime, kind) {
                    self.other_app_default(other, mime, kind, crate::desktop::SendFor::Files(id));
                }
                return "Another app".to_owned();
            }
            _ => {}
        }
        let Some(menu) = win.menu.take() else {
            return "no menu is up".to_owned();
        };
        win.menu_path = None;
        let place = win.menu_place.take();
        let (item, at) = (menu.item, menu.at);
        self.filer_request_draw(id);
        // GET INFO, on a click: a place's in the Properties box (grown out
        // of the menu, Back leading to it again), the rest in the i card.
        if action == Action::Properties {
            match place {
                Some(place) => {
                    let name = file_name(&place);
                    let thing = crate::desktop::item_for(place, name, true);
                    self.filer_open_props_of(id, &thing, at, Some(menu));
                }
                None => {
                    if let Some(win) = self.filer.win_mut(id) {
                        win.info_on = true;
                    }
                    self.filer_info_due(id);
                    self.filer_hover(id);
                }
            }
            return "Get info".to_owned();
        }
        self.filer_act_on(id, action, item, at, place);
        format!("{action:?}")
    }

    /// The right-click's menu without a pointer (Shift+F10, the Menu key,
    /// the rig): on thing `item` — at its middle, brought into view — or on
    /// the folder, at the middle of the field.
    fn filer_menu_from_keys(&mut self, id: u32, item: Option<usize>) {
        let icon_scale = self.icon_scale();
        if let (Some(i), Some(win)) = (item, self.filer.win_mut(id)) {
            let layout = win.layout(icon_scale);
            win.scroll_to = layout.reveal(i, win.scroll_to, win.items.len());
            win.scroll = win.scroll_to;
        }
        let Some(win) = self.filer.win(id) else {
            return;
        };
        let layout = win.layout(icon_scale);
        let at = match item {
            Some(i) => {
                let c = layout.cell(i);
                (c.x + c.w / 2.0, c.y + c.h / 2.0 - win.scroll)
            }
            None => (layout.body.x + layout.body.w / 2.0, layout.body.y + layout.body.h / 2.0),
        };
        self.filer_open_menu(id, at, item);
    }

    /// THE ANOTHER APP PAGE of the menu (`other_apps.rs`): every app that
    /// opens files, found off the loop ("Looking for apps…" meanwhile),
    /// and the file's kind for the check.
    fn filer_other_apps(&mut self, id: u32) {
        use crate::desktop_menu::{OtherApps, Page};
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(path) = win.menu_path.clone() else {
            return;
        };
        if let Some(menu) = win.menu.as_mut() {
            menu.show_other(OtherApps {
                looking: true,
                ..Default::default()
            });
        }
        self.filer_hover(id);
        self.filer_request_draw(id);
        let apps = self.other_app_entries();
        self.desktop_off_loop(
            move || {
                let mime = crate::other_apps::mime_of(Path::new(&path));
                let kind = mime.as_deref().and_then(crate::other_apps::kind_of);
                (crate::other_apps::openers(apps), mime, kind)
            },
            move |app, (apps, mime, kind)| {
                let Some(win) = app.filer.win_mut(id) else {
                    return;
                };
                if let Some(menu) = win.menu.as_mut().filter(|m| m.page == Page::OtherApp) {
                    if let Some(o) = menu.other.as_mut() {
                        o.mime = mime;
                    }
                    menu.other_found(apps, kind);
                }
                app.filer_hover(id);
                app.filer_request_draw(id);
            },
        );
    }

    /// The menu's "Open with": its page turns to the apps that say they
    /// open this kind of file — asked off the loop, "Looking for apps…"
    /// meanwhile — each by its name, the system's own choice first.
    fn filer_menu_apps(&mut self, id: u32) {
        use crate::desktop_menu::{open_with_rows, Page};
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(path) = win.menu_path.clone() else {
            return;
        };
        win.asking_apps = true;
        if let Some(menu) = win.menu.as_mut() {
            menu.turn(Page::OpenWith, open_with_rows(None));
        }
        self.filer_hover(id);
        self.filer_request_draw(id);
        let asked = path.clone();
        self.desktop_off_loop(
            move || apps_for(&asked),
            move |app, ids| {
                // Each id as the dock knows it: its name and its command.
                let apps: Vec<(String, String, bool)> = ids
                    .iter()
                    .filter_map(|id| app.entries.iter().find(|e| &e.id == id))
                    .map(|e| (e.name.clone(), e.exec.clone(), e.needs_terminal))
                    .collect();
                let Some(win) = app
                    .filer
                    .win_mut(id)
                    .filter(|w| w.asking_apps && w.menu.is_some() && w.menu_path.as_ref() == Some(&path))
                else {
                    return;
                };
                win.asking_apps = false;
                let names: Vec<String> = apps.iter().map(|(name, ..)| name.clone()).collect();
                win.open_with = apps;
                if let Some(menu) = win.menu.as_mut() {
                    menu.turn(Page::OpenWith, open_with_rows(Some(&names)));
                }
                app.filer_hover(id);
                app.filer_request_draw(id);
            },
        );
    }

    /// The Send to page's rows for window `id`'s menu, if its menu is up on
    /// its first page or that one. Whether they went in.
    pub(crate) fn filer_send_page(&mut self, id: u32, rows: &[Row]) -> bool {
        use crate::desktop_menu::Page;
        let Some(win) = self.filer.win_mut(id) else {
            return false;
        };
        let at = win.ptr;
        let Some(menu) = win.menu.as_mut().filter(|m| matches!(m.page, Page::Main | Page::Send)) else {
            return false;
        };
        menu.turn(Page::Send, rows.to_vec());
        menu.pointer(at);
        true
    }

    /// A button of the selection pill.
    fn filer_pill_act(&mut self, id: u32, act: chrome::PillAct) {
        use chrome::PillAct;
        match act {
            PillAct::Act(action) => self.filer_act(id, action, None, (0.0, 0.0)),
            PillAct::More => self.filer_pill_menu(id, false),
            PillAct::OpenWith => self.filer_pill_menu(id, true),
            PillAct::Clear => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.selected.clear();
                }
                self.filer_request_draw(id);
            }
            PillAct::Paste => self.filer_paste(id),
            PillAct::Forget => {
                self.filer.clip_forgot = self
                    .clipboard_files()
                    .map(|(paths, _)| paths.iter().map(|p| p.to_string_lossy().into_owned()).collect());
                let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
                for w in ids {
                    self.filer_request_draw(w);
                }
            }
        }
    }

    /// The pill's ⋯: the right-click's own menu for the selection, hanging
    /// UP from over the pill (`open_with`: turned to its Open with page).
    /// Again while it is up: it goes.
    fn filer_pill_menu(&mut self, id: u32, open_with: bool) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if win.menu.is_some() {
            win.menu = None;
            win.menu_path = None;
            return self.filer_request_draw(id);
        }
        let on = win
            .cursor
            .filter(|&i| win.items.get(i).is_some_and(|it| win.selected.contains(&it.path)))
            .or_else(|| win.items.iter().position(|it| win.selected.contains(&it.path)));
        let (pill, more) = (win.hits.pill, win.hits.pill_btns.iter().find(|(_, a)| *a == chrome::PillAct::More).map(|(r, _)| *r));
        self.filer_open_menu(id, (0.0, 0.0), on);
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let w = win.virt().0;
        if let Some(menu) = win.menu.as_mut() {
            // (Hanging UP from over the pill, its right edge by the ⋯; a new
            // page or size keeps its foot there.)
            let right = more.map_or(pill.x + pill.w, |r| r.x + r.w + 6.0);
            menu.hang((right.min(w - 4.0), pill.y - 10.0), false, false);
        }
        if open_with {
            self.filer_menu_apps(id);
        }
        self.filer_request_draw(id);
    }

    /// Say `said` in window `id`'s pill for a moment (a finished thing:
    /// "Copied 3 items to KINGSTON").
    pub(crate) fn filer_flash(&mut self, id: u32, said: String) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let at = Instant::now();
        win.flash = Some((said, at));
        self.filer_request_draw(id);
        let timer = calloop::timer::Timer::from_duration(chrome::FLASH + Duration::from_millis(40));
        let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            if let Some(win) = app.filer.win_mut(id).filter(|w| w.flash.as_ref().is_some_and(|(_, t)| *t == at)) {
                win.flash = None;
                app.filer_request_draw(id);
            }
            calloop::timer::TimeoutAction::Drop
        });
    }

    /// What a copy to `to` is said as: "Copied “x” to KINGSTON", "Copied 3
    /// items to Pictures".
    fn copied_words(made: &[PathBuf], to: &str) -> String {
        match made {
            [one] => fill(tr("Copied “{name}” to {to}"), &[("name", &file_name(one)), ("to", &to)]),
            _ => fill(tr("Copied {n} items to {to}"), &[("n", &made.len()), ("to", &to)]),
        }
    }

    /// Back to the menu's first page.
    fn filer_menu_back(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.asking_apps = false;
        if let Some(menu) = win.menu.as_mut() {
            menu.show_main();
        }
        self.filer_hover(id);
        self.filer_request_draw(id);
    }

    /// A row of the menu was chosen (or its key pressed): on the thing it
    /// was opened on, else on what is selected.
    fn filer_act(&mut self, id: u32, action: Action, item: Option<usize>, at: (f32, f32)) {
        self.filer_act_on(id, action, item, at, None);
    }

    /// The same for a menu on a PLACE of the sidebar (`place`: its folder).
    fn filer_act_on(&mut self, id: u32, action: Action, item: Option<usize>, at: (f32, f32), place: Option<PathBuf>) {
        // (The sort menu's rows.)
        if matches!(action, Action::SortBy(_) | Action::SortDir(_)) {
            return self.filer_sort_pick(id, action);
        }
        let selected = self.filer_selected(id);
        let Some(win) = self.filer.win(id) else {
            return;
        };
        // A menu's row is about the thing the menu was opened on: with what
        // is selected only while that thing is one of them. (A paste that
        // ended meanwhile selects what came — it must not be what is
        // thrown away.)
        let selected = match item.and_then(|i| win.items.get(i)) {
            Some(it) if !selected.contains(&it.path) => vec![it.path.clone()],
            _ => selected,
        };
        let dir = win.dir.clone();
        let bin = win.in_bin();
        // RECENTS is a place, not a folder: nothing is made or put in it.
        if dir == recent::root()
            && (matches!(action, Action::Paste | Action::NewFolder | Action::CleanUp | Action::Compress)
                || (action == Action::OpenTerminal && item.is_none()))
        {
            return;
        }
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
                // Only what lies IN the bin itself has a record there: a
                // thing inside a thrown-away folder shares its name with
                // nothing the bin knows — or, worse, with another entry.
                let root = bin_root();
                let names: Vec<String> = selected
                    .iter()
                    .map(Path::new)
                    .filter(|p| p.parent() == Some(root.as_path()))
                    .map(file_name)
                    .collect();
                let erase = action == Action::Erase;
                // (Off the loop: a big folder takes its time to go or come.)
                self.desktop_off_loop(
                    move || {
                        let trash = crate::trash::Trash::home();
                        let known = trash.list();
                        let mut stayed = Vec::new();
                        for name in names {
                            if erase {
                                info!("files: {name} deleted for good");
                                if let Err(e) = trash.erase(&name) {
                                    warn!("files: cannot delete {name}: {e}");
                                }
                                continue;
                            }
                            // Back where it was — never over something that
                            // has taken its place since.
                            let Some(was) = known.iter().find(|t| t.name == name) else {
                                continue;
                            };
                            if was.original_path.exists() {
                                stayed.push(was.display_name().to_owned());
                                continue;
                            }
                            match trash.restore(&name) {
                                Ok(to) => info!("files: {name} put back at {}", to.display()),
                                Err(e) => warn!("files: cannot put {name} back: {e}"),
                            }
                        }
                        stayed
                    },
                    move |app, stayed: Vec<String>| {
                        for name in stayed {
                            let body = format!("{name} {}", tr("is already there: it stays in the bin"));
                            std::thread::spawn(move || crate::desktop_send_notify(&body));
                        }
                        app.filer_list(id);
                        app.schedule_frame();
                    },
                );
            }
            Action::EmptyBin if bin == InBin::Top || place.as_deref() == Some(bin_root().as_path()) => {
                info!("files: the bin is emptied");
                self.desktop_off_loop(
                    || {
                        if let Err(e) = crate::trash::Trash::home().empty() {
                            warn!("files: the bin could not be emptied whole: {e}");
                        }
                    },
                    move |app, ()| {
                        app.filer_list(id);
                        app.schedule_frame();
                    },
                );
            }
            Action::Open if place.is_some() => {
                if let Some(place) = place.clone() {
                    self.filer_go(id, place);
                }
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
                let at_dir = match item.and_then(|i| win.items.get(i)) {
                    _ if place.is_some() => place.as_ref().map_or_else(String::new, |p| p.to_string_lossy().into_owned()),
                    Some(it) if matches!(it.kind, Kind::Folder | Kind::Volume) => it.path.clone(),
                    // (A thing of Recents: in the folder it is in.)
                    Some(it) if dir == recent::root() => {
                        Path::new(&it.path).parent().map_or_else(|| home().to_string_lossy().into_owned(), |p| p.to_string_lossy().into_owned())
                    }
                    _ => dir.to_string_lossy().into_owned(),
                };
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
                // (They wait in the pill to be put somewhere: the selection goes.)
                self.filer.clip_forgot = None;
                if let Some(win) = self.filer.win_mut(id) {
                    win.selected.clear();
                }
                self.filer_request_draw(id);
            }
            Action::Paste => self.filer_paste(id),
            // A stick of the sidebar: its name is typed on its desktop icon.
            Action::Rename if place.is_some() => {
                if let Some(place) = place.clone() {
                    self.desktop_volume_act(action, &place, at);
                }
            }
            Action::Rename => {
                let i = item.or_else(|| win.items.iter().position(|it| selected.first() == Some(&it.path)));
                if let Some(i) = i {
                    self.filer_begin_rename(id, i);
                }
            }
            Action::MoveToBin => {
                let paths: Vec<PathBuf> = selected.iter().map(PathBuf::from).collect();
                self.filer_bin(id, &paths); // the folder's watch takes them off the window
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
            Action::OpenApp(n) => {
                // A pick on the "Open with" page: that app, this file.
                let (Some((name, exec, terminal)), Some(path)) = (win.open_with.get(n).cloned(), selected.first())
                else {
                    return;
                };
                info!("files: {path} opened with {name}");
                let exec = format!("{exec} {}", launch::shell_quote(path));
                if let Err(e) = launch::launch(&exec, terminal, &self.config.launch.terminal) {
                    error!("files: {name} could not be started: {e:#}");
                    return;
                }
                let path = PathBuf::from(path);
                self.filer_used(&path);
            }
            Action::AddPlace | Action::RemovePlace => {
                // The folder the menu is on — or the place whose row it is.
                let Some(path) = place.clone().or_else(|| selected.first().map(PathBuf::from))
                else {
                    return;
                };
                let keep = action == Action::AddPlace;
                info!("files: {} {} the places", path.display(), if keep { "kept among" } else { "taken out of" });
                let file = marks_path();
                let text = marks_with(&std::fs::read_to_string(&file).unwrap_or_default(), &path, keep);
                if let Some(dir) = file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                crate::persist::write_text("files-places", &file, &text);
                self.filer_places();
            }
            Action::ToggleHidden => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.hidden = !win.hidden;
                }
                self.filer_list(id);
            }
            Action::NewFolder => self.filer_new_folder(id),
            Action::CleanUp => {
                info!("files: {} is in order of name again", dir.display());
                self.filer.orders.set(&dir, None);
                self.filer_list(id);
            }
            // Get info: the i card, on what is selected (else the folder).
            Action::Properties => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.info_on = true;
                }
                self.filer_info_due(id);
                self.filer_request_draw(id);
            }
            Action::OpenInTab => {
                let dir = place.clone().or_else(|| item.and_then(|i| win.items.get(i)).map(|it| PathBuf::from(&it.path)));
                if let Some(dir) = dir.filter(|d| d.is_dir() || *d == recent::root() || *d == bin_root()) {
                    self.filer_tab_open(id, dir);
                }
            }
            Action::QuickLook => {
                if let Some(i) = item {
                    if let Some(win) = self.filer.win_mut(id) {
                        win.cursor = Some(i);
                    }
                    self.filer_peek_open(id);
                }
            }
            Action::SelectAll => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.selected = win.items.iter().map(|it| it.path.clone()).collect();
                }
                self.filer_request_draw(id);
            }
            Action::Duplicate => self.filer_duplicate(id, selected),
            Action::PasteInto => {
                if let Some(dir) = item.and_then(|i| win.items.get(i)).map(|it| PathBuf::from(&it.path)) {
                    self.filer_paste_into(id, dir);
                }
            }
            Action::CopyToStick => {
                let Some((_, stick)) = self.one_stick(Some(&dir)) else {
                    return;
                };
                let paths: Vec<PathBuf> = selected.iter().map(PathBuf::from).collect();
                self.filer_bring(id, paths, stick, false, None, || {});
            }
            Action::SendTo(n) => {
                let paths: Vec<PathBuf> = selected.iter().map(PathBuf::from).collect();
                self.send_to(n, paths, Some(id));
            }
            Action::NewFolderWith => self.filer_group(id, selected),
            Action::Play
            | Action::Shuffle
            | Action::PlayHere
            | Action::Slideshow
            | Action::SlideshowHere
            | Action::SetWallpaper
            | Action::Rotate
            | Action::Print => self.filer_smart_act(id, action),
            // A stick's or a phone's own rows, from the sidebar: what its
            // icon on the desktop does.
            Action::Eject
            | Action::Format
            | Action::CheckDrive
            | Action::Mirror
            | Action::Camera
            | Action::CameraStop
            | Action::CameraSettings
            | Action::ImportPhotos
            | Action::Tether
            | Action::Configure
            | Action::HelpDebugging
            | Action::HelpFiles => {
                if let Some(place) = place.clone() {
                    self.desktop_volume_act(action, &place, at);
                }
            }
            _ => {}
        }
    }

    /// The clipboard's files into the folder shown: copied, or moved if
    /// they were cut. Off the loop; what arrived comes up selected.
    fn filer_paste(&mut self, id: u32) {
        if let Some(dir) = self.filer.win(id).filter(|w| w.put_here()).map(|w| w.dir.clone()) {
            self.filer_paste_into(id, dir);
        }
    }

    /// The clipboard's files into folder `dir` of window `id` (the one
    /// shown, or one of its folders: "Paste into folder").
    fn filer_paste_into(&mut self, id: u32, dir: PathBuf) {
        let Some((paths, cut)) = self.clipboard_files() else {
            return;
        };
        let into = dir.clone();
        let sources = paths.clone();
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
                for undo in undo_of(&sources, &brought) {
                    app.filer_did(id, undo);
                }
                info!(
                    "files: pasted {} into {} ({}; {failed} failed)",
                    brought.len(),
                    dir.display(),
                    if cut { "moved" } else { "copied" }
                );
                let now: Vec<String> = brought.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                if cut {
                    // The same files where they are now, as a plain copy: a
                    // second paste must not look for what has moved — and
                    // the pill offers them no more (a move is spent).
                    app.serve_files(&now, false);
                    app.filer.clip_forgot = Some(now.clone());
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
            .find(|p| p.path == dir && p.name != UP)
            .map_or_else(|| file_name(dir), |p| p.name.clone());
        let words = if says_move {
            tr("Moving {what} to {where}")
        } else {
            tr("Copying {what} to {where}")
        };
        Some(self.task_begin(&crate::i18n::fill(words, &[("what", &what), ("where", &place)])))
    }

    /// Throw `paths` away, remembering their names in the bin so window
    /// `id`'s history can put them back.
    pub(crate) fn filer_bin(&mut self, id: u32, paths: &[PathBuf]) {
        let paths = paths.to_vec();
        // (Off the loop: from another disk a thing is COPIED into the bin.)
        self.desktop_off_loop(
            move || {
                let trash = crate::trash::Trash::home();
                let (mut names, mut shown) = (Vec::new(), Vec::new());
                let mut failed = 0usize;
                for path in &paths {
                    match trash.trash(path) {
                        Ok(item) => {
                            info!("files: {} → Recycle Bin", path.display());
                            names.push(item.name);
                            shown.push(file_name(path));
                        }
                        Err(e) => {
                            warn!("files: cannot throw {} away: {e}", path.display());
                            failed += 1;
                        }
                    }
                }
                (names, shown, failed)
            },
            move |app, (names, shown, failed): (Vec<String>, Vec<String>, usize)| {
                if !names.is_empty() {
                    app.filer_binned(id, names, &shown);
                }
                if failed > 0 {
                    let body = crate::i18n::fill(tr("{n} could not be moved to the bin"), &[("n", &failed)]);
                    std::thread::spawn(move || crate::desktop_send_notify(&body));
                }
                // (The dock's Files section and its bin show it too.)
                app.refilter();
            },
        );
    }

    /// The clipboard changed: what is cut is drawn faded, so every window
    /// draws again.
    pub(crate) fn filer_clip_changed(&mut self) {
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            self.filer_request_draw(id);
        }
    }

    /// Enter in the typed path: go to the folder it stands for (a file's
    /// folder comes up with the file selected); a path that is nothing
    /// stays in the field.
    fn filer_go_typed(&mut self, id: u32) {
        let Some(text) = self.filer.win(id).and_then(|w| w.go_to.clone()) else {
            return;
        };
        let Some((dir, file)) = typed_place(&text, &home()).filter(|(dir, _)| self.filer_may_show(id, dir)) else {
            // (Nothing there: the field says so, and stays.)
            if let Some(win) = self.filer.win_mut(id) {
                win.go_bad = true;
            }
            return self.filer_request_draw(id);
        };
        if let Some(win) = self.filer.win_mut(id) {
            win.go_to = None;
            win.reveal = file.map(|f| f.to_string_lossy().into_owned());
            if win.dir == dir {
                self.filer_list(id);
                return self.filer_request_draw(id);
            }
        }
        self.filer_go(id, dir);
    }

    /// Some things could not be copied: said once, as a notification.
    fn filer_say_failed(&mut self, failed: usize) {
        if failed > 0 {
            let body = crate::i18n::fill(tr("{n} could not be copied"), &[("n", &failed)]);
            std::thread::spawn(move || crate::desktop_send_notify(&body));
        }
    }

    /// "Duplicate": a copy of each beside it, "name copy" (then "name copy
    /// 2"…), made off the loop; the first comes up selected, and Undo takes
    /// them to the bin.
    fn filer_duplicate(&mut self, id: u32, selected: Vec<String>) {
        if selected.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = selected.iter().map(PathBuf::from).collect();
        self.desktop_off_loop(
            move || {
                let mut made = Vec::new();
                for src in &paths {
                    let Some(dir) = src.parent() else {
                        continue;
                    };
                    let dest = copy_name(dir, &file_name(src), src.is_dir());
                    match crate::desktop_send::copy_all_metered(src, &dest, None) {
                        Ok(()) => {
                            info!("files: {} duplicated as {}", src.display(), dest.display());
                            made.push(dest);
                        }
                        Err(e) => warn!("files: cannot duplicate {}: {e}", src.display()),
                    }
                }
                made
            },
            move |app, made: Vec<PathBuf>| {
                if made.is_empty() {
                    return;
                }
                if let Some(win) = app.filer.win_mut(id) {
                    win.reveal = made.first().map(|p| p.to_string_lossy().into_owned());
                }
                let text = match made.as_slice() {
                    [one] => fill(tr("Duplicated “{name}”"), &[("name", &file_name(one))]),
                    _ => fill(tr("Duplicated {n} items"), &[("n", &made.len())]),
                };
                app.filer_did_said(id, Undo::Copied(made), text, "dup");
                app.filer_list(id);
            },
        );
    }

    /// "New folder with these": a new folder here ("New folder with
    /// items"), the selection moved into it and its name typed at once;
    /// Undo takes them out again, and the folder with them.
    fn filer_group(&mut self, id: u32, selected: Vec<String>) {
        let Some(dir) = self.filer.win(id).filter(|w| w.put_here()).map(|w| w.dir.clone()) else {
            return;
        };
        if selected.is_empty() {
            return;
        }
        let base = tr("New folder with items");
        let folder = (1..)
            .map(|n| if n == 1 { dir.join(base) } else { dir.join(format!("{base} {n}")) })
            .find(|p| !p.exists())
            .unwrap_or_else(|| dir.join(base));
        if let Err(e) = std::fs::create_dir(&folder) {
            error!("files: cannot make {}: {e}", folder.display());
            return;
        }
        let mut moved = Vec::new();
        let mut failed = 0usize;
        for path in &selected {
            let from = PathBuf::from(path);
            // Never over another of them: a search lists things of the
            // folders below too, and two may share a name (`rename` would
            // put one over the other, and that one was gone for good).
            let to = crate::bt_files::free_name(&folder, &file_name(&from));
            match std::fs::rename(&from, &to) {
                Ok(()) => moved.push((from, to)),
                Err(e) => {
                    warn!("files: cannot put {} in the new folder: {e}", from.display());
                    failed += 1;
                }
            }
        }
        info!("files: {} thing(s) put in {}", moved.len(), folder.display());
        if failed > 0 {
            let body = fill(tr("{n} could not be put in the new folder"), &[("n", &failed)]);
            std::thread::spawn(move || crate::desktop_send_notify(&body));
        }
        if let Some(win) = self.filer.win_mut(id) {
            win.rename_next = Some(folder.to_string_lossy().into_owned());
        }
        self.filer_did(id, Undo::Grouped { folder, moved });
        self.filer_list(id);
    }

    /// A new, empty folder here, named `untitled folder` (then `untitled
    /// folder 2`…), its name opened for typing once it is listed.
    fn filer_new_folder(&mut self, id: u32) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let dir = win.dir.clone();
        // (Recents is no folder: nothing is made in it.)
        if dir == recent::root() {
            return;
        }
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
        self.filer_did(id, Undo::Made(path));
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
        let is_file = item.kind == Kind::File;
        info!("files: renaming {text}");
        win.select_only(i);
        // (A folder is named whole; a file's ending stays out of it.)
        let stem = match crate::desktop::ending(&text).filter(|_| is_file) {
            Some(end) => text.len() - end.len() - 1,
            None => text.len(),
        };
        win.rename = Some(Rename { path, text, picked: stem, caret: stem });
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
            self.filer_did(id, Undo::Renamed {
                from: old.clone(),
                to: new.clone(),
            });
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
                if rename.picked > 0 {
                    rename.text.replace_range(..rename.picked, "");
                    rename.caret = 0;
                } else if let Some((at, _)) = rename.text[..rename.caret].char_indices().next_back() {
                    rename.text.replace_range(at..rename.caret, "");
                    rename.caret = at;
                }
                rename.picked = 0;
            }
            Keysym::Delete => {
                if rename.picked > 0 {
                    rename.text.replace_range(..rename.picked, "");
                    rename.caret = 0;
                } else if let Some(c) = rename.text[rename.caret..].chars().next() {
                    rename.text.replace_range(rename.caret..rename.caret + c.len_utf8(), "");
                }
                rename.picked = 0;
            }
            Keysym::Left => {
                rename.caret = if rename.picked > 0 {
                    0
                } else {
                    rename.text[..rename.caret].char_indices().next_back().map_or(0, |(at, _)| at)
                };
                rename.picked = 0;
            }
            Keysym::Right => {
                if rename.picked == 0 {
                    rename.caret += rename.text[rename.caret..].chars().next().map_or(0, char::len_utf8);
                }
                rename.picked = 0;
            }
            Keysym::Home => {
                rename.caret = 0;
                rename.picked = 0;
            }
            Keysym::End => {
                rename.caret = rename.text.len();
                rename.picked = 0;
            }
            _ => {
                let Some(text) = utf8.filter(|t| !ctrl && !t.is_empty() && !t.chars().any(char::is_control)) else {
                    return;
                };
                if rename.picked > 0 {
                    rename.text.replace_range(..rename.picked, "");
                    rename.caret = 0;
                    rename.picked = 0;
                }
                rename.text.insert_str(rename.caret, text);
                rename.caret += text.len();
            }
        }
        self.filer_request_draw(id);
    }

    /// The Properties box for item `i`: grown out of `menu` when it comes
    /// from its last row, by the pointer otherwise. A folder is weighed on
    /// a thread.
    fn filer_open_props(&mut self, id: u32, i: usize, at: (f32, f32), menu: Option<Menu>) {
        let Some(item) = self.filer.win(id).and_then(|w| w.items.get(i)).cloned() else {
            return;
        };
        self.filer_open_props_of(id, &item, at, menu);
    }

    /// The Properties box of `item` (a thing of the window, or a place of
    /// its sidebar).
    fn filer_open_props_of(&mut self, id: u32, item: &Item, at: (f32, f32), menu: Option<Menu>) {
        let Some(win) = self.filer.win_mut(id) else {
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

    /// This look's colours of a file window.
    pub(crate) fn filer_palette(&self) -> Palette {
        // Golem Files is an app: it wears what the APPS slider gives the apps
        // (Max, 2026-10-17: "now the bg color on golem files"), its Dark/Light
        // included — never the dock's sampled colour.
        let mut t = crate::palette::for_apps(&crate::palette::current());
        t.dynamic = false;
        palette(t.dark, self.config.theme.background_rgba(), self.config.theme.text_rgba(), &t)
    }

    /// The menu's and the box's paint in a window: the boxes' own surface,
    /// laid solid on the window's ground (there is no glass inside it). In
    /// the light, a pale warm grey panel with the window's ink.
    fn filer_panel_paint(&self, rect: Rect, ground: [f32; 4]) -> MenuPaint {
        let mut paint = self.desktop_panel_paint(rect);
        // (Golem Files keeps its menus' rules.) The menu is the SHELL's:
        // its colours and Dark/Light (Max, 2026-10-17), laid solid on the
        // window — and unlit, as the History is (the dark look's night light
        // sank its lower rows toward black).
        paint.glass = crate::content::UNLIT;
        // (The row under the pointer is tinted with the window's own accent.)
        paint.accent = self.filer_palette().accent;
        if crate::palette::shell().is_some() {
            paint.fill[3] = 1.0;
            return paint;
        }
        if !crate::palette::shell_dark() {
            paint.fill = [0.807, 0.784, 0.73, 1.0]; // #e8e5de
            paint.ink = self.filer_palette().ink;
            paint.wash = [0.0, 0.0, 0.0, 0.06];
            return paint;
        }
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
            return;
        }
        // (Used: in Recents, here and in every app's.)
        if item.kind == Kind::File {
            self.filer_used(Path::new(&item.path));
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
        let drawn = scale;
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

    /// A second click on the name of the one thing selected: once the
    /// double click's time is out (another click meanwhile — a double click
    /// — takes it back), its name is typed in place.
    fn filer_rename_soon(&mut self, id: u32, i: usize) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let Some(path) = win.items.get(i).map(|it| it.path.clone()) else {
            return;
        };
        win.rename_seq += 1;
        let seq = win.rename_seq;
        win.rename_due = Some(seq);
        let timer = calloop::timer::Timer::from_duration(DOUBLE_CLICK);
        let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            let ready = app.filer.win_mut(id).filter(|w| {
                w.rename_due == Some(seq) && w.rename.is_none() && w.selected.len() == 1 && w.selected.contains(&path)
            });
            if let Some(win) = ready {
                win.rename_due = None;
                if let Some(i) = win.items.iter().position(|it| it.path == path) {
                    app.filer_begin_rename(id, i);
                }
            }
            calloop::timer::TimeoutAction::Drop
        });
    }

    /// Search window `id`'s folder for `find` (`Some("")`: the field out and
    /// empty; `None`: put away, everything shown again).
    fn filer_find(&mut self, id: u32, find: Option<String>) {
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        win.find = find;
        // (What was found below was for other words.)
        win.deep.clear();
        win.deep_gen += 1;
        win.show_found();
        // (Not below the bin: what lies inside a thrown-away folder has no
        // record of its own, and the bin's rows are not for it.)
        let words = win
            .find
            .clone()
            .filter(|f| !f.trim().is_empty() && win.in_bin() == InBin::No && win.dir != recent::root());
        win.deep_busy = words.is_some();
        let (generation, dir, hidden) = (win.deep_gen, win.dir.clone(), win.hidden);
        // What is shown changed under everything that held a place in it.
        let present: HashSet<&str> = win.items.iter().map(|it| it.path.as_str()).collect();
        win.selected.retain(|p| present.contains(p.as_str()));
        win.cursor = None;
        win.anchor = None;
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
        // …and, once the typing has stopped a moment, the folders below
        // are searched too, off the loop.
        let Some(words) = words else {
            return;
        };
        let timer = calloop::timer::Timer::from_duration(DEEP_WAIT);
        let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            if app.filer.win(id).is_some_and(|w| w.deep_gen == generation) {
                let (dir, words) = (dir.clone(), words.clone());
                app.desktop_off_loop(
                    move || {
                        let items = search_below(&dir, &words, hidden);
                        let meta = meta_of(&items);
                        (items, meta)
                    },
                    move |app, (items, meta)| {
                        let Some(win) = app.filer.win_mut(id).filter(|w| w.deep_gen == generation) else {
                            return;
                        };
                        win.deep = items;
                        win.meta.extend(meta);
                        win.deep_busy = false;
                        win.show_found();
                        win.names.1.clear();
                        win.slide.clear();
                        app.filer_request_draw(id);
                    },
                );
            }
            calloop::timer::TimeoutAction::Drop
        });
    }

    /// Window `id` turned Files ↔ Media: what it shows is chosen again.
    fn filer_remode(&mut self, id: u32) {
        if let Some(win) = self.filer.win_mut(id) {
            win.show_found();
            let present: HashSet<&str> = win.items.iter().map(|it| it.path.as_str()).collect();
            win.selected.retain(|p| present.contains(p.as_str()));
            win.cursor = None;
            win.anchor = None;
            win.names.1.clear();
            win.slide.clear();
            win.scroll = 0.0;
            win.scroll_to = 0.0;
        }
        self.filer_relayout(id);
        self.filer_hover(id);
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

    /// The owner's zoom of the file windows changed: every window's field is
    /// laid out and drawn anew at it (the places and the head keep theirs).
    pub(crate) fn filer_zoomed(&mut self) {
        let (zoom, panel) = (self.files_zoom(), self.files_panel_zoom());
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            let scale = self.surface_scale(SurfaceKind::Files(id));
            if let Some(win) = self.filer.win_mut(id) {
                win.zoom = zoom;
                win.panel = panel;
                let _ = scale;
                // What is up was placed for the other size.
                win.menu = None;
                win.menu_path = None;
                win.props = None;
            }
            self.filer_relayout(id);
            self.filer_hover(id);
        }
    }

    /// The menus' size changed (`zoom.rs`): a menu up in a window is laid
    /// out again at once, the row under the pointer still the one under it.
    pub(crate) fn filer_menus_rescaled(&mut self) {
        let s = crate::desktop_menu::scale();
        let mut ids = Vec::new();
        for win in self.filer.wins.iter_mut() {
            if let Some(menu) = win.menu.as_mut() {
                menu.rescale(s);
                menu.pointer(win.ptr);
                ids.push(win.id);
            }
        }
        for id in ids {
            self.filer_request_draw(id);
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

    /// Every symbol read once, for all the windows (`Filer::glyphs`).
    fn filer_glyphs_warm(&mut self) {
        if self.filer.glyphs_asked {
            return;
        }
        let Some(icons) = self.filer.icons.as_ref() else {
            return;
        };
        self.filer.glyphs_asked = true;
        for (name, _) in crate::glyphs::GLYPHS {
            if let Some(req) = crate::desktop::icon_request(&crate::glyphs::key(name)) {
                icons.request(req);
            }
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
            if !crate::thumbs::previewable(&item.name) {
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
        // (The places as they are drawn: a folder, or its symbol alone.)
        let mut place_keys: Vec<String> = win.places.iter().map(|p| win.place_key(p)).collect();
        // (And the chrome's own: its symbols, the tabs', the i card's picture.)
        place_keys.extend(chrome::icon_keys(win));
        let mut worn: Vec<&str> = Vec::new();
        for key in place_keys
            .iter()
            .map(String::as_str)
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
            if let Some(chain) = win.chains.get(key).or_else(|| self.filer.glyphs.get(key)) {
                if !win.ink.contains_key(key) {
                    if win.ink.len() > 4000 {
                        win.ink.clear();
                    }
                    win.ink.insert(key.to_owned(), ink_rows(chain));
                }
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

    /// The look changed: every window lets go of the theme's icons (not the
    /// files' own pictures), asks for them again and draws in its new colours.
    /// Every file window draws again (the palette changed: no picture to drop).
    pub(crate) fn filer_redraw_all(&mut self) {
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            self.filer_request_draw(id);
        }
    }

    pub(crate) fn filer_look_changed(&mut self) {
        let mut ids = Vec::new();
        for win in self.filer.wins.iter_mut() {
            win.chains.retain(|k, _| k.starts_with("thumb:"));
            win.missing.clear();
            win.pending.clear();
            for held in win.uploaded.iter_mut() {
                if held.as_deref().is_some_and(|k| !k.starts_with("thumb:")) {
                    *held = None;
                }
            }
            ids.push(win.id);
        }
        for id in ids {
            self.filer_request_draw(id);
        }
    }

    /// The resolver answered: every window that asked for it files the
    /// pixels and draws.
    fn filer_on_icon(&mut self, res: crate::notif_icons::Resolved) {
        // (A symbol: kept for every window.)
        if crate::glyphs::is_key(&res.key) {
            if let Some(chain) = res.chain.as_ref() {
                self.filer.glyphs.insert(res.key.clone(), chain.clone());
            }
        }
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

    /// The file at `path` was written (Quick Look's pencil saved over it):
    /// every picture of it held is let go, so it is made again.
    pub(crate) fn thumb_stale(&mut self, path: &str) {
        let key = crate::desktop::thumb_key(path);
        self.thumb_map.remove(path);
        self.thumb_pending.remove(path);
        let mut redraw = Vec::new();
        for win in self.filer.wins.iter_mut() {
            if win.chains.remove(&key).is_some() {
                for held in win.uploaded.iter_mut() {
                    if held.as_deref() == Some(key.as_str()) {
                        *held = None;
                    }
                }
                redraw.push(win.id);
            }
        }
        for id in redraw {
            self.filer_request_draw(id);
        }
        if self.desktop.chains.remove(&key).is_some() {
            self.desktop.forget_uploaded(&key);
            self.request_desktop_draw();
        }
    }

    /// Draw one frame of window `id`.
    fn filer_draw(&mut self, id: u32) {
        // (The printers asked again once their answer is old; never waited for.)
        self.filer_printers(false);
        // (What the keys are on, and its neighbours, prepared for Quick Look.)
        if let Some((at, near)) = self.filer_peek_near(id) {
            self.peek_follow_keys(id, &at, near);
        }
        let icon_scale = self.icon_scale();
        let pal = self.filer_palette();
        let (ground, side_ground, ink) = (pal.ground, pal.side, pal.ink);
        // (A selected picture's backdrop: black 7.5 % in the light, white
        // 10 % in the dark — the mockup's, mixed as a page mixes them.)
        let pic_mark = if pal.dark {
            chrome::mix(ground, [1.0, 1.0, 1.0, 1.0], 0.10)
        } else {
            chrome::mix(ground, [0.0, 0.0, 0.0, 1.0], 0.075)
        };
        // (Every other row of a list: the "A · Unified" mockup's stripe —
        // #f5f5f7 on white, #262628 on #1e1e1e — faint enough that a
        // selected row, in the soft colour, stands out of them.)
        let zebra = if pal.dark {
            chrome::mix(ground, [1.0, 1.0, 1.0, 1.0], 0.035)
        } else {
            chrome::mix(ground, [0.0, 0.0, 0.0, 1.0], 0.04)
        };
        // (A selected row's band over a stripe: the soft colour laid on it.)
        let dark = pal.dark;
        let band_on_stripe = if pal.dark {
            chrome::mix(zebra, [1.0, 1.0, 1.0, 1.0], 0.10)
        } else {
            chrome::mix(zebra, [0.0, 0.0, 0.0, 1.0], 0.075)
        };
        // (A selected name's own box: the same soft colour laid again on
        // the box under it — one step darker.)
        let name_mark = if pal.dark {
            chrome::mix(pic_mark, [1.0, 1.0, 1.0, 1.0], 0.10)
        } else {
            chrome::mix(pic_mark, [0.0, 0.0, 0.0, 1.0], 0.075)
        };
        // (A typed name's field's hairline.)
        let hair = if pal.dark {
            chrome::mix(ground, [1.0, 1.0, 1.0, 1.0], 0.12)
        } else {
            chrome::mix(ground, [0.0, 0.0, 0.0, 1.0], 0.14)
        };
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
        // The folder on its way to where the scroll sent it: quickly after a
        // step of the wheel, slowly through a glide (the card's rates).
        let rate = if win.gliding { RATE_GLIDE } else { RATE_NOTCH };
        let (scroll, mut moving) = crate::animation::ease_toward(win.scroll, win.scroll_to, dt, rate, 0.4);
        win.scroll = scroll;
        if !moving {
            win.gliding = false;
        }
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
        // The sidebar opening or closing, and its places scrolling.
        let open = win.side_open || win.pinned;
        let (t, m) = crate::animation::ease_toward(win.side_t, if open { 1.0 } else { 0.0 }, dt, SIDE_RATE, 0.004);
        win.side_t = t;
        moving |= m;
        win.side_scroll_to = win.side_scroll_to.min(win.side_max());
        let (y, m) = crate::animation::ease_toward(win.side_scroll, win.side_scroll_to, dt, RATE_NOTCH, 0.4);
        win.side_scroll = y;
        moving |= m;
        win.last_frame = moving.then_some(now);
        self.filer_sync_icons(&mut win, &layout);

        let (w, h) = win.virt();
        let mut scene = Scene {
            alpha: 1.0,
            ..Default::default()
        };
        scene.rects.push(rect(Rect::new(0.0, 0.0, w, h), 0.0, ground));
        let _ = side_ground;
        let mut hits = Hits::default();

        // ── the chrome: the sidebar, the top line, the i card ──
        // (`filer/chrome.rs`; drawn after the files, over them.)
        let (chrome, m) = self.filer_chrome(&mut win, &pal, &mut hits, dt);
        if m {
            moving = true;
            win.last_frame = Some(now);
        }

        // ── the icons ──
        let mut grid = GridContent {
            clip: layout.body,
            ..Default::default()
        };
        let view = layout.in_view(win.scroll, win.items.len());
        // (A name has the whole column's width, not only its cell's: the
        // cells are narrow — Max wants the things close — and a word should
        // not break.)
        let label_w = (layout.pitch - 4.0).max(layout.cw - 12.0);
        if win.names.0 != icon_scale || win.names.1.len() != win.items.len() {
            win.names = (icon_scale, vec![None; win.items.len()]);
        }
        // Names are painted last of all: one under the menu or the box is
        // left out, or it would show through (the desktop's lesson).
        let cover = win.menu.as_ref().map(|m| m.rect).or(win.props.as_ref().map(|p| p.rect));
        // (What covers the files is in the window's space; the files are
        // laid out in their own, `zoom` times smaller from the body's corner.)
        let into_field = |r: Rect| {
            let z = layout.zoom;
            let (bx, by) = (layout.body.x, layout.body.y);
            Rect::new(bx + (r.x - bx) / z, by + (r.y - by) / z, r.w / z, r.h / z)
        };
        let cover = cover.map(into_field);
        // (The i card over the files: a name under it is left out too —
        // and the field a name is typed in, wider than its cell: the names
        // beside it.)
        let typing: Option<(usize, Rect)> = win.rename.as_ref().and_then(|r| {
            let i = win.items.iter().position(|it| it.path == r.path)?;
            let mut cell = layout.cell(i);
            cell.y -= win.scroll;
            let (px, line) = if layout.list { (TEXT_PX, TEXT_LINE) } else { (LABEL_FONT_PX, LABEL_LINE_PX) };
            let tw = win.renderer.as_mut().map_or(0.0, |rd| rd.measure_text(&r.text, px, None)).max(24.0);
            let top = if layout.list {
                cell.y + (cell.h - TEXT_LINE) / 2.0
            } else {
                layout.icon_in(&cell, icon_scale).y + layout.icon_in(&cell, icon_scale).h + LABEL_GAP
            };
            let x = if layout.list { cell.x + LIST_NAME_X - NAME_PAD.0 } else { name_field_x(&layout, cell.x + cell.w / 2.0, tw) };
            Some((i, Rect::new(x, top - NAME_PAD.1, tw + 2.0 * NAME_PAD.0, line + 2.0 * NAME_PAD.1)))
        });
        let over = [into_field(hits.card), typing.map_or(Rect::default(), |(_, r)| r)];
        let meets = |c: &Rect, r: &Rect| r.x < c.x + c.w && c.x < r.x + r.w && r.y < c.y + c.h && c.y < r.y + r.h;
        let covered = |r: Rect| cover.is_some_and(|c| meets(&c, &r)) || over.iter().any(|c| c.w > 0.0 && meets(c, &r));
        // MAKE ROOM: what is in hand leaves its place, and while a drag is
        // over the window the rest part around where it would land — each
        // easing to its display slot at the Apps grid's rate.
        // What was CUT waits, faded, to be pasted somewhere (the clipboard's
        // newest thing is those files, to be moved).
        let cut: HashSet<String> = match self.clipboard_files() {
            Some((paths, true)) => {
                let names: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                // (Forgotten in the pill: not waiting any more.)
                if self.filer.clip_forgot.as_ref() == Some(&names) {
                    HashSet::new()
                } else {
                    names.into_iter().collect()
                }
            }
            _ => HashSet::new(),
        };
        let held = self.filer.drag.as_ref().filter(|d| !d.dropped);
        let in_hand: HashSet<&str> = held
            .map(|d| d.paths.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let width = held.map_or(1, |d| d.paths.len());
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
        // THE LIST'S STRIPES (Max, 2026-10-21, the mockup's v44: "i like it,
        // make it real"): square, edge to edge — from the sidebar to the
        // window's right side (the grid's clip cuts them) — every other row
        // slot from the first row TO THE BOTTOM of what is seen, past the
        // last thing, scrolling with the rows: never a gap. The first row
        // plain, the second striped. An empty folder has none.
        let across = |y: f32| Rect::new(-2.0 * w, y, 5.0 * w, layout.ch);
        if layout.list && !win.items.is_empty() {
            let first = ((win.scroll - PAD) / layout.ch).floor().max(0.0) as usize;
            let last = ((win.scroll + layout.body.h - PAD) / layout.ch).ceil().max(0.0) as usize;
            for slot in (first..=last).filter(|s| s % 2 == 1) {
                let y = layout.body.y + PAD + slot as f32 * layout.ch - win.scroll;
                grid.rects.push(rect(across(y), 0.0, zebra));
            }
        }
        for i in view {
            let item = &win.items[i];
            if in_hand.contains(item.path.as_str()) {
                continue;
            }
            let mut cell = layout.cell_at(win.slide[i]);
            cell.y -= win.scroll;
            if layout.list && (win.selected.contains(&item.path) || win.drop_on == Some(i)) {
                // A selected row: a square band of the soft selection colour
                // across, over its stripe (no accent — the words stay as
                // they are: Max, 2026-10-21).
                let band = if i % 2 == 1 { band_on_stripe } else { pic_mark };
                grid.rects.push(rect(across(cell.y), 0.0, band));
            }
            // The picture's space, and the picture SITTING ON ITS BOTTOM (Max,
            // 2026-10-21: the name about 3 px under what is seen): a folder's
            // or a page's drawing has empty margin of its own, a wide photo
            // stands in the middle of its square — each is moved down by the
            // empty rows under what it shows (`Win::ink`, `ink_rows`).
            let slot = layout.icon_in(&cell, icon_scale);
            let (ink_top, ink_bottom) = if layout.list { (0.0, 0.0) } else { win.ink.get(&item.icon).copied().unwrap_or((0.0, 0.0)) };
            let icon = Rect::new(slot.x, slot.y + ink_bottom * slot.h, slot.w, slot.h);
            let seen_top = icon.y + ink_top * icon.h;
            let picked = win.selected.contains(&item.path);
            let faded = cut.contains(&item.path);
            if let Some(layer) = win.layer_of(&item.icon) {
                // (A file's own picture: as it is, whatever the palette.)
                let picture = if item.icon.starts_with("thumb:") { crate::content::PICTURE } else { 0 };
                grid.icons.push(IconInst {
                    rect: icon,
                    layer: layer | picture,
                    // (Washed toward the ground it stands on.)
                    // (At 45 %: what waits to be moved, the mockup's.)
                    tint: if faded { [ground[0], ground[1], ground[2], 0.55] } else { [0.0; 4] },
                    // (-1.3: the wash alone — at -1.0 a tinted icon also
                    // wears the dock's "to the bin" can.)
                    ring: if faded { -1.3 } else { -1.0 },
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
                // (The row whose name is being typed is not hidden by its own field.)
                let own = typing.is_some_and(|(t, _)| t == i);
                if !own && covered(Rect::new(name_x, top, cell.x + cell.w - name_x, TEXT_LINE)) {
                    continue;
                }
                let meta = win.meta.get(&item.path).copied().unwrap_or_default();
                if let Some(rename) = win.rename.as_ref().filter(|r| r.path == item.path) {
                    // (Left-aligned in a list, where the name was.)
                    let typed = Typed::of(rename, &mut |t| win.renderer.as_mut().map_or(0.0, |r| r.measure_text(t, TEXT_PX, None)));
                    typed.draw(&mut grid, name_x, top, (TEXT_PX, TEXT_LINE), (ground, hair, pal.accent, ink));
                } else if let Some(renderer) = win.renderer.as_mut() {
                    // (A thing found below is named with its folder.)
                    let full = if i >= win.deep_from {
                        format!("{}/{}", below(&item.path, &win.dir), item.name)
                    } else {
                        item.name.clone()
                    };
                    let name = crate::desktop::fit_label(&full, name_w, &mut |t| {
                        renderer.measure_text(t, TEXT_PX, None)
                    });
                    // (Selected: the name in its own soft box too, a step darker.)
                    if picked {
                        let nw = renderer.measure_text(&name, TEXT_PX, None).min(name_w);
                        let r = Rect::new(name_x - NAME_PAD.0, top - NAME_PAD.1, nw + 2.0 * NAME_PAD.0, TEXT_LINE + 2.0 * NAME_PAD.1);
                        // (A step darker than the band it is on.)
                        let under = if i % 2 == 1 { band_on_stripe } else { pic_mark };
                        let step = if dark { ([1.0, 1.0, 1.0, 1.0], 0.10) } else { ([0.0, 0.0, 0.0, 1.0], 0.075) };
                        grid.rects.push(rect(r, 4.0, chrome::mix(under, step.0, step.1)));
                    }
                    grid.labels.push(label(&name, (name_x, top), name_w + 2.0, false, faded));
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
                    let mut measure = |t: &str| renderer.measure_text(t, LABEL_FONT_PX, None);
                    win.names.1[i] = Some(if i >= win.deep_from {
                        // Found below: its name on one line, its folder dim
                        // on the other.
                        (
                            crate::desktop::fit_label(&item.name, label_w, &mut measure),
                            crate::desktop::fit_label(&below(&item.path, &win.dir), label_w, &mut measure),
                        )
                    } else {
                        let lines = two_lines(&item.name, label_w, &mut measure);
                        tracing::debug!(
                            "files: name {:?} within {label_w} (measured {}) → {:?}",
                            item.name,
                            measure(&item.name),
                            lines
                        );
                        lines
                    });
                }
            }
            // SELECTED, the mockup's "one soft box" (Max, 2026-10-21): ONE
            // soft box over the picture AND its name, hugging this thing (a
            // one-line name's box is not stretched to its neighbours'), the
            // name in its own box of the same soft colour on it — a step
            // darker; no accent. A folder a drag would be let go into wears it
            // too. The name stands about 3 px under what the picture shows.
            let top = slot.y + slot.h + LABEL_GAP;
            let cx = cell.x + cell.w / 2.0;
            let typing_here = win
                .rename
                .as_ref()
                .filter(|r| r.path == item.path)
                .map(|r| Typed::of(r, &mut |t| win.renderer.as_mut().map_or(0.0, |rd| rd.measure_text(t, LABEL_FONT_PX, None))));
            let lines: Vec<String> = win.names.1[i]
                .as_ref()
                .map(|(a, b)| [a, b].into_iter().filter(|l| !l.is_empty()).cloned().collect())
                .unwrap_or_default();
            let name_box = match &typing_here {
                // (A name being typed: its field, kept inside the field.)
                Some(t) => Rect::new(
                    name_field_x(&layout, cx, t.width()),
                    top - NAME_PAD.1,
                    t.width() + 2.0 * NAME_PAD.0,
                    LABEL_LINE_PX + 2.0 * NAME_PAD.1,
                ),
                None => {
                    let widest = lines
                        .iter()
                        .map(|l| win.renderer.as_mut().map_or(0.0, |r| r.measure_text(l, LABEL_FONT_PX, None)))
                        .fold(0.0, f32::max)
                        .min(label_w);
                    Rect::new(
                        cx - widest / 2.0 - NAME_PAD.0,
                        top - NAME_PAD.1,
                        widest + 2.0 * NAME_PAD.0,
                        lines.len().max(1) as f32 * LABEL_LINE_PX + 2.0 * NAME_PAD.1,
                    )
                }
            };
            if picked || win.drop_on == Some(i) {
                let w = slot.w.max(name_box.w) + 2.0 * SEL_PAD;
                // (Rows stand close: under a selected thing whose two-line
                // name's box reaches into this row, this box begins below it.)
                let above = i
                    .checked_sub(layout.cols)
                    .filter(|&j| win.items.get(j).is_some_and(|it| win.selected.contains(&it.path)))
                    .map_or(f32::MIN, |j| {
                        let n = win.names.1.get(j).and_then(Option::as_ref).map_or(1, |(_, b)| if b.is_empty() { 1 } else { 2 });
                        top - layout.ch + n as f32 * LABEL_LINE_PX + NAME_PAD.1 + SEL_PAD + 2.0
                    });
                let y = (seen_top - SEL_TOP).max(above);
                let outer = Rect::new(cx - w / 2.0, y, w, name_box.y + name_box.h + SEL_PAD - y);
                // (Unlit: the dark look's light would shade its foot black.)
                grid.rects.push(RectInst {
                    glass: crate::content::UNLIT,
                    ..rect(outer, 9.0, pic_mark)
                });
                if typing_here.is_none() {
                    grid.rects.push(rect(name_box, 4.0, name_mark));
                }
            }
            if let Some(typed) = typing_here {
                // (Centred under the picture, where the name was.)
                typed.draw(
                    &mut grid,
                    name_box.x + NAME_PAD.0,
                    top,
                    (LABEL_FONT_PX, LABEL_LINE_PX),
                    (ground, hair, pal.accent, ink),
                );
                continue;
            }
            if lines.is_empty() {
                continue;
            }
            if covered(Rect::new(cell.x + 6.0, top, cell.w - 12.0, 2.0 * LABEL_LINE_PX)) {
                continue;
            }
            let deep = i >= win.deep_from;
            for (n, line) in lines.iter().enumerate() {
                grid.labels.push(Label {
                    font_px: LABEL_FONT_PX,
                    line_px: LABEL_LINE_PX,
                    ..label(
                        line,
                        (cx, top + n as f32 * LABEL_LINE_PX),
                        label_w,
                        true,
                        faded || (n == 1 && deep),
                    )
                });
            }
        }
        if win.items.is_empty() && !win.loading {
            let words = if win.failed {
                tr("This folder can't be opened")
            } else if win.deep_busy {
                tr("Searching…")
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
            grid.rects.push(rect(band, 3.0, pal.band_fill));
            grid.rects.push(RectInst {
                border: 1.0,
                ..rect(band, 3.0, pal.band_line)
            });
        }
        // (No scroll bars: Max, 2026-10-15, "get rid of the scrollbars".)
        zoom_grid(&mut grid, (layout.body.x, layout.body.y), layout.zoom);
        // (The history stands over the files and words are painted last: a
        // word under it is left out — by the width it is drawn at, so a
        // list's names beside it stay.)
        for panel in [hits.hist, hits.pill].into_iter().filter(|p| p.w > 0.0) {
            if let Some(r) = win.renderer.as_mut() {
                chrome::leave_out(&mut grid.labels, panel, &mut |l| r.measure_text(&l.text, l.font_px, l.family));
            }
        }
        // (The files under the top line, beside the sidebar — as it is drawn
        // now: open over them, nothing of them shows through it.)
        let left = win.field_left().max(win.side_now());
        grid.clip = Rect::new(left, HEAD_H, (w - left).max(0.0), (h - HEAD_H).max(0.0));
        scene.grids.push(grid);
        // (The chrome's words under the menu or the box — the pill's, the top
        // line's, the i card's — are left out as the files' names are: words
        // are painted after every grid and showed through a big menu.)
        let mut chrome = chrome;
        let panel = win
            .menu
            .as_ref()
            .map(|m| Rect::new(m.rect.x, m.rect.y - 4.0 * m.scale, m.rect.w, m.rect.h + 4.0 * m.scale))
            .or(win.props.as_ref().map(|p| p.rect));
        if let (Some(panel), Some(r)) = (panel, win.renderer.as_mut()) {
            for g in chrome.iter_mut() {
                chrome::leave_out(&mut g.labels, panel, &mut |l| r.measure_text(&l.text, l.font_px, l.family));
            }
        }
        scene.grids.extend(chrome);
        win.hits = hits;
        // The menu and the box, over everything.
        if let Some(menu) = win.menu.as_ref() {
            // (A symbol by its name; an app's icon by its own key.)
            let glyph = |name: &str| {
                if name.contains(':') {
                    win.layer_of(name)
                } else {
                    win.layer_of(&chrome::glyph(name))
                }
            };
            menu.push(&mut scene, &self.filer_panel_paint(menu.rect, ground), &glyph);
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
        // (The i card follows the selection: what it says is read anew when
        // that changed.)
        self.filer_info_due(id);
        // (The history up says of each row whether it can still be taken
        // back: read again as it is drawn, at most once a second.)
        if let Some(win) = self.filer.win_mut(id).filter(|w| w.hist_open) {
            if win.hist_read.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                win.hist_read = Some(Instant::now());
                self.filer_history_check(id);
            }
        }
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
            Spot::Up => win.dir.parent().is_some(),
            Spot::Crumb(i) => i + 1 < win.hits.crumbs.len(),
            Spot::Place(_) | Spot::View(_) | Spot::Find | Spot::Col(_) | Spot::Pin | Spot::Sort | Spot::Info => true,
            Spot::Tab(_) | Spot::TabX(_) | Spot::TabAdd | Spot::Pencil => true,
            Spot::Undo | Spot::HistUndo(_) | Spot::Pill(_) | Spot::Here => true,
            Spot::Mode(n) => (n == 1) != win.media,
            Spot::Chrome | Spot::HistRow(_) => false,
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
        // A menu or the Properties box up: only its rows answer the pointer
        // — and only by their band: nothing opens on hover (Max,
        // 2026-10-21: "i dont want the open on hover on the menus").
        if let (Some(menu), Some(at)) = (win.menu.as_mut(), win.ptr) {
            if menu.pointer(Some(at)) {
                self.filer_request_draw(id);
            }
            return;
        }
        if win.props.is_some() {
            return;
        }
        let layout = win.layout(icon_scale);
        let waiting = win.side_wait.is_some();
        let side = win.side_follow();
        let wait = !waiting && win.side_wait.is_some();
        let spot = win.ptr.map_or(Spot::Nothing, |p| win.spot(p, &layout));
        let changed = spot != win.hover || side;
        win.hover = spot;
        if wait {
            self.filer_side_wait(id);
        }
        self.filer_cursor(id);
        // (An icon under the pointer looks no different: only the head's
        // and the places' rows answer it.)
        if changed {
            self.filer_request_draw(id);
        }
        // …but a video or a GIF under it, held there, plays in its tile.
        self.filer_live_hover(id);
    }

    /// The thing under the pointer in window `id`: its path, its name, the
    /// key of the picture its tile wears.
    pub(crate) fn filer_hovered(&self, id: u32) -> Option<(String, String, String)> {
        let win = self.filer.win(id)?;
        let Spot::Item(i) = win.hover else { return None };
        let it = win.items.get(i)?;
        (it.kind == Kind::File).then(|| (it.path.clone(), it.name.clone(), it.icon.clone()))
    }

    /// Put `chain` in the layer that holds picture `key` in window `id` (the
    /// picture stays known by its key); false when no layer holds it.
    pub(crate) fn filer_tile_layer(&mut self, id: u32, key: &str, chain: &[u8]) -> bool {
        let Some(win) = self.filer.win_mut(id) else {
            return false;
        };
        let Some(layer) = win.uploaded.iter().position(|u| u.as_deref() == Some(key)) else {
            return false;
        };
        if let Some(r) = win.renderer.as_mut() {
            r.update_icon_layer(layer as u32, chain);
        }
        self.filer_request_draw(id);
        true
    }

    /// The still picture `key` of window `id`, as it was before it moved.
    pub(crate) fn filer_still(&self, id: u32, key: &str) -> Option<Vec<u8>> {
        self.filer.win(id)?.chains.get(key).cloned()
    }

    /// Route a pointer event on window `id`.
    pub(crate) fn filer_pointer(&mut self, id: u32, event: wl_pointer::Event) {
        let icon_scale = self.icon_scale();
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
                    let raw = (surface_x as f32, surface_y as f32);
                    win.at_win = Some(raw);
                    win.ptr = Some(win.point(raw, icon_scale));
                    win.over_field = win.on_field(raw, icon_scale);
                }
                self.filer_hover(id);
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(win) = self.filer.win_mut(id) {
                    win.ptr = None;
                    win.at_win = None;
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
                    let raw = (surface_x as f32, surface_y as f32);
                    win.at_win = Some(raw);
                    win.ptr = Some(win.point(raw, icon_scale));
                    win.over_field = win.on_field(raw, icon_scale);
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
                    win.finger = axis_source == WEnum::Value(wl_pointer::AxisSource::Finger);
                }
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } => {
                // With Ctrl held the wheel ZOOMS (a notch a step, as the
                // keys): the hand is on it already.
                if self.modifiers.ctrl {
                    let Some(win) = self.filer.win_mut(id) else {
                        return;
                    };
                    win.zoom_run += value as f32;
                    let steps = (win.zoom_run / 15.0).trunc();
                    if steps != 0.0 {
                        win.zoom_run -= steps * 15.0;
                        // (Away from oneself is bigger. A menu up: the menu.)
                        let what = self
                            .filer
                            .win(id)
                            .map_or("files", |w| if w.menu.is_some() { "menu" } else { w.zoom_target() });
                        self.zoom(what, if steps < 0.0 { "in" } else { "out" });
                    }
                    return;
                }
                // (Over a menu that scrolls — the Another app page — its
                // rows; over any menu, nothing under it.)
                if let Some(win) = self.filer.win_mut(id).filter(|w| w.menu.is_some()) {
                    let at = win.ptr;
                    let finger = win.finger;
                    if let Some(menu) = win.menu.as_mut().filter(|m| at.is_some_and(|p| m.rect.contains(p))) {
                        let step = if finger { 1.0 } else { 29.0 * menu.scale / 15.0 };
                        if menu.scroll_by(value as f32 * step) {
                            menu.pointer(None);
                            menu.pointer(at);
                            self.filer_request_draw(id);
                        }
                    }
                    return;
                }
                // (Over the history: its rows, a notch or a row's height of
                // the fingers a row.)
                if let Some(win) = self
                    .filer
                    .win_mut(id)
                    .filter(|w| w.hist_open && w.ptr.is_some_and(|p| w.hits.hist.contains(p)))
                {
                    win.hist_first = (win.hist_first + value as f32 / if win.finger { 41.0 } else { 15.0 }).max(0.0);
                    self.filer_hover(id);
                    return self.filer_request_draw(id);
                }
                // (Over the places: they scroll, not the field.)
                if let Some(win) = self.filer.win_mut(id).filter(|w| w.at_win.is_some_and(|p| w.in_side(p))) {
                    let max = win.side_max();
                    win.side_scroll_to = (win.side_scroll_to + value as f32 * SCROLL_STEP).clamp(0.0, max);
                    if win.finger {
                        win.side_scroll = win.side_scroll_to;
                    }
                    return self.filer_request_draw(id);
                }
                self.filer_scroll(id, value as f32 * SCROLL_STEP, true);
            }
            // The fingers left the touchpad: a fast scroll glides on.
            wl_pointer::Event::AxisStop {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                ..
            } => self.filer_fling(id),
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
        // (The field scrolls in its own space: a step of the fingers moves it
        // as far on screen at any zoom.)
        let by = by / win.zoom;
        if !from_pointer {
            // (A key: a page, eased to.)
            win.gliding = false;
            win.scroll_to = (win.scroll_to + by).clamp(0.0, max);
            self.filer_hover(id);
            return self.filer_request_draw(id);
        }
        // (A glide under way is taken over from where it is.)
        let from = if win.gliding { win.scroll } else { win.scroll_to };
        win.gliding = false;
        win.scroll_run += by.abs();
        let now = Instant::now();
        let dt = win
            .scroll_last
            .map_or(0.1, |last| (now - last).as_secs_f32())
            .clamp(0.004, 0.1);
        win.scroll_vel = 0.55 * win.scroll_vel + 0.45 * (by / dt);
        win.scroll_last = Some(now);
        win.scroll_to = (from + by).clamp(0.0, max);
        // Under fingers the folder moves exactly as they do; a wheel's
        // notch is eased to.
        if win.finger {
            win.scroll = win.scroll_to;
        }
        self.filer_hover(id);
        self.filer_request_draw(id);
    }

    /// The fingers left the touchpad on a scroll: one that was going FAST,
    /// after some travel, keeps going — the folder glides on the same way
    /// and comes to rest, as a page does under a flick.
    fn filer_fling(&mut self, id: u32) {
        let icon_scale = self.icon_scale();
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let far = std::mem::take(&mut win.scroll_run) >= FLING_FAR;
        let fresh = win.scroll_last.is_some_and(|last| last.elapsed() < Duration::from_millis(90));
        let v = std::mem::take(&mut win.scroll_vel);
        win.scroll_last = None;
        if !fresh || !far || v.abs() < FLING_FAST {
            return;
        }
        let max = win.layout(icon_scale).max_scroll(win.items.len());
        win.scroll_to = (win.scroll_to + v * FLING_CARRY).clamp(0.0, max);
        win.gliding = true;
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
                if let Some(row) = chosen {
                    self.filer_menu_row(id, row);
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
        // The history up: a press on it or on its ↶ is theirs (the click
        // below); any other press puts it away — a right one opens the menu
        // there instead.
        if let Some(win) = self.filer.win_mut(id).filter(|w| w.hist_open && pressed) {
            if !win.hits.hist.contains(at) && !win.hits.undo.contains(at) {
                win.hist_open = false;
                self.filer_request_draw(id);
                if button != 0x111 {
                    self.filer_hover(id);
                    return;
                }
            }
        }
        if !pressed {
            // The left button came up with nothing carried off: a band
            // ends where it is; a click on one of a group leaves that one.
            if button == 0x110 {
                if let Some(win) = self.filer.win_mut(id) {
                    let press = win.press.take();
                    let banded = win.band.take().is_some();
                    let then = press.as_ref().filter(|_| !banded).map_or(Then::Nothing, |p| p.then);
                    let at = press
                        .as_ref()
                        .and_then(|p| p.path.as_ref())
                        .and_then(|p| win.items.iter().position(|it| it.path == *p));
                    if let Some(i) = press
                        .filter(|p| p.of_group && !banded)
                        .and_then(|p| p.path)
                        .and_then(|p| win.items.iter().position(|it| it.path == p))
                    {
                        win.select_only(i);
                    }
                    self.filer_request_draw(id);
                    match (then, at) {
                        (Then::Open, Some(i)) => {
                            if let Some(win) = self.filer.win_mut(id) {
                                win.opened = win.items.get(i).map(|it| (Instant::now(), it.path.clone()));
                            }
                            self.filer_activate(id, i);
                        }
                        (Then::Rename, Some(i)) => self.filer_rename_soon(id, i),
                        _ => {}
                    }
                }
            }
            return;
        }
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        // (A name waiting to be typed waits no more: another click came.)
        win.rename_due = None;
        // The Properties box: a press on its Back turns it back into the
        // menu it grew out of; one anywhere else puts it away. (A name being
        // typed is kept.)
        if let Some(props) = win.props.take() {
            let back = props.back_rect().is_some_and(|r| r.contains(at));
            if let (true, Some(mut menu)) = (back, props.back) {
                menu.grow_from = Some(props.rect);
                menu.t = 0.0;
                menu.pressed = None;
                menu.pointer(Some(at));
                win.menu = Some(menu);
            }
            self.filer_hover(id);
            self.filer_request_draw(id);
            return;
        }
        if win.rename.is_some() {
            self.filer_end_rename(id, true);
        }
        if let Some(win) = self.filer.win_mut(id) {
            // A path being typed lets go at a press anywhere but on its own
            // tab (the field; the pencil's click decides for itself).
            let at = win.ptr.unwrap_or_default();
            if win.go_to.is_some() && !win.hits.cur_tab.contains(at) {
                win.go_to = None;
                win.go_bad = false;
                self.filer_request_draw(id);
            }
        }
        match button {
            0x110 => self.filer_click(id, serial),
            0x111 => {
                let icon_scale = self.icon_scale();
                let Some(win) = self.filer.win(id) else {
                    return;
                };
                let layout = win.layout(icon_scale);
                // The ↶'s is its history.
                if win.hits.undo.contains(at) {
                    return self.filer_history_toggle(id);
                }
                // The icons' field has the menu of what is under the
                // pointer; a place of the sidebar, its own.
                if !win.on_chrome(at) {
                    let item = layout.index_at(at, win.scroll, win.items.len());
                    self.filer_open_menu(id, at, item);
                } else if let Some(n) = win.hits.places.iter().position(|r| r.contains(at)) {
                    self.filer_place_menu(id, n, at);
                }
            }
            // The middle button closes the tab it is on.
            0x112 => {
                let Some(win) = self.filer.win(id) else {
                    return;
                };
                let tab = win
                    .hits
                    .tabs
                    .iter()
                    .find(|(r, _)| r.contains(at))
                    .map(|(_, n)| *n)
                    .or_else(|| win.hits.cur_tab.contains(at).then_some(win.tab));
                if let Some(n) = tab {
                    self.filer_tab_close(id, n);
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
            // (Out of the bin a thing goes by "Put back": dragged out it
            // would land under the bin's own name for it, its record lost.)
            Some(_) if win.in_bin() == InBin::Top => win.press = None,
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
        let orderly = win.in_bin() == InBin::No
            && win.dir != recent::root()
            && win.sort.0 == SortKey::Name
            && win.find.as_ref().is_none_or(|f| f.is_empty());
        let hole = orderly.then(|| win.items[..i].iter().filter(|it| !paths.contains(&it.path)).count());
        // The dock comes up for it, as for a desktop icon: its bin is there.
        let dock_raised = self.ui.target() == crate::state::Target::Hidden;
        if dock_raised {
            self.handle_command(waverunner_proto::Command::Show);
        }
        self.filer.drag = Some(Lifted {
            win: id,
            paths,
            source,
            _icon: image,
            action: DndAction::empty(),
            dropped: false,
            dock_raised,
        });
        if let Some(win) = self.filer.win_mut(id) {
            win.gap = hole;
        }
        self.filer_request_draw(id);
        // The dock's bin reacts to it (`bin_motion`): a frame for it to see.
        self.schedule_frame();
    }

    /// Our drag was let go somewhere (its target may read the files for a
    /// long while yet): nothing is in hand any more — every thing is drawn
    /// where it is, so a click lands on what is seen.
    pub(crate) fn filer_drag_dropped(&mut self) {
        self.filer_dock_back();
        self.schedule_frame();
        // (Let go over one of our own windows: that window's drop settles
        // it — marked here first, the things would be drawn back in their
        // old places for a frame.)
        if self.filer.dnd.is_some() {
            return;
        }
        let Some(drag) = self.filer.drag.as_mut() else {
            return;
        };
        drag.dropped = true;
        let ids: Vec<u32> = self.filer.wins.iter().map(|w| w.id).collect();
        for id in ids {
            self.filer_request_draw(id);
        }
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
    /// The dock came up for a drag that is let go: it goes again.
    fn filer_dock_back(&mut self) {
        if let Some(drag) = self.filer.drag.as_mut().filter(|d| d.dock_raised) {
            drag.dock_raised = false;
            self.handle_command(waverunner_proto::Command::Hide);
        }
    }

    pub(crate) fn filer_drag_end(&mut self, done: bool) {
        self.filer_dock_back();
        self.schedule_frame();
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
                // (Not the room something is still arriving into.)
                if !w.gap_keep {
                    w.gap = None;
                }
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
        let layout = win.layout(icon_scale);
        let at_win = (x, y);
        win.at_win = Some(at_win);
        // THE SIDEBAR OPENS AT ONCE under a drag (Max, 2026-10-21: "the dirs
        // open when i drag something on top"), and folds when it leaves it.
        let mut side_moved = win.side_follow();
        if win.side_wait.take().is_some() {
            win.side_open = true;
            side_moved = true;
        }
        let at = if win.in_side(at_win) { at_win } else { layout.inward(at_win) };
        // Over the places: onto that place (the bin among them).
        let place = win.hits.places.iter().position(|r| r.contains(at));
        // SPRING-LOADED PLACES: held on a folder place, the window opens it
        // in this tab — after `SPRING`, a blink first (`filer_place_spring`).
        // Not Recents (nothing goes in it), not the bin (a drop there
        // throws away), not `..` (it climbs: `up_hold`), not the folder
        // shown.
        let springy = place.filter(|&n| {
            win.places.get(n).is_some_and(|p| {
                p.name != UP && p.path != recent::root() && !p.path.starts_with(bin_root()) && p.path != win.dir
            })
        });
        let new_spring = (springy != win.place_spring.map(|(n, _)| n)).then_some(springy).flatten();
        if springy != win.place_spring.map(|(n, _)| n) {
            win.place_spring = springy.map(|n| (n, Instant::now()));
            win.place_blink = None;
        }
        // On `..`: held there, the window climbs (`filer_up_tick`).
        let on_up = place.and_then(|n| win.places.get(n)).is_some_and(|p| p.name == UP);
        let climb = on_up && win.up_hold.is_none();
        if !on_up {
            win.up_hold = None;
        } else if climb {
            win.up_hold = Some(UpHold { since: Instant::now(), due: SPRING, ups: 0 });
        }
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
            && win.dir != recent::root()
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
        // Onto another folder: the hold on it starts now.
        let sprung = (onto != win.drop_on).then_some(onto).flatten();
        if onto != win.drop_on {
            win.spring = onto.map(|i| (i, Instant::now()));
        }
        if onto != win.drop_on || place != win.drop_place || gap != win.gap {
            win.drop_on = onto;
            win.drop_place = place;
            win.gap = gap;
            self.filer_request_draw(id);
        }
        if side_moved {
            self.filer_request_draw(id);
        }
        if let Some(n) = new_spring {
            self.filer_place_spring(id, n);
        }
        if scrolls {
            self.filer_edge_tick(id);
        }
        if climb {
            self.filer_up_tick(id);
        }
        // Held there `SPRING`, the folder opens under the drag (macOS's
        // spring-loaded folders; the dock's tiles do the same).
        if let Some(i) = sprung {
            let timer = calloop::timer::Timer::from_duration(SPRING);
            let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
                let open = app.filer.win(id).and_then(|w| {
                    let (on, since) = w.spring?;
                    (on == i && w.drop_on == Some(i) && since.elapsed() >= SPRING - Duration::from_millis(30))
                        .then(|| w.items.get(i).map(|it| PathBuf::from(&it.path)))
                        .flatten()
                });
                if let Some(dir) = open.filter(|_| app.filer.dnd_active() || app.filer.rig_drag) {
                    info!("files: a drag held on {} opens it", dir.display());
                    if let Some(win) = app.filer.win_mut(id) {
                        // (Only the first: a drag through three folders goes
                        // back to where it began.)
                        if win.sprung_from.is_none() {
                            win.sprung_from = Some((win.dir.clone(), win.back.len()));
                        }
                    }
                    app.filer_go(id, dir);
                }
                calloop::timer::TimeoutAction::Drop
            });
        }
    }

    /// A drag held on place `n` of window `id`: once it has been held there
    /// `SPRING`, the place BLINKS (its box goes, `BLINK`) and then opens in
    /// this tab — the drag goes on over its files, to be held on a folder
    /// there or let go. As the files' own spring, the window goes back to
    /// where the drag began if it leaves without being let go.
    fn filer_place_spring(&mut self, id: u32, n: usize) {
        const BLINK: Duration = Duration::from_millis(110);
        let held = move |app: &App| {
            app.filer.win(id).is_some_and(|w| {
                w.place_spring.is_some_and(|(m, since)| m == n && since.elapsed() >= SPRING - Duration::from_millis(30))
                    && w.drop_place == Some(n)
                    && (app.filer.dnd_active() || app.filer.rig_drag)
            })
        };
        let timer = calloop::timer::Timer::from_duration(SPRING);
        let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            if !held(app) {
                return calloop::timer::TimeoutAction::Drop;
            }
            if let Some(win) = app.filer.win_mut(id) {
                win.place_blink = Some(n);
            }
            app.filer_request_draw(id);
            let blink = calloop::timer::Timer::from_duration(BLINK);
            let _ = app.loop_handle.insert_source(blink, move |_, _, app: &mut App| {
                let ok = held(app);
                let Some(win) = app.filer.win_mut(id) else {
                    return calloop::timer::TimeoutAction::Drop;
                };
                win.place_blink = None;
                let to = win.places.get(n).map(|p| p.path.clone()).filter(|_| ok);
                if let Some(dir) = to {
                    info!("files: a drag held on the place {} opens it", dir.display());
                    // (Only the first: a drag through three folders goes back
                    // to where it began.)
                    if win.sprung_from.is_none() {
                        win.sprung_from = Some((win.dir.clone(), win.back.len()));
                    }
                    win.place_spring = None;
                    app.filer_go(id, dir);
                } else {
                    app.filer_request_draw(id);
                }
                calloop::timer::TimeoutAction::Drop
            });
            calloop::timer::TimeoutAction::Drop
        });
    }

    /// A drag held on `..` of window `id`: a level up once it has been held
    /// `SPRING`, another after `UP_SECOND`, then one every `UP_NEXT` — for as
    /// long as it stays there (`Win::up_hold`). The window stays where it
    /// climbed to: going up is choosing where to let go, not a peek.
    fn filer_up_tick(&mut self, id: u32) {
        let tick = Duration::from_millis(100);
        let _ = self.loop_handle.insert_source(calloop::timer::Timer::from_duration(tick), move |_, _, app: &mut App| {
            let state = app
                .filer
                .win(id)
                .filter(|_| app.filer.dnd_active())
                .and_then(|w| w.up_hold.map(|h| (h, w.dir.parent().map(Path::to_path_buf))));
            let Some((hold, parent)) = state else {
                return calloop::timer::TimeoutAction::Drop;
            };
            if hold.since.elapsed() < hold.due {
                return calloop::timer::TimeoutAction::ToDuration(tick);
            }
            let Some(parent) = parent else {
                // (The root: nowhere further.)
                if let Some(w) = app.filer.win_mut(id) {
                    w.up_hold = None;
                }
                return calloop::timer::TimeoutAction::Drop;
            };
            if let Some(w) = app.filer.win_mut(id) {
                let due = if hold.ups == 0 { UP_SECOND } else { UP_NEXT };
                w.up_hold = Some(UpHold { since: Instant::now(), due, ups: hold.ups + 1 });
                if w.sprung_from.is_none() {
                    w.sprung_from = Some((w.dir.clone(), w.back.len()));
                }
            }
            info!("files: a drag held on .. climbs to {}", parent.display());
            app.filer_go(id, parent);
            calloop::timer::TimeoutAction::ToDuration(tick)
        });
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
        // (The drag went elsewhere: a window it sprang open goes back.)
        if let Some(id) = self.filer.dnd.as_ref().map(|d| d.win) {
            if let Some(w) = self.filer.win_mut(id) {
                w.up_hold = None;
            }
            self.filer_spring_back(id);
        }
        if let Some(dnd) = self.filer.dnd.take() {
            if let Some(win) = self.filer.win_mut(dnd.win) {
                win.drop_on = None;
                win.drop_place = None;
                win.gap = None;
                win.edge = 0.0;
                // (The sidebar a drag opened folds with it.)
                win.place_spring = None;
                win.place_blink = None;
                win.side_open = false;
            }
            self.filer_request_draw(dnd.win);
        }
    }

    /// Let go on a window: into the folder under the pointer, else into
    /// the folder shown. The list of files is asked of the source and read
    /// off the loop — whoever the source is, a window of ours included.
    pub(crate) fn filer_dnd_drop(&mut self) {
        let back = self.filer.dnd.as_ref().map(|d| d.win);
        if let Some(w) = back.and_then(|id| self.filer.win_mut(id)) {
            w.up_hold = None;
        }
        self.filer_dnd_drop_in();
        // (Let go in a folder the drag took the window to: it stays there,
        // what was let go selected once it has come, and its way back is the
        // folder the drag began in — not every folder it passed through.)
        if let Some(w) = back.and_then(|id| self.filer.win_mut(id)) {
            if let Some((_, n)) = w.sprung_from.take() {
                w.back.truncate(n + 1);
                w.forward.clear();
            }
        }
        // (The bar's arrows say what the way back is now.)
        self.filer_note_nav();
    }

    /// A drag that took window `id` elsewhere left without a drop: back to
    /// where it began, its way back as it was.
    fn filer_spring_back(&mut self, id: u32) {
        let Some((from, n)) = self.filer.win_mut(id).and_then(|w| w.sprung_from.take()) else {
            return;
        };
        info!("files: back to {} after the drag", from.display());
        if let Some(w) = self.filer.win_mut(id) {
            w.back.truncate(n);
            w.forward.clear();
        }
        self.filer_show(id, from);
    }

    fn filer_dnd_drop_in(&mut self) {
        let Some(dnd) = self.filer.dnd.take() else {
            return;
        };
        let id = dnd.win;
        let carried = self.filer.drag_paths();
        let from_here = self.filer.drag.as_ref().is_some_and(|d| d.win == id);
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
        // THINGS OF THIS FOLDER put in another place of it (Max, 2026-10-14:
        // "cuando suelto un icono en el espacio que creó otro, no cae ahí:
        // vuelve a su lugar y vuelve a salir"): nothing is moved on the
        // disk, so nothing is read or awaited — they land in the room made,
        // now, and the folder's order is that from here on.
        let reordered = from_here
            && dir == win.dir
            && !carried.is_empty()
            && carried.iter().all(|p| Path::new(p).parent() == Some(dir.as_path()));
        // (The gap is taken only for this: another drop still needs it.)
        let gap = if reordered { win.gap.take() } else { None };
        if let Some(gap) = gap {
            let names = |items: &[Item]| -> Vec<String> { items.iter().map(|it| file_name(Path::new(&it.path))).collect() };
            let was = names(&win.items);
            let (items, slide) = settled(std::mem::take(&mut win.items), &win.slide, &carried, gap);
            win.items = items;
            win.slide = slide;
            win.all = win.items.clone();
            win.names.1.clear();
            win.cursor = win.items.iter().position(|it| it.path == carried[0]);
            win.anchor = win.cursor;
            win.edge = 0.0;
            win.drop_on = None;
            let order = names(&win.items);
            info!("files: {} thing(s) put in another place of {}", carried.len(), dir.display());
            // (Let go where it already was: the folder is not "arranged by
            // hand" for that.)
            if order != was {
                self.filer.orders.set(&dir, Some(order));
            }
            // In hand no more: drawn where it landed.
            if let Some(drag) = self.filer.drag.as_mut() {
                drag.dropped = true;
            }
            dnd.offer.finish(); // → the source's `dnd_finished` → `filer_drag_end`
            self.filer_request_draw(id);
            return;
        }
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
        // Nothing is put in Recents: it is a list of what was used.
        if dir == recent::root() {
            info!("files: nothing goes into Recents");
            then();
            return;
        }
        // Into the bin (its window, or its place): thrown away, not moved
        // into its folder — the bin keeps a record of where each came from.
        if dir.starts_with(bin_root()) {
            let out: Vec<PathBuf> = paths.into_iter().filter(|p| !p.starts_with(bin_root())).collect();
            self.filer_bin(id, &out);
            then();
            self.filer_list(id);
            return;
        }
        let into = dir.clone();
        let mut then = Some(then);
        // What comes stands where it was let go — said BEFORE it arrives, so
        // the folder's own watch lists it into the room made (not at the
        // end first, to jump there after); the room stays open meanwhile.
        let order_was = self.filer.orders.of(&dir).cloned();
        if let Some((before, after)) = arrange.as_ref() {
            let coming: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
            let mut order: Vec<String> = before.iter().filter(|n| !coming.contains(n)).cloned().collect();
            order.extend(coming.iter().cloned());
            order.extend(after.iter().filter(|n| !coming.contains(n)).cloned());
            self.filer.orders.set(&dir, Some(order));
            if let Some(win) = self.filer.win_mut(id) {
                win.gap_keep = win.gap.is_some();
            }
        }
        // (Across disks a drop is a copy: what was dragged stays where it was.)
        let sources = paths.clone();
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
                for undo in undo_of(&sources, &brought) {
                    app.filer_did(id, undo);
                }
                info!("files: {} thing(s) brought into {}", brought.len(), dir.display());
                // A copy (to a stick, say) is said in the pill.
                if !may_move && !brought.is_empty() {
                    let to = app
                        .desktop
                        .volumes
                        .iter()
                        .find(|v| v.path == dir)
                        .map_or_else(|| file_name(&dir), |v| v.name.clone());
                    app.filer_flash(id, Self::copied_words(&brought, &to));
                }
                if let Some(then) = then.take() {
                    then();
                }
                // What came stands where it was let go — under the names it
                // came to have (one already taken got its number); nothing
                // came: the folder's order is what it was.
                if let Some((before, after)) = arrange.clone() {
                    if brought.is_empty() {
                        app.filer.orders.set(&dir, order_was.clone());
                    } else {
                        let came: Vec<String> = brought.iter().map(|p| file_name(p)).collect();
                        let mut order: Vec<String> = before.into_iter().filter(|n| !came.contains(n)).collect();
                        order.extend(came.iter().cloned());
                        order.extend(after.into_iter().filter(|n| !came.contains(n)));
                        app.filer.orders.set(&dir, Some(order));
                    }
                }
                if let Some(win) = app.filer.win_mut(id) {
                    win.gap = None;
                    win.gap_keep = false;
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
        let icon_scale = self.icon_scale();
        let (ctrl, shift) = (self.modifiers.ctrl, self.modifiers.shift);
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        let at = win.ptr.unwrap_or_default();
        let layout_top = if win.list { LIST_TOP } else { HEAD_H };
        // What is under the pointer NOW: the folder may have scrolled under
        // it since it last moved (a wheel's eased notch, a walk by keys).
        let layout = win.layout(icon_scale);
        win.hover = win.ptr.map_or(Spot::Nothing, |p| win.spot(p, &layout));
        // (An empty search field is put away by a click elsewhere.)
        if win.find.as_deref() == Some("") && win.hover != Spot::Find && !win.hits.seek.contains(at) {
            self.filer_find(id, None);
            return;
        }
        match win.hover {
            Spot::Up => self.filer_up(id),
            Spot::Mode(n) => {
                // The other mode: the folder shows anew.
                if (n == 1) != win.media {
                    win.media = n == 1;
                    info!("files: window {id} in {} mode", if win.media { "media" } else { "files" });
                    self.filer_remode(id);
                }
            }
            Spot::Pin => {
                let pinned = !win.pinned;
                self.filer_pin(pinned);
            }
            Spot::Tab(n) => self.filer_tab_take(id, n),
            Spot::TabX(n) => self.filer_tab_close(id, n),
            Spot::TabAdd => self.filer_tab_new(id),
            Spot::Pencil => {
                // The way here as a path to type in, the tab's width; the
                // pencil again puts it back.
                if win.go_to.take().is_none() {
                    win.edit_w = win.hits.cur_tab.w;
                    win.go_to = Some(win.typed_here());
                }
                win.go_bad = false;
                self.filer_request_draw(id);
            }
            Spot::Sort => {
                if win.menu.is_some() {
                    win.menu = None;
                    win.menu_path = None;
                    self.filer_request_draw(id);
                } else {
                    self.filer_sort_menu(id);
                }
            }
            Spot::Info => {
                win.info_on = !win.info_on;
                self.filer_info_due(id);
                self.filer_hover(id);
                self.filer_request_draw(id);
            }
            // The ↶ takes back the newest that can be; a row's Undo, its own.
            Spot::Undo => self.filer_undo(id),
            Spot::Here => {
                if let Some((_, action)) = win.hits.here {
                    self.filer_smart_act(id, action);
                }
            }
            Spot::Pill(n) => {
                if let Some(act) = win.hits.pill_btns.get(n).map(|(_, a)| a.clone()) {
                    self.filer_pill_act(id, act);
                }
            }
            Spot::HistUndo(k) => self.filer_undo_at(id, k),
            Spot::Chrome | Spot::HistRow(_) => win.click = None,
            Spot::Crumb(n) => {
                if let Some((_, path)) = win.hits.crumbs.get(n).cloned() {
                    self.filer_go(id, path);
                }
            }
            Spot::Place(n) => match win.places.get(n).map(|p| (p.name == UP, p.path.clone())) {
                // (`..`: out of this folder, which is left selected.)
                Some((true, _)) => self.filer_up(id),
                Some((false, path)) => self.filer_go(id, path),
                None => {}
            },
            Spot::Item(i) => {
                let Some(path) = win.items.get(i).map(|it| it.path.clone()) else {
                    return;
                };
                let now = Instant::now();
                let twice = win
                    .click
                    .take()
                    .is_some_and(|(at, p)| p == path && now.duration_since(at) <= DOUBLE_CLICK);
                // (A second click that opened it already is not, with this
                // one, a double click that opens it again.)
                let opened = win
                    .opened
                    .as_ref()
                    .is_some_and(|(t, p)| *p == path && now.duration_since(*t) <= DOUBLE_CLICK);
                if twice && !ctrl && !opened {
                    self.filer_activate(id, i);
                    return;
                }
                win.click = Some((now, path.clone()));
                // A SECOND click on the one thing selected (Finder's way, Max
                // 2026-10-21): on its picture it opens, on its name the name is
                // typed — both when let go without travel (`filer_button`).
                let only = !ctrl && !shift && !opened && win.selected.len() == 1 && win.selected.contains(&path);
                let then = match (only, win.part_at(i, at, &layout, icon_scale)) {
                    (true, Part::Picture) => Then::Open,
                    (true, Part::Name) => Then::Rename,
                    _ => Then::Nothing,
                };
                // One of several selected: the group stays for a drag, and
                // the release decides (see `filer_button`).
                let of_group = !ctrl && !shift && win.selected.len() > 1 && win.selected.contains(&path);
                if shift {
                    // From where the selection began to here.
                    win.select_to(i, ctrl);
                } else if ctrl {
                    // One more, or one fewer.
                    if !win.selected.remove(&path) {
                        win.selected.insert(path.clone());
                    }
                    win.cursor = Some(i);
                    win.anchor = Some(i);
                } else if !of_group {
                    win.select_only(i);
                }
                win.press = Some(Press {
                    path: Some(path),
                    at,
                    serial,
                    of_group,
                    then,
                });
                self.filer_request_draw(id);
            }
            Spot::View(list) => self.filer_set_list(list),
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
                if at.1 > layout_top && !win.on_chrome(at) {
                    win.press = Some(Press {
                        path: None,
                        at,
                        serial,
                        of_group: false,
                        then: Then::Nothing,
                    });
                }
                if !ctrl && !win.selected.is_empty() {
                    win.selected.clear();
                    self.filer_request_draw(id);
                }
            }
        }
    }

    // ── Quick Look ──────────────────────────────────────────────────────

    /// What the keys are on in window `id`: the one Quick Look shows.
    fn filer_peek_at(&self, id: u32) -> Option<usize> {
        let win = self.filer.win(id)?;
        win.cursor
            .filter(|&i| win.items.get(i).is_some_and(|it| win.selected.contains(&it.path)))
            .or_else(|| win.items.iter().position(|it| win.selected.contains(&it.path)))
    }

    /// Thing `i` of window `id`, with its icon where it is on the screen.
    fn filer_peek_thing(&self, id: u32, i: usize) -> Option<crate::quicklook::Thing> {
        let icon_scale = self.icon_scale();
        let win = self.filer.win(id)?;
        let item = win.items.get(i)?;
        let layout = win.layout(icon_scale);
        let icon = layout.icon_in(&layout.cell(i), icon_scale);
        let (x, y) = layout.outward((icon.x, icon.y - win.scroll));
        let side = icon.w * layout.zoom;
        // The window's corner on ITS screen (it has the keyboard: it is the
        // active one; the screen with the focus is the pointer's).
        let screen = self.filer.screens.get(&id).and_then(|o| self.output_state.info(o)).and_then(|i| i.name);
        let origin = crate::hypr::active_window_geom()
            .zip(crate::hypr::monitor_info(screen.as_deref()).ok())
            .map_or((0.0, 0.0), |((wx, wy, _, _), m)| (wx as f32 - m.x as f32, wy as f32 - m.y as f32));
        // (One of several selected: its place among them.)
        let of = (win.selected.len() > 1 && win.selected.contains(&item.path)).then(|| {
            let before = win.items[..i].iter().filter(|it| win.selected.contains(&it.path)).count();
            (before + 1, win.selected.len())
        });
        Some(crate::quicklook::Thing {
            path: PathBuf::from(&item.path),
            name: item.name.clone(),
            thumb: win.chains.get(&item.icon).cloned(),
            icon: Rect::new(origin.0 + x, origin.1 + y, side, side),
            of,
        })
    }

    /// With Quick Look up on one of SEVERAL selected, the arrows walk among
    /// those only, round (Finder's way); the selection stays.
    fn filer_peek_walk(&mut self, id: u32, keysym: Keysym) -> bool {
        let by: isize = match keysym {
            Keysym::Left | Keysym::Up => -1,
            Keysym::Right | Keysym::Down => 1,
            _ => return false,
        };
        if self.modifiers.shift || self.modifiers.alt || !self.peek.up.as_ref().is_some_and(|u| u.win == id && !u.closing) {
            return false;
        }
        let icon_scale = self.icon_scale();
        let Some(win) = self.filer.win_mut(id) else {
            return false;
        };
        if win.selected.len() < 2 {
            return false;
        }
        let picked: Vec<usize> = (0..win.items.len()).filter(|&i| win.selected.contains(&win.items[i].path)).collect();
        let Some(k) = win.cursor.and_then(|c| picked.iter().position(|&i| i == c)) else {
            return false;
        };
        let to = picked[(k as isize + by).rem_euclid(picked.len() as isize) as usize];
        win.cursor = Some(to);
        win.gliding = false;
        win.scroll_to = win.layout(icon_scale).reveal(to, win.scroll_to, win.items.len());
        self.filer_request_draw(id);
        self.filer_peek_open(id);
        true
    }

    /// Space: Quick Look on what the keys are on.
    fn filer_peek_open(&mut self, id: u32) {
        let Some(i) = self.filer_peek_at(id) else { return };
        if let Some(thing) = self.filer_peek_thing(id, i) {
            self.peek_show(id, thing);
        }
    }

    /// The keys walked with Quick Look up: it shows where they went.
    fn filer_peek_follow(&mut self, id: u32) {
        if self.peek.up.as_ref().is_some_and(|u| u.win == id) {
            self.filer_peek_open(id);
        }
    }

    /// What the keys are on and the things round it, to be prepared (that
    /// one last: it is taken first).
    fn filer_peek_near(&self, id: u32) -> Option<(String, Vec<PathBuf>)> {
        let i = self.filer_peek_at(id)?;
        let win = self.filer.win(id)?;
        let cols = win.layout(self.icon_scale()).cols as isize;
        let near = [-cols, cols, -1, 1, 0]
            .into_iter()
            .filter_map(|d| usize::try_from(i as isize + d).ok())
            .filter_map(|j| win.items.get(j))
            .filter(|it| it.kind == Kind::File)
            .map(|it| PathBuf::from(&it.path))
            .collect();
        Some((win.items[i].path.clone(), near))
    }

    // ── the keyboard ────────────────────────────────────────────────────

    /// Put the keys on item `i` (it alone selected) and bring it into view.
    fn filer_walk_to(&mut self, id: u32, i: usize) {
        let icon_scale = self.icon_scale();
        // (With Shift the selection stretches to where the keys go.)
        let shift = self.modifiers.shift;
        let Some(win) = self.filer.win_mut(id) else {
            return;
        };
        if i >= win.items.len() {
            return;
        }
        if shift {
            win.select_to(i, false);
        } else {
            win.select_only(i);
        }
        win.gliding = false;
        win.scroll_to = win.layout(icon_scale).reveal(i, win.scroll_to, win.items.len());
        self.filer_request_draw(id);
    }

    /// A key while window `id` has the keyboard.
    pub(crate) fn filer_key(&mut self, id: u32, keysym: Keysym, utf8: Option<&str>) {
        // QUICK LOOK up for this window: its keys first (the arrows walk on
        // here, and it follows them).
        if self.peek_key(id, keysym, utf8) || self.filer_peek_walk(id, keysym) {
            return;
        }
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
        if win.hist_open && keysym == Keysym::Escape {
            win.hist_open = false;
            self.filer_hover(id);
            return self.filer_request_draw(id);
        }
        if win.menu.is_some() || win.props.is_some() {
            // (Ctrl +/−/0 make the MENU bigger or smaller, not the files.)
            let menu = win.menu.is_some() && ctrl;
            match keysym {
                Keysym::Escape => {
                    win.menu = None;
                    win.menu_path = None;
                    win.props = None;
                    self.filer_hover(id);
                    self.filer_request_draw(id);
                }
                Keysym::plus | Keysym::equal | Keysym::KP_Add if menu => {
                    self.zoom("menu", "in");
                }
                Keysym::minus | Keysym::KP_Subtract if menu => {
                    self.zoom("menu", "out");
                }
                Keysym::_0 | Keysym::KP_0 if menu => {
                    self.zoom("menu", "reset");
                }
                // THE KEYS IN THE MENU (Max, 2026-10-21): walk, choose,
                // turn its page and back, a letter, or what is typed for
                // its search (`Menu::key`).
                _ if !ctrl && !alt => {
                    let at = win.ptr;
                    let Some(m) = win.menu.as_mut() else {
                        return;
                    };
                    match m.key(keysym, utf8, at) {
                        MenuKey::Moved => self.filer_request_draw(id),
                        MenuKey::Choose(row) => {
                            self.filer_menu_row(id, row);
                        }
                        MenuKey::Back => self.filer_menu_back(id),
                        MenuKey::Nothing | MenuKey::Close => {}
                    }
                }
                _ => {}
            }
            return;
        }
        // Shift+F10 or the Menu key: the right-click's menu, on the
        // selection (its first thing) or on the folder.
        if keysym == Keysym::Menu || (keysym == Keysym::F10 && self.modifiers.shift) {
            let first = win.items.iter().position(|it| win.selected.contains(&it.path));
            return self.filer_menu_from_keys(id, first);
        }
        // THE LEFT HAND (Max, 2026-10-13): with Alt held, W A S D walk as
        // the arrows do, E is Enter and Q goes back — the mouse hand stays
        // on the mouse. (They are keys then, not letters: nothing is typed.)
        let (keysym, alt, utf8) = if alt {
            match keysym {
                Keysym::w | Keysym::W => (Keysym::Up, false, None),
                Keysym::a | Keysym::A => (Keysym::Left, false, None),
                Keysym::s | Keysym::S => (Keysym::Down, false, None),
                Keysym::d | Keysym::D => (Keysym::Right, false, None),
                // (E is Enter: it opens what the keys are on.)
                Keysym::e | Keysym::E => (Keysym::Return, false, None),
                Keysym::q | Keysym::Q => return self.filer_back(id),
                other => (other, true, utf8),
            }
        } else {
            (keysym, alt, utf8)
        };
        // A path being typed takes every key: Enter goes, Tab completes,
        // Escape leaves.
        if let Some(text) = win.go_to.as_mut() {
            win.go_bad = false;
            match keysym {
                Keysym::Return | Keysym::KP_Enter => return self.filer_go_typed(id),
                Keysym::Escape => win.go_to = None,
                Keysym::BackSpace => {
                    text.pop();
                }
                Keysym::Tab => *text = complete_path(text, &home()),
                _ => {
                    if let Some(t) = utf8.filter(|t| !ctrl && !alt && !t.is_empty() && !t.chars().any(char::is_control)) {
                        text.push_str(t);
                    }
                }
            }
            return self.filer_request_draw(id);
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
                    self.filer_peek_follow(id);
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
                if !win.selected.is_empty() {
                    win.selected.clear();
                    self.filer_request_draw(id);
                }
            }
            Keysym::F5 => self.filer_list(id),
            Keysym::z | Keysym::Z if ctrl => self.filer_undo(id),
            Keysym::l | Keysym::L if ctrl => {
                // The way here, as a path to be typed over (the pencil's).
                let mut text = win.typed_here();
                if !text.ends_with('/') {
                    text.push('/');
                }
                win.edit_w = win.hits.cur_tab.w;
                win.go_to = Some(text);
                self.filer_request_draw(id);
            }
            Keysym::f | Keysym::F if ctrl => {
                let open = win.find.is_none();
                self.filer_find(id, open.then(String::new));
            }
            Keysym::_1 if ctrl => self.filer_set_list(false),
            Keysym::_2 if ctrl => self.filer_set_list(true),
            // (What the pointer is on: the field or the panel.)
            Keysym::plus | Keysym::equal | Keysym::KP_Add if ctrl => {
                let what = win.zoom_target();
                self.zoom(what, "in");
            }
            Keysym::minus | Keysym::KP_Subtract if ctrl => {
                let what = win.zoom_target();
                self.zoom(what, "out");
            }
            Keysym::_0 | Keysym::KP_0 if ctrl => {
                let what = win.zoom_target();
                self.zoom(what, "reset");
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
            // (A tab, while there are others; the window with the last.)
            Keysym::w if ctrl && win.tabs.len() > 1 => {
                let n = win.tab;
                self.filer_tab_close(id, n);
            }
            Keysym::w if ctrl => self.filer_close(id),
            Keysym::t if ctrl => self.filer_tab_new(id),
            Keysym::Tab | Keysym::ISO_Left_Tab if ctrl && win.tabs.len() > 1 => {
                let n = win.tabs.len();
                let back = keysym == Keysym::ISO_Left_Tab || self.modifiers.shift;
                let to = if back { (win.tab + n - 1) % n } else { (win.tab + 1) % n };
                self.filer_tab_take(id, to);
            }
            // QUICK LOOK (Max, 2026-10-16: "press space on a file").
            Keysym::space if !ctrl && !alt => self.filer_peek_open(id),
            _ => {
                // TYPING SEARCHES (Max, 2026-10-13: "¿por qué no empieza a
                // buscar con solo typear?" — the clipboard box's way): the
                // first letter brings the field out with it in.
                let Some(text) = utf8.filter(|t| !ctrl && !alt && t.chars().all(|c| !c.is_control())) else {
                    return;
                };
                if text.trim().is_empty() {
                    return;
                }
                // (A `/` or a `~` begins a path, not a name.)
                if text == "/" || text == "~" {
                    win.edit_w = win.hits.cur_tab.w;
                    win.go_to = Some(text.to_owned());
                    return self.filer_request_draw(id);
                }
                self.filer_find(id, Some(text.to_owned()));
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
                // (`go recents`: the place of no folder.)
                let dir = if rest == "recents" { recent::root() } else { as_path(rest) };
                if !dir.is_dir() && dir != recent::root() {
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
                let Some(win) = self.filer.win(id) else {
                    return "closed".to_owned();
                };
                let item = rest.parse::<usize>().ok().filter(|&i| i < win.items.len());
                self.filer_menu_from_keys(id, item);
            }
            // `pick <row>`: as a click on that row of the menu that is up (a
            // page turns, the menu stays; anything else is done).
            "pick" => {
                let Some(row) = rest.trim().parse::<usize>().ok() else {
                    return "pick <row>".to_owned();
                };
                let said = self.filer_menu_row(id, row);
                info!("files: pick {row}: {said}");
                return said;
            }
            // `here`: as a click on the folder's own action by the ↶.
            "here" => {
                if let Some((_, action)) = self.filer.win(id).and_then(|w| w.hits.here) {
                    self.filer_smart_act(id, action);
                }
            }
            // `flash <words>`: said in the pill for a moment.
            "flash" => self.filer_flash(id, rest.to_owned()),
            "printers" => {
                self.filer_printers(true);
                return format!("{:?} (asking again)", self.filer.printers.can);
            }
            // `pill <n>`: as a click on the pill's `n`th button.
            "pill" => {
                let act = rest
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| self.filer.win(id)?.hits.pill_btns.get(n).map(|(_, a)| a.clone()));
                match act {
                    Some(act) => self.filer_pill_act(id, act),
                    None => return "pill <n>".to_owned(),
                }
            }
            // `rows` logs the rows of the menu that is up.
            "rows" => {
                let said = self.filer.win(id).and_then(|w| w.menu.as_ref()).map_or_else(
                    || "no menu is up".to_owned(),
                    |m| {
                        m.rows
                            .iter()
                            .enumerate()
                            .map(|(i, r)| match r {
                                Row::Item { label, off, .. } => format!("{i}: {label}{}", if *off { " (off)" } else { "" }),
                                Row::Note(t) => format!("{i}: ({t})"),
                                Row::Search(t) => format!("{i}: [{t}]"),
                                Row::Sep => format!("{i}: —"),
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    },
                );
                info!("files: menu rows:\n{said}");
                return said;
            }
            // `place <n>`: the menu of place n of the sidebar.
            "placemenu" => {
                if let Ok(n) = rest.trim().parse::<usize>() {
                    let at = self.filer.win(id).and_then(|w| w.hits.places.get(n).map(|r| (r.x + 20.0, r.y + r.h / 2.0))).unwrap_or((40.0, 100.0));
                    self.filer_place_menu(id, n, at);
                }
            }
            // (With a menu up on a file: its "Open with" page, and back.)
            "apps" => self.filer_menu_apps(id),
            "menuback" => self.filer_menu_back(id),
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
                // (`key ctrl <name>` / `key shift <name>`: with it held.)
                let (ctrl, rest) = rest.strip_prefix("ctrl ").map_or((false, rest), |r| (true, r));
                let (shift, rest) = rest.strip_prefix("shift ").map_or((false, rest), |r| (true, r));
                let key = match rest {
                    "left" => Keysym::Left,
                    "right" => Keysym::Right,
                    "up" => Keysym::Up,
                    "down" => Keysym::Down,
                    "enter" => Keysym::Return,
                    "back" => Keysym::BackSpace,
                    "escape" => Keysym::Escape,
                    "space" => Keysym::space,
                    "pgdown" => Keysym::Page_Down,
                    "pgup" => Keysym::Page_Up,
                    "p" => Keysym::p,
                    "plus" => Keysym::plus,
                    "minus" => Keysym::minus,
                    "zero" => Keysym::_0,
                    "home" => Keysym::Home,
                    "end" => Keysym::End,
                    "f2" => Keysym::F2,
                    "w" => Keysym::w,
                    "a" => Keysym::a,
                    "s" => Keysym::s,
                    "d" => Keysym::d,
                    "e" => Keysym::e,
                    "q" => Keysym::q,
                    "f10" => Keysym::F10,
                    "menukey" => Keysym::Menu,
                    _ => return format!("no key {rest}"),
                };
                let held = (self.modifiers.ctrl, self.modifiers.shift);
                self.modifiers.ctrl |= ctrl;
                self.modifiers.shift |= shift;
                self.filer_key(id, key, None);
                (self.modifiers.ctrl, self.modifiers.shift) = held;
            }
            "type" => self.filer_key(id, Keysym::NoSymbol, Some(rest)),
            "zoom" => return self.zoom("files", rest),
            "panel" => return self.zoom("files-panel", rest),
            "where" => {
                // `where item <n>|place <n>|crumb <n>|up|view|find|field`: the
                // middle of that part, in the window's own pixels (what a
                // pointer on the screen is moved to, from the window's
                // corner) — for tests with a real pointer.
                let icon_scale = self.icon_scale();
                let Some(win) = self.filer.win(id) else {
                    return "closed".to_owned();
                };
                let layout = win.layout(icon_scale);
                let mut words = rest.split_whitespace();
                let what = words.next().unwrap_or("");
                let n = words.next().and_then(|w| w.parse::<usize>().ok()).unwrap_or(0);
                let mid = |r: Rect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
                let at = match what {
                    "item" if n < win.items.len() => {
                        let mut c = layout.cell(n);
                        c.y -= win.scroll;
                        // (On its icon: the middle of a cell is between icon and name.)
                        let icon = layout.icon_in(&c, icon_scale);
                        Some(mid(icon))
                    }
                    "place" => win.hits.places.get(n).copied().map(mid),
                    "crumb" => win.hits.crumbs.get(n).map(|(r, _)| mid(*r)),
                    "up" => Some(mid(win.hits.up)),
                    "grid" => Some(mid(win.hits.grid)),
                    "list" => Some(mid(win.hits.list)),
                    "find" => Some(mid(win.hits.find)),
                    "seek" => Some(mid(win.hits.seek)),
                    "sort" => Some(mid(win.hits.sort)),
                    "undo" => Some(mid(win.hits.undo)),
                    "histrow" => win.hits.hist_rows.get(n).map(|(r, _)| mid(*r)),
                    "histundo" => {
                        let k = win.history.len().checked_sub(n + 1);
                        win.hits.hist_undo.iter().find(|(_, j)| Some(*j) == k).map(|(r, _)| mid(*r))
                    }
                    "info" => Some(mid(win.hits.info)),
                    "card" => Some(mid(win.hits.card)),
                    "pin" => Some(mid(win.hits.pin)),
                    "pencil" => Some(mid(win.hits.pencil)),
                    "add" => Some(mid(win.hits.tab_add)),
                    "current" => Some(mid(win.hits.cur_tab)),
                    "tab" => win.hits.tabs.iter().find(|(_, i)| *i == n).map(|(r, _)| mid(*r)),
                    "tabx" => win.hits.tab_x.iter().find(|(_, i)| *i == n).map(|(r, _)| mid(*r)),
                    "mode" => win.hits.mode.get(n).copied().map(mid),
                    "side" => Some((win.rail() / 2.0, win.virt().1 / 2.0)),
                    // Bare ground in the field, past the last thing.
                    "field" => Some((layout.body.x + layout.body.w - 20.0, layout.body.y + layout.body.h - 20.0)),
                    _ => None,
                };
                return match at {
                    Some(p) => {
                        let (x, y) = win.layout(self.icon_scale()).outward(p);
                        format!("where {x:.0} {y:.0}")
                    }
                    None => format!("where nothing: {rest}"),
                };
            }
            // `undo`: as Ctrl+Z; `hist open|close`, `hist undo <row>` (rows
            // counted from the top, the newest first); `hist` alone logs it.
            "undo" => self.filer_undo(id),
            "hist" => {
                let mut words = rest.split_whitespace();
                match (words.next(), words.next().and_then(|w| w.parse::<usize>().ok())) {
                    (Some("open"), _) if !self.filer.win(id).is_some_and(|w| w.hist_open) => self.filer_history_toggle(id),
                    (Some("close"), _) if self.filer.win(id).is_some_and(|w| w.hist_open) => self.filer_history_toggle(id),
                    (Some("undo"), Some(row)) => {
                        let k = self.filer.win(id).and_then(|w| w.history.len().checked_sub(row + 1));
                        if let Some(k) = k {
                            self.filer_undo_at(id, k);
                        }
                    }
                    _ => {}
                }
                let said = self.filer.win(id).map(history::state).unwrap_or_default();
                info!("files: history of window {id}:\n{said}");
                return said;
            }
            "path" => {
                // `path <text>`: as typed in the head, then Enter; `path? <text>`: Tab.
                if let Some(win) = self.filer.win_mut(id) {
                    win.go_to = Some(rest.to_owned());
                }
                self.filer_go_typed(id);
            }
            "view" => self.filer_set_list(rest == "list"),
            "mode" => {
                // `mode media|files`: as a click on the other circle.
                let media = rest == "media";
                if let Some(win) = self.filer.win_mut(id) {
                    win.media = media;
                }
                self.filer_remode(id);
            }
            "side" => {
                // `side open|close`: the sidebar open over the files, as on
                // hover (until the pointer moves).
                if let Some(win) = self.filer.win_mut(id) {
                    win.side_open = rest == "open";
                    win.dirty = true;
                }
                self.filer_request_draw(id);
            }
            "pin" => self.filer_pin(rest != "off"),
            "info" => {
                // `info on|off`: the i card.
                if let Some(win) = self.filer.win_mut(id) {
                    win.info_on = rest != "off";
                }
                self.filer_info_due(id);
                self.filer_request_draw(id);
            }
            "tab" => {
                // `tab new|close [n]|take <n>`: as the +, a tab's ×, a click.
                let mut words = rest.split_whitespace();
                let what = words.next().unwrap_or("");
                let n = words.next().and_then(|w| w.parse::<usize>().ok());
                let current = self.filer.win(id).map_or(0, |w| w.tab);
                match what {
                    "new" => self.filer_tab_new(id),
                    "close" => self.filer_tab_close(id, n.unwrap_or(current)),
                    "take" => self.filer_tab_take(id, n.unwrap_or(0)),
                    _ => return "tab new|close [n]|take <n>".to_owned(),
                }
            }
            "pencil" => {
                // `pencil`: as a click on the current tab's pencil.
                if let Some(win) = self.filer.win_mut(id) {
                    if win.go_to.take().is_none() {
                        win.edit_w = win.hits.cur_tab.w;
                        win.go_to = Some(win.typed_here());
                    }
                }
                self.filer_request_draw(id);
            }
            "sortmenu" => self.filer_sort_menu(id),
            "click" | "rclick" => {
                // `click <x> <y>`: a left click at that point of the window
                // (press and release, no travel), through the real code;
                // `rclick`, a right one.
                let n: Vec<f32> = rest.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                let [x, y] = n[..] else {
                    return "click <x> <y>".to_owned();
                };
                let icon_scale = self.icon_scale();
                if let Some(win) = self.filer.win_mut(id) {
                    win.at_win = Some((x, y));
                    win.ptr = Some(win.point((x, y), icon_scale));
                }
                self.filer_hover(id);
                let button = if verb == "rclick" { 0x111 } else { 0x110 };
                self.filer_button(id, button, true, 0);
                self.filer_button(id, button, false, 0);
            }
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
                        then: Then::Nothing,
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
                        // (A point of the field, as the other verbs take.)
                        let icon_scale = self.icon_scale();
                        let (x, y) = self.filer.win(id).map_or((x, y), |w| {
                            let (x, y) = w.layout(icon_scale).outward((x, y));
                            (x, y)
                        });
                        // (A rig's drag: the springs take it for a real one.)
                        self.filer.rig_drag = true;
                        self.filer_drag_over(id, x, y);
                    }
                    _ => {
                        self.filer.rig_drag = false;
                        if let Some(win) = self.filer.win_mut(id) {
                            win.gap = None;
                            win.edge = 0.0;
                            win.drop_on = None;
                            win.drop_place = None;
                            win.place_spring = None;
                            win.place_blink = None;
                            win.side_open = false;
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
wayland_client::delegate_noop!(App: ignore XdgToplevelTagManagerV1);

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
        assert_eq!(l.cols, 10); // (920 − 48 − 2·20) / 80 = 10.4
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
        // (Too narrow for a sidebar: the window lays its field from 0.)
        let l = Layout::of((400.0, 300.0), 1.0, false, 1.0, 0.0);
        assert_eq!(l.side_w, 0.0);
        assert_eq!(l.cols, 4); // (400 − 2·20) / 80 = 4.5
        assert_eq!(Layout::new((60, 60), 2.0).cols, 1);
    }

    #[test]
    fn the_item_under_a_point_follows_the_scroll() {
        let l = Layout::new((920, 600), 1.0);
        // (One on the second row, whatever the columns.)
        let i = l.cols + 1;
        let c = l.cell(i);
        let mid = (c.x + c.w / 2.0, c.y + c.h / 2.0);
        assert_eq!(l.index_at(mid, 0.0, 30), Some(i));
        assert_eq!(l.index_at(mid, l.ch, 30), Some(i + l.cols));
        assert_eq!(l.index_at((mid.0, mid.1 - l.ch), l.ch, 30), Some(i));
        // Past the last item, in the head and in the places: nothing.
        assert_eq!(l.index_at(mid, 0.0, i), None);
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
    fn a_picture_says_how_much_of_its_square_is_empty() {
        let n = crate::apps::ICON_SIZE as usize;
        let mut px = vec![0u8; n * n * 4];
        // (Ink from a quarter down to the middle: a wide photo, say.)
        for y in n / 4..n / 2 {
            for x in 0..n {
                px[(y * n + x) * 4 + 3] = 255;
            }
        }
        let (top, bottom) = ink_rows(&px);
        assert!((top - 0.25).abs() < 0.01 && (bottom - (0.5 - 1.0 / n as f32)).abs() < 0.01);
        assert_eq!(ink_rows(&vec![0u8; n * n * 4]), (0.0, 0.0));
    }

    #[test]
    fn a_typed_names_field_stays_inside_the_field() {
        let l = Layout::of((920.0, 600.0), 1.0, false, 1.0, SIDE_W);
        // (Centred under its cell where there is room; else pushed in.)
        let mid = l.body.x + l.body.w / 2.0;
        assert!((name_field_x(&l, mid, 100.0) - (mid - 55.0)).abs() < 0.01);
        assert!(name_field_x(&l, l.body.x, 300.0) >= l.body.x);
        let right = name_field_x(&l, l.body.x + l.body.w, 300.0);
        assert!(right + 310.0 <= l.body.x + l.body.w);
    }

    #[test]
    fn two_dots_lead_to_the_folder_this_one_is_in() {
        let home = Place {
            name: "Home".to_owned(),
            path: PathBuf::from("/home/max"),
            icon: String::new(),
            gap: false,
            start: true,
        };
        let places = with_up(std::slice::from_ref(&home), Path::new("/home/max/Documents/Work"), None);
        assert_eq!((places[0].name.as_str(), places[0].path.as_path()), (UP, Path::new("/home/max/Documents")));
        assert!(!places[1].gap && places[1].path == home.path);
        // At the root, at Home (the floor) and at Recents (a root of its
        // own): no `..`.
        assert_eq!(with_up(std::slice::from_ref(&home), Path::new("/"), None), [home.clone()]);
        let roots = [PathBuf::from("/home/max"), recent::root()];
        assert!(with_up(std::slice::from_ref(&home), Path::new("/home/max"), Some(&roots)).iter().all(|p| p.name != UP));
        assert!(with_up(std::slice::from_ref(&home), &recent::root(), Some(&roots)).iter().all(|p| p.name != UP));
        assert!(with_up(std::slice::from_ref(&home), Path::new("/home/max/Documents"), Some(&roots)).iter().any(|p| p.name == UP));
    }

    #[test]
    fn the_floor_is_the_deepest_root_a_folder_is_under() {
        // The floor: Home, a stick; the system's has none.
        let roots = [PathBuf::from("/home/max"), PathBuf::from("/run/media/max/STICK")];
        assert_eq!(floor(Path::new("/run/media/max/STICK/photos"), &roots), Some(Path::new("/run/media/max/STICK")));
        assert_eq!(floor(Path::new("/home/max/Documents"), &roots), Some(Path::new("/home/max")));
        assert_eq!(floor(Path::new("/etc"), &roots), None);
        assert_eq!(floor(Path::new("/home"), &roots), None);
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
        assert_eq!(two_lines("short.txt", 100.0 + crate::desktop::FIT_SLACK, &mut measure), ("short.txt".to_owned(), String::new()));
        let (a, b) = two_lines("holiday photos.zip", 100.0 + crate::desktop::FIT_SLACK, &mut measure);
        assert_eq!((a.as_str(), b.as_str()), ("holiday", "photos.zip"));
        let (a, b) = two_lines("abcdefghijklmnopqrstuvwxyz0123456789", 100.0 + crate::desktop::FIT_SLACK, &mut measure);
        assert_eq!(a, "abcdefghij");
        assert!(b.ends_with('…') && b.chars().count() <= 10);
    }

    #[test]
    fn archives_gio_mime_and_the_owners_places_are_read_right() {
        assert!(is_archive("Fotos.ZIP") && is_archive("a.tar.gz") && !is_archive("zip.txt"));
        let said = "Default application for “image/png”: org.gnome.Loupe.desktop\nRegistered applications:\n\torg.gnome.Loupe.desktop\n\tseam.desktop\nRecommended applications:\n\tseam.desktop\n";
        assert_eq!(parse_gio_mime(said), ["org.gnome.Loupe", "seam"]);
        assert!(parse_gio_mime("No default applications for “x/y”\n").is_empty());
        let text = "file:///home/max/Pictures/Wallpapers\nsftp://host/x Remote\nfile:///home/max/My%20Work Work\n";
        assert_eq!(
            parse_marks(text),
            [
                ("Wallpapers".to_owned(), PathBuf::from("/home/max/Pictures/Wallpapers")),
                ("Work".to_owned(), PathBuf::from("/home/max/My Work")),
            ]
        );
        // Added once, at the end; taken out, the others stay — the network one too.
        let more = marks_with(text, Path::new("/home/max/a b"), true);
        assert!(more.ends_with("file:///home/max/a%20b\n") && more.starts_with(text));
        assert_eq!(marks_with(&more, Path::new("/home/max/a b"), true), more);
        let less = marks_with(text, Path::new("/home/max/My Work"), false);
        assert_eq!(less, "file:///home/max/Pictures/Wallpapers\nsftp://host/x Remote\n");
        // A copy beside its original: before a file's ending, after a folder's name.
        let dir = std::env::temp_dir().join(format!("waverunner-copyname-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(copy_name(&dir, "report.txt", false), dir.join("report copy.txt"));
        assert_eq!(copy_name(&dir, "a.tar.gz", false), dir.join("a copy.tar.gz"));
        assert_eq!(copy_name(&dir, "v1.2 final", true), dir.join("v1.2 final copy"));
        std::fs::write(dir.join("report copy.txt"), "x").unwrap();
        assert_eq!(copy_name(&dir, "report.txt", false), dir.join("report copy 2.txt"));
        let _ = std::fs::remove_dir_all(&dir);
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
    fn things_let_go_in_the_room_made_land_there_at_once() {
        let items: Vec<Item> = ["a", "b", "c", "d", "e"].into_iter().map(item).collect();
        // "b" in hand: the rest closed up and parted at 3 (a c d _ e).
        let slide = [0.0, 1.0, 1.0, 2.0, 4.0];
        let (now, at) = settled(items.clone(), &slide, &["/x/b".to_owned()], 3);
        let names: Vec<&str> = now.iter().map(|it| it.name.as_str()).collect();
        assert_eq!(names, ["a", "c", "d", "b", "e"]);
        // The rest stay where the room put them; what was carried is in it.
        assert_eq!(at, [0.0, 1.0, 2.0, 3.0, 4.0]);
        // A group keeps its own order; a gap past the end is the end.
        let (now, _) = settled(items, &slide, &["/x/d".to_owned(), "/x/a".to_owned()], 99);
        let names: Vec<&str> = now.iter().map(|it| it.name.as_str()).collect();
        assert_eq!(names, ["b", "c", "e", "a", "d"]);
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
    fn the_grid_stands_in_the_middle_beside_the_sidebar() {
        let l = Layout::of((920.0, 600.0), 1.0, false, 1.0, SIDE_W);
        let first = l.cell(0);
        let last = l.cell(l.cols - 1);
        let (left, right) = (first.x - SIDE_W, 920.0 - (last.x + last.w));
        assert!((left - right).abs() < 0.01 && left >= GRID_AIR + PAD);
    }

    #[test]
    fn media_is_pictures_and_videos() {
        assert!(is_media("a.JPG") && is_media("clip.mp4") && is_media("x.heic"));
        assert!(!is_media("notes.txt") && !is_media("song.flac"));
    }

    #[test]
    fn a_list_is_one_column_of_rows_under_the_path() {
        let l = Layout::of((920.0, 600.0), 1.0, true, 1.0, SIDE_W);
        assert_eq!((l.cols, l.ch), (1, LIST_ROW));
        assert_eq!(l.body.y, LIST_TOP);
        // (Its rows as far in from the sidebar as from the window's edge.)
        let c = l.cell(0);
        assert!((c.x - (SIDE_W + LIST_AIR + PAD)).abs() < 0.01 && (920.0 - (c.x + c.w) - (LIST_AIR + PAD)).abs() < 0.01);
        let (a, b) = (l.cell(0), l.cell(1));
        assert!((b.y - a.y - LIST_ROW).abs() < 0.01 && (a.w - (l.body.w - 2.0 * PAD)).abs() < 0.01);
        // The row under a point, and how far DOWN it the pointer is.
        assert_eq!(l.index_at((a.x + 200.0, b.y + 4.0), 0.0, 5), Some(1));
        let (d, f) = l.slot_at((a.x + 200.0, b.y + 24.0), 0.0);
        assert_eq!(d, 1);
        assert!(f > 0.7);
        // Both columns in a wide window, none in a narrow one.
        assert!(matches!(l.columns(&a), (Some(_), Some(_))));
        let narrow = Layout::of((320.0, 400.0), 1.0, true, 1.0, SIDE_W);
        assert_eq!(narrow.columns(&narrow.cell(0)).1, None);
    }

    #[test]
    fn the_zoom_is_the_fields_alone() {
        let one = Layout::of((1000.0, 600.0), 1.0, false, 1.0, SIDE_W);
        let two = Layout::of((1000.0, 600.0), 1.0, false, 2.0, SIDE_W);
        // The places keep their width; the field still fills the window…
        assert_eq!(two.side_w, one.side_w);
        assert_eq!(two.shown_body(), one.body);
        // …laid out half as big, so half as many things a row.
        assert!(two.cols < one.cols);
        // A point over the field goes into its space and comes back out.
        let p = (two.body.x + 300.0, two.body.y + 100.0);
        let inside = two.inward(p);
        assert_eq!(inside, (two.body.x + 150.0, two.body.y + 50.0));
        assert_eq!(two.outward(inside), p);
        // A point over the places or the head is left alone.
        assert_eq!(two.inward((10.0, 200.0)), (10.0, 200.0));
        assert_eq!(two.inward((500.0, 5.0)), (500.0, 5.0));
    }

    #[test]
    fn by_size_date_or_kind_folders_stay_first_and_by_name_the_own_order() {
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
        // By name: the folder's own order, turned round when descending.
        assert_eq!(names(SortKey::Name, false), ["small", "zdir", "big"]);
        let own = vec![items[1].clone(), items[2].clone(), items[0].clone()];
        let back: Vec<String> = sorted(own, &meta, SortKey::Name, true).into_iter().map(|it| it.name).collect();
        assert_eq!(back, ["zdir", "small", "big"]);
        // By kind: the ending, then the name; folders first.
        let mut a = item("b.txt");
        a.path = "/x/b.txt".to_owned();
        let kinds: Vec<String> = sorted(vec![item("z.png"), a, items[1].clone(), item("a.txt")], &meta, SortKey::Kind, false)
            .into_iter()
            .map(|it| it.name)
            .collect();
        assert_eq!(kinds, ["zdir", "z.png", "a.txt", "b.txt"]);
    }

    #[test]
    fn shift_selects_from_where_it_began_either_way() {
        assert_eq!(span(2, 5), 2..=5);
        assert_eq!(span(5, 2), 2..=5);
        assert_eq!(span(3, 3), 3..=3);
    }

    #[test]
    fn a_search_goes_on_into_the_folders_below_nearest_first() {
        let dir = std::env::temp_dir().join(format!("waverunner-deep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        for f in ["Informe.txt", "a/informe 2.txt", "a/b/otro INFORME.pdf", "a/nada.txt", ".hidden/informe.txt"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let got = search_below(&dir, "informe", false);
        // The folder's own things are the listing's, not this walk's.
        let names: Vec<&str> = got.iter().map(|it| it.name.as_str()).collect();
        assert_eq!(names, ["informe 2.txt", "otro INFORME.pdf"]);
        assert_eq!(below(&got[1].path, &dir), "a/b");
        assert_eq!(search_below(&dir, "informe", true).len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_was_done_can_be_taken_back() {
        let d = std::env::temp_dir().join(format!("waverunner-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::create_dir_all(d.join("b")).unwrap();
        std::fs::write(d.join("a/x.txt"), "x").unwrap();
        std::fs::write(d.join("a/y.txt"), "y").unwrap();
        // x was moved to b, y copied: told apart by what is left behind.
        std::fs::rename(d.join("a/x.txt"), d.join("b/x.txt")).unwrap();
        std::fs::copy(d.join("a/y.txt"), d.join("b/y.txt")).unwrap();
        let sources = [d.join("a/x.txt"), d.join("a/y.txt")];
        let brought = [d.join("b/x.txt"), d.join("b/y.txt")];
        let undos = undo_of(&sources, &brought);
        assert_eq!(undos.len(), 2);
        assert_eq!(undos[0], Undo::Moved(vec![(d.join("a/x.txt"), d.join("b/x.txt"))]));
        assert!(take_back(undos[0].clone()).is_some());
        assert!(d.join("a/x.txt").exists() && !d.join("b/x.txt").exists());
        // Not every one arrived: nothing is guessed.
        assert!(undo_of(&sources, &brought[..1]).is_empty());
        // A name goes back; a new folder goes only while it is empty.
        std::fs::rename(d.join("a/x.txt"), d.join("a/z.txt")).unwrap();
        let renamed = Undo::Renamed {
            from: d.join("a/x.txt"),
            to: d.join("a/z.txt"),
        };
        assert!(take_back(renamed).is_some() && d.join("a/x.txt").exists());
        assert!(take_back(Undo::Made(d.join("a"))).is_none());
        std::fs::create_dir(d.join("new")).unwrap();
        assert!(take_back(Undo::Made(d.join("new"))).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_typed_path_names_a_folder_and_tab_finishes_it() {
        let d = std::env::temp_dir().join(format!("waverunner-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Documents/Work")).unwrap();
        std::fs::create_dir_all(d.join("Downloads")).unwrap();
        std::fs::write(d.join("Documents/note.txt"), "x").unwrap();
        let s = |p: &str| format!("{}/{p}", d.display());
        assert_eq!(typed_place("~/Documents", &d), Some((d.join("Documents"), None)));
        assert_eq!(
            typed_place(&s("Documents/note.txt"), &d),
            Some((d.join("Documents"), Some(d.join("Documents/note.txt"))))
        );
        assert_eq!(typed_place("nowhere", &d), None);
        // (A trailing slash or a `.` step is no other folder.)
        assert_eq!(typed_place("~/Documents/", &d), Some((d.join("Documents"), None)));
        assert_eq!(typed_place("~/./Documents/", &d), Some((d.join("Documents"), None)));
        assert_eq!(typed_place(&s("nowhere"), &d), None);
        // Two start with "Do": as far as they agree. One: all of it, and its slash.
        assert_eq!(complete_path(&s("D"), &d), s("Do"));
        assert_eq!(complete_path(&s("Doc"), &d), s("Documents/"));
        assert_eq!(complete_path("~/Dow", &d), "~/Downloads/");
        assert_eq!(complete_path(&s("Documents/n"), &d), s("Documents/note.txt"));
        assert_eq!(complete_path(&s("zzz"), &d), s("zzz"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn show_in_folder_opens_each_folder_once_with_its_thing() {
        let items = Shown::Items(vec![
            "file:///home/max/Downloads/a%20b.pdf".to_owned(),
            "file:///home/max/Downloads/c.zip".to_owned(),
            "file:///tmp/x.txt".to_owned(),
        ]);
        assert_eq!(
            shown_places(&items),
            [
                (PathBuf::from("/home/max/Downloads"), Some(PathBuf::from("/home/max/Downloads/a b.pdf"))),
                (PathBuf::from("/tmp"), Some(PathBuf::from("/tmp/x.txt"))),
            ]
        );
        let folders = Shown::Folders(vec!["file:///home/max/Pictures".to_owned()]);
        assert_eq!(shown_places(&folders), [(PathBuf::from("/home/max/Pictures"), None)]);
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

}

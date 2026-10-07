//! The DESKTOP: `~/Desktop` drawn as icons behind the windows, the way
//! macOS and Windows keep a desktop.
//!
//! One layer-shell surface on the `Bottom` layer (above the wallpaper,
//! below every window — see [`crate::surface::create_desktop_surface`]),
//! with its own renderer and icon texture array, like the deck. The folder
//! is listed into [`Item`]s and each takes a cell of a grid ([`Grid`]):
//! the cell it was last put in ([`Desktop::remembered`], kept on disk per
//! file in cell units, so a scale or icon-size change moves nothing), or
//! the first free cell column by column from the top-left for a file that
//! has none. Drawn with the Files section's tile idiom: the file-type
//! carrier (or the file's own thumbnail) over a one-line name. A launcher
//! (`.desktop` file) wears its app icon and name.
//!
//! A click opens an item (files and folders through `xdg-open`, a launcher
//! through its `Exec=`). A press that travels takes the icon into a real
//! Wayland drag (our data device, the icon as the drag image): let go on
//! the desktop it drops into the nearest free cell and stays there; on the
//! dock's Recycle Bin (the dock comes up for the drag) it goes to the
//! trash; on any other app it arrives there as a file (`text/uri-list`),
//! moved or copied as that app decides. Files dragged in from any other
//! app the same way are brought into the folder and placed at the cell they
//! were dropped on — moved when they are on the same filesystem, copied
//! otherwise (a file from a USB stick stays on it). The folder is watched
//! so the icons follow it. No hover feedback beyond the cursor (Max,
//! 2026-10-06: "we don't need magnification on the desktop"). Nothing
//! selects or renames yet.
//!
//! Pointer-free: `waverunner-ctl debug-desktop [reload|open <n>|move <n>
//! <col> <row>|import <col> <row> <uri…>|forget]`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use smithay_client_toolkit::data_device_manager::data_offer::DragOffer;
use smithay_client_toolkit::data_device_manager::data_source::DragSource;
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::raw::RawPool;
use tracing::{error, info, warn};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::protocol::{wl_pointer, wl_shm};
use wayland_client::{Proxy, WEnum};

use crate::content::{
    IconInst, Label, Rect, Scene, GRID_CELL_W, GRID_ICON, GRID_ICON_TOP,
    LABEL_FONT_PX, LABEL_LINE_PX, NO_PLATE, PLATE_STATIC,
};
use crate::launch;
use crate::App;

/// The one drop type the desktop takes, and the first it offers: a list of
/// `file://` URIs. A drag out also offers the path as plain text, for a
/// terminal or an editor.
const URI_LIST: &str = "text/uri-list";
const PLAIN_TEXT: &str = "text/plain;charset=utf-8";
/// The icon raster's side (level 0 of a mip chain, see `apps::ICON_SIZE`).
const ICON_PX: usize = crate::apps::ICON_SIZE as usize;

/// Breathing room between the icons and the surface's edges.
const MARGIN: f32 = 12.0;
/// A desktop cell: the grid's width, a little taller so rows do not touch.
const CELL_H: f32 = crate::content::GRID_CELL_H + 6.0;
/// From the icon's bottom to its name.
const LABEL_GAP: f32 = 5.0;
/// Items past this are not shown (a desktop with more icons than fit on a
/// screen is not one we lay out).
const ITEMS_MAX: usize = 256;
/// Headroom in the icon array, so a few files arriving are single-layer
/// writes rather than a reallocation each.
const LAYER_HEADROOM: u32 = 8;
/// How far a press travels before it is a drag and not a click.
const DRAG_START: f32 = 6.0;
/// Remembered positions kept for files that are not on the desktop any
/// more (a file away for a moment keeps its place); past this the absent
/// ones are forgotten.
const REMEMBERED_MAX: usize = 1000;
/// The name's colour, and the shadow under it that keeps it readable on any
/// wallpaper (white on a bright picture would otherwise vanish).
const INK: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const INK_SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.6];
/// Environment override of the folder, for a test rig that must not show
/// the owner's real desktop.
const DIR_ENV: &str = "WAVERUNNER_DESKTOP_DIR";
/// The positions store, in the daemon's data dir.
const POSITIONS_FILE: &str = "desktop.json";

/// What an item on the desktop is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Folder,
    File,
    /// A `.desktop` file: shows its app's name and icon, runs its `Exec=`.
    Launcher,
}

/// One thing on the desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub path: String,
    /// The name shown: the file name, or a launcher's `Name=`.
    pub name: String,
    pub kind: Kind,
    /// Which picture it wears — see [`icon_request`] for the keys.
    pub icon: String,
    /// A launcher's command line (field codes stripped) and whether it
    /// wants a terminal.
    pub exec: Option<(String, bool)>,
}

/// A cell of the grid: `(column, row)`, from the top-left.
pub(crate) type Slot = (usize, usize);

/// The cell grid the surface is divided into.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Grid {
    pub cols: usize,
    pub rows: usize,
    /// One cell's size.
    pub cw: f32,
    pub ch: f32,
}

impl Grid {
    /// The grid for a `w`×`h` surface at the dock's icon scale: as many
    /// whole cells as fit inside the margins, never fewer than one each
    /// way.
    pub fn new(w: f32, h: f32, icon_scale: f32) -> Self {
        let cw = GRID_CELL_W * icon_scale;
        let ch = CELL_H * icon_scale;
        Self {
            cols: (((w - 2.0 * MARGIN) / cw).floor() as usize).max(1),
            rows: (((h - 2.0 * MARGIN) / ch).floor() as usize).max(1),
            cw,
            ch,
        }
    }

    /// A cell's rectangle.
    pub fn rect(&self, (col, row): Slot) -> Rect {
        Rect::new(
            MARGIN + col as f32 * self.cw,
            MARGIN + row as f32 * self.ch,
            self.cw,
            self.ch,
        )
    }

    pub fn contains(&self, (col, row): Slot) -> bool {
        col < self.cols && row < self.rows
    }

    /// The cell under a point, if it is on the grid.
    pub fn slot_at(&self, (x, y): (f32, f32)) -> Option<Slot> {
        let (fx, fy) = ((x - MARGIN) / self.cw, (y - MARGIN) / self.ch);
        if fx < 0.0 || fy < 0.0 {
            return None;
        }
        let slot = (fx as usize, fy as usize);
        self.contains(slot).then_some(slot)
    }

    /// Every cell, column by column from the top-left — the order a file
    /// without a place of its own takes the first free one.
    pub fn slots(&self) -> impl Iterator<Item = Slot> + '_ {
        (0..self.cols).flat_map(move |c| (0..self.rows).map(move |r| (c, r)))
    }

    /// The free cell whose centre is nearest `centre` — where a dropped
    /// icon lands. `None` only when every cell is taken.
    pub fn nearest_free(&self, centre: (f32, f32), taken: &HashSet<Slot>) -> Option<Slot> {
        let d2 = |slot: Slot| {
            let r = self.rect(slot);
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            (cx - centre.0).powi(2) + (cy - centre.1).powi(2)
        };
        self.slots()
            .filter(|s| !taken.contains(s))
            .min_by(|a, b| d2(*a).total_cmp(&d2(*b)))
    }
}

/// Where each item goes: its remembered cell when that is on the grid and
/// not already taken (first come, in list order), else the first free cell
/// column by column; `None` when the grid is full.
///
/// The caller then REMEMBERS every placement ([`stick`]): a cell taken
/// automatically is as much the icon's own as one it was dragged to, so a
/// file arriving later takes a free cell and moves nothing — a desktop
/// whose icons shuffle when a file is saved is not one. Only `forget`
/// re-flows the lot.
pub(crate) fn place(
    items: &[Item],
    remembered: &HashMap<String, Slot>,
    grid: &Grid,
) -> Vec<Option<Slot>> {
    let mut slots = vec![None; items.len()];
    let mut taken: HashSet<Slot> = HashSet::new();
    for (i, item) in items.iter().enumerate() {
        if let Some(&slot) = remembered.get(&item.path) {
            if grid.contains(slot) && taken.insert(slot) {
                slots[i] = Some(slot);
            }
        }
    }
    let mut free = grid.slots().filter(|s| !taken.contains(s));
    for slot in slots.iter_mut().filter(|s| s.is_none()) {
        *slot = free.next();
    }
    slots
}

/// Record every placed item's cell in `remembered`; whether anything new
/// was learned (and the store needs writing).
pub(crate) fn stick(
    items: &[Item],
    slots: &[Option<Slot>],
    remembered: &mut HashMap<String, Slot>,
) -> bool {
    let mut learned = false;
    for (item, slot) in items.iter().zip(slots) {
        let Some(slot) = slot else { continue };
        if remembered.get(&item.path) != Some(slot) {
            remembered.insert(item.path.clone(), *slot);
            learned = true;
        }
    }
    learned
}

/// The drag image: the item's icon on a surface of its own that the
/// compositor carries under the pointer. Gone with the drag.
pub(crate) struct DragIcon {
    surface: WlSurface,
    _buffer: WlBuffer,
    _pool: RawPool,
}

impl Drop for DragIcon {
    fn drop(&mut self) {
        self.surface.destroy();
    }
}

/// A press of the left button, until it is a click or a drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Press {
    /// The item under it, or none for bare wallpaper.
    pub item: Option<usize>,
    pub at: (f32, f32),
    pub serial: u32,
}

/// A drag in progress: the items in hand, as a Wayland drag of ours. The
/// compositor owns the pointer from here; where it is comes back to us as
/// `wl_data_device` enter/motion on whichever of our surfaces it crosses.
pub(crate) struct Drag {
    /// The one grabbed first, then the rest of the selection it belonged
    /// to (they travel together and land keeping their arrangement).
    pub items: Vec<usize>,
    /// The dock was hidden when the icon was lifted, and came up for the
    /// drag (so the bin is there to drop on): it goes back down after.
    pub dock_raised: bool,
    /// The source other apps read the file from; dropping it ends the drag
    /// on the wire.
    pub source: DragSource,
    /// The drag image (none when the picture had not arrived), held for
    /// the drag's life: dropping it destroys the surface.
    pub _icon: Option<DragIcon>,
    /// What the drop target chose to do with the file.
    pub action: DndAction,
}

/// A drag hovering one of our surfaces — another app's, or our own — and
/// where its pointer is on that surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DndIn {
    pub pos: (f32, f32),
    /// Over the DOCK (the bin), not the desktop.
    pub on_dock: bool,
}

/// The desktop's state on the loop.
#[derive(Default)]
pub(crate) struct Desktop {
    pub items: Vec<Item>,
    pub grid: Grid,
    /// Each item's cell (`None`: no room for it).
    pub slots: Vec<Option<Slot>>,
    /// Where each file was last put, by path — the positions store.
    pub remembered: HashMap<String, Slot>,
    /// Pointer position on the surface, while it is over it.
    pub ptr: Option<(f32, f32)>,
    /// The left button is down: on which item (none: bare wallpaper),
    /// where, with which serial — until it comes up (a click) or the
    /// pointer travels (a drag of the item, or a rubber band).
    pub press: Option<Press>,
    pub drag: Option<Drag>,
    /// The selected items, by path (so a reload keeps them).
    pub selected: HashSet<String>,
    /// A rubber band being drawn: where it started and where it is.
    pub band: Option<((f32, f32), (f32, f32))>,
    /// A drag hovering one of our surfaces (another app's, or ours).
    pub dnd: Option<DndIn>,
    /// Icon pixels by key, kept so a new renderer or a reallocated array can
    /// be refilled without asking again.
    chains: HashMap<String, Vec<u8>>,
    /// Keys asked of the resolver and not answered yet.
    pending: HashSet<String>,
    /// Keys the theme had no icon for: asked once, not again until the
    /// folder is next read.
    missing: HashSet<String>,
    /// What each texture layer (= item index) holds right now.
    uploaded: Vec<Option<String>>,
    /// Layers allocated in the desktop renderer's icon array.
    capacity: u32,
}

/// The desktop folder: `$WAVERUNNER_DESKTOP_DIR`, else the XDG user dir
/// (`~/.config/user-dirs.dirs`), else `~/Desktop`. Created if missing — a
/// desktop that cannot receive a file is not one.
pub(crate) fn desktop_dir() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let dir = std::env::var_os(DIR_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            let conf = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            let text = std::fs::read_to_string(conf.join("user-dirs.dirs")).ok()?;
            user_desktop_dir(&text, &home)
        })
        .unwrap_or_else(|| home.join("Desktop"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("cannot create {}: {e}", dir.display());
    }
    dir
}

/// `XDG_DESKTOP_DIR` out of a `user-dirs.dirs` file (`KEY="$HOME/Desktop"`,
/// `$HOME` literal, the value quoted).
fn user_desktop_dir(text: &str, home: &Path) -> Option<PathBuf> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("XDG_DESKTOP_DIR="))?;
    let value = line["XDG_DESKTOP_DIR=".len()..].trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    (!value.is_empty()).then_some(path)
}

/// Everything shown in `dir`: no dotfiles, folders first, then by name
/// (case-insensitively, as a desktop is sorted, not as `ls` is). The order
/// only decides who takes a free cell first; placed icons keep their cell.
pub(crate) fn list(dir: &Path) -> Vec<Item> {
    let mut items: Vec<Item> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = e.path();
            // Through the link, so a symlinked folder is a folder.
            let is_dir = std::fs::metadata(&path).ok()?.is_dir();
            Some(item_for(path, name, is_dir))
        })
        .collect();
    items.sort_by(|a, b| {
        (b.kind == Kind::Folder)
            .cmp(&(a.kind == Kind::Folder))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    items.truncate(ITEMS_MAX);
    items
}

/// The item for one directory entry. A `.desktop` file that parses as an
/// application is a launcher; one that does not is just a file.
fn item_for(path: PathBuf, name: String, is_dir: bool) -> Item {
    let path_str = path.to_string_lossy().into_owned();
    if is_dir {
        return Item {
            path: path_str,
            name,
            kind: Kind::Folder,
            icon: asset_key("asset-folder"),
            exec: None,
        };
    }
    if name.ends_with(".desktop") {
        if let Some(app) = waverunner_core::index::parse_desktop_file(&path) {
            return Item {
                path: path_str,
                name: app.name,
                kind: Kind::Launcher,
                icon: app
                    .icon
                    .map(|i| format!("app:{i}"))
                    .unwrap_or_else(|| asset_key("asset-file")),
                exec: Some((app.exec, app.needs_terminal)),
            };
        }
    }
    let icon = asset_key(crate::files::file_asset_name(&name));
    Item {
        path: path_str,
        name,
        kind: Kind::File,
        icon,
        exec: None,
    }
}

/// The icon key for one of the dock's file-type carriers (`asset-folder`,
/// `asset-image`, …): `asset:<themed icon name>`.
fn asset_key(asset: &str) -> String {
    let themed = crate::apps::ICON_ASSETS
        .iter()
        .find(|(id, _)| *id == asset)
        .map_or("text-x-generic", |(_, icon)| icon);
    format!("asset:{themed}")
}

/// The key a file's own picture is filed under once it has one.
fn thumb_key(path: &str) -> String {
    format!("thumb:{path}")
}

/// What to ask the icon resolver for a key: a bare themed carrier
/// (`asset:`), a plated app tile (`app:`), or nothing — a `thumb:` comes
/// from the thumbnailer, not the theme.
fn icon_request(key: &str) -> Option<crate::notif_icons::Request> {
    if let Some(name) = key.strip_prefix("asset:") {
        Some(crate::notif_icons::Request {
            key: key.to_owned(),
            icon: name.to_owned(),
            name: String::new(),
            unplated: true,
        })
    } else {
        key.strip_prefix("app:").map(|name| crate::notif_icons::Request {
            key: key.to_owned(),
            icon: name.to_owned(),
            name: String::new(),
            unplated: false,
        })
    }
}

/// A local path as a `file://` URI (percent-encoded), one line of a
/// `text/uri-list`.
pub(crate) fn file_uri(path: &str) -> String {
    format!("file://{}", crate::trash::encode_path(Path::new(path)))
}

/// What a drag out carries for `mime`: the files' URI list, or their paths
/// as text (one per line); nothing for a type we never offered.
pub(crate) fn drag_payload(paths: &[&str], mime: &str) -> Option<String> {
    match mime {
        URI_LIST => Some(paths.iter().map(|p| format!("{}\r\n", file_uri(p))).collect()),
        PLAIN_TEXT => Some(paths.join("\n")),
        _ => None,
    }
}

/// The rectangle between two corners, whichever way they were dragged.
pub(crate) fn band_rect((x0, y0): (f32, f32), (x1, y1): (f32, f32)) -> Rect {
    Rect::new(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs())
}

fn intersects(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// The items whose icon or name the band touches.
pub(crate) fn band_hits(grid: &Grid, slots: &[Option<Slot>], icon_scale: f32, band: &Rect) -> Vec<usize> {
    slots
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.map(|s| (i, grid.rect(s))))
        .filter(|(_, cell)| {
            let icon = icon_rect(cell, icon_scale);
            let body = Rect::new(icon.x, icon.y, icon.w, icon.h + LABEL_GAP + LABEL_LINE_PX);
            intersects(&body, band)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Cells for a group let go together: the grabbed one at `anchor`, each
/// other one at its old offset from the grabbed one — or, where that is off
/// the grid or taken, the free cell nearest to it. `offsets[i]` is item i's
/// (column, row) distance from the grabbed item; `taken` holds every cell
/// the group did not own.
pub(crate) fn place_group(
    grid: &Grid,
    anchor: Slot,
    offsets: &[(i32, i32)],
    taken: &HashSet<Slot>,
) -> Vec<Option<Slot>> {
    let mut taken = taken.clone();
    let mut out = Vec::with_capacity(offsets.len());
    for (dc, dr) in offsets {
        let want = (anchor.0 as i32 + dc, anchor.1 as i32 + dr);
        let slot = if want.0 >= 0 && want.1 >= 0 {
            let s = (want.0 as usize, want.1 as usize);
            if grid.contains(s) && !taken.contains(&s) {
                Some(s)
            } else {
                let r = grid.rect((
                    (want.0.max(0) as usize).min(grid.cols - 1),
                    (want.1.max(0) as usize).min(grid.rows - 1),
                ));
                grid.nearest_free((r.x + r.w / 2.0, r.y + r.h / 2.0), &taken)
            }
        } else {
            let r = grid.rect((0, 0));
            grid.nearest_free((r.x + r.w / 2.0, r.y + r.h / 2.0), &taken)
        };
        if let Some(s) = slot {
            taken.insert(s);
        }
        out.push(slot);
    }
    out
}

/// The wash behind a selected item: one soft rounded band around its icon
/// and name (the mockup's look, 2026-10-07).
const SEL_WASH: [f32; 4] = [1.0, 1.0, 1.0, 0.16];
const SEL_RADIUS: f32 = 10.0;
/// The rubber band: a faint fill and a hairline.
const BAND_FILL: [f32; 4] = [1.0, 1.0, 1.0, 0.07];
const BAND_LINE: [f32; 4] = [1.0, 1.0, 1.0, 0.38];

fn sel_rect(cell: &Rect) -> Rect {
    Rect::new(cell.x + 10.0, cell.y + 4.0, cell.w - 20.0, cell.h - 10.0)
}

/// Premultiplied RGBA pixels (the icon rasters) into `wl_shm` ARGB8888 —
/// little-endian B, G, R, A per pixel, alpha premultiplied as before.
pub(crate) fn rgba_to_argb(src: &[u8], dst: &mut [u8]) {
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
}

/// The buffer scale that shows a 256 px icon raster at (about) the size
/// the icon has on the desktop: integer, never below 1.
pub(crate) fn icon_buffer_scale(icon_scale: f32) -> i32 {
    ((ICON_PX as f32 / (GRID_ICON * icon_scale)).round() as i32).max(1)
}

/// The `file://` URIs of a `text/uri-list` (one per line, `#` comments
/// skipped, percent-decoded) as local paths; other schemes are not files
/// and are left out.
pub(crate) fn uri_list_paths(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let rest = l.strip_prefix("file://")?;
            // `file:///home/x` and `file://localhost/home/x` both name /home/x.
            let path = match rest.find('/') {
                Some(0) => rest,
                Some(i) if &rest[..i] == "localhost" => &rest[i..],
                _ => return None,
            };
            Some(PathBuf::from(crate::trash::decode_path(path)))
        })
        .collect()
}

/// A name for `name` that is not taken in `dir`: the name itself, else
/// `name (2)`, `name (3)`… before the extension — the whole of it, so
/// `a.tar.gz` becomes `a (2).tar.gz`.
fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.split_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Copy a directory tree (symlinks followed).
fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

/// Bring `src` to `dest`: a move where one filesystem allows it, else a
/// copy (a file from another volume stays there too — what a desktop does
/// with a file from a stick).
fn bring(src: &Path, dest: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    if std::fs::metadata(src)?.is_dir() {
        copy_tree(src, dest)
    } else {
        std::fs::copy(src, dest).map(|_| ())
    }
}

/// Bring every file of `paths` into `dir` (one already there stays as it
/// is); the paths they now have, in order.
pub(crate) fn import(paths: &[PathBuf], dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for src in paths {
        let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if src.parent() == Some(dir) {
            out.push(src.clone()); // already on the desktop: only its cell changes
            continue;
        }
        if !src.exists() {
            warn!("desktop: dropped {} does not exist", src.display());
            continue;
        }
        let dest = unique_dest(dir, name);
        match bring(src, &dest) {
            Ok(()) => {
                info!("desktop: {} → {}", src.display(), dest.display());
                out.push(dest);
            }
            Err(e) => warn!("desktop: cannot bring {} in: {e}", src.display()),
        }
    }
    out
}

/// The icon's square in its cell: the grid's icon, centred, under the
/// cell's top padding.
fn icon_rect(cell: &Rect, icon_scale: f32) -> Rect {
    let size = GRID_ICON * icon_scale;
    Rect::new(
        cell.x + (cell.w - size) / 2.0,
        cell.y + GRID_ICON_TOP * icon_scale,
        size,
        size,
    )
}

/// The widest text under a cell's icon.
fn label_max_w(cell: &Rect) -> f32 {
    cell.w - 12.0
}

/// A name on one line under its icon: whole, or cut with an ellipsis where
/// the SHAPED text would run past `max_w` (`measure` is the renderer's
/// cached shaping). Not the grid's per-character estimate: file names are
/// any length in any script, and the estimate let "Holiday photos" wrap
/// onto a second line the one-line label never shows.
fn fit_label(text: &str, max_w: f32, measure: &mut dyn FnMut(&str) -> f32) -> String {
    if measure(text) <= max_w {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |n: usize| -> String {
        let head: String = chars[..n].iter().collect();
        format!("{}…", head.trim_end())
    };
    // The longest head whose cut form fits: binary search, log₂(len)
    // measurements, each cached by the renderer.
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(&cut(mid)) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

/// One tile to draw: an item's icon square and fitted name.
struct Tile<'a> {
    item: &'a Item,
    /// Its texture layer (= its index).
    layer: u32,
    /// Where the icon goes; the name sits under it.
    icon: Rect,
    name: &'a str,
    max_w: f32,
    /// Whether the picture has arrived (else the name alone).
    has_icon: bool,
    /// Above everything — the one in hand.
    overlay: bool,
}

/// One tile — icon (if its picture has arrived) and shadowed name — into
/// the scene.
fn push_tile(scene: &mut Scene, tile: &Tile) {
    let icon = tile.icon;
    if tile.has_icon {
        let inst = IconInst {
            rect: icon,
            layer: tile.layer,
            tint: [0.0; 4],
            ring: -1.0,
            // Carriers and thumbnails are bare, as in the Files section; a
            // launcher is an app tile and gets the plate the dock's static
            // surfaces use.
            plate: if tile.item.kind == Kind::Launcher {
                PLATE_STATIC
            } else {
                NO_PLATE
            },
        };
        if tile.overlay {
            scene.overlay.push(inst);
        } else {
            scene.icons.push(inst);
        }
    }
    let cx = icon.x + icon.w / 2.0;
    let top = icon.y + icon.h + LABEL_GAP;
    for (dy, color) in [(1.0, INK_SHADOW), (0.0, INK)] {
        scene.labels.push(Label {
            text: tile.name.to_owned(),
            pos: (cx, top + dy),
            max_w: tile.max_w,
            font_px: LABEL_FONT_PX,
            line_px: LABEL_LINE_PX,
            centered: true,
            dim: false,
            cache: true,
            clip: None,
            family: None,
            color: Some(color),
        });
    }
}

/// What one frame shows besides the placed items.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Live<'a> {
    /// The items in hand: their cells are left empty (the compositor
    /// carries the picture under the pointer).
    pub in_hand: &'a [usize],
    /// Which items are selected (washed).
    pub selected: &'a [bool],
    /// A rubber band being drawn.
    pub band: Option<Rect>,
}

/// One frame of the desktop: every placed item in its cell (its icon where
/// the picture has arrived — `has_icon[i]` — and its fitted name,
/// `names[i]`), a wash behind each selected one, except those in hand; and
/// the rubber band, if one is being drawn. A hovering drag shows nothing:
/// where it will land is not pointed out (Max, 2026-10-07: "we don't need
/// that").
pub(crate) fn scene(
    items: &[Item],
    slots: &[Option<Slot>],
    grid: &Grid,
    names: &[String],
    has_icon: &[bool],
    icon_scale: f32,
    live: Live,
) -> Scene {
    let mut scene = Scene {
        alpha: 1.0,
        ..Default::default()
    };
    for (i, item) in items.iter().enumerate() {
        if live.in_hand.contains(&i) {
            continue;
        }
        let Some(slot) = slots.get(i).copied().flatten() else {
            continue;
        };
        let cell = grid.rect(slot);
        if live.selected.get(i).copied().unwrap_or(false) {
            scene.rects.push(crate::content::RectInst {
                rect: sel_rect(&cell),
                radius: SEL_RADIUS,
                color: SEL_WASH,
                glass: 0.0,
                border: 0.0,
            });
        }
        push_tile(
            &mut scene,
            &Tile {
                item,
                layer: i as u32,
                icon: icon_rect(&cell, icon_scale),
                name: names.get(i).map_or(item.name.as_str(), String::as_str),
                max_w: label_max_w(&cell),
                has_icon: has_icon.get(i).copied().unwrap_or(false),
                overlay: false,
            },
        );
    }
    // The rubber band, over the icons: a faint fill and a hairline.
    if let Some(band) = live.band {
        for (color, border) in [(BAND_FILL, 0.0), (BAND_LINE, 1.0)] {
            scene.rects.push(crate::content::RectInst {
                rect: band,
                radius: 3.0,
                color,
                glass: 0.0,
                border,
            });
        }
    }
    scene
}

impl Desktop {
    /// Whether item `i`'s picture is in its texture layer.
    fn has_icon(&self, i: usize) -> bool {
        self.uploaded
            .get(i)
            .is_some_and(|u| u.as_deref() == Some(self.items[i].icon.as_str()))
    }

    /// Point every item for `path` at the picture under `key`.
    fn repoint(&mut self, path: &str, key: &str) -> bool {
        let mut changed = false;
        for item in self.items.iter_mut().filter(|it| it.path == path) {
            if item.icon != key {
                item.icon = key.to_owned();
                changed = true;
            }
        }
        changed
    }

    /// The item under `pos`, if any.
    pub(crate) fn hit(&self, pos: (f32, f32)) -> Option<usize> {
        let slot = self.grid.slot_at(pos)?;
        self.slots.iter().position(|s| *s == Some(slot))
    }

    /// The cells the placed items hold, less `except`'s.
    fn taken(&self, except: Option<usize>) -> HashSet<Slot> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != except)
            .filter_map(|(_, s)| *s)
            .collect()
    }

    /// Put item `i` in the free cell nearest `centre` and remember it
    /// there. Nothing moves when every other cell is taken.
    fn settle(&mut self, i: usize, centre: (f32, f32)) -> Option<Slot> {
        let slot = self.grid.nearest_free(centre, &self.taken(Some(i)))?;
        self.slots[i] = Some(slot);
        self.remembered.insert(self.items[i].path.clone(), slot);
        Some(slot)
    }

}

/// The positions store on disk: path → `[col, row]`.
fn load_remembered() -> HashMap<String, Slot> {
    crate::persist::read_json(&crate::persist::data_path(POSITIONS_FILE)).unwrap_or_default()
}

impl App {
    /// A `configure` for the desktop surface: learn its size, build or
    /// resize its renderer, then list the folder (first time) or lay the
    /// items out again (a size change).
    pub(crate) fn configure_desktop(
        &mut self,
        configure: smithay_client_toolkit::shell::wlr_layer::LayerSurfaceConfigure,
    ) {
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        let first = self.desktop_size == (0, 0);
        self.desktop_size = (width, height);
        let scale = self.surface_scale(crate::fractional::SurfaceKind::Desktop);
        let (pw, ph) = (
            crate::fractional::physical(width, scale),
            crate::fractional::physical(height, scale),
        );
        if let Some(fs) = &self.desktop_fscale {
            fs.set_logical_size(width, height);
        }
        if let Some(renderer) = self.desktop_renderer.as_mut() {
            renderer.set_scale(scale);
            renderer.resize(pw, ph);
        } else {
            let built = {
                let Some(layer) = self.desktop_layer.as_ref() else {
                    return;
                };
                crate::renderer::Renderer::new(&self.conn, layer.wl_surface(), pw, ph, scale)
            };
            match built {
                Ok(mut renderer) => {
                    // A renderer built after the pictures came (a failed
                    // first build): start it with an empty array of the
                    // right size, and the uploads below refill it.
                    if self.desktop.capacity > 0 {
                        renderer.alloc_icon_array(self.desktop.capacity);
                        self.desktop.uploaded.iter_mut().for_each(|u| *u = None);
                    }
                    self.desktop_renderer = Some(renderer);
                }
                Err(e) => {
                    // Never fatal: the shell must survive a desktop that
                    // cannot render (the dock's rule).
                    error!("desktop renderer init failed: {e:#}");
                    return;
                }
            }
        }
        if first {
            self.desktop.remembered = load_remembered();
            self.reload_desktop();
        } else {
            self.relayout_desktop();
        }
    }

    /// Read the folder again and show what is there now. Pictures already
    /// in hand are kept; files that left take theirs with them.
    pub(crate) fn reload_desktop(&mut self) {
        if self.desktop_layer.is_none() {
            return;
        }
        let dir = desktop_dir();
        let mut items = list(&dir);
        // A file's own picture, where the Files section already has one;
        // otherwise ask the thumbnailer (its answer lands in
        // `desktop_on_thumb`). Audio-only "videos" keep the audio icon.
        for item in items.iter_mut().filter(|it| it.kind == Kind::File) {
            if self.audio_paths.contains(&item.path) {
                item.icon = asset_key("asset-audio");
                continue;
            }
            if !crate::thumbs::thumbable(&item.name) {
                continue;
            }
            let key = thumb_key(&item.path);
            if let Some((_, pixels)) = self.thumb_map.get(&item.path) {
                self.desktop.chains.insert(key.clone(), pixels.clone());
            }
            if self.desktop.chains.contains_key(&key) {
                item.icon = key;
            } else if self.thumb_pending.insert(item.path.clone()) {
                self.thumbs.request(&item.path);
            }
        }
        // A drag survives a reload only if its items are all still there,
        // in the same places in the list.
        if let Some(d) = self.desktop.drag.as_ref() {
            let same = d.items.iter().all(|&i| {
                self.desktop
                    .items
                    .get(i)
                    .zip(items.get(i))
                    .is_some_and(|(a, b)| a.path == b.path)
            });
            if !same {
                self.desktop_drag_end(false);
            }
        }
        // The selection follows the files; one that left is no longer
        // selected.
        let present: HashSet<&str> = items.iter().map(|it| it.path.as_str()).collect();
        self.desktop.selected.retain(|p| present.contains(p.as_str()));
        self.desktop.items = items;
        // Drop the pictures nothing wears any more (a thumbnail is ~350 KB).
        let worn: HashSet<&str> = self.desktop.items.iter().map(|it| it.icon.as_str()).collect();
        self.desktop.chains.retain(|k, _| worn.contains(k.as_str()));
        self.desktop.pending.retain(|k| worn.contains(k.as_str()));
        self.desktop.missing.clear();
        // Positions of files long gone are forgotten once they pile up.
        if self.desktop.remembered.len() > REMEMBERED_MAX {
            let present: HashSet<&str> = self.desktop.items.iter().map(|it| it.path.as_str()).collect();
            self.desktop.remembered.retain(|p, _| present.contains(p.as_str()));
            self.save_desktop_positions();
        }
        self.relayout_desktop();
    }

    /// Place the items on the grid, open the input over them, bring their
    /// pictures into the texture array, and draw.
    pub(crate) fn relayout_desktop(&mut self) {
        let (w, h) = self.desktop_size;
        if w == 0 || h == 0 {
            return;
        }
        self.desktop.grid = Grid::new(w as f32, h as f32, self.icon_scale());
        self.desktop.slots = place(&self.desktop.items, &self.desktop.remembered, &self.desktop.grid);
        if stick(&self.desktop.items, &self.desktop.slots, &mut self.desktop.remembered) {
            self.save_desktop_positions();
        }
        self.sync_desktop_input();
        self.sync_desktop_icons();
        self.request_desktop_draw();
    }

    /// Write the positions store.
    fn save_desktop_positions(&self) {
        crate::persist::write_json(
            "desktop",
            &crate::persist::data_path(POSITIONS_FILE),
            &self.desktop.remembered,
        );
    }

    /// The whole surface takes the pointer. It used to be the icons' cells
    /// only (the wallpaper between them "not ours to catch"), but a drop
    /// from another app reaches a surface only through its input region —
    /// a file let go on bare wallpaper would never have arrived. Nothing
    /// but the wallpaper is under us, and a click on it does nothing.
    fn sync_desktop_input(&mut self) {
        let Some(layer) = self.desktop_layer.as_ref() else {
            return;
        };
        let (w, h) = self.desktop_size;
        crate::surface::set_input_rects(&self.compositor, layer, &[(0, 0, w as i32, h as i32)]);
    }

    /// Every item's picture into its layer (= its index): uploaded where
    /// the pixels are in hand, asked of the resolver where not.
    fn sync_desktop_icons(&mut self) {
        let count = self.desktop.items.len();
        if count == 0 {
            return;
        }
        // Size the array for the items plus headroom, so a file arriving is
        // a single-layer write and not a reallocation — which also clears
        // the array, hence `uploaded` is forgotten with it.
        if count as u32 > self.desktop.capacity {
            self.desktop.capacity = count as u32 + LAYER_HEADROOM;
            if let Some(r) = self.desktop_renderer.as_mut() {
                r.alloc_icon_array(self.desktop.capacity);
            }
            self.desktop.uploaded.clear();
        }
        self.desktop
            .uploaded
            .resize(self.desktop.capacity as usize, None);
        let mut asked = Vec::new();
        for i in 0..count {
            let key = self.desktop.items[i].icon.clone();
            if self.desktop.uploaded[i].as_deref() == Some(key.as_str()) {
                continue;
            }
            if let Some(chain) = self.desktop.chains.get(&key) {
                if let Some(r) = self.desktop_renderer.as_mut() {
                    r.update_icon_layer(i as u32, chain);
                    self.desktop.uploaded[i] = Some(key);
                }
            } else if !self.desktop.pending.contains(&key) && !self.desktop.missing.contains(&key)
            {
                if let Some(req) = icon_request(&key) {
                    self.desktop.pending.insert(key);
                    asked.push(req);
                }
            }
        }
        if let Some(icons) = self.desktop_icons.as_ref() {
            for req in asked {
                icons.request(req);
            }
        }
    }

    /// The resolver answered: file the pixels (a placeholder answer means
    /// the theme has no such icon — the name alone is shown) and draw.
    pub(crate) fn on_desktop_icon(&mut self, res: crate::notif_icons::Resolved) {
        self.desktop.pending.remove(&res.key);
        let Some(chain) = res.chain else {
            warn!("desktop: no icon for {}", res.key);
            self.desktop.missing.insert(res.key);
            return;
        };
        if !self.desktop.items.iter().any(|it| it.icon == res.key) {
            return; // nothing wears it any more
        }
        self.desktop.chains.insert(res.key, chain);
        self.sync_desktop_icons();
        self.request_desktop_draw();
    }

    /// The thumbnailer finished a file that is on the desktop: it wears its
    /// own picture from now on.
    pub(crate) fn desktop_on_thumb(&mut self, path: &str, pixels: &[u8]) {
        if !self.desktop.items.iter().any(|it| it.path == path) {
            return;
        }
        let key = thumb_key(path);
        self.desktop.chains.insert(key.clone(), pixels.to_vec());
        self.desktop.repoint(path, &key);
        self.sync_desktop_icons();
        self.request_desktop_draw();
    }

    /// A "video" on the desktop turned out to be audio only: the audio icon.
    pub(crate) fn desktop_on_audio_only(&mut self, path: &str) {
        if self.desktop.repoint(path, &asset_key("asset-audio")) {
            self.sync_desktop_icons();
            self.request_desktop_draw();
        }
    }

    /// Draw now, or once the frame in flight has been shown.
    fn request_desktop_draw(&mut self) {
        if self.desktop_frame_pending {
            self.desktop_dirty = true;
        } else {
            self.draw_desktop();
        }
    }

    /// Draw one desktop frame.
    pub(crate) fn draw_desktop(&mut self) {
        let (w, h) = self.desktop_size;
        if w == 0 || h == 0 {
            return;
        }
        self.desktop_dirty = false;
        let has_icon: Vec<bool> = (0..self.desktop.items.len())
            .map(|i| self.desktop.has_icon(i))
            .collect();
        let icon_scale = self.icon_scale();
        let in_hand: Vec<usize> = self
            .desktop
            .drag
            .as_ref()
            .map(|d| d.items.clone())
            .unwrap_or_default();
        let selected: Vec<bool> = self
            .desktop
            .items
            .iter()
            .map(|it| self.desktop.selected.contains(&it.path))
            .collect();
        let band = self.desktop.band.map(|(a, b)| band_rect(a, b));
        let Some(renderer) = self.desktop_renderer.as_mut() else {
            return;
        };
        let max_w = label_max_w(&self.desktop.grid.rect((0, 0)));
        let names: Vec<String> = self
            .desktop
            .items
            .iter()
            .map(|item| {
                fit_label(&item.name, max_w, &mut |t| {
                    renderer.measure_text(t, LABEL_FONT_PX, None)
                })
            })
            .collect();
        let scene = scene(
            &self.desktop.items,
            &self.desktop.slots,
            &self.desktop.grid,
            &names,
            &has_icon,
            icon_scale,
            Live {
                in_hand: &in_hand,
                selected: &selected,
                band,
            },
        );
        let (layer, qh, pending) = (
            self.desktop_layer.as_ref(),
            &self.qh,
            &mut self.desktop_frame_pending,
        );
        // No thumbnail base: an item's layer is its index, and a thumbnail
        // simply replaces the carrier in it, so none is exempt from the
        // squircle. (Golem's theme has it off.)
        if let Err(e) = renderer.render(
            &scene,
            INK,
            None,
            self.config.theme.icon_squircle,
            u32::MAX,
            self.desktop_visible.as_mut(),
            &mut || {
                if let (Some(layer), false) = (layer, *pending) {
                    let surface = layer.wl_surface();
                    surface.frame(qh, surface.clone());
                    *pending = true;
                }
            },
        ) {
            warn!("desktop render failed: {e:#}");
        }
    }

    /// Route a pointer event on the desktop surface: a left press that
    /// comes straight back up opens the item under it; one that travels
    /// takes the icon along and drops it where it is let go.
    pub(crate) fn desktop_pointer(&mut self, event: wl_pointer::Event) {
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface_x,
                surface_y,
                ..
            } => {
                // The compositor forgot our cursor on the last leave.
                self.enter_serial = serial;
                self.cursor_now = None;
                self.desktop_motion(surface_x as f32, surface_y as f32);
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => self.desktop_motion(surface_x as f32, surface_y as f32),
            wl_pointer::Event::Leave { .. } => {
                // (Also what the compositor sends the moment a drag of ours
                // starts: the pointer is its from then on.)
                self.pointer_surface = crate::options::PointerSurface::Dock;
                self.desktop.press = None;
                self.desktop.ptr = None;
                if self.desktop.band.take().is_some() {
                    self.request_desktop_draw();
                }
            }
            wl_pointer::Event::Button {
                serial,
                button,
                state: WEnum::Value(state),
                ..
            } if button == crate::BTN_LEFT => match state {
                wl_pointer::ButtonState::Pressed => {
                    let Some(at) = self.desktop.ptr else {
                        return;
                    };
                    let item = self.desktop.hit(at);
                    // A press on an item outside the selection makes it the
                    // selection (so a drag of it takes it alone); on one
                    // inside, the selection stands (the drag takes them all).
                    if let Some(i) = item {
                        let path = &self.desktop.items[i].path;
                        if !self.desktop.selected.contains(path) {
                            self.desktop.selected.clear();
                            self.desktop.selected.insert(path.clone());
                            self.request_desktop_draw();
                        }
                    }
                    self.desktop.press = Some(Press { item, at, serial });
                }
                wl_pointer::ButtonState::Released => {
                    let press = self.desktop.press.take();
                    if self.desktop.band.take().is_some() {
                        // The band's selection stands; the band itself goes.
                        self.request_desktop_draw();
                    } else if let Some(press) = press {
                        if self.desktop.drag.is_some() {
                            return;
                        }
                        let under = self.desktop.ptr.and_then(|p| self.desktop.hit(p));
                        match press.item {
                            // A click on an item opens it.
                            Some(i) if under == Some(i) => self.desktop_activate(i),
                            // A click on bare wallpaper clears the selection.
                            None if !self.desktop.selected.is_empty() => {
                                self.desktop.selected.clear();
                                self.request_desktop_draw();
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// The pointer moved over the desktop: a press that has travelled far
    /// enough becomes a drag of the item under it, or — from bare
    /// wallpaper — a rubber band selecting what it touches.
    fn desktop_motion(&mut self, x: f32, y: f32) {
        self.desktop.ptr = Some((x, y));
        if let Some((from, _)) = self.desktop.band {
            self.desktop.band = Some((from, (x, y)));
            let band = band_rect(from, (x, y));
            let hits = band_hits(&self.desktop.grid, &self.desktop.slots, self.icon_scale(), &band);
            self.desktop.selected = hits.into_iter().map(|i| self.desktop.items[i].path.clone()).collect();
            self.request_desktop_draw();
        } else if self.desktop.drag.is_none() {
            if let Some(press) = self.desktop.press {
                let (px, py) = press.at;
                if (x - px).hypot(y - py) >= DRAG_START {
                    match press.item {
                        Some(i) => self.desktop_lift(i, press.at, press.serial),
                        None => {
                            self.desktop.band = Some((press.at, (x, y)));
                            self.desktop.selected.clear();
                            self.request_desktop_draw();
                        }
                    }
                }
            }
        }
        self.desktop_cursor();
    }

    /// Take item `i` in hand: start a Wayland drag of ours with the file on
    /// offer and the icon as the drag image, gripped where the press landed
    /// on it. A hidden dock comes up: the bin is where a file is thrown
    /// away.
    fn desktop_lift(&mut self, i: usize, at: (f32, f32), serial: u32) {
        let Some(slot) = self.desktop.slots.get(i).copied().flatten() else {
            return;
        };
        let (Some(manager), Some(device), Some(layer)) = (
            self.data_device_manager.as_ref(),
            self.data_device.as_ref(),
            self.desktop_layer.as_ref(),
        ) else {
            warn!("desktop: no data device; icons cannot be dragged");
            return;
        };
        self.desktop.press = None;
        let icon_scale = self.icon_scale();
        let icon = icon_rect(&self.desktop.grid.rect(slot), icon_scale);
        let grip = (at.0 - icon.x, at.1 - icon.y);
        let source = manager.create_drag_and_drop_source(
            &self.qh,
            [URI_LIST, PLAIN_TEXT],
            DndAction::Move | DndAction::Copy,
        );
        let image = self.drag_icon(&self.desktop.items[i], grip, icon_scale);
        source.start_drag(
            device,
            layer.wl_surface(),
            image.as_ref().map(|d| &d.surface),
            serial,
        );
        // The drag image is committed once it has its role: Hyprland maps
        // it on the first commit that carries a texture.
        if let Some(image) = image.as_ref() {
            image.surface.commit();
        }
        let dock_raised = self.ui.target() == crate::state::Target::Hidden;
        // The grabbed item first, then the rest of its selection.
        let mut items = vec![i];
        items.extend(
            self.desktop
                .items
                .iter()
                .enumerate()
                .filter(|(j, it)| *j != i && self.desktop.selected.contains(&it.path))
                .map(|(j, _)| j),
        );
        info!(
            "desktop: {} in hand{}",
            self.desktop.items[i].name,
            if items.len() > 1 {
                format!(" with {} more", items.len() - 1)
            } else {
                String::new()
            }
        );
        self.desktop.drag = Some(Drag {
            items,
            dock_raised,
            source,
            _icon: image,
            action: DndAction::empty(),
        });
        if dock_raised {
            self.handle_command(waverunner_proto::Command::Show);
        }
        self.schedule_frame();
        self.request_desktop_draw();
    }

    /// The drag image for `item`: its picture (as the desktop draws it:
    /// the carrier, the thumbnail, the launcher's tile) on a surface of its
    /// own, scaled to about the icon's size on screen and offset so the
    /// point gripped stays under the pointer. `None` without the picture
    /// (the drag still happens; the compositor shows its own cursor).
    fn drag_icon(&self, item: &Item, grip: (f32, f32), icon_scale: f32) -> Option<DragIcon> {
        let chain = self.desktop.chains.get(&item.icon)?;
        let shm = self.shm.as_ref()?;
        let stride = ICON_PX * 4;
        let len = stride * ICON_PX;
        if chain.len() < len {
            return None;
        }
        let mut pool = match RawPool::new(len, shm) {
            Ok(pool) => pool,
            Err(e) => {
                warn!("desktop: no shm pool for the drag image ({e})");
                return None;
            }
        };
        rgba_to_argb(&chain[..len], &mut pool.mmap()[..len]);
        let buffer = pool.create_buffer(
            0,
            ICON_PX as i32,
            ICON_PX as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            (),
            &self.qh,
        );
        let surface = self.compositor.create_surface(&self.qh);
        let scale = icon_buffer_scale(icon_scale);
        surface.set_buffer_scale(scale);
        // The hotspot: the surface sits at the pointer less the grip (in
        // its own logical pixels).
        let (gx, gy) = (-grip.0.round() as i32, -grip.1.round() as i32);
        if surface.version() >= 5 {
            surface.attach(Some(&buffer), 0, 0);
            surface.offset(gx, gy);
        } else {
            surface.attach(Some(&buffer), gx, gy);
        }
        surface.damage_buffer(0, 0, ICON_PX as i32, ICON_PX as i32);
        Some(DragIcon {
            surface,
            _buffer: buffer,
            _pool: pool,
        })
    }

    /// Where our own drag's pointer is in the DOCK surface's coordinates
    /// (the bin watches it come); `None` when it is not over the dock.
    pub(crate) fn desktop_drag_dock_pos(&self) -> Option<(f32, f32)> {
        self.desktop.drag.as_ref()?;
        self.desktop.dnd.filter(|d| d.on_dock).map(|d| d.pos)
    }

    /// Put a group let go on the desktop down: the grabbed one (`items[0]`)
    /// in the free cell nearest `pos`, the others keeping their arrangement
    /// around it where the cells allow; all remembered.
    fn desktop_settle_group(&mut self, items: &[usize], pos: (f32, f32)) {
        let Some(&first) = items.first() else {
            return;
        };
        let taken: HashSet<Slot> = self
            .desktop
            .slots
            .iter()
            .enumerate()
            .filter(|(i, _)| !items.contains(i))
            .filter_map(|(_, s)| *s)
            .collect();
        let Some(anchor) = self.desktop.grid.nearest_free(pos, &taken) else {
            return;
        };
        let Some(from) = self.desktop.slots[first] else {
            return;
        };
        let offsets: Vec<(i32, i32)> = items
            .iter()
            .map(|&i| match self.desktop.slots[i] {
                Some((c, r)) => (c as i32 - from.0 as i32, r as i32 - from.1 as i32),
                None => (0, 0),
            })
            .collect();
        let placed = place_group(&self.desktop.grid, anchor, &offsets, &taken);
        for (&i, slot) in items.iter().zip(placed) {
            if let Some(slot) = slot {
                self.desktop.slots[i] = Some(slot);
                self.desktop
                    .remembered
                    .insert(self.desktop.items[i].path.clone(), slot);
                info!("desktop: {} → {slot:?}", self.desktop.items[i].name);
            }
        }
        self.save_desktop_positions();
    }

    /// Another app asked for the file in hand (or we did, over our own
    /// surfaces — nothing is read then). Written off the loop.
    pub(crate) fn desktop_send_drag(&mut self, mime: &str, pipe: smithay_client_toolkit::data_device_manager::WritePipe) {
        let Some(drag) = self.desktop.drag.as_ref() else {
            return;
        };
        let paths: Vec<&str> = drag
            .items
            .iter()
            .filter_map(|&i| self.desktop.items.get(i))
            .map(|it| it.path.as_str())
            .collect();
        let Some(payload) = drag_payload(&paths, mime) else {
            warn!("desktop: {mime} asked of a drag that never offered it");
            return;
        };
        let fd: std::os::fd::OwnedFd = pipe.into();
        let mime = mime.to_owned();
        std::thread::spawn(move || {
            use std::io::Write;
            let mut file = std::fs::File::from(fd);
            if let Err(e) = file.write_all(payload.as_bytes()) {
                warn!("desktop: writing the dragged file's {mime} failed: {e}");
            }
        });
    }

    /// The drop target chose what to do with the file in hand.
    pub(crate) fn desktop_drag_action(&mut self, action: DndAction) {
        if let Some(drag) = self.desktop.drag.as_mut() {
            drag.action = action;
        }
    }

    /// Our drag is over: dropped somewhere and done (`done`), or cancelled.
    /// A drop elsewhere as a move takes the file with it — the folder watch
    /// takes its icon off; a reload makes sure.
    pub(crate) fn desktop_drag_end(&mut self, done: bool) {
        let Some(drag) = self.desktop.drag.take() else {
            return;
        };
        info!(
            "desktop: {} {} (action {:?})",
            self.desktop.items.get(drag.items[0]).map_or("?", |it| it.name.as_str()),
            if done { "dropped" } else { "drag cancelled" },
            drag.action
        );
        if drag.dock_raised {
            self.handle_command(waverunner_proto::Command::Hide);
        }
        drop(drag); // the source and the drag image go with it
        self.desktop.dnd = None;
        self.desktop_dnd_offer = None;
        self.schedule_frame();
        if done {
            self.reload_desktop();
        } else {
            self.request_desktop_draw();
        }
    }

    /// A drag came over the desktop (`on_dock` false) or the dock: our own
    /// (the file in hand; accepted so the drop counts, nothing to read) or
    /// another app's (taken on the desktop if it carries files, as a move
    /// — or a copy, where a move is not possible; not on the dock).
    pub(crate) fn desktop_dnd_enter(&mut self, offer: DragOffer, on_dock: bool) {
        let own = self.desktop.drag.is_some();
        let mimes = offer.with_mime_types(|m| m.to_vec());
        let has_files = mimes.iter().any(|t| t == URI_LIST);
        if !own {
            info!(
                "desktop: a drag came over {} at ({:.0},{:.0}) offering {mimes:?}, actions {:?}",
                if on_dock { "the dock" } else { "us" },
                offer.x,
                offer.y,
                offer.source_actions
            );
        }
        if !has_files || (on_dock && !own) {
            offer.accept_mime_type(offer.serial, None);
            return;
        }
        offer.accept_mime_type(offer.serial, Some(URI_LIST.to_owned()));
        offer.set_actions(DndAction::Move | DndAction::Copy, DndAction::Move);
        self.desktop.dnd = Some(DndIn {
            pos: (offer.x as f32, offer.y as f32),
            on_dock,
        });
        self.desktop_dnd_offer = Some(offer);
        self.schedule_frame();
        self.request_desktop_draw();
    }

    /// The hovering drag moved: where it is decides the drop; over the dock
    /// the bin watches it come. Nothing on the desktop is redrawn for it.
    pub(crate) fn desktop_dnd_motion(&mut self, x: f32, y: f32) {
        if let Some(d) = self.desktop.dnd.as_mut() {
            d.pos = (x, y);
            if d.on_dock {
                self.schedule_frame();
            }
        }
    }

    /// The drag left for elsewhere — unless it has just been DROPPED here:
    /// Hyprland sends `leave` right after `drop` (`dropDrag` in its
    /// DataDevice.cpp), while the files are still on their way through the
    /// pipe. Throwing the offer away at that point lost the drop and left
    /// the other app's drag unfinished (Nautilus stuck mid-drag, 2026-10-07).
    pub(crate) fn desktop_dnd_leave(&mut self) {
        let dropped = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
            .is_some_and(|o| o.dropped);
        if dropped {
            return;
        }
        if self.desktop.dnd.take().is_some() {
            if self.desktop.drag.is_none() {
                info!("desktop: the drag left");
            }
            self.desktop_dnd_offer = None;
            self.schedule_frame();
            self.request_desktop_draw();
        }
    }

    /// Let go on one of our surfaces. Our own file: into the cell under it
    /// on the desktop, or the trash from the dock's bin — nothing to read,
    /// the drop is just finished. Another app's: ask for the list of
    /// files; it is read off the loop (the other app writes when it
    /// pleases) and lands in `desktop_dnd_received`.
    pub(crate) fn desktop_dnd_drop(&mut self) {
        if let Some(drag) = self.desktop.drag.as_ref() {
            let items = drag.items.clone();
            let Some(offer) = self.desktop_dnd_offer.take() else {
                return;
            };
            let Some(at) = self.desktop.dnd.take() else {
                return;
            };
            if at.on_dock {
                if self.dropped_on_trash(&self.current_layout(), at.pos) {
                    for &i in &items {
                        let path = self.desktop.items[i].path.clone();
                        info!("desktop: {} → Recycle Bin", self.desktop.items[i].name);
                        self.desktop.remembered.remove(&path);
                        self.desktop.selected.remove(&path);
                        self.trash_file(&path); // the folder watch takes it off the desktop
                    }
                }
            } else {
                self.desktop_settle_group(&items, at.pos);
            }
            offer.finish(); // → our source's `dnd_finished` → `desktop_drag_end`
            return;
        }
        // The live offer (the one kept since `enter` is a snapshot: its
        // position and selected action are as of then).
        let live = self
            .data_device
            .as_ref()
            .and_then(|d| d.data().drag_offer())
            .filter(|o| o.dropped);
        let Some(offer) = live.or_else(|| self.desktop_dnd_offer.clone()) else {
            info!("desktop: a drop with no drag over us; ignored");
            return;
        };
        if self.desktop_dnd_offer.is_none() {
            info!("desktop: a drop with no drag over us; ignored");
            return;
        }
        info!(
            "desktop: dropped at ({:.0},{:.0}), selected action {:?}; reading {URI_LIST}",
            offer.x, offer.y, offer.selected_action
        );
        if let Some(d) = self.desktop.dnd.as_mut() {
            d.pos = (offer.x as f32, offer.y as f32);
        }
        let pipe = match offer.receive(URI_LIST.to_owned()) {
            Ok(pipe) => pipe,
            Err(e) => {
                warn!("desktop: cannot receive the drop: {e}");
                self.desktop_dnd_abandon();
                return;
            }
        };
        // Through `OwnedFd`, NOT `into_raw_fd`: SCTK 0.19's calloop-flavoured
        // `ReadPipe::into_raw_fd` unwraps its inner fd, reads the number and
        // drops the owner — the pipe is closed before the thread reads it,
        // and every drop came back as 0 bytes (2026-10-07).
        let fd: std::os::fd::OwnedFd = pipe.into();
        let (tx, rx) = calloop::channel::channel::<String>();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut file = std::fs::File::from(fd);
            let mut text = String::new();
            if let Err(e) = file.read_to_string(&mut text) {
                warn!("desktop: reading the drop's files failed: {e}");
            }
            let _ = tx.send(text);
        });
        if self
            .loop_handle
            .insert_source(rx, |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(text) = event {
                    app.desktop_dnd_received(&text);
                }
            })
            .is_err()
        {
            warn!("desktop: cannot wait for the drop's files");
            self.desktop_dnd_abandon();
        }
    }

    /// A drop that cannot be taken after all: the offer is destroyed (the
    /// other app learns its drag was cancelled, and ends it) and the wash
    /// goes.
    fn desktop_dnd_abandon(&mut self) {
        if let Some(offer) = self.desktop_dnd_offer.take() {
            offer.destroy();
        }
        self.desktop.dnd = None;
        self.request_desktop_draw();
    }

    /// The dropped list arrived: bring the files in, clustered around the
    /// cell the drop was over, and tell the other app its drag is done.
    pub(crate) fn desktop_dnd_received(&mut self, list: &str) {
        let Some(offer) = self.desktop_dnd_offer.take() else {
            warn!("desktop: the drop's files arrived after its drag was gone");
            return;
        };
        let at = self.desktop.dnd.take().map(|d| d.pos);
        let paths = uri_list_paths(list);
        info!(
            "desktop: the drop's list is {} bytes → {} file path(s): {paths:?}",
            list.len(),
            paths.len()
        );
        let brought = import(&paths, &desktop_dir());
        self.desktop_place_brought(&brought, at);
        // Done, whatever came of the files: the other app must always hear
        // the end of its drag, or it stays mid-drag.
        offer.finish();
        self.reload_desktop();
    }

    /// Remember cells for files just brought in: each in the free cell
    /// nearest `at`, so a handful dropped together lands as a cluster
    /// around the drop point (without a point: the first free cells).
    fn desktop_place_brought(&mut self, brought: &[PathBuf], at: Option<(f32, f32)>) {
        let mut taken = self.desktop.taken(None);
        for path in brought {
            let key = path.to_string_lossy().into_owned();
            // One already on the desktop frees its old cell first.
            if let Some(i) = self.desktop.items.iter().position(|it| it.path == key) {
                if let Some(s) = self.desktop.slots[i] {
                    taken.remove(&s);
                }
            }
            let grid = self.desktop.grid;
            let slot = match at {
                Some(p) => grid.nearest_free(p, &taken),
                None => grid.slots().find(|s| !taken.contains(s)),
            };
            let Some(slot) = slot else {
                break;
            };
            taken.insert(slot);
            self.desktop.remembered.insert(key, slot);
        }
        if !brought.is_empty() {
            self.save_desktop_positions();
        }
    }

    /// A hand over an item, a fist around one in hand, the arrow between.
    fn desktop_cursor(&mut self) {
        let Some(device) = &self.cursor_device else {
            return;
        };
        let shape = if self.desktop.drag.is_some() {
            Shape::Grabbing
        } else if self.desktop.ptr.and_then(|p| self.desktop.hit(p)).is_some() {
            Shape::Pointer
        } else {
            Shape::Default
        };
        if self.cursor_now != Some(shape) {
            device.set_shape(self.enter_serial, shape);
            self.cursor_now = Some(shape);
        }
    }

    /// Open item `i`: a launcher runs, anything else opens in its app (a
    /// folder in the file manager) through `xdg-open`.
    pub(crate) fn desktop_activate(&mut self, i: usize) {
        // Opening is the end of a selection: no wash stays behind.
        if !self.desktop.selected.is_empty() {
            self.desktop.selected.clear();
            self.request_desktop_draw();
        }
        let Some(item) = self.desktop.items.get(i) else {
            return;
        };
        let (exec, terminal) = match &item.exec {
            Some((exec, terminal)) => (exec.clone(), *terminal),
            None => (format!("xdg-open {}", launch::shell_quote(&item.path)), false),
        };
        info!("desktop: open {} ({:?})", item.name, item.kind);
        if let Err(e) = launch::launch(&exec, terminal, &self.config.launch.terminal) {
            error!("desktop: opening {} failed: {e:#}", item.path);
        }
    }

    /// `debug-desktop` verb: the listing with its cells; `reload`; `open
    /// <n>`; `move <n> <col> <row>` (that cell, or the free one nearest
    /// it); `import <col> <row> <uri…>` (as a drop of those files on that
    /// cell); `forget` (every remembered position, the icons re-flow).
    pub(crate) fn desktop_debug(&mut self, what: &str) -> String {
        if self.desktop_layer.is_none() {
            return "no desktop surface (disabled, or closed)".to_owned();
        }
        if let Some(rest) = what.strip_prefix("select") {
            // `select 0 2 5` selects those; `select` alone clears.
            let picked: Vec<usize> = rest.split_whitespace().filter_map(|n| n.parse().ok()).collect();
            self.desktop.selected = picked
                .iter()
                .filter_map(|&i| self.desktop.items.get(i))
                .map(|it| it.path.clone())
                .collect();
            self.request_desktop_draw();
            return format!("{} selected", self.desktop.selected.len());
        }
        if let Some(rest) = what.strip_prefix("band ") {
            // `band x0 y0 x1 y1` draws a rubber band there (and selects what
            // it touches), as a drag from bare wallpaper would; `band` alone
            // is not a verb — the next pointer event ends it anyway.
            let n: Vec<f32> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            let [x0, y0, x1, y1] = n[..] else {
                return "band <x0> <y0> <x1> <y1>".to_owned();
            };
            self.desktop.band = Some(((x0, y0), (x1, y1)));
            let hits = band_hits(
                &self.desktop.grid,
                &self.desktop.slots,
                self.icon_scale(),
                &band_rect((x0, y0), (x1, y1)),
            );
            self.desktop.selected = hits.iter().map(|&i| self.desktop.items[i].path.clone()).collect();
            self.request_desktop_draw();
            return format!("band over {} item(s)", hits.len());
        }
        if let Some(rest) = what.strip_prefix("import ") {
            let mut w = rest.split_whitespace();
            let cell = w.next().and_then(|c| c.parse::<usize>().ok()).zip(
                w.next().and_then(|r| r.parse::<usize>().ok()),
            );
            let Some((c, r)) = cell else {
                return "import <col> <row> <uri…>".to_owned();
            };
            let list: String = w.map(|u| format!("{u}\n")).collect();
            let rect = self.desktop.grid.rect((c, r));
            let at = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
            let brought = import(&uri_list_paths(&list), &desktop_dir());
            self.desktop_place_brought(&brought, Some(at));
            self.reload_desktop();
            return format!("brought {} file(s) in at ({c},{r})", brought.len());
        }
        let mut words = what.split_whitespace();
        match (words.next(), words.next(), words.next(), words.next()) {
            (None, ..) => {
                let (w, h) = self.desktop_size;
                let g = self.desktop.grid;
                let mut out = format!(
                    "{} in {} on {w}×{h}: {}×{} cells of {:.0}×{:.0}, {} remembered",
                    self.desktop.items.len(),
                    desktop_dir().display(),
                    g.cols,
                    g.rows,
                    g.cw,
                    g.ch,
                    self.desktop.remembered.len()
                );
                for (i, item) in self.desktop.items.iter().enumerate() {
                    let at = match self.desktop.slots.get(i).copied().flatten() {
                        Some((c, r)) => format!("({c},{r})"),
                        None => "(no room)".to_owned(),
                    };
                    let pic = if self.desktop.has_icon(i) { "" } else { " [no picture yet]" };
                    out.push_str(&format!(
                        "\n  {i}: {:?} {} {at} {}{pic}",
                        item.kind, item.name, item.icon
                    ));
                }
                out
            }
            (Some("reload"), ..) => {
                self.reload_desktop();
                format!("reloaded: {} items", self.desktop.items.len())
            }
            (Some("open"), Some(n), ..) => match n.parse::<usize>() {
                Ok(i) if i < self.desktop.items.len() => {
                    self.desktop_activate(i);
                    format!("opened {i}")
                }
                _ => format!("no item {n}"),
            },
            (Some("move"), Some(n), Some(c), Some(r)) => {
                match (n.parse::<usize>(), c.parse::<usize>(), r.parse::<usize>()) {
                    (Ok(i), Ok(c), Ok(r)) if i < self.desktop.items.len() => {
                        let cell = self.desktop.grid.rect((c, r));
                        let centre = (cell.x + cell.w / 2.0, cell.y + cell.h / 2.0);
                        match self.desktop.settle(i, centre) {
                            Some(slot) => {
                                self.save_desktop_positions();
                                self.sync_desktop_input();
                                self.request_desktop_draw();
                                format!("{i} → {slot:?}")
                            }
                            None => "no free cell".to_owned(),
                        }
                    }
                    _ => format!("no item {n} / bad cell"),
                }
            }
            (Some("forget"), ..) => {
                self.desktop.remembered.clear();
                self.save_desktop_positions();
                self.relayout_desktop();
                "forgot every position".to_owned()
            }
            _ => "debug-desktop [reload|open <n>|move <n> <col> <row>|import <col> <row> <uri…>|forget]"
                .to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> Item {
        Item {
            path: format!("/d/{name}"),
            name: name.to_owned(),
            kind: Kind::File,
            icon: "asset:text-x-generic".into(),
            exec: None,
        }
    }

    #[test]
    fn user_dirs_desktop_resolves_home() {
        let home = Path::new("/home/x");
        let text = "# comment\nXDG_DOWNLOAD_DIR=\"$HOME/Downloads\"\nXDG_DESKTOP_DIR=\"$HOME/Desk top\"\n";
        assert_eq!(
            user_desktop_dir(text, home),
            Some(PathBuf::from("/home/x/Desk top"))
        );
        assert_eq!(
            user_desktop_dir("XDG_DESKTOP_DIR=\"/srv/desk\"", home),
            Some(PathBuf::from("/srv/desk"))
        );
        assert_eq!(user_desktop_dir("XDG_MUSIC_DIR=\"$HOME/Music\"", home), None);
        assert_eq!(user_desktop_dir("XDG_DESKTOP_DIR=\"\"", home), None);
    }

    #[test]
    fn grid_counts_whole_cells_inside_the_margins_never_zero() {
        // Cells 104×92 at scale 1: a 1000×400 surface holds 9 columns × 4 rows.
        let g = Grid::new(1000.0, 400.0, 1.0);
        assert_eq!((g.cols, g.rows), (9, 4));
        assert_eq!(g.rect((0, 0)), Rect::new(MARGIN, MARGIN, GRID_CELL_W, CELL_H));
        assert_eq!(g.rect((1, 2)).x, MARGIN + GRID_CELL_W);
        assert_eq!(g.rect((1, 2)).y, MARGIN + 2.0 * CELL_H);
        // A surface too small for one cell still has one.
        assert_eq!(Grid::new(10.0, 10.0, 1.0).cols, 1);
        // The icon-size setting scales the cells.
        assert!((Grid::new(1000.0, 1000.0, 1.3).cw - GRID_CELL_W * 1.3).abs() < 1e-4);
        // Column-major walk.
        let order: Vec<Slot> = Grid::new(240.0, 300.0, 1.0).slots().collect();
        assert_eq!(order, vec![(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)]);
    }

    #[test]
    fn slot_at_finds_the_cell_under_a_point() {
        let g = Grid::new(1000.0, 400.0, 1.0);
        assert_eq!(g.slot_at((MARGIN + 1.0, MARGIN + 1.0)), Some((0, 0)));
        assert_eq!(g.slot_at((MARGIN + GRID_CELL_W + 1.0, MARGIN + CELL_H + 1.0)), Some((1, 1)));
        assert_eq!(g.slot_at((1.0, 1.0)), None, "in the margin");
        assert_eq!(g.slot_at((5000.0, 20.0)), None, "past the grid");
    }

    #[test]
    fn place_keeps_remembered_cells_and_fills_the_rest_column_by_column() {
        let g = Grid::new(240.0, 300.0, 1.0); // 2 × 3
        let items = vec![item("a"), item("b"), item("c"), item("d")];
        let mut remembered = HashMap::new();
        remembered.insert("/d/b".to_owned(), (1, 2));
        remembered.insert("/d/c".to_owned(), (1, 2)); // the same cell: taken by b first
        remembered.insert("/d/d".to_owned(), (7, 7)); // off the grid
        let slots = place(&items, &remembered, &g);
        assert_eq!(slots, vec![Some((0, 0)), Some((1, 2)), Some((0, 1)), Some((0, 2))]);
        // A full grid leaves the rest without a cell.
        let many: Vec<Item> = (0..8).map(|i| item(&i.to_string())).collect();
        let slots = place(&many, &HashMap::new(), &g);
        assert_eq!(slots.iter().filter(|s| s.is_some()).count(), 6);
        assert_eq!(slots[7], None);
    }

    #[test]
    fn a_file_arriving_later_moves_nothing_once_placements_stick() {
        let g = Grid::new(240.0, 300.0, 1.0); // 2 × 3
        let mut remembered = HashMap::new();
        // First day: three files flow down the first column, and stick.
        let items = vec![item("b"), item("c"), item("d")];
        let slots = place(&items, &remembered, &g);
        assert!(stick(&items, &slots, &mut remembered));
        assert!(!stick(&items, &slots, &mut remembered), "nothing new the second time");
        // "a" sorts first — but b, c, d keep their cells; a takes the free one.
        let items = vec![item("a"), item("b"), item("c"), item("d")];
        let slots = place(&items, &remembered, &g);
        assert_eq!(slots, vec![Some((1, 0)), Some((0, 0)), Some((0, 1)), Some((0, 2))]);
        assert!(stick(&items, &slots, &mut remembered), "a's cell is learned");
        // "c" leaves: its cell is free for the next arrival, no one shifts.
        let items = vec![item("a"), item("b"), item("d"), item("e")];
        let slots = place(&items, &remembered, &g);
        assert_eq!(slots, vec![Some((1, 0)), Some((0, 0)), Some((0, 2)), Some((0, 1))]);
    }

    #[test]
    fn nearest_free_lands_a_drop_on_the_closest_open_cell() {
        let g = Grid::new(240.0, 300.0, 1.0); // 2 × 3
        let centre_of = |s: Slot| {
            let r = g.rect(s);
            (r.x + r.w / 2.0, r.y + r.h / 2.0)
        };
        let mut taken = HashSet::new();
        assert_eq!(g.nearest_free(centre_of((1, 1)), &taken), Some((1, 1)));
        taken.insert((1, 1));
        // Dropped on a taken cell: the nearest open neighbour, not the first.
        let near = g.nearest_free((centre_of((1, 1)).0, centre_of((1, 1)).1 + 10.0), &taken);
        assert_eq!(near, Some((1, 2)));
        // Nothing free: nowhere.
        let all: HashSet<Slot> = g.slots().collect();
        assert_eq!(g.nearest_free(centre_of((0, 0)), &all), None);
    }

    #[test]
    fn desktop_hit_settle_and_taken() {
        let mut d = Desktop {
            items: vec![item("a"), item("b")],
            grid: Grid::new(240.0, 300.0, 1.0),
            ..Default::default()
        };
        d.slots = place(&d.items, &d.remembered, &d.grid);
        let cell = d.grid.rect((0, 1));
        assert_eq!(d.hit((cell.x + 1.0, cell.y + 1.0)), Some(1));
        assert_eq!(d.hit((d.grid.rect((1, 0)).x + 1.0, MARGIN + 1.0)), None);
        // Settle b onto a's cell: it goes to the nearest free one instead,
        // and is remembered there.
        let a_cell = d.grid.rect((0, 0));
        let slot = d.settle(1, (a_cell.x + a_cell.w / 2.0, a_cell.y + a_cell.h / 2.0));
        assert_ne!(slot, Some((0, 0)));
        assert_eq!(d.slots[1], slot);
        assert_eq!(d.remembered.get("/d/b").copied(), slot);
        assert_eq!(d.taken(Some(1)), HashSet::from([(0, 0)]));
    }

    #[test]
    fn hit_finds_the_cell_under_the_pointer() {
        let g = Grid::new(1000.0, 400.0, 1.0);
        let d = Desktop {
            items: vec![item("a"), item("b"), item("c")],
            grid: g,
            slots: vec![Some((0, 0)), Some((0, 1)), Some((0, 2))],
            ..Default::default()
        };
        assert_eq!(d.hit((MARGIN + 1.0, MARGIN + 1.0)), Some(0));
        assert_eq!(d.hit((MARGIN + 1.0, MARGIN + CELL_H + 1.0)), Some(1));
        assert_eq!(d.hit((1.0, 1.0)), None);
        assert_eq!(d.hit((MARGIN + GRID_CELL_W + 1.0, MARGIN + 1.0)), None);
    }

    fn make_dir(files: &[(&str, &str)], dirs: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "waverunner-desktop-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for d in dirs {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for (name, content) in files {
            std::fs::write(dir.join(name), content).unwrap();
        }
        dir
    }

    #[test]
    fn listing_puts_folders_first_sorts_by_name_and_reads_launchers() {
        let dir = make_dir(
            &[
                ("zebra.png", ""),
                ("Apple.txt", ""),
                (".hidden", ""),
                (
                    "mail.desktop",
                    "[Desktop Entry]\nType=Application\nName=Mail\nExec=seam -golem-app mail %U\nIcon=mail-icon\n",
                ),
                ("broken.desktop", "not an entry"),
            ],
            &["Projects", "archive"],
        );
        let items = list(&dir);
        std::fs::remove_dir_all(&dir).ok();
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["archive", "Projects", "Apple.txt", "broken.desktop", "Mail", "zebra.png"]
        );
        assert_eq!(items[0].kind, Kind::Folder);
        assert_eq!(items[0].icon, "asset:folder");
        let mail = &items[4];
        assert_eq!(mail.kind, Kind::Launcher);
        assert_eq!(mail.icon, "app:mail-icon");
        assert_eq!(mail.exec, Some(("seam -golem-app mail".to_owned(), false)));
        // A `.desktop` that is not an entry is just a file.
        assert_eq!(items[3].kind, Kind::File);
        assert_eq!(items[5].icon, "asset:image-x-generic");
        assert!(items[5].exec.is_none());
    }

    #[test]
    fn icon_requests_bare_carriers_and_plated_apps_only() {
        let asset = icon_request("asset:folder").unwrap();
        assert!(asset.unplated);
        assert_eq!(asset.icon, "folder");
        let app = icon_request("app:firefox").unwrap();
        assert!(!app.unplated);
        assert_eq!(app.icon, "firefox");
        assert!(icon_request("thumb:/home/x/Desktop/a.png").is_none());
    }

    #[test]
    fn scene_draws_placed_items_arrived_icons_and_the_one_in_hand_on_top() {
        let items = vec![item("a.txt"), item("b.txt"), item("c.txt")];
        let g = Grid::new(1000.0, 400.0, 1.0);
        let slots = vec![Some((0, 0)), Some((0, 1)), None];
        let names: Vec<String> = items.iter().map(|i| i.name.clone()).collect();
        let quiet = Live::default();
        let s = scene(&items, &slots, &g, &names, &[true, false, true], 1.0, quiet);
        assert!(s.rects.is_empty(), "nothing but icons and names");
        // a has its icon; b's has not arrived; c has no cell at all.
        assert_eq!(s.icons.len(), 1);
        assert_eq!(s.icons[0].layer, 0);
        assert_eq!(s.labels.len(), 4, "a shadow and an ink label per placed item");
        assert_eq!(s.labels[0].color, Some(INK_SHADOW));
        assert_eq!(s.labels[1].color, Some(INK));
        assert_eq!(s.labels[1].text, "a.txt");
        // The icon rests centred under the cell's top padding.
        let cell = g.rect((0, 0));
        assert_eq!(s.icons[0].rect, icon_rect(&cell, 1.0));
        assert!((s.icons[0].rect.x - (cell.x + (cell.w - GRID_ICON) / 2.0)).abs() < 1e-4);
        // In hand: its cell is left empty (the compositor carries the
        // picture); nothing of it is drawn.
        let live = Live {
            in_hand: &[0],
            selected: &[],
            band: None,
        };
        let s = scene(&items, &slots, &g, &names, &[true, true, true], 1.0, live);
        assert!(s.overlay.is_empty());
        assert!(s.rects.is_empty(), "no wash for a hovering drag either");
        assert_eq!(s.icons.len(), 1, "only b stays in the grid");
        assert_eq!(s.icons[0].layer, 1);
        assert_eq!(s.labels.len(), 2, "b's two labels; a's are gone with it");
        // Selected: one wash behind b, inside its cell; a band on top.
        let band = Rect::new(5.0, 5.0, 200.0, 150.0);
        let live = Live {
            in_hand: &[],
            selected: &[false, true, false],
            band: Some(band),
        };
        let s = scene(&items, &slots, &g, &names, &[true, true, true], 1.0, live);
        assert_eq!(s.rects.len(), 3, "the wash, the band's fill and its line");
        let cell = g.rect((0, 1));
        assert_eq!(s.rects[0].rect, sel_rect(&cell));
        assert!(s.rects[0].rect.x > cell.x && s.rects[0].rect.y > cell.y);
        assert_eq!(s.rects[1].rect, band);
        assert_eq!(s.rects[2].border, 1.0);
    }

    #[test]
    fn a_rubber_band_selects_what_it_touches_either_way_round() {
        let g = Grid::new(1000.0, 400.0, 1.0);
        let slots = vec![Some((0, 0)), Some((0, 1)), Some((1, 0))];
        // Dragged up-left to down-right, or the reverse: the same rectangle.
        let a = band_rect((20.0, 20.0), (60.0, 60.0));
        let b = band_rect((60.0, 60.0), (20.0, 20.0));
        assert_eq!(a, b);
        assert_eq!(a, Rect::new(20.0, 20.0, 40.0, 40.0));
        // Touching the first icon only.
        assert_eq!(band_hits(&g, &slots, 1.0, &a), vec![0]);
        // Across two columns, through the names of the first row.
        let wide = band_rect((20.0, 80.0), (200.0, 90.0));
        assert_eq!(band_hits(&g, &slots, 1.0, &wide), vec![0, 2]);
        // In the margin: nothing.
        let none = band_rect((0.0, 0.0), (5.0, 5.0));
        assert!(band_hits(&g, &slots, 1.0, &none).is_empty());
    }

    #[test]
    fn a_group_lands_keeping_its_arrangement_where_it_can() {
        let g = Grid::new(240.0, 300.0, 1.0); // 2 × 3
        // Grabbed item at offset (0,0), one below it, one to its right.
        let offsets = [(0, 0), (0, 1), (1, 0)];
        let placed = place_group(&g, (0, 0), &offsets, &HashSet::new());
        assert_eq!(placed, vec![Some((0, 0)), Some((0, 1)), Some((1, 0))]);
        // Anchored at the bottom-right: the one below would fall off the
        // grid, the one to the right too — each takes the nearest free cell.
        let placed = place_group(&g, (1, 2), &offsets, &HashSet::new());
        assert_eq!(placed[0], Some((1, 2)));
        assert_ne!(placed[1], None);
        assert_ne!(placed[2], None);
        assert_ne!(placed[1], placed[2]);
        // A cell someone else holds is skipped for the nearest free one.
        let taken: HashSet<Slot> = HashSet::from([(0, 1)]);
        let placed = place_group(&g, (0, 0), &offsets, &taken);
        assert_eq!(placed[0], Some((0, 0)));
        assert_ne!(placed[1], Some((0, 1)));
        assert_eq!(placed[2], Some((1, 0)));
    }

    #[test]
    fn a_drag_out_offers_the_file_as_uri_list_and_as_text() {
        let path = "/home/x/Holiday photos/ü.png";
        assert_eq!(file_uri(path), "file:///home/x/Holiday%20photos/%C3%BC.png");
        assert_eq!(
            drag_payload(&[path], URI_LIST).as_deref(),
            Some("file:///home/x/Holiday%20photos/%C3%BC.png\r\n")
        );
        assert_eq!(drag_payload(&[path], PLAIN_TEXT).as_deref(), Some(path));
        assert_eq!(drag_payload(&[path], "image/png"), None);
        // A group: one URI per line, paths one per line.
        assert_eq!(
            drag_payload(&["/a/b", "/c d"], URI_LIST).as_deref(),
            Some("file:///a/b\r\nfile:///c%20d\r\n")
        );
        assert_eq!(drag_payload(&["/a/b", "/c d"], PLAIN_TEXT).as_deref(), Some("/a/b\n/c d"));
        // The round trip through the drop side.
        assert_eq!(uri_list_paths(&file_uri(path)), vec![PathBuf::from(path)]);
    }

    #[test]
    fn drag_image_pixels_swizzle_to_argb_and_scale_to_the_icon() {
        let src = [10u8, 20, 30, 40, 50, 60, 70, 80];
        let mut dst = [0u8; 8];
        rgba_to_argb(&src, &mut dst);
        assert_eq!(dst, [30, 20, 10, 40, 70, 60, 50, 80]);
        // 256 px raster shown at 54 logical px → scale 5 (51 px); never 0.
        assert_eq!(icon_buffer_scale(1.0), 5);
        assert_eq!(icon_buffer_scale(1.65), 3);
        assert_eq!(icon_buffer_scale(100.0), 1);
    }

    #[test]
    fn uri_lists_yield_local_paths_only_decoded() {
        let list = "# a comment\r\nfile:///home/x/Holiday%20photos/a%20b.png\r\nfile://localhost/srv/c.txt\nhttps://example.com/x\nfile://otherhost/nope\n\n";
        assert_eq!(
            uri_list_paths(list),
            vec![
                PathBuf::from("/home/x/Holiday photos/a b.png"),
                PathBuf::from("/srv/c.txt")
            ]
        );
    }

    #[test]
    fn import_moves_in_renames_clashes_and_leaves_desktop_files_alone() {
        let dir = make_dir(&[("taken.txt", "old")], &[]);
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("folder")).unwrap();
        std::fs::write(src.join("folder/inner.txt"), "in").unwrap();
        std::fs::write(src.join("taken.txt"), "new").unwrap();
        std::fs::write(src.join("plain.md"), "md").unwrap();
        let paths = vec![
            src.join("taken.txt"),
            src.join("plain.md"),
            src.join("folder"),
            dir.join("taken.txt"), // already on the desktop
            src.join("missing.txt"),
        ];
        let brought = import(&paths, &dir);
        assert_eq!(
            brought,
            vec![
                dir.join("taken (2).txt"),
                dir.join("plain.md"),
                dir.join("folder"),
                dir.join("taken.txt"),
            ]
        );
        // Moved, not copied: the sources are gone; the clash kept both.
        assert!(!src.join("plain.md").exists());
        assert_eq!(std::fs::read_to_string(dir.join("taken.txt")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(dir.join("taken (2).txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("folder/inner.txt")).unwrap(), "in");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unique_dest_counts_before_the_extension() {
        let dir = make_dir(&[("a.tar.gz", ""), ("a (2).tar.gz", ""), ("README", "")], &[]);
        assert_eq!(unique_dest(&dir, "a.tar.gz"), dir.join("a (3).tar.gz"));
        assert_eq!(unique_dest(&dir, "README"), dir.join("README (2)"));
        assert_eq!(unique_dest(&dir, "fresh.txt"), dir.join("fresh.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fit_label_cuts_by_measured_width_with_an_ellipsis() {
        // A "font" 6 px per char, the ellipsis included.
        let mut measure = |t: &str| t.chars().count() as f32 * 6.0;
        assert_eq!(fit_label("short", 60.0, &mut measure), "short");
        // 10 chars fit: 9 of the name plus "…".
        assert_eq!(fit_label("Holiday photos", 60.0, &mut measure), "Holiday p…");
        // A trailing space before the cut is trimmed, not shown.
        assert_eq!(fit_label("Holiday photos", 54.0, &mut measure), "Holiday…");
        // Nothing fits but the ellipsis itself.
        assert_eq!(fit_label("abc", 6.0, &mut measure), "…");
        // Multi-byte names are cut on characters, never inside one.
        assert_eq!(fit_label("héllo wörld", 36.0, &mut measure), "héllo…");
    }
}

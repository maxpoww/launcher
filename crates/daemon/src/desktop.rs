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
//! through its `Exec=`). A press that travels drags the icon; it drops into
//! the nearest free cell and stays there. The folder is watched so the
//! icons follow it. No hover feedback beyond the cursor (Max, 2026-10-06:
//! "we don't need magnification on the desktop"). Nothing selects, renames
//! or leaves the surface yet.
//!
//! Pointer-free: `waverunner-ctl debug-desktop [reload|open <n>|move <n>
//! <col> <row>|forget]`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{error, info, warn};
use wayland_client::protocol::wl_pointer;
use wayland_client::WEnum;

use crate::content::{
    IconInst, Label, Rect, Scene, GRID_CELL_W, GRID_ICON, GRID_ICON_TOP, LABEL_FONT_PX,
    LABEL_LINE_PX, NO_PLATE, PLATE_STATIC,
};
use crate::launch;
use crate::App;

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

/// A drag in progress: the item in hand, and where on its icon it was
/// gripped (so it does not jump under the pointer).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Drag {
    pub item: usize,
    pub grip: (f32, f32),
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
    /// The item the left button went down on, and where, until it comes up
    /// (a click) or the pointer travels (a drag).
    pub press: Option<(usize, (f32, f32))>,
    pub drag: Option<Drag>,
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

/// One frame of the desktop: every placed item in its cell (its icon where
/// the picture has arrived — `has_icon[i]` — and its fitted name,
/// `names[i]`), except the one in hand, drawn over everything with its icon
/// at `drag`'s position.
pub(crate) fn scene(
    items: &[Item],
    slots: &[Option<Slot>],
    grid: &Grid,
    names: &[String],
    has_icon: &[bool],
    icon_scale: f32,
    drag: Option<(usize, Rect)>,
) -> Scene {
    let mut scene = Scene {
        alpha: 1.0,
        ..Default::default()
    };
    for (i, item) in items.iter().enumerate() {
        let Some(slot) = slots.get(i).copied().flatten() else {
            continue;
        };
        let cell = grid.rect(slot);
        let in_hand = drag.filter(|(d, _)| *d == i).map(|(_, at)| at);
        push_tile(
            &mut scene,
            &Tile {
                item,
                layer: i as u32,
                icon: in_hand.unwrap_or_else(|| icon_rect(&cell, icon_scale)),
                name: names.get(i).map_or(item.name.as_str(), String::as_str),
                max_w: label_max_w(&cell),
                has_icon: has_icon.get(i).copied().unwrap_or(false),
                overlay: in_hand.is_some(),
            },
        );
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

    /// The placed items' cells, with their indices.
    fn cells(&self) -> impl Iterator<Item = (usize, Rect)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.map(|s| (i, self.grid.rect(s))))
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

    /// The icon rectangle of the item in hand, at the pointer.
    fn drag_rect(&self, icon_scale: f32) -> Option<Rect> {
        let drag = self.drag?;
        let (px, py) = self.ptr?;
        let size = GRID_ICON * icon_scale;
        Some(Rect::new(px - drag.grip.0, py - drag.grip.1, size, size))
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
        // A drag survives a reload only if its item is still there, in the
        // same place in the list.
        if let Some(d) = self.desktop.drag {
            let same = self
                .desktop
                .items
                .get(d.item)
                .zip(items.get(d.item))
                .is_some_and(|(a, b)| a.path == b.path);
            if !same {
                self.desktop.drag = None;
                self.desktop.press = None;
            }
        }
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

    /// The surface takes the pointer over the icons only — the wallpaper
    /// between them is not ours to catch — except while an icon is in
    /// hand, when the whole surface listens so the drag can cross it.
    fn sync_desktop_input(&mut self) {
        let Some(layer) = self.desktop_layer.as_ref() else {
            return;
        };
        let rects: Vec<(i32, i32, i32, i32)> = if self.desktop.drag.is_some() {
            let (w, h) = self.desktop_size;
            vec![(0, 0, w as i32, h as i32)]
        } else {
            self.desktop
                .cells()
                .map(|(_, c)| {
                    (
                        c.x.floor() as i32,
                        c.y.floor() as i32,
                        c.w.ceil() as i32,
                        c.h.ceil() as i32,
                    )
                })
                .collect()
        };
        crate::surface::set_input_rects(&self.compositor, layer, &rects);
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
        let drag = self
            .desktop
            .drag
            .zip(self.desktop.drag_rect(icon_scale))
            .map(|(d, r)| (d.item, r));
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
            drag,
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
                self.pointer_surface = crate::options::PointerSurface::Dock;
                // An icon in hand when the pointer goes is put down where it
                // was last seen, not lost.
                if self.desktop.drag.is_some() {
                    self.desktop_drop();
                }
                self.desktop.press = None;
                self.desktop.ptr = None;
            }
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(state),
                ..
            } if button == crate::BTN_LEFT => match state {
                wl_pointer::ButtonState::Pressed => {
                    self.desktop.press = self
                        .desktop
                        .ptr
                        .and_then(|p| self.desktop.hit(p).map(|i| (i, p)));
                }
                wl_pointer::ButtonState::Released => {
                    let press = self.desktop.press.take();
                    if self.desktop.drag.is_some() {
                        self.desktop_drop();
                    } else if let Some((i, _)) = press {
                        let under = self.desktop.ptr.and_then(|p| self.desktop.hit(p));
                        if under == Some(i) {
                            self.desktop_activate(i);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// The pointer moved over the desktop: an icon in hand follows it; a
    /// press that has travelled far enough becomes a drag.
    fn desktop_motion(&mut self, x: f32, y: f32) {
        self.desktop.ptr = Some((x, y));
        if self.desktop.drag.is_some() {
            self.request_desktop_draw();
        } else if let Some((i, (px, py))) = self.desktop.press {
            if (x - px).hypot(y - py) >= DRAG_START {
                self.desktop_lift(i, (px, py));
            }
        }
        self.desktop_cursor();
    }

    /// Take item `i` in hand, gripped where the press landed on its icon.
    fn desktop_lift(&mut self, i: usize, at: (f32, f32)) {
        let Some(slot) = self.desktop.slots.get(i).copied().flatten() else {
            return;
        };
        let icon = icon_rect(&self.desktop.grid.rect(slot), self.icon_scale());
        self.desktop.drag = Some(Drag {
            item: i,
            grip: (at.0 - icon.x, at.1 - icon.y),
        });
        self.sync_desktop_input();
        self.request_desktop_draw();
    }

    /// Put the icon in hand down: into the free cell nearest to where its
    /// icon is, remembered there.
    fn desktop_drop(&mut self) {
        let Some(drag) = self.desktop.drag.take() else {
            return;
        };
        let icon_scale = self.icon_scale();
        let size = GRID_ICON * icon_scale;
        if let Some((px, py)) = self.desktop.ptr {
            let centre = (px - drag.grip.0 + size / 2.0, py - drag.grip.1 + size / 2.0);
            if let Some(slot) = self.desktop.settle(drag.item, centre) {
                info!("desktop: {} → {slot:?}", self.desktop.items[drag.item].name);
                self.save_desktop_positions();
            }
        }
        self.sync_desktop_input();
        self.request_desktop_draw();
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
    /// it); `forget` (every remembered position, the icons re-flow).
    pub(crate) fn desktop_debug(&mut self, what: &str) -> String {
        if self.desktop_layer.is_none() {
            return "no desktop surface (disabled, or closed)".to_owned();
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
            _ => "debug-desktop [reload|open <n>|move <n> <col> <row>|forget]".to_owned(),
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
        let s = scene(&items, &slots, &g, &names, &[true, false, true], 1.0, None);
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
        // In hand: drawn over everything at the given rect, not in its cell.
        let at = Rect::new(300.0, 200.0, GRID_ICON, GRID_ICON);
        let s = scene(&items, &slots, &g, &names, &[true, true, true], 1.0, Some((0, at)));
        assert_eq!(s.overlay.len(), 1);
        assert_eq!(s.overlay[0].rect, at);
        assert_eq!(s.icons.len(), 1, "only b stays in the grid");
        assert_eq!(s.icons[0].layer, 1);
        assert_eq!(s.labels[1].pos.0, at.x + at.w / 2.0, "the name travels with it");
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

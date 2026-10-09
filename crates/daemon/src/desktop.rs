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
use smithay_client_toolkit::seat::keyboard::Keysym;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::raw::RawPool;
use tracing::{debug, error, info, warn};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::protocol::{wl_pointer, wl_shm};
use wayland_client::{Proxy, WEnum};

use crate::content::{
    IconInst, Label, Rect, Scene, GRID_CELL_W, GRID_ICON, GRID_ICON_TOP,
    LABEL_FONT_PX, LABEL_LINE_PX, NO_PLATE, PLATE_STATIC,
};
use crate::desktop_menu::{Action, Menu, MenuPaint};
use crate::desktop_props::Props;
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
/// Two clicks on one item within this are a double click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);
/// How far a press travels before it is a drag and not a click.
const DRAG_START: f32 = 6.0;
/// How long a sent pointer shape is trusted before it is sent again on the
/// next motion (see `Desktop::cursor_sent`).
const CURSOR_REFRESH: std::time::Duration = std::time::Duration::from_millis(120);
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
    /// Something plugged in and mounted (a stick, a phone — `mounts.rs`):
    /// its folder is elsewhere and it is NOT a file of the desktop's. It
    /// opens like a folder; it is ejected, never renamed, moved or binned.
    Volume,
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
    /// Where the first column starts: the columns are CENTRED on the
    /// surface, so the air left of the first is the air right of the last
    /// (Max, 2026-10-08: "the grid is not symmetrical, it should be").
    pub x0: f32,
    /// From one column's left edge to the next's: the cell's width plus
    /// whatever air the closer side margins left over, shared between the
    /// columns (Max, 2026-10-08: "closer to the edge, like half that air").
    pub pitch: f32,
}

impl Grid {
    /// The grid for a `w`×`h` surface at the dock's icon scale: as many
    /// whole cells as fit inside the margins, never fewer than one each
    /// way.
    pub fn new(w: f32, h: f32, icon_scale: f32) -> Self {
        let cw = GRID_CELL_W * icon_scale;
        let ch = CELL_H * icon_scale;
        let cols = (((w - 2.0 * MARGIN) / cw).floor() as usize).max(1);
        // Centred, the columns would leave half the spare width on each
        // side; the sides get half of THAT, and the rest goes between the
        // columns, so the first and last still sit the same way from their
        // edges.
        let spare = (w - cols as f32 * cw).max(0.0);
        let x0 = if cols > 1 { spare / 4.0 } else { spare / 2.0 };
        let pitch = if cols > 1 {
            (w - 2.0 * x0 - cw) / (cols - 1) as f32
        } else {
            cw
        };
        Self {
            cols,
            rows: (((h - 2.0 * MARGIN) / ch).floor() as usize).max(1),
            cw,
            ch,
            x0,
            pitch,
        }
    }

    /// A cell's rectangle.
    pub fn rect(&self, (col, row): Slot) -> Rect {
        Rect::new(
            self.x0 + col as f32 * self.pitch,
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
        if self.pitch <= 0.0 {
            return None;
        }
        let (fx, fy) = ((x - self.x0) / self.pitch, (y - MARGIN) / self.ch);
        if fx < 0.0 || fy < 0.0 {
            return None;
        }
        let slot = (fx as usize, fy as usize);
        // (The sliver of air between two columns is no one's.)
        let within = x - self.x0 - slot.0 as f32 * self.pitch < self.cw;
        (within && self.contains(slot)).then_some(slot)
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
    // What is plugged in (a stick, a phone) stands on the OTHER side: the
    // first free cell from the top-right, column by column leftwards — the
    // files keep the left (Max, 2026-10-08).
    let from_right: Vec<Slot> = (0..grid.cols)
        .rev()
        .flat_map(|c| (0..grid.rows).map(move |r| (c, r)))
        .collect();
    for (i, item) in items.iter().enumerate() {
        if slots[i].is_none() && item.kind == Kind::Volume {
            if let Some(slot) = from_right.iter().copied().find(|s| !taken.contains(s)) {
                taken.insert(slot);
                slots[i] = Some(slot);
            }
        }
    }
    let mut free = grid.slots().filter(|s| !taken.contains(s));
    for (i, slot) in slots.iter_mut().enumerate() {
        if slot.is_none() && items[i].kind != Kind::Volume {
            *slot = free.next();
        }
    }
    slots
}

/// Record every placed item's cell in `remembered`; whether anything new
/// was learned (and the store needs writing).
///
/// A cell remembered OFF the grid is kept: the grid is only smaller for now
/// (a resolution being tried, a bigger icon size) and the icon goes back
/// when it grows again — "a scale change moves nothing".
pub(crate) fn stick(
    items: &[Item],
    slots: &[Option<Slot>],
    grid: &Grid,
    remembered: &mut HashMap<String, Slot>,
) -> bool {
    let mut learned = false;
    for (item, slot) in items.iter().zip(slots) {
        let Some(slot) = slot else { continue };
        if remembered.get(&item.path).is_some_and(|r| !grid.contains(*r)) {
            continue;
        }
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
    buffer: WlBuffer,
    _pool: RawPool,
}

impl DragIcon {
    /// The surface the compositor carries under the pointer.
    pub(crate) fn surface(&self) -> &WlSurface {
        &self.surface
    }
}

impl Drop for DragIcon {
    fn drop(&mut self) {
        self.surface.destroy();
        // A dropped proxy sends nothing: without this the compositor kept
        // every drag's buffer for the daemon's life.
        self.buffer.destroy();
    }
}

/// An item's name being typed, in place: the desktop holds the keyboard
/// for exactly this long (the mockup, 2026-10-07).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rename {
    pub item: usize,
    pub text: String,
    /// The whole name is selected (as it opens): the first key replaces
    /// it, a Backspace clears it.
    pub all: bool,
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
    /// Where on the grabbed icon the press landed: the compositor's drag
    /// image sits at the pointer less this, and so do the rest of the
    /// group, drawn by us around it.
    pub grip: (f32, f32),
    /// Where the pointer was when the icon was lifted: the group's place
    /// until the drag's first motion, since the `enter` Hyprland sends
    /// carries the surface's centre as its position, not the pointer's.
    pub pos: (f32, f32),
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
    /// The right-click menu, while it is up.
    pub menu: Option<Menu>,
    /// What the menu's "Move to" page lists (`desktop_send.rs`), in the
    /// order of its `SendTo(n)`: the sticks and phones, then the other
    /// computers. `looking`: the devices have been asked for and not
    /// answered yet.
    pub targets: Vec<crate::desktop_send::Target>,
    pub looking: bool,
    /// The phones being used as a camera: volume path → the pid of the
    /// scrcpy feeding the camera device (shared with its waiting thread).
    pub cameras: std::sync::Arc<std::sync::Mutex<HashMap<String, u32>>>,
    /// What is plugged in and mounted, standing on the desktop as icons.
    pub volumes: Vec<crate::mounts::Mounted>,
    /// The Properties box, while it is up.
    pub props: Option<Props>,
    /// The last plain click on an item (its path, and when): a second one
    /// on the same item soon enough is a double click, which opens it.
    pub last_click: Option<(String, std::time::Instant)>,
    /// When the pointer's shape was last sent: it is sent again after a
    /// while even when nothing changed on our side, because the waveview
    /// plugin paints its edge-resize arrows straight onto the pointer (not
    /// through the protocol) and does not always take them off when the
    /// pointer comes onto the desktop — our cached "already the arrow" then
    /// kept a stale shape for good (Max, 2026-10-08: "still stuck").
    pub cursor_sent: Option<std::time::Instant>,
    /// An item's name being typed.
    pub rename: Option<Rename>,
    /// The desktop holds the keyboard: from a click on it until the pointer
    /// leaves it, or a window opens (see `App::desktop_take_keys`).
    pub keys: bool,
    /// The icons are put away (a click on bare wallpaper toggles it); the
    /// files stay. Kept in the settings store.
    pub hidden: bool,
    /// How much of the icons is on screen, 0..1, easing toward `hidden`'s
    /// opposite: they fade rather than blink.
    pub shown: f32,
    /// Devices plugged in WHILE the icons were put away: they show alone,
    /// once, until the next click on the wallpaper brings the rest back
    /// beside them (Max, 2026-10-08: "solo, but just once, when I connect them;
    /// after that they behave as the others").
    pub solo: HashSet<String>,
    pub solo_on: bool,
    /// How much of them is on screen, 0..1.
    pub solo_t: f32,
    /// The volume service has reported once: what it lists first was
    /// already plugged in, not just connected.
    pub mounts_seen: bool,
    /// A reload is on its way (see `desktop_reload_soon`).
    pub reload_pending: bool,
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
    // A shortcut wears its app's name and icon and RUNS on a double click
    // — only once it is trusted: marked executable (what "Allow to run"
    // does, and what a shortcut made on this computer is), or an installed
    // app's own (`App::reload_desktop` lifts those). Until then it is a
    // file like any other, under its real name: a downloaded
    // "Invoice.pdf" with a PDF's icon and a command of its own must not
    // pass for a document.
    if name.ends_with(".desktop") && is_executable(&path) {
        if let Some(app) = waverunner_core::index::parse_desktop_file(&path) {
            return launcher_item(path_str, app);
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

/// A trusted shortcut's item.
fn launcher_item(path: String, app: waverunner_core::index::AppEntry) -> Item {
    Item {
        path,
        name: app.name,
        kind: Kind::Launcher,
        icon: app
            .icon
            .map(|i| format!("app:{i}"))
            .unwrap_or_else(|| asset_key("asset-file")),
        exec: Some((app.exec, app.needs_terminal)),
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// The shortcut an item WOULD be, were it let run: a `.desktop` file shown
/// as a plain file because nothing vouches for it yet.
pub(crate) fn locked_shortcut(item: &Item) -> Option<waverunner_core::index::AppEntry> {
    (item.kind == Kind::File && item.path.ends_with(".desktop"))
        .then(|| waverunner_core::index::parse_desktop_file(Path::new(&item.path)))
        .flatten()
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

/// The item an arrow key goes to from the cell `from`: the nearest one that
/// way (`step` is one of the four directions), a row or column off counting
/// double so the straight neighbour wins. With nothing selected (`from`
/// none): the first item, by column then row.
pub(crate) fn neighbour(slots: &[Option<Slot>], from: Option<Slot>, step: (i32, i32)) -> Option<usize> {
    let placed = slots.iter().enumerate().filter_map(|(i, s)| s.map(|s| (i, s)));
    let Some(from) = from else {
        return placed.min_by_key(|(_, s)| *s).map(|(i, _)| i);
    };
    placed
        .filter_map(|(i, (c, r))| {
            let (dc, dr) = (c as i32 - from.0 as i32, r as i32 - from.1 as i32);
            let (along, across) = if step.0 != 0 { (dc * step.0, dr.abs()) } else { (dr * step.1, dc.abs()) };
            (along > 0).then_some((along + 2 * across, i))
        })
        .min()
        .map(|(_, i)| i)
}

/// A phone's serial out of how the volume service names its mount
/// (`mtp://Google_Pixel_8_Pro_3A301FDJG000UW/`: the last part) — what
/// `adb` and `scrcpy` know it by. `None` when it cannot be told.
pub(crate) fn phone_serial(uri: &str) -> Option<String> {
    let host = uri.split("://").nth(1)?.split('/').next()?;
    let host = host.strip_prefix('[').unwrap_or(host);
    let serial = host.rsplit('_').next()?;
    (serial.len() >= 6 && serial.chars().all(|c| c.is_ascii_alphanumeric())).then(|| serial.to_owned())
}

/// What scrcpy's window calls itself: its own name, and the name of the
/// wrapper it runs under when it comes from the nix store.
const MIRROR_CLASSES: [&str; 2] = ["scrcpy", ".scrcpy-wrapped"];
/// How much of the screen's free height a phone's mirror stands in.
const MIRROR_HEIGHT: f64 = 0.72;
/// A phone's shape when it would not say: 9 by 20, what most are near.
const PHONE_SHAPE: (f64, f64) = (9.0, 20.0);

/// The phone's screen, in its own pixels, short side first (`adb shell wm
/// size`; the last line is the size in force). The usual shape when it
/// cannot be asked.
fn phone_shape(serial: Option<&str>) -> (f64, f64) {
    let mut cmd = std::process::Command::new("adb");
    if let Some(serial) = serial {
        cmd.args(["-s", serial]);
    }
    cmd.args(["shell", "wm", "size"]).stderr(std::process::Stdio::null());
    cmd.output()
        .ok()
        .and_then(|out| parse_wm_size(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or(PHONE_SHAPE)
}

/// Whether a phone lets this computer in over `adb`.
#[derive(Debug, PartialEq)]
enum Debugging {
    Ready,
    /// The phone is plugged in but `adb` does not see it: debugging is off.
    Off,
    /// It is seen, and waits for the owner to allow this computer.
    NotAllowed,
    /// `adb` itself could not be asked; scrcpy is left to find out.
    Unknown,
}

fn phone_debugging(serial: &str) -> Debugging {
    // The first call starts adb's server and a phone takes a moment to
    // show up in it: ask again before saying it is not there.
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(700));
        }
        let Ok(out) = std::process::Command::new("adb")
            .arg("devices")
            .stderr(std::process::Stdio::null())
            .output()
        else {
            return Debugging::Unknown;
        };
        match debugging_of(&String::from_utf8_lossy(&out.stdout), serial) {
            Debugging::Off => {}
            other => return other,
        }
    }
    Debugging::Off
}

/// `adb devices`' answer for one phone (`<serial>\tdevice|unauthorized|…`).
fn debugging_of(listing: &str, serial: &str) -> Debugging {
    let state = listing.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        (parts.next() == Some(serial)).then(|| parts.next().unwrap_or(""))
    });
    match state {
        None => Debugging::Off,
        Some("device") => Debugging::Ready,
        Some("unauthorized") => Debugging::NotAllowed,
        Some(_) => Debugging::Unknown,
    }
}

/// `Physical size: 1344x2992` (and perhaps `Override size: …` after it) →
/// the last size said, short side first.
fn parse_wm_size(said: &str) -> Option<(f64, f64)> {
    let (a, b) = said.lines().rev().find_map(|l| l.rsplit(' ').next()?.trim().split_once('x'))?;
    let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
    (a > 0.0 && b > 0.0).then_some((a.min(b), a.max(b)))
}

/// The mirror's window for a phone of `shape` on a screen with `usable`
/// logical px of free height: standing, `MIRROR_HEIGHT` of it tall.
fn mirror_size((short, long): (f64, f64), usable: f64) -> (i64, i64) {
    let h = (usable * MIRROR_HEIGHT).round();
    ((h * short / long).round() as i64, h as i64)
}

/// What to tell the owner when a phone does not let this computer in,
/// asked before anything is started so it is said at once and exactly
/// (Max, 2026-10-09: "turn on USB debugging to use this feature").
fn phone_refuses(name: &str, serial: &str) -> Option<String> {
    let what = match phone_debugging(serial) {
        Debugging::Off => "turn on USB debugging on the phone (Settings, Developer options), then try again.",
        Debugging::NotAllowed => "allow USB debugging for this computer on the phone, then try again.",
        Debugging::Ready | Debugging::Unknown => return None,
    };
    info!("desktop: {name} does not let us in: {what}");
    Some(format!("{name}: {}", crate::i18n::tr(what)))
}

/// The camera device a phone's picture is fed into: a loopback one
/// (Golem's is "Android WebCam", /dev/video10), which every app then sees
/// as a camera. `None` on a system without one.
fn camera_device() -> Option<std::path::PathBuf> {
    let mut found: Vec<(bool, String)> = std::fs::read_dir("/sys/devices/virtual/video4linux")
        .ok()?
        .flatten()
        .filter_map(|e| {
            let node = e.file_name().to_string_lossy().into_owned();
            let name = std::fs::read_to_string(e.path().join("name")).unwrap_or_default();
            node.starts_with("video").then(|| (!name.contains("Android"), node))
        })
        .collect();
    found.sort();
    found.first().map(|(_, node)| std::path::Path::new("/dev").join(node))
}

/// The name of the camera node we put in the session's media service.
const CAMERA_NODE: &str = "golem-phone-camera";

/// Make the phone's camera one the apps can SEE. The loopback device only
/// says it is a camera while something feeds it, and the media service
/// (PipeWire) looked at it once, at boot, when nothing did: it has the
/// device and no camera on it, so Cheese, the browser's picker and the rest
/// listed only the built-in one (Max, 2026-10-09). Now that it is fed, a
/// source is made on it by hand, under the phone's own name; it lasts until
/// `camera_withdraw`.
fn camera_announce(name: &str, device: &std::path::Path) {
    camera_withdraw(); // one left by a dock that went away mid-camera
    let name: String = name.chars().filter(|c| !"\"\\{}".contains(*c)).collect();
    let props = format!(
        "{{ factory.name=api.v4l2.source api.v4l2.path={} node.name={CAMERA_NODE} \
         node.description=\"{name}\" node.nick=\"{name}\" media.class=Video/Source \
         media.role=Camera object.linger=true }}",
        device.display()
    );
    let made = std::process::Command::new("pw-cli")
        .args(["create-node", "spa-node-factory", &props])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !made.as_ref().is_ok_and(|s| s.success()) {
        warn!("desktop: the phone's camera could not be announced to the media service: {made:?}");
    }
}

/// Take our camera node away again (the phone stopped being one).
fn camera_withdraw() {
    let Ok(out) = std::process::Command::new("pw-dump").stderr(std::process::Stdio::null()).output() else {
        return;
    };
    let Ok(all) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return;
    };
    for object in all.as_array().into_iter().flatten() {
        if object["info"]["props"]["node.name"].as_str() != Some(CAMERA_NODE) {
            continue;
        }
        if let Some(id) = object["id"].as_u64() {
            let _ = std::process::Command::new("pw-cli")
                .args(["destroy", &id.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

/// Whether the loopback device is being fed (its `state` in sysfs reads
/// `capture` then, `output` while idle). On a driver without that file,
/// taken as fed: there is nothing better to ask.
fn camera_fed(device: &std::path::Path) -> bool {
    let Some(node) = device.file_name() else {
        return true;
    };
    match std::fs::read_to_string(std::path::Path::new("/sys/class/video4linux").join(node).join("state")) {
        Ok(state) => state.trim() == "capture",
        Err(_) => true,
    }
}

/// How long the camera's frames are held to even out their pace (ms).
const CAMERA_BUFFER_MS: u32 = 50;

/// How long scrcpy is given to start feeding the camera device.
const CAMERA_PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);

/// Use a phone's back camera as this computer's (scrcpy into the loopback
/// camera device, no window), until it is stopped from the same menu row
/// or the phone goes. What to tell the owner — that it is on, or why not.
fn camera(
    name: &str,
    serial: Option<&str>,
    path: &str,
    cameras: &std::sync::Mutex<HashMap<String, u32>>,
) -> Option<String> {
    use crate::i18n::tr;
    if let Some(said) = serial.and_then(|s| phone_refuses(name, s)) {
        return Some(said);
    }
    let Some(device) = camera_device() else {
        warn!("desktop: no loopback camera device on this system");
        return Some(tr("This computer has no camera device for a phone to use.").to_owned());
    };
    let mut cmd = std::process::Command::new("scrcpy");
    cmd.args(["--video-source=camera", "--camera-facing=back", "--camera-ar=16:9", "--max-size=1920"])
        // 60 pictures a second, where a webcam gives 30 (Max, 2026-10-09: at
        // 30 it "feels even less smooth" than the built-in one); the phone
        // was measured sending 1080p at 60 with none late or repeated. And
        // twice scrcpy's usual bit rate, for the picture's sake.
        .args(["--camera-fps=60", "--video-bit-rate=16M", "--no-audio", "--no-window"])
        // Frames come off the phone up to 20 ms early or late (measured);
        // held this long they go out evenly. (80 ms was tried first, on a
        // day the picture was choppy for another reason, and judged worse.)
        .arg(format!("--v4l2-buffer={CAMERA_BUFFER_MS}"))
        .arg(format!("--v4l2-sink={}", device.display()));
    if let Some(serial) = serial {
        cmd.arg(format!("--serial={serial}"));
    }
    // It goes when the dock goes: left behind, it kept the phone's camera
    // on with no row anywhere to stop it.
    // SAFETY: only an async-signal-safe call between fork and exec.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = match cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() {
        Ok(child) => child,
        Err(e) => {
            warn!("desktop: scrcpy could not be run: {e}");
            return Some(tr("The screen mirror (scrcpy) is not installed.").to_owned());
        }
    };
    if let Ok(mut on) = cameras.lock() {
        on.insert(path.to_owned(), child.id());
    }
    // It is a camera from the moment the device says it is fed, not
    // before (announced on a fixed wait, it was sometimes not a camera yet
    // and the apps got nothing).
    let asked = std::time::Instant::now();
    let mut started = false;
    while !started && asked.elapsed() < CAMERA_PATIENCE && matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(std::time::Duration::from_millis(200));
        started = camera_fed(&device);
    }
    let early = match child.try_wait().ok().flatten() {
        None if !started => {
            warn!("desktop: scrcpy never fed the camera device for {name}");
            let _ = child.kill();
            child.wait().ok()
        }
        other => other,
    };
    if early.is_none() {
        camera_announce(name, &device);
        crate::desktop_send_notify(&format!(
            "{name}: {}",
            tr("is this computer's camera now: pick it by its name in any app. Its menu stops it.")
        ));
    }
    let status = early.or_else(|| child.wait().ok());
    camera_withdraw();
    let by_hand = cameras.lock().ok().is_none_or(|mut on| on.remove(path).is_none());
    info!("desktop: {name} is no longer the camera ({status:?}, by hand: {by_hand})");
    match early {
        Some(status) if !status.success() && !by_hand => {
            warn!("desktop: scrcpy's camera gave up on {name} ({status})");
            Some(format!("{name}: {}", tr("its camera could not be used (this needs Android 12 or newer).")))
        }
        _ => None,
    }
}

/// Show a phone's screen in a window (scrcpy), until that window is
/// closed. What to tell the owner if it could not: the program is missing,
/// or the phone would not let it in.
fn mirror(name: &str, serial: Option<&str>) -> Option<String> {
    if let Some(said) = serial.and_then(|s| phone_refuses(name, s)) {
        return Some(said);
    }
    // The window is the phone's own shape, standing: left to itself it
    // opens at the size of any other window, the picture lost in it (Max,
    // 2026-10-09: "the window is huge").
    if let Ok(mon) = crate::hypr::focused_monitor() {
        let usable = mon.h - mon.reserved.1 - mon.reserved.3;
        let (w, h) = mirror_size(phone_shape(serial), usable);
        for class in MIRROR_CLASSES {
            crate::window_memory::declare_sized(class, w, h);
        }
    }
    let mut cmd = std::process::Command::new("scrcpy");
    cmd.arg(format!("--window-title={name}"));
    if let Some(serial) = serial {
        cmd.arg(format!("--serial={serial}"));
    }
    let started = std::time::Instant::now();
    match cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status() {
        Err(e) => {
            warn!("desktop: scrcpy could not be run: {e}");
            Some(crate::i18n::tr("The screen mirror (scrcpy) is not installed.").to_owned())
        }
        // It ended at once without a window: the phone did not answer adb.
        Ok(status) if !status.success() && started.elapsed().as_secs() < 8 => {
            warn!("desktop: scrcpy gave up on {name} ({status})");
            Some(format!(
                "{name}: {}",
                crate::i18n::tr("turn on USB debugging in the phone's developer options, and allow this computer when the phone asks.")
            ))
        }
        Ok(_) => None,
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
    crate::desktop_send::free_dest(dir, name)
}


/// Bring `src` to `dest`: a move where one filesystem allows it, else a
/// copy (a file from another volume stays there too — what a desktop does
/// with a file from a stick).
fn bring(src: &Path, dest: &Path, may_move: bool) -> std::io::Result<()> {
    if may_move && std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    crate::desktop_send::copy_all(src, dest)
}

/// Bring every file of `paths` into `dir` (one already there stays as it
/// is); the paths they now have, in order.
pub(crate) fn import(paths: &[PathBuf], dir: &Path, may_move: bool) -> Vec<PathBuf> {
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
        if crate::desktop_send::inside(dir, src) {
            warn!("desktop: {} holds the desktop; it cannot be put on it", src.display());
            continue;
        }
        let dest = unique_dest(dir, name);
        match bring(src, &dest, may_move) {
            Ok(()) => {
                info!("desktop: {} → {}", src.display(), dest.display());
                out.push(dest);
            }
            Err(e) => {
                warn!("desktop: cannot bring {} in: {e}", src.display());
                // Half a copy is worse than none.
                let _ = if dest.is_dir() {
                    std::fs::remove_dir_all(&dest)
                } else {
                    std::fs::remove_file(&dest)
                };
            }
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
    /// Something drawn over the icons (the menu): a name under it is left
    /// out, since names are painted last of all and would show through.
    cover: Option<Rect>,
    /// This item's name is being typed: `(text, all selected, text width)`
    /// — a field replaces the name.
    rename: Option<(&'a str, bool, f32)>,
}

/// The name field while typing: its pad around the text, corner, colours.
const FIELD_PAD_X: f32 = 7.0;
const FIELD_PAD_Y: f32 = 2.0;
const FIELD_RADIUS: f32 = 7.0;
const FIELD_BG: [f32; 4] = [0.0, 0.0, 0.0, 0.6];
const FIELD_RIM: [f32; 4] = [1.0, 1.0, 1.0, 0.18];
const FIELD_SEL: [f32; 4] = [1.0, 1.0, 1.0, 0.22];

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
    let name_rect = Rect::new(cx - tile.max_w / 2.0, top, tile.max_w, LABEL_LINE_PX + 1.0);
    if tile.cover.is_some_and(|c| intersects(&c, &name_rect)) {
        return;
    }
    if let Some((text, all, w)) = tile.rename {
        // The field: a dark pill around the text (never narrower than the
        // name's room), the text washed while it is all selected, a caret.
        let inner_w = w.max(24.0);
        let field = Rect::new(
            cx - inner_w / 2.0 - FIELD_PAD_X,
            top - FIELD_PAD_Y,
            inner_w + 2.0 * FIELD_PAD_X,
            LABEL_LINE_PX + 2.0 * FIELD_PAD_Y,
        );
        for (color, border) in [(FIELD_BG, 0.0), (FIELD_RIM, 1.0)] {
            scene.rects.push(crate::content::RectInst {
                rect: field,
                radius: FIELD_RADIUS,
                color,
                glass: 0.0,
                border,
            });
        }
        if all && w > 0.0 {
            scene.rects.push(crate::content::RectInst {
                rect: Rect::new(cx - w / 2.0, top + 1.0, w, LABEL_LINE_PX - 2.0),
                radius: 2.0,
                color: FIELD_SEL,
                glass: 0.0,
                border: 0.0,
            });
        }
        scene.rects.push(crate::content::RectInst {
            rect: Rect::new(cx + w / 2.0 + 1.0, top + 2.0, 1.0, LABEL_LINE_PX - 4.0),
            radius: 0.0,
            color: INK,
            glass: 0.0,
            border: 0.0,
        });
        scene.labels.push(Label {
            text: text.to_owned(),
            pos: (cx, top),
            max_w: inner_w + 2.0,
            font_px: LABEL_FONT_PX,
            line_px: LABEL_LINE_PX,
            centered: true,
            dim: false,
            cache: false,
            clip: None,
            family: None,
            color: Some(INK),
        });
        return;
    }
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
    /// The items in hand, the grabbed one first: their cells are left
    /// empty. The compositor carries the grabbed one's picture under the
    /// pointer; the rest travel with it, drawn here.
    pub in_hand: &'a [usize],
    /// Each item's texture layer (see `Desktop::layer_of`); empty: its place
    /// in the list.
    pub layers: &'a [u32],
    /// Where the grabbed icon is while the drag is over the desktop (its
    /// top-left), so the group can be drawn around it; `None` when the
    /// drag is elsewhere (over a window, the dock…), where the desktop is
    /// covered anyway.
    pub carried: Option<(f32, f32)>,
    /// When set, only these items (by path) are drawn: the devices shown
    /// alone over put-away icons.
    pub only: Option<&'a HashSet<String>>,
    /// Which items are selected (washed).
    pub selected: &'a [bool],
    /// A rubber band being drawn.
    pub band: Option<Rect>,
    /// The menu that is up, drawn over everything, and what it is painted
    /// with (the boxes' surface, read where it is).
    pub menu: Option<(&'a Menu, MenuPaint)>,
    /// The Properties box that is up, drawn the same way.
    pub props: Option<(&'a Props, MenuPaint)>,
    /// A name being typed: the item, the text, whether all of it is
    /// selected, and the text's measured width.
    pub rename: Option<(usize, &'a str, bool, f32)>,
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
            // In hand: drawn where it travels — at its old offset from the
            // grabbed one, which sits at the pointer. The grabbed one's own
            // picture is the compositor's drag image; only its name is
            // drawn, under that image (Max, 2026-10-07: "only one moves,
            // the others move invisible").
            let (Some(origin), Some(&lead)) = (live.carried, live.in_hand.first()) else {
                continue;
            };
            let (Some(slot), Some(lead_slot)) = (
                slots.get(i).copied().flatten(),
                slots.get(lead).copied().flatten(),
            ) else {
                continue;
            };
            let (own, lead_icon) = (
                icon_rect(&grid.rect(slot), icon_scale),
                icon_rect(&grid.rect(lead_slot), icon_scale),
            );
            let icon = Rect::new(
                origin.0 + own.x - lead_icon.x,
                origin.1 + own.y - lead_icon.y,
                own.w,
                own.h,
            );
            push_tile(
                &mut scene,
                &Tile {
                    item,
                    layer: live.layers.get(i).copied().unwrap_or(i as u32),
                    icon,
                    name: names.get(i).map_or(item.name.as_str(), String::as_str),
                    max_w: label_max_w(&grid.rect(slot)),
                    has_icon: i != lead && has_icon.get(i).copied().unwrap_or(false),
                    overlay: true,
                    cover: None,
                    rename: None,
                },
            );
            continue;
        }
        let Some(slot) = slots.get(i).copied().flatten() else {
            continue;
        };
        let cell = grid.rect(slot);
        if live.only.is_some_and(|only| !only.contains(&item.path)) {
            continue;
        }
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
                layer: live.layers.get(i).copied().unwrap_or(i as u32),
                icon: icon_rect(&cell, icon_scale),
                name: names.get(i).map_or(item.name.as_str(), String::as_str),
                max_w: label_max_w(&cell),
                has_icon: has_icon.get(i).copied().unwrap_or(false),
                overlay: false,
                cover: live.menu.map(|(m, _)| m.rect).or(live.props.map(|(p, _)| p.rect)),
                rename: live
                    .rename
                    .filter(|(r, ..)| *r == i)
                    .map(|(_, text, all, w)| (text, all, w)),
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
    // The menu, over everything.
    if let Some((menu, paint)) = live.menu {
        menu.push(&mut scene, &paint);
    }
    if let Some((props, paint)) = live.props {
        props.push(&mut scene, &paint);
    }
    scene
}

impl Desktop {
    /// Whether item `i`'s picture is in its texture layer.
    /// The texture layer holding item `i`'s picture, once it is there. A
    /// layer belongs to a PICTURE, not to a place in the list: fifty folders
    /// share one, and a file arriving (or a stick mounting, which goes in
    /// front) moves no one's — it used to re-upload every icon after it.
    fn layer_of(&self, i: usize) -> Option<u32> {
        let key = self.items.get(i)?.icon.as_str();
        self.uploaded
            .iter()
            .position(|u| u.as_deref() == Some(key))
            .map(|l| l as u32)
    }

    fn has_icon(&self, i: usize) -> bool {
        self.layer_of(i).is_some()
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

    /// The item under `pos`, if any (none while the icons are put away:
    /// what cannot be seen cannot be hit).
    pub(crate) fn hit(&self, pos: (f32, f32)) -> Option<usize> {
        let slot = self.grid.slot_at(pos)?;
        let i = self.slots.iter().position(|s| *s == Some(slot))?;
        // Put away, only a device shown alone is there to be hit.
        (!self.hidden || (self.solo_on && self.solo.contains(&self.items[i].path))).then_some(i)
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
    let mut all: HashMap<String, Slot> =
        crate::persist::read_json(&crate::persist::data_path(POSITIONS_FILE)).unwrap_or_default();
    // A plugged-in volume's cell is for as long as it is in: at a start it
    // takes its place on the right anew.
    all.retain(|p, _| !(p.starts_with("/run/media/") || p.starts_with("/media/") || p.contains("/gvfs/")));
    all
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
            self.desktop.hidden = self.settings.desktop_hidden;
            self.desktop.shown = if self.desktop.hidden { 0.0 } else { 1.0 };
            self.reload_desktop();
        } else {
            self.relayout_desktop();
        }
    }

    /// Do `job` off the loop (a copy takes as long as it takes, and the
    /// whole shell waits on this thread) and `then` with what it gives,
    /// back here.
    fn desktop_off_loop<R: Send + 'static>(
        &mut self,
        job: impl FnOnce() -> R + Send + 'static,
        mut then: impl FnMut(&mut App, R) + 'static,
    ) {
        let (tx, rx) = calloop::channel::channel::<R>();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
        let waiting = self.loop_handle.insert_source(rx, move |event, _, app: &mut App| {
            if let calloop::channel::Event::Msg(r) = event {
                then(app, r);
            }
        });
        if waiting.is_err() {
            warn!("desktop: cannot wait for work done off the loop");
        }
    }

    /// The folder changed: read it again in a moment — once for a burst
    /// (a hundred files copied in said so two hundred times).
    pub(crate) fn desktop_reload_soon(&mut self) {
        if self.desktop.reload_pending {
            return;
        }
        self.desktop.reload_pending = true;
        let timer = calloop::timer::Timer::from_duration(std::time::Duration::from_millis(40));
        let armed = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            app.desktop.reload_pending = false;
            app.reload_desktop();
            calloop::timer::TimeoutAction::Drop
        });
        if armed.is_err() {
            self.desktop.reload_pending = false;
            self.reload_desktop();
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
        // What is plugged in comes first (it takes the first free cells
        // the first time; after that, wherever it was put).
        let mut volumes: Vec<Item> = self
            .desktop
            .volumes
            .iter()
            .map(|v| Item {
                path: v.path.to_string_lossy().into_owned(),
                name: v.name.clone(),
                kind: Kind::Volume,
                icon: format!("asset:{}", if v.phone { "phone" } else { "drive-removable-media-usb" }),
                exec: None,
            })
            .collect();
        volumes.append(&mut items);
        let mut items = volumes;
        // A shortcut that is an installed app's own (the same file name and
        // the same command as one the dock indexed) is vouched for by that.
        for item in items.iter_mut() {
            let Some(app) = locked_shortcut(item) else {
                continue;
            };
            let file = Path::new(&item.path).file_name().map(|n| n.to_os_string());
            let installed = self.entries.iter().any(|e| {
                e.exec == app.exec && e.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_os_string()) == file
            });
            if installed {
                *item = launcher_item(item.path.clone(), app);
            }
        }
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
            if !self.desktop.chains.contains_key(&key) {
                if let Some((_, pixels)) = self.thumb_map.get(&item.path) {
                    self.desktop.chains.insert(key.clone(), pixels.clone());
                }
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
        // Whatever is up holds an item by its PLACE in the list, and the list
        // has just been made anew (a file arrived, a stick mounted: every
        // place after it moved). Each is carried over by its path, or let go
        // of if its file went — Enter in a rename must never name another.
        let old_path = |i: usize| self.desktop.items.get(i).map(|it| it.path.clone());
        let now_at = |path: Option<String>| path.and_then(|p| items.iter().position(|it| it.path == p));
        let rename_to = self.desktop.rename.as_ref().map(|r| now_at(old_path(r.item)));
        let menu_to = self
            .desktop
            .menu
            .as_ref()
            .and_then(|m| m.item)
            .map(|i| now_at(old_path(i)));
        let press_to = self
            .desktop
            .press
            .and_then(|p| p.item)
            .map(|i| now_at(old_path(i)));
        let props_gone = self
            .desktop
            .props
            .as_ref()
            .is_some_and(|p| !items.iter().any(|it| it.path == p.path));
        self.desktop.items = items;
        match rename_to {
            Some(Some(i)) => {
                if let Some(r) = self.desktop.rename.as_mut() {
                    r.item = i;
                }
            }
            Some(None) => self.desktop_end_rename(false),
            None => {}
        }
        match menu_to {
            Some(Some(i)) => {
                if let Some(m) = self.desktop.menu.as_mut() {
                    m.item = Some(i);
                }
            }
            Some(None) => self.desktop.menu = None,
            None => {}
        }
        match press_to {
            Some(Some(i)) => {
                if let Some(p) = self.desktop.press.as_mut() {
                    p.item = Some(i);
                }
            }
            Some(None) => self.desktop.press = None,
            None => {}
        }
        if props_gone {
            self.desktop.props = None;
        }
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
        if stick(&self.desktop.items, &self.desktop.slots, &self.desktop.grid, &mut self.desktop.remembered) {
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
        // The pictures worn now, each once, in the items' order.
        let mut worn: Vec<String> = Vec::new();
        for item in &self.desktop.items {
            if !worn.contains(&item.icon) {
                worn.push(item.icon.clone());
            }
        }
        if worn.is_empty() {
            return;
        }
        // A layer whose picture nothing wears any more is free again.
        for held in self.desktop.uploaded.iter_mut() {
            if held.as_ref().is_some_and(|k| !worn.contains(k)) {
                *held = None;
            }
        }
        // Size the array for the pictures plus headroom, so one arriving is
        // a single-layer write and not a reallocation — which also clears
        // the array, hence `uploaded` is forgotten with it.
        if worn.len() as u32 > self.desktop.capacity {
            self.desktop.capacity = worn.len() as u32 + LAYER_HEADROOM;
            if let Some(r) = self.desktop_renderer.as_mut() {
                r.alloc_icon_array(self.desktop.capacity);
            }
            self.desktop.uploaded.clear();
        }
        self.desktop
            .uploaded
            .resize(self.desktop.capacity as usize, None);
        let mut asked = Vec::new();
        for key in worn {
            if self.desktop.uploaded.iter().any(|u| u.as_deref() == Some(key.as_str())) {
                continue;
            }
            if let Some(chain) = self.desktop.chains.get(&key) {
                let free = self.desktop.uploaded.iter().position(Option::is_none);
                if let (Some(layer), Some(r)) = (free, self.desktop_renderer.as_mut()) {
                    r.update_icon_layer(layer as u32, chain);
                    self.desktop.uploaded[layer] = Some(key);
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
        // (A picture taken anew for the same file: its layer is written again.)
        for held in self.desktop.uploaded.iter_mut() {
            if held.as_deref() == Some(key.as_str()) {
                *held = None;
            }
        }
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

    /// Open the right-click menu at `at`: an item's (which becomes the
    /// selection, unless it is in it already) or the wallpaper's.
    pub(crate) fn desktop_open_menu(&mut self, at: (f32, f32)) {
        let item = self.desktop.hit(at);
        if let Some(i) = item {
            let path = &self.desktop.items[i].path;
            if !self.desktop.selected.contains(path) {
                self.desktop.selected.clear();
                self.desktop.selected.insert(path.clone());
            }
        }
        self.desktop.press = None;
        self.desktop.band = None;
        self.desktop.props = None;
        let (w, h) = self.desktop_size;
        // Several selected (the clicked one among them): the group's menu.
        let many = item.is_some() && self.desktop.selected.len() > 1;
        let mut menu = Menu::open(item, many, at, w as f32, h as f32);
        // On a plugged-in volume (alone): its own short menu.
        let on_volume = item
            .and_then(|i| self.desktop.items.get(i))
            .is_some_and(|it| it.kind == Kind::Volume);
        if on_volume && !many {
            let phone = item
                .and_then(|i| self.desktop.items.get(i))
                .is_some_and(|it| self.desktop.volumes.iter().any(|v| v.phone && v.path.as_os_str() == it.path.as_str()));
            let camera = item
                .and_then(|i| self.desktop.items.get(i))
                .is_some_and(|it| self.desktop.cameras.lock().is_ok_and(|c| c.contains_key(&it.path)));
            menu = menu.for_volume(phone, camera, h as f32);
        }
        // On a shortcut nothing vouches for: letting it run comes first.
        let locked = item
            .and_then(|i| self.desktop.items.get(i))
            .is_some_and(|it| locked_shortcut(it).is_some());
        if locked && !many {
            menu = menu.with_allow(h as f32);
        }
        // On the wallpaper, with files on the clipboard: they can be put here.
        if item.is_none() && self.clipboard_files().is_some() {
            menu = menu.with_paste(h as f32);
        }
        menu.hover = menu.hit(at);
        self.desktop.menu = Some(menu);
        self.request_desktop_draw();
    }

    /// What is plugged in changed (`mounts.rs`): its icons follow.
    pub(crate) fn on_mounts(&mut self, list: Vec<crate::mounts::Mounted>) {
        info!(
            "desktop: plugged in and mounted: {:?}",
            list.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()
        );
        // A volume that left takes its remembered cell with it: the same
        // stick comes back to the first free cells, not to a stale one.
        let gone: Vec<String> = self
            .desktop
            .volumes
            .iter()
            .filter(|v| !list.contains(v))
            .map(|v| v.path.to_string_lossy().into_owned())
            .collect();
        for path in gone {
            self.desktop.remembered.remove(&path);
            self.desktop.selected.remove(&path);
            self.desktop.solo.remove(&path);
        }
        // Connected just now, with the icons put away: it shows alone, once.
        if self.desktop.mounts_seen && self.desktop.hidden {
            let new: Vec<String> = list
                .iter()
                .filter(|v| !self.desktop.volumes.contains(v))
                .map(|v| v.path.to_string_lossy().into_owned())
                .collect();
            if !new.is_empty() {
                self.desktop.solo.extend(new);
                self.desktop.solo_on = true;
            }
        }
        self.desktop.mounts_seen = true;
        self.desktop.volumes = list;
        self.save_desktop_positions();
        self.reload_desktop();
    }

    /// Whether `path` is a plugged-in volume's folder.
    fn desktop_is_volume(&self, path: &str) -> bool {
        self.desktop.volumes.iter().any(|v| v.path.as_os_str() == path)
    }

    /// Let the volume at `path` go.
    fn desktop_eject(&mut self, path: &str) {
        if let Some(v) = self.desktop.volumes.iter().find(|v| v.path.as_os_str() == path) {
            info!("desktop: eject {}", v.name);
            crate::mounts::eject(v.uri.clone(), v.name.clone());
        }
    }

    /// The selected items' paths (the item a menu was opened on is among
    /// them), in the order they are on the desktop.
    fn desktop_selected_paths(&self) -> Vec<String> {
        self.desktop
            .items
            .iter()
            // Never a plugged-in volume: it is not a file to cut or send.
            .filter(|it| it.kind != Kind::Volume)
            .filter(|it| self.desktop.selected.contains(&it.path))
            .map(|it| it.path.clone())
            .collect()
    }

    /// Turn the menu to its "Move to" page: the sticks at once, the paired
    /// devices when the service has answered (asked off the loop).
    fn desktop_show_targets(&mut self) {
        self.desktop.targets = crate::desktop_send::sticks();
        // Everything mounted on the desktop is a place too — a phone above
        // all (Max, 2026-10-08: "my Pixel should appear there too, as any
        // Android, or mounted device"): it is not in /proc/mounts, the
        // volume service shows it through a folder of its own.
        for v in &self.desktop.volumes {
            let dest = if v.phone {
                crate::desktop_send::phone_dest(&v.path)
            } else {
                v.path.clone()
            };
            let listed = self.desktop.targets.iter().any(|t| {
                matches!(&t.place, crate::desktop_send::Place::Stick(p) if *p == v.path || *p == dest)
            });
            if !listed {
                self.desktop.targets.push(crate::desktop_send::Target {
                    name: v.name.clone(),
                    place: crate::desktop_send::Place::Stick(dest),
                    far: false,
                });
            }
        }
        self.desktop.looking = true;
        self.desktop_fill_targets();
        let (tx, rx) = calloop::channel::channel::<Vec<crate::desktop_send::Target>>();
        std::thread::spawn(move || {
            let _ = tx.send(crate::desktop_send::devices());
        });
        let waiting = self.loop_handle.insert_source(rx, |event, _, app: &mut App| {
            if let calloop::channel::Event::Msg(devices) = event {
                app.desktop.looking = false;
                // The carried things stay with the sticks; computers last.
                let (far, near): (Vec<_>, Vec<_>) = devices.into_iter().partition(|t| t.far);
                app.desktop.targets.retain(|t| matches!(t.place, crate::desktop_send::Place::Stick(_)));
                app.desktop.targets.extend(near);
                app.desktop.targets.extend(far);
                // Only if that page is still the one up.
                if app.desktop.menu.as_ref().is_some_and(|m| m.targets) {
                    app.desktop_fill_targets();
                }
            }
        });
        if waiting.is_err() {
            self.desktop.looking = false;
            self.desktop_fill_targets();
        }
    }

    /// Write the "Move to" page from what is known now.
    fn desktop_fill_targets(&mut self) {
        let h = self.desktop_size.1 as f32;
        let names = |far: bool| -> Vec<String> {
            self.desktop
                .targets
                .iter()
                .filter(|t| t.far == far)
                .map(|t| t.name.clone())
                .collect()
        };
        let (near, far) = (names(false), names(true));
        let looking = self.desktop.looking;
        let at = self.desktop.ptr;
        if let Some(menu) = self.desktop.menu.as_mut() {
            menu.show_targets(&near, &far, looking, h);
            menu.hover = at.and_then(|p| menu.hit(p));
        }
        self.request_desktop_draw();
    }

    /// Put the clipboard's files on the desktop, around `at`: copied, or
    /// moved if they were cut (once — a cut is spent by its paste).
    fn desktop_paste(&mut self, at: (f32, f32)) {
        let Some((paths, cut)) = self.clipboard_files() else {
            return;
        };
        let dir = desktop_dir();
        self.desktop_off_loop(
            move || crate::desktop_send::put(&paths, &dir, cut),
            move |app, (brought, failed)| {
                info!(
                    "desktop: pasted {} ({}; {failed} failed)",
                    brought.len(),
                    if cut { "moved" } else { "copied" }
                );
                app.desktop_place_brought(&brought, Some(at));
                if cut {
                    // The same files, where they are now, as a plain copy: a
                    // second paste somewhere else must not look for what
                    // has moved.
                    let now: Vec<String> =
                        brought.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                    app.serve_files(&now, false);
                }
                app.reload_desktop();
            },
        );
    }

    /// A row of the menu was chosen.
    fn desktop_menu_act(&mut self, action: Action, item: Option<usize>, at: (f32, f32)) {
        match action {
            Action::Open => {
                // The whole selection opens (the clicked item is in it).
                let mut all: Vec<usize> = self
                    .desktop
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, it)| self.desktop.selected.contains(&it.path))
                    .map(|(i, _)| i)
                    .collect();
                if all.is_empty() {
                    all.extend(item);
                }
                for i in all {
                    self.desktop_activate(i);
                }
            }
            Action::OpenTerminal => {
                // The dock's right-click idiom on files: a terminal in the
                // folder, or the file's folder.
                let Some(it) = item.and_then(|i| self.desktop.items.get(i)) else {
                    return;
                };
                let path = Path::new(&it.path);
                let dir = if matches!(it.kind, Kind::Folder | Kind::Volume) {
                    path.to_path_buf()
                } else {
                    path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.to_path_buf())
                };
                let dir = dir.to_string_lossy().into_owned();
                let exec = format!(
                    "cd {} && exec {}",
                    launch::shell_quote(&dir),
                    self.config.launch.terminal
                );
                info!("desktop: terminal at {dir}");
                if let Err(e) = launch::launch(&exec, false, &self.config.launch.terminal) {
                    error!("desktop: terminal launch failed: {e:#}");
                }
            }
            Action::Rename => {
                if let Some(i) = item {
                    self.desktop_begin_rename(i);
                }
            }
            Action::Properties => {
                if let Some(i) = item {
                    self.desktop_open_props(i, at, None);
                }
            }
            // (Turning the page is the release handler's: the menu stays up.)
            Action::MoveTo | Action::Back => {}
            Action::Cut | Action::Copy => {
                let paths = self.desktop_selected_paths();
                let cut = action == Action::Cut;
                info!("desktop: {} {}", if cut { "cut" } else { "copied" }, paths.join(", "));
                self.serve_files(&paths, cut);
            }
            Action::Paste => self.desktop_paste(at),
            Action::Mirror => {
                let Some(path) = item.and_then(|i| self.desktop.items.get(i)).map(|it| it.path.clone()) else {
                    return;
                };
                let Some(v) = self.desktop.volumes.iter().find(|v| v.path.as_os_str() == path.as_str()) else {
                    return;
                };
                let (name, serial) = (v.name.clone(), phone_serial(&v.uri));
                self.desktop.selected.clear();
                info!("desktop: mirroring {name} (serial {serial:?})");
                // Off the loop: it runs for as long as the window is open.
                std::thread::spawn(move || {
                    if let Some(said) = mirror(&name, serial.as_deref()) {
                        crate::desktop_send_notify(&said);
                    }
                });
            }
            Action::Camera => {
                let Some(path) = item.and_then(|i| self.desktop.items.get(i)).map(|it| it.path.clone()) else {
                    return;
                };
                let Some(v) = self.desktop.volumes.iter().find(|v| v.path.as_os_str() == path.as_str()) else {
                    return;
                };
                let (name, serial) = (v.name.clone(), phone_serial(&v.uri));
                self.desktop.selected.clear();
                // On already: this is the way to end it.
                // (Taken off the list first: its thread reads that as "by hand".)
                let running = self.desktop.cameras.lock().ok().and_then(|mut c| c.remove(&path));
                if let Some(pid) = running {
                    info!("desktop: {name} stops being the camera");
                    // SAFETY: a signal to a child of ours, by its pid.
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGTERM);
                    }
                    return;
                }
                info!("desktop: {name} as the camera (serial {serial:?})");
                let cameras = self.desktop.cameras.clone();
                std::thread::spawn(move || {
                    if let Some(said) = camera(&name, serial.as_deref(), &path, &cameras) {
                        crate::desktop_send_notify(&said);
                    }
                });
            }
            Action::Eject => {
                if let Some(path) = item.and_then(|i| self.desktop.items.get(i)).map(|it| it.path.clone()) {
                    self.desktop.selected.clear();
                    self.desktop_eject(&path);
                }
            }
            Action::SendTo(n) => {
                let Some(target) = self.desktop.targets.get(n).cloned() else {
                    return;
                };
                let paths: Vec<PathBuf> =
                    self.desktop_selected_paths().into_iter().map(PathBuf::from).collect();
                // (Only volumes were selected: nothing of the desktop's to send.)
                if paths.is_empty() {
                    return;
                }
                self.desktop.selected.clear();
                info!("desktop: {} file(s) → {}", paths.len(), target.name);
                // Off the loop: a copy to a stick takes as long as it takes.
                let (tx, rx) = calloop::channel::channel::<String>();
                std::thread::spawn(move || {
                    let _ = tx.send(crate::desktop_send::send(&paths, &target));
                });
                let waiting = self.loop_handle.insert_source(rx, |event, _, app: &mut App| {
                    if let calloop::channel::Event::Msg(said) = event {
                        info!("desktop: {said}");
                        crate::desktop_send_notify(&said);
                        app.reload_desktop();
                    }
                });
                if waiting.is_err() {
                    warn!("desktop: cannot wait for the files to be sent");
                }
            }
            Action::MoveToHome => {
                // The selection leaves the desktop for the home folder (the
                // clicked item is in it); a name already there gets its
                // number. The folder watch takes the icons off.
                let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
                let paths: Vec<String> = self.desktop.selected.drain().collect();
                // (A plugged-in volume in the selection stays where it is.)
                let paths: Vec<String> =
                    paths.into_iter().filter(|p| !self.desktop_is_volume(p)).collect();
                self.desktop_off_loop(
                    move || {
                        let mut moved = Vec::new();
                        for path in paths {
                            let src = Path::new(&path);
                            let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
                                continue;
                            };
                            let dest = unique_dest(&home, name);
                            // A move: across disks the copy must be whole
                            // before the original goes (`put` sees to it).
                            let (brought, _) =
                                crate::desktop_send::put(&[src.to_path_buf()], &home, true);
                            if brought.is_empty() {
                                warn!("desktop: cannot move {} home", src.display());
                            } else {
                                info!("desktop: {} → {}", src.display(), dest.display());
                                moved.push(path);
                            }
                        }
                        moved
                    },
                    |app, moved: Vec<String>| {
                        for path in moved {
                            app.desktop.remembered.remove(&path);
                        }
                        app.save_desktop_positions();
                        app.reload_desktop();
                    },
                );
            }
            Action::MoveToBin => {
                // The selection goes (the clicked item is in it).
                let paths: Vec<String> = self.desktop.selected.drain().collect();
                for path in paths {
                    // A plugged-in volume dropped on the bin's row is let
                    // go, never thrown away with all that is on it.
                    if self.desktop_is_volume(&path) {
                        self.desktop_eject(&path);
                        continue;
                    }
                    if let Some(it) = self.desktop.items.iter().find(|it| it.path == path) {
                        info!("desktop: {} → Recycle Bin", it.name);
                    }
                    self.desktop.remembered.remove(&path);
                    self.trash_file(&path); // the folder watch takes it off the desktop
                }
            }
            Action::NewFolder => self.desktop_new_folder(at),
            Action::AllowRun => {
                let Some(it) = item.and_then(|i| self.desktop.items.get(i)) else {
                    return;
                };
                use std::os::unix::fs::PermissionsExt;
                let path = Path::new(&it.path);
                let allowed = std::fs::metadata(path).and_then(|m| {
                    let mut perms = m.permissions();
                    perms.set_mode(perms.mode() | 0o100);
                    std::fs::set_permissions(path, perms)
                });
                match allowed {
                    Ok(()) => info!("desktop: {} may run from now on", it.name),
                    Err(e) => warn!("desktop: cannot let {} run: {e}", it.path),
                }
                self.reload_desktop();
            }
            Action::CleanUp => {
                self.desktop.remembered.clear();
                self.save_desktop_positions();
                info!("desktop: cleaned up");
                self.relayout_desktop();
            }
        }
    }

    /// Turn the Properties box back into the menu it grew out of: the panel
    /// shrinks to the menu's shape, the pointer at `at`.
    fn desktop_props_back(&mut self, at: (f32, f32)) {
        let Some(props) = self.desktop.props.take() else {
            return;
        };
        if let Some(mut menu) = props.back {
            menu.grow_from = Some(props.rect);
            menu.t = 0.0;
            menu.pressed = None;
            menu.hover = menu.hit(at);
            self.desktop.menu = Some(menu);
        }
        self.request_desktop_draw();
    }

    /// Open the Properties box for item `i` by `at`. A folder's total size
    /// is walked off the loop and filled in when it comes back.
    pub(crate) fn desktop_open_props(&mut self, i: usize, at: (f32, f32), menu: Option<Menu>) {
        let Some(item) = self.desktop.items.get(i) else {
            return;
        };
        let (w, h) = self.desktop_size;
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        let props = Props::open(item, at, menu, w as f32, h as f32, &home);
        if item.kind == Kind::Folder {
            let path = item.path.clone();
            let (tx, rx) = calloop::channel::channel::<(String, u64, u64)>();
            std::thread::spawn(move || {
                let (bytes, files) = crate::desktop_props::folder_size(Path::new(&path));
                let _ = tx.send((path, bytes, files));
            });
            let _ = self.loop_handle.insert_source(rx, |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg((path, bytes, files)) = event {
                    if let Some(p) = app.desktop.props.as_mut().filter(|p| p.path == path) {
                        p.set_size(bytes, files);
                        app.request_desktop_draw();
                    }
                }
            });
        }
        info!("desktop: properties of {}", props.title);
        self.desktop.props = Some(props);
        self.request_desktop_draw();
    }

    /// A new, empty folder in the cell nearest `at`, named `untitled folder`
    /// (then `untitled folder 2`…), its name opened for typing.
    fn desktop_new_folder(&mut self, at: (f32, f32)) {
        // Its name is typed at once: with the icons put away the field
        // would hold the keyboard unseen. They come back for it.
        if self.desktop.hidden {
            self.desktop_toggle_hidden();
        }
        let dir = desktop_dir();
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
            error!("desktop: cannot make {}: {e}", path.display());
            return;
        }
        info!("desktop: new folder {}", path.display());
        let key = path.to_string_lossy().into_owned();
        if let Some(slot) = self.desktop.grid.nearest_free(at, &self.desktop.taken(None)) {
            self.desktop.remembered.insert(key.clone(), slot);
            self.save_desktop_positions();
        }
        self.reload_desktop();
        if let Some(i) = self.desktop.items.iter().position(|it| it.path == key) {
            self.desktop_begin_rename(i);
        }
    }

    /// Open item `i`'s name for typing: the whole name selected, and the
    /// keyboard ours — exclusively, for a layer on the Bottom layer is
    /// given it on hover with that and never with `OnDemand` (no click
    /// follows the menu's). It goes back the moment the name is settled.
    fn desktop_begin_rename(&mut self, i: usize) {
        let Some(it) = self.desktop.items.get(i) else {
            return;
        };
        let Some(layer) = self.desktop_layer.as_ref() else {
            return;
        };
        // A launcher is renamed by its file, not its `Name=`: what is typed
        // is the file's new name.
        let file_name = Path::new(&it.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| it.name.clone());
        info!("desktop: renaming {file_name}");
        self.desktop.selected.clear();
        self.desktop.selected.insert(it.path.clone());
        self.desktop.rename = Some(Rename {
            item: i,
            text: file_name,
            all: true,
        });
        crate::surface::set_interactive(layer, true);
        let _ = self.conn.flush();
        self.cancel_keyboard_handback(crate::KbSurface::Desktop);
        self.request_desktop_draw();
    }

    /// The name is settled: kept (`commit`, if it changed and is not taken)
    /// or left as it was. The keyboard goes back to the window it came from.
    pub(crate) fn desktop_end_rename(&mut self, commit: bool) {
        let Some(rename) = self.desktop.rename.take() else {
            return;
        };
        // The keyboard goes back to the window it came from — unless the
        // desktop holds it anyway (a click on it): then it stays for the
        // shortcuts.
        if self.desktop.keys {
            // Kept, but no longer exclusively: an exclusive layer has the
            // POINTER pinned to it too, and no window could be clicked.
            if let Some(layer) = self.desktop_layer.as_ref() {
                crate::surface::set_on_demand(layer);
            }
            let _ = self.conn.flush();
        } else if self.desktop_layer.is_some() {
            // Armed before the release: the compositor's `leave` completes it.
            self.begin_keyboard_handback(crate::KbSurface::Desktop, None);
            if let Some(layer) = self.desktop_layer.as_ref() {
                crate::surface::set_interactive(layer, false);
            }
            let _ = self.conn.flush();
        }
        let renamed = commit && self.desktop_apply_rename(rename.item, rename.text.trim());
        if renamed {
            self.reload_desktop();
        } else {
            self.request_desktop_draw();
        }
    }

    /// `fs::rename` item `i` to `name` in its folder, carrying its cell and
    /// its selection to the new path. False when nothing was done.
    fn desktop_apply_rename(&mut self, i: usize, name: &str) -> bool {
        let Some(it) = self.desktop.items.get(i) else {
            return false;
        };
        let old = PathBuf::from(&it.path);
        let Some(dir) = old.parent() else {
            return false;
        };
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            warn!("desktop: {name:?} is not a name");
            return false;
        }
        let new = dir.join(name);
        if new == old {
            return false;
        }
        if new.exists() {
            warn!("desktop: {} exists; {} keeps its name", new.display(), old.display());
            return false;
        }
        if let Err(e) = std::fs::rename(&old, &new) {
            error!("desktop: cannot rename {} → {}: {e}", old.display(), new.display());
            return false;
        }
        info!("desktop: {} → {}", old.display(), new.display());
        let (old_key, new_key) = (it.path.clone(), new.to_string_lossy().into_owned());
        if let Some(slot) = self.desktop.remembered.remove(&old_key) {
            self.desktop.remembered.insert(new_key.clone(), slot);
            self.save_desktop_positions();
        }
        if self.desktop.selected.remove(&old_key) {
            self.desktop.selected.insert(new_key);
        }
        true
    }

    /// A key while a name is being typed: text goes in (the first key
    /// replaces the selected name), Backspace takes out, Enter keeps,
    /// Escape leaves the old name.
    pub(crate) fn desktop_key(&mut self, keysym: Keysym, utf8: Option<&str>) {
        let Some(rename) = self.desktop.rename.as_mut() else {
            return;
        };
        match keysym {
            Keysym::Return | Keysym::KP_Enter => self.desktop_end_rename(true),
            Keysym::Escape => self.desktop_end_rename(false),
            Keysym::BackSpace => {
                if rename.all {
                    rename.text.clear();
                    rename.all = false;
                } else {
                    rename.text.pop();
                }
                self.request_desktop_draw();
            }
            _ => {
                let Some(s) = utf8.filter(|s| !s.is_empty() && !s.chars().any(char::is_control)) else {
                    return;
                };
                if self.modifiers.ctrl {
                    return;
                }
                if rename.all {
                    rename.text.clear();
                    rename.all = false;
                }
                rename.text.push_str(s);
                self.request_desktop_draw();
            }
        }
    }

    /// A click on the desktop gives it the keyboard, so the shortcuts a
    /// file manager has work on what is selected (Max, 2026-10-09: Ctrl+X/C/V
    /// — and Delete, Enter, F2, the arrows, Escape, Ctrl+A). It stays until
    /// a window is clicked, as on any desktop.
    ///
    /// How, on Hyprland: a layer that asks for the keyboard "on demand" is
    /// given it whenever the pointer moves over it — so the desktop asks
    /// for nothing at rest, or crossing wallpaper would take the typing
    /// from a window. At a click it grabs the keyboard EXCLUSIVELY (that is
    /// immediate), and the moment it has it (`desktop_keys_arrived`) drops
    /// to on-demand: an exclusive layer has the pointer pinned to it too
    /// (every click, anywhere, goes to it — the first cut of this trapped
    /// the whole workspace). On-demand it keeps the keyboard until a click
    /// gives it to a window (Golem focuses by click); the `leave` that
    /// brings puts the desktop back to asking for nothing.
    pub(crate) fn desktop_take_keys(&mut self) {
        if self.desktop.keys {
            return;
        }
        let Some(layer) = self.desktop_layer.as_ref() else {
            return;
        };
        crate::surface::set_interactive(layer, true);
        let _ = self.conn.flush();
        self.cancel_keyboard_handback(crate::KbSurface::Desktop);
        self.desktop.keys = true;
        debug!("desktop: has the keyboard");
    }

    /// Give the keyboard back to the window it came from. A name being
    /// typed keeps it until it is settled.
    pub(crate) fn desktop_drop_keys(&mut self) {
        if !self.desktop.keys || self.desktop.rename.is_some() {
            return;
        }
        self.desktop.keys = false;
        debug!("desktop: gives the keyboard back");
        self.begin_keyboard_handback(crate::KbSurface::Desktop, None);
        if let Some(layer) = self.desktop_layer.as_ref() {
            crate::surface::set_interactive(layer, false);
        }
        let _ = self.conn.flush();
    }

    /// The keyboard has arrived on the desktop (the compositor's `enter`):
    /// the exclusive grab that fetched it is let go at once, and the
    /// keyboard kept "on demand" — it stays until a window is clicked,
    /// while the pointer is free to go and click one.
    pub(crate) fn desktop_keys_arrived(&mut self) {
        if !self.desktop.keys || self.desktop.rename.is_some() {
            return;
        }
        if let Some(layer) = self.desktop_layer.as_ref() {
            crate::surface::set_on_demand(layer);
        }
        let _ = self.conn.flush();
    }

    /// A key while the desktop holds the keyboard: a file manager's
    /// shortcuts, on what is selected.
    pub(crate) fn desktop_shortcut(&mut self, keysym: Keysym) {
        let at = self.desktop.ptr.unwrap_or((self.desktop.grid.x0, MARGIN));
        // A menu or a box up: Escape puts it away; nothing else is for it.
        if self.desktop.menu.is_some() || self.desktop.props.is_some() {
            if keysym == Keysym::Escape {
                self.desktop.menu = None;
                self.desktop.props = None;
                self.request_desktop_draw();
            }
            return;
        }
        if self.desktop.hidden {
            return;
        }
        let ctrl = self.modifiers.ctrl;
        match keysym {
            Keysym::c | Keysym::C if ctrl => self.desktop_menu_act(Action::Copy, None, at),
            Keysym::x | Keysym::X if ctrl => self.desktop_menu_act(Action::Cut, None, at),
            Keysym::v | Keysym::V if ctrl => self.desktop_paste(at),
            Keysym::a | Keysym::A if ctrl => {
                self.desktop.selected = self
                    .desktop
                    .items
                    .iter()
                    .zip(&self.desktop.slots)
                    .filter(|(_, slot)| slot.is_some())
                    .map(|(it, _)| it.path.clone())
                    .collect();
                self.request_desktop_draw();
            }
            Keysym::Delete | Keysym::KP_Delete if !self.desktop.selected.is_empty() => {
                self.desktop_menu_act(Action::MoveToBin, None, at);
                self.request_desktop_draw();
            }
            Keysym::Return | Keysym::KP_Enter if !self.desktop.selected.is_empty() => {
                self.desktop_menu_act(Action::Open, None, at);
            }
            Keysym::F2 => {
                // One thing selected, and not a plugged-in volume.
                let mut picked = self
                    .desktop
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, it)| self.desktop.selected.contains(&it.path));
                if let (Some((i, it)), None) = (picked.next(), picked.next()) {
                    if it.kind != Kind::Volume {
                        self.desktop_begin_rename(i);
                    }
                }
            }
            Keysym::Escape => {
                if self.desktop.selected.is_empty() {
                    self.desktop_drop_keys();
                } else {
                    self.desktop.selected.clear();
                    self.request_desktop_draw();
                }
            }
            Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down => {
                let step = match keysym {
                    Keysym::Left => (-1, 0),
                    Keysym::Right => (1, 0),
                    Keysym::Up => (0, -1),
                    _ => (0, 1),
                };
                let from = self
                    .desktop
                    .items
                    .iter()
                    .zip(&self.desktop.slots)
                    .find(|(it, _)| self.desktop.selected.contains(&it.path))
                    .and_then(|(_, slot)| *slot);
                if let Some(i) = neighbour(&self.desktop.slots, from, step) {
                    self.desktop.selected.clear();
                    self.desktop.selected.insert(self.desktop.items[i].path.clone());
                    self.request_desktop_draw();
                }
            }
            _ => {}
        }
    }

    /// A window was clicked (the keyboard went to another one): a click
    /// anywhere else lets go of the desktop's selection, and a window is
    /// anywhere else (Max, 2026-10-08). Not while our own things hold the
    /// moment: an icon in hand on its way to that window, a name being
    /// typed, a menu up.
    pub(crate) fn desktop_focus_moved(&mut self) {
        if self.desktop.selected.is_empty()
            || self.desktop.drag.is_some()
            || self.desktop.rename.is_some()
            || self.desktop.menu.is_some()
            || self.desktop.props.is_some()
        {
            return;
        }
        self.desktop.selected.clear();
        self.request_desktop_draw();
    }

    /// Put the icons away, or bring them back: they fade over a few
    /// frames. Whatever was selected or being banded is let go of.
    pub(crate) fn desktop_toggle_hidden(&mut self) {
        self.desktop.hidden = !self.desktop.hidden;
        self.desktop.selected.clear();
        self.desktop.band = None;
        self.desktop.solo.clear();
        self.desktop.solo_on = false;
        self.desktop.solo_t = 0.0;
        self.settings.desktop_hidden = self.desktop.hidden;
        self.settings.save();
        info!(
            "desktop: icons {}",
            if self.desktop.hidden { "put away" } else { "back" }
        );
        self.request_desktop_draw();
    }

    /// Draw now, or once the frame in flight has been shown. Every change
    /// of state comes through here, so the pointer's shape is settled here
    /// too (see `desktop_cursor`).
    fn request_desktop_draw(&mut self) {
        self.desktop_cursor();
        // (The menus' own surface follows every change too.)
        self.request_desktop_top_draw();
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
        // The icons fade in or out toward where `hidden` says, dt-based like
        // every ease in the daemon; while they move, the frame callback
        // brings the next frame.
        let now = std::time::Instant::now();
        let dt = self
            .desktop_last_frame
            .map(|l| now.duration_since(l).as_secs_f32().min(0.1))
            .unwrap_or(0.0);
        let target = if self.desktop.hidden { 0.0 } else { 1.0 };
        let (shown, mut moving) =
            crate::animation::ease_toward(self.desktop.shown, target, dt, 14.0, 0.004);
        self.desktop.shown = shown;
        // Devices shown alone fade the same way, in and out.
        let solo_target = if self.desktop.hidden && self.desktop.solo_on { 1.0 } else { 0.0 };
        let (solo_t, solo_moving) =
            crate::animation::ease_toward(self.desktop.solo_t, solo_target, dt, 14.0, 0.004);
        self.desktop.solo_t = solo_t;
        moving |= solo_moving;
        if !self.desktop.solo_on && solo_t <= 0.0 {
            self.desktop.solo.clear();
        }
        // The menus live on their own surface above the windows
        // (`desktop_top.rs`); only without it are they eased and drawn here.
        let on_top = self.desktop_top_ready();
        if !on_top {
            moving |= self.desktop_ease_panels(dt);
        }
        self.desktop_last_frame = moving.then_some(now);
        let layer_of: Vec<Option<u32>> = (0..self.desktop.items.len())
            .map(|i| self.desktop.layer_of(i))
            .collect();
        let has_icon: Vec<bool> = layer_of.iter().map(Option::is_some).collect();
        let layers: Vec<u32> = layer_of.iter().map(|l| l.unwrap_or(0)).collect();
        let icon_scale = self.icon_scale();
        let in_hand: Vec<usize> = self
            .desktop
            .drag
            .as_ref()
            .map(|d| d.items.clone())
            .unwrap_or_default();
        let carried = self
            .desktop
            .drag
            .as_ref()
            .zip(self.desktop.dnd.filter(|d| !d.on_dock))
            .map(|(d, at)| (at.pos.0 - d.grip.0, at.pos.1 - d.grip.1));
        let selected: Vec<bool> = self
            .desktop
            .items
            .iter()
            .map(|it| self.desktop.selected.contains(&it.path))
            .collect();
        let band = self.desktop.band.map(|(a, b)| band_rect(a, b));
        // ONE group fades at a time, by the surface's own opacity: all the
        // icons (in or out), or — once they are away — the devices shown
        // alone. A menu or a box up over put-away icons must be seen whole.
        let lit = !on_top && (self.desktop.menu.is_some() || self.desktop.props.is_some());
        let all_group = !self.desktop.hidden || shown > 0.004;
        let only: Option<HashSet<String>> = (!all_group).then(|| self.desktop.solo.clone());
        let group_alpha = if all_group { shown } else { solo_t };
        let panel_paint = |rect: Option<Rect>| rect.filter(|_| !on_top).map(|r| self.desktop_panel_paint(r));
        let menu_paint = panel_paint(self.desktop.menu.as_ref().map(|m| m.rect));
        let props_paint = panel_paint(self.desktop.props.as_ref().map(|p| p.rect));
        let Some(renderer) = self.desktop_renderer.as_mut() else {
            return;
        };
        let rename_view = self
            .desktop
            .rename
            .as_ref()
            .map(|r| (r.item, r.text.as_str(), r.all, renderer.measure_text(&r.text, LABEL_FONT_PX, None)));
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
        let mut scene = scene(
            &self.desktop.items,
            &self.desktop.slots,
            &self.desktop.grid,
            &names,
            &has_icon,
            icon_scale,
            Live {
                in_hand: &in_hand,
                layers: &layers,
                carried,
                selected: &selected,
                band,
                only: only.as_ref(),
                menu: self.desktop.menu.as_ref().zip(menu_paint),
                props: self.desktop.props.as_ref().zip(props_paint),
                rename: rename_view,
            },
        );
        scene.alpha = if lit { 1.0 } else { group_alpha };
        let (layer, qh, pending) = (
            self.desktop_layer.as_ref(),
            &self.qh,
            &mut self.desktop_frame_pending,
        );
        // No thumbnail base: an item's layer is its index, and a thumbnail
        // simply replaces the carrier in it, so none is exempt from the
        // squircle. (Golem's theme has it off.)
        let mut presented = false;
        match renderer.render(
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
            Ok(crate::renderer::Frame::Presented) => presented = true,
            Ok(crate::renderer::Frame::Unchanged) => {}
            Err(e) => warn!("desktop render failed: {e:#}"),
        }
        if moving {
            // The ease's next frame: on the frame callback when this one
            // was presented — or, when it changed nothing (the first frame
            // of a fade has dt = 0, so nothing moves and nothing is
            // committed, and no callback ever comes), on a timer. Without
            // the timer the fade stalled at its first step until something
            // else redrew the desktop (a rubber band did; a click did not —
            // Max, 2026-10-07: "plain click won't do it").
            self.desktop_dirty = true;
            if !presented && !self.desktop_frame_pending {
                self.schedule_desktop_tick();
            }
        }
    }

    /// Draw the desktop again shortly, for an easing frame that presented
    /// nothing (and so gets no frame callback).
    fn schedule_desktop_tick(&mut self) {
        if self.desktop_tick_timer {
            return;
        }
        let timer = calloop::timer::Timer::from_duration(std::time::Duration::from_millis(8));
        let armed = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.desktop_tick_timer = false;
                app.draw_desktop();
                calloop::timer::TimeoutAction::Drop
            })
            .is_ok();
        if armed {
            self.desktop_tick_timer = true;
        }
    }

    /// Route a pointer event on the desktop surface: a left press that
    /// comes straight back up opens the item under it; one that travels
    /// takes the icon along and drops it where it is let go.
    pub(crate) fn desktop_pointer(&mut self, event: wl_pointer::Event) {
        // A press anywhere settles a name being typed, keeping what was
        // typed (the mockup's rule) — else the field stayed up, holding
        // the keyboard, under whatever the click went on to do.
        if self.desktop.rename.is_some()
            && matches!(
                event,
                wl_pointer::Event::Button {
                    state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                    ..
                }
            )
        {
            self.desktop_end_rename(true);
        }
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
                if self.desktop.press.is_some() || self.desktop.band.is_some() {
                    debug!(
                        "desktop: pointer left mid-press (press {:?}, band {})",
                        self.desktop.press.map(|p| p.item),
                        self.desktop.band.is_some()
                    );
                }
                self.pointer_surface = crate::options::PointerSurface::Dock;
                self.desktop.press = None;
                self.desktop.ptr = None;
                // A menu drawn ON the desktop would hang under whatever took
                // the pointer, and goes with it. On its own surface it takes
                // the pointer itself (this very leave), and stays.
                let gone = self.desktop.band.take().is_some()
                    | (!self.desktop_top_ready()
                        && (self.desktop.menu.take().is_some() | self.desktop.props.take().is_some()));
                if gone {
                    self.request_desktop_draw();
                }
            }
            // The right button, let go: the menu for what is under it.
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(wl_pointer::ButtonState::Released),
                ..
            } if button == crate::BTN_RIGHT => {
                // With a menu up the pointer is on the menus' surface, which
                // covers the windows too: a right-click there is not on the
                // desktop — like any click off the menu, it only closes it.
                if self.pointer_surface == crate::options::PointerSurface::DesktopTop {
                    if self.desktop.menu.take().is_some() | self.desktop.props.take().is_some() {
                        self.desktop.selected.clear();
                        self.request_desktop_draw();
                    }
                } else if let Some(at) = self.desktop.ptr {
                    self.desktop_open_menu(at);
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
                        debug!("desktop: press with no pointer position (no enter yet); ignored");
                        return;
                    };
                    // A click on the desktop gives it the keyboard (not one
                    // on the menus' surface, which lies over the windows too).
                    if self.pointer_surface == crate::options::PointerSurface::Desktop {
                        self.desktop_take_keys();
                    }
                    debug!(
                        "desktop: press at ({:.0},{:.0}) on {} (menu {}, hidden {})",
                        at.0,
                        at.1,
                        self.desktop.hit(at).map_or("wallpaper".to_owned(), |i| self.desktop.items[i].name.clone()),
                        self.desktop.menu.is_some(),
                        self.desktop.hidden
                    );
                    // The Properties box: a press on Back turns it back into
                    // the menu it grew out of; one anywhere else puts it
                    // away, and is nothing more than that.
                    if let Some(back) = self.desktop.props.as_ref().map(|p| p.back_rect()) {
                        if back.is_some_and(|r| r.contains(at)) {
                            self.desktop_props_back(at);
                        } else {
                            self.desktop.props = None;
                            self.desktop.selected.clear();
                            self.request_desktop_draw();
                        }
                        return;
                    }
                    // While the menu is up, the left button is its: a press
                    // on a row arms it, one anywhere else just closes it
                    // (and is not a click on the desktop).
                    if let Some(menu) = self.desktop.menu.as_mut() {
                        match menu.hit(at) {
                            Some(row) => menu.pressed = Some(row),
                            None => {
                                // The menu goes, and what it was for is
                                // let go of with it (Max, 2026-10-08).
                                self.desktop.menu = None;
                                self.desktop.selected.clear();
                                self.request_desktop_draw();
                            }
                        }
                        return;
                    }
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
                    // A row pressed and released: its action, and the menu
                    // goes.
                    if let Some(menu) = self.desktop.menu.as_mut() {
                        let pressed = menu.pressed.take();
                        let under = self.desktop.ptr.and_then(|p| menu.hit(p));
                        if let Some(row) = pressed.filter(|r| Some(*r) == under) {
                            if let Some(action) = menu.action(row) {
                                // "Move to" and "Back" turn the page: the
                                // menu stays up.
                                match action {
                                    Action::MoveTo => {
                                        self.desktop_show_targets();
                                        return;
                                    }
                                    Action::Back => {
                                        let h = self.desktop_size.1 as f32;
                                        let at = self.desktop.ptr;
                                        menu.show_main(h);
                                        menu.hover = at.and_then(|p| menu.hit(p));
                                        self.request_desktop_draw();
                                        return;
                                    }
                                    _ => {}
                                }
                                let (item, at) = (menu.item, menu.at);
                                self.desktop.menu = None;
                                self.desktop_menu_act(action, item, at);
                                self.request_desktop_draw();
                            }
                        }
                        return;
                    }
                    let press = self.desktop.press.take();
                    debug!(
                        "desktop: release at {:?}; press {:?}, band {}, drag {}",
                        self.desktop.ptr,
                        press.map(|p| (p.item, p.at)),
                        self.desktop.band.is_some(),
                        self.desktop.drag.is_some()
                    );
                    if self.desktop.band.take().is_some() {
                        // The band's selection stands; the band itself goes.
                        self.request_desktop_draw();
                    } else if let Some(press) = press {
                        if self.desktop.drag.is_some() {
                            return;
                        }
                        let under = self.desktop.ptr.and_then(|p| self.desktop.hit(p));
                        match press.item {
                            // A click on an item selects it, alone; a
                            // second click on it soon after opens it
                            // (Max, 2026-10-08: "one click select, double
                            // click opens").
                            Some(i) if under == Some(i) => {
                                let path = self.desktop.items[i].path.clone();
                                let now = std::time::Instant::now();
                                let double = self.desktop.last_click.take().is_some_and(|(p, t)| {
                                    p == path && now.duration_since(t) <= DOUBLE_CLICK
                                });
                                if double {
                                    self.desktop_activate(i);
                                } else {
                                    self.desktop.last_click = Some((path.clone(), now));
                                    self.desktop.selected.clear();
                                    self.desktop.selected.insert(path);
                                    self.request_desktop_draw();
                                }
                            }
                            // A click on bare wallpaper lets go of a
                            // selection first; only with nothing selected
                            // does it put the icons away, or bring them
                            // back (Max, 2026-10-07/08).
                            None if !self.desktop.selected.is_empty() => {
                                self.desktop.selected.clear();
                                self.request_desktop_draw();
                            }
                            // With a device shown alone, the click brings
                            // the others back to it; the next one puts
                            // them all away (Max, 2026-10-08). The others
                            // appear at once: the surface has ONE opacity,
                            // and fading them in would blink the device
                            // that is already there.
                            None if self.desktop.hidden && self.desktop.solo_on => {
                                // (Set BEFORE the toggle draws: one frame
                                // at the old zero opacity was the blink.)
                                self.desktop.shown = 1.0;
                                self.desktop_toggle_hidden();
                            }
                            None => self.desktop_toggle_hidden(),
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
        if let Some(menu) = self.desktop.menu.as_mut() {
            // The menu has the pointer: only its hover follows it. Coming
            // onto its last row, Properties, turns the menu into that box.
            let over = menu.hit((x, y));
            if over != menu.hover {
                menu.hover = over;
                let props = over.and_then(|r| menu.action(r)) == Some(Action::Properties);
                if let (true, Some(i)) = (props, menu.item) {
                    if let Some(menu) = self.desktop.menu.take() {
                        self.desktop_open_props(i, (x, y), Some(menu));
                    }
                } else if over.and_then(|r| menu.action(r)) == Some(Action::Back) {
                    // Coming onto Back turns the page back, as it does in
                    // the Properties box (Max, 2026-10-08).
                    let h = self.desktop_size.1 as f32;
                    menu.show_main(h);
                    menu.hover = menu.hit((x, y));
                }
                self.request_desktop_draw();
            }
            self.desktop_cursor();
            return;
        }
        if let Some(props) = self.desktop.props.as_mut() {
            // The Properties box: only its Back row answers the pointer,
            // and coming onto it is enough — as coming onto Properties was
            // (Max, 2026-10-08: "make back work also on hover").
            if props.back_rect().is_some_and(|r| r.contains((x, y))) {
                self.desktop_props_back((x, y));
            }
            self.desktop_cursor();
            return;
        }
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
                        // No band over put-away icons: there is nothing to
                        // select, and the release will bring them back.
                        None if !self.desktop.hidden => {
                            debug!("desktop: band from ({:.0},{:.0})", px, py);
                            self.desktop.band = Some((press.at, (x, y)));
                            self.desktop.selected.clear();
                            self.request_desktop_draw();
                        }
                        None => {}
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
        // A plugged-in volume in hand is only ever COPIED from: an app that
        // took a move would empty the stick into itself.
        let volume = self.desktop.items[i].kind == Kind::Volume
            || self
                .desktop
                .items
                .iter()
                .any(|it| it.kind == Kind::Volume && self.desktop.selected.contains(&it.path));
        let source = manager.create_drag_and_drop_source(
            &self.qh,
            [URI_LIST, PLAIN_TEXT],
            if volume { DndAction::Copy } else { DndAction::Move | DndAction::Copy },
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
            grip,
            pos: self.desktop.ptr.unwrap_or(at),
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
        self.drag_image(chain, grip, icon_scale)
    }

    /// A drag image from ANY picture: `rgba` premultiplied, `w`×`h` buffer
    /// pixels (both whole multiples of `scale`), shown at 1/`scale` of
    /// that, gripped at `grip` (logical px from its top-left corner). The
    /// card's items travel as pictures of themselves (`card/dnd.rs`).
    pub(crate) fn drag_picture(
        &self,
        rgba: &[u8],
        w: usize,
        h: usize,
        scale: i32,
        grip: (f32, f32),
    ) -> Option<DragIcon> {
        let shm = self.shm.as_ref()?;
        let (stride, len) = (w * 4, w * h * 4);
        if w == 0 || h == 0 || rgba.len() < len {
            return None;
        }
        let mut pool = match RawPool::new(len, shm) {
            Ok(pool) => pool,
            Err(e) => {
                warn!("no shm pool for the drag image ({e})");
                return None;
            }
        };
        rgba_to_argb(&rgba[..len], &mut pool.mmap()[..len]);
        let buffer = pool.create_buffer(
            0,
            w as i32,
            h as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            (),
            &self.qh,
        );
        let surface = self.compositor.create_surface(&self.qh);
        surface.set_buffer_scale(scale.max(1));
        let (gx, gy) = (-grip.0.round() as i32, -grip.1.round() as i32);
        if surface.version() >= 5 {
            surface.attach(Some(&buffer), 0, 0);
            surface.offset(gx, gy);
        } else {
            surface.attach(Some(&buffer), gx, gy);
        }
        surface.damage_buffer(0, 0, w as i32, h as i32);
        Some(DragIcon {
            surface,
            buffer,
            _pool: pool,
        })
    }

    /// A drag image from a picture's pixels (`chain`: an `ICON_SIZE`² RGBA
    /// mip chain), gripped at `grip`. The card's pictures travel this way
    /// too (`card.rs`).
    pub(crate) fn drag_image(&self, chain: &[u8], grip: (f32, f32), icon_scale: f32) -> Option<DragIcon> {
        let shm = self.shm.as_ref()?;
        if chain.len() < ICON_PX * ICON_PX * 4 {
            return None;
        }
        // A buffer's size must be a whole multiple of its scale (the
        // protocol says so, though Hyprland lets it pass): the picture sits
        // in the middle of the next size up that is, on clear pixels.
        let scale = icon_buffer_scale(icon_scale);
        let side = ICON_PX.div_ceil(scale as usize) * scale as usize;
        let (stride, pad) = (side * 4, (side - ICON_PX) / 2);
        let len = stride * side;
        let mut pool = match RawPool::new(len, shm) {
            Ok(pool) => pool,
            Err(e) => {
                warn!("no shm pool for the drag image ({e})");
                return None;
            }
        };
        {
            let mem = &mut pool.mmap()[..len];
            mem.fill(0);
            for y in 0..ICON_PX {
                let from = y * ICON_PX * 4;
                let to = (y + pad) * stride + pad * 4;
                rgba_to_argb(&chain[from..from + ICON_PX * 4], &mut mem[to..to + ICON_PX * 4]);
            }
        }
        let buffer = pool.create_buffer(
            0,
            side as i32,
            side as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            (),
            &self.qh,
        );
        let surface = self.compositor.create_surface(&self.qh);
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
        surface.damage_buffer(0, 0, side as i32, side as i32);
        Some(DragIcon {
            surface,
            buffer,
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
        // The enter's position is NOT the pointer's: Hyprland sends the
        // surface's centre there (`updateDrag` in its DataDevice.cpp) and
        // the real one with the first motion. Our own drag starts where
        // the pointer was lifted (the group was drawn mid-screen for a
        // moment otherwise — Max, 2026-10-07: "a ghost on another place");
        // another app's is placed only by its motions and drop.
        let pos = match self.desktop.drag.as_ref() {
            Some(d) if !on_dock => d.pos,
            _ => (offer.x as f32, offer.y as f32),
        };
        self.desktop.dnd = Some(DndIn { pos, on_dock });
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
            } else if self.desktop.drag.is_some() {
                // Our own group travels with the pointer.
                self.request_desktop_draw();
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
                        // A volume dragged onto the bin is ejected (macOS's
                        // gesture) — NEVER trashed with what is on it.
                        if self.desktop.items[i].kind == Kind::Volume {
                            self.desktop.selected.remove(&path);
                            self.desktop_eject(&path);
                            continue;
                        }
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
        let dropped = offer.clone();
        let at = (offer.x as f32, offer.y as f32);
        // The drop is in hand; what hovers from here on is another drag's.
        self.desktop_dnd_offer = None;
        self.desktop.dnd = None;
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
            .insert_source(rx, move |event, _, app: &mut App| {
                if let calloop::channel::Event::Msg(text) = event {
                    // THIS drop's offer and place: by now another drag may
                    // be over us, and finishing its offer would be a
                    // protocol error.
                    app.desktop_dnd_received(&text, &dropped, at);
                }
            })
            .is_err()
        {
            warn!("desktop: cannot wait for the drop's files");
            offer.destroy();
            self.request_desktop_draw();
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
    pub(crate) fn desktop_dnd_received(&mut self, list: &str, offer: &DragOffer, at: (f32, f32)) {
        let paths = uri_list_paths(list);
        info!(
            "desktop: the drop's list is {} bytes → {} file path(s): {paths:?}",
            list.len(),
            paths.len()
        );
        // A move, unless the source only lets its files be copied (the
        // card's items stay on the card).
        let may_move = offer.source_actions.is_empty() || offer.source_actions.contains(DndAction::Move);
        let (dir, offer) = (desktop_dir(), offer.clone());
        self.desktop_off_loop(
            move || import(&paths, &dir, may_move),
            move |app, brought| {
                app.desktop_place_brought(&brought, Some(at));
                // Done, whatever came of the files: the other app must
                // always hear the end of its drag, or it stays mid-drag.
                offer.finish();
                app.reload_desktop();
            },
        );
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
    /// The pointer's shape on the desktop (Max, 2026-10-07): the plain
    /// arrow over wallpaper, files and launchers; the index over a FOLDER
    /// and over a menu row; a crosshair while a rubber band is drawn; the
    /// fist while an icon is in hand. Sent again after every change of
    /// state, not only on motion — a shape left over from the last state
    /// (the band's, the drag's) used to stay until the pointer moved.
    pub(crate) fn desktop_cursor(&mut self) {
        // Only while the pointer is on the desktop: Hyprland applies a shape
        // request from whichever client holds the pointer focus, and the
        // dock and the bar are this same client — a request sent from here
        // while the pointer is on them would redraw THEIR pointer.
        if !matches!(
            self.pointer_surface,
            crate::options::PointerSurface::Desktop | crate::options::PointerSurface::DesktopTop
        ) || self.desktop.ptr.is_none()
        {
            return;
        }
        let Some(device) = &self.cursor_device else {
            return;
        };
        // Stale after a while: something outside the protocol may have
        // repainted the pointer since (see `cursor_sent`).
        let now = std::time::Instant::now();
        if self
            .desktop
            .cursor_sent
            .is_some_and(|t| now.duration_since(t) > CURSOR_REFRESH)
        {
            self.cursor_now = None;
        }
        let over_row = self
            .desktop
            .menu
            .as_ref()
            .zip(self.desktop.ptr)
            .is_some_and(|(m, p)| m.hit(p).is_some());
        let over_folder = self
            .desktop
            .ptr
            .and_then(|p| self.desktop.hit(p))
            .and_then(|i| self.desktop.items.get(i))
            .is_some_and(|it| matches!(it.kind, Kind::Folder | Kind::Volume));
        let shape = if self.desktop.drag.is_some() {
            Shape::Grabbing
        } else if self.desktop.band.is_some() {
            Shape::Crosshair
        } else if over_row
            || (self.desktop.menu.is_none() && self.desktop.props.is_none() && over_folder)
        {
            Shape::Pointer
        } else {
            Shape::Default
        };
        if self.cursor_now != Some(shape) {
            debug!(
                "desktop: pointer shape {shape:?} (serial {}, was {:?})",
                self.enter_serial, self.cursor_now
            );
            device.set_shape(self.enter_serial, shape);
            self.cursor_now = Some(shape);
            self.desktop.cursor_sent = Some(now);
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
        if locked_shortcut(item).is_some() {
            info!("desktop: {} is a shortcut not yet let run", item.name);
            let body = format!(
                "{} {}",
                item.name,
                crate::i18n::tr("is a shortcut from elsewhere. If you trust it: right-click → Allow to run.")
            );
            std::thread::spawn(move || crate::desktop_send_notify(&body));
            return;
        }
        let (exec, terminal) = match &item.exec {
            Some((exec, terminal)) => (exec.clone(), *terminal),
            // A plugged-in volume is asked of the FILE MANAGER by name (the
            // desktop's own FileManager1 service): `xdg-open` types a mount
            // point as `inode/mount-point`, which nothing claims, and hands
            // it to the browser (Max, 2026-10-08: *"now it opens on seam"*).
            // `xdg-open` stays as the way out if no file manager answers.
            None if item.kind == Kind::Volume => (
                format!(
                    "busctl --user call org.freedesktop.FileManager1 /org/freedesktop/FileManager1 \
                     org.freedesktop.FileManager1 ShowFolders ass 1 {uri} '' || xdg-open {path}",
                    uri = launch::shell_quote(&file_uri(&item.path)),
                    path = launch::shell_quote(&item.path),
                ),
                false,
            ),
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
        // `menu [n]`: the menu on item n (or the wallpaper's, mid-surface);
        // `pick <row>`: choose that row of the open menu.
        if let Some(rest) = what.strip_prefix("menu") {
            let (w, h) = self.desktop_size;
            let at = match rest.trim().parse::<usize>().ok().and_then(|i| self.desktop.slots.get(i).copied().flatten()) {
                Some(slot) => {
                    let r = icon_rect(&self.desktop.grid.rect(slot), self.icon_scale());
                    (r.x + r.w / 2.0, r.y + r.h / 2.0)
                }
                None => (w as f32 / 2.0, h as f32 / 2.0),
            };
            self.desktop_open_menu(at);
            let rows = self.desktop.menu.as_ref().map_or(0, |m| m.rows.len());
            return format!("menu at ({:.0},{:.0}) with {rows} rows", at.0, at.1);
        }
        if let Some(rest) = what.strip_prefix("props ") {
            return match rest.trim().parse::<usize>().ok().and_then(|i| self.desktop.slots.get(i).copied().flatten().map(|s| (i, s))) {
                Some((i, slot)) => {
                    let r = icon_rect(&self.desktop.grid.rect(slot), self.icon_scale());
                    self.desktop_open_props(i, (r.x + r.w / 2.0, r.y + r.h / 2.0), None);
                    format!("properties of {i}")
                }
                None => "props <n>".to_owned(),
            };
        }
        if let Some(rest) = what.strip_prefix("pick ") {
            let Some(menu) = self.desktop.menu.take() else {
                return "no menu is up".to_owned();
            };
            return match rest.trim().parse::<usize>().ok().and_then(|r| menu.action(r)) {
                // The page turns and the menu stays, as under the pointer.
                Some(Action::MoveTo) => {
                    self.desktop.menu = Some(menu);
                    self.desktop_show_targets();
                    "MoveTo".to_owned()
                }
                Some(Action::Back) => {
                    let mut menu = menu;
                    menu.show_main(self.desktop_size.1 as f32);
                    self.desktop.menu = Some(menu);
                    self.request_desktop_draw();
                    "Back".to_owned()
                }
                Some(action) => {
                    self.desktop_menu_act(action, menu.item, menu.at);
                    self.request_desktop_draw();
                    format!("{action:?}")
                }
                None => "pick <row of an action>".to_owned(),
            };
        }
        match what.trim() {
            "hide" | "show" | "toggle" => {
                let want_hidden = match what.trim() {
                    "hide" => true,
                    "show" => false,
                    _ => !self.desktop.hidden,
                };
                if want_hidden != self.desktop.hidden {
                    self.desktop_toggle_hidden();
                }
                return format!("icons {}", if self.desktop.hidden { "put away" } else { "shown" });
            }
            _ => {}
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
            let brought = import(&uri_list_paths(&list), &desktop_dir(), true);
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
            _ => "debug-desktop [reload|open <n>|move <n> <col> <row>|import <col> <row> <uri…>|select <n…>|band <x0> <y0> <x1> <y1>|hide|show|toggle|forget]"
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
        // 9 × 104 = 936 of 1000: 64 spare. A quarter of it on each side (half
        // of what centring would leave), the rest shared between the columns.
        assert_eq!(g.x0, 16.0);
        assert_eq!(g.rect((0, 0)), Rect::new(16.0, MARGIN, GRID_CELL_W, CELL_H));
        let last = g.rect((8, 0));
        assert!((1000.0 - (last.x + last.w) - 16.0).abs() < 1e-3, "the same air on both sides");
        assert!((g.pitch - (GRID_CELL_W + 4.0)).abs() < 1e-3);
        assert!((g.rect((1, 2)).x - (16.0 + g.pitch)).abs() < 1e-3);
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
        assert_eq!(g.slot_at((g.x0 + 1.0, MARGIN + 1.0)), Some((0, 0)));
        assert_eq!(g.slot_at((g.x0 + g.pitch + 1.0, MARGIN + CELL_H + 1.0)), Some((1, 1)));
        assert_eq!(g.slot_at((g.x0 + GRID_CELL_W + 1.0, MARGIN + 1.0)), None, "between two columns");
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
        assert!(stick(&items, &slots, &g, &mut remembered));
        assert!(!stick(&items, &slots, &g, &mut remembered), "nothing new the second time");
        // "a" sorts first — but b, c, d keep their cells; a takes the free one.
        let items = vec![item("a"), item("b"), item("c"), item("d")];
        let slots = place(&items, &remembered, &g);
        assert_eq!(slots, vec![Some((1, 0)), Some((0, 0)), Some((0, 1)), Some((0, 2))]);
        assert!(stick(&items, &slots, &g, &mut remembered), "a's cell is learned");
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
        assert_eq!(d.hit((g.x0 + 1.0, MARGIN + 1.0)), Some(0));
        assert_eq!(d.hit((g.x0 + 1.0, MARGIN + CELL_H + 1.0)), Some(1));
        assert_eq!(d.hit((1.0, 1.0)), None);
        assert_eq!(d.hit((g.x0 + g.pitch + 1.0, MARGIN + 1.0)), None);
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
        // Not marked executable, a shortcut is a file under its real name…
        let locked = list(&dir);
        let mail = locked.iter().find(|i| i.name == "mail.desktop").expect("shown as a file");
        assert_eq!(mail.kind, Kind::File);
        assert!(mail.exec.is_none());
        assert!(locked_shortcut(mail).is_some(), "…that could be let run");
        // …and once it is (Allow to run), it is the app's shortcut.
        {
            use std::os::unix::fs::PermissionsExt;
            let f = dir.join("mail.desktop");
            let mut perms = std::fs::metadata(&f).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&f, perms).unwrap();
        }
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
            layers: &[],
            carried: None,
            only: None,
            selected: &[],
            band: None,
            menu: None,
            props: None,
            rename: None,
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
            layers: &[],
            carried: None,
            only: None,
            selected: &[false, true, false],
            band: Some(band),
            menu: None,
            props: None,
            rename: None,
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
    fn a_group_in_hand_travels_around_the_grabbed_one_which_is_the_compositors() {
        let items = vec![item("a"), item("b"), item("c")];
        let g = Grid::new(1000.0, 400.0, 1.0);
        let slots = vec![Some((0, 0)), Some((0, 1)), Some((1, 0))];
        let names: Vec<String> = items.iter().map(|i| i.name.clone()).collect();
        // a grabbed, with b; the drag is over the desktop with a's icon at (300,200).
        let live = Live {
            in_hand: &[0, 1],
            layers: &[],
            carried: Some((300.0, 200.0)),
            only: None,
            selected: &[true, true, false],
            band: None,
            menu: None,
            props: None,
            rename: None,
        };
        let s = scene(&items, &slots, &g, &names, &[true, true, true], 1.0, live);
        // b's icon travels, in the overlay, one cell below where a's is.
        assert_eq!(s.overlay.len(), 1, "b's picture; a's is the compositor's");
        assert_eq!(s.overlay[0].layer, 1);
        assert!((s.overlay[0].rect.x - 300.0).abs() < 1e-4);
        assert!((s.overlay[0].rect.y - (200.0 + CELL_H)).abs() < 1e-4);
        // c stays in its cell; a and b carry their names (a's under the image).
        assert_eq!(s.icons.len(), 1);
        assert_eq!(s.icons[0].layer, 2);
        assert_eq!(s.labels.len(), 6, "two labels each for a, b and c");
        assert!(s.labels.iter().any(|l| l.text == "a" && (l.pos.0 - (300.0 + GRID_ICON / 2.0)).abs() < 1e-4));
        // The drag elsewhere (over the dock): the group is simply not drawn.
        let away = Live {
            in_hand: &[0, 1],
            layers: &[],
            carried: None,
            only: None,
            selected: &[true, true, false],
            band: None,
            menu: None,
            props: None,
            rename: None,
        };
        let s = scene(&items, &slots, &g, &names, &[true, true, true], 1.0, away);
        assert!(s.overlay.is_empty());
        assert_eq!(s.labels.len(), 2, "only c");
    }

    #[test]
    fn what_is_plugged_in_stands_on_the_right_and_shows_alone_once() {
        let g = Grid::new(240.0, 300.0, 1.0); // 2 × 3
        let mut vol = item("STICK");
        vol.kind = Kind::Volume;
        let mut phone = item("Pixel");
        phone.kind = Kind::Volume;
        let items = vec![vol, phone, item("a"), item("b")];
        let slots = place(&items, &HashMap::new(), &g);
        // Volumes: top-right, downwards; files: top-left, as ever.
        assert_eq!(slots, vec![Some((1, 0)), Some((1, 1)), Some((0, 0)), Some((0, 1))]);
        let names: Vec<String> = items.iter().map(|i| i.name.clone()).collect();
        let all = [true, true, true, true];
        // Icons away, the stick shown alone: it is all that is drawn, and
        // all that can be hit.
        let solo: HashSet<String> = HashSet::from([items[0].path.clone()]);
        let live = Live { only: Some(&solo), ..Default::default() };
        let s = scene(&items, &slots, &g, &names, &all, 1.0, live);
        assert_eq!(s.icons.len(), 1);
        assert_eq!(s.icons[0].layer, 0);
        assert!((s.icons[0].rect.w - GRID_ICON).abs() < 1e-4, "whole: it fades, it does not shrink");
        assert_eq!(s.labels.len(), 2);
        let mid = |s: Slot| { let r = g.rect(s); (r.x + r.w / 2.0, r.y + r.h / 2.0) };
        let mut d = Desktop { items, grid: g, slots, hidden: true, solo, solo_on: true, ..Default::default() };
        assert_eq!(d.hit(mid((1, 0))), Some(0));
        assert_eq!(d.hit(mid((1, 1))), None, "the phone was not just connected");
        assert_eq!(d.hit(mid((0, 0))), None);
        // Once put away with the rest, nothing answers.
        d.solo_on = false;
        assert_eq!(d.hit(mid((1, 0))), None);
    }

    #[test]
    fn a_smaller_grid_does_not_overwrite_where_an_icon_lives() {
        let big = Grid::new(1000.0, 400.0, 1.0); // 9 × 4
        let small = Grid::new(240.0, 300.0, 1.0); // 2 × 3
        let items = vec![item("a")];
        let mut remembered = HashMap::from([("/d/a".to_owned(), (7, 3))]);
        // For now it sits where there is room…
        let slots = place(&items, &remembered, &small);
        assert_eq!(slots, vec![Some((0, 0))]);
        assert!(!stick(&items, &slots, &small, &mut remembered));
        // …and its own cell is still known when the grid is itself again.
        assert_eq!(remembered["/d/a"], (7, 3));
        assert_eq!(place(&items, &remembered, &big), vec![Some((7, 3))]);
    }

    #[test]
    fn a_dropped_folder_holding_the_desktop_is_refused_and_copy_only_keeps_the_source() {
        let dir = make_dir(&[], &["home/Desktop", "elsewhere"]);
        std::fs::write(dir.join("elsewhere/f.txt"), "f").unwrap();
        let desk = dir.join("home/Desktop");
        assert!(import(&[dir.join("home")], &desk, true).is_empty());
        assert!(std::fs::read_dir(&desk).unwrap().next().is_none(), "nothing half-made");
        // A source that only lets its files be copied keeps them.
        let brought = import(&[dir.join("elsewhere/f.txt")], &desk, false);
        assert_eq!(brought, vec![desk.join("f.txt")]);
        assert!(dir.join("elsewhere/f.txt").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_phone_says_whether_it_lets_us_in() {
        let listing = "List of devices attached\n3A301FDJG000UW\tunauthorized\nABCDEF123\tdevice\n\n";
        assert_eq!(debugging_of(listing, "3A301FDJG000UW"), Debugging::NotAllowed);
        assert_eq!(debugging_of(listing, "ABCDEF123"), Debugging::Ready);
        assert_eq!(debugging_of(listing, "ZZZZZZ"), Debugging::Off);
        assert_eq!(debugging_of("X1Y2Z3\toffline\n", "X1Y2Z3"), Debugging::Unknown);
    }

    #[test]
    fn a_mirror_window_has_the_phones_shape() {
        assert_eq!(parse_wm_size("Physical size: 1344x2992\n"), Some((1344.0, 2992.0)));
        assert_eq!(
            parse_wm_size("Physical size: 1344x2992\nOverride size: 1008x2244\n"),
            Some((1008.0, 2244.0))
        );
        assert_eq!(parse_wm_size("error: device unauthorized"), None);
        assert_eq!(mirror_size((1344.0, 2992.0), 1000.0), (323, 720));
    }

    #[test]
    fn a_phones_serial_is_read_from_its_mount() {
        assert_eq!(
            phone_serial("mtp://Google_Pixel_8_Pro_3A301FDJG000UW/").as_deref(),
            Some("3A301FDJG000UW")
        );
        assert_eq!(phone_serial("mtp://[usb:001,012]/"), None, "a bus address is not a serial");
        assert_eq!(phone_serial("file:///run/media/x/STICK"), None);
    }

    #[test]
    fn arrows_go_to_the_nearest_icon_that_way() {
        // a b .      (0,0) (1,0)
        // c . d      (0,1)       (2,1)
        let slots = vec![Some((0, 0)), Some((1, 0)), Some((0, 1)), Some((2, 1)), None];
        assert_eq!(neighbour(&slots, None, (1, 0)), Some(0), "nothing selected: the first");
        assert_eq!(neighbour(&slots, Some((0, 0)), (1, 0)), Some(1));
        assert_eq!(neighbour(&slots, Some((0, 0)), (0, 1)), Some(2));
        assert_eq!(neighbour(&slots, Some((0, 0)), (-1, 0)), None, "nothing that way");
        // From c rightwards: d in its own row beats b one row up… at equal cost the
        // straight one wins (b: 1 along + 2×1 across = 3; d: 2 along = 2).
        assert_eq!(neighbour(&slots, Some((0, 1)), (1, 0)), Some(3));
        assert_eq!(neighbour(&slots, Some((2, 1)), (0, -1)), Some(1));
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
        let brought = import(&paths, &dir, true);
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

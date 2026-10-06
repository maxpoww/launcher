//! The DESKTOP: `~/Desktop` drawn as icons behind the windows, the way
//! macOS and Windows keep a desktop.
//!
//! One layer-shell surface on the `Bottom` layer (above the wallpaper,
//! below every window — see [`crate::surface::create_desktop_surface`]),
//! with its own renderer and icon texture array, like the deck. The folder
//! is listed into [`Item`]s, laid out on a grid that fills column by column
//! from the top-left, and drawn with the Files section's tile idiom: the
//! file-type carrier (or the file's own thumbnail) over a one-line name.
//! A launcher (`.desktop` file) wears its app icon and name.
//!
//! Read-only for now: a click opens the item (files and folders through
//! `xdg-open`, a launcher through its `Exec=`), the pointer magnifies the
//! icons under it as it does over the grid, and the folder is watched so
//! the icons follow it. Nothing moves, selects, renames or drags yet.
//!
//! Pointer-free: `waverunner-ctl debug-desktop [reload|open <n>|hover <x> <y>]`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::shell::WaylandSurface;
use tracing::{error, info, warn};
use wayland_client::protocol::wl_pointer;
use wayland_client::WEnum;

use crate::content::{
    falloff, IconInst, Label, Rect, Scene, GRID_CELL_W, GRID_ICON, GRID_ICON_TOP, GRID_MAGNIFY,
    GRID_MAG_RADIUS, LABEL_FONT_PX, LABEL_LINE_PX, NO_PLATE, PLATE_STATIC,
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
/// The name's colour, and the shadow under it that keeps it readable on any
/// wallpaper (white on a bright picture would otherwise vanish).
const INK: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const INK_SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.6];
/// Environment override of the folder, for a test rig that must not show
/// the owner's real desktop.
const DIR_ENV: &str = "WAVERUNNER_DESKTOP_DIR";

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

/// The desktop's state on the loop.
#[derive(Default)]
pub(crate) struct Desktop {
    pub items: Vec<Item>,
    /// One cell per item that fits on the surface, in item order (an item
    /// without a cell is not shown).
    pub cells: Vec<Rect>,
    /// Pointer position on the surface, while it is over an icon.
    pub ptr: Option<(f32, f32)>,
    /// The item the left button went down on, until it comes up.
    pub pressed: Option<usize>,
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
/// (case-insensitively, as a desktop is sorted, not as `ls` is).
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

/// Cells for `n` items on a `w`×`h` surface at the dock's icon scale:
/// column by column from the top-left, a new column when one is full, and
/// no cell at all for an item past the last column.
pub(crate) fn layout(n: usize, w: f32, h: f32, icon_scale: f32) -> Vec<Rect> {
    let cw = GRID_CELL_W * icon_scale;
    let ch = CELL_H * icon_scale;
    let rows = (((h - 2.0 * MARGIN) / ch).floor() as usize).max(1);
    let cols = (((w - 2.0 * MARGIN) / cw).floor() as usize).max(1);
    (0..n.min(rows * cols))
        .map(|i| {
            let (col, row) = (i / rows, i % rows);
            Rect::new(MARGIN + col as f32 * cw, MARGIN + row as f32 * ch, cw, ch)
        })
        .collect()
}

/// The item under `pos`, if any.
pub(crate) fn hit(cells: &[Rect], pos: (f32, f32)) -> Option<usize> {
    cells.iter().position(|c| c.contains(pos))
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

/// One frame of the desktop: each shown item's icon (where its picture has
/// arrived — `has_icon[i]`) magnified toward the pointer as the grid's are,
/// and its name (`names[i]`, already fitted) beneath, shadowed so it reads
/// on any wallpaper.
pub(crate) fn scene(
    items: &[Item],
    cells: &[Rect],
    names: &[String],
    has_icon: &[bool],
    ptr: Option<(f32, f32)>,
    icon_scale: f32,
) -> Scene {
    let mut scene = Scene {
        alpha: 1.0,
        ..Default::default()
    };
    for (i, (item, cell)) in items.iter().zip(cells).enumerate() {
        let at_rest = icon_rect(cell, icon_scale);
        let (cx, cy) = (at_rest.x + at_rest.w / 2.0, at_rest.y + at_rest.h / 2.0);
        let magnify = match ptr {
            Some((px, py)) => {
                let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                1.0 + (GRID_MAGNIFY - 1.0) * falloff(d, GRID_MAG_RADIUS)
            }
            None => 1.0,
        };
        let size = at_rest.w * magnify;
        if has_icon.get(i).copied().unwrap_or(false) {
            scene.icons.push(IconInst {
                rect: Rect::new(cx - size / 2.0, cy - size / 2.0, size, size),
                layer: i as u32,
                tint: [0.0; 4],
                ring: -1.0,
                // Carriers and thumbnails are bare, as in the Files section;
                // a launcher is an app tile and gets the plate the dock's
                // static surfaces use.
                plate: if item.kind == Kind::Launcher {
                    PLATE_STATIC
                } else {
                    NO_PLATE
                },
            });
        }
        let max_w = label_max_w(cell);
        let text = names.get(i).cloned().unwrap_or_else(|| item.name.clone());
        let top = at_rest.y + at_rest.h + LABEL_GAP;
        for (dy, color) in [(1.0, INK_SHADOW), (0.0, INK)] {
            scene.labels.push(Label {
                text: text.clone(),
                pos: (cx, top + dy),
                max_w,
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
        self.desktop.items = items;
        // Drop the pictures nothing wears any more (a thumbnail is ~350 KB).
        let worn: HashSet<&str> = self.desktop.items.iter().map(|it| it.icon.as_str()).collect();
        self.desktop.chains.retain(|k, _| worn.contains(k.as_str()));
        self.desktop.pending.retain(|k| worn.contains(k.as_str()));
        self.desktop.missing.clear();
        self.desktop.pressed = None;
        self.relayout_desktop();
    }

    /// Place the items on the surface, open the input over them, bring their
    /// pictures into the texture array, and draw.
    pub(crate) fn relayout_desktop(&mut self) {
        let (w, h) = self.desktop_size;
        if w == 0 || h == 0 {
            return;
        }
        let icon_scale = self.icon_scale();
        self.desktop.cells = layout(self.desktop.items.len(), w as f32, h as f32, icon_scale);
        self.sync_desktop_input();
        self.sync_desktop_icons();
        self.request_desktop_draw();
    }

    /// The surface takes the pointer over the icons only; the wallpaper
    /// between them is not ours to catch.
    fn sync_desktop_input(&mut self) {
        let Some(layer) = self.desktop_layer.as_ref() else {
            return;
        };
        let rects: Vec<(i32, i32, i32, i32)> = self
            .desktop
            .cells
            .iter()
            .map(|c| {
                (
                    c.x.floor() as i32,
                    c.y.floor() as i32,
                    c.w.ceil() as i32,
                    c.h.ceil() as i32,
                )
            })
            .collect();
        crate::surface::set_input_rects(&self.compositor, layer, &rects);
    }

    /// Every shown item's picture into its layer (= its index): uploaded
    /// where the pixels are in hand, asked of the resolver where not.
    fn sync_desktop_icons(&mut self) {
        let shown = self.desktop.cells.len();
        if shown == 0 {
            return;
        }
        // Size the array for what is shown plus headroom, so a file arriving
        // is a single-layer write and not a reallocation — which also clears
        // the array, hence `uploaded` is forgotten with it.
        if shown as u32 > self.desktop.capacity {
            self.desktop.capacity = shown as u32 + LAYER_HEADROOM;
            if let Some(r) = self.desktop_renderer.as_mut() {
                r.alloc_icon_array(self.desktop.capacity);
            }
            self.desktop.uploaded.clear();
        }
        self.desktop
            .uploaded
            .resize(self.desktop.capacity as usize, None);
        let mut asked = Vec::new();
        for i in 0..shown {
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
        let has_icon: Vec<bool> = (0..self.desktop.cells.len())
            .map(|i| self.desktop.has_icon(i))
            .collect();
        let icon_scale = self.icon_scale();
        let Some(renderer) = self.desktop_renderer.as_mut() else {
            return;
        };
        let names: Vec<String> = self
            .desktop
            .items
            .iter()
            .zip(&self.desktop.cells)
            .map(|(item, cell)| {
                fit_label(&item.name, label_max_w(cell), &mut |t| {
                    renderer.measure_text(t, LABEL_FONT_PX, None)
                })
            })
            .collect();
        let scene = scene(
            &self.desktop.items,
            &self.desktop.cells,
            &names,
            &has_icon,
            self.desktop.ptr,
            icon_scale,
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

    /// Route a pointer event on the desktop surface: motion steers the
    /// magnification, a left press-and-release on one item opens it.
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
                self.desktop.pressed = None;
                if self.desktop.ptr.take().is_some() {
                    self.request_desktop_draw();
                }
            }
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(state),
                ..
            } if button == crate::BTN_LEFT => {
                let under = self.desktop.ptr.and_then(|p| hit(&self.desktop.cells, p));
                match state {
                    wl_pointer::ButtonState::Pressed => self.desktop.pressed = under,
                    wl_pointer::ButtonState::Released => {
                        let pressed = self.desktop.pressed.take();
                        if let (Some(i), true) = (under, pressed == under) {
                            self.desktop_activate(i);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The pointer moved over the desktop: the icons under it swell.
    fn desktop_motion(&mut self, x: f32, y: f32) {
        self.desktop.ptr = Some((x, y));
        self.desktop_cursor();
        self.request_desktop_draw();
    }

    /// A hand over an item, the arrow between them.
    fn desktop_cursor(&mut self) {
        let Some(device) = &self.cursor_device else {
            return;
        };
        let over = self.desktop.ptr.and_then(|p| hit(&self.desktop.cells, p));
        let shape = if over.is_some() {
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
    /// <n>`; `hover <x> <y>` (the pointer drawn there, for a picture).
    pub(crate) fn desktop_debug(&mut self, what: &str) -> String {
        if self.desktop_layer.is_none() {
            return "no desktop surface (disabled, or closed)".to_owned();
        }
        let mut words = what.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (None, _, _) => {
                let (w, h) = self.desktop_size;
                let mut out = format!(
                    "{} in {} on {w}×{h}, {} shown",
                    self.desktop.items.len(),
                    desktop_dir().display(),
                    self.desktop.cells.len()
                );
                for (i, item) in self.desktop.items.iter().enumerate() {
                    let cell = self.desktop.cells.get(i).map_or_else(
                        || "(not shown)".to_owned(),
                        |c| format!("({:.0},{:.0} {:.0}×{:.0})", c.x, c.y, c.w, c.h),
                    );
                    let pic = if self.desktop.has_icon(i) { "" } else { " [no picture yet]" };
                    out.push_str(&format!(
                        "\n  {i}: {:?} {} {cell} {}{pic}",
                        item.kind, item.name, item.icon
                    ));
                }
                out
            }
            (Some("reload"), _, _) => {
                self.reload_desktop();
                format!("reloaded: {} items", self.desktop.items.len())
            }
            (Some("open"), Some(n), _) => match n.parse::<usize>() {
                Ok(i) if i < self.desktop.items.len() => {
                    self.desktop_activate(i);
                    format!("opened {i}")
                }
                _ => format!("no item {n}"),
            },
            (Some("hover"), Some(x), Some(y)) => match (x.parse::<f32>(), y.parse::<f32>()) {
                (Ok(x), Ok(y)) => {
                    self.desktop.ptr = Some((x, y));
                    self.request_desktop_draw();
                    format!("pointer drawn at {x},{y}")
                }
                _ => "hover <x> <y>".to_owned(),
            },
            _ => "debug-desktop [reload|open <n>|hover <x> <y>]".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn layout_fills_columns_top_down_then_moves_right() {
        // Cells 104×92 at scale 1: a 400-tall surface holds 4 rows.
        let cells = layout(6, 1000.0, 400.0, 1.0);
        assert_eq!(cells.len(), 6);
        assert_eq!((cells[0].x, cells[0].y), (MARGIN, MARGIN));
        assert_eq!(cells[1].y, MARGIN + CELL_H);
        assert_eq!(cells[3].y, MARGIN + 3.0 * CELL_H);
        // The fifth starts the second column, back at the top.
        assert_eq!((cells[4].x, cells[4].y), (MARGIN + GRID_CELL_W, MARGIN));
        assert_eq!(cells[5].y, MARGIN + CELL_H);
    }

    #[test]
    fn layout_drops_what_does_not_fit_and_never_divides_by_zero() {
        // 1 row × 2 columns: the third item has no cell.
        assert_eq!(layout(3, 240.0, 120.0, 1.0).len(), 2);
        // A surface too small for one cell still gives one row and column
        // rather than nothing (or a panic).
        assert_eq!(layout(2, 10.0, 10.0, 1.0).len(), 1);
        assert!(layout(0, 1000.0, 1000.0, 1.0).is_empty());
        // The icon-size setting scales the cells.
        let big = layout(1, 1000.0, 1000.0, 1.3)[0];
        assert!((big.w - GRID_CELL_W * 1.3).abs() < 1e-4);
    }

    #[test]
    fn hit_finds_the_cell_under_the_pointer() {
        let cells = layout(3, 1000.0, 400.0, 1.0);
        assert_eq!(hit(&cells, (MARGIN + 1.0, MARGIN + 1.0)), Some(0));
        assert_eq!(hit(&cells, (MARGIN + 1.0, MARGIN + CELL_H + 1.0)), Some(1));
        assert_eq!(hit(&cells, (1.0, 1.0)), None);
        assert_eq!(hit(&cells, (MARGIN + GRID_CELL_W + 1.0, MARGIN + 1.0)), None);
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
    fn scene_draws_only_arrived_icons_and_every_name_twice() {
        let items = vec![
            Item {
                path: "/d/a.txt".into(),
                name: "a.txt".into(),
                kind: Kind::File,
                icon: "asset:text-x-generic".into(),
                exec: None,
            },
            Item {
                path: "/d/b.txt".into(),
                name: "b.txt".into(),
                kind: Kind::File,
                icon: "asset:text-x-generic".into(),
                exec: None,
            },
        ];
        let cells = layout(2, 1000.0, 400.0, 1.0);
        let names = vec!["a.txt".to_owned(), "b.txt".to_owned()];
        let s = scene(&items, &cells, &names, &[true, false], None, 1.0);
        assert_eq!(s.icons.len(), 1);
        assert_eq!(s.icons[0].layer, 0);
        assert_eq!(s.labels.len(), 4, "a shadow and an ink label per item");
        assert_eq!(s.labels[0].color, Some(INK_SHADOW));
        assert_eq!(s.labels[1].color, Some(INK));
        assert_eq!(s.labels[1].text, "a.txt");
        // The pointer on an icon's centre swells it to the grid's peak.
        let icon = icon_rect(&cells[0], 1.0);
        let centre = (icon.x + icon.w / 2.0, icon.y + icon.h / 2.0);
        let s = scene(&items, &cells, &names, &[true, false], Some(centre), 1.0);
        assert!((s.icons[0].rect.w - GRID_ICON * GRID_MAGNIFY).abs() < 1e-3);
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

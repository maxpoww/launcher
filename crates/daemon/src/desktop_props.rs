//! The desktop's Properties box (Max, 2026-10-08: "a little box with the
//! properties of the dir… default info a user will want to know"): hovering
//! the menu's last row turns the menu INTO this box — the panel grows from
//! the menu's shape to its own — with the item's name and what there is to
//! know: what it is, what is in it, how big, where it lives, who may change
//! it, when it was changed, made and last opened. A Back row at the top
//! turns it back into the menu; a click anywhere else closes it.
//!
//! Pure here: reading the facts, laying the panel out and drawing it. A
//! folder's total size is walked off the loop ([`folder_size`]) and filled
//! in when it arrives. The desktop (`desktop.rs`) opens and closes it.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::SystemTime;

use crate::content::{GridContent, Label, Rect, RectInst, Scene, FONT_BOLD};
use crate::desktop::{Item, Kind};
use crate::desktop_menu::{Menu, MenuPaint};

/// The panel's width, its padding, and the lines' metrics.
pub(crate) const WIDTH: f32 = 336.0;
const PAD: f32 = 16.0;
/// The Back row (only when there is a menu to go back to).
const BACK_H: f32 = 28.0;
const BACK_GAP: f32 = 6.0;
const TITLE_PX: f32 = 15.0;
const TITLE_LINE: f32 = 20.0;
const TITLE_GAP: f32 = 10.0;
const ROW_H: f32 = 25.0;
const FONT_PX: f32 = 13.0;
const LINE_PX: f32 = 17.0;
/// Where the values start, from the panel's inner left edge.
const VALUE_X: f32 = 96.0;
/// How far from the pointer the panel's corner sits (opened without a menu).
const GAP: f32 = 6.0;
/// The keys' ink, and the rule's, as shares of the panel's ink.
const KEY_INK: f32 = 0.55;
const LINE_INK: f32 = 0.14;
/// What the Size line says until a folder has been walked.
pub(crate) const CALCULATING: &str = "Calculating…";

/// A Properties box that is up.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Props {
    /// The item it describes (a folder's size is filed by this).
    pub path: String,
    pub title: String,
    /// The lines: what, and the answer.
    pub rows: Vec<(&'static str, String)>,
    pub rect: Rect,
    /// Its entrance, 0..1.
    pub t: f32,
    /// The menu it grew out of: Back turns it back into that.
    pub back: Option<Menu>,
    /// The shape it grows from (the menu's panel); `None`: it fades in.
    pub grow_from: Option<Rect>,
}

/// `a` on its way to `b`.
pub(crate) fn lerp_rect(a: &Rect, b: &Rect, t: f32) -> Rect {
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rect::new(l(a.x, b.x), l(a.y, b.y), l(a.w, b.w), l(a.h, b.h))
}

/// `2026-10-08 13:05` in local time (language-neutral), or nothing for a
/// time the filesystem did not keep.
pub(crate) fn date_text(time: Option<SystemTime>) -> Option<String> {
    let secs = time?.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs() as libc::time_t;
    // SAFETY: `localtime_r` fills a caller-owned `tm` from a valid `time_t`.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return None;
        }
        Some(format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min
        ))
    }
}

/// What kind of thing a file is, in a word or two, by its name.
fn file_kind(name: &str) -> String {
    let what = match crate::files::file_asset_name(name) {
        "asset-audio" => "Audio",
        "asset-video" => "Video",
        "asset-image" => "Image",
        "asset-pdf" => "Document",
        "asset-archive" => "Archive",
        "asset-doc" => "Document",
        "asset-code" => "Code",
        _ => "File",
    };
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 8 => {
            format!("{what} ({})", ext.to_ascii_lowercase())
        }
        _ => what.to_owned(),
    }
}

/// A size as people read it: "0 B", "9.8 KB", "2.4 MB", "1.2 GB". (The
/// gear's formatter only knows MB and GB: a small file read "0 MB".)
pub(crate) fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let (mut value, mut unit) = (bytes as f64 / 1024.0, 0);
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `3 folders, 12 files` — what a folder holds at its top, with how many of
/// them are hidden when some are.
pub(crate) fn contains_text(folders: usize, files: usize, hidden: usize) -> String {
    let n = |n: usize, one: &str, many: &str| match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    let mut parts = Vec::new();
    if folders > 0 {
        parts.push(n(folders, "folder", "folders"));
    }
    if files > 0 {
        parts.push(n(files, "file", "files"));
    }
    if parts.is_empty() {
        return "Nothing".to_owned();
    }
    let mut out = parts.join(", ");
    if hidden > 0 {
        out.push_str(&format!(" ({hidden} hidden)"));
    }
    out
}

/// A path with the home folder written `~`.
pub(crate) fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Whether we may change the thing at `path`.
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated path; `access` only reads it.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Whose it is: `You`, or the account's name (its number, failing that).
fn owner_text(uid: u32) -> String {
    // SAFETY: `getuid` has no preconditions; `getpwuid` returns a pointer
    // into static storage (or null), read at once on this one thread.
    unsafe {
        if uid == libc::getuid() {
            return "You".to_owned();
        }
        let pw = libc::getpwuid(uid);
        if !pw.is_null() && !(*pw).pw_name.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        }
    }
    format!("user {uid}")
}

/// The lines for `item`, read from the disk now. A folder's Size says
/// [`CALCULATING`] until [`Props::set_size`] fills it in.
pub(crate) fn rows_for(item: &Item, home: &Path) -> Vec<(&'static str, String)> {
    let path = Path::new(&item.path);
    let meta = std::fs::metadata(path).ok();
    let link = std::fs::read_link(path).ok();
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    match item.kind {
        Kind::Folder | Kind::Volume => {
            rows.push(("Kind", if item.kind == Kind::Volume { "Drive" } else { "Folder" }.to_owned()));
            let (mut folders, mut files, mut hidden) = (0usize, 0usize, 0usize);
            for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
                if e.file_name().to_string_lossy().starts_with('.') {
                    hidden += 1;
                }
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    folders += 1;
                } else {
                    files += 1;
                }
            }
            rows.push(("Contains", contains_text(folders, files, hidden)));
            rows.push(("Size", CALCULATING.to_owned()));
        }
        Kind::Launcher => {
            rows.push(("Kind", "Application".to_owned()));
            if let Some((exec, _)) = &item.exec {
                rows.push(("Runs", exec.clone()));
            }
        }
        Kind::File => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            rows.push(("Kind", file_kind(&name)));
            if let Some(m) = &meta {
                rows.push(("Size", size_text(m.len())));
            }
            if crate::files::file_asset_name(&name) == "asset-image" {
                if let Ok((w, h)) = image::image_dimensions(path) {
                    rows.push(("Dimensions", format!("{w} × {h}")));
                }
            }
        }
    }
    if let Some(to) = link {
        rows.push(("Link to", tilde(&to, home)));
    }
    if let Some(parent) = path.parent() {
        rows.push(("Where", tilde(parent, home)));
    }
    if let Some(m) = &meta {
        rows.push(("Owner", owner_text(m.uid())));
        rows.push((
            "Access",
            if writable(path) { "Read and write" } else { "Read only" }.to_owned(),
        ));
    }
    for (key, time) in [
        ("Modified", meta.as_ref().and_then(|m| m.modified().ok())),
        ("Created", meta.as_ref().and_then(|m| m.created().ok())),
        ("Opened", meta.as_ref().and_then(|m| m.accessed().ok())),
    ] {
        if let Some(d) = date_text(time) {
            rows.push((key, d));
        }
    }
    rows
}

/// Everything under `dir`, added up: bytes and how many files. Links are
/// counted as links, not followed (no loops, nothing counted twice).
pub(crate) fn folder_size(dir: &Path) -> (u64, u64) {
    let (mut bytes, mut files) = (0u64, 0u64);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(m) = e.path().symlink_metadata() else {
                continue;
            };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                bytes += m.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

impl Props {
    /// A box for `item` on a `w`×`h` surface: grown out of `menu` (its
    /// corner where the menu's is, Back leading to it again), or by the
    /// pointer at `at` on its own.
    pub fn open(item: &Item, at: (f32, f32), menu: Option<Menu>, w: f32, h: f32, home: &Path) -> Self {
        let rows = rows_for(item, home);
        let title = Path::new(&item.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| item.name.clone());
        let back_h = if menu.is_some() { BACK_H + BACK_GAP } else { 0.0 };
        let height = PAD * 2.0 + back_h + TITLE_LINE + TITLE_GAP * 2.0 + 1.0 + rows.len() as f32 * ROW_H;
        let (x, y) = match &menu {
            Some(m) => (m.rect.x.min(w - WIDTH).max(0.0), m.rect.y.min(h - height).max(0.0)),
            None => (
                if at.0 + GAP + WIDTH <= w { at.0 + GAP } else { (at.0 - GAP - WIDTH).max(0.0) },
                if at.1 + GAP + height <= h { at.1 + GAP } else { (at.1 - GAP - height).max(0.0) },
            ),
        };
        Self {
            path: item.path.clone(),
            title,
            rows,
            rect: Rect::new(x, y, WIDTH, height),
            t: 0.0,
            grow_from: menu.as_ref().map(|m| m.rect),
            back: menu,
        }
    }

    /// The Back row, when there is a menu to go back to.
    pub fn back_rect(&self) -> Option<Rect> {
        self.back.as_ref()?;
        Some(Rect::new(self.rect.x + PAD - 8.0, self.rect.y + PAD - 4.0, 76.0, BACK_H))
    }

    /// A folder's walk came back: its Size line.
    pub fn set_size(&mut self, bytes: u64, files: u64) {
        if let Some(row) = self.rows.iter_mut().find(|(k, _)| *k == "Size") {
            row.1 = if files == 0 {
                "Empty".to_owned()
            } else {
                let n = if files == 1 { "1 file".to_owned() } else { format!("{files} files") };
                format!("{}, {n} in all", size_text(bytes))
            };
        }
    }

    /// Draw the box over everything in `scene`: the panel on its way from
    /// the menu's shape to its own (or fading in), its lines appearing as
    /// it opens.
    pub fn push(&self, scene: &mut Scene, paint: &MenuPaint) {
        let t = self.t.clamp(0.0, 1.0);
        let (panel, panel_alpha) = match &self.grow_from {
            Some(from) => (lerp_rect(from, &self.rect, t), 1.0),
            None => (Rect::new(self.rect.x, self.rect.y + (1.0 - t) * -4.0, self.rect.w, self.rect.h), t),
        };
        let fade = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * t];
        let ink_at = |a: f32| fade([paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * a]);
        let mut grid = GridContent {
            clip: panel,
            ..Default::default()
        };
        grid.rects.push(RectInst {
            rect: panel,
            radius: paint.radius,
            color: [paint.fill[0], paint.fill[1], paint.fill[2], paint.fill[3] * panel_alpha],
            glass: 0.0,
            border: 0.0,
        });
        // The lines sit where they will end up; the growing panel uncovers them.
        let (x, inner_w) = (self.rect.x + PAD, self.rect.w - 2.0 * PAD);
        let mut y = self.rect.y + PAD;
        let label = |text: &str, pos: (f32, f32), max_w: f32, px: f32, line: f32, bold: bool, color: [f32; 4]| Label {
            text: text.to_owned(),
            pos,
            max_w,
            font_px: px,
            line_px: line,
            centered: false,
            dim: false,
            cache: false,
            clip: Some(Rect::new(
                pos.0,
                panel.y,
                max_w.min(panel.x + panel.w - pos.0).max(0.0),
                panel.h,
            )),
            family: bold.then_some(FONT_BOLD),
            color: Some(color),
        };
        if let Some(back) = self.back_rect() {
            grid.labels.push(label(
                "‹  Back",
                (back.x + 8.0, back.y + (BACK_H - LINE_PX) / 2.0),
                back.w - 12.0,
                FONT_PX,
                LINE_PX,
                false,
                ink_at(0.86),
            ));
            y += BACK_H + BACK_GAP;
        }
        grid.labels.push(label(&self.title, (x, y), inner_w, TITLE_PX, TITLE_LINE, true, ink_at(1.0)));
        y += TITLE_LINE + TITLE_GAP;
        grid.rects.push(RectInst {
            rect: Rect::new(x, y, inner_w, 1.0),
            radius: 0.0,
            color: ink_at(LINE_INK),
            glass: 0.0,
            border: 0.0,
        });
        y += 1.0 + TITLE_GAP;
        for (key, value) in &self.rows {
            let top = y + (ROW_H - LINE_PX) / 2.0;
            grid.labels.push(label(key, (x, top), VALUE_X - 8.0, FONT_PX, LINE_PX, false, ink_at(KEY_INK)));
            grid.labels.push(label(
                value,
                (x + VALUE_X, top),
                inner_w - VALUE_X,
                FONT_PX,
                LINE_PX,
                false,
                ink_at(0.92),
            ));
            y += ROW_H;
        }
        scene.grids.push(grid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAINT: MenuPaint = MenuPaint {
        fill: [0.1, 0.1, 0.12, 0.9],
        ink: [0.9, 0.9, 0.9, 1.0],
        wash: [1.0, 1.0, 1.0, 0.1],
        radius: 10.0,
    };

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("waverunner-props-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn folder_item(path: &Path) -> Item {
        Item {
            path: path.to_string_lossy().into_owned(),
            name: "Projects".into(),
            kind: Kind::Folder,
            icon: String::new(),
            exec: None,
        }
    }

    #[test]
    fn a_folder_says_what_it_holds_and_a_walk_fills_its_size() {
        let dir = tmp("folder");
        let f = dir.join("Projects");
        std::fs::create_dir_all(f.join("sub")).unwrap();
        std::fs::write(f.join("a.txt"), "12345").unwrap();
        std::fs::write(f.join(".hidden"), "").unwrap();
        std::fs::write(f.join("sub/b.txt"), "123").unwrap();
        let mut p = Props::open(&folder_item(&f), (10.0, 10.0), None, 2000.0, 2000.0, &dir);
        assert_eq!(p.title, "Projects");
        assert_eq!(p.rows[0], ("Kind", "Folder".to_owned()));
        assert_eq!(p.rows[1], ("Contains", "1 folder, 2 files (1 hidden)".to_owned()));
        assert_eq!(p.rows[2], ("Size", CALCULATING.to_owned()));
        assert_eq!(p.rows[3], ("Where", "~".to_owned()));
        assert_eq!(p.rows[4], ("Owner", "You".to_owned()));
        assert_eq!(p.rows[5], ("Access", "Read and write".to_owned()));
        assert!(p.rows.iter().any(|(k, _)| *k == "Modified"));
        assert!(p.rows.iter().any(|(k, _)| *k == "Opened"));
        let (bytes, files) = folder_size(&f);
        assert_eq!((bytes, files), (8, 3));
        p.set_size(bytes, files);
        assert!(p.rows[2].1.ends_with("3 files in all"), "{}", p.rows[2].1);
        p.set_size(0, 0);
        assert_eq!(p.rows[2].1, "Empty");
        // On its own: no Back row; one grid, the panel and the rule, the
        // title and two labels a row.
        assert!(p.back_rect().is_none());
        p.t = 1.0;
        let mut scene = Scene::default();
        p.push(&mut scene, &PAINT);
        let g = &scene.grids[0];
        assert_eq!(g.rects.len(), 2);
        assert_eq!(g.rects[0].rect, p.rect);
        assert_eq!(g.labels.len(), 1 + 2 * p.rows.len());
        assert_eq!(g.labels[0].family, Some(FONT_BOLD));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grown_out_of_the_menu_it_starts_as_the_menus_shape_and_has_a_way_back() {
        let dir = tmp("grown");
        let f = dir.join("Projects");
        std::fs::create_dir_all(&f).unwrap();
        let menu = Menu::open(Some(0), false, (100.0, 100.0), 2000.0, 2000.0);
        let menu_rect = menu.rect;
        let mut p = Props::open(&folder_item(&f), (0.0, 0.0), Some(menu), 2000.0, 2000.0, &dir);
        // Its corner is the menu's; it is wider and has the Back row.
        assert_eq!((p.rect.x, p.rect.y), (menu_rect.x, menu_rect.y));
        assert!(p.rect.w > menu_rect.w);
        let back = p.back_rect().expect("a way back");
        assert!(back.y >= p.rect.y && back.y + back.h < p.rect.y + p.rect.h);
        // At the start the panel IS the menu's panel, opaque; at the end, its own.
        let mut scene = Scene::default();
        p.push(&mut scene, &PAINT);
        assert_eq!(scene.grids[0].rects[0].rect, menu_rect);
        assert_eq!(scene.grids[0].rects[0].color[3], PAINT.fill[3]);
        p.t = 1.0;
        let mut scene = Scene::default();
        p.push(&mut scene, &PAINT);
        let g = &scene.grids[0];
        assert_eq!(g.rects[0].rect, p.rect);
        assert_eq!(g.labels[0].text, "‹  Back");
        // Near the surface's edge it is pulled in to stay whole.
        let menu = Menu::open(Some(0), false, (900.0, 700.0), 1000.0, 800.0);
        let p = Props::open(&folder_item(&f), (0.0, 0.0), Some(menu), 1000.0, 800.0, &dir);
        assert!(p.rect.x + p.rect.w <= 1000.0 && p.rect.y + p.rect.h <= 800.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_says_its_kind_and_size() {
        let dir = tmp("file");
        let f = dir.join("Desktop");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("notes.TXT"), vec![0u8; 2048]).unwrap();
        let item = Item {
            path: f.join("notes.TXT").to_string_lossy().into_owned(),
            name: "notes.TXT".into(),
            kind: Kind::File,
            icon: String::new(),
            exec: None,
        };
        let p = Props::open(&item, (990.0, 790.0), None, 1000.0, 800.0, &dir);
        assert_eq!(p.rows[0], ("Kind", "File (txt)".to_owned()));
        assert_eq!(p.rows[1].0, "Size");
        assert_eq!(p.rows[2], ("Where", "~/Desktop".to_owned()));
        assert!(p.rect.x + p.rect.w <= 1000.0 && p.rect.y + p.rect.h <= 800.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn small_words() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(2048), "2.0 KB");
        assert_eq!(size_text(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(contains_text(0, 0, 0), "Nothing");
        assert_eq!(contains_text(0, 1, 0), "1 file");
        assert_eq!(contains_text(2, 5, 3), "2 folders, 5 files (3 hidden)");
        assert_eq!(file_kind("README"), "File");
        assert_eq!(file_kind("a.tar.gz"), "Archive (gz)");
        assert_eq!(tilde(Path::new("/srv/x"), Path::new("/home/m")), "/srv/x");
        assert!(date_text(None).is_none());
        assert_eq!(date_text(Some(SystemTime::UNIX_EPOCH)).map(|d| d.len()), Some(16));
        let (a, b) = (Rect::new(0.0, 0.0, 10.0, 10.0), Rect::new(10.0, 20.0, 30.0, 50.0));
        assert_eq!(lerp_rect(&a, &b, 0.5), Rect::new(5.0, 10.0, 20.0, 30.0));
    }
}

//! The desktop's Properties box (Max, 2026-10-08: "a little box with the
//! properties of the dir… default info a user will want to know"): opened
//! from an icon's menu, a small panel of the boxes' material by the pointer
//! with the item's name and a few lines — what it is, how big, what is in
//! it, where it lives, when it was changed and made.
//!
//! Pure here: reading the facts, laying the panel out and drawing it. A
//! folder's total size is walked off the loop ([`folder_size`]) and filled
//! in when it arrives. The desktop (`desktop.rs`) opens and closes it.

use std::path::Path;
use std::time::SystemTime;

use crate::content::{GridContent, Label, Rect, RectInst, Scene, FONT_BOLD};
use crate::desktop::{Item, Kind};
use crate::desktop_menu::MenuPaint;

/// The panel's width, its padding, and the lines' metrics.
pub(crate) const WIDTH: f32 = 272.0;
const PAD: f32 = 14.0;
const TITLE_PX: f32 = 14.0;
const TITLE_LINE: f32 = 19.0;
const TITLE_GAP: f32 = 9.0;
const ROW_H: f32 = 22.0;
const FONT_PX: f32 = 12.5;
const LINE_PX: f32 = 16.0;
/// Where the values start, from the panel's inner left edge.
const VALUE_X: f32 = 78.0;
/// How far from the pointer the panel's corner sits.
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
    /// Its entrance, 0..1, as the menu's.
    pub t: f32,
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

/// `n items`, with how many of them are folders when some are.
pub(crate) fn contains_text(items: usize, folders: usize) -> String {
    let n = match items {
        0 => return "Nothing".to_owned(),
        1 => "1 item".to_owned(),
        n => format!("{n} items"),
    };
    match folders {
        0 => n,
        1 => format!("{n} (1 folder)"),
        f => format!("{n} ({f} folders)"),
    }
}

/// A path with the home folder written `~`.
pub(crate) fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The lines for `item`, read from the disk now. A folder's Size says
/// [`CALCULATING`] until [`Props::set_size`] fills it in.
pub(crate) fn rows_for(item: &Item, home: &Path) -> Vec<(&'static str, String)> {
    let path = Path::new(&item.path);
    let meta = std::fs::metadata(path).ok();
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    match item.kind {
        Kind::Folder => {
            rows.push(("Kind", "Folder".to_owned()));
            let (mut items, mut folders) = (0usize, 0usize);
            for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
                items += 1;
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    folders += 1;
                }
            }
            rows.push(("Contains", contains_text(items, folders)));
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
                rows.push(("Size", crate::gear_pages::size_text(m.len())));
            }
        }
    }
    if let Some(parent) = path.parent() {
        rows.push(("Where", tilde(parent, home)));
    }
    if let Some(d) = date_text(meta.as_ref().and_then(|m| m.modified().ok())) {
        rows.push(("Modified", d));
    }
    if let Some(d) = date_text(meta.as_ref().and_then(|m| m.created().ok())) {
        rows.push(("Created", d));
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
    /// A box for `item` by the pointer at `at`, kept on a `w`×`h` surface.
    pub fn open(item: &Item, at: (f32, f32), w: f32, h: f32, home: &Path) -> Self {
        let rows = rows_for(item, home);
        let title = Path::new(&item.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| item.name.clone());
        let height = PAD * 2.0 + TITLE_LINE + TITLE_GAP * 2.0 + 1.0 + rows.len() as f32 * ROW_H;
        let x = if at.0 + GAP + WIDTH <= w { at.0 + GAP } else { (at.0 - GAP - WIDTH).max(0.0) };
        let y = if at.1 + GAP + height <= h { at.1 + GAP } else { (at.1 - GAP - height).max(0.0) };
        Self {
            path: item.path.clone(),
            title,
            rows,
            rect: Rect::new(x, y, WIDTH, height),
            t: 0.0,
        }
    }

    /// A folder's walk came back: its Size line.
    pub fn set_size(&mut self, bytes: u64, files: u64) {
        if let Some(row) = self.rows.iter_mut().find(|(k, _)| *k == "Size") {
            row.1 = if files == 0 {
                "Empty".to_owned()
            } else {
                let n = if files == 1 { "1 file".to_owned() } else { format!("{files} files") };
                format!("{} ({n})", crate::gear_pages::size_text(bytes))
            };
        }
    }

    /// Draw the box over everything in `scene`, as the menu is drawn.
    pub fn push(&self, scene: &mut Scene, paint: &MenuPaint) {
        let t = self.t.clamp(0.0, 1.0);
        let lift = (1.0 - t) * -4.0;
        let fade = |c: [f32; 4]| [c[0], c[1], c[2], c[3] * t];
        let ink_at = |a: f32| fade([paint.ink[0], paint.ink[1], paint.ink[2], paint.ink[3] * a]);
        let panel = Rect::new(self.rect.x, self.rect.y + lift, self.rect.w, self.rect.h);
        let mut grid = GridContent {
            clip: panel,
            ..Default::default()
        };
        grid.rects.push(RectInst {
            rect: panel,
            radius: paint.radius,
            color: fade(paint.fill),
            glass: 0.0,
            border: 0.0,
        });
        let (x, inner_w) = (panel.x + PAD, panel.w - 2.0 * PAD);
        let mut y = panel.y + PAD;
        let label = |text: &str, pos: (f32, f32), max_w: f32, px: f32, line: f32, bold: bool, color: [f32; 4]| Label {
            text: text.to_owned(),
            pos,
            max_w,
            font_px: px,
            line_px: line,
            centered: false,
            dim: false,
            cache: false,
            clip: Some(Rect::new(pos.0, panel.y, max_w, panel.h)),
            family: bold.then_some(FONT_BOLD),
            color: Some(color),
        };
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

    #[test]
    fn a_folder_says_what_it_holds_and_a_walk_fills_its_size() {
        let dir = tmp("folder");
        let f = dir.join("Projects");
        std::fs::create_dir_all(f.join("sub")).unwrap();
        std::fs::write(f.join("a.txt"), "12345").unwrap();
        std::fs::write(f.join("sub/b.txt"), "123").unwrap();
        let item = Item {
            path: f.to_string_lossy().into_owned(),
            name: "Projects".into(),
            kind: Kind::Folder,
            icon: String::new(),
            exec: None,
        };
        let mut p = Props::open(&item, (10.0, 10.0), 2000.0, 2000.0, &dir);
        assert_eq!(p.title, "Projects");
        assert_eq!(p.rows[0], ("Kind", "Folder".to_owned()));
        assert_eq!(p.rows[1], ("Contains", "2 items (1 folder)".to_owned()));
        assert_eq!(p.rows[2], ("Size", CALCULATING.to_owned()));
        assert_eq!(p.rows[3], ("Where", "~".to_owned()));
        assert!(p.rows.iter().any(|(k, _)| *k == "Modified"));
        let (bytes, files) = folder_size(&f);
        assert_eq!((bytes, files), (8, 2));
        p.set_size(bytes, files);
        assert!(p.rows[2].1.ends_with("(2 files)"), "{}", p.rows[2].1);
        // Drawn: one grid, the panel and the rule, the title and two labels a row.
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
    fn a_file_says_its_kind_and_size_and_the_box_stays_on_the_surface() {
        let dir = tmp("file");
        let f = dir.join("Desktop");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("shot.PNG"), vec![0u8; 2048]).unwrap();
        let item = Item {
            path: f.join("shot.PNG").to_string_lossy().into_owned(),
            name: "shot.PNG".into(),
            kind: Kind::File,
            icon: String::new(),
            exec: None,
        };
        let p = Props::open(&item, (990.0, 790.0), 1000.0, 800.0, &dir);
        assert_eq!(p.rows[0], ("Kind", "Image (png)".to_owned()));
        assert_eq!(p.rows[1].0, "Size");
        assert_eq!(p.rows[2], ("Where", "~/Desktop".to_owned()));
        assert!(p.rect.x + p.rect.w <= 1000.0 && p.rect.y + p.rect.h <= 800.0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn small_words() {
        assert_eq!(contains_text(0, 0), "Nothing");
        assert_eq!(contains_text(1, 0), "1 item");
        assert_eq!(contains_text(5, 2), "5 items (2 folders)");
        assert_eq!(file_kind("README"), "File");
        assert_eq!(file_kind("a.tar.gz"), "Archive (gz)");
        assert_eq!(tilde(Path::new("/srv/x"), Path::new("/home/m")), "/srv/x");
        assert!(date_text(None).is_none());
        assert_eq!(date_text(Some(SystemTime::UNIX_EPOCH)).map(|d| d.len()), Some(16));
    }
}

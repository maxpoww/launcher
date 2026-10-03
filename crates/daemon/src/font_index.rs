//! The system font database, built once per process and remembered between
//! runs.
//!
//! `FontSystem::new()` scans every system font file — 231 files, 394 MB on a
//! Golem laptop — reading each one's tables to learn its names and style.
//! Every renderer (dock, OPTIONS bar, deck) did it again. On a spinning disk
//! the cold scan was the largest part of the shell's start: with the fonts
//! already cached the dock's surfaces were up 7–10 s sooner (Acer E5-573,
//! night dogfood 2026-10-03).
//!
//! Now the faces' metadata is kept in `$XDG_CACHE_HOME/waverunner/font-index.json`
//! (the `fontconfig` cache idea). A start with a valid index registers every
//! face WITHOUT opening its file; a file is mapped the first time a face is
//! actually used, so a start reads only the fonts it draws with. The index is
//! keyed by what decides the font set — the fontconfig file (a store path
//! that changes whenever the system's fonts do) and the user's font folders'
//! modification times — and rebuilt by a full scan when that changes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use glyphon::cosmic_text::fontdb::{
    Database, FaceInfo, Language, Source, Stretch, Style, Weight, ID,
};
use serde::{Deserialize, Serialize};

const FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Index {
    format: u32,
    key: String,
    faces: Vec<Face>,
}

#[derive(Serialize, Deserialize)]
struct Face {
    path: PathBuf,
    index: u32,
    /// The English (US) name first, as fontdb keeps them.
    families: Vec<String>,
    post_script_name: String,
    style: u8,
    weight: u16,
    stretch: u16,
    monospaced: bool,
}

/// The process-wide font database (a clone is cheap: faces are shared).
pub(crate) fn database() -> Database {
    static DB: OnceLock<Database> = OnceLock::new();
    DB.get_or_init(build).clone()
}

/// The locale cosmic-text would pick (`LC_ALL` > `LC_MESSAGES` > `LANG`, as
/// a BCP 47 tag), for `FontSystem::new_with_locale_and_db`.
pub(crate) fn locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .map(|v| {
            v.split(['.', '@'])
                .next()
                .unwrap_or("en_US")
                .replace('_', "-")
        })
        .unwrap_or_else(|| "en-US".to_string())
}

fn build() -> Database {
    let started = std::time::Instant::now();
    let path = crate::apps::cache_base().join("waverunner/font-index.json");
    let key = key();
    if let Some(db) = load(&path, &key) {
        tracing::info!(
            "fonts: {} faces from the index in {:?}",
            db.len(),
            started.elapsed()
        );
        return db;
    }
    let mut db = Database::new();
    db.load_system_fonts();
    defaults(&mut db);
    tracing::info!(
        "fonts: scanned {} faces in {:?}; index saved",
        db.len(),
        started.elapsed()
    );
    save(&path, &key, &db);
    db
}

/// cosmic-text's own generic-family defaults (`FontSystem::new_with_fonts`).
fn defaults(db: &mut Database) {
    db.set_monospace_family("Fira Mono");
    db.set_sans_serif_family("Fira Sans");
    db.set_serif_family("DejaVu Serif");
}

/// What decides the font set: the fontconfig file fontdb reads (resolved —
/// on NixOS a store path that changes with the system's fonts) and the
/// newest modification time under each user font folder.
fn key() -> String {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".config"));
    let data_home = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".local/share"));
    let mut key = format!("v{}", env!("CARGO_PKG_VERSION"));
    let mut files: Vec<PathBuf> = std::env::var("FONTCONFIG_FILE")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    files.push(config_home.join("fontconfig/fonts.conf"));
    files.push(PathBuf::from("/etc/fonts/fonts.conf"));
    for f in files {
        let resolved = std::fs::canonicalize(&f)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "-".into());
        key.push_str(&format!("|{}={}@{}", f.display(), resolved, mtime(&f)));
    }
    for dir in [
        data_home.join("fonts"),
        home.join(".fonts"),
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
    ] {
        key.push_str(&format!("|{}@{}", dir.display(), newest(&dir, 4)));
    }
    key
}

fn mtime(p: &Path) -> u128 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

/// The newest modification time of `dir` and the folders under it (a font
/// added to a subfolder changes only that subfolder).
fn newest(dir: &Path, depth: u32) -> u128 {
    let mut best = mtime(dir);
    if depth == 0 {
        return best;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                best = best.max(newest(&e.path(), depth - 1));
            }
        }
    }
    best
}

fn load(path: &Path, key: &str) -> Option<Database> {
    let index: Index = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if index.format != FORMAT || index.key != key || index.faces.is_empty() {
        return None;
    }
    let mut db = Database::new();
    for f in index.faces {
        let data: Arc<dyn AsRef<[u8]> + Sync + Send> = Arc::new(LazyFile::new(f.path.clone()));
        let mut families = f.families.into_iter();
        let first = families.next()?;
        let families = std::iter::once((first, Language::English_UnitedStates))
            .chain(families.map(|n| (n, Language::Unknown)))
            .collect();
        db.push_face_info(FaceInfo {
            id: ID::dummy(),
            source: Source::SharedFile(f.path, data),
            index: f.index,
            families,
            post_script_name: f.post_script_name,
            style: match f.style {
                1 => Style::Italic,
                2 => Style::Oblique,
                _ => Style::Normal,
            },
            weight: Weight(f.weight),
            stretch: stretch_from(f.stretch),
            monospaced: f.monospaced,
        });
    }
    defaults(&mut db);
    Some(db)
}

fn save(path: &Path, key: &str, db: &Database) {
    let mut faces = Vec::new();
    for info in db.faces() {
        let file = match &info.source {
            Source::File(p) | Source::SharedFile(p, _) => p.clone(),
            // A font held only in memory cannot be found again from an
            // index: keep scanning instead.
            Source::Binary(_) => return,
        };
        faces.push(Face {
            path: file,
            index: info.index,
            families: info.families.iter().map(|(n, _)| n.clone()).collect(),
            post_script_name: info.post_script_name.clone(),
            style: match info.style {
                Style::Normal => 0,
                Style::Italic => 1,
                Style::Oblique => 2,
            },
            weight: info.weight.0,
            stretch: stretch_to(info.stretch),
            monospaced: info.monospaced,
        });
    }
    let index = Index {
        format: FORMAT,
        key: key.to_string(),
        faces,
    };
    let Ok(text) = serde_json::to_vec(&index) else {
        return;
    };
    let Some(dir) = path.parent() else { return };
    let tmp = dir.join("font-index.json.tmp");
    let written = std::fs::create_dir_all(dir)
        .and_then(|_| std::fs::write(&tmp, text))
        .and_then(|_| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        tracing::warn!("fonts: could not save the index: {e}");
    }
}

fn stretch_to(s: Stretch) -> u16 {
    match s {
        Stretch::UltraCondensed => 1,
        Stretch::ExtraCondensed => 2,
        Stretch::Condensed => 3,
        Stretch::SemiCondensed => 4,
        Stretch::Normal => 5,
        Stretch::SemiExpanded => 6,
        Stretch::Expanded => 7,
        Stretch::ExtraExpanded => 8,
        Stretch::UltraExpanded => 9,
    }
}

fn stretch_from(n: u16) -> Stretch {
    match n {
        1 => Stretch::UltraCondensed,
        2 => Stretch::ExtraCondensed,
        3 => Stretch::Condensed,
        4 => Stretch::SemiCondensed,
        6 => Stretch::SemiExpanded,
        7 => Stretch::Expanded,
        8 => Stretch::ExtraExpanded,
        9 => Stretch::UltraExpanded,
        _ => Stretch::Normal,
    }
}

/// A font file mapped on first use. A face whose file is gone reads as
/// empty, which cosmic-text treats as a face it cannot parse (skipped).
struct LazyFile {
    path: PathBuf,
    map: OnceLock<Option<memmap2::Mmap>>,
}

impl LazyFile {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            map: OnceLock::new(),
        }
    }
}

impl AsRef<[u8]> for LazyFile {
    fn as_ref(&self) -> &[u8] {
        let map = self.map.get_or_init(|| {
            let file = std::fs::File::open(&self.path).ok()?;
            // SAFETY: store paths and font folders are not rewritten in
            // place; a font replaced on disk keeps the old mapping (as
            // fontdb's own memmap loading does).
            unsafe { memmap2::Mmap::map(&file) }.ok()
        });
        map.as_deref().unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_gives_back_the_scanned_faces() {
        let mut scanned = Database::new();
        scanned.load_system_fonts();
        if scanned.is_empty() {
            return; // no fonts in this build environment
        }
        let dir = std::env::temp_dir().join(format!("font-index-test-{}", std::process::id()));
        let path = dir.join("font-index.json");
        save(&path, "k", &scanned);
        assert!(
            load(&path, "other key").is_none(),
            "a different key must miss"
        );
        let loaded = load(&path, "k").expect("the index loads");
        let sig = |db: &Database| {
            let mut v: Vec<_> = db
                .faces()
                .map(|f| {
                    (
                        f.post_script_name.clone(),
                        f.families[0].0.clone(),
                        f.index,
                        f.weight.0,
                        f.monospaced,
                        stretch_to(f.stretch),
                    )
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(sig(&scanned), sig(&loaded));
        // A face from the index reads its file on first use.
        let id = loaded.faces().next().expect("a face").id;
        assert!(loaded
            .with_face_data(id, |data, _| !data.is_empty())
            .unwrap_or(false));
        let _ = std::fs::remove_dir_all(dir);
    }
}

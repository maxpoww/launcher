//! Best-effort JSON persistence shared by every on-disk store (usage,
//! pins, grid order, groups, managed packages).
//!
//! One implementation so every store behaves the same way: reads treat
//! a missing or unparsable file as empty (never fatal), and writes are
//! atomic (temp file + rename, parent dirs created) so a crash mid-write
//! can't truncate a store. Failures are logged under the store's tag and
//! swallowed — the in-memory state is always correct for the running
//! session.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::warn;

/// Path of a file in the daemon's XDG data directory
/// (`$XDG_DATA_HOME/waverunner/`, falling back to `~/.local/share/...`).
/// Every on-disk store keys its file off this.
pub fn data_path(file_name: &str) -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")
        });
    base.join("waverunner").join(file_name)
}

/// Parse `path` as JSON. Missing → `None`, quietly (first run).
/// Malformed → the original is PRESERVED beside the store as
/// `<name>.corrupt-<epoch>` and `None` is returned. Never silently
/// discard: with the old behavior the next write destroyed the user's
/// data under a fresh default (the dev box lost its whole grid order to
/// exactly this, 2026-09-03 — `apps-order.json.bak-…-shredded`). A
/// schema change that stops parsing an old store lands here too, which
/// is the right call: preserve, start clean, leave the bytes for rescue.
/// The rescue is loud so the DockMenu aging check can collect it.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let rescue = sibling(path, &format!(".corrupt-{secs}"));
            match std::fs::rename(path, &rescue) {
                Ok(()) => warn!(
                    "{path:?} is corrupt ({e}); preserved as {rescue:?}, starting empty"
                ),
                Err(re) => warn!(
                    "{path:?} is corrupt ({e}); rescue rename failed too ({re}); starting empty"
                ),
            }
            None
        }
    }
}

/// `path` with `suffix` APPENDED to its file name (`foo.json` →
/// `foo.json<suffix>`) — unlike `with_extension`, which would substitute
/// the extension and let two stores differing only by extension collide.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Serialize `value` (pretty, for hand-inspection) and write it
/// atomically to `path`.
pub fn write_json(tag: &str, path: &Path, value: &impl Serialize) {
    match serde_json::to_string_pretty(value) {
        Ok(json) => write_text(tag, path, &json),
        Err(e) => warn!("{tag}: serialize failed: {e}"),
    }
}

/// Best-effort atomic write (temp + rename), creating parent dirs.
pub fn write_text(tag: &str, path: &Path, contents: &str) {
    write_bytes(tag, path, contents.as_bytes());
}

/// Best-effort atomic binary write (temp + rename), creating parent dirs — for
/// non-text stores like the notification image cache.
pub fn write_bytes(tag: &str, path: &Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            warn!("{tag}: cannot create {dir:?}: {e}");
            return;
        }
    }
    // Appended, not substituted (`foo.json` → `foo.json.tmp`): with
    // `with_extension`, stores differing only by extension would share
    // one temp name.
    let tmp = sibling(path, ".tmp");
    let write = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = write {
        warn!("{tag}: cannot write {path:?}: {e}");
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_garbage() {
        let dir = std::env::temp_dir().join("waverunner-persist-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("store.json");
        write_json("test", &path, &vec!["a".to_string(), "b".to_string()]);
        let back: Option<Vec<String>> = read_json(&path);
        assert_eq!(back, Some(vec!["a".to_string(), "b".to_string()]));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_json::<Vec<String>>(&path), None);
        // The corrupt original is PRESERVED beside the store, and the
        // store path itself is now free (a rewrite starts clean).
        assert!(!path.exists(), "corrupt store should be renamed away");
        let rescued: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("store.json.corrupt-")
            })
            .collect();
        assert_eq!(rescued.len(), 1, "exactly one rescue file");
        assert_eq!(
            std::fs::read_to_string(rescued[0].path()).unwrap(),
            "not json",
            "rescue preserves the original bytes"
        );
        // A missing store stays quiet (no rescue spawned).
        assert_eq!(read_json::<Vec<String>>(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tmp_name_is_appended_not_substituted() {
        assert_eq!(
            sibling(Path::new("/a/b/foo.json"), ".tmp"),
            PathBuf::from("/a/b/foo.json.tmp")
        );
        assert_eq!(
            sibling(Path::new("/a/b/foo.rgba"), ".tmp"),
            PathBuf::from("/a/b/foo.rgba.tmp")
        );
    }
}

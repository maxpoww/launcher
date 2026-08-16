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

/// Parse `path` as JSON, or `None` if it's missing or malformed.
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
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
    let tmp = path.with_extension("tmp");
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
        let path = dir.join("store.json");
        write_json("test", &path, &vec!["a".to_string(), "b".to_string()]);
        let back: Option<Vec<String>> = read_json(&path);
        assert_eq!(back, Some(vec!["a".to_string(), "b".to_string()]));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_json::<Vec<String>>(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

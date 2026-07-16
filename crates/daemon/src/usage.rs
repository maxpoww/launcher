//! Persistent app-launch frequency tracking.
//!
//! Counts are stored in `$XDG_DATA_HOME/waverunner/usage.json`
//! (falling back to `~/.local/share/waverunner/usage.json`) as a
//! flat JSON object: `{ "app-id": count, ... }`.
//!
//! Writes are synchronous and best-effort: a failed write is logged but
//! never fatal — the in-memory counts are always correct for the
//! running session.

use std::collections::HashMap;
use std::path::PathBuf;

/// In-memory launch-frequency database, backed by a JSON file.
pub struct UsageDb {
    counts: HashMap<String, u32>,
    path: PathBuf,
}

impl UsageDb {
    /// Load from disk, or start empty if the file doesn't exist yet.
    pub fn load() -> Self {
        let path = data_path("usage.json");
        let counts = crate::persist::read_json(&path).unwrap_or_default();
        Self { counts, path }
    }

    /// Increment the launch count for `app_id` and persist.
    pub fn increment(&mut self, app_id: &str) {
        *self.counts.entry(app_id.to_owned()).or_insert(0) += 1;
        crate::persist::write_json("usage", &self.path, &self.counts);
    }

    /// Return the number of times `app_id` has been launched (0 if unknown).
    pub fn count(&self, app_id: &str) -> u32 {
        self.counts.get(app_id).copied().unwrap_or(0)
    }
}

/// Path of a file in the daemon's XDG data directory
/// (`$XDG_DATA_HOME/waverunner/`, falling back to `~/.local/share/...`).
/// Shared with the pin database.
pub fn data_path(file_name: &str) -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")
        });
    base.join("waverunner").join(file_name)
}

//! Persistent dock pins.
//!
//! The dock shows exactly the pinned apps (in order); everything else
//! lives in the Apps grid. Pinning is drag-to-dock; unpinning is
//! drag-off-dock (the app returns to the grid).
//!
//! Stored in `$XDG_DATA_HOME/waverunner/pins.json` as `{ "pins": [...] }`
//! (a bare `[...]` array, and an older `{ "pins", "excluded" }` object,
//! are both accepted — any `excluded` list is ignored now).
//!
//! Writes are best-effort; a failed write is logged but never fatal.

use std::path::PathBuf;

use tracing::{info, warn};

/// Ordered list of explicitly pinned app IDs.
pub struct PinDb {
    pins: Vec<String>,
    path: PathBuf,
}

impl PinDb {
    /// Load from disk, or start empty if the file doesn't exist yet.
    pub fn load() -> Self {
        let path = crate::usage::data_path("pins.json");
        let pins = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| parse_file(&s))
            .unwrap_or_default();
        Self { pins, path }
    }

    /// Ordered slice of pinned app IDs.
    pub fn pins(&self) -> &[String] {
        &self.pins
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pins.iter().any(|p| p == id)
    }

    /// Insert `app_id` before dock slot `slot`, moving it if already pinned.
    pub fn pin_at(&mut self, app_id: &str, slot: usize) {
        self.pins.retain(|p| p != app_id);
        let slot = slot.min(self.pins.len());
        self.pins.insert(slot, app_id.to_owned());
        info!("pin: {} at slot {} → pins={:?}", app_id, slot, self.pins);
        self.save();
    }

    /// Remove `app_id` from the pins (drag-off-dock, or a dead pin whose
    /// app was uninstalled) — it returns to the Apps grid.
    pub fn unpin(&mut self, app_id: &str) {
        let before = self.pins.len();
        self.pins.retain(|p| p != app_id);
        if self.pins.len() != before {
            info!("unpin: {} → pins={:?}", app_id, self.pins);
            self.save();
        }
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!("pins: cannot create data dir: {e}");
                return;
            }
        }
        let json = serde_json::json!({ "pins": self.pins });
        if let Err(e) = std::fs::write(&self.path, json.to_string()) {
            warn!("pins: write failed: {e}");
        }
    }
}

/// Parse `{ "pins": [...] }` or a legacy bare `[...]` array (an older
/// `excluded` field, if present, is ignored).
fn parse_file(s: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    match &v {
        serde_json::Value::Array(arr) => Some(strings_from_array(arr)),
        serde_json::Value::Object(map) => Some(
            map.get("pins")
                .and_then(|v| v.as_array())
                .map(|a| strings_from_array(a))
                .unwrap_or_default(),
        ),
        _ => None,
    }
}

fn strings_from_array(arr: &[serde_json::Value]) -> Vec<String> {
    arr.iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

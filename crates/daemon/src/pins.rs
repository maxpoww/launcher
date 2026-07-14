//! Persistent dock pin + exclusion list.
//!
//! Stored in `$XDG_DATA_HOME/waverunner/pins.json` as:
//!   `{ "pins": [...], "excluded": [...] }`
//! (old plain-array format is accepted for backward compat — treated as pins only).
//!
//! Writes are best-effort; a failed write is logged but never fatal.

use std::path::PathBuf;

use tracing::{info, warn};

/// Ordered list of explicitly pinned app IDs plus the exclusion list.
pub struct PinDb {
    pins: Vec<String>,
    excluded: Vec<String>,
    path: PathBuf,
}

impl PinDb {
    /// Load from disk, or start empty if the file doesn't exist yet.
    pub fn load() -> Self {
        let path = crate::usage::data_path("pins.json");
        let (pins, excluded) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| parse_file(&s))
            .unwrap_or_default();
        Self {
            pins,
            excluded,
            path,
        }
    }

    /// Ordered slice of pinned app IDs.
    pub fn pins(&self) -> &[String] {
        &self.pins
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pins.iter().any(|p| p == id)
    }

    pub fn is_excluded(&self, id: &str) -> bool {
        self.excluded.iter().any(|e| e == id)
    }

    /// Insert `app_id` before dock slot `slot`, moving it if already pinned.
    /// Also removes it from the exclusion list (user explicitly re-added it).
    pub fn pin_at(&mut self, app_id: &str, slot: usize) {
        self.excluded.retain(|e| e != app_id);
        self.pins.retain(|p| p != app_id);
        let slot = slot.min(self.pins.len());
        self.pins.insert(slot, app_id.to_owned());
        info!("pin: {} at slot {} → pins={:?}", app_id, slot, self.pins);
        self.save();
    }

    /// Remove `app_id` from the pins *without* excluding it — used when
    /// the app a pin points at is uninstalled, so a dead pin can't linger
    /// and later latch onto a same-named package search result.
    pub fn unpin(&mut self, app_id: &str) {
        let before = self.pins.len();
        self.pins.retain(|p| p != app_id);
        if self.pins.len() != before {
            info!("unpin: {} → pins={:?}", app_id, self.pins);
            self.save();
        }
    }

    /// Remove from dock entirely: unpins and excludes from usage-sort fill.
    /// Drag-from-dock drops here. Pinning (drag-to-dock) reverses it.
    pub fn exclude(&mut self, app_id: &str) {
        self.pins.retain(|p| p != app_id);
        if !self.is_excluded(app_id) {
            self.excluded.push(app_id.to_owned());
        }
        info!("exclude: {} → excluded={:?}", app_id, self.excluded);
        self.save();
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                warn!("pins: cannot create data dir: {e}");
                return;
            }
        }
        let json = serde_json::json!({
            "pins": self.pins,
            "excluded": self.excluded,
        });
        if let Err(e) = std::fs::write(&self.path, json.to_string()) {
            warn!("pins: write failed: {e}");
        }
    }
}

/// Parse `{ "pins": [...], "excluded": [...] }` or `[...]` (legacy).
fn parse_file(s: &str) -> Option<(Vec<String>, Vec<String>)> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    match &v {
        serde_json::Value::Array(arr) => {
            let pins = strings_from_array(arr);
            Some((pins, vec![]))
        }
        serde_json::Value::Object(map) => {
            let pins = map
                .get("pins")
                .and_then(|v| v.as_array())
                .map(|a| strings_from_array(a))
                .unwrap_or_default();
            let excluded = map
                .get("excluded")
                .and_then(|v| v.as_array())
                .map(|a| strings_from_array(a))
                .unwrap_or_default();
            Some((pins, excluded))
        }
        _ => None,
    }
}

fn strings_from_array(arr: &[serde_json::Value]) -> Vec<String> {
    arr.iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

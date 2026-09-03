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

use tracing::info;

/// Ordered list of explicitly pinned app IDs.
pub struct PinDb {
    pins: Vec<String>,
    path: PathBuf,
}

impl PinDb {
    /// Load from disk, or start empty if the file doesn't exist yet.
    pub fn load() -> Self {
        let path = crate::persist::data_path("pins.json");
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
        crate::persist::write_json(
            "pins",
            &self.path,
            &serde_json::json!({ "pins": self.pins }),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db(pins: &[&str]) -> PinDb {
        PinDb {
            pins: pins.iter().map(|s| s.to_string()).collect(),
            path: std::env::temp_dir().join("waverunner-test-pins.json"),
        }
    }

    #[test]
    fn parses_current_and_legacy_shapes() {
        // Current object form.
        assert_eq!(
            parse_file(r#"{ "pins": ["a", "b"] }"#),
            Some(vec!["a".into(), "b".into()])
        );
        // Legacy bare array.
        assert_eq!(parse_file(r#"["a"]"#), Some(vec!["a".into()]));
        // Older object with an excluded list: pins kept, excluded ignored.
        assert_eq!(
            parse_file(r#"{ "pins": ["a"], "excluded": ["x"] }"#),
            Some(vec!["a".into()])
        );
        // An object without pins is an empty dock, not an error.
        assert_eq!(parse_file(r#"{ "excluded": ["x"] }"#), Some(vec![]));
    }

    #[test]
    fn corrupted_files_never_panic_and_never_invent_pins() {
        assert_eq!(parse_file("not json"), None);
        assert_eq!(parse_file(r#""just a string""#), None);
        assert_eq!(parse_file("42"), None);
        // Non-string entries are dropped, the rest survive.
        assert_eq!(
            parse_file(r#"["a", 7, null, "b"]"#),
            Some(vec!["a".into(), "b".into()])
        );
    }

    #[test]
    fn pin_at_moves_and_clamps() {
        let mut d = db(&["a", "b", "c"]);
        // Re-pinning an existing app MOVES it (no duplicate).
        d.pin_at("a", 2);
        assert_eq!(d.pins(), ["b", "c", "a"]);
        // A slot past the end clamps to the end instead of panicking.
        d.pin_at("new", 99);
        assert_eq!(d.pins(), ["b", "c", "a", "new"]);
    }

    #[test]
    fn unpin_removes_and_missing_is_a_noop() {
        let mut d = db(&["a", "b"]);
        d.unpin("a");
        assert_eq!(d.pins(), ["b"]);
        d.unpin("ghost");
        assert_eq!(d.pins(), ["b"]);
        assert!(!d.is_pinned("a"));
        assert!(d.is_pinned("b"));
    }
}

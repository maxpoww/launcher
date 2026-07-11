//! Persistent Apps-grid order: install date by default, manual on top.
//!
//! Nix store mtimes are normalized to the epoch, so "install date" is
//! tracked as first-seen order: every scan appends ids it hasn't seen
//! before to the end of the list (the macOS Launchpad rule — new apps
//! land last, nothing else moves). Dragging an app to a new position
//! rewrites the list, which is the manual override. Vanished apps keep
//! their slot so a reinstall comes back where it was.
//!
//! Stored in `$XDG_DATA_HOME/waverunner/apps-order.json` as
//! `{ "order": ["id", ...] }`. Writes are best-effort.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Default, Serialize, Deserialize)]
struct FileFormat {
    order: Vec<String>,
}

/// The grid-order list, persisted like the pin db.
pub struct OrderDb {
    order: Vec<String>,
    path: PathBuf,
}

impl OrderDb {
    /// Load from disk, or start empty if the file doesn't exist yet.
    pub fn load() -> Self {
        let path = crate::usage::data_path("apps-order.json");
        let order = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<FileFormat>(&s).ok())
            .map(|f| f.order)
            .unwrap_or_default();
        Self { order, path }
    }

    /// Record newly seen ids at the end of the order (in the order the
    /// iterator yields them — the caller's scan order seeds the very
    /// first baseline). Saves only when something was new.
    pub fn sync<'a>(&mut self, ids: impl Iterator<Item = &'a str>) {
        let mut added = false;
        for id in ids {
            if !self.order.iter().any(|o| o == id) {
                self.order.push(id.to_owned());
                added = true;
            }
        }
        if added {
            self.save();
        }
    }

    /// Sort position of `id`; unknown ids sort last (stable).
    pub fn index_of(&self, id: &str) -> usize {
        self.order
            .iter()
            .position(|o| o == id)
            .unwrap_or(self.order.len())
    }

    /// Move `id` so it sits immediately before the id currently at
    /// grid position `before` *within the given visible sequence* —
    /// the drag-to-reorder mutation. `before == visible.len()` moves
    /// it to the end.
    pub fn move_within(&mut self, id: &str, visible: &[&str], before: usize) {
        if visible.get(before).copied() == Some(id) {
            return; // dropped back onto its own slot: no-op
        }
        // Resolve the anchor in the persistent list before removal.
        let anchor = visible.get(before).map(|a| (*a).to_owned());
        self.order.retain(|o| o != id);
        let at = match &anchor {
            Some(anchor) => self
                .order
                .iter()
                .position(|o| o == anchor)
                .unwrap_or(self.order.len()),
            None => self.order.len(),
        };
        self.order.insert(at, id.to_owned());
        info!("reorder: {id} -> slot {before} (anchor {anchor:?})");
        self.save();
    }

    /// Place `id` immediately before `anchor` (a newly created box
    /// takes its target app's grid position). Unknown anchors append.
    pub fn insert_before(&mut self, id: &str, anchor: &str) {
        self.order.retain(|o| o != id);
        let at = self
            .order
            .iter()
            .position(|o| o == anchor)
            .unwrap_or(self.order.len());
        self.order.insert(at, id.to_owned());
        self.save();
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                warn!("order: cannot create {dir:?}: {e}");
                return;
            }
        }
        let json = serde_json::json!(FileFormat {
            order: self.order.clone()
        });
        let tmp = self.path.with_extension("json.tmp");
        let write =
            std::fs::write(&tmp, json.to_string()).and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = write {
            warn!("order: cannot write {:?}: {e}", self.path);
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> OrderDb {
        OrderDb {
            order: Vec::new(),
            path: std::env::temp_dir().join("waverunner-order-test.json"),
        }
    }

    #[test]
    fn sync_appends_new_ids_only() {
        let mut d = db();
        d.sync(["a", "b"].into_iter());
        d.sync(["b", "c", "a"].into_iter());
        assert_eq!(d.order, vec!["a", "b", "c"]);
        assert!(d.index_of("a") < d.index_of("c"));
        assert_eq!(d.index_of("zz"), 3); // unknown sorts last
    }

    #[test]
    fn move_within_reorders_against_the_visible_slice() {
        let mut d = db();
        d.sync(["a", "b", "c", "d"].into_iter());
        // Grid shows b, c, d (a is pinned away); drag d before b.
        d.move_within("d", &["b", "c", "d"], 0);
        assert_eq!(d.order, vec!["a", "d", "b", "c"]);
        // Drag a to the end of the visible slice.
        d.move_within("a", &["d", "b", "c"], 3);
        assert_eq!(d.order, vec!["d", "b", "c", "a"]);
    }
}

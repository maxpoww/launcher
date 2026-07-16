//! Persistent Apps-grid order: install date by default, manual on top —
//! a thin persistence wrapper around the shared [`PagedList`] page model
//! (see `pages.rs`; a box's members use the very same model).
//!
//! Nix store mtimes are normalized to the epoch, so "install date" is
//! tracked as first-seen order: every scan appends ids it hasn't seen
//! before to the end of the last page (the Launchpad rule — new apps
//! land last, nothing else moves). Vanished apps keep their slot so a
//! reinstall comes back where it was.
//!
//! Stored in `$XDG_DATA_HOME/waverunner/apps-order.json` as
//! `{ "pages": [["id", ...], ...] }`; the pre-pages flat
//! `{ "order": ["id", ...] }` format loads as a single page (a later
//! `normalize` splits it by the real capacity). Writes are best-effort.

use crate::pages::PagedList;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::info;

#[derive(Default, Serialize, Deserialize)]
struct FileFormat {
    #[serde(default)]
    pages: PagedList,
    /// Legacy flat order (pre-pages); read-only migration input.
    #[serde(default, skip_serializing)]
    order: Vec<String>,
}

/// The paged grid-order list, persisted like the pin db.
pub struct OrderDb {
    list: PagedList,
    path: PathBuf,
}

impl OrderDb {
    /// Load from disk, or start empty if the file doesn't exist yet.
    /// A legacy flat `order` list becomes one big page (normalize
    /// splits it once the real page capacity is known).
    pub fn load() -> Self {
        let path = crate::usage::data_path("apps-order.json");
        let f: FileFormat = crate::persist::read_json(&path).unwrap_or_default();
        let list = if !f.pages.is_empty() {
            f.pages
        } else {
            PagedList::from_flat(f.order)
        };
        Self { list, path }
    }

    /// The pages, in grid order. Pages may be under-full; ids of hidden
    /// items (pinned / grouped) still occupy their stored position.
    pub fn pages(&self) -> &[Vec<String>] {
        self.list.pages()
    }

    /// Record newly seen ids at the end of the last page (in the order
    /// the iterator yields them — the caller's scan order seeds the very
    /// first baseline). Saves only when something was new.
    pub fn sync<'a>(&mut self, ids: impl Iterator<Item = &'a str>) {
        let mut added = false;
        for id in ids {
            if !self.list.contains(id) {
                self.list.push(id);
                added = true;
            }
        }
        if added {
            self.save();
        }
    }

    /// Cascade over-full pages by the display capacity, counting only
    /// visible ids; fold hidden-only pages into a neighbor (see
    /// [`PagedList::normalize`]).
    pub fn normalize(&mut self, cap: usize, visible: impl Fn(&str) -> bool) {
        if self.list.normalize(cap, visible) {
            self.save();
        }
    }

    /// Forget the grid slot of any box no longer alive: a `group:` id not
    /// in `live` is dropped (a deleted box keeps nothing). App ids are
    /// always kept — a vanished app returns to its slot on reinstall.
    pub fn forget_dead_boxes(&mut self, live: &std::collections::HashSet<String>) {
        let before = self.list.len();
        self.list
            .retain(|id| !id.starts_with("group:") || live.contains(id));
        if self.list.len() != before {
            self.save();
        }
    }

    /// Place `id` immediately before `anchor`, inside the anchor's page
    /// (a newly created box takes its target app's grid position).
    /// Unknown anchors append to the last page.
    pub fn insert_before(&mut self, id: &str, anchor: &str) {
        self.move_before(id, anchor);
    }

    /// `insert_before` with move semantics (`id` leaves its old spot).
    pub fn move_before(&mut self, id: &str, anchor: &str) {
        if anchor == id {
            return;
        }
        info!("reorder: {id} -> before {anchor}");
        self.list.move_before(id, anchor);
        self.save();
    }

    /// Move `id` to the end of `page`, creating it as needed — the
    /// drop-on-a-fresh-page mutation.
    pub fn move_to_page_end(&mut self, id: &str, page: usize) {
        info!("reorder: {id} -> end of page {page}");
        self.list.move_to_page_end(id, page);
        self.save();
    }

    fn save(&self) {
        crate::persist::write_json(
            "order",
            &self.path,
            &FileFormat {
                pages: self.list.clone(),
                order: Vec::new(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> OrderDb {
        OrderDb {
            list: PagedList::default(),
            path: std::env::temp_dir().join("waverunner-order-test.json"),
        }
    }

    #[test]
    fn sync_appends_new_ids_only() {
        let mut d = db();
        d.sync(["a", "b"].into_iter());
        d.sync(["b", "c", "a"].into_iter());
        assert_eq!(d.pages(), &[vec!["a", "b", "c"]]);
    }

    #[test]
    fn move_before_reorders_within_a_page() {
        let mut d = db();
        d.sync(["a", "b", "c", "d"].into_iter());
        d.move_before("d", "b");
        assert_eq!(d.pages(), &[vec!["a", "d", "b", "c"]]);
        // An unknown anchor appends to the last page.
        d.move_before("a", "zz");
        assert_eq!(d.pages(), &[vec!["d", "b", "c", "a"]]);
    }

    #[test]
    fn legacy_flat_order_loads_as_one_page() {
        let f: FileFormat = serde_json::from_str(r#"{"order":["a","b"]}"#).unwrap();
        assert!(f.pages.is_empty());
        let list = PagedList::from_flat(f.order);
        assert_eq!(list.pages(), &[vec!["a", "b"]]);
    }

    #[test]
    fn forget_dead_boxes_drops_only_dead_group_ids() {
        let mut d = db();
        d.sync(["a", "group:box-1", "b", "group:box-2"].into_iter());
        let live: std::collections::HashSet<String> =
            ["group:box-2".to_string()].into_iter().collect();
        d.forget_dead_boxes(&live);
        assert_eq!(d.pages(), &[vec!["a", "b", "group:box-2"]]);
    }
}

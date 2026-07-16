//! Persistent Apps-grid order: install date by default, manual on top —
//! organized as *pages* (macOS Launchpad model).
//!
//! The grid is a sequence of pages; each page holds ids in order and may
//! be *under-full* (empty space at its tail). That's what lets a drag
//! park an app alone on a fresh page: pages don't reflow into each other
//! on their own. Only two things move ids across pages: an explicit drag
//! (`move_before` / `move_to_page_end`) and `normalize`, which cascades
//! over-full pages forward after the page capacity shrinks (or after an
//! insert overfills a page).
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

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Default, Serialize, Deserialize)]
struct FileFormat {
    #[serde(default)]
    pages: Vec<Vec<String>>,
    /// Legacy flat order (pre-pages); read-only migration input.
    #[serde(default, skip_serializing)]
    order: Vec<String>,
}

/// The paged grid-order list, persisted like the pin db.
pub struct OrderDb {
    pages: Vec<Vec<String>>,
    path: PathBuf,
}

impl OrderDb {
    /// Load from disk, or start empty if the file doesn't exist yet.
    /// A legacy flat `order` list becomes one big page (normalize
    /// splits it once the real page capacity is known).
    pub fn load() -> Self {
        let path = crate::usage::data_path("apps-order.json");
        let f = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<FileFormat>(&s).ok())
            .unwrap_or_default();
        let pages = if !f.pages.is_empty() {
            f.pages
        } else if !f.order.is_empty() {
            vec![f.order]
        } else {
            Vec::new()
        };
        Self { pages, path }
    }

    /// The pages, in grid order. Pages may be under-full; ids of hidden
    /// items (pinned / grouped) still occupy their stored position.
    pub fn pages(&self) -> &[Vec<String>] {
        &self.pages
    }

    /// Record newly seen ids at the end of the last page (in the order
    /// the iterator yields them — the caller's scan order seeds the very
    /// first baseline). Saves only when something was new.
    pub fn sync<'a>(&mut self, ids: impl Iterator<Item = &'a str>) {
        let mut added = false;
        for id in ids {
            if !self.contains(id) {
                if self.pages.is_empty() {
                    self.pages.push(Vec::new());
                }
                self.pages
                    .last_mut()
                    .expect("just ensured")
                    .push(id.to_owned());
                added = true;
            }
        }
        if added {
            self.save();
        }
    }

    /// Cascade over-full pages forward so every page shows at most
    /// `cap` items: overflow moves to the head of the next page (keeping
    /// global order), creating pages as needed. Only *visible* ids count
    /// toward the capacity — hidden ones (pinned / grouped) occupy no
    /// display cell and ride along with their page. Under-full pages are
    /// left alone — their tail gaps are the point. Empty pages drop.
    pub fn normalize(&mut self, cap: usize, visible: impl Fn(&str) -> bool) {
        let cap = cap.max(1);
        let mut changed = false;
        let mut i = 0;
        while i < self.pages.len() {
            // Split where the (cap+1)-th visible id sits; trailing hidden
            // ids between the cap-th and that point stay put.
            let mut seen = 0usize;
            let split_at = self.pages[i].iter().position(|id| {
                if visible(id) {
                    seen += 1;
                    seen > cap
                } else {
                    false
                }
            });
            if let Some(at) = split_at {
                let overflow: Vec<String> = self.pages[i].split_off(at);
                if i + 1 == self.pages.len() {
                    self.pages.push(Vec::new());
                }
                // Prepend, preserving the overflow's order.
                for (k, id) in overflow.into_iter().enumerate() {
                    self.pages[i + 1].insert(k, id);
                }
                changed = true;
            }
            i += 1;
        }
        changed |= self.drop_empty_pages();
        if changed {
            self.save();
        }
    }

    fn contains(&self, id: &str) -> bool {
        self.pages.iter().any(|p| p.iter().any(|o| o == id))
    }

    /// Storage position of `id`: (page, index within page).
    fn slot_of(&self, id: &str) -> Option<(usize, usize)> {
        self.pages
            .iter()
            .enumerate()
            .find_map(|(p, page)| page.iter().position(|o| o == id).map(|i| (p, i)))
    }

    fn remove(&mut self, id: &str) {
        for page in &mut self.pages {
            page.retain(|o| o != id);
        }
    }

    fn drop_empty_pages(&mut self) -> bool {
        let before = self.pages.len();
        self.pages.retain(|p| !p.is_empty());
        self.pages.len() != before
    }

    /// Forget the grid slot of any box no longer alive: a `group:` id not
    /// in `live` is dropped (a deleted box keeps nothing). App ids are
    /// always kept — a vanished app returns to its slot on reinstall.
    pub fn forget_dead_boxes(&mut self, live: &std::collections::HashSet<String>) {
        let before: usize = self.pages.iter().map(Vec::len).sum();
        for page in &mut self.pages {
            page.retain(|id| !id.starts_with("group:") || live.contains(id));
        }
        let changed = self.pages.iter().map(Vec::len).sum::<usize>() != before;
        if self.drop_empty_pages() || changed {
            self.save();
        }
    }

    /// Place `id` immediately before `anchor`, inside the anchor's page
    /// (a newly created box takes its target app's grid position; a page
    /// overfilled by the insert cascades on the next `normalize`).
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
        self.remove(id);
        match self.slot_of(anchor) {
            Some((p, i)) => self.pages[p].insert(i, id.to_owned()),
            None => {
                if self.pages.is_empty() {
                    self.pages.push(Vec::new());
                }
                self.pages
                    .last_mut()
                    .expect("just ensured")
                    .push(id.to_owned());
            }
        }
        self.drop_empty_pages();
        self.save();
    }

    /// Move `id` to the end of `page`, creating intermediate pages as
    /// needed — the drop-on-a-fresh-page mutation (`page` one past the
    /// last existing page starts a new one).
    pub fn move_to_page_end(&mut self, id: &str, page: usize) {
        info!("reorder: {id} -> end of page {page}");
        self.remove(id);
        while self.pages.len() <= page {
            self.pages.push(Vec::new());
        }
        self.pages[page].push(id.to_owned());
        self.drop_empty_pages();
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
            pages: self.pages.clone(),
            order: Vec::new(),
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
            pages: Vec::new(),
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
        // Drag d before b (b's page position, even with hidden ids around).
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
        assert_eq!(f.order, vec!["a", "b"]);
        // load() maps this to a single page — mirror that mapping here.
        let pages = if !f.pages.is_empty() {
            f.pages
        } else {
            vec![f.order]
        };
        assert_eq!(pages, vec![vec!["a", "b"]]);
    }

    #[test]
    fn normalize_cascades_overflow_and_keeps_gaps() {
        let mut d = db();
        d.pages = vec![
            vec!["a", "b", "c", "d"]
                .into_iter()
                .map(String::from)
                .collect(),
            vec!["e"].into_iter().map(String::from).collect(),
        ];
        d.normalize(3, |_| true);
        // Page 0 overflowed: d cascades to the head of page 1.
        assert_eq!(d.pages(), &[vec!["a", "b", "c"], vec!["d", "e"]]);
        // Under-full pages stay under-full (the gap is the feature).
        d.normalize(3, |_| true);
        assert_eq!(d.pages(), &[vec!["a", "b", "c"], vec!["d", "e"]]);
    }

    #[test]
    fn normalize_counts_only_visible_ids() {
        let mut d = db();
        // h* are hidden (pinned/grouped): they occupy no display cell,
        // so a page with 3 visible ids among hidden ones is NOT over-full
        // at cap 3 — the 4th visible id is what cascades.
        d.pages = vec![["a", "h1", "b", "h2", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect()];
        d.normalize(3, |id| !id.starts_with('h'));
        assert_eq!(d.pages(), &[vec!["a", "h1", "b", "h2", "c"], vec!["d"]]);
    }

    #[test]
    fn move_to_page_end_creates_the_page_and_leaves_a_gap() {
        let mut d = db();
        d.sync(["a", "b", "c"].into_iter());
        d.normalize(3, |_| true);
        // Drag c onto a fresh page 1: page 0 keeps a tail gap.
        d.move_to_page_end("c", 1);
        assert_eq!(d.pages(), &[vec!["a", "b"], vec!["c"]]);
        // Dragging the last member out of a page drops the empty page.
        d.move_to_page_end("c", 0);
        assert_eq!(d.pages(), &[vec!["a", "b", "c"]]);
    }

    #[test]
    fn move_before_crosses_pages() {
        let mut d = db();
        d.pages = vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string()],
        ];
        d.move_before("c", "a");
        assert_eq!(d.pages(), &[vec!["c", "a", "b"]]);
    }
}

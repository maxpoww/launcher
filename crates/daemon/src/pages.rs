//! The shared paged-list model (macOS Launchpad style) behind both the
//! Apps grid order and a box's members: a sequence of pages of ids where
//! each page may be *under-full* at its tail. Tail gaps are the point —
//! they are what lets a drag park an id alone on a fresh page and have
//! it stay there. Pages never reflow into each other on their own; only
//! an explicit move (`move_before` / `move_to_page_end`) or `normalize`
//! (cascading over-full pages forward) shifts ids across pages.
//!
//! Serialized as `[["id", ...], ...]`; a legacy flat `["id", ...]` list
//! deserializes as a single page (a later `normalize` splits it by the
//! real capacity).

use serde::{Deserialize, Deserializer, Serialize};

/// Pages of ids, each possibly under-full at its tail.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(transparent)]
pub struct PagedList {
    pages: Vec<Vec<String>>,
}

impl<'de> Deserialize<'de> for PagedList {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        // Accept both the paged form and the legacy flat list.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Paged(Vec<Vec<String>>),
            Flat(Vec<String>),
        }
        Ok(match Wire::deserialize(de)? {
            Wire::Paged(pages) => Self { pages },
            Wire::Flat(flat) if flat.is_empty() => Self::default(),
            Wire::Flat(flat) => Self { pages: vec![flat] },
        })
    }
}

impl PagedList {
    /// A single page holding `ids` (normalize splits it later).
    pub fn from_flat(ids: Vec<String>) -> Self {
        if ids.is_empty() {
            Self::default()
        } else {
            Self { pages: vec![ids] }
        }
    }

    pub fn pages(&self) -> &[Vec<String>] {
        &self.pages
    }

    /// All ids in page-major order.
    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.pages.iter().flatten()
    }

    pub fn len(&self) -> usize {
        self.pages.iter().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.iter().all(Vec::is_empty)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.pages.iter().any(|p| p.iter().any(|o| o == id))
    }

    /// Position of `id`: (page, index within page).
    pub fn position(&self, id: &str) -> Option<(usize, usize)> {
        self.pages
            .iter()
            .enumerate()
            .find_map(|(p, page)| page.iter().position(|o| o == id).map(|i| (p, i)))
    }

    /// Append `id` to the end of the last page.
    pub fn push(&mut self, id: &str) {
        if self.pages.is_empty() {
            self.pages.push(Vec::new());
        }
        self.pages
            .last_mut()
            .expect("just ensured")
            .push(id.to_owned());
    }

    /// Remove `id` wherever it is; emptied pages drop. True if found.
    pub fn remove(&mut self, id: &str) -> bool {
        let found = self.contains(id);
        for page in &mut self.pages {
            page.retain(|o| o != id);
        }
        self.drop_empty_pages();
        found
    }

    /// Keep only ids satisfying `pred`; emptied pages drop.
    pub fn retain(&mut self, pred: impl Fn(&str) -> bool) {
        for page in &mut self.pages {
            page.retain(|id| pred(id));
        }
        self.drop_empty_pages();
    }

    /// Move `id` immediately before `anchor`, inside the anchor's page
    /// (an overfilled page cascades on the next `normalize`). Unknown
    /// anchors append to the last page.
    pub fn move_before(&mut self, id: &str, anchor: &str) {
        if anchor == id {
            return;
        }
        self.remove(id);
        match self.position(anchor) {
            Some((p, i)) => self.pages[p].insert(i, id.to_owned()),
            None => self.push(id),
        }
    }

    /// Move `id` to the end of `page` — the drop-on-a-fresh-page
    /// mutation (`page` past the last existing page starts a new one).
    pub fn move_to_page_end(&mut self, id: &str, page: usize) {
        self.remove(id);
        while self.pages.len() <= page {
            self.pages.push(Vec::new());
        }
        self.pages[page].push(id.to_owned());
        self.drop_empty_pages();
    }

    /// Legacy flat-position move: `id` lands immediately before the id
    /// currently at flat position `before` (page-major); past the end
    /// appends to the last page.
    pub fn move_before_flat(&mut self, id: &str, before: usize) {
        let anchor = self.iter().nth(before).cloned();
        match anchor {
            Some(a) if a != id => self.move_before(id, &a),
            Some(_) => {}
            None => {
                let last = self.pages.len().saturating_sub(1);
                self.move_to_page_end(id, last);
            }
        }
    }

    /// Cascade over-full pages forward so every page shows at most
    /// `cap` items, counting only ids `visible` accepts (hidden ids
    /// occupy no display cell and ride along). Then fold pages with no
    /// visible ids into a neighbor so they can't fragment the layout.
    /// Under-full pages stay under-full. Returns whether anything moved.
    pub fn normalize(&mut self, cap: usize, visible: impl Fn(&str) -> bool) -> bool {
        let cap = cap.max(1);
        let mut changed = false;
        let mut i = 0;
        while i < self.pages.len() {
            // Split where the (cap+1)-th visible id sits; trailing
            // hidden ids between the cap-th and that point stay put.
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
        // Hidden-only pages display as nothing: fold them into the
        // previous page's tail (the first page into the next head).
        let mut i = 1;
        while i < self.pages.len() {
            if self.pages[i].iter().any(|id| visible(id)) {
                i += 1;
            } else {
                let orphan = self.pages.remove(i);
                self.pages[i - 1].extend(orphan);
                changed = true;
            }
        }
        while self.pages.len() > 1 && !self.pages[0].iter().any(|id| visible(id)) {
            let orphan = self.pages.remove(0);
            for (k, id) in orphan.into_iter().enumerate() {
                self.pages[0].insert(k, id);
            }
            changed = true;
        }
        changed |= self.drop_empty_pages();
        changed
    }

    fn drop_empty_pages(&mut self) -> bool {
        let before = self.pages.len();
        self.pages.retain(|p| !p.is_empty());
        self.pages.len() != before
    }
}

/// Resolve a drop: among `items` — (display slot, payload) pairs with
/// the dragged item excluded — the anchor is the item at-or-past the
/// `gap` slot on the gap's page (the drop lands *before* it). `None`
/// means the gap sits past the page's items (its empty tail, or a ghost
/// page): the drop appends to that page. One rule for the Apps grid and
/// the open box.
pub fn drop_anchor<T>(
    items: impl Iterator<Item = (usize, T)>,
    gap: usize,
    cap: usize,
) -> Option<T> {
    let cap = cap.max(1);
    let dp = gap / cap;
    items
        .filter(|(s, _)| *s >= gap && *s / cap == dp)
        .min_by_key(|(s, _)| *s)
        .map(|(_, t)| t)
}

/// Display slot of an undragged item while a drag is in flight — the
/// make-room reflow shared by the Apps grid and the open box. Shifts are
/// page-local (Launchpad): the dragged item's page compacts to close the
/// hole it left (`origin`, its rest slot), the gap's page parts to open
/// `gap`, and a same-page reorder does the classic two-way shift between
/// them. `origin` is `None` for inserts (dock/package drags own no cell).
pub fn shifted_slot(s: usize, origin: Option<usize>, gap: usize, cap: usize) -> usize {
    let cap = cap.max(1);
    let (sp, gp) = (s / cap, gap / cap);
    match origin {
        Some(o) => {
            let op = o / cap;
            if sp == op && sp == gp {
                if o < s && s <= gap {
                    s - 1
                } else if gap <= s && s < o {
                    s + 1
                } else {
                    s
                }
            } else if sp == op && s > o {
                s - 1 // close the origin hole
            } else if sp == gp && s >= gap {
                s + 1 // open the gap
            } else {
                s
            }
        }
        None => {
            if sp == gp && s >= gap {
                s + 1
            } else {
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(pages: &[&[&str]]) -> PagedList {
        PagedList {
            pages: pages
                .iter()
                .map(|p| p.iter().map(|s| s.to_string()).collect())
                .collect(),
        }
    }

    #[test]
    fn flat_deserializes_as_one_page() {
        let l: PagedList = serde_json::from_str(r#"["a","b"]"#).unwrap();
        assert_eq!(l.pages(), &[vec!["a", "b"]]);
        let l: PagedList = serde_json::from_str(r#"[["a"],["b"]]"#).unwrap();
        assert_eq!(l.pages().len(), 2);
    }

    #[test]
    fn move_to_page_end_creates_and_keeps_gaps() {
        let mut l = list(&[&["a", "b", "c"]]);
        l.move_to_page_end("c", 1);
        assert_eq!(l.pages(), &[vec!["a", "b"], vec!["c"]]);
        // A second id JOINS the page instead of displacing the first.
        l.move_to_page_end("b", 1);
        assert_eq!(l.pages(), &[vec!["a"], vec!["c", "b"]]);
        assert!(!l.normalize(3, |_| true)); // gaps persist
    }

    #[test]
    fn normalize_cascades_and_folds_hidden_only_pages() {
        let mut l = list(&[&["a", "h1", "b", "h2", "c", "d"], &["h3"]]);
        assert!(l.normalize(3, |id| !id.starts_with('h')));
        // d (4th visible) cascades; h3's page folds into its neighbor.
        assert_eq!(
            l.pages(),
            &[vec!["a", "h1", "b", "h2", "c"], vec!["d", "h3"]]
        );
    }

    #[test]
    fn shifted_slot_reflows_page_locally() {
        // Same-page reorder (cap 3, page 0): hole at 0, gap at 2.
        assert_eq!(shifted_slot(1, Some(0), 2, 3), 0);
        assert_eq!(shifted_slot(2, Some(0), 2, 3), 1);
        // Gap parked at the hole: identity.
        assert_eq!(shifted_slot(1, Some(0), 0, 3), 1);
        // Cross-page: origin page compacts, gap page parts.
        assert_eq!(shifted_slot(2, Some(1), 4, 3), 1); // after the hole
        assert_eq!(shifted_slot(4, Some(1), 4, 3), 5); // at the gap
        assert_eq!(shifted_slot(0, Some(1), 4, 3), 0); // before the hole
                                                       // Insert (no origin): only the gap's page shifts.
        assert_eq!(shifted_slot(4, None, 4, 3), 5);
        assert_eq!(shifted_slot(2, None, 4, 3), 2);
    }

    #[test]
    fn move_before_flat_matches_page_major_order() {
        let mut l = list(&[&["a", "b"], &["c"]]);
        l.move_before_flat("c", 0);
        assert_eq!(l.pages(), &[vec!["c", "a", "b"]]);
        l.move_before_flat("c", 3);
        assert_eq!(l.pages(), &[vec!["a", "b", "c"]]);
    }
}

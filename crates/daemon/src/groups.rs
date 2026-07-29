//! Persistent app groups ("boxes", macOS/GNOME style): dropping one
//! Apps-grid app onto another creates a group; dropping an app onto a
//! group cell adds it; dragging a member out removes it. A box needs at
//! least two members: the moment it drops below two it's deleted entirely
//! (its lone app returns to the loose grid, and nothing about the box is
//! kept).
//!
//! Members are a [`PagedList`] — the same paged model as the Apps grid —
//! so a member dragged onto a fresh 3×3 page *stays* there (pages may be
//! under-full; they never reflow on their own).
//!
//! Stored in `$XDG_DATA_HOME/waverunner/groups.json` as
//! `{ "groups": [ { "name": null, "members": [["id", ...], ...] }, ... ] }`
//! (legacy flat member lists load as a single page). Writes are
//! best-effort; a failed write is logged but never fatal.

use crate::pages::PagedList;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use tracing::info;

/// Members shown per box page (the magnified box's 3×3 grid) — the
/// content layer's [`crate::content::OPEN_BOX_CAP`], re-exported so the
/// data model and the box geometry can't drift apart.
pub const PAGE_CAP: usize = crate::content::OPEN_BOX_CAP;

/// One app group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    /// Stable identity, key into the grid-order db (grid position
    /// survives other boxes dissolving). Generated at creation.
    #[serde(default)]
    pub id: String,
    /// Custom name; `None` renders a generated "<First> +N" label.
    pub name: Option<String>,
    /// Member desktop-file ids, paged (3×3 per page, tail gaps kept).
    pub members: PagedList,
}

#[derive(Default, Serialize, Deserialize)]
struct FileFormat {
    groups: Vec<Group>,
}

/// The group list, persisted like the pin db.
pub struct GroupDb {
    groups: Vec<Group>,
    /// Save target (unused under test, where `save` is a no-op).
    #[cfg_attr(test, allow(dead_code))]
    path: PathBuf,
}

impl GroupDb {
    /// Load from disk, or start empty if the file doesn't exist yet.
    /// Boxes from before stable ids get one assigned (and saved).
    pub fn load() -> Self {
        let path = crate::persist::data_path("groups.json");
        let groups = crate::persist::read_json::<FileFormat>(&path)
            .map(|f| f.groups)
            .unwrap_or_default();
        let mut db = Self { groups, path };
        if db.groups.iter().any(|g| g.id.is_empty()) {
            for i in 0..db.groups.len() {
                if db.groups[i].id.is_empty() {
                    db.groups[i].id = fresh_id();
                }
            }
            db.save();
        }
        // A legacy flat member list loads as one big page: split it by
        // the box capacity or members past the ninth are unreachable.
        let mut split = false;
        for g in &mut db.groups {
            split |= g.members.normalize(PAGE_CAP, |_| true);
        }
        if split {
            db.save();
        }
        db
    }

    /// Index of the group with the given stable id.
    pub fn index_by_id(&self, id: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.id == id)
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Index of the group containing `id`, if any.
    pub fn group_of(&self, id: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.members.contains(id))
    }

    /// Whether `id` lives inside any group (hidden from the loose grid).
    pub fn is_grouped(&self, id: &str) -> bool {
        self.group_of(id).is_some()
    }

    /// Create a new group of `[target, dragged]` (the drop target
    /// leads, macOS-style) and return its stable id. Members already
    /// in other groups move out of them first.
    pub fn create(&mut self, target: &str, dragged: &str) -> String {
        self.remove_member(dragged);
        self.remove_member(target);
        let id = fresh_id();
        self.groups.push(Group {
            id: id.clone(),
            name: None,
            members: PagedList::from_flat(vec![target.to_owned(), dragged.to_owned()]),
        });
        info!("group created: [{target}, {dragged}] as {id}");
        self.save();
        id
    }

    /// Add `id` to the group at `index` (moving it out of any other
    /// group). Out-of-range indices and existing members are no-ops.
    pub fn add(&mut self, index: usize, id: &str) {
        let mut index = index;
        if index >= self.groups.len() {
            return;
        }
        // Move `id` out of its old group first, tracking the index
        // shift if that group dissolves ahead of the target.
        if let Some(from) = self.group_of(id) {
            if from == index {
                return; // already a member
            }
            self.groups[from].members.remove(id);
            if self.groups[from].members.len() < 2 {
                self.groups.remove(from);
                if from < index {
                    index -= 1;
                }
            }
        }
        self.groups[index].members.push(id);
        self.groups[index].members.normalize(PAGE_CAP, |_| true);
        info!("group {index}: added {id}");
        self.save();
    }

    /// Swap a member id in place, keeping its slot — a pending install's
    /// attr becomes the real app id once it lands in the box.
    pub fn replace_member(&mut self, index: usize, old: &str, new: &str) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        if group.members.replace(old, new) {
            info!("group {index}: {old} -> {new}");
            self.save();
        }
    }

    /// Move a member to sit before flat member position `before` within
    /// its group (legacy flat-position reordering).
    pub fn move_member(&mut self, index: usize, id: &str, before: usize) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        group.members.move_before_flat(id, before);
        group.members.normalize(PAGE_CAP, |_| true);
        info!("group {index}: moved {id} before position {before}");
        self.save();
    }

    /// Move a member immediately before `anchor` on the anchor's page.
    pub fn move_member_before(&mut self, index: usize, id: &str, anchor: &str) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        group.members.move_before(id, anchor);
        group.members.normalize(PAGE_CAP, |_| true);
        info!("group {index}: moved {id} before {anchor}");
        self.save();
    }

    /// Move a member to the end of the box page `page`, creating it as
    /// needed — the drop-on-a-fresh-box-page mutation.
    pub fn move_member_to_page_end(&mut self, index: usize, id: &str, page: usize) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        group.members.move_to_page_end(id, page);
        group.members.normalize(PAGE_CAP, |_| true);
        info!("group {index}: moved {id} to end of box page {page}");
        self.save();
    }

    /// Remove `id` from whatever group holds it; a group left with fewer
    /// than two members is deleted entirely. Returns true if anything
    /// changed.
    pub fn remove_member(&mut self, id: &str) -> bool {
        let Some(index) = self.group_of(id) else {
            return false;
        };
        self.groups[index].members.remove(id);
        if self.groups[index].members.len() < 2 {
            info!("group {index} removed (fewer than two members)");
            self.groups.remove(index);
        } else {
            info!("group {index}: removed {id}");
        }
        self.save();
        true
    }

    /// Drop members whose app no longer exists (per `exists`), then delete
    /// any box left with fewer than two members — nothing about a removed
    /// box is kept. Returns whether anything changed. Run after an app
    /// rescan so uninstalling a box's members makes it vanish on its own.
    pub fn prune(&mut self, exists: &HashSet<String>) -> bool {
        let members_before: usize = self.groups.iter().map(|g| g.members.len()).sum();
        for g in &mut self.groups {
            g.members.retain(|m| exists.contains(m));
        }
        let groups_before = self.groups.len();
        self.groups.retain(|g| g.members.len() >= 2);
        let changed = self.groups.len() != groups_before
            || self.groups.iter().map(|g| g.members.len()).sum::<usize>() != members_before;
        if changed {
            info!("pruned dead members / undersized boxes");
            self.save();
        }
        changed
    }

    /// Display label: the custom name, or "<First member's name> +N".
    /// `name_of` resolves a member id to its display name.
    pub fn label(&self, index: usize, name_of: impl Fn(&str) -> Option<String>) -> String {
        let Some(group) = self.groups.get(index) else {
            return "Box".to_owned();
        };
        if let Some(name) = &group.name {
            return name.clone();
        }
        let first = group
            .members
            .iter()
            .find_map(|m| name_of(m))
            .unwrap_or_else(|| "Box".to_owned());
        match group.members.len() {
            0 | 1 => first,
            n => format!("{first} +{}", n - 1),
        }
    }

    #[cfg(test)]
    fn save(&self) {}

    #[cfg(not(test))]
    fn save(&self) {
        crate::persist::write_json(
            "groups",
            &self.path,
            &FileFormat {
                groups: self.groups.clone(),
            },
        );
    }
}

/// A unique, stable box id (creation-time keyed; collisions defended
/// with a process-local counter).
fn fresh_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    format!("box-{millis}-{n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> GroupDb {
        GroupDb {
            groups: Vec::new(),
            path: std::env::temp_dir().join("waverunner-groups-test.json"),
        }
    }

    fn flat(g: &GroupDb, idx: usize) -> Vec<String> {
        g.groups()[idx].members.iter().cloned().collect()
    }

    #[test]
    fn create_add_and_dissolve() {
        let mut g = db();
        let id = g.create("firefox", "gimp");
        let idx = g.index_by_id(&id).unwrap();
        assert_eq!(flat(&g, idx), vec!["firefox", "gimp"]);
        assert!(g.is_grouped("gimp"));

        g.add(idx, "vlc");
        assert_eq!(g.groups()[idx].members.len(), 3);

        // Adding an existing member is a no-op.
        g.add(idx, "vlc");
        assert_eq!(g.groups()[idx].members.len(), 3);

        assert!(g.remove_member("vlc"));
        assert_eq!(g.groups()[idx].members.len(), 2);

        // Dropping below two members deletes the box; its lone app goes loose.
        g.remove_member("firefox");
        assert!(g.groups().is_empty());
        assert!(!g.is_grouped("gimp"));
    }

    #[test]
    fn create_steals_members_from_other_groups() {
        let mut g = db();
        let first = g.create("a", "b");
        let second = g.create("c", "b"); // b moves; first drops to [a] and is deleted
        assert_eq!(g.groups().len(), 1);
        assert!(g.index_by_id(&first).is_none());
        let idx = g.index_by_id(&second).unwrap();
        assert_eq!(flat(&g, idx), vec!["c", "b"]);
        assert!(!g.is_grouped("a"));
    }

    #[test]
    fn add_below_two_shifts_indices() {
        let mut g = db();
        g.create("a", "b"); // group 0 [a, b]
        g.create("c", "d"); // group 1 [c, d]
                            // Moving `b` into group 1 drops group 0 to [a] (< 2), deleting it
                            // mid-add and shifting group 1's index down.
        g.add(1, "b");
        assert_eq!(g.groups().len(), 1);
        assert_eq!(flat(&g, 0), vec!["c", "d", "b"]);
        assert!(!g.is_grouped("a"));
    }

    #[test]
    fn members_parked_on_a_page_stay_there() {
        let mut g = db();
        let id = g.create("a", "b");
        let idx = g.index_by_id(&id).unwrap();
        for m in ["c", "d"] {
            g.add(idx, m);
        }
        // Drag c onto a fresh box page 1: it stays; d JOINS it there
        // instead of displacing it (the old dense list could only ever
        // hold one member past a full page).
        g.move_member_to_page_end(idx, "c", 1);
        g.move_member_to_page_end(idx, "d", 1);
        assert_eq!(
            g.groups()[idx].members.pages(),
            &[vec!["a", "b"], vec!["c", "d"]]
        );
    }

    #[test]
    fn prune_drops_dead_members_and_undersized_boxes() {
        let mut g = db();
        let id = g.create("a", "b");
        let idx = g.index_by_id(&id).unwrap();
        g.add(idx, "c"); // box [a, b, c]
        g.create("x", "y"); // second box [x, y]
                            // c, x, y were uninstalled; a and b remain.
        let exists: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        assert!(g.prune(&exists));
        // First box keeps [a, b]; the second (both gone) is deleted.
        assert_eq!(g.groups().len(), 1);
        assert_eq!(flat(&g, 0), vec!["a", "b"]);
        // Losing one more member drops it below two → the box is removed.
        let exists2: HashSet<String> = ["a".to_string()].into_iter().collect();
        assert!(g.prune(&exists2));
        assert!(g.groups().is_empty());
    }

    #[test]
    fn labels_generate_from_first_member() {
        let mut g = db();
        g.create("firefox", "gimp");
        let label = g.label(0, |id| (id == "firefox").then(|| "Firefox".to_owned()));
        assert_eq!(label, "Firefox +1");
    }
}

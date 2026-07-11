//! Persistent app groups ("boxes", macOS/GNOME style): dropping one
//! Apps-grid app onto another creates a group; dropping an app onto a
//! group cell adds it; dragging a member out removes it. Groups with
//! fewer than two members dissolve back into loose apps.
//!
//! Stored in `$XDG_DATA_HOME/waverunner/groups.json` as
//! `{ "groups": [ { "name": null, "members": ["id", ...] }, ... ] }`.
//! Writes are best-effort; a failed write is logged but never fatal.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

/// One app group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    /// Custom name; `None` renders a generated "<First> +N" label.
    pub name: Option<String>,
    /// Member desktop-file ids, in insertion order.
    pub members: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct FileFormat {
    groups: Vec<Group>,
}

/// The group list, persisted like the pin db.
pub struct GroupDb {
    groups: Vec<Group>,
    path: PathBuf,
}

impl GroupDb {
    /// Load from disk, or start empty if the file doesn't exist yet.
    pub fn load() -> Self {
        let path = crate::usage::data_path("groups.json");
        let groups = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<FileFormat>(&s).ok())
            .map(|f| f.groups)
            .unwrap_or_default();
        Self { groups, path }
    }

    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Index of the group containing `id`, if any.
    pub fn group_of(&self, id: &str) -> Option<usize> {
        self.groups
            .iter()
            .position(|g| g.members.iter().any(|m| m == id))
    }

    /// Whether `id` lives inside any group (hidden from the loose grid).
    pub fn is_grouped(&self, id: &str) -> bool {
        self.group_of(id).is_some()
    }

    /// Create a new group of `[target, dragged]` (the drop target
    /// leads, macOS-style) and return its index. Members already in
    /// other groups move out of them first.
    pub fn create(&mut self, target: &str, dragged: &str) -> usize {
        self.remove_member(dragged);
        self.remove_member(target);
        self.groups.push(Group {
            name: None,
            members: vec![target.to_owned(), dragged.to_owned()],
        });
        info!("group created: [{target}, {dragged}]");
        self.save();
        self.groups.len() - 1
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
            self.groups[from].members.retain(|m| m != id);
            if self.groups[from].members.len() < 2 {
                self.groups.remove(from);
                if from < index {
                    index -= 1;
                }
            }
        }
        self.groups[index].members.push(id.to_owned());
        info!("group {index}: added {id}");
        self.save();
    }

    /// Move a whole group to position `to` in the group list (manual
    /// reordering of the leading box cells).
    pub fn move_group(&mut self, from: usize, to: usize) {
        if from >= self.groups.len() {
            return;
        }
        let group = self.groups.remove(from);
        let to = to.min(self.groups.len());
        self.groups.insert(to, group);
        info!("group moved: {from} -> {to}");
        self.save();
    }

    /// Move a member to sit before member position `before` within its
    /// group (manual reordering inside an open box).
    pub fn move_member(&mut self, index: usize, id: &str, before: usize) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        if group.members.get(before).map(String::as_str) == Some(id) {
            return; // its own slot: no-op
        }
        let anchor = group.members.get(before).cloned();
        group.members.retain(|m| m != id);
        let at = anchor
            .and_then(|a| group.members.iter().position(|m| *m == a))
            .unwrap_or(group.members.len());
        group.members.insert(at, id.to_owned());
        self.save();
    }

    /// Remove `id` from whatever group holds it; a group left with
    /// fewer than two members dissolves. Returns true if anything
    /// changed.
    pub fn remove_member(&mut self, id: &str) -> bool {
        let Some(index) = self.group_of(id) else {
            return false;
        };
        self.groups[index].members.retain(|m| m != id);
        if self.groups[index].members.len() < 2 {
            info!("group {index} dissolved (fewer than two members)");
            self.groups.remove(index);
        } else {
            info!("group {index}: removed {id}");
        }
        self.save();
        true
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

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                warn!("groups: cannot create {dir:?}: {e}");
                return;
            }
        }
        let json = serde_json::json!(FileFormat {
            groups: self.groups.clone()
        });
        let tmp = self.path.with_extension("json.tmp");
        let write =
            std::fs::write(&tmp, json.to_string()).and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = write {
            warn!("groups: cannot write {:?}: {e}", self.path);
            let _ = std::fs::remove_file(&tmp);
        }
    }
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

    #[test]
    fn create_add_and_dissolve() {
        let mut g = db();
        let idx = g.create("firefox", "gimp");
        assert_eq!(g.groups()[idx].members, vec!["firefox", "gimp"]);
        assert!(g.is_grouped("gimp"));

        g.add(idx, "vlc");
        assert_eq!(g.groups()[idx].members.len(), 3);

        // Adding an existing member is a no-op.
        g.add(idx, "vlc");
        assert_eq!(g.groups()[idx].members.len(), 3);

        assert!(g.remove_member("vlc"));
        assert_eq!(g.groups()[idx].members.len(), 2);

        // Dropping to one member dissolves the group.
        g.remove_member("firefox");
        assert!(g.groups().is_empty());
        assert!(!g.is_grouped("gimp"));
    }

    #[test]
    fn create_steals_members_from_other_groups() {
        let mut g = db();
        g.create("a", "b");
        let idx = g.create("c", "b"); // b moves; [a] dissolves
        assert_eq!(g.groups().len(), 1);
        assert_eq!(g.groups()[idx].members, vec!["c", "b"]);
        assert!(!g.is_grouped("a"));
    }

    #[test]
    fn add_survives_a_dissolve_shifting_indices() {
        let mut g = db();
        g.create("a", "b"); // group 0
        g.create("c", "d"); // group 1
                            // Moving `b` into group 1 dissolves group 0 mid-flight.
        g.add(1, "b");
        assert_eq!(g.groups().len(), 1);
        assert_eq!(g.groups()[0].members, vec!["c", "d", "b"]);
        assert!(!g.is_grouped("a"));
    }

    #[test]
    fn labels_generate_from_first_member() {
        let mut g = db();
        g.create("firefox", "gimp");
        let label = g.label(0, |id| (id == "firefox").then(|| "Firefox".to_owned()));
        assert_eq!(label, "Firefox +1");
    }
}

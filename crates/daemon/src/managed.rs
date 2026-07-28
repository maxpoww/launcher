//! The waverunner-managed package list — apps the user installed through
//! the launcher via `nix profile`.
//!
//! The **declarative record** lives at
//! `~/.config/home-manager/waverunner-packages.nix` — a home-manager
//! module that travels to a new machine for reproducibility. Actual
//! installation and removal use `nix profile install/remove`; the `.nix`
//! is regenerated on every mutation as a portable snapshot only.
//! A JSON sidecar in the daemon's data dir caches each attr's shipped
//! desktop-entry ids so an uninstall drag can map an app back to the attr
//! that provides it. Writes are best-effort (logged, never fatal).

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One waverunner-installed package.
#[derive(Clone, Serialize, Deserialize)]
struct ManagedPkg {
    /// nixpkgs attribute — the install handle and the `.nix` list entry.
    attr: String,
    /// Desktop-entry ids the package ships (plus its attr): used to map an
    /// installed app back to the attr that provides it.
    desktop_ids: Vec<String>,
    /// Whether the package ships a GUI `.desktop` launcher, learned from
    /// its store path at install time. `Some(true)` = a real app, so no
    /// synthetic terminal tile is ever fabricated for it (not even in the
    /// window after install, before its `.desktop` is scanned — that
    /// fabrication used to get pinned in the real app's place).
    /// `Some(false)` = a CLI-only tool (it gets a terminal tile). `None` =
    /// unknown (a legacy entry from before this was recorded, or a store
    /// path that could not be read) — the tile decision then falls back to
    /// "is it already covered by a scanned `.desktop`?".
    #[serde(default)]
    gui: Option<bool>,
    /// Transient (never serialized): whether this package's install has
    /// actually completed. Only confirmed packages are written to disk, so a
    /// still-installing stage — or several staged at once while their
    /// rebuilds run one at a time — never leaks into `managed.json` and gets
    /// a phantom terminal tile before its real `.desktop` exists. Packages
    /// loaded from disk are confirmed by definition (they were written).
    #[serde(skip_serializing, default = "confirmed_on_load")]
    confirmed: bool,
}

/// Deserialization default for [`ManagedPkg::confirmed`]: anything already on
/// disk was, by definition, a confirmed install.
fn confirmed_on_load() -> bool {
    true
}

/// The attr → desktop-id/gui cache behind the declarative package list.
/// The list (`~/.config/waverunner/packages.list`, see [`crate::applier`])
/// is the source of truth for *which* packages are installed; this JSON
/// sidecar only remembers what each attr ships, so an uninstall drag can map
/// an app back to its attr and so CLI-only tools get a launch tile.
pub struct ManagedDb {
    pkgs: Vec<ManagedPkg>,
    /// JSON sidecar caching the attr → desktop-id / gui mapping.
    json_path: PathBuf,
}

impl ManagedDb {
    /// Load the sidecar. Reconciliation against the declarative list happens
    /// at startup via [`Self::adopt_list`].
    pub fn load() -> Self {
        let json_path = crate::persist::data_path("managed.json");
        let pkgs: Vec<ManagedPkg> = crate::persist::read_json(&json_path).unwrap_or_default();
        Self { pkgs, json_path }
    }

    /// Reconcile the cache with the declarative package list (the source of
    /// truth): adopt any listed attr the cache is missing (mapping by its own
    /// name until an app resolves), and drop cached attrs no longer listed
    /// (uninstalled out-of-band, or a hand-edited list). Persists on change.
    pub fn adopt_list(&mut self, attrs: &[String]) {
        let listed: HashSet<&str> = attrs.iter().map(String::as_str).collect();
        let before = self.pkgs.len();
        self.pkgs.retain(|p| listed.contains(p.attr.as_str()));
        let known: HashSet<String> = self.pkgs.iter().map(|p| p.attr.clone()).collect();
        let mut changed = self.pkgs.len() != before;
        for attr in attrs {
            if !known.contains(attr) {
                self.pkgs.push(ManagedPkg {
                    desktop_ids: vec![attr.clone()],
                    attr: attr.clone(),
                    gui: None,
                    confirmed: true, // it's in the declarative list = installed
                });
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    /// Whether `attr` is in the managed list.
    pub fn contains(&self, attr: &str) -> bool {
        self.pkgs.iter().any(|p| p.attr == attr)
    }

    /// The managed attr that provides the app with desktop id `id`, if any.
    pub fn attr_for_app(&self, id: &str) -> Option<String> {
        self.pkgs
            .iter()
            .find(|p| p.attr == id || p.desktop_ids.iter().any(|d| d == id))
            .map(|p| p.attr.clone())
    }

    /// Every desktop id (and attr) the managed packages ship — the set of
    /// app ids that waverunner installed and can therefore uninstall.
    pub fn removable_ids(&self) -> HashSet<String> {
        self.pkgs
            .iter()
            .flat_map(|p| std::iter::once(p.attr.clone()).chain(p.desktop_ids.iter().cloned()))
            .collect()
    }

    /// Stage a package in the in-memory list as *unconfirmed* — nothing is
    /// written to disk until [`Self::confirm`] runs on the install's
    /// completion. Keeps a still-installing (or killed-mid-install) stage,
    /// and other stages queued behind it, out of `managed.json` — otherwise
    /// they'd get a phantom terminal tile before their `.desktop` exists.
    pub fn stage(&mut self, attr: &str, mut desktop_ids: Vec<String>) {
        if !desktop_ids.iter().any(|d| d == attr) {
            desktop_ids.push(attr.to_owned());
        }
        match self.pkgs.iter_mut().find(|p| p.attr == attr) {
            Some(p) => p.desktop_ids = desktop_ids,
            None => self.pkgs.push(ManagedPkg {
                attr: attr.to_owned(),
                desktop_ids,
                gui: None,
                confirmed: false,
            }),
        }
    }

    /// After the install's rebuild lands and the app resolves, record what
    /// the package actually ships: merge in the real `.desktop` id and
    /// whether it is a GUI app, mark it confirmed, and persist. This is what
    /// makes the grid resolve to the real app and stops a synthetic terminal
    /// tile from ever standing in for a GUI app.
    pub fn note_installed(&mut self, attr: &str, desktop_ids: &[String], gui: bool) {
        if let Some(p) = self.pkgs.iter_mut().find(|p| p.attr == attr) {
            for id in desktop_ids {
                if !p.desktop_ids.iter().any(|d| d == id) {
                    p.desktop_ids.push(id.clone());
                }
            }
            p.gui = Some(gui);
            p.confirmed = true;
        }
        self.save();
    }

    /// Mark `attr`'s install as complete and persist. Only confirmed
    /// packages reach disk, so a batch of simultaneous installs each land in
    /// `managed.json` as their own rebuild finishes — never all at once when
    /// the first of them completes.
    pub fn confirm(&mut self, attr: &str) {
        if let Some(p) = self.pkgs.iter_mut().find(|p| p.attr == attr) {
            p.confirmed = true;
        }
        self.save();
    }

    /// Remove a staged package from the in-memory list. It was never written
    /// (unconfirmed), so disk stays clean. Call when an install fails.
    pub fn revert(&mut self, attr: &str) {
        self.pkgs.retain(|p| p.attr != attr);
    }

    /// All managed attrs currently staged or confirmed.
    pub fn all_attrs(&self) -> Vec<String> {
        self.pkgs.iter().map(|p| p.attr.clone()).collect()
    }

    /// Attrs whose GUI-ness was never learned (`gui == None`): adopted from
    /// the package list on startup or a migrated legacy entry, rather than
    /// classified by an install's resolve. The scan reconciler links these
    /// to their real app or marks them CLI-only.
    pub fn gui_unknown_attrs(&self) -> Vec<String> {
        self.pkgs
            .iter()
            .filter(|p| p.gui.is_none())
            .map(|p| p.attr.clone())
            .collect()
    }

    /// Desktop ids stored for `attr` (used for dock-pin resolution after
    /// a managed install without a pending grid tile).
    pub fn desktop_ids_for(&self, attr: &str) -> Vec<String> {
        self.pkgs
            .iter()
            .find(|p| p.attr == attr)
            .map(|p| p.desktop_ids.clone())
            .unwrap_or_default()
    }

    /// Drop a managed package from the cache. The actual uninstall (editing
    /// the declarative list + rebuild) is driven separately via
    /// [`crate::applier::apply_uninstall`].
    pub fn remove(&mut self, attr: &str) {
        self.pkgs.retain(|p| p.attr != attr);
        self.save();
    }

    /// Persist the attr → desktop-id / gui cache — only the *confirmed*
    /// packages (a staged-but-not-yet-installed entry never reaches disk).
    /// The declarative package list, not this file, records what is installed.
    fn save(&self) {
        let confirmed: Vec<&ManagedPkg> = self.pkgs.iter().filter(|p| p.confirmed).collect();
        crate::persist::write_json("managed", &self.json_path, &confirmed);
    }
}

/// A read-only snapshot of the managed cache — each attr with the desktop
/// ids it ships and its GUI flag — read straight from disk. Lets the indexer
/// thread (which has no [`ManagedDb`]) synthesize tiles for installed CLI
/// tools that ship no `.desktop`. Falls back to the declarative package list
/// when the sidecar is missing (a fresh cache), so every listed attr still
/// gets classified.
pub fn snapshot() -> Vec<(String, Vec<String>, Option<bool>)> {
    let json_path = crate::persist::data_path("managed.json");
    let pkgs: Vec<ManagedPkg> = crate::persist::read_json(&json_path).unwrap_or_default();
    if !pkgs.is_empty() {
        return pkgs
            .into_iter()
            .map(|p| (p.attr, p.desktop_ids, p.gui))
            .collect();
    }
    crate::applier::list_attrs()
        .into_iter()
        .map(|attr| (attr.clone(), vec![attr], None))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(tag: &str) -> (ManagedDb, PathBuf) {
        let dir = std::env::temp_dir().join(format!("wr-managed-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ManagedDb {
            pkgs: Vec::new(),
            json_path: dir.join("managed.json"),
        };
        (db, dir)
    }

    #[test]
    fn add_remove_map_and_persist() {
        let (mut d, dir) = db("crud");
        d.stage("vlc", vec!["vlc".into()]);
        d.stage("google-chrome", vec![]); // attr auto-added as its own id
        // stage is in-memory only — the sidecar should not exist yet
        assert!(!d.json_path.exists());
        d.confirm("vlc");
        d.confirm("google-chrome");
        assert!(d.contains("vlc"));
        // an app maps back to the attr that ships its desktop id
        assert_eq!(
            d.attr_for_app("google-chrome").as_deref(),
            Some("google-chrome")
        );
        assert!(d.removable_ids().contains("vlc"));
        // the sidecar persists and reloads to the same attrs
        let reloaded: Vec<ManagedPkg> = crate::persist::read_json(&d.json_path).unwrap();
        let mut attrs: Vec<&str> = reloaded.iter().map(|p| p.attr.as_str()).collect();
        attrs.sort_unstable();
        assert_eq!(attrs, vec!["google-chrome", "vlc"]);
        d.remove("vlc");
        assert!(!d.contains("vlc"));
        assert!(d.attr_for_app("vlc").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn adopt_list_reconciles_with_the_declarative_set() {
        let (mut d, dir) = db("adopt");
        d.stage("vlc", vec!["vlc-real".into()]);
        d.confirm("vlc");
        // The list drops vlc and adds two new attrs.
        d.adopt_list(&["mpv".to_string(), "fzf".to_string()]);
        assert!(!d.contains("vlc"), "unlisted attr dropped");
        assert!(d.contains("mpv") && d.contains("fzf"), "listed attrs adopted");
        // Adopting an unchanged set is a no-op (still consistent).
        d.adopt_list(&["fzf".to_string(), "mpv".to_string()]);
        assert!(d.contains("mpv") && d.contains("fzf"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn note_installed_records_real_ids_and_gui_flag() {
        let (mut d, dir) = db("note-installed");
        // A GUI app whose shipped desktop stem happens to equal its attr
        // (davinci-resolve.desktop) — the exact case that used to be
        // masked by a synthetic terminal tile.
        d.stage("davinci-resolve", vec![]);
        assert!(d.pkgs[0].gui.is_none(), "gui unknown until install lands");
        // note_installed marks confirmed + persists on its own.
        d.note_installed("davinci-resolve", &["davinci-resolve".into()], true);
        assert_eq!(d.pkgs[0].gui, Some(true), "recorded as a GUI app");
        // A CLI tool ships no desktop entry: gui is Some(false).
        d.stage("fzf", vec![]);
        d.note_installed("fzf", &[], false);
        assert_eq!(
            d.pkgs.iter().find(|p| p.attr == "fzf").unwrap().gui,
            Some(false)
        );
        // The gui flag survives a round-trip through the JSON sidecar, so
        // the indexer's snapshot can read it back and suppress a terminal
        // tile for the GUI app while keeping one for the CLI tool.
        let reloaded: Vec<ManagedPkg> = crate::persist::read_json(&d.json_path).unwrap();
        let gui_of = |attr: &str| reloaded.iter().find(|p| p.attr == attr).map(|p| p.gui);
        assert_eq!(gui_of("davinci-resolve"), Some(Some(true)));
        assert_eq!(gui_of("fzf"), Some(Some(false)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn note_installed_merges_distinct_desktop_ids() {
        let (mut d, dir) = db("note-merge");
        d.stage("chromium", vec!["chromium".into()]);
        // The real launcher differs from the attr (chromium-browser.desktop).
        d.note_installed("chromium", &["chromium-browser".into(), "chromium".into()], true);
        let p = &d.pkgs[0];
        assert!(p.desktop_ids.contains(&"chromium".to_string()));
        assert!(p.desktop_ids.contains(&"chromium-browser".to_string()));
        // No duplicates from re-merging the attr.
        assert_eq!(
            p.desktop_ids.iter().filter(|d| *d == "chromium").count(),
            1
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn revert_does_not_touch_disk() {
        let (mut d, dir) = db("revert");
        d.stage("obs-studio", vec![]);
        assert!(d.contains("obs-studio"));
        // revert while install is in-flight: in-memory list reverts, disk untouched
        d.revert("obs-studio");
        assert!(!d.contains("obs-studio"));
        assert!(!d.json_path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn desktop_ids_distinguish_wrapped_apps() {
        let (mut d, dir) = db("wrapped");
        // chromium ships a differently-named desktop entry
        d.stage("chromium", vec!["chromium-browser".into()]);
        d.confirm("chromium");
        assert_eq!(
            d.attr_for_app("chromium-browser").as_deref(),
            Some("chromium")
        );
        assert_eq!(d.attr_for_app("chromium").as_deref(), Some("chromium"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gui_unknown_tracks_unclassified_attrs() {
        let (mut d, dir) = db("gui-unknown");
        d.stage("chromium", vec!["chromium".into()]);
        d.confirm("chromium");
        // Adopted / migrated: installed but GUI-ness not yet learned.
        assert_eq!(d.gui_unknown_attrs(), vec!["chromium".to_string()]);
        // The scan reconciler links it to its real app → no longer unknown.
        d.note_installed("chromium", &["chromium-browser".into()], true);
        assert!(d.gui_unknown_attrs().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_confirmed_installs_reach_disk() {
        // The concurrency guarantee: staging several packages at once (a
        // batch of simultaneous drags) must not write the not-yet-installed
        // ones to disk when the first finishes — that used to give brave a
        // phantom terminal tile while chromium's install completed first.
        let (mut d, dir) = db("confirmed");
        d.stage("chromium", vec!["chromium".into()]);
        d.stage("brave", vec!["brave".into()]);
        d.confirm("chromium"); // chromium's rebuild finished first
        let on_disk: Vec<ManagedPkg> = crate::persist::read_json(&d.json_path).unwrap();
        let attrs: Vec<&str> = on_disk.iter().map(|p| p.attr.as_str()).collect();
        assert_eq!(attrs, vec!["chromium"], "only the confirmed install written");
        // Reloaded packages are confirmed by definition.
        assert!(on_disk[0].confirmed);
        // brave lands once its own install confirms.
        d.confirm("brave");
        let on_disk: Vec<ManagedPkg> = crate::persist::read_json(&d.json_path).unwrap();
        let mut attrs: Vec<&str> = on_disk.iter().map(|p| p.attr.as_str()).collect();
        attrs.sort_unstable();
        assert_eq!(attrs, vec!["brave", "chromium"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}

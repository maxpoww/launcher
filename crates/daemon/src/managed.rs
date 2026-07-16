//! The waverunner-managed home-manager package list — the apps the user
//! installed through the launcher.
//!
//! The **source of truth** is `~/.config/home-manager/waverunner-packages.nix`
//! (`{ pkgs }: with pkgs; [ … ]`) — the file home-manager reads and the one
//! that travels to a new machine. A JSON sidecar in the daemon's data dir
//! caches each attr's shipped desktop-entry ids, so an uninstall drag can
//! map a launched app back to the attr that installs it. Every mutation
//! regenerates the `.nix`; the nix worker then runs `home-manager switch`
//! to apply it. Writes are best-effort (logged, never fatal).

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::info;

/// One waverunner-installed package.
#[derive(Clone, Serialize, Deserialize)]
struct ManagedPkg {
    /// nixpkgs attribute — the install handle and the `.nix` list entry.
    attr: String,
    /// Desktop-entry ids the package ships (plus its attr): used to map an
    /// installed app back to the attr that provides it.
    desktop_ids: Vec<String>,
}

/// The managed package list, backed by the generated `.nix` and a JSON
/// sidecar.
pub struct ManagedDb {
    pkgs: Vec<ManagedPkg>,
    /// `waverunner-packages.nix` — the file home-manager imports.
    nix_path: PathBuf,
    /// JSON sidecar caching the attr → desktop-id mapping.
    json_path: PathBuf,
}

impl ManagedDb {
    /// Load the sidecar, then reconcile it with the attrs actually present
    /// in the `.nix` (so a hand-copied list from another machine still
    /// works — its attrs map by their own name until an app resolves).
    pub fn load() -> Self {
        let json_path = crate::usage::data_path("managed.json");
        let nix_path = home_manager_dir().join("waverunner-packages.nix");
        let mut pkgs: Vec<ManagedPkg> = crate::persist::read_json(&json_path).unwrap_or_default();
        // Adopt any attr present in the .nix but missing from the sidecar.
        let known: HashSet<String> = pkgs.iter().map(|p| p.attr.clone()).collect();
        let mut adopted = false;
        for attr in parse_nix_attrs(&nix_path) {
            if !known.contains(&attr) {
                pkgs.push(ManagedPkg {
                    desktop_ids: vec![attr.clone()],
                    attr,
                });
                adopted = true;
            }
        }
        let db = Self {
            pkgs,
            nix_path,
            json_path,
        };
        if adopted {
            db.save();
        }
        db
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

    /// Add (or refresh the desktop ids of) a managed package and rewrite
    /// the `.nix` + sidecar.
    pub fn add(&mut self, attr: &str, mut desktop_ids: Vec<String>) {
        if !desktop_ids.iter().any(|d| d == attr) {
            desktop_ids.push(attr.to_owned());
        }
        match self.pkgs.iter_mut().find(|p| p.attr == attr) {
            Some(p) => p.desktop_ids = desktop_ids,
            None => self.pkgs.push(ManagedPkg {
                attr: attr.to_owned(),
                desktop_ids,
            }),
        }
        self.save();
    }

    /// Record that `attr` provides the app with desktop id `app_id` (the
    /// id it actually got after installing — often different from the
    /// attr, e.g. `chromium` → `chromium-browser`). Keeps uninstall and
    /// removable detection working when the index had no desktop hints.
    pub fn link_app(&mut self, attr: &str, app_id: &str) {
        let mut changed = false;
        if let Some(p) = self.pkgs.iter_mut().find(|p| p.attr == attr) {
            if !p.desktop_ids.iter().any(|d| d == app_id) {
                p.desktop_ids.push(app_id.to_owned());
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    /// Drop a managed package and rewrite the `.nix` + sidecar.
    pub fn remove(&mut self, attr: &str) {
        self.pkgs.retain(|p| p.attr != attr);
        self.save();
    }

    /// Regenerate the `.nix` (home-manager's source) and the JSON sidecar.
    fn save(&self) {
        let mut attrs: Vec<&str> = self.pkgs.iter().map(|p| p.attr.as_str()).collect();
        attrs.sort_unstable();
        let mut nix = String::from(
            "# Managed by waverunner — user-installed apps.\n\
             #\n\
             # waverunner adds/removes attributes here on drag-install / uninstall,\n\
             # then runs `home-manager switch`. Edit through the launcher.\n\
             { pkgs }:\n\n\
             with pkgs; [\n",
        );
        for attr in attrs {
            nix.push_str("  ");
            nix.push_str(attr);
            nix.push('\n');
        }
        nix.push_str("]\n");
        crate::persist::write_text("managed", &self.nix_path, &nix);
        info!("managed: wrote {:?}", self.nix_path);
        crate::persist::write_json("managed", &self.json_path, &self.pkgs);
    }
}

/// A read-only snapshot of the managed packages — each attr with the
/// desktop ids it ships — read straight from disk. Lets the indexer thread
/// (which has no [`ManagedDb`]) synthesize tiles for installed CLI tools
/// that ship no `.desktop`. Falls back to the `.nix` for a hand-copied
/// list that has no sidecar yet.
pub fn snapshot() -> Vec<(String, Vec<String>)> {
    let json_path = crate::usage::data_path("managed.json");
    let pkgs: Vec<ManagedPkg> = crate::persist::read_json(&json_path).unwrap_or_default();
    if !pkgs.is_empty() {
        return pkgs.into_iter().map(|p| (p.attr, p.desktop_ids)).collect();
    }
    parse_nix_attrs(&home_manager_dir().join("waverunner-packages.nix"))
        .into_iter()
        .map(|attr| (attr.clone(), vec![attr]))
        .collect()
}

/// `~/.config/home-manager` (respecting `XDG_CONFIG_HOME`).
fn home_manager_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        });
    base.join("home-manager")
}

/// Extract the bare attribute tokens from a `with pkgs; [ … ]` list — one
/// attr per line, ignoring the wrapper syntax and `#` comments. Tolerant
/// of a hand-edited file; anything it can't read is simply skipped.
fn parse_nix_attrs(path: &PathBuf) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or("").trim();
            // Skip blanks and any line carrying wrapper punctuation.
            if line.is_empty() || line.contains(['{', '}', '[', ']', ':', ';']) {
                return None;
            }
            let token = line.split_whitespace().next()?;
            token
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
                .then(|| token.to_owned())
        })
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
            nix_path: dir.join("waverunner-packages.nix"),
            json_path: dir.join("managed.json"),
        };
        (db, dir)
    }

    #[test]
    fn add_remove_map_and_regenerate() {
        let (mut d, dir) = db("crud");
        d.add("vlc", vec!["vlc".into()]);
        d.add("google-chrome", vec![]); // attr auto-added as its own id
        assert!(d.contains("vlc"));
        // an app maps back to the attr that ships its desktop id
        assert_eq!(
            d.attr_for_app("google-chrome").as_deref(),
            Some("google-chrome")
        );
        assert!(d.removable_ids().contains("vlc"));
        // the .nix is regenerated, sorted, and parses back to the attrs
        assert_eq!(
            parse_nix_attrs(&dir.join("waverunner-packages.nix")),
            vec!["google-chrome".to_string(), "vlc".to_string()]
        );
        d.remove("vlc");
        assert!(!d.contains("vlc"));
        assert!(d.attr_for_app("vlc").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_hand_edited_list_ignoring_wrapper_and_comments() {
        let (_d, dir) = db("parse");
        let p = dir.join("waverunner-packages.nix");
        std::fs::write(
            &p,
            "{ pkgs }:\nwith pkgs; [\n  vlc # a comment\n  kdePackages.kdenlive\n]\n",
        )
        .unwrap();
        assert_eq!(
            parse_nix_attrs(&p),
            vec!["vlc".to_string(), "kdePackages.kdenlive".to_string()]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn desktop_ids_distinguish_wrapped_apps() {
        let (mut d, dir) = db("wrapped");
        // chromium ships a differently-named desktop entry
        d.add("chromium", vec!["chromium-browser".into()]);
        assert_eq!(
            d.attr_for_app("chromium-browser").as_deref(),
            Some("chromium")
        );
        assert_eq!(d.attr_for_app("chromium").as_deref(), Some("chromium"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

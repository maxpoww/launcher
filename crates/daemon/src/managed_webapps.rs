//! The set of webapps the user has *installed* from the catalog (dragged
//! onto the Apps grid), recorded declaratively.
//!
//! The portable record lives at `~/.config/home-manager/waverunner-webapps.nix`
//! — a snapshot of installed slugs that travels to a new machine; on first
//! run [`crate::webapps::materialize_catalog`] re-creates the launchers and
//! membership decides which show on the grid. A JSON sidecar mirrors it.
//! Writes are best-effort (logged, never fatal), matching [`crate::managed`].

use std::path::PathBuf;

use tracing::info;

/// The installed-webapps list, backed by the generated `.nix` and a JSON
/// sidecar (a bare `["slug", ...]` array).
pub struct ManagedWebapps {
    slugs: Vec<String>,
    /// `waverunner-webapps.nix` — the portable snapshot.
    nix_path: PathBuf,
    /// JSON sidecar in the daemon data dir.
    json_path: PathBuf,
}

impl ManagedWebapps {
    /// Load the sidecar, then adopt any slug present only in the `.nix`
    /// (a list hand-copied from another machine).
    pub fn load() -> Self {
        let json_path = crate::persist::data_path("managed-webapps.json");
        let nix_path = home_manager_dir().join("waverunner-webapps.nix");
        let mut slugs: Vec<String> = crate::persist::read_json(&json_path).unwrap_or_default();
        let mut adopted = false;
        for slug in parse_nix_slugs(&nix_path) {
            if !slugs.iter().any(|s| s == &slug) {
                slugs.push(slug);
                adopted = true;
            }
        }
        let db = Self {
            slugs,
            nix_path,
            json_path,
        };
        if adopted {
            db.save();
        }
        db
    }

    /// The installed slugs.
    #[allow(dead_code)] // accessor for reproduction/debug; used by the drag step
    pub fn slugs(&self) -> &[String] {
        &self.slugs
    }

    /// Whether `slug` is installed.
    pub fn contains(&self, slug: &str) -> bool {
        self.slugs.iter().any(|s| s == slug)
    }

    /// Record `slug` as installed and rewrite the `.nix` + sidecar.
    pub fn add(&mut self, slug: &str) {
        if !self.slugs.iter().any(|s| s == slug) {
            self.slugs.push(slug.to_owned());
            self.save();
        }
    }

    /// Drop `slug` and rewrite the `.nix` + sidecar.
    pub fn remove(&mut self, slug: &str) {
        let before = self.slugs.len();
        self.slugs.retain(|s| s != slug);
        if self.slugs.len() != before {
            self.save();
        }
    }

    /// Regenerate the declarative `.nix` snapshot and the JSON sidecar.
    fn save(&self) {
        let mut slugs: Vec<&str> = self.slugs.iter().map(String::as_str).collect();
        slugs.sort_unstable();
        let mut nix = String::from(
            "# Managed by waverunner — webapps installed from the catalog.\n\
             #\n\
             # Portable snapshot of installed slugs; waverunner materializes the\n\
             # launchers from the catalog (~/.config/webapps.list) on start. Copy\n\
             # this file to another machine to reproduce the installed set.\n\
             {\n  waverunner.installedWebapps = [\n",
        );
        for slug in &slugs {
            nix.push_str("    \"");
            nix.push_str(slug);
            nix.push_str("\"\n");
        }
        nix.push_str("  ];\n}\n");
        crate::persist::write_text("managed-webapps", &self.nix_path, &nix);
        info!("managed-webapps: wrote {:?}", self.nix_path);
        crate::persist::write_json("managed-webapps", &self.json_path, &self.slugs);
    }
}

/// `~/.config/home-manager` (respecting `XDG_CONFIG_HOME`).
fn home_manager_dir() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        })
        .join("home-manager")
}

/// Extract the quoted slug tokens from the snapshot's `installedWebapps`
/// list. Tolerant of hand edits; anything unparseable is skipped.
fn parse_nix_slugs(path: &PathBuf) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let (Some(a), Some(b)) = (line.find('"'), line.rfind('"')) {
            if b > a {
                out.push(line[a + 1..b].to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(tag: &str) -> (ManagedWebapps, PathBuf) {
        let dir = std::env::temp_dir().join(format!("wr-webapps-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ManagedWebapps {
            slugs: Vec::new(),
            nix_path: dir.join("waverunner-webapps.nix"),
            json_path: dir.join("managed-webapps.json"),
        };
        (db, dir)
    }

    #[test]
    fn add_remove_and_regenerate() {
        let (mut d, dir) = db("crud");
        d.add("netflix");
        d.add("gmail");
        d.add("netflix"); // idempotent
        assert!(d.contains("netflix"));
        assert_eq!(
            parse_nix_slugs(&dir.join("waverunner-webapps.nix")),
            vec!["gmail".to_string(), "netflix".to_string()]
        );
        d.remove("netflix");
        assert!(!d.contains("netflix"));
        assert_eq!(
            parse_nix_slugs(&dir.join("waverunner-webapps.nix")),
            vec!["gmail".to_string()]
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}

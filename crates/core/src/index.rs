//! Index of launchable applications discovered from `.desktop` entries.
//!
//! Scanning walks the XDG application dirs in precedence order (data
//! home first), deduplicates by desktop-file ID, honors
//! `NoDisplay`/`Hidden`, and strips Exec field codes. Indexing runs on
//! a background thread in the daemon and sends a finished
//! `DesktopIndex` over a channel.

use std::collections::HashSet;
use std::path::PathBuf;

use freedesktop_desktop_entry::{default_paths, get_languages_from_env, DesktopEntry, Iter};

/// One launchable application, extracted from a `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    /// Desktop-file ID (`org.mozilla.firefox`, `codium`, …), unique.
    pub id: String,
    /// Display name (localized `Name=`).
    pub name: String,
    /// Optional `GenericName=`/`Comment=`, shown as a subtitle and used as
    /// a secondary match target.
    pub description: Option<String>,
    /// The `Exec=` line with field codes (`%f`, `%u`, …) already stripped.
    pub exec: String,
    /// Icon name for lookup via `freedesktop-icons`.
    pub icon: Option<String>,
    /// Whether `Terminal=true` was set (needs a terminal emulator wrapper).
    pub needs_terminal: bool,
}

/// The full application index the search UI ranks against.
#[derive(Debug, Clone, Default)]
pub struct DesktopIndex {
    /// All discovered entries, deduplicated by desktop-file ID and
    /// sorted by display name.
    pub entries: Vec<AppEntry>,
}

impl DesktopIndex {
    /// Scan the XDG data dirs for `.desktop` files and build the index.
    pub fn scan() -> Self {
        Self::scan_paths(default_paths())
    }

    /// Scan an explicit set of `applications` directories (testable core
    /// of [`DesktopIndex::scan`]). Earlier paths take precedence when
    /// two files share a desktop-file ID.
    pub fn scan_paths(paths: impl Iterator<Item = PathBuf>) -> Self {
        let locales = get_languages_from_env();
        let mut seen: HashSet<String> = HashSet::new();
        let mut entries = Vec::new();

        for path in Iter::new(paths) {
            let entry = match DesktopEntry::from_path(path, Some(&locales)) {
                Ok(entry) => entry,
                Err(_) => continue, // not valid UTF-8 / not an entry: skip
            };
            let id = entry.id().to_string();
            if seen.contains(&id) {
                continue;
            }
            if entry.type_().is_some_and(|t| t != "Application") {
                continue;
            }
            if entry.no_display() || entry.hidden() {
                // Still claims the ID: an overriding NoDisplay entry in
                // data home is how users hide a system-wide app.
                seen.insert(id);
                continue;
            }
            let Some(exec) = entry.exec() else {
                continue;
            };
            let exec = strip_field_codes(exec);
            if exec.is_empty() {
                continue;
            }
            let name = entry
                .name(&locales)
                .map(|n| n.to_string())
                .unwrap_or_else(|| id.clone());
            let description = entry
                .comment(&locales)
                .or_else(|| entry.generic_name(&locales))
                .map(|d| d.to_string());

            entries.push(AppEntry {
                name,
                description,
                exec,
                icon: entry.icon().map(str::to_string),
                needs_terminal: entry.terminal(),
                id: id.clone(),
            });
            seen.insert(id);
        }

        entries.sort_by_key(|e| e.name.to_lowercase());
        Self { entries }
    }

    /// Names of all entries, in index order — the haystack handed to
    /// [`crate::Searcher::search`].
    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }
}

/// Remove `%f`-style field codes from an `Exec=` line, per the desktop
/// entry spec; `%%` unescapes to a literal `%`.
fn strip_field_codes(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // `%%` unescapes; any other (or trailing) field code is dropped.
        if let Some('%') = chars.next() {
            out.push('%');
        }
    }
    // Collapse the double spaces dropped codes leave behind.
    let mut collapsed = String::with_capacity(out.len());
    let mut last_space = false;
    for c in out.trim().chars() {
        if c == ' ' {
            if !last_space {
                collapsed.push(c);
            }
            last_space = true;
        } else {
            collapsed.push(c);
            last_space = false;
        }
    }
    collapsed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_apps_dir(files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "waverunner-index-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let apps = dir.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        for (name, content) in files {
            std::fs::write(apps.join(name), content).unwrap();
        }
        apps
    }

    #[test]
    fn scans_and_sorts_entries() {
        let apps = write_apps_dir(&[
            (
                "zed.desktop",
                "[Desktop Entry]\nType=Application\nName=Zed\nExec=zed %U\n",
            ),
            (
                "alpha.desktop",
                "[Desktop Entry]\nType=Application\nName=Alpha\nExec=alpha\nIcon=alpha-icon\n",
            ),
        ]);
        let index = DesktopIndex::scan_paths(std::iter::once(apps.clone()));
        std::fs::remove_dir_all(apps.parent().unwrap()).ok();

        assert_eq!(index.names(), vec!["Alpha", "Zed"]);
        assert_eq!(index.entries[0].icon.as_deref(), Some("alpha-icon"));
        assert_eq!(index.entries[1].exec, "zed");
    }

    #[test]
    fn skips_nodisplay_and_hidden() {
        let apps = write_apps_dir(&[
            (
                "shown.desktop",
                "[Desktop Entry]\nType=Application\nName=Shown\nExec=shown\n",
            ),
            (
                "nodisplay.desktop",
                "[Desktop Entry]\nType=Application\nName=Hidden A\nExec=a\nNoDisplay=true\n",
            ),
            (
                "hidden.desktop",
                "[Desktop Entry]\nType=Application\nName=Hidden B\nExec=b\nHidden=true\n",
            ),
        ]);
        let index = DesktopIndex::scan_paths(std::iter::once(apps.clone()));
        std::fs::remove_dir_all(apps.parent().unwrap()).ok();

        assert_eq!(index.names(), vec!["Shown"]);
    }

    #[test]
    fn strips_field_codes() {
        assert_eq!(strip_field_codes("app %f"), "app");
        assert_eq!(strip_field_codes("app %F --flag %u"), "app --flag");
        assert_eq!(
            strip_field_codes("app --pct=100%% run"),
            "app --pct=100% run"
        );
        assert_eq!(strip_field_codes("app"), "app");
    }
}

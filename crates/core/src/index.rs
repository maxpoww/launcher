//! Index of launchable applications discovered from `.desktop` entries.
//!
//! P1–P3 only need the *types* so the daemon can be wired end-to-end;
//! the actual scan (via the `freedesktop-desktop-entry` crate, with
//! locale handling and a persistent cache) is Phase 4 work — see
//! IMPLEMENTATION_PLAN.md. Indexing runs on a background thread in the
//! daemon and sends a finished `DesktopIndex` over a channel.

/// One launchable application, extracted from a `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    /// Display name (localized `Name=`).
    pub name: String,
    /// Optional `GenericName=`/`Comment=`, shown as a subtitle and used as
    /// a secondary match target.
    pub description: Option<String>,
    /// The `Exec=` line with field codes (`%f`, `%u`, …) already stripped.
    pub exec: String,
    /// Icon name for lookup via `freedesktop-icons` (Phase 5).
    pub icon: Option<String>,
    /// Whether `Terminal=true` was set (needs a terminal emulator wrapper).
    pub needs_terminal: bool,
}

/// The full application index the search UI ranks against.
#[derive(Debug, Clone, Default)]
pub struct DesktopIndex {
    /// All discovered entries, deduplicated by desktop-file ID.
    pub entries: Vec<AppEntry>,
}

impl DesktopIndex {
    /// Scan XDG data dirs for `.desktop` files and build the index.
    ///
    /// Phase 4: currently returns an empty index so the daemon skeleton
    /// links and runs; replaced by a real `freedesktop-desktop-entry`
    /// scan when P4 starts.
    pub fn scan() -> Self {
        Self::default()
    }

    /// Names of all entries, in index order — the haystack handed to
    /// [`crate::Searcher::search`].
    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }
}

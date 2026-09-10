//! Uninstall residue sweep (#60): when an app is uninstalled through the
//! dock, the user-level droppings it left — config, cache, data, state
//! dirs — follow it into the Recycle Bin. Without this, a machine
//! accumulates the graveyard of every app ever tried (measured on the dev
//! box, 2026-09-10: one day's dogfood churn left 9 orphan dirs / ~112 MB;
//! five years leaves hundreds). "A working system after 5 years" (Max)
//! means an uninstall leaves NOTHING behind.
//!
//! To the TRASH, never `rm`: residue can be real user data (a browser
//! profile, a mail store). Trashing gets it out of the system while the
//! Recycle Bin keeps it restorable — the same consent model as dragging
//! the app out. The FreeDesktop trash move is a same-filesystem rename:
//! instant even for a multi-GB profile.
//!
//! Matching is conservative: exact (case-insensitive) directory-name
//! matches against names DERIVED from the uninstalled package (attr,
//! desktop ids and their reverse-DNS tails), plus a curated table for the
//! apps whose dirs share nothing with their name (brave → BraveSoftware).
//! Short names (< 4 chars) never match, and the desktop's own plumbing is
//! protected outright.

use std::path::PathBuf;

use tracing::{info, warn};

/// Apps whose residue names the heuristic cannot derive. Names starting
/// with `.` live directly under `$HOME`; the rest are matched inside the
/// XDG bases like derived candidates. Extend as the map machines teach us.
const CURATED: &[(&str, &[&str])] = &[
    ("brave", &["BraveSoftware"]),
    ("firefox", &[".mozilla"]),
    ("thunderbird", &[".thunderbird"]),
    ("vscode", &["Code", ".vscode"]),
    ("vscodium", &["VSCodium", ".vscode-oss"]),
    ("telegram-desktop", &["TelegramDesktop"]),
    ("signal-desktop", &["Signal"]),
    ("google-chrome", &["google-chrome"]),
    ("obsidian", &["obsidian", ".obsidian"]),
];

/// Directory names that are NEVER residue, whatever matches them: the
/// desktop's own plumbing and shared infrastructure.
const PROTECTED: &[&str] = &[
    "waverunner",
    "systemd",
    "nix",
    "hypr",
    "hyprland",
    "dconf",
    "pulse",
    "pipewire",
    "wireplumber",
    "fontconfig",
    "gtk-3.0",
    "gtk-4.0",
    "qt5ct",
    "qt6ct",
    "environment.d",
    "autostart",
    "mime",
    "applications",
    "icons",
    "themes",
    "fonts",
    "Trash",
];

/// Minimum length for a derived candidate — a 2–3 letter name ("go", "sh")
/// would false-match half a home dir.
const MIN_LEN: usize = 4;

/// The XDG bases residue lives in.
fn bases() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let config = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".config"));
    let cache = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".cache"));
    let data = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".local/share"));
    let state = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".local/state"));
    vec![config, cache, data, state]
}

/// Candidate residue directory names for an uninstalled package: the attr,
/// its last dotted segment (`kdePackages.kdenlive` → `kdenlive`), every
/// desktop id and each id's reverse-DNS tail (`org.kde.krita` → `krita`),
/// plus the curated names. Lowercased; protected and too-short names are
/// dropped. Curated `.dot` names pass through as-is (matched under $HOME).
fn candidates(attr: &str, app_ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let name = s.to_lowercase();
        let bare = name.trim_start_matches('.');
        if bare.len() < MIN_LEN {
            return;
        }
        if PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(bare)) {
            return;
        }
        if !out.contains(&name) {
            out.push(name);
        }
    };
    push(attr);
    if let Some(tail) = attr.rsplit('.').next() {
        push(tail);
    }
    for id in app_ids {
        push(id);
        if let Some(tail) = id.rsplit('.').next() {
            push(tail);
        }
    }
    for (pkg, names) in CURATED {
        if pkg.eq_ignore_ascii_case(attr)
            || app_ids.iter().any(|i| i.eq_ignore_ascii_case(pkg))
        {
            for n in *names {
                push(n);
            }
        }
    }
    out
}

/// Sweep an uninstalled package's residue into the Recycle Bin. Called on
/// a CONFIRMED uninstall (the rebuild landed and the package is gone).
/// Best-effort: every failure is logged and skipped, never fatal.
pub fn sweep(attr: &str, app_ids: &[String]) {
    let cands = candidates(attr, app_ids);
    if cands.is_empty() {
        return;
    }
    let trash = crate::trash::Trash::home();
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let mut hits: Vec<PathBuf> = Vec::new();
    for base in bases() {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if cands.iter().any(|c| c.eq_ignore_ascii_case(name)) {
                hits.push(entry.path());
            }
        }
    }
    // Curated `.dot` homes ($HOME/.mozilla and friends) — exact only.
    for c in &cands {
        if c.starts_with('.') {
            let p = home.join(c);
            if p.exists() {
                hits.push(p);
            }
        }
    }
    for path in hits {
        match trash.trash(&path) {
            Ok(item) => info!(
                "residue of {attr}: {} → Recycle Bin ({})",
                path.display(),
                item.display_name()
            ),
            Err(e) => warn!("residue of {attr}: cannot trash {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn derives_tails_and_filters() {
        let c = candidates("kdePackages.kdenlive", &s(&["org.kde.kdenlive"]));
        assert!(c.contains(&"kdenlive".to_string()));
        // Full reverse-DNS id also allowed (some apps use it as dir name).
        assert!(c.contains(&"org.kde.kdenlive".to_string()));
        // The attr's package-set prefix never leaks in as its own candidate.
        assert!(!c.contains(&"kdepackages".to_string()) || c.contains(&"kdepackages.kdenlive".to_string()));
    }

    #[test]
    fn curated_and_protected() {
        let c = candidates("brave", &s(&["brave-browser"]));
        assert!(c.contains(&"bravesoftware".to_string()));
        let c = candidates("firefox", &s(&["firefox"]));
        assert!(c.contains(&".mozilla".to_string()));
        // Protected names never become candidates even on exact match.
        let c = candidates("hyprland", &s(&["hyprland"]));
        assert!(c.is_empty());
        // Short names never match anything.
        let c = candidates("go", &s(&["go"]));
        assert!(c.is_empty());
    }
}

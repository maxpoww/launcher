//! THE LOOK: light or dark (Max, 2026-10-14: "make them darker when dark mode
//! is on… the 3 icons on the dock should follow… and the dark light mode of
//! files"). One flag for the whole daemon, set at start from the owner's
//! choice (`controls::dark_now`) and by the control panel's Dark mode.
//!
//! What follows it:
//! - the ICON THEME: Golem's icons come in two tones, `Golem` (silver) and
//!   `Golem-Dark` (graphite); every `IconLoader` asks [`icon_theme`] which to
//!   walk and rebuilds itself when the [`generation`] moves, so a rescan or a
//!   new request after a change is in the new tone;
//! - the dock's own three figures (apps, control panel, bin): [`figure`];
//! - Golem Files' window: its ground, ink and washes (`filer.rs`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

static DARK: AtomicBool = AtomicBool::new(true);
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Whether the desktop is in its dark look.
pub(crate) fn dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

/// Set the look; whether it changed.
pub(crate) fn set_dark(dark: bool) -> bool {
    let changed = DARK.swap(dark, Ordering::Relaxed) != dark;
    if changed {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    changed
}

/// Moves on every change of the look: what was drawn for the old one is
/// stale.
pub(crate) fn generation() -> u32 {
    GENERATION.load(Ordering::Relaxed)
}

/// The icon theme to walk for the configured one in the present look: a
/// theme that has a `-Dark` sibling installed wears it in the dark, and a
/// `-Dark` theme gives way to its light self in the light.
pub(crate) fn icon_theme(configured: &str) -> String {
    let base = configured.strip_suffix("-Dark").unwrap_or(configured);
    let want = if dark() { format!("{base}-Dark") } else { base.to_owned() };
    if want == configured || installed(&want) {
        want
    } else {
        configured.to_owned()
    }
}

/// Whether an icon theme of that name is installed anywhere on the data path.
pub(crate) fn installed(theme: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let data_home = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{home}/.local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    std::iter::once(data_home)
        .chain(data_dirs.split(':').map(str::to_owned))
        .any(|d| std::path::Path::new(&d).join("icons").join(theme).join("index.theme").is_file())
}

/// One of the dock's own figures in the present look. They are cast SILVER;
/// in the dark they are the same figure in GRAPHITE, as Golem's icons are
/// (the cast's stops and the gloss and bevel lights brought down).
pub(crate) fn figure(svg: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !dark() {
        return std::borrow::Cow::Borrowed(svg);
    }
    let mut s = String::from_utf8_lossy(svg).into_owned();
    for (from, to) in [
        ("#fefefe", "#9a9fa8"),
        ("#fafafa", "#8d929b"),
        ("#eeeeee", "#757a83"),
        ("#e6e6e6", "#666a73"),
        ("stop-opacity=\"0.22\"", "stop-opacity=\"0.12\""),
        ("flood-color=\"#fff\" flood-opacity=\"0.5\"", "flood-color=\"#fff\" flood-opacity=\"0.25\""),
    ] {
        s = s.replace(from, to);
    }
    std::borrow::Cow::Owned(s.into_bytes())
}

impl crate::App {
    /// The control panel changed the look: everything that wears one is
    /// asked for again in the new tone — the dock's icons (a rescan: every
    /// loader builds itself again, the three figures are drawn anew), the
    /// desktop's and every file window's (their pictures dropped, so they
    /// ask again), and the file windows' own colours.
    pub(crate) fn look_set(&mut self, dark: bool) {
        if !set_dark(dark) {
            return;
        }
        tracing::info!("look: {}", if dark { "dark" } else { "light" });
        self.indexer.request_rescan_fresh();
        self.desktop_look_changed();
        self.filer_look_changed();
        // The palette's other variant, in the shell and the apps.
        self.colors_apply(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test: the look is one flag for the whole process, and tests run
    // side by side.
    #[test]
    fn the_look_recolours_figures_and_picks_the_theme() {
        let silver = include_bytes!("../assets/dock/apps.svg");
        set_dark(false);
        assert_eq!(&*figure(silver), &silver[..]);
        assert_eq!(icon_theme("no-such-theme-anywhere-Dark"), "no-such-theme-anywhere-Dark");
        let before = generation();
        set_dark(true);
        assert_eq!(generation(), before + 1, "a change moves the generation");
        assert!(!set_dark(true), "setting it again is no change");
        let graphite = figure(silver);
        let text = String::from_utf8_lossy(&graphite);
        assert!(!text.contains("#fefefe") && text.contains("#9a9fa8"), "every cast stop is graphite");
        assert_eq!(icon_theme("no-such-theme-anywhere"), "no-such-theme-anywhere");
    }
}

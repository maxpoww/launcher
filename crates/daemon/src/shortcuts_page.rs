//! THE SHORTCUTS PAGE (Max, 2026-11-06, the Controls gap audit: "Golem lives
//! on Super shortcuts and there is no way to discover them"): Controls →
//! SHORTCUTS, the keys Golem answers to, each with what it does, in the
//! panel's word-row idiom (the keys at the right of their words, as the
//! Launcher page's choices stand at the right of their names). A list to
//! READ: nothing here is picked or changed (the binds are Golem's own,
//! `system/home/hyprland.lua`; changing them is another day's page).
//!
//! The rows are a table of ours, not read from the compositor: its `binds`
//! answer names every Golem bind `__lua` with a number, no words. The table
//! follows the lua file; a bind added there is added here.

use crate::controls::{line_of, ControlView, Group};
use crate::i18n::tr;

/// What a shortcut does and its keys, under a heading (a `None` what).
/// The keys are written as they are read aloud, Super first. The essentials
/// only: the panel does not scroll, and a line a shortcut is what fits.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Golem", ""),
    ("Open the dock's box", "Super (tap)"),
    ("Quick controls", "Super + H"),
    ("Terminal", "Super + E"),
    ("Files", "Super + F"),
    ("Voice to text (hold to dictate)", "Super + Space"),
    ("The card: clipboard · emoji · memory", "Super + . , /"),
    ("Screenshot of a region", "Print"),
    ("Lock the screen", "Super + L"),
    ("Windows", ""),
    ("Close the window", "Super + Q"),
    ("Float or tile the window", "Super + Z"),
    ("Overview of every window", "Super + R"),
    ("Next window", "Super + Tab"),
    ("Move the focus", "Super + W A S D"),
    ("Workspaces", ""),
    ("Go to a workspace", "Super + 1…9"),
    ("Send the window to a workspace", "Super + Shift + 1…9"),
];

/// The page: a heading is a centred line, a shortcut a line of its own —
/// the keys, then what they do (nothing is picked here).
pub(crate) fn view(view: &mut ControlView) {
    view.subtitle = tr("What the keys do. Super is the key with the logo.").to_owned();
    for (what, keys) in SHORTCUTS {
        if keys.is_empty() {
            view.groups.push(Group { centred: true, ..line_of(tr(what).to_owned()) });
        } else {
            view.groups.push(line_of(format!("{keys}  ·  {}", tr(what))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_has_words_and_the_headings_none() {
        let mut headings = 0;
        for (what, keys) in SHORTCUTS {
            assert!(!what.is_empty());
            if keys.is_empty() {
                headings += 1;
            } else {
                assert!(keys.contains("Super") || keys == &"Print", "{what}: {keys}");
            }
        }
        assert_eq!(headings, 3);
        let mut v = ControlView { label: "Shortcuts", ..Default::default() };
        view(&mut v);
        assert_eq!(v.groups.len(), SHORTCUTS.len());
        assert!(v.groups.iter().all(|g| g.choices.is_empty()), "nothing to pick on this page");
        assert_eq!(v.groups.iter().filter(|g| g.centred).count(), headings);
    }
}

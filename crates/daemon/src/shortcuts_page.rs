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

use crate::controls::{group_of, line_of, Choice, ControlView, Group};
use crate::i18n::tr;

/// What a shortcut does and its keys, under a heading (a `None` what).
/// The keys are written as they are read aloud, Super first.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Golem", ""),
    ("Open the dock's box", "Super (tap)"),
    ("Control panel", "Super + Ctrl"),
    ("Quick controls", "Super + H"),
    ("Fast launch an app", "Super + J"),
    ("Terminal", "Super + E"),
    ("Files", "Super + F"),
    ("Voice to text (hold to dictate)", "Super + Space"),
    ("The card: clipboard · emoji · memory", "Super + . , /"),
    ("Screenshot of a region", "Print"),
    ("Lock the screen", "Super + L"),
    ("Sleep", "Super + Escape"),
    ("Windows", ""),
    ("Close the window", "Super + Q"),
    ("Float or tile the window", "Super + Z"),
    ("Stage mode", "Super + Enter"),
    ("Overview of every window", "Super + R"),
    ("Next window", "Super + Tab"),
    ("Move the focus", "Super + W A S D"),
    ("Move the window", "Super + Shift + W A S D"),
    ("Resize the window", "Super + arrows"),
    ("Drag a window", "Super + mouse"),
    ("Workspaces", ""),
    ("Go to a workspace", "Super + 1…9"),
    ("Send the window to a workspace", "Super + Shift + 1…9"),
];

/// The page: a heading is a line, a shortcut a word row with its keys at
/// the right (the row's one choice, never lit: nothing is picked here).
pub(crate) fn view(view: &mut ControlView) {
    view.subtitle = tr("What the keys do. Super is the key with the logo.").to_owned();
    for (what, keys) in SHORTCUTS {
        if keys.is_empty() {
            view.groups.push(Group { centred: true, ..line_of(tr(what).to_owned()) });
        } else {
            let key = Choice { text: (*keys).to_owned(), on: false, value: String::new(), swatch: Vec::new(), caption: false, icon: None, hint: None };
            view.groups.push(Group { labelled: true, ..group_of(Some(what), "shortcut", vec![key]) });
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
        assert!(v.groups.iter().filter(|g| g.labelled).all(|g| g.choices.len() == 1 && !g.choices[0].on));
    }
}

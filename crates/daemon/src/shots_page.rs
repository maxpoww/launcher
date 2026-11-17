//! Controls → SCREENSHOTS (Max, 2026-11-16, the same night Print became a
//! whole-screen shot with its preview on the card's pill): what the Print key
//! takes, what is in the picture, and what becomes of a shot.
//!
//! The page is in the idiom the others set (`controls.rs`, `panel.rs`): a
//! WORD ROW for the one choice of several, THE KIT's switch for each on/off
//! (the hint under a switch for what it costs), a caption over a set of
//! switches. It does not scroll: it has to fit a 900-pixel screen with the
//! Dock's hint out, so the films' one choice is a word row, not a set —
//!
//! - Print takes: The whole screen · Let me choose first. The second makes a
//!   TAP of Print open the live tool (`capture.rs`) instead of shooting; held,
//!   the key still records.
//! - In the picture: Banner, Dock, Pointer. Banner off: the banner's strip is
//!   CUT OFF a whole-screen shot, in pixels, before the file and the clipboard
//!   (`capture_shot::strip_px`); the editor then opens on the cut picture.
//!   Dock off: the dock is drawn empty for the shot and comes back after — the
//!   shot is taken a moment later, and the switch says so while it is off.
//!   Pointer: `grim -c`. A region or a window taken with the live tool is what
//!   was chosen, as it is: these three are for whole-screen pictures.
//! - After a shot: Copy to the clipboard, Show the preview. With no preview
//!   the shot says itself with the ordinary notification that carries the
//!   picture and opens it: a shot is never silent.
//! - Screen recording: Silent · With sound — what the held key and
//!   Ctrl+Print record with, and what the live tool's Sound pill starts from.
//!
//! The choices are `screenshots` in `~/.config/golem/settings.json`
//! ([`ShotChoice`]; absent keys are the defaults, and the defaults are not
//! written), saved by the settings' worker and read where they are used
//! ([`ShotChoice::now`]: the file is read only when it has changed).
//!
//! Pointer-free: `waverunner-ctl display show screenshots`, `display pick
//! <group> <n>` (groups from 0: Print takes · — · Banner · Dock · Pointer · —
//! · clipboard · preview · Screen recording; a switch's 0 is on, its 1 off;
//! Screen recording's 0 is Silent, its 1 With sound).

use serde::{Deserialize, Serialize};

use crate::controls::{choice_of, group_of, line_of, on_off_of, ControlView, Group};
use crate::display::GolemSettings;
use crate::i18n::tr;

/// The screenshots' choices, as `settings.json` keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ShotChoice {
    /// A tap of Print opens the live tool to choose first (else it takes the
    /// whole screen at once).
    pub choose_first: bool,
    /// IN THE PICTURE: the banner (else its strip is cut off a whole-screen
    /// shot), the dock (else it is taken off the screen for the shot), the
    /// pointer.
    pub banner: bool,
    pub dock: bool,
    pub pointer: bool,
    /// AFTER A SHOT: onto the clipboard, and its preview on the card's pill.
    pub clipboard: bool,
    pub preview: bool,
    /// FILMS: with their sound.
    pub sound: bool,
}

impl Default for ShotChoice {
    fn default() -> Self {
        Self {
            choose_first: false,
            banner: true,
            dock: true,
            pointer: false,
            clipboard: true,
            preview: true,
            sound: false,
        }
    }
}

impl ShotChoice {
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The choices as the file has them now.
    pub(crate) fn now() -> Self {
        GolemSettings::load().screenshots
    }

    /// Change them and save.
    pub(crate) fn change(f: impl FnOnce(&mut Self)) {
        GolemSettings::change(|s| {
            f(&mut s.screenshots);
            s.store();
        });
    }

    /// A pick of the page: `key` set to `value`. False when the key is not
    /// one of the page's (nothing is changed then).
    pub(crate) fn set(&mut self, key: &str, value: &str) -> bool {
        let on = value == "on";
        match key {
            "shots-print" => self.choose_first = value == "choose",
            "shots-banner" => self.banner = on,
            "shots-dock" => self.dock = on,
            "shots-pointer" => self.pointer = on,
            "shots-clipboard" => self.clipboard = on,
            "shots-preview" => self.preview = on,
            "shots-sound" => self.sound = on,
            _ => return false,
        }
        true
    }
}

/// The page (`controls::read("Screenshots")`, on the settings' worker).
pub(crate) fn view(view: &mut ControlView) {
    view_of(view, ShotChoice::now());
}

/// The page for choices `c`.
fn view_of(view: &mut ControlView, c: ShotChoice) {
    view.subtitle = tr("What the Print key takes and what happens next.").to_owned();
    let takes = vec![
        choice_of(tr("The whole screen"), !c.choose_first, "screen"),
        choice_of(tr("Let me choose first"), c.choose_first, "choose"),
    ];
    view.groups.push(Group {
        labelled: true,
        ..group_of(Some("Print takes"), "shots-print", takes)
    });
    view.groups.push(line_of(tr("In the picture").to_owned()));
    view.groups
        .push(on_off_of(Some("Banner"), "shots-banner", c.banner));
    let mut dock = on_off_of(Some("Dock"), "shots-dock", c.dock);
    // (Taking the dock off the screen takes a moment: said while it is off,
    // and only then.)
    if !c.dock {
        if let Some(on) = dock.choices.first_mut() {
            on.hint = Some(tr("The shot is taken a moment later.").to_owned());
        }
    }
    view.groups.push(dock);
    view.groups
        .push(on_off_of(Some("Pointer"), "shots-pointer", c.pointer));
    view.groups.push(line_of(tr("After a shot").to_owned()));
    view.groups.push(on_off_of(
        Some("Copy to the clipboard"),
        "shots-clipboard",
        c.clipboard,
    ));
    view.groups.push(on_off_of(
        Some("Show the preview"),
        "shots-preview",
        c.preview,
    ));
    // (A word row: one line, the films' one choice.)
    let sound = vec![
        choice_of(tr("Silent"), !c.sound, "off"),
        choice_of(tr("With sound"), c.sound, "on"),
    ];
    view.groups.push(Group {
        labelled: true,
        ..group_of(Some("Screen recording"), "shots-sound", sound)
    });
}

/// A pick on the worker. True when the key was this page's.
pub(crate) fn apply(key: &str, value: &str) -> bool {
    // (Asked of a copy first: a key of another page changes no file.)
    if !ShotChoice::default().set(key, value) {
        return false;
    }
    ShotChoice::change(|c| {
        c.set(key, value);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_keys_are_the_defaults_and_the_defaults_are_not_written() {
        let d = ShotChoice::default();
        assert!(
            !d.choose_first
                && d.banner
                && d.dock
                && !d.pointer
                && d.clipboard
                && d.preview
                && !d.sound
        );
        // A file that has never heard of screenshots.
        let s: GolemSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.screenshots, d);
        let s: GolemSettings =
            serde_json::from_str(r#"{"dark":true,"natural_scroll":false}"#).unwrap();
        assert_eq!(s.screenshots, d);
        assert!(
            !serde_json::to_string(&s).unwrap().contains("screenshots"),
            "the defaults are not written"
        );
        // Some keys only: the others keep their defaults.
        let s: GolemSettings =
            serde_json::from_str(r#"{"screenshots":{"dock":false,"sound":true}}"#).unwrap();
        assert_eq!(
            s.screenshots,
            ShotChoice {
                dock: false,
                sound: true,
                ..d
            }
        );
        // An empty object too.
        let s: GolemSettings = serde_json::from_str(r#"{"screenshots":{}}"#).unwrap();
        assert_eq!(s.screenshots, d);
    }

    #[test]
    fn the_choices_come_back_as_they_were_saved_with_what_else_the_file_holds() {
        let all = ShotChoice {
            choose_first: true,
            banner: false,
            dock: false,
            pointer: true,
            clipboard: false,
            preview: false,
            sound: true,
        };
        let mut s: GolemSettings =
            serde_json::from_str(r#"{"dark":true,"somebody_elses":{"a":1}}"#).unwrap();
        s.screenshots = all;
        let text = serde_json::to_string(&s).unwrap();
        let back: GolemSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.screenshots, all);
        assert_eq!(back.dark, Some(true));
        assert!(
            back.other.contains_key("somebody_elses"),
            "what it does not know is carried through"
        );
        assert_eq!(back, s);
        // One at a time, each alone.
        for (key, value, want) in [
            (
                "shots-print",
                "choose",
                ShotChoice {
                    choose_first: true,
                    ..ShotChoice::default()
                },
            ),
            (
                "shots-banner",
                "off",
                ShotChoice {
                    banner: false,
                    ..ShotChoice::default()
                },
            ),
            (
                "shots-dock",
                "off",
                ShotChoice {
                    dock: false,
                    ..ShotChoice::default()
                },
            ),
            (
                "shots-pointer",
                "on",
                ShotChoice {
                    pointer: true,
                    ..ShotChoice::default()
                },
            ),
            (
                "shots-clipboard",
                "off",
                ShotChoice {
                    clipboard: false,
                    ..ShotChoice::default()
                },
            ),
            (
                "shots-preview",
                "off",
                ShotChoice {
                    preview: false,
                    ..ShotChoice::default()
                },
            ),
            (
                "shots-sound",
                "on",
                ShotChoice {
                    sound: true,
                    ..ShotChoice::default()
                },
            ),
        ] {
            let mut c = ShotChoice::default();
            assert!(c.set(key, value), "{key}");
            assert_eq!(c, want, "{key} = {value}");
            let mut s = GolemSettings {
                screenshots: c,
                ..GolemSettings::default()
            };
            let back: GolemSettings =
                serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
            assert_eq!(back.screenshots, want, "{key} through the file");
            // …and set back, the file forgets it.
            s.screenshots = ShotChoice::default();
            assert!(!serde_json::to_string(&s).unwrap().contains("screenshots"));
        }
        // Print takes the whole screen again.
        let mut c = ShotChoice {
            choose_first: true,
            ..ShotChoice::default()
        };
        assert!(c.set("shots-print", "screen"));
        assert_eq!(c, ShotChoice::default());
        // A key of another page is not this one's, and changes nothing.
        let mut c = all;
        assert!(!c.set("launcher-away", "on") && !c.set("shots", "on") && !c.set("", ""));
        assert_eq!(c, all);
    }

    #[test]
    fn the_page_is_a_word_row_then_switches_under_their_captions() {
        let mut v = ControlView::default();
        view_of(&mut v, ShotChoice::default());
        let keys: Vec<&str> = v.groups.iter().map(|g| g.key).collect();
        assert_eq!(
            keys,
            [
                "shots-print",
                "",
                "shots-banner",
                "shots-dock",
                "shots-pointer",
                "",
                "shots-clipboard",
                "shots-preview",
                "shots-sound"
            ]
        );
        // Print takes: one of two words, the whole screen lit.
        let print = &v.groups[0];
        assert!(print.labelled && print.choices.len() == 2);
        assert_eq!(
            print
                .choices
                .iter()
                .map(|c| (c.value.as_str(), c.on))
                .collect::<Vec<_>>(),
            [("screen", true), ("choose", false)]
        );
        // The captions are lines with nothing to pick.
        for (i, words) in [(1, "In the picture"), (5, "After a shot")] {
            assert!(v.groups[i].choices.is_empty());
            assert_eq!(v.groups[i].caption.as_deref(), Some(words));
        }
        // Every other row is a switch, on or off as the choice is.
        let on = |i: usize| {
            let g = &v.groups[i];
            assert_eq!(
                g.choices
                    .iter()
                    .map(|c| c.value.as_str())
                    .collect::<Vec<_>>(),
                ["on", "off"],
                "{}",
                g.key
            );
            g.choices[0].on
        };
        // The films: a word row, Silent lit (not a switch: its first is off).
        let films = &v.groups[8];
        assert!(films.labelled);
        assert_eq!(
            films
                .choices
                .iter()
                .map(|c| (c.value.as_str(), c.on))
                .collect::<Vec<_>>(),
            [("off", true), ("on", false)]
        );
        assert_eq!(
            [on(2), on(3), on(4), on(6), on(7)],
            [true, true, false, true, true]
        );
        assert!(!v.subtitle.is_empty());
    }

    #[test]
    fn the_dock_says_the_shot_comes_a_moment_later_only_while_it_is_off() {
        let hint = |c: ShotChoice| {
            let mut v = ControlView::default();
            view_of(&mut v, c);
            let hints: Vec<(&str, String)> = v
                .groups
                .iter()
                .filter_map(|g| {
                    g.choices
                        .first()
                        .and_then(|c| c.hint.clone())
                        .map(|h| (g.key, h))
                })
                .collect();
            hints
        };
        assert!(
            hint(ShotChoice::default()).is_empty(),
            "nothing to say while the dock is in the picture"
        );
        let off = hint(ShotChoice {
            dock: false,
            ..ShotChoice::default()
        });
        assert_eq!(
            off,
            [("shots-dock", "The shot is taken a moment later.".to_owned())]
        );
        // (No other switch has one, whatever is chosen.)
        let all_off = hint(ShotChoice {
            choose_first: true,
            banner: false,
            dock: true,
            pointer: true,
            clipboard: false,
            preview: false,
            sound: true,
        });
        assert!(all_off.is_empty());
    }
}

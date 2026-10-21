//! THE LAUNCHER PAGE (Max, 2026-10-20; the lab
//! https://claude.ai/artifact/XoqB8GaLMKp76AeFCA9eyc): the control panel's
//! setting for the dock and its box. Five rows — Stay away, Animated icons,
//! Position, Arrange apps, Other monitors — and a line that says how the
//! icons are made bigger (Ctrl +/− with the launcher open: there is no row
//! for it).
//!
//! The choices are the DOCK's own (`settings.json` in the data dir,
//! [`crate::settings::Settings`]): the page is read on the controls' worker
//! from the file ([`crate::controls::read`]), a pick is made here on the UI
//! thread ([`App::launcher_pick`]), saved, and the page read again.
//!
//! THE PAGE FIRST, THE FEATURES LATER (Max, 2026-10-20: "we will work on the
//! features later, first do the controls box"): every choice is saved and
//! shown, and none acts on the dock yet. A first cut of Stay away, Animated
//! icons and Arrange apps waits on the branch `launcher-features-wip`.
//!
//! THE BUILT-IN SCREEN ALWAYS HAS THE LAUNCHER, and the MAIN one when the
//! built-in is off (a closed lid, a desktop with none). That is done, not
//! said: the page lists only the OTHER monitors, each by its name and where
//! it stands from that one ("DELL U2720Q (above)").

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::display::Monitor;
use crate::App;

/// The edge of the screen the dock stands on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Side {
    Left,
    #[default]
    Bottom,
    Right,
}

impl Side {
    pub(crate) const ALL: [Side; 3] = [Side::Left, Side::Bottom, Side::Right];

    /// What a pick sends, and what the page says.
    pub(crate) fn value(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Bottom => "bottom",
            Side::Right => "right",
        }
    }

    pub(crate) fn word(self) -> &'static str {
        match self {
            Side::Left => "Left",
            Side::Bottom => "Bottom",
            Side::Right => "Right",
        }
    }

    fn of(value: &str) -> Option<Side> {
        Side::ALL.into_iter().find(|s| s.value() == value)
    }
}

/// How the Apps grid is ordered: by use (most used first), by name, or as
/// the owner put it by hand (the grid's own paged order, `order.rs`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Arrange {
    Use,
    Name,
    #[default]
    Hand,
}

impl Arrange {
    pub(crate) const ALL: [Arrange; 3] = [Arrange::Use, Arrange::Name, Arrange::Hand];

    pub(crate) fn value(self) -> &'static str {
        match self {
            Arrange::Use => "use",
            Arrange::Name => "name",
            Arrange::Hand => "hand",
        }
    }

    pub(crate) fn word(self) -> &'static str {
        match self {
            Arrange::Use => "By use",
            Arrange::Name => "By name",
            Arrange::Hand => "By hand",
        }
    }

    fn of(value: &str) -> Option<Arrange> {
        Arrange::ALL.into_iter().find(|a| a.value() == value)
    }
}

/// A laptop's own panel.
pub(crate) fn built_in(m: &Monitor) -> bool {
    ["eDP", "LVDS", "DSI"].iter().any(|p| m.name.starts_with(p))
}

/// The screen that always has the launcher: the built-in one, else the one
/// at the origin of the layout, else the first.
pub(crate) fn main_screen(screens: &[Monitor]) -> Option<usize> {
    screens
        .iter()
        .position(built_in)
        .or_else(|| screens.iter().position(|m| m.pos.0.abs() < 0.5 && m.pos.1.abs() < 0.5))
        .or(if screens.is_empty() { None } else { Some(0) })
}

/// A screen's rectangle among the screens, in logical pixels.
fn rect(m: &Monitor) -> (f64, f64, f64, f64) {
    let s = if m.scale > 0.0 { m.scale } else { 1.0 };
    (m.pos.0, m.pos.1, f64::from(m.mode.w) / s, f64::from(m.mode.h) / s)
}

/// Where `other` stands from `main`: wholly above or below it, else at its
/// left or right.
pub(crate) fn where_from(main: &Monitor, other: &Monitor) -> &'static str {
    let (mx, my, mw, mh) = rect(main);
    let (ox, oy, ow, oh) = rect(other);
    if oy + oh <= my + 1.0 {
        "above"
    } else if oy >= my + mh - 1.0 {
        "below"
    } else if ox + ow / 2.0 < mx + mw / 2.0 {
        "left"
    } else {
        "right"
    }
}

/// The OTHER monitors as the page lists them: (what it is known by, its
/// name, where it stands from the main screen), in the compositor's order.
pub(crate) fn others(screens: &[Monitor]) -> Vec<(String, String, &'static str)> {
    let Some(main) = main_screen(screens) else { return Vec::new() };
    screens
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != main)
        .map(|(_, m)| (m.selector(), m.title(), where_from(&screens[main], m)))
        .collect()
}

impl App {
    /// A pick on the Launcher page (`launcher-*` keys, see `controls::read`):
    /// saved, nothing more yet.
    pub(crate) fn launcher_pick(&mut self, key: &str, value: &str) {
        match key {
            "launcher-away" => {
                self.settings.launcher_away = value == "on";
                self.settings.save();
            }
            "launcher-motion" => {
                self.settings.launcher_still = value == "off";
                self.settings.save();
            }
            "launcher-side" => {
                if let Some(side) = Side::of(value) {
                    self.settings.launcher_side = side;
                    self.settings.save();
                }
            }
            "launcher-arrange" => {
                if let Some(arrange) = Arrange::of(value) {
                    self.settings.launcher_arrange = arrange;
                    self.settings.save();
                }
            }
            // A monitor's word: the launcher on it goes, or comes back.
            "launcher-screen" => {
                let off = &mut self.settings.launcher_screens_off;
                if let Some(i) = off.iter().position(|s| s == value) {
                    off.remove(i);
                } else {
                    off.push(value.to_owned());
                }
                self.settings.save();
            }
            _ => return,
        }
        info!("launcher: {key} = {value}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::Mode;

    fn screen(name: &str, model: &str, pos: (f64, f64), w: u32, h: u32, scale: f64) -> Monitor {
        let mode = Mode { w, h, hz: 60.0 };
        Monitor {
            name: name.to_owned(),
            desc: format!("Maker {model} 0001"),
            model: model.to_owned(),
            mode,
            scale,
            modes: vec![mode],
            focused: false,
            pos,
        }
    }

    #[test]
    fn the_built_in_screen_is_main_and_not_listed() {
        let screens = [
            screen("DP-1", "U2720Q", (0.0, -1440.0), 2560, 1440, 1.0),
            screen("eDP-1", "", (0.0, 0.0), 2880, 1800, 2.0),
            screen("HDMI-A-1", "LG", (-1920.0, 0.0), 1920, 1080, 1.0),
        ];
        assert_eq!(main_screen(&screens), Some(1));
        let listed = others(&screens);
        let names: Vec<(&str, &str)> = listed.iter().map(|(_, n, w)| (n.as_str(), *w)).collect();
        assert_eq!(names, [("U2720Q", "above"), ("LG", "left")]);
        assert_eq!(listed[0].0, "desc:Maker U2720Q 0001");
    }

    #[test]
    fn with_the_built_in_off_the_screen_at_the_origin_is_main() {
        let screens = [
            screen("DP-2", "B", (1920.0, 0.0), 1920, 1080, 1.0),
            screen("DP-1", "A", (0.0, 0.0), 1920, 1080, 1.0),
        ];
        assert_eq!(main_screen(&screens), Some(1));
        let listed = others(&screens);
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].1.as_str(), listed[0].2), ("B", "right"));
    }

    #[test]
    fn one_screen_has_no_others() {
        assert!(others(&[screen("eDP-1", "", (0.0, 0.0), 1920, 1200, 1.0)]).is_empty());
        assert!(others(&[]).is_empty());
    }

    #[test]
    fn a_screen_below_is_below() {
        let main = screen("eDP-1", "", (0.0, 0.0), 2880, 1800, 2.0);
        assert_eq!(where_from(&main, &screen("DP-1", "X", (0.0, 900.0), 1920, 1080, 1.0)), "below");
        assert_eq!(where_from(&main, &screen("DP-1", "X", (1440.0, 0.0), 1920, 1080, 1.0)), "right");
    }
}

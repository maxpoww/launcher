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
//! THE PAGE FIRST, THE FEATURES ONE BY ONE (Max, 2026-10-20: "we will work
//! on the features later, first do the controls box"). In, each shown live
//! where it is seen: Position (the side dock, `content::layout_side`), Stay
//! away (`intellihide`), Animated icons (`icons_move`; turned on, the icons
//! play their turn — `icons_wave`), Arrange apps (`arrange_shown`; the dock
//! bounces, `dock_bounce`) and Other monitors (the dock follows the call to
//! a chosen screen, `dock_screen.rs`; a screen turned on shows its edge for
//! a beat). EVERY CHANGE KEEPS THE DOCK AND THE BOX ON (Max, 2026-10-21).
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
    /// The screen's edge the dock stands on.
    pub(crate) fn dock_side(&self) -> Side {
        self.settings.launcher_side
    }

    /// Whether the dock parks itself in view while nothing covers its place
    /// (intellihide): the config's, unless the owner asked it to STAY AWAY —
    /// then it is off the screen always and comes only when called (Super,
    /// or the pointer at its edge), leaving again as the pointer does.
    pub(crate) fn intellihide(&self) -> bool {
        self.config.input.intellihide && !self.settings.launcher_away
    }

    /// STAY AWAY turned on: the dock and the box it was picked in STAY
    /// (Max, 2026-10-21: "all changes should keep the dock and the box on");
    /// the dock goes when the box closes, and from then on stays away.
    /// Turned off, its place is judged again and a free one parks it.
    fn stay_away_changed(&mut self) {
        self.zone_free = false;
        if !self.settings.launcher_away {
            self.on_layout_changed();
        }
    }

    /// A pick on the Launcher page (`launcher-*` keys, see `controls::read`):
    /// saved, and each takes effect at once (shown where it is seen).
    pub(crate) fn launcher_pick(&mut self, key: &str, value: &str) {
        match key {
            "launcher-away" => {
                self.settings.launcher_away = value == "on";
                self.settings.save();
                self.stay_away_changed();
            }
            "launcher-motion" => {
                self.settings.launcher_still = value == "off";
                self.settings.save();
                // Turned on, the moving icons show it: each plays its turn.
                if value == "on" {
                    self.icons_wave();
                }
            }
            "launcher-side" => {
                if let Some(side) = Side::of(value) {
                    self.settings.launcher_side = side;
                    self.settings.save();
                    self.side_changed();
                }
            }
            "launcher-arrange" => {
                if let Some(arrange) = Arrange::of(value) {
                    self.settings.launcher_arrange = arrange;
                    self.settings.save();
                    // Nothing opens (Max, 2026-10-21: "dont need to show
                    // anything, just a bounce of the dock"): the grid takes
                    // it, the dock bounces, the box stays.
                    if self.arrange_shown != arrange {
                        self.arrange_shown = arrange;
                        self.refilter();
                    }
                    self.dock_bounce();
                }
            }
            // A monitor's word: the launcher on it goes, or comes back.
            "launcher-screen" => {
                let off = &mut self.settings.launcher_screens_off;
                let on = if let Some(i) = off.iter().position(|s| s == value) {
                    off.remove(i);
                    true
                } else {
                    off.push(value.to_owned());
                    false
                };
                self.settings.save();
                // Turned on, its edge shows for a beat on that screen; off,
                // it goes (and the dock, once it is not up there).
                if on {
                    self.screen_chosen(value);
                } else {
                    self.sync_edges();
                }
            }
            _ => return,
        }
        info!("launcher: {key} = {value}");
    }

    /// The whole dock bounces once: a setting it takes was changed (Arrange
    /// apps). See `dock_hop_offset`.
    pub(crate) fn dock_bounce(&mut self) {
        self.dock_hop = Some(std::time::Instant::now());
        self.dirty = true;
        self.schedule_frame();
    }

    /// The dock's moving icons move (Animated icons on).
    pub(crate) fn icons_move(&self) -> bool {
        !self.settings.launcher_still
    }

    /// ANIMATED ICONS turned on (Max, 2026-10-21: "when I turn on animations
    /// I want to see the icons move"): every moving icon on the dock plays
    /// its turn once, as a click plays it.
    fn icons_wave(&mut self) {
        self.compass.click();
        self.whatsapp.click();
        self.youtube.click();
        self.ytmusic.click();
        self.discord.click();
        self.spotify.click();
        self.google.click();
        self.chrome.click();
        self.files_hover.click();
        self.dirty = true;
        self.schedule_frame();
    }

    /// The Apps grid's cells (entry indices) sorted as `arrange` says: by
    /// use, most used first, ties by name; or by name. By hand: as they are.
    pub(crate) fn arrange_sorted(&self, cells: &mut [usize], arrange: Arrange) {
        let name = |i: usize| self.entries.get(i).map(|e| e.name.to_lowercase()).unwrap_or_default();
        match arrange {
            Arrange::Use => cells.sort_by_cached_key(|&i| {
                let uses = self.entries.get(i).map_or(0, |e| self.usage.count(&e.id));
                (std::cmp::Reverse(uses), name(i))
            }),
            Arrange::Name => cells.sort_by_cached_key(|&i| name(i)),
            Arrange::Hand => {}
        }
    }

    /// A drag in a grid arranged by use or by name: the grid as it is on
    /// screen becomes the order by hand, and Arrange apps turns to By hand.
    pub(crate) fn arrange_by_hand_from_view(&mut self) {
        let shown: Vec<String> = self.search.visible[crate::content::SECTION_APPS]
            .iter()
            .filter_map(|&i| self.entries.get(i).map(|e| e.id.clone()))
            .collect();
        self.order.take_order(&shown, self.apps_cap);
        self.settings.launcher_arrange = Arrange::Hand;
        self.settings.save();
        self.arrange_shown = Arrange::Hand;
        info!("launcher: a drag in the sorted grid — arranged by hand from here");
    }

    /// POSITION picked: the dock takes the new edge at once — its extents
    /// (the box opens across beside a side dock), its input strip, its zone.
    fn side_changed(&mut self) {
        let (dock, full) = self.scaled_extents();
        self.ui.set_extents(dock, full);
        self.zone_free = false;
        self.input_extent = None;
        self.sync_input_region();
        self.on_layout_changed();
        // The other screens' edges move to the new side.
        self.sync_edges();
        self.dirty = true;
        self.schedule_frame();
    }

    /// A side-scroll beside a SIDE dock (Max, 2026-10-21: "right to open
    /// the left dock, scroll left to open the right dock"): away from the
    /// dock's edge it summons and opens the box, as a scroll down opens the
    /// bottom one; back toward the edge it closes the box to the dock. The
    /// fingers' way: with natural scrolling the reported values run the
    /// other way round (`input.natural_scroll`, as for the bottom dock).
    pub(crate) fn side_scroll(&mut self, value: f64) {
        use crate::state::Target;
        use waverunner_proto::Command;
        // An open box over the dock, or one in the grid, takes it to page.
        if self.stack_open() {
            self.box_page_scroll(value);
            return;
        }
        let toward_edge = if self.dock_side() == Side::Left { -1.0 } else { 1.0 };
        let natural = if self.config.input.natural_scroll { -1.0 } else { 1.0 };
        // + = away from the edge: open.
        let out = -value * toward_edge * natural;
        let target = self.ui.target();
        if target == Target::Hidden && !self.pointer_on_reveal_strip() {
            return;
        }
        if target == Target::Open {
            // (The rest of the gesture that opened it must not close it.)
            if self.scroll.open_at.is_some_and(|t| t.elapsed() < crate::EXPAND_BLEED_COOLDOWN) {
                return;
            }
            if self.control_panel {
                return;
            }
        }
        self.scroll.accum += out;
        if self.scroll.accum >= crate::SCROLL_THRESHOLD {
            self.scroll.accum = 0.0;
            match target {
                Target::Hidden => self.handle_command(Command::Show),
                Target::Dock => {
                    self.scroll.open_at = Some(std::time::Instant::now());
                    self.handle_command(Command::Expand);
                }
                Target::Open => {}
            }
        } else if self.scroll.accum <= -crate::SCROLL_THRESHOLD {
            self.scroll.accum = 0.0;
            if target == Target::Open {
                self.handle_command(Command::Collapse);
            }
        }
    }

    /// The input region beside a side dock (see `sync_input_region`): a
    /// strip `extent` deep down the dock's edge, as long as the card is now
    /// — or the whole surface while a box or fast launch is up.
    pub(crate) fn sync_side_input_region(&mut self, extent: u32) {
        let (sw, sh) = self.buffer_size;
        let full = self.stack_open() || self.fast.open;
        let rect = if full {
            (0, 0, sw as i32, sh as i32)
        } else {
            let layout = self.layout_at(self.ui.extent_of(self.ui.target()).max(self.ui.extent()));
            let (y, h) = (layout.card_top.floor() as i32, layout.card_h.ceil() as i32);
            let e = extent.min(sw) as i32;
            let x = if self.dock_side() == Side::Left { 0 } else { sw as i32 - e };
            (x, y.max(0), e, h.min(sh as i32))
        };
        // (Kept as the extent/inset pair the bottom dock compares: the
        // strip's depth and where it starts.)
        let key = (rect.2 as u32, (rect.1.max(0) as u32) << 16 | rect.3.max(0) as u32);
        if self.input_extent != Some(key.0) || self.input_inset != Some(key.1) {
            match crate::surface::set_input_rect(&self.compositor, &self.layer, rect) {
                Ok(()) => {
                    self.input_extent = Some(key.0);
                    self.input_inset = Some(key.1);
                }
                Err(e) => tracing::warn!("failed to set input region: {e:#}"),
            }
        }
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
            transform: 0,
            mm: (0, 0),
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

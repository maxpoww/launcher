//! ROUND CORNERS, PART BY PART (Max, 2026-11-18: "quiero control bordes
//! redondeados que active y desactive bordes redondeados con click…
//! adentro quiero opciones (ventanas, menus, dock, iconos, OPTIONS, etc)
//! todo para poder elegir entre que tiene borde redondo y que no").
//!
//! Controls → Rounded corners: one switch for all of them, then one a
//! part. What a part drawn square means:
//! - every surface of the dock's knows its part (`Renderer::set_part`), and
//!   the renderer draws that surface's rounded rects, shadows and frosted
//!   backdrops with their corner times [`k`] — 0 is square; a grid may say a
//!   part of its own (`GridContent::part`: THE ONE MENU, wherever it is
//!   drawn);
//! - the icons' plates are drawn square (`icon.wgsl`, `Globals::light.y`);
//! - the APP WINDOWS are made square by the compositor: one rule over every
//!   window, declared after Golem's own (settings.lua runs last; a live one
//!   is the newest), so the title bars and the overview's cards — drawn with
//!   the window's own corner — follow.

use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

/// The parts that can be square, in the order the page lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    /// The apps' windows (the compositor's corner), their title bars, the
    /// overview's cards; Quick Look and the window switcher, drawn in the
    /// windows' curve.
    Windows,
    /// THE ONE MENU, wherever it is drawn, and the Properties box.
    Menus,
    /// The dock, its box, Controls, the quick controls, fast launch.
    Dock,
    /// The plates the icons stand on.
    Icons,
    /// The banner's pills and their boxes, and the card.
    Options,
    /// Golem Files' windows inside, and the desktop.
    Files,
}

pub(crate) const PARTS: [Part; 6] = [
    Part::Windows,
    Part::Menus,
    Part::Dock,
    Part::Icons,
    Part::Options,
    Part::Files,
];

impl Part {
    /// Its name in `settings.json`.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Part::Windows => "windows",
            Part::Menus => "menus",
            Part::Dock => "dock",
            Part::Icons => "icons",
            Part::Options => "options",
            Part::Files => "files",
        }
    }

    /// Its switch's key on the page.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Part::Windows => "corners-windows",
            Part::Menus => "corners-menus",
            Part::Dock => "corners-dock",
            Part::Icons => "corners-icons",
            Part::Options => "corners-options",
            Part::Files => "corners-files",
        }
    }

    /// Its name on the page (translated where it is drawn).
    pub(crate) fn label(self) -> &'static str {
        match self {
            Part::Windows => "App windows",
            Part::Menus => "Menus",
            Part::Dock => "Dock",
            Part::Icons => "Icons",
            Part::Options => "OPTIONS",
            Part::Files => "Files and desktop",
        }
    }

    fn bit(self) -> u32 {
        1 << self as u32
    }
}

/// What the owner chose (`corners` in `settings.json`; as made = every
/// part round, not written).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Corners {
    /// Rounded corners at all: off, every part is square.
    pub round: bool,
    /// The parts drawn square while `round` is on, by [`Part::id`]. Kept
    /// while it is off, so turning it on again gives them back.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub square: Vec<String>,
}

impl Default for Corners {
    fn default() -> Self {
        Self {
            round: true,
            square: Vec::new(),
        }
    }
}

impl Corners {
    pub(crate) fn is_default(&self) -> bool {
        self.round && self.square.is_empty()
    }

    /// Is this part drawn square now?
    pub(crate) fn square(&self, part: Part) -> bool {
        !self.round || self.square.iter().any(|p| p == part.id())
    }

    /// The part's own switch (round = on), whatever the switch for all says.
    pub(crate) fn part_round(&self, part: Part) -> bool {
        !self.square.iter().any(|p| p == part.id())
    }

    /// One bit a part drawn square.
    pub(crate) fn bits(&self) -> u32 {
        PARTS
            .iter()
            .filter(|p| self.square(**p))
            .fold(0, |b, p| b | p.bit())
    }

    /// A switch of the page: `corners` (all) or a part's key, `on` = round.
    /// False for a key that is not one of them.
    pub(crate) fn set(&mut self, key: &str, on: bool) -> bool {
        if key == "corners" {
            self.round = on;
            return true;
        }
        let Some(part) = PARTS.iter().find(|p| p.key() == key) else {
            return false;
        };
        self.square.retain(|p| p != part.id());
        if !on {
            self.square.push(part.id().to_owned());
            // (Kept in the page's order: the file reads the same whatever
            // the order of the clicks.)
            self.square
                .sort_by_key(|id| PARTS.iter().position(|p| p.id() == id));
        }
        true
    }
}

/// The parts drawn square now, a bit each (all clear = Golem as made). Read
/// by every renderer at every frame; set by [`adopt`].
static SQUARE: AtomicU32 = AtomicU32::new(0);

/// How round a part's corners are drawn: 1 as made, 0 square.
pub(crate) fn k(part: Part) -> f32 {
    if SQUARE.load(Ordering::Relaxed) & part.bit() != 0 {
        0.0
    } else {
        1.0
    }
}

/// Take a choice on: the shell's surfaces at their next frame. True when
/// something changed (the caller asks every surface for a frame).
pub(crate) fn adopt(c: &Corners) -> bool {
    let bits = c.bits();
    SQUARE.swap(bits, Ordering::Relaxed) != bits
}

/// The rule that makes every app window square, on or off: one rule over
/// every window, so it outranks Golem's own corners (the floating windows',
/// the framed, the staged) declared before it. In a pcall of its own:
/// settings.lua runs as one, and a rule refused must not take what follows
/// along.
pub(crate) fn windows_lua(square: bool) -> String {
    format!(
        "pcall(function() hl.window_rule({{ name = \"golem-square-windows\", match = {{ class = \".*\" }}, rounding = 0, enabled = {square} }}) end)"
    )
}

/// The waveview plugin's corners (its title bars' discs, the overview's and
/// the spread's cards, the minimize's flight) square with the windows, or
/// round again (Max, 2026-11-18: "cuando untoggle round corners, los botones
/// de la barra de titulo siguen siendo redondos"). In a pcall of its own: a
/// plugin from before it has no `corners`, and it must not take what goes
/// with it along.
pub(crate) fn plugin_lua(square: bool) -> String {
    format!("pcall(function() hl.plugin.waveview.corners({square}) end)")
}

/// The windows square now (the rule, the plugin), as `settings.json` says.
pub(crate) fn windows_square() -> bool {
    crate::display::GolemSettings::load()
        .corners
        .square(Part::Windows)
}

/// What the APPS wear of the choice (Max, 2026-11-19: "hacé los casos de
/// golem" — what stayed round with every corner off: GTK apps, Seam, the
/// lock screen): `windows` — the app windows square, their frames and their
/// own window buttons (GTK's, Seam's traffic lights); `all` — the switch for
/// all is off, every corner of their insides square too (and the lock
/// screen's field). Written into the theme the dock publishes for them
/// (`palette.rs`: gtk4.css, the GTK 3 theme, colors.json, hyprlock.conf).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AppsCorners {
    pub windows: bool,
    pub all: bool,
}

pub(crate) fn apps_corners() -> AppsCorners {
    let c = crate::display::GolemSettings::load().corners;
    AppsCorners {
        windows: c.square(Part::Windows),
        all: !c.round,
    }
}

/// The corner Golem's staged window gets (`hypr::assert_stage_frame`): its
/// own, or none while the windows are square.
pub(crate) fn stage_rounding() -> u32 {
    if k(Part::Windows) > 0.0 {
        12
    } else {
        0
    }
}

/// Controls → Rounded corners: the switch for all, then (while it is on) a
/// switch a part, round = on.
pub(crate) fn view(view: &mut crate::controls::ControlView) {
    use crate::controls::{line_of, on_off_of};
    use crate::i18n::tr;
    let c = crate::display::GolemSettings::load().corners;
    view.subtitle = tr("Which parts of StandardOS have round corners.").to_owned();
    view.groups
        .push(on_off_of(Some("Rounded corners"), "corners", c.round));
    if !c.round {
        view.groups
            .push(line_of(tr("Every corner is square.").to_owned()));
        return;
    }
    view.groups.push(line_of(tr("Part by part").to_owned()));
    for part in PARTS {
        view.groups.push(on_off_of(
            Some(part.label()),
            part.key(),
            c.part_round(part),
        ));
    }
}

/// The page's keys (the UI thread's: every surface is drawn again).
pub(crate) fn is_key(key: &str) -> bool {
    key == "corners" || key.starts_with("corners-")
}

impl crate::App {
    /// A switch of the page: kept, then on every surface and window at once.
    pub(crate) fn corners_pick(&mut self, key: &str, value: &str) {
        let on = value == "on";
        crate::display::GolemSettings::change(|s| {
            if s.corners.set(key, on) {
                s.store();
            }
        });
        self.corners_apply();
    }

    /// The choice in `settings.json` on the shell's surfaces and the
    /// compositor's windows (at start, and after a pick).
    pub(crate) fn corners_apply(&mut self) {
        let c = crate::display::GolemSettings::load().corners;
        let windows = c.square(Part::Windows);
        // (Told only when it changes: at start the compositor has it from
        // settings.lua already, and a rule never declared needs no "off".)
        static TOLD: AtomicU32 = AtomicU32::new(0);
        let told = if windows { 2 } else { 1 };
        if TOLD.load(Ordering::Relaxed) != told {
            let first = TOLD.load(Ordering::Relaxed) == 0;
            if windows || !first {
                // The compositor's rule and the plugin's own corners together.
                let lua = format!("{} {}", windows_lua(windows), plugin_lua(windows));
                if !crate::hypr::eval_ok(&lua) {
                    tracing::warn!("rounded corners: the compositor refused {lua}");
                }
            }
            TOLD.store(told, Ordering::Relaxed);
        }
        // The apps' theme again when what they wear of it changed (not at
        // start: the dock's first publish writes it whole).
        static APPS: AtomicU32 = AtomicU32::new(0);
        let a = apps_corners();
        let apps = 1 + u32::from(a.windows) + 2 * u32::from(a.all);
        let was = APPS.swap(apps, Ordering::Relaxed);
        if was != 0 && was != apps {
            crate::palette::corners_changed();
        }
        if adopt(&c) {
            tracing::info!(
                "rounded corners: square {:?}",
                PARTS
                    .iter()
                    .filter(|p| c.square(**p))
                    .map(|p| p.id())
                    .collect::<Vec<_>>()
            );
            self.draw();
            self.draw_options();
            self.draw_card();
            self.filer_redraw_all();
            self.request_desktop_draw();
            self.request_desktop_top_draw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_made_every_part_is_round_and_nothing_is_written() {
        let c = Corners::default();
        assert!(c.is_default() && c.bits() == 0);
        assert!(PARTS.iter().all(|p| !c.square(*p)));
        let s: crate::display::GolemSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.corners, c);
        assert!(!serde_json::to_string(&s).unwrap().contains("corners"));
    }

    #[test]
    fn a_part_turned_off_is_square_and_the_switch_for_all_squares_them_all() {
        let mut c = Corners::default();
        assert!(c.set("corners-dock", false) && c.set("corners-windows", false));
        assert_eq!(
            c.square,
            vec!["windows".to_owned(), "dock".to_owned()],
            "in the page's order"
        );
        assert!(c.square(Part::Dock) && c.square(Part::Windows) && !c.square(Part::Menus));
        assert!(c.set("corners", false));
        assert!(PARTS.iter().all(|p| c.square(*p)), "off: all square");
        assert!(
            !c.part_round(Part::Dock) && c.part_round(Part::Menus),
            "each part's own choice kept"
        );
        c.set("corners", true);
        assert!(
            !c.square(Part::Menus) && c.square(Part::Dock),
            "back on: the choice as it was"
        );
        c.set("corners-dock", true);
        c.set("corners-windows", true);
        assert!(c.is_default());
        assert!(!c.set("corners-bogus", false));
        let json = serde_json::to_string(&Corners {
            round: false,
            square: vec!["menus".into()],
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Corners>(&json).unwrap().bits(),
            (1 << PARTS.len()) - 1
        );
    }

    #[test]
    fn the_square_windows_rule_is_written_only_while_they_are_square() {
        let mut s = crate::display::GolemSettings::default();
        assert!(!s.lua().contains("golem-square-windows"));
        s.corners.set("corners-windows", false);
        assert!(
            s.lua().contains("golem-square-windows")
                && s.lua().contains("rounding = 0, enabled = true")
        );
        s.corners = Corners::default();
        s.corners.set("corners", false);
        assert!(
            s.lua().contains("enabled = true"),
            "all square: the windows too"
        );
        assert!(windows_lua(false).contains("enabled = false"));
        assert_eq!(
            plugin_lua(true),
            "pcall(function() hl.plugin.waveview.corners(true) end)"
        );
    }
}

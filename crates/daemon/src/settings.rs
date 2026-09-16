//! Golem's own settings — what the gear's pages set, and where they are kept.
//!
//! One store rather than a file per switch: the readout has eight pages and
//! they will fill up, and a settings file the user can read in one go is worth
//! more than a tidy separation nobody sees. Missing or unparsable reads as the
//! defaults, like every other store here (see [`crate::persist`]).

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::hypr;
use crate::App;

/// Everything the gear's pages can set. Every field must have a `Default` that
/// means "as Golem ships" — a fresh machine and a deleted store are the same
/// thing, and neither should surprise anyone.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    /// Golem as a **floating** window manager instead of a tiling one
    /// (Max, 2026-09-13, page 1's first setting).
    pub(crate) floating: bool,
}

impl Settings {
    /// Read the store, or the defaults.
    pub(crate) fn load() -> Self {
        crate::persist::read_json(&crate::persist::data_path("settings.json")).unwrap_or_default()
    }

    fn save(&self) {
        crate::persist::write_json(
            "settings",
            &crate::persist::data_path("settings.json"),
            self,
        );
    }
}

impl App {
    /// Whether Golem is currently a floating window manager.
    pub(crate) fn floating_mode(&self) -> bool {
        self.settings.floating
    }

    /// Flip between tiling and floating.
    pub(crate) fn toggle_floating_mode(&mut self) {
        self.set_floating_mode(!self.settings.floating);
    }

    /// Become a floating (or tiling) window manager: **the rule first, then the
    /// windows**.
    ///
    /// Order matters. The rule decides how the next window MAPS, so it is set
    /// before anything else moves; the sweep then brings the windows that are
    /// already open into the same world. Doing it the other way round leaves a
    /// gap in which an app launched mid-toggle arrives under the old regime.
    pub(crate) fn set_floating_mode(&mut self, on: bool) {
        if self.settings.floating == on {
            // Asked for the mode it is already in. Not a no-op: this is the
            // "make it SO" lever for a desk that drifted from the store — the
            // compositor forgets runtime rules, and windows that mapped in a
            // rule-less gap sit tiled under a floating store with nothing to
            // repair them (2026-09-15). Re-assert the rule and bring the
            // windows into line; the stage is left alone because there the
            // mode owns every window's geometry.
            hypr::set_float_rule(on);
            if !self.stage.is_on() {
                hypr::float_all(on);
                if !on {
                    self.schedule_solitary_pseudo();
                }
            }
            return;
        }
        self.settings.floating = on;
        self.settings.save();
        hypr::set_float_rule(on);
        hypr::float_all(on);
        // The solitary-pseudo rule is a TILING rule (a lone tile gets Golem's
        // proportions); with everything floating there are no tiles for it to
        // find, and when tiling comes back it should re-judge the desktop it
        // actually has. So the sweep is skipped while floating and run once on
        // the way back.
        if !on {
            self.schedule_solitary_pseudo();
        }
        self.draw_options();
    }

    /// Re-tell the compositor about the mode. The rule lives in Hyprland, not
    /// on disk, so a compositor restart forgets it while the store still
    /// remembers — the same shape as the screen's remembered colour
    /// (`screen.rs`), and for the same reason: state the daemon owns has to be
    /// re-asserted, not assumed.
    ///
    /// The WINDOWS are left alone here: on a fresh compositor there are none
    /// worth sweeping, and on a daemon restart they are already as the user
    /// left them.
    pub(crate) fn reassert_floating_mode(&self) {
        if self.settings.floating {
            hypr::set_float_rule(true);
        }
    }

    /// A window just MAPPED. While Golem is a floating window manager it
    /// should have arrived floating under the rule — one that arrives tiled
    /// is proof the compositor lost the rule (config reloads forget runtime
    /// rules; a re-assertion can lose a socket race at session start). Put
    /// the rule back — rule first, as always — and float the window that
    /// slipped past it.
    ///
    /// Yes, that window arrives tiled and then jumps, exactly what the rule
    /// exists to prevent (`hypr::set_float_rule`). This is the fallback, not
    /// the design: the jump happens only in the failure case, where the
    /// alternative is a window that stays tiled forever.
    ///
    /// The stage is skipped — there the mode owns every window's geometry —
    /// and so is anything that arrived floating or fullscreen on its own.
    pub(crate) fn heal_floating_map(&self, addr: &str) {
        if !self.settings.floating || self.stage.is_on() {
            return;
        }
        let windows = hypr::layout_windows();
        let Some(w) = windows.iter().find(|w| w.address == addr) else {
            return;
        };
        if !matches!(w.mode, hypr::WindowMode::Tiled | hypr::WindowMode::Pseudo) {
            return;
        }
        warn!("floating mode: {addr} mapped tiled — the compositor lost the float rule; healing");
        hypr::set_float_rule(true);
        hypr::float_window(w);
    }
}

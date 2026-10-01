//! Window memory: an app reopens where you left it (Max, 2026-10-01 — "i want
//! that", macOS's way: size AND position, per app, on whatever workspace).
//!
//! On Wayland an app cannot place its own window, so the compositor has to
//! remember for it. Golem does it with the same tool as its floating mode
//! (`hypr::set_float_rule`): a named Hyprland window rule per app, carrying
//! that app's last size and spot, applied AS THE WINDOW MAPS — it opens there,
//! never somewhere else first and then jumps. A per-app rule wins over the
//! floating mode's catch-all `center` (measured 2026-10-01).
//!
//! - **Remembering.** Every floating window's geometry is noted while it is
//!   open (`track_windows`): the moment a pointer drag of it ends (the
//!   waveview plugin's `window-placed` — Hyprland itself has no resize
//!   event), on window events, and on a slow backstop tick. When an app's LAST window closes, its last
//!   good geometry becomes the app's place. A maximized, fullscreen, staged
//!   or minimized window leaves the place as it was.
//! - **A second window** of an app already open opens a step down-right of
//!   the place, macOS's cascade: the rule is re-declared with the offset as
//!   windows of the app come and go.
//! - **Another screen.** Places are kept relative to the monitor, and every
//!   rule fits them back into the monitor's usable area (shrunk if the screen
//!   is smaller, pulled back on screen) — re-declared when monitors change.
//! - Only while Golem is a floating window manager: tiling places nothing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::{hypr, App};

/// A window smaller than this is a dialog or a popup, not the app's window.
const MIN_W: f64 = 240.0;
const MIN_H: f64 = 160.0;
/// The cascade step for each window of an app already open (logical px).
const CASCADE: f64 = 30.0;
/// How often open windows' geometry is re-read as a backstop. A pointer drag
/// is reported the moment it ends (the plugin's `window-placed`); this tick
/// only catches what has no pointer — a keyboard or scripted resize.
const TRACK_EVERY: Duration = Duration::from_secs(3);

/// An app's remembered place: its window's size and its top-left corner
/// relative to the monitor's top-left, logical px.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Place {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// One open window, as last seen.
struct Seen {
    class: String,
    /// The last geometry fit to remember (`None` until it has been floating,
    /// normal-sized and on a real workspace at least once).
    place: Option<Place>,
}

/// Window memory: the places, saved, and what is open now.
#[derive(Default)]
pub(crate) struct WindowMemory {
    places: HashMap<String, Place>,
    seen: HashMap<String, Seen>,
    path: Option<PathBuf>,
    ticking: bool,
}

impl WindowMemory {
    pub(crate) fn load() -> Self {
        let path = crate::persist::data_path("window-places.json");
        Self {
            places: crate::persist::read_json(&path).unwrap_or_default(),
            path: Some(path),
            ..Self::default()
        }
    }

    fn save(&self) {
        if let Some(path) = &self.path {
            crate::persist::write_json("window places", path, &self.places);
        }
    }

    /// How many windows of `class` are open, `except` one.
    fn open_of(&self, class: &str, except: Option<&str>) -> usize {
        self.seen
            .iter()
            .filter(|(a, s)| s.class == class && Some(a.as_str()) != except)
            .count()
    }
}

/// A rule name for an app class: Hyprland names are free text, but keep it
/// tidy and unique per class.
fn rule_name(class: &str) -> String {
    let tidy: String = class
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("golem-place-{tidy}")
}

/// `class` as an anchored regex literal (Hyprland matches classes by regex).
fn class_regex(class: &str) -> String {
    let mut out = String::from("^");
    for c in class.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('$');
    // Inside a Lua string literal: escape the backslashes and quotes again.
    out.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Fit a place, cascaded `nth` steps, into a monitor's usable area: shrunk to
/// fit, then pulled fully on screen. Returns monitor-relative `(x, y, w, h)`.
pub(crate) fn fit(place: Place, nth: usize, mon: &hypr::MonitorInfo) -> (f64, f64, f64, f64) {
    let (l, t, r, b) = mon.reserved;
    let (ux, uy) = (l, t);
    let (uw, uh) = ((mon.w - l - r).max(1.0), (mon.h - t - b).max(1.0));
    let w = place.w.min(uw).round();
    let h = place.h.min(uh).round();
    let off = CASCADE * nth as f64;
    let x = (place.x + off).clamp(ux, ux + uw - w).round();
    let y = (place.y + off).clamp(uy, uy + uh - h).round();
    (x, y, w, h)
}

impl App {
    /// Re-read every open window's geometry; note the ones fit to remember.
    pub(crate) fn track_windows(&mut self) {
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let mut open = HashMap::new();
        for c in hypr::client_geometries() {
            let fit_to_keep = c.floating
                && !c.fullscreen
                && !c.special
                && !c.staged
                && c.w >= MIN_W
                && c.h >= MIN_H;
            let prev = self.window_memory.seen.remove(&c.address);
            let place = if fit_to_keep {
                Some(Place { x: c.x - mon.x, y: c.y - mon.y, w: c.w, h: c.h })
            } else {
                prev.and_then(|p| p.place)
            };
            open.insert(c.address, Seen { class: c.class, place });
        }
        self.window_memory.seen = open;
    }

    /// Keep the geometry fresh while Golem floats (a slow tick: resizes have
    /// no event of their own).
    pub(crate) fn start_window_tracking(&mut self) {
        if self.window_memory.ticking {
            return;
        }
        self.window_memory.ticking = true;
        let timer = calloop::timer::Timer::from_duration(TRACK_EVERY);
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            if app.settings.floating {
                app.track_windows();
            }
            calloop::timer::TimeoutAction::ToDuration(TRACK_EVERY)
        });
    }

    /// A window mapped: note it, and step the app's rule on so its NEXT
    /// window cascades rather than landing on top of this one.
    pub(crate) fn remember_window_opened(&mut self, addr: &str) {
        if !self.settings.floating {
            return;
        }
        self.track_windows();
        if let Some(class) = self.window_memory.seen.get(addr).map(|s| s.class.clone()) {
            self.declare_place_rule(&class);
        }
    }

    /// A window closed. If it was the app's last, its last good geometry is
    /// the app's place now; either way the cascade steps back.
    pub(crate) fn remember_window_closed(&mut self, addr: &str) {
        let Some(gone) = self.window_memory.seen.remove(addr) else {
            return;
        };
        let last = self.window_memory.open_of(&gone.class, None) == 0;
        if last && self.settings.floating {
            if let Some(place) = gone.place {
                if self.window_memory.places.get(&gone.class) != Some(&place) {
                    debug!("window memory: {} remembered at {place:?}", gone.class);
                    self.window_memory.places.insert(gone.class.clone(), place);
                    self.window_memory.save();
                }
            }
        }
        self.declare_place_rule(&gone.class);
    }

    /// Declare (or re-declare) one app's rule: its place, cascaded once per
    /// window of it already open, fit to the focused monitor. No place, or
    /// tiling: the rule is switched off.
    fn declare_place_rule(&self, class: &str) {
        let name = rule_name(class);
        let regex = class_regex(class);
        let place = self.window_memory.places.get(class).copied();
        let mon = hypr::focused_monitor().ok();
        let lua = match (place, mon, self.settings.floating) {
            (Some(place), Some(mon), true) => {
                let nth = self.window_memory.open_of(class, None);
                let (x, y, w, h) = fit(place, nth, &mon);
                format!(
                    "hl.window_rule({{ name = \"{name}\", match = {{ class = \"{regex}\" }}, \
                     size = {{ {w}, {h} }}, move = \"{x} {y}\", enabled = true }})"
                )
            }
            _ => format!(
                "hl.window_rule({{ name = \"{name}\", match = {{ class = \"{regex}\" }}, enabled = false }})"
            ),
        };
        hypr::dispatch(&format!("(function() {lua} return hl.dsp.no_op() end)()"));
    }

    /// Put every app's rule back: at start, and whenever the compositor
    /// forgot them or the monitor changed (`reassert_floating_mode`).
    pub(crate) fn reassert_place_rules(&mut self) {
        self.track_windows();
        let classes: Vec<String> = self.window_memory.places.keys().cloned().collect();
        for class in &classes {
            self.declare_place_rule(class);
        }
        if !classes.is_empty() {
            info!("window memory: {} app place(s) asserted", classes.len());
        }
        self.start_window_tracking();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon() -> hypr::MonitorInfo {
        hypr::MonitorInfo {
            x: 0.0,
            y: 0.0,
            w: 2000.0,
            h: 1250.0,
            reserved: (0.0, 28.0, 0.0, 0.0),
            active_ws: 1,
            scale: 1.6,
            name: "eDP-1".into(),
        }
    }

    #[test]
    fn a_place_fits_the_screen_and_cascades() {
        let p = Place { x: 300.0, y: 200.0, w: 900.0, h: 600.0 };
        assert_eq!(fit(p, 0, &mon()), (300.0, 200.0, 900.0, 600.0), "as left");
        assert_eq!(fit(p, 2, &mon()), (360.0, 260.0, 900.0, 600.0), "two windows open: two steps");
        // A bigger screen's place on a smaller one: shrunk, then on screen.
        let big = Place { x: 2400.0, y: 900.0, w: 2600.0, h: 1500.0 };
        let (x, y, w, h) = fit(big, 0, &mon());
        assert_eq!((w, h), (2000.0, 1222.0), "no bigger than the usable area");
        assert_eq!((x, y), (0.0, 28.0), "and pulled fully on screen, under the bar");
    }

    #[test]
    fn class_regexes_are_literal() {
        assert_eq!(class_regex("webapp-youtube"), "^webapp-youtube$");
        assert_eq!(class_regex("org.gnome.Nautilus"), "^org\\\\.gnome\\\\.Nautilus$");
        assert_eq!(rule_name("org.gnome.Nautilus"), "golem-place-org-gnome-Nautilus");
    }
}

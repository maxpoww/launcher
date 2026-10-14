//! Window memory: an app reopens where you left it (Max, 2026-10-01 — "i want
//! that", macOS's way: size AND position, per app, on whatever workspace).
//!
//! On Wayland an app cannot place its own window, so the compositor has to
//! remember for it. Golem does it with the same tool as its floating mode
//! (`hypr::set_float_rule`): a named Hyprland window rule per app, carrying
//! that app's last size and spot, applied AS THE WINDOW MAPS — it opens there,
//! never somewhere else first and then jumps. A per-app rule wins over the
//! floating mode's catch-all `center` (measured 2026-10-01 — and by the
//! compositor's own order: rules apply in the order they were FIRST declared,
//! a re-declaration by name changes the rule in place, `move` and `center`
//! overwrite each other, and the catch-all is declared first, at start:
//! `reassert_floating_mode` before `reassert_place_rules`).
//!
//! - **Remembering.** An app's place is where you last PUT a window of it —
//!   the moment a pointer drag of one ends (the waveview plugin's
//!   `window-placed`; Hyprland itself has no resize event) — or where a
//!   window of it was when it closed. ANY window of the app, not only its
//!   last: a terminal is never down to zero windows, so a last-window rule
//!   never fired for it and foot opened dead centre for two weeks (Max,
//!   2026-10-14: "the terminal just does nothing"). A maximized, fullscreen,
//!   staged or minimized window leaves the place as it was.
//! - **Not on top of another.** The spot is cascaded a step down-right,
//!   macOS's way, for as long as a floating window on the showing workspace
//!   already has its top-left corner there — the app's own other windows and
//!   everybody else's alike (Max, 2026-10-14: "an open window should not
//!   open on top of another, and it happens all the time"). Off the bottom
//!   or the right, the walk wraps back to the edge. Every app's rule is
//!   recomputed whenever a window comes, goes or lands, and re-declared only
//!   when its answer changed.
//! - **Another screen.** Places are kept relative to the monitor, and every
//!   rule fits them back into the monitor's usable area (shrunk if the screen
//!   is smaller, pulled back on screen) — re-declared when monitors change.
//! - Only while Golem is a floating window manager: tiling places nothing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::{hypr, App};

/// A window smaller than this is a dialog or a popup, not the app's window.
const MIN_W: f64 = 240.0;
const MIN_H: f64 = 160.0;
/// The cascade step: how far down-right a window opens from a corner that is
/// taken (logical px), and how near a corner counts as taken.
const CASCADE: f64 = 30.0;
/// How many steps the cascade walks before giving up and using the place as
/// it is: a desk with that many corners taken is past helping.
const MAX_STEPS: usize = 64;
/// How often open windows' geometry is re-read as a backstop. A pointer drag
/// is reported the moment it ends (the plugin's `window-placed`); this tick
/// only catches what has no pointer — a keyboard or scripted move.
const TRACK_EVERY: Duration = Duration::from_secs(3);

/// Classes whose rule was declared past this module (`declare_sized`): the
/// next sync re-declares them whatever it last said.
static OUT_OF_BAND: Mutex<Vec<String>> = Mutex::new(Vec::new());

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
    /// Its top-left corner now, monitor-relative, if it floats on a real
    /// workspace: a spot the next window should not open on.
    corner: Option<(f64, f64)>,
    workspace: i64,
}

/// Window memory: the places, saved, and what is open now.
#[derive(Default)]
pub(crate) struct WindowMemory {
    places: HashMap<String, Place>,
    seen: HashMap<String, Seen>,
    /// The rule last sent for each class, so a recompute speaks only when its
    /// answer changed.
    declared: HashMap<String, String>,
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

    /// `place` is `class`'s place from now on, if that is news.
    fn adopt(&mut self, class: &str, place: Place, how: &str) {
        if self.places.get(class) == Some(&place) {
            return;
        }
        debug!("window memory: {class} remembered at {place:?} ({how})");
        self.places.insert(class.to_owned(), place);
        self.save();
    }

    /// The corners taken on workspace `ws`: where a new window must not land.
    fn corners_on(&self, ws: i64) -> Vec<(f64, f64)> {
        self.seen
            .values()
            .filter(|s| s.workspace == ws)
            .filter_map(|s| s.corner)
            .collect()
    }
}

/// Make the NEXT window of `class` open floating, `w`×`h` and centred,
/// whatever was remembered for it and whether or not Golem is floating —
/// for a window whose right size is known before it opens (a phone's
/// mirror). It goes out under the class's own rule name, so there is one
/// rule for the app and the last one declared is the one that holds; what
/// the owner then does to the window is remembered as for any other. Any
/// thread.
pub(crate) fn declare_sized(class: &str, w: i64, h: i64) {
    let (name, regex) = (rule_name(class), class_regex(class));
    hypr::dispatch(&format!(
        "(function() hl.window_rule({{ name = \"{name}\", match = {{ class = \"{regex}\" }}, \
         float = true, size = {{ {w}, {h} }}, center = true, enabled = true }}) \
         return hl.dsp.no_op() end)()"
    ));
    if let Ok(mut oob) = OUT_OF_BAND.lock() {
        oob.push(class.to_owned());
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

/// Fit a place into a monitor's usable area — shrunk to fit, then pulled
/// fully on screen — and cascade it clear of `taken`, the monitor-relative
/// top-left corners of the windows already on the workspace: a step
/// down-right for as long as the corner is within a step of a taken one,
/// wrapping to the top edge off the bottom and to the left edge off the
/// right. Returns monitor-relative `(x, y, w, h)`.
pub(crate) fn fit(place: Place, taken: &[(f64, f64)], mon: &hypr::MonitorInfo) -> (f64, f64, f64, f64) {
    let (l, t, r, b) = mon.reserved;
    let (ux, uy) = (l, t);
    let (uw, uh) = ((mon.w - l - r).max(1.0), (mon.h - t - b).max(1.0));
    let w = place.w.min(uw).round();
    let h = place.h.min(uh).round();
    let (max_x, max_y) = (ux + uw - w, uy + uh - h);
    let (x0, y0) = (place.x.clamp(ux, max_x).round(), place.y.clamp(uy, max_y).round());
    let free = |x: f64, y: f64| {
        !taken
            .iter()
            .any(|&(tx, ty)| (tx - x).abs() < CASCADE && (ty - y).abs() < CASCADE)
    };
    let (mut x, mut y) = (x0, y0);
    for _ in 0..MAX_STEPS {
        if free(x, y) {
            return (x, y, w, h);
        }
        x += CASCADE;
        y += CASCADE;
        if y > max_y {
            y = uy;
        }
        if x > max_x {
            x = ux;
        }
    }
    (x0, y0, w, h)
}

impl App {
    /// Re-read every open window's geometry; note the ones fit to remember,
    /// and where every floating one stands.
    pub(crate) fn track_windows(&mut self) {
        let Ok(mon) = hypr::focused_monitor() else {
            return;
        };
        let mut open = HashMap::new();
        for c in hypr::client_geometries() {
            let on_desk = c.floating && !c.special;
            let fit_to_keep = on_desk && !c.fullscreen && !c.staged && c.w >= MIN_W && c.h >= MIN_H;
            let prev = self.window_memory.seen.remove(&c.address);
            let here = Place { x: c.x - mon.x, y: c.y - mon.y, w: c.w, h: c.h };
            let place = if fit_to_keep { Some(here) } else { prev.and_then(|p| p.place) };
            open.insert(
                c.address,
                Seen {
                    class: c.class,
                    place,
                    corner: on_desk.then_some((here.x, here.y)),
                    workspace: c.workspace,
                },
            );
        }
        self.window_memory.seen = open;
    }

    /// Keep the geometry fresh while Golem floats (a slow tick: moves without
    /// a pointer have no event of their own), and the rules with it.
    pub(crate) fn start_window_tracking(&mut self) {
        if self.window_memory.ticking {
            return;
        }
        self.window_memory.ticking = true;
        let timer = calloop::timer::Timer::from_duration(TRACK_EVERY);
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            if app.settings.floating {
                app.track_windows();
                app.sync_place_rules();
            }
            calloop::timer::TimeoutAction::ToDuration(TRACK_EVERY)
        });
    }

    /// A window mapped: note it, and step every rule whose spot it took.
    pub(crate) fn remember_window_opened(&mut self, _addr: &str) {
        if !self.settings.floating {
            return;
        }
        self.track_windows();
        self.sync_place_rules();
    }

    /// A pointer drag of a window just ended: where it landed is its app's
    /// place now, and every rule is re-judged against where it stands.
    pub(crate) fn remember_window_placed(&mut self, addr: &str) {
        if !self.settings.floating {
            return;
        }
        self.track_windows();
        let landed = self
            .window_memory
            .seen
            .get(addr)
            .and_then(|s| s.place.map(|p| (s.class.clone(), p)));
        if let Some((class, place)) = landed {
            self.window_memory.adopt(&class, place, "placed");
        }
        self.sync_place_rules();
    }

    /// A window closed: where it was is its app's place now, and the corner
    /// it took is free again.
    pub(crate) fn remember_window_closed(&mut self, addr: &str) {
        let Some(gone) = self.window_memory.seen.remove(addr) else {
            return;
        };
        if !self.settings.floating {
            return;
        }
        // A window hanging on the side (`side.rs`) closed where it was parked,
        // not where it is used: that is no place for the app.
        if let Some(place) = gone.place.filter(|_| !self.side.has(addr)) {
            self.window_memory.adopt(&gone.class, place, "closed");
        }
        self.sync_place_rules();
    }

    /// One app's rule as it should read now: its place, cascaded clear of
    /// `taken`, fit to the monitor. No place, no monitor, or tiling: the rule
    /// is switched off.
    fn place_rule_lua(&self, class: &str, mon: Option<&hypr::MonitorInfo>, taken: &[(f64, f64)]) -> String {
        let name = rule_name(class);
        let regex = class_regex(class);
        match (self.window_memory.places.get(class), mon, self.settings.floating) {
            (Some(&place), Some(mon), true) => {
                let (x, y, w, h) = fit(place, taken, mon);
                format!(
                    "hl.window_rule({{ name = \"{name}\", match = {{ class = \"{regex}\" }}, \
                     size = {{ {w}, {h} }}, move = \"{x} {y}\", enabled = true }})"
                )
            }
            _ => format!(
                "hl.window_rule({{ name = \"{name}\", match = {{ class = \"{regex}\" }}, enabled = false }})"
            ),
        }
    }

    /// Every app's rule, recomputed against the windows open now, and sent to
    /// the compositor where its answer changed.
    fn sync_place_rules(&mut self) {
        if let Ok(mut oob) = OUT_OF_BAND.lock() {
            for class in oob.drain(..) {
                self.window_memory.declared.remove(&class);
            }
        }
        let mon = hypr::focused_monitor().ok();
        let taken = mon
            .as_ref()
            .map(|m| self.window_memory.corners_on(m.active_ws))
            .unwrap_or_default();
        let classes: Vec<String> = self.window_memory.places.keys().cloned().collect();
        let mut changed = 0;
        for class in classes {
            let lua = self.place_rule_lua(&class, mon.as_ref(), &taken);
            if self.window_memory.declared.get(&class) == Some(&lua) {
                continue;
            }
            // Remembered as sent only when the compositor said ok: a rule lost
            // on the socket is said again at the next sync, not assumed.
            if hypr::dispatch_checked(&format!("(function() {lua} return hl.dsp.no_op() end)()")) {
                self.window_memory.declared.insert(class, lua);
            }
            changed += 1;
        }
        if changed > 0 {
            debug!("window memory: {changed} rule(s) re-declared");
        }
    }

    /// Put every app's rule back, whatever was said before: at start, and
    /// whenever the compositor forgot them or the monitor changed
    /// (`reassert_floating_mode`).
    pub(crate) fn reassert_place_rules(&mut self) {
        self.window_memory.declared.clear();
        self.track_windows();
        self.sync_place_rules();
        let n = self.window_memory.places.len();
        if n > 0 {
            info!("window memory: {n} app place(s) asserted");
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
    fn a_place_fits_the_screen() {
        let p = Place { x: 300.0, y: 200.0, w: 900.0, h: 600.0 };
        assert_eq!(fit(p, &[], &mon()), (300.0, 200.0, 900.0, 600.0), "as left");
        // A bigger screen's place on a smaller one: shrunk, then on screen.
        let big = Place { x: 2400.0, y: 900.0, w: 2600.0, h: 1500.0 };
        let (x, y, w, h) = fit(big, &[], &mon());
        assert_eq!((w, h), (2000.0, 1222.0), "no bigger than the usable area");
        assert_eq!((x, y), (0.0, 28.0), "and pulled fully on screen, under the bar");
    }

    #[test]
    fn a_taken_corner_steps_the_place_down_right() {
        let p = Place { x: 300.0, y: 200.0, w: 900.0, h: 600.0 };
        let taken = [(300.0, 200.0), (330.0, 230.0), (1000.0, 1000.0)];
        assert_eq!(fit(p, &taken, &mon()), (360.0, 260.0, 900.0, 600.0), "two steps past two windows");
        // Nearly there counts as there.
        assert_eq!(fit(p, &[(310.0, 190.0)], &mon()), (330.0, 230.0, 900.0, 600.0));
        // Off the bottom the walk wraps to the top edge, under the bar.
        let low = Place { x: 300.0, y: 640.0, w: 900.0, h: 600.0 };
        assert_eq!(fit(low, &[(300.0, 640.0)], &mon()), (330.0, 28.0, 900.0, 600.0));
        // A window that fills the width can only walk down.
        let wide = Place { x: 0.0, y: 28.0, w: 2000.0, h: 600.0 };
        assert_eq!(fit(wide, &[(0.0, 28.0)], &mon()), (0.0, 58.0, 2000.0, 600.0));
    }

    #[test]
    fn class_regexes_are_literal() {
        assert_eq!(class_regex("webapp-youtube"), "^webapp-youtube$");
        assert_eq!(class_regex("org.gnome.Nautilus"), "^org\\\\.gnome\\\\.Nautilus$");
        assert_eq!(rule_name("org.gnome.Nautilus"), "golem-place-org-gnome-Nautilus");
    }
}

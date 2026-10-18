//! ZOOM: Ctrl + and Ctrl − make the top bar and the desktop bigger or
//! smaller, each where the hand is (Max, 2026-10-10: *"I focus the desktop,
//! press Ctrl++, the icons grow; same with the pills and the banner on
//! OPTIONS: I hover it, press, they grow"*). Ctrl 0 puts it back.
//!
//! - **The bar** is zoomed on HOVER. It never has the keyboard, so the keys
//!   are compositor binds that exist exactly while the pointer is on it
//!   (the card's way, `card/mod.rs::card_keys`): made on the pointer's enter,
//!   taken away on its leave. Its zoom multiplies the bar's own scale
//!   (`App::options_scale`): the banner, every pill, every box.
//! - **The desktop** is zoomed when it is FOCUSED (a click on it gives it
//!   the keyboard): the keys come to it as its other shortcuts do. Its zoom
//!   multiplies the icon scale its grid is laid out at; an icon keeps its
//!   cell, counted in cells.
//!
//! - **A group open over the dock** (Max, 2026-10-13: *"add ctrl+ +- to zoom
//!   in and out on the group on top of the dock, and ctrl+0 to reset"*) is
//!   zoomed while it is OPEN: the dock has no keyboard either, so the keys
//!   are compositor binds for as long as the stack stands, the bar's way
//!   (and before the bar's, should the pointer be on it). Its zoom
//!   multiplies the scale its members are drawn at.
//!
//! - **A file manager window** (`filer.rs`) is a window with the keyboard:
//!   the keys come to it, and they zoom WHAT THE POINTER IS ON (Max,
//!   2026-10-14): its field — the things — laid out and drawn at its own zoom
//!   (`filer::Layout::zoom`), or its places — their ICONS only, not their
//!   words, and further: up to `PANEL_MAX`.
//!
//! All are the owner's standing choice (`settings.json`).

use tracing::info;

use crate::App;

const MIN: f32 = 0.7;
const MAX: f32 = 2.0;
const STEP: f32 = 0.1;

/// The keys, as the compositor names them, and what each does.
const KEYS: [(&str, &str); 8] = [
    ("CTRL + plus", "in"),
    ("CTRL + SHIFT + plus", "in"),
    ("CTRL + equal", "in"),
    ("CTRL + KP_Add", "in"),
    ("CTRL + minus", "out"),
    ("CTRL + KP_Subtract", "out"),
    ("CTRL + 0", "reset"),
    ("CTRL + KP_0", "reset"),
];

/// A stored zoom as a factor (never set: 1).
fn factor(stored: f32) -> f32 {
    if stored > 0.0 { stored.clamp(MIN, MAX) } else { 1.0 }
}

/// The names' size at a stack zoom of `zoom`: one step for every two whole
/// steps the zoom is from 1, either way.
fn text_factor(zoom: f32) -> f32 {
    let steps = ((zoom - 1.0) / STEP).round() as i32;
    1.0 + (steps / 2) as f32 * STEP
}

/// `now` after `how` (`in`, `out`, `reset`, or a number): a tenth a step,
/// kept between `MIN` and `MAX`. `None` for a word it does not know.
#[cfg(test)]
pub(crate) fn stepped(now: f32, how: &str) -> Option<f32> {
    stepped_to(now, how, MIN, MAX)
}

/// [`stepped`] between `min` and `max`.
fn stepped_to(now: f32, how: &str, min: f32, max: f32) -> Option<f32> {
    let to = match how {
        "in" => now + STEP,
        "out" => now - STEP,
        "reset" => 1.0,
        other => other.parse::<f32>().ok()?,
    };
    Some(((to * 10.0).round() / 10.0).clamp(min, max))
}

/// How far a file window's places' icons may grow (Max, 2026-10-14: "zoom
/// more on the panel icons").
const PANEL_MAX: f32 = 3.0;
/// …and how small: small enough that the places become their symbols alone
/// (`filer::SYMBOL_FROM`).
const PANEL_MIN: f32 = 0.4;

impl App {
    /// How much bigger than its own size the bar is drawn.
    pub(crate) fn bar_zoom(&self) -> f32 {
        factor(self.settings.bar_zoom)
    }

    /// How much bigger than the dock's icons the desktop's are.
    pub(crate) fn desktop_zoom(&self) -> f32 {
        factor(self.settings.desktop_zoom)
    }

    /// How much bigger than their own size a dock stack's members are.
    pub(crate) fn stack_zoom(&self) -> f32 {
        factor(self.settings.stack_zoom)
    }

    /// How much bigger than a label a dock stack's names are: they grow
    /// with its zoom, but half as fast — a step for every two of the
    /// icons' (Max, 2026-10-13: "each 2 hits the text grows once").
    pub(crate) fn stack_text_zoom(&self) -> f32 {
        text_factor(self.stack_zoom())
    }

    /// How much bigger than their own size fast launch and the quick
    /// controls are drawn (Max, 2026-10-13: Ctrl +/−/0 while one is open).
    pub(crate) fn fast_zoom(&self) -> f32 {
        factor(self.settings.fast_zoom)
    }

    /// How much bigger than its own size everything in a file manager
    /// window is drawn (Max, 2026-10-13: "Ctrl +/− and Ctrl 0, it zooms all").
    pub(crate) fn files_zoom(&self) -> f32 {
        factor(self.settings.files_zoom)
    }

    /// The same for a file manager window's panel: its places and its head.
    pub(crate) fn files_panel_zoom(&self) -> f32 {
        let z = self.settings.files_panel_zoom;
        if z > 0.0 { z.clamp(PANEL_MIN, PANEL_MAX) } else { 1.0 }
    }

    /// Zoom the bar, the desktop or a dock stack (`what`) in, out, back, or
    /// to a number (`how`). What was done.
    pub(crate) fn zoom(&mut self, what: &str, how: &str) -> String {
        const USAGE: &str = "zoom bar|desktop|stack|fast|files|files-panel in|out|reset|<n>";
        let now = match what {
            "bar" => self.bar_zoom(),
            "desktop" => self.desktop_zoom(),
            "stack" => self.stack_zoom(),
            "fast" => self.fast_zoom(),
            "files" => self.files_zoom(),
            "files-panel" => self.files_panel_zoom(),
            _ => return USAGE.to_owned(),
        };
        let (min, max) = if what == "files-panel" { (PANEL_MIN, PANEL_MAX) } else { (MIN, MAX) };
        let Some(to) = stepped_to(now, how, min, max) else {
            return USAGE.to_owned();
        };
        if (to - now).abs() < 0.001 {
            if what == "fast" {
                self.fast_touch();
            }
            return format!("{what} zoom stays {to}");
        }
        info!("zoom: {what} {now} → {to}");
        if what == "bar" {
            self.settings.bar_zoom = to;
            self.settings.save();
            // The bar's scale moved: its height (and the room it takes from
            // the windows), its text, its boxes — as for a change of screen.
            self.sync_options_zone();
            self.tasks_changed();
        } else if what == "fast" {
            self.settings.fast_zoom = to;
            self.settings.save();
            // Zooming is using it: its idle clock starts over (the keys come
            // through the compositor, not through its own keyboard).
            self.fast_touch();
            self.schedule_frame();
        } else if what == "files" || what == "files-panel" {
            if what == "files" {
                self.settings.files_zoom = to;
            } else {
                self.settings.files_panel_zoom = to;
            }
            self.settings.save();
            self.filer_zoomed();
        } else if what == "stack" {
            self.settings.stack_zoom = to;
            self.settings.save();
            // Its plate changed size: so did what the pointer may touch.
            self.sync_input_region();
            self.schedule_frame();
        } else {
            self.settings.desktop_zoom = to;
            self.settings.save();
            self.relayout_desktop();
        }
        format!("{what} zoom {to}")
    }

    /// The pointer came onto the bar or left it: Ctrl +/−/0 are the bar's
    /// for exactly that long.
    pub(crate) fn bar_zoom_keys(&mut self, on: bool) {
        if on == self.bar_zoom_keys {
            return;
        }
        self.bar_zoom_keys = on;
        self.sync_zoom_keys();
    }

    /// A dock that went down while it held Ctrl +/−/0 — the pointer on the
    /// bar or the card, fast launch or a group open — left them bound to it,
    /// and a new one starts believing nothing is: every app lost its own
    /// zoom keys (Seam's page zoom among them) until the pointer crossed the
    /// bar again (the Acer, 2026-10-17, after a restart). A dock starts by
    /// giving them back. (The card binds the same keys: one release for both.)
    pub(crate) fn zoom_keys_release(&mut self) {
        let lua: String = KEYS.iter().map(|(key, _)| format!("hl.unbind(\"{key}\") ")).collect();
        self.zoom_keys_for = None;
        if !crate::hypr::eval_ok(&lua) {
            tracing::debug!("zoom: Ctrl +/- not given back at start (no compositor yet?)");
        }
    }

    /// Bind Ctrl +/−/0 to what they zoom now — fast launch or the quick
    /// controls when one is up, else a group open over the dock, else the
    /// bar under the pointer — or give them back.
    pub(crate) fn sync_zoom_keys(&mut self) {
        let stack = self.dock_stack.is_some() || self.dir_stack.as_ref().is_some_and(|ds| !ds.in_grid);
        // Fast launch or the quick controls up: the keys are theirs, over
        // whatever is behind (a group left open, the bar under the pointer) —
        // the compositor's binds fire before our own keyboard sees a key.
        let want = if self.fast.open {
            Some("fast")
        } else if stack {
            Some("stack")
        } else if self.bar_zoom_keys {
            Some("bar")
        } else {
            None
        };
        if want == self.zoom_keys_for {
            return;
        }
        self.zoom_keys_for = want;
        let ctl = crate::hypr::ctl_program();
        let mut lua = String::new();
        for (key, how) in KEYS {
            lua.push_str(&format!("hl.unbind(\"{key}\") "));
            if let (Some(what), Some(ctl)) = (want, &ctl) {
                lua.push_str(&format!(
                    "hl.bind(\"{key}\", hl.dsp.exec_cmd(\"{} debug-desktop zoom {what} {how}\")) ",
                    ctl.display()
                ));
            }
        }
        let ok = crate::hypr::eval_ok(&lua);
        tracing::debug!("zoom: Ctrl +/- are {want:?}'s (compositor said {ok})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zoom_goes_by_tenths_between_its_ends() {
        assert_eq!(stepped(1.0, "in"), Some(1.1));
        assert_eq!(stepped(1.1, "out"), Some(1.0));
        assert_eq!(stepped(1.7, "reset"), Some(1.0));
        assert_eq!(stepped(2.0, "in"), Some(2.0));
        assert_eq!(stepped(0.7, "out"), Some(0.7));
        assert_eq!(stepped(1.0, "1.34"), Some(1.3));
        assert_eq!(stepped(1.0, "bigger"), None);
        // A stack's names take a step for every two of its icons'.
        let text: Vec<f32> = [0.8, 0.9, 1.0, 1.1, 1.2, 1.3, 1.4].iter().map(|&z| (text_factor(z) * 10.0).round() / 10.0).collect();
        assert_eq!(text, [0.9, 1.0, 1.0, 1.0, 1.1, 1.1, 1.2]);
        // Never set is as shipped; a stored one is kept within the ends.
        assert_eq!(factor(0.0), 1.0);
        assert_eq!(factor(5.0), MAX);
    }
}

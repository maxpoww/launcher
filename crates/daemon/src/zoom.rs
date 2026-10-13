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
//! Both are the owner's standing choice (`settings.json`).

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

/// `now` after `how` (`in`, `out`, `reset`, or a number): a tenth a step,
/// kept between `MIN` and `MAX`. `None` for a word it does not know.
pub(crate) fn stepped(now: f32, how: &str) -> Option<f32> {
    let to = match how {
        "in" => now + STEP,
        "out" => now - STEP,
        "reset" => 1.0,
        other => other.parse::<f32>().ok()?,
    };
    Some(((to * 10.0).round() / 10.0).clamp(MIN, MAX))
}

impl App {
    /// How much bigger than its own size the bar is drawn.
    pub(crate) fn bar_zoom(&self) -> f32 {
        factor(self.settings.bar_zoom)
    }

    /// How much bigger than the dock's icons the desktop's are.
    pub(crate) fn desktop_zoom(&self) -> f32 {
        factor(self.settings.desktop_zoom)
    }

    /// Zoom the bar or the desktop (`what`) in, out, back, or to a number
    /// (`how`). What was done.
    pub(crate) fn zoom(&mut self, what: &str, how: &str) -> String {
        let now = match what {
            "bar" => self.bar_zoom(),
            "desktop" => self.desktop_zoom(),
            _ => return "zoom bar|desktop in|out|reset|<n>".to_owned(),
        };
        let Some(to) = stepped(now, how) else {
            return "zoom bar|desktop in|out|reset|<n>".to_owned();
        };
        if (to - now).abs() < 0.001 {
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
        let ctl = crate::hypr::ctl_program();
        let mut lua = String::new();
        for (key, how) in KEYS {
            lua.push_str(&format!("hl.unbind(\"{key}\") "));
            if let (true, Some(ctl)) = (on, &ctl) {
                lua.push_str(&format!(
                    "hl.bind(\"{key}\", hl.dsp.exec_cmd(\"{} debug-desktop zoom bar {how}\")) ",
                    ctl.display()
                ));
            }
        }
        let ok = crate::hypr::eval_ok(&lua);
        tracing::debug!("zoom: Ctrl +/- {} for the bar (compositor said {ok})", if on { "taken" } else { "given back" });
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
        // Never set is as shipped; a stored one is kept within the ends.
        assert_eq!(factor(0.0), 1.0);
        assert_eq!(factor(5.0), MAX);
    }
}

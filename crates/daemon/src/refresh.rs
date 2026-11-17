//! A screen's REFRESH RATE (Max, 2026-11-15: "i want the dinamic frames
//! toggle on configurations, on monitors, where you choose the frame rate, a
//! 'dinamic' and also i want min FPS on battery … when unplugged the screen
//! goes to 60").
//!
//! Two rows of the Monitors page, each the picked screen's own:
//!
//! - **Refresh rate**: the panel's rates at the size in use, and DYNAMIC —
//!   the top rate with adaptive sync on the desktop (the compositor's
//!   `vrr = 1` for that screen), so the panel follows what is drawn. A
//!   number is a fixed rate, adaptive sync left to fullscreen windows
//!   (`vrr = 2`). Dynamic is a choice, never the default: on many panels
//!   every change of cadence shows as a step of brightness (the flicker of
//!   2026-09-10, 10-20 and 11-15 on Max's own).
//! - **On battery**: the rate the screen goes to when the computer is
//!   unplugged, back to the other when it is plugged in. Nothing chosen: the
//!   lowest rate that is still 60 or more. On the charger a screen whose rate
//!   nobody chose runs at its top rate.
//!
//! The rate on the battery is only ever PUT ON the screen (a rule evaluated,
//! never saved): the saved rule is the charger's. So the dock puts the right
//! one on whenever the source changes, as it starts, when the compositor
//! reads its config again and when a screen is plugged in
//! (`App::refresh_assert`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// What the owner chose for one screen's refresh.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RefreshChoice {
    /// Dynamic picked (true) or a fixed rate picked (false); absent: the
    /// row never touched, the compositor's own default stands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<bool>,
    /// The rate on the battery: absent = [`battery_default`], `0` = the
    /// same as plugged in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery: Option<f64>,
    /// The rate on the charger, kept while the battery's is on the screen
    /// (what to go back to).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugged: Option<f64>,
}

pub(crate) type Refresh = BTreeMap<String, RefreshChoice>;

/// The compositor's `vrr` for a screen whose rate row was picked.
pub(crate) fn vrr_of(refresh: &Refresh, selector: &str) -> Option<u8> {
    refresh.get(selector)?.dynamic.map(|on| if on { 1 } else { 2 })
}

const UNKNOWN: u8 = 0;
const CHARGER: u8 = 1;
const BATTERY: u8 = 2;
/// The power source as last seen (the page reads it on its own thread).
static SOURCE: AtomicU8 = AtomicU8::new(UNKNOWN);

/// Note the source; whether it is news.
pub(crate) fn note_source(on_charger: bool) -> bool {
    let now = if on_charger { CHARGER } else { BATTERY };
    SOURCE.swap(now, Ordering::Relaxed) != now
}

/// On the charger (`Some(true)`), the battery, or not seen yet.
pub(crate) fn source() -> Option<bool> {
    match SOURCE.load(Ordering::Relaxed) {
        CHARGER => Some(true),
        BATTERY => Some(false),
        _ => None,
    }
}

/// Whether this computer has a battery (the row is for those that do).
pub(crate) fn has_battery() -> bool {
    if std::env::var_os("WAVERUNNER_RIG_RATES").is_some() {
        return true;
    }
    std::fs::read_dir("/sys/class/power_supply")
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| std::fs::read_to_string(e.path().join("type")).is_ok_and(|t| t.trim() == "Battery"))
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.5
}

/// The rate of `rates` that stands for `hz` (the nearest).
pub(crate) fn nearest(rates: &[f64], hz: f64) -> Option<f64> {
    rates.iter().copied().min_by(|a, b| (a - hz).abs().total_cmp(&(b - hz).abs()))
}

/// The battery's rate when nothing was chosen: the lowest that is still 60
/// or more (48 on a panel that has 48 and 60 is not a saving anyone asked
/// for). `None`: no such rate.
pub(crate) fn battery_default(rates: &[f64]) -> Option<f64> {
    rates.iter().copied().filter(|hz| *hz > 59.5).min_by(f64::total_cmp)
}

/// The rate the screen should be at, or `None` to leave it as it is.
/// `chosen`: its owner chose its rate (a saved rule, the Refresh rate row);
/// `rates`: the panel's at the size in use, ascending; `now`: the one on.
///
/// On the charger a screen nobody chose a rate for runs at its TOP rate
/// (Max, 2026-11-15: "if a user have a 240hz monitor … they want their 240"),
/// as the battery's is the lowest of 60 or more: the rate kept when the cable
/// was pulled counts only for a chosen one. (Kept for every screen, it held
/// one at 60 — the compositor's old default, never anybody's choice.)
pub(crate) fn target(on_charger: bool, choice: &RefreshChoice, chosen: bool, rates: &[f64], now: f64) -> Option<f64> {
    if rates.len() < 2 {
        return None;
    }
    let plugged = if chosen { choice.plugged.and_then(|hz| nearest(rates, hz)) } else { rates.last().copied() };
    let want = if on_charger {
        plugged
    } else {
        match choice.battery {
            Some(hz) if hz <= 0.0 => plugged,
            Some(hz) => nearest(rates, hz),
            // (Never UP to it: a screen the owner keeps under 60.)
            None => battery_default(rates).filter(|hz| *hz < now - 0.5),
        }
    }?;
    (!same(want, now)).then_some(want)
}

/// Whether a panel's EDID says it can vary its rate: a range descriptor
/// whose lowest vertical rate is well under its highest.
pub(crate) fn edid_dynamic(edid: &[u8]) -> bool {
    if edid.len() < 128 {
        return false;
    }
    (54..=108).step_by(18).any(|at| {
        let d = &edid[at..at + 18];
        if d[..3] != [0, 0, 0] || d[3] != 0xFD {
            return false;
        }
        // (Byte 4: the rates' offsets, +255 for those past a byte.)
        let (lo, hi) = (u16::from(d[5]), u16::from(d[6]));
        let (lo, hi) = match d[4] & 0b11 {
            0b10 => (lo, hi + 255),
            0b11 => (lo + 255, hi + 255),
            _ => (lo, hi),
        };
        lo > 0 && hi >= lo + 10
    })
}

/// Whether the screen on connector `name` can do Dynamic: what the
/// compositor `said` (Golem's Hyprland: `vrrCapable`, the kernel's own
/// word — a range in the EDID is not adaptive sync: many 60 Hz screens
/// list one), else the EDID.
pub(crate) fn can_dynamic(name: &str, said: Option<bool>) -> bool {
    if std::env::var_os("WAVERUNNER_RIG_RATES").is_some() {
        return true;
    }
    if let Some(said) = said {
        return said;
    }
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return false;
    }
    let suffix = format!("-{name}");
    std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(&suffix))
        .any(|e| std::fs::read(e.path().join("edid")).is_ok_and(|bytes| edid_dynamic(&bytes)))
}

/// A rig's made-up rates (`WAVERUNNER_RIG_RATES=60,165`): a headless
/// output lists one mode, and these rows need two.
pub(crate) fn rig_rates() -> Vec<f64> {
    std::env::var("WAVERUNNER_RIG_RATES")
        .ok()
        .map(|t| t.split(',').filter_map(|w| w.trim().parse::<f64>().ok()).filter(|hz| (1.0..=1000.0).contains(hz)).collect())
        .unwrap_or_default()
}

/// `165 Hz`, `59.94 Hz`.
pub(crate) fn hz_text(hz: f64) -> String {
    if (hz - hz.round()).abs() < 0.01 {
        format!("{hz:.0} Hz")
    } else {
        format!("{hz:.2} Hz")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(dynamic: Option<bool>, battery: Option<f64>, plugged: Option<f64>) -> RefreshChoice {
        RefreshChoice { dynamic, battery, plugged }
    }

    #[test]
    fn unplugged_the_screen_goes_to_sixty_and_back_when_plugged_in() {
        let rates = [60.0, 165.0];
        let nothing = RefreshChoice::default();
        assert_eq!(target(false, &nothing, true, &rates, 165.0), Some(60.0));
        // (Back to what it was at, kept as the battery's went on.)
        let kept = choice(None, None, Some(165.0));
        assert_eq!(target(true, &kept, true, &rates, 60.0), Some(165.0));
        // Already there: left alone.
        assert_eq!(target(false, &nothing, true, &rates, 60.0), None);
        assert_eq!(target(true, &kept, true, &rates, 165.0), None);
        // A chosen screen with nothing kept: nothing to go back to.
        assert_eq!(target(true, &nothing, true, &rates, 60.0), None);
        // An owner's 120 on a 165 screen is what the charger brings back.
        let owners = choice(Some(false), None, Some(120.0));
        assert_eq!(target(true, &owners, true, &[60.0, 120.0, 165.0], 60.0), Some(120.0));
    }

    #[test]
    fn a_screen_nobody_chose_a_rate_for_runs_at_its_top_on_the_charger() {
        let rates = [60.0, 165.0];
        assert_eq!(target(true, &RefreshChoice::default(), false, &rates, 60.0), Some(165.0));
        assert_eq!(target(true, &RefreshChoice::default(), false, &rates, 165.0), None);
        // A 60 kept when the cable was pulled under the compositor's old
        // default is nobody's choice: the top all the same.
        let stale = choice(None, None, Some(60.0));
        assert_eq!(target(true, &stale, false, &rates, 60.0), Some(165.0));
        // The battery's is unchanged by it, and "Same" is the top too.
        assert_eq!(target(false, &stale, false, &rates, 165.0), Some(60.0));
        assert_eq!(target(false, &choice(None, Some(0.0), None), false, &rates, 60.0), Some(165.0));
    }

    #[test]
    fn the_battery_rate_is_the_owners_when_chosen() {
        let rates = [60.0, 120.0, 165.0];
        assert_eq!(target(false, &choice(None, Some(120.0), None), true, &rates, 165.0), Some(120.0));
        // "Same": the charger's, on the battery too.
        assert_eq!(target(false, &choice(None, Some(0.0), Some(165.0)), true, &rates, 165.0), None);
        assert_eq!(target(false, &choice(None, Some(0.0), Some(165.0)), true, &rates, 60.0), Some(165.0));
    }

    #[test]
    fn nothing_chosen_never_goes_under_sixty_nor_up() {
        assert_eq!(battery_default(&[48.0, 60.0]), Some(60.0));
        // 48 and 60: at 60 it stays; at 48 (the owner's) it is not raised.
        assert_eq!(target(false, &RefreshChoice::default(), true, &[48.0, 60.0], 60.0), None);
        assert_eq!(target(false, &RefreshChoice::default(), true, &[48.0, 60.0], 48.0), None);
        // One rate: nothing to choose.
        assert_eq!(target(false, &RefreshChoice::default(), false, &[60.0], 60.0), None);
        assert_eq!(target(true, &RefreshChoice::default(), false, &[60.0], 60.0), None);
        assert_eq!(battery_default(&[30.0, 50.0]), None);
    }

    #[test]
    fn a_rate_row_picked_says_the_screens_adaptive_sync() {
        let mut all = Refresh::new();
        assert_eq!(vrr_of(&all, "eDP-1"), None);
        all.insert("eDP-1".into(), choice(None, Some(60.0), None));
        assert_eq!(vrr_of(&all, "eDP-1"), None);
        all.insert("eDP-1".into(), choice(Some(true), None, None));
        assert_eq!(vrr_of(&all, "eDP-1"), Some(1));
        all.insert("eDP-1".into(), choice(Some(false), None, None));
        assert_eq!(vrr_of(&all, "eDP-1"), Some(2));
    }

    #[test]
    fn an_edid_with_a_range_of_rates_can_do_dynamic() {
        let mut edid = vec![0u8; 128];
        assert!(!edid_dynamic(&edid));
        // A range descriptor in the second slot: 48 to 165 Hz.
        edid[72..77].copy_from_slice(&[0, 0, 0, 0xFD, 0]);
        edid[77] = 48;
        edid[78] = 165;
        assert!(edid_dynamic(&edid));
        // 59 to 61: one rate, with its tolerance.
        edid[77] = 59;
        edid[78] = 61;
        assert!(!edid_dynamic(&edid));
        // Past a byte: 48 to 300 (45 + 255).
        edid[76] = 0b10;
        edid[77] = 48;
        edid[78] = 45;
        assert!(edid_dynamic(&edid));
        assert!(!edid_dynamic(&edid[..100]));
    }

    #[test]
    fn the_compositors_word_on_adaptive_sync_wins_over_the_edid() {
        // (A rig makes every screen able: nothing to tell there.)
        if std::env::var_os("WAVERUNNER_RIG_RATES").is_some() {
            return;
        }
        assert!(can_dynamic("DP-9", Some(true)), "said able, though no EDID is read");
        assert!(!can_dynamic("eDP-1", Some(false)), "said unable, whatever its EDID's range");
        let screen = serde_json::json!([{
            "name": "DP-1", "width": 2560, "height": 1440, "refreshRate": 165.0, "vrrCapable": true,
            "availableModes": ["2560x1440@165.00Hz", "2560x1440@60.00Hz"],
        }]);
        assert_eq!(crate::display::parse_monitors(&screen)[0].vrr_capable, Some(true));
    }

    #[test]
    fn rates_read_as_people_say_them() {
        assert_eq!(hz_text(165.001), "165 Hz");
        assert_eq!(hz_text(59.94), "59.94 Hz");
    }
}

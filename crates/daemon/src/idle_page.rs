//! THE LOCK SCREEN AND SLEEP (Max, 2026-11-06, the Controls gap audit: the
//! screen's lock stood fixed at five minutes and showed nowhere; the idle
//! times lived only in the gear box): Controls → LOCK SCREEN — after how
//! long away the screen locks, the password asked when the computer starts,
//! Lock now — and Controls → SLEEP — the screen off and the computer asleep
//! after how long, the lid.
//!
//! The times are hypridle's: the gear box's own machinery (`gear_pages.rs`
//! `apply_idle`: Golem's config copied with the owner's numbers, a drop-in on
//! the unit), kept in the dock's settings (`Settings::lock_secs`,
//! `screen_off_secs`, `sleep_secs`; `None` = the system's own, `Some(0)` =
//! never) and applied on the UI thread, which owns them
//! (`App::control_pick`, keys `idle-*`). The password at start is the
//! compositor's: the lock screen run as the session comes up
//! (`settings.lua`, `GolemSettings::lock_at_start` — hyprlock as the
//! compositor's child, the one way it survives everything else). Lock now
//! goes the same way, through the compositor.

use crate::controls::{choice_of, group_of, on_off_of, ControlView, Group};
use crate::display::GolemSettings;
use crate::i18n::{fill, tr};

/// The times offered, in seconds (0 = never).
const TIMES: [u32; 7] = [0, 60, 300, 600, 900, 1800, 3600];

/// A time as a word: Never, 5 min, 1 h.
fn time_word(secs: u32) -> String {
    match secs {
        0 => tr("Never").to_owned(),
        s if s % 3600 == 0 => fill(tr("{n} h"), &[("n", &(s / 3600))]),
        s => fill(tr("{n} min"), &[("n", &(s / 60))]),
    }
}

/// The row of times, the one in use lit — the owner's, else the system's
/// own (`base`), else the nearest offered.
fn times_group(caption: &'static str, key: &'static str, now: u32) -> Group {
    let mut stops: Vec<u32> = TIMES.to_vec();
    if !stops.contains(&now) {
        stops.push(now);
        stops.sort_unstable();
    }
    let choices = stops.iter().map(|s| choice_of(time_word(*s), *s == now, s.to_string())).collect();
    Group { labelled: true, ..group_of(Some(caption), key, choices) }
}

/// The system's own times, from Golem's hypridle config: (lock, screen
/// off, sleep).
fn base_times() -> (u32, u32, u32) {
    let base = crate::gear_pages::idle_base_text().unwrap_or_default();
    let (screen, sleep) = crate::gear_pages::idle_base(&base);
    let lock = crate::gear_pages::idle_lock_base(&base);
    (lock.unwrap_or(300), screen.unwrap_or(360), sleep.unwrap_or(900))
}

/// Controls → Lock screen.
pub(crate) fn lock_view(view: &mut ControlView) {
    let dock = crate::settings::Settings::load();
    let (lock, _, _) = base_times();
    let now = dock.lock_secs.unwrap_or(lock);
    view.subtitle = tr(if now == 0 { "The screen never locks by itself." } else { "Away for a while, the screen locks. Your password opens it." }).to_owned();
    view.groups.push(times_group("Lock after", "idle-lock", now));
    let at_start = GolemSettings::load().lock_at_start.unwrap_or(false);
    view.groups.push(on_off_of(Some("Ask the password when the computer starts"), "lock-start", at_start));
    if crate::launch::on_path("hyprlock") {
        view.groups.push(Group { buttons: true, ..group_of(None, "lock-now", vec![choice_of(tr("Lock now"), false, "now")]) });
    }
}

/// Controls → Sleep.
pub(crate) fn sleep_view(view: &mut ControlView) {
    let dock = crate::settings::Settings::load();
    let (_, screen, sleep) = base_times();
    let (screen, sleep) = (dock.screen_off_secs.unwrap_or(screen), dock.sleep_secs.unwrap_or(sleep));
    view.subtitle = tr("Left alone, the screen goes off, then the computer sleeps.").to_owned();
    view.groups.push(times_group("Screen off after", "idle-screen", screen));
    view.groups.push(times_group("Sleep after", "idle-sleep", sleep));
    if !crate::hypr::switch_names().is_empty() {
        view.groups.push(on_off_of(Some("Closing the lid does nothing"), "idle-lid", dock.lid_nothing));
    }
}

/// A pick on the worker (the UI thread takes the `idle-*` keys). True when
/// the key was this page's.
pub(crate) fn apply(key: &str, value: &str) -> bool {
    match key {
        "lock-start" => GolemSettings::change(|s| {
            s.lock_at_start = Some(value == "on");
            s.store();
        }),
        "lock-now" => crate::hypr::dispatch("hl.dsp.exec_cmd(\"sh -c 'pidof hyprlock || hyprlock'\")"),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_not_offered_is_added_and_lit() {
        let g = times_group("Lock after", "idle-lock", 420);
        assert_eq!(g.choices.len(), TIMES.len() + 1);
        assert_eq!(g.choices.iter().find(|c| c.on).map(|c| c.value.as_str()), Some("420"));
        let g = times_group("Lock after", "idle-lock", 0);
        assert_eq!(g.choices.len(), TIMES.len());
        assert!(g.choices[0].on);
    }
}

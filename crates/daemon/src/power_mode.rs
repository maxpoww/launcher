//! THE POWER MODE FOLLOWS THE CHARGER. Plugged in, a computer has no reason to
//! wait: Golem asks for "performance" there and "balanced" on the battery, and
//! what the owner picks (the control panel's Power mode, the gear's Processor
//! page) is remembered FOR THAT SOURCE, so a choice made on the charger is not
//! undone by the next time it is unplugged, or the other way round.
//!
//! Taken from Omarchy (`omarchy-powerprofiles-set`, 2026-10-30). Measured on the
//! dev box, on the charger, balanced then performance (`docs/omarchy.md` in
//! Golem has the whole table): a small job right after idling, the shape of
//! everything a desktop does, 31.8 ms then 13.7 ms; one thread flat out 3.20 s
//! then 2.26 s; all of them 6.58 s then 4.84 s; 44 C then 51 C at rest.
//!
//! The machine's own daemon (`power-profiles-daemon`) does the work and decides
//! nothing about the source; it is asked through `powerprofilesctl`, off the
//! loop. Without it nothing happens.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};

use tracing::{debug, info};

use crate::display::GolemSettings;

const UNKNOWN: u8 = 0;
const CHARGER: u8 = 1;
const BATTERY: u8 = 2;

/// The source as last told by the brain.
static SOURCE: AtomicU8 = AtomicU8::new(UNKNOWN);

/// What a source asks for when the owner has picked nothing for it.
fn by_default(on_charger: bool) -> &'static str {
    if on_charger {
        "performance"
    } else {
        "balanced"
    }
}

/// The mode for a source: the owner's pick, the default, or, where this machine
/// does not have it, balanced (every machine with the daemon has that).
fn wanted(on_charger: bool, picked: Option<&str>, has: &[String]) -> Option<String> {
    let knows = |name: &str| has.iter().any(|h| h == name);
    [picked.unwrap_or(by_default(on_charger)), "balanced"]
        .into_iter()
        .find(|name| knows(name))
        .map(str::to_owned)
}

/// A look at the power supply (often; it costs a compare). Only a change, or
/// the first one, does anything, on a thread of its own.
pub(crate) fn follow(on_charger: bool) {
    // A nested test dock does not change the computer's power mode.
    if crate::palette::rig() {
        return;
    }
    let now = if on_charger { CHARGER } else { BATTERY };
    if SOURCE.swap(now, Ordering::Relaxed) == now {
        return;
    }
    let spawned = std::thread::Builder::new().name("power-mode".into()).spawn(move || {
        let Some(list) = ask(&["list"]) else {
            debug!("power mode: no daemon to ask");
            return;
        };
        let settings = GolemSettings::load();
        let picked = if on_charger { &settings.power_charger } else { &settings.power_battery };
        let Some(mode) = wanted(on_charger, picked.as_deref(), &crate::sys::parse_profiles(&list)) else {
            return;
        };
        if ask(&["get"]).is_some_and(|now| now.trim() == mode) {
            return;
        }
        let source = if on_charger { "the charger" } else { "the battery" };
        info!("power mode: {mode}, on {source}");
        if ask(&["set", &mode]).is_none() {
            debug!("power mode: {mode} was refused");
        }
    });
    if let Err(e) = spawned {
        debug!("power mode: no thread ({e})");
    }
}

/// The owner picked a mode: it is theirs for this source from now on.
pub(crate) fn picked(mode: &str) {
    let source = SOURCE.load(Ordering::Relaxed);
    if source == UNKNOWN {
        return;
    }
    GolemSettings::change(|s| {
        let kept = if source == CHARGER { &mut s.power_charger } else { &mut s.power_battery };
        *kept = Some(mode.to_owned());
        s.store();
    });
}

fn ask(args: &[&str]) -> Option<String> {
    let out = Command::new("powerprofilesctl")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<String> {
        ["power-saver", "balanced", "performance"].map(String::from).to_vec()
    }

    #[test]
    fn the_charger_asks_for_performance_and_the_battery_for_balanced() {
        assert_eq!(wanted(true, None, &all()).as_deref(), Some("performance"));
        assert_eq!(wanted(false, None, &all()).as_deref(), Some("balanced"));
    }

    #[test]
    fn the_owners_pick_stands_for_its_source() {
        assert_eq!(wanted(true, Some("balanced"), &all()).as_deref(), Some("balanced"));
        assert_eq!(wanted(false, Some("power-saver"), &all()).as_deref(), Some("power-saver"));
    }

    #[test]
    fn a_mode_the_machine_lacks_is_balanced() {
        let two = ["power-saver", "balanced"].map(String::from).to_vec();
        assert_eq!(wanted(true, None, &two).as_deref(), Some("balanced"));
        assert_eq!(wanted(true, Some("performance"), &two).as_deref(), Some("balanced"));
        assert_eq!(wanted(true, None, &[]), None);
    }

    #[test]
    fn saved_settings_read_back() {
        let s = GolemSettings { power_charger: Some("balanced".into()), ..Default::default() };
        let json = serde_json::to_string(&s).unwrap();
        let back: GolemSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.power_charger.as_deref(), Some("balanced"));
        assert_eq!(back.power_battery, None);
        assert!(!serde_json::to_string(&GolemSettings::default()).unwrap().contains("power_"));
    }
}

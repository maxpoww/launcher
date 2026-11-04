//! THE SLEEP TIMER (Max, 2026-11-04: "we need another button to the left of
//! the lock one … how about a timer? like turn off in X mins" — and, asked,
//! HIBERNATE when it runs out: nothing is lost, Golem's own "off").
//!
//! The gear box's footer is [timer, lock, power]: the power button's two
//! behind it (sleep, restart) come out over the lock and the timer. Each
//! click on the timer steps it as a television's sleep button does — 15 →
//! 30 → 60 → 90 minutes → off —, the button lit and showing the minutes
//! left while it runs.
//!
//! THE SYSTEM KEEPS IT: a transient user timer (`systemd-run --user
//! --on-active`, unit `golem-sleep-timer`) hibernates (or, where the machine
//! cannot hibernate, sleeps), so it holds through a dock restart; the
//! deadline is kept in a file and read back when the dock starts. A minute
//! before, a notification says so, with Cancel: sent on the session bus and
//! its ActionInvoked waited for (no `notify-send`: the dev box has none).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;

use calloop::timer::{TimeoutAction, Timer};
use tracing::{info, warn};

use crate::App;
use crate::i18n::tr;

/// The steps a click goes through, in minutes; past the last: off.
const STEPS: [u32; 4] = [15, 30, 60, 90];
/// The user unit that does it.
const UNIT: &str = "golem-sleep-timer";
/// How long before the end the notice comes.
const NOTICE: i64 = 60;

/// The timer's state on the loop.
#[derive(Default)]
pub(crate) struct SleepTimer {
    /// When it runs out (unix seconds).
    deadline: Option<i64>,
    /// Each setting counted: a notice due for an earlier one says nothing.
    run: u32,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Where the deadline is kept (for the dock started again).
fn state_file() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let state = std::env::var_os("XDG_STATE_HOME").map_or_else(|| home.join(".local/state"), PathBuf::from);
    state.join("waverunner").join("sleep-timer")
}

/// The next step after `left` minutes shown: the first longer one, or off.
pub(crate) fn next_step(left: Option<u32>) -> Option<u32> {
    match left {
        None => Some(STEPS[0]),
        Some(m) => STEPS.iter().copied().find(|&s| s > m),
    }
}

/// A number as a button's glyph (the buttons hold `&'static str`): made
/// once each.
pub(crate) fn minutes_glyph(m: u32) -> &'static str {
    static GLYPHS: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    let glyphs = GLYPHS.get_or_init(|| (0..=99).map(|n: u32| &*Box::leak(n.to_string().into_boxed_str())).collect());
    glyphs[m.min(99) as usize]
}

/// A notice with a Cancel button, on the session's notification server;
/// whether Cancel was pressed before it closed or `wait` ran out.
fn notice_cancelled(title: &str, body: &str, cancel: &str, wait: Duration) -> bool {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return false;
    };
    runtime
        .block_on(async {
            let conn = zbus::Connection::session().await.ok()?;
            let proxy = zbus::Proxy::new(
                &conn,
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
            )
            .await
            .ok()?;
            // (Listening before the notice goes, so no answer is missed.)
            let mut invoked = proxy.receive_signal("ActionInvoked").await.ok()?;
            let mut closed = proxy.receive_signal("NotificationClosed").await.ok()?;
            let hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
            let id: u32 = proxy
                .call("Notify", &("Golem", 0u32, "appointment-soon", title, body, vec!["cancel", cancel], hints, -1i32))
                .await
                .ok()?;
            let answer = async {
                loop {
                    tokio::select! {
                        Some(m) = invoked.next() => {
                            if let Ok((n, key)) = m.body().deserialize::<(u32, String)>() {
                                if n == id {
                                    return key == "cancel";
                                }
                            }
                        }
                        Some(m) = closed.next() => {
                            if m.body().deserialize::<(u32, u32)>().is_ok_and(|(n, _)| n == id) {
                                return false;
                            }
                        }
                        else => return false,
                    }
                }
            };
            Some(tokio::time::timeout(wait, answer).await.unwrap_or(false))
        })
        .unwrap_or(false)
}

/// systemctl --user, its status.
fn user_systemctl(args: &[&str]) -> bool {
    Command::new("systemctl").arg("--user").args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

impl App {
    /// The minutes left (rounded up), while it runs.
    pub(crate) fn sleep_timer_left(&self) -> Option<u32> {
        let left = self.sleep_timer.deadline? - now();
        (left > 0).then(|| ((left + 59) / 60) as u32)
    }

    /// A click on the timer: its next step (the last one: off).
    pub(crate) fn sleep_timer_step(&mut self) {
        let next = next_step(self.sleep_timer_left());
        self.sleep_timer_set(next);
    }

    /// Set to run out in `minutes` (none: off), in the system.
    pub(crate) fn sleep_timer_set(&mut self, minutes: Option<u32>) {
        self.sleep_timer.run = self.sleep_timer.run.wrapping_add(1);
        self.sleep_timer.deadline = minutes.map(|m| now() + i64::from(m) * 60);
        let file = state_file();
        match self.sleep_timer.deadline {
            Some(at) => {
                if let Some(dir) = file.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let tmp = file.with_extension("tmp");
                if std::fs::write(&tmp, at.to_string()).and_then(|()| std::fs::rename(&tmp, &file)).is_err() {
                    warn!("sleep timer: its deadline not kept");
                }
            }
            None => {
                let _ = std::fs::remove_file(&file);
            }
        }
        // (Hibernate where the machine can; else sleep.)
        let action = if self.power_hibernates() { "hibernate" } else { "suspend" };
        info!("sleep timer: {}", minutes.map_or_else(|| "off".to_owned(), |m| format!("{action} in {m} min")));
        // The system's own timer, made again each time (on a thread: these
        // ask the user manager).
        self.desktop_off_loop(
            move || {
                user_systemctl(&["stop", &format!("{UNIT}.timer")]);
                user_systemctl(&["reset-failed", &format!("{UNIT}.timer"), &format!("{UNIT}.service")]);
                minutes.is_none_or(|m| {
                    Command::new("systemd-run")
                        .args(["--user", "--quiet", "--collect", "--unit", UNIT, "--on-active", &format!("{m}m")])
                        .args(["--timer-property=AccuracySec=1s", "--description", "Golem: the sleep timer", "systemctl", action])
                        .stdin(Stdio::null())
                        .status()
                        .is_ok_and(|s| s.success())
                })
            },
            |app, ok| {
                if !ok {
                    warn!("sleep timer: the system's timer could not be made");
                    app.sleep_timer.deadline = None;
                    let _ = std::fs::remove_file(state_file());
                }
                app.gear_changed();
            },
        );
        self.sleep_timer_notice_due();
        self.gear_changed();
        // (The minutes on the button go down: the box drawn again as they do.)
        self.sleep_timer_tick();
    }

    /// The dock started: a timer set before it, still running, is shown
    /// again (and its notice due); one that ran out or went is forgotten.
    pub(crate) fn sleep_timer_restore(&mut self) {
        let Some(at) = std::fs::read_to_string(state_file()).ok().and_then(|t| t.trim().parse::<i64>().ok()) else {
            return;
        };
        if at <= now() || !user_systemctl(&["is-active", "--quiet", &format!("{UNIT}.timer")]) {
            let _ = std::fs::remove_file(state_file());
            return;
        }
        info!("sleep timer: still running, {} s left", at - now());
        self.sleep_timer.deadline = Some(at);
        self.sleep_timer_notice_due();
        self.sleep_timer_tick();
    }

    /// The notice, a minute before the end: what will happen, and Cancel.
    fn sleep_timer_notice_due(&mut self) {
        let Some(at) = self.sleep_timer.deadline else {
            return;
        };
        let run = self.sleep_timer.run;
        let wait = (at - NOTICE - now()).max(0) as u64;
        let _ = self.loop_handle.insert_source(Timer::from_duration(Duration::from_secs(wait)), move |_, _, app: &mut App| {
            if app.sleep_timer.run == run && app.sleep_timer_left().is_some() {
                app.sleep_timer_notice(run);
            }
            TimeoutAction::Drop
        });
    }

    /// `debug-gear timer-notice`: the minute-before notice now, for the
    /// running timer (its Cancel is as real as ever).
    pub(crate) fn sleep_timer_notice_now(&mut self) {
        let run = self.sleep_timer.run;
        self.sleep_timer_notice(run);
    }

    fn sleep_timer_notice(&mut self, run: u32) {
        let title = if self.power_hibernates() { tr("Hibernating in 1 min") } else { tr("Going to sleep in 1 min") };
        let (body, cancel) = (tr("The sleep timer ran out"), tr("Cancel"));
        // (Waited for off the loop, a little past the end: the machine goes.)
        let wait = Duration::from_secs(NOTICE as u64 + 30);
        self.desktop_off_loop(
            move || notice_cancelled(title, body, cancel, wait),
            move |app, cancel| {
                if cancel && app.sleep_timer.run == run {
                    info!("sleep timer: cancelled from its notice");
                    app.sleep_timer_set(None);
                }
            },
        );
    }

    /// While it runs: the box drawn again each half minute (the minutes on
    /// the button).
    fn sleep_timer_tick(&mut self) {
        let run = self.sleep_timer.run;
        let _ = self.loop_handle.insert_source(Timer::from_duration(Duration::from_secs(30)), move |_, _, app: &mut App| {
            if app.sleep_timer.run != run || app.sleep_timer_left().is_none() {
                // (Ran out: the machine went and came back; nothing to keep.)
                if app.sleep_timer.run == run {
                    app.sleep_timer.deadline = None;
                    let _ = std::fs::remove_file(state_file());
                }
                app.gear_changed();
                return TimeoutAction::Drop;
            }
            app.gear_changed();
            TimeoutAction::ToDuration(Duration::from_secs(30))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_steps_as_a_tvs_sleep_button() {
        assert_eq!(next_step(None), Some(15));
        assert_eq!(next_step(Some(15)), Some(30));
        assert_eq!(next_step(Some(12)), Some(15));
        assert_eq!(next_step(Some(60)), Some(90));
        assert_eq!(next_step(Some(90)), None);
        assert_eq!(minutes_glyph(30), "30");
    }
}

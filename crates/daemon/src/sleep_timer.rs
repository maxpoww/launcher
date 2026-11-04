//! THE SLEEP TIMER (Max, 2026-11-04: "we need another button to the left of
//! the lock one … how about a timer? like turn off in X mins" — and, asked,
//! HIBERNATE when it runs out: nothing is lost, Golem's own "off").
//!
//! The gear box's footer is [timer, lock, power]: the power button's two
//! behind it (sleep, restart) come out over the lock and the timer. A click
//! opens the footer's field to TYPE the time (Max, 2026-11-04: "quiero
//! introducir el tiempo manualmente"): minutes ("45"), hours and minutes
//! ("1:30", "1h30", "2h"); Enter with nothing typed sets nothing. The words
//! say "turn off" (Max: "only the words, we want it to hibernate"). While it
//! runs the COUNTDOWN stands close beside its hourglass ("15:05"), words on
//! the box, no pill ("no usemos una píldora"); a click on it opens the field
//! with the whole minutes written, no seconds, to edit (Max: "no secs when
//! editing"); the pointer on the hourglass turns it into an X — a click
//! cancels. (Gone, by Max's word: the 15 → 30 → 60 → 90 → off cycle of the
//! first cut, a "+15" beside the countdown, a "(15)" default.)
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

/// No timer runs longer than this.
const LONGEST: i64 = 12 * 60;
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

/// The time typed in the field, in minutes: "45", "1:30", "1h30", "2h",
/// "90m". `None`: nothing typed, not a time, or 0.
pub(crate) fn parse_minutes(text: &str) -> Option<i64> {
    let t: String = text.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return None;
    }
    let num = |s: &str| -> Option<i64> { if s.is_empty() { Some(0) } else { s.parse::<i64>().ok() } };
    let minutes = if let Some((h, m)) = t.split_once(':') {
        num(h)? * 60 + num(m)?
    } else if let Some((h, m)) = t.split_once('h') {
        num(h)? * 60 + num(m.trim_end_matches("min").trim_end_matches('m'))?
    } else {
        num(t.trim_end_matches("min").trim_end_matches('m'))?
    };
    (minutes > 0).then_some(minutes.min(LONGEST))
}

/// The countdown as the button shows it: minutes and seconds ("15:05",
/// "89:59"), with hours from 100 minutes on ("2:05:00").
pub(crate) fn countdown(secs: i64) -> String {
    let secs = secs.max(0);
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if secs >= 100 * 60 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{}:{s:02}", secs / 60)
    }
}

/// The minutes the field is opened with to edit: the whole ones, no seconds
/// ("15:05" → "15"), and never less than one.
pub(crate) fn whole_minutes(secs: i64) -> i64 {
    (secs / 60).max(1)
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
    /// The seconds left, while it runs.
    pub(crate) fn sleep_timer_left(&self) -> Option<i64> {
        let left = self.sleep_timer.deadline? - now();
        (left > 0).then_some(left)
    }

    /// Running: the X — cancel it.
    pub(crate) fn sleep_timer_cancel(&mut self) {
        self.sleep_timer_set(None);
    }

    /// The field's time, typed: set it. Whether it read as a time.
    pub(crate) fn sleep_timer_typed(&mut self, text: &str) -> bool {
        let Some(minutes) = parse_minutes(text) else {
            return false;
        };
        self.sleep_timer_set(Some(now() + minutes * 60));
        true
    }

    /// Set to run out at `until` (unix seconds; none: off), in the system.
    pub(crate) fn sleep_timer_set(&mut self, until: Option<i64>) {
        self.sleep_timer.run = self.sleep_timer.run.wrapping_add(1);
        self.sleep_timer.deadline = until;
        let secs = until.map(|at| (at - now()).max(1));
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
        info!("sleep timer: {}", secs.map_or_else(|| "off".to_owned(), |s| format!("{action} in {}", countdown(s))));
        // The system's own timer, made again each time (on a thread: these
        // ask the user manager).
        self.desktop_off_loop(
            move || {
                user_systemctl(&["stop", &format!("{UNIT}.timer")]);
                user_systemctl(&["reset-failed", &format!("{UNIT}.timer"), &format!("{UNIT}.service")]);
                secs.is_none_or(|s| {
                    Command::new("systemd-run")
                        .args(["--user", "--quiet", "--collect", "--unit", UNIT, "--on-active", &format!("{s}s")])
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
        // (It hibernates; the words say "off", as the timer's own do.)
        let title = if self.power_hibernates() { tr("Turning off in 1 min") } else { tr("Going to sleep in 1 min") };
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

    /// How often the countdown is drawn again: each second while the gear box
    /// shows it, else each half minute (nothing on screen counts down).
    fn sleep_timer_every(&self) -> Duration {
        if self.gear_page() == Some(crate::gear::PageKind::Gear) {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(30)
        }
    }

    /// While it runs: the box drawn again as the countdown goes.
    fn sleep_timer_tick(&mut self) {
        let run = self.sleep_timer.run;
        let every = self.sleep_timer_every();
        let _ = self.loop_handle.insert_source(Timer::from_duration(every), move |_, _, app: &mut App| {
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
            TimeoutAction::ToDuration(app.sleep_timer_every())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_time_reads_as_minutes() {
        assert_eq!(parse_minutes(""), None);
        assert_eq!(parse_minutes("45"), Some(45));
        assert_eq!(parse_minutes(" 45 min "), Some(45));
        assert_eq!(parse_minutes("90m"), Some(90));
        assert_eq!(parse_minutes("1:30"), Some(90));
        assert_eq!(parse_minutes("1h30"), Some(90));
        assert_eq!(parse_minutes("2h"), Some(120));
        assert_eq!(parse_minutes("0"), None);
        assert_eq!(parse_minutes("soon"), None);
        assert_eq!(parse_minutes("100000"), Some(LONGEST));
    }

    #[test]
    fn the_countdown_reads_as_a_clock() {
        assert_eq!(countdown(15 * 60 + 5), "15:05");
        assert_eq!(countdown(59), "0:59");
        assert_eq!(countdown(89 * 60 + 59), "89:59");
        assert_eq!(countdown(125 * 60), "2:05:00");
        assert_eq!(countdown(-3), "0:00");
    }

    #[test]
    fn the_field_edits_whole_minutes() {
        assert_eq!(whole_minutes(15 * 60 + 5), 15);
        assert_eq!(whole_minutes(30), 1);
        assert_eq!(whole_minutes(0), 1);
    }
}

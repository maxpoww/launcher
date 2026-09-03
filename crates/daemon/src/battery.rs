//! Battery OPTION — v2: the bell carries the alarm (per Max's spec).
//!
//! Senses only via the Brain ([`options_engine::SystemMetrics`], `/sys`
//! polled every 3 s by the system collector). Escalation ladder, all
//! states requiring *discharging* (plugging in clears everything):
//!
//! - **≤ 10 %** — one notification ("plug in"), and the bell's accent goes
//!   **red** instead of the usual unread amber (`notif.rs` reads
//!   [`crate::App::battery_pulse`]).
//! - **≤ 7 %** — the red bell **beats slowly** (~2 s period, dt-free sine
//!   driven by a 33 ms frame chain that only runs while beating).
//! - **≤ 5 %** — `systemctl suspend`: the session sleeps instead of dying.
//! - **woken still ≤ 5 % and unplugged** — the bell becomes a red beating
//!   **warning glyph** (the awareness symbol) for [`AWARE_SECS`], then the
//!   machine **hibernates** (logind `CanHibernate` = yes on this box; a
//!   failed hibernate falls back to suspend). Repeats on every wake until
//!   connected — the session can not be lost to a flat battery.
//!
//! Wake detection: Brain snapshots arrive every ~3 s; a gap over
//! [`WAKE_GAP`] means we slept.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use options_engine::ContextState;
use tracing::{info, warn};

use crate::App;

/// Discharging at or below this: red bell + one notification.
pub(crate) const LOW_PCT: u8 = 10;
/// Discharging at or below this: the bell beats.
pub(crate) const BEAT_PCT: u8 = 7;
/// Discharging at or below this: suspend (hibernate after a wake).
pub(crate) const SUSPEND_PCT: u8 = 5;
/// Beat period for the bell pulse.
const BEAT_PERIOD: Duration = Duration::from_millis(2000);
/// How long the awareness symbol shows before the post-wake hibernate.
const AWARE_SECS: u64 = 5;
/// A snapshot gap this large means the machine was asleep.
const WAKE_GAP: Duration = Duration::from_secs(60);
/// No AUTOMATIC sleep within this long of the daemon starting. Shipped without
/// it, the very first snapshot on a drained (or lying — old Acers chronically
/// report "discharging 0%" on AC) battery suspended the machine seconds after
/// the cursor appeared, on every boot, wake after wake: to the person standing
/// there the ISO simply "doesn't boot" (2026-09-03, the Air AND the Acer, the
/// same night). A machine that just booted has a person at it trying to use
/// it; "protecting the session" by killing a seconds-old session protects
/// nothing. All the *warnings* (red bell, beat, notification) still fire
/// instantly — only the ladder's sleep rungs wait.
pub(crate) const BOOT_GRACE: Duration = Duration::from_secs(180);
/// Consecutive Critical snapshots (~3 s apart) required before any auto-sleep:
/// one flaky ACPI read must never put the machine down.
pub(crate) const CRITICAL_STREAK: u8 = 3;

/// The battery alarm ladder (order matters: `>=` comparisons).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum BatteryAlarm {
    #[default]
    None,
    /// ≤10% discharging: red bell accent.
    Low,
    /// ≤7%: red accent beats.
    Beating,
    /// ≤5%: warning glyph replaces the bell; suspend/hibernate ladder runs.
    Critical,
}

/// The alarm rung for one battery reading. Pure: `charging` clears everything
/// (whatever the gauge claims), a missing battery is calm, thresholds are the
/// `<=` ladder above.
pub(crate) fn alarm_for(pct: Option<u8>, charging: bool) -> BatteryAlarm {
    let Some(pct) = pct else {
        return BatteryAlarm::None;
    };
    match pct {
        _ if charging => BatteryAlarm::None,
        p if p <= SUSPEND_PCT => BatteryAlarm::Critical,
        p if p <= BEAT_PCT => BatteryAlarm::Beating,
        p if p <= LOW_PCT => BatteryAlarm::Low,
        _ => BatteryAlarm::None,
    }
}

/// What the sleep rungs do for one snapshot — the decision 92e97e6's two
/// guards protect, extracted pure so every rung and guard is testable (the
/// shipped showstopper class: this is the code path that once put a machine
/// down seconds after boot, and the VM gate can never exercise it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SleepDecision {
    /// No automatic sleep this snapshot.
    Stay,
    /// First crossing while awake (armed): suspend now.
    Suspend,
    /// Woke still critical + unplugged: awareness pause, then hibernate.
    HibernateAfterAware,
}

/// Decide the sleep rung. Both guards precede everything, including the
/// post-wake hibernate: a freshly-booted machine (BOOT_GRACE) or an unproven
/// reading (CRITICAL_STREAK) never sleeps automatically. `armed` is the
/// one-shot latch so a suspend fires once per critical episode.
pub(crate) fn sleep_decision(
    alarm: BatteryAlarm,
    woke: bool,
    armed: bool,
    since_start: Duration,
    critical_streak: u8,
) -> SleepDecision {
    if alarm < BatteryAlarm::Critical {
        return SleepDecision::Stay;
    }
    if since_start < BOOT_GRACE || critical_streak < CRITICAL_STREAK {
        return SleepDecision::Stay;
    }
    if woke {
        return SleepDecision::HibernateAfterAware;
    }
    if armed {
        return SleepDecision::Suspend;
    }
    SleepDecision::Stay
}

impl App {
    /// React to a Brain snapshot's battery state. Called from `on_brain`.
    pub(crate) fn check_battery(&mut self, ctx: &ContextState) {
        // Wake detection first: a long silence between snapshots = we slept.
        let woke = self
            .battery_last_snapshot
            .is_some_and(|t| t.elapsed() > WAKE_GAP);
        self.battery_last_snapshot = Some(Instant::now());

        let alarm = alarm_for(ctx.metrics.battery_pct, ctx.metrics.is_charging);
        let pct = ctx.metrics.battery_pct.unwrap_or(0);
        let prev = self.battery_alarm;
        self.set_battery_alarm(alarm);
        // Streak of consecutive Critical readings — the sleep rungs' evidence.
        self.battery_critical_streak = if alarm == BatteryAlarm::Critical {
            self.battery_critical_streak.saturating_add(1)
        } else {
            0
        };

        // One notification on entering the ladder (not on every state walk).
        if prev == BatteryAlarm::None && alarm >= BatteryAlarm::Low {
            info!("battery: {pct}% discharging — notifying");
            notify(
                &crate::i18n::tr("Battery at {pct}% — plug in soon.")
                    .replace("{pct}", &pct.to_string()),
            );
        }

        // Re-arm whenever we're out of the critical band.
        if alarm < BatteryAlarm::Critical {
            self.battery_suspend_armed = true;
        }
        match sleep_decision(
            alarm,
            woke,
            self.battery_suspend_armed,
            self.battery_started.elapsed(),
            self.battery_critical_streak,
        ) {
            SleepDecision::Stay => {}
            // Critical, woken from sleep still critical+unplugged → awareness
            // symbol for a beat, then hibernate (every wake, per the spec).
            SleepDecision::HibernateAfterAware => {
                warn!("battery: woke at {pct}% unplugged — hibernating in {AWARE_SECS}s");
                self.battery_aware_until = Some(Instant::now() + Duration::from_secs(AWARE_SECS));
                self.draw_options();
                let timer = Timer::from_duration(Duration::from_secs(AWARE_SECS));
                let _ = self
                    .loop_handle
                    .insert_source(timer, |_, _, app: &mut App| {
                        app.battery_aware_until = None;
                        // Still critical and unplugged after the pause? Sleep deep.
                        if app.battery_alarm == BatteryAlarm::Critical {
                            warn!("battery: hibernating to preserve the session");
                            sleep_machine("hibernate");
                        } else {
                            app.draw_options();
                        }
                        TimeoutAction::Drop
                    });
            }
            SleepDecision::Suspend => {
                // First crossing while awake: suspend, once per episode.
                self.battery_suspend_armed = false;
                warn!("battery: {pct}% and discharging — suspending to save the session");
                sleep_machine("suspend");
            }
        }
    }

    /// Transition the alarm state, driving redraws and the beat frame chain.
    fn set_battery_alarm(&mut self, alarm: BatteryAlarm) {
        if self.battery_alarm == alarm {
            return;
        }
        info!("battery alarm: {:?} → {alarm:?}", self.battery_alarm);
        self.battery_alarm = alarm;
        if alarm >= BatteryAlarm::Beating {
            self.battery_beat_epoch.get_or_insert_with(Instant::now);
            self.schedule_battery_beat();
        } else {
            self.battery_beat_epoch = None;
        }
        self.draw_options();
    }

    /// The bell's battery accent: `None` when calm; `Some(strength 0..=1)`
    /// while alarmed — steady `1.0` at Low, a slow eased sine at Beating+.
    /// (`notif.rs` blends the bell toward red by this and swaps the glyph
    /// to the warning symbol at Critical / while the awareness pause runs.)
    pub(crate) fn battery_pulse(&self) -> Option<f32> {
        match self.battery_alarm {
            BatteryAlarm::None => None,
            BatteryAlarm::Low => Some(1.0),
            _ => {
                let t = self
                    .battery_beat_epoch
                    .map_or(0.0, |e| e.elapsed().as_secs_f32());
                let phase = (t / BEAT_PERIOD.as_secs_f32()).fract();
                let tri = 1.0 - (phase * 2.0 - 1.0).abs();
                // Smoothstepped triangle: breathes 0.35..1.0, never vanishes.
                Some(0.35 + 0.65 * (tri * tri * (3.0 - 2.0 * tri)))
            }
        }
    }

    /// Whether the bell should show the awareness/warning glyph: at
    /// Critical, and during the post-wake awareness pause.
    pub(crate) fn battery_warning(&self) -> bool {
        self.battery_alarm == BatteryAlarm::Critical || self.battery_aware_until.is_some()
    }

    /// ~30 fps frame chain that runs only while the bell beats.
    fn schedule_battery_beat(&mut self) {
        if self.battery_beat_pending {
            return;
        }
        self.battery_beat_pending = true;
        let timer = Timer::from_duration(Duration::from_millis(33));
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.battery_beat_pending = false;
                if app.battery_alarm >= BatteryAlarm::Beating {
                    app.draw_options();
                    app.schedule_battery_beat();
                }
                TimeoutAction::Drop
            });
    }
}

/// Fire a desktop notification through the session bus. `busctl` ships with
/// systemd, so it exists on every Golem machine; failure is logged, never
/// fatal.
fn notify(body: &str) {
    let r = std::process::Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            "susssasa{sv}i",
            "waverunner",
            "0",
            "battery-caution",
            crate::i18n::tr("Battery low"),
            body,
            "0",
            "0",
            "10000",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = r {
        warn!("battery: notification failed to spawn: {e}");
    }
}

/// `systemctl suspend|hibernate`, detached; hibernate falls back to suspend
/// if it can't spawn (and logind itself falls back per its config).
fn sleep_machine(mode: &str) {
    match std::process::Command::new("systemctl").arg(mode).spawn() {
        Ok(_) => {}
        Err(e) if mode == "hibernate" => {
            warn!("battery: hibernate failed to spawn ({e}); suspending instead");
            sleep_machine("suspend");
        }
        Err(e) => warn!("battery: {mode} failed to spawn: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ladder's rungs at their exact boundaries, and the two absolute
    /// clears: charging (whatever the gauge claims — old Acers report
    /// "discharging 0%" on AC, so plugging in must beat any percentage), and
    /// a machine with no battery at all (the VM, desktops).
    #[test]
    fn alarm_ladder_boundaries_and_clears() {
        assert_eq!(alarm_for(None, false), BatteryAlarm::None);
        assert_eq!(alarm_for(Some(100), false), BatteryAlarm::None);
        assert_eq!(alarm_for(Some(11), false), BatteryAlarm::None);
        assert_eq!(alarm_for(Some(10), false), BatteryAlarm::Low);
        assert_eq!(alarm_for(Some(8), false), BatteryAlarm::Low);
        assert_eq!(alarm_for(Some(7), false), BatteryAlarm::Beating);
        assert_eq!(alarm_for(Some(6), false), BatteryAlarm::Beating);
        assert_eq!(alarm_for(Some(5), false), BatteryAlarm::Critical);
        assert_eq!(alarm_for(Some(0), false), BatteryAlarm::Critical);
        // Charging clears EVERY rung, including the lying-gauge 0%.
        for pct in [0, 3, 5, 7, 10] {
            assert_eq!(alarm_for(Some(pct), true), BatteryAlarm::None, "{pct}%");
        }
    }

    /// The shipped-showstopper guard (92e97e6): within BOOT_GRACE nothing
    /// sleeps automatically — not the awake suspend, not the post-wake
    /// hibernate — no matter how critical the reading claims to be.
    #[test]
    fn boot_grace_blocks_every_sleep_rung() {
        let just_booted = Duration::from_secs(3); // the first brain snapshot
        for woke in [false, true] {
            assert_eq!(
                sleep_decision(BatteryAlarm::Critical, woke, true, just_booted, 200),
                SleepDecision::Stay,
                "woke={woke}"
            );
        }
        // One tick before the boundary still holds; at the boundary it acts.
        assert_eq!(
            sleep_decision(
                BatteryAlarm::Critical,
                false,
                true,
                BOOT_GRACE - Duration::from_millis(1),
                CRITICAL_STREAK
            ),
            SleepDecision::Stay
        );
        assert_eq!(
            sleep_decision(
                BatteryAlarm::Critical,
                false,
                true,
                BOOT_GRACE,
                CRITICAL_STREAK
            ),
            SleepDecision::Suspend
        );
    }

    /// One flaky ACPI read must never put the machine down: the streak has to
    /// reach CRITICAL_STREAK consecutive snapshots first (a reset streak — a
    /// single good reading in between — starts the count over upstream).
    #[test]
    fn critical_streak_blocks_until_proven() {
        let up = BOOT_GRACE * 2;
        for streak in 0..CRITICAL_STREAK {
            for woke in [false, true] {
                assert_eq!(
                    sleep_decision(BatteryAlarm::Critical, woke, true, up, streak),
                    SleepDecision::Stay,
                    "streak={streak} woke={woke}"
                );
            }
        }
        assert_eq!(
            sleep_decision(BatteryAlarm::Critical, false, true, up, CRITICAL_STREAK),
            SleepDecision::Suspend
        );
    }

    /// Past both guards: waking still-critical hibernates (armed or not —
    /// every wake, per the spec); awake it suspends exactly once per episode
    /// (the armed latch), then stays.
    #[test]
    fn sleep_rungs_past_the_guards() {
        let up = BOOT_GRACE * 2;
        for armed in [true, false] {
            assert_eq!(
                sleep_decision(BatteryAlarm::Critical, true, armed, up, CRITICAL_STREAK),
                SleepDecision::HibernateAfterAware,
                "armed={armed}"
            );
        }
        assert_eq!(
            sleep_decision(BatteryAlarm::Critical, false, true, up, CRITICAL_STREAK),
            SleepDecision::Suspend
        );
        // The latch spent: no second suspend from continued critical reads.
        assert_eq!(
            sleep_decision(BatteryAlarm::Critical, false, false, up, CRITICAL_STREAK),
            SleepDecision::Stay
        );
        // And below Critical nothing ever sleeps, guards or no guards.
        for alarm in [BatteryAlarm::None, BatteryAlarm::Low, BatteryAlarm::Beating] {
            assert_eq!(
                sleep_decision(alarm, true, true, up, 200),
                SleepDecision::Stay,
                "{alarm:?}"
            );
        }
    }
}

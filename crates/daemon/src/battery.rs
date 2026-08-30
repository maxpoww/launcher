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

impl App {
    /// React to a Brain snapshot's battery state. Called from `on_brain`.
    pub(crate) fn check_battery(&mut self, ctx: &ContextState) {
        // Wake detection first: a long silence between snapshots = we slept.
        let woke = self
            .battery_last_snapshot
            .is_some_and(|t| t.elapsed() > WAKE_GAP);
        self.battery_last_snapshot = Some(Instant::now());

        let Some(pct) = ctx.metrics.battery_pct else {
            self.set_battery_alarm(BatteryAlarm::None);
            return;
        };
        let charging = ctx.metrics.is_charging;

        let alarm = match pct {
            _ if charging => BatteryAlarm::None,
            p if p <= SUSPEND_PCT => BatteryAlarm::Critical,
            p if p <= BEAT_PCT => BatteryAlarm::Beating,
            p if p <= LOW_PCT => BatteryAlarm::Low,
            _ => BatteryAlarm::None,
        };
        let prev = self.battery_alarm;
        self.set_battery_alarm(alarm);

        // One notification on entering the ladder (not on every state walk).
        if prev == BatteryAlarm::None && alarm >= BatteryAlarm::Low {
            info!("battery: {pct}% discharging — notifying");
            notify(&format!("Battery at {pct}% — plug in soon."));
        }

        // Re-arm whenever we're out of the critical band.
        if alarm < BatteryAlarm::Critical {
            self.battery_suspend_armed = true;
            return;
        }
        // Critical. Woken from sleep still critical+unplugged → awareness
        // symbol for a beat, then hibernate (every wake, per the spec).
        if woke {
            warn!("battery: woke at {pct}% unplugged — hibernating in {AWARE_SECS}s");
            self.battery_aware_until = Some(Instant::now() + Duration::from_secs(AWARE_SECS));
            self.draw_options();
            let timer = Timer::from_duration(Duration::from_secs(AWARE_SECS));
            let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
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
        } else if self.battery_suspend_armed {
            // First crossing while awake: suspend.
            self.battery_suspend_armed = false;
            warn!("battery: {pct}% and discharging — suspending to save the session");
            sleep_machine("suspend");
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
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
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
            "--user", "call",
            "org.freedesktop.Notifications", "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications", "Notify",
            "susssasa{sv}i",
            "waverunner", "0", "battery-caution", "Battery low", body,
            "0", "0", "10000",
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

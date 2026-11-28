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
//!   driven by a 33 ms frame chain that only runs while beating), and the
//!   machine **sleeps**, once a run down: the sleep IS the message — "plug
//!   in to keep using it" (Max, 2026-11-28). Woken again in this band, the
//!   person came back to it: it stays up.
//! - **≤ 5 %** — the bell becomes a red beating **warning glyph** (the
//!   awareness symbol) for [`AWARE_SECS`], then the machine **hibernates**:
//!   the session goes to disk and comes back whole. Awake or just woken,
//!   every time, until it is plugged in. A hibernate the system refuses (no
//!   swap) is a suspend instead; one that never happens is a suspend after
//!   [`HIBERNATE_PATIENCE`].
//!
//! ASLEEP, NOTHING HERE RUNS. 2026-11-28 the dev box slept at 5 % and woke
//! dead, the session gone: so a sleep that starts on a nearly empty battery
//! is the SYSTEM's to end — Golem's `power/low-battery.nix` sets a wake-up
//! and hibernates the machine if nobody came back by then.
//!
//! Wake detection: a snapshot gap counted on the BOOT clock (which runs
//! while the machine sleeps) and not on the monotonic one (which stops):
//! [`boot_clock`]. With `Instant` alone a sleep never showed, and until
//! 2026-11-28 the post-wake rung had never fired.

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use options_engine::ContextState;
use tracing::{info, warn};

use crate::App;

/// Discharging at or below this: red bell + one notification.
pub(crate) const LOW_PCT: u8 = 10;
/// Discharging at or below this: the bell beats and the machine sleeps
/// (once a run down — the message).
pub(crate) const SLEEP_PCT: u8 = 7;
/// Discharging at or below this: hibernate.
pub(crate) const HIBERNATE_PCT: u8 = 5;
/// Beat period for the bell pulse.
const BEAT_PERIOD: Duration = Duration::from_millis(2000);
/// How long the awareness symbol shows before the hibernate.
const AWARE_SECS: u64 = 5;
/// This much sleep between two snapshots counts as a wake.
const WAKE_GAP: Duration = Duration::from_secs(20);
/// A hibernate asked this long ago that never happened (we are still awake,
/// never slept) is given up for a plain suspend: asleep beats flat.
pub(crate) const HIBERNATE_PATIENCE: Duration = Duration::from_secs(120);
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
/// Consecutive snapshots in the sleep band (~3 s apart) required before any
/// auto-sleep: one flaky ACPI read must never put the machine down.
pub(crate) const SLEEP_STREAK: u8 = 3;

/// The battery alarm ladder (order matters: `>=` comparisons).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum BatteryAlarm {
    #[default]
    None,
    /// ≤10% discharging: red bell accent.
    Low,
    /// ≤7%: red accent beats; the machine sleeps once (the message).
    Beating,
    /// ≤5%: warning glyph replaces the bell; the machine hibernates.
    Critical,
}

/// The alarm rung for one battery reading. Pure: `charging` clears everything
/// (whatever the gauge claims), a missing battery is calm, thresholds are the
/// `<=` ladder above.
pub(crate) fn alarm_for(pct: Option<u8>, charging: bool, on_ac: bool) -> BatteryAlarm {
    let Some(pct) = pct else {
        return BatteryAlarm::None;
    };
    match pct {
        // On wall power the battery is not draining, whatever it reads — a
        // dead battery on AC reports "Not charging 0%", not a real 0% drain
        // (the ASUS X550LC, 2026-09-09). Clears the alarm like charging does.
        _ if charging || on_ac => BatteryAlarm::None,
        p if p <= HIBERNATE_PCT => BatteryAlarm::Critical,
        p if p <= SLEEP_PCT => BatteryAlarm::Beating,
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
    /// Sleep now: the 7 % message, or a 5 % hibernate that never happened.
    Suspend,
    /// 5 % or under, unplugged: awareness pause, then hibernate.
    HibernateAfterAware,
}

/// Decide the sleep rung. Both guards precede everything: a freshly-booted
/// machine (BOOT_GRACE) or an unproven reading (SLEEP_STREAK) never sleeps
/// automatically. `suspend_armed` is the one-shot latch of the 7 % sleep (one
/// a run down); `hibernate_asked` is how long ago, awake, a hibernate was
/// asked for in this run down (None = not yet, or we have slept since).
pub(crate) fn sleep_decision(
    alarm: BatteryAlarm,
    woke: bool,
    suspend_armed: bool,
    hibernate_asked: Option<Duration>,
    since_start: Duration,
    streak: u8,
) -> SleepDecision {
    if alarm < BatteryAlarm::Beating {
        return SleepDecision::Stay;
    }
    if since_start < BOOT_GRACE || streak < SLEEP_STREAK {
        return SleepDecision::Stay;
    }
    if alarm == BatteryAlarm::Critical {
        return match hibernate_asked {
            _ if woke => SleepDecision::HibernateAfterAware,
            None => SleepDecision::HibernateAfterAware,
            // Asked, and we are still here, never having slept: it did not
            // happen. Asleep at least.
            Some(waited) if waited >= HIBERNATE_PATIENCE => SleepDecision::Suspend,
            Some(_) => SleepDecision::Stay,
        };
    }
    // 7 % → 5 %: the sleep is the message, once a run down. Woken in this
    // band, the person came back to it — let them work on until 5 %.
    if !woke && suspend_armed {
        SleepDecision::Suspend
    } else {
        SleepDecision::Stay
    }
}

/// Time since boot INCLUDING time asleep (CLOCK_BOOTTIME). `Instant` is
/// CLOCK_MONOTONIC on Linux, which stops while the machine sleeps.
pub(crate) fn boot_clock() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid out-pointer; CLOCK_BOOTTIME exists on every
    // kernel Golem runs.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) } != 0 {
        return Duration::ZERO;
    }
    Duration::new(
        ts.tv_sec.max(0) as u64,
        ts.tv_nsec.clamp(0, 999_999_999) as u32,
    )
}

/// Whether the machine slept between two snapshots: the boot clock moved
/// further than the monotonic one by more than [`WAKE_GAP`].
pub(crate) fn slept_between(boot_step: Duration, mono_step: Duration) -> bool {
    boot_step.saturating_sub(mono_step) > WAKE_GAP
}

impl App {
    /// React to a Brain snapshot's battery state. Called from `on_brain`.
    pub(crate) fn check_battery(&mut self, ctx: &ContextState) {
        // Wake detection first: time asleep since the last snapshot.
        let (mono, boot) = (Instant::now(), boot_clock());
        let woke = self
            .battery_last_snapshot
            .is_some_and(|(m, b)| slept_between(boot.saturating_sub(b), mono - m));
        self.battery_last_snapshot = Some((mono, boot));

        let alarm = alarm_for(
            ctx.metrics.battery_pct,
            ctx.metrics.is_charging,
            ctx.metrics.on_ac,
        );
        let pct = ctx.metrics.battery_pct.unwrap_or(0);
        let prev = self.battery_alarm;
        self.set_battery_alarm(alarm);
        // Streak of consecutive readings in the sleep band — the sleep rungs'
        // evidence.
        self.battery_sleep_streak = if alarm >= BatteryAlarm::Beating {
            self.battery_sleep_streak.saturating_add(1)
        } else {
            0
        };
        if woke {
            // Whatever was asked before the sleep is over; and a wake in the
            // band is the person back at it — the 7 % message has been seen.
            self.battery_hibernate_asked = None;
            if alarm >= BatteryAlarm::Beating {
                self.battery_suspend_armed = false;
            }
            info!("battery: woke at {pct}% ({alarm:?})");
        }

        // One notification on entering the ladder (not on every state walk).
        if prev == BatteryAlarm::None && alarm >= BatteryAlarm::Low {
            info!("battery: {pct}% discharging — notifying");
            notify(
                &crate::i18n::tr("Battery at {pct}% — plug in soon.")
                    .replace("{pct}", &pct.to_string()),
            );
        }

        // Re-arm whenever we're out of the bands (plugged in, or charged past).
        if alarm < BatteryAlarm::Beating {
            self.battery_suspend_armed = true;
        }
        if alarm < BatteryAlarm::Critical {
            self.battery_hibernate_asked = None;
        }
        match sleep_decision(
            alarm,
            woke,
            self.battery_suspend_armed,
            self.battery_hibernate_asked.map(|t| t.elapsed()),
            self.battery_started.elapsed(),
            self.battery_sleep_streak,
        ) {
            SleepDecision::Stay => {}
            // 5 % or under, unplugged → the awareness symbol for a beat, then
            // hibernate (awake, and again on every wake, per the spec).
            SleepDecision::HibernateAfterAware => {
                warn!("battery: {pct}% unplugged — hibernating in {AWARE_SECS}s");
                self.battery_hibernate_asked = Some(Instant::now());
                self.battery_aware_until = Some(Instant::now() + Duration::from_secs(AWARE_SECS));
                self.draw_options();
                let asked = boot_clock();
                let timer = Timer::from_duration(Duration::from_secs(AWARE_SECS));
                let _ = self
                    .loop_handle
                    .insert_source(timer, move |_, _, app: &mut App| {
                        app.battery_aware_until = None;
                        // Slept during the pause (the system's own low-battery
                        // hibernate, a lid): the world changed — the next
                        // snapshot decides afresh.
                        let slept = slept_between(
                            boot_clock().saturating_sub(asked),
                            Duration::from_secs(AWARE_SECS),
                        );
                        // Still critical and unplugged after the pause? Sleep deep.
                        if !slept && app.battery_alarm == BatteryAlarm::Critical {
                            warn!("battery: hibernating to preserve the session");
                            sleep_machine("hibernate");
                        } else {
                            app.draw_options();
                        }
                        TimeoutAction::Drop
                    });
            }
            SleepDecision::Suspend if alarm == BatteryAlarm::Critical => {
                // The hibernate never happened: asleep beats flat. Asked again
                // from scratch after this.
                self.battery_hibernate_asked = None;
                warn!("battery: {pct}% — the hibernate did not happen; suspending instead");
                sleep_machine("suspend");
            }
            SleepDecision::Suspend => {
                // 7 %, awake: the message — sleep, once a run down.
                self.battery_suspend_armed = false;
                warn!("battery: {pct}% and discharging — sleeping: plug in to keep using it");
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

/// `systemctl suspend|hibernate` on a thread of its own. A hibernate the
/// system refuses (logind says no: no swap, no resume device) is a suspend
/// instead. A test rig runs on the machine itself: it never sleeps it.
fn sleep_machine(verb: &'static str) {
    if std::env::var_os("GOLEM_RIG").is_some() {
        info!("battery: a rig — would {verb}");
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("battery-sleep".into())
        .spawn(move || {
            if !systemctl(verb) && verb == "hibernate" {
                warn!("battery: hibernate refused; suspending instead");
                systemctl("suspend");
            }
        });
    if let Err(e) = spawned {
        warn!("battery: no thread for {verb}: {e}");
    }
}

/// Run `systemctl <verb>`; whether it was accepted.
fn systemctl(verb: &str) -> bool {
    match std::process::Command::new("systemctl").arg(verb).output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            warn!(
                "battery: systemctl {verb}: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            warn!("battery: cannot run systemctl {verb}: {e}");
            false
        }
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
        assert_eq!(alarm_for(None, false, false), BatteryAlarm::None);
        assert_eq!(alarm_for(Some(100), false, false), BatteryAlarm::None);
        assert_eq!(alarm_for(Some(11), false, false), BatteryAlarm::None);
        assert_eq!(alarm_for(Some(10), false, false), BatteryAlarm::Low);
        assert_eq!(alarm_for(Some(8), false, false), BatteryAlarm::Low);
        assert_eq!(alarm_for(Some(7), false, false), BatteryAlarm::Beating);
        assert_eq!(alarm_for(Some(6), false, false), BatteryAlarm::Beating);
        assert_eq!(alarm_for(Some(5), false, false), BatteryAlarm::Critical);
        assert_eq!(alarm_for(Some(0), false, false), BatteryAlarm::Critical);
        // Charging clears EVERY rung, including the lying-gauge 0%.
        for pct in [0, 3, 5, 7, 10] {
            assert_eq!(
                alarm_for(Some(pct), true, false),
                BatteryAlarm::None,
                "{pct}%"
            );
        }
        // On AC clears EVERY rung too: a dead battery on wall power reads
        // "Not charging 0%" (the ASUS X550LC, #38) — not a real drain.
        for pct in [0, 3, 5, 7, 10] {
            assert_eq!(
                alarm_for(Some(pct), false, true),
                BatteryAlarm::None,
                "{pct}% on AC"
            );
        }
    }

    const UP: Duration = Duration::from_secs(3600);

    fn decide(alarm: BatteryAlarm, woke: bool, armed: bool, asked: Option<u64>) -> SleepDecision {
        sleep_decision(
            alarm,
            woke,
            armed,
            asked.map(Duration::from_secs),
            UP,
            SLEEP_STREAK,
        )
    }

    /// The shipped-showstopper guard (92e97e6): within BOOT_GRACE nothing
    /// sleeps automatically — not the 7 % sleep, not the 5 % hibernate — no
    /// matter how low the reading claims to be.
    #[test]
    fn boot_grace_blocks_every_sleep_rung() {
        let just_booted = Duration::from_secs(3); // the first brain snapshot
        for alarm in [BatteryAlarm::Beating, BatteryAlarm::Critical] {
            for woke in [false, true] {
                assert_eq!(
                    sleep_decision(alarm, woke, true, None, just_booted, 200),
                    SleepDecision::Stay,
                    "{alarm:?} woke={woke}"
                );
            }
        }
        // One tick before the boundary still holds; at the boundary it acts.
        let before = BOOT_GRACE - Duration::from_millis(1);
        assert_eq!(
            sleep_decision(
                BatteryAlarm::Beating,
                false,
                true,
                None,
                before,
                SLEEP_STREAK
            ),
            SleepDecision::Stay
        );
        assert_eq!(
            sleep_decision(
                BatteryAlarm::Beating,
                false,
                true,
                None,
                BOOT_GRACE,
                SLEEP_STREAK
            ),
            SleepDecision::Suspend
        );
    }

    /// One flaky ACPI read must never put the machine down: the streak has to
    /// reach SLEEP_STREAK consecutive snapshots first (a reset streak — a
    /// single good reading in between — starts the count over upstream).
    #[test]
    fn streak_blocks_until_proven() {
        for streak in 0..SLEEP_STREAK {
            for alarm in [BatteryAlarm::Beating, BatteryAlarm::Critical] {
                for woke in [false, true] {
                    assert_eq!(
                        sleep_decision(alarm, woke, true, None, UP, streak),
                        SleepDecision::Stay,
                        "streak={streak} {alarm:?} woke={woke}"
                    );
                }
            }
        }
        assert_eq!(
            decide(BatteryAlarm::Beating, false, true, None),
            SleepDecision::Suspend
        );
    }

    /// 7 %: the sleep is the message — once a run down, awake only; a wake in
    /// the band is the person back at it, and they work on.
    #[test]
    fn seven_percent_sleeps_once() {
        assert_eq!(
            decide(BatteryAlarm::Beating, false, true, None),
            SleepDecision::Suspend
        );
        // The latch spent: no second sleep from continued 6-7 % reads.
        assert_eq!(
            decide(BatteryAlarm::Beating, false, false, None),
            SleepDecision::Stay
        );
        // Woken in the band (armed or not): stay up.
        for armed in [true, false] {
            assert_eq!(
                decide(BatteryAlarm::Beating, true, armed, None),
                SleepDecision::Stay
            );
        }
    }

    /// 5 %: hibernate — awake, and on every wake; asked once (the awareness
    /// pause runs meanwhile), and a hibernate that never happened becomes a
    /// suspend after HIBERNATE_PATIENCE.
    #[test]
    fn five_percent_hibernates() {
        for armed in [true, false] {
            for woke in [false, true] {
                assert_eq!(
                    decide(BatteryAlarm::Critical, woke, armed, None),
                    SleepDecision::HibernateAfterAware,
                    "armed={armed} woke={woke}"
                );
            }
        }
        // Asked a moment ago: wait for it (no second pause on the next reads).
        assert_eq!(
            decide(BatteryAlarm::Critical, false, false, Some(3)),
            SleepDecision::Stay
        );
        let patience = HIBERNATE_PATIENCE.as_secs();
        assert_eq!(
            decide(BatteryAlarm::Critical, false, false, Some(patience - 1)),
            SleepDecision::Stay
        );
        // Still here, never slept: it did not happen — asleep at least.
        assert_eq!(
            decide(BatteryAlarm::Critical, false, false, Some(patience)),
            SleepDecision::Suspend
        );
        // A wake always asks again.
        assert_eq!(
            decide(BatteryAlarm::Critical, true, false, Some(patience * 9)),
            SleepDecision::HibernateAfterAware
        );
    }

    /// Above 7 % (or plugged in) nothing ever sleeps, guards or no guards.
    #[test]
    fn calm_never_sleeps() {
        for alarm in [BatteryAlarm::None, BatteryAlarm::Low] {
            for woke in [false, true] {
                assert_eq!(
                    sleep_decision(alarm, woke, true, None, UP, 200),
                    SleepDecision::Stay,
                    "{alarm:?}"
                );
            }
        }
    }

    /// A wake is time the BOOT clock counted and the monotonic one did not:
    /// the 3 s between two snapshots is no wake however the loop was late,
    /// an hour asleep inside those 3 s is.
    #[test]
    fn a_wake_is_boot_time_not_monotonic_time() {
        let s = Duration::from_secs;
        assert!(!slept_between(s(3), s(3)));
        assert!(
            !slept_between(s(90), s(90)),
            "a stalled loop is not a sleep"
        );
        assert!(!slept_between(s(3) + WAKE_GAP, s(3)));
        assert!(slept_between(s(3) + WAKE_GAP + s(1), s(3)));
        assert!(slept_between(s(3603), s(3)), "an hour asleep");
        // The clocks never run backwards, but a bad read must not panic.
        assert!(!slept_between(s(0), s(5)));
        // The boot clock reads, and runs at least as fast as the monotonic.
        let (b0, m0) = (boot_clock(), Instant::now());
        assert!(b0 > Duration::ZERO);
        let (b1, m1) = (boot_clock(), Instant::now());
        assert!(!slept_between(b1 - b0, m1 - m0));
    }
}

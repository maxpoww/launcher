//! AUTO LIGHTING (Max, 2026-10-18): the Lighting slider follows the sun —
//! the day as designed (50), the night down to 20 — and changes only in
//! FRONT OF THE OWNER.
//!
//! The law (Golem §7, "In Front of the User"): a change the SYSTEM starts,
//! that the user is meant to notice, happens only while they are present and
//! settled, visibly and smoothly — never into an empty room. So:
//!
//! - PRESENCE is Wayland's ext-idle-notify-v1: one notification at 120 s
//!   ("away": idled = gone, resumed = back) and one at 3 s ("a quiet
//!   moment"); a running `hyprlock` is "not here" too, and so is a sleep (the
//!   monotonic clock stops in it, the wall clock does not: `slept`).
//! - SETTLED is present for two minutes without a break (since the dock
//!   started, or since the owner came back).
//! - A LOOK at the sun happens when the owner is present, settled and not
//!   under a fullscreen window: once a clock hour while they stay, and at
//!   once when they settle after an absence (or after the dock started).
//!   Hours missed are never replayed: one fade, to now.
//! - The FADE takes 20 s, eased. During it only the SHELL changes, cheaply
//!   (the bar, the boxes, the dock, the desktop's veil); the apps, the web
//!   and the files take the new step once, at the next quiet moment after it
//!   (`colors_apply`, the heavy publish).
//! - What the owner last SAW is what is stored: a restart shows it, and the
//!   change then happens again in front of them.
//!
//! Turning Auto ON is the owner's own act — they are plainly there — so the
//! light goes to the sun's at once. Dragging the Lighting slider turns Auto
//! off: the slider is manual.
//!
//! Everything degrades quietly: no idle protocol, no place on the planet
//! (the timezone gives none) → Auto does nothing on its own; the slider
//! always works. At rest the cost is one 30 s tick; the fade's ~15 frames a
//! second run only for its 20 s, and a frame that lands on the same step
//! redraws nothing.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use calloop::timer::{TimeoutAction, Timer};
use tracing::{debug, info, warn};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{self, ExtIdleNotificationV1};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::{self, ExtIdleNotifierV1};

use crate::palette::{self, ShellPlan};
use crate::App;

/// The light in the day (as designed) and at night.
pub(crate) const DAY: u8 = 50;
pub(crate) const NIGHT: u8 = 20;
/// Away after this long without input; a quiet moment after this long.
const AWAY_MS: u32 = 120_000;
const QUIET_MS: u32 = 3_000;
/// Settled: present this long without a break. (The grace keeps the look off
/// the very instant the idle notification of the same two minutes is sent.)
const SETTLE: Duration = Duration::from_secs(120);
const SETTLE_GRACE: Duration = Duration::from_secs(5);
/// How often the scheduler looks up; the fade's frame; its length.
const TICK: Duration = Duration::from_secs(30);
const FRAME: Duration = Duration::from_millis(66);
const FADE: Duration = Duration::from_secs(20);
/// A change smaller than this is not worth a fade.
const MIN_STEP: u8 = 3;
/// The wall clock ran this much further than the monotonic one between two
/// ticks: the computer slept (or its clock was set).
const SLEPT_GAP: Duration = Duration::from_secs(20);
/// The veil's strongest wash (at Lighting 0).
const VEIL_MAX: f32 = 0.45;

// ------------------------------------------------------------ the pure part

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The Lighting the sun asks for, from its elevation in degrees: the day from
/// +6° up, the night from −12° down (the end of nautical twilight), eased
/// between.
pub(crate) fn curve(elevation_deg: f64) -> u8 {
    if !elevation_deg.is_finite() {
        return DAY;
    }
    let t = smoothstep((6.0 - elevation_deg) / 18.0);
    (f64::from(DAY) + (f64::from(NIGHT) - f64::from(DAY)) * t).round() as u8
}

/// Where the fade is after `elapsed`: eased from `from` to `to`.
pub(crate) fn fade_value(from: u8, to: u8, elapsed: Duration) -> u8 {
    let e = smoothstep(elapsed.as_secs_f64() / FADE.as_secs_f64());
    let v = f64::from(from) + (f64::from(to) - f64::from(from)) * e;
    v.round().clamp(0.0, 100.0) as u8
}

/// The target worth a fade from `current`: a step under [`MIN_STEP`] is none.
pub(crate) fn decide(current: u8, target: u8) -> Option<u8> {
    (current.abs_diff(target) >= MIN_STEP).then_some(target)
}

/// The black wash over the wallpaper at a Lighting: none from 50 up, and
/// 0.45 at 0. Off when the owner did not ask for the background.
pub(crate) fn veil_alpha(lighting: u8, on: bool) -> f32 {
    if !on {
        return 0.0;
    }
    f32::from(50u8.saturating_sub(lighting)) / 50.0 * VEIL_MAX
}

/// Whether the computer slept between two ticks: the monotonic clock does
/// not run in a sleep, the wall clock does.
pub(crate) fn slept(mono: Duration, wall: Duration) -> bool {
    wall > mono + SLEPT_GAP
}

/// The clock hour of a moment in this time zone (`gmtoff` seconds east): the
/// same number all through a wall-clock hour.
pub(crate) fn hour_key(unix: i64, gmtoff: i64) -> i64 {
    (unix + gmtoff).div_euclid(3600)
}

/// What the scheduler is told at each tick.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Facts {
    /// Since some fixed start (the dock's).
    pub at: Duration,
    pub hour: i64,
    pub auto: bool,
    /// The owner is at the screen (not away, not locked, and the means of
    /// knowing are there).
    pub present: bool,
    /// A fullscreen window covers the bar.
    pub fullscreen: bool,
    pub fading: bool,
}

/// WHEN TO LOOK AT THE SUN: a small state machine, told the facts at each
/// tick. It says "look now" or not; the look itself (the curve, the fade) is
/// the caller's.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// Present without a break since (None: not present).
    since: Option<Duration>,
    /// The clock hour last looked at. None: look at once when settled.
    last: Option<i64>,
}

impl Plan {
    pub(crate) fn step(&mut self, f: &Facts) -> bool {
        if !f.auto || !f.present {
            self.absent();
            return false;
        }
        let since = *self.since.get_or_insert(f.at);
        if f.at.saturating_sub(since) < SETTLE + SETTLE_GRACE {
            return false;
        }
        // Not now, but not lost: the look stays owed until the room is right.
        if f.fullscreen || f.fading {
            return false;
        }
        if self.last == Some(f.hour) {
            return false;
        }
        self.last = Some(f.hour);
        true
    }

    /// The owner is gone: the next time they settle, look at once.
    pub(crate) fn absent(&mut self) {
        self.since = None;
        self.last = None;
    }

    /// The owner is back at `at`: settled two minutes from now.
    pub(crate) fn back(&mut self, at: Duration) {
        self.since = Some(at);
        self.last = None;
    }

    /// A look was taken outside the schedule (Auto just turned on).
    pub(crate) fn looked(&mut self, hour: i64) {
        self.last = Some(hour);
    }
}

// --------------------------------------------------- what the drawing reads

/// 255: no fade in progress.
const NONE: u8 = 255;
/// The Lighting a fade shows now.
static SHOWN: AtomicU8 = AtomicU8::new(NONE);
/// The Lighting the apps, the web and the files were last made for.
static APPLIED: AtomicU8 = AtomicU8::new(NONE);
static VEIL: AtomicBool = AtomicBool::new(false);

/// The Lighting a fade in progress shows, else None (the saved one).
pub(crate) fn shown() -> Option<u8> {
    Some(SHOWN.load(Ordering::Relaxed)).filter(|v| *v != NONE)
}

/// Whether the Lighting moved since the last time everything was made for it.
pub(crate) fn applied(v: u8) -> bool {
    APPLIED.swap(v, Ordering::Relaxed) != v
}

/// The background switch, as the choice has it; whether it changed.
pub(crate) fn set_veil(on: bool) -> bool {
    VEIL.swap(on, Ordering::Relaxed) != on
}

/// The wash over the wallpaper now (0: none).
pub(crate) fn veil_now() -> f32 {
    veil_alpha(palette::tint::lighting(), VEIL.load(Ordering::Relaxed))
}

// -------------------------------------------------------- the clock and /proc

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// This zone's offset east of UTC at `unix`, seconds.
fn gmtoff_at(unix: i64) -> i64 {
    // SAFETY: `localtime_r` writes only into the `tm` we give it.
    unsafe {
        let t = unix as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() { 0 } else { tm.tm_gmtoff as i64 }
    }
}

/// Whether a process of that `comm` exists — a `/proc` sweep, no subprocess.
fn process_running(name: &str) -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return false;
    };
    dir.flatten().any(|entry| {
        let file = entry.file_name();
        let Some(pid) = file.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit())) else {
            return false;
        };
        std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim_end() == name)
    })
}

// ------------------------------------------------------------- the live part

/// Which of the two notifications spoke.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    Away,
    Quiet,
}

/// The two notifications on the seat, kept alive.
struct Watch {
    _away: ExtIdleNotificationV1,
    _quiet: ExtIdleNotificationV1,
}

/// A fade in progress.
struct Fade {
    from: u8,
    to: u8,
    began: Instant,
    /// The step last put on the screen.
    shown: u8,
    /// How the shell's pair is made at each step (made once, at the start).
    shell: ShellPlan,
    frames: u32,
}

pub(crate) struct Lighting {
    notifier: Option<ExtIdleNotifierV1>,
    watch: Option<Watch>,
    origin: Instant,
    plan: Plan,
    /// Auto, as the choice has it.
    on: bool,
    away: bool,
    quiet: bool,
    fade: Option<Fade>,
    /// The frame timer is running.
    frames: bool,
    /// A fade ended: the apps, the web and the files are owed the new step.
    apply_pending: bool,
    last_tick: Option<(Instant, SystemTime)>,
}

impl Lighting {
    pub(crate) fn new(notifier: Option<ExtIdleNotifierV1>) -> Self {
        Self {
            notifier,
            watch: None,
            origin: Instant::now(),
            plan: Plan::default(),
            on: false,
            away: false,
            quiet: false,
            fade: None,
            frames: false,
            apply_pending: false,
            last_tick: None,
        }
    }

    /// The owner is known to be gone: away, or the screen is locked.
    fn gone(&self) -> bool {
        self.away || process_running("hyprlock")
    }

    /// The owner is at the screen: it can be known, and they are not gone.
    fn present(&self) -> bool {
        self.watch.is_some() && !self.gone()
    }

    fn since_start(&self) -> Duration {
        self.origin.elapsed()
    }
}

impl App {
    /// The seat is there: ask to be told when the owner goes and comes. The
    /// quiet moment ignores idle inhibitors (a film playing is not input) when
    /// the compositor can; the away one respects them (a film being watched is
    /// someone there).
    pub(crate) fn lighting_watch(&mut self, qh: &QueueHandle<App>, seat: &WlSeat) {
        let l = &mut self.lighting;
        let (Some(n), None) = (l.notifier.as_ref(), l.watch.as_ref()) else {
            return;
        };
        let away = n.get_idle_notification(AWAY_MS, seat, qh, Kind::Away);
        let quiet = if n.version() >= 2 {
            n.get_input_idle_notification(QUIET_MS, seat, qh, Kind::Quiet)
        } else {
            n.get_idle_notification(QUIET_MS, seat, qh, Kind::Quiet)
        };
        l.watch = Some(Watch { _away: away, _quiet: quiet });
        debug!("lighting: watching the seat for presence");
    }

    /// The choice's Auto, as `colors_apply` read it.
    pub(crate) fn lighting_mirror(&mut self, on: bool) {
        if self.lighting.on != on {
            self.lighting.on = on;
            self.lighting.plan.absent();
        }
    }

    /// Start the 30 s tick (once, at the dock's start).
    pub(crate) fn lighting_start(&mut self) {
        if self.lighting.notifier.is_none() {
            debug!("lighting: no idle-notify; Auto will not move the light by itself");
        }
        // The dock has just started and the owner is here: settled two
        // minutes from now.
        let at = self.lighting.since_start();
        self.lighting.plan.back(at);
        let timer = Timer::from_duration(TICK);
        let inserted = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            app.lighting_tick();
            TimeoutAction::ToDuration(TICK)
        });
        if let Err(e) = inserted {
            warn!("lighting: tick timer failed ({e}); Auto will not move the light by itself");
        }
    }

    fn lighting_tick(&mut self) {
        let (now, wall) = (Instant::now(), SystemTime::now());
        let slept_now = self.lighting.last_tick.is_some_and(|(m, w)| slept(now.duration_since(m), wall.duration_since(w).unwrap_or_default()));
        self.lighting.last_tick = Some((now, wall));
        if slept_now {
            debug!("lighting: the computer slept; the owner has to settle again");
            self.lighting.plan.absent();
        }
        // What a fade owes the apps, if the room has gone quiet since.
        self.lighting_apply_if_due();
        if !self.lighting.on {
            return;
        }
        let unix = unix_now();
        let facts = Facts {
            at: self.lighting.since_start(),
            hour: hour_key(unix, gmtoff_at(unix)),
            auto: true,
            present: self.lighting.present(),
            fullscreen: self.options_paused(),
            fading: self.lighting.fade.is_some(),
        };
        if self.lighting.plan.step(&facts) {
            self.lighting_look();
        }
    }

    /// Auto was just turned on, by the owner: the light goes to the sun's now.
    pub(crate) fn lighting_auto_on(&mut self) {
        let unix = unix_now();
        self.lighting.plan.looked(hour_key(unix, gmtoff_at(unix)));
        self.lighting_look();
    }

    /// Look at the sun: a fade to its light if that is far enough from this.
    fn lighting_look(&mut self) {
        let Some(elevation) = options_engine::sun_elevation_deg(unix_now() as f64) else {
            debug!("lighting: no place on the planet from the time zone; Auto has nothing to follow");
            return;
        };
        let target = curve(elevation);
        let from = self.lighting.fade.as_ref().map_or_else(palette::tint::lighting, |f| f.shown);
        match decide(from, target) {
            Some(to) => self.lighting_fade(from, to),
            None => debug!("lighting: sun at {elevation:.1}° asks {target}, shown {from}: close enough"),
        }
    }

    fn lighting_fade(&mut self, from: u8, to: u8) {
        info!("lighting: {from} -> {to} over {} s", FADE.as_secs());
        let shell = palette::shell_plan(&palette::choice(), crate::look::dark());
        SHOWN.store(from, Ordering::Relaxed);
        self.lighting.fade = Some(Fade { from, to, began: Instant::now(), shown: from, shell, frames: 0 });
        self.lighting.apply_pending = false;
        if self.lighting.frames {
            return;
        }
        let timer = Timer::from_duration(FRAME);
        let inserted = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            if app.lighting_frame() {
                TimeoutAction::ToDuration(FRAME)
            } else {
                app.lighting.frames = false;
                TimeoutAction::Drop
            }
        });
        match inserted {
            Ok(_) => self.lighting.frames = true,
            Err(e) => {
                warn!("lighting: frame timer failed ({e}); the light changes at once");
                self.lighting_finish(to);
            }
        }
    }

    /// One frame of the fade. False: it is over (or was ended).
    fn lighting_frame(&mut self) -> bool {
        let Some(f) = self.lighting.fade.as_mut() else {
            return false;
        };
        f.frames += 1;
        let elapsed = f.began.elapsed();
        let v = fade_value(f.from, f.to, elapsed);
        let (to, before, frames) = (f.to, f.shown, f.frames);
        let done = elapsed >= FADE;
        if v != before {
            f.shown = v;
            palette::tint::set_lighting(v);
            SHOWN.store(v, Ordering::Relaxed);
            // Only the shell changes, and only when the step did: ~30 steps
            // in all, not 300 frames. (The apps are made for it afterwards.)
            let follows = f.shell.follows_lighting();
            if follows {
                palette::store_shell(f.shell.pair(v));
            }
            let veil_moved = veil_alpha(before, VEIL.load(Ordering::Relaxed)) != veil_alpha(v, VEIL.load(Ordering::Relaxed));
            if follows {
                self.draw_options();
                self.draw();
            }
            if veil_moved {
                self.request_desktop_draw();
            }
        }
        if done {
            self.lighting_finish(to);
            return false;
        }
        // The room is looked at once a second: the owner left, or locked the
        // screen — the light stays where it is, and settles with them again.
        // (`gone`, not `present`: a fade the owner asked for needs no idle
        // protocol to run.)
        if frames % 15 == 0 && self.lighting.gone() {
            debug!("lighting: nobody here; the fade stops");
            self.lighting.plan.absent();
            self.lighting_stop();
            return false;
        }
        true
    }

    /// The fade reached `to`: that is what the owner last saw, so it is what
    /// is kept; the apps, the web and the files take it at a quiet moment.
    fn lighting_finish(&mut self, to: u8) {
        self.lighting.fade = None;
        SHOWN.store(NONE, Ordering::Relaxed);
        palette::tint::set_lighting(to);
        store(to);
        self.lighting.apply_pending = true;
        self.lighting_apply_if_due();
    }

    /// The fade ended where it stands (the owner left): keep what they saw.
    fn lighting_stop(&mut self) {
        if let Some(v) = self.lighting_cancel() {
            palette::tint::set_lighting(v);
            store(v);
            self.lighting.apply_pending = true;
            self.lighting_apply_if_due();
        }
    }

    /// End a fade in progress without keeping anything; the step it showed,
    /// if there was one. (The owner's own hand takes over.)
    pub(crate) fn lighting_cancel(&mut self) -> Option<u8> {
        self.lighting.apply_pending = false;
        SHOWN.store(NONE, Ordering::Relaxed);
        self.lighting.fade.take().map(|f| f.shown)
    }

    /// Make everything for the new step once it is quiet — or at once when
    /// there is no way to tell.
    fn lighting_apply_if_due(&mut self) {
        let l = &self.lighting;
        if l.apply_pending && l.fade.is_none() && (l.quiet || l.watch.is_none()) {
            self.lighting.apply_pending = false;
            self.colors_apply(false);
        }
    }

    /// The 120 s notification: the owner went, or came back.
    fn lighting_away(&mut self, away: bool) {
        self.lighting.away = away;
        if away {
            self.lighting.plan.absent();
            self.lighting_stop();
        } else {
            let at = self.lighting.since_start();
            self.lighting.plan.back(at);
        }
        debug!("lighting: owner {}", if away { "away" } else { "back" });
    }

    /// The 3 s notification: the room is quiet, or it is not.
    fn lighting_quiet(&mut self, quiet: bool) {
        self.lighting.quiet = quiet;
        if quiet {
            self.lighting_apply_if_due();
        }
    }
}

/// Keep `v` as the Lighting the owner last saw.
fn store(v: u8) {
    crate::display::GolemSettings::change(|s| {
        let mut c = s.colors.clone().unwrap_or_default();
        c.lighting = Some(v);
        s.colors = Some(c);
        s.store();
    });
}

// ---------------------------------------------------------------- Wayland

impl Dispatch<ExtIdleNotifierV1, ()> for App {
    fn event(_: &mut Self, _: &ExtIdleNotifierV1, _: ext_idle_notifier_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtIdleNotificationV1, Kind> for App {
    fn event(app: &mut Self, _: &ExtIdleNotificationV1, event: ext_idle_notification_v1::Event, kind: &Kind, _: &Connection, _: &QueueHandle<Self>) {
        use ext_idle_notification_v1::Event;
        match (kind, event) {
            (Kind::Away, Event::Idled) => app.lighting_away(true),
            (Kind::Away, Event::Resumed) => app.lighting_away(false),
            (Kind::Quiet, Event::Idled) => app.lighting_quiet(true),
            (Kind::Quiet, Event::Resumed) => app.lighting_quiet(false),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// An owner who is there, in the room, at `at` seconds of the hour `hour`.
    fn here(at: u64, hour: i64) -> Facts {
        Facts { at: secs(at), hour, auto: true, present: true, fullscreen: false, fading: false }
    }

    #[test]
    fn the_sun_sets_the_light_day_to_night() {
        assert_eq!(curve(20.0), 50, "high sun: as designed");
        assert_eq!(curve(6.0), 50, "the day starts at +6°");
        assert_eq!(curve(0.0), 42, "the horizon: a third of the way down, eased");
        assert_eq!(curve(-6.0), 28);
        assert_eq!(curve(-12.0), 20, "the night starts at -12°");
        assert_eq!(curve(-20.0), 20);
        assert_eq!(curve(f64::NAN), 50, "no reading: the day");
        // The sun sinking from +20° to -20° never raises the light.
        let mut prev = DAY;
        for tenth in (-200..=200).rev() {
            let v = curve(f64::from(tenth) / 10.0);
            assert!(v <= prev, "the light never rises as the sun sinks");
            assert!((NIGHT..=DAY).contains(&v));
            prev = v;
        }
    }

    #[test]
    fn the_fade_is_eased_and_lands_exactly() {
        assert_eq!(fade_value(50, 20, secs(0)), 50);
        assert_eq!(fade_value(50, 20, secs(10)), 35, "the middle");
        assert_eq!(fade_value(50, 20, secs(20)), 20);
        assert_eq!(fade_value(50, 20, secs(60)), 20, "late frames stay at the end");
        // Slow at both ends: the first second moves less than a linear 1/20.
        assert!(50 - fade_value(50, 20, secs(1)) <= 1);
        assert!(fade_value(20, 50, secs(1)) <= 21);
        let mut prev = 50;
        for ms in (0..=20_000).step_by(66) {
            let v = fade_value(50, 20, Duration::from_millis(ms));
            assert!(v <= prev, "monotone");
            prev = v;
        }
    }

    #[test]
    fn a_small_step_is_not_worth_a_fade() {
        assert_eq!(decide(50, 49), None);
        assert_eq!(decide(50, 48), None);
        assert_eq!(decide(50, 47), Some(47));
        assert_eq!(decide(20, 22), None);
        assert_eq!(decide(20, 23), Some(23));
        assert_eq!(decide(30, 30), None);
    }

    #[test]
    fn the_veil_is_a_wash_below_fifty_and_only_when_asked() {
        assert_eq!(veil_alpha(80, true), 0.0);
        assert_eq!(veil_alpha(50, true), 0.0);
        assert!((veil_alpha(0, true) - 0.45).abs() < 1e-6);
        assert!((veil_alpha(25, true) - 0.225).abs() < 1e-6);
        assert_eq!(veil_alpha(0, false), 0.0, "the switch off draws nothing");
    }

    #[test]
    fn a_sleep_is_the_wall_clock_running_ahead() {
        assert!(!slept(secs(30), secs(30)));
        assert!(!slept(secs(30), secs(31)));
        assert!(slept(secs(30), secs(3600)));
        assert!(!slept(secs(31), secs(5)), "a clock set back is no sleep");
    }

    #[test]
    fn the_hour_is_the_walls_hour() {
        // (Any instant on a UTC hour's start: only the differences count.)
        let hour_start = 1_792_300_800 - 1_792_300_800 % 3600;
        assert_eq!(hour_key(hour_start + 3599, 0), hour_key(hour_start, 0));
        assert_eq!(hour_key(hour_start + 3600, 0), hour_key(hour_start, 0) + 1);
        // In a zone half an hour off, the hour turns at the half.
        assert_eq!(hour_key(hour_start + 1799, 1800), hour_key(hour_start, 1800));
        assert_eq!(hour_key(hour_start + 1800, 1800), hour_key(hour_start, 1800) + 1);
        assert_eq!(hour_key(-1, 0), -1, "before the epoch rounds down");
    }

    #[test]
    fn nobody_here_nothing_moves() {
        let mut plan = Plan::default();
        for t in [0, 130, 500, 4000] {
            let f = Facts { present: false, ..here(t, 10) };
            assert!(!plan.step(&f), "away at {t}");
        }
    }

    #[test]
    fn auto_off_nothing_moves() {
        let mut plan = Plan::default();
        for t in [0, 130, 500, 4000] {
            let f = Facts { auto: false, ..here(t, 10 + (t / 3600) as i64) };
            assert!(!plan.step(&f), "auto off at {t}");
        }
    }

    #[test]
    fn back_and_settled_looks_once_then_once_an_hour() {
        let mut plan = Plan::default();
        // The owner settles in: nothing for two minutes…
        assert!(!plan.step(&here(0, 10)));
        assert!(!plan.step(&here(60, 10)));
        assert!(!plan.step(&here(120, 10)), "two minutes exactly is not yet past the grace");
        // …then ONE look, at once.
        assert!(plan.step(&here(150, 10)));
        // The same hour: never again, however long they stay.
        for t in [180, 600, 1800, 3000] {
            assert!(!plan.step(&here(t, 10)), "same hour at {t}");
        }
        // The first tick of the next hour: one look.
        assert!(plan.step(&here(3630, 11)));
        assert!(!plan.step(&here(3660, 11)));
    }

    #[test]
    fn hours_missed_are_not_replayed() {
        let mut plan = Plan::default();
        assert!(!plan.step(&here(0, 10)));
        assert!(plan.step(&here(150, 10)));
        // They leave for five hours.
        plan.absent();
        assert!(!plan.step(&Facts { present: false, ..here(1000, 12) }));
        assert!(!plan.step(&Facts { present: false, ..here(17_000, 15) }));
        // They are back: settled in two minutes, then ONE look — not one per missed hour.
        plan.back(secs(17_100));
        assert!(!plan.step(&here(17_150, 15)));
        assert!(plan.step(&here(17_230, 15)));
        assert!(!plan.step(&here(17_260, 15)));
    }

    #[test]
    fn a_fullscreen_window_makes_the_look_wait_not_vanish() {
        let mut plan = Plan::default();
        assert!(!plan.step(&here(0, 10)));
        let full = |t, hour| Facts { fullscreen: true, ..here(t, hour) };
        assert!(!plan.step(&full(150, 10)), "a film: not now");
        assert!(!plan.step(&full(4000, 11)), "still not, hours later");
        // The film ends: the look comes, once, for NOW.
        assert!(plan.step(&here(4030, 11)));
        assert!(!plan.step(&here(4060, 11)));
    }

    #[test]
    fn a_fade_in_progress_is_not_started_again() {
        let mut plan = Plan::default();
        assert!(!plan.step(&here(0, 10)));
        assert!(!plan.step(&Facts { fading: true, ..here(150, 10) }));
        assert!(plan.step(&here(180, 10)), "still owed once it is over");
    }

    #[test]
    fn leaving_and_returning_must_settle_again() {
        let mut plan = Plan::default();
        assert!(!plan.step(&here(0, 10)));
        assert!(plan.step(&here(150, 10)));
        // A short absence in the same hour: on return a look happens again
        // (the light may have moved while they were away), after settling.
        plan.absent();
        plan.back(secs(400));
        assert!(!plan.step(&here(450, 10)));
        assert!(plan.step(&here(530, 10)));
    }

    // One test: these are process-wide flags, and tests run side by side.
    #[test]
    fn what_the_drawing_reads() {
        assert_eq!(shown(), None);
        assert!(set_veil(true));
        assert!(!set_veil(true), "no change the second time");
        assert!(set_veil(false));
        assert!(applied(37), "the first time, and whenever it moves");
        assert!(!applied(37));
        assert!(applied(38));
    }
}

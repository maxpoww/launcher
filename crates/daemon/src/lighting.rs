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

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use calloop::timer::{TimeoutAction, Timer};
use tracing::{debug, info, warn};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{self, ExtIdleNotificationV1};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::{self, ExtIdleNotifierV1};

use crate::palette::{self, ShellPlan};
use crate::App;

/// The light in the day (as designed) and at night, within the Dark look;
/// the LINE (docs/colours-model.md) also has the Light look by day: see [`line`].
pub(crate) const DAY: u8 = 50;
pub(crate) const NIGHT: u8 = 20;
/// The leap over the gap between the looks, in a fade that crosses it: the
/// shell's two looks crossfade this long.
const GAP: Duration = Duration::from_secs(8);
/// DUSK (Max, 2026-10-22: "the sunset does not last 20 seconds, it should be
/// smooth during 20 min, stronger than during the day… i dont want jumps at
/// all"): in its last degrees above the horizon the light goes on down to
/// its floor (Light 0), and in its first below it the dark starts from its
/// lightest (Dark 100) — the two looks at their nearest where they meet, the
/// darkening stronger than the day's on both sides: ~12 min above, ~25 below
/// at this latitude.
const DUSK_ABOVE: f64 = 2.5;
const DUSK_BELOW: f64 = 5.0;
/// The sun is looked at this often (seconds): tiny steps, never a jump
/// (Max, 2026-10-20: "slow and smooth all the time"; 2026-10-21: "no jumps
/// at all" — ten minutes let a step grow to three or more points, which the
/// apps took in one frame).
const SLOT_SECS: i64 = 60;
/// THE HOLD: the apps' look — the one step other people's programs cannot
/// fade — stays the old one through the dock's fade across the looks and
/// turns under the slow dissolve at its end. Should that never come (a
/// failure on the way), it goes at a quiet moment this long after at most.
/// (Hiding it in a moment of motion, 2026-10-20, still showed.)
const HOLD_MAX: Duration = Duration::from_secs(30 * 60);
/// Away after this long without input; a quiet moment after this long.
const AWAY_MS: u32 = 120_000;
const QUIET_MS: u32 = 3_000;
/// Settled: present this long without a break. (The grace keeps the look off
/// the very instant the idle notification of the same two minutes is sent.)
const SETTLE: Duration = Duration::from_secs(120);
const SETTLE_GRACE: Duration = Duration::from_secs(5);
/// BACK FROM AWAY, the first touch is the cue: the catch-up starts this long
/// after it (Max, 2026-10-20: "when the user touches something… after 2s the
/// change is noticed, not a jump, the light goes down like dimming a light
/// in a room"). It waited the two minutes of settling, and then a tick.
const RETURN_CUE: Duration = Duration::from_secs(2);
/// How often the scheduler looks up; the fade's frame; its length.
const TICK: Duration = Duration::from_secs(30);
const FRAME: Duration = Duration::from_millis(66);
const FADE: Duration = Duration::from_secs(20);
/// The fade's length now: `FADE`, or a walk's step while one goes
/// (`lighting_walk`).
static FADE_MS: AtomicU64 = AtomicU64::new(20_000);
fn fade_time() -> Duration {
    Duration::from_millis(FADE_MS.load(Ordering::Relaxed))
}
/// Where the veil begins on the line: Light 44.
const VEIL_FROM: u8 = 75;
/// A change smaller than this is not worth a fade: one point of the line
/// (~0.005 of lightness on the grounds, under what an eye sees change).
const MIN_STEP: u8 = 1;
/// The most the apps take in one frame. A bigger step (a return, the first
/// look after a start) reaches them under the slow dissolve.
const APPS_STEP: u8 = 2;
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

/// THE SUN'S LINE: the Lighting the sun asks for, as the box's one line
/// (`palette::line_of`, both looks). The Light look from the horizon up — one
/// even slope through the afternoon, ~Light 15 a few degrees up, as designed
/// near +25°, Light 60 from +45° (Max, 2026-10-21: "slightly stronger during
/// the afternoon, we don't want a massive jump at 19") — and the Dark look
/// below it, down to Dark 20 past −12° (the end of nautical twilight). Round
/// the horizon, DUSK: the light down to Light 0 at it, the dark from Dark 100
/// under it (`DUSK_ABOVE`, `DUSK_BELOW`). The flip is the horizon (Max,
/// 2026-10-20: "light in the morning, dark when it gets dark, like the Auto
/// of macOS"); the gap between the halves is what a fade leaps.
pub(crate) fn line(elevation_deg: f64) -> u8 {
    if !elevation_deg.is_finite() {
        return palette::line_of(true, DAY);
    }
    let e = elevation_deg;
    let light = if e >= 0.0 {
        15.0 * smoothstep(e / DUSK_ABOVE) + 45.0 * (e.min(45.0) / 45.0 * std::f64::consts::FRAC_PI_2).sin()
    } else {
        f64::from(NIGHT) + 40.0 * smoothstep((e + 12.0) / 12.0) + 40.0 * (1.0 - smoothstep(-e / DUSK_BELOW))
    };
    palette::line_of(e < 0.0, light.round().min(100.0) as u8)
}

/// Eased from `from` to `to` over `span`.
fn eased(from: u8, to: u8, elapsed: Duration, span: Duration) -> u8 {
    if span.is_zero() {
        return to;
    }
    let e = smoothstep(elapsed.as_secs_f64() / span.as_secs_f64());
    let v = f64::from(from) + (f64::from(to) - f64::from(from)) * e;
    v.round().clamp(0.0, 100.0) as u8
}

/// Where the fade is after `elapsed`: eased from `from` to `to`.
pub(crate) fn fade_value(from: u8, to: u8, elapsed: Duration) -> u8 {
    eased(from, to, elapsed, fade_time())
}

/// Where a fade ON THE LINE is after `elapsed`. Within a look, [`fade_value`].
/// Across the gap, three legs: the old look to its edge, the leap over the
/// gap ([`GAP`] long: the shell crossfades its two edges), the new look from
/// its edge — the 20 s shared by the two looks' legs in proportion.
pub(crate) fn fade_line(from: u8, to: u8, elapsed: Duration) -> u8 {
    let (fd, _) = palette::line_to(from);
    let (td, _) = palette::line_to(to);
    if fd == td {
        return fade_value(from, to, elapsed);
    }
    let (edge_from, edge_to) = if fd { (45u8, 55u8) } else { (55, 45) };
    let a = f64::from(from.abs_diff(edge_from));
    let b = f64::from(to.abs_diff(edge_to));
    let total = (a + b).max(1.0);
    let ta = fade_time().mul_f64(a / total);
    let tb = fade_time().mul_f64(b / total);
    if elapsed < ta {
        return eased(from, edge_from, elapsed, ta);
    }
    let e2 = elapsed - ta;
    if e2 < GAP {
        let k = e2.as_secs_f64() / GAP.as_secs_f64();
        return (f64::from(edge_from) + (f64::from(edge_to) - f64::from(edge_from)) * k).round() as u8;
    }
    eased(edge_to, to, e2 - GAP, tb)
}

/// How long a fade from `from` to `to` takes: the leap added when it crosses.
pub(crate) fn fade_len(from: u8, to: u8) -> Duration {
    if palette::line_to(from).0 == palette::line_to(to).0 { fade_time() } else { fade_time() + GAP }
}

/// Whether a line value is in the gap (the leap between the looks).
fn in_gap(v: u8) -> bool {
    (46..=54).contains(&v)
}

/// The target worth a fade from `current`: a step under [`MIN_STEP`] is none.
pub(crate) fn decide(current: u8, target: u8) -> Option<u8> {
    (current.abs_diff(target) >= MIN_STEP).then_some(target)
}

/// The black wash over the wallpaper at a point of the LINE: one eased
/// slope from nothing at Light 44 (`VEIL_FROM`) down through the gap to
/// 0.45 at Dark 0 — the wallpaper darkens all through dusk with no step at
/// the switch (Max, 2026-10-22: the background stayed white past the switch;
/// it began only under Dark 50). (`on`: the wallpaper's Lighting is on this
/// line at all.)
pub(crate) fn veil_alpha(line: u8, on: bool) -> f32 {
    if !on {
        return 0.0;
    }
    let t = (f32::from(VEIL_FROM.saturating_sub(line)) / f32::from(VEIL_FROM)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t) * VEIL_MAX
}

/// The slot of a moment: the sun is looked at once a slot.
pub(crate) fn slot_key(unix: i64) -> i64 {
    unix.div_euclid(SLOT_SECS)
}

/// Whether the computer slept between two ticks: the monotonic clock does
/// not run in a sleep, the wall clock does.
pub(crate) fn slept(mono: Duration, wall: Duration) -> bool {
    wall > mono + SLEPT_GAP
}

/// The clock hour of a moment in this time zone (`gmtoff` seconds east): the
/// same number all through a wall-clock hour. (The sun is looked at by the
/// slot now, `slot_key`; kept for the day a look wants the wall's hour.)
#[allow(dead_code)]
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
    /// Back from away: settled at the cue, not after the two minutes.
    returned: bool,
}

impl Plan {
    pub(crate) fn step(&mut self, f: &Facts) -> bool {
        if !f.auto || !f.present {
            self.absent();
            return false;
        }
        let since = *self.since.get_or_insert(f.at);
        let settle = if self.returned { RETURN_CUE } else { SETTLE + SETTLE_GRACE };
        if f.at.saturating_sub(since) < settle {
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

    /// Whether the next look is the first since the owner settled (a catch-up:
    /// in front of them, whole — the apps' step with it, not held).
    pub(crate) fn first_look(&self) -> bool {
        self.last.is_none()
    }

    /// The owner is gone: the next time they settle, look at once.
    pub(crate) fn absent(&mut self) {
        self.since = None;
        self.last = None;
        self.returned = false;
    }

    /// The owner is here at `at` (the dock has just started): settled two
    /// minutes from now.
    pub(crate) fn back(&mut self, at: Duration) {
        self.since = Some(at);
        self.last = None;
        self.returned = false;
    }

    /// The owner came back from away and touched something at `at`: the
    /// look is due [`RETURN_CUE`] from now.
    pub(crate) fn returned(&mut self, at: Duration) {
        self.back(at);
        self.returned = true;
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

/// The point of the LINE a fade in progress shows, else None (the saved one).
pub(crate) fn shown() -> Option<u8> {
    Some(SHOWN.load(Ordering::Relaxed)).filter(|v| *v != NONE)
}

/// The line the sun last asked for (255: not looked at yet).
static SUN: AtomicU8 = AtomicU8::new(NONE);

/// THE SUN'S LINE now, as the look and the light within it: the step a fade
/// shows, else the line the sun last asked for. None until the sun was looked
/// at. (`ColorChoice::levels` takes it for the surfaces whose Ambient is on.)
pub(crate) fn ambient_line() -> Option<(bool, u8)> {
    shown().or_else(|| Some(SUN.load(Ordering::Relaxed)).filter(|v| *v != NONE)).map(palette::line_to)
}

/// THE HELD LOOK: the point of the line the apps and the web stay at after the
/// sun crossed the looks, until the step is let go (255: none). Read by
/// `ambient_line_for`, so every theme made meanwhile keeps the old look.
static HELD_DARK: AtomicU8 = AtomicU8::new(NONE);
static HELD_LIGHT: AtomicU8 = AtomicU8::new(NONE);

pub(crate) fn held() -> Option<(bool, u8)> {
    let d = HELD_DARK.load(Ordering::Relaxed);
    (d != NONE).then(|| (d == 1, HELD_LIGHT.load(Ordering::Relaxed)))
}
fn hold(at: Option<(bool, u8)>) {
    match at {
        Some((d, l)) => {
            HELD_LIGHT.store(l, Ordering::Relaxed);
            HELD_DARK.store(u8::from(d), Ordering::Relaxed);
        }
        None => HELD_DARK.store(NONE, Ordering::Relaxed),
    }
}

/// [`ambient_line`] as a surface sees it: the programs and the web keep the
/// held look while there is one; the shell, the icons, the wallpaper (what
/// Golem paints itself, and can fade) see the sun's.
pub(crate) fn ambient_line_for(s: palette::Surface) -> Option<(bool, u8)> {
    match s {
        palette::Surface::Apps | palette::Surface::Web => held().or_else(ambient_line),
        _ => ambient_line(),
    }
}

/// The wallpaper's point on the line now: the sun's where its Ambient is on
/// (`set_veil`), else its own Lighting.
fn wall_line_now() -> u8 {
    if VEIL.load(Ordering::Relaxed) {
        if let Some((d, l)) = ambient_line() {
            return palette::line_of(d, l);
        }
    }
    let lv = palette::choice().levels_now(palette::Surface::Wall);
    palette::line_of(lv.dark, lv.light)
}

/// Whether the Lighting moved since the last time everything was made for it.
pub(crate) fn applied(v: u8) -> bool {
    APPLIED.swap(v, Ordering::Relaxed) != v
}

/// Whether the wallpaper's Lighting follows the sun (its tab's Ambient), as
/// the choice has it; whether that changed.
pub(crate) fn set_veil(on: bool) -> bool {
    VEIL.swap(on, Ordering::Relaxed) != on
}

/// The wash over the wallpaper now (0: none).
/// The overview (waveview) draws its own backdrop — the wallpaper alone — so
/// it is told the veil, or the wallpaper there kept its full light (Max,
/// 2026-10-18: "the darkness on the BG does not apply on overview"). Told only
/// when it moves; a plugin without `set_veil` answers with an error, ignored.
pub(crate) fn tell_overview() {
    static TOLD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
    let v = veil_now();
    if TOLD.swap(v.to_bits(), Ordering::Relaxed) != v.to_bits() {
        crate::hypr::eval(&format!("hl.plugin.waveview.set_veil({v:.3})"));
    }
}

pub(crate) fn veil_now() -> f32 {
    // A twilight: the veil between its two edges' (`lighting_twilight`).
    if let Some(t) = palette::tint::twilight() {
        let k = f32::from(t) / 1000.0;
        return veil_alpha(45, true) * (1.0 - k) + veil_alpha(55, true) * k;
    }
    veil_alpha(wall_line_now(), true)
}

// -------------------------------------------------------- the clock and /proc

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// This zone's offset east of UTC at `unix`, seconds.
#[allow(dead_code)]
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
    /// Points of the LINE.
    from: u8,
    to: u8,
    began: Instant,
    /// The step last put on the screen.
    shown: u8,
    /// How the shell's pair is made at each step, in each look (made once,
    /// at the start): the Dark plan and the Light one.
    shell: (ShellPlan, ShellPlan),
    /// Which tabs follow this fade: the Colours' four surfaces; the wallpaper.
    colours: bool,
    wall: bool,
    frames: u32,
}

impl Fade {
    /// The shell's pair at a point of the line: the look's plan at its
    /// light; in the gap, the two edges crossfaded (the leap).
    fn shell_pair(&self, v: u8) -> Option<([f32; 4], [f32; 4])> {
        if in_gap(v) {
            let k = f32::from(v - 45) / 10.0;
            return match (self.shell.0.pair(100), self.shell.1.pair(0)) {
                (Some(a), Some(b)) => Some((lerp4(a.0, b.0, k), lerp4(a.1, b.1, k))),
                (a, b) => b.or(a),
            };
        }
        let (dark, light) = palette::line_to(v);
        if dark { self.shell.0.pair(light) } else { self.shell.1.pair(light) }
    }
    fn shell_follows(&self) -> bool {
        self.colours && (self.shell.0.follows_lighting() || self.shell.1.follows_lighting())
    }
}

fn lerp4(a: [f32; 4], b: [f32; 4], k: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * k)
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
    /// The apps' look the sun crossed to, held since (see `HOLD_MAX`).
    owed: Option<(bool, Instant)>,
    /// The owner came back while a look was owed: it goes with their settling.
    returned: bool,
    /// A walk of the line by hand in progress (`lighting_walk`).
    walk: Option<Walk>,
    /// A walk by hand ended away from the sun: the sun waits until a walk
    /// back to it, or Auto turned on again.
    walked: bool,
}

/// A walk of the line by hand: where to (or the sun's own line), a point
/// every `step`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Walk {
    to: u8,
    step: Duration,
    sun: bool,
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
            owed: None,
            returned: false,
            walk: None,
            walked: false,
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
            self.lighting.walked = false;
            self.lighting.plan.absent();
            if !on {
                hold(None);
                self.lighting.owed = None;
            }
        }
    }

    /// Start the 30 s tick (once, at the dock's start).
    pub(crate) fn lighting_start(&mut self) {
        // The dock has just started: the owner is taken to be here.
        mark_away(false);
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
        // A walk by hand: the sun waits.
        if self.lighting.walk.is_some() || self.lighting.walked {
            return;
        }
        let unix = unix_now();
        let facts = Facts {
            at: self.lighting.since_start(),
            hour: slot_key(unix),
            auto: true,
            present: self.lighting.present(),
            fullscreen: self.options_paused(),
            fading: self.lighting.fade.is_some(),
        };
        // A look owed to the apps: let go once the owner is back and settled,
        // or after the hold's longest — under a dissolve, in a quiet moment.
        if let Some((_, since)) = self.lighting.owed {
            let settled = facts.present && !self.lighting.plan.first_look();
            if self.lighting.quiet && self.lighting.fade.is_none() && (since.elapsed() >= HOLD_MAX || (self.lighting.returned && settled)) {
                self.lighting_release_dissolved();
            }
        }
        if self.lighting.plan.step(&facts) {
            self.lighting_look();
        }
    }

    /// The apps and the web take the look the sun crossed to.
    fn lighting_release(&mut self) {
        let Some((dark, _)) = self.lighting.owed.take() else { return };
        self.lighting.returned = false;
        hold(None);
        let _ = std::thread::Builder::new().name("ambient-look".into()).spawn(move || crate::controls::assert_dark(dark));
        if crate::look::dark() == dark {
            self.colors_apply(true);
        } else {
            self.look_set(dark);
        }
        // A wallpaper that follows the colours takes the new look's card with
        // the apps (its own picture's fade, under the same dissolve) — it
        // stayed light behind dark apps (Max, 2026-10-22: "my BG is white
        // still").
        crate::wall_follow::refresh();
    }

    /// …under a still of every screen dissolving slowly to the result
    /// (without the means, the plain step).
    fn lighting_release_dissolved(&mut self) {
        debug!("lighting: the apps take the new look under a dissolve");
        self.dissolve_slow(Box::new(|app: &mut App| app.lighting_release()));
    }

    /// A step within the look too big for the apps to take in one frame:
    /// they take it under the slow dissolve.
    fn lighting_apply_dissolved(&mut self) {
        debug!("lighting: the apps take a big step under a dissolve");
        self.lighting.apply_pending = false;
        self.dissolve_slow(Box::new(|app: &mut App| app.colors_apply(false)));
    }

    /// Auto was just turned on, by the owner: the light goes to the sun's now.
    /// WALK THE LINE BY HAND (Max, 2026-10-22: "we change from dark to
    /// light, that includes all, even the wallpaper, it have to be 5 mins
    /// smooth and even"): from what is shown to `to` (None: the sun's own
    /// line) over about `secs`, a point at a time through the same fades —
    /// the shell, the veil and the apps step by step, the switch between the
    /// looks under the slow dissolve. Ended away from the sun, the sun waits
    /// until a walk back to it (or Auto turned on).
    pub(crate) fn lighting_walk(&mut self, to: Option<u8>, secs: u32) -> String {
        let from = self.lighting_shown();
        let target = match to {
            Some(v) => v.min(100),
            None => match options_engine::sun_elevation_deg(unix_now() as f64) {
                Some(e) => line(e),
                None => return "no sun to walk to (no place from the time zone)".to_owned(),
            },
        };
        if from == target {
            self.lighting.walked = to.is_some();
            return format!("already at {target}");
        }
        let crossing = palette::line_to(from).0 != palette::line_to(target).0;
        let points = u32::from(from.abs_diff(target)).saturating_sub(if crossing { 10 } else { 0 }).max(1);
        let along = Duration::from_secs(u64::from(secs)).saturating_sub(if crossing { GAP } else { Duration::ZERO });
        let step = (along / points).max(Duration::from_millis(400));
        FADE_MS.store(step.as_millis() as u64, Ordering::Relaxed);
        let fresh = self.lighting.walk.is_none();
        self.lighting.walk = Some(Walk { to: target, step, sun: to.is_none() });
        self.lighting.walked = false;
        if fresh {
            let timer = Timer::from_duration(step);
            let armed = self.loop_handle.insert_source(timer, |_, _, app: &mut App| match app.lighting_walk_step() {
                Some(next) => TimeoutAction::ToDuration(next),
                None => TimeoutAction::Drop,
            });
            if let Err(e) = armed {
                self.lighting.walk = None;
                FADE_MS.store(FADE.as_millis() as u64, Ordering::Relaxed);
                return format!("no timer for the walk ({e})");
            }
        }
        info!("lighting: walking {from} -> {target} by hand, a point every {:.1} s", step.as_secs_f64());
        format!("walking {from} -> {target}, a point every {:.1} s", step.as_secs_f64())
    }

    /// TWILIGHT BY HAND (`palette::tint::set_twilight`; Max, 2026-10-22: "can't
    /// you do it manually first? and then do the ambient"): a point between
    /// the looks — 0 the lightest dark … 1000 the dimmest light — the apps,
    /// the dock and the veil there at once; the look turns at the middle.
    /// None: back to the line. The sun waits meanwhile.
    pub(crate) fn lighting_twilight(&mut self, t: Option<u16>) -> String {
        let Some(t) = t.map(|t| t.min(1000)) else {
            palette::tint::set_twilight(None);
            self.colors_apply(false);
            self.veil_sync();
            return "twilight off".to_owned();
        };
        let dark = t < palette::tint::TWILIGHT_FLIP;
        palette::tint::set_twilight(Some(t));
        SUN.store(if dark { 45 } else { 55 }, Ordering::Relaxed);
        self.lighting.walked = true;
        if crate::look::dark() == dark {
            self.colors_apply(false);
        } else {
            let _ = std::thread::Builder::new().name("ambient-look".into()).spawn(move || crate::controls::assert_dark(dark));
            self.look_set(dark);
        }
        self.veil_sync();
        format!("twilight {t} ({})", if dark { "dark" } else { "light" })
    }

    /// The point of the line shown now: the fade's step, else the shell's.
    fn lighting_shown(&self) -> u8 {
        self.lighting.fade.as_ref().map_or_else(
            || {
                let lv = palette::choice().levels_now(palette::Surface::Shell);
                palette::line_of(lv.dark, lv.light)
            },
            |f| f.shown,
        )
    }

    /// One step of a walk; when to come again, or None: it is over.
    fn lighting_walk_step(&mut self) -> Option<Duration> {
        let w = self.lighting.walk?;
        // A fade, or the switch's dissolve, still going: wait for it.
        if self.lighting.fade.is_some() || self.dissolve.is_some() {
            return Some(Duration::from_millis(200));
        }
        let to = if w.sun { options_engine::sun_elevation_deg(unix_now() as f64).map_or(w.to, line) } else { w.to };
        let from = self.lighting_shown();
        if from == to {
            self.lighting.walk = None;
            self.lighting.walked = !w.sun;
            FADE_MS.store(FADE.as_millis() as u64, Ordering::Relaxed);
            info!("lighting: the walk by hand is done at {to}{}", if w.sun { " (the sun's: it takes over)" } else { " (the sun waits)" });
            return None;
        }
        let up = to > from;
        let next = if up { from + 1 } else { from - 1 };
        let next = if in_gap(next) { if up { 55 } else { 45 } } else { next };
        SUN.store(next, Ordering::Relaxed);
        self.lighting_fade(from, next);
        if self.lighting.fade.is_none() {
            // (Neither the colours' nor the wallpaper's light follows the line.)
            self.lighting.walk = None;
            FADE_MS.store(FADE.as_millis() as u64, Ordering::Relaxed);
            info!("lighting: the walk stops — no Ambient to walk");
            return None;
        }
        Some(w.step)
    }

    pub(crate) fn lighting_auto_on(&mut self) {
        self.lighting.walked = false;
        self.lighting.plan.looked(slot_key(unix_now()));
        self.lighting_look();
    }

    /// Look at the sun: a fade to its line if that is far enough from this.
    fn lighting_look(&mut self) {
        let Some(elevation) = options_engine::sun_elevation_deg(unix_now() as f64) else {
            debug!("lighting: no place on the planet from the time zone; Ambient has nothing to follow");
            return;
        };
        let target = line(elevation);
        // (From what is shown: the fade's step, else the shell's line — which,
        // under Ambient, is the sun's last — read before the new one is kept.)
        let from = self.lighting.fade.as_ref().map_or_else(
            || {
                let lv = palette::choice().levels_now(palette::Surface::Shell);
                palette::line_of(lv.dark, lv.light)
            },
            |f| f.shown,
        );
        match decide(from, target) {
            Some(to) => {
                SUN.store(target, Ordering::Relaxed);
                self.lighting_fade(from, to);
            }
            None => {
                debug!("lighting: sun at {elevation:.1}° asks {target}, shown {from}: close enough");
                // What is shown stays the line, so a slow sun adds up to a
                // fade. (Taking the target here, unshown, swallowed every
                // small step: the afternoon never moved, nothing was kept.)
                // The first look keeps it for every surface.
                if SUN.load(Ordering::Relaxed) == NONE {
                    SUN.store(from, Ordering::Relaxed);
                    store(from);
                    self.lighting.apply_pending = true;
                    self.lighting_apply_if_due();
                }
            }
        }
    }

    /// The dock's start: the line the sun asks for now, kept for every
    /// surface whose Ambient is on, before anything is made — so a start
    /// never shows the levels of another hour, or each surface its own.
    /// (Nothing is on the screen yet; from here the light moves only in
    /// front of the owner, by the schedule.)
    pub(crate) fn lighting_wake(&mut self) {
        let picked = palette::choice();
        if !picked.ambient_of(palette::Surface::Shell) && !picked.ambient_of(palette::Surface::Wall) {
            return;
        }
        let Some(elevation) = options_engine::sun_elevation_deg(unix_now() as f64) else {
            return;
        };
        let target = line(elevation);
        SUN.store(target, Ordering::Relaxed);
        store(target);
        info!("lighting: the sun at {elevation:.1}° asks {target} at the start");
    }

    fn lighting_fade(&mut self, from: u8, to: u8) {
        let picked = palette::choice();
        let (colours, wall) = (picked.ambient_of(palette::Surface::Shell), picked.ambient_of(palette::Surface::Wall));
        if !colours && !wall {
            return;
        }
        // Crossing the looks: the programs keep the old one until the fade
        // ends and lets it go under the slow dissolve (`lighting_finish`).
        if colours && palette::line_to(from).0 != palette::line_to(to).0 && held().is_none() {
            hold(Some(palette::line_to(from)));
        }
        info!("lighting: {from} -> {to} over {} s{}", fade_len(from, to).as_secs(), if palette::line_to(from).0 != palette::line_to(to).0 { ", across the looks" } else { "" });
        let shell = (palette::shell_plan_look(&picked, true), palette::shell_plan_look(&picked, false));
        SHOWN.store(from, Ordering::Relaxed);
        self.lighting.fade = Some(Fade { from, to, began: Instant::now(), shown: from, shell, colours, wall, frames: 0 });
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
        let v = fade_line(f.from, f.to, elapsed);
        let (to, before, frames) = (f.to, f.shown, f.frames);
        let done = elapsed >= fade_len(f.from, f.to);
        if v != before {
            f.shown = v;
            palette::tint::set_lighting(palette::line_to(v).1);
            SHOWN.store(v, Ordering::Relaxed);
            // Only the shell changes, and only when the step did: ~30 steps
            // in all, not 300 frames. (The apps are made for it afterwards.)
            let follows = f.shell_follows();
            if follows {
                palette::store_shell(f.shell_pair(v));
            }
            let veil_moved = f.wall && veil_alpha(before, true) != veil_alpha(v, true);
            if follows {
                self.draw_options();
                self.draw();
            }
            if veil_moved {
                self.veil_sync();
                tell_overview();
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
        let colours = self.lighting.fade.as_ref().is_some_and(|f| f.colours);
        let from = self.lighting.fade.as_ref().map(|f| f.from);
        self.lighting.fade = None;
        SHOWN.store(NONE, Ordering::Relaxed);
        SUN.store(to, Ordering::Relaxed);
        let (dark, light) = palette::line_to(to);
        palette::tint::set_lighting(light);
        store(to);
        // The look crossed: at the END of the dock's own fade, the dock's
        // whole look and the programs' turn together under the SLOW dissolve
        // of every screen — a light dimming, never a cut (Max, 2026-10-21:
        // "no jumps at all"; flipping them under a moment of motion still
        // showed). Held at the old look until the stills are up.
        if colours && crate::look::dark() != dark {
            self.lighting.owed = Some((dark, Instant::now()));
            if held().is_none() {
                if let Some(f) = from {
                    hold(Some(palette::line_to(f)));
                }
            }
            self.lighting_release_dissolved();
            return;
        }
        // Within the look: a step of a point or two the apps take at the next
        // quiet moment (nothing to see); a bigger one under the dissolve.
        if colours && from.is_some_and(|f| f.abs_diff(to) > APPS_STEP) {
            self.lighting_apply_dissolved();
            return;
        }
        self.lighting.apply_pending = true;
        self.lighting_apply_if_due();
    }

    /// The fade ended where it stands (the owner left): keep what they saw.
    /// (A stop in the gap lands on its near edge.)
    fn lighting_stop(&mut self) {
        if let Some(v) = self.lighting_cancel() {
            let v = if in_gap(v) { if v < 50 { 45 } else { 55 } } else { v };
            palette::tint::set_lighting(palette::line_to(v).1);
            SUN.store(v, Ordering::Relaxed);
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
        hold(None);
        self.lighting.owed = None;
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
        mark_away(away);
        if away {
            self.lighting.plan.absent();
            self.lighting_stop();
        } else {
            let at = self.lighting.since_start();
            self.lighting.plan.returned(at);
            self.lighting.returned = self.lighting.owed.is_some();
            // The tick comes every 30 s: this one comes at the cue.
            let cue = Timer::from_duration(RETURN_CUE + Duration::from_millis(100));
            if let Err(e) = self.loop_handle.insert_source(cue, |_, _, app: &mut App| {
                app.lighting_tick();
                TimeoutAction::Drop
            }) {
                warn!("lighting: no timer for the return ({e}); the next tick looks");
            }
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

/// THE OWNER AWAY, for the rest of the system: `$XDG_RUNTIME_DIR/golem/away`
/// is there exactly while they are (Golem's golem-autobrightness holds the
/// screen still meanwhile — it dimmed it to "off" behind Max's back,
/// 2026-10-21). In the runtime dir, so a login never starts away.
fn mark_away(away: bool) {
    let Some(run) = std::env::var_os("XDG_RUNTIME_DIR") else { return };
    let dir = std::path::Path::new(&run).join("golem");
    let file = dir.join("away");
    let done = if away {
        std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&file, b""))
    } else {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    };
    if let Err(e) = done {
        warn!("lighting: cannot mark the owner {}: {e}", if away { "away" } else { "back" });
    }
}

/// Keep the point `v` of the line as the look and the light the owner last
/// saw, in every surface whose Ambient is on (so Ambient turned off, or a
/// restart, starts from it: nothing jumps).
fn store(v: u8) {
    use palette::Surface;
    let (dark, light) = palette::line_to(v);
    crate::display::GolemSettings::change(|s| {
        let mut c = s.colors.clone().unwrap_or_default();
        let (colours, wall) = (c.ambient_of(Surface::Shell), c.ambient_of(Surface::Wall));
        let mut own: Vec<(&mut Option<bool>, &mut Option<u8>)> = Vec::new();
        if colours {
            own.push((&mut c.shell_dark, &mut c.shell_light));
            own.push((&mut c.apps_dark, &mut c.apps_light));
            own.push((&mut c.icons_dark, &mut c.icons_light));
            own.push((&mut c.web_dark, &mut c.web_light));
        }
        if wall {
            own.push((&mut c.wall_dark, &mut c.wall_light));
        }
        for (d, l) in own {
            *d = Some(dark);
            *l = Some(light);
        }
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
    fn the_sun_draws_the_line_light_by_day_dark_by_night() {
        use crate::palette::line_to;
        // (The line has 45 steps a half: a light reads back within one.)
        let near = |e: f64, want: (bool, u8), why: &str| {
            let (d, l) = line_to(line(e));
            assert!(d == want.0 && l.abs_diff(want.1) <= 1, "{why}: {e}° -> ({d}, {l}), wanted {want:?}");
        };
        near(45.0, (false, 60), "a high sun: Light 60");
        near(24.5, (false, 50), "the afternoon as designed");
        near(10.0, (false, 30), "an hour before dusk: well on the way down, not all at the end");
        near(2.5, (false, 21), "dusk begins: the light goes on down, stronger");
        near(0.0, (false, 0), "the horizon: the light at its floor");
        near(-0.1, (true, 100), "under the horizon: the lightest dark — the two looks at their nearest");
        near(-2.5, (true, 75), "dusk below: the dark deepening, stronger than the night's");
        near(-6.0, (true, 40), "dusk over");
        near(-12.0, (true, 20), "the night from -12°");
        near(-20.0, (true, 20), "night");
        near(f64::NAN, (true, DAY), "no reading: Dark as designed");
        // The sun sinking from +45° to -20° never raises the line.
        let mut prev = 100;
        for tenth in (-200..=450).rev() {
            let v = line(f64::from(tenth) / 10.0);
            assert!(v <= prev, "the line never rises as the sun sinks ({tenth})");
            assert!(!in_gap(v), "the sun never asks for the gap");
            prev = v;
        }
    }

    #[test]
    fn a_fade_across_the_looks_leaps_the_gap_in_its_own_time() {
        use crate::palette::{line_of, line_to};
        let (from, to) = (line_of(false, 15), line_of(true, 60)); // dusk: Light 15 -> Dark 60
        assert_eq!(fade_line(from, to, secs(0)), from);
        assert_eq!(fade_line(from, to, fade_len(from, to)), to);
        assert_eq!(fade_len(from, to), FADE + GAP);
        // The legs share the 20 s by their length; the gap takes GAP.
        let mut was_light = true;
        let mut gap_ms = 0;
        let mut prev = from;
        for ms in (0..=(FADE + GAP).as_millis() as u64 + 500).step_by(66) {
            let v = fade_line(from, to, Duration::from_millis(ms));
            assert!(v <= prev, "monotone");
            if in_gap(v) {
                gap_ms += 66;
            }
            if was_light && line_to(v).0 && !in_gap(v) {
                was_light = false;
            }
            prev = v;
        }
        assert!(!was_light, "it ends Dark");
        let want = GAP.as_millis() as u64;
        assert!((want * 3 / 4..=want + 600).contains(&gap_ms), "the leap is about {want}: {gap_ms} ms");
        // Within one look it is the plain fade.
        assert_eq!(fade_line(line_of(true, 50), line_of(true, 20), secs(10)), fade_value(line_of(true, 50), line_of(true, 20), secs(10)));
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
    fn every_point_of_the_line_is_a_fade_and_none_is_none() {
        assert_eq!(decide(50, 49), Some(49));
        assert_eq!(decide(20, 22), Some(22));
        assert_eq!(decide(30, 30), None);
    }

    #[test]
    fn the_veil_is_one_slope_through_dusk_and_only_when_asked() {
        use crate::palette::line_of;
        assert_eq!(veil_alpha(line_of(false, 60), true), 0.0, "a bright day: none");
        assert_eq!(veil_alpha(line_of(false, 44), true), 0.0, "it begins at Light 44");
        assert!((veil_alpha(line_of(true, 0), true) - 0.45).abs() < 1e-6, "the darkest night: 0.45");
        let (before, after) = (veil_alpha(line_of(false, 0), true), veil_alpha(line_of(true, 100), true));
        assert!(before > 0.0 && after > before && after - before < 0.09, "no step at the switch: {before} → {after}");
        let mut prev = 0.0;
        for v in (0..=100u8).rev() {
            let a = veil_alpha(v, true);
            assert!(a >= prev, "it only deepens as the line goes down");
            prev = a;
        }
        assert_eq!(veil_alpha(line_of(true, 0), false), 0.0, "the switch off draws nothing");
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
    fn back_from_away_the_look_comes_two_seconds_after_the_first_touch() {
        let mut plan = Plan::default();
        assert!(!plan.step(&here(0, 10)));
        assert!(plan.step(&here(150, 10)));
        plan.absent();
        plan.returned(secs(5000));
        assert!(!plan.step(&here(5001, 11)), "not in the same instant");
        assert!(plan.step(&here(5002, 11)), "two seconds after the touch");
        assert!(!plan.step(&here(5032, 11)));
        // A dock start is not a return: it settles its two minutes.
        plan.back(secs(9000));
        assert!(!plan.step(&here(9002, 12)));
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

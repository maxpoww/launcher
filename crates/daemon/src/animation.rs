//! dt-based animation primitives: eased timelines and a damped spring.
//!
//! Everything here is frame-rate independent — animators advance by a
//! wall-clock `dt`, never by a fixed per-frame increment. The unit tests
//! step the same animator at 60 Hz and 144 Hz and assert both land on the
//! same result.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use waverunner_core::config::{CurveConfig, CurveKind};

/// The reduce-motion accessibility intent (`[accessibility]` in
/// config.toml), set once at startup. A process-wide flag rather than a
/// parameter because every primitive below honours it and threading it
/// through each of the dozens of call sites would be pure churn.
static REDUCE_MOTION: AtomicBool = AtomicBool::new(false);

/// Set the reduce-motion flag (startup only).
pub fn set_reduce_motion(on: bool) {
    REDUCE_MOTION.store(on, Ordering::Relaxed);
}

/// Whether every animation should snap straight to its end state.
pub fn reduce_motion() -> bool {
    REDUCE_MOTION.load(Ordering::Relaxed)
}

/// Linear interpolation between `a` and `b` by `t` (unclamped).
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// [`lerp`] over an RGBA quad — the one every OPTIONS surface uses to cross a
/// pill's colour into a box's.
///
/// It lived as a private copy in `notif.rs` AND in `clipboard.rs`, and the
/// playing box would have made a third. Two identical copies are a coincidence;
/// three are a habit, and §3's argument about rate constants applies just as
/// well to formulas — equal only until someone tunes one of them.
pub fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
        lerp(a[3], b[3], t),
    ]
}

// --- One Material (OptionUXRules.md §3) --------------------------------------
// The OPTIONS surface moves at ONE tempo: every morph is an exponential approach
// sharing the time constant below, so a long travel and a short one have the
// same acceleration profile and read as the same stuff. Modules import this
// vocabulary; a module-local rate constant is a defect even while it still
// happens to be equal, because it is equal only until someone tunes one of them.
//
// Shared is the TEMPO, not the choreography: stagger, hold, direction and order
// stay each OPTION's own. One rate does not mean one moment.

/// The surface's one morph rate, in exponential-approach units (the value
/// closes ≈`RATE`× of the remaining distance per second, so the time constant
/// is `1/RATE` seconds). Tuning this retimes the whole bar at once — which is
/// the point.
pub const MORPH_RATE: f32 = 13.0;

/// Where a morph may call itself finished: half a logical pixel, i.e. under
/// what the screen can show. Stated in real geometry so that every animation
/// stops at the same *visible* distance rather than at the same number in
/// whatever unit it happens to run on.
pub const SETTLE_PX: f32 = 0.5;

/// [`SETTLE_PX`] converted for a normalized 0→1 progress that drives `span_px`
/// of geometry.
///
/// A progress value is not a distance, so its threshold has to be mapped back
/// through the span it carries — otherwise the same-looking epsilon means a
/// tenth of a pixel on a wide morph and a whole pixel on a narrow one, and two
/// animations that read as equally done stop at different visible distances.
/// Clamped so a span smaller than the threshold still terminates.
pub fn settle_t(span_px: f32) -> f32 {
    (SETTLE_PX / span_px.abs().max(SETTLE_PX)).min(0.5)
}

/// The tempo for content *tracking* — a list gliding to a scroll target rather
/// than a shape morphing into another shape. Faster than [`MORPH_RATE`] because
/// it is chasing a position the user is actively driving, where lag reads as
/// weight rather than grace.
///
/// It is a second named tempo, not a local deviation: §3 allows the vocabulary
/// to have more than one word in it, and forbids modules from inventing them.
pub const SCROLL_RATE: f32 = 20.0;

/// Where a fade may call itself finished: one step of 8-bit colour, i.e. under
/// what the display can show.
///
/// The same idea as [`SETTLE_PX`] — settle at the limit of what is visible —
/// measured in the unit the animation actually runs on. An opacity drives no
/// geometry, so pixels are the wrong ruler for it; borrowing them would make a
/// fade stop at a threshold that means nothing to it.
pub const SETTLE_ALPHA: f32 = 1.0 / 255.0;

/// How long an OPTION stays open after the pointer has left it.
///
/// Shared by every OPTION, and deliberately short. Dwelling on something is
/// expressed by *keeping the pointer on it* — that vocabulary is already in the
/// user's hand, so an OPTION that lingers on its own has decided for them that
/// they were still interested. Leave, and it leaves.
///
/// This grace is not reading time: it exists so that crossing the gap between
/// two parts of one OPTION (the bell and its mute pill, a pill and the box
/// beneath it) does not register as leaving. It is sized for a hand in transit,
/// which is why one value fits everywhere — hands cross gaps at the same speed
/// on every part of the bar.
///
/// NOT to be confused with an auto-withdraw dwell — how long something that
/// arrived *on its own* stays readable (a notification's flash, ranked by
/// urgency). Nobody's pointer arrived there, so nobody's pointer can leave;
/// that duration answers a different question and each OPTION owns it.
pub const LEAVE_HOLD: Duration = Duration::from_millis(300);

/// Frame-rate-independent exponential approach: step `current` toward
/// `target` by the decay factor `1 − exp(−dt·rate)`, snapping onto the
/// target once within `snap`. Returns the new value and whether it is
/// still moving — the shared ease behind every "glide to rest" in the
/// daemon (page slides, the make-room reflows, the dock parting).
/// Under reduce-motion it lands on the target immediately.
pub fn ease_toward(current: f32, target: f32, dt: f32, rate: f32, snap: f32) -> (f32, bool) {
    if reduce_motion() {
        return (target, false);
    }
    let delta = target - current;
    if delta.abs() > snap {
        (current + delta * (1.0 - (-dt * rate).exp()), true)
    } else {
        (target, false)
    }
}

/// A small underdamped follower chasing a moving target with its own
/// tempo, carrying momentum between frames — an AGUA water body. Each
/// body (card silhouette, dock icons, box content) runs its own
/// follower with slightly different constants, so their swells peak at
/// slightly different moments: overlapping action, not lockstep.
#[derive(Debug, Clone)]
pub struct Follower {
    pub pos: f32,
    vel: f32,
    k: f32,
    c: f32,
}

impl Follower {
    /// A follower at rest on 1.0 with stiffness `k` and damping `c`.
    pub fn new(k: f32, c: f32) -> Self {
        Self {
            pos: 1.0,
            vel: 0.0,
            k,
            c,
        }
    }

    /// Chase `target` for `dt` seconds (substepped for stability).
    /// Under reduce-motion the body rests on the target at once — flat
    /// water, no swell.
    pub fn step(&mut self, target: f32, dt: f32) {
        if reduce_motion() {
            self.pos = target;
            self.vel = 0.0;
            return;
        }
        let mut remaining = dt.min(0.25);
        while remaining > 0.0 {
            let h = remaining.min(MAX_SUBSTEP);
            let accel = -self.k * (self.pos - target) - self.c * self.vel;
            self.vel += accel * h;
            self.pos += self.vel * h;
            remaining -= h;
        }
    }

    /// Still meaningfully away from rest (1.0) or moving.
    pub fn is_active(&self) -> bool {
        (self.pos - 1.0).abs() > 0.000_5 || self.vel.abs() > 0.005
    }

    /// Inject a velocity impulse (pos-units / s); the spring drives pos back
    /// to 1.0 from whatever displaced position results.
    pub fn kick(&mut self, vel: f32) {
        self.vel += vel;
    }

    /// Land exactly on rest.
    pub fn snap(&mut self) {
        self.pos = 1.0;
        self.vel = 0.0;
    }
}

/// Progress below which a settled animator snaps to its endpoint.
const SETTLE_EPSILON: f32 = 1e-3;

/// Maximum spring integration substep. Large `dt` values (e.g. after the
/// event loop slept) are split so the semi-implicit Euler stays stable.
const MAX_SUBSTEP: f32 = 1.0 / 240.0;

/// A critically-ish damped spring targeting 1.0 from 0.0.
#[derive(Debug, Clone)]
pub struct Spring {
    stiffness: f32,
    damping: f32,
    mass: f32,
    position: f32,
    velocity: f32,
}

impl Spring {
    fn new(stiffness: f32, damping: f32, mass: f32) -> Self {
        Self {
            stiffness: stiffness.max(1.0),
            damping: damping.max(0.0),
            mass: mass.max(1e-3),
            position: 0.0,
            velocity: 0.0,
        }
    }

    fn step(&mut self, dt: f32) -> f32 {
        if reduce_motion() {
            self.position = 1.0;
            self.velocity = 0.0;
            return 1.0;
        }
        let mut remaining = dt.min(0.25); // clamp pathological pauses
        while remaining > 0.0 {
            let h = remaining.min(MAX_SUBSTEP);
            let displacement = self.position - 1.0;
            let accel = (-self.stiffness * displacement - self.damping * self.velocity) / self.mass;
            self.velocity += accel * h;
            self.position += self.velocity * h;
            remaining -= h;
        }
        self.position
    }

    fn is_settled(&self) -> bool {
        (self.position - 1.0).abs() < SETTLE_EPSILON && self.velocity.abs() < SETTLE_EPSILON * 10.0
    }
}

/// A fixed-duration eased timeline from 0.0 to 1.0.
#[derive(Debug, Clone)]
pub struct Timed {
    kind: CurveKind,
    duration: Duration,
    elapsed: Duration,
}

impl Timed {
    fn step(&mut self, dt: f32) -> f32 {
        if reduce_motion() {
            self.elapsed = self.duration;
            return 1.0;
        }
        self.elapsed += Duration::from_secs_f32(dt.max(0.0));
        let t = (self.elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        ease(self.kind, t)
    }

    fn is_settled(&self) -> bool {
        self.elapsed >= self.duration
    }
}

fn ease(kind: CurveKind, t: f32) -> f32 {
    match kind {
        // A timeline never carries the spring kind (see Animator::new),
        // but fall back to linear rather than panicking.
        CurveKind::Spring => t,
        CurveKind::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
        CurveKind::EaseOutQuart => 1.0 - (1.0 - t).powi(4),
        CurveKind::EaseInCubic => t.powi(3),
    }
}

/// A running animation from 0.0 to 1.0, built from a [`CurveConfig`].
#[derive(Debug, Clone)]
pub enum Animator {
    /// Physical spring.
    Spring(Spring),
    /// Eased fixed-duration timeline.
    Timed(Timed),
}

impl Animator {
    /// Build a fresh animator (at progress 0.0) from configuration.
    pub fn new(config: &CurveConfig) -> Self {
        match config.kind {
            CurveKind::Spring => Animator::Spring(Spring::new(
                config.spring_stiffness,
                config.spring_damping,
                config.spring_mass,
            )),
            kind => Animator::Timed(Timed {
                kind,
                duration: Duration::from_millis(u64::from(config.duration_ms.max(1))),
                elapsed: Duration::ZERO,
            }),
        }
    }

    /// Advance by `dt` seconds and return the new progress.
    ///
    /// Progress is nominally in `0.0..=1.0`; a spring may overshoot 1.0
    /// slightly while settling.
    pub fn step(&mut self, dt: f32) -> f32 {
        match self {
            Animator::Spring(s) => s.step(dt),
            Animator::Timed(t) => t.step(dt),
        }
    }

    /// Instantaneous progress velocity (per second): the spring's live
    /// speed, zero for eased timelines — drives the AGUA squash &
    /// stretch, which only springs deserve.
    pub fn velocity(&self) -> f32 {
        match self {
            Animator::Spring(s) => s.velocity,
            Animator::Timed(_) => 0.0,
        }
    }

    /// True once the animation has effectively reached its target; the
    /// caller should snap to the endpoint and stop requesting frames.
    pub fn is_settled(&self) -> bool {
        match self {
            Animator::Spring(s) => s.is_settled(),
            Animator::Timed(t) => t.is_settled(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests that touch the process-wide reduce-motion flag
    /// against the one test with mid-flight assertions, so parallel test
    /// threads can't observe each other's flag state.
    static FLAG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// How long a morph of `span` logical px takes to settle, in seconds —
    /// the exponential approach's `ln(span/ε)/rate`, walked frame by frame so
    /// it measures the real primitive rather than the formula.
    fn settle_secs(span: f32) -> f32 {
        let (mut v, mut t) = (0.0f32, 0.0f32);
        let eps = settle_t(span);
        loop {
            let (nv, moving) = ease_toward(v, 1.0, 1.0 / 240.0, MORPH_RATE, eps);
            v = nv;
            t += 1.0 / 240.0;
            if !moving || t > 5.0 {
                return t;
            }
        }
    }

    #[test]
    fn one_material_every_morph_stops_at_the_same_visible_distance() {
        // §3: a progress value is not a distance, so its settle threshold is
        // mapped back through the span it drives. A 12px nudge and a 400px
        // sweep must both stop half a logical pixel short — not at the same
        // number in whatever unit each happens to run on.
        for span in [12.0, 27.0, 180.0, 380.0, 505.0] {
            let remaining_px = settle_t(span) * span;
            assert!(
                (remaining_px - SETTLE_PX).abs() < 1e-3,
                "a {span}px morph stopped {remaining_px}px short, not {SETTLE_PX}px"
            );
        }
        // A span below the threshold still terminates rather than easing forever.
        assert!(settle_t(0.0) > 0.0 && settle_t(0.0) <= 0.5);
    }

    #[test]
    fn one_material_the_leave_hold_is_a_transit_grace_not_reading_time() {
        // §3: leave, and it leaves. Dwelling is expressed by keeping the
        // pointer on a thing; a hold long enough to *read* by has decided on
        // the user's behalf that they were still interested. The clock used to
        // sit at 1500ms and that is exactly what it felt like.
        assert!(
            LEAVE_HOLD >= Duration::from_millis(150),
            "too brief to survive a hand crossing between two parts of one OPTION"
        );
        assert!(
            LEAVE_HOLD <= Duration::from_millis(400),
            "{LEAVE_HOLD:?} is reading time — the OPTION is deciding you are still looking"
        );
    }

    #[test]
    fn one_material_duration_grows_gently_with_distance() {
        // Walks the real primitive, so it has to hold the flag: under
        // reduce-motion `ease_toward` lands instantly and every span would
        // "settle" in one step, quietly turning this into a test of nothing.
        let _g = hold_flag();
        set_reduce_motion(false);
        // The shared RATE, not a shared stopwatch: a longer travel starts
        // faster and takes a little longer. The spread across the bar's real
        // spans stays inside a few hundred ms — one material at every scale,
        // rather than a 12px nudge and a 505px drawer both taking exactly as
        // long as each other (which is what a fixed duration would give).
        let small = settle_secs(12.0);
        let large = settle_secs(505.0);
        assert!(small < large, "a longer travel must not finish sooner");
        assert!(
            large - small < 0.35,
            "spread of {:.0}ms is a different material, not a longer one",
            (large - small) * 1000.0
        );
        // And the whole bar lands in the range the design language asks for.
        for span in [12.0, 27.0, 180.0, 380.0, 505.0] {
            let s = settle_secs(span);
            assert!(
                (0.2..0.8).contains(&s),
                "a {span}px morph settles in {:.0}ms",
                s * 1000.0
            );
        }
    }
    fn hold_flag() -> std::sync::MutexGuard<'static, ()> {
        FLAG_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn reduce_motion_snaps_every_primitive() {
        let _g = hold_flag();
        set_reduce_motion(true);
        // The shared glide lands on its target at once, reporting settled.
        assert_eq!(
            ease_toward(0.0, 10.0, 1.0 / 144.0, 12.0, 0.5),
            (10.0, false)
        );
        // Both animator kinds reach settled progress on the first step.
        for cfg in [
            CurveConfig::default(), // spring
            CurveConfig {
                kind: CurveKind::EaseOutCubic,
                duration_ms: 200,
                ..CurveConfig::default()
            },
        ] {
            let mut a = Animator::new(&cfg);
            assert!((a.step(1.0 / 144.0) - 1.0).abs() < f32::EPSILON);
            assert!(a.is_settled());
        }
        // The AGUA water rests flat: a kicked follower is back on rest
        // after one step, with no residual velocity to swell later.
        let mut f = Follower::new(120.0, 14.0);
        f.kick(5.0);
        f.step(1.0, 1.0 / 144.0);
        assert!(!f.is_active());
        set_reduce_motion(false);
    }

    #[test]
    fn ease_toward_steps_and_snaps() {
        let _g = hold_flag();
        let (v, moving) = ease_toward(0.0, 10.0, 1.0 / 60.0, 12.0, 0.5);
        assert!(moving && v > 0.0 && v < 10.0);
        // Within snap distance: lands exactly, reports settled.
        assert_eq!(ease_toward(9.8, 10.0, 1.0 / 60.0, 12.0, 0.5), (10.0, false));
        // Framerate independence: 60 Hz and 144 Hz land together.
        let run = |hz: f32| {
            let (mut v, dt) = (0.0, 1.0 / hz);
            for _ in 0..hz as usize {
                v = ease_toward(v, 10.0, dt, 12.0, 0.001).0;
            }
            v
        };
        assert!((run(60.0) - run(144.0)).abs() < 0.05);
    }

    fn run(mut animator: Animator, hz: f32, seconds: f32) -> f32 {
        let dt = 1.0 / hz;
        let steps = (seconds * hz) as usize;
        let mut p = 0.0;
        for _ in 0..steps {
            p = animator.step(dt);
        }
        p
    }

    #[test]
    fn eased_timeline_is_framerate_independent() {
        let cfg = CurveConfig {
            kind: CurveKind::EaseOutCubic,
            duration_ms: 200,
            ..CurveConfig::default()
        };
        let at_60 = run(Animator::new(&cfg), 60.0, 0.1);
        let at_144 = run(Animator::new(&cfg), 144.0, 0.1);
        assert!(
            (at_60 - at_144).abs() < 0.02,
            "60Hz={at_60} vs 144Hz={at_144}"
        );
    }

    #[test]
    fn eased_timeline_settles_at_one() {
        let cfg = CurveConfig {
            kind: CurveKind::EaseInCubic,
            duration_ms: 140,
            ..CurveConfig::default()
        };
        let mut a = Animator::new(&cfg);
        a.step(0.2);
        assert!(a.is_settled());
        assert!((a.step(0.016) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn spring_is_framerate_independent() {
        let cfg = CurveConfig::default(); // spring
        let at_60 = run(Animator::new(&cfg), 60.0, 0.15);
        let at_144 = run(Animator::new(&cfg), 144.0, 0.15);
        assert!(
            (at_60 - at_144).abs() < 0.02,
            "60Hz={at_60} vs 144Hz={at_144}"
        );
    }

    #[test]
    fn spring_settles_near_one_without_big_overshoot() {
        // Fixed engine params — independent of the tunable config default.
        let cfg = CurveConfig {
            kind: CurveKind::Spring,
            spring_stiffness: 550.0,
            spring_damping: 42.0,
            spring_mass: 1.0,
            ..CurveConfig::default()
        };
        let mut a = Animator::new(&cfg);
        let mut max_p: f32 = 0.0;
        for _ in 0..(2.0 * 144.0) as usize {
            max_p = max_p.max(a.step(1.0 / 144.0));
        }
        assert!(a.is_settled(), "spring never settled");
        assert!(max_p < 1.06, "overshoot too large: {max_p}");
    }

    #[test]
    fn lerp_endpoints() {
        assert_eq!(lerp(10.0, 20.0, 0.0), 10.0);
        assert_eq!(lerp(10.0, 20.0, 1.0), 20.0);
    }
}

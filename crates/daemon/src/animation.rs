//! dt-based animation primitives: eased timelines and a damped spring.
//!
//! Everything here is frame-rate independent — animators advance by a
//! wall-clock `dt`, never by a fixed per-frame increment. The unit tests
//! step the same animator at 60 Hz and 144 Hz and assert both land on the
//! same result.

use std::time::Duration;

use waverunner_core::config::{CurveConfig, CurveKind};

/// Linear interpolation between `a` and `b` by `t` (unclamped).
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Frame-rate-independent exponential approach: step `current` toward
/// `target` by the decay factor `1 − exp(−dt·rate)`, snapping onto the
/// target once within `snap`. Returns the new value and whether it is
/// still moving — the shared ease behind every "glide to rest" in the
/// daemon (page slides, the make-room reflows, the dock parting).
pub fn ease_toward(current: f32, target: f32, dt: f32, rate: f32, snap: f32) -> (f32, bool) {
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
    pub fn step(&mut self, target: f32, dt: f32) {
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

    /// Landed on `target` and still — no more frames needed to chase it.
    pub fn is_settled_at(&self, target: f32) -> bool {
        (self.pos - target).abs() < 0.001 && self.vel.abs() < 0.01
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

    #[test]
    fn ease_toward_steps_and_snaps() {
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

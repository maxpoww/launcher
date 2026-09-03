//! Jelly membrane: the four edge springs plus a delayed-impulse queue that
//! make a rounded panel (the main card, an open group box) wobble like jelly
//! when the pointer crosses its boundary.

use crate::animation::Follower;
use crate::content::{Rect, JELLY_C, JELLY_K, JELLY_KICK, JELLY_SCALE};

/// A rectangular jelly membrane: one damped spring per edge
/// (left/right/top/bottom) plus a queue of delayed impulses.
///
/// Each edge spring rests at `1.0`; a poke kicks it away and it rings back.
/// The pending queue holds `(edge_idx, velocity, delay_remaining_secs)`
/// impulses that fire once their timer elapses — the anticipation, main
/// kick, and Poisson cross-coupling of a single poke all arrive on
/// staggered delays.
pub struct JellyMembrane {
    left: Follower,
    right: Follower,
    top: Follower,
    bottom: Follower,
    /// Pending impulses: `(edge_idx 0..=3, velocity, delay_remaining_secs)`.
    pending: Vec<(u8, f32, f32)>,
}

impl Default for JellyMembrane {
    fn default() -> Self {
        Self::new()
    }
}

impl JellyMembrane {
    /// A membrane with all four edges at rest and no queued impulses.
    pub fn new() -> Self {
        Self {
            left: Follower::new(JELLY_K, JELLY_C),
            right: Follower::new(JELLY_K, JELLY_C),
            top: Follower::new(JELLY_K, JELLY_C),
            bottom: Follower::new(JELLY_K, JELLY_C),
            pending: Vec::new(),
        }
    }

    /// The edge spring for index `0..=3` (left/right/top/bottom).
    fn edge(&mut self, idx: u8) -> &mut Follower {
        match idx {
            0 => &mut self.left,
            1 => &mut self.right,
            2 => &mut self.top,
            _ => &mut self.bottom,
        }
    }

    /// Fire an organic edge-crossing poke on this membrane.
    ///
    /// `rect` is the membrane bounds, `pos`/`prev_pos` the pointer in
    /// surface px, and `entering` = the pointer just moved inside.
    ///
    /// Three layers combine into the poke:
    ///   1. Velocity-scaled kick — fast crossing = strong poke.
    ///   2. Anticipation — a tiny opposite kick fires now so the edge
    ///      briefly resists before giving way (membrane feel).
    ///   3. Poisson cross-coupling — adjacent perpendicular edges ripple in
    ///      after a short propagation delay (compress one axis → expand the
    ///      other).
    ///
    /// Sign convention per edge: left/top positive = inward (right/down);
    /// right/bottom positive = outward (right/down).
    pub fn poke(
        &mut self,
        rect: Rect,
        pos: (f32, f32),
        prev_pos: Option<(f32, f32)>,
        entering: bool,
    ) {
        let cx = rect.x + rect.w * 0.5;
        let cy = rect.y + rect.h * 0.5;
        let nx = (pos.0 - cx) / (rect.w * 0.5 + 1.0);
        let ny = (pos.1 - cy) / (rect.h * 0.5 + 1.0);

        // Pointer speed (px since last event) → kick scale.
        let speed = prev_pos
            .map(|(px, py)| {
                let dx = pos.0 - px;
                let dy = pos.1 - py;
                (dx * dx + dy * dy).sqrt()
            })
            .unwrap_or(10.0);
        let speed_factor = (speed / 10.0).clamp(0.35, 2.0);
        let k = JELLY_KICK * speed_factor;
        let cross = k * 0.14; // Poisson coupling fraction

        // dir: +1 entering (inward), -1 leaving (outward).
        let dir = if entering { 1.0_f32 } else { -1.0_f32 };
        const ANTICIPATION: f32 = 0.09;
        const MAIN_MS: f32 = 0.008;
        const CROSS_MS: f32 = 0.028;
        if nx.abs() > ny.abs() {
            if nx > 0.0 {
                // Right edge crossed: inward = leftward = negative for the right spring.
                self.right.kick(dir * k * ANTICIPATION); // resist first
                self.pending.push((1, -dir * k, MAIN_MS));
                // Poisson: top/bottom expand outward on compression.
                self.pending.push((2, -dir * cross, CROSS_MS));
                self.pending.push((3, dir * cross, CROSS_MS));
            } else {
                // Left edge crossed: inward = rightward = positive for the left spring.
                self.left.kick(-dir * k * ANTICIPATION);
                self.pending.push((0, dir * k, MAIN_MS));
                self.pending.push((2, -dir * cross, CROSS_MS));
                self.pending.push((3, dir * cross, CROSS_MS));
            }
        } else if ny > 0.0 {
            // Bottom edge crossed: inward = upward = negative for the bottom spring.
            self.bottom.kick(dir * k * ANTICIPATION);
            self.pending.push((3, -dir * k, MAIN_MS));
            // Poisson: left/right expand outward on compression.
            self.pending.push((0, -dir * cross, CROSS_MS));
            self.pending.push((1, dir * cross, CROSS_MS));
        } else {
            // Top edge crossed: inward = downward = positive for the top spring.
            self.top.kick(-dir * k * ANTICIPATION);
            self.pending.push((2, dir * k, MAIN_MS));
            self.pending.push((0, -dir * cross, CROSS_MS));
            self.pending.push((1, dir * cross, CROSS_MS));
        }
    }

    /// Drain delayed kicks: decrement each timer by `dt`, apply the velocity
    /// to its edge spring once elapsed, else re-queue. The take/push pattern
    /// avoids borrowing the queue and the spring fields simultaneously.
    pub fn drain(&mut self, dt: f32) {
        let queued = std::mem::take(&mut self.pending);
        for (edge, vel, delay) in queued {
            let remaining = delay - dt;
            if remaining <= 0.0 {
                self.edge(edge).kick(vel);
            } else {
                self.pending.push((edge, vel, remaining));
            }
        }
    }

    /// Advance all four edge springs one step toward rest.
    pub fn step(&mut self, dt: f32) {
        self.left.step(1.0, dt);
        self.right.step(1.0, dt);
        self.top.step(1.0, dt);
        self.bottom.step(1.0, dt);
    }

    /// Any edge still ringing or moving (ignores the pending queue — see
    /// [`has_pending`](Self::has_pending)).
    pub fn is_active(&self) -> bool {
        self.left.is_active()
            || self.right.is_active()
            || self.top.is_active()
            || self.bottom.is_active()
    }

    /// Whether delayed impulses are still waiting to fire.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Snap all four edges exactly to rest.
    pub fn snap(&mut self) {
        self.left.snap();
        self.right.snap();
        self.top.snap();
        self.bottom.snap();
    }

    /// Per-edge pixel offsets `(left, right, top, bottom)` for the current
    /// spring positions, scaled to surface px. Feeds `card_push` / `box_push`.
    pub fn offsets(&self) -> (f32, f32, f32, f32) {
        let s = JELLY_SCALE;
        (
            (self.left.pos - 1.0) * s,
            (self.right.pos - 1.0) * s,
            (self.top.pos - 1.0) * s,
            (self.bottom.pos - 1.0) * s,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 200.0,
        h: 100.0,
    };

    /// Drain everything, then run the membrane at display cadence for
    /// `secs` seconds.
    fn run(m: &mut JellyMembrane, secs: f32) {
        m.drain(1.0); // all delays are < 1 s
        let mut t = 0.0;
        while t < secs {
            m.step(1.0 / 60.0);
            t += 1.0 / 60.0;
        }
    }

    #[test]
    fn poke_targets_the_crossed_edge() {
        // Crossing near the right edge must displace the right spring most.
        let mut m = JellyMembrane::new();
        m.poke(RECT, (198.0, 50.0), Some((205.0, 50.0)), true);
        m.drain(1.0);
        m.step(0.05);
        let (l, r, _t, _b) = m.offsets();
        assert!(r.abs() > 0.0, "right spring got the main kick");
        assert!(
            r.abs() > l.abs(),
            "main kick outweighs cross-coupling (r={r}, l={l})"
        );
    }

    #[test]
    fn membrane_always_comes_to_rest() {
        // The frame loop's liveness depends on this: is_active() drives
        // redraws, so a spring that never settles pins the daemon at
        // 60 fps forever. Worst-case poke (max speed factor) must settle.
        let mut m = JellyMembrane::new();
        m.poke(RECT, (198.0, 50.0), Some((0.0, 50.0)), true); // huge speed, clamped
        run(&mut m, 5.0);
        assert!(!m.is_active(), "membrane still ringing after 5 s");
        assert!(!m.has_pending());
        let (l, r, t, b) = m.offsets();
        for (name, v) in [("l", l), ("r", r), ("t", t), ("b", b)] {
            assert!(v.abs() < 0.01, "{name} edge rests off-zero: {v}");
        }
    }

    #[test]
    fn impulses_fire_exactly_once_across_fragmented_drains() {
        let mut m = JellyMembrane::new();
        m.poke(RECT, (198.0, 50.0), Some((190.0, 50.0)), true);
        assert!(m.has_pending(), "poke queues delayed impulses");
        // Fragmented dt smaller than any delay: nothing may be lost.
        for _ in 0..40 {
            m.drain(0.001);
        }
        assert!(!m.has_pending(), "all impulses fired after 40 ms of drains");
        // A further drain on the empty queue is a no-op (no double kicks:
        // capture the state and confirm draining again changes nothing).
        let before = m.offsets();
        m.drain(1.0);
        assert_eq!(before, m.offsets());
    }

    #[test]
    fn kick_magnitude_is_bounded_for_wild_pointer_jumps() {
        // A pointer warp (thousands of px between events) must not launch
        // the membrane into orbit: the speed factor clamps at 2.0. Compare
        // peak displacement against a moderate crossing.
        let peak = |speed_from: f32| {
            let mut m = JellyMembrane::new();
            m.poke(RECT, (198.0, 50.0), Some((speed_from, 50.0)), true);
            m.drain(1.0);
            let mut worst = 0.0f32;
            for _ in 0..300 {
                m.step(1.0 / 60.0);
                let (_, r, _, _) = m.offsets();
                worst = worst.max(r.abs());
            }
            worst
        };
        let warp = peak(90_000.0);
        let moderate = peak(178.0); // 20 px between events → factor 2.0 exactly
        assert!(
            warp <= moderate * 1.01,
            "warp {warp} vs moderate {moderate}"
        );
    }

    #[test]
    fn snap_rests_edges_but_preserves_pending() {
        // The frame loop only snaps when has_pending() is false — snap
        // deliberately does NOT clear the queue, so a mid-flight wobble
        // can't be half-swallowed. This test pins that contract; if snap
        // ever starts draining the queue, frame.rs's guard becomes dead
        // code and this fails.
        let mut m = JellyMembrane::new();
        m.poke(RECT, (198.0, 50.0), Some((190.0, 50.0)), true);
        m.snap();
        assert_eq!(m.offsets(), (0.0, 0.0, 0.0, 0.0), "edges snapped to rest");
        assert!(m.has_pending(), "pending impulses survive a snap");
    }
}

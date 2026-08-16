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

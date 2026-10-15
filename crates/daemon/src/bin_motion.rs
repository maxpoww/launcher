//! THE BIN'S MOTION on the dock (Max, 2026-10-15, the Dock Motion Lab —
//! https://claude.ai/artifact/Pn8Ao3JUeU6JKDiwxUhobo). Five states, each with
//! its own move, fed from anywhere in the shell:
//!
//! - HOVER (nothing in hand): the lid lifts, tilted up on its left, and the
//!   can gives a little shake below it, once a pass;
//! - DRAGGING something anywhere (a desktop icon, a file window's things, an
//!   app): the tile goes the traffic light's ORANGE, the lid stands lifted,
//!   the can shakes once;
//! - OVER it with something: RED, the tile swells, the lid wide open;
//! - OPEN (a file window is in the bin): the lid up and aside, a soft rim;
//! - DROPPED on it: the red goes, the lid shuts with a tap, the can swallows.
//!
//! [`BinMotion::step`] eases it all and [`BinMotion::pose`] says where the
//! can and the lid stand, in the figure's 100 units.

use crate::animation::ease_toward;

/// How high the lid stands lifted, in the figure's units (the lab's "Altura
/// de la tapa abierta"), and how far it tilts.
const LIFT: f32 = 8.0;
const TILT: f32 = 5.0;
/// The ease of every state change (≈ the lab's 220 ms, soft).
const RATE: f32 = 16.0;
/// The can's shake: its length and its swings (degrees) at those shares of it.
const SHAKE_SECS: f32 = 0.44;
const SHAKE: [(f32, f32); 6] = [(0.0, 0.0), (0.2, -3.5), (0.45, 2.5), (0.7, -1.2), (0.88, 0.5), (1.0, 0.0)];
/// The swallow after a drop.
const DROP_SECS: f32 = 0.85;

/// What the shell says this frame.
#[derive(Clone, Copy, Default)]
pub(crate) struct Inputs {
    /// The pointer on the bin's tile with nothing in hand.
    pub hover: bool,
    /// Something of ours in hand, anywhere.
    pub dragging: bool,
    /// ...and over the bin.
    pub over: bool,
    /// A file window is in the bin.
    pub open: bool,
}

/// Where everything stands this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Pose {
    /// The tile's washes (0..1): the traffic light's orange, then its red.
    pub orange: f32,
    pub red: f32,
    /// The tile's size (1 = rest).
    pub swell: f32,
    /// The open rim (0..1).
    pub open: f32,
    /// The lid: moved by (dx, dy), turned by `rot` degrees about its middle
    /// (50, 30).
    pub lid: (f32, f32, f32),
    /// The can: turned `rot` degrees and scaled (sx, sy) about its foot (50, 82).
    pub can: (f32, f32, f32),
}

#[derive(Default)]
pub(crate) struct BinMotion {
    last: Inputs,
    lift: f32,
    wide: f32,
    aside: f32,
    orange: f32,
    red: f32,
    /// The shake's clock (None = still).
    shake: Option<f32>,
    /// The swallow, once something is let go over it.
    drop: Option<Swallow>,
}

/// The swallow's clock (0..1) and where the lid, the red and the swell stood
/// when it began.
#[derive(Clone, Copy)]
struct Swallow {
    t: f32,
    lid: (f32, f32, f32),
    red: f32,
    swell: f32,
}

/// Smoothstep between two keyframes.
fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A keyframed value at share `t`.
fn keyed(keys: &[(f32, f32)], t: f32) -> f32 {
    for w in keys.windows(2) {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        if t <= t1 {
            return v0 + (v1 - v0) * smooth((t - t0) / (t1 - t0).max(1e-6));
        }
    }
    keys.last().map_or(0.0, |k| k.1)
}

impl BinMotion {
    /// Step by `dt` seconds; whether anything still moves.
    pub(crate) fn step(&mut self, now: Inputs, dt: f32) -> bool {
        let was = self.last;
        self.last = now;
        // Let go over it: the swallow (whatever stood, it goes from there).
        if was.over && !now.dragging {
            let p = self.pose();
            self.drop = Some(Swallow { t: 0.0, lid: p.lid, red: p.red, swell: p.swell });
            (self.lift, self.wide, self.aside, self.orange, self.red, self.shake) = (0.0, 0.0, 0.0, 0.0, 0.0, None);
        }
        // The can shakes as the pointer comes on it, and as a drag begins —
        // never while it swallows: the hover waits for the swallow to end,
        // then comes in whole (the lid lifts, the can shakes).
        let swallowing = self.drop.is_some();
        if !swallowing && ((now.hover && !was.hover && !now.dragging) || (now.dragging && !was.dragging)) {
            self.shake = Some(0.0);
        }
        let mut moving = false;
        let mut ease = |v: &mut f32, to: bool| {
            let (n, m) = ease_toward(*v, if to { 1.0 } else { 0.0 }, dt, RATE, 0.002);
            *v = n;
            moving |= m;
        };
        if self.drop.is_none() {
            ease(&mut self.lift, now.hover || now.dragging);
            ease(&mut self.wide, now.over);
            ease(&mut self.orange, now.dragging && !now.over);
            ease(&mut self.red, now.over);
        }
        ease(&mut self.aside, now.open && !now.dragging);
        if let Some(t) = &mut self.shake {
            *t += dt / SHAKE_SECS;
            if *t >= 1.0 {
                self.shake = None;
            }
            moving = true;
        }
        if let Some(d) = &mut self.drop {
            d.t += dt / DROP_SECS;
            if d.t >= 1.0 {
                self.drop = None;
                // The pointer still on it: now the hover.
                if now.hover {
                    self.shake = Some(0.0);
                }
            }
            moving = true;
        }
        moving
    }

    pub(crate) fn pose(&self) -> Pose {
        let mut p = Pose {
            orange: self.orange,
            red: self.red,
            swell: 1.0 + 0.12 * self.wide,
            open: self.aside,
            lid: (
                14.0 * self.aside,
                -LIFT * (self.lift + 1.1 * self.wide) - 1.6 * LIFT * self.aside,
                TILT * self.lift + 8.0 * self.wide + 24.0 * self.aside,
            ),
            can: (0.0, 1.0, 1.0),
        };
        if let Some(t) = self.shake {
            p.can.0 = keyed(&SHAKE, t);
        }
        if let Some(Swallow { t, lid, red, swell }) = self.drop {
            // The red and the swell go; the lid shuts with a tap; the can swallows.
            let fade = 1.0 - smooth(t / 0.45);
            p.red = red * fade;
            p.swell = 1.0 + (swell - 1.0) * fade;
            let shut = 1.0 - smooth(t / 0.28);
            let tap = keyed(&[(0.0, 0.0), (0.28, 0.0), (0.33, 1.2), (0.4, -1.0), (0.48, 0.0), (1.0, 0.0)], t);
            p.lid = (p.lid.0 + lid.0 * shut, p.lid.1 + lid.1 * shut + tap, p.lid.2 + lid.2 * shut);
            p.can = (
                0.0,
                keyed(&[(0.0, 1.0), (0.34, 1.0), (0.46, 1.06), (0.6, 0.96), (0.72, 1.02), (0.84, 1.0), (1.0, 1.0)], t),
                keyed(&[(0.0, 1.0), (0.34, 1.0), (0.46, 0.9), (0.6, 1.05), (0.72, 0.98), (0.84, 1.0), (1.0, 1.0)], t),
            );
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(m: &mut BinMotion, i: Inputs, secs: f32) {
        let mut t = 0.0;
        while t < secs {
            m.step(i, 0.016);
            t += 0.016;
        }
    }

    #[test]
    fn each_state_has_its_look() {
        let mut m = BinMotion::default();
        assert_eq!(m.pose().swell, 1.0);
        run(&mut m, Inputs { hover: true, ..Default::default() }, 0.6);
        assert!(m.pose().lid.1 < -7.0 && m.pose().orange == 0.0, "hover lifts the lid, no colour");
        run(&mut m, Inputs { dragging: true, ..Default::default() }, 0.6);
        assert!(m.pose().orange > 0.95 && m.pose().red < 0.05, "dragging: orange");
        run(&mut m, Inputs { dragging: true, over: true, ..Default::default() }, 0.6);
        let over = m.pose();
        assert!(over.red > 0.95 && over.orange < 0.05 && over.swell > 1.1 && over.lid.1 < -15.0, "over: red, swollen, wide open");
        // Let go over it: the swallow, then all at rest.
        m.step(Inputs::default(), 0.016);
        run(&mut m, Inputs::default(), 0.25);
        assert!(m.pose().can.2 < 1.0 || m.pose().can.2 > 1.0 || m.pose().red < over.red, "swallowing");
        run(&mut m, Inputs::default(), 1.0);
        let rest = m.pose();
        assert!(rest.red < 0.01 && rest.lid.1.abs() < 0.05 && (rest.swell - 1.0).abs() < 0.01 && rest.can == (0.0, 1.0, 1.0));
        run(&mut m, Inputs { open: true, ..Default::default() }, 0.6);
        assert!(m.pose().open > 0.95 && m.pose().lid.0 > 13.0, "open: the lid aside");
    }

    #[test]
    fn let_go_with_the_pointer_on_it_the_swallow_comes_first_then_the_hover() {
        let mut m = BinMotion::default();
        run(&mut m, Inputs { dragging: true, over: true, ..Default::default() }, 0.6);
        // Let go: the pointer is on the bin with nothing in hand any more.
        let hover = Inputs { hover: true, ..Default::default() };
        m.step(hover, 0.016);
        run(&mut m, hover, 0.5);
        assert!(m.shake.is_none() && m.lift < 0.01, "no hover while it swallows");
        run(&mut m, hover, 0.4);
        assert!(m.drop.is_none(), "swallowed");
        assert!(m.shake.is_some() || m.lift > 0.5, "then the hover comes in");
        run(&mut m, hover, 0.6);
        assert!(m.pose().lid.1 < -7.0, "the lid lifted for the hover");
    }
}

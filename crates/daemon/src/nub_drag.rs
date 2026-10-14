//! The [current task] pill's right-click "nub drag" (2026-09-18).
//!
//! Right-click the current-task pill and HOLD: the cursor LOCKS in place
//! (Wayland pointer-constraints) and raw touchpad deltas (relative-pointer) move
//! the focused window while you hold, magnetically snapping to the usable
//! screen's edges and corners. Release to DROP. A right-click that never moved
//! (press-release with no motion) centres the window instead.
//!
//! All the Wayland plumbing — the two manager globals, the four `Dispatch`
//! impls, and the `App` fields — lives in `main.rs`; this file is the gesture
//! logic. Moves go out over the *blocking* Hyprland socket, so they are
//! rate-limited to one per [`MOVE_INTERVAL`].

use std::time::{Duration, Instant};

use smithay_client_toolkit::shell::WaylandSurface;
use tracing::debug;
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_locked_pointer_v1::ZwpLockedPointerV1, zwp_pointer_constraints_v1::Lifetime,
};
use wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1::ZwpRelativePointerV1;

use crate::{hypr, App};

/// Magnetic snap distance to a usable-area edge (logical px).
const SNAP: f64 = 24.0;
/// Net cursor travel under this (logical px) counts as a *click*, not a drag —
/// a start-then-stop with no movement, which centres the window.
const CLICK_MAX: f64 = 6.0;
/// Touchpad delta → window pixels. 1:1; the dial if the drag feels off.
const GAIN: f64 = 1.0;
/// Minimum spacing between window-move dispatches. Each is a blocking Hyprland
/// socket round-trip, so uncapped touchpad motion would flood the event loop.
const MOVE_INTERVAL: Duration = Duration::from_millis(12);

/// State of an in-flight nub drag.
pub(crate) struct NubDrag {
    locked: ZwpLockedPointerV1,
    relative: ZwpRelativePointerV1,
    addr: String,
    start_x: i32,
    start_y: i32,
    win_w: i32,
    win_h: i32,
    /// Usable-area edges (logical): left, top, right, bottom — the snap targets.
    /// Infinite when the monitor couldn't be read, which disables snapping.
    ux0: f64,
    uy0: f64,
    ux1: f64,
    uy1: f64,
    acc_x: f64,
    acc_y: f64,
    last_move: Instant,
}

impl App {
    /// Begin a nub drag: lock the cursor on the OPTIONS surface and open a
    /// relative-pointer feed. No-op if the protocols are missing, a drag is
    /// already running, or there is no focused window to move.
    pub(crate) fn nub_start(&mut self) {
        if self.nub_drag.is_some() {
            return;
        }
        let (Some(pc), Some(rpm), Some(ptr)) = (
            self.pointer_constraints.clone(),
            self.relative_pointer_manager.clone(),
            self.pointer.clone(),
        ) else {
            return;
        };
        let Some(surface) = self.options_layer.as_ref().map(|l| l.wl_surface().clone()) else {
            return;
        };
        let Some(addr) = hypr::active_window() else {
            return;
        };
        let Some((start_x, start_y, win_w, win_h)) = hypr::active_window_geom() else {
            return;
        };
        // Usable rect from the focused monitor; if it can't be read, snapping is
        // disabled (infinite edges never trip) but the drag still works.
        let (ux0, uy0, ux1, uy1) = match hypr::focused_monitor() {
            Ok(m) => {
                let (l, t, r, b) = m.reserved;
                (m.x + l, m.y + t, m.x + m.w - r, m.y + m.h - b)
            }
            Err(_) => (
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::INFINITY,
            ),
        };
        let qh = self.qh.clone();
        // Lock the pointer to where it sits (cursor stops moving) and open the
        // relative feed that still reports deltas while it is locked. Flush now
        // so the compositor engages both before the first motion arrives.
        let locked = pc.lock_pointer(&surface, &ptr, None, Lifetime::Oneshot, &qh, ());
        let relative = rpm.get_relative_pointer(&ptr, &qh, ());
        let _ = self.conn.flush();
        debug!("nub drag: grabbed {addr} at ({start_x},{start_y})");
        // The window is in hand: its card is away until it is dropped.
        self.card_window_lifted(&addr);
        self.nub_drag = Some(NubDrag {
            locked,
            relative,
            addr,
            start_x,
            start_y,
            win_w,
            win_h,
            ux0,
            uy0,
            ux1,
            uy1,
            acc_x: 0.0,
            acc_y: 0.0,
            last_move: Instant::now(),
        });
    }

    /// A relative-pointer delta while dragging: accumulate and (rate-limited)
    /// move the window to `start + accumulated`, snapped to the edges.
    pub(crate) fn nub_motion(&mut self, dx: f64, dy: f64) {
        let Some(nd) = self.nub_drag.as_mut() else {
            return;
        };
        nd.acc_x += dx * GAIN;
        nd.acc_y += dy * GAIN;
        if nd.last_move.elapsed() < MOVE_INTERVAL {
            return;
        }
        nd.last_move = Instant::now();
        let (x, y) = nd.target();
        move_window(&nd.addr, x, y);
    }

    /// End the drag on the second right-click: a start-then-stop that barely
    /// moved centres the window; otherwise it lands where it was dragged. Then
    /// the lock and relative feed are torn down and the cursor is free again.
    pub(crate) fn nub_end(&mut self) {
        let Some(nd) = self.nub_drag.take() else {
            return;
        };
        let travelled = (nd.acc_x * nd.acc_x + nd.acc_y * nd.acc_y).sqrt();
        if travelled < CLICK_MAX && self.side_click(&nd.addr) {
            // A window of the side's: called to the centre, or parked again
            // (`side.rs` owns that round trip).
        } else if travelled < CLICK_MAX {
            // A click, not a drag: centre the window in the usable area (only if
            // we actually read the monitor — otherwise leave it where it is).
            if nd.ux0.is_finite() && nd.ux1.is_finite() {
                let x = (nd.ux0 + (nd.ux1 - nd.ux0 - nd.win_w as f64) / 2.0).round() as i32;
                let y = (nd.uy0 + (nd.uy1 - nd.uy0 - nd.win_h as f64) / 2.0).round() as i32;
                // …and BACK on the next click (Max, 2026-10-12: the place it
                // was centred FROM is remembered, a second right-click on
                // the pill returns it there, and the place stays remembered
                // until the window closes — see `click_move`).
                let home = self.nub_home.get(&nd.addr).copied();
                let (to, keep) = click_move((nd.start_x, nd.start_y), (x, y), home);
                match keep {
                    Some(h) => self.nub_home.insert(nd.addr.clone(), h),
                    None => self.nub_home.remove(&nd.addr),
                };
                if let Some((tx, ty)) = to {
                    move_window(&nd.addr, tx, ty);
                }
            }
        } else {
            let (x, y) = nd.target();
            move_window(&nd.addr, x, y);
            // A parked window dragged by the pill: let go far enough out,
            // else snapped back into its place (`side.rs`).
            self.side_drag_ended(&nd.addr);
        }
        nd.locked.destroy();
        nd.relative.destroy();
        let _ = self.conn.flush();
        debug!("nub drag: dropped {} (travelled {travelled:.0})", nd.addr);
        self.card_window_placed(&nd.addr);
    }
}

/// A window's position (its top-left corner, in layout coordinates).
type Place = (i32, i32);

/// How near the centre a window has to stand to count as centred: a window
/// can settle a pixel or two off where it was sent (odd sizes, the app's own
/// size steps), and that must still read as "it is in the centre".
const CENTRED_WITHIN: i32 = 3;

/// What a right-click on the pill does to a window standing at `at`, given the
/// `centre` it would be centred at and the place remembered for it (`home`):
/// where to move it (if anywhere), and the place to remember afterwards.
///
/// * Not in the centre → it goes to the centre, and where it stood is the
///   place remembered (a window moved by hand since the last time has a new
///   one).
/// * In the centre, with a place remembered → it goes back there, and the
///   place STAYS remembered: the next click centres it again and the one
///   after returns it, as often as wanted, until the window closes.
/// * In the centre with nothing remembered → nothing to do.
fn click_move(at: Place, centre: Place, home: Option<Place>) -> (Option<Place>, Option<Place>) {
    let centred =
        (at.0 - centre.0).abs() <= CENTRED_WITHIN && (at.1 - centre.1).abs() <= CENTRED_WITHIN;
    match (centred, home) {
        (false, _) => (Some(centre), Some(at)),
        (true, Some(h)) => (Some(h), Some(h)),
        (true, None) => (None, None),
    }
}

impl App {
    /// A window closed: the place its centring remembered goes with it
    /// (addresses are reused, so it must not wait for another window).
    pub(crate) fn nub_window_closed(&mut self, addr: &str) {
        self.nub_home.remove(addr);
    }
}

impl NubDrag {
    fn target(&self) -> (i32, i32) {
        snap_target(
            self.start_x, self.start_y, self.win_w, self.win_h, self.acc_x, self.acc_y, self.ux0,
            self.uy0, self.ux1, self.uy1,
        )
    }
}

// ─────────────────────── the TOUCHPAD path: scroll-drag ──────────────────────
//
// On this touchpad "right-click-hold-and-move" is a two-finger SCROLL (verified:
// zero button-holds, hundreds of scroll events — the cursor freezes because
// scroll doesn't move it). So the gesture that actually fits is: two-finger
// scroll while hovering the [current task] pill moves the focused window, and
// the still cursor is exactly the "frozen pointer" Max wanted.

/// Scroll units → window pixels. NEGATIVE so the window follows the fingers
/// (raw scroll direction is inverted for a drag), and >1 for a bit more speed.
const SCROLL_GAIN: f64 = -2.0;
/// A gap longer than this between scroll events starts a FRESH drag — the window
/// is re-grabbed from where it now sits, so a second push continues cleanly
/// instead of snapping back to the first grab's origin.
const SCROLL_REGRAB: Duration = Duration::from_millis(200);

/// A THROW: a short, fast two-finger swipe on the pill that stops sends the
/// window to the side (`side.rs`; Max, 2026-10-14: *"a fast short swipe on
/// the task pill to send the windows there"*). Judged the card's way
/// (`card/place.rs`) — when the swipe STOPS, by how brief it was: a flick is
/// over in a moment, while moving the window fast by hand is a longer scroll
/// however quick. So: the whole run lasted no longer than [`FLING_BRIEF`],
/// covered at least [`FLING_SCROLL`] (scroll units, any direction), never
/// turned back, and then went quiet for [`FLING_QUIET`].
const FLING_SCROLL: f64 = 40.0;
const FLING_BRIEF: Duration = Duration::from_millis(170);
const FLING_QUIET: Duration = Duration::from_millis(30);

/// One run of scroll on the pill: when it began, its last step, what it adds
/// up to, and whether it ever turned back.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fling {
    began: Instant,
    last: Instant,
    sx: f64,
    sy: f64,
    turned: bool,
}

impl Fling {
    /// The run after one more step of (`dx`, `dy`) at `now`: the same run if
    /// the step came soon enough after the last, a new one otherwise.
    fn step(run: Option<Fling>, now: Instant, dx: f64, dy: f64) -> Fling {
        match run {
            Some(f) if now.saturating_duration_since(f.last) <= FLING_QUIET => Fling {
                last: now,
                sx: f.sx + dx,
                sy: f.sy + dy,
                turned: f.turned || f.sx * dx + f.sy * dy < 0.0,
                ..f
            },
            _ => Fling { began: now, last: now, sx: dx, sy: dy, turned: false },
        }
    }

    fn stopped(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last) > FLING_QUIET
    }

    /// Whether, now that it has stopped, it was a throw.
    fn thrown(&self) -> bool {
        !self.turned
            && self.last.saturating_duration_since(self.began) <= FLING_BRIEF
            && (self.sx * self.sx + self.sy * self.sy).sqrt() >= FLING_SCROLL
    }
}

/// A scroll-driven window drag (no button, no lock — scroll already leaves the
/// cursor still). Re-grabs on a fresh gesture; accumulates and moves otherwise.
pub(crate) struct ScrollDrag {
    addr: String,
    start_x: i32,
    start_y: i32,
    win_w: i32,
    win_h: i32,
    ux0: f64,
    uy0: f64,
    ux1: f64,
    uy1: f64,
    acc_x: f64,
    acc_y: f64,
    last_scroll: Instant,
    last_move: Instant,
    /// The run of scroll steps being judged for a throw.
    fling: Option<Fling>,
    /// A judge is waiting for the run to stop.
    judging: bool,
}

impl App {
    /// Feed a scroll delta (dx, dy in scroll units) into the scroll drag while
    /// the pointer is over the current-task pill. `dx`/`dy` are the raw axis
    /// values; the window follows them.
    pub(crate) fn nub_scroll(&mut self, dx: f64, dy: f64) {
        let fresh = match &self.scroll_drag {
            None => true,
            Some(sd) => sd.last_scroll.elapsed() > SCROLL_REGRAB,
        };
        if fresh {
            let Some(addr) = hypr::active_window() else {
                return;
            };
            let Some((x, y, w, h)) = hypr::active_window_geom() else {
                return;
            };
            let (ux0, uy0, ux1, uy1) = match hypr::focused_monitor() {
                Ok(m) => {
                    let (l, t, r, b) = m.reserved;
                    (m.x + l, m.y + t, m.x + m.w - r, m.y + m.h - b)
                }
                Err(_) => (
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                    f64::INFINITY,
                    f64::INFINITY,
                ),
            };
            self.scroll_drag = Some(ScrollDrag {
                addr,
                start_x: x,
                start_y: y,
                win_w: w,
                win_h: h,
                ux0,
                uy0,
                ux1,
                uy1,
                acc_x: 0.0,
                acc_y: 0.0,
                last_scroll: Instant::now(),
                last_move: Instant::now(),
                fling: None,
                judging: false,
            });
            // A parked window scrolled about is in hand: watched for leaving
            // the side, snapped back when it goes still (`side.rs`).
            let addr = self.scroll_drag.as_ref().map(|sd| sd.addr.clone()).unwrap_or_default();
            self.side_drag_began(&addr, true);
        }
        let Some(sd) = self.scroll_drag.as_mut() else {
            return;
        };
        sd.acc_x += dx * SCROLL_GAIN;
        sd.acc_y += dy * SCROLL_GAIN;
        let now = Instant::now();
        sd.last_scroll = now;
        sd.fling = Some(Fling::step(sd.fling, now, dx, dy));
        if !sd.judging {
            sd.judging = true;
            self.arm_fling_judge();
        }
        let Some(sd) = self.scroll_drag.as_mut() else {
            return;
        };
        // The card is away while the window is being scrolled about.
        let addr = sd.addr.clone();
        self.card_window_nudged(&addr);
        let Some(sd) = self.scroll_drag.as_mut() else {
            return;
        };
        if sd.last_move.elapsed() < MOVE_INTERVAL {
            return;
        }
        sd.last_move = Instant::now();
        let (tx, ty) = snap_target(
            sd.start_x, sd.start_y, sd.win_w, sd.win_h, sd.acc_x, sd.acc_y, sd.ux0, sd.uy0, sd.ux1,
            sd.uy1,
        );
        move_window(&sd.addr, tx, ty);
    }
}

impl App {
    /// Wait for the run of scroll on the pill to stop, then judge it: a throw
    /// sends the window to the side and ends the drag; anything else was the
    /// plain drag it looked like. One judge per run; it waits on while steps
    /// keep coming.
    fn arm_fling_judge(&mut self) {
        let timer = calloop::timer::Timer::from_duration(FLING_QUIET + Duration::from_millis(5));
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            let Some(sd) = app.scroll_drag.as_mut() else {
                return calloop::timer::TimeoutAction::Drop;
            };
            let Some(f) = sd.fling else {
                sd.judging = false;
                return calloop::timer::TimeoutAction::Drop;
            };
            if !f.stopped(Instant::now()) {
                return calloop::timer::TimeoutAction::ToDuration(FLING_QUIET);
            }
            sd.judging = false;
            sd.fling = None;
            if f.thrown() {
                let addr = sd.addr.clone();
                debug!("pill scroll: a throw ({:.0} units in {:?})", (f.sx * f.sx + f.sy * f.sy).sqrt(), f.last - f.began);
                app.scroll_drag = None;
                app.side_throw(&addr);
            }
            calloop::timer::TimeoutAction::Drop
        });
    }
}

/// The window's target top-left = start + accumulated, with each axis
/// magnetically snapped when its near edge is within [`SNAP`] of the usable
/// area's edge. Corners fall out of both axes snapping at once.
#[allow(clippy::too_many_arguments)]
fn snap_target(
    start_x: i32,
    start_y: i32,
    win_w: i32,
    win_h: i32,
    acc_x: f64,
    acc_y: f64,
    ux0: f64,
    uy0: f64,
    ux1: f64,
    uy1: f64,
) -> (i32, i32) {
    let (w, h) = (win_w as f64, win_h as f64);
    let mut x = start_x as f64 + acc_x;
    let mut y = start_y as f64 + acc_y;
    if (x - ux0).abs() <= SNAP {
        x = ux0;
    } else if (x + w - ux1).abs() <= SNAP {
        x = ux1 - w;
    }
    if (y - uy0).abs() <= SNAP {
        y = uy0;
    } else if (y + h - uy1).abs() <= SNAP {
        y = uy1 - h;
    }
    (x.round() as i32, y.round() as i32)
}

/// Move a window to an absolute logical position via the Hyprland socket.
pub(crate) fn move_window(addr: &str, x: i32, y: i32) {
    hypr::dispatch(&format!(
        "hl.dsp.window.move({{ x = {x}, y = {y}, window = \"address:{addr}\" }})"
    ));
}

// ─────────────────────── the PINCH path: pinch-to-resize ─────────────────────
//
// A two-finger pinch while hovering the [current task] pill resizes the focused
// window about its centre (Max, 2026-09-18). The pinch protocol gives an
// ABSOLUTE `scale` since the pinch began (1.0 = unchanged), so the new size is
// simply start-size × scale — no accumulation. Verified reaching the bar.

/// Window size clamps (logical px) so a hard pinch can't shrink it to nothing or
/// blow it past the screen.
const PINCH_MIN_W: f64 = 360.0;
const PINCH_MIN_H: f64 = 240.0;
const PINCH_MAX_W: f64 = 3840.0;
const PINCH_MAX_H: f64 = 2160.0;

/// An in-flight pinch resize: the window's size at pinch-start and the fixed
/// centre it grows/shrinks around.
pub(crate) struct PinchResize {
    addr: String,
    start_w: f64,
    start_h: f64,
    cx: f64,
    cy: f64,
    last_resize: Instant,
}

impl App {
    /// Pinch began over the pill: snapshot the window's size + centre.
    pub(crate) fn pinch_begin(&mut self) {
        let Some(addr) = hypr::active_window() else {
            return;
        };
        // A parked window's size is the column's (`side.rs`): a pinch does
        // nothing to it.
        if self.side.has(&addr) {
            return;
        }
        let Some((x, y, w, h)) = hypr::active_window_geom() else {
            return;
        };
        self.card_window_lifted(&addr);
        self.pinch_drag = Some(PinchResize {
            addr,
            start_w: w as f64,
            start_h: h as f64,
            cx: x as f64 + w as f64 / 2.0,
            cy: y as f64 + h as f64 / 2.0,
            last_resize: Instant::now(),
        });
    }

    /// A pinch update: resize the window to start-size × `scale`, keeping its
    /// centre fixed. Rate-limited (each dispatch is a blocking socket round-trip).
    pub(crate) fn pinch_scale(&mut self, scale: f64) {
        let Some(pd) = self.pinch_drag.as_mut() else {
            return;
        };
        if pd.last_resize.elapsed() < MOVE_INTERVAL {
            return;
        }
        pd.last_resize = Instant::now();
        let nw = (pd.start_w * scale).clamp(PINCH_MIN_W, PINCH_MAX_W);
        let nh = (pd.start_h * scale).clamp(PINCH_MIN_H, PINCH_MAX_H);
        resize_about_centre(&pd.addr, nw.round() as i32, nh.round() as i32, pd.cx, pd.cy);
    }

    /// Pinch ended: drop the state.
    pub(crate) fn pinch_end(&mut self) {
        if let Some(pd) = self.pinch_drag.take() {
            self.card_window_placed(&pd.addr);
        }
    }
}

/// Resize a window toward `w`×`h` and keep its centre at `cx,cy`, in one socket
/// round-trip: resize, read the size the window actually GOT, then move so that
/// size sits on the centre. The window may refuse the size — a floating
/// window keeps its own min/max (Golem's hyprland-floating-resize-limits patch;
/// YouTube in Seam wants >= 856 px wide) — and centring the ASKED size then
/// shoved it sideways every step (Max, 2026-09-30: "they shake, move to other
/// sides"; unpatched, the compositor threw it off-screen).
fn resize_about_centre(addr: &str, w: i32, h: i32, cx: f64, cy: f64) {
    hypr::dispatch(&format!(
        "(function() \
           local a = \"address:{addr}\" \
           hl.dispatch(hl.dsp.window.resize({{ x = {w}, y = {h}, window = a }})) \
           local win = hl.get_window(a) \
           local sw, sh = {w}, {h} \
           if win and win.size then sw, sh = win.size.x, win.size.y end \
           hl.dispatch(hl.dsp.window.move({{ x = math.floor({cx} - sw / 2 + 0.5), y = math.floor({cy} - sh / 2 + 0.5), window = a }})) \
           return hl.dsp.no_op() end)()"
    ));
}

#[cfg(test)]
mod fling_tests {
    use super::*;

    fn run(steps: &[(u64, f64, f64)]) -> Fling {
        let t0 = Instant::now();
        let mut f = None;
        for &(ms, dx, dy) in steps {
            f = Some(Fling::step(f, t0 + Duration::from_millis(ms), dx, dy));
        }
        f.expect("at least one step")
    }

    #[test]
    fn a_flick_is_a_throw_and_a_slow_or_wavering_scroll_is_not() {
        assert!(run(&[(0, 20.0, 0.0), (8, 25.0, 2.0), (16, 20.0, 1.0)]).thrown(), "brief, far, one way");
        assert!(run(&[(0, 0.0, -30.0), (10, 0.0, -30.0)]).thrown(), "any direction");
        assert!(!run(&[(0, 10.0, 0.0), (12, 5.0, 0.0)]).thrown(), "too short a way");
        assert!(!run(&[(0, 20.0, 0.0), (100, 20.0, 0.0), (200, 20.0, 0.0)]).thrown(), "too long: a drag");
        assert!(!run(&[(0, 30.0, 0.0), (10, -30.0, 0.0), (20, 30.0, 0.0)]).thrown(), "turned back");
    }

    #[test]
    fn a_pause_starts_a_new_run() {
        let f = run(&[(0, 30.0, 0.0), (10, 30.0, 0.0), (200, 10.0, 0.0)]);
        assert!((f.sx - 10.0).abs() < f64::EPSILON, "the old run is gone");
        assert!(!f.thrown());
    }
}

#[cfg(test)]
mod click_tests {
    use super::click_move;

    #[test]
    fn a_click_centres_then_returns_and_keeps_the_place() {
        let (centre, there) = ((500, 300), (40, 60));
        // First click: to the centre, remembering where it stood.
        assert_eq!(click_move(there, centre, None), (Some(centre), Some(there)));
        // Second click (it settled a pixel off the centre): back, place kept.
        assert_eq!(click_move((501, 299), centre, Some(there)), (Some(there), Some(there)));
        // Third: centred again from the same place; fourth: back again.
        assert_eq!(click_move(there, centre, Some(there)), (Some(centre), Some(there)));
        assert_eq!(click_move(centre, centre, Some(there)), (Some(there), Some(there)));
    }

    #[test]
    fn a_window_moved_by_hand_has_a_new_place_to_return_to() {
        let (centre, old, new) = ((500, 300), (40, 60), (900, 120));
        assert_eq!(click_move(new, centre, Some(old)), (Some(centre), Some(new)));
    }

    #[test]
    fn a_centred_window_with_no_place_remembered_stays() {
        assert_eq!(click_move((500, 300), (500, 300), None), (None, None));
    }
}

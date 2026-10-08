//! WHERE the card is on its window and how it gets there: its box, the
//! pace it travels at, and what makes a scroll a throw.

use serde::{Deserialize, Serialize};

use crate::content::Rect;
use crate::hypr::WindowSpot;

/// The card's width, and how far it sits in from its window's top, right
/// and bottom edges (the mockup's 10). The width was the mockup's 320 until
/// Max sized a card by hand and kept it (2026-10-08: *"i want the size the
/// card is now on this window to be the default size of the card"* — 472;
/// then *"is kind of big, lets make it 420px"*).
pub(crate) const WIDTH: f32 = 420.0;

/// The card's width can be changed by its side edges (Max, 2026-10-08: *"i
/// want to resize the card width"*): a press within [`GRIP`] of either side
/// takes that edge, between these limits (the first mockup's were 240–520).
/// The width is the WINDOW's, like the place the card was slid to: each
/// window has its own, kept until that window closes.
pub(super) const WIDTH_MIN: f32 = 240.0;

pub(super) const WIDTH_MAX: f32 = 640.0;

pub(super) const GRIP: f32 = 7.0;

pub(super) const INSET: f32 = 10.0;

/// How far past either side of its window the card may be slid.
pub(super) const OVERHANG: f32 = 12.0;

/// How far the card slides per unit of scroll on its window's title bar.
pub(super) const SLIDE_PER_SCROLL: f32 = 2.5;

/// A THROW: a short, fast scroll that ends sends the card all the way to
/// that side, outside its window (Max, 2026-10-08: *"a fast scroll sends the
/// card to the end… just with a short fast scroll on the bar or on the
/// card"*). It is judged when the scroll STOPS, by how brief it was — a flick
/// is over in a moment, while moving the card fast by hand is a longer scroll
/// however quick. (The first cut threw on speed alone, mid-scroll: *"too hard
/// now to move without throw it"*, then *"i want to be able to move the card
/// fast without throwing it"*.) So: the whole scroll lasted no longer than
/// [`FLING_BRIEF`], covered at least [`FLING_SCROLL`], one way, and then went
/// quiet for [`FLING_QUIET`].
pub(super) const FLING_SCROLL: f32 = 40.0;

pub(super) const FLING_BRIEF: std::time::Duration = std::time::Duration::from_millis(170);

// 70 at first: the card slid with the flick, then stood still for that long
// before it took off — it read as getting stuck half way (Max, 2026-10-08:
// *"it feels like it gets stuck (slower) on the middle of the window"*). Now
// as short as the gaps between scroll steps allow, and no wait at all where
// the touchpad says the fingers lifted (`AxisStop`, over the card).
pub(super) const FLING_QUIET: std::time::Duration = std::time::Duration::from_millis(30);

/// HOW THE CARD TRAVELS sideways — every way it is moved by scroll, and a
/// throw: it does not jump to where the scroll says, it GOES there, never
/// faster than [`TRAVEL_SPEED`] and never gaining speed faster than
/// [`TRAVEL_ACCEL`] (logical px/s and px/s²). A slow scroll it simply keeps
/// up with. A flick it falls behind, and the throw that follows carries on
/// at the same pace to the end — one motion, where the first cut slid at the
/// fingers' speed and then switched to a glide of its own (Max, 2026-10-08:
/// *"the sliding is not constant, like the first slide is fast because it
/// catch the speed of my fingers… maybe setting a max acceleration"*).
/// `card rate <speed> [accel]` changes both on the running dock.
// 7000 / 50000 at first; then a higher top speed reached about as gently
// (Max: *"make it faster but not snappy"*) — the trip is shorter, its start
// and its landing are not sharper.
pub(super) const TRAVEL_SPEED: f32 = 12000.0;

pub(super) const TRAVEL_ACCEL: f32 = 60000.0;

/// How it settles: its speed is at most this many times the distance left
/// (per second), so the last stretch eases in — and never less than
/// [`TRAVEL_CREEP`], so the ease has an end.
pub(super) const TRAVEL_BRAKE: f32 = 28.0;

pub(super) const TRAVEL_CREEP: f32 = 40.0;

/// One run of sliding scroll: when it began, when its last step came, what
/// it adds up to, and whether it ever turned back.
#[derive(Debug, Clone, Copy)]
pub(super) struct Swipe {
    pub began: std::time::Instant,
    pub last: std::time::Instant,
    pub sum: f32,
    pub turned: bool,
}

impl Swipe {
    /// The run after one more step of `delta` at `now`: the same run if the
    /// step came soon enough after the last, a new one otherwise.
    pub(super) fn step(run: Option<Swipe>, now: std::time::Instant, delta: f32) -> Swipe {
        match run {
            Some(sw) if now - sw.last <= FLING_QUIET => Swipe {
                last: now,
                sum: sw.sum + delta,
                turned: sw.turned || sw.sum * delta < 0.0,
                ..sw
            },
            _ => Swipe {
                began: now,
                last: now,
                sum: delta,
                turned: false,
            },
        }
    }

    /// Whether, now that it has stopped, it was a throw — and which way
    /// (`true`: the card goes left, as scroll down/right slides it).
    pub(super) fn thrown(&self) -> Option<bool> {
        (!self.turned && self.last - self.began <= FLING_BRIEF && self.sum.abs() >= FLING_SCROLL)
            .then_some(self.sum > 0.0)
    }
}

/// Where the card was put on a window: how far from the SIDE it is nearer
/// to. Kept that way and not as a share of the window's width, so a card
/// resting 10 px in from the right — or thrown out past the left edge — is
/// still exactly there when the window is made wider or narrower (as a
/// share of the width, a card outside on the left crept back over the
/// window's edge when the window shrank).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) enum Place {
    /// The card's left edge, from the window's left edge.
    FromLeft(f32),
    /// The window's right edge, from the card's right edge.
    FromRight(f32),
}

impl Place {
    /// Where a card that was never moved is: inside, on the right.
    pub(super) const REST: Place = Place::FromRight(INSET);

    /// The card's left edge from the window's, on a window `window_w` wide
    /// with the card `width` wide.
    pub(super) fn left(self, window_w: f32, width: f32) -> f32 {
        match self {
            Place::FromLeft(d) => d,
            Place::FromRight(d) => window_w - d - width,
        }
    }

    /// The place of a card whose left edge is `left` from the window's: by
    /// the side its middle is nearer to.
    pub(super) fn of(left: f32, window_w: f32, width: f32) -> Place {
        if left + width / 2.0 >= window_w / 2.0 {
            Place::FromRight(window_w - left - width)
        } else {
            Place::FromLeft(left)
        }
    }

    /// Where a thrown card comes to rest: OUTSIDE its window, beside it
    /// with a little air, on the left (`to_left`) or the right — as far as
    /// a slide can take it (Max, 2026-10-08: *"i meant to the end outside
    /// the window"*; the first cut stopped at the inside edges).
    pub(super) fn thrown(to_left: bool, width: f32) -> Place {
        if to_left {
            Place::FromLeft(-(width + OVERHANG))
        } else {
            Place::FromRight(-(width + OVERHANG))
        }
    }
}

/// The card's box on a window at `spot`: under the bar, its left edge `left`
/// from the window's (kept within how far a slide may go), `width` wide.
///
/// Its edges land on whole pixels OF THE SCREEN (`scale` physical per
/// logical), not on whole logical ones: at Golem's 1.6× a card travelling
/// sideways stepped 1.6, 1.6, 3.2 pixels where it now steps evenly, and
/// that unevenness was most of what made a slide look rough (Max,
/// 2026-10-08: *"see if you can make the sliding smoother"*).
pub(crate) fn card_rect(spot: &WindowSpot, left: f32, scale: f32, width: f32) -> Rect {
    let left = clamp_left(left, spot.w, width);
    let scale = if scale > 0.0 { scale } else { 1.0 };
    let snap = |v: f32| (v * scale).round() / scale;
    Rect::new(
        snap(spot.x + left),
        snap(spot.y + INSET),
        snap(width),
        snap(spot.h - 2.0 * INSET),
    )
}

/// One frame of the card's travel from `at` toward `to`. Its speed gains at
/// most `accel`, never passes `top`, and comes down as the place nears —
/// [`TRAVEL_BRAKE`] times the distance left — so it sets off and settles
/// without a jolt, and a scroll it is following (a place that keeps moving a
/// little ahead of it) is one even motion, not a string of starts and stops.
/// It arrives exactly. Returns where it is and its speed after `dt`.
pub(crate) fn travel(at: f32, to: f32, speed: f32, dt: f32, top: f32, accel: f32) -> (f32, f32) {
    let left = to - at;
    if left.abs() < 0.3 {
        return (to, 0.0);
    }
    let want = (left.abs() * TRAVEL_BRAKE).clamp(TRAVEL_CREEP, top.max(TRAVEL_CREEP));
    let speed = if want > speed {
        (speed + accel * dt).min(want)
    } else {
        want
    };
    let step = speed * dt;
    if step >= left.abs() {
        (to, 0.0)
    } else {
        (at + step * left.signum(), speed)
    }
}

/// How far the card's left edge may go from its window's: fully out on
/// the left with a little air, to fully out on the right.
pub(crate) fn clamp_left(left: f32, window_w: f32, width: f32) -> f32 {
    left.clamp(-(width + OVERHANG), window_w + OVERHANG)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn spot() -> WindowSpot {
        WindowSpot {
            x: 100.0,
            y: 50.0,
            w: 900.0,
            h: 600.0,
            visible: true,
        }
    }

    #[test]
    fn the_card_rests_inside_its_window_on_the_right() {
        let s = spot();
        let r = card_rect(&s, Place::REST.left(s.w, WIDTH), 1.0, WIDTH);
        assert_eq!(
            (r.x, r.y, r.w, r.h),
            (100.0 + 900.0 - 10.0 - WIDTH, 60.0, WIDTH, 580.0)
        );
        // However far it is asked to go, no further than a slide may.
        assert_eq!(card_rect(&s, -9999.0, 1.0, WIDTH).x, 100.0 - WIDTH - 12.0);
        assert_eq!(card_rect(&s, 9999.0, 1.0, WIDTH).x, 100.0 + 900.0 + 12.0);
    }

    #[test]
    fn a_place_is_kept_from_the_nearer_side_and_survives_a_resize() {
        // Left half: from the left edge. Right half: from the right edge.
        assert_eq!(Place::of(40.0, 900.0, 400.0), Place::FromLeft(40.0));
        assert_eq!(Place::of(480.0, 900.0, 400.0), Place::FromRight(20.0));
        // The window grows or shrinks: each keeps its distance to its side.
        assert_eq!(Place::FromLeft(40.0).left(1400.0, 400.0), 40.0);
        assert_eq!(
            Place::FromRight(20.0).left(1400.0, 400.0),
            1400.0 - 20.0 - 400.0
        );
        assert_eq!(Place::FromRight(20.0).left(600.0, 400.0), 180.0);
        // And it reads back as it was written.
        for left in [-412.0, 0.0, 250.0, 912.0] {
            assert_eq!(Place::of(left, 900.0, 400.0).left(900.0, 400.0), left);
        }
    }

    #[test]
    fn a_thrown_card_rests_beside_its_window_whatever_the_window_becomes() {
        let (out_left, out_right) = (Place::thrown(true, 400.0), Place::thrown(false, 400.0));
        for w in [600.0, 900.0, 2000.0] {
            // Exactly as far as a slide by hand can take it — at any width
            // (as a share of the width, the left one crept back in).
            assert_eq!(out_left.left(w, 400.0), clamp_left(-9999.0, w, 400.0));
            assert_eq!(out_right.left(w, 400.0), clamp_left(9999.0, w, 400.0));
        }
    }

    #[test]
    fn the_card_travels_at_a_capped_pace_eases_in_and_arrives_exactly() {
        // From rest it gains speed, no faster than the acceleration allows.
        let (at, speed) = travel(0.0, 5000.0, 0.0, 0.01, 6000.0, 50_000.0);
        assert_eq!(speed, 500.0);
        assert_eq!(at, 5.0);
        // At full pace it never goes faster, however far the place is.
        let (at, speed) = travel(0.0, 100_000.0, 6000.0, 0.01, 6000.0, 50_000.0);
        assert_eq!((at, speed), (60.0, 6000.0));
        // Near its place it slows: its speed is tied to the distance left.
        let (_, speed) = travel(0.0, 50.0, 6000.0, 0.001, 6000.0, 50_000.0);
        assert_eq!(speed, 50.0 * TRAVEL_BRAKE);
        // …but never to nothing, and it stops dead on arrival, either way.
        let (_, speed) = travel(0.0, 0.5, 0.0, 0.001, 6000.0, 1e9);
        assert_eq!(speed, TRAVEL_CREEP);
        assert_eq!(travel(0.2, 0.0, 6000.0, 0.01, 6000.0, 50_000.0), (0.0, 0.0));
        let (at, _) = travel(500.0, 0.0, 6000.0, 0.01, 6000.0, 50_000.0);
        assert!(at < 500.0 && at > 0.0);
    }

    #[test]
    fn the_cards_edges_land_on_whole_screen_pixels() {
        let s = WindowSpot {
            x: 100.3,
            y: 50.0,
            w: 900.0,
            h: 600.0,
            visible: true,
        };
        let r = card_rect(&s, 450.0, 1.6, WIDTH);
        assert!(((r.x * 1.6) - (r.x * 1.6).round()).abs() < 1e-3);
        // At 1× that is the whole logical pixel, as before.
        assert_eq!(card_rect(&s, 450.0, 1.0, WIDTH).x, 550.0);
    }

    #[test]
    fn a_run_of_scroll_is_one_run_until_it_pauses() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let a = Swipe::step(None, t0, 10.0);
        let b = Swipe::step(Some(a), t0 + ms(10), 15.0);
        assert_eq!((b.sum, b.began, b.turned), (25.0, t0, false));
        // Turning back is remembered.
        assert!(Swipe::step(Some(b), t0 + ms(20), -5.0).turned);
        // A pause: a new run.
        let c = Swipe::step(Some(b), t0 + ms(400), 7.0);
        assert_eq!((c.sum, c.began), (7.0, t0 + ms(400)));
    }

    #[test]
    fn only_a_brief_one_way_scroll_is_a_throw() {
        let t0 = std::time::Instant::now();
        let ms = std::time::Duration::from_millis;
        let swipe = |lasted: u64, sum: f32, turned: bool| Swipe {
            began: t0,
            last: t0 + ms(lasted),
            sum,
            turned,
        };
        // A flick: over in a moment.
        assert_eq!(swipe(90, 80.0, false).thrown(), Some(true));
        assert_eq!(swipe(90, -80.0, false).thrown(), Some(false));
        // Moving the card fast by hand: much more scroll, but it lasts.
        assert_eq!(swipe(600, 900.0, false).thrown(), None);
        // A small nudge, and a scroll that turned back.
        assert_eq!(swipe(60, 12.0, false).thrown(), None);
        assert_eq!(swipe(90, 80.0, true).thrown(), None);
    }
}

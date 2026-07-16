//! One horizontal pager — the scroll/paging state machine shared by
//! every popup section (Apps / Install / Files) and the open box.
//!
//! A pager owns a pixel position easing toward a page-aligned target,
//! the wheel accumulator that decides when a scroll gesture turns a
//! page, and the page-turn logic itself (cyclic wrap for wheel paging,
//! clamped at the ends for drag paging). Sections and the box used to
//! carry hand-rolled copies of all three; one implementation means the
//! grid and the box can't drift apart in feel.

use std::time::{Duration, Instant};

/// Accumulated scroll needed to turn one page (≈ two wheel notches).
const PAGE_SCROLL_THRESHOLD: f64 = 30.0;

/// Minimum time between wheel page turns, so a fast flick moves exactly
/// one page instead of spinning the (cyclic) pager.
pub const PAGE_COOLDOWN: Duration = Duration::from_millis(250);

/// Exponential ease rate of the page slide (~200 ms to settle at 60 fps).
const SLIDE_RATE: f32 = 12.0;

/// Horizontal paging state: a visual position easing toward a
/// page-aligned target, plus the wheel-turn accumulator.
#[derive(Default)]
pub struct Pager {
    /// Scroll offset in pixels (visual, lags behind `target`).
    pub pos: f32,
    /// Scroll animation target; `pos` eases toward this each frame.
    pub target: f32,
    /// Accumulated scroll toward the next page turn (resets on direction
    /// change and after each turn).
    page_accum: f64,
    /// When the last page turn happened, for [`PAGE_COOLDOWN`].
    page_turned_at: Option<Instant>,
}

impl Pager {
    /// Back to page 0, accumulators cleared.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Ease `pos` toward `target` (exponential decay, dt-based). Returns
    /// true while still moving — the caller keeps frames coming.
    pub fn ease(&mut self, dt: f32) -> bool {
        let (pos, moving) =
            crate::animation::ease_toward(self.pos, self.target, dt, SLIDE_RATE, 0.5);
        self.pos = pos;
        moving
    }

    /// Whether `pos` still has distance to cover toward `target`.
    pub fn is_settling(&self) -> bool {
        (self.target - self.pos).abs() > 0.5
    }

    /// Accumulate wheel scroll toward a page turn: turning requires
    /// [`PAGE_SCROLL_THRESHOLD`] worth of travel, successive turns are
    /// at least [`PAGE_COOLDOWN`] apart, and a direction change discards
    /// progress. Returns the direction when a turn is due.
    pub fn wheel(&mut self, value: f64) -> Option<i64> {
        if self
            .page_turned_at
            .is_some_and(|t| t.elapsed() < PAGE_COOLDOWN)
        {
            return None;
        }
        if value * self.page_accum < 0.0 {
            self.page_accum = 0.0;
        }
        self.page_accum += value;
        if self.page_accum.abs() >= PAGE_SCROLL_THRESHOLD {
            let dir = if self.page_accum > 0.0 { 1 } else { -1 };
            self.page_accum = 0.0;
            self.page_turned_at = Some(Instant::now());
            Some(dir)
        } else {
            None
        }
    }

    /// Slide one page in `dir` (+1 next, -1 previous). `wrap` cycles
    /// past either end (infinite scroll) by shifting the visual position
    /// one full strip so the slide still moves in the gesture direction —
    /// rendering is cyclic, so the shift is invisible; without `wrap`
    /// the pager stops at the ends. Returns whether the target moved.
    pub fn turn(&mut self, dir: i64, wrap: bool, n_pages: usize, page_w: f32) -> bool {
        if n_pages <= 1 {
            return false;
        }
        let total = n_pages as f32 * page_w;
        // Use the target (intended page) not the animated position so
        // mid-animation events don't mis-compute the page.
        let current = (self.target / page_w).round() as i64;
        let next = current + dir;
        if next < 0 {
            if !wrap {
                return false;
            }
            self.pos += total;
            self.target = (n_pages - 1) as f32 * page_w;
        } else if next >= n_pages as i64 {
            if !wrap {
                return false;
            }
            self.pos -= total;
            self.target = 0.0;
        } else {
            self.target = next as f32 * page_w;
        }
        true
    }

    /// The page index the pager is headed to.
    pub fn page(&self, page_w: f32) -> usize {
        (self.target / page_w.max(1.0)).round().max(0.0) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_wraps_or_clamps() {
        let mut p = Pager::default();
        // Clamped at the first page.
        assert!(!p.turn(-1, false, 3, 100.0));
        assert_eq!(p.page(100.0), 0);
        // Wrap backward jumps to the last page (with the strip shift).
        assert!(p.turn(-1, true, 3, 100.0));
        assert_eq!(p.page(100.0), 2);
        assert_eq!(p.pos, 300.0);
        // Forward from the last page: clamped without wrap...
        assert!(!p.turn(1, false, 3, 100.0));
        // ...wraps to page 0 with it.
        assert!(p.turn(1, true, 3, 100.0));
        assert_eq!(p.page(100.0), 0);
        // A single page never turns.
        assert!(!p.turn(1, true, 1, 100.0));
    }

    #[test]
    fn wheel_thresholds_and_direction_reset() {
        let mut p = Pager::default();
        assert_eq!(p.wheel(15.0), None);
        // Direction change discards the old progress.
        assert_eq!(p.wheel(-15.0), None);
        assert_eq!(p.wheel(-16.0), Some(-1));
        // Cooldown eats the immediate follow-up.
        assert_eq!(p.wheel(-40.0), None);
    }

    #[test]
    fn ease_settles() {
        let mut p = Pager {
            target: 100.0,
            ..Default::default()
        };
        assert!(p.ease(1.0 / 60.0));
        assert!(p.pos > 0.0 && p.pos < 100.0);
        for _ in 0..300 {
            p.ease(1.0 / 60.0);
        }
        assert_eq!(p.pos, 100.0);
        assert!(!p.is_settling());
    }
}

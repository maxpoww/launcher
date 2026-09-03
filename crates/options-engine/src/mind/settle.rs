//! Don't put anything on the bar that isn't staying.
//!
//! The decision itself ([`super::decide`]) is a pure function of one context
//! snapshot, and the context moves constantly: Hyprland is configured
//! `follow_mouse = 2`, so the pointer merely crossing a terminal on its way
//! somewhere else re-focuses it, and the whole OptionSet is rewritten for as
//! long as the pointer is over it. Watching that live on the 2013 Air, the bar
//! alternated between a git set and a media set every one to three seconds
//! while Max just watched a video.
//!
//! A suggestion that appears and vanishes before you can read it is worse than
//! no suggestion: it is motion in the corner of the eye that teaches you to
//! ignore the bar. So an offer must be continuously on the decision for
//! [`APPEAR_DWELL`] before it earns a pill.
//!
//! What this deliberately does NOT do is hold an offer open after the decision
//! drops it. A control that is still on the bar after its context is gone is a
//! control that does the wrong thing when clicked — "Commit all" for a repo you
//! have left. Slow to appear, immediate to leave.
//!
//! Warnings skip the wait entirely. "Your camera is on" is not a suggestion.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::{AffordanceKind, OptionSet};

/// How long an offer must stay on the decision before it may take a pill.
/// Long enough that a pointer crossing a window cannot rewrite the bar, short
/// enough that a deliberate act (plugging in a drive, starting a screen share)
/// still feels immediate.
const APPEAR_DWELL: Duration = Duration::from_millis(1200);

/// Per-offer arrival time, keyed by the affordance's static id.
#[derive(Default)]
pub(crate) struct Settle {
    /// When each currently-offered id first appeared on an unbroken run of
    /// decisions. Dropping off the decision forgets it, so a flapping offer
    /// restarts its wait every time and never surfaces.
    since: HashMap<&'static str, Instant>,
}

impl Settle {
    /// Filter `set` down to the offers that have earned a place, updating the
    /// arrival bookkeeping. Ranking and order are preserved.
    pub(crate) fn apply(&mut self, mut set: OptionSet, now: Instant) -> OptionSet {
        let mut fresh: HashMap<&'static str, Instant> = HashMap::with_capacity(set.items.len());
        for a in &set.items {
            let since = self.since.get(a.id).copied().unwrap_or(now);
            fresh.insert(a.id, since);
        }
        self.since = fresh;
        let since = &self.since;
        set.items.retain(|a| {
            // Safety is never made to wait its turn.
            if a.kind == AffordanceKind::Warning {
                return true;
            }
            since
                .get(a.id)
                .is_some_and(|t| now.duration_since(*t) >= APPEAR_DWELL)
        });
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mind::{Affordance, AffordanceAction, OptionSet};
    use crate::state::Layer;

    fn offer(id: &'static str, kind: AffordanceKind) -> Affordance {
        Affordance {
            id,
            kind,
            title: id.into(),
            detail: String::new(),
            relevance: 0.5,
            reason: "test",
            source: Layer::Compositor,
            action: AffordanceAction::None,
        }
    }

    fn set(items: Vec<Affordance>) -> OptionSet {
        OptionSet {
            items,
            ..Default::default()
        }
    }

    /// A control has to stay put before it is shown; a warning never waits.
    #[test]
    fn controls_wait_out_the_dwell_and_warnings_do_not() {
        let mut s = Settle::default();
        let t0 = Instant::now();
        let out = s.apply(
            set(vec![
                offer("git.commit", AffordanceKind::Control),
                offer("compositor.camera", AffordanceKind::Warning),
            ]),
            t0,
        );
        assert_eq!(
            out.items.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec!["compositor.camera"],
            "the warning shows at once, the control waits"
        );
        // Still inside the dwell.
        let out = s.apply(
            set(vec![offer("git.commit", AffordanceKind::Control)]),
            t0 + APPEAR_DWELL - Duration::from_millis(1),
        );
        assert!(out.items.is_empty());
        // Past it, on an unbroken run → it earns its pill.
        let out = s.apply(
            set(vec![offer("git.commit", AffordanceKind::Control)]),
            t0 + APPEAR_DWELL,
        );
        assert_eq!(out.items.len(), 1);
    }

    /// The pointer-graze case: an offer that keeps coming and going never gets
    /// to flash on the bar, because each disappearance restarts its wait.
    #[test]
    fn a_flapping_offer_never_surfaces() {
        let mut s = Settle::default();
        let t0 = Instant::now();
        let mut t = t0;
        for _ in 0..5 {
            // Present for 800 ms…
            for step in [0, 400, 800] {
                let out = s.apply(
                    set(vec![offer("git.commit", AffordanceKind::Control)]),
                    t + Duration::from_millis(step),
                );
                assert!(out.items.is_empty(), "never long enough to show");
            }
            // …then gone, which forgets it.
            t += Duration::from_millis(1000);
            let _ = s.apply(set(vec![]), t);
            t += Duration::from_millis(500);
        }
    }

    /// Leaving is immediate: an offer the decision drops is gone from the same
    /// output, never held over on a stale context.
    #[test]
    fn dropping_off_the_decision_removes_it_at_once() {
        let mut s = Settle::default();
        let t0 = Instant::now();
        s.apply(set(vec![offer("git.commit", AffordanceKind::Control)]), t0);
        let shown = s.apply(
            set(vec![offer("git.commit", AffordanceKind::Control)]),
            t0 + APPEAR_DWELL,
        );
        assert_eq!(shown.items.len(), 1);
        let gone = s.apply(set(vec![]), t0 + APPEAR_DWELL + Duration::from_millis(10));
        assert!(gone.items.is_empty());
    }
}

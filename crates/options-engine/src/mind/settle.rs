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

    /// The earliest instant at which a currently-waiting offer will have sat
    /// out its dwell — the wake-up the mind schedules alongside the context
    /// stream. The decision loop is otherwise purely change-driven, so without
    /// this an offer that arrived just before the context went quiet would
    /// stay hidden past its dwell, waiting on an unrelated event to re-run the
    /// decision. `None` when nothing is waiting (no timer needed).
    pub(crate) fn next_deadline(&self, now: Instant) -> Option<Instant> {
        self.since
            .values()
            .map(|t| *t + APPEAR_DWELL)
            .filter(|d| *d > now)
            .min()
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

    /// Rank churn within an unbroken run must not restart the dwell: the
    /// pointer-graze the settle stage exists for often reorders the surviving
    /// offers rather than dropping them, and a reorder is not a disappearance.
    /// The decision's own order is preserved on the way out.
    #[test]
    fn rank_shuffle_keeps_the_dwell_and_the_decisions_order() {
        let mut s = Settle::default();
        let t0 = Instant::now();
        let a = || offer("git.commit", AffordanceKind::Control);
        let b = || offer("media.playpause", AffordanceKind::Control);
        s.apply(set(vec![a(), b()]), t0);
        // Mid-wait the two swap ranks; both stay on the decision throughout.
        let out = s.apply(set(vec![b(), a()]), t0 + Duration::from_millis(600));
        assert!(out.items.is_empty(), "still inside the dwell");
        let out = s.apply(set(vec![b(), a()]), t0 + APPEAR_DWELL);
        assert_eq!(
            out.items.iter().map(|x| x.id).collect::<Vec<_>>(),
            vec!["media.playpause", "git.commit"],
            "both earned their pill from the ORIGINAL arrival; order is the decision's"
        );
    }

    /// The pill-cap interaction: an offer squeezed out of the decision's capped
    /// set (rank 6 of a 5-pill bar) is indistinguishable from one whose context
    /// vanished — its wait restarts when it re-enters. Slow to appear cuts both
    /// ways: rank-flapping around the cap boundary can never flash a pill.
    #[test]
    fn getting_capped_out_of_the_set_restarts_the_wait() {
        let mut s = Settle::default();
        let t0 = Instant::now();
        let a = || offer("git.commit", AffordanceKind::Control);
        s.apply(set(vec![a()]), t0);
        // A higher-ranked crowd pushes it past the cap upstream: it is simply
        // absent from the next decision the settle stage sees.
        s.apply(set(vec![]), t0 + Duration::from_millis(600));
        // Back in the set: the old arrival time must be forgotten…
        let out = s.apply(set(vec![a()]), t0 + APPEAR_DWELL);
        assert!(out.items.is_empty(), "the interrupted wait does not resume");
        // …and a fresh unbroken dwell earns the pill.
        let out = s.apply(set(vec![a()]), t0 + APPEAR_DWELL + APPEAR_DWELL);
        assert_eq!(out.items.len(), 1);
    }

    /// The wake-up the mind schedules: `next_deadline` names the earliest
    /// pending offer's dwell expiry, and goes quiet once nothing is waiting.
    #[test]
    fn next_deadline_tracks_the_earliest_pending_offer() {
        let mut s = Settle::default();
        let t0 = Instant::now();
        assert_eq!(s.next_deadline(t0), None, "nothing tracked, no timer");
        s.apply(set(vec![offer("git.commit", AffordanceKind::Control)]), t0);
        assert_eq!(s.next_deadline(t0), Some(t0 + APPEAR_DWELL));
        // A later arrival must not move the earliest deadline…
        let t1 = t0 + Duration::from_millis(300);
        s.apply(
            set(vec![
                offer("git.commit", AffordanceKind::Control),
                offer("media.playpause", AffordanceKind::Control),
            ]),
            t1,
        );
        assert_eq!(s.next_deadline(t1), Some(t0 + APPEAR_DWELL));
        // …and once the first has surfaced, the second's expiry is next.
        let t2 = t0 + APPEAR_DWELL;
        assert_eq!(s.next_deadline(t2), Some(t1 + APPEAR_DWELL));
        // Everything surfaced: no timer at all.
        let t3 = t1 + APPEAR_DWELL;
        assert_eq!(s.next_deadline(t3), None);
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

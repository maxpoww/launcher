//! Usage-aware focus cycling — the ranking brain behind the current-task
//! pill's click (and, via the `focus-next` ctl verb, any future Super+Tab
//! bind).
//!
//! Every real focus change earns the window a point in a **frecency** score
//! (frequency × recency: points decay exponentially, half-life
//! [`HALF_LIFE`]), so the windows the user actually bounces between rank
//! highest, while a one-off glance at a window decays away — the two-window
//! workflow stays one click apart even after digressions (Max's 4-window
//! scenario, 2026-08-31).
//!
//! A click starts a **walk**: a frozen snapshot of the scope's windows,
//! ranked by frecency (ties broken by the compositor's own focus history),
//! with the starting window appended last so the cycle wraps home.
//! Consecutive clicks within [`WALK_CONTINUE`] advance the walk — the
//! freeze is what makes the lesser-used windows reachable at all (a live
//! re-rank would bounce between the top two forever). When the walk
//! expires, the window the user LANDED on earns the point; the windows
//! passed through never do (pill-driven focus changes are suppressed in
//! the stats hook).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::{hypr, App};

/// Frecency half-life: a focus point loses half its weight this often.
const HALF_LIFE: Duration = Duration::from_secs(600);
/// Clicks this close together continue the current walk; a later click
/// starts a fresh one (and commits the previous landing).
const WALK_CONTINUE: Duration = Duration::from_secs(3);

/// Decaying focus-frequency scores, keyed by window address.
#[derive(Default)]
pub(crate) struct Frecency {
    scores: HashMap<String, (f32, Instant)>,
}

impl Frecency {
    /// The address's score decayed to `now`.
    fn score(&self, addr: &str, now: Instant) -> f32 {
        self.scores
            .get(addr)
            .map(|&(s, at)| s * decay_factor(now.duration_since(at)))
            .unwrap_or(0.0)
    }

    /// Award one focus point to `addr` (a real focus change, or a walk's
    /// final landing).
    pub(crate) fn note(&mut self, addr: &str) {
        let now = Instant::now();
        let prev = self.score(addr, now);
        self.scores.insert(addr.to_owned(), (prev + 1.0, now));
        // Opportunistic prune: fully-decayed entries (and long-gone
        // windows with them) drop out instead of accumulating forever.
        self.scores
            .retain(|_, &mut (s, at)| s * decay_factor(now.duration_since(at)) > 0.01);
    }
}

/// Exponential decay with [`HALF_LIFE`].
fn decay_factor(elapsed: Duration) -> f32 {
    0.5f32.powf(elapsed.as_secs_f32() / HALF_LIFE.as_secs_f32())
}

/// An in-flight cycle: the frozen, ranked window order and where we are.
pub(crate) struct FocusWalk {
    /// Ranked addresses (frecency desc, focus-history tiebreak), the
    /// starting window last so the wrap returns home.
    order: Vec<String>,
    /// Index of the last window focused by the walk.
    idx: usize,
    /// Whether this walk cycles the current workspace (left click) or the
    /// other workspaces (right click) — a click of the other kind starts
    /// fresh rather than continuing.
    same_ws: bool,
    last_click: Instant,
}

impl FocusWalk {
    /// The address the walk currently has focused.
    fn landed(&self) -> &str {
        &self.order[self.idx % self.order.len()]
    }
}

/// One window's ranking inputs, read from `hyprctl clients`.
struct Candidate {
    addr: String,
    score: f32,
    /// Compositor focus recency (0 = focused) — the tiebreak, and the
    /// whole order for windows with no frecency yet.
    history: i32,
}

/// Rank candidates: frecency descending, then compositor focus history
/// (most recent first). Pure, for testability.
fn rank(mut cands: Vec<Candidate>) -> Vec<String> {
    cands.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.history.cmp(&b.history))
    });
    cands.into_iter().map(|c| c.addr).collect()
}

impl App {
    /// A real (non-walk) focus change: award the frecency point — unless
    /// this is the change our own walk dispatch caused. Also commits a
    /// walk that expired naturally: its landing earns the point now.
    pub(crate) fn note_focus_change(&mut self, addr: &str) {
        if self
            .walk_focus_pending
            .as_deref()
            .is_some_and(|p| p == addr)
        {
            // Our own dispatch landing — not user signal (the walk's final
            // landing is awarded at commit instead).
            self.walk_focus_pending = None;
            return;
        }
        self.commit_expired_walk();
        // A focus change by hand (window click, keybind) supersedes any
        // still-running walk: commit its landing and start clean.
        if let Some(walk) = self.focus_walk.take() {
            self.frecency.note(walk.landed());
        }
        self.frecency.note(addr);
    }

    /// If a walk has expired (no continuation click), its landing was the
    /// user's choice: award the point and clear it.
    fn commit_expired_walk(&mut self) {
        if self
            .focus_walk
            .as_ref()
            .is_some_and(|w| w.last_click.elapsed() >= WALK_CONTINUE)
        {
            let walk = self.focus_walk.take().expect("checked above");
            self.frecency.note(walk.landed());
        }
    }

    /// Cycle focus: `same_ws` = walk the current workspace's windows
    /// (left click / focus-next), else the other workspaces' (right
    /// click). Consecutive calls continue the frozen walk; a fresh call
    /// snapshots and ranks anew.
    pub(crate) fn cycle_focus(&mut self, same_ws: bool) {
        self.commit_expired_walk();
        // Continue an in-flight walk of the same kind.
        if let Some(walk) = self
            .focus_walk
            .as_mut()
            .filter(|w| w.same_ws == same_ws && w.last_click.elapsed() < WALK_CONTINUE)
        {
            walk.idx += 1;
            walk.last_click = Instant::now();
            let target = walk.order[walk.idx % walk.order.len()].clone();
            self.walk_focus_to(&target);
            return;
        }
        // A different-kind walk mid-flight: its landing was chosen too.
        if let Some(walk) = self.focus_walk.take() {
            self.frecency.note(walk.landed());
        }
        let Some((windows, focused)) = hypr::workspace_windows() else {
            return;
        };
        let now = Instant::now();
        let current_ws = windows
            .iter()
            .find(|w| Some(&w.addr) == focused.as_ref())
            .map(|w| w.workspace);
        let mut cands: Vec<Candidate> = windows
            .iter()
            .filter(|w| Some(&w.addr) != focused.as_ref())
            .filter(|w| w.workspace > 0) // never cycle into a special workspace
            .filter(|w| match (same_ws, current_ws) {
                (true, Some(ws)) => w.workspace == ws,
                (false, Some(ws)) => w.workspace != ws,
                // No focused window (empty workspace): everything counts.
                (_, None) => true,
            })
            .map(|w| Candidate {
                addr: w.addr.clone(),
                score: self.frecency.score(&w.addr, now),
                history: w.history,
            })
            .collect();
        if cands.is_empty() {
            debug!("focus cycle: nothing to cycle to (same_ws={same_ws})");
            return;
        }
        let mut order = rank(std::mem::take(&mut cands));
        // The starting window goes last, so the wrap returns home.
        if same_ws {
            if let Some(home) = focused {
                order.push(home);
            }
        }
        let target = order[0].clone();
        self.focus_walk = Some(FocusWalk {
            order,
            idx: 0,
            same_ws,
            last_click: Instant::now(),
        });
        self.walk_focus_to(&target);
    }

    /// Dispatch focus to `addr`, marking it as walk-driven so the stats
    /// hook doesn't count the hop as user signal.
    fn walk_focus_to(&mut self, addr: &str) {
        self.walk_focus_pending = Some(addr.to_owned());
        if let Err(e) = hypr::focus_window_direct(addr) {
            warn!("focus cycle: cannot focus {addr}: {e:#}");
            self.walk_focus_pending = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(addr: &str, score: f32, history: i32) -> Candidate {
        Candidate {
            addr: addr.to_owned(),
            score,
            history,
        }
    }

    #[test]
    fn rank_orders_by_frecency_then_history() {
        let order = rank(vec![
            cand("junk", 0.2, 1),
            cand("partner", 9.0, 3),
            cand("fresh", 0.0, 0),
            cand("other", 4.0, 2),
        ]);
        // Heavy users first; the never-scored window falls back to the
        // compositor's recency and still beats nothing-with-worse-history.
        assert_eq!(order, vec!["partner", "other", "junk", "fresh"]);
    }

    #[test]
    fn rank_unscored_windows_follow_compositor_history() {
        let order = rank(vec![
            cand("older", 0.0, 5),
            cand("newer", 0.0, 2),
        ]);
        assert_eq!(order, vec!["newer", "older"]);
    }

    #[test]
    fn frecency_decays_and_prunes() {
        let mut f = Frecency::default();
        f.note("a");
        f.note("a");
        f.note("b");
        let now = Instant::now();
        assert!(f.score("a", now) > f.score("b", now));
        // Half-life decay: a two-point score halves per HALF_LIFE.
        let later = now + HALF_LIFE;
        assert!((f.score("a", later) - f.score("a", now) / 2.0).abs() < 0.01);
        // Unknown windows score zero.
        assert_eq!(f.score("ghost", now), 0.0);
    }
}

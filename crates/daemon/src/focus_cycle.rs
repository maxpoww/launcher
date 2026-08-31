//! Interaction-aware focus cycling — the ranking brain behind the
//! current-task pill's click and the `focus-next` / `focus-other` ctl verbs
//! (bound to Super+Tab).
//!
//! The model, from Max's own description (2026-08-31): **interaction
//! commits the cycle**, not a timer.
//!
//! - Toggle from a settled window → the window you last actually WORKED in.
//! - Toggle again without having interacted → you're still searching →
//!   continue down the ranked list (wrapping home at the end).
//! - The moment you interact — a key, a click, a scroll aimed at the
//!   window (waveview watches compositor input and sends `interacted`) —
//!   the walk commits: that window earns its usage point, and the next
//!   toggle starts a fresh walk from rule one.
//!
//! So a window you merely passed through on the way somewhere never earns
//! anything and never becomes the partner: the 1↔2 pair survives a detour
//! through 3 and 4 (which the older focus-only, timer-committed version got
//! wrong — it treated *looking* as *using*).
//!
//! Ranking a fresh walk:
//! 1. the most recently INTERACTED other window (the partner — "take me
//!    back to what I was doing"),
//! 2. then the rest by **frecency** (interaction points decaying with
//!    [`HALF_LIFE`], so habits fade as they should),
//! 3. then never-interacted windows by the compositor's own focus history,
//! 4. and the starting window last, so the cycle wraps home.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::{hypr, App};

/// Frecency half-life: an interaction point loses half its weight this
/// often.
const HALF_LIFE: Duration = Duration::from_secs(600);

/// Decaying interaction scores, keyed by window address. The stored
/// `Instant` doubles as the window's last-interaction time — the partner
/// slot's sort key.
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

    /// When the window was last interacted with, if ever.
    fn last(&self, addr: &str) -> Option<Instant> {
        self.scores.get(addr).map(|&(_, at)| at)
    }

    /// Award one interaction point to `addr`.
    fn note(&mut self, addr: &str) {
        let now = Instant::now();
        let prev = self.score(addr, now);
        self.scores.insert(addr.to_owned(), (prev + 1.0, now));
        // Opportunistic prune: fully-decayed entries (and the long-closed
        // windows with them) drop out instead of accumulating forever.
        self.scores
            .retain(|_, &mut (s, at)| s * decay_factor(now.duration_since(at)) > 0.01);
    }
}

/// Exponential decay with [`HALF_LIFE`].
fn decay_factor(elapsed: Duration) -> f32 {
    0.5f32.powf(elapsed.as_secs_f32() / HALF_LIFE.as_secs_f32())
}

/// An in-flight cycle: the frozen ranked order, and where in it we are.
/// Lives until an interaction commits it (or a manual focus change
/// abandons it) — never a timeout.
pub(crate) struct FocusWalk {
    /// Ranked addresses, the starting window last so the wrap returns home.
    order: Vec<String>,
    /// Index of the window the walk currently has focused.
    idx: usize,
    /// Whether this walk cycles the current workspace (left click /
    /// focus-next) or the other workspaces (right click / focus-other) —
    /// a toggle of the other kind starts fresh.
    same_ws: bool,
}

/// One window's ranking inputs.
struct Candidate {
    addr: String,
    score: f32,
    /// Last interaction, if the window was ever used.
    last: Option<Instant>,
    /// Compositor focus recency (0 = focused) — the fallback order for
    /// windows never interacted with.
    history: i32,
}

/// Rank candidates: the most recently interacted window first (the
/// partner), then frecency descending, then compositor focus history.
/// Pure, for testability.
fn rank(mut cands: Vec<Candidate>) -> Vec<String> {
    cands.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.history.cmp(&b.history))
    });
    // Lift the freshest interaction to the front: "take me back to what I
    // was doing" beats "take me to what I use most" — after a detour into
    // a rarely-used window, one toggle still returns to the real partner.
    if let Some(partner) = cands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.last.map(|t| (i, t)))
        .max_by_key(|&(_, t)| t)
        .map(|(i, _)| i)
    {
        let c = cands.remove(partner);
        cands.insert(0, c);
    }
    cands.into_iter().map(|c| c.addr).collect()
}

impl App {
    /// The user interacted with the focused window (waveview saw a key,
    /// click, or scroll aimed at it — one message per window visit). That
    /// commits any in-flight walk and earns the window its usage point:
    /// only windows actually WORKED IN rank, never ones passed through.
    pub(crate) fn note_interaction(&mut self) {
        let Some((addr, _, _)) = hypr::active_window_info() else {
            return;
        };
        // Reaching the walk's current landing by using it = committed.
        self.focus_walk = None;
        self.walk_focus_pending = None;
        debug!("focus cycle: interaction in {addr}");
        self.frecency.note(&addr);
    }

    /// A focus change the cycle did not cause (a window click, a
    /// directional keybind): abandon any in-flight walk — the user left
    /// the cycle by hand. No points are awarded here; only interaction
    /// earns them.
    pub(crate) fn note_focus_change(&mut self, addr: &str) {
        if self
            .walk_focus_pending
            .as_deref()
            .is_some_and(|p| p == addr)
        {
            self.walk_focus_pending = None; // our own hop — walk continues
            return;
        }
        self.focus_walk = None;
    }

    /// Cycle focus: `same_ws` = walk the current workspace's windows
    /// (left click / focus-next), else the other workspaces' (right click /
    /// focus-other). An uncommitted walk advances; otherwise a fresh walk
    /// is snapshotted and ranked.
    pub(crate) fn cycle_focus(&mut self, same_ws: bool) {
        // Continue an in-flight walk of the same kind: no interaction has
        // committed it, so the user is still searching.
        if let Some(walk) = self.focus_walk.as_mut().filter(|w| w.same_ws == same_ws) {
            walk.idx += 1;
            let target = walk.order[walk.idx % walk.order.len()].clone();
            self.walk_focus_to(&target);
            return;
        }
        let Some((windows, focused)) = hypr::workspace_windows() else {
            return;
        };
        let now = Instant::now();
        let current_ws = windows
            .iter()
            .find(|w| Some(&w.addr) == focused.as_ref())
            .map(|w| w.workspace);
        let cands: Vec<Candidate> = windows
            .iter()
            .filter(|w| Some(&w.addr) != focused.as_ref())
            .filter(|w| w.workspace > 0) // never cycle into a special workspace
            .filter(|w| match (same_ws, current_ws) {
                (true, Some(ws)) => w.workspace == ws,
                (false, Some(ws)) => w.workspace != ws,
                // Nothing focused (empty workspace): everything counts.
                (_, None) => true,
            })
            .map(|w| Candidate {
                addr: w.addr.clone(),
                score: self.frecency.score(&w.addr, now),
                last: self.frecency.last(&w.addr),
                history: w.history,
            })
            .collect();
        if cands.is_empty() {
            debug!("focus cycle: nothing to cycle to (same_ws={same_ws})");
            return;
        }
        let mut order = rank(cands);
        // Home goes last, so continuing past the list wraps back.
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
        });
        self.walk_focus_to(&target);
    }

    /// Dispatch focus to `addr`, marking it as walk-driven so the
    /// focus-change hook doesn't mistake it for the user leaving.
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

    fn cand(addr: &str, score: f32, last: Option<Instant>, history: i32) -> Candidate {
        Candidate {
            addr: addr.to_owned(),
            score,
            last,
            history,
        }
    }

    #[test]
    fn partner_is_the_freshest_interaction_then_frecency() {
        let now = Instant::now();
        let old = now - Duration::from_secs(300);
        // `heavy` is used most overall, but `partner` is where the user
        // just was — one toggle must go back there.
        let order = rank(vec![
            cand("heavy", 9.0, Some(old), 3),
            cand("partner", 4.0, Some(now), 2),
            cand("fresh", 0.0, None, 0),
        ]);
        assert_eq!(order, vec!["partner", "heavy", "fresh"]);
    }

    #[test]
    fn never_interacted_windows_follow_compositor_history() {
        let order = rank(vec![
            cand("older", 0.0, None, 5),
            cand("newer", 0.0, None, 2),
        ]);
        assert_eq!(order, vec!["newer", "older"]);
    }

    #[test]
    fn passed_through_windows_never_become_the_partner() {
        // Max's exact scenario: windows 1 and 2 are the working pair; 3
        // and 4 were only ever cycled THROUGH (no interaction → no score,
        // no last). From 2, the first stop must be 1 — not 3 or 4, even
        // though the compositor focused them more recently.
        let now = Instant::now();
        let order = rank(vec![
            cand("w1", 6.0, Some(now), 3),
            cand("w3", 0.0, None, 1),
            cand("w4", 0.0, None, 2),
        ]);
        assert_eq!(order, vec!["w1", "w3", "w4"]);
    }

    #[test]
    fn frecency_decays_and_prunes() {
        let mut f = Frecency::default();
        f.note("a");
        f.note("a");
        f.note("b");
        let now = Instant::now();
        assert!(f.score("a", now) > f.score("b", now));
        let later = now + HALF_LIFE;
        assert!((f.score("a", later) - f.score("a", now) / 2.0).abs() < 0.01);
        assert_eq!(f.score("ghost", now), 0.0);
        assert!(f.last("a").is_some() && f.last("ghost").is_none());
    }
}

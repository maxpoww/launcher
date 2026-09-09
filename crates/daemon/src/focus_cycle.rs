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

// --- Per-workspace focus memory ---------------------------------------------
// A workspace should be where you left it, including which window you were in.
//
// The compositor does remember, and restores it when a workspace is entered
// through its focus dispatcher — which is what Super+N and the overview use, so
// those already behave. A workspace arrived at by SWIPE does not go through
// that path, and you land on the space you left but in whichever window the
// cursor happens to be over. The space is restored; your place in it is not.
//
// So the daemon keeps the same note the compositor does, and hands it back when
// nobody else has.

/// How long to let the compositor settle its own focus after a workspace change
/// before correcting it. Long enough that we are answering its final answer
/// rather than racing it, short enough to read as arriving already focused.
const WS_FOCUS_SETTLE: Duration = Duration::from_millis(90);

/// Where a just-arrived-at workspace's focus should end up: our own note of
/// where the user was, what the arrival actually focused, and the compositor's
/// own history as the last resort. `None` means leave it alone.
fn settle_target(
    want: Option<&str>,
    focused: Option<&str>,
    history: Option<&str>,
) -> Option<String> {
    match (want, focused) {
        // Our note wins: it is the window the user LEFT here, as against
        // whatever the arrival happened to land on.
        (Some(w), _) => Some(w.to_owned()),
        // No note, but the space handed us something. A first visit is the
        // compositor's call, not ours.
        (None, Some(_)) => None,
        // No note and nothing focused: the space is live but the user has
        // landed nowhere — the cursor came down on an empty patch of a space
        // whose windows all float, and no window claimed them. The
        // compositor's history is trustworthy in exactly this case, because
        // no arrival focus displaced it.
        (None, None) => history.map(str::to_owned),
    }
}

impl App {
    /// Note which window is focused on which workspace, so the space can be
    /// handed back intact later. Called wherever a focus change is noticed.
    pub(crate) fn note_ws_focus(&mut self) {
        let Some((addr, ws)) = hypr::active_focus() else {
            return;
        };
        // A restore is in flight for this space: the focus being seen right now
        // is the arrival focus we are about to correct, not a choice the user
        // made. Recording it would overwrite the very note we are restoring
        // from, and the memory would quietly become "wherever the cursor was".
        if self.ws_restoring == Some(ws) {
            return;
        }
        self.ws_focus.insert(ws, addr);
    }

    /// A workspace became active. If the way in did not restore the window you
    /// left there, put it back.
    pub(crate) fn on_workspace_changed(&mut self, ws: i64) {
        // The overview owns focus while it is up: landing on a workspace by
        // clicking a window in the map means that window, not the remembered
        // one. Restoring here would overrule the click.
        if self.overview_active {
            return;
        }
        // STAGE mode owns the screen: the stage, the deck and the OPTIONS bar,
        // nothing else. A workspace swipe would slide all three away.
        //
        // The gestures cannot simply be switched off — this compositor's Lua API
        // has no way to unregister one (re-registering is refused as
        // "overshadowed", and `action = "none"` is rejected), so instead the
        // stage takes focus straight back. With the workspace animation already
        // silenced for the mode, a swipe lands on nothing visible.
        //
        // The return is unconditional, and that matters: whichever way this
        // workspace was arrived at, the restore below must not run while the
        // stage is up. It hands focus to whatever that workspace last had
        // focused — which on a workspace holding one window is the staged task
        // and invisible, but on a workspace holding ten is some other window
        // entirely, silently taken out from under the task just put on stage.
        if self.stage.is_on() {
            if let Some(addr) = self.stage.staged().map(str::to_owned) {
                if !crate::hypr::window_is_on(&addr, ws) {
                    crate::hypr::focus_window_no_warp(&addr);
                }
            }
            return;
        }
        // No note is not the same as nothing to do: a space we have never held
        // can still be arrived at with NOTHING focused at all, which is what
        // happens when the cursor lands on an empty patch of a space whose
        // windows all float. That is settled below from the compositor's own
        // history rather than left as it is.
        let want = self.ws_focus.get(&ws).cloned();
        self.ws_restoring = Some(ws);
        let timer = calloop::timer::Timer::from_duration(WS_FOCUS_SETTLE);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                app.restore_ws_focus(ws, want.as_deref());
                app.ws_restoring = None;
                calloop::timer::TimeoutAction::Drop
            });
    }

    /// Settle the focus of a workspace just arrived at, unless the world moved
    /// on while we waited.
    fn restore_ws_focus(&mut self, ws: i64, want: Option<&str>) {
        // Someone opened the overview, or swiped on somewhere else entirely,
        // in the 90 ms we spent waiting: their move, not ours.
        if self.overview_active {
            return;
        }
        // Where we are is read from the WORKSPACE, never from the focused
        // window — because there may not be one. Arriving over an empty patch
        // of a space whose windows all float leaves nothing focused at all,
        // and asking a window that does not exist which space it is on used to
        // make this give up exactly when it was needed most.
        let Some((now_ws, _)) = hypr::active_workspace() else {
            return;
        };
        if now_ws != ws {
            return;
        }
        let focused = hypr::active_focus().map(|(a, _)| a);
        // The history is only consulted when nothing is focused, so the extra
        // read costs nothing on the ordinary path.
        let history = focused
            .is_none()
            .then(|| hypr::last_focused_on(ws))
            .flatten();
        let Some(target) = settle_target(want, focused.as_deref(), history.as_deref()) else {
            return;
        };
        // Already there: nothing to correct, and dispatching anyway would be a
        // focus event nobody needed.
        if focused.as_deref() == Some(target.as_str()) {
            return;
        }
        // Closed since, or dragged to another space: the note is stale, and
        // focusing a window that is not here would yank the user off the
        // workspace they just arrived at.
        if !hypr::window_is_on(&target, ws) {
            self.ws_focus.remove(&ws);
            return;
        }
        debug!(
            "workspace {ws}: focusing {target} (arrived on {})",
            focused.as_deref().unwrap_or("nothing")
        );
        hypr::focus_window(&target);
        self.ws_focus.insert(ws, target);
    }

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

    /// Arriving at a workspace: what focus should settle on, in each of the
    /// four states the arrival can leave behind.
    #[test]
    fn a_space_hands_back_the_window_you_left_in_it() {
        // Held a note: it wins over whatever the arrival landed on. This is
        // the whole point — the swipe focuses the window under the cursor,
        // and the user wants the one they were working in.
        assert_eq!(
            settle_target(Some("a"), Some("b"), None).as_deref(),
            Some("a")
        );
        // Including when the arrival focused nothing at all.
        assert_eq!(settle_target(Some("a"), None, None).as_deref(), Some("a"));
        // First visit, and the space handed us something: the compositor's
        // call, not ours. Overriding here would be inventing a preference we
        // were never told.
        assert_eq!(settle_target(None, Some("b"), Some("c")), None);
        // The live bug (2026-09-04): scrolling into a space of floating
        // windows with the cursor over an empty patch focuses NOTHING. No
        // note to go on, so the compositor's own history settles it — which is
        // trustworthy here precisely because no arrival focus displaced it.
        assert_eq!(settle_target(None, None, Some("c")).as_deref(), Some("c"));
        // A genuinely empty space has nothing to focus and must not invent
        // one; a stale history entry is likewise checked against the space
        // before it is used (see `restore_ws_focus`).
        assert_eq!(settle_target(None, None, None), None);
    }
}

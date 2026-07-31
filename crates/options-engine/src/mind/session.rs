//! Temporal self-awareness: the mind's memory across snapshots.
//!
//! [`decide`](super::decide) is pure and instantaneous — it sees one moment.
//! But real awareness has duration: *how long* you've been coding, whether
//! commands keep failing. [`Session`] carries that memory in the [`Mind`]'s
//! loop and distils it into a pure [`Temporal`] summary fed back into each
//! decision, so temporal affordances (take a break; several failures in a row)
//! stay decidable by the same pure function.
//!
//! [`Mind`]: super::Mind

use std::time::Instant;

use crate::state::ContextState;

use super::activity::{infer_activity, Activity};

/// A pure, point-in-time summary of the session's memory — the temporal facts
/// [`decide_with`](super::decide::decide_with) reasons about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Temporal {
    /// Seconds spent continuously in the current activity.
    pub activity_secs: u64,
    /// Consecutive failed shell commands (reset by a successful one).
    pub failure_streak: u32,
}

/// The mind's rolling memory. Lives in the decision loop; `observe` folds each
/// new context in and returns the current [`Temporal`].
pub(crate) struct Session {
    activity: Activity,
    since: Instant,
    last_cmd: Option<String>,
    last_exit: Option<i32>,
    failure_streak: u32,
}

impl Session {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            activity: Activity::Unknown,
            since: now,
            last_cmd: None,
            last_exit: None,
            failure_streak: 0,
        }
    }

    /// Fold a new context snapshot into the session at time `now`, returning the
    /// updated temporal summary. `now` is a parameter so this is testable.
    pub(crate) fn observe(&mut self, ctx: &ContextState, now: Instant) -> Temporal {
        // Activity duration: reset the clock whenever the activity changes.
        let activity = infer_activity(ctx);
        if activity != self.activity {
            self.activity = activity;
            self.since = now;
        }

        // Failure streak: a *new* completed command (its cmd or exit differs
        // from the last we saw) advances or resets the streak.
        let cmd = ctx.app_internal.shell_last_cmd.clone();
        let exit = ctx.app_internal.shell_exit_code;
        let is_new = cmd.is_some() && (cmd != self.last_cmd || exit != self.last_exit);
        if is_new {
            match exit {
                Some(code) if code != 0 => self.failure_streak += 1,
                Some(_) => self.failure_streak = 0,
                None => {}
            }
            self.last_cmd = cmd;
            self.last_exit = exit;
        }

        Temporal {
            activity_secs: now.saturating_duration_since(self.since).as_secs(),
            failure_streak: self.failure_streak,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ActiveWindow;
    use std::time::Duration;

    fn coding_ctx() -> ContextState {
        ContextState {
            window: ActiveWindow {
                class: "Code".into(),
                pid: 1,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn activity_duration_accumulates_and_resets_on_change() {
        let t0 = Instant::now();
        let mut s = Session::new(t0);
        let a = s.observe(&coding_ctx(), t0);
        assert_eq!(a.activity_secs, 0);
        let a = s.observe(&coding_ctx(), t0 + Duration::from_secs(120));
        assert_eq!(a.activity_secs, 120);
        // Switching activity resets the clock.
        let a = s.observe(&ContextState::default(), t0 + Duration::from_secs(121));
        assert_eq!(a.activity_secs, 0);
    }

    #[test]
    fn failure_streak_counts_consecutive_failures() {
        let t0 = Instant::now();
        let mut s = Session::new(t0);
        let mut ctx = coding_ctx();

        let fail = |ctx: &mut ContextState, cmd: &str, code: i32| {
            ctx.app_internal.shell_last_cmd = Some(cmd.into());
            ctx.app_internal.shell_exit_code = Some(code);
        };

        fail(&mut ctx, "a", 1);
        assert_eq!(s.observe(&ctx, t0).failure_streak, 1);
        fail(&mut ctx, "b", 2);
        assert_eq!(s.observe(&ctx, t0).failure_streak, 2);
        // Re-observing the same command doesn't double-count.
        assert_eq!(s.observe(&ctx, t0).failure_streak, 2);
        // A success resets it.
        fail(&mut ctx, "c", 0);
        assert_eq!(s.observe(&ctx, t0).failure_streak, 0);
    }
}

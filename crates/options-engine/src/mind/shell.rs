//! Shell arrangement: *where* you are, as distinct from what you are doing.
//!
//! [`Activity`](super::Activity) answers "what is the user doing" — coding,
//! drawing, on a call. This module answers the other half of the question, and
//! Max named it on 2026-09-12 while approving the seventeen contexts: **normal
//! tile, stage, overview, empty workspace, second monitor — all of those will
//! condition the options too.**
//!
//! They are a genuinely separate axis, not more contexts. You can be *coding on
//! the stage*, *coding in the overview*, or *coding with a second monitor
//! attached*, and each wants something different on the bar while the thing you
//! are doing has not changed at all. Folding them into `Activity` would force a
//! choice between two facts that are both true.
//!
//! # Who owns this
//!
//! The collectors cannot sense it: STAGE and the overview are **waverunner's
//! own modes**, not compositor state, and the daemon is the only thing that
//! knows it has entered one. So this travels the opposite way from every other
//! signal — the surface pushes it into the mind, exactly the way
//! [`Temporal`](super::Temporal) is supplied by the decision loop rather than
//! sensed. [`Mind::set_shell`](super::Mind::set_shell) is the door.
//!
//! # Not yet modelled
//!
//! `~/GolemOne/Estructure.md` also lists **Stage 2** and **zoomout** modes.
//! They are real and deliberately absent here: Max named five states, these are
//! those five, and a mode earns a variant when an OPTION needs to tell it apart
//! (the census growth rule). Adding one is a variant and a match arm.

use serde::Serialize;

/// How the shell is arranged right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum ShellMode {
    /// The ordinary tiled session — windows laid out, the bar over them.
    #[default]
    NormalTile,
    /// STAGE: one task alone at the stage rect, the deck of every other task
    /// in the gap beneath it (`stage.rs`, Super+Enter).
    Stage,
    /// The waveview overview owns the screen.
    Overview,
}

impl ShellMode {
    /// The stable lower-case name, for the OPTIONS list and debug output.
    pub fn name(self) -> &'static str {
        match self {
            ShellMode::NormalTile => "normal tile",
            ShellMode::Stage => "stage",
            ShellMode::Overview => "overview",
        }
    }
}

/// Which surface an OPTION belongs to — the arrangement half of the OPTIONS
/// list's `Shows in:` line.
///
/// Max, 2026-09-12, setting the rule: *"stage will show the context options +
/// a couple of stage-specific options; overview will show overview options,
/// because overview is not meant to show per-context options — it's meant to
/// show window-management options."*
///
/// So the arrangement does not merely *add* to the context axis, it can
/// **replace** it. In the overview, what you were doing stops being the
/// question: you are looking at your windows, so the bar is about windows. That
/// makes this a property of the OPTION, not a filter the surface applies
/// afterwards — the Mind should not rank what it would never show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum ShowsIn {
    /// The ordinary case: an OPTION about **what you are doing**. Present on
    /// the normal tile and on the stage; never in the overview.
    #[default]
    Context,
    /// STAGE-specific — about the stage itself, alongside the context OPTIONS.
    Stage,
    /// Overview-specific — window management. The only kind the overview shows.
    Overview,
}

impl ShowsIn {
    /// Whether an OPTION with this belonging is shown under `mode`.
    pub fn fits(self, mode: ShellMode) -> bool {
        match mode {
            // Context OPTIONS only — the plain session.
            ShellMode::NormalTile => self == ShowsIn::Context,
            // The context set, plus the stage's own.
            ShellMode::Stage => matches!(self, ShowsIn::Context | ShowsIn::Stage),
            // Window management only. What you were doing is not the question.
            ShellMode::Overview => self == ShowsIn::Overview,
        }
    }
}

/// The arrangement half of the context: where the user is, how many screens
/// they have, and whether there is anything here at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ShellState {
    /// Normal tile / stage / overview.
    pub mode: ShellMode,
    /// The current workspace holds **no windows** — a *new* workspace (Max,
    /// 2026-09-12).
    ///
    /// Related to but NOT the same as [`Activity::Idle`](super::Activity::Idle):
    /// Idle means nothing has focus, which is a fact about attention. This is a
    /// fact about the room — a workspace can hold an unfocused floating window
    /// and be non-empty while nothing is focused, and the two want different
    /// OPTIONS. A new workspace is an invitation; an unfocused one is a pause.
    ///
    /// It is therefore counted, never inferred: the daemon asks the compositor
    /// (`hypr::active_workspace_is_empty`).
    pub workspace_empty: bool,
    /// How many displays are attached. `1` is the common case; `>= 2` is the
    /// "second monitor" state, where an OPTION may offer to move something, or
    /// where a per-screen offer has to say *which* screen it means.
    pub monitors: u8,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            mode: ShellMode::NormalTile,
            workspace_empty: false,
            // One screen, not zero: a shell with no display is not a state the
            // mind should ever reason about, and defaulting to 0 would make
            // `has_second_screen()` quietly wrong on the first tick.
            monitors: 1,
        }
    }
}

impl ShellState {
    /// Whether more than one display is attached.
    pub fn has_second_screen(self) -> bool {
        self.monitors >= 2
    }

    /// Whether the bar is over the live desktop (as opposed to the overview
    /// owning the screen). The daemon's existing `Presence`/`DESKTOP_ONLY`
    /// contract is this question asked one surface at a time.
    pub fn is_desktop(self) -> bool {
        !matches!(self.mode, ShellMode::Overview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_one_screen_on_the_normal_tile() {
        let s = ShellState::default();
        assert_eq!(s.mode, ShellMode::NormalTile);
        assert!(!s.workspace_empty);
        assert_eq!(s.monitors, 1);
        assert!(
            !s.has_second_screen(),
            "the default must not claim a second screen"
        );
        assert!(s.is_desktop());
    }

    #[test]
    fn the_overview_is_the_only_non_desktop_mode() {
        assert!(ShellState {
            mode: ShellMode::Stage,
            ..Default::default()
        }
        .is_desktop());
        assert!(!ShellState {
            mode: ShellMode::Overview,
            ..Default::default()
        }
        .is_desktop());
    }

    #[test]
    fn second_screen_starts_at_two() {
        for (n, want) in [(0, false), (1, false), (2, true), (3, true)] {
            let s = ShellState {
                monitors: n,
                ..Default::default()
            };
            assert_eq!(s.has_second_screen(), want, "{n} monitors");
        }
    }

    /// Max's rule, 2026-09-12: the stage adds to the context set; the overview
    /// REPLACES it. This is the test that stops a future context OPTION
    /// quietly leaking into window-management territory.
    #[test]
    fn the_overview_replaces_the_context_axis_and_the_stage_adds_to_it() {
        // Normal tile: context OPTIONS, nothing else.
        assert!(ShowsIn::Context.fits(ShellMode::NormalTile));
        assert!(!ShowsIn::Stage.fits(ShellMode::NormalTile));
        assert!(!ShowsIn::Overview.fits(ShellMode::NormalTile));

        // Stage: the context set PLUS the stage's own.
        assert!(ShowsIn::Context.fits(ShellMode::Stage));
        assert!(ShowsIn::Stage.fits(ShellMode::Stage));
        assert!(!ShowsIn::Overview.fits(ShellMode::Stage));

        // Overview: window management ONLY — what you were doing is not the
        // question while you are looking at your windows.
        assert!(!ShowsIn::Context.fits(ShellMode::Overview));
        assert!(!ShowsIn::Stage.fits(ShellMode::Overview));
        assert!(ShowsIn::Overview.fits(ShellMode::Overview));
    }

    #[test]
    fn context_is_the_default_belonging() {
        assert_eq!(ShowsIn::default(), ShowsIn::Context);
    }

    #[test]
    fn every_mode_has_a_distinct_name() {
        let all = [ShellMode::NormalTile, ShellMode::Stage, ShellMode::Overview];
        let mut names: Vec<&str> = all.iter().map(|m| m.name()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before);
        assert!(names.iter().all(|n| !n.is_empty()));
    }
}

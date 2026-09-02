//! What the mind produces: **affordances** — the concrete "options" OPTIONS
//! offers at a given moment.
//!
//! An affordance is the atom of the whole system's purpose: the right tool or
//! piece of information, surfaced when its use is logical. The mind ranks them,
//! removes the irrelevant ones, and hands the surface a small, ordered set to
//! integrate into the environment. This module is just the data; the deciding
//! is in [`super::decide`].

use serde::{Deserialize, Serialize};

use super::activity::Activity;
use crate::state::Layer;

// `AffordanceKind` round-trips (no borrows); `Affordance`/`OptionSet` are
// serialize-only (see their derives).

/// The character of an affordance — which shapes how skill calibration and the
/// surface treat it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AffordanceKind {
    /// Ambient information (now playing, current branch). Fades for experts.
    Info,
    /// Scaffolding/help the user *might* want (a hint, an offer of assistance:
    /// "want a hand?"). Fades most for experts — the more skilled you are, the
    /// less you need the hand-holding.
    Action,
    /// A direct control the user invokes to *do* the obvious thing in this
    /// context (play/pause, mute mic, commit & push). Unlike [`Action`], it is
    /// NOT scaffolding — an expert wants their controls just as much as a
    /// novice — so skill never fades it. This is the heart of "the right
    /// action at the right moment": the button you were about to reach for.
    Control,
    /// Safety / time-critical (battery, screen sharing). Never suppressed by
    /// skill — it is always relevant when true.
    Warning,
}

/// What triggering an affordance *does*. The engine describes the action
/// declaratively; the surface (the waverunner daemon) executes it. Kept to a
/// small, safe vocabulary that covers the overwhelming majority of desktop
/// offers — see `/home/max/Golem/options-catalog.md`.
///
/// `Deserialize` too (unlike the rest of the affordance): a surface may want to
/// round-trip an action, and the payloads are owned `String`s, not the
/// `&'static str` ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AffordanceAction {
    /// Pure information — nothing to trigger (the resting kind for `Info`/
    /// `Warning` affordances that only tell you something).
    None,
    /// Run a program, fire-and-forget: `argv[0]` is the executable, the rest
    /// its arguments (no shell, so no quoting/injection surface). Covers
    /// playerctl / wpctl / brightnessctl / git / grim / xdg-open — nearly
    /// every control in the catalog.
    Spawn { argv: Vec<String> },
    /// A Hyprland compositor dispatch (e.g. `"fullscreen"`, `"killactive"`),
    /// run through the daemon's existing `hypr::` helpers.
    HyprDispatch(String),
    /// Open a URL with the user's default handler (`xdg-open`).
    OpenUrl(String),
}

impl AffordanceAction {
    /// Whether this action actually does something when triggered (i.e. the
    /// affordance is a live button, not just information).
    pub fn is_actionable(&self) -> bool {
        !matches!(self, AffordanceAction::None)
    }
}

/// One surfaced option: a scored, self-describing unit the surface can render.
///
/// `Serialize` (not `Deserialize`): the mind emits these to a surface; they are
/// created from context, never parsed back in-engine (the `&'static str` ids
/// are compile-time constants).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Affordance {
    /// Stable identity (`"media.now_playing"`, `"system.battery_low"`, …), so
    /// the surface can animate the same affordance across frames.
    pub id: &'static str,
    pub kind: AffordanceKind,
    /// Primary line.
    pub title: String,
    /// Secondary line / context.
    pub detail: String,
    /// Final relevance in `0.0..=1.0` after calibration.
    pub relevance: f32,
    /// Why it was surfaced — for debugging and for diegetic phrasing.
    pub reason: &'static str,
    /// The context layer it derives from, so the mind can gate it on that
    /// source being alive (never surface from a dead sensor).
    pub source: Layer,
    /// What triggering it does. `None` for pure information; a real action for
    /// a [`AffordanceKind::Control`] (or an actionable `Action`). This is the
    /// field that turns a described offer into a working button.
    #[serde(default = "AffordanceAction::none_default")]
    pub action: AffordanceAction,
}

impl AffordanceAction {
    /// Serde default for [`Affordance::action`] so older serialized affordances
    /// (or hand-written ones) deserialize to the inert `None`.
    fn none_default() -> Self {
        AffordanceAction::None
    }
}

/// The mind's output: the ranked, suppressed, capped set of options for one
/// context snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct OptionSet {
    /// The mind's read of what the user is doing — the situation these options
    /// were chosen for.
    pub activity: Activity,
    /// Affordances, highest relevance first.
    pub items: Vec<Affordance>,
    /// The [`ContextState`](crate::ContextState) generation this was decided
    /// from, so subscribers can correlate options with the context that made
    /// them.
    pub generation: u64,
}

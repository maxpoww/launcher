//! What the mind produces: **affordances** — the concrete "options" OPTIONS
//! offers at a given moment.
//!
//! An affordance is the atom of the whole system's purpose: the right tool or
//! piece of information, surfaced when its use is logical. The mind ranks them,
//! removes the irrelevant ones, and hands the surface a small, ordered set to
//! integrate into the environment. This module is just the data; the deciding
//! is in [`super::decide`].

use serde::{Deserialize, Serialize};

use crate::state::Layer;

// `AffordanceKind` round-trips (no borrows); `Affordance`/`OptionSet` are
// serialize-only (see their derives).

/// The character of an affordance — which shapes how skill calibration and the
/// surface treat it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AffordanceKind {
    /// Ambient information (now playing, current branch). Fades for experts.
    Info,
    /// Something the user can act on (scaffolding/help). Fades most for experts.
    Action,
    /// Safety / time-critical (battery, screen sharing). Never suppressed by
    /// skill — it is always relevant when true.
    Warning,
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
}

/// The mind's output: the ranked, suppressed, capped set of options for one
/// context snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct OptionSet {
    /// Affordances, highest relevance first.
    pub items: Vec<Affordance>,
    /// The [`ContextState`](crate::ContextState) generation this was decided
    /// from, so subscribers can correlate options with the context that made
    /// them.
    pub generation: u64,
}

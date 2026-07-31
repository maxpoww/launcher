//! The internal wire between collectors and the aggregator.
//!
//! Each collector runs independently and emits small [`Update`]s over an
//! `mpsc`; the single aggregator task applies them to the master
//! [`ContextState`](crate::ContextState) and republishes. This decouples the
//! layers entirely — a high-frequency source (window focus) can never block a
//! low-frequency one (battery), and one dead collector can't stall the rest.

use crate::state::{ActiveWindow, GitContext, Layer, MediaState, SystemMetrics};

/// A partial change to the context. Only the variants the current collectors
/// produce exist; more are added as each layer lands (Layer 1 today).
#[derive(Debug, Clone)]
pub enum ContextDelta {
    /// The focused window changed (or cleared — `ActiveWindow::default()` means
    /// nothing is focused).
    Window(ActiveWindow),
    /// Active Hyprland submap (`""` = the default map).
    Submap(String),
    /// Active keyboard layout description.
    ActiveLayout(String),
    /// Whether any screencast/screenshare is live.
    Screencasting(bool),
    /// Focus-switching velocity (window switches per second, trailing window) —
    /// a behavioural metric derived from compositor events, no input capture.
    FocusSwitchVelocity(f32),
    /// System hardware sample (CPU / RAM / battery).
    Metrics(SystemMetrics),
    /// Git context of the focused window's working directory (or cleared
    /// default when it isn't in a repo / nothing is focused).
    Git(GitContext),
    /// The active media player's state, or `None` when nothing is playing/present.
    Media(Option<MediaState>),
}

/// A message from a collector to the aggregator: either a data change or a
/// pure liveness signal (connected / disconnected) carrying no data.
#[derive(Debug, Clone)]
pub enum Update {
    /// A data change from `Layer`.
    Delta(Layer, ContextDelta),
    /// A liveness change for `Layer` (e.g. socket connected/dropped).
    Health(Layer, bool),
}

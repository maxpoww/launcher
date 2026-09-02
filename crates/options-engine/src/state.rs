//! The unified perception model: what OPTIONS *senses* about the current
//! context at one instant.
//!
//! [`ContextState`] is the single serializable snapshot every subscriber sees.
//! It is **perception, not decision** — the "mind" that ranks and surfaces
//! affordances sits above this, reading these snapshots. Keeping the sensed
//! state pure and separate is deliberate (the OPTIONS philosophy: the system
//! reads your actions, then decides what is *logical* to surface).
//!
//! The struct layout below is the project's canonical data model. On top of it
//! this crate adds [`Health`] (per-layer freshness + liveness), because a real
//! context-aware mind must tell *"the mic is off"* apart from *"we cannot see
//! the mic"* — acting on dead data is the classic failure mode.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveWindow {
    /// Compositor window address (`0x…`) — the stable handle consumers need
    /// to *act* on the window (focus/close dispatches target it). Empty when
    /// nothing is focused.
    #[serde(default)]
    pub address: String,
    pub class: String,
    pub title: String,
    pub pid: u32,
    pub workspace_id: i32,
    pub is_fullscreen: bool,
    pub is_floating: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSelection {
    pub highlighted_text: Option<String>,
    pub char_count: usize,
    pub is_code: bool,
    pub contains_url: bool,
    /// The clipboard holds what looks like a single local filesystem path
    /// (an absolute `/…` or `~/…` line) — intent to open that file/folder.
    pub is_path: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppInternalContext {
    pub shell_last_cmd: Option<String>,
    pub shell_exit_code: Option<i32>,
    pub editor_file: Option<PathBuf>,
    pub editor_language: Option<String>,
    pub editor_diagnostics_count: u32,
    pub browser_url: Option<String>,
    pub is_reading_docs: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitContext {
    pub repo_root: Option<PathBuf>,
    pub branch: Option<String>,
    pub is_dirty: bool,
    /// The `origin` remote as a browsable **https web URL** (github/gitlab/…),
    /// normalized from an ssh or https git URL, or `None` when there's no
    /// origin. Lets the mind offer "Open on GitHub/GitLab".
    pub remote_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BehavioralMetrics {
    pub keys_per_minute: u32,
    /// Friction indicator: rapid BackSpace / undo bursts in a short window.
    pub undo_burst_count: u32,
    pub is_hesitating: bool,
    pub focus_switch_velocity: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaState {
    pub player_name: String,
    pub title: String,
    pub artist: String,
    pub is_playing: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioState {
    pub is_mic_active: bool,
    pub default_sink_volume: u32,
    pub is_muted: bool,
}

/// A summary of the live (unread) notifications the OPTIONS notification daemon
/// is holding. Sensed from `org.options.Notifications` — the mind turns this into
/// a "you have a critical notification / N unread" affordance. Kept to a summary
/// (count + worst urgency + the newest one's identity), not the full list: the
/// context model surfaces *whether it's worth attention*, and the notification
/// OPTION itself owns the rich list + interaction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationContext {
    /// How many notifications are currently active (unread) on the daemon.
    pub active_count: usize,
    /// Whether any active notification is Critical urgency.
    pub has_critical: bool,
    /// The newest active notification's app name (for the affordance detail).
    pub latest_app: String,
    /// The newest active notification's summary line.
    pub latest_summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub cpu_usage_pct: f32,
    pub ram_usage_pct: f32,
    pub battery_pct: Option<u8>,
    pub is_charging: bool,
    /// Whether this machine has a controllable backlight (`/sys/class/backlight`
    /// is non-empty) — a laptop panel, not a desktop monitor. Lets the mind
    /// offer brightness controls only where they'd actually do something.
    pub has_backlight: bool,
    /// Whether a camera (`/dev/video*`) is currently open by some process — a
    /// live webcam, usually a video call. Privacy awareness, like `is_mic_active`.
    pub is_camera_active: bool,
}

/// NixOS deploy state: is the system we're *running* the system we last *built*?
///
/// Both flags default to `false` — the healthy, in-sync case, and also the
/// resting value on non-NixOS hosts (where the collector never produces data,
/// so the [`Layer::System`] health stays dark and the mind never surfaces
/// these). Derived by comparing the fully-resolved store paths of
/// `/run/booted-system`, `/run/current-system`, and the latest built profile
/// `/nix/var/nix/profiles/system`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployHealth {
    /// The activated (current) system differs from the newest built profile —
    /// the last `nixos-rebuild switch` didn't take effect. The more serious of
    /// the two: the deploy silently failed to activate.
    pub not_activated: bool,
    /// The booted system differs from the newest built profile — a newer
    /// generation was built but we're still running an older one, so a reboot is
    /// needed to actually run the latest.
    pub stale_generation: bool,
}

/// The collector layers, used to stamp per-layer [`Health`]. One layer may feed
/// several fields of [`ContextState`]; this identifies the *source*, not the
/// destination field (e.g. focus-switch velocity is a behavioural metric but is
/// derived by the compositor layer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Layer {
    /// Layer 1 — Hyprland compositor & spatial geometry.
    Compositor,
    /// Layer 2 (part) — Wayland selection / clipboard.
    Selection,
    /// Layer 2 (part) — shell / editor / browser bridges over a unix socket.
    AppBridge,
    /// Layer 3 — cognitive & behavioural input metrics.
    Behavior,
    /// Layer 4 — media, audio, and hardware/system state.
    Hardware,
    /// Layer 4 (part) — NixOS deploy health (generation drift / pending reboot).
    /// Tracked separately from [`Layer::Hardware`] so its liveness reflects the
    /// deploy sensor alone (reading the nix profiles), not the /proc sampler.
    System,
    /// Layer 4 (part) — the OPTIONS notification daemon's live list, sensed over
    /// `org.options.Notifications`. Its own layer so liveness reflects the
    /// notification bus alone (the daemon being reachable).
    Notifications,
}

/// Liveness and freshness of one collector layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayerHealth {
    /// Whether the source is currently connected/streaming.
    pub alive: bool,
    /// Milliseconds (since engine start) of this layer's most recent data
    /// update. `None` = it has never produced data. Lets the mind treat stale
    /// fields as unknown rather than current.
    pub last_update_ms: Option<u64>,
}

/// Per-layer health, so subscribers can weigh each field by its source's
/// liveness and freshness.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Health {
    pub compositor: LayerHealth,
    pub selection: LayerHealth,
    pub app_bridge: LayerHealth,
    pub behavior: LayerHealth,
    pub hardware: LayerHealth,
    pub system: LayerHealth,
    pub notifications: LayerHealth,
}

impl Health {
    fn layer_mut(&mut self, layer: Layer) -> &mut LayerHealth {
        match layer {
            Layer::Compositor => &mut self.compositor,
            Layer::Selection => &mut self.selection,
            Layer::AppBridge => &mut self.app_bridge,
            Layer::Behavior => &mut self.behavior,
            Layer::Hardware => &mut self.hardware,
            Layer::System => &mut self.system,
            Layer::Notifications => &mut self.notifications,
        }
    }

    /// Record a data update from `layer` at `ms`: marks it alive and fresh.
    pub(crate) fn stamp(&mut self, layer: Layer, ms: u64) {
        let h = self.layer_mut(layer);
        h.alive = true;
        h.last_update_ms = Some(ms);
    }

    /// Record only a liveness change (connect/disconnect) without new data.
    pub(crate) fn set_alive(&mut self, layer: Layer, alive: bool) {
        self.layer_mut(layer).alive = alive;
    }
}

/// One complete, serializable snapshot of the sensed desktop context.
///
/// Published atomically through a `tokio::sync::watch` channel: every
/// subscriber always sees a whole, internally-consistent snapshot, never a
/// half-applied mix of two updates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextState {
    pub window: ActiveWindow,
    pub selection: TextSelection,
    pub app_internal: AppInternalContext,
    pub git: GitContext,
    pub behavior: BehavioralMetrics,
    pub media: Option<MediaState>,
    pub audio: AudioState,
    pub metrics: SystemMetrics,
    pub deploy: DeployHealth,
    pub notifications: NotificationContext,
    pub hypr_submap: String,
    pub active_layout: String,
    pub is_screencasting: bool,

    // --- provenance (added by the engine, not in the raw spec) ---
    /// Per-layer liveness + freshness.
    pub health: Health,
    /// Monotonic snapshot id, incremented on every applied update. Lets
    /// subscribers detect "did anything change" cheaply and order snapshots.
    pub generation: u64,
}

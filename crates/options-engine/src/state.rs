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
    /// The clipboard holds what looks like a bare git commit hash (7–40 hex
    /// chars, nothing else) — in a repo, intent to inspect that commit.
    pub is_git_sha: bool,
    /// The clipboard holds SEVERAL absolute paths (or `file://` URIs), one
    /// per line — the shape a file manager's multi-file Copy leaves behind.
    /// Count is what was seen within the bounded snippet; the dir is the
    /// files' deepest common parent folder (decoded, plain path).
    pub multi_path_count: usize,
    pub multi_path_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppInternalContext {
    pub shell_last_cmd: Option<String>,
    pub shell_exit_code: Option<i32>,
    /// The focused shell's working directory (from the shell bridge) — lets
    /// OPTIONS offer "open the current folder in Files".
    pub shell_cwd: Option<PathBuf>,
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

/// One window, as the compositor sees it. The **inventory** entry — the thing
/// the engine never had (finding #83).
///
/// `ActiveWindow` describes the one window in front of you. This describes every
/// other one, so a decision can finally reason about the room instead of the
/// desk: which workspace a thing is on, whether it is even on this monitor,
/// whether the sound is coming from something you cannot see.
///
/// Everything here is returned by the same `j/clients` read, so nothing in it
/// costs an extra query (Max, 2026-09-12: *"everything that costs nothing is
/// good to know than not to"*).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Compositor handle (`0x…`), stable for the window's life.
    pub address: String,
    pub class: String,
    pub title: String,
    /// Owning process — **the join key** to an audio stream (see [`Playing`]).
    pub pid: u32,
    /// Workspace id; negative for special workspaces.
    pub workspace_id: i32,
    /// Workspace name as shown to the user ("1", "web", …).
    pub workspace_name: String,
    /// Monitor index this window lives on.
    pub monitor: i32,
    pub is_fullscreen: bool,
    pub is_floating: bool,
    /// Top-left in logical pixels.
    pub at: (i32, i32),
    /// Size in logical pixels.
    pub size: (i32, i32),
}

/// What a player is doing. MPRIS distinguishes three states and the engine used
/// to collapse them into a bool, which made *"a video paused on ws8"*
/// inexpressible — one of the concrete failures behind finding #83.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackState {
    Playing,
    Paused,
    #[default]
    Stopped,
}

/// Where the engine learned about a stream of sound.
///
/// The two sources answer different halves of the question and neither is
/// sufficient: **MPRIS** knows *what* is playing (title, artist, position) but
/// only for apps that publish it, while **PipeWire** knows *everything that is
/// actually making noise* — including the ffplay, the game and the browser tab
/// that publish nothing at all — plus where that sound is routed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayingSource {
    /// An MPRIS player on the session bus, by its bus name.
    Mpris { bus: String },
    /// A raw PipeWire output stream, by node id. These are the ones MPRIS is
    /// blind to — on the dev box, an `ffplay` playing to the speakers while the
    /// only MPRIS players on the bus were a phone's (2026-09-12).
    Stream { node: u32 },
}

/// One thing making (or holding) sound. The engine keeps a **list** of these:
/// the whole point of #83 is that "video on Chrome ws1, music on Spotify ws5,
/// a video paused on ws8" is three entries, not a choice between them.
///
/// The window and workspace are deliberately NOT copied in here. They are
/// looked up through [`pid`](Self::pid) against
/// [`ContextState::windows`] — see [`ContextState::window_of`] — so the two
/// inventories can never disagree about where a window is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playing {
    pub source: PlayingSource,
    /// The app's own name for itself — MPRIS `Identity`, or PipeWire's
    /// `application.name`.
    pub app: String,
    /// Track title, when the source publishes one (MPRIS only).
    pub title: String,
    pub artist: String,
    pub state: PlaybackState,
    /// Owning process — the join key to [`ContextState::windows`]. `None` when
    /// the source cannot name one (some MPRIS players publish no pid).
    pub pid: Option<u32>,
    /// The sink this is routed to (`target.object`), when known. The answer to
    /// "which speakers is it coming out of" on a machine with five of them.
    pub output: Option<String>,
    /// Position/length in seconds; `0` when unknown or a live stream.
    pub position_secs: u64,
    pub length_secs: u64,
    /// Cover art, as MPRIS publishes it (`mpris:artUrl`).
    ///
    /// **Always a local `file://` path in practice** — Chromium, Firefox and
    /// kdeconnect each write the image out and point at it, verified on the dev
    /// box 2026-09-12. That matters more than it looks: art from a browser
    /// could have been an `https` URL, and fetching one would have been the
    /// first network call anywhere in this shell. The engine has no network by
    /// construction (method §6) and this does not change that — a surface
    /// reading it must treat a non-`file` scheme as "no art" rather than as
    /// something to go and get.
    #[serde(default)]
    pub art_url: Option<String>,
}

impl Playing {
    /// Whether this is making sound right now, as opposed to sitting paused.
    pub fn is_playing(&self) -> bool {
        self.state == PlaybackState::Playing
    }
}

/// One audio output device. The inventory the engine never had: the dev box has
/// five (three HDMI/DisplayPort, the speakers, an EasyEffects sink) and the
/// engine knew the volume of exactly one of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioSink {
    /// PipeWire node name — what a stream's `target.object` matches against.
    pub name: String,
    /// Human description ("sof-hda-dsp Speaker").
    pub description: String,
    pub id: u32,
    pub is_default: bool,
    pub volume_pct: u32,
    pub muted: bool,
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
    /// Whether a Mains (AC) supply reads `online` — the machine is on wall
    /// power. A present-but-dead battery reports "Not charging 0%" (not
    /// "Discharging") while plugged in (the ASUS X550LC, 2026-09-09; the
    /// class the battery ladder's comments already knew as "old machines
    /// report discharging 0% on AC"). `is_charging` alone can't tell that
    /// apart from a real drain, so both the alarm and the battery
    /// affordance also clear on `on_ac`.
    pub on_ac: bool,
    /// Whether this machine has a controllable backlight (`/sys/class/backlight`
    /// is non-empty) — a laptop panel, not a desktop monitor. Lets the mind
    /// offer brightness controls only where they'd actually do something.
    pub has_backlight: bool,
    /// Whether a camera (`/dev/video*`) is currently open by some process — a
    /// live webcam, usually a video call. Privacy awareness, like `is_mic_active`.
    pub is_camera_active: bool,
    /// Whether NO network interface (other than loopback) is up — confirmed
    /// offline, from `/sys/class/net/*/operstate`. Inverted (down, not up) so
    /// the default `false` means "assume fine" and nothing surfaces until the
    /// sensor has positively observed a disconnected machine.
    pub is_network_down: bool,
    /// Whether a screen recording is in progress (a `wf-recorder` process
    /// exists). Drives the record ↔ stop-recording control pair.
    pub is_recording: bool,
    /// Home filesystem usage percent (statvfs on `$HOME`, df's arithmetic);
    /// `None` when unreadable. Drives the disk-almost-full warning.
    pub disk_usage_pct: Option<f32>,
    /// GPU busy percent — the integrated GPU's busiest engine, from the i915
    /// PMU (see `collectors::gpu`). `None` on a machine that cannot be asked:
    /// no Intel GPU, or `kernel.perf_event_paranoid > 1`. A `None` here is
    /// "we cannot tell", which is why the readout omits the column entirely
    /// rather than showing a zero it does not know to be true.
    pub gpu_usage_pct: Option<f32>,
    /// Whether the XDG trash holds anything to empty (`Trash/files`
    /// non-empty) — the cheapest disk-space remedy the shell can offer.
    pub trash_has_items: bool,
    /// Approximate bytes sitting in the trash (bounded sweep) — lets the
    /// empty-trash offer say what it actually reclaims.
    pub trash_bytes: u64,
}

/// How the machine is reaching the network right now.
///
/// Kept as a shape rather than a bool because "connected" is not one state:
/// a wired link that is simply up and a wireless one that is 40% of the way to
/// dropping want different things said about them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkLink {
    /// Nothing is up (or nothing has been observed yet — the honest default,
    /// like [`SystemMetrics::is_network_down`], which stays its own field
    /// because the offer that reads it predates this).
    #[default]
    Down,
    /// A wired interface is up.
    Wired,
    /// A wireless interface is up.
    Wireless,
}

/// The network, as far as the kernel will say locally: what kind of link is up
/// and, for wireless, how good it is.
///
/// Read from `/sys/class/net/*` and `/proc/net/wireless` — **no network traffic
/// and no privileges**, which is what keeps this inside the engine at all (see
/// `method.md` §6: the Brain has no network dependency and must never acquire
/// one). The SSID is deliberately absent: it needs `iw`/nl80211, and a name is
/// not something the readout it feeds has room for anyway.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkState {
    pub link: NetworkLink,
    /// Wireless signal quality as a percentage, when a wireless link is up and
    /// the kernel reports one. `None` on a wired link, and on a wireless one
    /// whose driver publishes no quality.
    ///
    /// ⚠️ This is **signal strength, not throughput**: the kernel's
    /// driver-scaled quality column, conventionally out of 70, which on common
    /// drivers is just `RSSI + 110` (a reading of 41 is −69 dBm). Its zero is a
    /// signal nobody ever sees, so as a percentage it is compressed and
    /// flattering — good for drawing an aerial at its strength, misleading as a
    /// number. Max asked what it meant, 2026-09-13, and the readout shows
    /// [`speed_mbps`](Self::speed_mbps) instead.
    pub signal_pct: Option<u8>,
    /// How much data is actually moving, in **bytes per second**, both
    /// directions together.
    ///
    /// A delta of `/sys/class/net/<if>/statistics/{rx,tx}_bytes` over the time
    /// between two polls — pure kernel counters, no privileges, nothing asked
    /// of any service. `None` until two samples of the same interface exist
    /// (a rate needs a span), and reset whenever the link moves to a different
    /// interface, because a counter difference across two devices is not a rate
    /// of anything.
    ///
    /// This is the live flow, not the link's capacity. The negotiated bitrate
    /// was carried here briefly (2026-09-13) and dropped with its reader: it
    /// never moves, so on a panel beside CPU and RAM it was one more number
    /// that sits still.
    pub throughput_bps: Option<u64>,
    /// The interface carrying the link (`wlan0`, `enp0s31f6`) — the one piece
    /// of identity that is free to read, and the one a future network module
    /// will want.
    pub interface: String,
}

/// Bluetooth, from BlueZ.
///
/// Separate from [`SystemMetrics`] because it has a separate failure: the
/// adapter can be absent, `bluetoothd` can be down, and neither of those means
/// "nothing is connected" — they mean *we cannot say*, which is why this rides
/// its own [`Layer::Bluetooth`] and stays dark until the bus answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothState {
    /// Whether an adapter exists at all. `false` = this machine has no
    /// Bluetooth, which is different from having it switched off.
    pub present: bool,
    /// Whether an adapter is powered on.
    pub powered: bool,
    /// How many devices are connected.
    pub connected: usize,
    /// The name of one connected device (the first BlueZ reports) — enough for
    /// a tooltip or a future module, and free to carry.
    pub device: String,
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

/// Layer 4 (part) — daylight: where the sun stands relative to the horizon at
/// this machine's location, and whether the screen is already compensating.
///
/// Location comes from the system timezone (`/etc/localtime` → the tzdb's
/// `zone1970.tab` coordinates) — city-level accuracy, which is all a sunset
/// needs, and fully local: no network, no GPS, no configuration.
///
/// Both flags default to `false` (daytime, no filter) — also the resting value
/// when the collector can't resolve a location, where the [`Layer::Daylight`]
/// health simply stays dark and the mind never surfaces daylight affordances.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaylightState {
    /// The sun is below the horizon — from sunset, through the night, until
    /// sunrise. Computed, not scheduled: "is it set *now*", so a session that
    /// starts at 23:00 still knows the sun is down.
    pub after_sunset: bool,
    /// A `hyprsunset` process is running, i.e. the screen is already warmed —
    /// however it was started. Lets the offer withdraw itself once taken.
    pub eye_protection_on: bool,
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
    /// Layer 4 (part) — daylight (sun above/below the horizon + hyprsunset
    /// presence). Its own layer so an unresolvable location leaves it dark
    /// rather than asserting "daytime".
    Daylight,
    /// Layer 4 (part) — Bluetooth, over BlueZ. Its own layer because an absent
    /// adapter, a stopped `bluetoothd` and "nothing is paired" are three
    /// different answers, and only the last one is knowledge.
    Bluetooth,
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
    pub daylight: LayerHealth,
    pub bluetooth: LayerHealth,
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
            Layer::Daylight => &mut self.daylight,
            Layer::Bluetooth => &mut self.bluetooth,
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
    /// **Every window the compositor holds**, not just the focused one — the
    /// inventory added for finding #83. Max, 2026-09-12: *"the brain should be
    /// aware of the context. different windows IS the context."*
    #[serde(default)]
    pub windows: Vec<WindowInfo>,
    /// **Everything making or holding sound**, MPRIS and PipeWire merged by pid.
    /// Replaced `media: Option<MediaState>`, which reported one player and so
    /// could not express two — or notice a player with no MPRIS at all.
    ///
    /// Derived: the aggregator recomputes it whenever either half arrives.
    #[serde(default)]
    pub playing: Vec<Playing>,
    /// The MPRIS half of [`playing`](Self::playing), kept so a delta from one
    /// source cannot erase the other. Read [`playing`](Self::playing) instead.
    #[serde(default)]
    pub mpris_players: Vec<Playing>,
    /// The PipeWire half of [`playing`](Self::playing). Read
    /// [`playing`](Self::playing) instead.
    #[serde(default)]
    pub audio_streams: Vec<Playing>,
    /// Every audio output device, with its own volume and mute.
    #[serde(default)]
    pub outputs: Vec<AudioSink>,
    pub audio: AudioState,
    pub metrics: SystemMetrics,
    pub deploy: DeployHealth,
    pub notifications: NotificationContext,
    pub daylight: DaylightState,
    /// What kind of link is up, and how good it is (wireless).
    pub network: NetworkState,
    /// Bluetooth: adapter present/powered, and what is connected.
    pub bluetooth: BluetoothState,
    pub hypr_submap: String,
    pub active_layout: String,
    pub is_screencasting: bool,
    /// The most recently added file in the Downloads folder (within a short
    /// window) — intent to open it. `None` when nothing arrived recently.
    pub recent_download: Option<PathBuf>,

    // --- provenance (added by the engine, not in the raw spec) ---
    /// Per-layer liveness + freshness.
    pub health: Health,
    /// Monotonic snapshot id, incremented on every applied update. Lets
    /// subscribers detect "did anything change" cheaply and order snapshots.
    pub generation: u64,
}

impl ContextState {
    /// The window a process owns, if the compositor is holding one — the join
    /// that turns "something is playing" into "something is playing **there**".
    ///
    /// The relationship is kept as a lookup rather than a copied field so the
    /// two inventories can never disagree: a window that moved workspace since
    /// the audio collector last ran reports its *current* workspace, not a
    /// stale one baked in at the time the sound started.
    pub fn window_for_pid(&self, pid: u32) -> Option<&WindowInfo> {
        self.windows.iter().find(|w| w.pid == pid)
    }

    /// The window behind a [`Playing`], when it has a pid and that pid owns a
    /// window. `None` for a headless player (mpd), a phone over kdeconnect, or
    /// a source that publishes no pid at all.
    pub fn window_of(&self, p: &Playing) -> Option<&WindowInfo> {
        p.pid.and_then(|pid| self.window_for_pid(pid))
    }

    /// Which workspace a [`Playing`] is on — resolved through its window.
    ///
    /// This is the field that makes Max's sentence expressible: *"a video on
    /// Chrome on ws1 and music on Spotify on ws5 and a video paused on ws8"* is
    /// three entries in [`playing`](Self::playing), each answering this.
    pub fn workspace_of(&self, p: &Playing) -> Option<i32> {
        self.window_of(p).map(|w| w.workspace_id)
    }

    /// Only the sources actually making sound right now.
    pub fn playing_now(&self) -> impl Iterator<Item = &Playing> {
        self.playing.iter().filter(|p| p.is_playing())
    }

    /// Whether the focused window is itself one of the things making sound —
    /// "am I looking at the thing I can hear".
    pub fn focused_is_playing(&self) -> bool {
        self.window.pid != 0
            && self
                .playing_now()
                .any(|p| p.pid == Some(self.window.pid))
    }

    /// The sound coming from somewhere the user is NOT looking: playing, and
    /// either on another workspace, or owned by a window that is not focused.
    /// The concrete shape of "I can hear something and I don't know where".
    pub fn playing_out_of_sight(&self) -> impl Iterator<Item = &Playing> {
        self.playing_now()
            .filter(|p| p.pid.is_none_or(|pid| pid != self.window.pid))
    }

    /// The default output device, if the inventory has one.
    pub fn default_output(&self) -> Option<&AudioSink> {
        self.outputs.iter().find(|o| o.is_default)
    }
}

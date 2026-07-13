//! waverunner daemon: persistent Wayland layer-shell launcher.
//!
//! Single-threaded calloop event loop driving three sources:
//! the Wayland connection, the IPC control socket, and (from P4 on) a
//! channel from the background desktop-entry indexer.
//!
//! Rendering is frame-callback driven: while animating we request a
//! `wl_surface.frame` callback per drawn frame; once settled we stop, and
//! the process goes fully idle.

mod animation;
mod apps;
mod content;
mod groups;
mod hypr;
mod ipc;
mod launch;
mod nix;
mod order;
mod pins;
mod renderer;
mod state;
mod surface;
mod usage;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::Context;
use calloop::channel;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, LoopHandle};
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_registry,
    delegate_seat, registry_handlers,
};
use tracing::{debug, error, info, warn};
use waverunner_core::index::AppEntry;
use waverunner_core::{Config, Searcher};
use waverunner_proto::Command;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};

use crate::content::Hit;
use crate::renderer::Renderer;
use crate::state::{Target, UiState};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "waverunner=info".into()),
        )
        .init();

    let config = Config::load().context("loading config.toml")?;
    info!(
        "config loaded: {}x{} popup",
        config.window.width, config.window.height
    );

    let conn = Connection::connect_to_env()
        .context("connecting to Wayland (is this a Wayland session?)")?;
    let (globals, event_queue) = registry_queue_init(&conn).context("wayland registry init")?;
    let qh: QueueHandle<App> = event_queue.handle();

    let mut event_loop: EventLoop<App> =
        EventLoop::try_new().context("creating calloop event loop")?;
    WaylandSource::new(conn.clone(), event_queue)
        .insert(event_loop.handle())
        .map_err(|e| anyhow::anyhow!("registering wayland source: {e}"))?;

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor not available")?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .context("zwlr_layer_shell_v1 not available (compositor must support wlr-layer-shell)")?;

    // The surface is tall enough for the fully risen card, the gap
    // beneath it, the magnification headroom above, and the transparent
    // drag margin around it (room for a dragged icon to roam past the
    // card edges); the card slides within the extent range only.
    let full_extent = config.window.height + config.window.bottom_margin;
    let surface_height =
        full_extent + content::MAGNIFY_HEADROOM as u32 + content::DRAG_MARGIN_TOP as u32;
    let surface_width = config.window.width + 2 * content::DRAG_MARGIN_X as u32;
    let layer = surface::create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        surface_width,
        surface_height,
    );

    // App discovery runs on the one allowed background thread; it
    // rescans on request (dock reveals) and delivers results over this
    // channel. The initial scan is queued by spawn_indexer.
    let (apps_tx, apps_rx) = channel::channel::<apps::LoadedApps>();
    let indexer = apps::spawn_indexer(config.theme.icon_theme.clone(), apps_tx);

    // The nix thread owns the nixpkgs package index (Install-section
    // search) and serializes `nix profile` mutations; results arrive
    // over this channel.
    let (nix_tx, nix_rx) = channel::channel::<nix::Event>();
    let nix = nix::spawn(nix_tx, config.theme.icon_theme.clone());
    // Learn which installed apps live in the imperative profile (so only
    // those offer uninstall); refreshed after every mutation.
    nix.request(nix::Request::ProfilePaths);

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        conn: conn.clone(),
        qh: qh.clone(),
        loop_handle: event_loop.handle(),
        compositor,
        layer,
        renderer: None,
        ui: UiState::new(
            config.animation.clone(),
            // Docked, the card is a floating bar: dock height plus the
            // bottom gap it now hovers above the screen edge.
            (config.window.input_bar_height + config.window.bottom_margin) as f32,
            // Fully open, the card has risen its own height plus the gap.
            full_extent as f32,
        ),
        config,
        buffer_size: (0, 0),
        scale_factor: 1,
        last_frame: None,
        frame_pending: false,
        dirty: false,
        keyboard: None,
        pointer: None,
        hide_deadline: None,
        rest_hide_pending: false,
        restore_window: None,
        pending_refocus: None,
        focus_launched: None,
        interactive: false,
        input_extent: None,
        entries: Vec::new(),
        kinds: Vec::new(),
        icon_layers: Vec::new(),
        base_len: 0,
        asset_folder: None,
        asset_file: None,
        asset_pkg: None,
        nix,
        pkg_hits: Vec::new(),
        pkg_hits_query: None,
        pkg_hit_icons: Vec::new(),
        pkg_hit_placeholders: Vec::new(),
        pkg_layer_base: 0,
        pkg_state: PkgIndexState::Loading,
        busy_ids: HashSet::new(),
        failed_ids: HashMap::new(),
        profile_paths: HashSet::new(),
        removable_ids: HashSet::new(),
        profile_elements: HashSet::new(),
        installed_app_ids: HashSet::new(),
        file_index: Vec::new(),
        files_dir: None,
        groups: groups::GroupDb::load(),
        app_group: None,
        group_minis: Vec::new(),
        order: order::OrderDb::load(),
        reorder_slot: None,
        reorder_dwell: None,
        apps_slide: Vec::new(),
        just_dropped: None,
        prev_searching: false,
        dock_slide: Vec::new(),
        mag_sleep: None,
        mag_amount: 1.0,
        group_anim: 1.0,
        group_anim_target: 1.0,
        group_origin: None,
        pending_icons: None,
        hover: None,
        pointer_pos: None,
        scroll: ScrollState::default(),
        gesture: GestureState::default(),
        search: SearchState::default(),
        indexer,
        usage: usage::UsageDb::load(),
        pins: pins::PinDb::load(),
        dock_order: Vec::new(),
        last_rescan: Instant::now(),
        bounce: None,
        placeholders: Vec::new(),
        zone_free: false,
        exit: false,
    };

    let socket_path = waverunner_proto::socket_path();
    let _socket_guard = ipc::listen(&event_loop.handle(), &socket_path)?;

    event_loop
        .handle()
        .insert_source(apps_rx, |event, _, app| {
            if let channel::Event::Msg(loaded) = event {
                app.on_apps_loaded(loaded);
            }
        })
        .map_err(|e| anyhow::anyhow!("registering apps channel: {e}"))?;

    event_loop
        .handle()
        .insert_source(nix_rx, |event, _, app| {
            if let channel::Event::Msg(event) = event {
                app.on_nix_event(event);
            }
        })
        .map_err(|e| anyhow::anyhow!("registering nix channel: {e}"))?;

    // Intellihide: watch Hyprland window events so the dock can stay
    // up while nothing overlaps its zone, plus a steady poll — this
    // Hyprland emits no event for float toggles or float moves/resizes,
    // so events alone can never catch every layout change. Optional:
    // without Hyprland IPC the dock simply always auto-hides.
    if app.config.input.intellihide {
        match hypr::subscribe(&event_loop.handle()) {
            Ok(()) => {
                app.on_layout_changed();
                if let Err(e) = event_loop.handle().insert_source(
                    Timer::from_duration(ZONE_POLL_INTERVAL),
                    |_, _, app: &mut App| {
                        app.on_layout_changed();
                        TimeoutAction::ToDuration(ZONE_POLL_INTERVAL)
                    },
                ) {
                    warn!("zone poll timer failed ({e}); intellihide is event-driven only");
                }
            }
            Err(e) => warn!("intellihide inactive: {e:#}"),
        }
    }

    info!("daemon up; try: waverunner-ctl toggle");
    while !app.exit {
        event_loop
            .dispatch(None, &mut app)
            .context("event loop dispatch")?;
    }
    info!("layer surface closed, exiting");
    Ok(())
}

/// Everything the event loop mutates: Wayland state objects, the
/// renderer, and the UI state machine.
pub struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,

    conn: Connection,
    qh: QueueHandle<App>,
    loop_handle: LoopHandle<'static, App>,
    compositor: CompositorState,
    layer: LayerSurface,
    renderer: Option<Renderer>,

    ui: UiState,
    config: Config,
    /// Current buffer size in physical pixels, from `configure`.
    buffer_size: (u32, u32),
    scale_factor: i32,
    /// Timestamp of the previous drawn frame, for dt computation.
    last_frame: Option<Instant>,
    /// True while a frame callback is in flight (avoid double-requesting).
    frame_pending: bool,
    /// Scene changed since the last draw; the pending frame callback
    /// (if any) will redraw.
    dirty: bool,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Deadline of the pending auto-hide, if the pointer has left the
    /// dock. Re-entry clears it, invalidating the in-flight timer.
    hide_deadline: Option<Instant>,
    /// A box close is settling to the dock and should rest a beat, then
    /// hide, once the collapse animation finishes.
    rest_hide_pending: bool,
    /// Window that had focus when the box grabbed the keyboard — focus
    /// returns here on a plain close. Cleared on launch (the app takes
    /// focus) and on external focus loss (the user chose another window).
    restore_window: Option<String>,
    /// Refocus deferred until the box settles. Under `follow_mouse` the
    /// compositor won't route the keyboard to a window while our layer
    /// still covers the pointer, and it only re-evaluates on real pointer
    /// motion — so we dispatch the focus only once the card has shrunk out
    /// from under the cursor and stopped committing frames.
    pending_refocus: Option<String>,
    /// Set when we just launched an app: the next window to map (within a
    /// grace window) is focused, since focus-follows-mouse won't hand
    /// keyboard focus to an app whose window opens away from the cursor.
    focus_launched: Option<Instant>,
    /// Last keyboard-interactivity value sent to the compositor.
    interactive: bool,
    /// Last input-region extent sent to the compositor.
    input_extent: Option<u32>,

    /// Discovered applications and home folders (icon texture layers
    /// are aligned with this order), plus transient file-search result
    /// entries appended past `base_len` while a query is live.
    entries: Vec<AppEntry>,
    /// What each entry is (app / file / icon asset), aligned with `entries`.
    kinds: Vec<apps::EntryKind>,
    /// Texture layer per entry: its own index for indexed entries, a
    /// generic asset layer for transient file-search results.
    icon_layers: Vec<u32>,
    /// Length of the indexed (non-transient) prefix of `entries`.
    base_len: usize,
    /// Texture layers of the generic folder/file/package icons, with
    /// their placeholder flags.
    asset_folder: Option<(u32, bool)>,
    asset_file: Option<(u32, bool)>,
    asset_pkg: Option<(u32, bool)>,
    /// Handle to the nix threads (package index + profile mutations).
    nix: nix::Nix,
    /// Top-ranked packages for `pkg_hits_query`, delivered async by the
    /// nix thread; the Install section renders these.
    pkg_hits: Vec<nix::PkgEntry>,
    /// The query `pkg_hits` answers — `""` is the recommendations
    /// storefront, `None` means no answer yet. Stale hits keep showing
    /// until the fresh rank lands (no flicker between keystrokes).
    pkg_hits_query: Option<String>,
    /// Rasterized icons for `pkg_hits` (aligned), uploaded into the
    /// texture array's reserved tail; and their letter-tile flags.
    pkg_hit_icons: Vec<Vec<u8>>,
    pkg_hit_placeholders: Vec<bool>,
    /// First reserved texture layer: the app icon count from the last
    /// `set_icons` upload.
    pkg_layer_base: u32,
    /// Whether the package index is usable yet (drives the Install hint).
    pkg_state: PkgIndexState,
    /// Entry ids (package attrs / desktop ids) with a profile mutation
    /// in flight; their cells render dimmed and ignore input.
    busy_ids: HashSet<String>,
    /// Recently failed mutations: their cells flash "Failed" for
    /// [`FAIL_FLASH`] (details go to the log).
    failed_ids: HashMap<String, Instant>,
    /// Store-path closure of the imperative `nix profile` (async from
    /// the nix thread). An app whose resolved `.desktop` path lands in
    /// here was installed via the profile and can be uninstalled.
    profile_paths: HashSet<String>,
    /// Ids of the currently-indexed apps that are removable — derived
    /// from `profile_paths` whenever apps or the profile change. Only
    /// these offer the Install section as an uninstall drop target.
    removable_ids: HashSet<String>,
    /// Imperative `nix profile` element names (== install attrs) — one
    /// of the signals for hiding already-installed packages from the
    /// Install list.
    profile_elements: HashSet<String>,
    /// Desktop ids of every installed app, refreshed on rescan — matched
    /// against a package's `.desktop` stems / attr to drop it from the
    /// Install list once it's installed.
    installed_app_ids: HashSet<String>,
    /// Home-tree file index the search ranks against (fresh per rescan).
    file_index: Vec<apps::FileEntry>,
    /// Directory the Files section is navigated into (`None` = the
    /// top-level home-folder strip).
    files_dir: Option<std::path::PathBuf>,
    /// Persistent app groups ("boxes") shown in the Apps grid.
    groups: groups::GroupDb,
    /// Group the Apps section is navigated into (index into `groups`).
    app_group: Option<usize>,
    /// Group cells for the renderer: (transient entry index, member
    /// texture layers for the 2×2 mini preview). Rebuilt per refilter.
    group_minis: Vec<(usize, [Option<u32>; 4])>,
    /// Persistent Apps-grid order (install date + manual overrides).
    order: order::OrderDb,
    /// Drag-to-reorder: the make-room gap's display slot (`None` = no
    /// grid drag in flight).
    reorder_slot: Option<usize>,
    /// Pending gap move: the wanted slot and when the pointer started
    /// hovering it (the gap moves after [`REORDER_DWELL`]).
    reorder_dwell: Option<(usize, Instant)>,
    /// Per-cell animated display indices for the Apps grid (the
    /// make-room glide); identity when nothing is in flight.
    apps_slide: Vec<f32>,
    /// The id of an app just dropped by a drag: on the next refilter it
    /// starts at rest in its chosen cell instead of carrying its old
    /// animated position, so the icon lands where it was dropped rather
    /// than gliding there from its origin. One-shot (cleared on use).
    just_dropped: Option<String>,
    /// Whether the previous refilter was showing search results, so the
    /// grid can tell when the query was just cleared — leaving a search
    /// reshuffles wholesale and must not glide the icons back.
    prev_searching: bool,
    /// Animated displacement per dock slot, in slot units (the dock's
    /// make-room glide during drags).
    dock_slide: Vec<f32>,
    /// Magnification sleeps until this instant after a drop (the
    /// placement must be still; the magnify wave returns a beat later).
    mag_sleep: Option<Instant>,
    /// Magnification amplitude envelope (0 = off, 1 = full): fades out
    /// around drags/drops and fades back in — never pops.
    mag_amount: f32,
    /// Group-open transition: raw progress (0..1), its target, and the
    /// surface point the group expands from (the clicked tile).
    group_anim: f32,
    group_anim_target: f32,
    group_origin: Option<(f32, f32)>,
    /// Icons that arrived before the renderer existed.
    pending_icons: Option<Vec<Vec<u8>>>,
    /// Item currently under the pointer.
    hover: Option<Hit>,
    /// Pointer position in surface coordinates, while inside.
    pointer_pos: Option<(f32, f32)>,
    /// Grid scrolling / paging state.
    scroll: ScrollState,
    /// Press-and-drag gesture state.
    gesture: GestureState,
    /// Search box and query state.
    search: SearchState,
    /// Handle to the background indexer thread.
    indexer: apps::Indexer,
    /// Persistent launch-frequency database; drives sort order.
    usage: usage::UsageDb,
    /// Pinned app IDs in user-defined dock order.
    pins: pins::PinDb,
    /// Dock display order: maps slot index → entry index. Rebuilt
    /// whenever entries are loaded or pins change.
    dock_order: Vec<usize>,
    /// When the last rescan was requested, for the reveal cooldown.
    last_rescan: Instant,
    /// A launch bounce in flight: (entry index, start time).
    bounce: Option<(usize, Instant)>,
    /// Which entries have placeholder tiles (no resolved icon).
    placeholders: Vec<bool>,
    /// Intellihide: no window currently overlaps the dock zone, so the
    /// dock parks visible instead of auto-hiding.
    zone_free: bool,

    exit: bool,
}

/// Readiness of the nixpkgs package index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PkgIndexState {
    /// The nix thread is still loading or dumping the index.
    Loading,
    /// The index is searchable (rank queries get answers).
    Ready,
    /// No cache and the dump failed; package search is unavailable.
    Failed,
}

/// Grid scrolling and paging state.
#[derive(Default)]
struct ScrollState {
    /// Accumulated vertical scroll over the dock, for the expand gesture.
    accum: f64,
    /// Set when a scroll gesture triggers Expand. Events within
    /// EXPAND_BLEED_COOLDOWN are eaten so the gesture doesn't bleed
    /// through and advance a page. Cleared immediately by AxisStop so
    /// keyboard Toggle has zero cooldown.
    open_at: Option<Instant>,
    /// Per-section paging state — every popup section scrolls
    /// independently.
    per: [SectionScroll; content::N_SECTIONS],
}

impl ScrollState {
    /// Reset every section to page 0 and clear paging accumulators.
    fn reset_sections(&mut self) {
        self.per = Default::default();
    }
}

/// One section's horizontal paging state.
#[derive(Default)]
struct SectionScroll {
    /// Scroll offset in pixels (visual, lags behind `target`).
    pos: f32,
    /// Scroll animation target; `pos` eases toward this each frame.
    target: f32,
    /// Accumulated scroll toward the next page turn (resets on direction
    /// change and after each turn).
    page_accum: f64,
    /// When the last page turn happened, for PAGE_COOLDOWN.
    page_turned_at: Option<Instant>,
}

/// Press-and-drag gesture state.
#[derive(Default)]
struct GestureState {
    /// Item a left-button press started on; release on the same item
    /// activates it.
    pressed: Option<Hit>,
    /// Surface position where the current button press started; used to
    /// detect when a press-move crosses the drag threshold.
    press_pos: Option<(f32, f32)>,
    /// Active drag-and-drop, if any.
    dragging: Option<DragState>,
}

/// Search box and query state.
struct SearchState {
    /// Live query (empty = unfiltered).
    query: String,
    /// Fuzzy matcher (heap-heavy, allocated once per daemon).
    matcher: Searcher,
    /// Indices into `entries` shown per section, ranked by the query
    /// (every entry in order when the query is empty). Apps land in the
    /// Apps section, home folders in Files; Install is empty for now.
    visible: [Vec<usize>; content::N_SECTIONS],
    /// Auto-selected position (best match while searching) as a flat
    /// index across the sections in order; Enter launches it.
    selected: Option<usize>,
    /// Whether the search box is expanded (vs. compact circle button).
    open: bool,
    /// Animated expansion of the search widget: 0.0 = button, 1.0 = pill.
    expand: f32,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            query: String::new(),
            matcher: Searcher::new(),
            visible: Default::default(),
            selected: None,
            open: false,
            expand: 0.0,
        }
    }
}

/// State of an in-progress drag-and-drop gesture.
struct DragState {
    /// Which entry is being moved (index into `App::entries`).
    entry_idx: usize,
    /// True when the drag started from a dock slot; releasing outside
    /// the dock zone then unpins the entry.
    from_dock: bool,
    /// Current pointer position in surface coordinates.
    pos: (f32, f32),
}

/// Accumulated scroll (in wl_pointer axis units; one wheel notch ≈ 15)
/// needed to trigger the dock-expand / popup-collapse gesture.
const SCROLL_THRESHOLD: f64 = 10.0;

/// Accumulated scroll needed to turn one grid page (≈ two wheel notches).
const PAGE_SCROLL_THRESHOLD: f64 = 30.0;

/// Minimum time between page turns, so a fast flick moves exactly one
/// page instead of spinning the (cyclic) grid.
const PAGE_COOLDOWN: Duration = Duration::from_millis(250);

/// Cap on file-search results shown in the Files section.
const FILE_RESULTS_MAX: usize = 24;

/// Minimum query length before the package index is ranked — one
/// character matches half of nixpkgs and helps no one.
const PKG_QUERY_MIN: usize = 2;

/// How long a failed install/remove flashes "Failed" on its cell.
const FAIL_FLASH: Duration = Duration::from_secs(5);

/// After the box closes, the dock rests this long before it hides — a
/// brief beat parked as a dock instead of vanishing straight away.
const DOCK_REST_AFTER_CLOSE: Duration = Duration::from_millis(200);

/// How long the pointer must linger over a new grid slot before the
/// make-room gap moves there. Folding is immediate; reordering is
/// deliberate — that split keeps side-neighbor folds reachable.
const REORDER_DWELL: Duration = Duration::from_millis(180);

/// Horizontal band within a cell (fraction of its width) where hovering
/// an app rings a fold target and a drop makes/joins a box. Kept fairly
/// narrow so folding is deliberate — outside it the pointer makes room
/// to reorder — but wide enough that folding onto a side-by-side
/// neighbour stays easy.
const FOLD_BAND: std::ops::Range<f32> = 0.30..0.70;

/// Exponential make-room glide rate (per second) for the grid and dock
/// reflow while dragging — higher is snappier, lower is slower.
const MAKEROOM_RATE: f32 = 18.0;

/// Magnification blackout after a drop: the placement stays perfectly
/// still for this long before the magnify wave may return.
const MAG_SLEEP_AFTER_DROP: Duration = Duration::from_secs(1);

/// Cap on entries listed when navigated into a directory.
const FILES_LIST_MAX: usize = 300;

/// How long scroll events are eaten after a scroll gesture expands the
/// dock: the events of that same gesture keep arriving in the Open state
/// and must not page the grid. Cleared early by AxisStop (finger lift).
const EXPAND_BLEED_COOLDOWN: Duration = Duration::from_millis(300);

/// Linux evdev code for the left mouse button.
const BTN_LEFT: u32 = 0x110;

/// Linux evdev code for the right mouse button.
const BTN_RIGHT: u32 = 0x111;

/// Minimum time between app-index rescans. Summoning the dock checks
/// freshness; mashing toggle does not scan repeatedly.
const RESCAN_COOLDOWN: Duration = Duration::from_secs(30);

/// Steady re-poll interval for the dock zone (two tiny local-socket
/// queries): float toggles/moves/resizes and the re-tiling they cause
/// emit no Hyprland event, so polling is the only reliable signal. The
/// render loop stays fully idle between polls.
const ZONE_POLL_INTERVAL: Duration = Duration::from_millis(800);

/// Duration of the launch bounce (two decaying hops), after which the
/// card hides.
const BOUNCE_DURATION: Duration = Duration::from_millis(550);
/// Peak height of the launch bounce, in logical pixels.
const BOUNCE_HEIGHT: f32 = 18.0;

impl App {
    /// Entry point for IPC commands (called from ipc.rs) and for
    /// internally generated commands (Escape, focus loss, scroll).
    pub fn handle_command(&mut self, command: Command) {
        // Summoning or expanding is the moment freshness matters:
        // rescan (coalesced, cooldown-limited) so newly installed and
        // uninstalled apps are reflected without a restart.
        if matches!(command, Command::Toggle | Command::Expand) {
            self.maybe_rescan();
        }
        let prev = self.ui.target();
        if self.ui.apply(command) {
            self.sync_surface_state();
            // A box close (Open→Dock) parks as a dock, then hides after a
            // beat (armed once the collapse settles, in the frame loop).
            // Rising back to Open cancels that.
            match self.ui.target() {
                Target::Dock if prev == Target::Open => self.rest_hide_pending = true,
                Target::Open => {
                    self.rest_hide_pending = false;
                    self.hide_deadline = None;
                }
                _ => {}
            }
            self.schedule_frame();
        }
    }

    /// Re-evaluate the dock zone against Hyprland's window layout
    /// (intellihide). Called on relevant compositor events and from the
    /// steady zone poll: float toggles, interactive float moves/resizes,
    /// and the re-tiling they cause all emit *no* Hyprland event
    /// (verified on socket2), so events alone can never be sufficient.
    fn on_layout_changed(&mut self) {
        if !self.config.input.intellihide {
            return;
        }
        // On any IPC failure, assume occupied: that is the plain
        // auto-hide behavior the daemon has without intellihide.
        let free = hypr::focused_monitor()
            .and_then(|mon| {
                let zone_w = self.config.window.width as f64;
                let zone_h =
                    (self.config.window.input_bar_height + self.config.window.bottom_margin) as f64;
                let zone = (
                    mon.x + (mon.w - zone_w) / 2.0,
                    mon.y + mon.h - zone_h,
                    zone_w,
                    zone_h,
                );
                hypr::zone_state(zone, mon.active_ws).map(|state| !state.occupied)
            })
            .unwrap_or_else(|e| {
                debug!("dock zone query failed: {e:#}");
                false
            });

        if free == self.zone_free {
            return;
        }
        debug!("dock zone free: {free}");
        self.zone_free = free;
        if free {
            // Nothing needs the space: park the dock visible.
            self.hide_deadline = None;
            if self.ui.target() == Target::Hidden {
                self.handle_command(Command::Show);
            }
        } else if self.ui.target() == Target::Dock && self.pointer_pos.is_none() {
            // A window moved in and the user isn't on the dock: dodge.
            self.handle_command(Command::Hide);
        }
    }

    /// A window just mapped: if we launched an app moments ago, give it
    /// keyboard focus (once). With focus-follows-mouse the compositor
    /// otherwise leaves it in the background when it opens off the cursor.
    fn on_window_opened(&mut self, addr: &str) {
        const FOCUS_LAUNCH_GRACE: Duration = Duration::from_secs(10);
        if self.focus_launched.is_some_and(|t| t.elapsed() < FOCUS_LAUNCH_GRACE) {
            self.focus_launched = None;
            hypr::focus_window(addr);
        }
    }

    /// Request an app-index rescan unless one was requested recently.
    fn maybe_rescan(&mut self) {
        if self.last_rescan.elapsed() >= RESCAN_COOLDOWN {
            self.last_rescan = Instant::now();
            self.indexer.request_rescan();
        }
    }

    /// A message from the nix threads: the index became searchable, a
    /// rank answer arrived, or a profile mutation finished.
    fn on_nix_event(&mut self, event: nix::Event) {
        match event {
            nix::Event::IndexReady(count) => {
                info!("package index ready: {count} packages");
                self.pkg_state = PkgIndexState::Ready;
                self.refilter();
            }
            nix::Event::IndexFailed => {
                self.pkg_state = PkgIndexState::Failed;
                self.schedule_frame();
            }
            nix::Event::Ranked { query, hits } => {
                self.pkg_hits = hits;
                // Icons for the previous hits don't fit these; show the
                // generic tile until this query's HitIcons arrive.
                self.pkg_hit_icons.clear();
                self.pkg_hit_placeholders.clear();
                // Re-render only if the answer matches what's typed; an
                // outdated one keeps waiting for its follower.
                let current = query == self.search.query;
                self.pkg_hits_query = Some(query);
                if current {
                    self.refilter();
                }
            }
            nix::Event::HitIcons {
                query,
                icons,
                placeholders,
            } => {
                if Some(&query) == self.pkg_hits_query.as_ref() {
                    self.pkg_hit_icons = icons;
                    self.pkg_hit_placeholders = placeholders;
                    self.upload_pkg_icons();
                    if query == self.search.query {
                        // Transients hold per-entry layer/placeholder:
                        // rebuild them onto the fresh icons.
                        self.refilter();
                    }
                }
            }
            nix::Event::Done { id, ok } => {
                debug!("profile mutation for {id}: ok={ok}");
                self.busy_ids.remove(&id);
                if ok {
                    // The profile changed under us: rescan so the Apps
                    // grid gains/loses the entry.
                    self.indexer.request_rescan();
                } else {
                    // Flash "Failed" on the cell; a timer clears it.
                    self.failed_ids.insert(id, Instant::now());
                    let timer = Timer::from_duration(FAIL_FLASH);
                    if let Err(e) = self
                        .loop_handle
                        .insert_source(timer, |_, _, app: &mut App| {
                            app.failed_ids.retain(|_, t| t.elapsed() < FAIL_FLASH);
                            app.schedule_frame();
                            TimeoutAction::Drop
                        })
                    {
                        warn!("cannot arm fail-flash timer: {e}");
                    }
                }
                self.update_hover();
                self.schedule_frame();
            }
            nix::Event::ProfilePaths { closure, elements } => {
                self.profile_paths = closure.into_iter().collect();
                self.profile_elements = elements.into_iter().collect();
                self.recompute_removable();
                self.schedule_frame();
            }
        }
    }

    /// Recompute which indexed apps are removable: an app is removable
    /// when its `.desktop` file resolves into the imperative profile's
    /// store-path closure. Cheap filesystem canonicalization per app,
    /// run when either the apps or the profile change; also refreshes
    /// the set of installed desktop ids used to hide installed packages.
    fn recompute_removable(&mut self) {
        self.installed_app_ids = self
            .entries
            .iter()
            .zip(&self.kinds)
            .take(self.base_len)
            .filter(|(_, &k)| k == apps::EntryKind::App)
            .map(|(e, _)| e.id.clone())
            .collect();
        let profile = &self.profile_paths;
        self.removable_ids = self
            .entries
            .iter()
            .zip(&self.kinds)
            .take(self.base_len)
            .filter(|(_, &k)| k == apps::EntryKind::App)
            .filter_map(|(e, _)| {
                let canon = std::fs::canonicalize(e.path.as_ref()?).ok()?;
                let root = nix::store_path_root(&canon)?;
                profile.contains(root.to_str()?).then(|| e.id.clone())
            })
            .collect();
    }

    /// Whether a nixpkgs package is already installed on the system —
    /// matched to an installed app by the `.desktop` ids it ships, by
    /// its attr used as a desktop id, or by its attr as an imperative
    /// profile element name. Such packages are hidden from the Install
    /// list (you can't install what's already there).
    fn pkg_installed(&self, p: &nix::PkgEntry) -> bool {
        p.icons.iter().any(|s| self.installed_app_ids.contains(s))
            || self.installed_app_ids.contains(&p.attr)
            || self.profile_elements.contains(&p.attr)
    }

    /// The indexer thread finished: adopt the entries and upload icons
    /// (or stash them until the renderer exists).
    fn on_apps_loaded(&mut self, loaded: apps::LoadedApps) {
        info!("app index ready: {} entries", loaded.entries.len());

        // Sort by descending launch frequency so the most-used entries
        // appear first in both the dock and the unfiltered grids. Ties
        // preserve the alphabetical order that comes from the indexer.
        let mut combined: Vec<(
            waverunner_core::index::AppEntry,
            apps::EntryKind,
            Vec<u8>,
            bool,
        )> = loaded
            .entries
            .into_iter()
            .zip(loaded.kinds)
            .zip(loaded.icons)
            .zip(loaded.placeholders)
            .map(|(((e, k), i), p)| (e, k, i, p))
            .collect();
        combined.sort_by_key(|(e, _, _, _)| std::cmp::Reverse(self.usage.count(&e.id)));
        let mut kinds = Vec::with_capacity(combined.len());
        let mut icons = Vec::with_capacity(combined.len());
        let mut placeholders = Vec::with_capacity(combined.len());
        self.entries = combined
            .into_iter()
            .map(|(e, k, i, p)| {
                kinds.push(k);
                icons.push(i);
                placeholders.push(p);
                e
            })
            .collect();
        self.kinds = kinds;
        self.placeholders = placeholders;
        self.base_len = self.entries.len();
        self.icon_layers = (0..self.base_len as u32).collect();
        self.file_index = loaded.files;
        let asset = |id: &str| {
            self.entries
                .iter()
                .position(|e| e.id == id)
                .map(|i| (i as u32, self.placeholders[i]))
        };
        self.asset_folder = asset("asset-folder");
        self.asset_file = asset("asset-file");
        self.asset_pkg = asset("asset-pkg");

        // Record first-seen order (install date): new apps append at
        // the end of the grid, macOS-style. The very first sync seeds
        // the baseline in the current (usage) order.
        self.order.sync(
            self.entries
                .iter()
                .zip(&self.kinds)
                .filter(|(_, k)| **k == apps::EntryKind::App)
                .map(|(e, _)| e.id.as_str()),
        );

        self.pkg_layer_base = icons.len() as u32;
        match self.renderer.as_mut() {
            Some(renderer) => {
                renderer.set_icons(&icons);
                // set_icons rebuilt the array: restore the package-hit
                // icons into its reserved tail.
                self.upload_pkg_icons();
            }
            None => self.pending_icons = Some(icons),
        }
        // Indices may have shifted: drop any armed click or in-flight drag,
        // rebuild the dock order, re-rank the query, re-resolve hover.
        self.gesture.pressed = None;
        self.gesture.dragging = None;
        self.recompute_dock_order();
        self.recompute_removable();
        self.refilter();
        self.schedule_frame();
    }

    /// Re-rank entries against the current query, fanning matches into
    /// their sections best-first, with the top match overall
    /// auto-selected so Enter launches it. With a query, the Files
    /// section searches the whole home-tree file index (transient
    /// entries borrowing a generic icon); without one it shows the
    /// top-level home folders, most-used first.
    fn refilter(&mut self) {
        // Snapshot the Apps cells' animated display positions (by
        // entry id — indices won't survive the rebuild) so the new
        // list can pick them up seamlessly below.
        let old_slide: Vec<(String, f32)> = self.search.visible[content::SECTION_APPS]
            .iter()
            .enumerate()
            .filter_map(|(i, &e)| {
                let d = self.apps_slide.get(i).copied()?;
                Some((self.entries.get(e)?.id.clone(), d))
            })
            .collect();
        // Drop the previous query's transient file-result entries.
        self.entries.truncate(self.base_len);
        self.kinds.truncate(self.base_len);
        self.placeholders.truncate(self.base_len);
        self.icon_layers.truncate(self.base_len);

        let searching = !self.search.query.is_empty();
        // Clearing the query snaps the grid from ranked order back to
        // its resting order — a wholesale reshuffle the carry-over must
        // skip, or every icon glides back to its cell.
        let leaving_search = self.prev_searching && !searching;
        self.prev_searching = searching;
        let names: Vec<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        let ranked = self.search.matcher.rank(&self.search.query, &names);
        let mut visible: [Vec<usize>; content::N_SECTIONS] = Default::default();
        for idx in ranked {
            match self.kinds.get(idx) {
                Some(apps::EntryKind::App) => visible[content::SECTION_APPS].push(idx),
                // While searching, the file index below covers folders
                // too — don't list them twice.
                Some(apps::EntryKind::File) if !searching => {
                    visible[content::SECTION_FILES].push(idx)
                }
                _ => {}
            }
        }
        // The Install section fills with or without a query (live
        // search vs the recommendations storefront).
        visible[content::SECTION_INSTALL] = self.pkg_results();
        self.group_minis.clear();
        if searching {
            visible[content::SECTION_FILES] = self.file_results();
        } else {
            // Hide pinned apps from the grid when the search box is
            // empty — they're already visible on the dock. Grouped
            // apps live inside their box, not loose in the grid.
            visible[content::SECTION_APPS].retain(|&idx| {
                let id = &self.entries[idx].id;
                !self.pins.is_pinned(id) && !self.groups.is_grouped(id)
            });
            // A dissolved group can't stay open.
            if self
                .app_group
                .is_some_and(|g| g >= self.groups.groups().len())
            {
                self.app_group = None;
            }
            if let Some(g) = self.app_group {
                // Inside a box: its members are the whole Apps grid.
                let members = self.groups.groups()[g].members.clone();
                visible[content::SECTION_APPS] = members
                    .iter()
                    .filter_map(|id| self.entries.iter().position(|e| &e.id == id))
                    .collect();
            } else {
                // Boxes and loose apps share one grid order: install
                // date with manual drags on top (new boxes take their
                // target's slot; unseen ids append at the end).
                let mut cells = self.group_cells();
                cells.append(&mut visible[content::SECTION_APPS]);
                let ids: Vec<String> = cells.iter().map(|&i| self.entries[i].id.clone()).collect();
                self.order.sync(ids.iter().map(String::as_str));
                cells.sort_by_key(|&idx| self.order.index_of(&self.entries[idx].id));
                visible[content::SECTION_APPS] = cells;
            }
            if self.files_dir.is_some() {
                // Navigated into a folder: list its contents instead of
                // the top-level home strip.
                visible[content::SECTION_FILES] = self.dir_listing();
            }
        }
        self.search.visible = visible;
        // Visual continuity: every surviving cell keeps its current
        // animated display position and eases to its new seat from
        // there — a rebuilt list never snaps icons, not even for the
        // one synchronous frame this refilter may draw. New entries
        // start at rest.
        let vis = &self.search.visible[content::SECTION_APPS];
        let mut slide: Vec<f32> = (0..vis.len()).map(|i| i as f32).collect();
        if !searching && !leaving_search {
            // (Ranked search results churn per keystroke; gliding
            // between ranks would be noise, so carry-over is for the
            // loose grid and box views only.)
            for (i, &e) in vis.iter().enumerate() {
                if let Some(entry) = self.entries.get(e) {
                    // The just-dropped icon keeps its rest position (`i`)
                    // so it appears in the chosen cell — never glides in
                    // from where it was picked up.
                    if self.just_dropped.as_deref() == Some(entry.id.as_str()) {
                        continue;
                    }
                    if let Some(&(_, d)) = old_slide.iter().find(|(oid, _)| *oid == entry.id) {
                        slide[i] = d;
                    }
                }
            }
        }
        self.just_dropped = None;
        self.apps_slide = slide;
        self.search.selected = if self.search.query.is_empty() || self.flat_len() == 0 {
            None
        } else {
            Some(0)
        };
        self.scroll.reset_sections();
        self.update_hover();
        self.schedule_frame();
    }

    /// Append one transient Files-section entry (kind File, generic
    /// folder/file icon layer) and return its index. `id` is the path.
    fn push_transient_file(&mut self, id: &str, name: &str, exec: String, is_dir: bool) -> usize {
        let asset = if is_dir {
            self.asset_folder
        } else {
            self.asset_file
        };
        // Without an asset icon, fall back to a letter-tile placeholder.
        let (layer, placeholder) = asset.unwrap_or((0, true));
        self.entries.push(AppEntry {
            id: id.to_owned(),
            name: name.to_owned(),
            description: Some(id.to_owned()),
            exec,
            icon: None,
            needs_terminal: false,
            path: None,
        });
        self.kinds.push(apps::EntryKind::File);
        self.placeholders.push(placeholder);
        self.icon_layers.push(layer);
        self.entries.len() - 1
    }

    /// Rank the home-tree file index against the query and append the
    /// top matches as transient entries, returning their indices for
    /// the Files section.
    fn file_results(&mut self) -> Vec<usize> {
        let names: Vec<&str> = self.file_index.iter().map(|f| f.name.as_str()).collect();
        let ranked = self.search.matcher.rank(&self.search.query, &names);
        let mut out = Vec::new();
        for fi in ranked.into_iter().take(FILE_RESULTS_MAX) {
            let f = self.file_index[fi].clone();
            let exec = format!("xdg-open {}", launch::shell_quote(&f.path));
            out.push(self.push_transient_file(&f.path, &f.name, exec, f.is_dir));
        }
        out
    }

    /// Copy the current package-hit icons into the icon texture array's
    /// reserved tail (base = app icon count of the last `set_icons`).
    fn upload_pkg_icons(&mut self) {
        let base = self.pkg_layer_base;
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        for (i, pixels) in self.pkg_hit_icons.iter().enumerate() {
            renderer.update_icon_layer(base + i as u32, pixels);
        }
    }

    /// Append one transient Install-section entry for the `hit_idx`-th
    /// ranked package (kind Package, its own icon from the reserved
    /// texture tail) and return its index. The entry id is the package
    /// attr — the install handle.
    fn push_transient_pkg(&mut self, pkg: &nix::PkgEntry, hit_idx: usize) -> usize {
        let (id, name, version) = (pkg.attr.clone(), pkg.name.clone(), pkg.version.clone());
        let (layer, placeholder) = if hit_idx < self.pkg_hit_icons.len() {
            (
                self.pkg_layer_base + hit_idx as u32,
                self.pkg_hit_placeholders[hit_idx],
            )
        } else {
            // No rasterized icon delivered (shouldn't happen): generic
            // package icon.
            self.asset_pkg.unwrap_or((0, true))
        };
        self.entries.push(AppEntry {
            id,
            name,
            description: Some(version),
            exec: String::new(),
            icon: None,
            needs_terminal: false,
            path: None,
        });
        self.kinds.push(apps::EntryKind::Package);
        self.placeholders.push(placeholder);
        self.icon_layers.push(layer);
        self.entries.len() - 1
    }

    /// Rank the package index against the query and append the top
    /// matches as transient entries, returning their indices for the
    /// Install section.
    fn pkg_results(&mut self) -> Vec<usize> {
        if self.pkg_state != PkgIndexState::Ready {
            return Vec::new();
        }
        // Ranking the index is too slow for the render path: the nix
        // thread does it and answers with a Ranked event, which
        // refilters again. Until then the previous hits keep showing.
        // The empty query is the recommendations storefront; a 1-char
        // query keeps showing whatever is up (no flash to empty).
        let query = self.search.query.clone();
        let rankable = query.is_empty() || query.chars().count() >= PKG_QUERY_MIN;
        if rankable && self.pkg_hits_query.as_deref() != Some(query.as_str()) {
            self.nix.request(nix::Request::Rank { query });
        }
        let hits = self.pkg_hits.clone();
        // Already-installed packages drop out of the list; the original
        // hit index is kept so each shown package keeps its own icon.
        let shown: Vec<(usize, nix::PkgEntry)> = hits
            .into_iter()
            .enumerate()
            .filter(|(_, p)| !self.pkg_installed(p))
            .collect();
        shown
            .into_iter()
            .map(|(i, p)| self.push_transient_pkg(&p, i))
            .collect()
    }

    /// Build one transient Apps-grid cell per group (id
    /// `group:<stable-id>`, kind Group) and record the member texture
    /// layers for the 2×2 mini preview. Returns the cells' entry
    /// indices, group order.
    fn group_cells(&mut self) -> Vec<usize> {
        // Snapshot first: labels and member layers read groups+entries
        // while pushing mutates the entry arrays.
        let snapshot: Vec<(String, String, [Option<u32>; 4])> = (0..self.groups.groups().len())
            .map(|g| {
                let label = self.groups.label(g, |id| {
                    self.entries
                        .iter()
                        .find(|e| e.id == id)
                        .map(|e| e.name.clone())
                });
                let mut minis = [None; 4];
                for (k, member) in self.groups.groups()[g].members.iter().take(4).enumerate() {
                    minis[k] = self
                        .entries
                        .iter()
                        .position(|e| &e.id == member)
                        .and_then(|idx| self.icon_layers.get(idx).copied());
                }
                (
                    format!("group:{}", self.groups.groups()[g].id),
                    label,
                    minis,
                )
            })
            .collect();
        snapshot
            .into_iter()
            .map(|(gid, label, minis)| {
                self.entries.push(AppEntry {
                    id: gid,
                    name: label,
                    description: None,
                    exec: String::new(),
                    icon: None,
                    needs_terminal: false,
                    path: None,
                });
                self.kinds.push(apps::EntryKind::Group);
                self.placeholders.push(false);
                self.icon_layers.push(0);
                let idx = self.entries.len() - 1;
                self.group_minis.push((idx, minis));
                idx
            })
            .collect()
    }

    /// Open a box: its members expand out of the clicked tile.
    fn open_group(&mut self, g: usize) {
        self.group_origin = self.pointer_pos;
        self.group_anim = 0.0;
        self.group_anim_target = 1.0;
        self.app_group = Some(g);
        self.refilter();
    }

    /// Close the open box: members glide back into the tile; the view
    /// actually switches once the animation lands (see `draw`).
    fn close_group(&mut self) {
        if self.app_group.is_none() {
            return;
        }
        if self.group_origin.is_none() {
            self.group_origin = self.pointer_pos;
        }
        self.group_anim_target = 0.0;
        self.schedule_frame();
    }

    /// Display name of the open group, for the Apps section title.
    fn apps_group_name(&self) -> String {
        let Some(g) = self.app_group else {
            return String::new();
        };
        self.groups.label(g, |id| {
            self.entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.name.clone())
        })
    }

    /// Hint shown centered in an empty Install section.
    fn install_hint(&self) -> &'static str {
        match self.pkg_state {
            PkgIndexState::Loading => "Indexing nixpkgs…",
            PkgIndexState::Failed => "Package search unavailable",
            PkgIndexState::Ready if self.search.query.is_empty() => {
                "Search to install from nixpkgs"
            }
            PkgIndexState::Ready if self.search.query.chars().count() < PKG_QUERY_MIN => {
                "Keep typing…"
            }
            // A live query with zero matches: the generic "No results".
            PkgIndexState::Ready => "",
        }
    }

    /// Transient listing of the navigated directory's visible children —
    /// folders first, alphabetical. (Going up is the "‹ Back" button by
    /// the section title; a terminal there is a right-click away.)
    fn dir_listing(&mut self) -> Vec<usize> {
        let Some(dir) = self.files_dir.clone() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut children: Vec<(bool, String, String)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    return None;
                }
                let is_dir = e.file_type().ok()?.is_dir();
                Some((is_dir, name, e.path().to_string_lossy().into_owned()))
            })
            .collect();
        // Folders first, then files, each alphabetical.
        children.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (is_dir, name, path) in children.into_iter().take(FILES_LIST_MAX) {
            let exec = format!("xdg-open {}", launch::shell_quote(&path));
            out.push(self.push_transient_file(&path, &name, exec, is_dir));
        }
        out
    }

    /// Files-section navigation: returns true when the hit was a folder
    /// and the strip navigated into it (plain files fall through to the
    /// launch path). Clicking a folder in search results jumps there
    /// too, clearing the query.
    fn try_navigate(&mut self, entry_idx: usize) -> bool {
        if self.kinds.get(entry_idx) != Some(&apps::EntryKind::File) {
            return false;
        }
        let Some(path) = self
            .entries
            .get(entry_idx)
            .and_then(|e| e.description.clone())
        else {
            return false;
        };
        if !std::fs::metadata(&path)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            return false;
        }
        self.files_dir = Some(std::path::PathBuf::from(path));
        // A folder clicked in search results jumps navigation there;
        // the query has done its job.
        self.search.query.clear();
        self.search.open = false;
        self.refilter();
        true
    }

    /// Go up one level from the navigated directory; reaching (or
    /// escaping) home lands back on the home-folder strip.
    fn files_nav_up(&mut self) {
        let home = std::env::var("HOME").unwrap_or_default();
        self.files_dir = self
            .files_dir
            .as_ref()
            .and_then(|d| d.parent().map(std::path::Path::to_path_buf))
            .filter(|p| {
                !home.is_empty() && p.starts_with(&home) && *p != std::path::Path::new(&home)
            });
        self.refilter();
    }

    /// The directory a Files-section entry stands for: the folder itself,
    /// or the containing folder of a plain file. `None` for non-File
    /// entries. (The path lives in `description`; the id is a path only
    /// for transient entries, home-strip folders use "folder-<name>".)
    fn entry_dir_path(&self, entry_idx: usize) -> Option<std::path::PathBuf> {
        if self.kinds.get(entry_idx) != Some(&apps::EntryKind::File) {
            return None;
        }
        let path = self.entries.get(entry_idx)?.description.as_deref()?;
        let path = std::path::Path::new(path);
        if std::fs::metadata(path).ok()?.is_dir() {
            Some(path.to_path_buf())
        } else {
            path.parent().map(std::path::Path::to_path_buf)
        }
    }

    /// Right-click on a Files cell: open a terminal in that directory
    /// (the folder itself, or a file's containing folder), with launch
    /// feedback and dismissal like any other activation.
    fn open_terminal_at(&mut self, entry_idx: usize) {
        let Some(dir) = self.entry_dir_path(entry_idx) else {
            return;
        };
        let dir = dir.to_string_lossy().into_owned();
        let exec = format!(
            "cd {} && exec {}",
            launch::shell_quote(&dir),
            self.config.launch.terminal
        );
        info!("terminal at {dir}");
        if let Err(e) = launch::launch(&exec, false, &self.config.launch.terminal) {
            error!("terminal launch failed: {e:#}");
            return;
        }
        self.bounce = Some((entry_idx, Instant::now()));
        self.schedule_frame();
        let timer = Timer::from_duration(BOUNCE_DURATION);
        if self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.dismiss();
                TimeoutAction::Drop
            })
            .is_err()
        {
            self.dismiss();
        }
    }

    /// Display string of the navigated directory ("~/Documents/x"),
    /// empty at the top level.
    fn files_path_display(&self) -> String {
        let Some(dir) = &self.files_dir else {
            return String::new();
        };
        let home = std::env::var("HOME").unwrap_or_default();
        let s = dir.to_string_lossy();
        match s.strip_prefix(&home) {
            Some(rest) if !home.is_empty() => format!("~{rest}"),
            _ => s.into_owned(),
        }
    }

    /// Total selectable cells across all sections (flat keyboard order:
    /// Apps, Install, Files).
    fn flat_len(&self) -> usize {
        self.search.visible.iter().map(Vec::len).sum()
    }

    /// Map a flat selection index to (section, cell within section).
    fn flat_to_pos(&self, mut i: usize) -> Option<(usize, usize)> {
        for (s, list) in self.search.visible.iter().enumerate() {
            if i < list.len() {
                return Some((s, i));
            }
            i -= list.len();
        }
        None
    }

    /// Layout for an arbitrary card extent at the current scroll offsets.
    fn layout_at(&self, extent: f32) -> content::Layout {
        content::layout(
            &self.config,
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            extent,
            self.dock_order.len(),
            std::array::from_fn(|s| self.search.visible[s].len()),
            std::array::from_fn(|s| self.scroll.per[s].pos),
            [self.app_group.is_some(), false, self.files_dir.is_some()],
        )
    }

    /// Layout for the current animation extent.
    fn current_layout(&self) -> content::Layout {
        self.layout_at(self.ui.extent())
    }

    /// Rebuild `dock_order`: pinned entries first (in pin order), then
    /// most-used non-pinned apps (entries are already usage-sorted).
    /// Folders never auto-fill the dock, but an explicit pin works.
    fn recompute_dock_order(&mut self) {
        self.dock_order.clear();
        for pin_id in self.pins.pins() {
            if let Some(idx) = self.entries.iter().position(|e| &e.id == pin_id) {
                if !self.dock_order.contains(&idx) {
                    self.dock_order.push(idx);
                }
            }
        }
        for idx in 0..self.entries.len() {
            if self.kinds.get(idx) != Some(&apps::EntryKind::App) {
                continue;
            }
            if !self.dock_order.contains(&idx) && !self.pins.is_excluded(&self.entries[idx].id) {
                self.dock_order.push(idx);
            }
        }
        // No truncation here — layout() clamps to the available width.
    }

    /// Which dock slot a drag should insert before, given the pointer
    /// position.  Returns `None` when the pointer is outside the dock band.
    fn drag_dock_insert(&self, layout: &content::Layout, pos: (f32, f32)) -> Option<usize> {
        let slots = &layout.dock_slots;
        if slots.is_empty() {
            return None;
        }
        let (x, y) = pos;
        let dock_top = layout.card_top;
        let dock_bottom = slots[0].y + slots[0].h;
        if y < dock_top || y > dock_bottom {
            return None;
        }
        let insert = slots
            .iter()
            .position(|s| x < s.x + s.w / 2.0)
            .unwrap_or(slots.len());
        Some(insert)
    }

    /// Finish a drag. Apps pin at the dock slot they were dropped on
    /// (dock-origin drags dropped elsewhere unpin), a profile app
    /// dropped on the Install section uninstalls, and a package dropped
    /// on the Apps section (or the dock) installs. `released` is false
    /// when the drag ended by the pointer leaving the surface — that
    /// path never installs or uninstalls anything.
    fn drop_drag(&mut self, drag: DragState, insert: Option<usize>, released: bool) {
        let Some(entry) = self.entries.get(drag.entry_idx) else {
            return; // entries were replaced mid-drag
        };
        let (id, path) = (entry.id.clone(), entry.path.clone());
        let kind = self.kinds.get(drag.entry_idx).copied();
        // Whatever this drop rearranges, the dragged icon itself must
        // land in its chosen cell, not glide there from its origin: the
        // next refilter places it at rest (self-clears after use).
        self.just_dropped = Some(id.clone());
        let layout = self.current_layout();
        let section = if released {
            content::section_at(&layout, drag.pos)
        } else {
            None
        };
        // Visual snapshot (absolute/display positions) of everything
        // that might rearrange: any unfinished make-room glide then
        // completes smoothly across the drop instead of snapping.
        let dock_vis: Vec<(usize, f32)> = self
            .dock_order
            .iter()
            .enumerate()
            .take(layout.dock_slots.len())
            .filter(|(_, &e)| e != drag.entry_idx)
            .map(|(k, &e)| {
                let slot = &layout.dock_slots[k];
                let shift = self.dock_slide.get(k).copied().unwrap_or(0.0);
                (e, slot.x + slot.w / 2.0 + shift * slot.w)
            })
            .collect();
        debug!(
            "drop: id={id} kind={kind:?} from_dock={} insert={insert:?} section={section:?}",
            drag.from_dock
        );
        match kind {
            Some(apps::EntryKind::Package) => {
                if (section == Some(content::SECTION_APPS) || insert.is_some())
                    && !self.busy_ids.contains(&id)
                {
                    info!("installing {id}");
                    self.busy_ids.insert(id.clone());
                    self.nix.request(nix::Request::Install { attr: id });
                }
            }
            Some(apps::EntryKind::App)
                if section == Some(content::SECTION_INSTALL)
                    && self.removable_ids.contains(&id)
                    && !self.busy_ids.contains(&id) =>
            {
                // Uninstall — gated to profile-managed apps, so this only
                // runs for something `nix profile remove` can actually
                // remove (non-removable apps fall through and snap back).
                if let Some(path) = path.map(|p| p.to_string_lossy().into_owned()) {
                    info!("uninstalling {id}");
                    self.busy_ids.insert(id.clone());
                    self.nix.request(nix::Request::Remove {
                        id,
                        desktop_path: path,
                    });
                } else {
                    debug!("{id} has no desktop path; cannot uninstall");
                }
            }
            _ => {
                // Grid gestures: an app dropped on another app creates a
                // box, on a box joins it, in a gap reorders (boxes too).
                // A dock app dragged into the grid unpins and lands there
                // the same way. Dropping out of the grid (and off the
                // dock) is a no-op — the icon snaps back to its place.
                let boxed = released
                    && insert.is_none()
                    && matches!(
                        kind,
                        Some(apps::EntryKind::App) | Some(apps::EntryKind::Group)
                    )
                    && self.search.query.is_empty()
                    && self.handle_grid_drop(drag.entry_idx, &id, drag.pos);
                if !boxed && kind != Some(apps::EntryKind::Group) {
                    if let Some(slot) = insert {
                        // Dock reorder. Drop in the visible gap: the
                        // dock parts in compact coordinates (origin
                        // removed), so translate the raw slot.
                        let slot = if drag.from_dock {
                            let origin =
                                self.dock_order.iter().position(|&e| e == drag.entry_idx);
                            slot - usize::from(origin.is_some_and(|o| o < slot))
                        } else {
                            slot
                        };
                        // Everything left of the drop keeps its exact
                        // place: usage-filled slots there become
                        // explicit pins first (pin_at's index is
                        // pins-relative).
                        let prefix: Vec<String> = self
                            .dock_order
                            .iter()
                            .filter(|&&e| e != drag.entry_idx)
                            .take(slot)
                            .map(|&e| self.entries[e].id.clone())
                            .collect();
                        for (k, pid) in prefix.iter().enumerate() {
                            self.pins.pin_at(pid, k);
                        }
                        self.pins.pin_at(&id, slot);
                    }
                    // insert == None: dropped outside the dock band —
                    // no unpin, the icon just returns to the dock.
                }
            }
        }
        self.reorder_slot = None;
        self.reorder_dwell = None;
        self.recompute_dock_order();
        // Remap the dock glide onto the new arrangement *before*
        // anything draws: every icon keeps its exact current visual
        // position and eases to rest — nothing snaps at the drop, and
        // icons already at their seat (the common case) don't move at
        // all. (The grid gets the same continuity inside refilter.)
        let new_layout = self.current_layout();
        let n_dock = new_layout.dock_slots.len();
        self.dock_slide = vec![0.0; n_dock];
        for (k, &e) in self.dock_order.iter().take(n_dock).enumerate() {
            if let Some(&(_, cx)) = dock_vis.iter().find(|&&(ve, _)| ve == e) {
                let slot = &new_layout.dock_slots[k];
                self.dock_slide[k] = (cx - (slot.x + slot.w / 2.0)) / slot.w;
            }
        }
        // Magnification sleeps a full second so the icon simply *is*
        // placed before any wave returns.
        self.mag_sleep = Some(Instant::now() + MAG_SLEEP_AFTER_DROP);
        self.refilter();
    }

    /// The Apps display cell under `pos` plus the within-cell
    /// fractions, from static slot geometry (display slots never move;
    /// only items glide between them — that's what keeps targeting
    /// stable while everything animates).
    fn apps_display_cell(
        &self,
        layout: &content::Layout,
        pos: (f32, f32),
    ) -> Option<(usize, f32, f32)> {
        let sec = &layout.sections[content::SECTION_APPS];
        if !sec.viewport.contains(pos) || sec.n_pages == 0 {
            return None;
        }
        let page_w = sec.viewport.w.max(1.0);
        let ax = (pos.0 - sec.viewport.x + sec.scroll).max(0.0);
        let page = ((ax / page_w).floor() as usize) % sec.n_pages.max(1);
        let fx = (ax.rem_euclid(page_w)) / content::GRID_CELL_W;
        let fy = (pos.1 - sec.viewport.y) / content::GRID_CELL_H;
        let col = (fx.floor() as usize).min(sec.cols.saturating_sub(1));
        let row = (fy.floor() as usize).min(sec.rows.saturating_sub(1));
        let d = page * sec.cols * sec.rows + row * sec.cols + col;
        Some((d, fx.fract(), fy.fract().clamp(0.0, 1.0)))
    }

    /// Track the drag's grid target in display space. The make-room
    /// gap starts at the pickup slot (nothing moves on pickup); it
    /// only moves after the pointer *lingers* over a new slot
    /// ([`REORDER_DWELL`]) — that dwell is what makes folding onto a
    /// side neighbor possible at all: icons no longer dive out of the
    /// way the moment you approach them. Hovering an item rings it as
    /// a fold target immediately. Returns the fold target, if any;
    /// the gap itself lives in `self.reorder_slot`.
    fn update_grid_target(&mut self, layout: &content::Layout) -> Option<(usize, usize)> {
        let Some(drag) = self.gesture.dragging.as_ref() else {
            self.reorder_slot = None;
            self.reorder_dwell = None;
            return None;
        };
        let kind = self.kinds.get(drag.entry_idx).copied();
        let visible = &self.search.visible[content::SECTION_APPS];
        let (len, pos) = (visible.len(), drag.pos);
        let orig = visible.iter().position(|&v| v == drag.entry_idx);
        // Two ways to target the grid: a grid-origin app/box reordering
        // itself, or a dock-origin app dragged in to unpin it. The dock
        // app owns no cell, so its gap is a brand-new slot (nothing to
        // vacate) and it may land one past the end (append).
        let inserting = drag.from_dock;
        let grid_drag = self.search.query.is_empty()
            && if inserting {
                kind == Some(apps::EntryKind::App)
            } else {
                orig.is_some()
                    && len > 0
                    && matches!(
                        kind,
                        Some(apps::EntryKind::App) | Some(apps::EntryKind::Group)
                    )
            };
        if !grid_drag {
            self.reorder_slot = None;
            self.reorder_dwell = None;
            return None;
        }
        // The gap rests at the app's own cell (reorder — nothing moves
        // on pickup) or past the end (insert — the grid stays whole
        // until the pointer asks for a slot).
        let slot = *self.reorder_slot.get_or_insert(orig.unwrap_or(len));

        // Off the grid: a reorder leaves the gap where it was (you may
        // be reaching for the dock); an insert closes the grid back up.
        let Some((d, fx, fy)) = self.apps_display_cell(layout, pos) else {
            if inserting {
                self.reorder_slot = None;
            }
            self.reorder_dwell = None;
            return None;
        };
        let max_d = if inserting { len } else { len.saturating_sub(1) };
        let d = d.min(max_d);
        if d == slot {
            self.reorder_dwell = None;
            return None; // hovering the gap: stable
        }
        // The item shown at display cell `d`: hovering it rings a fold
        // target immediately (apps only; boxes never fold or nest).
        let compact = d - usize::from(d > slot);
        let full = compact + orig.map_or(0, |o| usize::from(compact >= o));
        if kind == Some(apps::EntryKind::App)
            && FOLD_BAND.contains(&fx)
            && (0.08..0.92).contains(&fy)
        {
            let foldable = self.search.visible[content::SECTION_APPS]
                .get(full)
                .is_some_and(|&t| match self.kinds.get(t) {
                    Some(apps::EntryKind::Group) => true,
                    Some(apps::EntryKind::App) => self.app_group.is_none(),
                    _ => false,
                });
            if foldable {
                self.reorder_dwell = None;
                return Some((content::SECTION_APPS, full));
            }
        }
        // Reordering: move the gap only after a dwell. Only a dock
        // insert gets the right-edge nudge (so it can append past the
        // last icon); a reorder moves the gap straight to the hovered
        // cell. The old unconditional nudge could leap the gap over a
        // neighbour, sliding two icons for a single step.
        let want = if inserting && fx >= 0.85 {
            (d + 1).min(max_d)
        } else {
            d
        };
        if want == slot {
            self.reorder_dwell = None;
            return None;
        }
        match self.reorder_dwell {
            Some((w, since)) if w == want => {
                if since.elapsed() >= REORDER_DWELL {
                    self.reorder_slot = Some(want);
                    self.reorder_dwell = None;
                }
                // Keep frames coming while the dwell clock runs.
                self.dirty = true;
            }
            _ => {
                self.reorder_dwell = Some((want, Instant::now()));
                self.dirty = true;
            }
        }
        None
    }

    /// Resolve a grid-origin drop; true when it was a grid gesture
    /// (fold / reorder / leave-box), false to fall through to pinning.
    fn handle_grid_drop(&mut self, entry_idx: usize, id: &str, pos: (f32, f32)) -> bool {
        let layout = self.current_layout();
        let kind = self.kinds.get(entry_idx).copied();
        let visible = &self.search.visible[content::SECTION_APPS];
        let len = visible.len();
        // A grid app/box knows its own cell; a dock app dragged in to
        // unpin has none (`orig` == None) and lands as a fresh insert.
        let orig = visible.iter().position(|&v| v == entry_idx);
        let inside = layout.sections[content::SECTION_APPS]
            .viewport
            .contains(pos);
        if !inside {
            // Dropped outside the grid (a dock app snaps back and stays
            // pinned; a grid member dragged out just returns): no-op.
            return false;
        }
        // A dock app that lands in the grid unpins as it does.
        if orig.is_none() {
            self.pins.exclude(id);
        }
        let slot = self.reorder_slot.unwrap_or(orig.unwrap_or(len));
        // Fold wins when the pointer sits on a foldable item's center.
        if kind == Some(apps::EntryKind::App) {
            if let Some((d, fx, _)) = self.apps_display_cell(&layout, pos) {
                let d = d.min(len.saturating_sub(1));
                if d != slot && FOLD_BAND.contains(&fx) {
                    let compact = d - usize::from(d > slot);
                    let full = compact + orig.map_or(0, |o| usize::from(compact >= o));
                    if let Some(&target_idx) = self.search.visible[content::SECTION_APPS].get(full)
                    {
                        let target_id = self.entries[target_idx].id.clone();
                        match self.kinds.get(target_idx) {
                            Some(apps::EntryKind::Group) => {
                                if let Some(g) = target_id
                                    .strip_prefix("group:")
                                    .and_then(|gid| self.groups.index_by_id(gid))
                                {
                                    self.groups.add(g, id);
                                    self.refilter();
                                    return true;
                                }
                            }
                            Some(apps::EntryKind::App) if self.app_group.is_none() => {
                                // The new box takes the target's grid
                                // position.
                                let box_id = self.groups.create(&target_id, id);
                                self.order
                                    .insert_before(&format!("group:{box_id}"), &target_id);
                                self.refilter();
                                return true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        // Otherwise the drop lands in the gap, wherever it is now.
        if let Some(g) = self.app_group {
            let n_members = self.groups.groups()[g].members.len();
            let full_before =
                (slot + usize::from(orig.is_some_and(|o| slot >= o))).min(n_members);
            self.groups.move_member(g, id, full_before);
        } else {
            // Boxes and apps share one order: anchor on the compacted
            // list (dragged removed) — the item displayed at the gap.
            let compact_ids: Vec<String> = self.search.visible[content::SECTION_APPS]
                .iter()
                .filter(|&&idx| idx != entry_idx)
                .map(|&idx| self.entries[idx].id.clone())
                .collect();
            let refs: Vec<&str> = compact_ids.iter().map(String::as_str).collect();
            self.order.move_within(id, &refs, slot);
        }
        self.refilter();
        true
    }

    /// The section that would accept the dragged entry if dropped at
    /// `pos` (drop-target highlight): Apps installs a package, Install
    /// uninstalls an app.
    fn drag_drop_section(
        &self,
        layout: &content::Layout,
        pos: (f32, f32),
        entry_idx: usize,
    ) -> Option<usize> {
        let section = content::section_at(layout, pos)?;
        match self.kinds.get(entry_idx) {
            Some(apps::EntryKind::Package) if section == content::SECTION_APPS => Some(section),
            // Only apps installed via the imperative profile can be
            // uninstalled here — home-manager / system apps don't offer
            // the target at all (dropping there just snaps back).
            Some(apps::EntryKind::App)
                if section == content::SECTION_INSTALL
                    && self
                        .entries
                        .get(entry_idx)
                        .is_some_and(|e| self.removable_ids.contains(&e.id)) =>
            {
                Some(section)
            }
            _ => None,
        }
    }

    /// Drop a pointer position left stale above the collapsed dock.
    ///
    /// When our input region shrinks out from under a motionless cursor
    /// this Hyprland sends no `wl_pointer.leave` (verified quirk), so a
    /// cursor parked over the open card is still reported as being on the
    /// dock after it collapses. That suppresses both the auto-hide guard
    /// and intellihide's dodge (`pointer_pos.is_none()`), leaving the dock
    /// parked visible until a real mouse motion. On settle, if the last
    /// known pointer sits above the live input region, treat it as gone.
    fn reconcile_stale_pointer(&mut self) {
        let (Some((_, y)), Some(extent)) = (self.pointer_pos, self.input_extent) else {
            return;
        };
        let region_top = self.buffer_size.1 as f32 - extent as f32;
        if y >= region_top {
            return; // genuinely within the dock's live region
        }
        self.pointer_pos = None;
        self.hover = None;
    }

    /// What the pointer is over right now (`None` when outside).
    fn hover_at_pointer(&self) -> Option<Hit> {
        self.pointer_pos
            .and_then(|pos| content::hit_test(&self.current_layout(), pos, self.search.open))
    }

    /// Recompute which item the pointer is over; redraw on change.
    fn update_hover(&mut self) {
        let hover = self.hover_at_pointer();
        if hover != self.hover {
            debug!(
                "hover: {:?} -> {:?} at {:?} (extent {})",
                self.hover,
                hover,
                self.pointer_pos,
                self.ui.extent()
            );
            self.hover = hover;
            self.schedule_frame();
        }
    }

    /// Resolve a hit to an action: launch an entry or toggle the search box.
    fn activate_hit(&mut self, hit: Hit) {
        match hit {
            Hit::DockIcon(slot) => {
                if let Some(&entry_idx) = self.dock_order.get(slot) {
                    self.activate(entry_idx);
                }
            }
            Hit::GridCell(s, i) => {
                if let Some(entry_idx) = self.search.visible[s].get(i).copied() {
                    // Folders in the Files section navigate instead of
                    // launching (the popup stays open); boxes in the
                    // Apps grid open the same way.
                    if s == content::SECTION_FILES && self.try_navigate(entry_idx) {
                        return;
                    }
                    if self.kinds.get(entry_idx) == Some(&apps::EntryKind::Group) {
                        let g = self
                            .entries
                            .get(entry_idx)
                            .and_then(|e| e.id.strip_prefix("group:"))
                            .and_then(|gid| self.groups.index_by_id(gid));
                        if let Some(g) = g {
                            self.open_group(g);
                        }
                        return;
                    }
                    self.activate(entry_idx);
                }
            }
            Hit::SearchButton => {
                self.search.open = !self.search.open;
                if !self.search.open {
                    self.search.query.clear();
                    self.refilter();
                }
                self.schedule_frame();
            }
            Hit::FilesBack => self.files_nav_up(),
            Hit::AppsBack => self.close_group(),
        }
    }

    /// Get out of the way. From the open box, retreat to the dock first
    /// so it rests a beat there before hiding (the rest-then-hide timer
    /// finishes the job); from the dock, hide outright — unless nothing
    /// overlaps the zone, where intellihide keeps the dock parked.
    fn dismiss(&mut self) {
        let command = if self.ui.target() == Target::Open || self.zone_free {
            Command::Collapse
        } else {
            Command::Hide
        };
        self.handle_command(command);
    }

    /// Launch an entry by index; its icon plays a bounce (macOS launch
    /// feedback), then the card gets out of the way.
    fn activate(&mut self, index: usize) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        // Packages aren't launchable — installing is a drag to the Apps
        // section, not a click.
        if self.kinds.get(index) == Some(&apps::EntryKind::Package) {
            return;
        }
        let (exec, id) = (entry.exec.clone(), entry.id.clone());
        let needs_terminal = entry.needs_terminal;
        if let Err(e) = launch::launch(&exec, needs_terminal, &self.config.launch.terminal) {
            error!("launch failed for {id}: {e:#}");
        }
        self.usage.increment(&id);
        // Hand keyboard focus to the launched app. Drop our exclusive
        // grab now (before its window maps) and clear the return-focus
        // target so we don't pull focus back to the window we came from;
        // then focus the app's window the moment it opens — the
        // compositor won't with focus-follows-mouse if the cursor is
        // parked off it (Chrome and other slow starters especially).
        self.restore_window = None;
        self.focus_launched = Some(Instant::now());
        if self.interactive {
            surface::set_interactive(&self.layer, false);
            self.interactive = false;
        }
        self.bounce = Some((index, Instant::now()));
        self.schedule_frame();
        let timer = Timer::from_duration(BOUNCE_DURATION);
        if let Err(e) = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.dismiss();
                TimeoutAction::Drop
            })
        {
            warn!("failed to arm launch-hide timer: {e}");
            self.dismiss();
        }
    }

    /// Current upward offset of the launch bounce, expiring it when
    /// done: two hops decaying in height.
    fn bounce_offset(&mut self) -> Option<(usize, f32)> {
        let (index, start) = self.bounce?;
        let t = start.elapsed().as_secs_f32() / BOUNCE_DURATION.as_secs_f32();
        if t >= 1.0 {
            self.bounce = None;
            return None;
        }
        let hops = (2.0 * std::f32::consts::PI * t).sin().abs();
        Some((index, BOUNCE_HEIGHT * hops * (1.0 - 0.45 * t)))
    }

    /// Push keyboard interactivity and the pointer input region to the
    /// compositor whenever the targeted rest state changes. The input
    /// region covers the *target* rect immediately so the dock/popup is
    /// interactive without waiting for the slide to finish.
    fn sync_surface_state(&mut self) {
        if self.ui.target() == Target::Hidden {
            // Fresh card next time it rises.
            self.scroll.reset_sections();
            self.hover = None;
            self.search.open = false;
            if self.files_dir.take().is_some() {
                self.refilter();
            }
        }
        if self.ui.target() == Target::Open {
            self.scroll.reset_sections();
            // open_at is set only when expand was triggered by a scroll gesture
            // (in on_scroll), so keyboard Toggle has no cooldown.
        }
        // The search only lives while the popup is open.
        if self.ui.target() != Target::Open && !self.search.query.is_empty() {
            self.search.query.clear();
            self.search.open = false;
            self.refilter();
        }
        let interactive = self.ui.wants_keyboard();
        if interactive != self.interactive {
            if interactive {
                // Grabbing the keyboard for type-to-search steals focus
                // from whatever window has it — remember it so a plain
                // close can hand focus back.
                self.restore_window = hypr::active_window();
                debug!("grab keyboard; restore target = {:?}", self.restore_window);
                surface::set_interactive(&self.layer, true);
            } else {
                // Release the grab. Handing focus back to where we came
                // from happens in the keyboard `leave` handler, which
                // fires once the compositor has actually taken our
                // keyboard away — dispatching a focus before that races
                // the release and Hyprland drops it.
                surface::set_interactive(&self.layer, false);
            }
            self.interactive = interactive;
        }

        self.sync_input_region();
    }

    /// Size the pointer input region to whatever is currently visible,
    /// not just the target rest point: while a hide slides the dock down
    /// the still-visible bar must keep taking input (so returning to it
    /// re-summons), and once it settles the region must shrink back to
    /// the reveal strip. Run every frame while animating *and* on target
    /// change, since the visible extent moves between those.
    fn sync_input_region(&mut self) {
        let mut extent = self
            .ui
            .extent_of(self.ui.target())
            .max(self.ui.extent())
            .round() as u32;
        // While hidden, keep a thin strip alive at the bottom edge so
        // touching it with the pointer can reveal the dock.
        if self.ui.target() == Target::Hidden && self.config.input.edge_reveal {
            extent = extent.max(self.config.input.edge_reveal_px);
        }
        if self.input_extent != Some(extent) {
            match surface::set_input_extent(
                &self.compositor,
                &self.layer,
                self.buffer_size,
                extent,
                content::DRAG_MARGIN_X as u32,
            ) {
                Ok(()) => self.input_extent = Some(extent),
                Err(e) => warn!("failed to set input region: {e:#}"),
            }
        }
    }

    /// Mark the scene damaged. Draws immediately when no frame callback
    /// is in flight; otherwise the damage is coalesced onto the pending
    /// callback, so redraw rate never exceeds the display refresh no
    /// matter how fast input events arrive.
    fn schedule_frame(&mut self) {
        if self.renderer.is_none() {
            debug!("frame requested before first configure; deferring");
            return;
        }
        self.dirty = true;
        if !self.frame_pending {
            self.last_frame = None; // waking from idle: don't count idle time as dt
            self.draw();
        }
    }

    /// Render one frame and request the next frame callback, which rides
    /// on this frame's commit. The callback redraws only if the scene is
    /// animating or damaged again — otherwise it fires clean and the
    /// daemon goes fully idle (no further frame requests).
    fn draw(&mut self) {
        if self.renderer.is_none() {
            return;
        }

        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(1.0 / 60.0);
        self.last_frame = Some(now);

        let was_animating = self.ui.is_animating();
        let animating = self.ui.tick(dt);
        if animating {
            // The card is moving under a possibly stationary pointer:
            // keep the hover highlight glued to what is really beneath it.
            // (Not update_hover(): its schedule_frame would recurse here.)
            self.hover = self.hover_at_pointer();
        }
        if was_animating && !animating {
            // Settled: correct the input region to the final rest point.
            // Doing this only on settle (not every frame) avoids a
            // set-region + surface commit per frame of the slide, which
            // stuttered the animation; the region set at the transition
            // start already covers the visible card for the whole move.
            self.sync_input_region();
            // The region just shrank; a cursor parked over the old card
            // gets no leave from this compositor, so clear it now or the
            // dock stays stuck visible (no dodge / no auto-hide) until the
            // mouse moves.
            self.reconcile_stale_pointer();
            // The layer just shrank off the cursor and stopped committing
            // frames. Force focus back to the window we opened over now:
            // this is the quiet moment where a forced re-bind of the
            // keyboard seat won't be clobbered by follow_mouse or an
            // in-flight surface commit.
            if let Some(addr) = self.pending_refocus.take() {
                debug!("settled ({:?}); forcing focus to {addr}", self.ui.target());
                hypr::focus_window(&addr);
            }
            // A box close settling to the dock rests a beat, then hides
            // (the timer's guard keeps it if the pointer is on the dock).
            if self.rest_hide_pending && self.ui.target() == Target::Dock {
                self.rest_hide_pending = false;
                self.schedule_hide_after(DOCK_REST_AFTER_CLOSE);
            }
        }

        // Advance search-box expand animation (200 ms).
        let search_target = if self.search.open { 1.0f32 } else { 0.0 };
        let search_animating = self.search.expand != search_target;
        if search_animating {
            let delta = dt / 0.2;
            self.search.expand = if search_target > self.search.expand {
                (self.search.expand + delta).min(1.0)
            } else {
                (self.search.expand - delta).max(0.0)
            };
        }

        // Smooth-scroll each section: pos eases toward target with an
        // exponential decay (~200 ms to settle at 60 fps).
        let mut scroll_animating = false;
        for sec in &mut self.scroll.per {
            let delta = sec.target - sec.pos;
            if delta.abs() > 0.5 {
                scroll_animating = true;
                let k = 1.0 - (-dt * 12.0f32).exp();
                sec.pos += delta * k;
            } else if delta != 0.0 {
                sec.pos = sec.target;
            }
        }

        self.dirty = false;

        let wl_surface = self.layer.wl_surface();
        wl_surface.frame(&self.qh, wl_surface.clone());
        self.frame_pending = true;

        let bounce = self.bounce_offset();
        let layout = self.current_layout();
        // (layout.scroll is the cyclic-wrapped image of list_scroll; the
        // raw value is what animates, so never sync it back from layout.)
        // Grid drag: track the make-room gap under the pointer and the
        // fold target (ring); remember which cell to hide (the ghost
        // is its visual).
        let over_cell = self.update_grid_target(&layout);
        // The icon in hand is the ghost: hide its resting cell — the
        // grid cell for grid-origin drags, the dock slot for
        // dock-origin ones.
        let dock_hidden = self
            .gesture
            .dragging
            .as_ref()
            .filter(|d| d.from_dock)
            .map(|d| d.entry_idx);
        let drag_hidden = self.gesture.dragging.as_ref().and_then(|drag| {
            (!drag.from_dock
                && matches!(
                    self.kinds.get(drag.entry_idx),
                    Some(apps::EntryKind::App) | Some(apps::EntryKind::Group)
                ))
            .then_some(drag.entry_idx)
        });
        // Make-room glide: each Apps cell eases toward its display
        // slot (the gap starts at the pickup slot, so nothing moves
        // until the pointer asks for room). ~90 ms exponential
        // ease-out — crisp, no overshoot.
        let apps_len = self.search.visible[content::SECTION_APPS].len();
        if self.apps_slide.len() != apps_len {
            self.apps_slide = (0..apps_len).map(|i| i as f32).collect();
        }
        let orig_pos = drag_hidden.and_then(|e| {
            self.search.visible[content::SECTION_APPS]
                .iter()
                .position(|&v| v == e)
        });
        // A dock app dragged into the grid opens a brand-new slot (it
        // has no cell to vacate): cells at or past the gap slide down
        // one to make room — like a reorder, but leaving no hole behind.
        let insert_gap = self
            .gesture
            .dragging
            .as_ref()
            .filter(|d| {
                d.from_dock
                    && matches!(self.kinds.get(d.entry_idx), Some(apps::EntryKind::App))
            })
            .and(self.reorder_slot);
        let mut slide_animating = false;
        let k = 1.0 - (-dt * MAKEROOM_RATE).exp();
        for i in 0..apps_len {
            let mut target = i as f32;
            if let Some(op) = orig_pos {
                if i != op {
                    // Items display at their compacted index, stepping
                    // over the gap wherever it currently sits.
                    let compact = if i > op { i - 1 } else { i };
                    let gap = self.reorder_slot.unwrap_or(op);
                    target = (compact + usize::from(compact >= gap)) as f32;
                }
            } else if let Some(gap) = insert_gap {
                target = (i + usize::from(i >= gap)) as f32;
            }
            let cur = self.apps_slide[i];
            if (cur - target).abs() > 0.005 {
                self.apps_slide[i] = cur + (target - cur) * k;
                slide_animating = true;
            } else {
                self.apps_slide[i] = target;
            }
        }
        // Dock make-room glide, same idea in dock-slot units: dragging
        // over the dock parts the icons around the insertion point;
        // a dock-origin drag's gap rests at its old slot meanwhile.
        let dock_insert_now =
            self.gesture
                .dragging
                .as_ref()
                .and_then(|d| match self.kinds.get(d.entry_idx) {
                    Some(apps::EntryKind::App) | Some(apps::EntryKind::File) => {
                        self.drag_dock_insert(&layout, d.pos)
                    }
                    _ => None,
                });
        let n_dock = layout.dock_slots.len();
        if self.dock_slide.len() != n_dock {
            self.dock_slide = vec![0.0; n_dock];
        }
        let dock_origin = self
            .gesture
            .dragging
            .as_ref()
            .filter(|d| d.from_dock)
            .and_then(|d| self.dock_order.iter().position(|&e| e == d.entry_idx))
            .filter(|&o| o < n_dock);
        for kk in 0..n_dock {
            let target = match (dock_insert_now, dock_origin) {
                (Some(g), Some(o)) => {
                    if kk == o {
                        0.0
                    } else {
                        let compact = if kk > o { kk - 1 } else { kk };
                        let g_c = g - usize::from(o < g);
                        (compact + usize::from(compact >= g_c)) as f32 - kk as f32
                    }
                }
                // A foreign icon hovers: part the row around the slot.
                (Some(g), None) => {
                    if kk >= g {
                        0.5
                    } else {
                        -0.5
                    }
                }
                _ => 0.0,
            };
            let cur = self.dock_slide[kk];
            if (cur - target).abs() > 0.005 {
                self.dock_slide[kk] = cur + (target - cur) * k;
                slide_animating = true;
            } else {
                self.dock_slide[kk] = target;
            }
        }
        if slide_animating {
            self.dirty = true;
        }
        // Magnification is dead while dragging and stays dead for a
        // beat after a drop — the landing must be perfectly still.
        // It never pops back either: an amplitude envelope fades it
        // in over ~350 ms once the sleep ends (and out on drag start).
        if let Some(until) = self.mag_sleep {
            if Instant::now() >= until {
                self.mag_sleep = None;
            } else {
                // Keep frames coming so the wake-up isn't missed.
                self.dirty = true;
            }
        }
        let mag_target = if self.gesture.dragging.is_none() && self.mag_sleep.is_none() {
            1.0f32
        } else {
            0.0
        };
        if (self.mag_amount - mag_target).abs() > 0.005 {
            let step = dt / 0.35;
            self.mag_amount = if mag_target > self.mag_amount {
                (self.mag_amount + step).min(1.0)
            } else {
                // Fading out fast keeps drag starts crisp.
                (self.mag_amount - step * 3.0).max(0.0)
            };
            self.dirty = true;
        } else {
            self.mag_amount = mag_target;
        }
        let mag_pointer = if self.mag_amount > 0.0 {
            self.pointer_pos
        } else {
            None
        };

        // Box open/close transition (~180 ms, eased below).
        if self.group_anim != self.group_anim_target {
            let step = dt / 0.18;
            if self.group_anim_target > self.group_anim {
                self.group_anim = (self.group_anim + step).min(self.group_anim_target);
            } else {
                self.group_anim = (self.group_anim - step).max(self.group_anim_target);
            }
            if self.group_anim <= 0.0 && self.group_anim_target <= 0.0 {
                // Fully collapsed back into the tile: leave the box.
                self.app_group = None;
                self.group_origin = None;
                self.group_anim = 1.0;
                self.group_anim_target = 1.0;
                self.refilter();
            } else {
                self.dirty = true;
            }
        }
        let group_expand = {
            let t = self.group_anim.clamp(0.0, 1.0);
            1.0 - (1.0 - t).powi(3) // ease-out cubic
        };

        let drag_frame = self
            .gesture
            .dragging
            .as_ref()
            .map(|drag| content::DragFrame {
                entry_idx: drag.entry_idx,
                pos: drag.pos,
                drop_section: self.drag_drop_section(&layout, drag.pos, drag.entry_idx),
                over_cell,
            });
        let busy: Vec<bool> = self
            .entries
            .iter()
            .map(|e| self.busy_ids.contains(&e.id))
            .collect();
        let failed: Vec<bool> = self
            .entries
            .iter()
            .map(|e| {
                self.failed_ids
                    .get(&e.id)
                    .is_some_and(|t| t.elapsed() < FAIL_FLASH)
            })
            .collect();
        let scene = content::scene(
            &self.config,
            &layout,
            &self.entries,
            &self.search.visible,
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            &content::FrameInput {
                // Suppress hover highlight and magnification while dragging.
                hover: if drag_frame.is_none() {
                    self.hover
                } else {
                    None
                },
                alpha: self.ui.alpha(),
                pointer: mag_pointer,
                mag_amount: self.mag_amount,
                bounce,
                query: &self.search.query,
                selected: self.search.selected.and_then(|i| self.flat_to_pos(i)),
                search_expand: self.search.expand,
                placeholders: &self.placeholders,
                layers: &self.icon_layers,
                files_path: &self.files_path_display(),
                dock_order: &self.dock_order,
                drag: drag_frame,
                install_hint: self.install_hint(),
                busy: &busy,
                failed: &failed,
                group_minis: &self.group_minis,
                apps_group: &self.apps_group_name(),
                apps_slide: &self.apps_slide,
                drag_hidden,
                dock_hidden,
                dock_slide: &self.dock_slide,
                group_expand,
                group_origin: self.group_origin,
            },
        );
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Err(e) = renderer.render(&scene, self.config.theme.text_rgba()) {
            error!("render failed: {e:#}");
        }
        if search_animating && self.search.expand != search_target {
            self.dirty = true;
        }
        if scroll_animating
            && self
                .scroll
                .per
                .iter()
                .any(|sec| (sec.target - sec.pos).abs() > 0.5)
            && self.ui.target() == Target::Open
        {
            self.dirty = true;
        }
    }

    /// One vertical-scroll step of `value` axis units.
    ///
    /// Hidden with the pointer on the reveal strip, scrolling up summons
    /// the dock (the compositor keeps our stale pointer focus when the
    /// dock hides under a parked cursor, so this — not a re-enter — is
    /// the recovery path). Docked, the wheel is the expand gesture, so
    /// one continuous scroll rides Hidden → Dock → Open. Open, it pages
    /// the grid (down = next, up = previous) and never collapses the
    /// popup — dismissal is Escape / pointer-leave / toggle only.
    fn on_scroll(&mut self, value: f64) {
        let target = self.ui.target();
        match target {
            Target::Hidden | Target::Dock => {
                // While hidden, only in-strip scroll may summon (stale
                // focus can also deliver events from anywhere the open
                // card used to be — a scroll there belongs to the window
                // beneath).
                if target == Target::Hidden && !self.pointer_on_reveal_strip() {
                    return;
                }
                self.scroll.accum += value;
                let mut toward_open = self.scroll.accum;
                if self.config.input.natural_scroll {
                    toward_open = -toward_open;
                }
                if toward_open <= -SCROLL_THRESHOLD {
                    self.scroll.accum = 0.0;
                    if target == Target::Dock {
                        // Mark that expand was triggered by scroll so the
                        // bleed-through events are eaten until the gesture
                        // ends (AxisStop clears this).
                        self.scroll.open_at = Some(Instant::now());
                        self.handle_command(Command::Expand);
                    } else {
                        // No bleed cooldown: the rest of the gesture
                        // should keep accumulating toward Expand.
                        self.handle_command(Command::Show);
                    }
                } else if toward_open >= SCROLL_THRESHOLD {
                    // Scrolling away from expand: nothing to do, just
                    // keep the accumulator bounded.
                    self.scroll.accum = 0.0;
                }
            }
            Target::Open => {
                if self
                    .scroll
                    .open_at
                    .is_some_and(|t| t.elapsed() < EXPAND_BLEED_COOLDOWN)
                {
                    return;
                }
                // Page the section under the pointer; each section
                // scrolls independently.
                if let Some(section) = self
                    .pointer_pos
                    .and_then(|pos| content::section_at(&self.current_layout(), pos))
                {
                    self.page_scroll(section, value);
                }
            }
        }
    }

    /// Whether the pointer is on the edge-reveal strip (the bottom
    /// `edge_reveal_px` of the surface). False when edge reveal is off.
    fn pointer_on_reveal_strip(&self) -> bool {
        if !self.config.input.edge_reveal {
            return false;
        }
        let strip_top = self.buffer_size.1 as f32 - self.config.input.edge_reveal_px as f32;
        self.pointer_pos.is_some_and(|(_, y)| y >= strip_top)
    }

    /// Horizontal scroll: pages the section under the pointer.
    /// (Untestable in the dev VM — its virtual pointer has no horizontal
    /// axis; verify on real hardware.)
    fn on_hscroll(&mut self, value: f64) {
        if self.ui.target() == Target::Open {
            if let Some(section) = self
                .pointer_pos
                .and_then(|pos| content::section_at(&self.current_layout(), pos))
            {
                self.page_scroll(section, value);
            }
        }
    }

    /// Accumulate scroll toward a page turn of `section`: turning
    /// requires PAGE_SCROLL_THRESHOLD worth of travel, and successive
    /// turns are at least PAGE_COOLDOWN apart — so one notch nudges, a
    /// deliberate scroll turns one page, and a fast flick can't spin
    /// the wheel.
    fn page_scroll(&mut self, section: usize, value: f64) {
        let sec = &mut self.scroll.per[section];
        if sec
            .page_turned_at
            .is_some_and(|t| t.elapsed() < PAGE_COOLDOWN)
        {
            return;
        }
        // A direction change discards progress toward the old direction.
        if value * sec.page_accum < 0.0 {
            sec.page_accum = 0.0;
        }
        sec.page_accum += value;
        if sec.page_accum.abs() >= PAGE_SCROLL_THRESHOLD {
            let dir = if sec.page_accum > 0.0 { 1 } else { -1 };
            sec.page_accum = 0.0;
            sec.page_turned_at = Some(Instant::now());
            self.page_by(section, dir);
        }
    }

    /// Slide one section's grid a page in `dir` (+1 = next, -1 =
    /// previous), wrapping past either end (infinite scroll).
    fn page_by(&mut self, section: usize, dir: i64) {
        // Use the SETTLED (full-extent) layout: mid-open-animation the
        // current layout has a tiny viewport and a bogus page count.
        let settled = self.layout_at(self.ui.extent_of(Target::Open));
        let sec_layout = &settled.sections[section];
        let page_w = sec_layout.viewport.w.max(1.0);
        let n_pages = sec_layout.n_pages;
        if n_pages <= 1 {
            return;
        }
        let total_w = n_pages as f32 * page_w;
        let sec = &mut self.scroll.per[section];
        // Use the target (intended page) not the animated position so
        // mid-animation events don't mis-compute the page.
        let current_page = (sec.target / page_w).round() as i64;
        let next_page = current_page + dir;
        if next_page < 0 {
            // Wrap to the last page. Shift the animated position one full
            // strip right so the slide still moves in the gesture
            // direction; rendering is cyclic, so the shift is invisible.
            sec.pos += total_w;
            sec.target = (n_pages - 1) as f32 * page_w;
        } else if next_page >= n_pages as i64 {
            // Wrap to the first page (shift one strip left, as above).
            sec.pos -= total_w;
            sec.target = 0.0;
        } else {
            sec.target = next_page as f32 * page_w;
        }
        self.update_hover();
        self.schedule_frame();
    }

    fn handle_key_event(&mut self, keysym: Keysym, utf8: Option<&str>) {
        match keysym {
            Keysym::Escape => {
                // Step out of an open box first; dismiss on the next.
                if self.app_group.is_some() && self.search.query.is_empty() {
                    self.close_group();
                    return;
                }
                self.search.query.clear();
                self.search.open = false;
                self.refilter();
                self.dismiss();
            }
            Keysym::Return | Keysym::KP_Enter => {
                if let Some((s, i)) = self.search.selected.and_then(|i| self.flat_to_pos(i)) {
                    self.activate_hit(Hit::GridCell(s, i));
                }
            }
            Keysym::BackSpace => {
                if self.search.query.pop().is_some() {
                    if self.search.query.is_empty() {
                        self.search.open = false;
                    }
                    self.refilter();
                }
            }
            Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down => {
                if self.flat_len() > 0 {
                    let layout = self.current_layout();
                    let cols = layout.sections[content::SECTION_APPS].cols.max(1);
                    let last = self.flat_len() - 1;
                    // When nothing is selected yet, the first arrow key
                    // lands on item 0 regardless of direction. Selection
                    // walks the sections as one flat list (Apps → Files).
                    let next = if let Some(cur) = self.search.selected {
                        match keysym {
                            Keysym::Left => cur.saturating_sub(1),
                            Keysym::Right => (cur + 1).min(last),
                            Keysym::Up => cur.saturating_sub(cols),
                            _ => (cur + cols).min(last),
                        }
                    } else {
                        0
                    };
                    self.search.selected = Some(next);
                    if let Some((s, cell)) = self.flat_to_pos(next) {
                        self.scroll.per[s].target =
                            content::scroll_to_reveal(&layout.sections[s], cell);
                    }
                    self.schedule_frame();
                }
            }
            _ => {
                if let Some(text) = utf8 {
                    let printable: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !printable.is_empty() {
                        self.search.open = true;
                        self.search.query.push_str(&printable);
                        self.refilter();
                    }
                }
            }
        }
    }

    fn schedule_autohide(&mut self) {
        let delay = Duration::from_millis(u64::from(self.config.input.autohide_delay_ms));
        self.schedule_hide_after(delay);
    }

    /// Arm a one-shot hide after `delay` (unless the pointer is back on
    /// the dock when it fires). A later call supersedes an earlier one.
    fn schedule_hide_after(&mut self, delay: Duration) {
        let deadline = Instant::now() + delay;
        self.hide_deadline = Some(deadline);
        let timer = Timer::from_duration(delay);
        if let Err(e) = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.hide_deadline == Some(deadline) {
                    app.hide_deadline = None;
                    // The pointer may be back on the dock without a fresh
                    // Enter to have cancelled us (stale focus) — don't hide
                    // out from under it.
                    if !matches!(app.hover_at_pointer(), Some(Hit::DockIcon(_))) {
                        // Fully hide, or just fall back to the parked dock
                        // when the zone is free (intellihide).
                        app.dismiss();
                    }
                }
                TimeoutAction::Drop
            })
        {
            warn!("failed to arm auto-hide timer: {e}");
            self.hide_deadline = None;
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        // Known risk area: fractional scaling. Integer scale only for now;
        // wp-fractional-scale-v1 is P5 polish.
        info!("scale factor: {new_factor}");
        self.scale_factor = new_factor;
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        self.frame_pending = false;
        if self.ui.is_animating() || self.bounce.is_some() || self.dirty {
            self.draw();
        } else {
            self.last_frame = None;
            debug!("settled in {:?}, going idle", self.ui.target());
        }
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (mut width, mut height) = configure.new_size;
        if width == 0 {
            width = self.config.window.width + 2 * content::DRAG_MARGIN_X as u32;
        }
        if height == 0 {
            height = self.config.window.height
                + self.config.window.bottom_margin
                + content::MAGNIFY_HEADROOM as u32
                + content::DRAG_MARGIN_TOP as u32;
        }
        debug!("configure: {width}x{height}");
        self.buffer_size = (width, height);

        match self.renderer.as_mut() {
            Some(renderer) => renderer.resize(width, height),
            None => match Renderer::new(&self.conn, self.layer.wl_surface(), width, height) {
                Ok(mut renderer) => {
                    if let Some(icons) = self.pending_icons.take() {
                        renderer.set_icons(&icons);
                    }
                    self.renderer = Some(renderer);
                    self.upload_pkg_icons();
                }
                Err(e) => {
                    error!("renderer init failed: {e:#}");
                    self.exit = true;
                    return;
                }
            },
        }
        // The buffer size is now known: restrict pointer input to the
        // visible content (empty region while hidden = click-through).
        self.input_extent = None;
        self.sync_surface_state();
        // First (and per-configure) draw: layer-shell requires a commit in
        // response to configure; presenting a frame satisfies it.
        self.draw();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let lh = self.loop_handle.clone();
            match self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                lh,
                Box::new(
                    |app: &mut App, _kb: &wl_keyboard::WlKeyboard, event: KeyEvent| {
                        app.handle_key_event(event.keysym, event.utf8.as_deref());
                    },
                ),
            ) {
                Ok(keyboard) => self.keyboard = Some(keyboard),
                Err(e) => warn!("cannot get keyboard: {e}"),
            }
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            // Raw wl_pointer (see the Dispatch impl below for why sctk's
            // frame-batched pointer helper is not used).
            self.pointer = Some(seat.get_pointer(qh, ()));
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        debug!("keyboard focus gained");
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        debug!(
            "keyboard focus lost; target={:?}, restore={:?}",
            self.ui.target(),
            self.restore_window
        );
        if self.ui.target() == Target::Open {
            // Still open when the keyboard left us: the user focused
            // another window (alt-tab, click elsewhere). Respect their
            // choice — drop the return target and collapse to the dock.
            self.restore_window = None;
            self.handle_command(Command::Collapse);
        } else {
            // We initiated the close. Don't refocus yet: the card is
            // still a full-size layer over the cursor and mid-animation,
            // so a focus now gets clobbered by follow_mouse. Defer it to
            // the settle, when the layer has shrunk out from under the
            // pointer and gone quiet.
            self.pending_refocus = self.restore_window.take();
        }
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key_event(event.keysym, event.utf8.as_deref());
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _modifiers: Modifiers,
        _layout: u32,
    ) {
    }
}

/// Raw `wl_pointer` dispatch — deliberately *not* sctk's `PointerHandler`.
///
/// sctk batches pointer events and only delivers them when a
/// `wl_pointer.frame` event arrives, but this compositor (Hyprland) only
/// sends `frame` alongside enter/leave/button: plain motion arrives
/// frameless and would sit buffered forever, freezing hover on whatever
/// the enter event hit first. Processing each event as it arrives is
/// what the mainstream toolkits do and works on both behaviors.
impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(
        app: &mut Self,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            } => {
                // Any pending auto-hide is off: the pointer is back.
                app.hide_deadline = None;
                app.pointer_pos = Some((surface_x as f32, surface_y as f32));
                // While hidden, only the edge-reveal strip is
                // pointer-sensitive, so entering means "summon".
                if app.ui.target() == Target::Hidden && app.config.input.edge_reveal {
                    app.handle_command(Command::Show);
                }
                app.update_hover();
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                let pos = (surface_x as f32, surface_y as f32);
                app.pointer_pos = Some(pos);
                // Hiding under a parked cursor keeps our (now stale)
                // pointer focus — the compositor never re-sends Enter for
                // the reveal strip. Treat in-strip motion as the edge
                // touch it is, or the strip stays dead until the pointer
                // fully leaves and returns.
                // Summon back from the reveal strip, or from the dock
                // itself while it is mid-hide: as the input region shrinks
                // during a hide this Hyprland keeps stale pointer focus and
                // sends motion, never a fresh Enter, so a pointer wandering
                // back onto the sliding dock would otherwise be ignored and
                // it would vanish out from under the cursor.
                if app.ui.target() == Target::Hidden
                    && (app.pointer_on_reveal_strip()
                        || matches!(app.hover_at_pointer(), Some(Hit::DockIcon(_))))
                {
                    app.hide_deadline = None;
                    app.handle_command(Command::Show);
                }
                // Detect drag start: press armed and pointer moved beyond
                // the 6-px threshold (distinguishes drag from sloppy click).
                if app.gesture.dragging.is_none() {
                    if let (Some(pp), Some(hit)) = (app.gesture.press_pos, app.gesture.pressed) {
                        let dx = pos.0 - pp.0;
                        let dy = pos.1 - pp.1;
                        if dx * dx + dy * dy > 6.0 * 6.0 {
                            let entry_idx = match hit {
                                Hit::DockIcon(slot) => app.dock_order.get(slot).copied(),
                                Hit::GridCell(s, cell) => app.search.visible[s].get(cell).copied(),
                                Hit::SearchButton | Hit::FilesBack | Hit::AppsBack => None,
                            };
                            // Cells with a profile mutation in flight
                            // can't start a new drag.
                            let undraggable = entry_idx.is_some_and(|i| {
                                app.entries
                                    .get(i)
                                    .is_some_and(|e| app.busy_ids.contains(&e.id))
                            });
                            if let (Some(entry_idx), false) = (entry_idx, undraggable) {
                                let from_dock = matches!(hit, Hit::DockIcon(_));
                                app.gesture.dragging = Some(DragState {
                                    entry_idx,
                                    from_dock,
                                    pos,
                                });
                                app.gesture.pressed = None;
                            }
                        }
                    }
                } else if let Some(ref mut drag) = app.gesture.dragging {
                    drag.pos = pos;
                }
                if app.ui.target() != Target::Hidden {
                    if app.gesture.dragging.is_none() {
                        app.update_hover();
                    }
                    app.schedule_frame();
                }
            }
            wl_pointer::Event::Leave { .. } => {
                app.scroll.accum = 0.0;
                app.pointer_pos = None;
                app.gesture.pressed = None;
                app.gesture.press_pos = None;
                // If a dock drag is in flight when the pointer leaves the
                // surface, treat it as a drop outside the dock: unpin and
                // leave the popup open (don't autohide).
                if let Some(drag) = app.gesture.dragging.take() {
                    app.drop_drag(drag, None, false);
                    return; // skip autohide — keep popup open
                }
                app.update_hover();
                if app.ui.target() != Target::Hidden {
                    app.schedule_frame(); // relax any magnification
                    if app.config.input.autohide {
                        if app.ui.target() == Target::Open {
                            // Full popup is up and the pointer left — the user
                            // clicked or moved to another window. Dismiss now;
                            // no grace period needed (the card is fully open,
                            // not just a slim dock sliver to accidentally graze).
                            app.dismiss();
                        } else if !app.rest_hide_pending {
                            // A close settling to the dock owns the hide
                            // (its rest); don't undercut it with the
                            // shorter autohide grace.
                            app.schedule_autohide();
                        }
                    }
                }
            }
            wl_pointer::Event::Button { button, state, .. } if button == BTN_LEFT => {
                match state {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                        app.update_hover();
                        app.gesture.pressed = app.hover;
                        app.gesture.press_pos = app.pointer_pos;
                    }
                    WEnum::Value(wl_pointer::ButtonState::Released) => {
                        app.gesture.press_pos = None;
                        // Drag drop: pin/unpin and never treat as a click.
                        if let Some(drag) = app.gesture.dragging.take() {
                            let insert = app.drag_dock_insert(&app.current_layout(), drag.pos);
                            app.drop_drag(drag, insert, true);
                        } else {
                            // Native button behavior: activate on release,
                            // only if it happens on the item the press armed
                            // (dragging away cancels the click).
                            app.update_hover();
                            if let Some(hit) = app.gesture.pressed.take() {
                                if app.hover == Some(hit) {
                                    app.activate_hit(hit);
                                }
                                // else: drag-cancel — do nothing.
                            } else if app.ui.target() == Target::Open {
                                // Press started on no interactive element (card
                                // background / transparent area): treat as a
                                // click-outside gesture and dismiss the popup.
                                app.dismiss();
                            }
                        }
                    }
                    _ => {}
                }
            }
            wl_pointer::Event::Button { button, state, .. }
                if button == BTN_RIGHT
                    && state == WEnum::Value(wl_pointer::ButtonState::Released) =>
            {
                // Right-click on a Files cell opens a terminal in that
                // directory (a file's containing folder).
                app.update_hover();
                if let Some(Hit::GridCell(s, cell)) = app.hover {
                    if s == content::SECTION_FILES {
                        if let Some(entry_idx) = app.search.visible[s].get(cell).copied() {
                            app.open_terminal_at(entry_idx);
                        }
                    }
                }
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } => {
                app.on_scroll(value);
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                value,
                ..
            } => {
                app.on_hscroll(value);
            }
            wl_pointer::Event::AxisStop {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                ..
            } => {
                // Gesture ended: clear the expand-bleed cooldown immediately so
                // the next gesture can navigate pages without the 300 ms wait.
                app.scroll.open_at = None;
            }
            _ => {} // frame, axis metadata, other buttons/axes
        }
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_layer!(App);
delegate_registry!(App);

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
mod hypr;
mod ipc;
mod launch;
mod pins;
mod renderer;
mod state;
mod surface;
mod usage;

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
    // beneath it, and the magnification headroom above; the card slides
    // within the extent range only (headroom excluded).
    let full_extent = config.window.height + config.window.bottom_margin;
    let surface_height = full_extent + content::MAGNIFY_HEADROOM as u32;
    let layer = surface::create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        config.window.width,
        surface_height,
    );

    // App discovery runs on the one allowed background thread; it
    // rescans on request (dock reveals) and delivers results over this
    // channel. The initial scan is queued by spawn_indexer.
    let (apps_tx, apps_rx) = channel::channel::<apps::LoadedApps>();
    let indexer = apps::spawn_indexer(config.theme.icon_theme.clone(), apps_tx);

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
            config.window.input_bar_height as f32,
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
        interactive: false,
        input_extent: None,
        entries: Vec::new(),
        kinds: Vec::new(),
        icon_layers: Vec::new(),
        base_len: 0,
        asset_folder: None,
        asset_file: None,
        file_index: Vec::new(),
        files_dir: None,
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
    /// Texture layers of the generic folder/file icons, with their
    /// placeholder flags.
    asset_folder: Option<(u32, bool)>,
    asset_file: Option<(u32, bool)>,
    /// Home-tree file index the search ranks against (fresh per rescan).
    file_index: Vec<apps::FileEntry>,
    /// Directory the Files section is navigated into (`None` = the
    /// top-level home-folder strip).
    files_dir: Option<std::path::PathBuf>,
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
        if self.ui.apply(command) {
            self.sync_surface_state();
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

    /// Request an app-index rescan unless one was requested recently.
    fn maybe_rescan(&mut self) {
        if self.last_rescan.elapsed() >= RESCAN_COOLDOWN {
            self.last_rescan = Instant::now();
            self.indexer.request_rescan();
        }
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

        match self.renderer.as_mut() {
            Some(renderer) => renderer.set_icons(&icons),
            None => self.pending_icons = Some(icons),
        }
        // Indices may have shifted: drop any armed click or in-flight drag,
        // rebuild the dock order, re-rank the query, re-resolve hover.
        self.gesture.pressed = None;
        self.gesture.dragging = None;
        self.recompute_dock_order();
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
        // Drop the previous query's transient file-result entries.
        self.entries.truncate(self.base_len);
        self.kinds.truncate(self.base_len);
        self.placeholders.truncate(self.base_len);
        self.icon_layers.truncate(self.base_len);

        let searching = !self.search.query.is_empty();
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
        if searching {
            visible[content::SECTION_FILES] = self.file_results();
        } else {
            // Hide pinned apps from the grid when the search box is
            // empty — they're already visible on the dock.
            visible[content::SECTION_APPS]
                .retain(|&idx| !self.pins.is_pinned(&self.entries[idx].id));
            if self.files_dir.is_some() {
                // Navigated into a folder: list its contents instead of
                // the top-level home strip.
                visible[content::SECTION_FILES] = self.dir_listing();
            }
        }
        self.search.visible = visible;
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
            self.files_dir.is_some(),
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

    /// Finish a drag: pin at the dock slot it was dropped on, or — when
    /// dropped outside the dock band — remove a dock-origin drag from the
    /// dock entirely (unpin + exclude from the usage-sort fill).
    fn drop_drag(&mut self, drag: DragState, insert: Option<usize>) {
        let Some(entry) = self.entries.get(drag.entry_idx) else {
            return; // entries were replaced mid-drag
        };
        let app_id = entry.id.clone();
        debug!(
            "drop: id={app_id} from_dock={} insert={insert:?}",
            drag.from_dock
        );
        match insert {
            Some(slot) => self.pins.pin_at(&app_id, slot),
            None if drag.from_dock => self.pins.exclude(&app_id),
            None => {}
        }
        self.recompute_dock_order();
        self.update_hover();
        self.schedule_frame();
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
                    // launching (the popup stays open).
                    if s == content::SECTION_FILES && self.try_navigate(entry_idx) {
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
        }
    }

    /// Get out of the way: fully hide, or just collapse to the dock when
    /// nothing overlaps its zone (intellihide keeps the dock parked).
    fn dismiss(&mut self) {
        let command = if self.zone_free {
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
        let (exec, id) = (entry.exec.clone(), entry.id.clone());
        let needs_terminal = entry.needs_terminal;
        if let Err(e) = launch::launch(&exec, needs_terminal, &self.config.launch.terminal) {
            error!("launch failed for {id}: {e:#}");
        }
        self.usage.increment(&id);
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
            surface::set_interactive(&self.layer, interactive);
            self.interactive = interactive;
        }

        let mut extent = self.ui.extent_of(self.ui.target()).round() as u32;
        // While hidden, keep a thin strip alive at the bottom edge so
        // touching it with the pointer can reveal the dock.
        if self.ui.target() == Target::Hidden && self.config.input.edge_reveal {
            extent = extent.max(self.config.input.edge_reveal_px);
        }
        if self.input_extent != Some(extent) {
            match surface::set_input_extent(&self.compositor, &self.layer, self.buffer_size, extent)
            {
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

        let animating = self.ui.tick(dt);
        if animating {
            // The card is moving under a possibly stationary pointer:
            // keep the hover highlight glued to what is really beneath it.
            // (Not update_hover(): its schedule_frame would recurse here.)
            self.hover = self.hover_at_pointer();
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
        let drag_frame = self
            .gesture
            .dragging
            .as_ref()
            .map(|drag| content::DragFrame {
                entry_idx: drag.entry_idx,
                pos: drag.pos,
                dock_insert: self.drag_dock_insert(&layout, drag.pos),
            });
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
                pointer: if drag_frame.is_none() {
                    self.pointer_pos
                } else {
                    None
                },
                bounce,
                query: &self.search.query,
                selected: self.search.selected.and_then(|i| self.flat_to_pos(i)),
                search_expand: self.search.expand,
                placeholders: &self.placeholders,
                layers: &self.icon_layers,
                files_path: &self.files_path_display(),
                dock_order: &self.dock_order,
                drag: drag_frame,
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
        let deadline = Instant::now() + delay;
        self.hide_deadline = Some(deadline);
        let timer = Timer::from_duration(delay);
        if let Err(e) = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.hide_deadline == Some(deadline) {
                    app.hide_deadline = None;
                    // Fully hide, or just fall back to the parked dock
                    // when the zone is free (intellihide).
                    app.dismiss();
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
            width = self.config.window.width;
        }
        if height == 0 {
            height = self.config.window.height
                + self.config.window.bottom_margin
                + content::MAGNIFY_HEADROOM as u32;
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
        // Focus loss (alt-tab, click elsewhere) collapses the open popup
        // back to the dock; the dock itself persists until toggled away.
        debug!("keyboard focus lost");
        self.handle_command(Command::Collapse);
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
                if app.ui.target() == Target::Hidden && app.pointer_on_reveal_strip() {
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
                                Hit::SearchButton | Hit::FilesBack => None,
                            };
                            if let Some(entry_idx) = entry_idx {
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
                    app.drop_drag(drag, None);
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
                        } else {
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
                            app.drop_drag(drag, insert);
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

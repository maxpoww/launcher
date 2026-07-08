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
mod ipc;
mod launch;
mod renderer;
mod state;
mod surface;

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
use waverunner_core::Config;
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
        scroll_accum: 0.0,
        hide_deadline: None,
        interactive: false,
        input_extent: None,
        entries: Vec::new(),
        pending_icons: None,
        hover: None,
        pressed: None,
        pointer_pos: None,
        list_scroll: 0.0,
        indexer,
        last_rescan: Instant::now(),
        bounce: None,
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
    /// Accumulated vertical scroll over the dock, for the expand gesture.
    scroll_accum: f64,
    /// Deadline of the pending auto-hide, if the pointer has left the
    /// dock. Re-entry clears it, invalidating the in-flight timer.
    hide_deadline: Option<Instant>,
    /// Last keyboard-interactivity value sent to the compositor.
    interactive: bool,
    /// Last input-region extent sent to the compositor.
    input_extent: Option<u32>,

    /// Discovered applications, sorted by name (icon texture layers are
    /// aligned with this order).
    entries: Vec<AppEntry>,
    /// Icons that arrived before the renderer existed.
    pending_icons: Option<Vec<Vec<u8>>>,
    /// Item currently under the pointer.
    hover: Option<Hit>,
    /// Item a left-button press started on; release on the same item
    /// activates it.
    pressed: Option<Hit>,
    /// Pointer position in surface coordinates, while inside.
    pointer_pos: Option<(f32, f32)>,
    /// App-grid scroll offset in pixels (clamped during layout).
    list_scroll: f32,
    /// Handle to the background indexer thread.
    indexer: apps::Indexer,
    /// When the last rescan was requested, for the reveal cooldown.
    last_rescan: Instant,
    /// A launch bounce in flight: (entry index, start time).
    bounce: Option<(usize, Instant)>,

    exit: bool,
}

/// Accumulated scroll (in wl_pointer axis units; one wheel notch ≈ 15)
/// needed to trigger the dock-expand / popup-collapse gesture.
const SCROLL_THRESHOLD: f64 = 10.0;

/// Linux evdev code for the left mouse button.
const BTN_LEFT: u32 = 0x110;

/// Minimum time between app-index rescans. Summoning the dock checks
/// freshness; mashing toggle does not scan repeatedly.
const RESCAN_COOLDOWN: Duration = Duration::from_secs(2);

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
        if matches!(command, Command::Show | Command::Toggle | Command::Expand) {
            self.maybe_rescan();
        }
        if self.ui.apply(command) {
            self.sync_surface_state();
            self.schedule_frame();
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
        self.entries = loaded.entries;
        match self.renderer.as_mut() {
            Some(renderer) => renderer.set_icons(&loaded.icons),
            None => self.pending_icons = Some(loaded.icons),
        }
        // Indices may have shifted: drop any armed click and re-resolve
        // what the pointer is over.
        self.pressed = None;
        self.update_hover();
        self.schedule_frame();
    }

    /// Layout for the current animation extent and scroll offset.
    fn current_layout(&self) -> content::Layout {
        content::layout(
            &self.config,
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            self.ui.extent(),
            self.entries.len(),
            self.list_scroll,
        )
    }

    /// Recompute which item the pointer is over; redraw on change.
    fn update_hover(&mut self) {
        let hover = self
            .pointer_pos
            .and_then(|pos| content::hit_test(&self.current_layout(), pos));
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

    /// Launch the app under the pointer; its icon plays a bounce
    /// (macOS launch feedback), then the card hides.
    fn activate(&mut self, index: usize) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        if let Err(e) = launch::launch(&entry.exec) {
            error!("launch failed for {}: {e:#}", entry.id);
        }
        self.bounce = Some((index, Instant::now()));
        self.schedule_frame();
        let timer = Timer::from_duration(BOUNCE_DURATION);
        if let Err(e) = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.handle_command(Command::Hide);
                TimeoutAction::Drop
            })
        {
            warn!("failed to arm launch-hide timer: {e}");
            self.handle_command(Command::Hide);
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
            self.list_scroll = 0.0;
            self.hover = None;
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
            self.hover = self
                .pointer_pos
                .and_then(|pos| content::hit_test(&self.current_layout(), pos));
        }
        self.dirty = false;

        let wl_surface = self.layer.wl_surface();
        wl_surface.frame(&self.qh, wl_surface.clone());
        self.frame_pending = true;

        let bounce = self.bounce_offset();
        let layout = self.current_layout();
        self.list_scroll = layout.scroll; // keep the clamped value
        let scene = content::scene(
            &self.config,
            &layout,
            &self.entries,
            (self.buffer_size.0 as f32, self.buffer_size.1 as f32),
            &content::FrameInput {
                hover: self.hover,
                alpha: self.ui.alpha(),
                pointer: self.pointer_pos,
                bounce,
            },
        );
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Err(e) = renderer.render(&scene, self.config.theme.text_rgba()) {
            error!("render failed: {e:#}");
        }
    }

    /// One vertical-scroll step of `value` axis units.
    ///
    /// Docked, the wheel is the expand/collapse gesture; open, it
    /// scrolls the grid, and pushing past the top accumulates into the
    /// collapse gesture instead. Natural scroll (default): scrolling
    /// down expands, up collapses; classic direction when disabled.
    fn on_scroll(&mut self, value: f64) {
        match self.ui.target() {
            Target::Dock => self.scroll_accum += value,
            Target::Open => {
                let next = self.list_scroll + value as f32;
                if next < 0.0 && self.list_scroll <= 0.0 {
                    self.scroll_accum += value;
                } else {
                    // Normal grid scrolling: any partial collapse
                    // gesture is abandoned.
                    self.scroll_accum = 0.0;
                    self.list_scroll = next.max(0.0);
                    self.update_hover();
                    self.schedule_frame();
                }
            }
            Target::Hidden => {}
        }
        let mut toward_open = self.scroll_accum;
        if self.config.input.natural_scroll {
            toward_open = -toward_open;
        }
        if toward_open <= -SCROLL_THRESHOLD {
            self.scroll_accum = 0.0;
            self.handle_command(Command::Expand);
        } else if toward_open >= SCROLL_THRESHOLD {
            self.scroll_accum = 0.0;
            self.handle_command(Command::Collapse);
        }
    }

    /// Arm the auto-hide grace timer after the pointer leaves the dock.
    /// A later pointer re-entry clears `hide_deadline`, turning the
    /// in-flight timer into a no-op when it fires.
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
                    app.handle_command(Command::Hide);
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
            match self.seat_state.get_keyboard(qh, &seat, None) {
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
        if event.keysym == Keysym::Escape {
            // Escape only arrives while open (the dock never has keys):
            // slide back down to the dock.
            self.handle_command(Command::Collapse);
        }
        // P4: feed printable keys / Up / Down / Return into the search UI.
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
                app.pointer_pos = Some((surface_x as f32, surface_y as f32));
                app.update_hover();
                // Magnification is a function of the pointer position:
                // every move damages the scene (coalesced to refresh).
                if app.ui.target() != Target::Hidden {
                    app.schedule_frame();
                }
            }
            wl_pointer::Event::Leave { .. } => {
                app.scroll_accum = 0.0;
                app.pointer_pos = None;
                app.pressed = None;
                app.update_hover();
                if app.ui.target() != Target::Hidden {
                    app.schedule_frame(); // relax any magnification
                    if app.config.input.autohide {
                        app.schedule_autohide();
                    }
                }
            }
            wl_pointer::Event::Button { button, state, .. } if button == BTN_LEFT => {
                match state {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                        app.update_hover();
                        app.pressed = app.hover;
                    }
                    WEnum::Value(wl_pointer::ButtonState::Released) => {
                        // Native button behavior: activate on release,
                        // only if it happens on the item the press armed
                        // (dragging away cancels the click).
                        app.update_hover();
                        if let Some(hit) = app.pressed.take() {
                            if app.hover == Some(hit) {
                                match hit {
                                    Hit::DockIcon(i) | Hit::GridCell(i) => app.activate(i),
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                value,
                ..
            } => {
                app.on_scroll(value);
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

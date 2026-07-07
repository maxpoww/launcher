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
mod ipc;
mod renderer;
mod state;
mod surface;

use std::time::Instant;

use anyhow::Context;
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, registry_handlers,
};
use tracing::{debug, error, info, warn};
use waverunner_core::Config;
use waverunner_proto::Command;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface};
use wayland_client::{Connection, QueueHandle};

use crate::renderer::Renderer;
use crate::state::UiState;

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

    let layer = surface::create_layer_surface(
        &compositor,
        &layer_shell,
        &qh,
        config.window.width,
        config.window.height,
    );

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        conn: conn.clone(),
        qh: qh.clone(),
        compositor,
        layer,
        renderer: None,
        ui: UiState::new(
            config.animation.clone(),
            config.window.input_bar_height as f32,
            config.window.height as f32,
        ),
        config,
        buffer_size: (0, 0),
        scale_factor: 1,
        last_frame: None,
        frame_pending: false,
        keyboard: None,
        pointer: None,
        scroll_accum: 0.0,
        interactive: false,
        input_extent: None,
        exit: false,
    };

    let socket_path = waverunner_proto::socket_path();
    let _socket_guard = ipc::listen(&event_loop.handle(), &socket_path)?;

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
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Accumulated vertical scroll over the dock, for the expand gesture.
    scroll_accum: f64,
    /// Last keyboard-interactivity value sent to the compositor.
    interactive: bool,
    /// Last input-region extent sent to the compositor.
    input_extent: Option<u32>,

    exit: bool,
}

/// Accumulated scroll (in wl_pointer axis units; one wheel notch ≈ 15)
/// needed to trigger the dock-expand / popup-collapse gesture.
const SCROLL_THRESHOLD: f64 = 10.0;

impl App {
    /// Entry point for IPC commands (called from ipc.rs) and for
    /// internally generated commands (Escape, focus loss, scroll).
    pub fn handle_command(&mut self, command: Command) {
        if self.ui.apply(command) {
            self.sync_surface_state();
            self.schedule_frame();
        }
    }

    /// Push keyboard interactivity and the pointer input region to the
    /// compositor whenever the targeted rest state changes. The input
    /// region covers the *target* rect immediately so the dock/popup is
    /// interactive without waiting for the slide to finish.
    fn sync_surface_state(&mut self) {
        let interactive = self.ui.wants_keyboard();
        if interactive != self.interactive {
            surface::set_interactive(&self.layer, interactive);
            self.interactive = interactive;
        }

        let extent = self.ui.extent_of(self.ui.target()).round() as u32;
        if self.input_extent != Some(extent) {
            match surface::set_input_extent(&self.compositor, &self.layer, self.buffer_size, extent)
            {
                Ok(()) => self.input_extent = Some(extent),
                Err(e) => warn!("failed to set input region: {e:#}"),
            }
        }
    }

    /// Draw immediately if the surface is ready and no frame callback is
    /// already in flight.
    fn schedule_frame(&mut self) {
        if self.renderer.is_none() {
            debug!("frame requested before first configure; deferring");
            return;
        }
        if !self.frame_pending {
            self.last_frame = None; // animation resumes: don't count idle time as dt
            self.draw();
        }
    }

    /// Render one frame; while animating, request the next frame callback
    /// *before* presenting so it rides on this frame's commit.
    fn draw(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };

        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(1.0 / 60.0);
        self.last_frame = Some(now);

        let animating = self.ui.tick(dt);

        if animating {
            let wl_surface = self.layer.wl_surface();
            wl_surface.frame(&self.qh, wl_surface.clone());
            self.frame_pending = true;
        } else {
            self.frame_pending = false;
            self.last_frame = None;
            debug!("settled in {:?}, going idle", self.ui.target());
        }

        if let Err(e) = renderer.render(self.ui.extent(), self.ui.alpha()) {
            error!("render failed: {e:#}");
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
        if self.ui.is_animating() {
            self.draw();
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
            height = self.config.window.height;
        }
        debug!("configure: {width}x{height}");
        self.buffer_size = (width, height);

        match self.renderer.as_mut() {
            Some(renderer) => renderer.resize(width, height),
            None => {
                match Renderer::new(
                    &self.conn,
                    self.layer.wl_surface(),
                    width,
                    height,
                    self.config.theme.background_rgba(),
                    self.config.theme.corner_radius,
                ) {
                    Ok(renderer) => self.renderer = Some(renderer),
                    Err(e) => {
                        error!("renderer init failed: {e:#}");
                        self.exit = true;
                        return;
                    }
                }
            }
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
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(e) => warn!("cannot get pointer: {e}"),
            }
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

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match event.kind {
                PointerEventKind::Axis { vertical, .. } => {
                    self.scroll_accum += vertical.absolute;
                }
                PointerEventKind::Leave { .. } => {
                    self.scroll_accum = 0.0;
                }
                _ => {}
            }
        }
        // Natural scroll (default): scrolling down (positive axis values,
        // content-follows-fingers) on the dock expands to the full popup,
        // scrolling up collapses. Classic direction when disabled.
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
delegate_pointer!(App);
delegate_layer!(App);
delegate_registry!(App);

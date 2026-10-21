//! OTHER MONITORS (Max, 2026-10-21: "it follows you"): ONE dock, and it comes
//! out on whichever chosen screen it is called from — Super there, or the
//! pointer at that screen's edge. The built-in screen always has it (the main
//! one when the built-in is off: `launcher_prefs::main_screen`); the others
//! unless turned off on the Launcher page (`launcher_screens_off`).
//!
//! A layer surface cannot change screens, so the dock is MADE AGAIN on the
//! one it is called to: a new surface there, and once the compositor has sized
//! it (its first configure) the renderer moves onto it (`Renderer::retarget`:
//! the icons, pipelines and labels stay) and the old surface goes. The call it
//! came for is then run there, as if it had been made there.
//!
//! The pointer at another screen's edge is caught by an EDGE: a thin invisible
//! strip on each chosen screen the dock is not on, along the edge the dock
//! stands on, as deep and as long as the dock's own reveal strip. Touched, it
//! brings the dock over. A screen turned on in the page shows its edge for a
//! beat (the choice seen where it lands).
//!
//! The dock's screen going (unplugged, a lid shut) closes its surface: it is
//! made again on the main screen instead of the shell ending with it.
//!
//! NOT FROM THE OUTSIDE (Max, 2026-10-21, the TV on top of the laptop: "the
//! dock must not show when I'm coming from the outside — from under, here,
//! or from the left when it is on the left"; then "now I can't call the dock
//! from the TV"): where another screen goes on across the dock's edge
//! (`shared`, an edge's own `shared`) a touch is TOLD APART before it calls.
//! The pointer coming over from that screen moves the compositor's focus to
//! this one (`focusedmon`) the moment it crosses; one already on this screen
//! going down to the edge does not. So a touch there waits `CROSS_WAIT` for
//! that news: the screen just focused (`CROSSED`) = it came over, passing —
//! no call until it leaves the strip; else it is a call, if the pointer is
//! still there (one going on through to the other screen has left).

use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::raw::RawPool;
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use std::time::{Duration, Instant};
use tracing::{info, warn};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::QueueHandle;
use waverunner_proto::Command;

use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::fractional::{Fractional, SurfaceKind, SurfaceScale};
use crate::launcher_prefs::{self, Side};
use crate::state::Target;
use crate::visible::{SurfaceVisible, VisibleRegions};
use crate::App;

/// What a surface made again needs: the globals the first ones were made
/// with (the dock's here, the rest of the shell's in `shell_screens.rs`).
pub(crate) struct Kit {
    pub(crate) shell: LayerShell,
    pub(crate) fractional: Option<Fractional>,
    pub(crate) visible: Option<VisibleRegions>,
    /// For the veil's stretched pixel.
    pub(crate) viewporter: Option<WpViewporter>,
}

impl Kit {
    pub(crate) fn bind(
        globals: &GlobalList,
        qh: &QueueHandle<App>,
        fractional: Option<Fractional>,
        visible: Option<VisibleRegions>,
    ) -> Option<Kit> {
        let shell = LayerShell::bind(globals, qh)
            .map_err(|e| warn!("screens: no layer shell of its own ({e}); the shell stays where it starts"))
            .ok()?;
        let viewporter = globals.bind::<WpViewporter, App, _>(qh, 1..=1, ()).ok();
        Some(Kit { shell, fractional, visible, viewporter })
    }
}

/// The dock being made again on another screen, until it is up there.
struct Move {
    layer: LayerSurface,
    fscale: Option<SurfaceScale>,
    visible: Option<SurfaceVisible>,
    /// `None`: wherever the compositor puts it (no screen known to send it to).
    output: Option<WlOutput>,
    /// The calls it was made for, run there once it is.
    then: Vec<Command>,
    /// The old surface was closed under us (its screen went): nothing is
    /// drawn until the new one is up.
    old_gone: bool,
}

/// An edge that catches the pointer on a screen the dock is not on.
struct Edge {
    layer: LayerSurface,
    output: WlOutput,
    side: Side,
    /// Along the edge, logical px (the dock's resting plate).
    len: u32,
    /// Its size from the compositor (none before its configure).
    size: Option<(u32, u32)>,
    /// Shown for a beat when it is painted (a screen just turned on).
    glow: bool,
    /// The stretches of its screen's edge another screen goes on across (px
    /// along the edge from the screen's start).
    shared: Vec<(f64, f64)>,
}

/// How long a screen just turned on shows its edge.
const GLOW: std::time::Duration = std::time::Duration::from_millis(1400);

/// How long a touch where another screen goes on across waits for the
/// compositor's news that the pointer just came over from it.
const CROSS_WAIT: Duration = Duration::from_millis(80);
/// A screen focused this recently, at the end of that wait, was just come
/// into: the touch was the pointer coming over.
const CROSSED: Duration = Duration::from_millis(400);

/// A touch waiting to be told apart: on the dock's own strip, or on the edge
/// of another screen.
#[derive(Clone, PartialEq)]
pub(crate) enum Touch {
    Dock,
    Edge(WlOutput),
}

/// Whether the pointer came over to `screen` just now: the compositor's
/// focus moved there (`focused`: the screen and how long ago) `CROSSED` ago
/// or less.
fn came_over(focused: Option<(&str, Duration)>, screen: Option<&str>) -> bool {
    matches!((focused, screen), (Some((name, ago)), Some(screen)) if name == screen && ago < CROSSED)
}

/// The edge's pixels (`wl_shm` ARGB8888, little-endian, premultiplied): clear,
/// or lit — a soft white fading inward from the screen's edge.

#[derive(Default)]
pub(crate) struct DockScreens {
    moving: Option<Move>,
    /// The calls a move came for, between its swap and the configure's draw.
    then: Vec<Command>,
    edges: Vec<Edge>,
    /// The screen the dock is on (its surface's enter; set on a move).
    output: Option<WlOutput>,
    /// The stretches of that screen's edge on the dock's side another screen
    /// goes on across (px along the edge from its start): a touch there is
    /// told apart before it calls.
    pub(crate) shared: Vec<(f64, f64)>,
    /// The screen the compositor last moved its focus to, and when
    /// (`focusedmon`: the pointer crossing over is one).
    focused: Option<(String, Instant)>,
    /// A touch waiting `CROSS_WAIT` to be told apart.
    touch: Option<Touch>,
    /// The pointer came over from another screen and is still on the strip
    /// or the edge: not a call until it leaves.
    passing: bool,
}

/// The stretches of screen `name`'s edge on `side` another screen goes on
/// across, in px along that edge from its start (left, or top).
pub(crate) fn shared_stretches(all: &[crate::hypr::MonitorInfo], name: &str, side: Side) -> Vec<(f64, f64)> {
    let Some(m) = all.iter().find(|m| m.name == name) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for n in all.iter().filter(|n| n.name != name) {
        let (touch, (a0, a1), (b0, b1), origin) = match side {
            Side::Bottom => ((n.y - (m.y + m.h)).abs() < 2.0, (m.x, m.x + m.w), (n.x, n.x + n.w), m.x),
            Side::Left => ((n.x + n.w - m.x).abs() < 2.0, (m.y, m.y + m.h), (n.y, n.y + n.h), m.y),
            Side::Right => ((n.x - (m.x + m.w)).abs() < 2.0, (m.y, m.y + m.h), (n.y, n.y + n.h), m.y),
        };
        let (lo, hi) = (a0.max(b0), a1.min(b1));
        if touch && hi > lo {
            out.push((lo - origin, hi - origin));
        }
    }
    out
}

/// Whether the screen at `i` has the launcher: the main one always, the
/// others unless turned off.
pub(crate) fn has_launcher(screens: &[crate::display::Monitor], i: usize, off: &[String]) -> bool {
    launcher_prefs::main_screen(screens) == Some(i) || !off.contains(&screens[i].selector())
}

/// The calls the dock follows to the screen they come from: the ones that
/// bring it up (Super, its box, the boxes drawn on it).
fn follows(command: &Command) -> bool {
    matches!(
        command,
        Command::Toggle
            | Command::Expand
            | Command::ControlPanel
            | Command::Modules(_)
            | Command::FastLaunch
            | Command::FastControls
    )
}

fn edge_pixels(w: u32, h: u32, side: Side, lit: bool) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    if !lit {
        return px;
    }
    let depth = match side {
        Side::Bottom => h,
        Side::Left | Side::Right => w,
    }
    .max(1);
    for y in 0..h {
        for x in 0..w {
            // How far in from the screen's edge, 0 at the edge.
            let d = match side {
                Side::Bottom => h - 1 - y,
                Side::Left => x,
                Side::Right => w - 1 - x,
            };
            let a = (0.55 * (1.0 - d as f32 / depth as f32) * 255.0).round() as u8;
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[a, a, a, a]);
        }
    }
    px
}

/// An edge's layer surface, committed with nothing attached (its configure
/// gives it its size and a clear buffer). Centred on its edge, as the dock's
/// own strip is.
fn edge_new(
    compositor: &CompositorState,
    shell: &LayerShell,
    qh: &QueueHandle<App>,
    output: &WlOutput,
    side: Side,
    depth: u32,
    len: u32,
) -> LayerSurface {
    let surface = compositor.create_surface(qh);
    let layer = shell.create_layer_surface(qh, surface, Layer::Top, Some("waverunner-edge"), Some(output));
    match side {
        Side::Bottom => {
            layer.set_anchor(Anchor::BOTTOM);
            layer.set_size(len, depth);
        }
        Side::Left => {
            layer.set_anchor(Anchor::LEFT);
            layer.set_size(depth, len);
        }
        Side::Right => {
            layer.set_anchor(Anchor::RIGHT);
            layer.set_size(depth, len);
        }
    }
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.commit();
    layer
}

impl App {
    /// The screen the dock is on, by its connector name.
    pub(crate) fn dock_screen_name(&self) -> Option<String> {
        let output = self.dock_screens.output.as_ref()?;
        self.output_state.info(output).and_then(|i| i.name)
    }

    /// The logical size of the screen the dock is on (else the shell's).
    pub(crate) fn dock_screen_size(&self) -> Option<(f32, f32)> {
        self.dock_screens
            .output
            .as_ref()
            .and_then(|o| self.output_state.info(o))
            .and_then(|i| i.logical_size)
            .map(|(w, h)| (w as f32, h as f32))
    }

    pub(crate) fn dock_screen_h(&self) -> Option<f32> {
        self.dock_screen_size().map(|(_, h)| h).or_else(|| self.output_logical_height())
    }

    pub(crate) fn dock_screen_w(&self) -> Option<f32> {
        self.dock_screen_size().map(|(w, _)| w).or_else(|| self.output_logical_width())
    }

    /// The dock's size on the screen it is on: its own (`dock_sizes`), else
    /// the main screen's. Set, not yet applied (the caller lays out).
    fn dock_size_here(&mut self) {
        let level = self
            .dock_screen_selector()
            .and_then(|s| self.settings.dock_sizes.get(&s).copied())
            .unwrap_or(self.icon_size_home);
        self.icon_size = level.min(crate::content::ICON_SCALES.len() - 1);
    }

    /// What the dock's screen is known by (`Monitor::selector`).
    fn dock_screen_selector(&self) -> Option<String> {
        let name = self.dock_screen_name()?;
        crate::display::monitors().into_iter().find(|m| m.name == name).map(|m| m.selector())
    }

    /// A Ctrl +/− level picked: kept for the dock's screen; on the main one
    /// (or the only one) it is the main screen's size too.
    pub(crate) fn keep_dock_size(&mut self) {
        let two = self.output_state.outputs().nth(1).is_some();
        let home = !two || self.main_output().is_none_or(|m| self.dock_screens.output.as_ref() == Some(&m));
        if home {
            self.icon_size_home = self.icon_size;
            crate::save_icon_size(self.icon_size);
        }
        if two {
            if let Some(sel) = self.dock_screen_selector() {
                self.settings.dock_sizes.insert(sel, self.icon_size);
                self.settings.save();
            }
        }
    }

    /// What the OPTIONS bar takes off the top of the dock's screen: nothing on
    /// a screen it is not on.
    pub(crate) fn dock_top_reserved(&self) -> f32 {
        match (&self.dock_screens.output, &self.shell_output) {
            (Some(dock), Some(bar)) if dock != bar => 0.0,
            _ => self.options_bar_h(),
        }
    }

    pub(crate) fn output_named(&self, name: &str) -> Option<WlOutput> {
        self.output_state
            .outputs()
            .find(|o| self.output_state.info(o).and_then(|i| i.name).as_deref() == Some(name))
    }

    /// Nothing may be drawn on the dock: its screen went and the new one is
    /// not up yet.
    pub(crate) fn dock_lost(&self) -> bool {
        self.dock_screens.moving.as_ref().is_some_and(|m| m.old_gone)
    }

    /// A call that brings the dock up, from a chosen screen it is not on:
    /// the dock is made again there and the call runs once it is (true —
    /// the caller leaves it). Calls while a move is on wait for it.
    pub(crate) fn dock_follow(&mut self, command: &Command) -> bool {
        if !follows(command) {
            return false;
        }
        if let Some(m) = self.dock_screens.moving.as_mut() {
            m.then.push(command.clone());
            return true;
        }
        // Not while something of the dock's is up where it is: the call
        // closes that there.
        if self.ui.target() == Target::Open || self.fast.open {
            return false;
        }
        if self.output_state.outputs().nth(1).is_none() {
            return false;
        }
        let screens = crate::display::monitors();
        let off = &self.settings.launcher_screens_off;
        let chosen = |i: usize| has_launcher(&screens, i, off);
        let target = match screens.iter().position(|m| m.focused) {
            Some(i) if chosen(i) => i,
            // Called from a screen without it: it comes out where it is,
            // unless that has none either (turned off while it was there).
            _ => {
                let here = self.dock_screen_name();
                let here = screens.iter().position(|m| Some(&m.name) == here.as_ref());
                match (here, launcher_prefs::main_screen(&screens)) {
                    (Some(h), _) if chosen(h) => return false,
                    (_, Some(main)) => main,
                    _ => return false,
                }
            }
        };
        let Some(output) = self.output_named(&screens[target].name) else {
            return false;
        };
        if self.dock_screens.output.as_ref() == Some(&output) {
            return false;
        }
        info!("dock: called on {}: made again there", screens[target].name);
        self.move_dock_to(Some(output), vec![command.clone()], false)
    }

    /// Make the dock again on `output` (see the module's note). False when it
    /// cannot be (no kit): it stays where it is.
    fn move_dock_to(&mut self, output: Option<WlOutput>, then: Vec<Command>, old_gone: bool) -> bool {
        let Some(kit) = self.screen_kit.as_ref() else {
            return false;
        };
        let (_, h) = self.scaled_surface_size();
        let layer = crate::surface::create_layer_surface(
            &self.compositor,
            &kit.shell,
            &self.qh,
            0,
            h,
            self.config.window.render_scale.max(1),
            output.as_ref(),
        );
        let fscale = kit
            .fractional
            .as_ref()
            .map(|f| f.attach(layer.wl_surface(), SurfaceKind::Dock, &self.qh));
        let visible = kit.visible.as_ref().map(|v| v.attach(layer.wl_surface(), &self.qh));
        // (A move already on is replaced: its calls carry over.)
        let (mut then, mut old_gone) = (then, old_gone);
        if let Some(old) = self.dock_screens.moving.take() {
            then.splice(0..0, old.then);
            old_gone |= old.old_gone;
            drop_surface(old.layer, old.fscale, old.visible);
        }
        self.dock_screens.moving = Some(Move { layer, fscale, visible, output, then, old_gone });
        true
    }

    /// The main screen's output (the built-in, else the one at the origin).
    pub(crate) fn main_output(&self) -> Option<WlOutput> {
        let screens = crate::display::monitors();
        let i = launcher_prefs::main_screen(&screens)?;
        self.output_named(&screens[i].name)
    }

    /// The fractional state of the dock being made again, for a scale it is
    /// told before it takes over (the event would otherwise land on the old
    /// one and the new one never hear it).
    pub(crate) fn dock_next_fscale(
        &mut self,
        proxy: &wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
    ) -> Option<&mut SurfaceScale> {
        self.dock_screens.moving.as_mut()?.fscale.as_mut().filter(|f| f.is(proxy))
    }

    /// Whether this configure is for the dock being made again.
    pub(crate) fn is_dock_next(&self, surface: &WlSurface) -> bool {
        self.dock_screens.moving.as_ref().is_some_and(|m| m.layer.wl_surface() == surface)
    }

    /// The dock made again has its size: it takes over (the renderer moves
    /// onto it, the old surface goes) and the configure goes on as the
    /// dock's. False: it could not (the renderer would not move), it is let
    /// go and the dock stays where it was.
    pub(crate) fn dock_arrived(&mut self) -> bool {
        let Some(m) = self.dock_screens.moving.take() else {
            return false;
        };
        if let Some(renderer) = self.renderer.as_mut() {
            if let Err(e) = renderer.retarget(&self.conn, m.layer.wl_surface()) {
                if m.old_gone {
                    // Nowhere else to be: a renderer of its own there (the
                    // configure makes it; the icons come with the next scan).
                    warn!("dock: cannot move its renderer ({e:#}); a new one");
                    self.renderer = None;
                } else {
                    warn!("dock: cannot draw on the other screen ({e:#}); it stays");
                    drop_surface(m.layer, m.fscale, m.visible);
                    for command in m.then {
                        self.handle_command(command);
                    }
                    return false;
                }
            }
        }
        let old_layer = std::mem::replace(&mut self.layer, m.layer);
        let old_fscale = std::mem::replace(&mut self.dock_fscale, m.fscale);
        let old_visible = std::mem::replace(&mut self.dock_visible, m.visible);
        drop_surface(old_layer, old_fscale, old_visible);
        if m.output.is_some() {
            self.dock_screens.output = m.output;
        }
        // Its size on this screen (laid out just below).
        self.dock_size_here();
        self.dock_screens.then = m.then;
        // A frame asked of the old surface never comes; its input region,
        // keyboard and pointer went with it.
        self.frame_pending = false;
        self.input_extent = None;
        if self.interactive {
            crate::surface::set_interactive(&self.layer, true);
        }
        self.pointer_pos = None;
        self.hover = None;
        // Sized for this screen (a configure follows if that differs).
        self.refresh_scaled_geometry();
        self.sync_edges();
        true
    }

    /// The calls the move was made for, now that the dock is drawn there.
    pub(crate) fn dock_arrived_then(&mut self) {
        for command in std::mem::take(&mut self.dock_screens.then) {
            self.handle_command(command);
        }
    }

    /// The dock's surface entered a screen (its first map, at start).
    pub(crate) fn dock_entered(&mut self, output: &WlOutput) {
        if self.dock_screens.output.as_ref() == Some(output) {
            return;
        }
        let first = self.dock_screens.output.is_none();
        self.dock_screens.output = Some(output.clone());
        if self.output_state.outputs().nth(1).is_some() {
            self.dock_size_here();
        }
        // At START it is on the main screen, as the rest of the shell is
        // (`shell_screens.rs`), wherever the focus was; from then on it
        // follows the calls.
        if first && self.dock_screens.moving.is_none() && self.output_state.outputs().nth(1).is_some() {
            if let Some(home) = self.main_output().filter(|home| home != output) {
                info!("dock: started on another screen; made again on the main one");
                self.move_dock_to(Some(home), Vec::new(), false);
            }
        }
        self.refresh_scaled_geometry();
        self.sync_edges();
    }

    /// A surface of the dock's was closed by the compositor (its screen
    /// went). True when it was one of them.
    pub(crate) fn dock_closed(&mut self, surface: &WlSurface) -> bool {
        if self.is_dock_next(surface) {
            // The screen it was being made on went: back where it is, or home
            // when that went too.
            let m = self.dock_screens.moving.take().expect("checked");
            let (then, old_gone) = (m.then, m.old_gone);
            drop_surface(m.layer, m.fscale, m.visible);
            if old_gone {
                let home = self.main_output();
                self.move_dock_to(home, then, true);
            } else {
                for command in then {
                    self.handle_command(command);
                }
            }
            return true;
        }
        if self.layer.wl_surface() == surface {
            warn!("dock: its screen went; made again on the main one");
            self.dock_screens.output = None;
            match self.dock_screens.moving.as_mut() {
                Some(m) => m.old_gone = true,
                None => {
                    let home = self.main_output();
                    if !self.move_dock_to(home, Vec::new(), true) {
                        // Nothing to make it again with: the old way out.
                        self.exit = true;
                    }
                }
            }
            return true;
        }
        if let Some(i) = self.dock_screens.edges.iter().position(|e| e.layer.wl_surface() == surface) {
            self.dock_screens.edges.remove(i);
            return true;
        }
        false
    }

    /// The dock's layer and its edges' (`App::nudge_layers`).
    pub(crate) fn dock_layers(&self) -> Vec<&LayerSurface> {
        std::iter::once(&self.layer).chain(self.dock_screens.edges.iter().map(|e| &e.layer)).collect()
    }

    /// Whether `surface` is an edge's.
    pub(crate) fn is_dock_edge(&self, surface: &WlSurface) -> bool {
        self.dock_screens.edges.iter().any(|e| e.layer.wl_surface() == surface)
    }

    /// An edge was sized: give it a clear buffer of that size (a surface
    /// with nothing attached is not there to be touched).
    pub(crate) fn configure_dock_edge(&mut self, surface: &WlSurface, configure: LayerSurfaceConfigure) {
        let Some(i) = self.dock_screens.edges.iter().position(|e| e.layer.wl_surface() == surface) else {
            return;
        };
        let (w, h) = configure.new_size;
        if w == 0 || h == 0 {
            return;
        }
        self.dock_screens.edges[i].size = Some((w, h));
        if std::mem::take(&mut self.dock_screens.edges[i].glow) {
            self.glow_edge(i);
        } else {
            self.paint_edge(i, false);
        }
    }

    /// Show the edge for a beat, then clear it again.
    fn glow_edge(&mut self, i: usize) {
        self.paint_edge(i, true);
        let output = self.dock_screens.edges[i].output.clone();
        let armed = self.loop_handle.insert_source(Timer::from_duration(GLOW), move |_, _, app: &mut App| {
            if let Some(i) = app.dock_screens.edges.iter().position(|e| e.output == output) {
                app.paint_edge(i, false);
            }
            TimeoutAction::Drop
        });
        if let Err(e) = armed {
            warn!("dock edge: no timer for its beat ({e})");
        }
    }

    /// Attach the edge's buffer: clear (only there to be touched), or lit.
    fn paint_edge(&self, i: usize, lit: bool) {
        let edge = &self.dock_screens.edges[i];
        let (Some((w, h)), Some(shm)) = (edge.size, self.shm.as_ref()) else {
            return;
        };
        let mut pool = match RawPool::new((w * h * 4) as usize, shm) {
            Ok(pool) => pool,
            Err(e) => {
                warn!("dock edge: no shm pool ({e})");
                return;
            }
        };
        pool.mmap().copy_from_slice(&edge_pixels(w, h, edge.side, lit));
        let buffer = pool.create_buffer(0, w as i32, h as i32, (w * 4) as i32, wl_shm::Format::Argb8888, (), &self.qh);
        let surface = edge.layer.wl_surface();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.commit();
        // (The compositor keeps what it was given.)
        buffer.destroy();
    }

    /// A screen turned on in the page (`launcher-screen`): its edge is made
    /// and shown for a beat.
    pub(crate) fn screen_chosen(&mut self, selector: &str) {
        self.sync_edges();
        let screens = crate::display::monitors();
        let Some(output) = screens
            .iter()
            .find(|m| m.selector() == selector)
            .and_then(|m| self.output_named(&m.name))
        else {
            return;
        };
        // (Not configured yet: its configure shows it.)
        if let Some(i) = self.dock_screens.edges.iter().position(|e| e.output == output) {
            if self.dock_screens.edges[i].size.is_some() {
                self.glow_edge(i);
            } else {
                self.dock_screens.edges[i].glow = true;
            }
        }
    }

    /// The pointer touched an edge at `at` (on its surface): the dock comes
    /// over to that screen — where another screen goes on across, once the
    /// touch is told apart.
    pub(crate) fn dock_edge_touched(&mut self, surface: &WlSurface, at: (f64, f64)) {
        let Some(edge) = self.dock_screens.edges.iter().find(|e| e.layer.wl_surface() == surface) else {
            return;
        };
        let output = edge.output.clone();
        // (The edge stands centred on its screen's edge.)
        let screen_len = self.output_state.info(&output).and_then(|i| i.logical_size).map(|(w, h)| match edge.side {
            Side::Bottom => f64::from(w),
            Side::Left | Side::Right => f64::from(h),
        });
        let start = screen_len.map_or(0.0, |l| (l - f64::from(edge.len)) / 2.0);
        let along = start
            + match edge.side {
                Side::Bottom => at.0,
                Side::Left | Side::Right => at.1,
            };
        if edge.shared.iter().any(|&(lo, hi)| along >= lo && along < hi) {
            self.dock_touch_shared(Touch::Edge(output));
        } else {
            self.dock_edge_called(output);
        }
    }

    /// The pointer left an edge.
    pub(crate) fn dock_edge_left(&mut self) {
        if matches!(self.dock_screens.touch, Some(Touch::Edge(_))) {
            self.dock_screens.touch = None;
        }
        self.dock_screens.passing = false;
    }

    /// An edge called the dock: it comes over to that screen.
    fn dock_edge_called(&mut self, output: WlOutput) {
        if self.dock_suppressed() || self.dock_screens.moving.is_some() {
            return;
        }
        info!(
            "dock: the pointer at the edge of {}: made again there",
            self.output_state.info(&output).and_then(|i| i.name).unwrap_or_default()
        );
        self.move_dock_to(Some(output), vec![Command::Show], false);
    }

    /// The compositor moved its focus to screen `name` (`focusedmon`: the
    /// pointer crossed over, or something there took the focus).
    pub(crate) fn screen_focused(&mut self, name: &str) {
        self.dock_screens.focused = Some((name.to_owned(), Instant::now()));
    }

    /// The pointer touched a stretch another screen goes on across: told
    /// apart once the news of a crossing has had `CROSS_WAIT` to come.
    pub(crate) fn dock_touch_shared(&mut self, touch: Touch) {
        if self.dock_screens.passing || self.dock_screens.touch.is_some() {
            return;
        }
        self.dock_screens.touch = Some(touch.clone());
        let armed = self.loop_handle.insert_source(Timer::from_duration(CROSS_WAIT), move |_, _, app: &mut App| {
            if app.dock_screens.touch.as_ref() == Some(&touch) {
                app.dock_screens.touch = None;
                app.dock_touch_judged(touch.clone());
            }
            TimeoutAction::Drop
        });
        if let Err(e) = armed {
            // (No wait to be had: a touch is a call, as on a lone screen.)
            warn!("dock: no timer to tell a touch apart ({e})");
            if let Some(touch) = self.dock_screens.touch.take() {
                self.dock_touch_judged(touch);
            }
        }
    }

    /// The pointer left the dock's surface: a touch waiting there passed on
    /// through, and the next one is told apart afresh.
    pub(crate) fn dock_touch_left(&mut self, touch: Touch) {
        if self.dock_screens.touch.as_ref() == Some(&touch) {
            self.dock_screens.touch = None;
        }
        self.dock_screens.passing = false;
    }

    /// A touch told apart: the pointer came over from the other screen
    /// (passing), or it went to the edge on this one (a call).
    fn dock_touch_judged(&mut self, touch: Touch) {
        let screen = match &touch {
            Touch::Dock => self.dock_screen_name(),
            Touch::Edge(output) => self.output_state.info(output).and_then(|i| i.name),
        };
        let focused = self.dock_screens.focused.as_ref().map(|(name, at)| (name.as_str(), at.elapsed()));
        if came_over(focused, screen.as_deref()) {
            info!("dock: the pointer came over to {} from another screen: not a call", screen.unwrap_or_default());
            self.dock_screens.passing = true;
            return;
        }
        match touch {
            // (Still on the strip and still hidden: else it was let be.)
            Touch::Dock => {
                if self.ui.target() == Target::Hidden && self.pointer_on_reveal_strip() {
                    self.hide_deadline = None;
                    self.handle_command(Command::Show);
                }
            }
            Touch::Edge(output) => self.dock_edge_called(output),
        }
    }

    /// Keep an edge on every chosen screen the dock is not on, along the
    /// dock's side, and the dock itself off a screen not chosen (it goes
    /// home). Run when the screens, the choice or the side change, and when
    /// the dock moves.
    pub(crate) fn sync_edges(&mut self) {
        let two = self.output_state.outputs().nth(1).is_some();
        let screens = if two { crate::display::monitors() } else { Vec::new() };
        // Where the screens go on across the dock's edge (none with one).
        let all = if two { crate::hypr::all_monitors() } else { Vec::new() };
        let side_now = self.dock_side();
        self.dock_screens.shared = self.dock_screen_name().map_or_else(Vec::new, |name| shared_stretches(&all, &name, side_now));
        let off = &self.settings.launcher_screens_off;
        let chosen: Vec<WlOutput> = (0..screens.len())
            .filter(|&i| has_launcher(&screens, i, off))
            .filter_map(|i| self.output_named(&screens[i].name))
            .collect();
        // The dock on a screen not chosen (turned off, or it started there),
        // once it is not up there (the box a screen is turned off in stays
        // until it closes; its hide brings this back).
        let up = self.ui.target() == Target::Open || self.fast.open;
        if self.dock_screens.moving.is_none() && !chosen.is_empty() && !up {
            if let Some(here) = self.dock_screens.output.clone() {
                if !chosen.contains(&here) {
                    let home = self.main_output();
                    if home.is_some() && home.as_ref() != Some(&here) {
                        info!("dock: its screen is not chosen; made again on the main one");
                        self.move_dock_to(home, Vec::new(), false);
                    }
                }
            }
        }
        let side = self.dock_side();
        let dock_on = self
            .dock_screens
            .moving
            .as_ref()
            .and_then(|m| m.output.clone())
            .or_else(|| self.dock_screens.output.clone());
        let want: Vec<WlOutput> = if self.config.input.edge_reveal {
            chosen.into_iter().filter(|o| Some(o) != dock_on.as_ref()).collect()
        } else {
            Vec::new()
        };
        self.dock_screens.edges.retain(|e| e.side == side && want.contains(&e.output));
        let Some(kit) = self.screen_kit.as_ref() else {
            return;
        };
        let depth = self.config.input.edge_reveal_px.max(1);
        for output in want {
            if self.dock_screens.edges.iter().any(|e| e.output == output) {
                continue;
            }
            let len = self.edge_len(&output, side);
            let layer = edge_new(&self.compositor, &kit.shell, &self.qh, &output, side, depth, len);
            self.dock_screens.edges.push(Edge { layer, output, side, len, size: None, glow: false, shared: Vec::new() });
        }
        // (A screen turned or resized: the edges kept are as long as it lets them.)
        self.resize_edges();
        // Where another screen goes on across each edge.
        for i in 0..self.dock_screens.edges.len() {
            let name = self.output_state.info(&self.dock_screens.edges[i].output).and_then(|i| i.name).unwrap_or_default();
            self.dock_screens.edges[i].shared = shared_stretches(&all, &name, side);
        }
    }
}

impl App {
    /// How long an edge on `output` is: as long as the dock's resting plate
    /// (as its own strip is), never longer than that screen.
    fn edge_len(&self, output: &WlOutput, side: Side) -> u32 {
        let len = if self.dock_basin_w < 1.0 {
            self.config.window.width as f32 * self.icon_scale()
        } else {
            self.dock_basin_w
        }
        .round()
        .max(1.0) as u32;
        let along = |size: (f32, f32)| match side {
            Side::Bottom => size.0,
            Side::Left | Side::Right => size.1,
        };
        let Some(there) = self
            .output_state
            .info(output)
            .and_then(|i| i.logical_size)
            .map(|(w, h)| along((w as f32, h as f32)).max(1.0) as u32)
        else {
            return len;
        };
        // A dock that spans its own screen spans this one too (its length
        // was measured on another).
        let here = self.dock_screen_size().map(along);
        if here.is_some_and(|h| len as f32 >= 0.9 * h) {
            return there;
        }
        len.min(there)
    }

    /// The dock's plate changed length: the edges follow (while it rests —
    /// an open box gathers the plate to it).
    pub(crate) fn resize_edges(&mut self) {
        if self.dock_screens.edges.is_empty() || self.ui.target() == Target::Open {
            return;
        }
        let depth = self.config.input.edge_reveal_px.max(1);
        for i in 0..self.dock_screens.edges.len() {
            let (output, side) = (self.dock_screens.edges[i].output.clone(), self.dock_screens.edges[i].side);
            let len = self.edge_len(&output, side);
            let edge = &mut self.dock_screens.edges[i];
            if edge.len == len {
                continue;
            }
            edge.len = len;
            match side {
                Side::Bottom => edge.layer.set_size(len, depth),
                Side::Left | Side::Right => edge.layer.set_size(depth, len),
            }
            // (Its configure repaints it at the new size.)
            edge.layer.commit();
        }
    }
}

/// Let a dock surface go: its scale and visible-region objects first, then
/// the surface (dropping the layer destroys it).
fn drop_surface(layer: LayerSurface, fscale: Option<SurfaceScale>, visible: Option<SurfaceVisible>) {
    if let Some(f) = fscale {
        f.destroy();
    }
    if let Some(v) = visible {
        v.destroy();
    }
    drop(layer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Mode, Monitor};

    fn screen(name: &str, desc: &str, pos: (f64, f64)) -> Monitor {
        Monitor {
            name: name.into(),
            desc: desc.into(),
            model: String::new(),
            mode: Mode { w: 1920, h: 1080, hz: 60.0 },
            scale: 1.0,
            modes: Vec::new(),
            focused: false,
            pos,
            transform: 0,
            mm: (0, 0),
        }
    }

    #[test]
    fn the_main_screen_always_has_the_launcher() {
        let screens = [screen("DP-1", "Dell U2720Q", (0.0, -1080.0)), screen("eDP-1", "", (0.0, 0.0))];
        let off = vec!["desc:Dell U2720Q".to_owned(), "eDP-1".to_owned()];
        assert!(!has_launcher(&screens, 0, &off));
        // (Turned off by an old choice or not, the built-in keeps it.)
        assert!(has_launcher(&screens, 1, &off));
        assert!(has_launcher(&screens, 0, &[]));
    }

    #[test]
    fn with_the_built_in_off_the_one_at_the_origin_keeps_it() {
        let screens = [screen("DP-2", "LG", (1920.0, 0.0)), screen("DP-1", "Dell", (0.0, 0.0))];
        let off = vec!["desc:LG".to_owned(), "desc:Dell".to_owned()];
        assert!(!has_launcher(&screens, 0, &off));
        assert!(has_launcher(&screens, 1, &off));
    }

    #[test]
    fn an_edge_is_clear_or_lit_from_the_screen_edge_inward() {
        assert!(edge_pixels(4, 3, Side::Bottom, false).iter().all(|&b| b == 0));
        let px = edge_pixels(1, 3, Side::Bottom, true);
        let alpha = |row: usize| px[row * 4 + 3];
        // Brightest on the last row (the screen's edge), fading up.
        assert!(alpha(2) > alpha(1) && alpha(1) > alpha(0));
        // Premultiplied white: every channel equals the alpha.
        assert!(px.chunks(4).all(|p| p[0] == p[3] && p[1] == p[3] && p[2] == p[3]));
        let right = edge_pixels(3, 1, Side::Right, true);
        assert!(right[2 * 4 + 3] > right[3]);
        let left = edge_pixels(3, 1, Side::Left, true);
        assert!(left[3] > left[2 * 4 + 3]);
    }

    fn mon(name: &str, x: f64, y: f64, w: f64, h: f64) -> crate::hypr::MonitorInfo {
        crate::hypr::MonitorInfo { x, y, w, h, reserved: (0.0, 0.0, 0.0, 0.0), active_ws: 1, scale: 1.0, name: name.to_owned() }
    }

    #[test]
    fn where_another_screen_goes_on_across_the_docks_edge_is_found() {
        // Max's (2026-10-21): the TV over the laptop, a little wider.
        let all = [mon("eDP-1", 0.0, 0.0, 2000.0, 1250.0), mon("HDMI-A-1", -24.0, -1080.0, 2048.0, 1080.0)];
        // The TV's bottom edge: the laptop goes on across 24..2024 of it.
        assert_eq!(shared_stretches(&all, "HDMI-A-1", Side::Bottom), vec![(24.0, 2024.0)]);
        // The laptop's bottom: nothing under it — a strip all along.
        assert!(shared_stretches(&all, "eDP-1", Side::Bottom).is_empty());
        // A screen on the laptop's left: its left edge is the laptop's
        // right… the laptop's LEFT edge is shared where they face.
        let side = [mon("eDP-1", 0.0, 0.0, 2000.0, 1250.0), mon("DP-1", -1920.0, 200.0, 1920.0, 1080.0)];
        assert_eq!(shared_stretches(&side, "eDP-1", Side::Left), vec![(200.0, 1250.0)]);
        assert!(shared_stretches(&side, "eDP-1", Side::Right).is_empty());
        assert_eq!(shared_stretches(&side, "DP-1", Side::Right), vec![(0.0, 1050.0)]);
    }

    #[test]
    fn a_touch_right_after_the_focus_came_to_its_screen_is_the_pointer_coming_over() {
        let ms = Duration::from_millis;
        // Up from the laptop into the TV: the focus came to the TV as it crossed.
        assert!(came_over(Some(("HDMI-A-1", ms(85))), Some("HDMI-A-1")));
        // Down to the TV's edge from on the TV: the focus came there long ago.
        assert!(!came_over(Some(("HDMI-A-1", ms(5000))), Some("HDMI-A-1")));
        // The focus went to the other screen: this touch is not its crossing.
        assert!(!came_over(Some(("eDP-1", ms(85))), Some("HDMI-A-1")));
        // Nothing known: a call, as on a lone screen.
        assert!(!came_over(None, Some("HDMI-A-1")));
        assert!(!came_over(Some(("HDMI-A-1", ms(85))), None));
    }

    #[test]
    fn only_the_calls_that_bring_it_up_follow() {
        assert!(follows(&Command::Toggle));
        assert!(follows(&Command::FastLaunch));
        assert!(!follows(&Command::Show));
        assert!(!follows(&Command::Hide));
        assert!(!follows(&Command::Collapse));
    }
}

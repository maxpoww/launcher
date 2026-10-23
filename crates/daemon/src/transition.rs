//! A change of scale, dissolved.
//!
//! Changing a screen's scale is not one change but a pile of them: the
//! compositor rescales the output at once, every client (this one included)
//! is then shown with its OLD buffer stretched to the new scale until it has
//! drawn a new one, layer surfaces are resized, tiled windows re-flow, the
//! card re-lays its pills. Each lands on its own frame, and together they
//! read as the screen jumping (Max, 2026-10-01: "it jumps as crazy switching
//! scales").
//!
//! None of that can be made to land at once, so none of it is shown. Before
//! the change:
//!
//! 1. the screen is captured (`wlr-screencopy`, the way the colour match
//!    reads it),
//! 2. that still is laid over everything on an overlay layer surface — the
//!    very pixels that are on the screen, so nothing is seen to happen,
//! 3. the change is made underneath,
//! 4. after a beat for everything to settle, the still fades out
//!    (`wp_alpha_modifier_v1`: the compositor does the blending).
//!
//! What is seen is one dissolve from the old screen to the new.
//!
//! The same, slower, carries the LIGHT across its two looks
//! (`dissolve_slow`, `lighting.rs`): a still on EVERY screen, held while the
//! apps take their new colours, faded out over 2.5 s.
//!
//! **Under the still, nothing glides.** The compositor slides every layer
//! surface to its new place when a monitor is re-arranged (`layers`
//! animation, ~0.4 s) and tiled windows fly to their new sizes; our own card
//! eases too. All of it would still be moving when the still lifts and read
//! as a second, late move. So for as long as the still is up, two named
//! compositor rules turn animation off for the shell's layers and for
//! windows (the animation tick honours `no_anim`, so even a slide already
//! started lands at once), and our own motion snaps (`reduce_motion`).
//!
//! **The still itself must not jump.** Its surface is as large as the output
//! in LOGICAL pixels, and that is exactly what a scale change redefines: left
//! alone, the compositor would show it magnified (or shrunk) until the
//! configure for the new size had been answered. So the size it will have at
//! the new scale is committed just BEFORE the change is asked for: the
//! compositor takes a commit as it arrives but applies a monitor rule at the
//! start of its next frame, so both are in that frame.
//!
//! The still is plain shm with a viewport — no renderer, no GPU surface to
//! set up — and it takes no input: a click during the dissolve reaches what
//! is really there.
//!
//! Whatever goes wrong (no protocol, a capture that never arrives, a
//! configure that never comes), the change is made anyway, just not
//! dissolved, and the still can never outlive [`HARD_CAP`].

use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::compositor::Region;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::raw::RawPool;
use tracing::{debug, info, warn};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1;
use wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_v1::WpAlphaModifierV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};

use crate::{hypr, App};

/// The overlay's layer namespace (its compositor rule: no map animation).
const NAMESPACE: &str = "golem-transition";

/// While the still is up: the shell's layers and every window land where
/// they belong at once instead of gliding there. Named rules, so turning
/// them on and off is the same two rules each time.
fn still_rules(on: bool) {
    hypr::eval(&format!(
        "hl.layer_rule({{ name = \"golem-transition-shell\", match = {{ namespace = \"^waverunner\" }}, no_anim = true, enabled = {on} }}); \
         hl.window_rule({{ name = \"golem-transition-windows\", match = {{ class = \".*\" }}, no_anim = true, enabled = {on} }})"
    ));
}
/// The still must be on the screen this soon, or the change is made bare.
const COVER_DEADLINE: Duration = Duration::from_millis(300);
/// How long the still holds after the change, for the screen under it to
/// settle (our own surfaces re-laid, the wallpaper redrawn).
const HOLD: Duration = Duration::from_millis(180);
/// The fade from the still to the new screen.
const FADE: Duration = Duration::from_millis(260);
const FADE_TICK: Duration = Duration::from_millis(16);
/// The still is taken down by now whatever happened: a frozen picture left
/// over the desktop would look like a hang.
const HARD_CAP: Duration = Duration::from_millis(2000);
/// THE SLOW DISSOLVE (`dissolve_slow`), for the light crossing between the
/// looks: the apps' new look shows through the still as a light dims, never
/// in one frame (Max, 2026-10-21: the terminal went white to black in half a
/// second, "aggressive").
const SLOW_HOLD: Duration = Duration::from_millis(300);
/// (8 s: in 2.5 a white screen going black read as a jump — Max,
/// 2026-10-22, at the first dusk.)
const SLOW_FADE: Duration = Duration::from_millis(8000);

/// What is done under the still.
pub(crate) type Then = Box<dyn FnOnce(&mut App)>;

/// The globals a dissolve needs, bound once. Any may be missing: then
/// changes are simply not dissolved.
pub(crate) struct Veil {
    layer_shell: LayerShell,
    pub(crate) viewporter: Option<WpViewporter>,
    alpha: Option<WpAlphaModifierV1>,
}

impl Veil {
    pub(crate) fn bind(globals: &GlobalList, qh: &QueueHandle<App>, layer_shell: LayerShell) -> Self {
        Self {
            layer_shell,
            viewporter: globals.bind::<WpViewporter, App, _>(qh, 1..=1, ()).ok(),
            alpha: globals.bind::<WpAlphaModifierV1, App, _>(qh, 1..=1, ()).ok(),
        }
    }
}

/// Marks a screencopy frame as a dissolve's (the colour match's use `()`).
pub(crate) struct Shot;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    /// Waiting for the capture.
    Shooting,
    /// The overlay is created; waiting for its size.
    Mapping,
    /// The still is committed; waiting for the frame that shows it.
    Presenting,
    /// On the screen.
    Shown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// The stills are being put up.
    Preparing,
    /// Every still is up, the change made under them.
    Covered,
    Fading(Instant),
}

/// One screen's still.
struct Still {
    output: WlOutput,
    /// The scale the output is about to have (the still's size is worked out
    /// from it before the change); None: it keeps its own.
    new_scale: Option<f64>,
    stage: Stage,
    frame: Option<ZwlrScreencopyFrameV1>,
    /// Whether the engine was told a capture of ours is running.
    capturing: bool,
    pool: Option<RawPool>,
    buffer: Option<WlBuffer>,
    /// The still, in pixels.
    px: (u32, u32),
    format: wl_shm::Format,
    y_invert: bool,
    layer: Option<LayerSurface>,
    viewport: Option<WpViewport>,
    alpha: Option<WpAlphaModifierSurfaceV1>,
}

impl Still {
    fn new(output: WlOutput, new_scale: Option<f64>, frame: ZwlrScreencopyFrameV1) -> Self {
        Self {
            output,
            new_scale,
            stage: Stage::Shooting,
            frame: Some(frame),
            capturing: true,
            pool: None,
            buffer: None,
            px: (0, 0),
            format: wl_shm::Format::Xrgb8888,
            y_invert: false,
            layer: None,
            viewport: None,
            alpha: None,
        }
    }

    /// Take it down. The surface goes first, so the buffer is no longer in
    /// use when it is destroyed.
    fn teardown(mut self) {
        if self.capturing {
            options_engine::end_self_capture();
        }
        if let Some(alpha) = self.alpha.take() {
            alpha.destroy();
        }
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        drop(self.layer.take());
        if let Some(frame) = self.frame.take() {
            frame.destroy();
        }
        if let Some(buffer) = self.buffer.take() {
            buffer.destroy();
        }
    }
}

/// A dissolve in flight: a still on each screen it covers (the screen whose
/// scale changes; every screen for the light's look).
pub(crate) struct Dissolve {
    /// Tells this run's timers from a later run's.
    id: u64,
    then: Option<Then>,
    /// The light's slow pace (`SLOW_HOLD`, `SLOW_FADE`), not a scale's.
    slow: bool,
    phase: Phase,
    stills: Vec<Still>,
}

impl Dissolve {
    /// Take every still down; what was to be done under them, if not done.
    fn teardown(mut self) -> Option<Then> {
        for still in self.stills.drain(..) {
            still.teardown();
        }
        self.then.take()
    }

    fn of_frame(&self, frame: &ZwlrScreencopyFrameV1) -> Option<usize> {
        self.stills.iter().position(|s| s.frame.as_ref() == Some(frame))
    }

    fn of_surface(&self, surface: &WlSurface) -> Option<usize> {
        self.stills.iter().position(|s| s.layer.as_ref().is_some_and(|l| l.wl_surface() == surface))
    }

    /// Every still left is on its screen.
    fn all_shown(&self) -> bool {
        self.stills.iter().all(|s| s.stage == Stage::Shown)
    }
}

/// The size, in logical pixels, of an output `px` pixels large at `scale` —
/// rounded as the compositor rounds it.
fn logical(px: (u32, u32), scale: f64) -> (i32, i32) {
    let side = |p: u32| ((f64::from(p) / scale).round() as i32).max(1);
    (side(px.0), side(px.1))
}

/// The fade's curve: the still's opacity `t` (0..1) of the way through.
fn opacity(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

fn opaque_format(format: wl_shm::Format) -> bool {
    matches!(format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Xbgr8888)
}

fn supported_format(format: wl_shm::Format) -> bool {
    opaque_format(format) || matches!(format, wl_shm::Format::Argb8888 | wl_shm::Format::Abgr8888)
}

impl App {
    /// Run `then` — a change that sets `output_name`'s scale to `new_scale` —
    /// under a still of the screen, and dissolve to the result. Without the
    /// means to (or with motion reduced) `then` just runs.
    pub(crate) fn dissolve(&mut self, output_name: &str, new_scale: f64, then: Then) {
        let over = self.output_by_name(output_name).map(|o| vec![(o, Some(new_scale))]).unwrap_or_default();
        self.dissolve_over(over, then, false);
    }

    /// The light's dissolve: `then` — a change of the look, of no screen's
    /// size — under a still of EVERY screen, held a moment longer for the
    /// apps to take their new look, then faded out slowly (a screen left
    /// bare was a cut in one frame — Max, 2026-10-21: "no jumps at all").
    pub(crate) fn dissolve_slow(&mut self, then: Then) {
        let over = self.output_state.outputs().map(|o| (o, None)).collect();
        self.dissolve_over(over, then, true);
    }

    fn dissolve_over(&mut self, over: Vec<(WlOutput, Option<f64>)>, then: Then, slow: bool) {
        // A second change while one dissolves: the first is finished at once.
        self.dissolve_end();
        let ready = !crate::animation::reduce_motion()
            && self.veil.viewporter.is_some()
            && self.veil.alpha.is_some()
            && self.shm.is_some()
            && !over.is_empty();
        let (true, Some(mgr)) = (ready, self.screencopy.clone()) else {
            then(self);
            return;
        };
        self.dissolve_runs += 1;
        let id = self.dissolve_runs;
        let stills = over
            .into_iter()
            .map(|(output, new_scale)| {
                // Ours, not a screen share (see `start_options_capture`).
                options_engine::begin_self_capture();
                let frame = mgr.capture_output(0, &output, &self.qh, Shot);
                Still::new(output, new_scale, frame)
            })
            .collect();
        self.dissolve = Some(Dissolve { id, then: Some(then), slow, phase: Phase::Preparing, stills });
        // Not covered in time: what is up covers its screen, and the change
        // is made rather than late.
        self.dissolve_timer(id, COVER_DEADLINE, |app| {
            if app.dissolve.as_ref().is_some_and(|d| d.phase == Phase::Preparing) {
                app.dissolve_cover_late();
            }
        });
        let cap = if slow { HARD_CAP + SLOW_HOLD + SLOW_FADE } else { HARD_CAP };
        self.dissolve_timer(id, cap, move |app| {
            warn!("dissolve: still up after {cap:?}; taking it down");
            app.dissolve_end();
        });
    }

    /// Run `f` after `after`, if dissolve `id` is still the one in flight.
    fn dissolve_timer(&mut self, id: u64, after: Duration, f: impl Fn(&mut App) + 'static) {
        let armed = self.loop_handle.insert_source(Timer::from_duration(after), move |_, _, app: &mut App| {
            if app.dissolve.as_ref().is_some_and(|d| d.id == id) {
                f(app);
            }
            TimeoutAction::Drop
        });
        if let Err(e) = armed {
            // A dissolve that cannot be timed is not worth the risk.
            warn!("dissolve: cannot arm a timer ({e})");
            self.dissolve_end();
        }
    }

    /// End the dissolve now: the stills go, and what was to be done under
    /// them is done if it has not been.
    pub(crate) fn dissolve_end(&mut self) {
        if let Some(d) = self.dissolve.take() {
            // The stills were up: motion is given back, ours and the
            // compositor's.
            if d.phase != Phase::Preparing {
                crate::animation::set_reduce_motion(self.config.accessibility.reduce_motion);
                still_rules(false);
            }
            if let Some(then) = d.teardown() {
                then(self);
            }
        }
    }

    /// Whether `surface` is a still's.
    pub(crate) fn is_dissolve_surface(&self, surface: &WlSurface) -> bool {
        self.dissolve.as_ref().is_some_and(|d| d.of_surface(surface).is_some())
    }

    /// The capture's `buffer` event: somewhere for the frame to go.
    fn dissolve_shot_buffer(&mut self, frame: &ZwlrScreencopyFrameV1, format: WEnum<wl_shm::Format>, width: u32, height: u32, stride: u32) {
        let (Some(d), Some(shm)) = (self.dissolve.as_mut(), self.shm.as_ref()) else {
            return;
        };
        let Some(i) = d.of_frame(frame) else {
            return;
        };
        let s = &mut d.stills[i];
        let WEnum::Value(format) = format else {
            return;
        };
        let len = (height as usize).saturating_mul(stride as usize);
        if s.buffer.is_some() || !supported_format(format) || len == 0 {
            return;
        }
        let mut pool = match RawPool::new(len, shm) {
            Ok(pool) => pool,
            Err(e) => {
                warn!("dissolve: no shm pool for the still ({e})");
                return;
            }
        };
        let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, (), &self.qh);
        frame.copy(&buffer);
        s.pool = Some(pool);
        s.buffer = Some(buffer);
        s.px = (width, height);
        s.format = format;
    }

    /// A capture is in its buffer: lay it over its screen.
    fn dissolve_shot_ready(&mut self, frame: &ZwlrScreencopyFrameV1) {
        let output = {
            let Some(d) = self.dissolve.as_mut() else {
                return;
            };
            let Some(i) = d.of_frame(frame) else {
                return;
            };
            let s = &mut d.stills[i];
            if s.capturing {
                options_engine::end_self_capture();
                s.capturing = false;
            }
            if s.stage != Stage::Shooting || s.buffer.is_none() {
                return;
            }
            // A format with an alpha channel: the capture's alpha is not
            // ours to trust, and the still must hide what is under it.
            if !opaque_format(s.format) {
                if let Some(pool) = s.pool.as_mut() {
                    for px in pool.mmap().chunks_exact_mut(4) {
                        px[3] = 0xFF;
                    }
                }
            }
            s.output.clone()
        };
        // The still appears at once and whole: a map animation would show
        // the screen sliding or fading over itself.
        hypr::eval(&format!(
            "hl.layer_rule({{ name = \"{NAMESPACE}\", match = {{ namespace = \"{NAMESPACE}\" }}, no_anim = true }})"
        ));
        let region = match Region::new(&self.compositor) {
            Ok(region) => region,
            Err(e) => {
                warn!("dissolve: no input region ({e})");
                self.dissolve_end();
                return;
            }
        };
        let surface = self.compositor.create_surface(&self.qh);
        let layer =
            self.veil
                .layer_shell
                .create_layer_surface(&self.qh, surface, Layer::Overlay, Some(NAMESPACE), Some(&output));
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        // Over the bars' reserved zones too, not laid out around them.
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // No input: a click goes to what is really there.
        layer.wl_surface().set_input_region(Some(region.wl_region()));
        layer.commit();
        if let Some(d) = self.dissolve.as_mut() {
            if let Some(i) = d.of_frame(frame) {
                d.stills[i].layer = Some(layer);
                d.stills[i].stage = Stage::Mapping;
            }
        }
    }

    /// A capture failed: that screen goes without a still; with none left,
    /// the change is made bare.
    fn dissolve_shot_failed(&mut self, frame: &ZwlrScreencopyFrameV1) {
        let (none, ready) = {
            let Some(d) = self.dissolve.as_mut() else {
                return;
            };
            let Some(i) = d.of_frame(frame) else {
                return;
            };
            d.stills.remove(i).teardown();
            (d.stills.is_empty(), d.phase == Phase::Preparing && d.all_shown())
        };
        if none {
            warn!("dissolve: the capture failed; changing without it");
            self.dissolve_end();
        } else {
            warn!("dissolve: a screen's capture failed; that screen changes without its still");
            if ready {
                self.dissolve_cover();
            }
        }
    }

    /// A `configure` for a still's surface.
    pub(crate) fn configure_dissolve(&mut self, wl: &WlSurface, configure: LayerSurfaceConfigure) {
        let (w, h) = configure.new_size;
        let qh = self.qh.clone();
        let (Some(d), Some(viewporter), Some(alpha)) =
            (self.dissolve.as_mut(), self.veil.viewporter.as_ref(), self.veil.alpha.as_ref())
        else {
            return;
        };
        let Some(i) = d.of_surface(wl) else {
            return;
        };
        let s = &mut d.stills[i];
        let Some(layer) = s.layer.as_ref() else {
            return;
        };
        if w == 0 || h == 0 {
            return;
        }
        let surface = layer.wl_surface();
        if s.stage != Stage::Mapping {
            // The output's logical size changed under the still (our own
            // doing): keep covering it.
            if let Some(viewport) = &s.viewport {
                viewport.set_destination(w as i32, h as i32);
            }
            surface.commit();
            return;
        }
        let Some(buffer) = s.buffer.as_ref() else {
            return;
        };
        // The still's pixels are the screen's pixels: shown at the output's
        // logical size they land one to one.
        let viewport = viewporter.get_viewport(surface, &qh, ());
        viewport.set_destination(w as i32, h as i32);
        let alpha = alpha.get_surface(surface, &qh, ());
        alpha.set_multiplier(u32::MAX);
        surface.set_buffer_scale(1);
        if s.y_invert {
            surface.set_buffer_transform(wl_output::Transform::Flipped180);
        }
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(0, 0, s.px.0 as i32, s.px.1 as i32);
        // The frame that shows it is its cue.
        surface.frame(&qh, surface.clone());
        surface.commit();
        s.viewport = Some(viewport);
        s.alpha = Some(alpha);
        s.stage = Stage::Presenting;
    }

    /// A still is on its screen; once every one is, the change is made.
    pub(crate) fn dissolve_covered(&mut self, wl: &WlSurface) {
        let ready = {
            let Some(d) = self.dissolve.as_mut() else {
                return;
            };
            let Some(i) = d.of_surface(wl) else {
                return;
            };
            if d.stills[i].stage != Stage::Presenting {
                return;
            }
            d.stills[i].stage = Stage::Shown;
            d.phase == Phase::Preparing && d.all_shown()
        };
        if ready {
            self.dissolve_cover();
        }
    }

    /// The deadline came with stills still on their way: the ones up cover
    /// their screens and the change is made; with none up, it is made bare.
    fn dissolve_cover_late(&mut self) {
        let none = {
            let Some(d) = self.dissolve.as_mut() else {
                return;
            };
            let (shown, late): (Vec<Still>, Vec<Still>) = d.stills.drain(..).partition(|s| s.stage == Stage::Shown);
            d.stills = shown;
            for still in late {
                still.teardown();
            }
            d.stills.is_empty()
        };
        if none {
            warn!("dissolve: the still was not up in time; changing without it");
            self.dissolve_end();
        } else {
            warn!("dissolve: a screen's still was not up in time; that screen changes without it");
            self.dissolve_cover();
        }
    }

    /// The stills are up: make the change under them.
    fn dissolve_cover(&mut self) {
        let (id, then, slow) = {
            let Some(d) = self.dissolve.as_mut() else {
                return;
            };
            if d.phase != Phase::Preparing {
                return;
            }
            d.phase = Phase::Covered;
            // The size a still has at its screen's new scale, committed
            // before the change is asked for: both land in the compositor's
            // next frame.
            for s in &d.stills {
                if let (Some(scale), Some(viewport), Some(layer)) = (s.new_scale, &s.viewport, &s.layer) {
                    let (w, h) = logical(s.px, scale);
                    viewport.set_destination(w, h);
                    layer.wl_surface().commit();
                }
            }
            (d.id, d.then.take(), d.slow)
        };
        if let Err(e) = self.conn.flush() {
            debug!("dissolve: flush failed: {e}");
        }
        // Under the still nothing glides to its new place (see the module
        // doc): the compositor's rules, and our own motion.
        still_rules(true);
        crate::animation::set_reduce_motion(true);
        if let Some(then) = then {
            then(self);
        }
        self.dissolve_timer(id, if slow { SLOW_HOLD } else { HOLD }, move |app| app.dissolve_fade(id));
    }

    /// Fade the stills out, a tick at a time, then take them down.
    fn dissolve_fade(&mut self, id: u64) {
        let t = {
            let Some(d) = self.dissolve.as_mut() else {
                return;
            };
            let from = match d.phase {
                Phase::Fading(from) => from,
                _ => {
                    let now = Instant::now();
                    d.phase = Phase::Fading(now);
                    now
                }
            };
            let fade = if d.slow { SLOW_FADE } else { FADE };
            let t = from.elapsed().as_secs_f32() / fade.as_secs_f32();
            if t < 1.0 {
                let multiplier = (f64::from(opacity(t)) * f64::from(u32::MAX)) as u32;
                for s in &d.stills {
                    if let (Some(alpha), Some(layer)) = (&s.alpha, &s.layer) {
                        alpha.set_multiplier(multiplier);
                        layer.wl_surface().commit();
                    }
                }
            }
            t
        };
        if t >= 1.0 {
            info!("dissolve: done");
            self.dissolve_end();
        } else {
            self.dissolve_timer(id, FADE_TICK, move |app| app.dissolve_fade(id));
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, Shot> for App {
    fn event(
        app: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &Shot,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // A frame of a dissolve already ended, or of a still let go.
        let Some(i) = app.dissolve.as_ref().and_then(|d| d.of_frame(frame)) else {
            return;
        };
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer { format, width, height, stride } => {
                app.dissolve_shot_buffer(frame, format, width, height, stride);
            }
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                if let (Some(d), WEnum::Value(f)) = (app.dissolve.as_mut(), flags) {
                    d.stills[i].y_invert = f.bits() & 1 != 0;
                }
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => app.dissolve_shot_ready(frame),
            zwlr_screencopy_frame_v1::Event::Failed => app.dissolve_shot_failed(frame),
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(App: ignore WpAlphaModifierV1);
wayland_client::delegate_noop!(App: ignore WpAlphaModifierSurfaceV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_still_measures_what_the_output_will() {
        // The MacBook's panel at each scale it offers, and the dev box's.
        assert_eq!(logical((1440, 900), 1.0), (1440, 900));
        assert_eq!(logical((1440, 900), 1.25), (1152, 720));
        assert_eq!(logical((1440, 900), 100.0 / 120.0), (1728, 1080));
        assert_eq!(logical((1440, 900), 1.5), (960, 600));
        assert_eq!(logical((3200, 2000), 1.6), (2000, 1250));
    }

    #[test]
    fn the_fade_starts_opaque_ends_clear_and_never_turns_back() {
        assert_eq!(opacity(0.0), 1.0);
        assert_eq!(opacity(1.0), 0.0);
        assert_eq!(opacity(2.0), 0.0);
        let mut last = 1.0;
        for i in 1..=20 {
            let a = opacity(i as f32 / 20.0);
            assert!(a <= last, "opacity rises at step {i}");
            last = a;
        }
    }

    #[test]
    fn only_formats_the_still_can_wear_are_taken() {
        assert!(supported_format(wl_shm::Format::Xrgb8888) && opaque_format(wl_shm::Format::Xrgb8888));
        assert!(supported_format(wl_shm::Format::Argb8888) && !opaque_format(wl_shm::Format::Argb8888));
        assert!(!supported_format(wl_shm::Format::Xrgb2101010));
    }
}

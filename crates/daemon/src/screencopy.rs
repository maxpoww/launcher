//! Colour-match the OPTIONS bar to a maximized window.
//!
//! When Hyprland "smart gaps" leaves a single window filling the screen flush
//! under the bar (see [`crate::hypr::top_fill`]), we sample that window's top
//! row via the `wlr-screencopy` protocol and paint the bar that flat colour,
//! so window + bar read as one continuous surface. Otherwise the bar stays
//! its near-transparent self.
//!
//! Wayland forbids reading another window's pixels directly, so screencopy is
//! the only way. We capture the whole focused output into an shm buffer, read
//! the single physical row at the window's top, and average it. Captures are
//! event-driven (Hyprland layout events) plus a slow resample while matched
//! (for content whose top colour changes). Everything degrades gracefully:
//! without the protocol the bar simply never colour-matches.

use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::shm::raw::RawPool;
use tracing::{debug, warn};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_shm;
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::{
    self, ZwlrScreencopyManagerV1,
};

use crate::{hypr, App};

/// Poll cadence for re-evaluating the colour-match. Runs whenever the bar is
/// enabled — matched or not — so the bar converges on the current window
/// within one tick even if a compositor event was missed or arrived while the
/// layout was mid-animation. It doubles as the resample loop that tracks a
/// matched window whose content colour changes on the fly.
const POLL: Duration = Duration::from_millis(700);

/// Colour histogram for the dominant-colour (mode) sample: quantised RGB key →
/// (pixel count, r sum, g sum, b sum) so the winning bucket can be averaged.
type ColorHist = std::collections::HashMap<(u8, u8, u8), (u32, u32, u32, u32)>;

/// The output + row to sample for the current match.
pub(crate) struct CaptureTarget {
    output: WlOutput,
    sample_y: u32,
}

/// An in-flight screencopy of the focused output.
pub(crate) struct Capture {
    frame: ZwlrScreencopyFrameV1,
    buffer: Option<WlBuffer>,
    width: u32,
    height: u32,
    stride: u32,
    format: wl_shm::Format,
    y_invert: bool,
    sample_y: u32,
    copied: bool,
}

impl App {
    /// Re-evaluate whether the bar should colour-match, on layout changes:
    /// - fullscreen: match the fullscreen window's top colour, so when the bar
    ///   is revealed (dwell) it's opaque and blends instead of a transparent
    ///   strip over the app;
    /// - smart-gaps: match a maximized window flush under the bar;
    /// - otherwise: the transparent bar.
    pub(crate) fn reeval_options_bar(&mut self) {
        // Keep the safety-net poll alive whenever matching is possible, so a
        // missed event can never leave the bar stuck (blue wallpaper or a
        // stale colour). Idempotent — the pending guard collapses repeats.
        self.schedule_options_poll();
        if self.options_layer.is_none() || self.screencopy.is_none() {
            return;
        }
        if self.options_fullscreen {
            if let Ok(mon) = hypr::focused_monitor() {
                if let Some(output) = self.output_by_name(&mon.name) {
                    // Sample the fullscreen content a few px below the bar —
                    // past the bar's own drawn extent (bar_h + overhang) so a
                    // revealed opaque bar never samples itself.
                    let sample_y = ((self.config.options.height as f64 + 4.0)
                        * mon.scale.max(0.1))
                    .round() as u32;
                    self.begin_options_match(output, sample_y);
                    return;
                }
            }
            self.clear_options_match();
            return;
        }
        match hypr::top_fill(self.config.options.height as f64) {
            Some(tf) => match self.output_by_name(&tf.monitor) {
                Some(output) => self.begin_options_match(output, tf.sample_y),
                None => {
                    debug!("options: no wl_output named {}", tf.monitor);
                    self.clear_options_match();
                }
            },
            None => self.clear_options_match(),
        }
    }

    /// Set the colour-match target to `output`/`sample_y` and kick a capture.
    /// The always-on poll (see [`Self::schedule_options_poll`]) drives the
    /// resample cadence, so this doesn't schedule one itself.
    fn begin_options_match(&mut self, output: wayland_client::protocol::wl_output::WlOutput, sample_y: u32) {
        self.options_match = Some(CaptureTarget { output, sample_y });
        self.start_options_capture();
    }

    /// Drop any match and repaint the transparent bar.
    fn clear_options_match(&mut self) {
        let changed = self.options_match.take().is_some() | self.options_bar_matched.take().is_some();
        self.abort_capture();
        if changed {
            self.draw_options();
        }
    }

    fn abort_capture(&mut self) {
        if let Some(cap) = self.capture.take() {
            cap.frame.destroy();
            if let Some(buf) = cap.buffer {
                buf.destroy();
            }
        }
    }

    fn output_by_name(&self, name: &str) -> Option<WlOutput> {
        self.output_state
            .outputs()
            .find(|o| self.output_state.info(o).and_then(|i| i.name).as_deref() == Some(name))
    }

    /// Begin capturing the focused output (one capture at a time).
    pub(crate) fn start_options_capture(&mut self) {
        if self.capture.is_some() {
            return;
        }
        let Some(mgr) = self.screencopy.clone() else {
            return;
        };
        let Some(target) = self.options_match.as_ref() else {
            return;
        };
        let output = target.output.clone();
        let sample_y = target.sample_y;
        let frame = mgr.capture_output(0, &output, &self.qh, ());
        self.capture = Some(Capture {
            frame,
            buffer: None,
            width: 0,
            height: 0,
            stride: 0,
            format: wl_shm::Format::Xrgb8888,
            y_invert: false,
            sample_y,
            copied: false,
        });
    }

    /// `buffer` event: allocate the shm buffer and kick off the copy. Extra
    /// buffer offers (e.g. dmabuf) after we've picked an shm one are ignored.
    fn options_capture_buffer(
        &mut self,
        format: WEnum<wl_shm::Format>,
        width: u32,
        height: u32,
        stride: u32,
    ) {
        if self.capture.as_ref().is_none_or(|c| c.copied) {
            return;
        }
        let WEnum::Value(fmt) = format else {
            return;
        };
        if !supported_format(fmt) {
            return;
        }
        let needed = (height as usize).saturating_mul(stride as usize);
        if needed == 0 {
            return;
        }
        if self.shm_pool.is_none() {
            let Some(shm) = self.shm.as_ref() else {
                return;
            };
            match RawPool::new(needed.max(4096), shm) {
                Ok(p) => self.shm_pool = Some(p),
                Err(e) => {
                    warn!("options: shm pool alloc failed: {e}");
                    return;
                }
            }
        }
        let Some(pool) = self.shm_pool.as_mut() else {
            return;
        };
        if pool.len() < needed {
            if let Err(e) = pool.resize(needed) {
                warn!("options: shm pool resize failed: {e}");
                return;
            }
        }
        let buffer =
            pool.create_buffer(0, width as i32, height as i32, stride as i32, fmt, (), &self.qh);
        if let Some(cap) = self.capture.as_mut() {
            cap.frame.copy(&buffer);
            cap.buffer = Some(buffer);
            cap.width = width;
            cap.height = height;
            cap.stride = stride;
            cap.format = fmt;
            cap.copied = true;
        }
    }

    /// `ready` event: the buffer holds the frame — sample the window's top
    /// row, paint the bar, and schedule the next resample.
    fn options_capture_ready(&mut self) {
        let Some(cap) = self.capture.take() else {
            return;
        };
        let color = self.read_sample(&cap);
        cap.frame.destroy();
        if let Some(buf) = cap.buffer {
            buf.destroy();
        }
        // Update the colour if we got one; the poll runs on its own timer
        // (see `schedule_options_poll`), so re-evaluation keeps going regardless.
        if self.options_match.is_some() {
            if let Some(color) = color {
                if self.options_bar_matched != Some(color) {
                    self.options_bar_matched = Some(color);
                    self.draw_options();
                }
            }
        }
    }

    fn options_capture_failed(&mut self) {
        self.abort_capture();
        debug!("options: screencopy failed");
    }

    /// Read the window's top strip into one opaque colour — the *dominant*
    /// colour there, so text, icons, a search box or a 1px highlight can't
    /// drag the match off the real header background. We **trim the horizontal
    /// edges** (where CSD rounding, the window border and a right-edge
    /// scrollbar live) and take the **mode** over a shallow band. See the
    /// bucket logic below for why mode beats mean/median on real windows.
    fn read_sample(&mut self, cap: &Capture) -> Option<[f32; 4]> {
        if cap.width == 0 || cap.height == 0 {
            return None;
        }
        let width = cap.width as usize;
        // Horizontal sampling deliberately skips two zones: the outer ~3% (CSD
        // rounding, the window border, a right-edge scrollbar) AND the central
        // third — where a browser's URL/search field or an app's centred title
        // sits. That centre block is a big patch of a *different* colour than
        // the surrounding chrome (Chrome's grey omnibox on its black toolbar;
        // Firefox's grey URL field on its white toolbar), and reading through
        // it is exactly what made the bar match the field instead of the
        // toolbar. Sampling only the sides reads the toolbar *background* — the
        // colour the eye takes as "the window's top" — on any app.
        let outer = (width / 33).clamp(4, 60);
        let cl = width * 34 / 100;
        let cr = width * 66 / 100;
        if width <= outer * 2 + 4 || cl <= outer || cr >= width - outer {
            return None;
        }
        let step = (width / 400).max(1);
        // While the notification drawer is open it paints its panel over the
        // window on the right, where we sample. Rather than freeze the match,
        // exclude just the box's own columns so we keep reading the live window
        // to either side of it — the bar recolours as you swipe workspaces with
        // the box open. The box hugs the right edge; its rect is in surface-
        // logical px, mapped to buffer columns by width / surface_width.
        let exclude: Option<(usize, usize)> = if self.notif.occludes_below_bar() {
            let sw = self.options_size.0 as f32;
            let r = self.notif_rect();
            if sw > 0.0 && r.w > 0.0 {
                let px = width as f32 / sw;
                let ex0 = (r.x * px).floor().max(0.0) as usize;
                let ex1 = (((r.x + r.w) * px).ceil() as usize + 1).min(width);
                (ex1 > ex0).then_some((ex0.saturating_sub(1), ex1))
            } else {
                None
            }
        } else {
            None
        };
        // `sample_y` is physical-from-top; in a y-inverted buffer that maps to a
        // row counted from the bottom. "Deeper into the window" is +dy from the
        // top, i.e. a smaller row index when inverted.
        let base = if cap.y_invert {
            cap.height.saturating_sub(1).saturating_sub(cap.sample_y)
        } else {
            cap.sample_y
        };

        let pool = self.shm_pool.as_mut()?;
        let map = pool.mmap();
        let bytes: &[u8] = &map[..];

        // Pool a short band of the toolbar and take the **dominant colour**
        // (statistical mode), not a median. A header is rarely one flat colour
        // at the pixel level — text, icons, tabs, a highlight line — and a
        // median blends those into a shade that matches nothing. The mode locks
        // onto the background that dominates the (side-sampled) band.
        //
        // Colours are quantised into 16-level buckets so near-identical shades
        // group; the winning bucket's members are then averaged for a precise
        // result rather than the coarse bucket key.
        let mut buckets: ColorHist = ColorHist::new();
        for dy in 0u32..=5 {
            let row = if cap.y_invert {
                base.saturating_sub(dy)
            } else {
                (base + dy).min(cap.height - 1)
            };
            let start = (row as usize) * (cap.stride as usize);
            let Some(rowbytes) = bytes.get(start..start + width * 4) else {
                continue;
            };
            let mut x = outer;
            while x < width - outer {
                // Jump over the central third (URL/search field, centred title).
                if x >= cl && x < cr {
                    x = cr;
                    continue;
                }
                // Jump over the notification box's columns when it's open.
                if let Some((ex0, ex1)) = exclude {
                    if x >= ex0 && x < ex1 {
                        x = ex1.max(x + step);
                        continue;
                    }
                }
                let (rr, gg, bb) = channels(cap.format, &rowbytes[x * 4..x * 4 + 4]);
                let e = buckets.entry((rr & 0xF0, gg & 0xF0, bb & 0xF0)).or_insert((0, 0, 0, 0));
                e.0 += 1;
                e.1 += rr as u32;
                e.2 += gg as u32;
                e.3 += bb as u32;
                x += step;
            }
        }
        let (_, &(n, rsum, gsum, bsum)) = buckets.iter().max_by_key(|(_, v)| v.0)?;
        if n == 0 {
            return None;
        }
        let (mr, mg, mb) = ((rsum / n) as u8, (gsum / n) as u8, (bsum / n) as u8);
        debug!("options: colour-match window top = #{mr:02x}{mg:02x}{mb:02x}");
        // The captured bytes are sRGB-encoded (display values), but the bar's
        // swapchain is an sRGB surface that re-encodes shader output — so we
        // must hand it the *linear* colour, or it comes out doubly-brightened
        // (a washed, greyish version of the real window colour).
        Some([
            srgb_to_linear(mr as f32 / 255.0),
            srgb_to_linear(mg as f32 / 255.0),
            srgb_to_linear(mb as f32 / 255.0),
            1.0,
        ])
    }

    /// Run the colour-match re-evaluation on a steady timer for as long as the
    /// bar is enabled — whether or not it's currently matched.
    ///
    /// This is the system's self-healing spine. Matching is otherwise driven
    /// by Hyprland layout events, but an event can be missed, or fire while a
    /// workspace-switch animation is still mid-flight (so the window isn't yet
    /// where IPC will report it a beat later). Either way the bar could stick —
    /// showing blurred wallpaper, or a stale colour from another window. The
    /// poll guarantees the bar reconverges on the true current window within
    /// one tick regardless. While matched it also serves as the resample loop,
    /// tracking a window whose content colour changes with no layout event.
    ///
    /// Self-sustaining: each tick reschedules the next, so a transient failed
    /// capture or empty read can't stop it. The pending guard keeps the
    /// event-driven and timer-driven callers from stacking duplicate timers.
    pub(crate) fn schedule_options_poll(&mut self) {
        if self.options_poll_pending || !self.config.options.enabled || self.screencopy.is_none() {
            return;
        }
        self.options_poll_pending = true;
        let timer = Timer::from_duration(POLL);
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            app.options_poll_pending = false;
            app.reeval_options_bar();
            TimeoutAction::Drop
        });
    }
}

/// sRGB (0..1) → linear (0..1), matching the swapchain's sRGB encode.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// 32-bit packed formats we know how to read.
fn supported_format(f: wl_shm::Format) -> bool {
    matches!(
        f,
        wl_shm::Format::Xrgb8888
            | wl_shm::Format::Argb8888
            | wl_shm::Format::Xbgr8888
            | wl_shm::Format::Abgr8888
    )
}

/// Extract (R, G, B) from a 4-byte pixel per its little-endian format.
fn channels(f: wl_shm::Format, px: &[u8]) -> (u8, u8, u8) {
    match f {
        // …bgr8888 stored little-endian ⇒ bytes are [R, G, B, x].
        wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 => (px[0], px[1], px[2]),
        // …rgb8888 stored little-endian ⇒ bytes are [B, G, R, x].
        _ => (px[2], px[1], px[0]),
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZwlrScreencopyManagerV1,
        _: zwlr_screencopy_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // The manager emits no events.
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => app.options_capture_buffer(format, width, height, stride),
            zwlr_screencopy_frame_v1::Event::Flags { flags } => {
                if let (Some(cap), WEnum::Value(f)) = (app.capture.as_mut(), flags) {
                    cap.y_invert = f.bits() & 1 != 0;
                }
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => app.options_capture_ready(),
            zwlr_screencopy_frame_v1::Event::Failed => app.options_capture_failed(),
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, ()> for App {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Release is ignored — screencopy buffers are single-use and we
        // destroy them explicitly after reading.
    }
}

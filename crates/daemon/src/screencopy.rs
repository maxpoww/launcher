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

/// Resample cadence while colour-matched, to track content-colour changes.
const RESAMPLE: Duration = Duration::from_millis(1000);

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
    /// start a capture when a maximized window is flush under the bar, else
    /// fall back to the transparent bar.
    pub(crate) fn reeval_options_bar(&mut self) {
        if self.options_layer.is_none() || self.screencopy.is_none() {
            return;
        }
        match hypr::top_fill(self.config.options.height as f64) {
            Some(tf) => match self.output_by_name(&tf.monitor) {
                Some(output) => {
                    self.options_match = Some(CaptureTarget {
                        output,
                        sample_y: tf.sample_y,
                    });
                    self.start_options_capture();
                    // Keep tracking on-the-fly colour changes while matched.
                    self.schedule_options_resample();
                }
                None => {
                    debug!("options: no wl_output named {}", tf.monitor);
                    self.clear_options_match();
                }
            },
            None => self.clear_options_match(),
        }
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
        // Update the colour if we got one; the resample loop runs on its own
        // timer (see `schedule_options_resample`), so it keeps going regardless.
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

    /// Average the sampled row of the captured frame into an opaque colour.
    fn read_sample(&mut self, cap: &Capture) -> Option<[f32; 4]> {
        if cap.width == 0 || cap.height == 0 {
            return None;
        }
        let pool = self.shm_pool.as_mut()?;
        let map = pool.mmap();
        let bytes: &[u8] = &map[..];
        let row = if cap.y_invert {
            cap.height.saturating_sub(1).saturating_sub(cap.sample_y)
        } else {
            cap.sample_y
        }
        .min(cap.height - 1);
        let start = (row as usize) * (cap.stride as usize);
        let rowbytes = bytes.get(start..start + (cap.width as usize) * 4)?;
        let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
        // Sample up to ~256 pixels across the row — plenty for a flat average.
        let step = ((cap.width as usize) / 256).max(1);
        let mut x = 0;
        while x < cap.width as usize {
            let px = &rowbytes[x * 4..x * 4 + 4];
            let (rr, gg, bb) = channels(cap.format, px);
            r += rr as u64;
            g += gg as u64;
            b += bb as u64;
            n += 1;
            x += step;
        }
        // The captured bytes are sRGB-encoded (display values), but the bar's
        // swapchain is an sRGB surface that re-encodes shader output — so we
        // must hand it the *linear* colour, or it comes out doubly-brightened
        // (a washed, greyish version of the real window colour).
        (n > 0).then_some([
            srgb_to_linear((r / n) as f32 / 255.0),
            srgb_to_linear((g / n) as f32 / 255.0),
            srgb_to_linear((b / n) as f32 / 255.0),
            1.0,
        ])
    }

    /// Keep resampling while colour-matched (catches a window whose top colour
    /// changes on the fly — a new page, a theme switch — with no layout event).
    ///
    /// The loop self-sustains off its own timer, independent of any single
    /// capture's outcome, so a transient failed/empty capture can't silently
    /// stop it; it only ends when the match is dropped.
    fn schedule_options_resample(&mut self) {
        if self.options_poll_pending || self.options_match.is_none() {
            return;
        }
        self.options_poll_pending = true;
        let timer = Timer::from_duration(RESAMPLE);
        let _ = self.loop_handle.insert_source(timer, |_, _, app: &mut App| {
            app.options_poll_pending = false;
            if app.options_match.is_some() {
                app.start_options_capture();
                app.schedule_options_resample();
            }
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

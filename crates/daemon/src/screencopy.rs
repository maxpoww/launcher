//! Colour-match the OPTIONS bar (and the dock) to a maximized window.
//!
//! When Hyprland "smart gaps" leaves a single window filling the screen flush
//! under the bar (see [`crate::hypr::top_fill`]) or flush above the dock (see
//! [`crate::hypr::bottom_fill`]), we sample that window's edge row via the
//! `wlr-screencopy` protocol and paint the surface that flat colour, so
//! window and surface read as one continuous piece. Otherwise each surface
//! falls back to its own frosted backdrop — the blurred wallpaper right
//! behind it.
//!
//! Wayland forbids reading another window's pixels directly, so screencopy is
//! the only way. We capture the whole focused output into an shm buffer, read
//! the physical rows either surface currently wants, and average each. Both
//! surfaces share ONE capture (a [`Slot`] per row wanted).
//!
//! WHEN a capture is taken is the whole cost of this module — a capture the
//! compositor has to redraw the screen for is two orders of magnitude dearer
//! than one that rides a frame it was drawing anyway. See
//! [`App::pump_capture`]: one frame object is always held; it is asked to
//! deliver when something wants a sample, and otherwise sits until the screen
//! changes. At rest nothing is captured at all. Everything degrades
//! gracefully: without the protocol neither surface ever colour-matches.

use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::shm::raw::RawPool;
use tracing::{debug, warn};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_shm;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::{
    self, ZwlrScreencopyManagerV1,
};

use crate::{hypr, App};

/// Poll cadence for re-evaluating both surfaces' colour-match. Runs whenever
/// either could plausibly match — matched or not — so each converges on the
/// current window within one tick even if a compositor event was missed or
/// arrived while the layout was mid-animation. It doubles as the resample
/// loop that tracks a matched window whose content colour changes on the fly.
const POLL: Duration = Duration::from_millis(700);

/// A capture that has neither delivered nor failed after this long is
/// presumed lost (compositor churn — e.g. rapid workspace swipes — can
/// swallow a screencopy's events). Since only ONE capture may be in
/// flight and identical wants never abort it, a lost capture would
/// otherwise block every future sample and freeze the colour for good;
/// [`App::reap_stalled_capture`] clears it.
///
/// A real round-trip is single-digit milliseconds, so this is ~100x
/// headroom — it was 1500ms, which (reaped only on a [`POLL`] tick, so
/// rounded up to the next multiple) could freeze a colour switch for
/// well over two seconds. Reaping now also happens on demand, before
/// each capture, so the worst case is this value plus one round-trip
/// rather than this value plus a poll period.
const CAPTURE_STALL: Duration = Duration::from_millis(600);

/// A frame that is WAITING for the screen to change is supposed to sit for as
/// long as nothing moves; it is only replaced after this long in case its
/// events were lost.
const SENTINEL_STALL: Duration = Duration::from_secs(300);

/// The least time between two samples while nothing asks for one: the pace
/// the bar follows a colour that changes with no layout event (a page under a
/// maximized browser) when the screen keeps changing.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(800);

/// An unasked sample waits for the shell's own drawing to rest this long…
///
/// A capture is not free even when nothing is forced: the compositor redraws
/// what changed since the last one and copies the whole output out — 16 ms
/// of GPU on the Acer, a whole frame's budget. At [`SAMPLE_INTERVAL`] through
/// every animation of ours that was a late frame every 0.8 s, for a sample
/// of a backdrop that our own animation was the only thing changing.
const OWN_REST: Duration = Duration::from_millis(150);

/// …but never longer than this after the last one: a backdrop that changes
/// under a long animation of ours (a video behind an open launcher) is still
/// followed, at this pace instead of [`SAMPLE_INTERVAL`].
const OWN_DEFER_MAX: Duration = Duration::from_millis(2400);

/// The least time between two samples that something DID ask for (an event
/// storm must not become a capture storm).
const DEMAND_FLOOR: Duration = Duration::from_millis(40);

/// How long a sample that MUST come waits for the screen to change by itself
/// before the compositor is made to redraw for it.
const DEMAND_DEADLINE: Duration = Duration::from_millis(80);

/// Quick follow-up re-evaluation after a sample actually changed a
/// colour: the screen was probably still moving when that capture read
/// it (workspace slide, window animation), so look again shortly instead
/// of letting a transitional colour sit until the next [`POLL`] tick.
const SETTLE_BURST: Duration = Duration::from_millis(180);

/// Colour histogram for the dominant-colour (mode) sample: quantised RGB key →
/// (pixel count, r sum, g sum, b sum) so the winning bucket can be averaged.
type ColorHist = std::collections::HashMap<(u8, u8, u8), (u32, u32, u32, u32)>;

/// A bar-side frost sample reads from beside its box to the middle of the
/// screen — ITS OWN HALF, not a fixed-width patch beside the box.
///
/// Both extremes were tried on 2026-09-13. A narrow patch tracks a gradient
/// beautifully and is far too twitchy: it slides whenever a satellite pill
/// appears next to it, and a 90px slide over a wallpaper with any feature in it
/// swung the clipboard's colour by 3x on open. The whole screen, on the other
/// hand, is what the clipboard box used to read and the reason it wore the far
/// side's colour. Half each is the stable middle: it still tells a blue left
/// from an orange right (which is the whole point of two samples), and one pill
/// moving changes a twentieth of it.
const FROST_HALF: f32 = 0.5;

/// The columns a sample may read: starting at `anchor` and stepping outward in
/// direction `dir`, take every second column that isn't ours, until `want` are
/// gathered or the walk leaves `lo..hi`. Runs of our own paint (`exclude`) are
/// jumped over, not merely skipped — the walk comes out the far side and keeps
/// collecting, so a box wider than the sample can no longer starve it.
fn clean_cols(
    anchor: usize,
    dir: isize,
    want: usize,
    lo: usize,
    hi: usize,
    exclude: &[(usize, usize)],
) -> Vec<usize> {
    let mut cols = Vec::with_capacity(want.min(hi.saturating_sub(lo) / 2 + 1));
    let mut x = anchor as isize;
    while cols.len() < want && x >= lo as isize && (x as usize) < hi {
        let xu = x as usize;
        match exclude.iter().find(|&&(ex0, ex1)| xu >= ex0 && xu < ex1) {
            // Land one column clear of the run, in the direction of travel.
            Some(&(ex0, ex1)) => {
                x = if dir > 0 {
                    ex1 as isize
                } else {
                    ex0 as isize - 1
                }
            }
            None => {
                cols.push(xu);
                x += dir * 2;
            }
        }
    }
    cols
}

/// Which surface a sampled row feeds, and which regime it was read under.
/// The bar and the dock each have a "matched a flush window" reading and a
/// "no window, read the frosted backdrop" reading — four combinations total,
/// each landing in its own `App` field (see [`App::options_capture_ready`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Slot {
    /// OPTIONS bar, window-flush match → `options_bar_matched`.
    BarMatch,
    /// OPTIONS bar, no flush window → wallpaper frost over the bar's RIGHT
    /// half, outward from the notification box → `options_pill_color`.
    BarFrost,
    /// Dock, window-flush match → `dock_bar_matched`.
    DockMatch,
    /// Dock, no flush window → wallpaper frost → `dock_pill_color`.
    DockFrost,
    /// No flush window → wallpaper frost over the bar's LEFT half, outward
    /// from the clipboard/settings drawers → `clip_pill_color`. Its own slot,
    /// sampled on its own side: it used to just reuse `options_pill_color`
    /// (read next to notif, the opposite edge of the bar), which is why the
    /// clipboard box's fill/zebra took whatever the wallpaper happens to be on
    /// the far side of the screen instead of what's actually behind it.
    ClipFrost,
}

impl Slot {
    /// Short tag for debug logging.
    fn tag(self) -> &'static str {
        match self {
            Slot::BarMatch | Slot::BarFrost => "options",
            Slot::DockMatch | Slot::DockFrost => "dock",
            Slot::ClipFrost => "clip",
        }
    }
}

/// What a surface currently wants sampled: the output to capture and the
/// physical row to read it from.
#[derive(Clone)]
pub(crate) struct SampleWant {
    output: WlOutput,
    slot: Slot,
    sample_y: u32,
}

/// The merged capture target for the in-flight (or next) screencopy: one
/// output, one or two rows to read from it (bar's want, dock's want, or
/// both — whichever are currently `Some`).
pub(crate) struct CaptureTarget {
    output: WlOutput,
    samples: Vec<(Slot, u32)>,
}

/// How far along a capture is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Stage {
    /// The frame object exists; the compositor has not said what buffer it
    /// wants yet.
    AwaitBuffer,
    /// It has its buffer and waits to be asked to fill it. Costs the
    /// compositor nothing.
    Primed,
    /// Asked to fill it: with the next frame the compositor draws (a
    /// patient capture), or at once (a forced one).
    Shared,
}

/// How much a fresh sample is wanted.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Demand {
    /// Nothing asked: sample when the screen changes, at most every
    /// [`SAMPLE_INTERVAL`].
    Idle,
    /// The compositor reported a change (a window moved, a workspace
    /// switched): sample the frame that shows it. If the screen does not
    /// change after all, the last sample still stands.
    Fresh,
    /// Something of OURS changed what is read (new rows, a box opened over
    /// the sample columns): a sample must come even if the screen is still.
    Must,
}

/// A screencopy of the focused output, from frame object to delivery.
pub(crate) struct Capture {
    frame: ZwlrScreencopyFrameV1,
    buffer: Option<WlBuffer>,
    width: u32,
    height: u32,
    stride: u32,
    format: wl_shm::Format,
    y_invert: bool,
    stage: Stage,
    /// Filled at once (`copy`: the compositor redraws the whole output for
    /// it) instead of with the next frame it draws anyway.
    forced: bool,
    /// When the frame object was made, and when it was asked to deliver.
    started: std::time::Instant,
    shared_at: Option<std::time::Instant>,
}

impl App {
    /// Re-evaluate whether the bar should colour-match, on layout changes:
    /// - fullscreen: match the fullscreen window's top colour, so when the bar
    ///   is revealed (dwell) it's opaque and blends instead of a transparent
    ///   strip over the app;
    /// - smart-gaps: match a maximized window flush under the bar;
    /// - otherwise: the transparent bar.
    pub(crate) fn reeval_options_bar(&mut self) {
        // Paused entirely during fullscreen — no poll, no capture, so the
        // continuous screencopy stops blocking the fullscreen client's
        // direct-scanout. The bar draws only a transparent frame meanwhile.
        // Resumed on fullscreen exit.
        if self.options_paused() {
            self.bar_want = None;
            self.clip_want = None;
            self.rebuild_capture_target();
            return;
        }
        // Keep the safety-net poll alive whenever matching is possible, so a
        // missed event can never leave either surface stuck (blue wallpaper
        // or a stale colour). Idempotent — the pending guard collapses repeats.
        self.schedule_options_poll();
        if self.options_layer.is_none() || self.screencopy.is_none() {
            return;
        }
        // On the stage the match is wrong twice over: the staged window is a
        // card floating in its own inset, no longer flush under the bar, and
        // the backdrop it should blend with is the dimmed wallpaper. So the
        // bar keeps its transparent frost, sampling that backdrop instead
        // (Max, 2026-09-06: "we don't need that on stage mode").
        if self.stage.is_on() {
            self.eval_transparent_bar();
            return;
        }
        match hypr::top_fill(self.options_bar_h() as f64) {
            Some(tf) => match self.output_by_name(&tf.monitor) {
                Some(output) => self.begin_options_match(output, tf.sample_y),
                None => {
                    debug!("options: no wl_output named {}", tf.monitor);
                    self.clear_options_match();
                }
            },
            None => self.eval_transparent_bar(),
        }
    }

    /// The dock's twin of [`Self::reeval_options_bar`] — same triggers, same
    /// fallback shape, [`hypr::bottom_fill`] instead of `top_fill`.
    ///
    /// One extra wrinkle the bar handles differently: the dock's own card
    /// crosses both of its sample rows (the frost row at dock mid-height,
    /// the match row just above the dock band — and the open card covers
    /// far more). Sampling our own columns would read our OWN drawn card
    /// and feed the paint back into itself, so `read_sample` excludes the
    /// card's footprint UNCONDITIONALLY and reads the clean screen beside
    /// it — in EVERY state, so the resting dock and the open box always
    /// compute the same colour from the same source. No colour switch on
    /// open/close (Max, 2026-09-10).
    pub(crate) fn reeval_dock_bar(&mut self) {
        if self.options_paused() {
            self.dock_want = None;
            self.rebuild_capture_target();
            return;
        }
        self.schedule_options_poll();
        if self.screencopy.is_none() {
            return;
        }
        if self.stage.is_on() {
            self.eval_transparent_dock();
            return;
        }
        match hypr::bottom_fill() {
            Some(bf) => match self.output_by_name(&bf.monitor) {
                Some(output) => self.begin_dock_match(output, bf.sample_y),
                None => {
                    debug!("dock: no wl_output named {}", bf.monitor);
                    self.clear_dock_match();
                }
            },
            None => self.eval_transparent_dock(),
        }
    }

    /// No window to match ⇒ the bar stays transparent, floating on the
    /// blurred wallpaper. Sample that backdrop — the bar's *own* frosted
    /// colour — CONTINUOUSLY, not just while a drawer is open: it is what the
    /// pills' text and washes have to contrast against, so without it the bar
    /// paints a static theme ink and goes unreadable over a light wallpaper
    /// (Max, 2026-08-31: "with the bg i set up, the contrast is garbage").
    /// Same 700ms cadence the matched path already pays.
    ///
    /// Requests TWO frost samples off the same row — [`Slot::BarFrost`] for
    /// the right half of the bar, [`Slot::ClipFrost`] for the left — because
    /// the boxes sit at opposite edges and a wallpaper can genuinely differ
    /// between them (see [`Slot::ClipFrost`]'s doc for the bug this fixes: the
    /// clipboard box used to borrow notif's sample). Each reads outward from
    /// its own edge's drawers to the middle of the screen; what makes that
    /// honest is [`read_sample`](App::read_sample) skipping every column we
    /// paint ourselves, pills included.
    fn eval_transparent_bar(&mut self) {
        let had_match = self.options_bar_matched.take().is_some();
        if let Ok(mon) = hypr::focused_monitor() {
            if let Some(output) = self.output_by_name(&mon.name) {
                // A row inside the bar itself (mid-height) — always the
                // blurred wallpaper, since the reserved zone keeps windows
                // out from under the bar.
                let sample_y = ((self.options_bar_h() as f64 * 0.5) * mon.scale.max(0.1))
                    .round()
                    .max(1.0) as u32;
                if had_match {
                    self.draw_options();
                }
                self.bar_want = Some(SampleWant {
                    output: output.clone(),
                    slot: Slot::BarFrost,
                    sample_y,
                });
                self.clip_want = Some(SampleWant {
                    output,
                    slot: Slot::ClipFrost,
                    sample_y,
                });
                self.rebuild_capture_target();
                return;
            }
        }
        self.bar_want = None;
        self.clip_want = None;
        self.rebuild_capture_target();
        if had_match {
            self.draw_options();
        }
    }

    /// No window to match ⇒ the dock stays its frosted self. Sample a row
    /// in the BOTTOM GAP — the `gaps_out` wallpaper strip under the
    /// windows — not at dock mid-height: with tiled windows the mid-height
    /// row crossed their bottom edges and window borders, so the dock (and
    /// the window-border gradient it feeds) tinted itself from window
    /// content — and once the borders went adaptive, from its own paint
    /// (Max, 2026-09-11: "it should still sample the bg at the bottom of
    /// the windows"). The gap row is honest wallpaper in every non-flush
    /// layout; a window sitting flush at the bottom is the DockMatch case,
    /// not this one. Still NOT through our own card: `read_sample`
    /// excludes the card's columns in every state (see
    /// [`Self::reeval_dock_bar`]), so the floating dock never reads its
    /// own paint from the strip beneath it.
    fn eval_transparent_dock(&mut self) {
        /// Logical px above the screen's bottom edge: inside the 10px
        /// `gaps_out` strip, below the ~3px window borders that hug the
        /// window edge at the top of the gap.
        const GAP_ROW_UP: f64 = 3.0;
        let had_match = self.dock_bar_matched.take().is_some();
        if let Ok(mon) = hypr::focused_monitor() {
            if let Some(output) = self.output_by_name(&mon.name) {
                let sample_y = ((mon.h - GAP_ROW_UP) * mon.scale.max(0.1)).round().max(1.0) as u32;
                if had_match {
                    self.draw();
                }
                self.dock_want = Some(SampleWant {
                    output,
                    slot: Slot::DockFrost,
                    sample_y,
                });
                self.rebuild_capture_target();
                return;
            }
        }
        self.dock_want = None;
        self.rebuild_capture_target();
        if had_match {
            self.draw();
        }
    }

    /// Set the bar's colour-match target to `output`/`sample_y` and rebuild
    /// the merged capture. The always-on poll (see
    /// [`Self::schedule_options_poll`]) drives the resample cadence, so this
    /// doesn't schedule one itself.
    fn begin_options_match(&mut self, output: WlOutput, sample_y: u32) {
        self.bar_want = Some(SampleWant {
            output,
            slot: Slot::BarMatch,
            sample_y,
        });
        self.rebuild_capture_target();
    }

    /// The dock's twin of [`Self::begin_options_match`].
    fn begin_dock_match(&mut self, output: WlOutput, sample_y: u32) {
        self.dock_want = Some(SampleWant {
            output,
            slot: Slot::DockMatch,
            sample_y,
        });
        self.rebuild_capture_target();
    }

    /// Drop the bar's match and repaint the transparent bar.
    fn clear_options_match(&mut self) {
        let changed = self.bar_want.take().is_some() | self.options_bar_matched.take().is_some();
        self.clip_want = None;
        self.rebuild_capture_target();
        if changed {
            self.draw_options();
        }
    }

    /// The dock's twin of [`Self::clear_options_match`].
    fn clear_dock_match(&mut self) {
        let changed = self.dock_want.take().is_some() | self.dock_bar_matched.take().is_some();
        self.rebuild_capture_target();
        if changed {
            self.draw();
        }
    }

    /// Merge `bar_want`/`dock_want`/`clip_want` into the one `CaptureTarget`
    /// a capture actually reads, and kick a capture if anything is wanted.
    /// Called at the end of every path above, so the three surfaces' wants
    /// are always folded into a single in-flight (or about-to-start)
    /// screencopy rather than each paying for its own.
    pub(crate) fn rebuild_capture_target(&mut self) {
        // Every path that changes a surface's colour REGIME ends here (see
        // the doc above), so this is where the window borders learn about
        // it — the sample-landed half lives in `options_capture_ready`.
        // Between them they cover every writer of the matched/frost fields,
        // and both are event-driven: the push must never ride the frame
        // loop (see `push_window_border`).
        self.push_window_border();
        let wants = [
            self.bar_want.as_ref(),
            self.dock_want.as_ref(),
            self.clip_want.as_ref(),
        ];
        let samples: Vec<(Slot, u32)> = wants
            .iter()
            .filter_map(|w| w.as_ref().map(|w| (w.slot, w.sample_y)))
            .collect();
        if samples.is_empty() {
            self.abort_capture();
            self.options_match = None;
            self.capture_demand = Demand::Idle;
            self.demand_since = None;
            return;
        }
        // Any want's output — in the ordinary single-monitor-focused
        // workflow they always agree; on the rare tick where they briefly
        // disagree (a monitor change mid-evaluation) the next poll heals it.
        let Some(output) = wants.iter().find_map(|w| w.map(|w| w.output.clone())) else {
            return;
        };
        let changed = self
            .options_match
            .as_ref()
            .is_none_or(|t| t.samples != samples || t.output != output);
        // A frame of another output is no use.
        if self
            .options_match
            .as_ref()
            .is_some_and(|t| t.output != output)
        {
            self.abort_capture();
        }
        self.options_match = Some(CaptureTarget { output, samples });
        // Why this was called decides how much a sample is wanted: a poll
        // tick asks for nothing; a compositor event for the frame that shows
        // the change; anything of ours (new rows, a box over the sample
        // columns, the dock coming to rest) for a sample no matter what.
        let demand = if changed {
            Demand::Must
        } else if self.capture_from_poll {
            Demand::Idle
        } else if self.capture_from_event {
            Demand::Fresh
        } else {
            Demand::Must
        };
        self.demand_capture(demand);
    }

    /// Drop a capture whose events are never coming: it would otherwise hold
    /// the one slot forever.
    pub(crate) fn reap_stalled_capture(&mut self) {
        let stalled = self.capture.as_ref().is_some_and(|c| match c.stage {
            Stage::AwaitBuffer => c.started.elapsed() > CAPTURE_STALL,
            Stage::Primed => false,
            Stage::Shared => c.shared_at.is_some_and(|t| {
                t.elapsed()
                    > if c.forced {
                        CAPTURE_STALL
                    } else {
                        SENTINEL_STALL
                    }
            }),
        });
        if stalled {
            debug!("options: capture stalled; reaping");
            self.abort_capture();
        }
    }

    pub(crate) fn abort_capture(&mut self) {
        if let Some(cap) = self.capture.take() {
            cap.frame.destroy();
            if let Some(buf) = cap.buffer {
                buf.destroy();
            }
            options_engine::end_self_capture();
        }
    }

    pub(crate) fn output_by_name(&self, name: &str) -> Option<WlOutput> {
        self.output_state
            .outputs()
            .find(|o| self.output_state.info(o).and_then(|i| i.name).as_deref() == Some(name))
    }

    /// Ask for a sample (see [`Demand`]) and move the capture along.
    fn demand_capture(&mut self, demand: Demand) {
        if demand > self.capture_demand {
            self.capture_demand = demand;
        }
        if self.capture_demand != Demand::Idle && self.demand_since.is_none() {
            self.demand_since = Some(std::time::Instant::now());
        }
        self.pump_capture();
    }

    /// THE SAMPLER. One frame object is kept at all times, and what the
    /// screen costs us is decided by when it is asked to deliver:
    ///
    /// - A capture taken with `copy` makes Hyprland redraw the WHOLE output
    ///   — 8 ms of its CPU and 18 ms of GPU on the Acer's empty desktop,
    ///   blur and all — and so does the first frame of a capture "session"
    ///   (one ends 500 ms after its last frame). Every sample used to be one
    ///   of those: 2–3 a second through any animation (night audit,
    ///   2026-10-04).
    /// - A frame object made right after a delivery is inside the session,
    ///   and may then WAIT, unasked, at no cost at all ([`Stage::Primed`]).
    ///   Asked with `copy_with_damage` it is filled with the next frame the
    ///   compositor draws — and if the screen changed while it waited,
    ///   Hyprland redraws just what changed and delivers at once.
    ///
    /// So: after each delivery the next frame object is made immediately and
    /// held. It is asked to deliver when a sample is wanted ([`Demand`]) or,
    /// unasked, once [`SAMPLE_INTERVAL`] has passed — after which it sits as
    /// the sentinel until the screen changes. Nothing is ever forced, except
    /// a sample that MUST come while the screen stands still.
    pub(crate) fn pump_capture(&mut self) {
        if self.options_match.is_none() || self.screencopy.is_none() {
            return;
        }
        self.reap_stalled_capture();
        let now = std::time::Instant::now();
        match self.capture.as_ref().map(|c| c.stage) {
            None => self.begin_frame(),
            Some(Stage::AwaitBuffer | Stage::Shared) => {}
            Some(Stage::Primed) => {
                let due = match self.capture_demand {
                    // Unasked: when the interval has passed AND the shell's
                    // own drawing has rested (see [`OWN_REST`]).
                    Demand::Idle => match (self.capture_next_at, crate::renderer::last_present()) {
                        (Some(at), Some(drawn)) => {
                            let rested = drawn + OWN_REST;
                            let latest = self.capture_delivered.map_or(at, |t| t + OWN_DEFER_MAX);
                            Some(at.max(rested.min(latest)))
                        }
                        (at, _) => at,
                    },
                    Demand::Fresh | Demand::Must => {
                        self.capture_delivered.map(|t| t + DEMAND_FLOOR)
                    }
                };
                match due {
                    Some(at) if at > now => self.arm_share_timer(at - now),
                    _ => self.share_frame(),
                }
            }
        }
        if self.capture_demand == Demand::Must {
            self.arm_capture_deadline();
        }
    }

    /// Make the frame object of the next capture.
    fn begin_frame(&mut self) {
        let Some(mgr) = self.screencopy.clone() else {
            return;
        };
        let Some(target) = self.options_match.as_ref() else {
            return;
        };
        let output = target.output.clone();
        // Declare the capture as ours BEFORE asking for it: Hyprland announces
        // every screencopy session on its event socket, and the OPTIONS Mind
        // would otherwise surface "Screen is being shared" for the bar's own
        // colour-match (see `options_engine::begin_self_capture`).
        options_engine::begin_self_capture();
        let frame = mgr.capture_output(0, &output, &self.qh, ());
        self.capture = Some(Capture {
            frame,
            buffer: None,
            width: 0,
            height: 0,
            stride: 0,
            format: wl_shm::Format::Xrgb8888,
            y_invert: false,
            stage: Stage::AwaitBuffer,
            forced: std::mem::take(&mut self.capture_force),
            started: std::time::Instant::now(),
            shared_at: None,
        });
    }

    /// Ask the primed frame to deliver.
    fn share_frame(&mut self) {
        let patient_ok = self.screencopy.as_ref().is_some_and(|m| m.version() >= 2);
        let idle = self.capture_demand == Demand::Idle;
        let Some(cap) = self.capture.as_mut() else {
            return;
        };
        let (Stage::Primed, Some(buffer)) = (cap.stage, cap.buffer.as_ref()) else {
            return;
        };
        if !patient_ok {
            cap.forced = true;
        }
        crate::perf::CAPTURE.hit();
        if cap.forced {
            crate::perf::CAPTURE_FORCED.hit();
            cap.frame.copy(buffer);
        } else {
            if idle {
                crate::perf::CAPTURE_SENTINEL.hit();
            }
            cap.frame.copy_with_damage(buffer);
        }
        cap.stage = Stage::Shared;
        cap.shared_at = Some(std::time::Instant::now());
    }

    /// Look at the capture again in `wait` (the sample interval has passed).
    fn arm_share_timer(&mut self, wait: Duration) {
        if self.capture_share_timer {
            return;
        }
        let armed = self
            .loop_handle
            .insert_source(Timer::from_duration(wait), |_, _, app: &mut App| {
                app.capture_share_timer = false;
                app.pump_capture();
                TimeoutAction::Drop
            })
            .is_ok();
        self.capture_share_timer = armed;
    }

    /// A sample MUST come: if the screen does not change by itself within
    /// [`DEMAND_DEADLINE`] of the asking, make the compositor draw.
    fn arm_capture_deadline(&mut self) {
        if self.capture_deadline_timer {
            return;
        }
        let waited = self.demand_since.map_or(Duration::ZERO, |t| t.elapsed());
        let wait = DEMAND_DEADLINE
            .saturating_sub(waited)
            .max(Duration::from_millis(1));
        let armed = self
            .loop_handle
            .insert_source(Timer::from_duration(wait), |_, _, app: &mut App| {
                app.capture_deadline_timer = false;
                app.capture_deadline();
                TimeoutAction::Drop
            })
            .is_ok();
        self.capture_deadline_timer = armed;
    }

    fn capture_deadline(&mut self) {
        if self.capture_demand != Demand::Must || self.options_match.is_none() {
            return;
        }
        if self
            .demand_since
            .is_some_and(|t| t.elapsed() < DEMAND_DEADLINE)
        {
            self.arm_capture_deadline();
            return;
        }
        match self.capture.as_ref().map(|c| (c.stage, c.forced)) {
            // Already being forced: the reaper minds it.
            Some((Stage::Shared, true)) => {}
            // Waiting on a screen that is not changing: replace it.
            Some((Stage::Shared, false)) => {
                self.abort_capture();
                self.capture_force = true;
                self.begin_frame();
            }
            Some((Stage::Primed, _)) => {
                if let Some(cap) = self.capture.as_mut() {
                    cap.forced = true;
                }
                self.share_frame();
            }
            Some((Stage::AwaitBuffer, _)) => {
                if let Some(cap) = self.capture.as_mut() {
                    cap.forced = true;
                }
            }
            None => {
                self.capture_force = true;
                self.begin_frame();
            }
        }
    }

    /// `buffer` event: allocate the shm buffer; the frame is then primed (see
    /// [`Self::pump_capture`]). Extra buffer offers (e.g. dmabuf) after we've
    /// picked an shm one are ignored.
    fn options_capture_buffer(
        &mut self,
        format: WEnum<wl_shm::Format>,
        width: u32,
        height: u32,
        stride: u32,
    ) {
        if self
            .capture
            .as_ref()
            .is_none_or(|c| c.stage != Stage::AwaitBuffer)
        {
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
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            fmt,
            (),
            &self.qh,
        );
        if let Some(cap) = self.capture.as_mut() {
            cap.buffer = Some(buffer);
            cap.width = width;
            cap.height = height;
            cap.stride = stride;
            cap.format = fmt;
            cap.stage = Stage::Primed;
        }
        self.pump_capture();
    }

    /// `ready` event: the buffer holds the frame — sample every row that is
    /// wanted NOW (the frame has them all), paint whichever changed, and make
    /// the next frame object while the capture session is still open.
    fn options_capture_ready(&mut self) {
        let Some(cap) = self.capture.take() else {
            return;
        };
        self.options_capture_failing = false;
        let wants: Vec<(Slot, u32)> = [&self.bar_want, &self.dock_want, &self.clip_want]
            .into_iter()
            .filter_map(|w| w.as_ref().map(|w| (w.slot, w.sample_y)))
            .collect();
        let mut bar_changed = false;
        let mut dock_changed = false;
        for (slot, sample_y) in wants {
            let Some(color) = self.read_sample(&cap, slot, sample_y) else {
                continue;
            };
            let target = match slot {
                Slot::BarMatch => &mut self.options_bar_matched,
                Slot::BarFrost => &mut self.options_pill_color,
                Slot::DockMatch => &mut self.dock_bar_matched,
                Slot::DockFrost => &mut self.dock_pill_color,
                Slot::ClipFrost => &mut self.clip_pill_color,
            };
            if *target != Some(color) {
                *target = Some(color);
                match slot {
                    Slot::BarMatch | Slot::BarFrost | Slot::ClipFrost => bar_changed = true,
                    Slot::DockMatch | Slot::DockFrost => dock_changed = true,
                }
            }
        }
        cap.frame.destroy();
        if let Some(buf) = cap.buffer {
            buf.destroy();
        }
        options_engine::end_self_capture();
        let changed = bar_changed || dock_changed;
        let now = std::time::Instant::now();
        self.capture_delivered = Some(now);
        self.capture_demand = Demand::Idle;
        self.demand_since = None;
        // A colour just moved — the screen was likely still animating when
        // this capture read it (workspace slide, window settling): the next
        // look may come soon, so the colour converges on the settled screen.
        // Otherwise the usual interval. Either way the next frame WAITS for
        // the screen to change: a settled screen is not captured again.
        self.capture_next_at = Some(
            now + if changed {
                SETTLE_BURST
            } else {
                SAMPLE_INTERVAL
            },
        );
        if changed {
            crate::perf::COLOR_CHANGED.hit();
            // Fresh colours: hand the window borders their new gradient
            // (the regime half of this lives in `rebuild_capture_target`).
            self.push_window_border();
        }
        if bar_changed {
            self.draw_options();
        }
        if dock_changed {
            self.draw();
        }
        // The next frame object, made inside the capture session (see
        // `pump_capture`).
        self.pump_capture();
    }

    /// Re-evaluate both surfaces after [`SETTLE_BURST`] — the quick retry
    /// after a capture failed outright. Idempotent: a burst already armed is
    /// left alone.
    fn arm_settle_burst(&mut self) {
        if self.options_burst_pending {
            return;
        }
        let timer = Timer::from_duration(SETTLE_BURST);
        let armed = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.options_burst_pending = false;
                app.reeval_options_bar();
                app.reeval_dock_bar();
                TimeoutAction::Drop
            })
            .is_ok();
        self.options_burst_pending = armed;
    }

    fn options_capture_failed(&mut self) {
        self.abort_capture();
        debug!("options: screencopy failed");
        // Retry on the burst rather than waiting out a whole POLL: a
        // failure is most likely mid-transition (the compositor was busy),
        // which is exactly when the colour must not sit still. Only the
        // FIRST of a run, though — if failures are persistent, retrying
        // several times a second buys nothing and the slow poll is the
        // right cadence to keep knocking at.
        if !self.options_capture_failing {
            self.arm_settle_burst();
        }
        self.options_capture_failing = true;
    }

    /// Read one opaque colour from the captured frame for one wanted row.
    /// Two regimes:
    /// - **match** (`BarMatch`/`DockMatch`): the *dominant* colour of the
    ///   window's edge strip (mode over the sides, skipping edges + the
    ///   centre third) — the real header/footer background.
    /// - **frost** (`BarFrost`/`DockFrost`): the *mean* of that surface's own
    ///   backdrop — the bar reads a band beside the open notif box, the dock
    ///   reads broadly across its own width.
    fn read_sample(&mut self, cap: &Capture, slot: Slot, sample_y: u32) -> Option<[f32; 4]> {
        if cap.width == 0 || cap.height == 0 {
            return None;
        }
        let width = cap.width as usize;
        // Horizontal sampling deliberately skips two zones: the outer ~3% (CSD
        // rounding, the window border, a right-edge scrollbar) AND the central
        // third — where a browser's URL/search field or an app's centred title
        // sits. That centre block is a big patch of a *different* colour than
        // the surrounding chrome (Chrome: grey omnibox on **black**; Firefox:
        // grey URL field on **white**), and reading through it is exactly what
        // made a surface match the field instead of the toolbar. Sampling only
        // the sides reads the toolbar/footer *background* — the colour the eye
        // takes as "the window's edge" — on any app.
        let outer = (width / 33).clamp(4, 60);
        let cl = width * 34 / 100;
        let cr = width * 66 / 100;
        if width <= outer * 2 + 4 || cl <= outer || cr >= width - outer {
            return None;
        }
        let step = (width / 400).max(1);

        // Columns a sample must skip because something of OURS is painted
        // there: the notif/clip drawer panels over the BAR's rows, and the
        // open launcher card over the DOCK's rows (added below). Rather
        // than freeze while occluded, exclude just those columns so the
        // surface keeps reading the live screen to either side. A small
        // Vec, not one range — the two drawers can be open at once.
        let mut exclude: Vec<(usize, usize)> = Vec::new();
        if matches!(slot, Slot::BarMatch | Slot::BarFrost | Slot::ClipFrost) {
            let sw = self.options_size.0 as f32;
            if sw > 0.0 {
                let px = width as f32 / sw;
                let mut push_exclusion = |r: crate::content::Rect| {
                    if r.w <= 0.0 {
                        return;
                    }
                    let ex0 = (r.x * px).floor().max(0.0) as usize;
                    let ex1 = (((r.x + r.w) * px).ceil() as usize + 1).min(width);
                    if ex1 > ex0 {
                        exclude.push((ex0.saturating_sub(1), ex1));
                    }
                };
                if slot == Slot::BarMatch {
                    // Below the bar, only what is actually painted there.
                    if self.notif.occludes_below_bar() {
                        push_exclusion(self.notif_rect());
                    }
                    if self.clip_occludes_below_bar() {
                        push_exclusion(self.clip_rect());
                    }
                    if self.stats_occludes_below_bar() {
                        push_exclusion(self.stats_geom());
                    }
                } else {
                    // The FROST row runs through the middle of the bar — the
                    // one row no window can reach (the reserved zone), which is
                    // exactly why it is sampled there. But it is also the row
                    // every PILL sits on, and a pill is our own paint: the band
                    // beside the clipboard landed squarely on the clipboard and
                    // cava pills and averaged OUR grey (#282a2c) while the band
                    // beside notif read the true wallpaper (#01060a) — so one
                    // box came out twice as light as the other with nothing on
                    // screen to explain it (Max, 2026-09-13: "why is the
                    // clipboard bluer than the notis?").
                    for r in self.options_pill_columns() {
                        push_exclusion(r);
                    }
                    // And each edge's drawers are excluded at their FULL width
                    // whether they are open or not, so opening a box cannot
                    // move the sample — otherwise the colour would switch on
                    // open, the one thing Max already ruled out for the dock
                    // ("i dont want a colors switch between the dock and the
                    // open boxmenu", 2026-09-10).
                    let span = |l: f32, r: f32| crate::content::Rect::new(l, 0.0, r - l, 1.0);
                    push_exclusion(span(0.0, self.options_left_drawer_right()));
                    push_exclusion(span(self.options_right_drawer_left(), sw));
                }
            }
        }
        // The dock NEVER reads its own columns — docked, opening, or open.
        // History, in order: reading straight through fed our own paint
        // back into itself (the stepped colour crawl); freezing while open
        // left the box colour-stale during workspace swipes; and excluding
        // the card only WHILE OPEN meant the resting dock (sampling through
        // its own translucent card) and the open box (sampling the clean
        // screen beside it) wore two DIFFERENT colours, switching on every
        // open/close — Max, 2026-09-10: "i want all to look all time as the
        // OPEN BOX! i dont want a colors switch between the dock and the
        // open boxmenu." So the exclusion is unconditional: both states
        // read the same clean columns beside the card's footprint — one
        // colour source, no switch. Same trick BarMatch uses for the
        // notif/clip drawers. The bar surface spans the full monitor width,
        // so `options_size.0` doubles as the monitor's logical width for
        // the logical→physical mapping; the card is centered on it.
        if matches!(slot, Slot::DockMatch | Slot::DockFrost) {
            let sw = self.options_size.0 as f32;
            if sw <= 0.0 {
                // Can't place the card's columns — no sample beats reading
                // our own card and re-entering the feedback loop.
                return None;
            }
            let px = width as f32 / sw;
            // Breath/jelly can push the card a few px past its rest rect.
            let pad = 12.0;
            let card_w = self.config.window.width as f32;
            let l = ((sw - card_w) / 2.0 - pad).max(0.0);
            let r = ((sw + card_w) / 2.0 + pad).min(sw);
            let ex0 = (l * px).floor().max(0.0) as usize;
            let ex1 = (((r * px).ceil() as usize) + 1).min(width);
            if ex1 > ex0 {
                exclude.push((ex0.saturating_sub(1), ex1));
            }
        }

        // Frost sample: the bar reads *immediately beside its own box* (so the
        // box takes the wallpaper colour right next to the pill — sampling the
        // far side instead mis-matches wallpapers that vary left-to-right) —
        // notif walks leftward from its right-edge pill, the clipboard walks
        // rightward from its left-edge pill, its own dedicated sample so it
        // stops borrowing notif's (see [`Slot::ClipFrost`]).
        //
        // It WALKS outward instead of reading a fixed band because everything
        // of ours on that row is excluded (every pill, and an open box's top
        // strip). A fixed band can be covered entirely — the settings
        // readout's panel is wider than the clipboard's, so with it open the
        // whole band was our own paint and the left frost went stale — while a
        // walk steps over what is ours and keeps going until it has
        // [`FROST_COLS`] clean columns of real screen. Nearest clean pixels
        // win, which is the rule the frost always meant.
        //
        // The dock has no adjacent box to dodge, so it reads broadly across
        // its own width instead.
        let frost_walk: Option<(usize, isize)> = match slot {
            Slot::BarFrost => {
                let sw = self.options_size.0 as f32;
                (sw > 0.0).then(|| {
                    let px = width as f32 / sw;
                    let edge = (self.options_right_drawer_left() * px).max(0.0) as usize;
                    let gap = (width / 100).max(2);
                    let anchor = edge.saturating_sub(gap).min(width.saturating_sub(1));
                    (anchor, -1)
                })
            }
            Slot::ClipFrost => {
                let sw = self.options_size.0 as f32;
                (sw > 0.0).then(|| {
                    let px = width as f32 / sw;
                    let edge = (self.options_left_drawer_right() * px)
                        .min(width as f32)
                        .max(0.0) as usize;
                    let gap = (width / 100).max(2);
                    ((edge + gap).min(width.saturating_sub(1)), 1)
                })
            }
            _ => None,
        };
        let frost_band: Option<(usize, usize)> = match slot {
            Slot::DockFrost => (width > outer * 2).then_some((outer, width - outer)),
            _ => None,
        };

        // `sample_y` is physical-from-top; in a y-inverted buffer that maps to a
        // row counted from the bottom. "Deeper into the window" is +dy from the
        // top, i.e. a smaller row index when inverted.
        let base = if cap.y_invert {
            cap.height.saturating_sub(1).saturating_sub(sample_y)
        } else {
            sample_y
        };

        let pool = self.shm_pool.as_mut()?;
        let map = pool.mmap();
        let bytes: &[u8] = &map[..];

        // Frost path: average the clean columns this slot asked for (see above)
        // — the nearest ones outward from a bar box, or the dock's own width.
        if matches!(slot, Slot::BarFrost | Slot::DockFrost | Slot::ClipFrost) {
            let cols = match (frost_walk, frost_band) {
                (Some((anchor, dir)), _) => {
                    // Bounded by the screen's middle (see [`FROST_HALF`]) —
                    // unless the drawers already reach past it, in which case
                    // this side takes what is left rather than nothing.
                    let mid = (width as f32 * FROST_HALF) as usize;
                    let (lo, hi) = match dir {
                        d if d > 0 => (outer, mid.max(anchor + 2).min(width - outer)),
                        _ => (mid.min(anchor.saturating_sub(2)).max(outer), width - outer),
                    };
                    clean_cols(anchor, dir, usize::MAX, lo, hi, &exclude)
                }
                (None, Some((bl, br))) => clean_cols(bl, 1, usize::MAX, bl, br, &exclude),
                (None, None) => return None,
            };
            if cols.is_empty() {
                return None;
            }
            let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
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
                for &x in &cols {
                    let (rr, gg, bb) = channels(cap.format, &rowbytes[x * 4..x * 4 + 4]);
                    r += rr as u64;
                    g += gg as u64;
                    b += bb as u64;
                    n += 1;
                }
            }
            if n == 0 {
                return None;
            }
            let (mr, mg, mb) = ((r / n) as u8, (g / n) as u8, (b / n) as u8);
            let (first, last) = (cols[0], cols[cols.len() - 1]);
            tracing::trace!("{}: frost skipped {exclude:?}", slot.tag());
            debug!(
                "{}: frost colour = #{mr:02x}{mg:02x}{mb:02x} ({} clean cols {first}..{last} @ row {base})",
                slot.tag(),
                cols.len()
            );
            return Some([
                srgb_to_linear(mr as f32 / 255.0),
                srgb_to_linear(mg as f32 / 255.0),
                srgb_to_linear(mb as f32 / 255.0),
                1.0,
            ]);
        }

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
                // Jump over the notif/clipboard box's columns when open.
                if let Some(&(_, ex1)) = exclude.iter().find(|&&(ex0, ex1)| x >= ex0 && x < ex1) {
                    x = ex1.max(x + step);
                    continue;
                }
                let (rr, gg, bb) = channels(cap.format, &rowbytes[x * 4..x * 4 + 4]);
                let e = buckets
                    .entry((rr & 0xF0, gg & 0xF0, bb & 0xF0))
                    .or_insert((0, 0, 0, 0));
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
        debug!(
            "{}: colour-match edge = #{mr:02x}{mg:02x}{mb:02x}",
            slot.tag()
        );
        // The captured bytes are sRGB-encoded (display values), but the
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

    /// Run the colour-match re-evaluation on a steady timer for as long as
    /// either surface could plausibly match — whether or not either
    /// currently is.
    ///
    /// This is the system's self-healing spine. Matching is otherwise driven
    /// by Hyprland layout events, but an event can be missed, or fire while a
    /// workspace-switch animation is still mid-flight (so the window isn't yet
    /// where IPC will report it a beat later). Either way a surface could
    /// stick — showing blurred wallpaper, or a stale colour from another
    /// window. The poll guarantees both reconverge on the true current window
    /// within one tick regardless. While matched it also serves as the
    /// resample loop, tracking a window whose content colour changes with no
    /// layout event.
    ///
    /// Self-sustaining: each tick reschedules the next, so a transient failed
    /// capture or empty read can't stop it. The pending guard keeps the
    /// event-driven and timer-driven callers from stacking duplicate timers.
    pub(crate) fn schedule_options_poll(&mut self) {
        // Paused during fullscreen: no colour-match captures at all (the
        // continuous screencopy is what blocks the fullscreen client's
        // direct-scanout). The fullscreen-exit path reschedules the poll.
        if self.options_paused() {
            return;
        }
        if self.options_poll_pending || !self.config.options.enabled || self.screencopy.is_none() {
            return;
        }
        // The zone poll (800 ms, whenever intellihide is on — the default)
        // already re-evaluates both surfaces on every tick; a second timer
        // doing the same 100 ms faster only doubled the idle work.
        if self.zone_poll_running {
            return;
        }
        self.options_poll_pending = true;
        let timer = Timer::from_duration(POLL);
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.options_poll_pending = false;
                // Reap a capture whose events never came (see
                // [`CAPTURE_STALL`]) so the reevals below can start a
                // fresh one instead of queuing behind a zombie forever.
                // `start_options_capture` reaps too — this is the net for
                // the case where no want survives to get that far.
                app.reap_stalled_capture();
                // Captures started from here may be patient ones (see
                // `start_options_capture`).
                app.capture_from_poll = true;
                app.reeval_options_bar();
                app.reeval_dock_bar();
                app.capture_from_poll = false;
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
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Only the frame in flight: one that was replaced has nothing to say.
        if app.capture.as_ref().is_none_or(|c| &c.frame != frame) {
            return;
        }
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
            // What the screen changed in, to wake this capture (with
            // `RUST_LOG=info,waverunner::screencopy=debug`): the way to find
            // out WHO keeps a resting screen drawing.
            zwlr_screencopy_frame_v1::Event::Damage {
                x,
                y,
                width,
                height,
            } => tracing::debug!("capture woken by damage {width}x{height} at {x},{y}"),
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

#[cfg(test)]
mod tests {
    use super::clean_cols;

    /// With nothing of ours in the way the walk is just "every second column,
    /// outward from the anchor" — in both directions.
    #[test]
    fn walks_outward_from_the_anchor() {
        assert_eq!(clean_cols(10, 1, 3, 0, 100, &[]), vec![10, 12, 14]);
        assert_eq!(clean_cols(10, -1, 3, 0, 100, &[]), vec![10, 8, 6]);
    }

    /// A run of our own paint is JUMPED, not abandoned: the walk comes out the
    /// far side and keeps collecting. This is what stops a box wider than the
    /// sample from starving it (the settings readout over the clip band).
    #[test]
    fn jumps_over_our_own_paint() {
        let ours = [(12, 40)];
        assert_eq!(clean_cols(10, 1, 3, 0, 100, &ours), vec![10, 40, 42]);
        // …and leftward, landing one column clear of the run's near edge.
        assert_eq!(clean_cols(45, -1, 3, 0, 100, &ours), vec![45, 43, 41]);
        assert_eq!(clean_cols(41, -1, 2, 0, 100, &ours), vec![41, 11]);
    }

    /// Bounds end the walk; an anchor with no room at all yields nothing
    /// (the caller keeps the previous colour rather than inventing one).
    #[test]
    fn stops_at_the_bounds() {
        assert_eq!(clean_cols(6, -1, 9, 4, 100, &[]), vec![6, 4]);
        assert!(clean_cols(50, 1, 9, 0, 50, &[]).is_empty());
        assert!(clean_cols(10, 1, 9, 0, 100, &[(0, 100)]).is_empty());
    }
}

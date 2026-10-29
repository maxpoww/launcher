//! Counters for the shell's hot paths — always on, almost free (two relaxed
//! atomic adds and, where timed, two clock reads).
//!
//! "Is it faster?" was answered from outside the process for the first two
//! unify rounds (CPU time, wakeups, GPU memory). That cannot say how many
//! frames a surface drew for one animation, or how long the event loop spent
//! inside a draw — the numbers that decide whether an optimisation worked.
//! `waverunner-ctl debug-perf` logs every counter since the last call and
//! resets them, so a scripted workload reads as one line before and one after.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

pub(crate) struct Counter {
    calls: AtomicU64,
    nanos: AtomicU64,
    /// The longest single timing since the last report: the frame that
    /// was felt, which an average hides.
    max_nanos: AtomicU64,
}

impl Counter {
    const fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
            max_nanos: AtomicU64::new(0),
        }
    }

    /// Record one timing of `nanos` (for work timed by the caller).
    pub(crate) fn record_nanos(&self, nanos: u64) {
        self.calls.fetch_add(1, Relaxed);
        self.nanos.fetch_add(nanos, Relaxed);
        self.max_nanos.fetch_max(nanos, Relaxed);
    }

    /// Count one occurrence.
    pub(crate) fn hit(&self) {
        self.calls.fetch_add(1, Relaxed);
    }

    /// Add `n` to the count.
    pub(crate) fn add(&self, n: u64) {
        self.calls.fetch_add(n, Relaxed);
    }

    /// Count one occurrence and the time until the guard drops.
    pub(crate) fn time(&self) -> Timed<'_> {
        Timed {
            counter: self,
            start: Instant::now(),
        }
    }

    fn take(&self) -> (u64, f64) {
        (
            self.calls.swap(0, Relaxed),
            self.nanos.swap(0, Relaxed) as f64 / 1e6,
        )
    }

    fn take_max_ms(&self) -> f64 {
        self.max_nanos.swap(0, Relaxed) as f64 / 1e6
    }
}

pub(crate) struct Timed<'a> {
    counter: &'a Counter,
    start: Instant,
}

impl Drop for Timed<'_> {
    fn drop(&mut self) {
        self.counter.record_nanos(self.start.elapsed().as_nanos() as u64);
    }
}

/// The event loop's own lateness: a 25 ms timer records how late each tick
/// fires. Late = the loop was busy (one draw, one blocking call, one file
/// write) — the time the shell could not answer anything. `calls` are the
/// ticks that were late by more than 30 ms; the timing is the lateness.
pub(crate) static LOOP_LATE: Counter = Counter::new();

thread_local! {
    /// What the loop's thread did since the lateness tick last fired, for a
    /// stall's line (`mark`).
    static MARKS: std::cell::RefCell<Vec<(&'static str, std::time::Instant)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Note a step on the loop's thread: a stall's line (`loop: stalled …`)
/// then says what the loop did, and when, since the tick before it — the
/// steps that may take their time at a start are marked (the renderers,
/// the measures, the desktop's reload, the apps' landing). Two words and a
/// clock read.
pub(crate) fn mark(what: &'static str) {
    MARKS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() < 64 {
            m.push((what, std::time::Instant::now()));
        }
    });
}

/// The marks since the last take, each with its ms from `since`.
pub(crate) fn take_marks(since: std::time::Instant) -> String {
    MARKS.with(|m| {
        let marks = std::mem::take(&mut *m.borrow_mut());
        marks
            .iter()
            .map(|(what, at)| format!("{what} +{}", at.saturating_duration_since(since).as_millis()))
            .collect::<Vec<_>>()
            .join(", ")
    })
}
/// The ticks themselves (how long the report covers, at 40 a second).
pub(crate) static LOOP_TICKS: Counter = Counter::new();
/// A whole dock frame: animation step, scene build, render.
pub(crate) static DOCK_DRAW: Counter = Counter::new();
/// The desktop's folder read into icons (`reload_desktop`), on the loop.
pub(crate) static DESKTOP_RELOAD: Counter = Counter::new();
/// A desktop frame (`draw_desktop`), the ink map started (`desktop_ink_start`),
/// an icon or a thumbnail landing (`on_desktop_icon` / `desktop_on_thumb`).
pub(crate) static DESKTOP_DRAW: Counter = Counter::new();
pub(crate) static DESKTOP_INK: Counter = Counter::new();
pub(crate) static DESKTOP_ICON: Counter = Counter::new();
/// The file index taking the watched folders' tops (`merge_tops`), on the loop.
pub(crate) static FILES_MERGE: Counter = Counter::new();
/// A `files …` command (open a folder, close, …), whole.
pub(crate) static FILES_CMD: Counter = Counter::new();
/// The app index landing on the loop (`on_apps_loaded`), whole, and every
/// icon chain written into a GPU array (`write_icon_chain`).
pub(crate) static APPS_LOADED: Counter = Counter::new();
/// Work handed back from off the loop (`desktop_off_loop`'s `then`), the
/// desktop folder's and the watched folders' inotify callbacks, the desktop
/// top layer's draw request.
pub(crate) static OFF_LOOP_THEN: Counter = Counter::new();
pub(crate) static INOTIFY_DESKTOP: Counter = Counter::new();
pub(crate) static INOTIFY_FILES: Counter = Counter::new();
pub(crate) static DESKTOP_TOP: Counter = Counter::new();
/// One thumbnail taken into its layers (`on_thumb_pixels`).
pub(crate) static THUMB_LANDED: Counter = Counter::new();
/// The notification rows measured (`measure_notif`), whole.
pub(crate) static NOTIF_MEASURE: Counter = Counter::new();
pub(crate) static ICON_UPLOAD: Counter = Counter::new();
/// A whole OPTIONS-bar frame.
pub(crate) static OPTIONS_DRAW: Counter = Counter::new();
/// A whole deck frame.
pub(crate) static DECK_DRAW: Counter = Counter::new();
/// `Renderer::render` on any surface (inside the three above).
pub(crate) static RENDER: Counter = Counter::new();
/// …of which: waiting for a swapchain image, the submit, the present.
pub(crate) static RENDER_ACQUIRE: Counter = Counter::new();
pub(crate) static RENDER_SUBMIT: Counter = Counter::new();
pub(crate) static RENDER_PRESENT: Counter = Counter::new();
/// `Renderer::measure_text`.
pub(crate) static MEASURE: Counter = Counter::new();
/// A label shaped (a cache miss, or an uncached label).
pub(crate) static SHAPE: Counter = Counter::new();
/// A request that went to Hyprland's socket.
pub(crate) static HYPR_REQUEST: Counter = Counter::new();
/// A read answered from the turn's snapshot.
pub(crate) static HYPR_CACHED: Counter = Counter::new();
/// A screen capture started (colour match / frost sample).
pub(crate) static CAPTURE: Counter = Counter::new();

/// …of which: unasked ones (the sentinel: delivered when the screen changes),
/// and FORCED ones (the compositor redraws the whole output for each). The
/// rest were asked for and rode a frame the compositor drew anyway.
pub(crate) static CAPTURE_SENTINEL: Counter = Counter::new();
pub(crate) static CAPTURE_FORCED: Counter = Counter::new();
/// A capture whose sample moved a colour.
pub(crate) static COLOR_CHANGED: Counter = Counter::new();

/// Frames whose damage was worked out (see `crate::damage`), the pixels they
/// damaged, the pixels of their surfaces, how many showed exactly what the
/// frame before did (those were not drawn), and the rectangles presented.
pub(crate) static DAMAGE_FRAMES: Counter = Counter::new();
pub(crate) static DAMAGE_PX: Counter = Counter::new();
pub(crate) static SURFACE_PX: Counter = Counter::new();
pub(crate) static DAMAGE_NONE: Counter = Counter::new();
pub(crate) static DAMAGE_RECTS: Counter = Counter::new();
/// `WAVERUNNER_DAMAGE_CHECK=1`: frames read back and compared, those with
/// pixels that changed OUTSIDE their damage, and how many such pixels.
pub(crate) static DAMAGE_CHECKED: Counter = Counter::new();
pub(crate) static DAMAGE_MISSED: Counter = Counter::new();
pub(crate) static DAMAGE_MISSED_PX: Counter = Counter::new();
/// `WAVERUNNER_DAMAGE_CHECK=paths`: frames drawn both ways (straight into
/// the target, and offscreen then copied), and those that came out different.
pub(crate) static PATHS_CHECKED: Counter = Counter::new();
pub(crate) static PATHS_DIFFER: Counter = Counter::new();
/// Visible regions sent to the compositor (see `crate::visible`).
pub(crate) static VISIBLE_SET: Counter = Counter::new();
/// The damage check: frames with drawn pixels OUTSIDE the visible region
/// the compositor was given (see `crate::visible`) — it would not show them.
pub(crate) static VISIBLE_MISSED: Counter = Counter::new();

/// Every counter since the last report, as one line; resets them.
pub(crate) fn report() -> String {
    let timed = |name: &str, c: &Counter| {
        let (n, ms) = c.take();
        let max = c.take_max_ms();
        format!("{name} {n} in {ms:.1} ms (max {max:.1})")
    };
    let count = |name: &str, c: &Counter| format!("{name} {}", c.take().0);
    let (stalls, late_ms) = LOOP_LATE.take();
    let worst = LOOP_LATE.take_max_ms();
    let ticks = LOOP_TICKS.take().0;
    [
        format!("loop {ticks} ticks, {stalls} stalls >30 ms ({late_ms:.0} ms late, worst {worst:.1})"),
        timed("dock", &DOCK_DRAW),
        timed("desktop-reload", &DESKTOP_RELOAD),
        timed("desktop-draw", &DESKTOP_DRAW),
        timed("desktop-ink", &DESKTOP_INK),
        timed("desktop-icon", &DESKTOP_ICON),
        timed("files-merge", &FILES_MERGE),
        timed("files-cmd", &FILES_CMD),
        timed("apps-loaded", &APPS_LOADED),
        timed("off-loop-then", &OFF_LOOP_THEN),
        timed("inotify-desktop", &INOTIFY_DESKTOP),
        timed("inotify-files", &INOTIFY_FILES),
        timed("desktop-top", &DESKTOP_TOP),
        timed("thumb-landed", &THUMB_LANDED),
        timed("notif-measure", &NOTIF_MEASURE),
        timed("icon-upload", &ICON_UPLOAD),
        timed("options", &OPTIONS_DRAW),
        timed("deck", &DECK_DRAW),
        timed("render", &RENDER),
        timed("acquire", &RENDER_ACQUIRE),
        timed("submit", &RENDER_SUBMIT),
        timed("present", &RENDER_PRESENT),
        timed("measure_text", &MEASURE),
        timed("shapes", &SHAPE),
        timed("hypr", &HYPR_REQUEST),
        count("hypr-cached", &HYPR_CACHED),
        count("captures", &CAPTURE),
        count("sentinel", &CAPTURE_SENTINEL),
        count("forced", &CAPTURE_FORCED),
        count("colour-changes", &COLOR_CHANGED),
        damage(),
    ]
    .join(" | ")
}

fn damage() -> String {
    let frames = DAMAGE_FRAMES.take().0;
    let (px, of) = (DAMAGE_PX.take().0, SURFACE_PX.take().0);
    let none = DAMAGE_NONE.take().0;
    let pct = if of > 0 {
        px as f64 * 100.0 / of as f64
    } else {
        0.0
    };
    let rects = DAMAGE_RECTS.take().0 as f64 / frames.max(1) as f64;
    let mut line = format!(
        "damage {pct:.1}% over {frames} frames ({none} skipped, {rects:.1} rects, {} regions)",
        VISIBLE_SET.take().0
    );
    let checked = DAMAGE_CHECKED.take().0;
    let (missed, missed_px) = (DAMAGE_MISSED.take().0, DAMAGE_MISSED_PX.take().0);
    if checked > 0 {
        line += &format!(" | damage-check {checked} frames, {missed} wrong ({missed_px} px)");
    }
    let clipped = VISIBLE_MISSED.take().0;
    if checked > 0 {
        line += &format!(", {clipped} outside the visible region");
    }
    let (paths, differ) = (PATHS_CHECKED.take().0, PATHS_DIFFER.take().0);
    if paths > 0 {
        line += &format!(" | path-check {paths} frames, {differ} differ");
    }
    line
}

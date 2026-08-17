//! The clipboard OPTION's data plane.
//!
//! A background worker watches the Wayland clipboard (via `wl-paste --watch`,
//! the canonical wlr-data-control client — focus-independent, unlike our own
//! `wl_data_device` which only sees the selection while the surface holds
//! keyboard focus) and reports each change as a classified [`ClipEntry`]: plain
//! **text**, a **files** copy (`text/uri-list`), or an **image**. The UI thread
//! folds those into a browsable, persisted history; picking an entry sends a
//! [`ClipCommand::Copy`] back so the worker restores it with `wl-copy`.
//!
//! Same discipline as the notification worker and the app indexer: it lives on
//! its own thread and talks to the calloop loop **only** over channels (one
//! event loop; shared state stays on it). It respawns the watcher with backoff
//! and never brings the loop down.
//!
//! The engine's `selection` collector still senses the *latest* clipboard for
//! the mind's ambient awareness; this OPTION owns the rich history and the
//! copy-back, exactly the way notifications are split between the engine's
//! summary collector and the notification OPTION's live list.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use calloop::channel::Sender;
use calloop::timer::{TimeoutAction, Timer};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::animation::{ease_toward, lerp};
use crate::content::{Label, Rect, RectInst, Scene};
use crate::options::{
    hover_grow, push_neumorph, wash, PillId, BOND_GAP, FONT_PX, GLYPH_CLIPBOARD, GLYPH_CLOSE,
    GLYPH_COPY, GLYPH_CUT, GLYPH_SELECT_ALL, LINE_PX, NERD, PILL_MARGIN_Y, PILL_PAD_X, RED_GLYPH,
};
use crate::App;

/// Target width of the extended preview pill (mirrors the notification OPTION's
/// preview so the two elements read as siblings). The open history box keeps it.
const PEEK_W: f32 = 380.0;
/// Glide rate of the peek morph (exponential approach), matched to the bell.
const MORPH_RATE: f32 = 13.0;
const MORPH_EPS: f32 = 0.001;

// ---- history box (mirrors the notification history drawer) ----
/// Fully-expanded box height (within the reserved dropdown area).
const EXPANDED_H: f32 = 505.0;
/// Box height with no clips — a small "Nothing copied yet" panel.
const EMPTY_H: f32 = 120.0;
/// A clip row shows up to this many lines of the clip; the row height grows
/// with the line count (single-line clips stay compact).
const MAX_ROW_LINES: usize = 5;
/// Inner vertical padding of a row (top and bottom).
const ROW_PAD_Y: f32 = 9.0;
/// Inner horizontal padding of a row / the box.
const ROW_PAD_X: f32 = 14.0;
/// Bottom padding below the last row before scrolling stops.
const LIST_PAD: f32 = 6.0;
/// Gap between the row text and the trailing time.
const TEXT_GAP: f32 = 8.0;
/// The box's corner radius once fully open (the collapsed pill is a stadium).
const BOX_RADIUS: f32 = 10.0;
/// Adaptive zebra striping — lighten a dark box, darken a light one.
const STRIPE_LIGHTEN: f32 = 0.31;
const STRIPE_DARKEN: f32 = 0.48;
/// Resting (muted) list-ink opacity; the hovered row pops to full contrast.
const LIST_DIM: f32 = 0.55;
const LIST_DIM_LIGHT: f32 = 0.82;
/// Per-row delete (×) hot-square, top-right.
const DELETE_SZ: f32 = 18.0;
/// A multiplication-sign × for the delete controls.
const GLYPH_X: &str = "\u{00d7}";
/// Crimson for the × delete when it's the pointer target.
const CRIMSON: [f32; 4] = [0.878, 0.322, 0.322, 1.0];
/// Leading row glyphs distinguishing non-text clips (until thumbnails land).
const GLYPH_IMAGE: &str = "\u{f03e}"; // fa-image
const GLYPH_FILES: &str = "\u{f0c6}"; // fa-paperclip
/// One wheel notch (`wl_pointer` axis units) — the travel that opens the box.
const NOTCH: f32 = 15.0;
/// Pixels of list scroll per axis unit.
const SCROLL_SPEED: f32 = 3.0;
/// Exponential approach rate of `list_scroll` toward its target.
const SCROLL_RATE: f32 = 20.0;

/// What the pointer is over inside the open history box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipHit {
    None,
    /// A clip row (index into `history`) — click copies it to the clipboard.
    Row(usize),
    /// A row's × delete control.
    Delete(usize),
    /// The footer's ✕ (clear all).
    ClearAll,
}
/// Grace before the preview collapses once the pointer leaves — enough to cross
/// a small gap, snappy otherwise. Matches the bell's `LEAVE_HOLD`.
const LEAVE_HOLD: Duration = Duration::from_millis(300);
/// A fresh clip beats the small pill for this long — one slow heartbeat (swell +
/// settle), the same single-period pulse as the bell's muted-arrival blink.
const BEAT_DURATION: Duration = Duration::from_millis(500);
const BEAT_PERIOD: Duration = Duration::from_millis(500);
/// Gap between the copy/cut/select action pills (and from the small pill).
pub(crate) const ACTION_GAP: f32 = 6.0;
/// After a selection, the action pills show for this long then fade — a
/// confirming glance. They don't linger (deselect can't be detected: the
/// primary selection persists by design), but they re-summon whenever the
/// pointer returns to the clipboard corner while a selection is in play. Reset
/// while you keep selecting, and held while the pointer is on the cluster.
const SHOW_GRACE: Duration = Duration::from_millis(2600);
/// The three selection-action pills, in slide-out order.
pub(crate) const ACTIONS: [(PillId, &str); 3] = [
    (PillId::ClipCopy, GLYPH_COPY),
    (PillId::ClipCut, GLYPH_CUT),
    (PillId::ClipSelectAll, GLYPH_SELECT_ALL),
];

/// On-disk history store (in the daemon's XDG data dir, beside the notif one).
const HISTORY_FILE: &str = "clipboard-history.json";
/// How many entries to retain; older ones (and their image side files) drop.
const MAX_HISTORY: usize = 200;
/// How long to wait before respawning the watcher after it exits/errors.
const RESPAWN: Duration = Duration::from_secs(3);
/// Max characters of clipboard text retained per entry (the clipboard can hold
/// anything — keep the store bounded).
const TEXT_CAP: usize = 100_000;
/// Max characters shown on an entry's one-line preview.
const PREVIEW_CAP: usize = 200;

/// What a captured clip is, so the UI can render it appropriately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipKind {
    /// Plain text (snippets, URLs, code).
    Text,
    /// A `text/uri-list` copy from a file manager (one or more `file://` URIs).
    Files,
    /// An image (PNG/other), stored as bytes on disk.
    Image,
}

/// One captured clipboard entry. Serialized directly to the history store; image
/// pixels live in a side file (`image_path`), never inline, so the JSON stays
/// small.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClipEntry {
    /// Stable per-session id (assigned by the UI on capture; `0` from the
    /// worker). Used to address an entry for copy-back / removal.
    #[serde(default)]
    pub id: u64,
    pub kind: ClipKind,
    pub timestamp_ms: u64,
    /// One-line label for the collapsed preview / list row.
    pub preview: String,
    /// Full text payload for [`ClipKind::Text`] / [`ClipKind::Files`] (used for
    /// copy-back and search); empty for an image.
    #[serde(default)]
    pub text: String,
    /// The MIME type to restore the clip with (`text/plain;charset=utf-8`,
    /// `text/uri-list`, `image/png`, …).
    pub mime: String,
    /// Side file holding the original image bytes, for an image clip.
    #[serde(default)]
    pub image_path: Option<PathBuf>,
    /// Image pixel dimensions (0 for text/files).
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Content hash, for dedup (a re-copied clip moves to the top instead of
    /// piling up).
    pub hash: u64,
}

/// Something the worker observed.
pub enum ClipEvent {
    /// A new clip landed on the clipboard.
    Captured(ClipEntry),
    /// The *primary* (highlight) selection went non-empty (`true`, the user just
    /// selected something) or empty (`false`). Drives the copy/cut/select pills.
    Selection(bool),
}

/// A UI intent for the worker.
pub enum ClipCommand {
    /// Restore this payload to the system clipboard (`wl-copy`). Used by the
    /// history box (a later stage); wired end to end now.
    #[allow(dead_code)]
    Copy {
        mime: String,
        text: String,
        image_path: Option<PathBuf>,
    },
    /// Copy the current *primary* (highlight) selection to the clipboard — the
    /// copy-pill action. Robust: it lifts the selected text straight from the
    /// primary selection rather than injecting a Ctrl+C into the focused app
    /// (whose active highlight may already be gone).
    CopySelection,
}

/// Handle to the clipboard worker: send [`ClipCommand`]s. Dropping it stops the
/// command side after its current recv.
pub struct ClipHandle {
    tx: mpsc::Sender<ClipCommand>,
}

impl ClipHandle {
    pub fn send(&self, cmd: ClipCommand) {
        if let Err(e) = self.tx.send(cmd) {
            warn!("clipboard worker gone, dropping command: {e}");
        }
    }
}

/// Start the clipboard worker. `events` is the calloop channel the UI loop
/// listens on; the returned [`ClipHandle`] carries copy-back commands.
pub fn spawn(events: Sender<ClipEvent>) -> ClipHandle {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || run_worker(events, rx))
        .expect("spawn clipboard worker thread");
    ClipHandle { tx }
}

/// Worker entry: two detached watch loops feeding `events` (the clipboard and
/// the primary/highlight selection), and this thread serving copy-back commands
/// until the handle is dropped.
fn run_worker(events: Sender<ClipEvent>, commands: mpsc::Receiver<ClipCommand>) {
    let primary_events = events.clone();
    std::thread::Builder::new()
        .name("clipboard-watch".into())
        .spawn(move || watch_loop(events))
        .expect("spawn clipboard watch thread");
    std::thread::Builder::new()
        .name("clipboard-primary".into())
        .spawn(move || primary_watch_loop(primary_events))
        .expect("spawn clipboard primary thread");

    while let Ok(cmd) = commands.recv() {
        match cmd {
            ClipCommand::Copy {
                mime,
                text,
                image_path,
            } => {
                let data = match &image_path {
                    Some(path) => match std::fs::read(path) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            warn!("clipboard: cannot read image {path:?}: {e}");
                            continue;
                        }
                    },
                    None => text.into_bytes(),
                };
                wl_copy(&mime, &data);
            }
            ClipCommand::CopySelection => {
                let text = paste_primary();
                if !text.trim().is_empty() {
                    wl_copy("text/plain;charset=utf-8", text.as_bytes());
                }
            }
        }
    }
}

/// Watch the *primary* (highlight) selection and report whether it holds text —
/// this is the "brain" detecting that the user selected something. We only need
/// the presence, never the content, so nothing is stored. Respawns on
/// exit/error like the clipboard watcher.
fn primary_watch_loop(events: Sender<ClipEvent>) {
    // The last selection we reported (`None` = empty), by content hash — so each
    // *distinct* new selection re-triggers the pills, while an app re-asserting
    // the identical selection doesn't. (Edge-only detection was wrong: the
    // primary stays non-empty across selections, so it fired only once.)
    let mut last: Option<u64> = None;
    loop {
        match run_primary_watch(&events, &mut last) {
            Ok(false) => return, // UI gone
            Ok(true) => debug!("clipboard: primary watch ended; respawning"),
            Err(e) => debug!("clipboard: primary watch error: {e}"),
        }
        std::thread::sleep(RESPAWN);
    }
}

fn run_primary_watch(events: &Sender<ClipEvent>, last: &mut Option<u64>) -> std::io::Result<bool> {
    let mut child = Command::new("wl-paste")
        .args(["--primary", "--watch", "sh", "-c", "printf '\\n'"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout"))?;
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        line?;
        // Re-read in Rust (a trigger fires on every selection change). Hash the
        // content so a genuinely new selection lands even while primary stays
        // non-empty; identical re-asserts are dropped.
        let text = paste_primary();
        let hash = (!text.trim().is_empty()).then(|| hash_bytes(text.as_bytes()));
        if hash == *last {
            continue;
        }
        *last = hash;
        if events.send(ClipEvent::Selection(hash.is_some())).is_err() {
            let _ = child.kill();
            return Ok(false);
        }
    }
    let _ = child.wait();
    Ok(true)
}

/// Read the primary selection as text (empty on error / no owner).
fn paste_primary() -> String {
    run_stdout(Command::new("wl-paste").args(["--primary", "--no-newline"]))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// Set the clipboard to `data` with `wl-copy` (which forks to serve the
/// selection, so the write returns promptly).
fn wl_copy(mime: &str, data: &[u8]) {
    let mut c = Command::new("wl-copy");
    c.arg("--type").arg(mime).stdin(Stdio::piped());
    match c.spawn() {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                if let Err(e) = stdin.write_all(data) {
                    warn!("clipboard: wl-copy stdin write failed: {e}");
                }
            }
            let _ = child.wait();
        }
        Err(e) => warn!("clipboard: cannot spawn wl-copy: {e}"),
    }
}

/// Spawn `wl-paste --watch` (a trigger that prints a newline per change) and, on
/// each change, re-read + classify the clipboard. Respawns on exit/error.
fn watch_loop(events: Sender<ClipEvent>) {
    let mut last_hash: Option<u64> = None;
    loop {
        match run_watch(&events, &mut last_hash) {
            // UI gone (channel closed): stop for good.
            Ok(false) => return,
            Ok(true) => debug!("clipboard: wl-paste --watch ended; respawning"),
            Err(e) => debug!("clipboard: watch error: {e}"),
        }
        std::thread::sleep(RESPAWN);
    }
}

/// `Ok(true)` = the watcher ended, respawn; `Ok(false)` = the UI is gone, stop.
fn run_watch(events: &Sender<ClipEvent>, last_hash: &mut Option<u64>) -> std::io::Result<bool> {
    // The command is a bare trigger: it fires on every selection change (any
    // type, including image-only), and we re-read the content ourselves so the
    // classification and any binary handling stays in Rust.
    let mut child = Command::new("wl-paste")
        .args(["--watch", "sh", "-c", "printf '\\n'"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout"))?;
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        line?; // each line = "the clipboard changed"
        if let Some(entry) = capture(last_hash) {
            if events.send(ClipEvent::Captured(entry)).is_err() {
                // UI gone: stop the watcher (and let the child be reaped).
                let _ = child.kill();
                return Ok(false);
            }
        }
    }
    let _ = child.wait();
    Ok(true)
}

/// Read and classify the current clipboard, or `None` if it's empty, unchanged,
/// or an unsupported type.
fn capture(last_hash: &mut Option<u64>) -> Option<ClipEntry> {
    let types = list_types();
    if types.is_empty() {
        return None;
    }
    let entry = if let Some(mime) = image_mime(&types) {
        classify_image(&mime)?
    } else if types.iter().any(|t| t == "text/uri-list") {
        classify_files(&paste_text(Some("text/uri-list")))?
    } else if types.iter().any(|t| t.starts_with("text/")) {
        classify_text(&paste_text(None), best_text_mime(&types))?
    } else {
        return None;
    };
    // Drop an unchanged capture (our own copy-back re-fires the watch, and some
    // apps re-assert the selection repeatedly).
    if *last_hash == Some(entry.hash) {
        return None;
    }
    *last_hash = Some(entry.hash);
    Some(entry)
}

fn classify_text(content: &str, mime: String) -> Option<ClipEntry> {
    if content.trim().is_empty() {
        return None;
    }
    let text: String = content.chars().take(TEXT_CAP).collect();
    Some(ClipEntry {
        hash: hash_bytes(text.as_bytes()),
        preview: one_line(&text),
        kind: ClipKind::Text,
        mime,
        text,
        ..base_entry()
    })
}

fn classify_files(uri_list: &str) -> Option<ClipEntry> {
    let names = file_names(uri_list);
    if names.is_empty() {
        return None;
    }
    Some(ClipEntry {
        hash: hash_bytes(uri_list.as_bytes()),
        preview: cap(&names.join(", "), PREVIEW_CAP),
        kind: ClipKind::Files,
        mime: "text/uri-list".into(),
        text: uri_list.to_owned(),
        ..base_entry()
    })
}

fn classify_image(mime: &str) -> Option<ClipEntry> {
    let bytes = paste_bytes(mime);
    if bytes.is_empty() {
        return None;
    }
    let hash = hash_bytes(&bytes);
    let (width, height) = image_dimensions(&bytes).unwrap_or((0, 0));
    let ext = mime.rsplit('/').next().unwrap_or("bin");
    let path = crate::persist::data_path(&format!("clipboard-images/{hash:016x}.{ext}"));
    crate::persist::write_bytes("clipboard-image", &path, &bytes);
    let preview = if width > 0 {
        format!("Image · {width}×{height}")
    } else {
        "Image".into()
    };
    Some(ClipEntry {
        hash,
        preview,
        kind: ClipKind::Image,
        mime: mime.to_owned(),
        image_path: Some(path),
        width,
        height,
        ..base_entry()
    })
}

/// The common defaults for a freshly captured entry (id assigned by the UI).
fn base_entry() -> ClipEntry {
    ClipEntry {
        id: 0,
        kind: ClipKind::Text,
        timestamp_ms: now_ms(),
        preview: String::new(),
        text: String::new(),
        mime: String::new(),
        image_path: None,
        width: 0,
        height: 0,
        hash: 0,
    }
}

/// `wl-paste --list-types`, one MIME per line (duplicates and legacy pseudo-
/// types like `TEXT`/`STRING` included — we only match on them).
fn list_types() -> Vec<String> {
    run_stdout(Command::new("wl-paste").arg("--list-types"))
        .map(|out| {
            String::from_utf8_lossy(&out)
                .split_whitespace()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn paste_text(mime: Option<&str>) -> String {
    let mut c = Command::new("wl-paste");
    c.arg("--no-newline");
    if let Some(m) = mime {
        c.arg("--type").arg(m);
    }
    run_stdout(&mut c)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

fn paste_bytes(mime: &str) -> Vec<u8> {
    run_stdout(Command::new("wl-paste").args(["--no-newline", "--type", mime])).unwrap_or_default()
}

/// Run `cmd`, returning stdout on a zero exit, else `None`.
fn run_stdout(cmd: &mut Command) -> Option<Vec<u8>> {
    match cmd.stderr(Stdio::null()).output() {
        Ok(out) if out.status.success() => Some(out.stdout),
        Ok(_) => None,
        Err(e) => {
            warn!("clipboard: {:?} failed: {e}", cmd.get_program());
            None
        }
    }
}

/// The best image MIME on offer (prefer PNG), or `None`.
fn image_mime(types: &[String]) -> Option<String> {
    if types.iter().any(|t| t == "image/png") {
        return Some("image/png".into());
    }
    types.iter().find(|t| t.starts_with("image/")).cloned()
}

/// The most specific text MIME to copy back with (prefer a charset-tagged one).
fn best_text_mime(types: &[String]) -> String {
    types
        .iter()
        .find(|t| t.starts_with("text/plain") && t.contains("charset"))
        .or_else(|| types.iter().find(|t| *t == "text/plain"))
        .cloned()
        .unwrap_or_else(|| "text/plain".into())
}

/// Parse a `text/uri-list` into display basenames (percent-decoded `file://`).
fn file_names(uri_list: &str) -> Vec<String> {
    uri_list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let path = l.strip_prefix("file://").unwrap_or(l);
            let decoded = percent_decode(path);
            decoded
                .rsplit('/')
                .find(|s| !s.is_empty())
                .unwrap_or(&decoded)
                .to_owned()
        })
        .collect()
}

/// Minimal percent-decoding for `file://` URIs (spaces etc.).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// First non-empty line, whitespace-collapsed and capped.
fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    cap(&line.split_whitespace().collect::<Vec<_>>().join(" "), PREVIEW_CAP)
}

fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The UI-side clipboard history: the browsable list the OPTION renders, plus
/// the worker handle for copy-back. Persisted across sessions.
pub(crate) struct ClipState {
    // Read by `copy_clip`, which the history box drives.
    #[allow(dead_code)]
    pub handle: Option<ClipHandle>,
    pub history: Vec<ClipEntry>,
    /// Next id to hand a freshly captured entry (monotonic, past any reloaded).
    next_id: u64,
    // ---- peek metamorphosis (bell-style) ----
    /// Preview open (hovered, or held briefly after the pointer leaves).
    peek_reveal: bool,
    /// Morph progress: 0 = resting circle → 1 = full preview pill.
    peek_t: f32,
    /// Frame-clock bookkeeping for the eased morph.
    last: Option<Instant>,
    frame_pending: bool,
    hold_deadline: Option<Instant>,
    /// When the fresh-clip beat on the small pill ends (`None` = not beating).
    blink_until: Option<Instant>,
    // ---- selection action pills (copy / cut / select) ----
    /// A selection context is in play (from a fresh selection until the focused
    /// window changes or an action is taken). While set, moving the pointer to
    /// the clipboard corner re-summons the pills; the peek preview is suppressed.
    selection_present: bool,
    /// Whether the pills are currently shown (the animation target for
    /// `actions_t`). Toggles within a `selection_present` context: on after a
    /// selection or a corner hover, off after the show grace elapses un-hovered.
    selection_active: bool,
    /// Slide-out progress of the action pills: 0 = tucked behind the small pill,
    /// 1 = fully fanned out to the right.
    actions_t: f32,
    /// When the shown pills fade if the pointer hasn't engaged them.
    grace_deadline: Option<Instant>,
    // ---- history box ----
    /// History drawer open (intent), set the instant a scroll opens it.
    pub(crate) expanded: bool,
    /// Vertical growth of the box: 0 = preview pill, 1 = full drawer.
    expand_t: f32,
    /// The eased full-open box height (px), eased toward the content-fit target
    /// so a content change (clearing, deleting a row) morphs smoothly.
    box_h: f32,
    /// Animated / target vertical scroll (px) within the list.
    list_scroll: f32,
    scroll_target: f32,
    /// Accumulated wheel delta so one notch opens the box.
    scroll_accum: f32,
    /// Row under the pointer (for the spotlight), and the fine hit target.
    hover_row: Option<usize>,
    hit: ClipHit,
    /// Pre-measured per-row heights (parallel to `history`), so the variable-
    /// height list lays out identically in the draw and the hit-test.
    row_heights: Vec<f32>,
}

impl ClipState {
    pub fn new(handle: Option<ClipHandle>) -> Self {
        let mut history: Vec<ClipEntry> =
            crate::persist::read_json(&crate::persist::data_path(HISTORY_FILE)).unwrap_or_default();
        history.sort_by_key(|e| std::cmp::Reverse(e.timestamp_ms));
        history.truncate(MAX_HISTORY);
        let next_id = history.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        let row_heights = history.iter().map(row_height_of).collect();
        Self {
            handle,
            history,
            next_id,
            peek_reveal: false,
            peek_t: 0.0,
            last: None,
            frame_pending: false,
            hold_deadline: None,
            blink_until: None,
            selection_present: false,
            selection_active: false,
            actions_t: 0.0,
            grace_deadline: None,
            expanded: false,
            expand_t: 0.0,
            box_h: 0.0,
            list_scroll: 0.0,
            scroll_target: 0.0,
            scroll_accum: 0.0,
            hover_row: None,
            hit: ClipHit::None,
            row_heights,
        }
    }
}

/// The lines a clip shows in a row: up to [`MAX_ROW_LINES`] physical lines for
/// text (blank edges trimmed); a single preview line for files / images.
fn clip_row_lines(entry: &ClipEntry) -> Vec<String> {
    if entry.kind != ClipKind::Text {
        return vec![entry.preview.clone()];
    }
    let mut lines: Vec<String> = entry.text.lines().map(|l| l.trim_end().to_owned()).collect();
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    lines.truncate(MAX_ROW_LINES);
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(entry.preview.clone());
    }
    lines
}

/// The laid-out height of a clip's row (grows with its shown line count).
fn row_height_of(entry: &ClipEntry) -> f32 {
    clip_row_lines(entry).len() as f32 * LINE_PX + 2.0 * ROW_PAD_Y
}

impl App {
    /// A clipboard change arrived from the worker: fold it into the history
    /// (newest first, de-duplicated) and persist.
    pub(crate) fn on_clip_event(&mut self, ev: ClipEvent) {
        match ev {
            ClipEvent::Selection(present) => self.on_clip_selection(present),
            ClipEvent::Captured(mut entry) => {
                // A re-copied clip moves to the top rather than piling up; it
                // keeps its original id so any UI reference stays valid.
                if let Some(pos) = self.clip.history.iter().position(|e| e.hash == entry.hash) {
                    entry.id = self.clip.history.remove(pos).id;
                } else {
                    entry.id = self.clip.next_id;
                    self.clip.next_id += 1;
                }
                self.clip.history.insert(0, entry);
                // Trim the tail, cleaning up any image side files it owned.
                while self.clip.history.len() > MAX_HISTORY {
                    if let Some(old) = self.clip.history.pop() {
                        if let Some(p) = &old.image_path {
                            let _ = std::fs::remove_file(p);
                        }
                    }
                }
                self.measure_clip_rows();
                self.save_clip_history();
                // Keep the open box fitted to the new content.
                if self.clip.expanded {
                    self.clip.box_h = self.clip_full_h();
                    let span = self.clip_scroll_span();
                    self.clip.list_scroll = self.clip.list_scroll.min(span);
                    self.clip.scroll_target = self.clip.scroll_target.min(span);
                    self.update_clip_hit();
                }
                debug!("clipboard: captured; {} entries", self.clip.history.len());
                // A fresh clip landed: beat the small pill (mirrors the bell's
                // muted-arrival blink) as a silent "captured" cue.
                self.trigger_clip_beat();
            }
        }
    }

    /// Click on the small pill: paste the current clip where the user is
    /// working. The newest history entry is always exactly the live clipboard
    /// (we capture every change), so this just injects the paste — no re-copy.
    pub(crate) fn clip_paste(&self) {
        if self.clip.history.is_empty() {
            return;
        }
        crate::hypr::paste_active();
    }

    /// Restore history entry `idx` to the system clipboard (a row click).
    pub(crate) fn copy_clip(&self, idx: usize) {
        let Some(e) = self.clip.history.get(idx) else {
            return;
        };
        if let Some(h) = &self.clip.handle {
            h.send(ClipCommand::Copy {
                mime: e.mime.clone(),
                text: e.text.clone(),
                image_path: e.image_path.clone(),
            });
        }
    }

    fn save_clip_history(&self) {
        crate::persist::write_json(
            "clipboard-history",
            &crate::persist::data_path(HISTORY_FILE),
            &self.clip.history,
        );
    }

    /// Geometry of the morphing preview/box given the pinned *left* edge of its
    /// resting slot (behind the small fixed pill), top, and band height: at rest
    /// it sits exactly under the small pill; as `peek_t` rises its left edge
    /// slides *right* (clearing the small pill plus a bond gap) and it grows to
    /// the preview width — the left-side mirror of the bell, opening rightward.
    pub(crate) fn clip_geom(&self, left0: f32, y: f32, ph: f32) -> Rect {
        let left = left0 + (ph + BOND_GAP) * self.clip.peek_t;
        let w = lerp(ph, PEEK_W, self.clip.peek_t).max(ph);
        // Grows down into the history drawer with `expand_t`. Use the eased
        // `box_h` (seeded on open) so a content change morphs smoothly.
        let full = if self.clip.box_h > 0.0 {
            self.clip.box_h
        } else {
            self.clip_full_h()
        };
        let h = lerp(ph, full, self.clip.expand_t);
        Rect::new(left, y, w, h)
    }

    /// Band height of the resting pill (bar minus its top/bottom margins).
    fn clip_band_h(&self) -> f32 {
        (self.config.options.height as f32 - 2.0 * PILL_MARGIN_Y).max(1.0)
    }

    /// The element's current rect (used by hit-testing / scroll / input region),
    /// recomputed from the same anchor the layout uses.
    fn clip_rect(&self) -> Rect {
        let ph = self.clip_band_h();
        self.clip_geom(crate::options::EDGE_PAD, PILL_MARGIN_Y, ph)
    }

    /// Total stacked height of all rows.
    fn clip_rows_total_h(&self) -> f32 {
        self.clip.row_heights.iter().sum()
    }

    /// Recompute the per-row heights after any history change (the variable-
    /// height list must lay out identically in the draw and the hit-test).
    fn measure_clip_rows(&mut self) {
        self.clip.row_heights = self.clip.history.iter().map(row_height_of).collect();
    }

    /// Fully-expanded box height: fit to content (rows + pad + footer, capped),
    /// or a small panel for the empty state.
    fn clip_full_h(&self) -> f32 {
        if self.clip.history.is_empty() {
            return EMPTY_H;
        }
        let content = self.clip_rows_total_h() + LIST_PAD + self.clip_footer_h();
        content.clamp(self.clip_band_h(), EXPANDED_H)
    }

    /// Diameter of the footer ✕ pill (larger than a bar pill — the box's primary
    /// action).
    fn clip_footer_button_d(&self) -> f32 {
        self.clip_band_h() * 1.4
    }

    /// Height reserved at the box bottom for the floating ✕ pill.
    fn clip_footer_h(&self) -> f32 {
        if self.clip.history.is_empty() {
            return 0.0;
        }
        self.clip_footer_button_d() + 3.0 * PILL_MARGIN_Y
    }

    /// Bottom edge the pointer-input region must reach while the box is open.
    pub(crate) fn clip_input_bottom(&self) -> f32 {
        PILL_MARGIN_Y + self.clip_full_h().max(self.clip.box_h)
    }

    /// The box's content region (above the floating footer, which grows in).
    fn clip_content_rect(&self, rect: Rect) -> Rect {
        let foot = self.clip_footer_h() * self.clip.expand_t;
        Rect::new(rect.x, rect.y, rect.w, (rect.h - foot).max(0.0))
    }

    /// The footer zone rect at the box bottom.
    fn clip_footer_rect(&self, rect: Rect) -> Rect {
        let h = self.clip_footer_h();
        Rect::new(rect.x, rect.y + rect.h - h, rect.w, h)
    }

    /// The centred footer ✕ (clear-all) button rect.
    fn clip_footer_button_rect(&self, rect: Rect) -> Rect {
        let f = self.clip_footer_rect(rect);
        let d = self.clip_footer_button_d();
        Rect::new(f.x + (f.w - d) / 2.0, f.y + (f.h - d) / 2.0, d, d)
    }

    /// Visible clip rows: `(index, row rect)`, newest (index 0) flush at the
    /// content top, each stacked below by its own (variable) height, shifted up
    /// by `list_scroll`. Shared by the draw and the hit-test so they can't
    /// disagree.
    fn clip_rows(&self, rect: Rect) -> Vec<(usize, Rect)> {
        let content = self.clip_content_rect(rect);
        let mut out = Vec::new();
        let mut top = content.y - self.clip.list_scroll;
        for (idx, &h) in self.clip.row_heights.iter().enumerate() {
            let bottom = top + h;
            if bottom > content.y && top < content.y + content.h {
                out.push((idx, Rect::new(rect.x, top, rect.w, h)));
            }
            top = bottom;
        }
        out
    }

    /// Maximum scroll (px): the list bottom past the visible content area.
    fn clip_scroll_span(&self) -> f32 {
        let visible = (EXPANDED_H - self.clip_footer_h()).max(0.0);
        (self.clip_rows_total_h() + LIST_PAD - visible).max(0.0)
    }

    /// Draw the preview/box element: the pill that slides out to the right from
    /// behind the small clipboard glyph — the most-recent clip while collapsed,
    /// the browsable history list once it grows down into the drawer. The glyph
    /// itself lives on the small fixed pill. Mirrors `push_notif_pill`.
    pub(crate) fn push_clip_pill(&self, scene: &mut Scene, rect: Rect) {
        let ph = self.clip_band_h();
        let bright = self.options_bar_is_bright();
        let peek = self.clip.peek_t;
        let e = self.clip.expand_t;
        // Nothing to show until it starts sliding out (at rest it hides fully
        // behind the small pill, which draws on top).
        if peek < 0.001 && e < 0.001 {
            return;
        }
        let radius = lerp(ph / 2.0, BOX_RADIUS, e);
        push_neumorph(scene, rect, radius, bright, 1.0);

        // Box fill: the pill's apparent colour (its backdrop with the wash
        // composited on) so the opaque open box reads as the pill grown. Same
        // maths as the notif box.
        let pill_base = self.options_rest_wash();
        let text_color = self.options_text_color();
        let backdrop = self.options_bar_matched.or(self.options_pill_color);
        let (fill, box_ink) = match backdrop {
            Some(c) => {
                let a = pill_base[3];
                let blend = [
                    c[0] * (1.0 - a) + pill_base[0] * a,
                    c[1] * (1.0 - a) + pill_base[1] * a,
                    c[2] * (1.0 - a) + pill_base[2] * a,
                    1.0,
                ];
                let ink = if self.options_bar_matched.is_some() {
                    text_color
                } else {
                    let lum = 0.2126 * blend[0] + 0.7152 * blend[1] + 0.0722 * blend[2];
                    if lum > 0.179 {
                        [0.0, 0.0, 0.0, 1.0]
                    } else {
                        [0.93, 0.93, 0.96, 1.0]
                    }
                };
                (blend, ink)
            }
            None => ([0.10, 0.10, 0.12, 1.0], [0.93, 0.93, 0.96, 1.0]),
        };
        scene.rects.push(RectInst {
            rect,
            radius,
            color: lerp4(pill_base, fill, e),
            glass: 0.0,
        });

        let ink = lerp4(text_color, box_ink, e);
        let dark_ink = ink[0] + ink[1] + ink[2] < 1.5;
        let hover_ink = if dark_ink {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };
        let list_dim = if dark_ink { LIST_DIM_LIGHT } else { LIST_DIM };
        let dim_ink = [ink[0], ink[1], ink[2], ink[3] * list_dim];

        // Collapsed preview of the newest clip, fading out as the box opens.
        let pa = ((peek - 0.35) / 0.5).clamp(0.0, 1.0) * (1.0 - e);
        if pa > 0.01 {
            if let Some(latest) = self.clip.history.first() {
                let tx = rect.x + PILL_PAD_X;
                let gty = rect.y + (ph - LINE_PX) / 2.0;
                let max_w = (rect.x + rect.w - PILL_PAD_X - tx).max(0.0);
                scene.labels.push(Label {
                    text: latest.preview.clone(),
                    pos: (tx, gty),
                    max_w,
                    font_px: FONT_PX,
                    line_px: LINE_PX,
                    centered: false,
                    dim: false,
                    cache: false,
                    family: None,
                    color: Some([ink[0], ink[1], ink[2], ink[3] * pa]),
                    clip: Some(rect),
                });
            }
        }

        // Open box: the list (fading in with the expand) and the footer.
        if e <= 0.01 {
            return;
        }
        let content = self.clip_content_rect(rect);
        if self.clip.history.is_empty() {
            let a = [dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * e];
            scene.labels.push(Label {
                text: "Nothing copied yet".to_owned(),
                pos: (
                    content.x + content.w / 2.0,
                    content.y + (content.h - LINE_PX) / 2.0,
                ),
                max_w: content.w,
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                family: None,
                color: Some(a),
                clip: Some(content),
            });
            return;
        }

        // Adaptive zebra stripe, pre-composited over the fill into an OPAQUE
        // colour so overlapping pieces overwrite rather than double-blend.
        let flum = 0.2126 * fill[0] + 0.7152 * fill[1] + 0.0722 * fill[2];
        let stripe = if flum <= 0.179 {
            wash(true, STRIPE_LIGHTEN)
        } else {
            wash(false, STRIPE_DARKEN)
        };
        let sa = stripe[3];
        let stripe_opaque = [
            stripe[0] * sa + fill[0] * (1.0 - sa),
            stripe[1] * sa + fill[1] * (1.0 - sa),
            stripe[2] * sa + fill[2] * (1.0 - sa),
            1.0,
        ];

        for (idx, rr) in self.clip_rows(rect) {
            self.push_clip_row(scene, idx, rr, content, e, ink, dim_ink, hover_ink, stripe_opaque);
        }
        self.push_clip_footer(scene, rect, e, bright);
    }

    /// Draw one clip row: zebra background, the clip preview (spotlit when
    /// hovered), a trailing relative time, and a × delete control on hover.
    #[allow(clippy::too_many_arguments)]
    fn push_clip_row(
        &self,
        scene: &mut Scene,
        idx: usize,
        rr: Rect,
        content: Rect,
        e: f32,
        ink: [f32; 4],
        dim_ink: [f32; 4],
        hover_ink: [f32; 4],
        stripe_opaque: [f32; 4],
    ) {
        let Some(entry) = self.clip.history.get(idx) else {
            return;
        };
        let hovered = self.clip.hover_row == Some(idx);
        // Clip the row to the content region so it can't spill over the footer.
        let top = rr.y.max(content.y);
        let bot = (rr.y + rr.h).min(content.y + content.h);
        if bot <= top {
            return;
        }
        // Zebra on odd rows (newest = 0 stays plain).
        if idx % 2 == 1 {
            scene.rects.push(RectInst {
                rect: Rect::new(rr.x, top, rr.w, bot - top),
                radius: 0.0,
                color: stripe_opaque,
                glass: 0.0,
            });
        }

        let col = if hovered {
            [hover_ink[0], hover_ink[1], hover_ink[2], e]
        } else {
            [dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * e]
        };
        let ty = rr.y + (rr.h - LINE_PX) / 2.0;
        let row_clip = Rect::new(rr.x, top, rr.w, bot - top);

        // Trailing time (or the × delete on hover) at the right.
        let right = rr.x + rr.w - ROW_PAD_X;
        let text_right = if hovered {
            // Delete hot-square, top-right.
            let dr = Rect::new(
                rr.x + rr.w - ROW_PAD_X - DELETE_SZ,
                rr.y + (rr.h - DELETE_SZ) / 2.0,
                DELETE_SZ,
                DELETE_SZ,
            );
            let on_x = self.clip.hit == ClipHit::Delete(idx);
            let xc = if on_x { CRIMSON } else { [ink[0], ink[1], ink[2], ink[3]] };
            scene.labels.push(Label {
                text: GLYPH_X.to_owned(),
                pos: (dr.x + dr.w / 2.0, dr.y + (dr.h - LINE_PX) / 2.0),
                max_w: dr.w + 6.0,
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                family: None,
                color: Some([xc[0], xc[1], xc[2], e]),
                clip: Some(row_clip),
            });
            dr.x - TEXT_GAP
        } else {
            let time = fmt_relative(entry.timestamp_ms);
            let tw = time.chars().count() as f32 * FONT_PX * 0.55;
            scene.labels.push(Label {
                text: time,
                pos: (right - tw, ty),
                max_w: tw + 6.0,
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: false,
                dim: false,
                cache: false,
                family: None,
                color: Some([col[0], col[1], col[2], col[3] * 0.85]),
                clip: Some(row_clip),
            });
            right - tw - TEXT_GAP
        };

        // Leading kind glyph on the first line for non-text clips, then the clip
        // text stacked line by line from the row top.
        let ty0 = rr.y + ROW_PAD_Y;
        let mut tx = rr.x + ROW_PAD_X;
        if let Some(glyph) = kind_glyph(entry.kind) {
            scene.labels.push(Label {
                text: glyph.to_owned(),
                pos: (tx, ty0),
                max_w: FONT_PX + 6.0,
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: false,
                dim: false,
                cache: true,
                family: Some(NERD),
                color: Some(col),
                clip: Some(row_clip),
            });
            tx += FONT_PX + TEXT_GAP;
        }
        let max_w = (text_right - tx).max(0.0);
        for (i, line) in clip_row_lines(entry).into_iter().enumerate() {
            scene.labels.push(Label {
                text: line,
                pos: (tx, ty0 + i as f32 * LINE_PX),
                max_w,
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: false,
                dim: false,
                cache: false,
                family: None,
                color: Some(col),
                clip: Some(row_clip),
            });
        }
    }

    /// Draw the footer ✕ (clear all) — an enlarged circle floating on the fill.
    fn push_clip_footer(&self, scene: &mut Scene, rect: Rect, alpha: f32, bright: bool) {
        let hovered = self.clip.hit == ClipHit::ClearAll;
        let br = self.clip_footer_button_rect(rect);
        let br = if hovered { hover_grow(br) } else { br };
        let radius = br.h / 2.0;
        push_neumorph(scene, br, radius, bright, alpha);
        let mut base = if hovered {
            self.options_hover_wash()
        } else {
            self.options_rest_wash()
        };
        base[3] *= alpha;
        scene.rects.push(RectInst {
            rect: br,
            radius,
            color: base,
            glass: 0.0,
        });
        let d0 = self.clip_footer_button_d();
        let gpx = d0 * 0.6;
        let gclip = Rect::new(br.x - 4.0, br.y - 4.0, br.w + 8.0, br.h + 8.0);
        scene.labels.push(Label {
            text: GLYPH_CLOSE.to_owned(),
            pos: (br.x + br.w / 2.0, br.y + (br.h - gpx) / 2.0),
            max_w: br.w + 16.0,
            font_px: gpx,
            line_px: gpx,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some([RED_GLYPH[0], RED_GLYPH[1], RED_GLYPH[2], RED_GLYPH[3] * alpha]),
            clip: Some(gclip),
        });
    }

    /// Whether the history drawer is open (intent) — for the colour-match to
    /// sample the bar's frosted colour for the box. (The box sits at the far
    /// left, clear of the centre window-colour sample, so it needs no column
    /// exclusion the way the right-side notif box does.)
    pub(crate) fn clip_drawer_open(&self) -> bool {
        self.clip.expanded
    }

    /// What the pointer is over inside the open box (footer > row delete > row).
    fn clip_hit(&self) -> ClipHit {
        if self.clip.expand_t < 0.5 {
            return ClipHit::None;
        }
        let Some(p) = self.options_ptr else {
            return ClipHit::None;
        };
        let rect = self.clip_rect();
        if self.clip_footer_rect(rect).contains(p) {
            if self.clip_footer_button_rect(rect).contains(p) {
                return ClipHit::ClearAll;
            }
            return ClipHit::None;
        }
        for (idx, rr) in self.clip_rows(rect) {
            if rr.contains(p) {
                let dr = Rect::new(
                    rr.x + rr.w - ROW_PAD_X - DELETE_SZ,
                    rr.y + (rr.h - DELETE_SZ) / 2.0,
                    DELETE_SZ,
                    DELETE_SZ,
                );
                if dr.contains(p) {
                    return ClipHit::Delete(idx);
                }
                return ClipHit::Row(idx);
            }
        }
        ClipHit::None
    }

    /// Recompute the box hit target + hovered row from the pointer; returns
    /// whether anything changed (so the caller can redraw). On pointer motion.
    pub(crate) fn update_clip_hit(&mut self) -> bool {
        let hit = self.clip_hit();
        let hover_row = match hit {
            ClipHit::Row(i) | ClipHit::Delete(i) => Some(i),
            _ => None,
        };
        let changed = hit != self.clip.hit || hover_row != self.clip.hover_row;
        self.clip.hit = hit;
        self.clip.hover_row = hover_row;
        changed
    }

    /// Whether the pointer is on a clickable box target (for the cursor shape).
    pub(crate) fn clip_box_hit_clickable(&self) -> bool {
        !matches!(self.clip.hit, ClipHit::None)
    }

    /// A wheel event over the clipboard OPTION (raw axis value): opens the box
    /// from the collapsed preview, then scrolls the list. Mirrors `notif_axis`.
    pub(crate) fn clip_axis(&mut self, value: f32) {
        let delta = if self.config.input.natural_scroll {
            value
        } else {
            -value
        };
        if self.clip.expanded {
            self.clip.hold_deadline = None; // a scroll keeps it open
            let span = self.clip_scroll_span();
            self.clip.scroll_target =
                (self.clip.scroll_target + delta * SCROLL_SPEED).clamp(0.0, span);
            self.clip.scroll_accum = 0.0;
            self.schedule_clip_frame();
        } else {
            self.clip.scroll_accum += delta;
            if self.clip.scroll_accum.abs() >= NOTCH {
                self.clip.scroll_accum = 0.0;
                self.open_clip_box();
            }
        }
    }

    /// Open the history drawer from the collapsed preview, newest flush at top.
    fn open_clip_box(&mut self) {
        if self.clip.history.is_empty() {
            return;
        }
        self.clip.hold_deadline = None;
        self.clip.peek_reveal = true; // keep the preview fully out under the box
        self.clip.expanded = true;
        self.clip.box_h = self.clip_full_h();
        self.clip.list_scroll = 0.0;
        self.clip.scroll_target = 0.0;
        self.clip.scroll_accum = 0.0;
        self.sync_options_input();
        self.reeval_options_bar();
        self.schedule_clip_frame();
    }

    /// Collapse the open history drawer back to the pill.
    fn close_clip_box(&mut self) {
        if self.clip.expanded {
            self.clip.expanded = false;
            self.clip.hit = ClipHit::None;
            self.clip.hover_row = None;
            self.sync_options_input();
            self.schedule_clip_frame();
        }
    }

    /// Handle a click inside the open box (footer / row delete / row). Returns
    /// whether it consumed the click.
    pub(crate) fn clip_box_click(&mut self) -> bool {
        match self.clip.hit {
            ClipHit::ClearAll => {
                self.clear_clip_history();
                true
            }
            ClipHit::Delete(i) => {
                self.delete_clip(i);
                true
            }
            ClipHit::Row(i) => {
                // Copy the clip back to the clipboard, then dismiss the picker so
                // the user can paste straight away.
                self.copy_clip(i);
                self.close_clip_box();
                true
            }
            ClipHit::None => false,
        }
    }

    /// Delete one clip from the history (its × control).
    fn delete_clip(&mut self, idx: usize) {
        if idx >= self.clip.history.len() {
            return;
        }
        let entry = self.clip.history.remove(idx);
        if let Some(p) = &entry.image_path {
            let _ = std::fs::remove_file(p);
        }
        self.measure_clip_rows();
        self.save_clip_history();
        if self.clip.history.is_empty() {
            self.close_clip_box();
        } else {
            self.clip.box_h = self.clip_full_h();
            let span = self.clip_scroll_span();
            self.clip.list_scroll = self.clip.list_scroll.min(span);
            self.clip.scroll_target = self.clip.scroll_target.min(span);
        }
        self.update_clip_hit();
        self.schedule_clip_frame();
    }

    /// Clear the whole history (footer ✕).
    fn clear_clip_history(&mut self) {
        for entry in &self.clip.history {
            if let Some(p) = &entry.image_path {
                let _ = std::fs::remove_file(p);
            }
        }
        self.clip.history.clear();
        self.clip.row_heights.clear();
        self.save_clip_history();
        self.close_clip_box();
    }

    /// Draw the small fixed clipboard-glyph pill (the left-edge anchor the box
    /// slides out from behind). Mirrors the bell (`push_notif_mute`): a fresh
    /// clip beats it — the whole pill pulses like a hover (grow + wash), swelling
    /// and settling over one heartbeat; a real hover pins it fully lifted.
    pub(crate) fn push_clip_glyph(&self, scene: &mut Scene, rect: Rect) {
        let hovered = self.options_hover == Some(PillId::Clipboard);
        let beat = self.clip_blink().unwrap_or(0.0);
        let lift = if hovered { 1.0 } else { beat };
        let grown = hover_grow(rect);
        let rect = Rect::new(
            lerp(rect.x, grown.x, lift),
            lerp(rect.y, grown.y, lift),
            lerp(rect.w, grown.w, lift),
            lerp(rect.h, grown.h, lift),
        );
        let bright = self.options_bar_is_bright();
        let radius = rect.h / 2.0; // stadium with w == h ⇒ circle
        push_neumorph(scene, rect, radius, bright, 1.0);
        // Same gentle beat colour as the bell's muted landing: on a dark bar the
        // peak flashes toward white, on a light bar toward the adaptive hover
        // wash; a real hover just settles at the normal hover wash.
        let hover = self.options_hover_wash();
        let peak = if hovered {
            hover
        } else if bright {
            [hover[0], hover[1], hover[2], 0.55]
        } else {
            [1.0, 1.0, 1.0, 0.5]
        };
        let base = lerp4(self.options_rest_wash(), peak, lift);
        scene.rects.push(RectInst {
            rect,
            radius,
            color: base,
            glass: 0.0,
        });

        let ink = self.options_text_color();
        let cx = rect.x + rect.w / 2.0;
        let ty = rect.y + (rect.h - LINE_PX) / 2.0;
        scene.labels.push(Label {
            text: GLYPH_CLIPBOARD.to_owned(),
            pos: (cx, ty),
            max_w: rect.w + 4.0,
            font_px: FONT_PX,
            line_px: LINE_PX,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some(ink),
            clip: Some(rect),
        });
    }

    /// Start a fresh-clip beat on the small pill and drive the frame loop.
    fn trigger_clip_beat(&mut self) {
        self.clip.blink_until = Some(Instant::now() + BEAT_DURATION);
        self.clip.last = None;
        self.schedule_clip_frame();
    }

    /// The fresh-clip heartbeat (`0.0..=1.0`, `None` when idle): a smooth pulse
    /// over one [`BEAT_PERIOD`] that swells to `1` and settles back to `0`.
    /// Smoothstepped so it breathes rather than ticks. Same shape as the bell's.
    fn clip_blink(&self) -> Option<f32> {
        let until = self.clip.blink_until?;
        let now = Instant::now();
        if now >= until {
            return None;
        }
        let rem = (until - now).as_secs_f32();
        let phase = (rem / BEAT_PERIOD.as_secs_f32()).fract();
        let tri = 1.0 - (phase * 2.0 - 1.0).abs();
        Some(tri * tri * (3.0 - 2.0 * tri)) // smoothstep for an eased beat
    }

    /// Recompute whether the preview should show, and manage the hold/collapse
    /// after the pointer leaves (mirrors the bell's peek). Hovering either the
    /// small pill or the box it reveals holds it open.
    pub(crate) fn update_clip_reveal(&mut self) {
        // While a selection is in play, the clipboard corner is the action pills'
        // home: hovering it (re-)summons them and holds them; moving away lets
        // them fade after the grace. The history peek is suppressed in this mode.
        if self.clip.selection_present {
            if self.clip_cluster_hovered() {
                self.clip.grace_deadline = None;
                if !self.clip.selection_active {
                    self.clip.selection_active = true;
                    self.clip.peek_reveal = false;
                    self.clip.last = None;
                    self.schedule_clip_frame();
                }
            } else if self.clip.selection_active && self.clip.grace_deadline.is_none() {
                self.arm_actions_grace();
            }
            return;
        }
        // Normal history peek (no selection context).
        let on = matches!(
            self.options_hover,
            Some(PillId::Clipboard | PillId::ClipboardBox)
        );
        if on {
            self.clip.hold_deadline = None;
            if !self.clip.peek_reveal && !self.clip.history.is_empty() {
                self.clip.peek_reveal = true;
                self.clip.last = None;
                self.schedule_clip_frame();
            }
        } else if self.clip.peek_reveal && self.clip.hold_deadline.is_none() {
            self.schedule_clip_collapse(LEAVE_HOLD);
        }
    }

    /// A new selection landed (the `--watch` fires on every primary change):
    /// open the action pills and (re)start their fade grace. `present == false`
    /// (a rare clear, for apps that do clear their primary) ends the context.
    fn on_clip_selection(&mut self, present: bool) {
        // Don't pop the action pills over the open history box.
        if self.clip.expanded {
            return;
        }
        if !present {
            self.end_clip_selection();
            return;
        }
        self.clip.selection_present = true;
        self.clip.peek_reveal = false; // the preview yields to the actions
        if !self.clip.selection_active {
            self.clip.selection_active = true;
            self.clip.last = None;
            self.schedule_clip_frame();
        }
        self.arm_actions_grace();
    }

    /// After the show grace, fade the pills unless the pointer is on the cluster.
    /// The selection *context* stays, so a corner hover re-summons them.
    fn arm_actions_grace(&mut self) {
        let deadline = Instant::now() + SHOW_GRACE;
        self.clip.grace_deadline = Some(deadline);
        let timer = Timer::from_duration(SHOW_GRACE);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.clip.grace_deadline == Some(deadline) {
                    app.clip.grace_deadline = None;
                    if !app.clip_cluster_hovered() {
                        app.collapse_clip_actions();
                    }
                }
                TimeoutAction::Drop
            });
    }

    /// Fade the pills out but keep the selection context (re-summonable).
    fn collapse_clip_actions(&mut self) {
        if self.clip.selection_active {
            self.clip.selection_active = false;
            self.clip.last = None;
            self.schedule_clip_frame();
        }
    }

    /// End the selection context entirely — the focused window changed or an
    /// action was taken, so there's nothing left to act on. Also fades the pills.
    pub(crate) fn end_clip_selection(&mut self) {
        self.clip.selection_present = false;
        self.clip.grace_deadline = None;
        self.collapse_clip_actions();
    }

    /// Whether the pointer is over any clipboard element — the small pill, the
    /// preview box, or one of the action pills.
    fn clip_cluster_hovered(&self) -> bool {
        matches!(
            self.options_hover,
            Some(
                PillId::Clipboard
                    | PillId::ClipboardBox
                    | PillId::ClipCopy
                    | PillId::ClipCut
                    | PillId::ClipSelectAll
            )
        )
    }

    /// Slide-out progress of the action pills (0 hidden → 1 fanned out), read by
    /// the layout to place them.
    pub(crate) fn clip_actions_t(&self) -> f32 {
        self.clip.actions_t
    }

    /// Draw one selection-action pill (copy / cut / select), fading + sliding in
    /// with the shared `actions_t`. Emerges from behind the small pill, which
    /// draws on top.
    pub(crate) fn push_clip_action(&self, scene: &mut Scene, rect: Rect, id: PillId, glyph: &str) {
        // Fade in a touch after the slide begins, so it reads as coming out from
        // under the small pill rather than blinking on.
        let a = ((self.clip.actions_t - 0.15) / 0.6).clamp(0.0, 1.0);
        if a <= 0.01 {
            return;
        }
        let bright = self.options_bar_is_bright();
        let hovered = self.options_hover == Some(id);
        let rect = if hovered { hover_grow(rect) } else { rect };
        let radius = rect.h / 2.0;
        push_neumorph(scene, rect, radius, bright, a);
        let base = if hovered {
            self.options_hover_wash()
        } else {
            self.options_rest_wash()
        };
        scene.rects.push(RectInst {
            rect,
            radius,
            color: [base[0], base[1], base[2], base[3] * a],
            glass: 0.0,
        });
        let ink = self.options_text_color();
        scene.labels.push(Label {
            text: glyph.to_owned(),
            pos: (rect.x + rect.w / 2.0, rect.y + (rect.h - LINE_PX) / 2.0),
            max_w: rect.w + 4.0,
            font_px: FONT_PX,
            line_px: LINE_PX,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some([ink[0], ink[1], ink[2], ink[3] * a]),
            clip: Some(rect),
        });
    }

    /// Click on an action pill.
    ///
    /// - **Copy** lifts the selected text straight from the *primary* selection
    ///   into the clipboard (robust — no dependence on the focused app still
    ///   showing an active highlight, which is why injecting Ctrl+C was flaky).
    /// - **Cut** does the same robust copy, then injects Ctrl+X so the app also
    ///   removes the selection.
    /// - **Select-all** injects Ctrl+A (only the app can do it), and keeps the
    ///   pills up so the user can chain into copy.
    ///
    /// Copy/cut retire the pills once done; the resulting clipboard change flows
    /// back through the watcher (history + beat).
    pub(crate) fn clip_action(&mut self, id: PillId) {
        match id {
            PillId::ClipCopy => {
                self.send_clip(ClipCommand::CopySelection);
                self.end_clip_selection();
            }
            PillId::ClipCut => {
                self.send_clip(ClipCommand::CopySelection);
                crate::hypr::send_shortcut_active("CTRL", "x");
                self.end_clip_selection();
            }
            PillId::ClipSelectAll => {
                // Only the app can select-all; it updates the primary, so the
                // watch re-fires and keeps the pills up for a follow-up copy.
                crate::hypr::send_shortcut_active("CTRL", "a");
                self.arm_actions_grace();
            }
            _ => {}
        }
    }

    fn send_clip(&self, cmd: ClipCommand) {
        if let Some(h) = &self.clip.handle {
            h.send(cmd);
        }
    }

    /// After the pointer leaves, hold briefly then collapse the preview.
    fn schedule_clip_collapse(&mut self, hold: Duration) {
        let deadline = Instant::now() + hold;
        self.clip.hold_deadline = Some(deadline);
        let timer = Timer::from_duration(hold);
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                if app.clip.hold_deadline == Some(deadline) {
                    app.clip.hold_deadline = None;
                    if !matches!(
                        app.options_hover,
                        Some(PillId::Clipboard | PillId::ClipboardBox)
                    ) {
                        app.clip.peek_reveal = false;
                        // Collapse the history drawer too, if it was open.
                        if app.clip.expanded {
                            app.clip.expanded = false;
                            app.clip.hit = ClipHit::None;
                            app.clip.hover_row = None;
                            app.sync_options_input();
                        }
                        app.clip.last = None;
                        app.schedule_clip_frame();
                    }
                }
                TimeoutAction::Drop
            });
    }

    fn schedule_clip_frame(&mut self) {
        if self.clip.frame_pending {
            return;
        }
        self.clip.frame_pending = true;
        if self.clip.last.is_none() {
            self.clip.last = Some(Instant::now());
        }
        let timer = Timer::from_duration(Duration::from_millis(8));
        let _ = self
            .loop_handle
            .insert_source(timer, |_, _, app: &mut App| {
                app.clip.frame_pending = false;
                app.tick_clip();
                TimeoutAction::Drop
            });
    }

    /// Advance the peek morph one frame; the pill width and preview fade derive
    /// from `peek_t` at draw time.
    fn tick_clip(&mut self) {
        let now = Instant::now();
        let dt = self
            .clip
            .last
            .map_or(0.0, |l| now.duration_since(l).as_secs_f32().min(0.05));
        self.clip.last = Some(now);
        let target = if self.clip.peek_reveal { 1.0 } else { 0.0 };
        let (pt, moving) = ease_toward(self.clip.peek_t, target, dt, MORPH_RATE, MORPH_EPS);
        self.clip.peek_t = pt;
        // Slide the action pills toward their selection-driven target.
        let atarget = if self.clip.selection_active { 1.0 } else { 0.0 };
        let (at, amoving) = ease_toward(self.clip.actions_t, atarget, dt, MORPH_RATE, MORPH_EPS);
        self.clip.actions_t = at;
        // Grow / collapse the history drawer.
        let etarget = if self.clip.expanded { 1.0 } else { 0.0 };
        let (et, em) = ease_toward(self.clip.expand_t, etarget, dt, MORPH_RATE, MORPH_EPS);
        self.clip.expand_t = et;
        // Ease the open-box height toward its content-fit target (smooth on a
        // content change); only while open, so there's no idle churn collapsed.
        let bm = if self.clip.expand_t > MORPH_EPS {
            let (bh, moving) =
                ease_toward(self.clip.box_h, self.clip_full_h(), dt, MORPH_RATE * 1.3, 0.5);
            self.clip.box_h = bh;
            moving
        } else {
            false
        };
        // Smooth list scrolling.
        let (ls, lm) = ease_toward(
            self.clip.list_scroll,
            self.clip.scroll_target,
            dt,
            SCROLL_RATE,
            0.5,
        );
        self.clip.list_scroll = ls;
        // Keep the beat pulsing until its deadline, then clear it.
        let beating = self.clip.blink_until.is_some_and(|u| now < u);
        if !beating {
            self.clip.blink_until = None;
        }
        self.draw_options();
        if moving || amoving || em || bm || lm || beating {
            self.schedule_clip_frame();
        } else {
            self.clip.last = None;
        }
    }
}

/// The leading glyph for a non-text clip row (`None` for plain text).
fn kind_glyph(kind: ClipKind) -> Option<&'static str> {
    match kind {
        ClipKind::Image => Some(GLYPH_IMAGE),
        ClipKind::Files => Some(GLYPH_FILES),
        ClipKind::Text => None,
    }
}

/// A short relative time for a clip row (e.g. `now`, `5m`, `3h`, `2d`).
fn fmt_relative(ms: u64) -> String {
    let now = now_ms();
    let secs = now.saturating_sub(ms) / 1000;
    if secs < 45 {
        "now".to_owned()
    } else if secs < 3600 {
        format!("{}m", (secs / 60).max(1))
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
        lerp(a[3], b[3], t),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_classifies_with_preview() {
        let e = classify_text("  hello  world  \nsecond line", "text/plain".into()).unwrap();
        assert_eq!(e.kind, ClipKind::Text);
        assert_eq!(e.preview, "hello world");
        assert!(e.hash != 0);
    }

    #[test]
    fn blank_text_is_dropped() {
        assert!(classify_text("   \n\t ", "text/plain".into()).is_none());
    }

    #[test]
    fn files_show_basenames() {
        let list = "file:///home/max/a%20b.txt\nfile:///home/max/photos/pic.png\n";
        let e = classify_files(list).unwrap();
        assert_eq!(e.kind, ClipKind::Files);
        assert_eq!(e.preview, "a b.txt, pic.png");
        assert_eq!(e.mime, "text/uri-list");
    }

    #[test]
    fn empty_uri_list_is_dropped() {
        assert!(classify_files("# comment only\n").is_none());
    }

    #[test]
    fn prefers_png_image_mime() {
        let types = vec!["image/bmp".into(), "image/png".into()];
        assert_eq!(image_mime(&types).as_deref(), Some("image/png"));
        assert_eq!(
            image_mime(&["image/jpeg".into()]).as_deref(),
            Some("image/jpeg")
        );
        assert!(image_mime(&["text/plain".into()]).is_none());
    }

    #[test]
    fn preview_is_capped() {
        let big = "x".repeat(500);
        let e = classify_text(&big, "text/plain".into()).unwrap();
        assert_eq!(e.preview.chars().count(), PREVIEW_CAP);
        assert!(e.preview.ends_with('…'));
    }
}

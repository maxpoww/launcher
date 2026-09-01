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
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use calloop::channel::Sender;
use calloop::timer::{TimeoutAction, Timer};
use serde::{Deserialize, Serialize};
use smithay_client_toolkit::seat::keyboard::Keysym;
use tracing::{debug, warn};

use crate::animation::{ease_toward, lerp};
use crate::content::{GridContent, IconInst, Label, Rect, RectInst, Scene};
use crate::options::{
    hover_grow, push_neumorph, wash, PillId, BOND_GAP, FONT_PX, GLYPH_CLIPBOARD,
    GLYPH_COPY, LINE_PX, NERD, PILL_MARGIN_Y, PILL_PAD_X,
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
/// A clip row shows up to this many lines of the clip (text word-wrapped to the
/// row width so a paragraph fills the card); the row height grows with the line
/// count and short clips stay compact.
const MAX_ROW_LINES: usize = 4;
/// Inner vertical padding of a row (top and bottom).
const ROW_PAD_Y: f32 = 9.0;
/// Inner horizontal padding of a row / the box.
const ROW_PAD_X: f32 = 14.0;
/// Bottom padding below the last row before scrolling stops.
const LIST_PAD: f32 = 6.0;
/// Right inset for the trailing time / delete can — tighter than `ROW_PAD_X` so
/// they hug the row's right edge (top-right corner).
const TRAIL_PAD_X: f32 = 6.0;
/// Gap between the row text and the trailing time.
const TEXT_GAP: f32 = 8.0;
/// Width reserved at a row's right edge for the trailing relative-time label (or
/// the hover delete square). Wrapping uses this fixed reserve — independent of the
/// actual time string and of hover — so the measured row height and the drawn text
/// can't disagree on where lines break.
const TIME_COL_W: f32 = 52.0;
/// The box's corner radius once fully open (the collapsed pill is a stadium).
const BOX_RADIUS: f32 = 10.0;
/// Adaptive zebra striping — lighten a dark box, darken a light one.
const STRIPE_LIGHTEN: f32 = 0.31;
const STRIPE_DARKEN: f32 = 0.48;
/// Resting list-ink opacity; the hovered row spends the headroom these leave
/// (see `options::hover_ink_for`). The twins of the notif box's — and see
/// that file for why the two regimes sit so far apart: a dark box has
/// contrast to spare, a backdrop-coloured light box does not.
const LIST_DIM: f32 = 0.67;
const LIST_DIM_LIGHT: f32 = 0.88;
const DELETE_SZ: f32 = 18.0;
/// fa-trash-o (outline can with vertical lines) — the per-item delete controls
/// (row + detail).
const GLYPH_TRASH: &str = "\u{f014}";
/// fa-pencil — the footer "new note" button (opens the note editor).
const GLYPH_NOTE: &str = "\u{f040}";
/// fa-book — the footer "dictionary" button.
const GLYPH_BOOK: &str = "\u{f02d}";
/// Gap between the two footer buttons (new note / dictionary).
const FOOTER_GAP: f32 = 26.0;
/// Height of the dictionary panel's search field — taller than a clip row for a
/// comfortable, obvious input box.
const DICT_FIELD_H: f32 = 46.0;
/// Leading row glyphs for non-text clips without a thumbnail (dirs / plain files).
const GLYPH_IMAGE: &str = "\u{f03e}"; // fa-image
const GLYPH_FILES: &str = "\u{f0c6}"; // fa-paperclip
const GLYPH_FOLDER: &str = "\u{f07b}"; // fa-folder (a directory clip)
const GLYPH_LINK: &str = "\u{f0c1}"; // fa-link (a link clip's tile placeholder)
/// Square icon/thumbnail tile at the left of every image / files row. The list
/// stays compact — the full-size preview lives in the metadata detail view.
const TILE_SZ: f32 = 56.0;
/// Detail view: top pill-row gap + left inset (pill diameter matches the paste
/// pill = `clip_band_h`). The row-grow open animation runs over this many secs.
const DETAIL_PILL_GAP: f32 = 6.0;
const DETAIL_PILL_X: f32 = 12.0;
/// Width of the "‹ Back" text button's hit area at the detail view's top-left.
const DETAIL_BACK_W: f32 = 78.0;
/// Downward nudge of the top-row controls from the box's top edge.
const DETAIL_PILL_Y: f32 = 7.0;
const DETAIL_OPEN_SECS: f32 = 0.34;
/// Height of one metadata row in the detail view.
const META_ROW_H: f32 = 30.0;
/// Detail text box insets: side margins (centred column) and top/bottom.
const DETAIL_TEXT_MX: f32 = 34.0;
const DETAIL_TEXT_MY: f32 = 30.0;
/// The metadata sheet's inner top inset + bottom pad.
const META_INNER_TOP: f32 = 16.0;
const META_INNER_BOT: f32 = 14.0;
/// The metadata sheet: resting height (fraction of box, clamped) and the max
/// it rises to when hovered.
const META_REST_FRAC: f32 = 0.30;
const META_REST_MIN: f32 = 128.0;
const META_REST_MAX: f32 = 176.0;
const META_FULL_FRAC: f32 = 0.62;
/// Left align of the metadata (matches the text box's left margin) + the label
/// column width, and the paste-log indent under the values.
const META_LABEL_W: f32 = 62.0;
const META_LOG_INDENT: f32 = 70.0;
/// fa-chevron-left, the back-pill glyph.
const GLYPH_BACK: &str = "\u{f053}";
/// fa-ellipsis, the placeholder 4th top pill.
const GLYPH_MORE: &str = "\u{f141}";
/// fa-external-link, the link "open in browser" pill (links only).
const GLYPH_OPEN: &str = "\u{f08e}";
/// Texture-array slots kept for clipboard thumbnails (recycled round-robin),
/// appended after the notif card avatars on the OPTIONS renderer.
const THUMB_CAP: usize = 32;
/// One wheel notch (`wl_pointer` axis units) — the travel that opens the box.
const NOTCH: f32 = 15.0;
/// Pixels of list scroll per axis unit.
const SCROLL_SPEED: f32 = 3.0;
/// Exponential approach rate of `list_scroll` toward its target.
const SCROLL_RATE: f32 = 20.0;

/// Smoothstep of the linear detail-open progress `p`, remapped to the sub-window
/// `[a, b]` and clamped. Used to stagger the detail view's elements so they
/// cascade (card leads, chrome follows) instead of arriving in lockstep; the
/// same remap run in reverse on close gives the mirror cascade.
fn stagger(p: f32, a: f32, b: f32) -> f32 {
    let t = ((p - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What the pointer is over inside the open history box.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipHit {
    None,
    /// A clip row (index into `history`) — click copies it to the clipboard.
    Row(usize),
    /// A row's × delete control.
    Delete(usize),
    /// The footer's "new note" button — opens the floating note editor.
    NewNote,
    /// The footer's "dictionary" button.
    Dictionary,
    /// The footer's "clear all" can (mirrors the notification box's).
    ClearAll,
    /// The detail view's top-strip pills.
    Back,
    DetailDelete,
    DetailCopy,
    DetailMore,
    /// Open the link in a browser (links only).
    DetailOpen,
}
/// Grace before the preview collapses once the pointer leaves — enough to cross
/// a small gap, snappy otherwise. Matches the bell's `LEAVE_HOLD`.
const LEAVE_HOLD: Duration = Duration::from_millis(300);
/// A fresh clip beats the small pill for this long — one slow heartbeat (swell +
/// settle), the same single-period pulse as the bell's muted-arrival blink.
const BEAT_DURATION: Duration = Duration::from_millis(500);
const BEAT_PERIOD: Duration = Duration::from_millis(500);
/// Gap between the copy-link pill and the small clipboard pill.
pub(crate) const LINK_GAP: f32 = 6.0;

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

/// One recorded paste of a clip: where it landed and when. Drives the "pasted
/// N times — where & when" section of the metadata detail view.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PasteRecord {
    /// The window it was pasted into (app class / title), best-effort.
    #[serde(default)]
    pub target: String,
    /// Unix-ms when the paste happened.
    #[serde(default)]
    pub when_ms: u64,
}

/// How many recent paste records to keep per clip (the total count is tracked
/// separately so it stays accurate even past this cap).
const PASTE_LOG_CAP: usize = 20;

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
    /// `text/uri-list`, `image/png`, …). Legacy: a restored clip now advertises
    /// a whole set of types at once (see [`clip_offers`]); this is kept for the
    /// image byte-type and for back-compat of stored history.
    pub mime: String,
    /// For a [`ClipKind::Files`] clip: the file-manager operation — `false` =
    /// copy, `true` = cut (move). Captured from `x-special/gnome-copied-files`
    /// (defaults to copy) and re-served in that type so a paste from history
    /// moves rather than copies, matching a real Ctrl+X.
    #[serde(default)]
    pub cut: bool,
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
    /// Where it was copied from — a file clip's source directory, or the focused
    /// window (app / title) at copy time. Best-effort; empty if unknown.
    #[serde(default)]
    pub source: String,
    /// Total number of times this clip has been pasted (may exceed
    /// `pastes.len()`, which is capped at [`PASTE_LOG_CAP`]).
    #[serde(default)]
    pub paste_count: u64,
    /// The most recent pastes (where & when), newest first, capped.
    #[serde(default)]
    pub pastes: Vec<PasteRecord>,
    /// For a link clip: the page title. Seeded from the source window's title at
    /// copy time, later replaced by a network unfurl's `og:title` when enabled.
    /// Empty for a non-link or an unresolved link.
    #[serde(default)]
    pub title: String,
    /// For a link clip: an optional preview/hero image side file (a capture of
    /// the source window, or the unfurled `og:image`), fed through the same
    /// thumbnail pipeline as image clips. `None` until enrichment produces one.
    #[serde(default)]
    pub preview_image: Option<PathBuf>,
    /// For a link clip: the page description from a network unfurl
    /// (`og:description`). Empty unless unfurl is enabled and succeeded.
    #[serde(default)]
    pub description: String,
}

impl ClipEntry {
    /// The clip's URL when the whole clip is a single bare http(s) link (a link
    /// clip), else `None`. Detected on the fly so it also applies to history
    /// captured before link enrichment existed.
    pub fn link_url(&self) -> Option<&str> {
        if self.kind != ClipKind::Text {
            return None;
        }
        detect_url(&self.text)
    }

    /// Whether this clip is a single-URL link clip (eligible for enrichment).
    pub fn is_link(&self) -> bool {
        self.link_url().is_some()
    }
}

/// If `text` is a single bare http(s) URL (no surrounding whitespace or extra
/// content), return it — the signal that a text clip is really a link.
fn detect_url(text: &str) -> Option<&str> {
    let t = text.trim();
    if t.split_whitespace().count() != 1 {
        return None;
    }
    let rest = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://"))?;
    // A real host has a dot before any path/query, and the scheme isn't the
    // whole string.
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.contains('.') && !host.is_empty() {
        Some(t)
    } else {
        None
    }
}

/// Something the worker observed.
pub enum ClipEvent {
    /// A new clip landed on the clipboard. Boxed: `ClipEntry` is large relative
    /// to the other variants.
    Captured(Box<ClipEntry>),
    /// An app read (pasted) the clip we're currently serving via the native
    /// data-control source — best-effort, debounced, our own re-reads excluded.
    /// Recorded against the current clip (where & when).
    Pasted,
}

/// A UI intent for the worker.
pub enum ClipCommand {
    /// Restore this clip to the system clipboard. The worker advertises the
    /// full set of types the clip should offer (see [`clip_offers`]) via a
    /// native data-control source, so a file clip pastes into thunar/nautilus
    /// (`x-special/gnome-copied-files`) *and* editors (`text/plain`) alike.
    /// Boxed: `ClipEntry` is large relative to the other variants.
    Copy(Box<ClipEntry>),
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

/// Worker entry: a detached clipboard watch loop feeding `events`, and this
/// thread serving copy-back commands until the handle is dropped.
fn run_worker(events: Sender<ClipEvent>, commands: mpsc::Receiver<ClipCommand>) {
    let source_events = events.clone();
    std::thread::Builder::new()
        .name("clipboard-watch".into())
        .spawn(move || watch_loop(events))
        .expect("spawn clipboard watch thread");

    // A native wlr-data-control source lets us own the selection with *every*
    // MIME type at once (so a file clip pastes into thunar AND editors). If the
    // compositor lacks the protocol we degrade to single-type `wl-copy`. The
    // source also reports pastes of what it serves back over `events`.
    let source = crate::clip_source::spawn(source_events);
    let serve = |offers: Vec<(String, Vec<u8>)>| match &source {
        Some(s) => s.serve(offers),
        None => {
            if let Some((mime, data)) = offers.first() {
                wl_copy(mime, data);
            }
        }
    };

    while let Ok(cmd) = commands.recv() {
        match cmd {
            ClipCommand::Copy(entry) => serve(clip_offers(&entry)),
        }
    }
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
    // First failure warns (a missing wl-paste otherwise kills the clipboard
    // OPTION in silence — SH audit F1); the 3 s respawn retries stay at debug.
    let mut warned = false;
    loop {
        match run_watch(&events, &mut last_hash) {
            // UI gone (channel closed): stop for good.
            Ok(false) => return,
            Ok(true) => debug!("clipboard: wl-paste --watch ended; respawning"),
            Err(e) if !warned => {
                warned = true;
                warn!("clipboard: watch failed ({e}) — is wl-paste installed? retrying");
            }
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
            if events.send(ClipEvent::Captured(Box::new(entry))).is_err() {
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
    let mut entry = if let Some(mime) = image_mime(&types) {
        classify_image(&mime)?
    } else if types.iter().any(|t| t == "text/uri-list" || t == GNOME_COPIED) {
        let (cut, uri_list) = read_file_clip(&types);
        classify_files(&uri_list, cut)?
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
    // Fetch the focused window once and use it for both the "copied from" label
    // and (for a link clip) the seed page title.
    let win = if entry.kind == ClipKind::Files {
        None
    } else {
        crate::hypr::active_window_where()
    };
    entry.source = capture_source(&entry, win.as_ref());
    if entry.is_link() {
        if let Some((class, title)) = &win {
            entry.title = clean_link_title(title);
            // Fallback hero: snapshot the source window, but only when the copy
            // came from a browser (where the window depicts the page — e.g.
            // Facebook and other sites that serve no og:image). If the opt-in
            // network unfurl later finds an og:image it supersedes this snapshot
            // (`on_unfurl`); a URL copied from a terminal/editor gets no snapshot
            // (just the glyph, or a miniature if unfurl resolves one). Best-effort
            // and synchronous on this worker thread (the render loop is untouched);
            // the focused window is still the copy source at this instant.
            if is_browser_source(class, title) {
                entry.preview_image = capture_window_snapshot(entry.hash);
            }
        }
    }
    Some(entry)
}

/// Snapshot the focused window to a side file via `grim`, for a link clip's
/// fallback hero image (used when no og:image is available). Best-effort: `None`
/// if the geometry is unavailable or `grim` fails / isn't installed.
fn capture_window_snapshot(hash: u64) -> Option<PathBuf> {
    let (x, y, w, h) = crate::hypr::active_window_geom()?;
    let path = crate::persist::data_path(&format!("clipboard-previews/{hash:016x}.png"));
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            warn!("clip preview: mkdir {}: {e}", dir.display());
            return None;
        }
    }
    let geom = format!("{x},{y} {w}x{h}");
    match Command::new("grim").arg("-g").arg(&geom).arg(&path).status() {
        Ok(s) if s.success() => Some(path),
        Ok(s) => {
            warn!("clip preview: grim exited {s}");
            None
        }
        Err(e) => {
            debug!("clip preview: grim unavailable: {e}");
            None
        }
    }
}

/// Whether a link was copied from a browser window — the only source whose live
/// snapshot depicts the page (so it makes a useful fallback hero). Matches the
/// window's app-class against known browser classes, then falls back to a known
/// browser title suffix (see [`BROWSER_SUFFIXES`]).
fn is_browser_source(class: &str, title: &str) -> bool {
    const BROWSER_CLASSES: &[&str] = &[
        "firefox", "chrome", "chromium", "brave", "edge", "vivaldi", "opera",
        "librewolf", "zen",
    ];
    let c = class.to_lowercase();
    if BROWSER_CLASSES.iter().any(|b| c.contains(b)) {
        return true;
    }
    let t = title.trim().to_lowercase();
    BROWSER_SUFFIXES.iter().any(|s| t.ends_with(s))
}

/// Browser chrome appended to tab titles, stripped from a seeded link title so
/// the heading reads as the page, not the browser. Lowercased for comparison.
const BROWSER_SUFFIXES: &[&str] = &[
    "mozilla firefox",
    "google chrome",
    "chromium",
    "brave",
    "microsoft edge",
    "vivaldi",
    "opera",
    "librewolf",
    "zen browser",
];

/// Tidy a browser tab title for use as a link title: trim, drop a trailing
/// " - <known browser>" / " — <known browser>" suffix, and cap the length. Only
/// *known* browser tails are stripped, so a page title that legitimately
/// contains " - " is left intact (the network unfurl later supplies a clean
/// `og:title` anyway).
fn clean_link_title(title: &str) -> String {
    let t = title.trim();
    let head = t
        .rsplit_once(" — ")
        .or_else(|| t.rsplit_once(" - "))
        .filter(|(head, tail)| {
            !head.trim().is_empty()
                && BROWSER_SUFFIXES.contains(&tail.trim().to_lowercase().as_str())
        })
        .map(|(head, _)| head.trim())
        .unwrap_or(t);
    cap(head, 80)
}

/// The "copied from" folder for a file clip: the parent directory of its first
/// path (falling back to the path itself if it has no parent). A file clip
/// always carries its own path, so — unlike a window source the compositor may
/// not report — this is always recoverable, even for clips restored from disk.
fn file_source(uri_list: &str) -> String {
    let Some(path) = first_file_path(uri_list) else {
        return String::new();
    };
    std::path::Path::new(&path)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or(path)
}

/// Best-effort "copied from" for a fresh capture: a file clip's source
/// directory, otherwise the focused window (app / title) at copy time.
fn capture_source(entry: &ClipEntry, win: Option<&(String, String)>) -> String {
    if entry.kind == ClipKind::Files {
        let dir = file_source(&entry.text);
        if !dir.is_empty() {
            return dir;
        }
    }
    match win {
        Some((class, title)) => window_label(class, title),
        None => String::new(),
    }
}

/// `"Class — Title"`, omitting an empty part; the title is trimmed to length.
fn window_label(class: &str, title: &str) -> String {
    let class = class.trim();
    let title = cap(title.trim(), 60);
    match (class.is_empty(), title.is_empty()) {
        (false, false) => format!("{class} — {title}"),
        (false, true) => class.to_owned(),
        (true, false) => title,
        (true, true) => String::new(),
    }
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

fn classify_files(uri_list: &str, cut: bool) -> Option<ClipEntry> {
    let names = file_names(uri_list);
    if names.is_empty() {
        return None;
    }
    Some(ClipEntry {
        // Fold the verb into the hash so copy vs cut of the same files are
        // distinct history entries (and dedup doesn't stick to a stale verb).
        hash: hash_bytes(format!("{}\n{uri_list}", verb(cut)).as_bytes()),
        preview: cap(&names.join(", "), PREVIEW_CAP),
        kind: ClipKind::Files,
        mime: "text/uri-list".into(),
        text: uri_list.to_owned(),
        cut,
        ..base_entry()
    })
}

/// The nixpkgs file-manager clipboard MIME that carries the copy/cut verb — the
/// type thunar (and nautilus/nemo/caja) actually paste files from.
const GNOME_COPIED: &str = "x-special/gnome-copied-files";

/// `"cut"` or `"copy"` — the verb word used by `x-special/gnome-copied-files`.
fn verb(cut: bool) -> &'static str {
    if cut {
        "cut"
    } else {
        "copy"
    }
}

/// Read the current file clip as `(cut, uri_list)`. Prefers
/// `x-special/gnome-copied-files` (which encodes the copy/cut verb); falls back
/// to a bare `text/uri-list` (always a copy).
fn read_file_clip(types: &[String]) -> (bool, String) {
    if types.iter().any(|t| t == GNOME_COPIED) {
        parse_gnome_copied(&paste_text(Some(GNOME_COPIED)))
    } else {
        (false, paste_text(Some("text/uri-list")))
    }
}

/// Parse an `x-special/gnome-copied-files` payload — a `copy`/`cut` verb line
/// followed by `file://` URIs — into `(cut, uri_list)`, where `uri_list` is the
/// newline-joined URIs (which [`file_names`]/[`file_offers`] then consume). A
/// missing/unknown verb line is treated as copy and kept as a URI.
fn parse_gnome_copied(raw: &str) -> (bool, String) {
    let lines: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let (cut, uris): (bool, &[&str]) = match lines.split_first() {
        Some((first, rest)) if first.eq_ignore_ascii_case("copy") => (false, rest),
        Some((first, rest)) if first.eq_ignore_ascii_case("cut") => (true, rest),
        _ => (false, &lines[..]),
    };
    (cut, uris.join("\n"))
}

/// The full set of `(mime, bytes)` a restored clip should advertise so it
/// pastes like a real Ctrl+V everywhere: file managers read
/// `x-special/gnome-copied-files`, GTK choosers/browsers read `text/uri-list`,
/// editors/terminals read `text/plain`.
fn clip_offers(entry: &ClipEntry) -> Vec<(String, Vec<u8>)> {
    match entry.kind {
        ClipKind::Text => text_offers(&entry.text),
        ClipKind::Files => file_offers(&entry.text, entry.cut),
        ClipKind::Image => match &entry.image_path {
            Some(path) => match std::fs::read(path) {
                Ok(bytes) => vec![(entry.mime.clone(), bytes)],
                Err(e) => {
                    warn!("clipboard: cannot read image {path:?}: {e}");
                    Vec::new()
                }
            },
            None => Vec::new(),
        },
    }
}

/// Text offered under its canonical type plus the legacy X11 aliases apps still
/// ask for (`UTF8_STRING`/`STRING`/`TEXT`) — all the same UTF-8 bytes.
fn text_offers(text: &str) -> Vec<(String, Vec<u8>)> {
    let bytes = text.as_bytes().to_vec();
    [
        "text/plain;charset=utf-8",
        "text/plain",
        "UTF8_STRING",
        "STRING",
        "TEXT",
    ]
    .iter()
    .map(|m| ((*m).to_owned(), bytes.clone()))
    .collect()
}

/// A file clip offered as `x-special/gnome-copied-files` (verb + URIs, the type
/// thunar pastes from), `text/uri-list` (CRLF-terminated URIs), and the decoded
/// local paths as `text/plain*` for editors/terminals.
fn file_offers(uri_list: &str, cut: bool) -> Vec<(String, Vec<u8>)> {
    let uris: Vec<String> = uri_list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect();
    if uris.is_empty() {
        return Vec::new();
    }
    let gnome = format!("{}\n{}", verb(cut), uris.join("\n"));
    let urilist = format!("{}\r\n", uris.join("\r\n"));
    let paths = uris
        .iter()
        .map(|u| percent_decode(u.strip_prefix("file://").unwrap_or(u)))
        .collect::<Vec<_>>()
        .join("\n");
    vec![
        (GNOME_COPIED.to_owned(), gnome.into_bytes()),
        ("text/uri-list".to_owned(), urilist.into_bytes()),
        ("text/plain;charset=utf-8".to_owned(), paths.clone().into_bytes()),
        ("text/plain".to_owned(), paths.clone().into_bytes()),
        ("UTF8_STRING".to_owned(), paths.into_bytes()),
    ]
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
        cut: false,
        image_path: None,
        width: 0,
        height: 0,
        hash: 0,
        source: String::new(),
        paste_count: 0,
        pastes: Vec::new(),
        title: String::new(),
        preview_image: None,
        description: String::new(),
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
    /// Hash of the clip we've handed our data-control source to own, so the
    /// newest content stays pasteable even after the app that produced it drops
    /// the selection. `None` until we've served one (avoids re-serving the same
    /// clip on our own capture re-read).
    served_hash: Option<u64>,
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
    // ---- copy-link affordance ----
    /// The focused window is a browser (has a copyable page URL), so the
    /// "copy link" pill should be out. Set from `refresh_options_content`.
    link_available: bool,
    /// Slide-out progress of the copy-link pill: 0 = tucked behind the small
    /// pill, 1 = fully out to its right.
    link_t: f32,
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
    // ---- metadata detail view ----
    /// The clip (by stable id) whose metadata detail is showing, if any. A
    /// right-click on a row opens it; the back button closes it.
    detail_id: Option<u64>,
    /// Intent: is the detail meant to be open? Separate from `detail_id`, which
    /// stays set through the whole close animation (so the shrink-back renders)
    /// and is cleared only once `detail_p` reaches 0.
    detail_open: bool,
    /// Open progress: 0 = list, 1 = detail. Drives the row-grows-into-card
    /// morph, the top pills, and the metadata sheet sliding up.
    detail_t: f32,
    /// Linear open progress (0..1) advanced at a constant rate; `detail_t` is its
    /// smoothstep, so the row-grow eases in and out instead of snapping.
    detail_p: f32,
    /// The clicked row's rect at open time — the content card grows from it.
    detail_src: Rect,
    /// Whether the clicked row was a zebra (striped) row, so the growing card
    /// keeps its exact tone (and the strip/metadata take the other tone).
    detail_src_striped: bool,
    /// Scroll offset (px) of the detail view's text content.
    detail_scroll: f32,
    /// Metadata sheet rise: 0 = resting (short), 1 = risen. Eased toward
    /// `detail_meta_target`, which the wheel drives (manual scroll-rise).
    detail_meta_t: f32,
    detail_meta_target: f32,
    /// Scroll offset (px) inside the metadata sheet once risen, eased to target.
    detail_meta_scroll: f32,
    detail_meta_scroll_target: f32,
    // ---- dictionary ("define a word") panel ----
    /// Intent: is the dictionary panel open? While set, the OPTIONS surface holds
    /// the keyboard and every key routes to `dict_query`.
    pub(crate) dict_open: bool,
    /// Open progress: 0 = list, 1 = dictionary panel (smoothstep of `dict_p`).
    dict_t: f32,
    /// Linear open progress advanced at a constant rate; `dict_t` is its
    /// smoothstep so the panel wipe eases in and out.
    dict_p: f32,
    /// The word being typed into the panel's search field.
    pub(crate) dict_query: String,
    /// Answer scroll: animated offset (px) and wheel target; reset to 0 whenever
    /// the query changes so a new lookup starts at the top.
    dict_scroll: f32,
    dict_scroll_target: f32,
    /// The resident word→definition map, loaded lazily on first open. `None`
    /// until the worker delivers it (or if the data file is missing/bad).
    dict_data: Option<crate::dict::Dict>,
    /// A load is in flight (so the panel shows "Loading…" and we don't re-spawn).
    dict_loading: bool,
    /// Reason the load failed (missing file / bad JSON), for the panel hint.
    dict_error: Option<String>,
    /// Network link-unfurl worker. `Some` only when `[options] link_unfurl` is
    /// enabled; `None` (the default) means links are never fetched.
    unfurl: Option<crate::unfurl::Unfurl>,
    /// Link clip ids with an unfurl already dispatched (dedup across recaptures).
    unfurl_sent: HashSet<u64>,
    // ---- thumbnails (image / file previews) ----
    /// Background thumbnailer (reuses the Files-section worker). `None` when the
    /// topbar is off.
    pub(crate) thumbs: Option<crate::thumbs::Thumbs>,
    /// Resolved thumbnail mip chains, appended to the OPTIONS icon array after
    /// the notif avatars; `icon_chains[slot]` is at renderer layer
    /// `notif_icon_chains.len() + slot`. Capped at [`THUMB_CAP`], recycled.
    pub(crate) icon_chains: Vec<Vec<u8>>,
    /// Thumbnail source path (hashed) → slot in `icon_chains`.
    icon_slot: HashMap<u64, u32>,
    /// slot → the key currently in it, for eviction on recycle.
    icon_key_at: Vec<u64>,
    /// Round-robin cursor for slot recycling once `icon_chains` is full.
    icon_next: usize,
    /// Source keys with a thumbnail request in flight (dedup).
    icon_pending: HashSet<u64>,
}

impl ClipState {
    pub fn new(
        handle: Option<ClipHandle>,
        thumbs: Option<crate::thumbs::Thumbs>,
        unfurl: Option<crate::unfurl::Unfurl>,
    ) -> Self {
        let mut history: Vec<ClipEntry> =
            crate::persist::read_json(&crate::persist::data_path(HISTORY_FILE)).unwrap_or_default();
        history.sort_by_key(|e| std::cmp::Reverse(e.timestamp_ms));
        history.truncate(MAX_HISTORY);
        // Backfill the source for file clips saved before it was derived (or
        // captured with the window unknown): a file clip's folder is always
        // recoverable from its own path, so no file clip should show "unknown".
        // Persist once if anything changed, so it's saved for good.
        let mut backfilled = false;
        for e in &mut history {
            if e.kind == ClipKind::Files && e.source.is_empty() {
                let dir = file_source(&e.text);
                if !dir.is_empty() {
                    e.source = dir;
                    backfilled = true;
                }
            }
        }
        if backfilled {
            crate::persist::write_json(
                "clipboard-history",
                &crate::persist::data_path(HISTORY_FILE),
                &history,
            );
        }
        let next_id = history.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        let row_heights = history.iter().map(row_height_of).collect();
        Self {
            handle,
            dict_open: false,
            dict_t: 0.0,
            dict_p: 0.0,
            dict_query: String::new(),
            dict_scroll: 0.0,
            dict_scroll_target: 0.0,
            dict_data: None,
            dict_loading: false,
            dict_error: None,
            unfurl,
            unfurl_sent: HashSet::new(),
            history,
            next_id,
            served_hash: None,
            detail_id: None,
            detail_open: false,
            detail_t: 0.0,
            detail_p: 0.0,
            detail_src: Rect::new(0.0, 0.0, 0.0, 0.0),
            detail_src_striped: false,
            detail_scroll: 0.0,
            detail_meta_t: 0.0,
            detail_meta_target: 0.0,
            detail_meta_scroll: 0.0,
            detail_meta_scroll_target: 0.0,
            peek_reveal: false,
            peek_t: 0.0,
            last: None,
            frame_pending: false,
            hold_deadline: None,
            blink_until: None,
            link_available: false,
            link_t: 0.0,
            expanded: false,
            expand_t: 0.0,
            box_h: 0.0,
            list_scroll: 0.0,
            scroll_target: 0.0,
            scroll_accum: 0.0,
            hover_row: None,
            hit: ClipHit::None,
            row_heights,
            thumbs,
            icon_chains: Vec::new(),
            icon_slot: HashMap::new(),
            icon_key_at: Vec::new(),
            icon_next: 0,
            icon_pending: HashSet::new(),
        }
    }
}

/// Remove a discarded entry's side files (the original image bytes and any link
/// preview/hero image), best-effort, so evicting/deleting a clip leaves nothing
/// behind on disk.
fn remove_clip_side_files(entry: &ClipEntry) {
    for p in [entry.image_path.as_ref(), entry.preview_image.as_ref()]
        .into_iter()
        .flatten()
    {
        let _ = std::fs::remove_file(p);
    }
}

/// The lines a clip shows in a row: the text word-wrapped to the row width `max_w`
/// and packed into up to [`MAX_ROW_LINES`] lines so a paragraph fills the card
/// instead of showing as one clipped line. Blank lines are dropped here (they're
/// honoured only in the metadata view and when the clip is copied). Files / images
/// show their single preview line.
fn clip_row_lines(entry: &ClipEntry, max_w: f32) -> Vec<String> {
    if entry.kind != ClipKind::Text {
        return vec![entry.preview.clone()];
    }
    let mut lines: Vec<String> = wrap_text(&entry.text, max_w, FONT_PX)
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.len() > MAX_ROW_LINES {
        lines.truncate(MAX_ROW_LINES);
        if let Some(last) = lines.last_mut() {
            last.push('…');
        }
    }
    if lines.is_empty() {
        lines.push(entry.preview.clone());
    }
    lines
}

/// Width available for a row's wrapped text: the expanded box width less the
/// horizontal padding, the leading tile (if any) and the trailing time column.
/// Shared by the height measure and the draw so their line-wrapping matches.
fn clip_text_col_w(has_tile: bool) -> f32 {
    let lead = ROW_PAD_X + if has_tile { TILE_SZ + TEXT_GAP } else { 0.0 };
    (PEEK_W - lead - ROW_PAD_X - TIME_COL_W).max(40.0)
}

/// The laid-out height of a clip's row: grows with its wrapped line count (up to
/// [`MAX_ROW_LINES`]), and is at least the tile height for image / files / link
/// rows. Short clips stay compact rather than padding out to a fixed height.
fn row_height_of(entry: &ClipEntry) -> f32 {
    let has_tile = !matches!(clip_tile(entry), ClipTile::None);
    let text_h = clip_row_lines(entry, clip_text_col_w(has_tile)).len() as f32 * LINE_PX;
    let tile_h = if has_tile { TILE_SZ } else { 0.0 };
    text_h.max(tile_h) + 2.0 * ROW_PAD_Y
}

impl App {
    /// A clipboard change arrived from the worker: fold it into the history
    /// (newest first, de-duplicated) and persist.
    pub(crate) fn on_clip_event(&mut self, ev: ClipEvent) {
        match ev {
            ClipEvent::Pasted => self.record_clip_paste(),
            ClipEvent::Captured(entry) => {
                let mut entry = *entry;
                // A re-copied clip moves to the top rather than piling up; it
                // keeps its original id (so any UI reference stays valid) and its
                // accumulated metadata (original source + paste log).
                if let Some(pos) = self.clip.history.iter().position(|e| e.hash == entry.hash) {
                    let old = self.clip.history.remove(pos);
                    entry.id = old.id;
                    if !old.source.is_empty() {
                        entry.source = old.source;
                    }
                    // Preserve link enrichment across a re-copy (a fresh capture
                    // only re-seeds the window title; the unfurled title/preview
                    // image would otherwise be lost, leaking its side file).
                    if entry.title.is_empty() {
                        entry.title = old.title;
                    }
                    if entry.preview_image.is_none() {
                        entry.preview_image = old.preview_image;
                    }
                    entry.paste_count = old.paste_count;
                    entry.pastes = old.pastes;
                } else {
                    entry.id = self.clip.next_id;
                    self.clip.next_id += 1;
                }
                self.clip.history.insert(0, entry);
                // Trim the tail, cleaning up any image side files it owned.
                while self.clip.history.len() > MAX_HISTORY {
                    if let Some(old) = self.clip.history.pop() {
                        remove_clip_side_files(&old);
                    }
                }
                self.measure_clip_rows();
                self.save_clip_history();
                // Own the freshly captured clip ourselves so it stays pasteable
                // after the app that produced it drops the selection (a plain
                // capture leaves us owning nothing → Ctrl+V finds an empty
                // clipboard). We advertise the full type set, so a file clip
                // still pastes into thunar & co.
                self.serve_newest_clip();
                // Keep the open box fitted to the new content.
                if self.clip.expanded {
                    self.clip.box_h = self.clip_full_h();
                    let span = self.clip_scroll_span();
                    self.clip.list_scroll = self.clip.list_scroll.min(span);
                    self.clip.scroll_target = self.clip.scroll_target.min(span);
                    self.update_clip_hit();
                }
                debug!("clipboard: captured; {} entries", self.clip.history.len());
                // Opt-in: kick off a network unfurl for a fresh link (clean
                // title + official og:image, replacing the window snapshot).
                let fresh_link = self
                    .clip
                    .history
                    .first()
                    .filter(|e| e.is_link())
                    .map(|e| (e.id, e.link_url().unwrap_or_default().to_owned()));
                if let Some((id, url)) = fresh_link {
                    self.request_clip_unfurl(id, &url);
                }
                // A fresh clip landed: beat the small pill (mirrors the bell's
                // muted-arrival blink) as a silent "captured" cue.
                self.trigger_clip_beat();
            }
        }
    }

    /// The source thread reported a paste of what it serves (the current clip =
    /// `history[0]`). Record where (the now-focused window) and when, and bump
    /// the count. Best-effort: only clips we serve are tracked (see
    /// [`ClipEvent::Pasted`]).
    fn record_clip_paste(&mut self) {
        let target = match crate::hypr::active_window_where() {
            Some((class, title)) => window_label(&class, &title),
            None => String::new(),
        };
        let Some(entry) = self.clip.history.first_mut() else {
            return;
        };
        entry.paste_count += 1;
        entry.pastes.insert(0, PasteRecord { target, when_ms: now_ms() });
        entry.pastes.truncate(PASTE_LOG_CAP);
        self.save_clip_history();
        if self.clip.expanded && self.clip.detail_id.is_some() {
            self.schedule_clip_frame();
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
    pub(crate) fn copy_clip(&mut self, idx: usize) {
        let Some(e) = self.clip.history.get(idx).cloned() else {
            return;
        };
        self.serve_clip(e);
    }

    /// Hand `entry` to our data-control source so it owns the selection with its
    /// full advertised type set, and remember it as the served clip.
    fn serve_clip(&mut self, entry: ClipEntry) {
        self.clip.served_hash = Some(entry.hash);
        if let Some(h) = &self.clip.handle {
            h.send(ClipCommand::Copy(Box::new(entry)));
        }
    }

    /// Keep the newest clip owned by our source so it stays pasteable even after
    /// the app that produced it releases the selection. Idempotent per clip (our
    /// own post-serve capture re-read hashes identically and is skipped). Also
    /// called once at startup: our source drops the selection when the daemon
    /// exits, so re-owning the newest on launch keeps it loaded across restarts.
    pub(crate) fn serve_newest_clip(&mut self) {
        let Some(entry) = self.clip.history.first() else {
            return;
        };
        if self.clip.served_hash == Some(entry.hash) {
            return;
        }
        let entry = entry.clone();
        self.serve_clip(entry);
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

    /// The two footer buttons — `[new note] [dictionary]` — a centred pair of
    /// equal circles with [`FOOTER_GAP`] between them, vertically centred in the
    /// footer zone. Shared by the draw and the hit-test.
    fn clip_footer_buttons(&self, rect: Rect) -> [(ClipHit, Rect); 3] {
        let f = self.clip_footer_rect(rect);
        let d = self.clip_footer_button_d();
        let total = 3.0 * d + 2.0 * FOOTER_GAP;
        let x0 = f.x + (f.w - total) / 2.0;
        let y = f.y + (f.h - d) / 2.0;
        let step = d + FOOTER_GAP;
        [
            (ClipHit::NewNote, Rect::new(x0, y, d, d)),
            (ClipHit::Dictionary, Rect::new(x0 + step, y, d, d)),
            (ClipHit::ClearAll, Rect::new(x0 + 2.0 * step, y, d, d)),
        ]
    }

    /// A history row's delete-can rect — ONE definition for the draw and the
    /// hit-test (they used to disagree in both axes: the drawn can sat
    /// top-aligned at `TRAIL_PAD_X` while the hit rect was vertically
    /// centred at `ROW_PAD_X`, so clicking the visible can on a tall row
    /// registered as a row click and *copied* instead of deleting — SH).
    /// Slightly outset for a forgiving click target.
    fn clip_row_can_rect(rr: Rect) -> Rect {
        let dr = Rect::new(
            rr.x + rr.w - TRAIL_PAD_X - DELETE_SZ,
            rr.y + ROW_PAD_Y,
            DELETE_SZ,
            DELETE_SZ,
        );
        Rect::new(dr.x - 3.0, dr.y - 3.0, dr.w + 6.0, dr.h + 6.0)
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
        // Opacity LEADS the height grow: `solid` is an ease-out of `e`, so the
        // panel goes opaque early and then finishes swelling out to full height.
        // Without this the fill fades in at the same rate it grows, which reads
        // as a translucent ghost inflating rather than a solid panel swelling.
        // `e` still drives all geometry (height, radius, the detail view); only
        // alpha/colour lerps use `solid`.
        let solid = 1.0 - (1.0 - e).powi(3);
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
        // The box is the pill grown: fill + ink both follow the bar's regime
        // (see `options_box_surface`), so the two never disagree.
        let (fill, box_ink) = self.options_box_surface();
        scene.rects.push(RectInst {
            rect,
            radius,
            // The PANEL is the only thing that goes translucent (frosted
            // glass); the pill it grows from keeps its own wash, and the
            // detail card / dict panel drawn later stay opaque.
            color: lerp4(
                pill_base,
                [fill[0], fill[1], fill[2], crate::options::BOX_ALPHA],
                solid,
            ),
            glass: 0.0,
        });

        let ink = lerp4(text_color, box_ink, solid);
        let dark_ink = ink[0] + ink[1] + ink[2] < 1.5;
        // The list rests at its darkest/strongest; the hovered row's text goes
        // LIGHTER (Max, 2026-08-31) — one clear direction, no row tinting.
        let hover_ink = crate::options::hover_ink_for(ink);
        let list_dim = if dark_ink { LIST_DIM_LIGHT } else { LIST_DIM };
        let dim_ink = [ink[0], ink[1], ink[2], ink[3] * list_dim];

        // Collapsed preview of the newest clip, fading out as the box solidifies
        // (complementary to the list fading in, so they cross-fade cleanly).
        let pa = ((peek - 0.35) / 0.5).clamp(0.0, 1.0) * (1.0 - solid);
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
            let a = [dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * solid];
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

        // Dictionary panel is a full-cover mode: it replaces the list/footer/
        // detail entirely (the renderer draws all labels in one late pass, so an
        // overlay rect can't mask row text — the rows must simply not be drawn).
        // It fades in over the box fill via `dict_t`.
        if self.clip.dict_open {
            self.push_clip_dict(scene, rect, e, ink, dim_ink, fill, bright);
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
            // Matches the panel: an opaque stripe over a translucent panel
            // blocks the compositor's blur on every other row.
            crate::options::BOX_ALPHA,
        ];

        // The content card grows out of the clicked row; the list is clipped to
        // the strip ABOVE the card (a hard partition, no cross-fade ghosting), so
        // the opaque card visibly *eats* the list as it grows.
        let dt = self.clip.detail_t;
        let (list_top, card_top) = if dt > 0.001 {
            let (content_full, _) = self.clip_detail_regions(rect);
            // The card LEADS (its top edge rides `detail_card_t`); the top strip
            // FOLLOWS (its lower edge rides the slower `detail_chrome_t`). The
            // list is clipped to the shrinking band between them, so the card
            // eats it from the bottom first and the strip sweeps the rest from
            // the top a beat later — still a hard partition, no plain-fill sliver.
            let card_top = lerp(self.clip.detail_src.y, content_full.y, self.detail_card_t());
            let list_top = rect.y + self.clip_detail_top_h() * self.detail_chrome_t();
            (list_top, card_top)
        } else {
            (content.y, content.y + content.h)
        };
        let list_clip = Rect::new(content.x, list_top, content.w, (card_top - list_top).max(0.0));
        if list_clip.h > 0.5 {
            for (idx, rr) in self.clip_rows(rect) {
                self.push_clip_row(
                    scene,
                    idx,
                    rr,
                    list_clip,
                    solid,
                    ink,
                    dim_ink,
                    hover_ink,
                    stripe_opaque,
                );
            }
        }
        if dt < 0.02 {
            self.push_clip_footer(scene, rect, solid, bright);
        }
        // Metadata detail view: the card keeps the clicked row's zebra tone; the
        // strip + metadata take the other tone.
        let (card_col, sheet_col) = if self.clip.detail_src_striped {
            (stripe_opaque, fill)
        } else {
            (fill, stripe_opaque)
        };
        self.push_clip_detail(scene, rect, e, ink, dim_ink, card_col, sheet_col, bright);
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

        // Hover marker: a hairline frame in the box's own ink, plus the ink
        // firming below. Nothing behind the text changes, so the frosted
        // blur reads through the row exactly as it does everywhere else.
        if hovered {
            crate::options::push_hover_frame(
                scene,
                Rect::new(rr.x, top, rr.w, bot - top),
                hover_ink,
                crate::options::HOVER_FRAME_ALPHA * e,
            );
        }
        let col = if hovered {
            [hover_ink[0], hover_ink[1], hover_ink[2], e]
        } else {
            [dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * e]
        };
        // Trailing time / delete can sit in the row's top-right corner.
        let ty = rr.y + ROW_PAD_Y;
        // Clip to the (possibly narrowed) content width so labels are masked at
        // the detail-wipe boundary rather than bleeding under the panel.
        let row_clip = Rect::new(content.x, top, content.w, bot - top);

        let tile = clip_tile(entry);

        // Trailing time (or the × delete on hover) at the right.
        let right = rr.x + rr.w - TRAIL_PAD_X;
        let text_right = if hovered {
            // Delete hot-square, top-right.
            let dr = Rect::new(
                rr.x + rr.w - TRAIL_PAD_X - DELETE_SZ,
                rr.y + ROW_PAD_Y,
                DELETE_SZ,
                DELETE_SZ,
            );
            debug_assert!(
                Self::clip_row_can_rect(rr).contains((dr.x + dr.w / 2.0, dr.y + dr.h / 2.0)),
                "drawn can must sit inside its hit rect"
            );
            let on_x = self.clip.hit == ClipHit::Delete(idx);
            // No red on the target — brighten to the hover ink instead. The list
            // can is also a touch larger than the detail/footer ones.
            let xc = if on_x { hover_ink } else { [ink[0], ink[1], ink[2], ink[3]] };
            scene.labels.push(Label {
                text: GLYPH_TRASH.to_owned(),
                pos: (dr.x + dr.w / 2.0, dr.y + (dr.h - LINE_PX) / 2.0),
                max_w: dr.w + 6.0,
                font_px: FONT_PX * 1.3,
                line_px: LINE_PX,
                centered: true,
                dim: false,
                cache: true,
                family: Some(NERD),
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

        // Left icon tile for image & files clips; text is inset past it.
        // Text-only clips start at the padding.
        let mut tx = rr.x + ROW_PAD_X;
        match tile {
            ClipTile::None => {}
            ClipTile::Glyph(glyph) => {
                let t = Rect::new(tx, rr.y + (rr.h - TILE_SZ) / 2.0, TILE_SZ, TILE_SZ);
                self.push_clip_tile_glyph(scene, t, glyph, ink, e, row_clip);
                tx += TILE_SZ + TEXT_GAP;
            }
            ClipTile::Thumb { key, glyph, .. } => {
                let t = Rect::new(tx, rr.y + (rr.h - TILE_SZ) / 2.0, TILE_SZ, TILE_SZ);
                if let Some(layer) = self.clip_thumb_layer(key) {
                    clip_grid(scene, content).icons.push(IconInst {
                        rect: t,
                        layer,
                        tint: [0.0; 4],
                        ring: -1.0,
                    });
                } else {
                    // Not resolved yet — a soft placeholder with the kind glyph.
                    self.push_clip_tile_glyph(scene, t, glyph, ink, e, row_clip);
                }
                tx += TILE_SZ + TEXT_GAP;
            }
        }

        // The clip text, vertically centred against the (possibly tall) tile.
        // Wrap to the shared column width (so line breaks match `row_height_of`),
        // but mask each label to the actual gap before the trailing time.
        let max_w = (text_right - tx).max(0.0);
        let lines = clip_row_lines(entry, clip_text_col_w(!matches!(tile, ClipTile::None)));
        let text_top = rr.y + (rr.h - lines.len() as f32 * LINE_PX) / 2.0;
        for (i, line) in lines.into_iter().enumerate() {
            scene.labels.push(Label {
                text: line,
                pos: (tx, text_top + i as f32 * LINE_PX),
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

    /// A soft rounded placeholder tile with a centred kind glyph (a directory /
    /// plain file, or an image thumbnail that hasn't resolved yet).
    fn push_clip_tile_glyph(
        &self,
        scene: &mut Scene,
        tile: Rect,
        glyph: &str,
        ink: [f32; 4],
        e: f32,
        clip: Rect,
    ) {
        scene.rects.push(RectInst {
            rect: tile,
            radius: 6.0,
            color: [ink[0], ink[1], ink[2], 0.10 * e],
            glass: 0.0,
        });
        let gpx = tile.w * 0.5;
        scene.labels.push(Label {
            text: glyph.to_owned(),
            pos: (tile.x + tile.w / 2.0, tile.y + (tile.h - gpx) / 2.0),
            max_w: tile.w + 6.0,
            font_px: gpx,
            line_px: gpx,
            centered: true,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some([ink[0], ink[1], ink[2], ink[3] * 0.7 * e]),
            clip: Some(clip),
        });
    }

    /// Draw the two footer buttons — `[new note] [dictionary]` — a centred pair
    /// of enlarged circles floating on the fill.
    fn push_clip_footer(&self, scene: &mut Scene, rect: Rect, alpha: f32, bright: bool) {
        let d0 = self.clip_footer_button_d();
        let gpx = d0 * 0.62;
        let g = self.options_text_color();
        for (hit, br0) in self.clip_footer_buttons(rect) {
            let hovered = self.clip.hit == hit;
            let br = if hovered { hover_grow(br0) } else { br0 };
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
            let glyph = match hit {
                ClipHit::Dictionary => GLYPH_BOOK,
                ClipHit::ClearAll => GLYPH_TRASH,
                _ => GLYPH_NOTE,
            };
            let gclip = Rect::new(br.x - 4.0, br.y - 4.0, br.w + 8.0, br.h + 8.0);
            scene.labels.push(Label {
                text: glyph.to_owned(),
                pos: (br.x + br.w / 2.0, br.y + (br.h - gpx) / 2.0),
                max_w: br.w + 16.0,
                font_px: gpx,
                line_px: gpx,
                centered: true,
                dim: false,
                cache: true,
                family: Some(NERD),
                color: Some([g[0], g[1], g[2], g[3] * alpha]),
                clip: Some(gclip),
            });
        }
    }

    /// Draw the dictionary "define a word" panel over the list: an opaque cover
    /// (wiping the rows/footer), a "‹ Back" button, the typed search field, and
    /// the resident definition — or a loading / not-installed / no-match hint.
    #[allow(clippy::too_many_arguments)]
    fn push_clip_dict(
        &self,
        scene: &mut Scene,
        rect: Rect,
        e: f32,
        ink: [f32; 4],
        dim_ink: [f32; 4],
        fill: [f32; 4],
        bright: bool,
    ) {
        let a = (self.clip.dict_t * e).clamp(0.0, 1.0);
        if a <= 0.001 {
            return;
        }
        // Opaque cover wiping the list/footer beneath (rounded like the box).
        scene.rects.push(RectInst {
            rect,
            radius: BOX_RADIUS,
            color: [fill[0], fill[1], fill[2], a],
            glass: 0.0,
        });

        // ---- "‹ Back" button (same seat / visuals as the detail view) ----
        let back = self.clip_dict_back_rect(rect);
        let hv = self.clip.hit == ClipHit::Back;
        let bcol = [ink[0], ink[1], ink[2], ink[3] * if hv { 1.0 } else { 0.72 } * a];
        let cy = back.y + (back.h - LINE_PX) / 2.0;
        let gclip = Rect::new(back.x - 4.0, back.y - 4.0, back.w + 8.0, back.h + 8.0);
        scene.labels.push(Label {
            text: GLYPH_BACK.to_owned(),
            pos: (back.x, cy),
            max_w: 20.0,
            font_px: FONT_PX * 0.92,
            line_px: LINE_PX,
            centered: false,
            dim: false,
            cache: true,
            family: Some(NERD),
            color: Some(bcol),
            clip: Some(gclip),
        });
        scene.labels.push(Label {
            text: "Back".to_owned(),
            pos: (back.x + 16.0, cy),
            max_w: back.w,
            font_px: FONT_PX * 0.95,
            line_px: LINE_PX,
            centered: false,
            dim: false,
            cache: true,
            family: None,
            color: Some(bcol),
            clip: Some(gclip),
        });

        // ---- typed search field (a taller stadium input under the back row) ----
        let (field, res) = self.clip_dict_layout(rect);
        push_neumorph(scene, field, field.h / 2.0, bright, a);
        let mut wash_c = self.options_rest_wash();
        wash_c[3] *= a;
        scene.rects.push(RectInst {
            rect: field,
            radius: field.h / 2.0,
            color: wash_c,
            glass: 0.0,
        });
        let field_font = FONT_PX * 1.12;
        let tx = field.x + PILL_PAD_X + 4.0;
        let ty = field.y + (field.h - LINE_PX) / 2.0;
        let fclip = Rect::new(field.x, field.y, field.w, field.h);
        let empty = self.clip.dict_query.is_empty();
        let (text, col) = if empty {
            ("Type a word…".to_owned(), dim_ink)
        } else {
            (self.clip.dict_query.clone(), ink)
        };
        scene.labels.push(Label {
            text,
            pos: (tx, ty),
            max_w: field.w - 2.0 * PILL_PAD_X,
            font_px: field_font,
            line_px: LINE_PX,
            centered: false,
            dim: false,
            cache: empty,
            family: None,
            color: Some([col[0], col[1], col[2], col[3] * a]),
            clip: Some(fclip),
        });
        // Caret: a thin bar after the estimated query width (no live measure on a
        // `&self` draw — the half-em estimate matches the row-time width guess).
        let cw = self.clip.dict_query.chars().count() as f32 * field_font * 0.5;
        let caret_x = (tx + cw).min(field.x + field.w - PILL_PAD_X);
        scene.rects.push(RectInst {
            rect: Rect::new(caret_x, field.y + field.h * 0.26, 2.0, field.h * 0.48),
            radius: 1.0,
            color: [ink[0], ink[1], ink[2], ink[3] * a * 0.8],
            glass: 0.0,
        });

        // ---- answer: scrollable per-language definitions, or a one-line hint ----
        let lines = self.dict_answer_lines(res.w);
        if lines.is_empty() {
            let query = self.clip.dict_query.trim();
            if query.is_empty() {
                return;
            }
            let msg = if self.clip.dict_data.is_some() {
                format!("No definition for “{query}”.")
            } else if self.clip.dict_loading {
                "Loading dictionary…".to_owned()
            } else {
                "Dictionary data not installed.".to_owned()
            };
            scene.labels.push(Label {
                text: msg,
                pos: (res.x, res.y + 2.0),
                max_w: res.w,
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: false,
                dim: false,
                cache: true,
                family: None,
                color: Some([dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * a]),
                clip: Some(res),
            });
            return;
        }
        // Draw the answer clipped to `res`, offset by the (eased) scroll. Only
        // lines whose band intersects the area are pushed.
        let mut y = res.y - self.clip.dict_scroll;
        for line in lines {
            if !line.text.is_empty() && y + line.advance > res.y && y < res.y + res.h {
                let c = match line.kind {
                    DictLineKind::Lang => [ink[0], ink[1], ink[2], ink[3] * 0.85 * a],
                    DictLineKind::Etym => [dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * 0.8 * a],
                    DictLineKind::Body => [dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * a],
                };
                scene.labels.push(Label {
                    text: line.text,
                    pos: (res.x, y),
                    max_w: res.w,
                    font_px: line.font_px,
                    line_px: LINE_PX,
                    centered: false,
                    dim: false,
                    cache: false,
                    family: None,
                    color: Some(c),
                    clip: Some(res),
                });
            }
            y += line.advance;
        }
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
        // Dictionary panel: only the "‹ Back" button is hittable; typing drives
        // the rest, and the list is hidden behind the panel.
        if self.clip.dict_open {
            if self.clip_dict_back_rect(rect).contains(p) {
                return ClipHit::Back;
            }
            return ClipHit::None;
        }
        // Detail view: only the top pills are hittable; the list is hidden.
        if self.clip.detail_open {
            for (hit, prect) in self.clip_detail_pills(rect) {
                if prect.contains(p) {
                    return hit;
                }
            }
            return ClipHit::None;
        }
        if self.clip_footer_rect(rect).contains(p) {
            for (hit, br) in self.clip_footer_buttons(rect) {
                if br.contains(p) {
                    return hit;
                }
            }
            return ClipHit::None;
        }
        for (idx, rr) in self.clip_rows(rect) {
            if rr.contains(p) {
                // Same rect the can is DRAWN at (top-aligned, TRAIL_PAD_X)
                // plus click slack — see `clip_row_can_rect`.
                if Self::clip_row_can_rect(rr).contains(p) {
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
            // In the dictionary panel, the wheel scrolls the answer.
            if self.clip.dict_open {
                let span = self.clip_dict_scroll_span();
                self.clip.dict_scroll_target =
                    (self.clip.dict_scroll_target + delta * SCROLL_SPEED).clamp(0.0, span);
                self.schedule_clip_frame();
                return;
            }
            // In the detail view, the wheel scrolls whichever region the pointer
            // is over: the risen metadata sheet, else the text content.
            if self.clip.detail_open {
                if self.clip_detail_meta_hovered() {
                    // Manual scroll-rise: raise the sheet first, then scroll its
                    // log in place (targets; `tick_clip` eases toward them).
                    let d = delta * SCROLL_SPEED;
                    let rise_px = 170.0;
                    if d > 0.0 {
                        let room = (1.0 - self.clip.detail_meta_target) * rise_px;
                        let grow = d.min(room);
                        self.clip.detail_meta_target += grow / rise_px;
                        let rest = d - grow;
                        if rest > 0.0 {
                            self.clip.detail_meta_scroll_target += rest;
                        }
                    } else {
                        let back = (-d).min(self.clip.detail_meta_scroll_target);
                        self.clip.detail_meta_scroll_target -= back;
                        let rest = -d - back;
                        if rest > 0.0 {
                            self.clip.detail_meta_target -= rest / rise_px;
                        }
                    }
                    self.clip.detail_meta_target = self.clip.detail_meta_target.clamp(0.0, 1.0);
                    let span = self.clip_detail_meta_scroll_span();
                    self.clip.detail_meta_scroll_target =
                        self.clip.detail_meta_scroll_target.clamp(0.0, span);
                } else {
                    let span = self.clip_detail_scroll_span();
                    self.clip.detail_scroll =
                        (self.clip.detail_scroll + delta * SCROLL_SPEED).clamp(0.0, span);
                }
                self.schedule_clip_frame();
                return;
            }
            let span = self.clip_scroll_span();
            self.clip.scroll_target =
                (self.clip.scroll_target + delta * SCROLL_SPEED).clamp(0.0, span);
            self.clip.scroll_accum = 0.0;
            self.request_clip_thumbs();
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
    pub(crate) fn open_clip_box(&mut self) {
        if self.clip.history.is_empty() {
            return;
        }
        self.clip.hold_deadline = None;
        self.clip.peek_reveal = true; // keep the preview fully out under the box
        self.clip.expanded = true;
        self.clip.box_h = self.clip_full_h();
        // Every open is a fresh box: list at the top, no detail view carried over
        // from a previous session. Done here (box still hidden) rather than on
        // close, so a collapse-while-in-detail can shrink away smoothly instead
        // of snapping to the list, and so it holds no matter which path collapsed
        // the box (the explicit close or the pointer-leave timer).
        self.clip.list_scroll = 0.0;
        self.clip.scroll_target = 0.0;
        self.clip.scroll_accum = 0.0;
        self.clip.detail_id = None;
        self.clip.detail_open = false;
        self.clip.detail_t = 0.0;
        self.clip.detail_p = 0.0;
        self.clip.detail_meta_t = 0.0;
        self.clip.detail_meta_target = 0.0;
        self.clip.detail_meta_scroll = 0.0;
        self.clip.detail_meta_scroll_target = 0.0;
        // A fresh box never carries a dictionary panel over (the loaded word list
        // is kept — only the open state and query reset).
        self.clip.dict_open = false;
        self.clip.dict_t = 0.0;
        self.clip.dict_p = 0.0;
        self.clip.dict_query.clear();
        self.clip.dict_scroll = 0.0;
        self.clip.dict_scroll_target = 0.0;
        self.sync_options_input();
        self.reeval_options_bar();
        self.request_clip_thumbs();
        self.schedule_clip_frame();
    }

    /// Collapse the open history drawer back to the pill.
    pub(crate) fn close_clip_box(&mut self) {
        if self.clip.expanded {
            self.clip.expanded = false;
            self.clip.hit = ClipHit::None;
            self.clip.hover_row = None;
            // Release the keyboard grab if the dictionary panel held it (it fades
            // away with the collapsing box; `open_clip_box` clears the rest).
            if self.clip.dict_open {
                self.clip.dict_open = false;
                if let Some(layer) = &self.options_layer {
                    crate::surface::set_interactive(layer, false);
                }
            }
            // The detail view (and scroll) are NOT reset here: the box collapses
            // showing whatever was on screen (the detail shrinks + fades away with
            // it), and `open_clip_box` wipes it back to a fresh list on the next
            // open — so there's no snap-to-list mid-collapse.
            self.sync_options_input();
            self.schedule_clip_frame();
        }
    }

    /// Open the metadata detail for history row `idx` (a right-click on a row):
    /// the clicked row grows into the content card, the pills drop in on top,
    /// and the metadata sheet slides up from the bottom.
    pub(crate) fn open_clip_detail(&mut self, idx: usize) {
        let Some(entry) = self.clip.history.get(idx) else {
            return;
        };
        let id = entry.id;
        // Capture the clicked row's rect so the content card can grow from it.
        let rect = self.clip_rect();
        let src = self
            .clip_rows(rect)
            .into_iter()
            .find(|(i, _)| *i == idx)
            .map(|(_, rr)| rr)
            .unwrap_or_else(|| self.clip_detail_regions(rect).0);
        self.clip.detail_id = Some(id);
        self.clip.detail_open = true;
        self.clip.detail_src = src;
        self.clip.detail_src_striped = idx % 2 == 1; // matches the zebra in push_clip_row
        self.clip.detail_scroll = 0.0;
        self.clip.detail_meta_t = 0.0;
        self.clip.detail_meta_target = 0.0;
        self.clip.detail_meta_scroll = 0.0;
        self.clip.detail_meta_scroll_target = 0.0;
        self.clip.hover_row = None;
        self.clip.hit = ClipHit::None;
        // Make sure the link hero's snapshot thumbnail is requested even if the
        // row was never on screen long enough to trigger it.
        self.request_clip_thumbs();
        self.schedule_clip_frame();
    }

    /// Close the detail view, sliding back to the list (the back button).
    fn close_clip_detail(&mut self) {
        // Keep `detail_id` set so the shrink-back animation can still render;
        // `tick_clip` clears it once the morph finishes.
        self.clip.detail_open = false;
        self.clip.detail_meta_target = 0.0;
        self.clip.detail_meta_scroll_target = 0.0;
        self.update_clip_hit();
        self.schedule_clip_frame();
    }

    /// The history entry the detail view is showing, if any.
    fn clip_detail_entry(&self) -> Option<&ClipEntry> {
        let id = self.clip.detail_id?;
        self.clip.history.iter().find(|e| e.id == id)
    }

    /// Whether a copied link's `host` belongs to an installed webapp — so "Open"
    /// routes it to that webapp rather than the plain browser. Hosts compared
    /// www-normalized and suffix-tolerant (a `youtube.com` webapp covers
    /// `www.youtube.com` / `music.youtube.com` links).
    fn link_has_webapp(&self, host: &str) -> bool {
        let norm = |h: &str| h.strip_prefix("www.").unwrap_or(h).to_owned();
        let h = norm(host);
        self.entries.iter().any(|e| {
            e.id.starts_with("webapp-")
                && crate::webapps::exec_app_host(&e.exec).is_some_and(|wh| {
                    let wh = norm(wh);
                    h == wh || h.ends_with(&format!(".{wh}")) || wh.ends_with(&format!(".{h}"))
                })
        })
    }

    /// The dictionary panel's "‹ Back" button rect — same seat as the detail
    /// view's back button (top-left, aligned with the pill row).
    fn clip_dict_back_rect(&self, rect: Rect) -> Rect {
        let d = self.clip_band_h();
        Rect::new(rect.x + DETAIL_PILL_X, rect.y + DETAIL_PILL_Y, DETAIL_BACK_W, d)
    }

    /// The dictionary panel's geometry: `(search field, answer area)`. Shared by
    /// the draw and the scroll-span so line breaks and bounds always agree.
    fn clip_dict_layout(&self, rect: Rect) -> (Rect, Rect) {
        let back = self.clip_dict_back_rect(rect);
        let field = Rect::new(
            rect.x + ROW_PAD_X,
            back.y + back.h + DETAIL_PILL_GAP + 6.0,
            rect.w - 2.0 * ROW_PAD_X,
            DICT_FIELD_H,
        );
        let res_top = field.y + field.h + 14.0;
        let res = Rect::new(
            rect.x + ROW_PAD_X,
            res_top,
            rect.w - 2.0 * ROW_PAD_X,
            (rect.y + rect.h - res_top - LIST_PAD).max(0.0),
        );
        (field, res)
    }

    /// The laid-out answer for the current query: a language subheading + wrapped
    /// definition lines per language that defines the word (a word in both shows
    /// both). Empty when there's no query, no data, or no match — those states
    /// draw a one-line hint instead. Shared by the draw and the scroll span.
    fn dict_answer_lines(&self, res_w: f32) -> Vec<DictLine> {
        let mut out = Vec::new();
        let query = self.clip.dict_query.trim();
        if query.is_empty() {
            return out;
        }
        let Some(dict) = self.clip.dict_data.as_ref() else {
            return out;
        };
        for (i, entry) in dict.lookup(query).into_iter().enumerate() {
            if i > 0 {
                out.push(DictLine::spacer(LINE_PX * 0.6));
            }
            out.push(DictLine {
                text: entry.lang.to_owned(),
                font_px: FONT_PX * 0.82,
                advance: LINE_PX * 1.15,
                kind: DictLineKind::Lang,
            });
            // Etymology (faint, in parentheses) between the label and the senses.
            if let Some(etym) = entry.etymology {
                for line in wrap_text(&format!("({etym})"), res_w, FONT_PX * 0.9) {
                    out.push(DictLine {
                        text: line,
                        font_px: FONT_PX * 0.9,
                        advance: LINE_PX * 0.95,
                        kind: DictLineKind::Etym,
                    });
                }
            }
            for line in wrap_text(entry.definition, res_w, FONT_PX) {
                out.push(DictLine {
                    text: line,
                    font_px: FONT_PX,
                    advance: LINE_PX,
                    kind: DictLineKind::Body,
                });
            }
        }
        out
    }

    /// Max scroll (px) of the dictionary answer — its total height past the
    /// visible answer area (0 for short answers / hints).
    fn clip_dict_scroll_span(&self) -> f32 {
        let rect = self.clip_rect();
        let (_, res) = self.clip_dict_layout(rect);
        let total: f32 = self.dict_answer_lines(res.w).iter().map(|l| l.advance).sum();
        (total - res.h).max(0.0)
    }

    /// Open the dictionary "define a word" panel: grab the keyboard on the
    /// OPTIONS surface so the search field can type, and kick a lazy load of the
    /// offline word list the first time.
    pub(crate) fn open_dict(&mut self) {
        self.clip.dict_open = true;
        self.clip.dict_query.clear();
        self.clip.dict_scroll = 0.0;
        self.clip.dict_scroll_target = 0.0;
        if self.clip.dict_data.is_none() && !self.clip.dict_loading {
            if let Some(tx) = &self.dict_tx {
                self.clip.dict_loading = true;
                crate::dict::spawn_load(tx.clone());
            }
        }
        if let Some(layer) = &self.options_layer {
            crate::surface::set_interactive(layer, true);
        }
        self.update_clip_hit();
        self.schedule_clip_frame();
    }

    /// Close the dictionary panel and release the keyboard grab.
    pub(crate) fn close_dict(&mut self) {
        self.clip.dict_open = false;
        if let Some(layer) = &self.options_layer {
            crate::surface::set_interactive(layer, false);
        }
        self.update_clip_hit();
        self.schedule_clip_frame();
    }

    /// Fold a finished dictionary load into the panel (or record why it failed).
    pub(crate) fn on_dict_loaded(&mut self, ev: crate::dict::Event) {
        let crate::dict::Event::Loaded(result) = ev;
        match result {
            Ok(d) => {
                self.clip.dict_data = Some(d);
                self.clip.dict_error = None;
            }
            Err(e) => {
                warn!("dict: {e}");
                self.clip.dict_error = Some(e);
            }
        }
        self.clip.dict_loading = false;
        self.schedule_clip_frame();
    }

    /// Handle one key while the dictionary panel owns the keyboard: Escape backs
    /// out, Backspace edits the query, printable characters extend it; the lookup
    /// itself is recomputed live at draw against the resident map.
    pub(crate) fn dict_key(&mut self, keysym: Keysym, utf8: Option<&str>) {
        match keysym {
            Keysym::Escape => {
                self.close_dict();
                return;
            }
            Keysym::BackSpace => {
                self.clip.dict_query.pop();
            }
            _ => {
                // Accept a single printable grapheme (letters, hyphen, apostrophe
                // — anything a headword may contain); ignore control keys, Enter,
                // navigation, and modifiers-only presses.
                if let Some(s) = utf8 {
                    if !s.is_empty() && !s.chars().any(|c| c.is_control()) {
                        self.clip.dict_query.push_str(s);
                    }
                }
            }
        }
        // A new lookup starts at the top of its answer.
        self.clip.dict_scroll = 0.0;
        self.clip.dict_scroll_target = 0.0;
        self.schedule_clip_frame();
    }

    /// Height of the top pill-row zone: tight to the pill height (the pills sit
    /// flush at the top, aligned with the paste pill; the strip is just a hair
    /// taller so it reads as the pill row, not a big band).
    fn clip_detail_top_h(&self) -> f32 {
        self.clip_band_h() + 2.0 * PILL_MARGIN_Y
    }

    /// Staggered detail-open progress for the content card — it LEADS, growing
    /// out of the clicked row first and settling early.
    fn detail_card_t(&self) -> f32 {
        stagger(self.clip.detail_p, 0.0, 0.70)
    }

    /// Staggered detail-open progress for the chrome that FOLLOWS the card: the
    /// top pill strip wiping down and the metadata sheet rising, both a beat
    /// behind so the view cascades open rather than snapping in all at once.
    fn detail_chrome_t(&self) -> f32 {
        stagger(self.clip.detail_p, 0.16, 0.92)
    }

    /// The settled content and metadata regions of the detail view within box
    /// `rect`: the top pill row, then the content card, then the metadata sheet
    /// (down to the box bottom). The sheet rests short and rises with
    /// `detail_meta_t`, shrinking the content.
    fn clip_detail_regions(&self, rect: Rect) -> (Rect, Rect) {
        let top_h = self.clip_detail_top_h();
        let inner_h = (rect.h - top_h).max(0.0);
        let rest = (rect.h * META_REST_FRAC)
            .clamp(META_REST_MIN, META_REST_MAX)
            .min(inner_h);
        // The cap can dip below `rest` while the box is mid-collapse (small
        // `rect.h`); keep it ≥ rest so the clamp below never panics.
        let cap = (rect.h * META_FULL_FRAC).min(inner_h).max(rest);
        let full = self.clip_detail_meta_natural().clamp(rest, cap);
        let meta_h = lerp(rest, full, self.clip.detail_meta_t).min(inner_h);
        let content_h = (inner_h - meta_h).max(0.0);
        let top = rect.y + top_h;
        let content = Rect::new(rect.x, top, rect.w, content_h);
        let meta = Rect::new(rect.x, top + content_h, rect.w, meta_h);
        (content, meta)
    }

    /// The natural (fully-shown) height of the metadata sheet content: the
    /// Source/Copied/Pasted rows plus one row per logged paste, and a leading
    /// Title row for an enriched link clip.
    fn clip_detail_meta_natural(&self) -> f32 {
        let entry = self.clip_detail_entry();
        let logs = entry.map(|e| e.pastes.len()).unwrap_or(0);
        // Title + About (link only), Source, Copied, Pasted-header, then a row
        // per paste.
        let title = entry.is_some_and(|e| e.is_link() && !e.title.is_empty());
        let about = entry.is_some_and(|e| e.is_link() && !e.description.is_empty());
        let base = 3 + usize::from(title) + usize::from(about);
        META_INNER_TOP + (base + logs) as f32 * META_ROW_H + META_INNER_BOT
    }

    /// Max scroll (px) inside the metadata sheet once risen (0 if it all fits).
    fn clip_detail_meta_scroll_span(&self) -> f32 {
        let rect = self.clip_rect();
        let (_, meta) = self.clip_detail_regions(rect);
        (self.clip_detail_meta_natural() - meta.h).max(0.0)
    }

    /// Whether the pointer is over the metadata sheet (so it should rise).
    fn clip_detail_meta_hovered(&self) -> bool {
        if self.clip.detail_id.is_none() {
            return false;
        }
        let Some(p) = self.options_ptr else {
            return false;
        };
        let rect = self.clip_rect();
        let (_, meta) = self.clip_detail_regions(rect);
        p.0 >= rect.x && p.0 <= rect.x + rect.w && p.1 >= meta.y && p.1 <= rect.y + rect.h
    }

    /// The top-row hit targets: a "‹ Back" text button pinned to the left, and
    /// the action pills clustered at the right — `[copy] [more] [×]`, with an
    /// extra `[open]` after copy for link clips (`[copy] [open] [more] [×]`).
    /// Pill diameter/top match the paste pill so all read as one row.
    fn clip_detail_pills(&self, rect: Rect) -> Vec<(ClipHit, Rect)> {
        let d = self.clip_band_h();
        let y = rect.y + DETAIL_PILL_Y;
        // Left: the text back button.
        let mut out = vec![(ClipHit::Back, Rect::new(rect.x + DETAIL_PILL_X, y, DETAIL_BACK_W, d))];
        // Right cluster, in left→right order.
        let mut cluster = vec![ClipHit::DetailCopy];
        if self.clip_detail_entry().is_some_and(|e| e.is_link()) {
            cluster.push(ClipHit::DetailOpen);
        }
        cluster.push(ClipHit::DetailMore);
        cluster.push(ClipHit::DetailDelete);
        // Right-align the cluster: the last item sits flush at the right edge.
        let rx = rect.x + rect.w - DETAIL_PILL_X;
        let n = cluster.len();
        for (i, hit) in cluster.into_iter().enumerate() {
            let j = (n - 1 - i) as f32; // 0 = rightmost
            out.push((hit, Rect::new(rx - (j + 1.0) * d - j * DETAIL_PILL_GAP, y, d, d)));
        }
        out
    }

    /// Max scroll (px) of the detail text (0 for non-text or short clips).
    fn clip_detail_scroll_span(&self) -> f32 {
        let Some(entry) = self.clip_detail_entry() else {
            return 0.0;
        };
        if entry.kind != ClipKind::Text {
            return 0.0;
        }
        let rect = self.clip_rect();
        let (content, _) = self.clip_detail_regions(rect);
        let max_w = (content.w - 2.0 * DETAIL_TEXT_MX).max(1.0);
        let text_h = wrap_text(&entry.text, max_w, FONT_PX).len() as f32 * LINE_PX;
        let view_h = (content.h - 2.0 * DETAIL_TEXT_MY).max(0.0);
        (text_h - view_h).max(0.0)
    }

    /// Draw the metadata detail view as it opens over the list (`detail_t`):
    /// the clicked row grows into the content card, the top pills fade in, and
    /// the metadata sheet slides up from the bottom edge. The sheet rises further
    /// with `detail_meta_t`; a text clip's content scrolls with `detail_scroll`.
    #[allow(clippy::too_many_arguments)]
    fn push_clip_detail(
        &self,
        scene: &mut Scene,
        rect: Rect,
        e: f32,
        ink: [f32; 4],
        dim_ink: [f32; 4],
        card_col: [f32; 4],
        sheet_col: [f32; 4],
        bright: bool,
    ) {
        let dt = self.clip.detail_t;
        if dt <= 0.001 || e <= 0.01 {
            return;
        }
        // Staggered sub-progresses: the card leads, the top strip + metadata sheet
        // follow a beat behind (see `detail_card_t` / `detail_chrome_t`).
        let card_t = self.detail_card_t();
        let chrome_t = self.detail_chrome_t();
        let Some(entry) = self.clip_detail_entry() else {
            return;
        };
        // Solid content — no fade; the card genuinely morphs, it doesn't dissolve.
        let a = e;
        let inkc = [ink[0], ink[1], ink[2], ink[3] * a];
        // Two opaque tones: `card_col` is the clicked row's zebra tone; `sheet_col`
        // is the other tone, for the top strip + metadata sheet.
        let card_c = [card_col[0], card_col[1], card_col[2], e];
        let sheet_c = [sheet_col[0], sheet_col[1], sheet_col[2], e];
        let (content_full, meta_full) = self.clip_detail_regions(rect);

        // ---- content card: grows from the clicked row into place ----
        let src = self.clip.detail_src;
        let card = Rect::new(
            lerp(src.x, content_full.x, card_t),
            lerp(src.y, content_full.y, card_t),
            lerp(src.w, content_full.w, card_t),
            lerp(src.h, content_full.h, card_t),
        );
        // Opaque backing — the card keeps the clicked row's zebra tone;
        // a solid panel that grows, not a fade.
        scene.rects.push(RectInst {
            rect: card,
            radius: 0.0,
            color: card_c,
            glass: 0.0,
        });

        // A link whose window snapshot has resolved shows it as a hero image;
        // otherwise (a plain text clip, or a link still awaiting/without a
        // snapshot) the card shows scrollable text — the link's URL as a
        // readable fallback.
        let link_hero = entry.is_link().then(|| match clip_tile(entry) {
            ClipTile::Thumb { key, .. } => self.clip_thumb_layer(key),
            _ => None,
        });
        if let Some(Some(layer)) = link_hero {
            // Square, aspect-kept (the snapshot letterboxes onto a transparent
            // square via the thumb pipeline); centred in the card.
            let side = (card.h - 2.0 * DETAIL_TEXT_MY)
                .min(card.w - 2.0 * DETAIL_TEXT_MX)
                .clamp(32.0, 360.0);
            let tile = Rect::new(
                card.x + (card.w - side) / 2.0,
                card.y + (card.h - side) / 2.0,
                side,
                side,
            );
            clip_grid(scene, card).icons.push(IconInst {
                rect: tile,
                layer,
                tint: [0.0; 4],
                ring: -1.0,
            });
        } else if entry.kind == ClipKind::Text {
            let tx = card.x + DETAIL_TEXT_MX;
            let max_w = (card.w - 2.0 * DETAIL_TEXT_MX).max(1.0);
            let view = Rect::new(
                card.x,
                card.y + DETAIL_TEXT_MY,
                card.w,
                (card.h - 2.0 * DETAIL_TEXT_MY).max(0.0),
            );
            let top = view.y - self.clip.detail_scroll;
            for (i, line) in wrap_text(&entry.text, max_w, FONT_PX).iter().enumerate() {
                if line.is_empty() {
                    continue;
                }
                let ly = top + i as f32 * LINE_PX;
                if ly + LINE_PX < view.y || ly > view.y + view.h {
                    continue;
                }
                scene.labels.push(Label {
                    text: line.clone(),
                    pos: (tx, ly),
                    max_w,
                    font_px: FONT_PX,
                    line_px: LINE_PX,
                    centered: false,
                    dim: false,
                    cache: false,
                    family: None,
                    color: Some(inkc),
                    clip: Some(view),
                });
            }
        } else {
            let side = (card.h - 2.0 * DETAIL_TEXT_MY)
                .min(card.w - 2.0 * DETAIL_TEXT_MX)
                .clamp(32.0, 300.0);
            let tile = Rect::new(
                card.x + (card.w - side) / 2.0,
                card.y + (card.h - side) / 2.0,
                side,
                side,
            );
            match clip_tile(entry) {
                ClipTile::Glyph(g) => self.push_clip_tile_glyph(scene, tile, g, ink, a, card),
                ClipTile::Thumb { key, glyph, .. } => {
                    if let Some(layer) = self.clip_thumb_layer(key) {
                        clip_grid(scene, card).icons.push(IconInst {
                            rect: tile,
                            layer,
                            tint: [0.0; 4],
                            ring: -1.0,
                        });
                    } else {
                        self.push_clip_tile_glyph(scene, tile, glyph, ink, a, card);
                    }
                }
                ClipTile::None => {}
            }
        }

        // ---- metadata sheet: slides up from the bottom edge (the other tone) ----
        let vis_h = meta_full.h * chrome_t;
        let mtop = rect.y + rect.h - vis_h;
        let mclip = Rect::new(rect.x, mtop, rect.w, vis_h);
        push_bottom_rounded(scene, mclip, BOX_RADIUS, sheet_c);
        scene.rects.push(RectInst {
            rect: Rect::new(rect.x, mtop, rect.w, 1.0),
            radius: 0.0,
            color: [ink[0], ink[1], ink[2], 0.12 * e],
            glass: 0.0,
        });

        let mx = rect.x + DETAIL_TEXT_MX;
        let vx = mx + META_LOG_INDENT;
        let mright = rect.x + rect.w - ROW_PAD_X;
        let mbot = rect.y + rect.h;
        let inkm = [ink[0], ink[1], ink[2], ink[3] * e];
        let dimm = [dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * e];
        let row = |scene: &mut Scene, y: f32, label: &str, value: String| {
            if value.is_empty() || y + META_ROW_H <= mtop || y >= mbot {
                return;
            }
            scene.labels.push(Label {
                text: label.to_owned(),
                pos: (mx, y),
                max_w: META_LABEL_W,
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: false,
                dim: false,
                cache: true,
                family: None,
                color: Some(dimm),
                clip: Some(mclip),
            });
            scene.labels.push(Label {
                text: value,
                pos: (vx, y),
                max_w: (mright - vx).max(0.0),
                font_px: FONT_PX,
                line_px: LINE_PX,
                centered: false,
                dim: false,
                cache: false,
                family: None,
                color: Some(inkm),
                clip: Some(mclip),
            });
        };
        let mut y = mtop + META_INNER_TOP - self.clip.detail_meta_scroll;
        // Title row (link clips only): the page title, seeded from the source
        // window at copy time and refined by the network unfurl when enabled.
        if entry.is_link() && !entry.title.is_empty() {
            row(scene, y, "Title:", entry.title.clone());
            y += META_ROW_H;
        }
        // About row (link only): the unfurled page description, one clipped line.
        if entry.is_link() && !entry.description.is_empty() {
            row(scene, y, "About:", entry.description.clone());
            y += META_ROW_H;
        }
        // Always show the Source row — the window couldn't always be identified
        // at copy time (empty workspace, a client with no class/title, a missed
        // query), and an empty value would silently drop the row while its slot
        // still advances, leaving a blank gap. Mirror the paste log's "somewhere".
        let source = if entry.source.is_empty() {
            "unknown".to_owned()
        } else {
            entry.source.clone()
        };
        row(scene, y, "Source:", source);
        y += META_ROW_H;
        let copied = {
            let b = fmt_datetime(entry.timestamp_ms);
            if entry.kind == ClipKind::Files && entry.cut {
                format!("{b} · cut")
            } else {
                b
            }
        };
        row(scene, y, "Copied:", copied);
        y += META_ROW_H;
        let pasted = match entry.paste_count {
            0 => "not yet".to_owned(),
            1 => "1 time".to_owned(),
            n => format!("{n} times"),
        };
        row(scene, y, "Pasted:", pasted);
        y += META_ROW_H;
        for rec in &entry.pastes {
            if y + META_ROW_H > mtop && y < mbot {
                let w = if rec.target.is_empty() {
                    "somewhere".to_owned()
                } else {
                    cap(&rec.target, 40)
                };
                let t = fmt_datetime(rec.when_ms);
                let tw = t.chars().count() as f32 * FONT_PX * 0.5;
                scene.labels.push(Label {
                    text: w,
                    pos: (vx, y),
                    max_w: (mright - vx - tw - 12.0).max(0.0),
                    font_px: FONT_PX * 0.95,
                    line_px: LINE_PX,
                    centered: false,
                    dim: false,
                    cache: false,
                    family: None,
                    color: Some(dimm),
                    clip: Some(mclip),
                });
                scene.labels.push(Label {
                    text: t,
                    pos: (mright - tw, y),
                    max_w: tw + 8.0,
                    font_px: FONT_PX * 0.9,
                    line_px: LINE_PX,
                    centered: false,
                    dim: false,
                    cache: false,
                    family: None,
                    color: Some(dimm),
                    clip: Some(mclip),
                });
            }
            y += META_ROW_H;
        }

        // ---- top pill strip: a solid band (the other tone) that WIPES DOWN
        // from the box's top edge as the detail opens (the "from-above" flow),
        // matching the mockup. Its rounded top corners stay pinned to the box;
        // its lower edge — and the divider that rides it — descend with the
        // trailing `chrome_t`, a beat behind the card. ----
        let top_h = self.clip_detail_top_h();
        let band_h = top_h * chrome_t;
        // The top strip takes the *content card* tone (not the sheet tone) so it
        // reads as one surface with the card behind it rather than a contrasting
        // band across the top.
        push_top_rounded(
            scene,
            Rect::new(rect.x, rect.y, rect.w, band_h),
            BOX_RADIUS,
            card_c,
        );

        // ---- top pills: seat in over the last few px of the wipe (once the
        // descending band fully covers them), so they arrive WITH the strip
        // instead of fading onto an already-open bar. ----
        let seat = ((band_h - self.clip_band_h()) / (2.0 * PILL_MARGIN_Y)).clamp(0.0, 1.0);
        let pa = seat * seat * (3.0 - 2.0 * seat) * e;
        if pa > 0.01 {
            let ink0 = self.options_text_color();
            for (hit, prect) in self.clip_detail_pills(rect) {
                let hv = self.clip.hit == hit;
                // The left control is a plain "‹ Back" text button, not a pill:
                // brighter on hover, muted at rest, left-aligned in its hit area.
                // The chevron is the Nerd `fa-chevron-left` glyph (fills its em box,
                // so it doesn't read tiny next to the word) drawn as its own label.
                if hit == ClipHit::Back {
                    let a = if hv { 1.0 } else { 0.72 };
                    let col = [ink0[0], ink0[1], ink0[2], ink0[3] * a * pa];
                    let cy = prect.y + (prect.h - LINE_PX) / 2.0;
                    let gclip = Rect::new(prect.x - 4.0, prect.y - 4.0, prect.w + 8.0, prect.h + 8.0);
                    scene.labels.push(Label {
                        text: GLYPH_BACK.to_owned(),
                        pos: (prect.x, cy),
                        max_w: 20.0,
                        font_px: FONT_PX * 0.92,
                        line_px: LINE_PX,
                        centered: false,
                        dim: false,
                        cache: true,
                        family: Some(NERD),
                        color: Some(col),
                        clip: Some(gclip),
                    });
                    scene.labels.push(Label {
                        text: "Back".to_owned(),
                        pos: (prect.x + 16.0, cy),
                        max_w: prect.w,
                        font_px: FONT_PX * 0.95,
                        line_px: LINE_PX,
                        centered: false,
                        dim: false,
                        cache: true,
                        family: None,
                        color: Some(col),
                        clip: Some(gclip),
                    });
                    continue;
                }
                let pr = if hv { hover_grow(prect) } else { prect };
                let radius = pr.h / 2.0;
                push_neumorph(scene, pr, radius, bright, pa);
                // Contrast against the strip: recess the pill (darker on a dark
                // bar, lighter on a light one), brighten on hover.
                let base = if hv {
                    self.options_hover_wash()
                } else if bright {
                    [1.0, 1.0, 1.0, 0.18]
                } else {
                    [0.0, 0.0, 0.0, 0.22]
                };
                scene.rects.push(RectInst {
                    rect: pr,
                    radius,
                    color: [base[0], base[1], base[2], base[3] * pa],
                    glass: 0.0,
                });
                let (glyph, gcol) = match hit {
                    ClipHit::DetailDelete => (GLYPH_TRASH, ink0),
                    ClipHit::DetailMore => (GLYPH_MORE, ink0),
                    ClipHit::DetailOpen => (GLYPH_OPEN, ink0),
                    _ => (GLYPH_COPY, ink0),
                };
                let gclip = Rect::new(pr.x - 4.0, pr.y - 4.0, pr.w + 8.0, pr.h + 8.0);
                scene.labels.push(Label {
                    text: glyph.to_owned(),
                    pos: (pr.x + pr.w / 2.0, pr.y + (pr.h - LINE_PX) / 2.0),
                    max_w: pr.w + 6.0,
                    font_px: FONT_PX * 0.92,
                    line_px: LINE_PX,
                    centered: true,
                    dim: false,
                    cache: true,
                    family: Some(NERD),
                    color: Some([gcol[0], gcol[1], gcol[2], gcol[3] * pa]),
                    clip: Some(gclip),
                });
            }
        }
    }

    /// Handle a click inside the open box (footer / row delete / row). Returns
    /// whether it consumed the click.
    pub(crate) fn clip_box_click(&mut self) -> bool {
        match self.clip.hit {
            ClipHit::NewNote => {
                // TODO: open the floating centred note editor.
                debug!("clip: new-note button (editor not yet wired)");
                true
            }
            ClipHit::Dictionary => {
                self.open_dict();
                true
            }
            ClipHit::ClearAll => {
                self.clear_all_clips();
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
            ClipHit::Back => {
                if self.clip.dict_open {
                    self.close_dict();
                } else {
                    self.close_clip_detail();
                }
                true
            }
            ClipHit::DetailCopy => {
                if let Some(idx) = self
                    .clip
                    .detail_id
                    .and_then(|id| self.clip.history.iter().position(|e| e.id == id))
                {
                    self.copy_clip(idx);
                }
                self.close_clip_box();
                true
            }
            ClipHit::DetailDelete => {
                self.delete_detail_clip();
                true
            }
            ClipHit::DetailOpen => {
                // Open the link where it belongs, then dismiss the picker. If its
                // host matches an installed webapp, open it *as that webapp*
                // (frameless, shared profile — logged in, uBlock, copy-link), the
                // way notifications route to their webapp. Otherwise fall back to
                // the default browser.
                let url = self
                    .clip_detail_entry()
                    .filter(|e| e.is_link())
                    .map(|e| e.text.trim().to_owned());
                if let Some(url) = url {
                    let as_webapp = crate::webapps::url_host(&url)
                        .is_some_and(|h| self.link_has_webapp(h));
                    let exec = if as_webapp {
                        crate::webapps::app_open_exec(&url)
                    } else {
                        format!("xdg-open {}", crate::launch::shell_quote(&url))
                    };
                    if let Err(e) = crate::launch::launch(&exec, false, "") {
                        warn!("clip: open link failed: {e}");
                    } else if !as_webapp {
                        // Raise the browser so the opened tab comes to the front
                        // (the app-mode window focuses itself; the browser doesn't).
                        crate::hypr::focus_browser();
                    }
                }
                self.close_clip_box();
                true
            }
            // Placeholder pill — reserved for a later action.
            ClipHit::DetailMore => true,
            ClipHit::None => false,
        }
    }

    /// Delete the clip the detail view is showing, then return to the list.
    fn delete_detail_clip(&mut self) {
        let idx = self
            .clip
            .detail_id
            .and_then(|id| self.clip.history.iter().position(|e| e.id == id));
        self.close_clip_detail();
        if let Some(idx) = idx {
            self.delete_clip(idx);
        }
    }

    /// Right-click inside the open box: open the hovered row's metadata detail.
    /// Returns whether it consumed the click.
    pub(crate) fn clip_box_right_click(&mut self) -> bool {
        if let ClipHit::Row(i) = self.clip.hit {
            self.open_clip_detail(i);
            true
        } else {
            false
        }
    }

    /// Delete one clip from the history (its × control).
    /// The footer can: wipe the whole history (side files included) and
    /// close the box — the clipboard's own contents stay served.
    fn clear_all_clips(&mut self) {
        debug!("clip: clear all ({} entries)", self.clip.history.len());
        for entry in std::mem::take(&mut self.clip.history) {
            remove_clip_side_files(&entry);
        }
        self.measure_clip_rows();
        self.save_clip_history();
        self.close_clip_box();
        self.update_clip_hit();
        self.schedule_clip_frame();
    }

    fn delete_clip(&mut self, idx: usize) {
        if idx >= self.clip.history.len() {
            return;
        }
        let entry = self.clip.history.remove(idx);
        remove_clip_side_files(&entry);
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

    /// The OPTIONS renderer layer of a resolved clip thumbnail (clipboard
    /// thumbnails follow the notif avatars in the shared array), or `None`.
    fn clip_thumb_layer(&self, key: u64) -> Option<u32> {
        self.clip
            .icon_slot
            .get(&key)
            .map(|s| self.notif_icon_chains.len() as u32 + s)
    }

    /// A thumbnail arrived from the worker: cache it in a (recycling) slot and
    /// re-upload the shared OPTIONS icon array.
    pub(crate) fn on_clip_thumb(&mut self, ev: crate::thumbs::Event) {
        let crate::thumbs::Event::Thumb { path, pixels } = ev else {
            return; // AudioOnly etc. — no thumbnail
        };
        let key = hash_bytes(path.as_bytes());
        self.clip.icon_pending.remove(&key);
        if self.clip.icon_slot.contains_key(&key) {
            return;
        }
        let slot = if self.clip.icon_chains.len() < THUMB_CAP {
            let s = self.clip.icon_chains.len();
            self.clip.icon_chains.push(pixels);
            self.clip.icon_key_at.push(key);
            s
        } else {
            let s = self.clip.icon_next % THUMB_CAP;
            self.clip.icon_next = self.clip.icon_next.wrapping_add(1);
            let old = self.clip.icon_key_at[s];
            self.clip.icon_slot.remove(&old);
            self.clip.icon_chains[s] = pixels;
            self.clip.icon_key_at[s] = key;
            s
        };
        self.clip.icon_slot.insert(key, slot as u32);
        self.upload_options_icons();
        self.draw_options();
    }

    /// Dispatch a network unfurl for link clip `id` (opt-in; no-op when the
    /// worker is absent). Deduped so a re-copy doesn't refetch.
    fn request_clip_unfurl(&mut self, id: u64, url: &str) {
        if url.is_empty() || self.clip.unfurl_sent.contains(&id) {
            return;
        }
        if let Some(u) = &self.clip.unfurl {
            u.request(id, url);
            self.clip.unfurl_sent.insert(id);
        }
    }

    /// A network unfurl resolved: fold its title/description/image into the clip
    /// (the cleaner `og:image` supersedes the window snapshot). Applied by id, so
    /// an evicted clip's late result is discarded (and its image cleaned up).
    pub(crate) fn on_unfurl(&mut self, ev: crate::unfurl::Event) {
        let crate::unfurl::Event::Done {
            id,
            title,
            description,
            image_path,
        } = ev;
        let Some(pos) = self.clip.history.iter().position(|e| e.id == id) else {
            if let Some(p) = image_path {
                let _ = std::fs::remove_file(p);
            }
            return;
        };
        {
            let entry = &mut self.clip.history[pos];
            if !title.is_empty() {
                entry.title = cap(&title, 120);
            }
            if !description.is_empty() {
                entry.description = cap(&description, 300);
            }
            if let Some(new_img) = image_path {
                if let Some(old) = entry.preview_image.replace(new_img) {
                    let _ = std::fs::remove_file(old);
                }
            }
        }
        self.measure_clip_rows();
        self.save_clip_history();
        self.request_clip_thumbs();
        self.schedule_clip_frame();
    }

    /// Request thumbnails for the currently-visible rows that still need one.
    fn request_clip_thumbs(&mut self) {
        if self.clip.thumbs.is_none() {
            return;
        }
        let rect = self.clip_rect();
        let reqs: Vec<(String, u64)> = self
            .clip_rows(rect)
            .into_iter()
            .filter_map(|(idx, _)| {
                let entry = self.clip.history.get(idx)?;
                match clip_tile(entry) {
                    ClipTile::Thumb { path, key, .. }
                        if !self.clip.icon_slot.contains_key(&key)
                            && !self.clip.icon_pending.contains(&key) =>
                    {
                        Some((path, key))
                    }
                    _ => None,
                }
            })
            .collect();
        for (path, key) in reqs {
            self.clip.icon_pending.insert(key);
            if let Some(t) = &self.clip.thumbs {
                t.request(&path);
            }
        }
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
        } else if self.clip.peek_reveal && self.clip.hold_deadline.is_none() && !self.clip.dict_open
        {
            // The dictionary panel holds the box open regardless of the pointer —
            // the user has to leave the box to reach the keyboard to type.
            self.schedule_clip_collapse(LEAVE_HOLD);
        }
    }

    /// Set whether the focused app has a copyable link (a browser is up), sliding
    /// the copy-link pill in/out. Called from `refresh_options_content` on focus
    /// changes; only kicks a frame when the state actually flips.
    pub(crate) fn set_clip_link_available(&mut self, available: bool) {
        if self.clip.link_available != available {
            self.clip.link_available = available;
            self.clip.last = None;
            self.schedule_clip_frame();
        }
    }

    /// Slide-out progress of the copy-link pill (0 hidden → 1 out), for the layout.
    pub(crate) fn clip_link_t(&self) -> f32 {
        self.clip.link_t
    }

    /// Draw the copy-link pill, fading + sliding in with `link_t`. Emerges from
    /// behind the small clipboard pill, which draws on top.
    pub(crate) fn push_clip_link(&self, scene: &mut Scene, rect: Rect, glyph: &str) {
        // Fade in a touch after the slide begins, so it reads as coming out from
        // under the small pill rather than blinking on.
        let a = ((self.clip.link_t - 0.15) / 0.6).clamp(0.0, 1.0);
        if a <= 0.01 {
            return;
        }
        let bright = self.options_bar_is_bright();
        let hovered = self.options_hover == Some(PillId::ClipCopyLink);
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

    /// Copy the focused browser's current URL to the clipboard — the copy-link
    /// pill's action. The copied URL flows back through the watcher, landing in
    /// the history as an (enriched) link clip.
    ///
    /// - **Full browser** (address bar): inject Ctrl+L (focus + select the URL),
    ///   then Ctrl+C, then Escape, spaced so the browser processes one before the
    ///   next.
    /// - **App-mode webapp** (no address bar): read the live URL off-thread from
    ///   the shared webapp Chrome's DevTools endpoint and put it on the clipboard.
    pub(crate) fn copy_active_link(&mut self) {
        let (class, title) = crate::hypr::active_window_where().unwrap_or_default();
        if crate::webapps::is_app_window(&class) {
            std::thread::spawn(move || {
                match crate::webapps::active_app_url(&class, &title) {
                    Some(url) => wl_copy("text/plain;charset=utf-8", url.as_bytes()),
                    None => warn!("copy-link: no URL for webapp {class} (debug port up?)"),
                }
            });
            return;
        }
        crate::hypr::send_shortcut_active("CTRL", "l");
        self.after_ms(90, |_| crate::hypr::send_shortcut_active("CTRL", "c"));
        self.after_ms(200, |_| crate::hypr::send_shortcut_active("", "escape"));
    }

    /// Run `f` on the event loop once, after `ms` milliseconds (one-shot timer).
    fn after_ms(&self, ms: u64, f: impl FnOnce(&mut App) + 'static) {
        let timer = Timer::from_duration(Duration::from_millis(ms));
        let mut f = Some(f);
        let _ = self.loop_handle.insert_source(timer, move |_, _, app: &mut App| {
            if let Some(f) = f.take() {
                f(app);
            }
            TimeoutAction::Drop
        });
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
                    if !app.clip.dict_open
                        && !matches!(
                            app.options_hover,
                            Some(PillId::Clipboard | PillId::ClipboardBox)
                        )
                    {
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
        // Slide the copy-link pill toward its target (out when a browser is up).
        let ltarget = if self.clip.link_available { 1.0 } else { 0.0 };
        let (lt, lmoving) = ease_toward(self.clip.link_t, ltarget, dt, MORPH_RATE, MORPH_EPS);
        self.clip.link_t = lt;
        // Grow / collapse the history drawer.
        let etarget = if self.clip.expanded { 1.0 } else { 0.0 };
        let (et, em) = ease_toward(self.clip.expand_t, etarget, dt, MORPH_RATE, MORPH_EPS);
        self.clip.expand_t = et;
        // Open/close the detail: advance a LINEAR progress at a constant rate,
        // then smoothstep it into `detail_t` so the row-grow eases in *and* out
        // (constant-rate ease_toward snapped at the start).
        let ptarget = if self.clip.detail_open { 1.0 } else { 0.0 };
        let dm = self.clip.detail_p != ptarget;
        if dm {
            let step = dt / DETAIL_OPEN_SECS;
            self.clip.detail_p = if ptarget > self.clip.detail_p {
                (self.clip.detail_p + step).min(1.0)
            } else {
                (self.clip.detail_p - step).max(0.0)
            };
        }
        let p = self.clip.detail_p;
        self.clip.detail_t = p * p * (3.0 - 2.0 * p); // smoothstep
        // The close morph is done: drop the entry reference.
        if !self.clip.detail_open && self.clip.detail_p <= 0.0 {
            self.clip.detail_id = None;
        }
        // Dictionary panel wipe: same constant-rate linear progress smoothstepped
        // into `dict_t`, so it eases in and out over the list.
        let dicttarget = if self.clip.dict_open { 1.0 } else { 0.0 };
        let dictm = self.clip.dict_p != dicttarget;
        if dictm {
            let step = dt / DETAIL_OPEN_SECS;
            self.clip.dict_p = if dicttarget > self.clip.dict_p {
                (self.clip.dict_p + step).min(1.0)
            } else {
                (self.clip.dict_p - step).max(0.0)
            };
        }
        let dp = self.clip.dict_p;
        self.clip.dict_t = dp * dp * (3.0 - 2.0 * dp);
        // Ease the metadata rise + its internal scroll toward the wheel targets.
        let (nmt, mm1) = ease_toward(
            self.clip.detail_meta_t,
            self.clip.detail_meta_target,
            dt,
            MORPH_RATE,
            MORPH_EPS,
        );
        self.clip.detail_meta_t = nmt;
        let (nms, mm2) = ease_toward(
            self.clip.detail_meta_scroll,
            self.clip.detail_meta_scroll_target,
            dt,
            SCROLL_RATE,
            0.5,
        );
        self.clip.detail_meta_scroll = nms;
        let mm = mm1 || mm2;
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
        // Smooth dictionary-answer scrolling.
        let (ds, dsm) = ease_toward(
            self.clip.dict_scroll,
            self.clip.dict_scroll_target,
            dt,
            SCROLL_RATE,
            0.5,
        );
        self.clip.dict_scroll = ds;
        // Keep the beat pulsing until its deadline, then clear it.
        let beating = self.clip.blink_until.is_some_and(|u| now < u);
        if !beating {
            self.clip.blink_until = None;
        }
        self.draw_options();
        if moving || lmoving || em || bm || lm || dm || dictm || dsm || mm || beating {
            self.schedule_clip_frame();
        } else {
            self.clip.last = None;
        }
    }
}

/// One laid-out line of the dictionary answer (a language subheading, a wrapped
/// definition line, or a blank spacer between languages).
enum DictLineKind {
    /// A language label ("English" / "Español"), drawn brighter as a subheading.
    Lang,
    /// The etymology ("Del lat. cor"), drawn faint and small under the label.
    Etym,
    /// A definition line, drawn in the dimmer body ink.
    Body,
}

/// A dictionary answer line with its own type size + vertical advance, so the
/// answer can be measured (for the scroll span) and drawn identically.
struct DictLine {
    text: String,
    font_px: f32,
    advance: f32,
    kind: DictLineKind,
}

impl DictLine {
    /// A blank vertical gap (no text) of `advance` px.
    fn spacer(advance: f32) -> Self {
        DictLine {
            text: String::new(),
            font_px: FONT_PX,
            advance,
            kind: DictLineKind::Body,
        }
    }
}

/// What a clip row shows at its left: nothing (text), a glyph tile (a directory
/// or a non-previewable file), or a thumbnail (image / previewable file) with a
/// glyph placeholder until it resolves.
enum ClipTile {
    None,
    Glyph(&'static str),
    Thumb {
        path: String,
        key: u64,
        glyph: &'static str,
    },
}

/// Classify a clip's left tile (see [`ClipTile`]). `key` is the source path's
/// hash — the thumbnail cache key.
fn clip_tile(entry: &ClipEntry) -> ClipTile {
    match entry.kind {
        // A link clip shows its window snapshot as a thumbnail once captured; a
        // link glyph stands in until then (and if the snapshot failed).
        ClipKind::Text if entry.is_link() => match &entry.preview_image {
            Some(p) => {
                let path = p.to_string_lossy().into_owned();
                let key = hash_bytes(path.as_bytes());
                ClipTile::Thumb {
                    path,
                    key,
                    glyph: GLYPH_LINK,
                }
            }
            None => ClipTile::Glyph(GLYPH_LINK),
        },
        ClipKind::Text => ClipTile::None,
        ClipKind::Image => match &entry.image_path {
            Some(p) => {
                let path = p.to_string_lossy().into_owned();
                let key = hash_bytes(path.as_bytes());
                ClipTile::Thumb {
                    path,
                    key,
                    glyph: GLYPH_IMAGE,
                }
            }
            None => ClipTile::Glyph(GLYPH_IMAGE),
        },
        ClipKind::Files => {
            let Some(path) = first_file_path(&entry.text) else {
                return ClipTile::Glyph(GLYPH_FILES);
            };
            if std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
                ClipTile::Glyph(GLYPH_FOLDER)
            } else if crate::thumbs::thumbable(&path) {
                let key = hash_bytes(path.as_bytes());
                ClipTile::Thumb {
                    path,
                    key,
                    glyph: GLYPH_FILES,
                }
            } else {
                ClipTile::Glyph(GLYPH_FILES)
            }
        }
    }
}

/// The first real path in a `text/uri-list` (percent-decoded `file://`).
fn first_file_path(uri_list: &str) -> Option<String> {
    uri_list
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| percent_decode(l.strip_prefix("file://").unwrap_or(l)))
}

/// The box's content-scissored grid the row thumbnails ride, so they clip to the
/// list interior (mirrors the notif box's grid). Reuses the last grid only if
/// it's already ours (same clip rect), else pushes a fresh one — so an open
/// notif box's grid (a different rect) can't clip the clip thumbnails.
fn clip_grid(scene: &mut Scene, content: Rect) -> &mut GridContent {
    if scene.grids.last().map(|g| g.clip) != Some(content) {
        scene.grids.push(GridContent {
            clip: content,
            ..Default::default()
        });
    }
    scene.grids.last_mut().expect("grid just ensured")
}

/// Local day-number (days since the epoch in *local* time) and clock hour/minute
/// for a unix-ms stamp — via `tm_gmtoff` so the day boundary is local midnight.
fn local_day_hm(ms: u64) -> (i64, i32, i32) {
    // SAFETY: `localtime_r` fills a caller-owned `tm`.
    unsafe {
        let t = (ms / 1000) as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let local = t as i64 + tm.tm_gmtoff as i64;
        (local.div_euclid(86_400), tm.tm_hour, tm.tm_min)
    }
}

/// The time label for a clip row: same-day items show the clock time (`14:32`),
/// older items a compact day count (`1d`/`2d`) — a full date would be too much.
fn fmt_relative(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let (day, hh, mm) = local_day_hm(ms);
    let (today, ..) = local_day_hm(now_ms());
    let diff = today - day;
    if diff <= 0 {
        format!("{hh:02}:{mm:02}")
    } else {
        format!("{diff}d")
    }
}

/// Word-wrap `text` to a pixel width, honouring existing newlines. Uses an
/// average-glyph-width estimate (the proportional UI font isn't measurable
/// here); the label's own `max_w` clips any line that estimates slightly long,
/// so wrapping is never wider than the region.
fn wrap_text(text: &str, max_w: f32, font_px: f32) -> Vec<String> {
    let max_chars = ((max_w / (font_px * 0.5)).floor() as usize).max(8);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let line = raw.trim_end();
        if line.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for word in line.split_whitespace() {
            let joined = if cur.is_empty() {
                word.chars().count()
            } else {
                cur.chars().count() + 1 + word.chars().count()
            };
            if !cur.is_empty() && joined > max_chars {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        out.push(cur);
    }
    out
}

/// Local absolute date + time for a unix-millis timestamp, e.g. `"Aug 18 · 14:32"`.
fn fmt_datetime(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    const MON: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // SAFETY: `localtime_r` fills a caller-owned `tm`.
    unsafe {
        let t = (ms / 1000) as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let mon = MON.get(tm.tm_mon as usize).copied().unwrap_or("");
        format!(
            "{mon} {} · {:02}:{:02}",
            tm.tm_mday, tm.tm_hour, tm.tm_min
        )
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

/// Draw an OPAQUE band with only its TOP corners rounded (the bottom is squared
/// off by a second rect). Colour must be opaque — the two rects overlap, so a
/// translucent colour would double up.
fn push_top_rounded(scene: &mut Scene, r: Rect, radius: f32, color: [f32; 4]) {
    scene.rects.push(RectInst {
        rect: r,
        radius,
        color,
        glass: 0.0,
    });
    let h = r.h - radius;
    if h > 0.0 {
        scene.rects.push(RectInst {
            rect: Rect::new(r.x, r.y + radius, r.w, h),
            radius: 0.0,
            color,
            glass: 0.0,
        });
    }
}

/// Draw an OPAQUE band with only its BOTTOM corners rounded (see
/// [`push_top_rounded`]).
fn push_bottom_rounded(scene: &mut Scene, r: Rect, radius: f32, color: [f32; 4]) {
    scene.rects.push(RectInst {
        rect: r,
        radius,
        color,
        glass: 0.0,
    });
    let h = r.h - radius;
    if h > 0.0 {
        scene.rects.push(RectInst {
            rect: Rect::new(r.x, r.y, r.w, h),
            radius: 0.0,
            color,
            glass: 0.0,
        });
    }
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
    fn detects_single_bare_urls_only() {
        assert_eq!(
            detect_url("https://youtu.be/LlPrMjAjEzY"),
            Some("https://youtu.be/LlPrMjAjEzY")
        );
        assert_eq!(
            detect_url("  http://example.com/a?b=1#c  "),
            Some("http://example.com/a?b=1#c")
        );
        // Not links: prose containing a URL, non-http schemes, or a bare scheme.
        assert_eq!(detect_url("see https://example.com now"), None);
        assert_eq!(detect_url("ftp://example.com"), None);
        assert_eq!(detect_url("https://localhost"), None); // no dot in host
        assert_eq!(detect_url("just text"), None);
    }

    #[test]
    fn link_url_only_for_text_clips() {
        let link = classify_text("https://example.com/x", "text/plain".into()).unwrap();
        assert!(link.is_link());
        let files = classify_files("file:///home/max/a.txt\n", false).unwrap();
        assert!(!files.is_link());
    }

    #[test]
    fn strips_only_known_browser_suffixes() {
        assert_eq!(clean_link_title("Rickroll - YouTube"), "Rickroll - YouTube"); // "YouTube" not a browser
        assert_eq!(
            clean_link_title("My Page — Mozilla Firefox"),
            "My Page"
        );
        assert_eq!(clean_link_title("Docs - Google Chrome"), "Docs");
        // A legitimate " - " in the title with an unknown tail is left intact.
        assert_eq!(clean_link_title("Fixes - a retrospective"), "Fixes - a retrospective");
    }

    #[test]
    fn files_show_basenames() {
        let list = "file:///home/max/a%20b.txt\nfile:///home/max/photos/pic.png\n";
        let e = classify_files(list, false).unwrap();
        assert_eq!(e.kind, ClipKind::Files);
        assert_eq!(e.preview, "a b.txt, pic.png");
        assert_eq!(e.mime, "text/uri-list");
        assert!(!e.cut);
    }

    #[test]
    fn empty_uri_list_is_dropped() {
        assert!(classify_files("# comment only\n", false).is_none());
    }

    #[test]
    fn copy_and_cut_of_same_files_are_distinct() {
        let list = "file:///home/max/a.txt\n";
        let copy = classify_files(list, false).unwrap();
        let cut = classify_files(list, true).unwrap();
        assert!(cut.cut && !copy.cut);
        assert_ne!(copy.hash, cut.hash, "verb must affect dedup hash");
    }

    #[test]
    fn parses_gnome_copied_verb_and_uris() {
        let (cut, uris) =
            parse_gnome_copied("cut\nfile:///home/max/a.txt\nfile:///home/max/b.txt");
        assert!(cut);
        assert_eq!(uris, "file:///home/max/a.txt\nfile:///home/max/b.txt");

        let (cut, uris) = parse_gnome_copied("copy\nfile:///home/max/a.txt");
        assert!(!cut);
        assert_eq!(uris, "file:///home/max/a.txt");
    }

    #[test]
    fn gnome_copied_without_verb_keeps_all_uris() {
        // A malformed payload (no copy/cut header) must not eat the first URI.
        let (cut, uris) = parse_gnome_copied("file:///home/max/a.txt");
        assert!(!cut);
        assert_eq!(uris, "file:///home/max/a.txt");
    }

    #[test]
    fn file_offers_carry_verb_and_all_types() {
        let offers = file_offers("file:///home/max/a%20b.txt\nfile:///home/max/c.txt", true);
        let by = |m: &str| -> Vec<u8> {
            offers.iter().find(|(k, _)| k == m).map(|(_, v)| v.clone()).unwrap()
        };
        // x-special/gnome-copied-files carries the cut verb + raw URIs.
        assert_eq!(
            String::from_utf8(by(GNOME_COPIED)).unwrap(),
            "cut\nfile:///home/max/a%20b.txt\nfile:///home/max/c.txt"
        );
        // text/uri-list is CRLF-terminated.
        assert_eq!(
            String::from_utf8(by("text/uri-list")).unwrap(),
            "file:///home/max/a%20b.txt\r\nfile:///home/max/c.txt\r\n"
        );
        // text/plain gives decoded local paths, one per line.
        assert_eq!(
            String::from_utf8(by("text/plain")).unwrap(),
            "/home/max/a b.txt\n/home/max/c.txt"
        );
    }

    #[test]
    fn text_offers_include_legacy_aliases() {
        let offers = text_offers("hello");
        let mimes: Vec<&str> = offers.iter().map(|(m, _)| m.as_str()).collect();
        assert!(mimes.contains(&"text/plain;charset=utf-8"));
        assert!(mimes.contains(&"UTF8_STRING"));
        assert!(offers.iter().all(|(_, v)| v == b"hello"));
    }

    #[test]
    fn empty_file_offer_is_empty() {
        assert!(file_offers("# comment only\n", false).is_empty());
    }

    #[test]
    fn window_label_joins_and_omits_empty() {
        assert_eq!(window_label("firefox", "GitHub"), "firefox — GitHub");
        assert_eq!(window_label("firefox", ""), "firefox");
        assert_eq!(window_label("", "Just a title"), "Just a title");
        assert_eq!(window_label("", ""), "");
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

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
use tracing::{debug, warn};

use crate::animation::{ease_toward, lerp};
use crate::content::{GridContent, IconInst, Label, Rect, RectInst, Scene};
use crate::options::{
    hover_grow, push_neumorph, wash, PillId, BOND_GAP, FONT_PX, GLYPH_CLIPBOARD, GLYPH_CLOSE,
    GLYPH_COPY, GLYPH_CUT, GLYPH_SELECT_ALL, LINE_PX, NERD, PILL_MARGIN_Y, PILL_PAD_X,
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
/// Leading row glyphs for non-text clips without a thumbnail (dirs / plain files).
const GLYPH_IMAGE: &str = "\u{f03e}"; // fa-image
const GLYPH_FILES: &str = "\u{f0c6}"; // fa-paperclip
const GLYPH_FOLDER: &str = "\u{f07b}"; // fa-folder (a directory clip)
/// Square icon/thumbnail tile at the left of every image / files row. The list
/// stays compact — the full-size preview lives in the metadata detail view.
const TILE_SZ: f32 = 56.0;
/// Detail view: top pill-row gap + left inset (pill diameter matches the paste
/// pill = `clip_band_h`). The row-grow open animation runs over this many secs.
const DETAIL_PILL_GAP: f32 = 6.0;
const DETAIL_PILL_X: f32 = 12.0;
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
/// Texture-array slots kept for clipboard thumbnails (recycled round-robin),
/// appended after the notif card avatars on the OPTIONS renderer.
const THUMB_CAP: usize = 32;
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
    /// The detail view's top-strip pills.
    Back,
    DetailDelete,
    DetailCopy,
    DetailMore,
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
}

/// Something the worker observed.
pub enum ClipEvent {
    /// A new clip landed on the clipboard.
    Captured(ClipEntry),
    /// The *primary* (highlight) selection went non-empty (`true`, the user just
    /// selected something) or empty (`false`). Drives the copy/cut/select pills.
    Selection(bool),
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
    Copy(ClipEntry),
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
    let source_events = events.clone();
    std::thread::Builder::new()
        .name("clipboard-watch".into())
        .spawn(move || watch_loop(events))
        .expect("spawn clipboard watch thread");
    std::thread::Builder::new()
        .name("clipboard-primary".into())
        .spawn(move || primary_watch_loop(primary_events))
        .expect("spawn clipboard primary thread");

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
            ClipCommand::CopySelection => {
                let text = paste_primary();
                if !text.trim().is_empty() {
                    serve(text_offers(&text));
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
    entry.source = capture_source(&entry);
    Some(entry)
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
fn capture_source(entry: &ClipEntry) -> String {
    if entry.kind == ClipKind::Files {
        let dir = file_source(&entry.text);
        if !dir.is_empty() {
            return dir;
        }
    }
    match crate::hypr::active_window_where() {
        Some((class, title)) => window_label(&class, &title),
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
    pub fn new(handle: Option<ClipHandle>, thumbs: Option<crate::thumbs::Thumbs>) -> Self {
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
            thumbs,
            icon_chains: Vec::new(),
            icon_slot: HashMap::new(),
            icon_key_at: Vec::new(),
            icon_next: 0,
            icon_pending: HashSet::new(),
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

/// The laid-out height of a clip's row (grows with its shown line count, and is
/// at least the tile height for image / files rows).
fn row_height_of(entry: &ClipEntry) -> f32 {
    let text_h = clip_row_lines(entry).len() as f32 * LINE_PX;
    let tile_h = if matches!(clip_tile(entry), ClipTile::None) {
        0.0
    } else {
        TILE_SZ
    };
    text_h.max(tile_h) + 2.0 * ROW_PAD_Y
}

impl App {
    /// A clipboard change arrived from the worker: fold it into the history
    /// (newest first, de-duplicated) and persist.
    pub(crate) fn on_clip_event(&mut self, ev: ClipEvent) {
        match ev {
            ClipEvent::Selection(present) => self.on_clip_selection(present),
            ClipEvent::Pasted => self.record_clip_paste(),
            ClipEvent::Captured(mut entry) => {
                // A re-copied clip moves to the top rather than piling up; it
                // keeps its original id (so any UI reference stays valid) and its
                // accumulated metadata (original source + paste log).
                if let Some(pos) = self.clip.history.iter().position(|e| e.hash == entry.hash) {
                    let old = self.clip.history.remove(pos);
                    entry.id = old.id;
                    if !old.source.is_empty() {
                        entry.source = old.source;
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
                        if let Some(p) = &old.image_path {
                            let _ = std::fs::remove_file(p);
                        }
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
            h.send(ClipCommand::Copy(entry));
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

        // The content card grows out of the clicked row; the list is clipped to
        // the strip ABOVE the card (a hard partition, no cross-fade ghosting), so
        // the opaque card visibly *eats* the list as it grows.
        let dt = self.clip.detail_t;
        let (list_top, card_top) = if dt > 0.001 {
            let (content_full, _) = self.clip_detail_regions(rect);
            let ct = lerp(self.clip.detail_src.y, content_full.y, dt);
            // The top pill strip wipes DOWN from the box edge as it opens; keep
            // the list clear of the part already swept (its lower edge rides
            // `dt`), so the band eats the list from the top as the card eats it
            // from the bottom — no plain-fill sliver in between.
            (rect.y + self.clip_detail_top_h() * dt, ct)
        } else {
            (content.y, content.y + content.h)
        };
        let list_clip = Rect::new(content.x, list_top, content.w, (card_top - list_top).max(0.0));
        if list_clip.h > 0.5 {
            for (idx, rr) in self.clip_rows(rect) {
                self.push_clip_row(
                    scene, idx, rr, list_clip, e, ink, dim_ink, hover_ink, stripe_opaque,
                );
            }
        }
        if dt < 0.02 {
            self.push_clip_footer(scene, rect, e, bright);
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

        let col = if hovered {
            [hover_ink[0], hover_ink[1], hover_ink[2], e]
        } else {
            [dim_ink[0], dim_ink[1], dim_ink[2], dim_ink[3] * e]
        };
        let ty = rr.y + (rr.h - LINE_PX) / 2.0;
        // Clip to the (possibly narrowed) content width so labels are masked at
        // the detail-wipe boundary rather than bleeding under the panel.
        let row_clip = Rect::new(content.x, top, content.w, bot - top);

        let tile = clip_tile(entry);

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
        let max_w = (text_right - tx).max(0.0);
        let lines = clip_row_lines(entry);
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
        let g = self.options_text_color();
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
            color: Some([g[0], g[1], g[2], g[3] * alpha]),
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
        // The drawer takes over the corner from any selection-pill context, so
        // the peek isn't suppressed under the open box (which would leave it a
        // tall, thin stripe).
        self.clip.selection_present = false;
        self.clip.selection_active = false;
        self.clip.grace_deadline = None;
        self.clip.peek_reveal = true; // keep the preview fully out under the box
        self.clip.expanded = true;
        self.clip.box_h = self.clip_full_h();
        self.clip.list_scroll = 0.0;
        self.clip.scroll_target = 0.0;
        self.clip.scroll_accum = 0.0;
        self.sync_options_input();
        self.reeval_options_bar();
        self.request_clip_thumbs();
        self.schedule_clip_frame();
    }

    /// Collapse the open history drawer back to the pill.
    fn close_clip_box(&mut self) {
        if self.clip.expanded {
            self.clip.expanded = false;
            self.clip.hit = ClipHit::None;
            self.clip.hover_row = None;
            // Reset the detail view so a reopened box starts on the list.
            self.clip.detail_id = None;
            self.clip.detail_open = false;
            self.clip.detail_t = 0.0;
            self.clip.detail_p = 0.0;
            self.clip.detail_meta_t = 0.0;
            self.clip.detail_meta_target = 0.0;
            self.clip.detail_meta_scroll = 0.0;
            self.clip.detail_meta_scroll_target = 0.0;
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

    /// Height of the top pill-row zone: tight to the pill height (the pills sit
    /// flush at the top, aligned with the paste pill; the strip is just a hair
    /// taller so it reads as the pill row, not a big band).
    fn clip_detail_top_h(&self) -> f32 {
        self.clip_band_h() + 2.0 * PILL_MARGIN_Y
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
    /// Source/Copied/Pasted rows plus one row per logged paste.
    fn clip_detail_meta_natural(&self) -> f32 {
        let logs = self
            .clip_detail_entry()
            .map(|e| e.pastes.len())
            .unwrap_or(0);
        // Source, Copied, Pasted-header, then a row per paste.
        META_INNER_TOP + (3 + logs) as f32 * META_ROW_H + META_INNER_BOT
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

    /// The four top-row pill rects with their hit targets (back, delete, copy,
    /// more — left to right, aligned with the paste pill).
    fn clip_detail_pills(&self, rect: Rect) -> [(ClipHit, Rect); 4] {
        // Same diameter and top as the paste pill, so all read as one row.
        let d = self.clip_band_h();
        let y = rect.y;
        let x0 = rect.x + DETAIL_PILL_X;
        let at = |i: f32| Rect::new(x0 + i * (d + DETAIL_PILL_GAP), y, d, d);
        [
            (ClipHit::Back, at(0.0)),
            (ClipHit::DetailDelete, at(1.0)),
            (ClipHit::DetailCopy, at(2.0)),
            (ClipHit::DetailMore, at(3.0)),
        ]
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
            lerp(src.x, content_full.x, dt),
            lerp(src.y, content_full.y, dt),
            lerp(src.w, content_full.w, dt),
            lerp(src.h, content_full.h, dt),
        );
        // Opaque backing — the card keeps the clicked row's zebra tone;
        // a solid panel that grows, not a fade.
        scene.rects.push(RectInst {
            rect: card,
            radius: 0.0,
            color: card_c,
            glass: 0.0,
        });

        if entry.kind == ClipKind::Text {
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
        let vis_h = meta_full.h * dt;
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
        // its lower edge — and the divider that rides it — descend with `dt`. ----
        let top_h = self.clip_detail_top_h();
        let band_h = top_h * dt;
        let band_bottom = rect.y + band_h;
        push_top_rounded(
            scene,
            Rect::new(rect.x, rect.y, rect.w, band_h),
            BOX_RADIUS,
            sheet_c,
        );
        if band_h > 1.0 {
            scene.rects.push(RectInst {
                rect: Rect::new(rect.x, band_bottom - 1.0, rect.w, 1.0),
                radius: 0.0,
                color: [ink[0], ink[1], ink[2], 0.12 * e * dt],
                glass: 0.0,
            });
        }

        // ---- top pills: seat in over the last few px of the wipe (once the
        // descending band fully covers them), so they arrive WITH the strip
        // instead of fading onto an already-open bar. ----
        let seat = ((band_h - self.clip_band_h()) / (2.0 * PILL_MARGIN_Y)).clamp(0.0, 1.0);
        let pa = seat * seat * (3.0 - 2.0 * seat) * e;
        if pa > 0.01 {
            let ink0 = self.options_text_color();
            for (hit, prect) in self.clip_detail_pills(rect) {
                let hv = self.clip.hit == hit;
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
                    ClipHit::Back => (GLYPH_BACK, ink0),
                    ClipHit::DetailDelete => (GLYPH_CLOSE, ink0),
                    ClipHit::DetailMore => (GLYPH_MORE, ink0),
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
            ClipHit::Back => {
                self.close_clip_detail();
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
            // Placeholder 4th pill — reserved for a later action.
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
        // While a selection is in play, the clipboard corner is the action pills'
        // home: hovering it (re-)summons them and holds them; moving away lets
        // them fade after the grace. The history peek is suppressed in this mode.
        // Never while the drawer is open, though — suppressing the peek there
        // leaves `peek_reveal` false against an `expanded` box, i.e. a tall, thin
        // stripe that never opens or closes.
        if self.clip.selection_present && !self.clip.expanded {
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
        // Keep the beat pulsing until its deadline, then clear it.
        let beating = self.clip.blink_until.is_some_and(|u| now < u);
        if !beating {
            self.clip.blink_until = None;
        }
        self.draw_options();
        if moving || amoving || em || bm || lm || dm || mm || beating {
            self.schedule_clip_frame();
        } else {
            self.clip.last = None;
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

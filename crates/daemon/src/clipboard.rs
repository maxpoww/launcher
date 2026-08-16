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
    hover_grow, push_neumorph, PillId, BOND_GAP, FONT_PX, GLYPH_CLIPBOARD, LINE_PX, NERD,
    PILL_PAD_X,
};
use crate::App;

/// Target width of the extended preview pill (mirrors the notification OPTION's
/// preview so the two elements read as siblings).
const PEEK_W: f32 = 380.0;
/// Glide rate of the peek morph (exponential approach), matched to the bell.
const MORPH_RATE: f32 = 13.0;
const MORPH_EPS: f32 = 0.001;
/// Grace before the preview collapses once the pointer leaves — enough to cross
/// a small gap, snappy otherwise. Matches the bell's `LEAVE_HOLD`.
const LEAVE_HOLD: Duration = Duration::from_millis(300);
/// A fresh clip beats the small pill for this long — one slow heartbeat (swell +
/// settle), the same single-period pulse as the bell's muted-arrival blink.
const BEAT_DURATION: Duration = Duration::from_millis(500);
const BEAT_PERIOD: Duration = Duration::from_millis(500);

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

/// A clipboard change the worker observed.
pub enum ClipEvent {
    Captured(ClipEntry),
}

/// A UI intent for the worker.
// Constructed by `copy_clip`, which the history box (stage 2) drives; wired end
// to end now so the copy-back path is ready when the UI lands.
#[allow(dead_code)]
pub enum ClipCommand {
    /// Restore this payload to the system clipboard (`wl-copy`).
    Copy {
        mime: String,
        text: String,
        image_path: Option<PathBuf>,
    },
}

/// Handle to the clipboard worker: send [`ClipCommand`]s. Dropping it stops the
/// command side after its current recv.
#[allow(dead_code)] // `send` is exercised by the box UI (stage 2).
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

/// Worker entry: a detached watch loop feeding `events`, and this thread serving
/// copy-back commands until the handle is dropped.
fn run_worker(events: Sender<ClipEvent>, commands: mpsc::Receiver<ClipCommand>) {
    std::thread::Builder::new()
        .name("clipboard-watch".into())
        .spawn(move || watch_loop(events))
        .expect("spawn clipboard watch thread");

    while let Ok(cmd) = commands.recv() {
        copy_back(cmd);
    }
}

/// Restore a payload to the clipboard with `wl-copy` (which forks to serve the
/// selection, so the write returns promptly).
fn copy_back(cmd: ClipCommand) {
    let ClipCommand::Copy {
        mime,
        text,
        image_path,
    } = cmd;
    let mut c = Command::new("wl-copy");
    c.arg("--type").arg(&mime).stdin(Stdio::piped());
    let data = match &image_path {
        Some(path) => match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!("clipboard: cannot read image {path:?} for copy-back: {e}");
                return;
            }
        },
        None => text.into_bytes(),
    };
    match c.spawn() {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                if let Err(e) = stdin.write_all(&data) {
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
}

impl ClipState {
    pub fn new(handle: Option<ClipHandle>) -> Self {
        let mut history: Vec<ClipEntry> =
            crate::persist::read_json(&crate::persist::data_path(HISTORY_FILE)).unwrap_or_default();
        history.sort_by_key(|e| std::cmp::Reverse(e.timestamp_ms));
        history.truncate(MAX_HISTORY);
        let next_id = history.iter().map(|e| e.id).max().unwrap_or(0) + 1;
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
        }
    }
}

impl App {
    /// A clipboard change arrived from the worker: fold it into the history
    /// (newest first, de-duplicated) and persist.
    pub(crate) fn on_clip_event(&mut self, ev: ClipEvent) {
        match ev {
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
                self.save_clip_history();
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

    /// Restore history entry `idx` to the system clipboard (a card click).
    #[allow(dead_code)] // wired to the history box in a later stage.
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
        Rect::new(left, y, w, ph)
    }

    /// Draw the preview/box element: the pill that slides out to the right from
    /// behind the small clipboard glyph, showing the most-recent clip (fading in
    /// with the peek). The glyph itself lives on the small fixed pill.
    pub(crate) fn push_clip_pill(&self, scene: &mut Scene, rect: Rect) {
        let bright = self.options_bar_is_bright();
        let peek = self.clip.peek_t;
        // Nothing to show until it starts sliding out (at rest it hides fully
        // behind the small pill, which draws on top).
        if peek < 0.001 {
            return;
        }
        let radius = rect.h / 2.0; // stadium ⇒ circle when w == h
        push_neumorph(scene, rect, radius, bright, 1.0);
        // The preview never reacts to hover (mirrors the box): resting wash.
        scene.rects.push(RectInst {
            rect,
            radius,
            color: self.options_rest_wash(),
            glass: 0.0,
        });

        // Preview of the newest clip, left-aligned, fading in with the peek.
        let pa = ((peek - 0.35) / 0.5).clamp(0.0, 1.0);
        if pa > 0.01 {
            if let Some(latest) = self.clip.history.first() {
                let ink = self.options_text_color();
                let tx = rect.x + PILL_PAD_X;
                let gty = rect.y + (rect.h - LINE_PX) / 2.0;
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
            // Nothing to preview on an empty history — just the resting glyph.
            if !self.clip.peek_reveal && !self.clip.history.is_empty() {
                self.clip.peek_reveal = true;
                self.clip.last = None;
                self.schedule_clip_frame();
            }
        } else if self.clip.peek_reveal && self.clip.hold_deadline.is_none() {
            self.schedule_clip_collapse(LEAVE_HOLD);
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
        // Keep the beat pulsing until its deadline, then clear it.
        let beating = self.clip.blink_until.is_some_and(|u| now < u);
        if !beating {
            self.clip.blink_until = None;
        }
        self.draw_options();
        if moving || beating {
            self.schedule_clip_frame();
        } else {
            self.clip.last = None;
        }
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

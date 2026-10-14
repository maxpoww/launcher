//! Background file thumbnails for the Files section and directory
//! stacks: real previews wherever one can be had, in priority order —
//! the freedesktop thumbnail cache (covers anything another app already
//! thumbnailed), in-process decoding for images, and the external
//! `ffmpegthumbnailer` / `pdftoppm` tools for videos and PDFs when
//! installed. Everything else keeps its type icon.
//!
//! One worker thread serializes the jobs (a listing can ask for dozens
//! at once); results return over a calloop channel as premultiplied
//! `ICON_SIZE`² RGBA ready for [`crate::renderer::Renderer::update_icon_layer`].

use std::path::PathBuf;
use std::sync::mpsc;

use calloop::channel::Sender;
use tracing::{debug, warn};

use crate::apps::ICON_SIZE;
use crate::files::file_asset_name;

/// Texture layers reserved for thumbnails past the package blocks —
/// slots recycle round-robin once full.
pub const THUMB_CAP: usize = 128;

/// A worker result for one requested file.
pub enum Event {
    /// A finished thumbnail: the file and its premultiplied RGBA8
    /// `ICON_SIZE`² pixels (a full mip chain).
    Thumb { path: String, pixels: Vec<u8> },
    /// A file classified as video by extension turned out to have no video
    /// stream (an audio-only `.mp4`/`.mov`/…): the daemon reclassifies it
    /// to the audio icon instead of leaving a misleading video icon.
    AudioOnly { path: String },
    /// A picture that MOVES (a GIF, a video), as a short loop: its first
    /// seconds as `ICON_SIZE`² premultiplied RGBA frames (no mips — they
    /// are made as each is shown), [`MOVING_FPS`] a second. Empty: none
    /// could be made.
    Moving { path: String, frames: Vec<Vec<u8>> },
}

/// A moving preview: how many frames a second, and how long a loop.
pub const MOVING_FPS: u32 = 12;
const MOVING_SECS: u32 = 4;

/// Handle to the thumbnailer thread.
pub struct Thumbs {
    requests: mpsc::Sender<String>,
    moving: mpsc::Sender<(String, u64)>,
    /// Which asking the moving previews are for: one asked for before the
    /// last [`Thumbs::forget_moving`] is not made.
    moving_round: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Thumbs {
    /// Queue a thumbnail job (deduplication is the caller's business).
    pub fn request(&self, path: &str) {
        let _ = self.requests.send(path.to_owned());
    }

    /// Queue the moving preview of a GIF or a video (answered with
    /// [`Event::Moving`], on a thread of its own: a still never waits
    /// behind one).
    pub fn request_moving(&self, path: &str) {
        let round = self.moving_round.load(std::sync::atomic::Ordering::SeqCst);
        let _ = self.moving.send((path.to_owned(), round));
    }

    /// The moving previews asked for so far are not wanted any more (the
    /// box they were for is gone): the ones still waiting are skipped —
    /// each is a run of ffmpeg and a dozen megabytes, and the card opened
    /// and closed five times had sixty of them queued.
    pub fn forget_moving(&self) {
        self.moving_round.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The program that reads the frames of what moves: the one named by
/// `WAVERUNNER_FFMPEG` (the dev launcher builds one and names it, as it
/// does the wallpaper player), else the one on the PATH.
fn ffmpeg_tool() -> Option<String> {
    if let Some(named) = std::env::var("WAVERUNNER_FFMPEG").ok().filter(|p| std::path::Path::new(p).is_file()) {
        return Some(named);
    }
    crate::launch::on_path("ffmpeg").then(|| "ffmpeg".to_owned())
}

/// Whether moving previews can be made on this computer at all.
pub fn can_move() -> bool {
    ffmpeg_tool().is_some()
}

/// The first seconds of the GIF or video at `path` as frames: each the
/// whole picture centred on a clear square, as a still thumbnail is.
fn moving_frames(path: &str) -> Vec<Vec<u8>> {
    let Some(tool) = ffmpeg_tool() else {
        return Vec::new();
    };
    let n = ICON_SIZE;
    let filter = format!(
        "fps={MOVING_FPS},scale={n}:{n}:force_original_aspect_ratio=decrease,format=rgba,pad={n}:{n}:-1:-1:color=black@0"
    );
    let out = std::process::Command::new(tool)
        .args(["-v", "error", "-nostdin", "-t", &MOVING_SECS.to_string(), "-i", path])
        .args(["-an", "-vf", &filter, "-frames:v", &(MOVING_FPS * MOVING_SECS).to_string()])
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    let bytes = match out {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => {
            warn!("thumb: ffmpeg exited {} for {path}", o.status);
            return Vec::new();
        }
        Err(e) => {
            warn!("thumb: ffmpeg failed to run: {e}");
            return Vec::new();
        }
    };
    let one = (n * n * 4) as usize;
    bytes
        .chunks_exact(one)
        .map(|frame| {
            // Premultiplied, as the icon pipeline is throughout (a GIF may
            // have clear parts; a video has none).
            let mut px = frame.to_vec();
            for p in px.chunks_exact_mut(4).filter(|p| p[3] != 255) {
                let a = p[3] as u32;
                p[0] = (p[0] as u32 * a / 255) as u8;
                p[1] = (p[1] as u32 * a / 255) as u8;
                p[2] = (p[2] as u32 * a / 255) as u8;
            }
            px
        })
        .collect()
}

/// Whether a file can plausibly get a real thumbnail (images always,
/// videos and PDFs via the cache or external tools).
pub fn thumbable(name: &str) -> bool {
    matches!(
        file_asset_name(name),
        "asset-image" | "asset-video" | "asset-pdf"
    )
}

/// Spawn the thumbnailer thread. It exits when either channel closes.
pub fn spawn(results: Sender<Event>) -> Thumbs {
    let (requests, rx) = mpsc::channel::<String>();
    let results_moving = results.clone();
    let spawned = std::thread::Builder::new()
        .name("waverunner-thumbs".into())
        .spawn(move || {
            while let Ok(path) = rx.recv() {
                let event = match thumbnail(&path) {
                    Some(pixels) => Event::Thumb { path, pixels },
                    // A "video" that produced no thumbnail is almost always
                    // audio-only (no video stream) — flag it so its icon
                    // becomes the audio one. Real failures on other kinds
                    // just keep their type icon.
                    None if file_asset_name(&path) == "asset-video" => {
                        debug!("no video stream, treating as audio: {path}");
                        Event::AudioOnly { path }
                    }
                    None => {
                        debug!("no thumbnail for {path}");
                        continue;
                    }
                };
                if results.send(event).is_err() {
                    return; // event loop is gone
                }
            }
        });
    if let Err(e) = spawned {
        warn!("cannot spawn thumbnailer thread: {e}");
    }
    let (moving, moving_rx) = mpsc::channel::<(String, u64)>();
    let moving_round = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let round_now = moving_round.clone();
    let spawned = std::thread::Builder::new().name("waverunner-moving".into()).spawn(move || {
        while let Ok((path, round)) = moving_rx.recv() {
            if round != round_now.load(std::sync::atomic::Ordering::SeqCst) {
                continue;
            }
            let frames = moving_frames(&path);
            debug!("thumb: {} moving frames for {path}", frames.len());
            if results_moving.send(Event::Moving { path, frames }).is_err() {
                return;
            }
        }
    });
    if let Err(e) = spawned {
        warn!("cannot spawn the moving previews' thread: {e}");
    }
    Thumbs { requests, moving, moving_round }
}

/// Produce one thumbnail: cache, then in-process decode, then external
/// tools; `None` when every source comes up empty.
fn thumbnail(path: &str) -> Option<Vec<u8>> {
    if let Some(px) = from_xdg_cache(path) {
        return Some(px);
    }
    let size = ICON_SIZE.to_string();
    let kind = file_asset_name(path);
    debug!("thumb: {path} classified {kind}");
    match kind {
        "asset-image" => {
            // A big picture (a wallpaper is 20 megapixels and more) takes a
            // moment to read whole: its preview is kept, and read back the
            // next time instead of made again.
            if let Some(px) = kept_preview(path) {
                return Some(px);
            }
            let px = decode_image(path)?;
            keep_preview(path, &px);
            Some(px)
        }
        "asset-video" => via_tool(path, "ffmpegthumbnailer", |src, dst| {
            vec![
                "-i".into(),
                src.into(),
                "-o".into(),
                dst.into(),
                "-s".into(),
                size.clone(),
            ]
        }),
        "asset-pdf" => via_tool(path, "pdftoppm", |src, dst| {
            // pdftoppm appends .png itself; hand it the stem.
            let stem = dst.trim_end_matches(".png").to_owned();
            vec![
                "-png".into(),
                "-f".into(),
                "1".into(),
                "-singlefile".into(),
                "-scale-to".into(),
                size.clone(),
                src.into(),
                stem,
            ]
        }),
        _ => None,
    }
}

/// The freedesktop thumbnail cache: `$XDG_CACHE_HOME/thumbnails/
/// {large,normal}/<md5 of the file URI>.png`. Free coverage for
/// anything another application already thumbnailed.
fn from_xdg_cache(path: &str) -> Option<Vec<u8>> {
    let uri = format!("file://{path}");
    let digest = format!("{:x}", md5::compute(uri.as_bytes()));
    let base = cache_base().join("thumbnails");
    for size in ["large", "normal"] {
        let png = base.join(size).join(format!("{digest}.png"));
        if png.exists() {
            if let Some(px) = decode_image(png.to_str()?) {
                return Some(px);
            }
        }
    }
    None
}

/// Pictures smaller than this are quick to read whole: no preview is kept.
const KEEP_FROM_BYTES: u64 = 600 * 1024;

/// Where the preview of the picture at `path` is kept: named by the file,
/// its size and when it last changed, so an edited picture gets a new one.
/// `None`: no such file, or a small one.
fn kept_path(path: &str) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() < KEEP_FROM_BYTES {
        return None;
    }
    let changed = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    // FNV-1a over the three: a name for a cache file, not a secret.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.bytes().chain(meta.len().to_le_bytes()).chain(changed.to_le_bytes()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(cache.join("waverunner/previews").join(format!("{h:016x}.rgba")))
}

/// The kept preview of a picture, if it is there and whole.
fn kept_preview(path: &str) -> Option<Vec<u8>> {
    let file = kept_path(path)?;
    let bytes = std::fs::read(&file).ok()?;
    crate::apps::still_used(&file);
    // (The same length a freshly made one has: anything else is a stale or
    // cut-short file.)
    let whole = crate::apps::with_mips(vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize]).len();
    (bytes.len() == whole).then_some(bytes)
}

/// Keep a preview for next time. Best effort: a full disk only means it is
/// made again.
fn keep_preview(path: &str, pixels: &[u8]) {
    let Some(file) = kept_path(path) else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = file.with_extension("tmp");
    if std::fs::write(&tmp, pixels).and_then(|()| std::fs::rename(&tmp, &file)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Decode an image file (png/jpeg/webp/gif/bmp/tiff — svg goes through
/// the icon rasterizer) and fit it into the icon square.
fn decode_image(path: &str) -> Option<Vec<u8>> {
    if path.to_ascii_lowercase().ends_with(".svg") {
        return crate::apps::rasterize_icon_file(path, path);
    }
    let img = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let thumb = img.thumbnail(ICON_SIZE, ICON_SIZE).to_rgba8();
    // Center the (aspect-kept) thumbnail on a transparent square and
    // premultiply — the icon pipeline is premultiplied throughout.
    let (tw, th) = (thumb.width(), thumb.height());
    let (ox, oy) = ((ICON_SIZE - tw) / 2, (ICON_SIZE - th) / 2);
    let mut out = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    for (y, row) in thumb.rows().enumerate() {
        for (x, px) in row.enumerate() {
            let a = px[3] as u32;
            let at = (((oy + y as u32) * ICON_SIZE + ox + x as u32) * 4) as usize;
            out[at] = (px[0] as u32 * a / 255) as u8;
            out[at + 1] = (px[1] as u32 * a / 255) as u8;
            out[at + 2] = (px[2] as u32 * a / 255) as u8;
            out[at + 3] = a as u8;
        }
    }
    // Thumbnails skip the frosted rounded plate `finish_tile` applies to
    // app icons: a file preview keeps its true square corners (and full
    // size/opacity), just mipped for clean minification.
    Some(crate::apps::with_mips(out))
}

/// Generate through an external thumbnailer into a scratch png, then
/// decode it. Logs (rather than silently swallowing) each failure mode so
/// a missing tool or a decode problem is diagnosable.
fn via_tool(path: &str, tool: &str, args: impl Fn(&str, &str) -> Vec<String>) -> Option<Vec<u8>> {
    let scratch = cache_base().join("waverunner");
    if let Err(e) = std::fs::create_dir_all(&scratch) {
        warn!("thumb: scratch dir {}: {e}", scratch.display());
        return None;
    }
    // Per-tool scratch name so a slow video job can't clobber a pdf's output.
    let dst = scratch.join(format!("scratch-{tool}.png"));
    let dst_s = dst.to_str()?.to_owned();
    let status = std::process::Command::new(tool)
        .args(args(path, &dst_s))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let status = match status {
        Ok(s) => s,
        // The classic cause: the tool isn't on the daemon's PATH.
        Err(e) => {
            warn!("thumb: '{tool}' failed to run (installed / on PATH?): {e}");
            return None;
        }
    };
    if !status.success() {
        warn!("thumb: '{tool}' exited {status} for {path}");
        return None;
    }
    let px = decode_image(&dst_s);
    if px.is_none() {
        warn!("thumb: '{tool}' output for {path} did not decode");
    }
    let _ = std::fs::remove_file(&dst);
    px
}

/// `$XDG_CACHE_HOME`, falling back to `~/.cache`.
fn cache_base() -> PathBuf {
    std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"))
}

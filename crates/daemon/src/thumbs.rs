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
pub const THUMB_CAP: usize = 64;

/// A finished thumbnail: the file it previews and its premultiplied
/// RGBA8 `ICON_SIZE`² pixels.
pub struct Event {
    pub path: String,
    pub pixels: Vec<u8>,
}

/// Handle to the thumbnailer thread.
pub struct Thumbs {
    requests: mpsc::Sender<String>,
}

impl Thumbs {
    /// Queue a thumbnail job (deduplication is the caller's business).
    pub fn request(&self, path: &str) {
        let _ = self.requests.send(path.to_owned());
    }
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
    let spawned = std::thread::Builder::new()
        .name("waverunner-thumbs".into())
        .spawn(move || {
            while let Ok(path) = rx.recv() {
                let Some(pixels) = thumbnail(&path) else {
                    debug!("no thumbnail for {path}");
                    continue;
                };
                if results.send(Event { path, pixels }).is_err() {
                    return; // event loop is gone
                }
            }
        });
    if let Err(e) = spawned {
        warn!("cannot spawn thumbnailer thread: {e}");
    }
    Thumbs { requests }
}

/// Produce one thumbnail: cache, then in-process decode, then external
/// tools; `None` when every source comes up empty.
fn thumbnail(path: &str) -> Option<Vec<u8>> {
    if let Some(px) = from_xdg_cache(path) {
        return Some(px);
    }
    match file_asset_name(path) {
        "asset-image" => decode_image(path),
        "asset-video" => via_tool(path, "ffmpegthumbnailer", |src, dst| {
            vec![
                "-i".into(),
                src.into(),
                "-o".into(),
                dst.into(),
                "-s".into(),
                "128".into(),
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
                "128".into(),
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
    Some(crate::apps::finish_tile(out))
}

/// Generate through an external thumbnailer into a scratch png, then
/// decode it. Quietly gives up when the tool isn't installed.
fn via_tool(path: &str, tool: &str, args: impl Fn(&str, &str) -> Vec<String>) -> Option<Vec<u8>> {
    let scratch = cache_base().join("waverunner");
    std::fs::create_dir_all(&scratch).ok()?;
    let dst = scratch.join("scratch-thumb.png");
    let dst_s = dst.to_str()?.to_owned();
    let status = std::process::Command::new(tool)
        .args(args(path, &dst_s))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let px = decode_image(&dst_s);
    let _ = std::fs::remove_file(&dst);
    px
}

/// `$XDG_CACHE_HOME`, falling back to `~/.cache`.
fn cache_base() -> PathBuf {
    std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"))
}

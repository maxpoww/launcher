//! Background application discovery: desktop-entry scan plus icon
//! rasterization, run on the one background thread the daemon allows.
//!
//! The thread is long-lived and rescans on request (the daemon asks
//! whenever the dock is summoned), so newly installed apps appear and
//! uninstalled ones vanish without a daemon restart. Rasterized icons
//! are cached across rescans keyed by resolved file path — a nix
//! store path changes when the theme updates, invalidating naturally —
//! so a rescan costs only the `.desktop` parse plus new icons. The
//! rasters are also persisted under `$XDG_CACHE_HOME/waverunner`, so a
//! cold daemon start skips rasterization entirely for unchanged icon
//! files. Results are handed to the event loop over a calloop channel;
//! nothing here touches Wayland or wgpu.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

use calloop::channel::Sender;
use resvg::tiny_skia;
use tracing::{debug, warn};
use waverunner_core::index::{AppEntry, DesktopIndex};

/// Side length of every rasterized icon, in pixels. All icons are
/// normalized to this size so the renderer can pack them into one
/// texture array layer each.
pub const ICON_SIZE: u32 = 48;

/// The indexer thread's result: entries plus one RGBA8 (premultiplied)
/// `ICON_SIZE`² image per entry, aligned by index.
pub struct LoadedApps {
    /// Discovered entries, sorted by name.
    pub entries: Vec<AppEntry>,
    /// Premultiplied RGBA8 pixels, `ICON_SIZE * ICON_SIZE * 4` bytes per
    /// entry; a generated placeholder tile where no icon was found.
    pub icons: Vec<Vec<u8>>,
    /// `true` for entries whose icon could not be resolved (placeholder tile).
    pub placeholders: Vec<bool>,
}

/// Handle to the long-lived indexer thread.
pub struct Indexer {
    requests: mpsc::Sender<()>,
}

impl Indexer {
    /// Ask for a rescan. Multiple queued requests coalesce into one
    /// scan; a dead indexer thread makes this a no-op.
    pub fn request_rescan(&self) {
        let _ = self.requests.send(());
    }
}

/// Spawn the indexer thread and queue the initial scan. The thread
/// exits when either channel closes (daemon shutdown).
pub fn spawn_indexer(icon_theme: String, results: Sender<LoadedApps>) -> Indexer {
    let (requests, rx) = mpsc::channel::<()>();
    let spawned = std::thread::Builder::new()
        .name("waverunner-index".into())
        .spawn(move || {
            let mut icon_cache: HashMap<String, (Vec<u8>, bool)> = HashMap::new();
            let disk_cache = DiskCache::new();
            while rx.recv().is_ok() {
                // Coalesce any requests that queued up meanwhile.
                while rx.try_recv().is_ok() {}

                let started = std::time::Instant::now();
                let index = DesktopIndex::scan();
                let (icons, placeholders): (Vec<_>, Vec<_>) = index
                    .entries
                    .iter()
                    .map(|entry| cached_icon(&mut icon_cache, &disk_cache, entry, &icon_theme))
                    .unzip();
                debug!(
                    "indexed {} apps in {:?}",
                    index.entries.len(),
                    started.elapsed()
                );
                if results
                    .send(LoadedApps {
                        entries: index.entries,
                        icons,
                        placeholders,
                    })
                    .is_err()
                {
                    return; // event loop is gone
                }
            }
        });
    if let Err(e) = spawned {
        warn!("cannot spawn indexer thread: {e}");
    }
    let indexer = Indexer { requests };
    indexer.request_rescan();
    indexer
}

/// Look an entry's icon up in (or insert it into) the raster caches:
/// in-memory first, then disk, then rasterize the icon file (persisting
/// the result). Returns `(pixels, is_placeholder)`.
fn cached_icon(
    cache: &mut HashMap<String, (Vec<u8>, bool)>,
    disk: &DiskCache,
    entry: &AppEntry,
    theme: &str,
) -> (Vec<u8>, bool) {
    let (key, path) = match resolve_icon_path(entry, theme) {
        Some(path) => (path.clone(), Some(path)),
        None => (format!("placeholder:{}", entry.name), None),
    };
    if let Some((pixels, is_placeholder)) = cache.get(&key) {
        return (pixels.clone(), *is_placeholder);
    }
    // Placeholder tiles are cheap to regenerate and never touch disk;
    // real icons go through the persistent cache.
    let cache_file = path.as_deref().and_then(|p| disk.file_for(p));
    let (pixels, is_placeholder) = match cache_file.as_deref().and_then(DiskCache::load) {
        Some(pixels) => (pixels, false),
        None => match path.and_then(|p| rasterize_icon_file(&p, &entry.id)) {
            Some(pixels) => {
                if let Some(file) = &cache_file {
                    disk.store(file, &pixels);
                }
                (pixels, false)
            }
            None => (placeholder_icon(&entry.name), true),
        },
    };
    cache.insert(key, (pixels.clone(), is_placeholder));
    (pixels, is_placeholder)
}

/// Persistent raster cache: one raw premultiplied-RGBA8 file per source
/// icon under `$XDG_CACHE_HOME/waverunner/icons-<SIZE>/` (falling back
/// to `~/.cache/...`), named by a hash of the icon's path, size, and
/// mtime. An edited icon file re-rasterizes via the mtime, and nix
/// store path churn (theme updates) invalidates via the path; stale
/// entries are tiny (`ICON_SIZE`² × 4 bytes) and simply linger.
struct DiskCache {
    dir: PathBuf,
}

impl DiskCache {
    fn new() -> Self {
        let base = std::env::var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")
            });
        Self {
            dir: base.join("waverunner").join(format!("icons-{ICON_SIZE}")),
        }
    }

    /// Cache file for `icon_path`, or `None` if the icon file cannot be
    /// stat'ed (vanished between resolve and now).
    fn file_for(&self, icon_path: &str) -> Option<PathBuf> {
        let meta = std::fs::metadata(icon_path).ok()?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let key = format!("{icon_path}|{}|{mtime}", meta.len());
        Some(self.dir.join(format!("{:016x}", fnv1a64(&key))))
    }

    /// Read a cached raster back, rejecting files of the wrong size
    /// (truncated write or a stale `ICON_SIZE`).
    fn load(file: &std::path::Path) -> Option<Vec<u8>> {
        let pixels = std::fs::read(file).ok()?;
        (pixels.len() == (ICON_SIZE * ICON_SIZE * 4) as usize).then_some(pixels)
    }

    /// Persist a raster via write-to-temp + rename, so a concurrent
    /// reader never sees a half-written file. Failures only cost a
    /// re-rasterize next cold start.
    fn store(&self, file: &std::path::Path, pixels: &[u8]) {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            debug!("icon cache: cannot create {:?}: {e}", self.dir);
            return;
        }
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        let write = std::fs::write(&tmp, pixels).and_then(|()| std::fs::rename(&tmp, file));
        if let Err(e) = write {
            debug!("icon cache: cannot write {file:?}: {e}");
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// FNV-1a, 64-bit: tiny, deterministic, and collision-safe enough for a
/// few hundred cache file names.
fn fnv1a64(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

/// Resolve an entry's icon name to a file path via the theme lookup.
fn resolve_icon_path(entry: &AppEntry, theme: &str) -> Option<String> {
    let name = entry.icon.as_deref()?;
    let path = if name.starts_with('/') {
        std::path::PathBuf::from(name)
    } else {
        freedesktop_icons::lookup(name)
            .with_size(ICON_SIZE as u16)
            .with_theme(theme)
            .find()
            .or_else(|| {
                freedesktop_icons::lookup(name)
                    .with_size(ICON_SIZE as u16)
                    .find()
            })?
    };
    Some(path.to_string_lossy().into_owned())
}

/// Read and rasterize one icon file to `ICON_SIZE`² premultiplied RGBA.
fn rasterize_icon_file(path: &str, id: &str) -> Option<Vec<u8>> {
    let path = std::path::Path::new(path);
    let data = std::fs::read(path).ok()?;
    let pixmap = match path.extension().and_then(|e| e.to_str()) {
        Some("svg") | Some("svgz") => rasterize_svg(&data)?,
        Some("png") => fit_pixmap(tiny_skia::Pixmap::decode_png(&data).ok()?),
        other => {
            debug!("unsupported icon format {other:?} for {id}");
            return None;
        }
    };
    Some(pixmap.take())
}

/// Render an SVG into an `ICON_SIZE`² pixmap.
fn rasterize_svg(data: &[u8]) -> Option<tiny_skia::Pixmap> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE)?;
    let size = tree.size();
    let scale = (ICON_SIZE as f32 / size.width()).min(ICON_SIZE as f32 / size.height());
    let tx = (ICON_SIZE as f32 - size.width() * scale) / 2.0;
    let ty = (ICON_SIZE as f32 - size.height() * scale) / 2.0;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty),
        &mut pixmap.as_mut(),
    );
    Some(pixmap)
}

/// Scale an arbitrary-size pixmap down (or up) into `ICON_SIZE`².
fn fit_pixmap(src: tiny_skia::Pixmap) -> tiny_skia::Pixmap {
    if src.width() == ICON_SIZE && src.height() == ICON_SIZE {
        return src;
    }
    let mut dst = match tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE) {
        Some(dst) => dst,
        None => return src,
    };
    let scale = (ICON_SIZE as f32 / src.width() as f32).min(ICON_SIZE as f32 / src.height() as f32);
    let tx = (ICON_SIZE as f32 - src.width() as f32 * scale) / 2.0;
    let ty = (ICON_SIZE as f32 - src.height() as f32 * scale) / 2.0;
    dst.draw_pixmap(
        0,
        0,
        src.as_ref(),
        &tiny_skia::PixmapPaint {
            quality: tiny_skia::FilterQuality::Bilinear,
            ..Default::default()
        },
        tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty),
        None,
    );
    dst
}

/// Deterministic colored tile for apps without a resolvable icon,
/// so every entry stays clickable and visually distinct.
fn placeholder_icon(name: &str) -> Vec<u8> {
    let hash = name
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let (r, g, b) = hue_to_rgb((hash % 360) as f32);

    let mut pixmap = match tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE) {
        Some(p) => p,
        None => return vec![0; (ICON_SIZE * ICON_SIZE * 4) as usize],
    };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(r, g, b, 215);
    paint.anti_alias = true;

    // Rounded-rectangle path with transparent corners so the tile blends
    // cleanly against any card background without hard square edges.
    let s = ICON_SIZE as f32;
    let (x, y, w, h, rad) = (4.0f32, 4.0, s - 8.0, s - 8.0, 10.0);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + rad, y);
    pb.line_to(x + w - rad, y);
    pb.quad_to(x + w, y, x + w, y + rad);
    pb.line_to(x + w, y + h - rad);
    pb.quad_to(x + w, y + h, x + w - rad, y + h);
    pb.line_to(x + rad, y + h);
    pb.quad_to(x, y + h, x, y + h - rad);
    pb.line_to(x, y + rad);
    pb.quad_to(x, y, x + rad, y);
    pb.close();
    if let Some(path) = pb.finish() {
        pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
    }
    pixmap.take()
}

/// Muted-palette hue to RGB (fixed saturation/lightness).
fn hue_to_rgb(hue: f32) -> (u8, u8, u8) {
    let h = hue / 60.0;
    let c = 0.35f32; // chroma: keep tiles muted
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = 0.35f32; // lightness floor
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh scratch cache dir plus a fake "icon" file inside it.
    /// `name` keeps parallel tests out of each other's directories.
    fn scratch(name: &str) -> (DiskCache, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "waverunner-icon-cache-test-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let icon = dir.join("icon.svg");
        std::fs::write(&icon, b"<svg/>").unwrap();
        (DiskCache { dir }, icon)
    }

    #[test]
    fn disk_cache_round_trips() {
        let (cache, icon) = scratch("round-trip");
        let icon = icon.to_string_lossy().into_owned();
        let file = cache.file_for(&icon).unwrap();
        assert_eq!(DiskCache::load(&file), None, "cold cache misses");

        let pixels = vec![7u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
        cache.store(&file, &pixels);
        assert_eq!(DiskCache::load(&file), Some(pixels));

        std::fs::remove_dir_all(&cache.dir).unwrap();
    }

    #[test]
    fn disk_cache_rejects_wrong_size_and_tracks_content() {
        let (cache, icon) = scratch("reject");
        let icon_str = icon.to_string_lossy().into_owned();
        let file = cache.file_for(&icon_str).unwrap();
        cache.store(&file, &[7u8; 3]);
        assert_eq!(DiskCache::load(&file), None, "truncated raster rejected");

        // Growing the icon file changes its cache key.
        std::fs::write(&icon, b"<svg></svg>").unwrap();
        assert_ne!(cache.file_for(&icon_str).unwrap(), file);
        // A vanished icon file has no key at all.
        assert_eq!(cache.file_for("/nonexistent/icon.svg"), None);

        std::fs::remove_dir_all(&cache.dir).unwrap();
    }
}

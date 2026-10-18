//! COLOURS ON THE WALLPAPERS (Max, 2026-10-18: "a slider at the top of the
//! wallpaper page, that says 'Colours', and sliding it applies the current
//! palette to the wallpapers on the box, and on the current wallpaper too").
//!
//! The slider (`ColorChoice::wallpaper_tint`) moves every picture toward the
//! palette in use through THE ICON FILTER's own maths (`palette::filter_rgb`,
//! built for a level of its own by `palette::filter_for`). Two places wear it:
//!
//! * THE BOX: each picture's tile is asked for under a made-up key
//!   (`tile_key`) that carries the filter; the thumbnailer reads the key,
//!   makes the usual preview and puts it through the filter (`filter_thumb`).
//! * THE WALLPAPER: a copy of the picture at the screen's size, filtered, is
//!   kept in `~/.cache/waverunner/tinted-walls/` and is what the picture
//!   program shows (`for_display`). `GolemSettings::wallpaper` is NEVER the
//!   copy: it stays the original, because Dynamic makes its palette from the
//!   wallpaper and would otherwise chase its own tint.
//!
//! What follows the colours already (`wall_follow`) and what moves (a video,
//! a GIF) is never tinted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

use crate::display::GolemSettings;
use crate::palette::{filter_rgb, Rgb};

type Filter = [[f32; 4]; 7];

/// How long the slider must rest before the wallpaper is made again (the
/// icons' own pace).
const REST: Duration = Duration::from_millis(400);

/// How many copies are kept (each is megabytes). The one in use stays.
const KEEP: usize = 4;

/// The drawing of a copy: a new look makes new files.
const VERSION: u64 = 1;

/// The most a copy may be asked for in a side.
const MAX_SIDE: u32 = 8192;

/// The key of a tinted tile starts with this; then the filter's hash, then
/// the picture's own absolute path (so its name and extension still read).
const KEY: &str = "/.wall-tint/";

// ------------------------------------------------------------------- pure

/// Whether a filter does anything.
pub(crate) fn active(f: &Filter) -> bool {
    f[0][0] > 0.0 && f[0][2] >= 1.0
}

/// A hash of a filter, stable between runs (it names files).
pub(crate) fn filter_hash(f: &Filter) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in f.iter().flatten() {
        for b in v.to_bits().to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// Whether the picture at `path` is one the slider tints: a still, that is
/// not the colours' own picture nor a tile of the first kind, with a filter
/// that does something.
pub(crate) fn tintable(path: &str, f: &Filter) -> bool {
    active(f) && !crate::controls::wallpaper_moves(path) && !crate::wall_follow::is_wall(path) && !crate::wall_follow::is_tile(path) && !is_copy(path)
}

/// Whether `path` is one of the copies made here.
pub(crate) fn is_copy(path: &str) -> bool {
    copies_dir().is_some_and(|d| Path::new(path).starts_with(d))
}

/// Whether the wallpaper reported up as `value` may be saved as the
/// wallpaper: never a copy (`GolemSettings::wallpaper` is the original).
pub(crate) fn saveable(value: &str) -> bool {
    !is_copy(value)
}

pub(crate) fn copies_dir() -> Option<PathBuf> {
    Some(crate::palette_card::cache_dir()?.join("tinted-walls"))
}

/// The name of the copy of `source` (made at `modified`) through filter `f`
/// at `w`×`h`: any of them changing makes another file.
pub(crate) fn cache_name(source: &str, modified: u64, f: &Filter, w: u32, h: u32) -> String {
    let mut x = filter_hash(f) ^ VERSION.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for b in source.bytes().chain(modified.to_le_bytes()) {
        x = (x ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{x:016x}-{w}x{h}.png")
}

static KEYS: Mutex<Option<HashMap<u64, Filter>>> = Mutex::new(None);

/// The key under which the tile of `file` is drawn: the file itself when the
/// slider is off or the file is not tinted, else a key that carries the
/// filter (remembered here for the thumbnailer to read back).
pub(crate) fn tile_key(file: &str, f: &Filter) -> String {
    if !Path::new(file).starts_with(crate::controls::wallpaper_dir()) || !tintable(file, f) {
        return file.to_owned();
    }
    let h = filter_hash(f);
    if let Ok(mut g) = KEYS.lock() {
        let map = g.get_or_insert_with(HashMap::new);
        if map.len() > 64 {
            map.clear();
        }
        map.insert(h, *f);
    }
    format!("{KEY}{h:016x}{file}")
}

/// A tinted tile's key taken apart: its filter and the picture. `None` for
/// any other path, and for a key of a filter that is not the one in use now
/// (a slider that moved on: its thumbnails are not made).
pub(crate) fn parse_key(key: &str, current: &Filter) -> Option<Result<(Filter, String), ()>> {
    let rest = key.strip_prefix(KEY)?;
    let (hex, file) = rest.split_at(rest.find('/')?);
    let h = u64::from_str_radix(hex, 16).ok()?;
    if h != filter_hash(current) {
        return Some(Err(()));
    }
    let f = KEYS.lock().ok()?.as_ref()?.get(&h).copied()?;
    Some(Ok((f, file.to_owned())))
}

/// `px` (sRGB, three bytes a pixel) through the filter, rows shared out to
/// the cores.
pub(crate) fn filter_pixels(px: &mut [u8], f: &Filter) {
    if !active(f) {
        return;
    }
    let threads = std::thread::available_parallelism().map_or(2, usize::from).min(8);
    let rows = px.len() / 3;
    let chunk = (rows / threads).max(4096) * 3;
    std::thread::scope(|sc| {
        for part in px.chunks_mut(chunk) {
            sc.spawn(move || {
                // (Runs of one colour — a sky, a flat — are common.)
                let mut last: (Rgb, Rgb) = ([0, 0, 0], filter_rgb([0, 0, 0], f));
                for p in part.chunks_exact_mut(3) {
                    let c = [p[0], p[1], p[2]];
                    if c != last.0 {
                        last = (c, filter_rgb(c, f));
                    }
                    p.copy_from_slice(&last.1);
                }
            });
        }
    });
}

/// A preview (premultiplied RGBA, `ICON_SIZE`², with its mip chain) through
/// the filter. Colours are taken to 5 bits for the lookup: a preview this
/// small shows no difference, and the tile costs a few thousand colours,
/// not sixty thousand.
pub(crate) fn filter_thumb(chain: Vec<u8>, f: &Filter) -> Vec<u8> {
    let n = crate::apps::ICON_SIZE as usize;
    if !active(f) || chain.len() < n * n * 4 {
        return chain;
    }
    let mut base = chain[..n * n * 4].to_vec();
    let mut memo: HashMap<u16, Rgb> = HashMap::new();
    for p in base.chunks_exact_mut(4).filter(|p| p[3] > 0) {
        let a = u32::from(p[3]);
        let un = |v: u8| if a == 255 { v } else { ((u32::from(v) * 255 + a / 2) / a).min(255) as u8 };
        let c = [un(p[0]), un(p[1]), un(p[2])];
        let q = (u16::from(c[0] >> 3) << 10) | (u16::from(c[1] >> 3) << 5) | u16::from(c[2] >> 3);
        let out = *memo.entry(q).or_insert_with(|| filter_rgb([(c[0] & 0xf8) | 4, (c[1] & 0xf8) | 4, (c[2] & 0xf8) | 4], f));
        for (d, o) in p[..3].iter_mut().zip(out) {
            *d = if a == 255 { o } else { (u32::from(o) * a / 255) as u8 };
        }
    }
    crate::apps::with_mips(base)
}

/// The size a copy of a `sw`×`sh` picture is made at for a `w`×`h` screen:
/// the smallest that still covers the screen, never larger than the picture.
pub(crate) fn copy_size(sw: u32, sh: u32, w: u32, h: u32) -> (u32, u32) {
    if sw == 0 || sh == 0 {
        return (sw, sh);
    }
    let scale = (f64::from(w) / f64::from(sw)).max(f64::from(h) / f64::from(sh));
    if scale >= 1.0 {
        return (sw, sh);
    }
    (((f64::from(sw) * scale).ceil() as u32).max(1), ((f64::from(sh) * scale).ceil() as u32).max(1))
}

// ----------------------------------------------------------------- effects

/// What this module put up last (the copy's path), so a slider that moved
/// back to where it was does nothing, and level 0 knows there is a copy to
/// take down.
static SHOWN: Mutex<Option<String>> = Mutex::new(None);

fn set_shown(p: Option<String>) {
    *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

fn shown() -> Option<String> {
    SHOWN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn modified_secs(p: &Path) -> Option<u64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(m.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs())
}

/// The screen's size, asked for a few times (the compositor may be a moment
/// from answering at the start).
fn screen() -> Option<(u32, u32)> {
    for _ in 0..10 {
        if let Some(s) = crate::display::wallpaper_size() {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    None
}

/// Where the copy of `source` for the screen `size` is (made or not yet).
fn target(source: &str, f: &Filter, size: (u32, u32)) -> Option<PathBuf> {
    let modified = modified_secs(Path::new(source))?;
    Some(copies_dir()?.join(cache_name(source, modified, f, size.0, size.1)))
}

/// The copy of `source` through `f` for the screen `size`, made if it is not
/// kept. `None`: the picture could not be read or written (the wallpaper
/// stays as it is).
fn ensure(source: &str, f: &Filter, size: (u32, u32)) -> Option<PathBuf> {
    if size.0 == 0 || size.1 == 0 || size.0 > MAX_SIDE || size.1 > MAX_SIDE {
        return None;
    }
    let out = target(source, f, size)?;
    if out.is_file() {
        return Some(out);
    }
    let started = std::time::Instant::now();
    let (px, w, h) = render(source, f, size)?;
    crate::palette_card::write_wall(&out, &px, w, h)?;
    info!("wallpaper: the tinted copy {w}x{h} ready in {:?}", started.elapsed());
    prune(out.to_str().unwrap_or(""));
    Some(out)
}

/// The picture at `source` read, brought to the screen's size and put through
/// the filter: its pixels and size.
pub(crate) fn render(source: &str, f: &Filter, size: (u32, u32)) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::open(source).ok()?.to_rgb8();
    let (nw, nh) = copy_size(img.width(), img.height(), size.0, size.1);
    let img = if (nw, nh) == (img.width(), img.height()) { img } else { image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle) };
    let (w, h) = (img.width(), img.height());
    let mut px = img.into_raw();
    filter_pixels(&mut px, f);
    Some((px, w, h))
}

/// Let go of the old copies (and any half-written one a crash left).
fn prune(in_use: &str) {
    let Some(dir) = copies_dir() else {
        return;
    };
    let mut pictures = Vec::new();
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(modified) = entry.metadata().and_then(|m| m.modified()).ok() else {
            continue;
        };
        if path.extension().is_some_and(|e| e == "png") {
            pictures.push((path, modified));
        } else if modified.elapsed().is_ok_and(|age| age > Duration::from_secs(600)) {
            let _ = std::fs::remove_file(&path);
        }
    }
    for gone in crate::wall_follow::stale(&pictures, KEEP, &[in_use]) {
        let _ = std::fs::remove_file(gone);
    }
}

/// What the picture program is to show for the wallpaper `path`: its tinted
/// copy when the slider is above 0 and the picture is a still, else `path`
/// itself. Records what it chose. For a thread that may wait.
pub(crate) fn for_display(path: &str) -> String {
    let f = crate::palette::wall_filter_now();
    if tintable(path, &f) {
        if let Some(copy) = screen().and_then(|size| ensure(path, &f, size)) {
            let copy = copy.to_string_lossy().into_owned();
            set_shown(Some(copy.clone()));
            return copy;
        }
        warn!("wallpaper: {path:?} could not be tinted, it stays as it is");
    }
    set_shown(None);
    path.to_owned()
}

static ASK: AtomicU64 = AtomicU64::new(0);

/// The slider (or the palette) moved: when it rests, the wallpaper is made
/// again — tinted, or the original back at 0. Cheap to call often; only the
/// last call of a run acts.
pub(crate) fn refresh() {
    let mine = ASK.fetch_add(1, Ordering::SeqCst) + 1;
    let spawned = std::thread::Builder::new().name("wall-tint".into()).spawn(move || {
        std::thread::sleep(REST);
        if ASK.load(Ordering::SeqCst) != mine {
            return;
        }
        apply();
    });
    if let Err(e) = spawned {
        warn!("wallpaper: no thread for the tint: {e}");
    }
}

fn apply() {
    let s = GolemSettings::load();
    // (Following the colours is already the palette; a moving wallpaper is
    // never tinted: both are `tintable`'s no.)
    let Some(original) = s.wallpaper.clone().filter(|_| !s.wallpaper_follows) else {
        return;
    };
    let f = crate::palette::wall_filter_now();
    let want = if tintable(&original, &f) { screen().and_then(|size| target(&original, &f, size)) } else { None };
    let now = shown();
    if want.as_ref().map(|p| p.to_string_lossy().into_owned()) == now {
        return;
    }
    // The same road as a pick (its fade); it saves the ORIGINAL again.
    crate::controls::pick_wallpaper(original);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jade() -> Filter {
        crate::palette::filter_for("jade", 100, false)
    }

    #[test]
    fn at_zero_the_filter_is_the_identity() {
        let f = crate::palette::filter_for("jade", 0, false);
        assert!(!active(&f));
        let mut px = vec![10u8, 200, 30, 128, 128, 128, 255, 0, 9];
        let before = px.clone();
        filter_pixels(&mut px, &f);
        assert_eq!(px, before);
        // And the slider's own zero in settings builds none at all.
        assert!(!active(&[[0.0; 4]; 7]));
    }

    #[test]
    fn at_a_hundred_a_grey_takes_the_palettes_colour() {
        let f = jade();
        assert!(active(&f));
        let out = filter_rgb([128, 128, 128], &f);
        let spread = out.iter().max().unwrap() - out.iter().min().unwrap();
        assert!(spread > 10, "a grey stayed grey: {out:?}");
        let mut px = vec![128u8; 3 * 10000];
        filter_pixels(&mut px, &f);
        assert_eq!(&px[..3], &out);
        assert_eq!(&px[px.len() - 3..], &out);
    }

    #[test]
    fn the_icons_filter_is_unchanged() {
        // The icons' row: 50 %, its own dark flag, through the shared maker.
        let a = crate::palette::filter_for("jade", 50, true);
        assert_eq!((a[0][0], a[0][1]), (0.5, 1.0));
        assert!(a[0][2] >= 1.0 && a[0][2] <= 6.0);
        let b = crate::palette::filter_for("jade", 50, false);
        assert_eq!(a[1..], b[1..]);
        // Dim alone (level 0) still dims, as the icons' Off + Dark does.
        let off = crate::palette::filter_for("jade", 0, true);
        assert_eq!((off[0][0], off[0][1]), (0.0, 1.0));
        assert_eq!(filter_rgb([200, 200, 200], &crate::palette::filter_for("jade", 0, false)), [200, 200, 200]);
    }

    #[test]
    fn the_copys_name_follows_level_palette_source_and_size() {
        let j = crate::palette::filter_for("jade", 60, false);
        let base = cache_name("/w/a.jpg", 100, &j, 2560, 1600);
        assert_eq!(base, cache_name("/w/a.jpg", 100, &j, 2560, 1600));
        assert_ne!(base, cache_name("/w/a.jpg", 100, &crate::palette::filter_for("jade", 61, false), 2560, 1600));
        assert_ne!(base, cache_name("/w/a.jpg", 100, &crate::palette::filter_for("golem", 60, false), 2560, 1600));
        assert_ne!(base, cache_name("/w/b.jpg", 100, &j, 2560, 1600));
        assert_ne!(base, cache_name("/w/a.jpg", 101, &j, 2560, 1600));
        assert_ne!(base, cache_name("/w/a.jpg", 100, &j, 1920, 1080));
    }

    #[test]
    fn what_follows_or_moves_is_never_tinted() {
        let f = jade();
        assert!(tintable("/home/x/Pictures/Wallpapers/a.jpg", &f));
        assert!(!tintable("/home/x/Pictures/Wallpapers/a.mp4", &f));
        assert!(!tintable("/home/x/Pictures/Wallpapers/a.gif", &f));
        assert!(!tintable("/home/x/Pictures/Wallpapers/a.jpg", &crate::palette::filter_for("jade", 0, false)));
        std::env::set_var("XDG_CACHE_HOME", "/tmp/wt-cache");
        assert!(!tintable("/tmp/wt-cache/waverunner/palette-walls/a-1x1.png", &f), "the colours' own picture");
        assert!(!tintable("/tmp/wt-cache/waverunner/palette-cards/a.png", &f), "the first tile");
        assert!(!tintable("/tmp/wt-cache/waverunner/tinted-walls/a.png", &f), "a copy is never tinted again");
    }

    #[test]
    fn the_saved_wallpaper_is_never_a_copy() {
        std::env::set_var("XDG_CACHE_HOME", "/tmp/wt-cache");
        assert!(!saveable("/tmp/wt-cache/waverunner/tinted-walls/0123-2560x1600.png"));
        assert!(saveable("/home/x/Pictures/Wallpapers/a.jpg"));
    }

    #[test]
    fn a_tiles_key_carries_the_filter_and_reads_back() {
        let f = jade();
        let dir = crate::controls::wallpaper_dir();
        let file = dir.join("a.jpg").to_string_lossy().into_owned();
        let key = tile_key(&file, &f);
        assert!(key.starts_with(KEY) && key.ends_with("/a.jpg"), "{key}");
        let Some(Ok((back, orig))) = parse_key(&key, &f) else { panic!("not read back: {key}") };
        assert_eq!((back, orig.as_str()), (f, file.as_str()));
        // A slider that moved on: its tile is stale.
        assert!(matches!(parse_key(&key, &crate::palette::filter_for("jade", 30, false)), Some(Err(()))));
        // Off, a moving picture, the first tile: the file as it is.
        assert_eq!(tile_key(&file, &crate::palette::filter_for("jade", 0, false)), file);
        let gif = dir.join("a.gif").to_string_lossy().into_owned();
        assert_eq!(tile_key(&gif, &f), gif);
        assert_eq!(tile_key("/elsewhere/a.jpg", &f), "/elsewhere/a.jpg");
        assert!(parse_key(&file, &f).is_none());
    }

    #[test]
    fn a_thumbnail_is_tinted_and_keeps_its_clear_corners() {
        let n = crate::apps::ICON_SIZE as usize;
        let mut base = vec![0u8; n * n * 4];
        for p in base.chunks_exact_mut(4).skip(n) {
            p.copy_from_slice(&[128, 128, 128, 255]);
        }
        let chain = crate::apps::with_mips(base);
        let len = chain.len();
        let out = filter_thumb(chain, &jade());
        assert_eq!(out.len(), len);
        assert_eq!(&out[..4], &[0, 0, 0, 0], "clear stays clear");
        let px = &out[(n + 5) * 4..(n + 5) * 4 + 4];
        assert_eq!(px[3], 255);
        assert!(px[..3].iter().max() != px[..3].iter().min(), "{px:?}");
    }

    #[test]
    fn a_copy_covers_the_screen_and_never_grows() {
        assert_eq!(copy_size(6000, 4000, 2560, 1600), (2560, 1707));
        assert_eq!(copy_size(4000, 6000, 2560, 1600), (2560, 3840));
        assert_eq!(copy_size(1920, 1080, 2560, 1600), (1920, 1080));
    }

    /// Samples for a human: `cargo test -- --ignored render_samples`.
    #[test]
    #[ignore]
    fn render_samples() {
        let dir = Path::new("/home/max/Pictures/Wallpapers");
        let pick = dir.join("graham-holtshausen-fUnfEz3VLv4-unsplash.jpg");
        let src = if pick.is_file() {
            pick
        } else {
            std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "jpg")).expect("a jpg")
        };
        let out = Path::new("/tmp/claude-1000/walltint-samples");
        std::fs::create_dir_all(out).unwrap();
        for level in [30u8, 60, 100] {
            let t = std::time::Instant::now();
            let (px, w, h) = render(src.to_str().unwrap(), &crate::palette::filter_for("jade", level, false), (1600, 1)).expect("rendered");
            println!("level {level}: {w}x{h} in {:?}", t.elapsed());
            image::save_buffer(out.join(format!("jade-{level}.png")), &px, w, h, image::ColorType::Rgb8).unwrap();
        }
        image::open(&src).unwrap().resize(1600, 1000, image::imageops::FilterType::Triangle).save(out.join("original.png")).unwrap();
    }
}

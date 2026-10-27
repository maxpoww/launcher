//! QUICK LOOK (Max, 2026-10-16, the Quick Look Lab —
//! https://claude.ai/artifact/MGRESgAmUyjC6rq1GL3ytn): Space on a thing in a
//! file window shows it, big, over everything, GROWING OUT OF ITS ICON; the
//! arrows walk to its neighbours with it up; Space or Escape put it back into
//! its icon. "Like Finder or better", for as many kinds as can be, VERY fast
//! even on weak machines, and ONE look for every kind.
//!
//! One frame for every kind: a head (the thing's icon, its name, "kind ·
//! size · dimensions", "Open with <app>", a ×) and a body. What fills the
//! body is drawn by us; what reads a kind (`peek_kinds.rs`) only hands over
//! pixels, text or facts:
//!
//! - a PICTURE (what `image` reads, SVG, a camera's RAW by the JPEG it
//!   carries; and a PDF's page, a video's frame, a font's sheet, what a
//!   thumbnailer draws) is made at the size it will be shown and cut in TILES
//!   of the icon array (`TILE` square, a texel of overlap round each so they
//!   meet without a seam), drawn clipped to the frame's corner (`icon.wgsl`'s
//!   clipped-picture mode) — no pipeline of its own. A PDF is turned a page
//!   at a time (PgUp/PgDn, the wheel), each page prepared on its own;
//! - a SONG is its cover, its names, its loudness along the way and a play
//!   button (`mpv`, stopped when Quick Look goes or moves on);
//! - TEXT (what reads as text, an archive's list) is its lines, in the bar's
//!   mono font, scrolled by the wheel and PgUp/PgDn;
//! - anything else is its picture from the file window and what it is.
//!
//! FAST: the frame shows at once with the window's own thumbnail blown up,
//! and the sharp one fades in when it is ready. It is usually ready already:
//! what the keys are on in a file window, and its neighbours, are PREPARED
//! in the background (`peek_want`) — one low-priority thread, the newest
//! wish first, the stale ones dropped — and kept within a budget (`BUDGET`).
//!
//! It is drawn on the desktop's top surface (`desktop_top.rs`: the Overlay
//! layer, the whole output), which takes the pointer while it is up: a
//! click off the frame closes it. The KEYBOARD stays with the file window
//! (as Finder's does), which hands Quick Look its keys (`peek_key`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use smithay_client_toolkit::reexports::calloop;
use tracing::{debug, info, warn};

use crate::apps::{with_mips, ICON_SIZE};
use crate::content::{
    GridContent, IconInst, Label, Rect, RectInst, Scene, ShadowInst, FONT_BOLD, WINDOW_CORNER,
};
use crate::i18n::{fill, tr};
use crate::peek_kinds as kinds;
use crate::App;

/// What `Up::win` is when Quick Look came from the DESKTOP's icons (a file
/// window's id never gets there).
pub(crate) const DESKTOP: u32 = u32::MAX;
/// …and from a recent of the DOCK BOX's lone app (`app_actions.rs`).
pub(crate) const BOX: u32 = u32::MAX - 1;

/// A tile is one layer of the icon array; it carries `STEP` texels of the
/// picture and the one beyond each edge again, so neighbours overlap.
pub(crate) const TILE: u32 = ICON_SIZE;
pub(crate) const STEP: u32 = TILE - 2;
/// The most tiles a picture is cut into: a bigger one is made smaller.
pub(crate) const TILES: u32 = 64;
/// The layers on the top surface: the tiles, the soft first picture, the
/// head's icon.
pub(crate) const LAYERS: u32 = 2 * TILES + 2;
/// A zoomed photo's sharp part (`peek_zoom`) has tiles of its own.
const DETAIL: u32 = TILES;
const SOFT: u32 = 2 * TILES;
const BADGE: u32 = 2 * TILES + 1;
/// What is kept prepared, at most.
const BUDGET: usize = 96 << 20;

/// What may be kept prepared: `BUDGET` at least, a 24th of the machine's
/// memory up to 256 MB. A picture is kept as 256-texel tiles (a poster ~7 MB,
/// a moving GIF up to 48 MB): at 96 MB, walking a folder of videos on a 4 GB
/// laptop let a neighbour go and prepared it again — a 4K poster twice in a
/// row, 8 s each, on the 2013 MacBook (2026-10-17).
fn budget() -> usize {
    static IT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *IT.get_or_init(|| {
        let total = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|m| {
                m.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|kb| kb.parse::<usize>().ok())
            })
            .map_or(0, |kb| kb * 1024);
        (total / 24).clamp(BUDGET, 256 << 20)
    })
}
/// Of the screen, at most.
const MAX: f32 = 0.82;
pub(crate) const HEAD: f32 = 46.0;
/// How long the frame takes to grow out of the icon (and to go back).
const GROW_SECS: f32 = 0.22;
/// The sharp picture fading in over the soft one.
const SHARP_SECS: f32 = 0.12;
/// Text: how much is read, and how it is set.
const TEXT_BYTES: usize = 256 * 1024;
const TEXT_LINES: usize = 3000;
const TEXT_PX: f32 = 13.0;
const TEXT_LINE: f32 = 19.0;
const TEXT_PAD: f32 = 18.0;
/// A song's frame, and its loudness in so many steps.
const SONG: (f32, f32) = (720.0, 280.0);
/// How much taller a song's frame grows while it is cut: the cutting line
/// goes UNDER the song, not over its cover.
const TRIM_ROOM: f32 = 108.0;
/// How much of the room a small moving picture grows to.
const MOVING_SHARE: f32 = 0.6;
const WAVE: usize = 120;
/// How far the wheel goes for a page of a PDF.
const WHEEL_PAGE: f32 = 60.0;
/// How long a word like "Saved" stays.
const NOTICE_SECS: f32 = 3.5;

/// What is prepared: a thing, and which page of it (0 for all but a PDF).
type Key = (PathBuf, u32);

/// A picture of `w`×`h` texels, cut in `cols`×`rows` tiles (their mip
/// chains, row by row).
pub(crate) struct Pic {
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) cols: u32,
    pub(crate) rows: u32,
    pub(crate) tiles: Vec<Vec<u8>>,
}

impl Pic {
    fn of(img: &image::RgbaImage) -> Self {
        let (cols, rows, tiles) = cut(img);
        Self {
            w: img.width(),
            h: img.height(),
            cols,
            rows,
            tiles,
        }
    }

    /// A picture already premultiplied, in tiles.
    pub(crate) fn of_premultiplied(img: &image::RgbaImage) -> Self {
        Self::of(img)
    }

    fn bytes(&self) -> usize {
        self.tiles.iter().map(Vec::len).sum()
    }
}

/// What a thing is shown as, once prepared.
pub(crate) enum Body {
    Picture(Pic),
    /// A song: its cover, its title, artist · album, length, loudness.
    Song {
        cover: Option<Pic>,
        title: String,
        sub: String,
        secs: f32,
        wave: Vec<f32>,
    },
    /// Lines of text.
    Text {
        lines: Vec<String>,
    },
    /// What it is: name → value.
    Info {
        lines: Vec<(&'static str, String)>,
    },
}

pub(crate) struct Prepared {
    pub body: Body,
    /// The head's second line: "JPEG · 4.2 MB · 6000 × 4000".
    pub meta: String,
    /// The app that opens it (a `.desktop` id), as the system says.
    pub app: Option<String>,
    /// A PDF's pages (0: not paged).
    pub pages: u32,
    /// A video's length (s) and own size: it plays.
    pub video: Option<(f32, (u32, u32))>,
    /// Text read WHOLE (the pencil may write it back).
    pub whole_text: bool,
    /// A photo's own size (what zooming in reads it back at).
    pub own: (u32, u32),
    /// A moving picture's frames (an animated GIF or WebP): played round,
    /// over its first frame.
    pub anim: Option<Arc<kinds::Anim>>,
    /// The file as it was read (when it last changed, its size): a file
    /// changed since — by another program — is read again.
    stamp: Option<(std::time::SystemTime, u64)>,
    /// Code's colours, a line at a time (`peek_syntax`; empty: plain).
    syntax: Vec<Vec<crate::peek_syntax::Span>>,
    /// A long page (a web page, a document, a table): which run of it this
    /// picture is (`peek_web`); the frame scrolls through the runs.
    pub(crate) long: Option<crate::peek_web::Long>,
    bytes: usize,
}

/// What plays: a song (`mpv`), or a video (`peek_kinds::Stream`, its
/// frames tagged with `tag`).
enum Playing {
    Song(std::process::Child),
    Video { stream: kinds::Stream, tag: u64 },
}

/// A song or a video playing.
struct Play {
    what: Playing,
    /// When it started, and from where in it.
    since: Instant,
    from: f32,
}

/// What is up.
pub(crate) struct Up {
    /// The file window it came from (its keys come here).
    pub win: u32,
    pub path: PathBuf,
    /// The page shown (a PDF's).
    page: u32,
    pub(crate) name: String,
    /// Its place among several selected, and how many.
    of: Option<(usize, usize)>,
    /// The thing's picture in its window (the soft first frame, the badge).
    thumb: Option<Vec<u8>>,
    /// Its icon on the screen: where it grows from and goes back to.
    from: Rect,
    /// Where this motion starts, and how far along it is (0..1).
    pub(crate) start: Rect,
    pub(crate) t: f32,
    pub(crate) closing: bool,
    /// The frame as last drawn.
    pub(crate) rect: Rect,
    /// The sharp body over the soft one (0..1).
    sharp: f32,
    /// Text scrolled by; the wheel gathered toward a page.
    scroll: f32,
    wheel: f32,
    /// The pointer on the surface, for the buttons' wash.
    pub(crate) ptr: Option<(f32, f32)>,
    play: Option<Play>,
    /// Where a stopped song or video is.
    pub(crate) at: f32,
    /// A video's newest frame not yet on the screen, how many it has shown,
    /// and their size.
    frame: Option<Vec<u8>>,
    pub(crate) frames: u32,
    /// Frames come in since it last started (re)playing.
    frames_of_play: u32,
    frame_size: (u32, u32),
    /// THE PENCIL taken (`peek_editor`).
    pub(crate) edit: Option<crate::peek_editor::Edit>,
    /// A word for a moment ("Saved"), since when.
    pub(crate) notice: Option<(String, Instant)>,
    /// THE PENCIL on text (`peek_text`), on a video or a song (`peek_trim`).
    pub(crate) text_edit: Option<crate::peek_text::TextEdit>,
    pub(crate) trim_edit: Option<crate::peek_trim::TrimEdit>,
    /// A photo zoomed and moved (`peek_zoom`).
    pub(crate) zoom: crate::peek_zoom::Zoom,
    /// A moving picture: how far into it (ms), and the frame shown.
    anim_ms: f32,
    anim_frame: Option<usize>,
    /// What the head says instead of the file's (`Thing::said`).
    pub(crate) said: Option<Said>,
    /// A page's button the pointer rests on, and since when: its tooltip
    /// (`peek_page.rs`).
    pub(crate) tip: Option<(crate::peek_page::Button, Instant)>,
    /// One tooltip has shown: the next show at once, while the pointer is
    /// on the head (the dock's names' way).
    pub(crate) tips_warm: bool,
}

impl Up {
    fn key(&self) -> Key {
        (self.path.clone(), self.page)
    }

    /// Whether it is a page shown by its picture (`Thing::said`).
    pub(crate) fn said_as_page(&self) -> bool {
        self.said.is_some()
    }

    /// The page shown (a PDF's; 0 otherwise).
    pub(crate) fn page_shown(&self) -> u32 {
        self.page
    }

    pub(crate) fn stop(&mut self) {
        if let Some(p) = self.play.take() {
            self.at = p.from + p.since.elapsed().as_secs_f32();
            match p.what {
                Playing::Song(child) => crate::peek_kinds::reap(Some(child)),
                Playing::Video { mut stream, .. } => stream.stop(),
            }
        }
    }

    /// A slideshow's photo comes up from black (its sharp body fades in).
    pub(crate) fn sharp_from_black(&mut self) {
        self.sharp = 0.0;
    }

    /// Where the song is, in seconds.
    pub(crate) fn song_at(&self) -> f32 {
        self.play
            .as_ref()
            .map_or(self.at, |p| p.from + p.since.elapsed().as_secs_f32())
    }
}

impl Drop for Up {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Job {
    key: Key,
    /// The most texels a picture is made at.
    max: (u32, u32),
    /// The ink of what is drawn here (a font's sheet), sRGB.
    ink: [u8; 3],
}

/// The worker's queue: the newest wish is taken first.
#[derive(Default)]
struct Queue {
    jobs: Mutex<Vec<Job>>,
    ready: Condvar,
}

#[derive(Default)]
pub(crate) struct QuickLook {
    pub(crate) up: Option<Up>,
    cache: HashMap<Key, Arc<Prepared>>,
    /// Oldest first, for the budget.
    order: Vec<Key>,
    bytes: usize,
    queue: Option<Arc<Queue>>,
    /// Asked of the worker and not back yet.
    asked: HashSet<Key>,
    /// For each file window, the thing its keys were on when its
    /// neighbours were last wished for.
    seen: HashMap<u32, String>,
    /// Where a video's frames come in (`peek_frame`), and the tag of the
    /// newest started (frames of an older one are dropped).
    frames: Option<calloop::channel::Sender<(u64, Vec<u8>)>>,
    pub(crate) tag: u64,
    /// The editor's threads answer here (`peek_editor::Msg`).
    pub(crate) edits: Option<calloop::channel::Sender<crate::peek_editor::Msg>>,
    /// A zoomed photo's sharp parts come in here (`peek_zoom`).
    pub(crate) zooms: Option<calloop::channel::Sender<(u64, Pic, [f32; 4])>>,
    /// The mono font's width of a character at the text's size (measured
    /// once the top surface's renderer is at hand).
    pub(crate) char_w: f32,
    /// Code's coloured pieces placed: for a line and where it is cut, where
    /// each piece begins and ends (measured in the font: no font is sure to
    /// be monospaced). Keyed by the words, so it holds while text is written.
    code_x: HashMap<(String, Vec<u16>), Vec<f32>>,
    /// FILES' "Play" and "Slideshow" (`peek_slides.rs`): what plays one
    /// after another, and the photos shown full screen.
    pub(crate) playlist: Option<crate::peek_slides::Playlist>,
    pub(crate) slides: Option<crate::peek_slides::Slides>,
}

impl QuickLook {
    /// A file window is gone: what it was on is forgotten.
    pub(crate) fn forget_window(&mut self, win: u32) {
        self.seen.remove(&win);
    }

    pub(crate) fn is_up(&self) -> bool {
        self.up.is_some()
    }

    fn get(&mut self, key: &Key) -> Option<Arc<Prepared>> {
        let p = self.cache.get(key)?.clone();
        if let Some(i) = self.order.iter().position(|o| o == key) {
            let o = self.order.remove(i);
            self.order.push(o);
        }
        Some(p)
    }

    fn keep(&mut self, key: Key, p: Prepared) {
        self.bytes += p.bytes;
        if let Some(old) = self.cache.insert(key.clone(), Arc::new(p)) {
            self.bytes -= old.bytes;
            self.order.retain(|o| o != &key);
        }
        self.order.push(key);
        // (Never what is up.)
        let up = self.up.as_ref().map(|u| u.path.clone());
        while self.bytes > budget() && self.order.len() > 1 {
            let Some(i) = self.order.iter().position(|o| Some(&o.0) != up.as_ref()) else {
                break;
            };
            let o = self.order.remove(i);
            if let Some(gone) = self.cache.remove(&o) {
                self.bytes -= gone.bytes;
            }
        }
    }
}

// ── preparing (on the worker) ───────────────────────────────────────────

fn is_picture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    crate::thumb_more::is_raw(&lower)
        || lower.ends_with(".svg")
        // (What `image` reads, also where no MIME database is installed.)
        || [".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".tif", ".tiff", ".avif", ".heic", ".heif", ".jxl", ".ico", ".tga", ".qoi", ".pnm", ".ppm"]
            .iter()
            .any(|e| lower.ends_with(e))
        || crate::thumb_more::mime_of(&lower).is_some_and(|m| m.starts_with("image/"))
}

/// The ending, shouted: what a kind is called in the head ("JPEG", "RS").
fn kind_word(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 5 => {
            ext.to_ascii_uppercase()
        }
        _ => tr("File").to_owned(),
    }
}

/// The app that opens a kind — asked once a kind (`xdg-mime` is a shell
/// script: a tenth of a second and more each time).
pub(crate) fn default_app(mime: &str) -> Option<String> {
    thread_local! {
        static KNOWN: std::cell::RefCell<HashMap<String, Option<String>>> = std::cell::RefCell::new(HashMap::new());
    }
    if let Some(known) = KNOWN.with(|k| k.borrow().get(mime).cloned()) {
        return known;
    }
    let asked = ask_default_app(mime);
    KNOWN.with(|k| k.borrow_mut().insert(mime.to_owned(), asked.clone()));
    asked
}

fn ask_default_app(mime: &str) -> Option<String> {
    let out = std::process::Command::new("xdg-mime")
        .args(["query", "default", mime])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!id.is_empty()).then(|| id.trim_end_matches(".desktop").to_owned())
}

/// Premultiply in place.
fn premultiply(img: &mut image::RgbaImage) {
    for px in img.pixels_mut() {
        let a = u32::from(px[3]);
        if a < 255 {
            for c in 0..3 {
                px[c] = (u32::from(px[c]) * a / 255) as u8;
            }
        }
    }
}

/// Decode a picture no bigger than `max` texels, premultiplied RGBA, and
/// its own size.
fn decode(path: &Path, max: (u32, u32)) -> Option<(image::RgbaImage, (u32, u32))> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".svg") {
        let data = std::fs::read(path).ok()?;
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let (w, h) = (tree.size().width(), tree.size().height());
        let k = (max.0 as f32 / w).min(max.1 as f32 / h);
        let (pw, ph) = (
            (w * k).round().max(1.0) as u32,
            (h * k).round().max(1.0) as u32,
        );
        let mut pixmap = resvg::tiny_skia::Pixmap::new(pw, ph)?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(k, k),
            &mut pixmap.as_mut(),
        );
        // (Already premultiplied.)
        return Some((
            image::RgbaImage::from_raw(pw, ph, pixmap.take())?,
            (w.round() as u32, h.round() as u32),
        ));
    }
    if crate::thumb_more::is_raw(&name) {
        // The JPEG a RAW carries (no RAW is developed here).
        let got = std::cell::RefCell::new(None);
        crate::thumb_more::by_exiftool(&path.to_string_lossy(), |jpg| {
            *got.borrow_mut() = decode(Path::new(jpg), max);
            got.borrow().is_some().then(Vec::new)
        });
        return got.into_inner();
    }
    // A JPEG: decoded small straight away (the IDCT shrinks it).
    if name.ends_with(".jpg") || name.ends_with(".jpeg") {
        if let Some((mut img, own)) = kinds::jpeg_scaled(path, max) {
            premultiply(&mut img);
            return Some((img, own));
        }
    }
    // (Turned as its camera says.)
    let Some(img) = kinds::open_oriented(path) else {
        // JPEG XL: libjxl's own decoder, whole, then brought to size.
        if name.ends_with(".jxl") {
            let img = kinds::jxl_picture(path)?;
            let own = img.dimensions();
            let (w, h) = tile_fit(own, max);
            let mut img = if (w, h) == own {
                img
            } else {
                image::imageops::thumbnail(&img, w, h)
            };
            premultiply(&mut img);
            return Some((img, own));
        }
        // What `image` cannot read — a phone's HEIC — FFMPEG can, in one
        // run (its own size read from what it says), then within the tiles.
        let (img, own) = kinds::ffmpeg_picture(path, max)?;
        let (w, h) = tile_fit(img.dimensions(), max);
        let mut img = if (w, h) == img.dimensions() {
            img
        } else {
            image::imageops::thumbnail(&img, w, h)
        };
        premultiply(&mut img);
        return Some((img, own));
    };
    let own = (img.width(), img.height());
    // (`thumbnail`: the quick reduction; a picture smaller stays as it is.)
    let img = if own.0 > max.0 || own.1 > max.1 {
        img.thumbnail(max.0, max.1)
    } else {
        img
    };
    let mut rgba = img.to_rgba8();
    premultiply(&mut rgba);
    Some((rgba, own))
}

/// The most texels that cut into `TILES` tiles at `max`'s shape.
pub(crate) fn tile_fit(own: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let (mut w, mut h) = (own.0.min(max.0) as f32, own.1.min(max.1) as f32);
    let k = (w / own.0 as f32).min(h / own.1 as f32);
    w = own.0 as f32 * k;
    h = own.1 as f32 * k;
    while (w / STEP as f32).ceil() * (h / STEP as f32).ceil() > TILES as f32 {
        w *= 0.94;
        h *= 0.94;
    }
    (w.max(1.0) as u32, h.max(1.0) as u32)
}

/// A picture made elsewhere brought within the tiles.
fn within_tiles(img: image::RgbaImage) -> image::RgbaImage {
    let (w, h) = tile_fit(img.dimensions(), img.dimensions());
    if (w, h) == img.dimensions() {
        img
    } else {
        image::imageops::thumbnail(&img, w, h)
    }
}

/// Cut a picture in tiles: tile (c, r) holds texels from `c·STEP − 1` and
/// `r·STEP − 1`, the edge texel repeated past the picture's edge, nothing
/// beyond that.
fn cut(img: &image::RgbaImage) -> (u32, u32, Vec<Vec<u8>>) {
    let (w, h) = img.dimensions();
    let (cols, rows) = (w.div_ceil(STEP), h.div_ceil(STEP));
    let mut tiles = Vec::with_capacity((cols * rows) as usize);
    for r in 0..rows {
        for c in 0..cols {
            let mut base = vec![0u8; (TILE * TILE * 4) as usize];
            for ty in 0..TILE {
                let y = (r * STEP + ty) as i64 - 1;
                if y > h as i64 {
                    break;
                }
                let y = y.clamp(0, h as i64 - 1) as u32;
                for tx in 0..TILE {
                    let x = (c * STEP + tx) as i64 - 1;
                    if x > w as i64 {
                        break;
                    }
                    let x = x.clamp(0, w as i64 - 1) as u32;
                    let at = ((ty * TILE + tx) * 4) as usize;
                    base[at..at + 4].copy_from_slice(&img.get_pixel(x, y).0);
                }
            }
            tiles.push(with_mips(base));
        }
    }
    (cols, rows, tiles)
}

fn text_of(path: &Path) -> Option<Vec<String>> {
    crate::thumb_more::text_head(&path.to_string_lossy())?;
    let mut buf = Vec::new();
    use std::io::Read;
    std::fs::File::open(path)
        .ok()?
        .take(TEXT_BYTES as u64)
        .read_to_end(&mut buf)
        .ok()?;
    let text = String::from_utf8_lossy(&buf);
    Some(
        text.lines()
            .take(TEXT_LINES)
            .map(|l| {
                let l = l.replace('\t', "    ");
                if l.chars().count() > 400 {
                    l.chars().take(400).collect()
                } else {
                    l
                }
            })
            .collect(),
    )
}

/// A folder's things, a line each ("    4.1 MB  photo.jpg", a folder with
/// a `/`), folders first, then by name; how many there are. `None` for an
/// empty folder (it is shown as what it is).
fn folder_list(path: &Path) -> Option<(Vec<String>, usize)> {
    // (All of them counted; the first `TEXT_LINES` listed.)
    let mut n = 0;
    let mut things: Vec<(bool, String, u64)> = std::fs::read_dir(path)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .inspect(|_| n += 1)
        .take(TEXT_LINES)
        .map(|e| {
            let meta = e.metadata().ok();
            let dir = meta.as_ref().is_some_and(std::fs::Metadata::is_dir);
            (
                dir,
                e.file_name().to_string_lossy().into_owned(),
                meta.map_or(0, |m| m.len()),
            )
        })
        .collect();
    if things.is_empty() {
        return None;
    }
    things.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let lines = things
        .into_iter()
        .map(|(dir, name, size)| {
            if dir {
                format!("{:>10}  {name}/", "")
            } else {
                format!("{:>10}  {name}", crate::desktop_props::size_text(size))
            }
        })
        .collect();
    Some((lines, n))
}

/// Whether the file can be written in the pencil's text editor: small, and
/// every line whole (what is written back is exactly what was read).
fn written_whole(path: &Path, size: u64) -> bool {
    size <= TEXT_BYTES as u64
        && text_of(path).is_some_and(|lines| {
            lines.len() < TEXT_LINES && lines.iter().all(|l| l.chars().count() < 400)
        })
}

fn picture(img: image::RgbaImage, meta: String, app: Option<String>, pages: u32) -> Prepared {
    let pic = Pic::of(&within_tiles(img));
    let bytes = pic.bytes();
    Prepared {
        body: Body::Picture(pic),
        meta,
        app,
        pages,
        video: None,
        whole_text: false,
        own: (0, 0),
        anim: None,
        stamp: None,
        syntax: Vec::new(),
        long: None,
        bytes,
    }
}

/// Make page `page` of `path` ready to be shown.
/// When `path` last changed, and its size.
fn stamp_of(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn prepare(path: &Path, page: u32, max: (u32, u32), ink: [u8; 3]) -> Prepared {
    let started = Instant::now();
    let stamp = stamp_of(path);
    let mut made = Prepared {
        stamp,
        ..prepare_kind(path, page, max, ink)
    };
    // (Code: its colours, read here with it — nothing on the loop.)
    if let (Body::Text { lines }, Some(lang)) = (&made.body, crate::peek_syntax::lang_of(path)) {
        if path.is_file() {
            made.syntax = crate::peek_syntax::spans(lines, lang);
        }
    }
    debug!(
        "quick look: {} (page {page}) in {:?}",
        path.display(),
        started.elapsed()
    );
    made
}

fn prepare_kind(path: &Path, page: u32, max: (u32, u32), ink: [u8; 3]) -> Prepared {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let meta = std::fs::metadata(path).ok();
    let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
    let dir = meta.as_ref().is_some_and(std::fs::Metadata::is_dir);
    let mime = if dir {
        Some("inode/directory")
    } else {
        crate::thumb_more::mime_of(&name)
    };
    let app = mime.and_then(default_app);
    let kind = kind_word(&name);
    let size_text = crate::desktop_props::size_text(size);
    if !dir {
        // A MOVING PICTURE: its frames, its first one as the picture.
        if let Some(anim) = kinds::animation(path, max) {
            let (w, h) = anim.size;
            if let Some(first) = image::RgbaImage::from_raw(w, h, anim.frames[0].0.clone()) {
                let own = image::image_dimensions(path).unwrap_or((w, h));
                let n = anim.frames.len();
                let bytes = anim.frames.iter().map(|f| f.0.len()).sum::<usize>();
                // (Its frames are premultiplied already.)
                let meta = format!(
                    "{kind} · {size_text} · {} × {} · {}",
                    own.0,
                    own.1,
                    fill(tr("{n} frames"), &[("n", &n)])
                );
                let p = picture(first, meta, app.clone(), 0);
                return Prepared {
                    anim: Some(Arc::new(anim)),
                    bytes: p.bytes + bytes,
                    ..p
                };
            }
        }
        // A WEB PAGE: as Seam shows it (`peek_web`); its source under the
        // pencil. (Not drawn: as text, as before.)
        // So are Markdown (formatted) and a table of values (a table).
        // (Not drawn yet: as text now, drawn off the way — `peek_page_watch`
        // swaps it in when it is there.)
        let page_now = crate::peek_web::drawn(path, &name, page, false);
        if page_now.is_none() && crate::peek_web::is_drawn(&name) {
            crate::peek_web::draw_soon(path, &name);
        }
        if let Some((img, long, words)) = page_now {
            {
                return Prepared {
                    own: (long.w, long.h),
                    long: Some(long),
                    whole_text: written_whole(path, size),
                    ..picture(img, format!("{kind} · {size_text} · {words}"), app, 0)
                };
            }
        }
        if is_picture(&name) {
            // (Its own size first: the tiles decide how big it is decoded.)
            let own = image::image_dimensions(path).unwrap_or(max);
            if let Some((img, own)) = decode(path, tile_fit(own, max)) {
                return Prepared {
                    own,
                    // (An SVG is drawn; under the pencil it is its text.)
                    whole_text: name.to_ascii_lowercase().ends_with(".svg")
                        && written_whole(path, size),
                    ..picture(
                        img,
                        format!("{kind} · {size_text} · {} × {}", own.0, own.1),
                        app,
                        0,
                    )
                };
            }
        }
        if kinds::is_pdf(&name) {
            if let Some((pages, aspect)) = kinds::pdf_facts(path) {
                if let Some(img) =
                    kinds::pdf_page(path, page.min(pages.saturating_sub(1)), aspect, max)
                {
                    let meta = format!(
                        "PDF · {size_text} · {}",
                        fill(tr("{n} pages"), &[("n", &pages)])
                    );
                    return picture(img, meta, app, pages);
                }
            }
        }
        if kinds::is_video(&name) {
            if let Some(probe) = kinds::probe(path) {
                if let Some(img) = kinds::frame(path, probe.secs * 0.1, max) {
                    let dims = probe
                        .size
                        .map_or(String::new(), |(w, h)| format!(" · {w} × {h}"));
                    let meta = format!("{kind} · {size_text}{dims} · {}", kinds::clock(probe.secs));
                    let own = probe.size.unwrap_or((img.width(), img.height()));
                    return Prepared {
                        video: Some((probe.secs, own)),
                        ..picture(img, meta, app, 0)
                    };
                }
            }
        }
        if kinds::is_audio(&name) {
            if let Some(probe) = kinds::probe(path) {
                let cover =
                    kinds::frame(path, 0.0, (600, 600)).map(|img| Pic::of(&within_tiles(img)));
                let wave = kinds::wave(path, WAVE).unwrap_or_default();
                let title = probe.title.clone().unwrap_or_else(|| {
                    name.rsplit_once('.')
                        .map_or(name.clone(), |(s, _)| s.to_owned())
                });
                let sub = [probe.artist.clone(), probe.album.clone()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                let bytes = cover.as_ref().map_or(0, Pic::bytes) + wave.len() * 4;
                return Prepared {
                    body: Body::Song {
                        cover,
                        title,
                        sub,
                        secs: probe.secs,
                        wave,
                    },
                    meta: format!("{kind} · {size_text} · {}", kinds::clock(probe.secs)),
                    app,
                    pages: 0,
                    video: None,
                    whole_text: false,
                    own: (0, 0),
                    anim: None,
                    stamp: None,
                    syntax: Vec::new(),
                    long: None,
                    bytes,
                };
            }
        }
        if crate::thumb_more::is_font(&name) {
            if let Some((img, family)) = kinds::font_sheet(path, max, ink) {
                return picture(img, format!("{kind} · {size_text} · {family}"), app, 0);
            }
        }
        if kinds::is_archive(&name) {
            if let Some((lines, files, total)) = kinds::archive_list(path) {
                let bytes = lines.iter().map(String::len).sum();
                let meta = format!(
                    "{kind} · {size_text} · {} · {}",
                    fill(tr("{n} files"), &[("n", &files)]),
                    fill(
                        tr("{size} unpacked"),
                        &[("size", &crate::desktop_props::size_text(total))]
                    )
                );
                return Prepared {
                    body: Body::Text { lines },
                    meta,
                    app,
                    pages: 0,
                    video: None,
                    whole_text: false,
                    own: (0, 0),
                    anim: None,
                    stamp: None,
                    syntax: Vec::new(),
                    long: None,
                    bytes,
                };
            }
        }
        // (A picture that would not decode is not offered as text — its
        // header would be, with a pencil to write in it. An SVG is text.)
        let text = if is_picture(&name) && !name.to_ascii_lowercase().ends_with(".svg") {
            None
        } else {
            text_of(path)
        };
        // (JSON on one long line — as programs write it — is laid out.)
        let laid_out = text
            .as_ref()
            .filter(|_| name.to_ascii_lowercase().ends_with(".json") && size <= 4 << 20)
            .filter(|lines| lines.iter().any(|l| l.chars().count() >= 400))
            .and_then(|_| std::fs::read_to_string(path).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .map(|t| {
                t.lines()
                    .take(TEXT_LINES)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            });
        let pretty = laid_out.is_some();
        let text = laid_out.or(text);
        if let Some(lines) = text {
            let bytes = lines.iter().map(String::len).sum();
            let n = lines.len();
            // (Read whole: small enough, no line cut short.)
            // (Laid out, it is not the file as it is: no pencil.)
            let whole = !pretty
                && size <= TEXT_BYTES as u64
                && n < TEXT_LINES
                && lines.iter().all(|l| l.chars().count() < 400);
            let count = if size > TEXT_BYTES as u64 || n >= TEXT_LINES {
                // (Only the start of a long file is read: said so.)
                fill(tr("first {n} lines"), &[("n", &n)])
            } else if n == 1 {
                tr("1 line").to_owned()
            } else {
                fill(tr("{n} lines"), &[("n", &n)])
            };
            let meta = format!("{kind} · {size_text} · {count}");
            return Prepared {
                body: Body::Text { lines },
                meta,
                app,
                pages: 0,
                video: None,
                whole_text: whole,
                own: (0, 0),
                anim: None,
                stamp: None,
                syntax: Vec::new(),
                long: None,
                bytes,
            };
        }
        if let Some(img) = kinds::thumbnailed(path, max) {
            return picture(img, format!("{kind} · {size_text}"), app, 0);
        }
    }
    // A FOLDER: what is in it, folders first, as an archive's list is.
    if dir {
        if let Some((lines, n)) = folder_list(path) {
            let bytes = lines.iter().map(String::len).sum();
            return Prepared {
                body: Body::Text { lines },
                meta: format!(
                    "{} · {}",
                    tr("Folder"),
                    fill(tr("{n} things"), &[("n", &n)])
                ),
                app,
                pages: 0,
                video: None,
                whole_text: false,
                own: (0, 0),
                anim: None,
                stamp: None,
                syntax: Vec::new(),
                long: None,
                bytes,
            };
        }
    }
    let mut lines = Vec::new();
    lines.push((
        tr("Kind"),
        if dir {
            tr("Folder").to_owned()
        } else {
            mime.map_or_else(|| kind.clone(), str::to_owned)
        },
    ));
    if dir {
        match std::fs::read_dir(path) {
            Ok(d) => {
                let n = d
                    .filter_map(Result::ok)
                    .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                    .count();
                lines.push((tr("Contains"), fill(tr("{n} things"), &[("n", &n)])));
            }
            // (Not "0 things": it cannot be looked into.)
            Err(_) => lines.push((tr("Access"), tr("No permission to open it").to_owned())),
        }
    } else if meta.is_some() {
        lines.push((tr("Size"), size_text.clone()));
        if std::fs::File::open(path).is_err() {
            lines.push((tr("Access"), tr("No permission to read it").to_owned()));
        }
    }
    // A link: where to, and whether that is still there.
    if let Ok(to) = std::fs::read_link(path) {
        let mut to = to.to_string_lossy().into_owned();
        if meta.is_none() {
            to = format!("{to} {}", tr("(missing)"));
        }
        lines.push((tr("Link to"), to));
    }
    if let Some(m) = crate::desktop_props::date_text(meta.as_ref().and_then(|m| m.modified().ok()))
    {
        lines.push((tr("Modified"), m));
    }
    if let (Some(parent), Some(home)) = (path.parent(), std::env::var_os("HOME")) {
        lines.push((
            tr("Where"),
            crate::desktop_props::tilde(parent, Path::new(&home)),
        ));
    }
    Prepared {
        body: Body::Info { lines },
        meta: if dir {
            tr("Folder").to_owned()
        } else {
            format!("{kind} · {size_text}")
        },
        app,
        pages: 0,
        video: None,
        whole_text: false,
        own: (0, 0),
        anim: None,
        stamp: None,
        syntax: Vec::new(),
        long: None,
        bytes: 1024,
    }
}

fn spawn(sender: calloop::channel::Sender<(Key, Prepared)>) -> Option<Arc<Queue>> {
    let queue = Arc::new(Queue::default());
    let q = queue.clone();
    let spawned = std::thread::Builder::new()
        .name("quick-look".into())
        .spawn(move || {
            // SAFETY: setpriority on the calling thread only.
            unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10) };
            loop {
                let job = {
                    let Ok(mut jobs) = q.jobs.lock() else { return };
                    while jobs.is_empty() {
                        jobs = match q.ready.wait(jobs) {
                            Ok(j) => j,
                            Err(_) => return,
                        };
                    }
                    jobs.pop()
                };
                let Some(job) = job else { continue };
                let made = prepare(&job.key.0, job.key.1, job.max, job.ink);
                if sender.send((job.key, made)).is_err() {
                    return;
                }
            }
        });
    match spawned {
        Ok(_) => Some(queue),
        Err(e) => {
            warn!("quick look: no worker: {e}");
            None
        }
    }
}

// ── on the loop ─────────────────────────────────────────────────────────

fn lerp(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::new(
        a.x + (b.x - a.x) * t,
        a.y + (b.y - a.y) * t,
        a.w + (b.w - a.w) * t,
        a.h + (b.h - a.h) * t,
    )
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// What a file window says of the thing its keys are on.
#[derive(Clone)]
pub(crate) struct Thing {
    pub path: PathBuf,
    pub name: String,
    pub thumb: Option<Vec<u8>>,
    /// Its icon, on the screen (logical px of the output).
    pub icon: Rect,
    /// Its place among several selected (from 1), and how many: Quick Look
    /// walks only those.
    pub of: Option<(usize, usize)>,
    /// Said as something else than its file: a page's picture.
    pub said: Option<Said>,
}

/// A PAGE shown by its picture (a recent of the dock box's,
/// `app_actions.rs`; drawn by `peek_page.rs`): the head says where it is and
/// Open in its app — for the file's kind and size and its app —, and under
/// the picture its whole title, how it was seen, its address.
#[derive(Clone, Debug)]
pub(crate) struct Said {
    pub meta: String,
    pub open: String,
    pub title: String,
    /// "Watched today at 13:02 · 5 times".
    pub facts: String,
    pub url: String,
    /// It has a picture of its own (else the file is its favicon's tile:
    /// small, beside the words).
    pub picture: bool,
}

/// The picture a body shows in its tiles.
fn pic_of(body: &Body) -> Option<&Pic> {
    match body {
        Body::Picture(p) => Some(p),
        Body::Song { cover, .. } => cover.as_ref(),
        _ => None,
    }
}

impl App {
    /// The most texels a picture is made at: the frame's biggest, in real
    /// pixels.
    pub(crate) fn peek_max(&self) -> (u32, u32) {
        let (w, h) = self.desktop_top.size;
        let scale = self.surface_scale(crate::fractional::SurfaceKind::DesktopTop);
        (
            ((w as f32 * MAX) * scale) as u32,
            (((h as f32 * MAX) - HEAD) * scale) as u32,
        )
    }

    /// Ask for `keys` to be prepared, the last first; what was asked before
    /// and is not among them is dropped.
    pub(crate) fn peek_want(&mut self, keys: Vec<Key>) {
        if !self.desktop_top_ready() {
            return;
        }
        if self.peek.queue.is_none() {
            let (sender, receiver) = calloop::channel::channel();
            let inserted = self
                .loop_handle
                .insert_source(receiver, |event, _, app: &mut App| {
                    if let calloop::channel::Event::Msg((key, made)) = event {
                        app.peek_ready(key, made);
                    }
                });
            if let Err(e) = inserted {
                warn!("quick look: {e}");
                return;
            }
            self.peek.queue = spawn(sender);
        }
        let max = self.peek_max();
        let ink = self.filer_palette().ink;
        let ink = [0, 1, 2].map(|i| (ink[i].clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8);
        let todo: Vec<Key> = keys
            .into_iter()
            .filter(|k| !self.peek.cache.contains_key(k))
            .collect();
        let Some(queue) = self.peek.queue.clone() else {
            return;
        };
        let Ok(mut jobs) = queue.jobs.lock() else {
            return;
        };
        jobs.clear();
        self.peek.asked.retain(|k| todo.contains(k));
        for key in todo {
            self.peek.asked.insert(key.clone());
            jobs.push(Job { key, max, ink });
        }
        queue.ready.notify_one();
    }

    /// Quick Look is gone (gone back into its icon, its window closed, its
    /// surface closed): what it held let go — the playing stopped (`Up`'s
    /// drop), the video's texture, the pointer given back to the windows.
    pub(crate) fn peek_gone(&mut self) {
        self.peek.up = None;
        self.peek.playlist = None;
        self.peek.slides = None;
        if let Some(renderer) = self.desktop_top.renderer.as_mut() {
            renderer.drop_video();
        }
        if self.desktop_top_ready() {
            self.request_desktop_top_draw();
        }
        // (Opened on another screen: the menus' surface goes home.)
        self.sync_parts_home();
    }

    /// What was prepared of `path` is stale (it was written).
    pub(crate) fn peek_forget(&mut self, path: &Path) {
        let gone: Vec<Key> = self
            .peek
            .cache
            .keys()
            .filter(|k| k.0 == path)
            .cloned()
            .collect();
        for k in gone {
            if let Some(p) = self.peek.cache.remove(&k) {
                self.peek.bytes -= p.bytes;
            }
            self.peek.order.retain(|o| o != &k);
        }
        // (A reading of it already on its way is of the old file: asked again.)
        self.peek.asked.retain(|k| k.0 != path);

        self.desktop_top.peek_tiles = None;
    }

    /// Wish for `key` too, ahead of what was wished for.
    pub(crate) fn peek_want_now(&mut self, key: Key) {
        if self.peek.cache.contains_key(&key) || self.peek.asked.contains(&key) {
            return;
        }
        let mut all: Vec<Key> = self.peek.asked.iter().cloned().collect();
        all.push(key);
        self.peek_want(all);
    }

    fn peek_ready(&mut self, key: Key, made: Prepared) {
        self.peek.asked.remove(&key);
        let shown = self.peek.up.as_ref().is_some_and(|u| u.key() == key);
        let pages = made.pages;
        self.peek.keep(key.clone(), made);
        if shown {
            // Its real shape: the frame goes there from where it is.
            if let Some(up) = self.peek.up.as_mut() {
                up.start = up.rect;
                up.t = 0.0;
                if up.page == 0 {
                    up.sharp = 0.0;
                }
            }
            self.request_desktop_top_draw();
            self.peek_autoplay();
            // (The next page, before it is asked for.)
            if key.1 + 1 < pages {
                self.peek_want_now((key.0, key.1 + 1));
            }
        }
    }

    /// What a file window's keys are on moved: its neighbours are wished for.
    pub(crate) fn peek_follow_keys(&mut self, win: u32, at: &str, near: Vec<PathBuf>) {
        if self.peek.seen.get(&win).is_some_and(|s| s == at) {
            return;
        }
        self.peek.seen.insert(win, at.to_owned());
        self.peek_want(near.into_iter().map(|p| (p, 0)).collect());
    }

    /// Show `thing` of file window `win` (or, if something is up, walk to it).
    /// `path` is a web page, a document or a table not drawn yet: look again
    /// every half second while it is the one up, and when its page is there,
    /// read it again (the text it was meanwhile gives way).
    fn peek_page_watch(&mut self, path: &Path, name: &str) {
        if !crate::peek_web::is_drawn(name) {
            return;
        }
        if crate::peek_web::ready(path, name) {
            // Drawn while it was a NEIGHBOUR, as text then: what is kept is
            // the stand-in — read it again as its page (walking a folder
            // showed the text for good).
            let stand_in = self
                .peek
                .cache
                .iter()
                .any(|(k, p)| k.0 == path && p.long.is_none());
            if stand_in {
                self.peek_forget(path);
            }
            return;
        }
        let (path, name) = (path.to_path_buf(), name.to_owned());
        debug!("quick look: waiting for the page of {}", path.display());
        let started = Instant::now();
        let timer = calloop::timer::Timer::from_duration(std::time::Duration::from_millis(500));
        let _ = self
            .loop_handle
            .insert_source(timer, move |_, _, app: &mut App| {
                let up = app
                    .peek
                    .up
                    .as_ref()
                    .is_some_and(|u| u.path == path && !u.closing);
                if !up || started.elapsed().as_secs() > 60 {
                    debug!("quick look: no more waiting for the page of {}", path.display());
                    return calloop::timer::TimeoutAction::Drop;
                }
                if crate::peek_web::ready(&path, &name) {
                    debug!("quick look: the page of {} is there", path.display());
                    app.peek_forget(&path);
                    app.peek_want_now((path.clone(), 0));
                    app.request_desktop_top_draw();
                    return calloop::timer::TimeoutAction::Drop;
                }
                calloop::timer::TimeoutAction::ToDuration(std::time::Duration::from_millis(500))
            });
    }

    /// What was prepared of `path`, if the file changed since: let go.
    fn peek_fresh(&mut self, path: &Path) {
        // (Not asked of a slow place — a phone over MTP: on the loop, one
        // look there can take seconds while the phone is busy.)
        if crate::filer::slow_mount(path).is_some() {
            return;
        }
        let stale = self
            .peek
            .cache
            .iter()
            .any(|(k, p)| k.0 == path && p.stamp.is_some() && p.stamp != stamp_of(path));
        if stale {
            self.peek_forget(path);
            // (Its little picture in the windows is of the old file too.)
            self.thumb_stale(&path.to_string_lossy());
        }
    }

    pub(crate) fn peek_show(&mut self, win: u32, thing: Thing) {
        // On the screen it is opened from (Max, 2026-10-21: "I open Golem
        // Files on the TV, press Quick Look on a file and it showed on the
        // laptop"): made again there first when it is not (`shell_screens.rs`).
        if self.peek_follow(win, &thing) {
            return;
        }
        if !self.desktop_top_ready() {
            info!("quick look: no top surface");
            return;
        }
        if self
            .peek
            .up
            .as_ref()
            .is_some_and(|u| u.path == thing.path && !u.closing)
        {
            return;
        }
        let key = (thing.path.clone(), 0);
        // (Changed on disk by another program since it was read: read again.)
        self.peek_fresh(&thing.path);
        // A page still being drawn: it takes the text's place when it is.
        self.peek_page_watch(&thing.path, &thing.name);
        // (Shown: the newest of what is kept.)
        let ready = self.peek.get(&key).is_some();
        let was = self.peek.up.as_ref().filter(|u| !u.closing).map(|u| u.rect);
        let start = was.unwrap_or(thing.icon);
        // (A song playing stops as its frame goes: `Up`'s drop.)
        self.peek.up = Some(Up {
            win,
            path: thing.path,
            page: 0,
            name: thing.name,
            of: thing.of,
            thumb: thing.thumb,
            from: thing.icon,
            start,
            t: 0.0,
            closing: false,
            rect: start,
            sharp: if ready { 1.0 } else { 0.0 },
            scroll: 0.0,
            wheel: 0.0,
            ptr: None,
            play: None,
            at: 0.0,
            frame: None,
            frames: 0,
            frames_of_play: 0,
            frame_size: (0, 0),
            edit: None,
            notice: None,
            text_edit: None,
            trim_edit: None,
            zoom: crate::peek_zoom::Zoom::new(),
            anim_ms: 0.0,
            anim_frame: None,
            said: thing.said,
            tip: None,
            tips_warm: false,
        });
        self.desktop_top.peek_tiles = None;
        self.desktop_top.peek_soft = false;
        self.peek_want_now(key);
        // (A video plays as it is shown, as Finder's does.)
        if ready {
            self.peek_autoplay();
        }
        self.request_desktop_top_draw();
    }

    /// Put it back into its icon.
    pub(crate) fn peek_close(&mut self) {
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        if up.closing {
            return;
        }
        up.stop();
        up.closing = true;
        self.peek.playlist = None;
        self.peek.slides = None;
        up.start = up.rect;
        up.t = 0.0;
        self.request_desktop_top_draw();
    }

    /// A page's address on the clipboard, and a word for it.
    pub(crate) fn peek_copy_link(&mut self, url: &str) {
        info!("quick look: link copied");
        self.copy_text(url);
        if let Some(up) = self.peek.up.as_mut() {
            up.notice = Some((tr("Link copied").to_owned(), Instant::now()));
        }
        self.request_desktop_top_draw();
    }

    /// Open it with its app (as a double click would) and close — a recent
    /// of the dock box's, in its own app.
    fn peek_open_app(&mut self) {
        let Some(up) = self.peek.up.as_ref() else {
            return;
        };
        if up.win == BOX {
            self.box_peek_open();
            return;
        }
        let exec = format!(
            "xdg-open {}",
            crate::launch::shell_quote(&up.path.to_string_lossy())
        );
        if let Err(e) = crate::launch::launch(&exec, false, &self.config.launch.terminal) {
            warn!("quick look: opening {} failed: {e:#}", up.path.display());
        }
        self.peek_close();
    }

    /// The prepared thing on screen: the page asked for, else (while it is
    /// made) the one whose tiles are up.
    pub(crate) fn peek_shown(&self) -> Option<(Key, &Arc<Prepared>)> {
        let up = self.peek.up.as_ref()?;
        let key = up.key();
        if let Some(p) = self.peek.cache.get(&key) {
            return Some((key, p));
        }
        let tiles = self
            .desktop_top
            .peek_tiles
            .as_ref()
            .filter(|k| k.0 == up.path)?;
        Some((tiles.clone(), self.peek.cache.get(tiles)?))
    }

    /// Turn a PDF's page by `by`.
    fn peek_turn(&mut self, by: i32) -> bool {
        let pages = self.peek_shown().map_or(0, |(_, p)| p.pages);
        let Some(up) = self.peek.up.as_mut() else {
            return false;
        };
        if pages < 2 {
            return false;
        }
        let page = (up.page as i32 + by).clamp(0, pages as i32 - 1) as u32;
        if page != up.page {
            up.page = page;
            // (Another page: fitted again.)
            up.zoom = crate::peek_zoom::Zoom::new();
            let key = up.key();
            // (The page after first, the page asked for last: the newest
            // wish is the first made.)
            if page + 1 < pages {
                self.peek_want_now((key.0.clone(), page + 1));
            }
            self.peek_want_now(key);
            self.request_desktop_top_draw();
        }
        true
    }

    /// How long the song or video that is up lasts, if it plays.
    fn peek_secs(&self) -> Option<f32> {
        let (_, p) = self.peek_shown()?;
        match &p.body {
            Body::Song { secs, .. } => Some(*secs),
            _ => p.video.map(|(secs, _)| secs),
        }
    }

    /// A video starts as soon as it is shown.
    fn peek_autoplay(&mut self) {
        let video = self.peek_shown().is_some_and(|(_, p)| p.video.is_some());
        // (A song plays as it is shown only in a playlist: Files' "Play".)
        let song = self.peek.playlist.is_some()
            && self
                .peek_shown()
                .is_some_and(|(_, p)| matches!(p.body, Body::Song { .. }));
        if (video || song)
            && self.peek.up.as_ref().is_some_and(|u| {
                u.play.is_none() && u.frames == 0 && !u.closing && u.trim_edit.is_none()
            })
        {
            self.peek_play_from(0.0);
        }
    }

    /// Play or stop the song or the video that is up.
    fn peek_play(&mut self) {
        let Some(secs) = self.peek_secs() else { return };
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        if up.play.is_some() {
            up.stop();
            self.request_desktop_top_draw();
        } else {
            let from = if up.at >= secs - 0.5 { 0.0 } else { up.at };
            self.peek_play_from(from);
        }
    }

    /// (Re)start what is up playing from `from` seconds.
    fn peek_play_from(&mut self, from: f32) {
        let video = self.peek_shown().and_then(|(_, p)| p.video);
        let max = self.peek_max();
        if video.is_some() && self.peek.frames.is_none() {
            let (sender, receiver) = calloop::channel::channel();
            let inserted = self
                .loop_handle
                .insert_source(receiver, |event, _, app: &mut App| {
                    if let calloop::channel::Event::Msg((tag, frame)) = event {
                        app.peek_frame(tag, frame);
                    }
                });
            if let Err(e) = inserted {
                warn!("quick look: {e}");
                return;
            }
            self.peek.frames = Some(sender);
        }
        self.peek.tag += 1;
        let tag = self.peek.tag;
        let frames = self.peek.frames.clone();
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        up.stop();
        up.at = from;
        up.frames_of_play = 0;
        let what = match (video, frames) {
            (Some((_, own)), Some(frames)) => {
                let size = kinds::stream_size(own, max);
                up.frame_size = size;
                kinds::stream(
                    &up.path,
                    from,
                    size,
                    own,
                    tag,
                    frames,
                    kinds::Way::QUICK_LOOK,
                )
                .map(|stream| Playing::Video { stream, tag })
            }
            (Some(_), None) => None,
            (None, _) => kinds::play(&up.path, from).map(Playing::Song),
        };
        up.play = what.map(|what| Play {
            what,
            since: Instant::now(),
            from,
        });
        self.request_desktop_top_draw();
    }

    /// A video's frame (an empty one: it ended).
    fn peek_frame(&mut self, tag: u64, frame: Vec<u8>) {
        let secs = self.peek_secs().unwrap_or(0.0);
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        let ours =
            matches!(&up.play, Some(Play { what: Playing::Video { tag: t, .. }, .. }) if *t == tag);
        if !ours {
            return;
        }
        if frame.is_empty() {
            up.stop();
            up.at = secs;
            if self.peek.playlist.is_some() {
                self.peek_list_next();
                return;
            }
        } else {
            // (Its clock starts with its first frame: the decoder takes a
            // moment to start.)
            if up.frame.is_none() && up.frames_of_play == 0 {
                if let Some(play) = up.play.as_mut() {
                    debug!(
                        "quick look: first frame {:?} after the start",
                        play.since.elapsed()
                    );
                    play.since = Instant::now();
                }
            }
            up.frames_of_play += 1;
            up.frame = Some(frame);
        }
        self.request_desktop_top_draw();
    }

    /// A key of file window `win` while Quick Look is up for it: whether it
    /// was Quick Look's (else the window goes on with it — the arrows walk,
    /// and Quick Look follows: `peek_show` again).
    pub(crate) fn peek_key(
        &mut self,
        win: u32,
        key: smithay_client_toolkit::seat::keyboard::Keysym,
        utf8: Option<&str>,
    ) -> bool {
        use smithay_client_toolkit::seat::keyboard::Keysym;
        if !self
            .peek
            .up
            .as_ref()
            .is_some_and(|u| u.win == win && !u.closing)
        {
            return false;
        }
        // The pencil taken: every key is the editor's.
        if self.peek.up.as_ref().is_some_and(|u| u.text_edit.is_some()) {
            self.text_key(key, utf8);
            return true;
        }
        if self.peek.up.as_ref().is_some_and(|u| u.trim_edit.is_some()) {
            self.trim_key(key);
            return true;
        }
        if self.peek.up.as_ref().is_some_and(|u| u.edit.is_some()) {
            self.edit_key(key, utf8);
            return true;
        }
        // A slideshow: the arrows turn it, Escape (or Space) ends it.
        if self.peek.slides.is_some() {
            match key {
                Keysym::Right | Keysym::Down => self.peek_slide_step(1),
                Keysym::Left | Keysym::Up => self.peek_slide_step(-1),
                _ => self.peek_close(),
            }
            return true;
        }
        match key {
            Keysym::space | Keysym::Escape => self.peek_close(),
            Keysym::Return | Keysym::KP_Enter => self.peek_open_app(),
            Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down => return false,
            Keysym::p | Keysym::P => self.peek_play(),
            // Zoom a photo in, out, back to fitted.
            Keysym::plus | Keysym::equal | Keysym::KP_Add | Keysym::minus | Keysym::KP_Subtract => {
                if let Some((body, _, _)) = self.peek_zoom_frame() {
                    let factor = if matches!(key, Keysym::minus | Keysym::KP_Subtract) {
                        0.8
                    } else {
                        1.25
                    };
                    self.peek_zoom_at(factor, (body.x + body.w / 2.0, body.y + body.h / 2.0));
                }
            }
            Keysym::_0 | Keysym::KP_0 => self.peek_zoom_reset(),
            Keysym::Page_Up | Keysym::Page_Down => {
                let by = if key == Keysym::Page_Up { -1 } else { 1 };
                if !self.peek_turn(by) {
                    let page = self.peek.up.as_ref().map_or(0.0, |u| u.rect.h - HEAD) * 0.9;
                    self.peek_scroll(by as f32 * page);
                }
            }
            _ => {}
        }
        true
    }

    fn peek_scroll(&mut self, by: f32) {
        let body_h = self.peek.up.as_ref().map_or(0.0, |u| u.rect.h - HEAD);
        // A LONG PAGE: it moves under the frame; the run that holds the
        // part on screen is asked for as the frame reaches it, its
        // neighbours ahead.
        if let Some(l) = self.peek_shown().and_then(|(_, p)| p.long) {
            let Some(up) = self.peek.up.as_mut() else {
                return;
            };
            let s = up.rect.w / l.w as f32;
            let most = (l.h as f32 * s - body_h).max(0.0);
            up.scroll = (up.scroll + by).clamp(0.0, most);
            let last = l.h.saturating_sub(1) / crate::peek_web::STEP;
            let k = (((up.scroll / s) as u32) / crate::peek_web::STEP).min(last);
            if k != up.page {
                up.page = k;
                let path = up.path.clone();
                // (The one wanted last is made first.)
                if k < last {
                    self.peek_want_now((path.clone(), k + 1));
                }
                if k > 0 {
                    self.peek_want_now((path.clone(), k - 1));
                }
                self.peek_want_now((path, k));
            }
            self.request_desktop_top_draw();
            return;
        }
        let Some(n) = self.peek_shown().and_then(|(_, p)| match &p.body {
            Body::Text { lines } => Some(lines.len()),
            _ => None,
        }) else {
            return;
        };
        let most = (n as f32 * TEXT_LINE + 2.0 * TEXT_PAD - body_h).max(0.0);
        if let Some(up) = self.peek.up.as_mut() {
            up.scroll = (up.scroll + by).clamp(0.0, most);
        }
        self.request_desktop_top_draw();
    }

    /// The wheel: text scrolls, a PDF turns its pages.
    fn peek_wheel(&mut self, by: f32) {
        let paged = self.peek_shown().is_some_and(|(_, p)| p.pages > 1);
        if !paged {
            return self.peek_scroll(by);
        }
        let Some(up) = self.peek.up.as_mut() else {
            return;
        };
        up.wheel += by;
        if up.wheel.abs() >= WHEEL_PAGE {
            let step = up.wheel.signum() as i32;
            up.wheel = 0.0;
            self.peek_turn(step);
        }
    }

    /// The pointer on the top surface while Quick Look is up.
    pub(crate) fn peek_pointer(
        &mut self,
        event: &smithay_client_toolkit::reexports::client::protocol::wl_pointer::Event,
    ) {
        use smithay_client_toolkit::reexports::client::protocol::wl_pointer::{
            Axis, ButtonState, Event,
        };
        use smithay_client_toolkit::reexports::client::WEnum;
        // A slideshow: a click on its × ends it, anywhere else turns it on.
        if self.peek.slides.is_some() {
            match event {
                Event::Enter { surface_x, surface_y, .. } | Event::Motion { surface_x, surface_y, .. } => {
                    if let Some(up) = self.peek.up.as_mut() {
                        up.ptr = Some((*surface_x as f32, *surface_y as f32));
                    }
                    self.request_desktop_top_draw();
                }
                Event::Button { state: WEnum::Value(ButtonState::Pressed), .. } => {
                    let at = self.peek.up.as_ref().and_then(|u| u.ptr).unwrap_or_default();
                    if self.slide_caption().is_some_and(|(_, x)| x.contains(at)) {
                        self.peek_close();
                    } else {
                        self.peek_slide_step(1);
                    }
                }
                _ => {}
            }
            return;
        }
        // The pencil taken: the pointer is the editor's.
        if self.peek.up.as_ref().is_some_and(|u| u.edit.is_some()) && self.edit_pointer(event) {
            return;
        }
        if self.peek.up.as_ref().is_some_and(|u| u.trim_edit.is_some())
            && matches!(event, Event::Motion { .. } | Event::Button { .. })
        {
            self.trim_pointer(event);
            return;
        }
        if self.peek.up.as_ref().is_some_and(|u| u.text_edit.is_some()) {
            match event {
                Event::Button {
                    state: WEnum::Value(ButtonState::Pressed),
                    ..
                } => {
                    let Some(up) = self.peek.up.as_ref() else {
                        return;
                    };
                    let at = up.ptr.unwrap_or((-1.0, -1.0));
                    let parts = crate::peek_editor::edit_parts(up.rect, false);
                    let char_w = self.peek_char_w();
                    if parts.save.contains(at) {
                        self.text_key_save(false);
                    } else if parts.copy.contains(at) {
                        self.text_key_save(true);
                    } else if parts.cancel.contains(at) {
                        self.text_key(smithay_client_toolkit::seat::keyboard::Keysym::Escape, None);
                    } else {
                        self.text_click(at, char_w);
                    }
                    return;
                }
                Event::Axis {
                    axis: WEnum::Value(Axis::VerticalScroll),
                    value,
                    ..
                } => {
                    if let Some(t) = self.peek.up.as_mut().and_then(|u| u.text_edit.as_mut()) {
                        t.scroll = (t.scroll + *value as f32).max(0.0);
                    }
                    self.request_desktop_top_draw();
                    return;
                }
                _ => {}
            }
        }
        match event {
            Event::Enter {
                surface_x,
                surface_y,
                ..
            }
            | Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if let Some(up) = self.peek.up.as_mut() {
                    up.ptr = Some((*surface_x as f32, *surface_y as f32));
                }
                // (A page's button rested on: its tooltip.)
                self.page_tip_follow();
                // (A zoomed photo being dragged.)
                self.peek_zoom_motion((*surface_x as f32, *surface_y as f32));
                self.request_desktop_top_draw();
            }
            Event::Leave { .. } => {
                if let Some(up) = self.peek.up.as_mut() {
                    up.ptr = None;
                    up.tip = None;
                    up.tips_warm = false;
                }
                self.request_desktop_top_draw();
            }
            // A photo: Ctrl with the wheel zooms about the pointer; zoomed,
            // the wheel moves it.
            Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } if self.peek_zoom_frame().is_some()
                && (self.modifiers.ctrl
                    || self.peek.up.as_ref().is_some_and(|u| u.zoom.zoomed())) =>
            {
                let v = *value as f32;
                let at = self
                    .peek
                    .up
                    .as_ref()
                    .and_then(|u| u.ptr)
                    .unwrap_or_default();
                let zoomed = self.peek.up.as_ref().is_some_and(|u| u.zoom.zoomed());
                match axis {
                    Axis::VerticalScroll if self.modifiers.ctrl => {
                        self.peek_zoom_at((-v * 0.01).exp(), at)
                    }
                    Axis::VerticalScroll if zoomed => self.peek_pan((0.0, -v)),
                    Axis::HorizontalScroll if zoomed => self.peek_pan((-v, 0.0)),
                    _ => {}
                }
            }
            Event::Axis {
                axis: WEnum::Value(Axis::VerticalScroll),
                value,
                ..
            } => self.peek_wheel(*value as f32),
            Event::Button {
                state: WEnum::Value(ButtonState::Released),
                ..
            } => self.peek_zoom_release(),
            Event::Button {
                state: WEnum::Value(ButtonState::Pressed),
                ..
            } => {
                let Some(up) = self.peek.up.as_ref() else {
                    return;
                };
                let at = up.ptr.unwrap_or((-1.0, -1.0));
                // A photo: a double click zooms; zoomed, a drag moves it.
                if self.peek_zoom_press(at) {
                    return;
                }
                let Some(up) = self.peek.up.as_ref() else {
                    return;
                };
                let parts = parts(up.rect);
                let song = self
                    .peek_shown()
                    .is_some_and(|(_, p)| matches!(p.body, Body::Song { .. }));
                let video = self.peek_shown().is_some_and(|(_, p)| p.video.is_some());
                let bar = video_bar(up.rect);
                let secs = self.peek_secs().unwrap_or(0.0);
                if video && bar.play.contains(at) {
                    self.peek_play();
                } else if video && bar.snap.contains(at) {
                    self.peek_snap();
                } else if video && bar.track.contains(at) {
                    // (A click on the line: from there.)
                    let k = ((at.0 - bar.track.x) / bar.track.w).clamp(0.0, 1.0);
                    self.peek_play_from(k * secs);
                } else if let Some(b) = self.page_button_at(at) {
                    use crate::peek_page::Button;
                    match b {
                        Button::Bin => self.box_peek_bin(),
                        Button::Copy => {
                            if let Some(url) = self.peek.up.as_ref().and_then(|u| u.said.as_ref()).map(|s| s.url.clone()) {
                                self.peek_copy_link(&url);
                            }
                        }
                        Button::Open => self.peek_open_app(),
                        Button::Close => self.peek_close(),
                    }
                } else if parts.pencil.contains(at) && self.peek_editable() {
                    self.edit_begin();
                } else if parts.close.contains(at) {
                    self.peek_close();
                } else if parts.open.contains(at) {
                    self.peek_open_app();
                } else if song && song_parts(self.song_area(up.rect)).play.contains(at) {
                    self.peek_play();
                } else if !up.rect.contains(at) {
                    self.peek_close();
                }
            }
            _ => {}
        }
    }

    /// Where the frame is going: the shape its body wants, centred.
    /// The room a song's frame keeps under it for the cutting line.
    fn song_room(&self) -> f32 {
        if self.peek.up.as_ref().is_some_and(|u| u.trim_edit.is_some()) {
            TRIM_ROOM
        } else {
            0.0
        }
    }

    /// The part of a song's frame `r` the song itself is laid out in.
    fn song_area(&self, r: Rect) -> Rect {
        Rect {
            h: r.h - self.song_room(),
            ..r
        }
    }

    fn peek_target(&self) -> Rect {
        let (sw, sh) = (
            self.desktop_top.size.0 as f32,
            self.desktop_top.size.1 as f32,
        );
        if self.peek.slides.is_some() {
            return Rect::new(0.0, 0.0, sw, sh);
        }
        let scale = self.surface_scale(crate::fractional::SurfaceKind::DesktopTop);
        let (mw, mh) = (sw * MAX, sh * MAX - HEAD);
        // The pencil taken: the edited picture's shape (room for the
        // toolbar under a small one).
        let editing = self
            .peek
            .up
            .as_ref()
            .and_then(|u| u.edit.as_ref())
            .and_then(|e| e.pic.as_ref())
            .map(|p| (p.w, p.h));
        if let Some((ew, eh)) = editing {
            let (pw, ph) = (ew as f32 / scale, eh as f32 / scale);
            let k = (mw / pw).min(mh / ph).min(1.0);
            let (w, h) = ((pw * k).max(600.0).min(sw * 0.94), (ph * k).max(320.0));
            return Rect::new(
                ((sw - w) / 2.0).round(),
                ((sh - h - HEAD) / 2.0).round(),
                w.round(),
                (h + HEAD).round(),
            );
        }
        // A PAGE (`peek_page.rs`): its picture grown, its words under it.
        if let Some(said) = self.peek.up.as_ref().and_then(|u| u.said.as_ref()) {
            let pic = match self.peek_shown().map(|(_, p)| &p.body) {
                Some(Body::Picture(p)) => Some((p.w as f32 / scale, p.h as f32 / scale)),
                _ => None,
            };
            let (w, h) = self.page_size(said, pic, (mw, mh), sw);
            return Rect::new(
                ((sw - w) / 2.0).round(),
                ((sh - h - HEAD) / 2.0).round(),
                w.round(),
                (h + HEAD).round(),
            );
        }
        let (w, h) = match self.peek_shown().map(|(_, p)| &p.body) {
            // A long page: as wide as it is drawn, as tall as there is room.
            Some(Body::Picture(_)) if self.peek_shown().is_some_and(|(_, p)| p.long.is_some()) => {
                let l =
                    self.peek_shown()
                        .and_then(|(_, p)| p.long)
                        .unwrap_or(crate::peek_web::Long {
                            w: 1,
                            h: 1,
                            y: 0,
                            ch: 1,
                        });
                let w = (l.w as f32).min(mw);
                ((w).max(380.0), (l.h as f32 * w / l.w as f32).min(mh))
            }
            Some(Body::Picture(p)) => {
                let (pw, ph) = (p.w as f32 / scale, p.h as f32 / scale);
                let fit = (mw / pw).min(mh / ph);
                // A still: never bigger than the picture itself. What MOVES
                // (a GIF, a video) is seen from further away: a small one
                // grows to a good part of the screen, as a player shows it
                // (Max, 2026-10-17: "the gifs show pretty small").
                let moving = self
                    .peek_shown()
                    .is_some_and(|(_, p)| p.anim.is_some() || p.video.is_some());
                let k = if moving {
                    (MOVING_SHARE * mw / pw)
                        .min(MOVING_SHARE * mh / ph)
                        .max(1.0)
                        .min(fit)
                } else {
                    fit.min(1.0)
                };
                ((pw * k).max(380.0), ph * k)
            }
            Some(Body::Song { .. }) => (SONG.0.min(mw), SONG.1 + self.song_room()),
            // (As tall as its lines, a page at least.)
            Some(Body::Text { lines }) => (
                (sw * 0.56).clamp(480.0, 1000.0).min(mw),
                (lines.len() as f32 * TEXT_LINE + 2.0 * TEXT_PAD).clamp(320.0, mh),
            ),
            Some(Body::Info { .. }) => (560.0, 260.0),
            // (Not ready: a square for the soft picture.)
            None => (mh.min(mw) * 0.7, mh.min(mw) * 0.7),
        };
        Rect::new(
            ((sw - w) / 2.0).round(),
            ((sh - h - HEAD) / 2.0).round(),
            w.round(),
            (h + HEAD).round(),
        )
    }

    /// Move Quick Look on by `dt`; whether it still moves.
    pub(crate) fn peek_ease(&mut self, dt: f32) -> bool {
        // (A zoomed photo that stood still: its sharp part asked for.)
        let zoom_waiting = self.peek_zoom_tick();
        // (A song of a playlist that ended: the next one — unless it ended
        // as it began: the player could not play it, and racing through the
        // rest would be no better.)
        let ended_after = self.peek.up.as_mut().and_then(|u| match &mut u.play {
            Some(Play {
                what: Playing::Song(child),
                since,
                ..
            }) => matches!(child.try_wait(), Ok(Some(_))).then(|| since.elapsed()),
            _ => None,
        });
        if let (Some(after), true) = (ended_after, self.peek.playlist.is_some()) {
            if let Some(up) = self.peek.up.as_mut() {
                up.stop();
            }
            if after < std::time::Duration::from_secs(1) && self.peek_secs().unwrap_or(0.0) > 2.0 {
                warn!("quick look: the song could not be played; the playlist stops");
                self.peek.playlist = None;
            } else {
                self.peek_list_next();
            }
        }
        let Some(target) = self.peek.up.as_ref().map(|u| {
            if u.closing {
                u.from
            } else {
                self.peek_target()
            }
        }) else {
            return false;
        };
        let ready = self.peek_shown().is_some();
        let secs = self.peek_secs().unwrap_or(0.0);
        // A moving picture goes round on its own clock.
        let anim = self.peek_shown().and_then(|(_, p)| p.anim.clone());
        let Some(up) = self.peek.up.as_mut() else {
            return false;
        };
        up.t = (up.t + dt / GROW_SECS).min(1.0);
        up.rect = lerp(up.start, target, ease_out(up.t));
        // A song that came to its end by itself: stopped (else the line
        // moved on for ever and its mpv was never waited for).
        let ended = match up.play.as_mut() {
            Some(Play {
                what: Playing::Song(child),
                ..
            }) => matches!(child.try_wait(), Ok(Some(_))),
            _ => false,
        };
        if ended {
            up.stop();
            up.at = secs;
        }
        // (A slideshow's photo fades in slower: one comes after another.)
        let fade = if self.peek.slides.is_some() { 0.45 } else { SHARP_SECS };
        if ready && up.sharp < 1.0 {
            up.sharp = (up.sharp + dt / fade).min(1.0);
        }
        if up.closing && up.t >= 1.0 {
            self.peek_gone();
            return true;
        }
        // (A song playing moves its line on.)
        if let Some(anim) = anim.as_ref().filter(|_| !up.closing && up.zoom.k <= 1.001) {
            up.anim_ms += dt * 1000.0;
            let i = anim.at(up.anim_ms);
            if up.anim_frame != Some(i) {
                up.anim_frame = Some(i);
                up.frame = Some(anim.frames[i].0.clone());
                up.frame_size = anim.size;
            }
        }
        let noticing = up
            .notice
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed().as_secs_f32() < NOTICE_SECS);
        up.t < 1.0
            || (ready && up.sharp < 1.0)
            || up.play.is_some()
            || noticing
            || zoom_waiting
            || anim.is_some()
    }

    /// Put what is shown in the top surface's layers.
    /// The mono font's width of a character (a guess until measured).
    /// Measure where code's coloured pieces begin and end on the lines in
    /// view (the shown file's, or the text being written's) not measured yet.
    fn peek_code_measure(&mut self) {
        let mut want: Vec<(String, Vec<u16>)> = Vec::new();
        if let Some(t) = self.peek.up.as_mut().and_then(|u| u.text_edit.as_mut()) {
            // Written text: coloured again when it changed.
            t.recolor();
            let first = (t.scroll / crate::peek_text::LINE).floor().max(0.0) as usize;
            for n in first..first + 90 {
                if let (Some(line), Some(spans)) = (t.lines.get(n), t.syntax.get(n)) {
                    if !spans.is_empty() {
                        want.push((crate::peek_text::shown(line), cuts_of(spans)));
                    }
                }
            }
        } else if let Some((_, p)) = self.peek_shown() {
            let first = self
                .peek
                .up
                .as_ref()
                .map_or(0, |u| (u.scroll / TEXT_LINE).floor().max(0.0) as usize);
            if let Body::Text { lines } = &p.body {
                // (A screenful and a half: what scrolls in next is ready.)
                for (line, spans) in lines.iter().zip(&p.syntax).skip(first).take(90) {
                    if !spans.is_empty() {
                        want.push((line.clone(), cuts_of(spans)));
                    }
                }
            }
        }
        want.retain(|k| !self.peek.code_x.contains_key(k));
        if want.is_empty() {
            return;
        }
        let Some(renderer) = self.desktop_top.renderer.as_mut() else {
            return;
        };
        // (Lines of files long gone are let go now and then.)
        if self.peek.code_x.len() > 4000 {
            self.peek.code_x.clear();
        }
        for (line, cuts) in want {
            let chars: Vec<char> = line.chars().collect();
            let xs = cuts
                .iter()
                .map(|&c| {
                    let prefix: String = chars[..(c as usize).min(chars.len())].iter().collect();
                    renderer.measure_text(&prefix, TEXT_PX, Some(crate::options::NERD))
                })
                .collect();
            self.peek.code_x.insert((line, cuts), xs);
        }
    }

    /// Where `line`, cut by `spans`, has its pieces (once measured).
    pub(crate) fn code_xs(
        &self,
        line: &str,
        spans: &[crate::peek_syntax::Span],
    ) -> Option<&Vec<f32>> {
        self.peek.code_x.get(&(line.to_owned(), cuts_of(spans)))
    }

    pub(crate) fn peek_char_w(&self) -> f32 {
        if self.peek.char_w > 0.0 {
            self.peek.char_w
        } else {
            crate::peek_text::PX * 0.6
        }
    }

    pub(crate) fn peek_upload(&mut self) {
        // The caret of text being written: measured in the font it is drawn in.
        if let (Some(t), Some(renderer)) = (
            self.peek.up.as_mut().and_then(|u| u.text_edit.as_mut()),
            self.desktop_top.renderer.as_mut(),
        ) {
            let (line, col) = t.at();
            if t.caret_x.is_none_or(|(l, c, _)| (l, c) != (line, col)) {
                let prefix = crate::peek_text::before(t.line_text(line), col);
                let x = renderer.measure_text(
                    &prefix,
                    crate::peek_text::PX,
                    Some(crate::options::NERD),
                );
                t.caret_x = Some((line, col, x));
            }
        }
        // Code: where its coloured pieces stand on the lines in view.
        self.peek_code_measure();
        if self.peek.char_w <= 0.0 {
            if let Some(renderer) = self.desktop_top.renderer.as_mut() {
                let w = renderer.measure_text(
                    "MMMMMMMMMM",
                    crate::peek_text::PX,
                    Some(crate::options::NERD),
                ) / 10.0;
                if w > 0.0 {
                    self.peek.char_w = w;
                }
            }
        }
        // A video's newest frame.
        if let Some(up) = self.peek.up.as_mut() {
            if let (Some(frame), Some(renderer)) =
                (up.frame.take(), self.desktop_top.renderer.as_mut())
            {
                renderer.set_video_frame(up.frame_size.0, up.frame_size.1, &frame);
                up.frames += 1;
            }
        }
        let Some(up) = self.peek.up.as_ref() else {
            return;
        };
        let Some(renderer) = self.desktop_top.renderer.as_mut() else {
            return;
        };
        if !self.desktop_top.peek_layers {
            renderer.alloc_icon_array(LAYERS);
            self.desktop_top.peek_layers = true;
        }
        if !self.desktop_top.peek_soft {
            if let Some(thumb) = up.thumb.as_ref() {
                renderer.update_icon_layer(SOFT, thumb);
                renderer.update_icon_layer(BADGE, thumb);
            }
            self.desktop_top.peek_soft = true;
        }
        // The pencil taken: its picture's tiles (what changed of them).
        if let Some(edit) = self.peek.up.as_mut().and_then(|u| u.edit.as_mut()) {
            if edit.rev != self.desktop_top.peek_edit_rev {
                if let (Some(pic), Some(renderer)) =
                    (edit.pic.as_ref(), self.desktop_top.renderer.as_mut())
                {
                    // (Up now: nothing waits — `Some(empty)`, not `None`,
                    // which would mean all of it.)
                    match edit.dirty.replace(HashSet::new()) {
                        Some(dirty) => {
                            for i in dirty {
                                if let Some(tile) = pic.tiles.get(i as usize) {
                                    renderer.update_icon_layer(i, tile);
                                }
                            }
                        }
                        None => {
                            for (i, tile) in pic.tiles.iter().enumerate() {
                                renderer.update_icon_layer(i as u32, tile);
                            }
                        }
                    }
                    self.desktop_top.peek_edit_rev = edit.rev;
                    // (The view's own tiles are not in the layers any more.)
                    self.desktop_top.peek_tiles = None;
                }
            }
            return;
        }
        // A zoomed photo's sharp part.
        if let Some(up) = self.peek.up.as_ref() {
            if let Some((part, _)) = up
                .zoom
                .detail
                .as_ref()
                .filter(|_| up.zoom.detail_rev != self.desktop_top.peek_detail_rev)
            {
                for (i, tile) in part.tiles.iter().enumerate() {
                    renderer.update_icon_layer(DETAIL + i as u32, tile);
                }
                self.desktop_top.peek_detail_rev = up.zoom.detail_rev;
            }
        }
        let Some(key) = self.peek.up.as_ref().map(Up::key) else {
            return;
        };
        if self.desktop_top.peek_tiles.as_ref() != Some(&key) {
            if let Some(p) = self.peek.cache.get(&key) {
                if let Some(pic) = pic_of(&p.body) {
                    for (i, tile) in pic.tiles.iter().enumerate() {
                        renderer.update_icon_layer(i as u32, tile);
                    }
                }
                self.desktop_top.peek_tiles = Some(key);
            }
        }
    }

    /// Quick Look into `scene`.
    /// Where a slideshow's caption is ("name · 3 of 52 · ×"), and its ×.
    pub(crate) fn slide_caption(&self) -> Option<(Rect, Rect)> {
        let slides = self.peek.slides.as_ref()?;
        let up = self.peek.up.as_ref()?;
        let (sw, sh) = (self.desktop_top.size.0 as f32, self.desktop_top.size.1 as f32);
        let words = slides.caption(&up.name);
        let w = (words.chars().count() as f32 * 7.0 + 18.0 + 34.0).min(sw - 40.0);
        let r = Rect::new(((sw - w) / 2.0).round(), sh - 24.0 - 32.0, w.round(), 32.0);
        let x = Rect::new(r.x + r.w - 30.0, r.y + 4.0, 24.0, 24.0);
        Some((r, x))
    }

    /// A SLIDESHOW's frame: black, the photo fitted to the screen and
    /// fading in, its caption at the foot.
    fn slides_push(&self, scene: &mut Scene, up: &Up, open: f32) {
        let (sw, sh) = (self.desktop_top.size.0 as f32, self.desktop_top.size.1 as f32);
        let full = Rect::new(0.0, 0.0, sw, sh);
        scene.rects.push(RectInst {
            rect: full,
            radius: 0.0,
            color: [0.0, 0.0, 0.0, open],
            glass: 0.0,
            border: 0.0,
        });
        let mut g = GridContent {
            clip: full,
            ..Default::default()
        };
        let shown = self.peek_shown();
        let tiles_up = shown
            .as_ref()
            .is_some_and(|(key, _)| self.desktop_top.peek_tiles.as_ref() == Some(key));
        if let Some(pic) = shown.and_then(|(_, p)| pic_of(&p.body)).filter(|_| tiles_up) {
            let room = Rect::new(0.0, 0.0, sw, sh - 72.0);
            let k = (room.w / pic.w as f32).min(room.h / pic.h as f32);
            let (w, h) = (pic.w as f32 * k, pic.h as f32 * k);
            let into = Rect::new(((sw - w) / 2.0).round(), ((room.h - h) / 2.0 + 12.0).round(), w, h);
            let (kx, ky) = (into.w / pic.w as f32, into.h / pic.h as f32);
            let clip = [into.x, into.y, into.x + into.w, into.y + into.h];
            for row in 0..pic.rows {
                for col in 0..pic.cols {
                    let x = into.x + (col * STEP) as f32 * kx - kx;
                    let y = into.y + (row * STEP) as f32 * ky - ky;
                    g.icons.push(IconInst {
                        rect: Rect::new(x, y, TILE as f32 * kx, TILE as f32 * ky),
                        layer: row * pic.cols + col,
                        tint: clip,
                        ring: -1.6,
                        plate: [0.0, up.sharp * open, 0.0, 0.0],
                    });
                }
            }
        }
        // The caption: its name, where in the show, ×.
        if let (Some((r, x)), Some(slides)) = (self.slide_caption(), self.peek.slides.as_ref()) {
            g.rects.push(RectInst {
                rect: r,
                radius: r.h / 2.0,
                color: [0.09, 0.09, 0.09, 0.85 * open],
                glass: 0.0,
                border: 0.0,
            });
            let ink = [1.0, 1.0, 1.0, 0.9 * open];
            g.labels.push(text_label(&slides.caption(&up.name), (r.x + 16.0, r.y + 8.0), r.w - 50.0, 13.0, None, ink));
            let hot = up.ptr.is_some_and(|p| x.contains(p));
            if hot {
                g.rects.push(RectInst {
                    rect: x,
                    radius: 12.0,
                    color: [1.0, 1.0, 1.0, 0.16 * open],
                    glass: 0.0,
                    border: 0.0,
                });
            }
            g.labels.push(Label {
                centered: true,
                ..text_label("×", (x.x + x.w / 2.0, x.y + 2.0), x.w, 16.0, None, ink)
            });
        }
        scene.grids.push(g);
    }

    pub(crate) fn peek_push(&self, scene: &mut Scene) {
        let Some(up) = self.peek.up.as_ref() else {
            return;
        };
        let (sw, sh) = (
            self.desktop_top.size.0 as f32,
            self.desktop_top.size.1 as f32,
        );
        let k = ease_out(up.t);
        // How open it is: growing, all of it; going back, less and less.
        let open = if up.closing { 1.0 - k } else { k.max(0.35) };
        // A SLIDESHOW: the photo on black, full screen.
        if self.peek.slides.is_some() {
            return self.slides_push(scene, up, if up.closing { 1.0 - k } else { 1.0 });
        }
        let pal = self.filer_palette();
        let r = up.rect;
        scene.rects.push(RectInst {
            rect: Rect::new(0.0, 0.0, sw, sh),
            radius: 0.0,
            color: [0.0, 0.0, 0.0, 0.28 * if up.closing { 1.0 - k } else { k }],
            glass: 0.0,
            border: 0.0,
        });
        scene.shadows.push(ShadowInst {
            rect: r,
            radius: 21.0,
            blur: 46.0,
            color: [0.0, 0.0, 0.0, 0.55 * open],
            edges: [0.35, 0.6, 0.45, 0.45],
        });
        let mut g = GridContent {
            clip: Rect::new(0.0, 0.0, sw, sh),
            ..Default::default()
        };
        // (Solid: the files' ground, whatever its own alpha.)
        let mut ground = pal.ground;
        ground[3] = open;
        // (A "border" wider than the frame fills all of it and keeps the
        // boxes' night light off it: that falls into black at the foot of a
        // ground this dark — a viewer is a window, not a box.)
        g.rects.push(RectInst {
            rect: r,
            radius: WINDOW_CORNER,
            color: ground,
            glass: 0.0,
            border: 1.0e4,
        });
        // THE PENCIL on text: the text being written.
        if up.text_edit.is_some() {
            let mut ink = pal.ink;
            ink[3] *= open;
            self.text_push(&mut g, r, open, ink, ground, self.peek_char_w());
            scene.grids.push(g);
            return;
        }
        // THE PENCIL taken: the editor draws the rest.
        if let Some(edit) = up.edit.as_ref() {
            let tiles_up = edit.pic.is_some() && self.desktop_top.peek_edit_rev == edit.rev;
            let mut over = GridContent {
                clip: g.clip,
                ..Default::default()
            };
            let mut ink = pal.ink;
            ink[3] *= open;
            self.edit_push(&mut g, &mut over, r, open, ink, ground, tiles_up);
            scene.grids.push(g);
            scene.grids.push(over);
            return;
        }
        let clip = [r.x, r.y, r.x + r.w, r.y + r.h];
        let clipped = |rect: Rect, layer: u32, a: f32| IconInst {
            rect,
            layer,
            tint: clip,
            ring: -1.6,
            plate: [WINDOW_CORNER, a, 0.0, 0.0],
        };
        let body = Rect::new(r.x, r.y + HEAD, r.w, (r.h - HEAD).max(0.0));
        let shown = self.peek_shown();
        let tiles_up = shown
            .as_ref()
            .is_some_and(|(key, _)| self.desktop_top.peek_tiles.as_ref() == Some(key));
        let prepared = shown.map(|(_, p)| p);
        let mut ink = pal.ink;
        ink[3] *= open;
        let mut dim = ink;
        dim[3] *= 0.6;
        // A picture in its tiles, filling `into`.
        let tiles_in = |g: &mut GridContent, pic: &Pic, into: Rect, a: f32, base: u32| {
            let kx = into.w / pic.w as f32;
            let ky = into.h / pic.h as f32;
            for row in 0..pic.rows {
                for col in 0..pic.cols {
                    let x = into.x + (col * STEP) as f32 * kx - kx;
                    let y = into.y + (row * STEP) as f32 * ky - ky;
                    g.icons.push(clipped(
                        Rect::new(x, y, TILE as f32 * kx, TILE as f32 * ky),
                        base + row * pic.cols + col,
                        a,
                    ));
                }
            }
        };
        // THE BODY.
        let mut over_page = None;
        let mut zoom_grid = None;
        let soft = 1.0 - up.sharp;
        let soft_square = |g: &mut GridContent, a: f32| {
            if up.thumb.is_some() && a > 0.01 {
                let s = body.w.min(body.h) * 0.92;
                g.icons.push(clipped(
                    Rect::new(
                        body.x + (body.w - s) / 2.0,
                        body.y + (body.h - s) / 2.0,
                        s,
                        s,
                    ),
                    SOFT,
                    a * open,
                ));
            }
        };
        match prepared.map(|p| &p.body) {
            // A PAGE (`peek_page.rs`): its picture in its room (its tile
            // beside the words), the words.
            _ if up.said.is_some() => {
                let said = up.said.as_ref().expect("checked");
                let l = self.page_layout(r, said);
                if let (Some(Body::Picture(pic)), true) = (prepared.map(|p| &p.body), tiles_up) {
                    let a = if up.closing { open } else { up.sharp };
                    if let Some(room) = l.pic.or(l.tile) {
                        tiles_in(&mut g, pic, crate::peek_editor::pic_rect(room, pic.w, pic.h), a, 0);
                    }
                }
                self.page_words_push(&mut g, said, &l, ink, dim);
            }
            Some(Body::Picture(pic)) if tiles_up => {
                soft_square(&mut g, soft);
                // (Opaque: tiles overlap by a texel, and a see-through
                // picture would show that as a grid.)
                let into = crate::peek_editor::pic_rect(body, pic.w, pic.h);
                let a = if up.closing { open } else { up.sharp };
                if let Some(l) = prepared.and_then(|p| p.long) {
                    // A LONG PAGE: its run where it stands in the page, the
                    // page moved by the scroll, cut to the body.
                    let s = body.w / l.w as f32;
                    let r = Rect::new(
                        body.x,
                        body.y + l.y as f32 * s - up.scroll,
                        body.w,
                        l.ch as f32 * s,
                    );
                    let mut zg = GridContent {
                        clip: body,
                        ..Default::default()
                    };
                    tiles_in(&mut zg, pic, r, a, 0);
                    zoom_grid = Some(zg);
                    // Where in the page: a thin bar at the right.
                    let total = l.h as f32 * s;
                    if total > body.h + 1.0 {
                        let mut over = GridContent {
                            clip: g.clip,
                            ..Default::default()
                        };
                        let bh = (body.h * body.h / total).max(28.0);
                        let by = body.y + up.scroll / (total - body.h) * (body.h - bh);
                        over.rects.push(RectInst {
                            rect: Rect::new(body.x + body.w - 9.0, by + 2.0, 5.0, bh - 4.0),
                            radius: 2.5,
                            color: [0.5, 0.5, 0.5, 0.55 * open],
                            glass: 0.0,
                            border: 0.0,
                        });
                        over_page = Some(over);
                    }
                } else if self.peek_zoom_frame().is_some() && up.zoom.zoomed() {
                    // ZOOMED: the whole where it stands, its sharp part over
                    // it, in a grid cut to the body.
                    let mut zg = GridContent {
                        clip: body,
                        ..Default::default()
                    };
                    let r = crate::peek_zoom::view_rect(body, into, &up.zoom);
                    tiles_in(&mut zg, pic, r, a, 0);
                    if let Some((part, region)) = up
                        .zoom
                        .detail
                        .as_ref()
                        .filter(|_| self.desktop_top.peek_detail_rev == up.zoom.detail_rev)
                    {
                        let pr = Rect::new(
                            r.x + region[0] * r.w,
                            r.y + region[1] * r.h,
                            region[2] * r.w,
                            region[3] * r.h,
                        );
                        tiles_in(&mut zg, part, pr, a, DETAIL);
                    }
                    zoom_grid = Some(zg);
                } else {
                    tiles_in(&mut g, pic, into, a, 0);
                    // A MOVING PICTURE: its frames over its first.
                    if prepared.is_some_and(|p| p.anim.is_some()) && up.frames > 0 && !up.closing {
                        scene.video = Some((scene.grids.len(), clipped(into, 0, 1.0)));
                    }
                }
                let pages = prepared.map_or(0, |p| p.pages);
                if pages > 1 {
                    // (Its own grid: a grid's fills go under its pictures.)
                    let mut over = GridContent {
                        clip: g.clip,
                        ..Default::default()
                    };
                    let g = &mut over;
                    let word = format!("{} / {pages}", up.page + 1);
                    let w = self.peek_word_w(&word) + 22.0;
                    let pill =
                        Rect::new(body.x + (body.w - w) / 2.0, body.y + body.h - 38.0, w, 24.0);
                    g.rects.push(RectInst {
                        rect: pill,
                        radius: 12.0,
                        color: [0.0, 0.0, 0.0, 0.55 * open],
                        glass: 0.0,
                        border: 0.0,
                    });
                    g.labels.push(Label {
                        centered: true,
                        ..text_label(
                            &word,
                            (pill.x + pill.w / 2.0, pill.y + 4.0),
                            w,
                            12.0,
                            Some(FONT_BOLD),
                            [1.0, 1.0, 1.0, open],
                        )
                    });
                    over_page = Some(over);
                }
                // A VIDEO: its frames over the poster, its bar over them.
                if let Some((secs, _)) = prepared.and_then(|p| p.video) {
                    if up.frames > 0 && !up.closing {
                        scene.video = Some((scene.grids.len(), clipped(body, 0, 1.0)));
                    }
                    let mut over = GridContent {
                        clip: g.clip,
                        ..Default::default()
                    };
                    let bar = video_bar(r);
                    let hot = up.ptr.is_some_and(|p| bar.pill.contains(p));
                    over.rects.push(RectInst {
                        rect: bar.pill,
                        radius: bar.pill.h / 2.0,
                        color: [0.06, 0.07, 0.09, (if hot { 0.78 } else { 0.62 }) * open],
                        glass: 0.0,
                        border: 0.0,
                    });
                    let white = [1.0, 1.0, 1.0, open];
                    let glyph = if up.play.is_some() {
                        "\u{f04c}"
                    } else {
                        "\u{f04b}"
                    };
                    over.labels.push(Label {
                        centered: true,
                        family: Some(crate::options::NERD),
                        ..text_label(
                            glyph,
                            (bar.play.x + bar.play.w / 2.0, bar.play.y + 6.0),
                            bar.play.w,
                            14.0,
                            None,
                            white,
                        )
                    });
                    let at = up.song_at().min(secs);
                    let done = if secs > 0.0 {
                        (at / secs).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let line = Rect::new(
                        bar.track.x,
                        bar.track.y + bar.track.h / 2.0 - 2.0,
                        bar.track.w,
                        4.0,
                    );
                    over.rects.push(RectInst {
                        rect: line,
                        radius: 2.0,
                        color: [1.0, 1.0, 1.0, 0.25 * open],
                        glass: 0.0,
                        border: 0.0,
                    });
                    over.rects.push(RectInst {
                        rect: Rect {
                            w: line.w * done,
                            ..line
                        },
                        radius: 2.0,
                        color: white,
                        glass: 0.0,
                        border: 0.0,
                    });
                    let time = format!("{} / {}", kinds::clock(at), kinds::clock(secs));
                    over.labels.push(text_label(
                        &time,
                        (bar.track.x + bar.track.w + 12.0, bar.play.y + 8.0),
                        100.0,
                        12.0,
                        None,
                        white,
                    ));
                    // The camera: this moment kept as a photo beside the video.
                    if up.ptr.is_some_and(|p| bar.snap.contains(p)) {
                        over.rects.push(RectInst {
                            rect: bar.snap,
                            radius: bar.snap.h / 2.0,
                            color: [1.0, 1.0, 1.0, 0.18 * open],
                            glass: 0.0,
                            border: 0.0,
                        });
                    }
                    over.labels.push(Label {
                        centered: true,
                        family: Some(crate::options::NERD),
                        ..text_label(
                            "\u{f030}",
                            (bar.snap.x + bar.snap.w / 2.0, bar.snap.y + 6.0),
                            bar.snap.w,
                            14.0,
                            None,
                            white,
                        )
                    });
                    over_page = Some(over);
                }
            }
            Some(Body::Song {
                cover,
                title,
                sub,
                secs,
                wave,
            }) => {
                let parts = song_parts(self.song_area(r));
                match cover {
                    Some(pic) if tiles_up => {
                        let k = (parts.cover.w / pic.w as f32).min(parts.cover.h / pic.h as f32);
                        let (w, h) = (pic.w as f32 * k, pic.h as f32 * k);
                        let into = Rect::new(
                            parts.cover.x + (parts.cover.w - w) / 2.0,
                            parts.cover.y + (parts.cover.h - h) / 2.0,
                            w,
                            h,
                        );
                        tiles_in(&mut g, pic, into, open, 0);
                    }
                    _ if up.thumb.is_some() => g.icons.push(clipped(parts.cover, SOFT, open)),
                    _ => {}
                }
                let x = parts.wave.x;
                g.labels.push(text_label(
                    title,
                    (x, parts.cover.y + 4.0),
                    parts.wave.w,
                    20.0,
                    Some(FONT_BOLD),
                    ink,
                ));
                g.labels.push(text_label(
                    sub,
                    (x, parts.cover.y + 34.0),
                    parts.wave.w,
                    13.5,
                    None,
                    dim,
                ));
                // The loudness along the way, what is played lit.
                let at = up.song_at();
                let done = if *secs > 0.0 {
                    (at / secs).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let n = wave.len().max(1);
                let step = parts.wave.w / n as f32;
                for (i, v) in wave.iter().enumerate() {
                    let bh = (parts.wave.h * v.max(0.06)).round();
                    let lit = (i as f32 + 0.5) / n as f32 <= done;
                    let mut c = ink;
                    c[3] *= if lit { 1.0 } else { 0.32 };
                    g.rects.push(RectInst {
                        rect: Rect::new(
                            x + i as f32 * step,
                            parts.wave.y + (parts.wave.h - bh) / 2.0,
                            (step - 1.5).max(1.0),
                            bh,
                        ),
                        radius: 1.0,
                        color: c,
                        glass: 0.0,
                        border: 0.0,
                    });
                }
                // Play / pause, and where it is.
                let hot = up.ptr.is_some_and(|p| parts.play.contains(p));
                g.rects.push(RectInst {
                    rect: parts.play,
                    radius: parts.play.h / 2.0,
                    color: [
                        ink[0],
                        ink[1],
                        ink[2],
                        (if hot { 1.0 } else { 0.88 }) * open,
                    ],
                    glass: 0.0,
                    border: 0.0,
                });
                let mut on_ink = ground;
                on_ink[3] = open;
                let glyph = if up.play.is_some() {
                    "\u{f04c}"
                } else {
                    "\u{f04b}"
                };
                g.labels.push(Label {
                    centered: true,
                    family: Some(crate::options::NERD),
                    ..text_label(
                        glyph,
                        (parts.play.x + parts.play.w / 2.0 + 1.0, parts.play.y + 9.0),
                        parts.play.w,
                        16.0,
                        None,
                        on_ink,
                    )
                });
                let time = format!("{} / {}", kinds::clock(at), kinds::clock(*secs));
                g.labels.push(text_label(
                    &time,
                    (parts.play.x + parts.play.w + 14.0, parts.play.y + 11.0),
                    160.0,
                    12.5,
                    None,
                    dim,
                ));
            }
            Some(Body::Text { lines }) => {
                let first = (up.scroll / TEXT_LINE).floor().max(0.0) as usize;
                let shown = ((body.h / TEXT_LINE).ceil() as usize + 2)
                    .min(lines.len().saturating_sub(first));
                let clip = Rect::new(body.x, body.y + 4.0, body.w, body.h - 10.0);
                for (n, line) in lines.iter().enumerate().skip(first).take(shown) {
                    let y = body.y + TEXT_PAD + n as f32 * TEXT_LINE - up.scroll;
                    // CODE: the line in pieces, each in its colour, each
                    // placed where it was measured to begin (not measured
                    // yet: plain this once).
                    if let Some(spans) = prepared
                        .and_then(|p| p.syntax.get(n))
                        .filter(|s| !s.is_empty())
                    {
                        if let Some(xs) = self.code_xs(line, spans) {
                            push_code_line(
                                &mut g,
                                line,
                                spans,
                                xs,
                                (body.x + TEXT_PAD, y),
                                body.w - 2.0 * TEXT_PAD,
                                clip,
                                ink,
                            );
                            continue;
                        }
                    }
                    g.labels.push(Label {
                        text: line.clone(),
                        pos: (body.x + TEXT_PAD, y),
                        max_w: body.w - 2.0 * TEXT_PAD,
                        font_px: TEXT_PX,
                        line_px: TEXT_LINE,
                        centered: false,
                        dim: false,
                        cache: false,
                        clip: Some(Rect::new(body.x, body.y + 4.0, body.w, body.h - 10.0)),
                        family: Some(crate::options::NERD),
                        color: Some(ink),
                    });
                }
            }
            Some(Body::Info { lines }) => {
                let s = (body.h - 40.0).clamp(64.0, 160.0);
                if up.thumb.is_some() {
                    g.icons.push(clipped(
                        Rect::new(body.x + 26.0, body.y + (body.h - s) / 2.0, s, s),
                        SOFT,
                        open,
                    ));
                }
                let x = body.x + 26.0 + s + 28.0;
                let top = body.y + (body.h - lines.len() as f32 * 24.0) / 2.0;
                for (n, (k, v)) in lines.iter().enumerate() {
                    let y = top + n as f32 * 24.0;
                    g.labels.push(text_label(k, (x, y), 110.0, 13.0, None, dim));
                    g.labels.push(text_label(
                        v,
                        (x + 110.0, y),
                        (r.x + r.w - x - 130.0).max(40.0),
                        13.0,
                        None,
                        ink,
                    ));
                }
            }
            _ => soft_square(&mut g, 1.0),
        }
        // THE PENCIL on a video or a song: the cut, over what is shown.
        if up.trim_edit.is_some() {
            let mut over = GridContent {
                clip: g.clip,
                ..Default::default()
            };
            self.trim_push(&mut g, &mut over, r, open, ink, ground);
            scene.grids.push(g);
            scene.grids.push(over);
            return;
        }
        // THE HEAD: its icon, its name and what it is, Open, ×.
        let parts = parts(r);
        if up.thumb.is_some() {
            g.icons.push(clipped(
                Rect::new(r.x + 14.0, r.y + 12.0, 22.0, 22.0),
                BADGE,
                open,
            ));
        }
        let open_word = match &up.said {
            Some(said) => said.open.clone(),
            None => prepared
                .and_then(|p| p.app.as_deref())
                .and_then(|id| self.entries.iter().find(|e| e.id == id))
                .map_or_else(
                    || tr("Open").to_owned(),
                    |e| fill(tr("Open with {app}"), &[("app", &e.name)]),
                ),
        };
        let open_w = parts.open.w.min(self.peek_word_w(&open_word) + 26.0);
        let open_rect = Rect::new(parts.open.x + parts.open.w - open_w, parts.open.y, open_w, parts.open.h);
        // (A page: the card's round buttons — `peek_page.rs`.)
        let page_left = up.said.as_ref().map(|_| crate::peek_page::head(r)[0].1.x);
        let text_w = (page_left.unwrap_or(open_rect.x) - (r.x + 46.0) - 12.0).max(20.0);
        // (Cut in the middle, as names are: `desktop::fit_name`.)
        let name = self.desktop_top.renderer.as_ref().map_or_else(
            || up.name.clone(),
            |r| crate::desktop::fit_name(&up.name, text_w, &mut |t| r.measure_text(t, 13.5, Some(FONT_BOLD))),
        );
        g.labels.push(text_label(
            &name,
            (r.x + 46.0, r.y + 7.0),
            text_w,
            13.5,
            Some(FONT_BOLD),
            ink,
        ));
        if let Some(said) = &up.said {
            g.labels.push(text_label(
                &said.meta,
                (r.x + 46.0, r.y + 25.0),
                text_w,
                11.5,
                None,
                dim,
            ));
        } else if let Some(p) = prepared {
            let meta = match up.of {
                Some((n, all)) => format!(
                    "{} · {}",
                    fill(
                        tr("{n} of {all}"),
                        &[("n", &n.to_string()), ("all", &all.to_string())]
                    ),
                    p.meta
                ),
                None => p.meta.clone(),
            };
            g.labels.push(text_label(
                &meta,
                (r.x + 46.0, r.y + 25.0),
                text_w,
                11.5,
                None,
                dim,
            ));
        }
        let wash = |rect: Rect| up.ptr.is_some_and(|p| rect.contains(p));
        let pill = |rect: Rect, hot: bool| RectInst {
            rect,
            radius: rect.h / 2.0,
            color: [
                ink[0],
                ink[1],
                ink[2],
                (if hot { 0.26 } else { 0.14 }) * open,
            ],
            glass: 0.0,
            border: 0.0,
        };
        // A PAGE: the card's round buttons, and nothing more (`peek_page.rs`);
        // the one rested on says what it does.
        let mut tip_grid = None;
        if page_left.is_some() {
            let copied = up.notice.as_ref().is_some_and(|(_, at)| at.elapsed().as_secs_f32() < NOTICE_SECS);
            self.page_head_push(&mut g, r, (ink, open), (up.ptr, copied));
            tip_grid = self.page_tip_push(g.clip, r, open, copied);
        } else {
        g.rects.push(pill(open_rect, wash(open_rect)));
        g.labels.push(Label {
            centered: true,
            ..text_label(
                &open_word,
                (open_rect.x + open_rect.w / 2.0, open_rect.y + 5.0),
                open_rect.w - 8.0,
                12.0,
                Some(FONT_BOLD),
                ink,
            )
        });
        if self.peek_editable() {
            g.rects.push(pill(parts.pencil, wash(parts.pencil)));
            g.labels.push(Label {
                centered: true,
                family: Some(crate::options::NERD),
                ..text_label(
                    "\u{f040}",
                    (parts.pencil.x + parts.pencil.w / 2.0, parts.pencil.y + 5.0),
                    parts.pencil.w,
                    13.0,
                    None,
                    ink,
                )
            });
        }
        g.rects.push(pill(parts.close, wash(parts.close)));
        g.labels.push(Label {
            centered: true,
            ..text_label(
                "×",
                (parts.close.x + parts.close.w / 2.0, parts.close.y + 5.0),
                parts.close.w,
                13.0,
                None,
                ink,
            )
        });
        }
        // A word for a moment ("Saved"), over the picture.
        let mut notice_grid = None;
        // (A page says it in its Copy link: `peek_page.rs`.)
        if let Some((word, at)) = up.notice.as_ref().filter(|_| up.said.is_none()) {
            let age = at.elapsed().as_secs_f32();
            if age < NOTICE_SECS {
                let fade = open * (1.0 - ((age - NOTICE_SECS + 0.6) / 0.6).clamp(0.0, 1.0));
                let w = self.peek_word_w(word) + 30.0;
                let pill = Rect::new(r.x + (r.w - w) / 2.0, r.y + HEAD + 14.0, w, 28.0);
                let mut ng = GridContent {
                    clip: g.clip,
                    ..Default::default()
                };
                ng.rects.push(RectInst {
                    rect: pill,
                    radius: 14.0,
                    color: [0.06, 0.07, 0.09, 0.85 * fade],
                    glass: 0.0,
                    border: 0.0,
                });
                ng.labels.push(Label {
                    centered: true,
                    ..text_label(
                        word,
                        (pill.x + pill.w / 2.0, pill.y + 6.0),
                        w,
                        12.5,
                        Some(FONT_BOLD),
                        [1.0, 1.0, 1.0, fade],
                    )
                });
                notice_grid = Some(ng);
            }
        }
        scene.grids.push(g);
        scene.grids.extend(zoom_grid);
        scene.grids.extend(over_page);
        scene.grids.extend(notice_grid);
        scene.grids.extend(tip_grid);
    }

    pub(crate) fn peek_word_w(&self, word: &str) -> f32 {
        // (Close enough without shaping: the bold sans is ~0.56 em a letter.)
        word.chars().count() as f32 * 12.0 * 0.56
    }
}

/// The head's buttons, for a frame at `r`: ×, the pencil, Open.
struct Parts {
    open: Rect,
    pencil: Rect,
    close: Rect,
}

fn parts(r: Rect) -> Parts {
    let close = Rect::new(r.x + r.w - 12.0 - 26.0, r.y + 10.0, 26.0, 26.0);
    let pencil = Rect::new(close.x - 8.0 - 32.0, r.y + 10.0, 32.0, 26.0);
    let open_w = (r.w * 0.4).min(240.0);
    Parts {
        open: Rect::new(pencil.x - 8.0 - open_w, r.y + 10.0, open_w, 26.0),
        pencil,
        close,
    }
}

/// A video's bar, for a frame at `r`: a pill at the foot of the picture
/// with play/pause, the line of where it is, and the time.
struct VideoBar {
    pill: Rect,
    play: Rect,
    track: Rect,
    /// Keep the moment on screen as a photo.
    snap: Rect,
}

fn video_bar(r: Rect) -> VideoBar {
    let w = (r.w - 48.0).clamp(160.0, 560.0);
    let pill = Rect::new(r.x + (r.w - w) / 2.0, r.y + r.h - 16.0 - 36.0, w, 36.0);
    let play = Rect::new(pill.x + 6.0, pill.y + 4.0, 28.0, 28.0);
    let snap = Rect::new(pill.x + pill.w - 6.0 - 28.0, pill.y + 4.0, 28.0, 28.0);
    let track = Rect::new(
        play.x + play.w + 12.0,
        pill.y + 6.0,
        (snap.x - 112.0 - (play.x + play.w + 12.0)).max(20.0),
        24.0,
    );
    VideoBar {
        pill,
        play,
        track,
        snap,
    }
}

/// A song's body, for a frame at `r`: its cover, its loudness, its button.
struct SongParts {
    cover: Rect,
    wave: Rect,
    play: Rect,
}

fn song_parts(r: Rect) -> SongParts {
    let pad = 24.0;
    let side = (r.h - HEAD - 2.0 * pad).max(40.0);
    let cover = Rect::new(r.x + pad, r.y + HEAD + pad, side, side);
    let x = cover.x + side + 26.0;
    let w = (r.x + r.w - pad - x).max(40.0);
    let wave = Rect::new(x, cover.y + 66.0, w, (side - 66.0 - 58.0).max(20.0));
    let play = Rect::new(x, cover.y + side - 40.0, 40.0, 40.0);
    SongParts { cover, wave, play }
}

/// Where a line's coloured pieces begin and end, in characters.
fn cuts_of(spans: &[crate::peek_syntax::Span]) -> Vec<u16> {
    spans
        .iter()
        .flat_map(|s| [s.from, s.from + s.len])
        .collect()
}

/// A kind of code's colour: the palette's hue for it, as the ink is (its
/// space, its alpha).
fn code_hue(class: crate::peek_syntax::Class, ink: [f32; 4]) -> [f32; 4] {
    use crate::peek_syntax::Class;
    let slot = match class {
        // (base16's comment grey is too faint on the frame.)
        Class::Comment => 0x04,
        Class::Str => 0x0B,
        Class::Num => 0x09,
        Class::Key => 0x0E,
        Class::Type => 0x0A,
    };
    let mut c = crate::palette::linear(crate::palette::current().c[slot]);
    // (A comment a third of the way to the ink: quieter than the code,
    // still read.)
    if class == Class::Comment {
        for i in 0..3 {
            c[i] += (ink[i] - c[i]) * 0.35;
        }
    }
    c[3] = ink[3];
    c
}

/// A line of code at `at` as a label a piece, each in its colour, at the
/// places measured (`xs`: each span's start and end).
#[allow(clippy::too_many_arguments)]
pub(crate) fn push_code_line(
    g: &mut GridContent,
    line: &str,
    spans: &[crate::peek_syntax::Span],
    xs: &[f32],
    at: (f32, f32),
    room: f32,
    clip: Rect,
    ink: [f32; 4],
) {
    let chars: Vec<char> = line.chars().collect();
    let mut piece = |a: usize, b: usize, x: f32, color: [f32; 4]| {
        let b = b.min(chars.len());
        if a >= b || chars[a..b].iter().all(|c| c.is_whitespace()) || x >= room {
            return;
        }
        g.labels.push(Label {
            text: chars[a..b].iter().collect(),
            pos: (at.0 + x, at.1),
            max_w: room - x,
            font_px: TEXT_PX,
            line_px: TEXT_LINE,
            centered: false,
            dim: false,
            cache: false,
            clip: Some(clip),
            family: Some(crate::options::NERD),
            color: Some(color),
        });
    };
    let (mut from, mut from_x) = (0, 0.0);
    for (k, s) in spans.iter().enumerate() {
        let (a, b) = (s.from as usize, (s.from + s.len) as usize);
        let (Some(&xa), Some(&xb)) = (xs.get(2 * k), xs.get(2 * k + 1)) else {
            break;
        };
        piece(from, a, from_x, ink);
        piece(a, b, xa, code_hue(s.class, ink));
        (from, from_x) = (b, xb);
    }
    piece(from, chars.len(), from_x, ink);
}

pub(crate) fn text_label(
    text: &str,
    pos: (f32, f32),
    max_w: f32,
    px: f32,
    family: Option<&'static str>,
    color: [f32; 4],
) -> Label {
    Label {
        text: text.to_owned(),
        pos,
        max_w,
        font_px: px,
        line_px: px * 1.3,
        centered: false,
        dim: false,
        // (The frame grows: its widths change, and a cached shape keeps the
        // width it was first made at.)
        cache: false,
        clip: None,
        family,
        color: Some(color),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_cut_in_tiles_that_meet() {
        let mut img = image::RgbaImage::new(600, 300);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgba([(x % 256) as u8, (y % 256) as u8, 7, 255]);
        }
        let (cols, rows, tiles) = cut(&img);
        assert_eq!((cols, rows), (3, 2));
        assert_eq!(tiles.len(), 6);
        // Tile (1, 0) begins one texel before x = STEP: that texel is the
        // picture's, so the two tiles agree where they overlap.
        let at = |t: &Vec<u8>, x: u32, y: u32| t[((y * TILE + x) * 4) as usize];
        assert_eq!(at(&tiles[1], 0, 1), ((STEP - 1) % 256) as u8);
        assert_eq!(at(&tiles[1], 1, 1), (STEP % 256) as u8);
    }

    #[test]
    fn a_big_picture_fits_the_tiles() {
        let (w, h) = tile_fit((6000, 4000), (3000, 1800));
        assert!(w.div_ceil(STEP) * h.div_ceil(STEP) <= TILES);
        assert!((w as f32 / h as f32 - 1.5).abs() < 0.02, "its shape kept");
        assert_eq!(
            tile_fit((300, 200), (3000, 1800)),
            (300, 200),
            "a small one as it is"
        );
    }

    #[test]
    fn text_is_read_and_a_picture_is_not() {
        let dir = std::env::temp_dir().join(format!("ql-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = dir.join("notes.md");
        std::fs::write(&t, "# hi\n\tthere\n").unwrap();
        let p = prepare(&t, 0, (800, 600), [255; 3]);
        assert!(matches!(&p.body, Body::Text { lines } if lines[1] == "    there"));
        let img = dir.join("a.png");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([10, 20, 30, 128]))
            .save(&img)
            .unwrap();
        let p = prepare(&img, 0, (800, 600), [255; 3]);
        assert!(matches!(
            &p.body,
            Body::Picture(Pic {
                w: 40,
                h: 20,
                cols: 1,
                rows: 1,
                ..
            })
        ));
        assert!(p.meta.contains("40 × 20"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_song_lays_out_inside_its_frame() {
        let r = Rect::new(100.0, 100.0, SONG.0, SONG.1 + HEAD);
        let s = song_parts(r);
        for part in [s.cover, s.wave, s.play] {
            assert!(
                part.x >= r.x
                    && part.y >= r.y + HEAD
                    && part.x + part.w <= r.x + r.w + 0.1
                    && part.y + part.h <= r.y + r.h + 0.1
            );
        }
        assert!(
            s.wave.y + s.wave.h <= s.play.y,
            "the button under the loudness"
        );
    }
}

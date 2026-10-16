//! QUICK LOOK (Max, 2026-10-16, the Quick Look Lab —
//! https://claude.ai/artifact/MGRESgAmUyjC6rq1GL3ytn): Space on a thing in a
//! file window shows it, big, over everything, GROWING OUT OF ITS ICON; the
//! arrows walk to its neighbours with it up; Space or Escape put it back into
//! its icon. "Like Finder or better", for as many kinds as can be, VERY fast
//! even on weak machines, and ONE look for every kind.
//!
//! One frame for every kind: a head (the thing's icon, its name, "kind ·
//! size · dimensions", "Open with <app>", a ×) and a body. What fills the
//! body is drawn by us; what reads a kind only hands over pixels or text:
//!
//! - a PICTURE (what `image` reads, SVG, a camera's RAW by the JPEG it
//!   carries) is decoded at the size it will be shown and cut in TILES of the
//!   icon array (`TILE` square, a texel of overlap round each so they meet
//!   without a seam), drawn clipped to the frame's corner (`icon.wgsl`'s
//!   clipped-picture mode) — no pipeline of its own;
//! - TEXT (what reads as text) is its first lines, in the bar's mono font;
//! - anything else is its picture from the file window and what it is.
//!
//! FAST: the frame shows at once with the window's own thumbnail blown up,
//! and the sharp one fades in when it is ready. It is usually ready already:
//! what the keys are on in a file window, and its neighbours, are PREPARED
//! in the background (`want`) — one low-priority thread, the newest wish
//! first, the stale ones dropped — and kept within a budget (`BUDGET`).
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
use crate::App;

/// A tile is one layer of the icon array; it carries `STEP` texels of the
/// picture and the one beyond each edge again, so neighbours overlap.
const TILE: u32 = ICON_SIZE;
const STEP: u32 = TILE - 2;
/// The most tiles a picture is cut into: a bigger one is decoded smaller.
pub(crate) const TILES: u32 = 64;
/// The layers on the top surface: the tiles, the soft first picture, the
/// head's icon.
pub(crate) const LAYERS: u32 = TILES + 2;
const SOFT: u32 = TILES;
const BADGE: u32 = TILES + 1;
/// What is kept prepared, at most.
const BUDGET: usize = 96 << 20;
/// Of the screen, at most.
const MAX: f32 = 0.82;
const HEAD: f32 = 46.0;
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

/// What a thing is shown as, once prepared.
pub(crate) enum Body {
    /// A picture of `w`×`h` texels, cut in `cols`×`rows` tiles (their mip
    /// chains, row by row).
    Picture {
        w: u32,
        h: u32,
        cols: u32,
        rows: u32,
        tiles: Vec<Vec<u8>>,
    },
    /// Its first lines.
    Text { lines: Vec<String> },
    /// What it is: name → value.
    Info { lines: Vec<(&'static str, String)> },
}

pub(crate) struct Prepared {
    pub body: Body,
    /// The head's second line: "JPEG · 4.2 MB · 6000 × 4000".
    pub meta: String,
    /// The app that opens it (a `.desktop` id), as the system says.
    pub app: Option<String>,
    bytes: usize,
}

/// What is up.
pub(crate) struct Up {
    /// The file window it came from (its keys come here).
    pub win: u32,
    pub path: PathBuf,
    name: String,
    /// The thing's picture in its window (the soft first frame, the badge).
    thumb: Option<Vec<u8>>,
    /// Its icon on the screen: where it grows from and goes back to.
    from: Rect,
    /// Where this motion starts, and how far along it is (0..1).
    start: Rect,
    t: f32,
    closing: bool,
    /// The frame as last drawn.
    rect: Rect,
    /// The sharp body over the soft one (0..1).
    sharp: f32,
    /// Text scrolled by.
    scroll: f32,
    /// The pointer on the surface, for the buttons' wash.
    ptr: Option<(f32, f32)>,
}

struct Job {
    path: PathBuf,
    /// The most texels a picture is decoded to.
    max: (u32, u32),
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
    cache: HashMap<PathBuf, Arc<Prepared>>,
    /// Oldest first, for the budget.
    order: Vec<PathBuf>,
    bytes: usize,
    queue: Option<Arc<Queue>>,
    /// Asked of the worker and not back yet.
    asked: HashSet<PathBuf>,
    /// For each file window, the thing its keys were on when its
    /// neighbours were last wished for.
    seen: HashMap<u32, String>,
}

impl QuickLook {
    pub(crate) fn is_up(&self) -> bool {
        self.up.is_some()
    }

    fn get(&mut self, path: &Path) -> Option<Arc<Prepared>> {
        let p = self.cache.get(path)?.clone();
        if let Some(i) = self.order.iter().position(|o| o == path) {
            let o = self.order.remove(i);
            self.order.push(o);
        }
        Some(p)
    }

    fn keep(&mut self, path: PathBuf, p: Prepared) {
        self.bytes += p.bytes;
        if let Some(old) = self.cache.insert(path.clone(), Arc::new(p)) {
            self.bytes -= old.bytes;
            self.order.retain(|o| o != &path);
        }
        self.order.push(path);
        // (Never what is up.)
        let up = self.up.as_ref().map(|u| u.path.clone());
        while self.bytes > BUDGET && self.order.len() > 1 {
            let Some(i) = self.order.iter().position(|o| Some(o) != up.as_ref()) else {
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
        || [".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".tif", ".tiff", ".avif", ".ico", ".tga", ".qoi", ".pnm", ".ppm"]
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

fn default_app(mime: &str) -> Option<String> {
    let out = std::process::Command::new("xdg-mime")
        .args(["query", "default", mime])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let id = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!id.is_empty()).then(|| id.trim_end_matches(".desktop").to_owned())
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
    let img = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let own = (img.width(), img.height());
    // (`thumbnail`: the quick reduction; a picture smaller stays as it is.)
    let img = if own.0 > max.0 || own.1 > max.1 {
        img.thumbnail(max.0, max.1)
    } else {
        img
    };
    let mut rgba = img.to_rgba8();
    for px in rgba.pixels_mut() {
        let a = px[3] as u32;
        if a < 255 {
            for c in 0..3 {
                px[c] = (px[c] as u32 * a / 255) as u8;
            }
        }
    }
    Some((rgba, own))
}

/// The most texels that cut into `TILES` tiles at `max`'s shape.
fn tile_fit(own: (u32, u32), max: (u32, u32)) -> (u32, u32) {
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

/// Make `path` ready to be shown.
fn prepare(path: &Path, max: (u32, u32)) -> Prepared {
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
    if !dir && is_picture(&name) {
        let started = Instant::now();
        // (Its own size first: the tiles' budget decides how big it is decoded.)
        let own = image::image_dimensions(path).unwrap_or(max);
        if let Some((img, own)) = decode(path, tile_fit(own, max)) {
            let (cols, rows, tiles) = cut(&img);
            debug!(
                "quick look: {} at {}×{} in {:?}",
                path.display(),
                img.width(),
                img.height(),
                started.elapsed()
            );
            let bytes = tiles.iter().map(Vec::len).sum();
            return Prepared {
                body: Body::Picture {
                    w: img.width(),
                    h: img.height(),
                    cols,
                    rows,
                    tiles,
                },
                meta: format!(
                    "{} · {} · {} × {}",
                    kind_word(&name),
                    crate::desktop_props::size_text(size),
                    own.0,
                    own.1
                ),
                app,
                bytes,
            };
        }
    }
    if !dir {
        if let Some(lines) = text_of(path) {
            let bytes = lines.iter().map(String::len).sum();
            let n = lines.len();
            return Prepared {
                body: Body::Text { lines },
                meta: format!(
                    "{} · {} · {}",
                    kind_word(&name),
                    crate::desktop_props::size_text(size),
                    fill(tr("{n} lines"), &[("n", &n)])
                ),
                app,
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
            mime.map_or_else(|| kind_word(&name), str::to_owned)
        },
    ));
    if dir {
        let n = std::fs::read_dir(path).map_or(0, |d| {
            d.filter_map(Result::ok)
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .count()
        });
        lines.push((tr("Contains"), fill(tr("{n} things"), &[("n", &n)])));
    } else {
        lines.push((tr("Size"), crate::desktop_props::size_text(size)));
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
            format!(
                "{} · {}",
                kind_word(&name),
                crate::desktop_props::size_text(size)
            )
        },
        app,
        bytes: 1024,
    }
}

fn spawn(sender: calloop::channel::Sender<(PathBuf, Prepared)>) -> Option<Arc<Queue>> {
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
                let made = prepare(&job.path, job.max);
                if sender.send((job.path, made)).is_err() {
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
pub(crate) struct Thing {
    pub path: PathBuf,
    pub name: String,
    pub thumb: Option<Vec<u8>>,
    /// Its icon, on the screen (logical px of the output).
    pub icon: Rect,
}

impl App {
    /// The most texels a picture is decoded to: the frame's biggest, in real
    /// pixels.
    fn peek_max(&self) -> (u32, u32) {
        let (w, h) = self.desktop_top.size;
        let scale = self.surface_scale(crate::fractional::SurfaceKind::DesktopTop);
        (
            ((w as f32 * MAX) * scale) as u32,
            (((h as f32 * MAX) - HEAD) * scale) as u32,
        )
    }

    /// Ask for `paths` to be prepared, the last first; what was asked
    /// before and is not among them is dropped.
    pub(crate) fn peek_want(&mut self, paths: Vec<PathBuf>) {
        if !self.desktop_top_ready() {
            return;
        }
        if self.peek.queue.is_none() {
            let (sender, receiver) = calloop::channel::channel();
            let inserted = self
                .loop_handle
                .insert_source(receiver, |event, _, app: &mut App| {
                    if let calloop::channel::Event::Msg((path, made)) = event {
                        app.peek_ready(path, made);
                    }
                });
            if let Err(e) = inserted {
                warn!("quick look: {e}");
                return;
            }
            self.peek.queue = spawn(sender);
        }
        let max = self.peek_max();
        let todo: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| !self.peek.cache.contains_key(p))
            .collect();
        let Some(queue) = self.peek.queue.clone() else {
            return;
        };
        let Ok(mut jobs) = queue.jobs.lock() else {
            return;
        };
        jobs.clear();
        self.peek.asked.retain(|p| todo.contains(p));
        for path in todo {
            self.peek.asked.insert(path.clone());
            jobs.push(Job { path, max });
        }
        queue.ready.notify_one();
    }

    fn peek_ready(&mut self, path: PathBuf, made: Prepared) {
        self.peek.asked.remove(&path);
        let shown = self.peek.up.as_ref().is_some_and(|u| u.path == path);
        self.peek.keep(path, made);
        if shown {
            // Its real shape: the frame goes there from where it is.
            if let Some(up) = self.peek.up.as_mut() {
                up.start = up.rect;
                up.t = 0.0;
                up.sharp = 0.0;
            }
            self.desktop_top.peek_tiles = None;
            self.request_desktop_top_draw();
        }
    }

    /// What a file window's keys are on moved: its neighbours are wished for.
    pub(crate) fn peek_follow_keys(&mut self, win: u32, at: &str, near: Vec<PathBuf>) {
        if self.peek.seen.get(&win).is_some_and(|s| s == at) {
            return;
        }
        self.peek.seen.insert(win, at.to_owned());
        self.peek_want(near);
    }

    /// Show `thing` of file window `win` (or, if something is up, walk to it).
    pub(crate) fn peek_show(&mut self, win: u32, thing: Thing) {
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
        // (Shown: the newest of what is kept.)
        let ready = self.peek.get(&thing.path).is_some();
        let was = self.peek.up.as_ref().filter(|u| !u.closing).map(|u| u.rect);
        let start = was.unwrap_or(thing.icon);
        self.peek.up = Some(Up {
            win,
            path: thing.path.clone(),
            name: thing.name,
            thumb: thing.thumb,
            from: thing.icon,
            start,
            t: 0.0,
            closing: false,
            rect: start,
            sharp: if ready { 1.0 } else { 0.0 },
            scroll: 0.0,
            ptr: None,
        });
        self.desktop_top.peek_tiles = None;
        self.desktop_top.peek_soft = false;
        if !self.peek.cache.contains_key(&thing.path) && !self.peek.asked.contains(&thing.path) {
            let mut all: Vec<PathBuf> = self.peek.asked.iter().cloned().collect();
            all.push(thing.path);
            self.peek_want(all);
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
        up.closing = true;
        up.start = up.rect;
        up.t = 0.0;
        self.request_desktop_top_draw();
    }

    /// Open it with its app (as a double click would) and close.
    fn peek_open_app(&mut self) {
        let Some(up) = self.peek.up.as_ref() else {
            return;
        };
        let exec = format!(
            "xdg-open {}",
            crate::launch::shell_quote(&up.path.to_string_lossy())
        );
        if let Err(e) = crate::launch::launch(&exec, false, &self.config.launch.terminal) {
            warn!("quick look: opening {} failed: {e:#}", up.path.display());
        }
        self.peek_close();
    }

    /// A key of file window `win` while Quick Look is up for it: whether it
    /// was Quick Look's (else the window goes on with it — the arrows walk,
    /// and Quick Look follows: `peek_show` again).
    pub(crate) fn peek_key(
        &mut self,
        win: u32,
        key: smithay_client_toolkit::seat::keyboard::Keysym,
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
        match key {
            Keysym::space | Keysym::Escape => self.peek_close(),
            Keysym::Return | Keysym::KP_Enter => self.peek_open_app(),
            Keysym::Left | Keysym::Right | Keysym::Up | Keysym::Down => return false,
            Keysym::Page_Up | Keysym::Page_Down => {
                let by = if key == Keysym::Page_Up { -1.0 } else { 1.0 };
                let page = self.peek.up.as_ref().map_or(0.0, |u| u.rect.h - HEAD) * 0.9;
                self.peek_scroll(by * page);
            }
            _ => {}
        }
        true
    }

    fn peek_scroll(&mut self, by: f32) {
        let body_h = self.peek.up.as_ref().map_or(0.0, |u| u.rect.h - HEAD);
        let Some(path) = self.peek.up.as_ref().map(|u| u.path.clone()) else {
            return;
        };
        let Some(p) = self.peek.cache.get(&path).cloned() else {
            return;
        };
        let Body::Text { lines } = &p.body else {
            return;
        };
        let most = (lines.len() as f32 * TEXT_LINE + 2.0 * TEXT_PAD - body_h).max(0.0);
        if let Some(up) = self.peek.up.as_mut() {
            up.scroll = (up.scroll + by).clamp(0.0, most);
        }
        self.request_desktop_top_draw();
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
                self.request_desktop_top_draw();
            }
            Event::Leave { .. } => {
                if let Some(up) = self.peek.up.as_mut() {
                    up.ptr = None;
                }
                self.request_desktop_top_draw();
            }
            Event::Axis {
                axis: WEnum::Value(Axis::VerticalScroll),
                value,
                ..
            } => self.peek_scroll(*value as f32),
            Event::Button {
                state: WEnum::Value(ButtonState::Pressed),
                ..
            } => {
                let Some(up) = self.peek.up.as_ref() else {
                    return;
                };
                let at = up.ptr.unwrap_or((-1.0, -1.0));
                let parts = parts(up.rect);
                if parts.close.contains(at) {
                    self.peek_close();
                } else if parts.open.contains(at) {
                    self.peek_open_app();
                } else if !up.rect.contains(at) {
                    self.peek_close();
                }
            }
            _ => {}
        }
    }

    /// Where the frame is going: the shape its body wants, centred.
    fn peek_target(&self, up: &Up) -> Rect {
        let (sw, sh) = (
            self.desktop_top.size.0 as f32,
            self.desktop_top.size.1 as f32,
        );
        let scale = self.surface_scale(crate::fractional::SurfaceKind::DesktopTop);
        let (mw, mh) = (sw * MAX, sh * MAX - HEAD);
        let (w, h) = match self.peek.cache.get(&up.path).map(|p| &p.body) {
            Some(Body::Picture { w, h, .. }) => {
                // (Never bigger than the picture itself.)
                let (pw, ph) = (*w as f32 / scale, *h as f32 / scale);
                let k = (mw / pw).min(mh / ph).min(1.0);
                ((pw * k).max(380.0), ph * k)
            }
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
        let Some(target) = self.peek.up.as_ref().map(|u| {
            if u.closing {
                u.from
            } else {
                self.peek_target(u)
            }
        }) else {
            return false;
        };
        let ready = self
            .peek
            .up
            .as_ref()
            .is_some_and(|u| self.peek.cache.contains_key(&u.path));
        let Some(up) = self.peek.up.as_mut() else {
            return false;
        };
        up.t = (up.t + dt / GROW_SECS).min(1.0);
        up.rect = lerp(up.start, target, ease_out(up.t));
        if ready && up.sharp < 1.0 {
            up.sharp = (up.sharp + dt / SHARP_SECS).min(1.0);
        }
        if up.closing && up.t >= 1.0 {
            self.peek.up = None;
            return true;
        }
        up.t < 1.0 || (ready && up.sharp < 1.0)
    }

    /// Put what is shown in the top surface's layers.
    pub(crate) fn peek_upload(&mut self) {
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
        if self.desktop_top.peek_tiles.as_ref() != Some(&up.path) {
            if let Some(Body::Picture { tiles, .. }) =
                self.peek.cache.get(&up.path).map(|p| &p.body)
            {
                for (i, tile) in tiles.iter().enumerate() {
                    renderer.update_icon_layer(i as u32, tile);
                }
            }
            if self.peek.cache.contains_key(&up.path) {
                self.desktop_top.peek_tiles = Some(up.path.clone());
            }
        }
    }

    /// Quick Look into `scene`.
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
        let clip = [r.x, r.y, r.x + r.w, r.y + r.h];
        let clipped = |rect: Rect, layer: u32, a: f32| IconInst {
            rect,
            layer,
            tint: clip,
            ring: -1.6,
            plate: [WINDOW_CORNER, a, 0.0, 0.0],
        };
        let body = Rect::new(r.x, r.y + HEAD, r.w, (r.h - HEAD).max(0.0));
        let prepared = self.peek.cache.get(&up.path);
        let mut ink = pal.ink;
        ink[3] *= open;
        let mut dim = ink;
        dim[3] *= 0.6;
        // THE BODY.
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
            Some(Body::Picture {
                w, h, cols, rows, ..
            }) if self.desktop_top.peek_tiles.as_ref() == Some(&up.path) => {
                let kx = body.w / *w as f32;
                let ky = body.h / *h as f32;
                soft_square(&mut g, soft);
                for row in 0..*rows {
                    for col in 0..*cols {
                        let x = body.x + (col * STEP) as f32 * kx - kx;
                        let y = body.y + (row * STEP) as f32 * ky - ky;
                        g.icons.push(clipped(
                            Rect::new(x, y, TILE as f32 * kx, TILE as f32 * ky),
                            row * cols + col,
                            // (Opaque: tiles overlap by a texel, and a
                            // see-through picture would show that as a grid.)
                            if up.closing { open } else { up.sharp },
                        ));
                    }
                }
            }
            Some(Body::Text { lines }) => {
                let first = (up.scroll / TEXT_LINE).floor().max(0.0) as usize;
                let shown = ((body.h / TEXT_LINE).ceil() as usize + 2)
                    .min(lines.len().saturating_sub(first));
                for (n, line) in lines.iter().enumerate().skip(first).take(shown) {
                    let y = body.y + TEXT_PAD + n as f32 * TEXT_LINE - up.scroll;
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
        // THE HEAD: its icon, its name and what it is, Open, ×.
        let parts = parts(r);
        if up.thumb.is_some() {
            g.icons.push(clipped(
                Rect::new(r.x + 14.0, r.y + 12.0, 22.0, 22.0),
                BADGE,
                open,
            ));
        }
        let open_word = prepared
            .and_then(|p| p.app.as_deref())
            .and_then(|id| self.entries.iter().find(|e| e.id == id))
            .map_or_else(
                || tr("Open").to_owned(),
                |e| fill(tr("Open with {app}"), &[("app", &e.name)]),
            );
        let open_w = parts.open.w.min(self.peek_word_w(&open_word) + 26.0);
        let open_rect = Rect::new(
            parts.open.x + parts.open.w - open_w,
            parts.open.y,
            open_w,
            parts.open.h,
        );
        let text_w = (open_rect.x - (r.x + 46.0) - 12.0).max(20.0);
        g.labels.push(text_label(
            &up.name,
            (r.x + 46.0, r.y + 7.0),
            text_w,
            13.5,
            Some(FONT_BOLD),
            ink,
        ));
        if let Some(p) = prepared {
            g.labels.push(text_label(
                &p.meta,
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
        scene.grids.push(g);
    }

    fn peek_word_w(&self, word: &str) -> f32 {
        // (Close enough without shaping: the bold sans is ~0.56 em a letter.)
        word.chars().count() as f32 * 12.0 * 0.56
    }
}

/// The head's buttons, for a frame at `r`.
struct Parts {
    open: Rect,
    close: Rect,
}

fn parts(r: Rect) -> Parts {
    let close = Rect::new(r.x + r.w - 12.0 - 26.0, r.y + 10.0, 26.0, 26.0);
    let open_w = (r.w * 0.4).min(240.0);
    Parts {
        open: Rect::new(close.x - 8.0 - open_w, r.y + 10.0, open_w, 26.0),
        close,
    }
}

fn text_label(
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
        let p = prepare(&t, (800, 600));
        assert!(matches!(&p.body, Body::Text { lines } if lines[1] == "    there"));
        let img = dir.join("a.png");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([10, 20, 30, 128]))
            .save(&img)
            .unwrap();
        let p = prepare(&img, (800, 600));
        assert!(matches!(
            &p.body,
            Body::Picture {
                w: 40,
                h: 20,
                cols: 1,
                rows: 1,
                ..
            }
        ));
        assert!(p.meta.contains("40 × 20"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

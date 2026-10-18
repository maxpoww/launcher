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
//! What follows the colours already (`wall_follow`) is never tinted. What
//! MOVES (a video, a GIF: played by `mpvpaper`) is graded on the screen by an
//! mpv shader that does exactly `Grade::apply` (`grade_shader`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

use crate::display::GolemSettings;
use crate::palette::Rgb;

type Filter = [[f32; 4]; 7];

/// How long the slider must rest before the wallpaper is made again (the
/// icons' own pace).
const REST: Duration = Duration::from_millis(400);

/// How many copies are kept (each is megabytes). The one in use stays.
const KEEP: usize = 4;

/// The drawing of a copy: a new look makes new files.
const VERSION: u64 = 2;

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

/// Whether the picture at `path` is one the slider tints: a still or a
/// moving one (Max, 2026-10-18: the GIFs and videos too), that is
/// not the colours' own picture nor a tile of the first kind, with a filter
/// that does something.
pub(crate) fn tintable(path: &str, f: &Filter) -> bool {
    active(f) && !crate::wall_follow::is_wall(path) && !crate::wall_follow::is_tile(path) && !is_copy(path)
}

/// Whether the slider tints `path` by making a COPY of it: a still. What
/// moves is graded on the screen by a shader (`grade_shader`), its tile
/// like a still's.
pub(crate) fn copyable(path: &str, f: &Filter) -> bool {
    tintable(path, f) && !crate::controls::wallpaper_moves(path)
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

/// THE WALLPAPERS' GRADE (Max, 2026-10-18: "the wallpapers are poorly
/// done"). Not the icons' maths: that one snaps every hue to the nearest of
/// the palette's — right for an icon's few flat colours — and on a
/// photograph its smooth hues broke into bands and speckled stripes. Here a
/// picture keeps its own light, every pixel's lightness as it is; its colour
/// comes from the palette laid out as a ramp from its darkest colour to its
/// lightest (a gradient map), mixed with the picture's own by the slider.
/// Smooth in, smooth out. (A grey palette: the picture in black and white.)
pub(crate) struct Grade {
    t: f32,
    /// The palette's colours as OKLab (L, a, b), darkest first.
    stops: Vec<(f32, f32, f32)>,
}

impl Grade {
    pub(crate) fn of(f: &Filter) -> Grade {
        let n = (f[0][2].max(0.0) as usize).min(6);
        let mut stops: Vec<(f32, f32, f32)> = (1..=n)
            .map(|k| {
                let [l, c, h, _] = f[k];
                (l, c * h.cos(), c * h.sin())
            })
            .collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        Grade { t: f[0][0].clamp(0.0, 1.0), stops }
    }

    /// The ramp's colour (a, b) at lightness `l`.
    fn ramp(&self, l: f32) -> (f32, f32) {
        let st = &self.stops;
        let Some(first) = st.first() else { return (0.0, 0.0) };
        if l <= first.0 {
            return (first.1, first.2);
        }
        for w in st.windows(2) {
            if l <= w[1].0 {
                let u = ((l - w[0].0) / (w[1].0 - w[0].0).max(1e-6)).clamp(0.0, 1.0);
                return (w[0].1 + (w[1].1 - w[0].1) * u, w[0].2 + (w[1].2 - w[0].2) * u);
            }
        }
        let last = st[st.len() - 1];
        (last.1, last.2)
    }

    /// One colour through the grade, as sRGB 0..=255, unrounded (the copy
    /// dithers it).
    pub(crate) fn apply(&self, c: Rgb) -> [f32; 3] {
        if self.t <= 0.0 {
            return c.map(f32::from);
        }
        let [l, a, b] = oklab(c);
        let (mut ra, mut rb) = self.ramp(l);
        // (Within what a screen shows at this lightness: near black and near
        // white there is little colour to have, and asking for more clips.)
        let room = 0.6 * l.min(1.0 - l).max(0.0);
        let rc = ra.hypot(rb);
        if rc > room {
            ra *= room / rc;
            rb *= room / rc;
        }
        from_oklab(l, a + (ra - a) * self.t, b + (rb - b) * self.t)
    }
}

fn srgb_to_linear(v: u8) -> f32 {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let x = i as f32 / 255.0;
            if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
        })
    })[usize::from(v)]
}

fn oklab(c: Rgb) -> [f32; 3] {
    let [r, g, b] = c.map(srgb_to_linear);
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab(l: f32, a: f32, b: f32) -> [f32; 3] {
    let lp = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let mp = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let sp = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    let lin = [
        4.076_741_7 * lp - 3.307_711_6 * mp + 0.230_969_94 * sp,
        -1.268_438 * lp + 2.609_757_4 * mp - 0.341_319_38 * sp,
        -0.004_196_086 * lp - 0.703_418_6 * mp + 1.707_614_7 * sp,
    ];
    lin.map(|x| {
        let x = x.clamp(0.0, 1.0);
        255.0 * if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
    })
}

/// A little noise per pixel (-0.5..0.5), the same for the same place: the
/// 8-bit copy of a smooth grade dithered instead of banded.
fn noise(i: usize) -> f32 {
    let mut x = i as u32;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x as f32 / u32::MAX as f32 - 0.5
}

/// `px` (sRGB, three bytes a pixel) through the grade, rows shared out to
/// the cores, dithered.
pub(crate) fn filter_pixels(px: &mut [u8], f: &Filter) {
    if !active(f) {
        return;
    }
    let grade = Grade::of(f);
    let grade = &grade;
    let threads = std::thread::available_parallelism().map_or(2, usize::from).min(8);
    let rows = px.len() / 3;
    let chunk = (rows / threads).max(4096) * 3;
    std::thread::scope(|sc| {
        for (n, part) in px.chunks_mut(chunk).enumerate() {
            sc.spawn(move || {
                // (Runs of one colour — a sky, a flat — are common.)
                let mut last: (Rgb, [f32; 3]) = ([0, 0, 0], grade.apply([0, 0, 0]));
                for (i, p) in part.chunks_exact_mut(3).enumerate() {
                    let c = [p[0], p[1], p[2]];
                    if c != last.0 {
                        last = (c, grade.apply(c));
                    }
                    let d = noise(n * chunk / 3 + i);
                    for (o, v) in p.iter_mut().zip(last.1) {
                        *o = (v + d).round().clamp(0.0, 255.0) as u8;
                    }
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
    let grade = Grade::of(f);
    let mut memo: HashMap<u16, Rgb> = HashMap::new();
    for p in base.chunks_exact_mut(4).filter(|p| p[3] > 0) {
        let a = u32::from(p[3]);
        let un = |v: u8| if a == 255 { v } else { ((u32::from(v) * 255 + a / 2) / a).min(255) as u8 };
        let c = [un(p[0]), un(p[1]), un(p[2])];
        let q = (u16::from(c[0] >> 3) << 10) | (u16::from(c[1] >> 3) << 5) | u16::from(c[2] >> 3);
        let out = *memo.entry(q).or_insert_with(|| grade.apply([(c[0] & 0xf8) | 4, (c[1] & 0xf8) | 4, (c[2] & 0xf8) | 4]).map(|v| v.round() as u8));
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
        if path.extension().is_some_and(|e| e == "glsl") {
            continue; // (the shaders have their own pruning)
        }
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
    if copyable(path, &f) {
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
    // (Following the colours is already the palette.)
    let Some(original) = s.wallpaper.clone().filter(|_| !s.wallpaper_follows) else {
        return;
    };
    let f = crate::palette::wall_filter_now();
    if crate::controls::wallpaper_moves(&original) {
        // A moving wallpaper keeps playing: the shader is swapped in place;
        // a player that cannot be told (one from before the shader, or none)
        // is started again the normal way.
        let shader = if tintable(&original, &f) { grade_shader(&f) } else { None };
        if !crate::controls::wallpaper_grade(shader.as_deref()) {
            crate::controls::pick_wallpaper(original);
        }
        return;
    }
    let want = if copyable(&original, &f) { screen().and_then(|size| target(&original, &f, size)) } else { None };
    let now = shown();
    if want.as_ref().map(|p| p.to_string_lossy().into_owned()) == now {
        return;
    }
    // The same road as a pick (its fade); it saves the ORIGINAL again.
    crate::controls::pick_wallpaper(original);
}

// ----------------------------------------------------------------- shader

/// The shader's drawing: a new look makes new files.
const SHADER_VERSION: u64 = 1;

/// The most shaders kept.
const SHADERS_KEPT: usize = 8;

fn glsl_f(v: f32) -> String {
    let s = format!("{v:?}");
    if s.contains(['.', 'e', 'E']) { s } else { format!("{s}.0") }
}

/// THE GRADE AS AN MPV SHADER: `Grade::apply` written in GLSL, the palette's
/// ramp (sorted stops) and the slider baked in as constants, so a video is
/// graded by the graphics card, frame by frame, exactly as a photo is by the
/// processor (Max, 2026-10-18: the slider "should also colour the GIF and
/// animated MP4 wallpapers"). `None` when the filter does nothing.
pub(crate) fn grade_glsl(f: &Filter) -> Option<String> {
    if !active(f) {
        return None;
    }
    let g = Grade::of(f);
    let st = &g.stops;
    let mut ramp = String::new();
    if let Some(first) = st.first() {
        ramp.push_str(&format!("    if (l <= {}) return vec2({}, {});\n", glsl_f(first.0), glsl_f(first.1), glsl_f(first.2)));
        for w in st.windows(2) {
            ramp.push_str(&format!(
                "    if (l <= {l1}) return mix(vec2({a0}, {b0}), vec2({a1}, {b1}), clamp((l - {l0}) / max({l1} - {l0}, 1e-6), 0.0, 1.0));\n",
                l0 = glsl_f(w[0].0), a0 = glsl_f(w[0].1), b0 = glsl_f(w[0].2),
                l1 = glsl_f(w[1].0), a1 = glsl_f(w[1].1), b1 = glsl_f(w[1].2),
            ));
        }
        let last = st[st.len() - 1];
        ramp.push_str(&format!("    return vec2({}, {});\n", glsl_f(last.1), glsl_f(last.2)));
    } else {
        ramp.push_str("    return vec2(0.0);\n");
    }
    Some(format!(
        r#"//!HOOK OUTPUT
//!BIND HOOKED
//!DESC golem wallpaper grade
// Written by Golem: the wallpapers' grade (wall_tint.rs Grade::apply) on the GPU.
const float T = {t};

float s2l(float x) {{ return x <= 0.04045 ? x / 12.92 : pow((x + 0.055) / 1.055, 2.4); }}
float l2s(float x) {{ x = clamp(x, 0.0, 1.0); return x <= 0.0031308 ? 12.92 * x : 1.055 * pow(x, 1.0 / 2.4) - 0.055; }}
float cbrt1(float x) {{ return sign(x) * pow(abs(x), 1.0 / 3.0); }}

vec2 ramp(float l) {{
{ramp}}}

vec4 hook() {{
    vec4 px = HOOKED_texOff(0);
    vec3 c = vec3(s2l(px.r), s2l(px.g), s2l(px.b));
    float l_ = cbrt1(0.41222147 * c.r + 0.53633255 * c.g + 0.05144599 * c.b);
    float m_ = cbrt1(0.2119035 * c.r + 0.6806995 * c.g + 0.10739696 * c.b);
    float s_ = cbrt1(0.08830246 * c.r + 0.28171885 * c.g + 0.6299787 * c.b);
    float L = 0.21045426 * l_ + 0.7936178 * m_ - 0.004072047 * s_;
    float A = 1.9779985 * l_ - 2.4285922 * m_ + 0.4505937 * s_;
    float B = 0.025904037 * l_ + 0.78277177 * m_ - 0.80867577 * s_;
    vec2 r = ramp(L);
    float room = 0.6 * max(min(L, 1.0 - L), 0.0);
    float rc = length(r);
    if (rc > room) r *= room / rc;
    A += (r.x - A) * T;
    B += (r.y - B) * T;
    float lp = pow(L + 0.39633778 * A + 0.21580376 * B, 3.0);
    float mp = pow(L - 0.105561346 * A - 0.06385417 * B, 3.0);
    float sp = pow(L - 0.08948418 * A - 1.2914855 * B, 3.0);
    vec3 o = vec3(4.0767417 * lp - 3.3077116 * mp + 0.23096994 * sp,
                  -1.268438 * lp + 2.6097574 * mp - 0.34131938 * sp,
                  -0.004196086 * lp - 0.7034186 * mp + 1.7076147 * sp);
    return vec4(l2s(o.r), l2s(o.g), l2s(o.b), px.a);
}}
"#,
        t = glsl_f(g.t),
        ramp = ramp,
    ))
}

/// Where the shader for `f` is kept.
fn shader_path(f: &Filter) -> Option<PathBuf> {
    let h = filter_hash(f) ^ SHADER_VERSION.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    Some(copies_dir()?.join(format!("grade-{h:016x}.glsl")))
}

/// The shader file for `f`, written if it is not kept (atomically: a player
/// never reads half of one). `None`: the filter does nothing, or it could
/// not be written.
pub(crate) fn grade_shader(f: &Filter) -> Option<PathBuf> {
    let text = grade_glsl(f)?;
    let out = shader_path(f)?;
    if out.is_file() {
        return Some(out);
    }
    std::fs::create_dir_all(out.parent()?).ok()?;
    let tmp = out.with_extension("tmp");
    if std::fs::write(&tmp, text).is_err() || std::fs::rename(&tmp, &out).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    prune_shaders(&out);
    Some(out)
}

fn prune_shaders(in_use: &Path) {
    let Some(dir) = in_use.parent() else { return };
    let mut found: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "glsl"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .collect();
    found.sort_by_key(|(_, m)| std::cmp::Reverse(*m));
    for (p, _) in found.into_iter().skip(SHADERS_KEPT).filter(|(p, _)| p != in_use) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::filter_rgb;

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
        // (Through the wallpapers' grade, not the icons' maths; dithered by ±1.)
        let out = Grade::of(&f).apply([128, 128, 128]);
        let spread = out.iter().copied().fold(f32::MIN, f32::max) - out.iter().copied().fold(f32::MAX, f32::min);
        assert!(spread > 10.0, "a grey stayed grey: {out:?}");
        let mut px = vec![128u8; 3 * 10000];
        filter_pixels(&mut px, &f);
        for p in [&px[..3], &px[px.len() - 3..]] {
            for k in 0..3 {
                assert!((f32::from(p[k]) - out[k]).abs() <= 1.0, "{p:?} vs {out:?}");
            }
        }
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
    fn what_follows_is_never_tinted_and_what_moves_has_no_copy() {
        let f = jade();
        assert!(tintable("/home/x/Pictures/Wallpapers/a.jpg", &f));
        assert!(tintable("/home/x/Pictures/Wallpapers/a.mp4", &f), "a video's tile is graded");
        assert!(tintable("/home/x/Pictures/Wallpapers/a.gif", &f));
        assert!(copyable("/home/x/Pictures/Wallpapers/a.jpg", &f));
        assert!(!copyable("/home/x/Pictures/Wallpapers/a.mp4", &f), "what moves gets a shader, not a copy");
        assert!(!copyable("/home/x/Pictures/Wallpapers/a.gif", &f));
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
        // Off, the first tile: the file as it is.
        assert_eq!(tile_key(&file, &crate::palette::filter_for("jade", 0, false)), file);
        let gif = dir.join("a.gif").to_string_lossy().into_owned();
        assert!(tile_key(&gif, &f).starts_with(KEY), "a GIF's tile is graded too");
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

    /// The shader's maths written out in Rust, step for step (floats, no
    /// lookup table), for the test below.
    fn glsl_port(g: &Grade, c: [f32; 3]) -> [f32; 3] {
        let s2l = |x: f32| if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
        let l2s = |x: f32| {
            let x = x.clamp(0.0, 1.0);
            if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
        };
        let cb = |x: f32| x.signum() * x.abs().powf(1.0 / 3.0);
        let [r, gg, b] = c.map(s2l);
        let l_ = cb(0.412_221_47 * r + 0.536_332_55 * gg + 0.051_445_99 * b);
        let m_ = cb(0.211_903_5 * r + 0.680_699_5 * gg + 0.107_396_96 * b);
        let s_ = cb(0.088_302_46 * r + 0.281_718_85 * gg + 0.629_978_7 * b);
        let l = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
        let mut a = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
        let mut bb = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;
        let (mut ra, mut rb) = g.ramp(l);
        let room = 0.6 * l.min(1.0 - l).max(0.0);
        let rc = ra.hypot(rb);
        if rc > room {
            ra *= room / rc;
            rb *= room / rc;
        }
        a += (ra - a) * g.t;
        bb += (rb - bb) * g.t;
        let lp = (l + 0.396_337_78 * a + 0.215_803_76 * bb).powi(3);
        let mp = (l - 0.105_561_346 * a - 0.063_854_17 * bb).powi(3);
        let sp = (l - 0.089_484_18 * a - 1.291_485_5 * bb).powi(3);
        [
            l2s(4.076_741_7 * lp - 3.307_711_6 * mp + 0.230_969_94 * sp),
            l2s(-1.268_438 * lp + 2.609_757_4 * mp - 0.341_319_38 * sp),
            l2s(-0.004_196_086 * lp - 0.703_418_6 * mp + 1.707_614_7 * sp),
        ]
    }

    #[test]
    fn the_shader_carries_the_palette_and_the_slider() {
        let f = crate::palette::filter_for("jade", 60, false);
        let g = Grade::of(&f);
        let text = grade_glsl(&f).unwrap();
        assert!(text.starts_with("//!HOOK OUTPUT\n//!BIND HOOKED\n"));
        assert!(text.contains(&format!("const float T = {};", glsl_f(g.t))));
        for (l, a, b) in &g.stops {
            assert!(text.contains(&glsl_f(*l)) && text.contains(&glsl_f(*a)) && text.contains(&glsl_f(*b)), "stop {l} {a} {b}");
        }
        assert_eq!(text.matches("if (l <=").count(), g.stops.len(), "one branch per stop");
        assert!(grade_glsl(&crate::palette::filter_for("jade", 0, false)).is_none(), "level 0 has no shader");
        assert_ne!(shader_path(&f), shader_path(&jade()));
        assert_eq!(glsl_f(1.0), "1.0");
        assert_eq!(glsl_f(-0.25), "-0.25");
    }

    #[test]
    fn the_shaders_maths_is_the_photos_grade() {
        // (A hand-made two-colour palette too: warm dark, pale blue.)
        let mut two = [[0.0f32; 4]; 7];
        two[0] = [0.8, 0.0, 2.0, 0.0];
        two[1] = [0.35, 0.12, 0.9, 0.0];
        two[2] = [0.9, 0.05, 4.0, 0.0];
        for (pal, f) in [("jade 100", jade()), ("jade 55", crate::palette::filter_for("jade", 55, false)), ("two", two)] {
            let g = Grade::of(&f);
            let mut worst = 0.0f32;
            for r in (0..=255).step_by(17) {
                for gr in (0..=255).step_by(17) {
                    for b in (0..=255).step_by(17) {
                        let cpu = g.apply([r as u8, gr as u8, b as u8]);
                        let gpu = glsl_port(&g, [r as f32 / 255.0, gr as f32 / 255.0, b as f32 / 255.0]).map(|x| x * 255.0);
                        for k in 0..3 {
                            worst = worst.max((cpu[k] - gpu[k]).abs());
                        }
                    }
                }
            }
            assert!(worst < 0.05, "{pal}: worst difference {worst}");
        }
    }

    /// Writes the shader for jade at 100 and 50 to $SHADER_OUT (for trying in mpv).
    #[test]
    #[ignore]
    fn write_shaders() {
        let out = std::env::var("SHADER_OUT").unwrap();
        for level in [100u8, 50] {
            let f = crate::palette::filter_for("jade", level, false);
            std::fs::write(format!("{out}/grade{level}.glsl"), grade_glsl(&f).unwrap()).unwrap();
        }
    }

    /// Compares real screenshots of mpv with and without the shader ($SHOTS: a
    /// folder with plain.png, g100.png, g50.png, cleared.png).
    #[test]
    #[ignore]
    fn compare_shots() {
        let dir = std::env::var("SHOTS").unwrap();
        let open = |n: &str| image::open(format!("{dir}/{n}.png")).unwrap().to_rgb8();
        let plain = open("plain");
        let points = [(100u32, 300u32), (300, 300), (600, 300), (850, 300), (1100, 400), (1350, 300), (850, 850), (100, 850), (600, 850), (1100, 850)];
        for (name, level) in [("g100", 100u8), ("g50", 50), ("cleared", 0)] {
            let shot = open(name);
            let g = Grade::of(&crate::palette::filter_for("jade", level, false));
            let mut worst = 0.0f32;
            let mut sum = 0.0f32;
            for &(x, y) in &points {
                let c = plain.get_pixel(x, y).0;
                let want = if level == 0 { c.map(f32::from) } else { g.apply(c) };
                let got = shot.get_pixel(x, y).0;
                let d = (0..3).map(|k| (want[k] - f32::from(got[k])).abs()).fold(0.0f32, f32::max);
                println!("{name} ({x},{y}) plain {c:?} -> cpu {:?} shader {got:?} max diff {d:.1}", want.map(|v| v.round() as i32));
                worst = worst.max(d);
                sum += d;
            }
            println!("{name}: worst {worst:.1}, mean {:.2}", sum / points.len() as f32);
        }
    }

    #[test]
    fn the_grade_is_smooth_and_keeps_the_light() {
        // A hue wheel at one lightness: the icons' maths snapped it to the
        // palette's hues (bands); the grade moves by small steps all round.
        let g = Grade::of(&crate::palette::filter_for("monet", 100, false));
        let wheel: Vec<Rgb> = (0..360).map(|d| crate::palette::tint::from_lch(0.65, 0.12, d as f32)).collect();
        let out: Vec<[f32; 3]> = wheel.iter().map(|&c| g.apply(c)).collect();
        for w in out.windows(2) {
            let step = (0..3).map(|k| (w[0][k] - w[1][k]).abs()).fold(0.0f32, f32::max);
            assert!(step < 12.0, "a jump of {step}");
        }
        // Its lightness is the picture's.
        for &c in &wheel {
            let o = g.apply(c).map(|v| v.round().clamp(0.0, 255.0) as u8);
            assert!((oklab(o)[0] - oklab(c)[0]).abs() < 0.03);
        }
        // Off is the picture, exactly.
        let off = Grade::of(&crate::palette::filter_for("monet", 0, false));
        assert_eq!(off.apply([12, 200, 99]), [12.0, 200.0, 99.0]);
        // A grey palette: black and white.
        let grey = Grade::of(&crate::palette::filter_for("fog", 100, false)).apply([200, 40, 40]);
        assert!((grey[0] - grey[1]).abs() < 2.0 && (grey[1] - grey[2]).abs() < 2.0, "{grey:?}");
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
        // (The picture Max set, 2026-10-18, in his palette.)
        let his = dir.join("pawel-czerwinski-PkiHF-EeT6o-unsplash.jpg");
        if his.is_file() {
            for (pal, level) in [("hacker", 60u8), ("hacker", 100), ("monet", 100)] {
                let (px, w, h) = render(his.to_str().unwrap(), &crate::palette::filter_for(pal, level, false), (1600, 1)).expect("rendered");
                image::save_buffer(out.join(format!("pawel-{pal}-{level}.png")), &px, w, h, image::ColorType::Rgb8).unwrap();
            }
            image::open(&his).unwrap().resize(1600, 1000, image::imageops::FilterType::Triangle).save(out.join("pawel-original.png")).unwrap();
        }
        image::open(&src).unwrap().resize(1600, 1000, image::imageops::FilterType::Triangle).save(out.join("original.png")).unwrap();
    }
}

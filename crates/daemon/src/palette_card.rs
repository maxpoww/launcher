//! THE PALETTE CARD as a picture (Max, 2026-10-16: the cards "as the ones on
//! the mockup, cinematic with the shapes, the shadows"; drawn with the panel's
//! flat rectangles they were "pixeled"). The approved Paper card of
//! `~/Golem/work/palettes` (its CSS, line for line) as an SVG, rasterised once
//! by resvg — real gradients, real blur — and kept as a PNG; the Colours page
//! shows it as the wallpaper box shows a wallpaper.

use std::path::PathBuf;

use crate::palette::{hex, Rgb};

/// The card's size in pixels (16:10, crisp at the biggest tile).
const W: u32 = 640;
const H: u32 = 400;

/// The cache the pictures of this module are kept in.
fn cache_dir() -> Option<PathBuf> {
    let cache = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok()?;
    Some(cache.join("waverunner"))
}

/// Where the cards are kept.
pub(crate) fn cards_dir() -> Option<PathBuf> {
    Some(cache_dir()?.join("palette-cards"))
}

/// A hash of these colours (and a salt: the version of a drawing).
fn colours_hash(colours: &[Rgb], salt: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in colours {
        for b in c {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    (h ^ salt).wrapping_mul(0x0100_0000_01b3)
}

/// Where the card of these colours (ground first, then the soul) is kept.
pub(crate) fn path(colours: &[Rgb]) -> Option<PathBuf> {
    // (The version of the drawing: a new look makes new files.)
    Some(cards_dir()?.join(format!("{:016x}.png", colours_hash(colours, 3))))
}

/// The card of these colours, drawn if it is not there yet.
pub(crate) fn ensure(colours: &[Rgb]) -> Option<PathBuf> {
    let p = path(colours)?;
    if p.is_file() {
        return Some(p);
    }
    let svg = svg(colours);
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(W, H)?;
    let k = W as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    std::fs::create_dir_all(p.parent()?).ok()?;
    let tmp = p.with_extension("tmp");
    pixmap.save_png(&tmp).ok()?;
    std::fs::rename(&tmp, &p).ok()?;
    Some(p)
}

/// DYNAMIC's card: the mockup's colour wheel (`.wheel i`, Max: "un círculo
/// cromático lindo") on the ground of the look — a conic sweep of eleven
/// stops (here 180 wedges), lit from above, darkened toward its rim, a drop
/// shadow under it.
pub(crate) fn wheel(dark: bool) -> Option<PathBuf> {
    let ground: Rgb = if dark { [0x1d, 0x20, 0x26] } else { [0xec, 0xec, 0xe8] };
    // (Its own name in the cache: a wheel is not a palette's colours.)
    let p = path(&[ground, [0xd1, 0x7e, 0xe1]])?;
    if p.is_file() {
        return Some(p);
    }
    let stops: [Rgb; 11] = [
        [0xe8, 0x52, 0x4f], [0xf0, 0x8a, 0x3e], [0xf2, 0xc9, 0x4c], [0xa8, 0xcf, 0x5a], [0x4f, 0xbf, 0x8f], [0x3f, 0xb6, 0xc6],
        [0x4b, 0x8f, 0xe0], [0x7a, 0x6f, 0xe0], [0xb2, 0x68, 0xd6], [0xe0, 0x5a, 0x9a], [0xe8, 0x52, 0x4f],
    ];
    let (cx, cy, r) = (80.0f32, 50.0f32, 34.0f32);
    let mut wedges = String::new();
    const N: usize = 180;
    for i in 0..N {
        let t = i as f32 / N as f32;
        // from 200deg (CSS: 0 = up, clockwise)
        let a0 = (200.0 + 360.0 * t).to_radians();
        let a1 = (200.0 + 360.0 * (t + 1.0 / N as f32) + 0.4).to_radians();
        let seg = t * 10.0;
        let (k, f) = (seg.floor() as usize, seg.fract());
        let c = |j: usize| f32::from(stops[k][j]) * (1.0 - f) + f32::from(stops[(k + 1).min(10)][j]) * f;
        let col = format!("#{:02x}{:02x}{:02x}", c(0).round() as u8, c(1).round() as u8, c(2).round() as u8);
        let pt = |a: f32| (cx + r * a.sin(), cy - r * a.cos());
        let ((x0, y0), (x1, y1)) = (pt(a0), pt(a1));
        wedges.push_str(&format!("<path d=\"M{cx} {cy}L{x0:.3} {y0:.3}A{r} {r} 0 0 1 {x1:.3} {y1:.3}Z\" fill=\"{col}\"/>"));
    }
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="100" viewBox="0 0 160 100">
<defs>
  <radialGradient id="lift" gradientUnits="userSpaceOnUse" cx="80" cy="0" r="144" gradientTransform="translate(80 0) scale(1 0.4861) translate(-80 0)">
    <stop offset="0" stop-color="#ffffff" stop-opacity="{lift}"/><stop offset="0.7" stop-color="#ffffff" stop-opacity="0"/>
  </radialGradient>
  <radialGradient id="shine" cx="0.5" cy="0.38" r="0.5"><stop offset="0" stop-color="#ffffff" stop-opacity="0.25"/><stop offset="0.45" stop-color="#ffffff" stop-opacity="0"/></radialGradient>
  <radialGradient id="rim" cx="0.5" cy="0.5" r="0.5"><stop offset="0.54" stop-color="#000000" stop-opacity="0"/><stop offset="1" stop-color="#000000" stop-opacity="0.165"/></radialGradient>
  <filter id="drop" x="-0.5" y="-0.5" width="2" height="2"><feGaussianBlur stdDeviation="4.2"/></filter>
</defs>
<rect width="160" height="100" fill="#{g}"/>
<rect width="160" height="100" fill="url(#lift)"/>
<circle cx="{cx}" cy="{sy}" r="{sr}" fill="#000000" fill-opacity="0.5" filter="url(#drop)"/>
<g>{wedges}</g>
<circle cx="{cx}" cy="{cy}" r="{r}" fill="url(#rim)"/>
<circle cx="{cx}" cy="{cy}" r="{r}" fill="url(#shine)"/>
<circle cx="{cx}" cy="{cy}" r="{ri}" fill="none" stroke="#ffffff" stroke-opacity="0.12" stroke-width="0.8"/>
</svg>
"##,
        lift = if dark { 0.07 } else { 0.5 },
        g = hex(ground),
        sy = cy + 8.4,
        sr = r - 5.6,
        ri = r - 0.4,
    );
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(W, H)?;
    let k = W as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pixmap.as_mut());
    std::fs::create_dir_all(p.parent()?).ok()?;
    let tmp = p.with_extension("tmp");
    pixmap.save_png(&tmp).ok()?;
    std::fs::rename(&tmp, &p).ok()?;
    Some(p)
}

/// What the capsules wear, in the card's own units (a 160 × 100 board): the
/// sheen down each one, the soft shadow cast to its left and the one on the
/// floor under the fan. Shared by the card and the wallpaper.
const CAPSULE_DEFS: &str = r##"  <linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#ffffff" stop-opacity="0.12"/>
    <stop offset="0.35" stop-color="#ffffff" stop-opacity="0"/>
    <stop offset="0.7" stop-color="#000000" stop-opacity="0"/>
    <stop offset="1" stop-color="#000000" stop-opacity="0.10"/>
  </linearGradient>
  <filter id="cast" x="-1" y="-0.5" width="3" height="2"><feGaussianBlur stdDeviation="2.2"/></filter>
  <filter id="floor" x="-0.5" y="-2" width="2" height="5"><feGaussianBlur stdDeviation="1.6"/></filter>
"##;

/// The capsules' board: the card is 160 × 100; a capsule 24 × 62, each one
/// 12 over the one before (the mockup's `.pill` 15 % × 62 %, 7.5 % apart).
const BOARD: (f32, f32) = (160.0, 100.0);
const CAPSULE: (f32, f32) = (24.0, 62.0);
const STEP: f32 = 12.0;

/// The fan of `n` capsules on the board: its left edge, and its width.
fn fan(n: usize) -> (f32, f32) {
    let span = CAPSULE.0 + (n.max(1) as f32 - 1.0) * STEP;
    ((BOARD.0 - span) / 2.0, span)
}

/// The floor's shadow and the capsules, on the board (the first on top:
/// drawn last). The card draws them as they are; the wallpaper draws them
/// scaled and moved, inside a group.
/// (No thin light along each capsule's top: on the cards and the wallpaper
/// alike it read as "a line on top of the palette colours that shows the
/// colours behind" — Max, 2026-10-18.)
fn capsule_group(soul: &[Rgb]) -> String {
    let (pw, ph) = CAPSULE;
    let (x0, y) = (fan(soul.len()).0, (BOARD.1 - ph) / 2.0);
    let mut out = format!(
        r##"<ellipse cx="80" cy="{fy}" rx="{frx}" ry="3.6" fill="#000000" fill-opacity="0.35" filter="url(#floor)"/>
"##,
        fy = 100.0 - 14.0 - 4.0,
        frx = 160.0 * 0.26,
    );
    for (k, c) in soul.iter().enumerate().rev() {
        let x = x0 + k as f32 * STEP;
        let r = pw / 2.0;
        // box-shadow: -5px 0 9px -3px #00000059 (one pill-width 24 ↔ the
        // mockup's ~17 px: the numbers scaled by 1.4).
        out.push_str(&format!(
            r##"<rect x="{sx}" y="{sy}" width="{sw}" height="{sh}" rx="{sr}" fill="#000000" fill-opacity="0.35" filter="url(#cast)"/>
<clipPath id="p{k}"><rect x="{x}" y="{y}" width="{pw}" height="{ph}" rx="{r}"/></clipPath>
<rect x="{x}" y="{y}" width="{pw}" height="{ph}" rx="{r}" fill="#{c}"/>
<rect x="{x}" y="{y}" width="{pw}" height="{ph}" rx="{r}" fill="url(#sheen)"/>
"##,
            sx = x - 7.0 + 4.2,
            sy = y + 4.2,
            sw = pw - 8.4,
            sh = ph - 8.4,
            sr = r - 4.2,
            c = hex(*c),
        ));
    }
    out
}

/// The ground and the soul of a swatch (ground first, then the soul).
fn parts(colours: &[Rgb]) -> (Rgb, &[Rgb]) {
    let ground = colours.first().copied().unwrap_or([0x1d, 0x20, 0x26]);
    (ground, if colours.len() > 1 { &colours[1..] } else { colours })
}

/// How strong the light on the ground is: a bright ground takes a lot of it,
/// a dark one very little (`.tile`: radial-gradient(90% 70% at 50% 0%,
/// var(--lift) 0%, transparent 70%)).
fn lift(ground: Rgb) -> f32 {
    if crate::palette::rel_lum(ground) > 0.35 {
        0.50
    } else {
        0.07
    }
}

/// The Paper card (the mockup's `.tile` + `.pill` + `.pills::after`), on a
/// 160 × 100 board.
pub(crate) fn svg(colours: &[Rgb]) -> String {
    let (ground, soul) = parts(colours);
    let mut out = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="100" viewBox="0 0 160 100">
<defs>
  <radialGradient id="lift" gradientUnits="userSpaceOnUse" cx="80" cy="0" r="144" gradientTransform="translate(80 0) scale(1 0.4861) translate(-80 0)">
    <stop offset="0" stop-color="#ffffff" stop-opacity="{lift}"/>
    <stop offset="0.7" stop-color="#ffffff" stop-opacity="0"/>
  </radialGradient>
{defs}</defs>
<rect width="160" height="100" fill="#{g}"/>
<rect width="160" height="100" fill="url(#lift)"/>
"##,
        g = hex(ground),
        lift = lift(ground),
        defs = CAPSULE_DEFS,
    );
    out.push_str(&capsule_group(soul));
    out.push_str("</svg>\n");
    out
}

// ------------------------------------------------------------- the wallpaper

/// THE WALLPAPER OF YOUR COLOURS (Max, 2026-10-18: a wallpaper that "follows
/// your colours"): the card's own picture — the ground, its light, the
/// capsules with their shadows — made for a whole screen. The fan is about
/// HALF the card's relative size (Max, 2026-10-18: tried 50 % bigger, back
/// to this — "smaller as it was"), centred across and a little below the
/// middle, and the ground runs out to the screen's exact shape, whatever it
/// is (2560 × 1600, 1920 × 1080, 3440 × 1440…).
///
/// Where the fan stands, for a screen `w` × `h`: the pixel the board's
/// middle goes to, and how many pixels a board unit is. The scale follows
/// the screen's height (or, on a tall one, what its width gives a 16:10
/// board), so the fan keeps its shape on every screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Frame {
    pub cx: f32,
    pub cy: f32,
    pub unit: f32,
}

/// The fan's share of the card's relative size (the card's capsules are 62 %
/// of its height; here about a third of the screen's).
const WALL_SCALE: f32 = 0.5;
/// How far below the middle the fan's own middle stands, as a share of the height.
const WALL_DROP: f32 = 0.05;

impl Frame {
    pub(crate) fn of(w: u32, h: u32) -> Frame {
        let (w, h) = (w as f32, h as f32);
        let basis = h.min(w / (BOARD.0 / BOARD.1));
        Frame { cx: w / 2.0, cy: h / 2.0 + WALL_DROP * h, unit: WALL_SCALE * basis / BOARD.1 }
    }

    /// Where a point of the board lands on the screen.
    #[cfg(test)]
    pub(crate) fn at(&self, bx: f32, by: f32) -> (f32, f32) {
        (self.cx + (bx - BOARD.0 / 2.0) * self.unit, self.cy + (by - BOARD.1 / 2.0) * self.unit)
    }

    /// The capsules' box on the screen: left, top, right, bottom.
    #[cfg(test)]
    pub(crate) fn fan_box(&self, n: usize) -> (f32, f32, f32, f32) {
        let (x0, span) = fan(n);
        let y0 = (BOARD.1 - CAPSULE.1) / 2.0;
        let (l, t) = self.at(x0, y0);
        let (r, b) = self.at(x0 + span, y0 + CAPSULE.1);
        (l, t, r, b)
    }
}

/// The wallpaper as an SVG `w` × `h` user units (so a pixel is a unit).
pub(crate) fn wall_svg(colours: &[Rgb], w: u32, h: u32) -> String {
    let (ground, soul) = parts(colours);
    let f = Frame::of(w, h);
    // (The light is the card's: 90 % across and 70 % down from the top
    // middle, fading out at 70 % of that.)
    let (rx, ry) = (0.9 * w as f32, 0.7 * h as f32);
    let mut out = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<defs>
  <radialGradient id="lift" gradientUnits="userSpaceOnUse" cx="{mx}" cy="0" r="{rx}" gradientTransform="translate({mx} 0) scale(1 {squash}) translate(-{mx} 0)">
    <stop offset="0" stop-color="#ffffff" stop-opacity="{lift}"/>
    <stop offset="0.7" stop-color="#ffffff" stop-opacity="0"/>
  </radialGradient>
{defs}</defs>
<rect width="{w}" height="{h}" fill="#{g}"/>
<rect width="{w}" height="{h}" fill="url(#lift)"/>
<g transform="translate({gx} {gy}) scale({unit}) translate(-80 -50)">
"##,
        mx = f.cx,
        squash = ry / rx,
        g = hex(ground),
        lift = lift(ground),
        defs = CAPSULE_DEFS,
        gx = f.cx,
        gy = f.cy,
        unit = f.unit,
    );
    out.push_str(&capsule_group(soul));
    out.push_str("</g>\n</svg>\n");
    out
}

/// A fine, deterministic noise over a picture's pixels (`rgb`: three bytes a
/// pixel, `w` pixels a row): each channel of each pixel moves by -1, 0 or +1
/// (a quarter, a half, a quarter), by a hash of where it is. A wide, soft
/// gradient has only so many steps of colour, and the eye reads the edges
/// between them as bands; the noise breaks those edges. The same picture
/// always gets the same noise.
pub(crate) fn dither(rgb: &mut [u8], w: u32) {
    let w = w.max(1) as usize;
    for (i, px) in rgb.chunks_exact_mut(3).enumerate() {
        let (x, y) = ((i % w) as u32, (i / w) as u32);
        // (murmur3's finaliser over the position.)
        let mut h = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca6b);
        h ^= h >> 16;
        h = h.wrapping_mul(0x85eb_ca6b);
        h ^= h >> 13;
        h = h.wrapping_mul(0xc2b2_ae35);
        h ^= h >> 16;
        for (c, v) in px.iter_mut().enumerate() {
            let bits = (h >> (2 * c)) & 3;
            let step = (bits & 1) as i16 + (bits >> 1) as i16 - 1;
            *v = (i16::from(*v) + step).clamp(0, 255) as u8;
        }
    }
}

/// The wallpaper `w` × `h`, drawn and dithered: three bytes a pixel.
pub(crate) fn wall_pixels(colours: &[Rgb], w: u32, h: u32) -> Option<Vec<u8>> {
    let svg = wall_svg(colours, w, h);
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    // (The ground is solid: premultiplied or not, the colours are the same.)
    let mut rgb: Vec<u8> = pixmap.data().chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    dither(&mut rgb, w);
    Some(rgb)
}

/// The wallpapers' drawing: a new look makes new files.
const WALL_VERSION: u64 = 3;

/// The most a wallpaper may be asked for in a side (a screen of 8K).
const WALL_MAX: u32 = 8192;

/// Where the wallpapers of these colours are kept.
pub(crate) fn walls_dir() -> Option<PathBuf> {
    Some(cache_dir()?.join("palette-walls"))
}

/// Where the wallpaper of these colours, at this size, is kept: named by the
/// colours, the size and the version of the drawing.
pub(crate) fn wall_path(colours: &[Rgb], w: u32, h: u32) -> Option<PathBuf> {
    Some(walls_dir()?.join(format!("{:016x}-{w}x{h}.png", colours_hash(colours, WALL_VERSION))))
}

/// Write `rgb` (three bytes a pixel, `w` × `h`) as a PNG at `p`: whole, to a
/// file of its own, and then renamed into place — a reader never finds half
/// a picture.
fn write_wall(p: &std::path::Path, rgb: &[u8], w: u32, h: u32) -> Option<()> {
    std::fs::create_dir_all(p.parent()?).ok()?;
    // (A name of its own per writer: two of them at once never share a file.)
    static WRITERS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = WRITERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = p.with_extension(format!("tmp-{}-{n}", std::process::id()));
    let written = std::fs::File::create(&tmp).ok().and_then(|f| {
        use image::ImageEncoder;
        // (Quick to write; the noise would make the best squeeze a long one for little.)
        image::codecs::png::PngEncoder::new_with_quality(
            std::io::BufWriter::new(f),
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::Sub,
        )
        .write_image(rgb, w, h, image::ExtendedColorType::Rgb8)
        .ok()
    });
    if written.is_none() || std::fs::rename(&tmp, p).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(())
}

/// The wallpaper of these colours at this size, drawn if it is not there yet.
/// Slow — a screen's worth of pixels: call it from a thread.
pub(crate) fn ensure_wall(colours: &[Rgb], w: u32, h: u32) -> Option<PathBuf> {
    if w == 0 || h == 0 || w > WALL_MAX || h > WALL_MAX {
        return None;
    }
    let p = wall_path(colours, w, h)?;
    if p.is_file() {
        return Some(p);
    }
    write_wall(&p, &wall_pixels(colours, w, h)?, w, h)?;
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_is_drawn_and_kept() {
        let dir = std::env::temp_dir().join(format!("golem-card-test-{}", std::process::id()));
        std::env::set_var("XDG_CACHE_HOME", &dir);
        let colours = [[0x1d, 0x20, 0x26], [0xe0, 0x52, 0x52], [0xff, 0xbe, 0x98], [0x8a, 0xb4, 0xf0], [0x9a, 0xd0, 0x8a], [0xc0, 0xa0, 0xe0], [0xe6, 0xe6, 0xe0]];
        let p = ensure(&colours).expect("drawn");
        let img = image::open(&p).expect("a png").to_rgba8();
        assert_eq!((img.width(), img.height()), (W, H));
        // The middle of the first capsule wears its colour (sheen aside).
        let px = img.get_pixel((W as f32 * (52.0 / 160.0)) as u32, H / 2);
        assert!(px[0] > 180 && px[1] < 120, "{px:?}");
        // The corner wears the ground.
        let g = img.get_pixel(4, H - 4);
        assert!(g[0] < 50, "{g:?}");
        // Dynamic's wheel: red at its top-left, its middle not the ground.
        let w = image::open(wheel(true).expect("a wheel")).expect("a png").to_rgba8();
        let mid = w.get_pixel(W / 2, H / 2);
        assert!(mid[0] > 60 || mid[1] > 60 || mid[2] > 60, "{mid:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The screens a wallpaper must fit: the owner's laptops and desks, and a
    /// few shapes nobody owns (a tall one, a small one).
    const SCREENS: [(u32, u32); 8] =
        [(2560, 1600), (1920, 1080), (3440, 1440), (1366, 768), (1440, 900), (3840, 2160), (1080, 1920), (800, 600)];

    const SAMPLE: [Rgb; 7] = [[0x1d, 0x20, 0x26], [0xe0, 0x52, 0x52], [0xff, 0xbe, 0x98], [0x8a, 0xb4, 0xf0], [0x9a, 0xd0, 0x8a], [0xc0, 0xa0, 0xe0], [0xe6, 0xe6, 0xe0]];

    #[test]
    fn the_wallpaper_fan_fits_any_screen_centred_across_and_a_little_low() {
        for (w, h) in SCREENS {
            for n in 1..=8 {
                let f = Frame::of(w, h);
                let (l, t, r, b) = f.fan_box(n);
                let (wf, hf) = (w as f32, h as f32);
                // Inside the frame, with air all round (the floor's shadow under it too).
                assert!(l > 0.02 * wf && r < 0.98 * wf, "{w}x{h} n={n}: {l}..{r}");
                assert!(t > 0.1 * hf && b < 0.8 * hf, "{w}x{h} n={n}: {t}..{b}");
                // Centred across to the pixel; its middle a little below the screen's.
                assert!(((l + r) / 2.0 - wf / 2.0).abs() < 0.5, "{w}x{h} n={n}");
                let mid = (t + b) / 2.0;
                assert!(mid > hf * 0.5 && mid < hf * 0.6, "{w}x{h} n={n}: {mid}");
                // The shadow on the floor under the fan stays on the screen.
                let (_, floor) = f.at(80.0, 92.0);
                assert!(floor < hf, "{w}x{h}: {floor}");
            }
        }
    }

    #[test]
    fn the_wallpaper_fan_is_half_the_cards_relative_size() {
        // The card's capsules are 62 % of its height; on a 16:10 screen the
        // wallpaper's are about half of that, and the same shape on every
        // screen as wide as that.
        let (_, t, _, b) = Frame::of(2560, 1600).fan_box(6);
        let share = (b - t) / 1600.0;
        assert!((0.29..=0.33).contains(&share), "{share}");
        let (l, _, r, _) = Frame::of(2560, 1600).fan_box(6);
        let (l2, t2, r2, b2) = Frame::of(3440, 1440).fan_box(6);
        // Wider screens do not make it wider: the height is the measure.
        assert!(((r2 - l2) / (b2 - t2) - (r - l) / (b - t)).abs() < 1e-3);
        // A tall screen is measured by its width instead (the fan must fit).
        let (l3, _, r3, _) = Frame::of(1080, 1920).fan_box(6);
        assert!(r3 - l3 < 1080.0 * 0.6);
    }

    #[test]
    fn the_wallpaper_is_drawn_at_the_exact_size_with_the_ground_and_the_capsules() {
        // Small screens of the shapes that matter, so the test is quick.
        for (w, h) in [(320, 200), (320, 180), (430, 180), (180, 320), (256, 192)] {
            let rgb = wall_pixels(&SAMPLE, w, h).expect("drawn");
            assert_eq!(rgb.len(), (w * h * 3) as usize);
            let at = |x: f32, y: f32| {
                let (x, y) = ((x as u32).min(w - 1), (y as u32).min(h - 1));
                let i = ((y * w + x) * 3) as usize;
                [rgb[i], rgb[i + 1], rgb[i + 2]]
            };
            // The corner under the light wears the ground (a little lit, a little dithered).
            let g = at(2.0, h as f32 - 3.0);
            assert!((i16::from(g[0]) - 0x1d).abs() < 8 && (i16::from(g[2]) - 0x26).abs() < 8, "{w}x{h}: {g:?}");
            // The first capsule (on top, at the fan's left) wears its colour.
            let f = Frame::of(w, h);
            let (cx, cy) = f.at(fan(6).0 + CAPSULE.0 / 2.0, 50.0);
            let c = at(cx, cy);
            assert!(c[0] > 150 && c[1] < 130 && c[2] < 130, "{w}x{h}: {c:?}");
            // The middle of the fan is not the ground.
            let m = at(f.cx, f.cy);
            assert!(i16::from(m[0]) + i16::from(m[1]) + i16::from(m[2]) > 3 * 60, "{w}x{h}: {m:?}");
        }
    }

    #[test]
    fn a_light_ground_is_lit_more_than_a_dark_one() {
        let dark = wall_svg(&[[0x1d, 0x20, 0x26], [0xe0, 0x52, 0x52]], 640, 400);
        let light = wall_svg(&[[0xec, 0xec, 0xe8], [0xe0, 0x52, 0x52]], 640, 400);
        assert!(dark.contains("stop-opacity=\"0.07\"") && light.contains("stop-opacity=\"0.5\""));
        assert!(dark.contains("viewBox=\"0 0 640 400\"") && dark.contains("#1d2026"));
    }

    #[test]
    fn the_dither_is_deterministic_and_moves_a_channel_by_one_at_most() {
        let (w, h) = (64u32, 48u32);
        let flat = vec![128u8; (w * h * 3) as usize];
        let (mut a, mut b) = (flat.clone(), flat.clone());
        dither(&mut a, w);
        dither(&mut b, w);
        assert_eq!(a, b, "the same picture gets the same noise");
        assert!(a.iter().zip(&flat).all(|(x, y)| x.abs_diff(*y) <= 1));
        // …and it really does move them: about three channels in four.
        let moved = a.iter().zip(&flat).filter(|(x, y)| x != y).count();
        let share = moved as f32 / flat.len() as f32;
        assert!((0.4..0.6).contains(&share), "{share}");
        // Both ways, evenly enough that the picture's brightness is not shifted.
        let (up, down) = (a.iter().filter(|v| **v == 129).count(), a.iter().filter(|v| **v == 127).count());
        assert!(up.abs_diff(down) * 10 < up + down, "{up} up, {down} down");
        // The ends of the range do not wrap.
        let mut ends: Vec<u8> = (0..300).map(|i| if i % 2 == 0 { 0 } else { 255 }).collect();
        dither(&mut ends, 10);
        assert!(ends.iter().enumerate().all(|(i, v)| if i % 2 == 0 { *v <= 1 } else { *v >= 254 }));
    }

    #[test]
    fn the_wallpaper_is_named_by_its_colours_its_size_and_the_version() {
        let name = |c: &[Rgb], w, h| format!("{:016x}-{w}x{h}.png", colours_hash(c, WALL_VERSION));
        let a = name(&SAMPLE, 2560, 1600);
        assert_eq!(a, name(&SAMPLE, 2560, 1600));
        assert_ne!(a, name(&SAMPLE[..6], 2560, 1600), "another colour set");
        let mut other = SAMPLE;
        other[3] = [0x8a, 0xb4, 0xf1];
        assert_ne!(a, name(&other, 2560, 1600), "one channel of one colour");
        assert_ne!(a, name(&SAMPLE, 1920, 1080), "another size");
        assert!(a.ends_with("-2560x1600.png"));
        // A new version of the drawing makes new files; so does it for a card.
        assert_ne!(colours_hash(&SAMPLE, WALL_VERSION), colours_hash(&SAMPLE, WALL_VERSION + 1));
        assert_ne!(colours_hash(&SAMPLE, WALL_VERSION), colours_hash(&SAMPLE, 1));
    }

    #[test]
    fn a_wallpaper_is_made_once_and_kept_whole() {
        // (Its own cache: the environment is shared by every test, so the
        // card test's directory is not borrowed.)
        let dir = std::env::temp_dir().join(format!("golem-wall-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x-96x60.png");
        let rgb = wall_pixels(&SAMPLE, 96, 60).unwrap();
        write_wall(&p, &rgb, 96, 60).expect("written");
        let img = image::open(&p).expect("a png").to_rgb8();
        assert_eq!((img.width(), img.height()), (96, 60));
        assert_eq!(img.as_raw(), &rgb, "the file holds exactly the pixels drawn");
        // Nothing half-written is left beside it.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Four wallpapers to LOOK at (`cargo test -- --ignored four_samples`):
    /// jade dark, vermilion dark, monet light, lagoon light, at the sizes of
    /// the owner's screens, in /tmp/claude-1000/follow-samples/.
    #[test]
    #[ignore]
    fn four_samples_of_the_wallpaper_are_written_to_look_at() {
        let out = std::path::PathBuf::from("/tmp/claude-1000/follow-samples");
        std::fs::create_dir_all(&out).unwrap();
        let all = crate::palette::all();
        for (id, dark, w, h) in [("jade", true, 2560, 1600), ("vermilion", true, 1920, 1080), ("monet", false, 2560, 1600), ("lagoon", false, 3440, 1440)] {
            let p = all.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("no palette {id}"));
            let swatch = crate::palette::swatch(p, dark);
            let started = std::time::Instant::now();
            let rgb = wall_pixels(&swatch, w, h).expect("drawn");
            let drawn = started.elapsed();
            let file = out.join(format!("{id}-{}-{w}x{h}.png", if dark { "dark" } else { "light" }));
            write_wall(&file, &rgb, w, h).expect("written");
            println!("{} drawn in {drawn:?}, written by {:?}, {} KB", file.display(), started.elapsed(), std::fs::metadata(&file).unwrap().len() / 1024);
        }
    }
}

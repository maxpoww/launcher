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

/// Where the card of these colours (ground first, then the soul) is kept.
pub(crate) fn path(colours: &[Rgb]) -> Option<PathBuf> {
    let cache = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok()?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in colours {
        for b in c {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    // (The version of the drawing: a new look makes new files.)
    h = (h ^ 2).wrapping_mul(0x0100_0000_01b3);
    Some(cache.join("waverunner/palette-cards").join(format!("{h:016x}.png")))
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

/// The Paper card (the mockup's `.tile` + `.pill` + `.pills::after`), on a
/// 160 × 100 board.
pub(crate) fn svg(colours: &[Rgb]) -> String {
    let ground = colours.first().copied().unwrap_or([0x1d, 0x20, 0x26]);
    let soul = if colours.len() > 1 { &colours[1..] } else { colours };
    let bright = crate::palette::rel_lum(ground) > 0.35;
    // .tile: radial-gradient(90% 70% at 50% 0%, var(--lift) 0%, transparent 70%)
    let lift = if bright { 0.50 } else { 0.07 };
    let n = soul.len().max(1) as f32;
    // .pill: width 15 %, height 62 %, each 7.5 % over the one before.
    let (pw, ph) = (24.0f32, 62.0f32);
    let step = 12.0f32;
    let span = pw + (n - 1.0) * step;
    let (x0, y) = ((160.0 - span) / 2.0, (100.0 - ph) / 2.0);
    let mut out = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="100" viewBox="0 0 160 100">
<defs>
  <radialGradient id="lift" gradientUnits="userSpaceOnUse" cx="80" cy="0" r="144" gradientTransform="translate(80 0) scale(1 0.4861) translate(-80 0)">
    <stop offset="0" stop-color="#ffffff" stop-opacity="{lift}"/>
    <stop offset="0.7" stop-color="#ffffff" stop-opacity="0"/>
  </radialGradient>
  <linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#ffffff" stop-opacity="0.12"/>
    <stop offset="0.35" stop-color="#ffffff" stop-opacity="0"/>
    <stop offset="0.7" stop-color="#000000" stop-opacity="0"/>
    <stop offset="1" stop-color="#000000" stop-opacity="0.10"/>
  </linearGradient>
  <filter id="cast" x="-1" y="-0.5" width="3" height="2"><feGaussianBlur stdDeviation="2.2"/></filter>
  <filter id="floor" x="-0.5" y="-2" width="2" height="5"><feGaussianBlur stdDeviation="1.6"/></filter>
</defs>
<rect width="160" height="100" fill="#{g}"/>
<rect width="160" height="100" fill="url(#lift)"/>
<ellipse cx="80" cy="{fy}" rx="{frx}" ry="3.6" fill="#000000" fill-opacity="0.35" filter="url(#floor)"/>
"##,
        g = hex(ground),
        fy = 100.0 - 14.0 - 4.0,
        frx = 160.0 * 0.26,
    );
    // The first on top: drawn last.
    for (k, c) in soul.iter().enumerate().rev() {
        let x = x0 + k as f32 * step;
        let r = pw / 2.0;
        // box-shadow: -5px 0 9px -3px #00000059 (one pill-width 24 ↔ the
        // mockup's ~17 px: the numbers scaled by 1.4).
        out.push_str(&format!(
            r##"<rect x="{sx}" y="{sy}" width="{sw}" height="{sh}" rx="{sr}" fill="#000000" fill-opacity="0.35" filter="url(#cast)"/>
<clipPath id="p{k}"><rect x="{x}" y="{y}" width="{pw}" height="{ph}" rx="{r}"/></clipPath>
<rect x="{x}" y="{y}" width="{pw}" height="{ph}" rx="{r}" fill="#{c}"/>
<rect x="{x}" y="{y}" width="{pw}" height="{ph}" rx="{r}" fill="url(#sheen)"/>
<rect x="{x}" y="{y}" width="{pw}" height="1.2" fill="#ffffff" fill-opacity="0.15" clip-path="url(#p{k})"/>
"##,
            sx = x - 7.0 + 4.2,
            sy = y + 4.2,
            sw = pw - 8.4,
            sh = ph - 8.4,
            sr = r - 4.2,
            c = hex(*c),
        ));
    }
    out.push_str("</svg>\n");
    out
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}

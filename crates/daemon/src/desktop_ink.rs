//! THE DESKTOP'S NAMES READ ON ANY WALLPAPER (Max, 2026-10-22: "look at the
//! text colour on my desktop icons. is screwed… (different images)"): white
//! with a one-pixel shadow vanished on a light picture — a palette's paper
//! ground. Now the picture BEHIND each name decides its ink: dark with a light
//! halo on a bright part, white with a dark one elsewhere.
//!
//! The picture is read from the wallpaper itself (a moving one by its first
//! frame, `controls::poster`), laid on the screen as the picture program lays
//! it — covering it, centred — into a small map of how bright each part is;
//! the Lighting's veil, a black wash over it, is taken off at drawing time.
//! Read once a wallpaper (`WALL_GEN`, bumped wherever one is put up) and a
//! screen size, off the loop. Without a map (an unreadable picture, before the
//! first read) the names are as before: white, shadowed.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::content::Rect;

/// Moves each time a wallpaper is put up (`controls::show_wallpaper`): the
/// desktop reads the new one when it sees it move.
pub(crate) static WALL_GEN: AtomicU64 = AtomicU64::new(1);

pub(crate) fn wallpaper_changed() {
    WALL_GEN.fetch_add(1, Ordering::Relaxed);
}

/// The map's columns; its rows follow the screen's shape.
const COLS: u32 = 96;
/// Brighter than this behind a name (relative luminance, after the veil),
/// the name is dark: where black ink starts to read better than white (the
/// two contrasts cross at ~0.2), a little toward white, the desktop's own.
const DARK_FROM: f32 = 0.24;

/// A name's two colours: its ink, and the halo drawn a pixel under it.
pub(crate) const LIGHT: ([f32; 4], [f32; 4]) = ([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.6]);
pub(crate) const DARK: ([f32; 4], [f32; 4]) = ([0.11, 0.11, 0.12, 1.0], [1.0, 1.0, 1.0, 0.55]);

/// How bright the wallpaper is on the screen, part by part.
#[derive(Debug)]
pub(crate) struct Map {
    cols: usize,
    rows: usize,
    /// Relative luminance (linear light), row by row.
    lum: Vec<f32>,
    /// The surface it was laid over (its own coordinates).
    size: (f32, f32),
}

impl Map {
    /// The wallpaper at `path` over a surface `size` large. Off the loop: it
    /// decodes a picture.
    pub(crate) fn read(path: &str, size: (f32, f32)) -> Option<Map> {
        let still = if crate::controls::wallpaper_is_video(path) && !path.to_ascii_lowercase().ends_with(".gif") {
            crate::controls::poster(path)?
        } else {
            path.to_owned()
        };
        let picture = image::open(&still).ok()?;
        // (Small first: a 6K photo is read once, not walked whole.)
        let small = picture.thumbnail(COLS * 4, COLS * 4).to_rgb8();
        Self::of(&small, size)
    }

    /// `picture` laid over a surface `size` large, covering it, centred.
    pub(crate) fn of(picture: &image::RgbImage, size: (f32, f32)) -> Option<Map> {
        let (iw, ih) = (picture.width() as f32, picture.height() as f32);
        let (sw, sh) = size;
        if iw < 1.0 || ih < 1.0 || sw < 1.0 || sh < 1.0 {
            return None;
        }
        let k = (sw / iw).max(sh / ih);
        let (vis_w, vis_h) = ((sw / k).min(iw), (sh / k).min(ih));
        let (x0, y0) = (((iw - vis_w) / 2.0) as u32, ((ih - vis_h) / 2.0) as u32);
        let seen = image::imageops::crop_imm(picture, x0, y0, (vis_w as u32).max(1), (vis_h as u32).max(1)).to_image();
        let cols = COLS as usize;
        let rows = ((COLS as f32 * sh / sw).round() as usize).max(1);
        let cells = image::imageops::resize(&seen, cols as u32, rows as u32, image::imageops::FilterType::Triangle);
        let lum = cells.pixels().map(|p| crate::palette::rel_lum([p[0], p[1], p[2]])).collect();
        Some(Map { cols, rows, lum, size })
    }

    /// How bright the picture is behind `rect` (the surface's coordinates):
    /// between its mean and its brightest part, since a name must read where
    /// it is lightest too.
    fn behind(&self, rect: Rect) -> f32 {
        let (sw, sh) = self.size;
        let col = |x: f32| ((x / sw * self.cols as f32).floor().max(0.0) as usize).min(self.cols - 1);
        let row = |y: f32| ((y / sh * self.rows as f32).floor().max(0.0) as usize).min(self.rows - 1);
        let (c0, c1, r0, r1) = (col(rect.x), col(rect.x + rect.w), row(rect.y), row(rect.y + rect.h));
        let (mut sum, mut n, mut most) = (0.0f32, 0.0f32, 0.0f32);
        for r in r0..=r1 {
            for c in c0..=c1 {
                let l = self.lum[r * self.cols + c];
                sum += l;
                n += 1.0;
                most = most.max(l);
            }
        }
        if n == 0.0 {
            return 0.0;
        }
        0.6 * sum / n + 0.4 * most
    }

    /// The colours of a name written in `rect`, the veil's black at `veil`
    /// (its alpha) over the picture.
    pub(crate) fn ink(&self, rect: Rect, veil: f32) -> ([f32; 4], [f32; 4]) {
        if self.behind(rect) * (1.0 - veil.clamp(0.0, 1.0)) > DARK_FROM {
            DARK
        } else {
            LIGHT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> image::RgbImage {
        image::RgbImage::from_fn(w, h, |x, y| image::Rgb(f(x, y)))
    }

    #[test]
    fn a_name_on_a_light_picture_is_dark_and_on_a_dark_one_white() {
        let size = (1600.0, 1000.0);
        let name = Rect::new(20.0, 120.0, 80.0, 18.0);
        let paper = Map::of(&plain(64, 40, |_, _| [0xf3, 0xe9, 0xe4]), size).unwrap();
        assert_eq!(paper.ink(name, 0.0), DARK, "Max's light palette wallpaper");
        let night = Map::of(&plain(64, 40, |_, _| [0x16, 0x18, 0x1d]), size).unwrap();
        assert_eq!(night.ink(name, 0.0), LIGHT);
        // The veil over the paper at night: dark enough for white again.
        assert_eq!(paper.ink(name, 0.85), LIGHT);
    }

    #[test]
    fn each_name_reads_its_own_part_of_the_picture() {
        // Dark on the left half, light on the right, as the screen shows it.
        let size = (1600.0, 1000.0);
        let half = Map::of(&plain(160, 100, |x, _| if x < 80 { [20, 20, 30] } else { [240, 240, 235] }), size).unwrap();
        assert_eq!(half.ink(Rect::new(30.0, 100.0, 80.0, 18.0), 0.0), LIGHT, "left: dark picture");
        assert_eq!(half.ink(Rect::new(1480.0, 100.0, 80.0, 18.0), 0.0), DARK, "right: light picture");
        // A name over a small bright patch reads it too (the brightest part counts).
        assert_eq!(half.ink(Rect::new(760.0, 100.0, 80.0, 18.0), 0.0), DARK, "astride the edge");
    }

    #[test]
    fn the_picture_is_laid_as_it_covers_the_screen() {
        // A tall picture on a wide screen: only its middle band is seen. Its
        // top and bottom are light, its middle dark — the names see dark.
        let size = (1600.0, 1000.0);
        let tall = Map::of(&plain(100, 400, |_, y| if (150..250).contains(&y) { [10, 10, 10] } else { [250, 250, 250] }), size).unwrap();
        assert_eq!(tall.ink(Rect::new(20.0, 20.0, 80.0, 18.0), 0.0), LIGHT, "the top of the SCREEN is the middle of the picture");
    }
}

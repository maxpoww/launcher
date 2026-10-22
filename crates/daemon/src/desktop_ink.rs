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

/// The map's columns (a dozen screen pixels each); its rows follow the
/// screen's shape.
const COLS: u32 = 160;
/// White is the desktop's own: the dark ink only where it reads this much
/// better at the worst part behind the name.
const DARK_BETTER: f32 = 1.15;
/// Under this contrast at the worst part behind it (a name across a bright
/// part and a dark one, a busy picture), neither ink reads alone: the halo
/// goes all round the letters.
const OUTLINE_UNDER: f32 = 3.0;
/// The dark ink's luminance (its sRGB colour below).
const DARK_LUM: f32 = 0.0117;

/// A name's colours: its ink, the halo, and whether the halo goes all round
/// the letters (else one pixel under them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ink {
    pub ink: [f32; 4],
    pub halo: [f32; 4],
    pub outline: bool,
}

pub(crate) const LIGHT: Ink = Ink { ink: [1.0, 1.0, 1.0, 1.0], halo: [0.0, 0.0, 0.0, 0.6], outline: false };
pub(crate) const DARK: Ink = Ink { ink: [0.11, 0.11, 0.12, 1.0], halo: [1.0, 1.0, 1.0, 0.55], outline: false };

fn ratio(a: f32, b: f32) -> f32 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

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

    /// How bright the picture is behind `rect` (the surface's
    /// coordinates), part by part: its darkest and its brightest.
    fn behind(&self, rect: Rect) -> (f32, f32) {
        let (sw, sh) = self.size;
        let col = |x: f32| ((x / sw * self.cols as f32).floor().max(0.0) as usize).min(self.cols - 1);
        let row = |y: f32| ((y / sh * self.rows as f32).floor().max(0.0) as usize).min(self.rows - 1);
        let (c0, c1, r0, r1) = (col(rect.x), col(rect.x + rect.w), row(rect.y), row(rect.y + rect.h));
        let (mut least, mut most) = (f32::MAX, 0.0f32);
        for r in r0..=r1 {
            for c in c0..=c1 {
                let l = self.lum[r * self.cols + c];
                least = least.min(l);
                most = most.max(l);
            }
        }
        (least.min(most), most)
    }

    /// The colours of a name written in `rect`, the veil's black at `veil`
    /// (its alpha) over the picture: the ink that reads best where the
    /// picture behind it is worst for it — white unless the dark reads
    /// clearly better — and the halo all round where even that is weak.
    pub(crate) fn ink(&self, rect: Rect, veil: f32) -> Ink {
        let keep = 1.0 - veil.clamp(0.0, 1.0);
        let (least, most) = self.behind(rect);
        let (least, most) = (least * keep, most * keep);
        // White reads worst on the brightest part, the dark on the darkest.
        let (light, dark) = (ratio(most, 1.0), ratio(least, DARK_LUM));
        let (mut ink, worst) = if dark > light * DARK_BETTER { (DARK, dark) } else { (LIGHT, light) };
        ink.outline = worst < OUTLINE_UNDER;
        ink
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
        assert_eq!(paper.ink(name, 0.0), DARK, "Max's light palette wallpaper: dark, a pixel's halo");
        let night = Map::of(&plain(64, 40, |_, _| [0x16, 0x18, 0x1d]), size).unwrap();
        assert_eq!(night.ink(name, 0.0), LIGHT);
        // The veil over the paper at night: dark enough for white again.
        assert_eq!(paper.ink(name, 0.85).ink, LIGHT.ink);
    }

    #[test]
    fn each_name_reads_its_own_part_of_the_picture() {
        // Dark on the left half, light on the right, as the screen shows it.
        let size = (1600.0, 1000.0);
        let half = Map::of(&plain(160, 100, |x, _| if x < 80 { [20, 20, 30] } else { [240, 240, 235] }), size).unwrap();
        assert_eq!(half.ink(Rect::new(30.0, 100.0, 80.0, 18.0), 0.0), LIGHT, "left: dark picture");
        assert_eq!(half.ink(Rect::new(1480.0, 100.0, 80.0, 18.0), 0.0), DARK, "right: light picture");
        // Astride the edge neither reads alone: white, its halo all round.
        let astride = half.ink(Rect::new(760.0, 100.0, 80.0, 18.0), 0.0);
        assert!(astride.outline, "a name across a bright part and a dark one is outlined");
        // A plain mid grey: both read; no outline.
        let grey = Map::of(&plain(64, 40, |_, _| [128, 128, 128]), size).unwrap();
        assert!(!grey.ink(Rect::new(30.0, 100.0, 80.0, 18.0), 0.0).outline);
    }

    /// EVERY WALLPAPER, LOOKED AT (run by hand: `cargo test -- --ignored
    /// ink_on_every_wallpaper`; GOLEM_INK_WALLS = the folder, default
    /// ~/Pictures/Wallpapers; GOLEM_INK_OUT = where the pictures go): names
    /// where the desktop puts them (both edges) and across the screen, each
    /// drawn in the ink chosen, and for each the contrast of its ink with the
    /// picture under it at its worst tenth — then the same for the old white.
    #[test]
    #[ignore]
    fn ink_on_every_wallpaper() {
        let home = std::env::var("HOME").unwrap_or_default();
        let dir = std::env::var("GOLEM_INK_WALLS").unwrap_or(format!("{home}/Pictures/Wallpapers"));
        let out = std::path::PathBuf::from(std::env::var("GOLEM_INK_OUT").unwrap_or("/tmp/claude-1000/ink-sheet".into()));
        std::fs::create_dir_all(&out).unwrap();
        let (sw, sh) = (800u32, 500u32);
        let mut names: Vec<Rect> = Vec::new();
        for k in 0..5 {
            names.push(Rect::new(10.0, 70.0 + 85.0 * k as f32, 80.0, 14.0));
            names.push(Rect::new(710.0, 70.0 + 85.0 * k as f32, 80.0, 14.0));
        }
        for gy in 0..4 {
            for gx in 0..5 {
                names.push(Rect::new(110.0 + 120.0 * gx as f32, 60.0 + 110.0 * gy as f32, 80.0, 14.0));
            }
        }
        let lum = |p: &image::Rgb<u8>| crate::palette::rel_lum([p[0], p[1], p[2]]);
        let ratio = |a: f32, b: f32| (a.max(b) + 0.05) / (a.min(b) + 0.05);
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        let (mut bad_new, mut bad_old, mut total) = (0, 0, 0);
        let mut worst: Vec<(f32, String)> = Vec::new();
        for path in files {
            let p = path.to_string_lossy().into_owned();
            let still = if crate::controls::wallpaper_is_video(&p) && !p.to_ascii_lowercase().ends_with(".gif") {
                match crate::controls::poster(&p) {
                    Some(s) => s,
                    None => {
                        println!("{p}: no still (no video tool here)");
                        continue;
                    }
                }
            } else {
                p.clone()
            };
            let Ok(img) = image::open(&still) else {
                println!("{p}: unreadable");
                continue;
            };
            let screen = img.resize_to_fill(sw, sh, image::imageops::FilterType::Triangle).to_rgb8();
            let map = Map::of(&img.thumbnail(COLS * 4, COLS * 4).to_rgb8(), (sw as f32, sh as f32)).unwrap();
            let mut canvas = image::RgbaImage::from_fn(sw, sh, |x, y| {
                let q = screen.get_pixel(x, y);
                image::Rgba([q[0], q[1], q[2], 255])
            });
            let mut file_worst = f32::MAX;
            for r in &names {
                let chosen = map.ink(*r, 0.0);
                let (ink, halo) = (chosen.ink, chosen.halo);
                let ink_l = crate::palette::rel_lum([(ink[0] * 255.0) as u8, (ink[1] * 255.0) as u8, (ink[2] * 255.0) as u8]);
                let mut c_new = Vec::new();
                let mut c_old = Vec::new();
                for y in r.y as u32..((r.y + r.h) as u32).min(sh) {
                    for x in r.x as u32..((r.x + r.w) as u32).min(sw) {
                        let l = lum(screen.get_pixel(x, y));
                        c_new.push(ratio(l, ink_l));
                        c_old.push(ratio(l, 1.0));
                    }
                }
                c_new.sort_by(f32::total_cmp);
                c_old.sort_by(f32::total_cmp);
                let (n10, o10) = (c_new[c_new.len() / 10], c_old[c_old.len() / 10]);
                total += 1;
                bad_new += usize::from(n10 < 3.0);
                bad_old += usize::from(o10 < 3.0);
                file_worst = file_worst.min(n10);
                // The name as the desktop draws it: the halo a pixel under, then the ink.
                let rgb = |c: [f32; 4]| [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8];
                let mut layer = vec![0u8; (sw * sh * 4) as usize];
                let offsets: &[(f32, f32)] = if chosen.outline { &[(0.0, 1.0), (0.0, -1.0), (1.0, 0.0), (-1.0, 0.0)] } else { &[(0.0, 1.0)] };
                crate::thumb_more::with_fonts(|fonts| {
                    let attrs = glyphon::Attrs::new().family(glyphon::Family::SansSerif);
                    for (dx, dy) in offsets {
                        crate::thumb_more::draw_text_in(&mut layer, sw, "Documents", attrs.clone(), 13.0, (r.x + dx, r.y + dy), (0.0, 0.0, sw as f32, sh as f32), rgb(halo), fonts);
                    }
                    Some(())
                });
                for (i, px) in canvas.pixels_mut().enumerate() {
                    let a = f32::from(layer[i * 4 + 3]) / 255.0 * halo[3];
                    for k in 0..3 {
                        px[k] = (f32::from(layer[i * 4 + k]) * a + f32::from(px[k]) * (1.0 - a)) as u8;
                    }
                }
                crate::thumb_more::with_fonts(|fonts| {
                    let attrs = glyphon::Attrs::new().family(glyphon::Family::SansSerif);
                    crate::thumb_more::draw_text_in(&mut canvas, sw, "Documents", attrs, 13.0, (r.x, r.y), (0.0, 0.0, sw as f32, sh as f32), rgb(ink), fonts);
                    Some(())
                });
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            canvas.save(out.join(format!("{name}.png"))).unwrap();
            worst.push((file_worst, name));
        }
        worst.sort_by(|a, b| a.0.total_cmp(&b.0));
        println!("names under 3:1 at their worst tenth: {bad_new} of {total} now, {bad_old} with white alone");
        for (c, n) in worst.iter().take(12) {
            println!("  {c:5.2}  {n}");
        }
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

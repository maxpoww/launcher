//! THE PENCIL on a picture (Max, 2026-10-16, the Quick Look Lab: "nuestro
//! pequeño y simple editor para lo más básico, recortar, escribir, tachar y
//! tinte… como en Android"). Quick Look's frame turns into the editor; this is
//! what it does to the picture.
//!
//! An edit is a list of OPERATIONS in the picture's own share of itself (0..1
//! across and down, of the picture as it stands after the ones before), so
//! the same list draws the small picture on the screen and, when it is
//! saved, the whole one off the loop — nothing is lost to the screen's size.
//! Undo is taking the last one off.

use image::RgbaImage;

/// A colour to draw with (sRGB).
pub(crate) type Ink = [u8; 3];

pub(crate) const INKS: [Ink; 6] = [
    [255, 90, 79],
    [255, 210, 63],
    [76, 195, 138],
    [79, 140, 255],
    [255, 255, 255],
    [17, 17, 17],
];

/// One thing done to the picture.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Op {
    /// A quarter turn clockwise.
    Turn,
    /// Keep only this part (x, y, w, h).
    Crop([f32; 4]),
    /// A line through these points, this wide (a share of the width).
    Stroke {
        points: Vec<(f32, f32)>,
        width: f32,
        ink: Ink,
    },
    /// Hide this part under big squares (x, y, w, h).
    Blur([f32; 4]),
    /// Words, their top-left here, this tall (a share of the height).
    Text {
        at: (f32, f32),
        text: String,
        size: f32,
        ink: Ink,
    },
    /// A PDF's page: this part marked as with a highlighter (x, y, w, h).
    Mark([f32; 4]),
    /// A signature in this part (x, y, w, h): its lines in shares of it.
    Sign {
        rect: [f32; 4],
        strokes: Vec<Vec<(f32, f32)>>,
    },
}

/// A signature's ink: a pen's dark blue.
pub(crate) const SIGN_INK: Ink = [20, 40, 107];
/// A highlighter's yellow.
pub(crate) const MARK_INK: Ink = [255, 214, 10];

/// The picture's colour, set as a whole.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tint {
    /// 0.5..1.5, 1 = as it is.
    pub light: f32,
    pub contrast: f32,
    /// -1 (cold) ..1 (warm).
    pub warmth: f32,
    pub grey: bool,
}

impl Default for Tint {
    fn default() -> Self {
        Self {
            light: 1.0,
            contrast: 1.0,
            warmth: 0.0,
            grey: false,
        }
    }
}

impl Tint {
    pub(crate) fn is_none(&self) -> bool {
        *self == Self::default()
    }
}

/// `img` with every operation done, then the tint (straight, not
/// premultiplied, RGBA).
pub(crate) fn render(img: &RgbaImage, ops: &[Op], tint: Tint) -> RgbaImage {
    let mut out = img.clone();
    for op in ops {
        apply(&mut out, op);
    }
    if !tint.is_none() {
        tinted(&mut out, tint);
    }
    out
}

/// Do one operation to `img` in place (a turn or a crop makes it anew).
pub(crate) fn apply(img: &mut RgbaImage, op: &Op) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    match op {
        Op::Turn => *img = image::imageops::rotate90(img),
        Op::Crop([x, y, cw, ch]) => {
            let (x0, y0) = ((x * w).round() as u32, (y * h).round() as u32);
            let (cw, ch) = (
                ((cw * w).round() as u32).max(1),
                ((ch * h).round() as u32).max(1),
            );
            *img = image::imageops::crop_imm(
                img,
                x0.min(img.width() - 1),
                y0.min(img.height() - 1),
                cw,
                ch,
            )
            .to_image();
        }
        Op::Stroke { points, width, ink } => stroke(img, points, width * w, *ink),
        Op::Blur([x, y, bw, bh]) => {
            let side = (w.max(h) / 60.0).max(6.0) as u32;
            blur(
                img,
                (x * w) as u32,
                (y * h) as u32,
                (bw * w) as u32,
                (bh * h) as u32,
                side,
            );
        }
        Op::Text {
            at,
            text,
            size,
            ink,
        } => words(img, (at.0 * w, at.1 * h), text, size * h, *ink),
        Op::Mark([x, y, mw, mh]) => {
            // (A highlighter darkens what is under it to its colour.)
            let (x0, y0) = ((x * w) as u32, (y * h) as u32);
            let (x1, y1) = (
                (((x + mw) * w) as u32).min(img.width()),
                (((y + mh) * h) as u32).min(img.height()),
            );
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let p = img.get_pixel_mut(xx, yy);
                    for c in 0..3 {
                        p[c] = (u32::from(p[c]) * u32::from(MARK_INK[c]) / 255) as u8;
                    }
                }
            }
        }
        Op::Sign { rect, strokes } => {
            for line in strokes {
                let points: Vec<(f32, f32)> = line
                    .iter()
                    .map(|&(sx, sy)| (rect[0] + sx * rect[2], rect[1] + sy * rect[3]))
                    .collect();
                stroke(img, &points, (rect[2] * w * 0.012).max(1.5), SIGN_INK);
            }
        }
    }
}

/// A line through `points` (shares of the picture), `width` texels wide.
pub(crate) fn stroke(img: &mut RgbaImage, points: &[(f32, f32)], width: f32, ink: Ink) {
    let (w, h) = (img.width(), img.height());
    let Some(mut pixmap) = resvg::tiny_skia::Pixmap::new(w, h) else {
        return;
    };
    let mut pb = resvg::tiny_skia::PathBuilder::new();
    let Some(&(x0, y0)) = points.first() else {
        return;
    };
    pb.move_to(x0 * w as f32, y0 * h as f32);
    if points.len() == 1 {
        // (A dot: a line of no length, with round ends.)
        pb.line_to(x0 * w as f32 + 0.01, y0 * h as f32);
    }
    for &(x, y) in &points[1..] {
        pb.line_to(x * w as f32, y * h as f32);
    }
    let Some(path) = pb.finish() else { return };
    let mut paint = resvg::tiny_skia::Paint::default();
    paint.set_color_rgba8(ink[0], ink[1], ink[2], 255);
    paint.anti_alias = true;
    let line = resvg::tiny_skia::Stroke {
        width: width.max(1.0),
        line_cap: resvg::tiny_skia::LineCap::Round,
        line_join: resvg::tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    pixmap.stroke_path(
        &path,
        &paint,
        &line,
        resvg::tiny_skia::Transform::identity(),
        None,
    );
    over(img, pixmap.data());
}

/// Lay premultiplied `top` (the same size) over straight `img`.
fn over(img: &mut RgbaImage, top: &[u8]) {
    for (px, t) in img.pixels_mut().zip(top.chunks_exact(4)) {
        let a = u32::from(t[3]);
        if a == 0 {
            continue;
        }
        for c in 0..3 {
            px[c] = (u32::from(t[c]) + u32::from(px[c]) * (255 - a) / 255).min(255) as u8;
        }
        px[3] = (a + u32::from(px[3]) * (255 - a) / 255).min(255) as u8;
    }
}

/// Squares of `side` texels, each the mean of what it covers.
fn blur(img: &mut RgbaImage, x: u32, y: u32, bw: u32, bh: u32, side: u32) {
    let (x1, y1) = ((x + bw).min(img.width()), (y + bh).min(img.height()));
    let mut by = y;
    while by < y1 {
        let mut bx = x;
        while bx < x1 {
            let (ex, ey) = ((bx + side).min(x1), (by + side).min(y1));
            let mut sum = [0u64; 4];
            let mut n = 0u64;
            for yy in by..ey {
                for xx in bx..ex {
                    let p = img.get_pixel(xx, yy);
                    for c in 0..4 {
                        sum[c] += u64::from(p[c]);
                    }
                    n += 1;
                }
            }
            let mean = image::Rgba(sum.map(|s| (s / n.max(1)) as u8));
            for yy in by..ey {
                for xx in bx..ex {
                    img.put_pixel(xx, yy, mean);
                }
            }
            bx = ex;
        }
        by = (by + side).min(y1).max(by + 1);
    }
}

/// Words on the picture, a dark edge round them so they read on anything.
fn words(img: &mut RgbaImage, at: (f32, f32), text: &str, size: f32, ink: Ink) {
    let (w, h) = (img.width(), img.height());
    let mut layer = vec![0u8; (w * h * 4) as usize];
    let drawn = crate::thumb_more::with_fonts(|fonts| {
        let attrs = glyphon::Attrs::new().weight(glyphon::Weight::BOLD);
        let clip = (0.0, 0.0, w as f32, h as f32);
        let edge = (size / 16.0).max(1.0);
        // (The edge: the words in a dark shade, a little round about.)
        for (dx, dy) in [(-edge, 0.0), (edge, 0.0), (0.0, -edge), (0.0, edge)] {
            crate::thumb_more::draw_text_in(
                &mut layer,
                w,
                text,
                attrs,
                size,
                (at.0 + dx, at.1 + dy),
                clip,
                [20, 20, 22],
                fonts,
            );
        }
        crate::thumb_more::draw_text_in(&mut layer, w, text, attrs, size, at, clip, ink, fonts);
        Some(())
    });
    if drawn.is_some() {
        over(img, &layer);
    }
}

/// The tint, in place.
pub(crate) fn tinted(img: &mut RgbaImage, t: Tint) {
    for px in img.pixels_mut() {
        let mut c = [px[0], px[1], px[2]].map(|v| f32::from(v) / 255.0);
        for v in &mut c {
            *v = ((*v * t.light - 0.5) * t.contrast + 0.5).clamp(0.0, 1.0);
        }
        if t.warmth != 0.0 {
            c[0] = (c[0] + 0.08 * t.warmth).clamp(0.0, 1.0);
            c[2] = (c[2] - 0.08 * t.warmth).clamp(0.0, 1.0);
        }
        if t.grey {
            let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            c = [l; 3];
        }
        px[0] = (c[0] * 255.0) as u8;
        px[1] = (c[1] * 255.0) as u8;
        px[2] = (c[2] * 255.0) as u8;
    }
}

/// Where an edited picture is written, and how.
pub(crate) enum SaveAs {
    /// Over the original (which is kept in the versions folder first).
    Same,
    /// Beside it, "name (edited).ext" (a picture that cannot be written in
    /// its own kind — a camera's RAW — becomes a JPEG).
    Copy,
}

/// The versions folder: the original of everything saved over is kept here.
pub(crate) fn versions_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/share")
        });
    base.join("golem/versions")
}

/// Whether a picture of this name can be written back in its own kind.
pub(crate) fn writable(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".bmp", ".tif", ".tiff"]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// The copy's name beside `path`: "name (edited).ext", numbered if taken.
pub(crate) fn copy_path(path: &std::path::Path, word: &str) -> std::path::PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && writable(&name) => (s.to_owned(), e.to_owned()),
        Some((s, _)) if !s.is_empty() => (s.to_owned(), "jpg".to_owned()),
        _ => (name.clone(), "jpg".to_owned()),
    };
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    for n in 1.. {
        let candidate = if n == 1 {
            format!("{stem} ({word}).{ext}")
        } else {
            format!("{stem} ({word} {n}).{ext}")
        };
        let p = dir.join(candidate);
        if !p.exists() {
            return p;
        }
    }
    unreachable!()
}

/// Save: the whole picture read again, every operation done to it, written
/// (the original kept first when it is written over). The path written.
pub(crate) fn save(
    path: &std::path::Path,
    ops: &[Op],
    tint: Tint,
    how: SaveAs,
    word: &str,
) -> anyhow::Result<std::path::PathBuf> {
    use anyhow::Context;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let whole = whole_picture(path).context("reading the picture")?;
    let done = render(&whole, ops, tint);
    let to = match how {
        SaveAs::Same if writable(&name) => {
            let dir = versions_dir();
            std::fs::create_dir_all(&dir)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            std::fs::copy(path, dir.join(format!("{stamp}-{name}")))
                .context("keeping the original")?;
            path.to_path_buf()
        }
        _ => copy_path(path, word),
    };
    let lower = to.to_string_lossy().to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        // (No alpha in a JPEG; a good quality.)
        let rgb = image::DynamicImage::ImageRgba8(done).to_rgb8();
        let file = std::fs::File::create(&to)?;
        let mut enc =
            image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), 92);
        enc.encode_image(&rgb)?;
    } else {
        done.save(&to).context("writing the picture")?;
    }
    Ok(to)
}

/// The whole picture, straight RGBA (a RAW: the JPEG it carries).
pub(crate) fn whole_picture(path: &std::path::Path) -> Option<RgbaImage> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if crate::thumb_more::is_raw(&name) {
        let got = std::cell::RefCell::new(None);
        crate::thumb_more::by_exiftool(&path.to_string_lossy(), |jpg| {
            *got.borrow_mut() = image::open(jpg).ok().map(|i| i.to_rgba8());
            got.borrow().is_some().then(Vec::new)
        });
        return got.into_inner();
    }
    Some(
        image::ImageReader::open(path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .decode()
            .ok()?
            .to_rgba8(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_ops_do_the_same_at_any_size() {
        let small = RgbaImage::from_pixel(100, 50, image::Rgba([10, 20, 30, 255]));
        let big = RgbaImage::from_pixel(400, 200, image::Rgba([10, 20, 30, 255]));
        let ops = [Op::Crop([0.5, 0.0, 0.5, 1.0]), Op::Turn];
        let (a, b) = (
            render(&small, &ops, Tint::default()),
            render(&big, &ops, Tint::default()),
        );
        assert_eq!(a.dimensions(), (50, 50));
        assert_eq!(b.dimensions(), (200, 200));
    }

    #[test]
    fn a_stroke_and_a_blur_change_what_they_cover_only() {
        let mut img = RgbaImage::from_fn(60, 40, |x, _| image::Rgba([(x * 4) as u8, 0, 0, 255]));
        let before = img.clone();
        apply(
            &mut img,
            &Op::Stroke {
                points: vec![(0.1, 0.5), (0.4, 0.5)],
                width: 0.05,
                ink: [0, 255, 0],
            },
        );
        assert_eq!(img.get_pixel(10, 20)[1], 255, "under the line");
        assert_eq!(
            img.get_pixel(50, 5),
            before.get_pixel(50, 5),
            "away from it"
        );
        apply(&mut img, &Op::Blur([0.5, 0.0, 0.5, 1.0]));
        assert_eq!(
            img.get_pixel(5, 5),
            before.get_pixel(5, 5),
            "outside the blur"
        );
        assert_eq!(
            img.get_pixel(30, 0),
            img.get_pixel(31, 0),
            "inside it, squares"
        );
    }

    #[test]
    fn a_copy_is_named_beside_and_a_raw_becomes_a_jpeg() {
        let d = std::env::temp_dir().join(format!("pe-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(
            copy_path(&d.join("a.png"), "edited").file_name().unwrap(),
            "a (edited).png"
        );
        std::fs::write(d.join("a (edited).png"), b"x").unwrap();
        assert_eq!(
            copy_path(&d.join("a.png"), "edited").file_name().unwrap(),
            "a (edited 2).png"
        );
        assert_eq!(
            copy_path(&d.join("b.ARW"), "edited").file_name().unwrap(),
            "b (edited).jpg"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn tint_none_is_none() {
        assert!(Tint::default().is_none());
        let mut img = RgbaImage::from_pixel(2, 2, image::Rgba([100, 150, 200, 255]));
        tinted(
            &mut img,
            Tint {
                grey: true,
                ..Tint::default()
            },
        );
        let p = img.get_pixel(0, 0);
        assert!(p[0] == p[1] && p[1] == p[2]);
    }
}

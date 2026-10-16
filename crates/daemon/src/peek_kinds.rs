//! What QUICK LOOK reads each kind with (`quicklook.rs` draws them all in
//! one frame). Each hands back pixels, text or a few facts; none opens a
//! window or has a look of its own. All of it runs on Quick Look's worker
//! thread, through the programs the system already has — a missing one only
//! means that kind falls back to what is plain to see (its picture from the
//! file window, and what it is).
//!
//! - a PDF: one page at a time, at the size it is shown — MuPDF's `mutool`
//!   where there is one (the fastest there is), else poppler's `pdftoppm`;
//! - a VIDEO: a frame a tenth of the way in, its length and size (`ffprobe`,
//!   `ffmpeg`);
//! - a SONG: its cover, title, artist, album, length and its loudness along
//!   the way (`ffprobe`, `ffmpeg`), played by `mpv` on a click;
//! - a FONT: a sheet in itself — "Aa", the alphabet, a line at four sizes;
//! - an ARCHIVE: what is in it (`bsdtar`, libarchive: nothing is unpacked);
//! - anything a freedesktop thumbnailer reads (an e-book…): its picture, big.

use std::path::Path;
use std::process::{Command, Stdio};

use image::RgbaImage;

use crate::launch::on_path;
use crate::thumb_more::{run, Scratch};

fn ext(name: &str) -> String {
    name.rsplit_once('.')
        .map_or(String::new(), |(_, e)| e.to_ascii_lowercase())
}

fn mime_is(name: &str, top: &str) -> bool {
    crate::thumb_more::mime_of(name).is_some_and(|m| m.starts_with(top))
}

pub(crate) fn is_pdf(name: &str) -> bool {
    ext(name) == "pdf"
}

pub(crate) fn is_video(name: &str) -> bool {
    matches!(
        ext(name).as_str(),
        "mp4"
            | "mkv"
            | "webm"
            | "mov"
            | "avi"
            | "m4v"
            | "mpg"
            | "mpeg"
            | "wmv"
            | "flv"
            | "3gp"
            | "ogv"
    ) || mime_is(name, "video/")
}

pub(crate) fn is_audio(name: &str) -> bool {
    matches!(
        ext(name).as_str(),
        "mp3" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "wav" | "aac" | "wma" | "aiff" | "alac"
    ) || mime_is(name, "audio/")
}

pub(crate) fn is_archive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        ".zip", ".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst",
        ".7z", ".rar", ".iso", ".cpio", ".xpi", ".jar", ".apk", ".cbz", ".cbr",
    ]
    .iter()
    .any(|e| lower.ends_with(e))
}

/// A program's standard output, if it ran well.
fn output(prog: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A picture file decoded and made premultiplied.
fn read_png(path: &str) -> Option<RgbaImage> {
    let mut img = image::open(path).ok()?.to_rgba8();
    for px in img.pixels_mut() {
        let a = u32::from(px[3]);
        if a < 255 {
            for c in 0..3 {
                px[c] = (u32::from(px[c]) * a / 255) as u8;
            }
        }
    }
    Some(img)
}

/// `w`×`h` at most, at `aspect` (width / height).
fn fit(aspect: f32, max: (u32, u32)) -> (u32, u32) {
    let (mw, mh) = (max.0 as f32, max.1 as f32);
    let (w, h) = if mw / mh > aspect {
        (mh * aspect, mh)
    } else {
        (mw, mw / aspect)
    };
    (w.round().max(1.0) as u32, h.round().max(1.0) as u32)
}

// ── PDF ─────────────────────────────────────────────────────────────────

/// How many pages, and the first one's shape (width / height).
pub(crate) fn pdf_facts(path: &Path) -> Option<(u32, f32)> {
    let p = path.to_str()?;
    let info = output("pdfinfo", &[p])?;
    let pages = info
        .lines()
        .find_map(|l| l.strip_prefix("Pages:"))
        .and_then(|v| v.trim().parse().ok())?;
    // "Page size:      595.276 x 841.89 pts (A4)"
    let aspect = info
        .lines()
        .find_map(|l| l.strip_prefix("Page size:"))
        .and_then(|v| {
            let mut it = v.split_whitespace();
            let w: f32 = it.next()?.parse().ok()?;
            it.next();
            let h: f32 = it.next()?.parse().ok()?;
            (w > 0.0 && h > 0.0).then_some(w / h)
        })
        .unwrap_or(210.0 / 297.0);
    Some((pages, aspect))
}

/// Page `page` (from 0), no bigger than `max`.
pub(crate) fn pdf_page(path: &Path, page: u32, aspect: f32, max: (u32, u32)) -> Option<RgbaImage> {
    let p = path.to_str()?;
    let (w, h) = fit(aspect, max);
    let out = Scratch::new("peek-pdf")?;
    let n = (page + 1).to_string();
    let done = if on_path("mutool") {
        let (ws, hs) = (w.to_string(), h.to_string());
        run(&[
            "mutool",
            "draw",
            "-q",
            "-o",
            out.path(),
            "-w",
            &ws,
            "-h",
            &hs,
            "-F",
            "png",
            p,
            &n,
        ]
        .map(str::to_owned))
    } else {
        // (`-singlefile` names it the prefix + ".png".)
        let prefix = out.path().trim_end_matches(".png").to_owned();
        let (ws, hs) = (w.to_string(), h.to_string());
        run(&[
            "pdftoppm",
            "-f",
            &n,
            "-l",
            &n,
            "-png",
            "-singlefile",
            "-scale-to-x",
            &ws,
            "-scale-to-y",
            &hs,
            p,
            &prefix,
        ]
        .map(str::to_owned))
    };
    if !done {
        return None;
    }
    // (A page is paper: on white, whatever it leaves clear.)
    let img = read_png(out.path())?;
    let mut paper =
        RgbaImage::from_pixel(img.width(), img.height(), image::Rgba([255, 255, 255, 255]));
    image::imageops::overlay(&mut paper, &img, 0, 0);
    Some(paper)
}

// ── video and sound ─────────────────────────────────────────────────────

/// What `ffprobe` says: length (s), picture size, and the tags asked for.
pub(crate) struct Probe {
    pub secs: f32,
    pub size: Option<(u32, u32)>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

pub(crate) fn probe(path: &Path) -> Option<Probe> {
    let p = path.to_str()?;
    let text = output(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_entries",
            "format=duration:format_tags=title,artist,album:stream=width,height",
            "-of",
            "default=nw=1",
            p,
        ],
    )?;
    let mut probe = Probe {
        secs: 0.0,
        size: None,
        title: None,
        artist: None,
        album: None,
    };
    let (mut w, mut h) = (None, None);
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.to_ascii_lowercase().as_str() {
            "duration" => probe.secs = v.parse().unwrap_or(0.0),
            "width" if w.is_none() => w = v.parse().ok(),
            "height" if h.is_none() => h = v.parse().ok(),
            "tag:title" if !v.is_empty() => probe.title = Some(v.to_owned()),
            "tag:artist" if !v.is_empty() => probe.artist = Some(v.to_owned()),
            "tag:album" if !v.is_empty() => probe.album = Some(v.to_owned()),
            _ => {}
        }
    }
    probe.size = w.zip(h).filter(|&(w, h): &(u32, u32)| w > 0 && h > 0);
    Some(probe)
}

/// A frame `at` seconds in, no bigger than `max` — or a song's cover.
pub(crate) fn frame(path: &Path, at: f32, max: (u32, u32)) -> Option<RgbaImage> {
    let ffmpeg = crate::thumbs::ffmpeg_tool()?;
    let out = Scratch::new("peek-frame")?;
    let scale = format!(
        "scale={}:{}:force_original_aspect_ratio=decrease",
        max.0, max.1
    );
    let at = format!("{at:.2}");
    let mut words: Vec<String> = vec![ffmpeg, "-v".into(), "error".into(), "-y".into()];
    if at != "0.00" {
        words.extend(["-ss".into(), at]);
    }
    words.extend(
        [
            "-i",
            path.to_str()?,
            "-an",
            "-frames:v",
            "1",
            "-vf",
            &scale,
            out.path(),
        ]
        .map(str::to_owned),
    );
    (run(&words) && Path::new(out.path()).is_file())
        .then(|| read_png(out.path()))
        .flatten()
}

/// The loudness along a song, `n` steps of 0..1 (its first quarter hour).
pub(crate) fn wave(path: &Path, n: usize) -> Option<Vec<f32>> {
    let ffmpeg = crate::thumbs::ffmpeg_tool()?;
    let out = Command::new(ffmpeg)
        .args(["-v", "error", "-t", "900", "-i"])
        .arg(path)
        .args(["-ac", "1", "-ar", "800", "-f", "s16le", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let samples: Vec<f32> = out
        .stdout
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]]).unsigned_abs()))
        .collect();
    if samples.len() < n {
        return None;
    }
    let per = samples.len() / n;
    let mut steps: Vec<f32> = samples
        .chunks(per)
        .take(n)
        .map(|c| c.iter().copied().fold(0.0, f32::max))
        .collect();
    let top = steps.iter().copied().fold(1.0, f32::max);
    for s in &mut steps {
        *s = (*s / top).sqrt();
    }
    Some(steps)
}

/// `m:ss` (or `h:mm:ss`).
pub(crate) fn clock(secs: f32) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Play a song from `from` seconds; it dies with the dock.
pub(crate) fn play(path: &Path, from: f32) -> Option<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("mpv");
    cmd.args(["--no-video", "--really-quiet", "--no-terminal"])
        .arg(format!("--start={from:.2}"))
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: only an async-signal-safe call between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    cmd.spawn()
        .map_err(|e| tracing::warn!("quick look: mpv: {e}"))
        .ok()
}

// ── fonts ───────────────────────────────────────────────────────────────

/// A font's sheet, `max` at most, its letters in `ink`; and its family.
pub(crate) fn font_sheet(
    path: &Path,
    max: (u32, u32),
    ink: [u8; 3],
) -> Option<(RgbaImage, String)> {
    let data = std::fs::read(path).ok()?;
    let (w, h) = fit(1.25, max);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let k = w as f32 / 1000.0;
    let family = crate::thumb_more::with_fonts(|fonts| {
        let before = fonts.db().len();
        fonts.db_mut().load_font_data(data);
        let face = fonts.db().faces().nth(before)?;
        let family = face.families.first()?.0.clone();
        let id = face.id;
        let attrs = glyphon::Attrs::new().family(glyphon::Family::Name(&family));
        let clip = (40.0 * k, 30.0 * k, w as f32 - 80.0 * k, h as f32 - 60.0 * k);
        let mut y = 30.0 * k;
        let mut line = |text: &str, size: f32, y: &mut f32| {
            crate::thumb_more::draw_text_in(
                &mut px,
                w,
                text,
                attrs,
                size * k,
                (clip.0, *y),
                clip,
                ink,
                fonts,
            );
            *y += size * k * 1.32;
        };
        line("Aa", 230.0, &mut y);
        line("ABCDEFGHIJKLMNÑOPQRSTUVWXYZ", 34.0, &mut y);
        line("abcdefghijklmnñopqrstuvwxyz", 34.0, &mut y);
        line("0123456789 ¿? ¡! & @ € $ ( ) « »", 34.0, &mut y);
        y += 18.0 * k;
        for size in [14.0, 20.0, 28.0, 40.0] {
            line(
                "El veloz murciélago hindú comía feliz cardillo y kiwi",
                size,
                &mut y,
            );
        }
        // (The face is this file's only: it goes with the drawing.)
        fonts.db_mut().remove_face(id);
        Some(family)
    })?;
    Some((RgbaImage::from_raw(w, h, px)?, family))
}

// ── archives ────────────────────────────────────────────────────────────

/// What is in an archive, a line each ("    4.1 MB  photos/a.jpg"), and
/// how many files; nothing unpacked.
pub(crate) fn archive_list(path: &Path) -> Option<(Vec<String>, usize, u64)> {
    let p = path.to_str()?;
    let text = output("bsdtar", &["-tvf", p])?;
    let (mut lines, mut files, mut total) = (Vec::new(), 0usize, 0u64);
    for l in text.lines() {
        // "-rw-r--r--  0 max users 4300000 Sep 14 2026 DSC_0412.jpg"
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 9 {
            continue;
        }
        let size: u64 = f[4].parse().unwrap_or(0);
        let name = f[8..].join(" ");
        let dir = l.starts_with('d') || name.ends_with('/');
        if !dir {
            files += 1;
            total += size;
        }
        let shown = if dir {
            String::new()
        } else {
            crate::desktop_props::size_text(size)
        };
        lines.push(format!("{shown:>10}  {name}"));
    }
    (!lines.is_empty()).then_some((lines, files, total))
}

// ── anything else with a thumbnailer ────────────────────────────────────

/// What a freedesktop thumbnailer draws of it, asked big.
pub(crate) fn thumbnailed(path: &Path, max: (u32, u32)) -> Option<RgbaImage> {
    let got = std::cell::RefCell::new(None);
    crate::thumb_more::by_thumbnailer_at(path.to_str()?, max.0.max(max.1).min(1024), |png| {
        *got.borrow_mut() = read_png(png);
        got.borrow().is_some().then(Vec::new)
    });
    got.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_told_by_their_names() {
        assert!(is_pdf("Informe.PDF"));
        assert!(
            is_video("a.mkv")
                && is_audio("b.flac")
                && is_archive("c.tar.gz")
                && is_archive("d.zip")
        );
        assert!(!is_video("notes.txt") && !is_archive("tarot.png"));
        assert_eq!(clock(247.4), "4:07");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(fit(2.0, (800, 600)), (800, 400));
        assert_eq!(fit(0.5, (800, 600)), (300, 600));
    }
}

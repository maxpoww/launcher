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

// ── pictures ────────────────────────────────────────────────────────────

/// A picture read whole, turned as its camera says (EXIF orientation: a
/// phone's photo taken upright is stored on its side).
pub(crate) fn open_oriented(path: &Path) -> Option<image::DynamicImage> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    Some(img)
}

/// A JPEG's orientation, from its header alone.
fn jpeg_orientation(path: &Path) -> image::metadata::Orientation {
    use image::ImageDecoder;
    std::fs::File::open(path)
        .ok()
        .and_then(|f| image::codecs::jpeg::JpegDecoder::new(std::io::BufReader::new(f)).ok())
        .and_then(|mut d| d.orientation().ok())
        .unwrap_or(image::metadata::Orientation::NoTransforms)
}

/// A JPEG decoded straight at the smallest of 1/1, 1/2, 1/4, 1/8 of its
/// size still as big as `max` asks (the IDCT does the shrinking: a 24 MP
/// photo for a 1200 px frame is decoded at 1/4 — a sixteenth of the work),
/// then brought to `max`, turned as its camera says. Straight RGBA, and its
/// own size (as turned). `None`: not a JPEG this reads (CMYK, 16 bits…).
pub(crate) fn jpeg_scaled(path: &Path, max: (u32, u32)) -> Option<(RgbaImage, (u32, u32))> {
    let file = std::fs::File::open(path).ok()?;
    let mut dec = jpeg_decoder::Decoder::new(std::io::BufReader::new(file));
    dec.read_info().ok()?;
    let info = dec.info()?;
    let own = (u32::from(info.width), u32::from(info.height));
    // (The size it is shown at, its shape kept.)
    let (tw, th) = fit(
        own.0 as f32 / own.1.max(1) as f32,
        (max.0.min(own.0), max.1.min(own.1)),
    );
    let (sw, sh) = dec
        .scale(
            tw.min(u32::from(u16::MAX)) as u16,
            th.min(u32::from(u16::MAX)) as u16,
        )
        .ok()?;
    let pixels = dec.decode().ok()?;
    let (sw, sh) = (u32::from(sw), u32::from(sh));
    let rgba: Vec<u8> = match dec.info()?.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&l| [l, l, l, 255]).collect(),
        _ => return None,
    };
    let img = RgbaImage::from_raw(sw, sh, rgba)?;
    let img = if (sw, sh) == (tw, th) {
        img
    } else {
        image::imageops::thumbnail(&img, tw, th)
    };
    let mut dynamic = image::DynamicImage::ImageRgba8(img);
    let orientation = jpeg_orientation(path);
    dynamic.apply_orientation(orientation);
    let turned = matches!(
        orientation,
        image::metadata::Orientation::Rotate90
            | image::metadata::Orientation::Rotate270
            | image::metadata::Orientation::Rotate90FlipH
            | image::metadata::Orientation::Rotate270FlipH
    );
    Some((
        dynamic.to_rgba8(),
        if turned { (own.1, own.0) } else { own },
    ))
}

/// A MOVING PICTURE (an animated GIF or WebP): its frames at the size they
/// are shown, premultiplied RGBA, and how long each stays (ms).
pub(crate) struct Anim {
    pub size: (u32, u32),
    pub frames: Vec<(Vec<u8>, u32)>,
}

impl Anim {
    /// The frame at `ms` into it, going round.
    pub(crate) fn at(&self, ms: f32) -> usize {
        let total: u32 = self.frames.iter().map(|f| f.1).sum();
        if total == 0 {
            return 0;
        }
        let mut t = (ms as u32) % total;
        for (i, (_, d)) in self.frames.iter().enumerate() {
            if t < *d {
                return i;
            }
            t -= d;
        }
        0
    }
}

/// How much a moving picture's frames may hold.
const ANIM_BYTES: usize = 48 << 20;

/// An animated GIF or WebP, its frames brought within `max` (and, all
/// together, within `ANIM_BYTES`). `None`: not animated (one frame), or not
/// read.
pub(crate) fn animation(path: &Path, max: (u32, u32)) -> Option<Anim> {
    use image::AnimationDecoder;
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let reader = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let frames: Vec<image::Frame> = if name.ends_with(".gif") {
        image::codecs::gif::GifDecoder::new(reader)
            .ok()?
            .into_frames()
            .take(2000)
            .collect::<Result<_, _>>()
            .ok()?
    } else if name.ends_with(".webp") {
        let decoder = image::codecs::webp::WebPDecoder::new(reader).ok()?;
        if !decoder.has_animation() {
            return None;
        }
        decoder
            .into_frames()
            .take(2000)
            .collect::<Result<_, _>>()
            .ok()?
    } else {
        return None;
    };
    if frames.len() < 2 {
        return None;
    }
    let (w0, h0) = frames[0].buffer().dimensions();
    let (mut w, mut h) = fit(w0 as f32 / h0.max(1) as f32, (max.0.min(w0), max.1.min(h0)));
    // (All the frames within the budget: smaller when there are many.)
    let need = frames.len() * (w * h * 4) as usize;
    if need > ANIM_BYTES {
        let k = (ANIM_BYTES as f32 / need as f32).sqrt();
        w = ((w as f32 * k) as u32).max(16);
        h = ((h as f32 * k) as u32).max(16);
    }
    let frames = frames
        .into_iter()
        .map(|f| {
            let (num, den) = f.delay().numer_denom_ms();
            // (Browsers treat a delay under 20 ms as 100 ms.)
            let ms = num.checked_div(den).unwrap_or(100);
            let ms = if ms < 20 { 100 } else { ms };
            let img = f.into_buffer();
            let mut img = if img.dimensions() == (w, h) {
                img
            } else {
                image::imageops::thumbnail(&img, w, h)
            };
            // (Premultiplied, as every picture drawn is.)
            for px in img.pixels_mut() {
                let a = u32::from(px[3]);
                if a < 255 {
                    for c in 0..3 {
                        px[c] = (u32::from(px[c]) * a / 255) as u8;
                    }
                }
            }
            (img.into_raw(), ms)
        })
        .collect();
    Some(Anim {
        size: (w, h),
        frames,
    })
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

/// Page `page`'s size in points (its first page's when not told apart).
pub(crate) fn pdf_points(path: &Path, page: u32) -> Option<(f32, f32)> {
    let p = path.to_str()?;
    let n = (page + 1).to_string();
    let info = output("pdfinfo", &["-f", &n, "-l", &n, p])?;
    let line = info
        .lines()
        .find(|l| l.contains("size:") && l.contains("pts"))?;
    let v = line.split(':').nth(1)?;
    let mut it = v.split_whitespace();
    let w: f32 = it.next()?.parse().ok()?;
    it.next();
    let h: f32 = it.next()?.parse().ok()?;
    (w > 0.0 && h > 0.0).then_some((w, h))
}

/// A part (x, y, w, h pixels) of page `page` drawn at `dpi`, on white.
pub(crate) fn pdf_crop(
    path: &Path,
    page: u32,
    dpi: f32,
    (x, y, w, h): (u32, u32, u32, u32),
) -> Option<RgbaImage> {
    let p = path.to_str()?;
    let out = Scratch::new("peek-pdf-part")?;
    let prefix = out.path().trim_end_matches(".png").to_owned();
    let n = (page + 1).to_string();
    let words = [
        "pdftoppm",
        "-f",
        &n,
        "-l",
        &n,
        "-png",
        "-singlefile",
        "-r",
        &format!("{dpi:.2}"),
        "-x",
        &x.to_string(),
        "-y",
        &y.to_string(),
        "-W",
        &w.to_string(),
        "-H",
        &h.to_string(),
        p,
        &prefix,
    ]
    .map(str::to_owned);
    if !run(&words) {
        return None;
    }
    let img = read_png(out.path())?;
    let mut paper =
        RgbaImage::from_pixel(img.width(), img.height(), image::Rgba([255, 255, 255, 255]));
    image::imageops::overlay(&mut paper, &img, 0, 0);
    Some(paper)
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

/// A VIDEO PLAYING: `ffmpeg` hands its frames over, `w`×`h` RGBA each, at
/// the video's own pace (at most 30 a second), from `from` seconds; the
/// sound is `mpv`'s, started with it. A thread reads the frames and sends
/// them on with `tag`; an empty frame says the video ended.
///
/// A BIG video (more than 1920×1080) is decoded AND made small on the
/// graphics chip when it can (VA-API: 4K to 720p cost a seventeenth of the
/// processor's time on the dev box): the render node that can is found once
/// (`gpu_node`, a frame tried on each), and if it fails before its first
/// frame the processor does it after all.
pub(crate) struct Stream {
    /// The decoder running now (the thread may start the processor's one
    /// after the chip's failed), and whether it was stopped.
    ffmpeg: std::sync::Arc<std::sync::Mutex<(Option<std::process::Child>, bool)>>,
    sound: Option<std::process::Child>,
}

impl Stream {
    pub(crate) fn stop(&mut self) {
        if let Ok(mut running) = self.ffmpeg.lock() {
            running.1 = true;
            if let Some(child) = running.0.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if let Some(child) = self.sound.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// How a video is decoded: by the processor, or on the chip at this render node.
#[derive(Clone)]
enum Decode {
    Cpu,
    Gpu(String),
}

fn decoder(
    ffmpeg: &str,
    path: &Path,
    from: f32,
    (w, h): (u32, u32),
    how: &Decode,
    frames: Option<u32>,
) -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-v", "error"]);
    let filter = match how {
        Decode::Cpu => format!("fps=30,scale={w}:{h},format=rgba"),
        Decode::Gpu(node) => {
            cmd.args([
                "-init_hw_device",
                &format!("vaapi=va:{node}"),
                "-hwaccel",
                "vaapi",
                "-hwaccel_device",
                "va",
                "-hwaccel_output_format",
                "vaapi",
            ]);
            format!("fps=30,scale_vaapi=w={w}:h={h}:format=rgba,hwdownload,format=rgba")
        }
    };
    cmd.args(["-ss", &format!("{from:.2}")]);
    if frames.is_none() {
        cmd.arg("-re");
    }
    cmd.arg("-i").arg(path).args(["-an", "-vf", &filter]);
    // (A try: one frame, thrown away — a frame held in a pipe nobody reads
    // would stop it.)
    match frames {
        Some(n) => cmd
            .args(["-frames:v", &n.to_string(), "-f", "null", "-"])
            .stdout(Stdio::null()),
        None => cmd.args(["-f", "rawvideo", "-"]).stdout(Stdio::piped()),
    };
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    // SAFETY: only an async-signal-safe call between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    cmd
}

/// The render node whose chip decodes and scales video here, found once
/// (one frame of `path` tried on each, at most four seconds).
fn gpu_node(ffmpeg: &str, path: &Path, size: (u32, u32)) -> Option<String> {
    static FOUND: std::sync::Mutex<Option<Option<String>>> = std::sync::Mutex::new(None);
    if let Some(found) = FOUND.lock().ok().and_then(|f| f.clone()) {
        return found;
    }
    let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.path().to_string_lossy().into_owned())
                .filter(|p| p.contains("renderD"))
                .collect()
        })
        .unwrap_or_default();
    nodes.sort();
    let found = nodes.into_iter().find(|node| {
        let Ok(mut child) =
            decoder(ffmpeg, path, 0.0, size, &Decode::Gpu(node.clone()), Some(1)).spawn()
        else {
            return false;
        };
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) if start.elapsed().as_secs() < 4 => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
        }
    });
    tracing::info!(
        "quick look: video on the graphics chip: {}",
        found.as_deref().unwrap_or("none here")
    );
    if let Ok(mut f) = FOUND.lock() {
        *f = Some(found.clone());
    }
    found
}

pub(crate) fn stream(
    path: &Path,
    from: f32,
    (w, h): (u32, u32),
    own: (u32, u32),
    tag: u64,
    frames: smithay_client_toolkit::reexports::calloop::channel::Sender<(u64, Vec<u8>)>,
) -> Option<Stream> {
    use std::io::Read;
    let ffmpeg = crate::thumbs::ffmpeg_tool()?;
    let running = std::sync::Arc::new(std::sync::Mutex::new((None::<std::process::Child>, false)));
    let shared = running.clone();
    let path_buf = path.to_path_buf();
    let big = own.0 > 1920 || own.1 > 1080;
    let size = (w * h * 4) as usize;
    let read = std::thread::Builder::new()
        .name("quick-look-video".into())
        .spawn(move || {
            // The chip first for a big one (if there is one that can), else, or
            // when it fails before its first frame, the processor.
            let mut ways = Vec::new();
            if big {
                if let Some(node) = gpu_node(&ffmpeg, &path_buf, (w, h)) {
                    ways.push(Decode::Gpu(node));
                }
            }
            ways.push(Decode::Cpu);
            for way in ways {
                let out = {
                    let Ok(mut r) = shared.lock() else { return };
                    if r.1 {
                        return;
                    }
                    let Ok(mut child) =
                        decoder(&ffmpeg, &path_buf, from, (w, h), &way, None).spawn()
                    else {
                        continue;
                    };
                    let out = child.stdout.take();
                    // (The way before, which gave nothing: waited for.)
                    if let Some(mut old) = r.0.take() {
                        let _ = old.wait();
                    }
                    r.0 = Some(child);
                    out
                };
                let Some(mut out) = out else { continue };
                let mut sent = 0u64;
                loop {
                    let mut frame = vec![0u8; size];
                    if out.read_exact(&mut frame).is_err() {
                        break;
                    }
                    sent += 1;
                    if frames.send((tag, frame)).is_err() {
                        return;
                    }
                }
                // (It played, or it was stopped: done. It never gave a frame:
                // the next way.)
                if sent > 0 || shared.lock().map_or(true, |r| r.1) {
                    let _ = frames.send((tag, Vec::new()));
                    return;
                }
                tracing::info!(
                    "quick look: video: {} gave no frame, trying the next way",
                    match way {
                        Decode::Gpu(_) => "the graphics chip",
                        Decode::Cpu => "the processor",
                    }
                );
            }
            let _ = frames.send((tag, Vec::new()));
        });
    if let Err(e) = read {
        tracing::warn!("quick look: no video thread: {e}");
        return None;
    }
    Some(Stream {
        ffmpeg: running,
        sound: play(path, from),
    })
}

/// A video's frame size for playing: its shape within `max`, at most 1280
/// wide (what a weak machine moves 30 times a second), even.
pub(crate) fn stream_size(own: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let aspect = own.0.max(1) as f32 / own.1.max(1) as f32;
    let (w, h) = fit(
        aspect,
        (
            max.0.min(1280).min(own.0.max(2)),
            max.1.min(720).min(own.1.max(2)),
        ),
    );
    (w & !1, h & !1)
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
    fn a_big_jpeg_is_decoded_small_and_its_shape_kept() {
        let d = std::env::temp_dir().join(format!("pk-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("big.jpg");
        let img = image::RgbImage::from_fn(1600, 1200, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        img.save(&f).unwrap();
        let (small, own) = jpeg_scaled(&f, (500, 500)).unwrap();
        assert_eq!(own, (1600, 1200));
        assert_eq!(small.dimensions(), (500, 375));
        std::fs::remove_dir_all(&d).ok();
    }

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

#[cfg(test)]
mod bench {
    /// `QL_BENCH=<a big jpeg> cargo test --release -- --ignored jpeg_bench --nocapture`
    #[test]
    #[ignore]
    fn jpeg_bench() {
        let Some(path) = std::env::var_os("QL_BENCH") else {
            return;
        };
        let path = std::path::Path::new(&path);
        for _ in 0..2 {
            let t = std::time::Instant::now();
            let full = image::open(path).unwrap();
            let _ = full.thumbnail(1180, 690);
            let a = t.elapsed();
            let t = std::time::Instant::now();
            let _ = super::jpeg_scaled(path, (1180, 690)).unwrap();
            let b = t.elapsed();
            eprintln!("image (zune) whole + thumbnail: {a:?} · jpeg-decoder scaled: {b:?}");
        }
    }
}

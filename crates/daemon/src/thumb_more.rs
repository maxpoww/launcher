//! MORE THUMBNAILS (Max, 2026-10-15: "as many files as possible shown as
//! thumbnails"). What `thumbs.rs` asks for when a file is not a picture it
//! can read itself:
//!
//! - the system's own THUMBNAILERS (the freedesktop way GNOME's apps use:
//!   `$XDG_DATA_DIRS/thumbnailers/*.thumbnailer`, each naming the MIME types
//!   it previews and a command with `%i %u %o %s`). Whatever a package brings
//!   — videos and songs' covers (ffmpegthumbnailer), PDF, comics and books
//!   (Papers), office documents (libgsf), epub — is ours too. The type of a
//!   file is read from the shared MIME database's globs (`mime/globs2`);
//! - FFMPEG for what it can read: a video's frame, a song's cover, the
//!   pictures `image` cannot (HEIC, AVIF, JPEG XL, PSD, EXR, TGA…);
//! - a camera's RAW photo: its own JPEG inside (`dcraw -e`);
//! - drawn here: a TEXT file (code, notes, a log) as a page with its first
//!   lines, and a FONT as "Aa" in itself.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::apps::ICON_SIZE;

/// How long one outside program may take for one file.
const PATIENCE: Duration = Duration::from_secs(15);

/// The directories the shared data is in, the owner's first.
fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut dirs = vec![std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from)];
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    dirs
}

/// The MIME type of a file by its name: the shared database's `*.ext`
/// globs (the heaviest one wins; the first seen at a tie).
pub(crate) fn mime_of(name: &str) -> Option<&'static str> {
    static GLOBS: OnceLock<HashMap<String, (u32, String)>> = OnceLock::new();
    let globs = GLOBS.get_or_init(|| {
        let mut map: HashMap<String, (u32, String)> = HashMap::new();
        for dir in data_dirs() {
            let Ok(text) = std::fs::read_to_string(dir.join("mime/globs2")) else { continue };
            parse_globs(&text, &mut map);
        }
        map
    });
    let lower = name.to_ascii_lowercase();
    // The longest ending first (`tar.gz` before `gz`).
    let mut at = lower.find('.');
    while let Some(i) = at {
        if let Some((_, mime)) = globs.get(&lower[i + 1..]) {
            return Some(mime.as_str());
        }
        at = lower[i + 1..].find('.').map(|j| i + 1 + j);
    }
    None
}

/// `weight:mime:glob[:flags]` lines; only `*.ext` globs are kept.
fn parse_globs(text: &str, map: &mut HashMap<String, (u32, String)>) {
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let mut f = line.splitn(4, ':');
        let (Some(w), Some(mime), Some(glob)) = (f.next(), f.next(), f.next()) else { continue };
        let Some(ext) = glob.strip_prefix("*.") else { continue };
        if ext.contains(['*', '?', '[']) {
            continue;
        }
        let w: u32 = w.parse().unwrap_or(50);
        let ext = ext.to_ascii_lowercase();
        if map.get(&ext).is_none_or(|(had, _)| w > *had) {
            map.insert(ext, (w, mime.to_owned()));
        }
    }
}

/// One `.thumbnailer`: its command (`%i` the path, `%u` its URI, `%o` the
/// picture to write, `%s` the size) and the program it needs.
#[derive(Debug, Clone, PartialEq)]
struct Thumbnailer {
    exec: String,
    try_exec: Option<String>,
}

/// The system's thumbnailers by the MIME types they take (the first found
/// for a type wins: the owner's own before the system's).
fn thumbnailers() -> &'static HashMap<String, Thumbnailer> {
    static ALL: OnceLock<HashMap<String, Thumbnailer>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut map = HashMap::new();
        for dir in data_dirs() {
            let Ok(entries) = std::fs::read_dir(dir.join("thumbnailers")) else { continue };
            let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            files.sort();
            for file in files.iter().filter(|f| f.extension().is_some_and(|e| e == "thumbnailer")) {
                let Ok(text) = std::fs::read_to_string(file) else { continue };
                if let Some((t, mimes)) = parse_thumbnailer(&text) {
                    for mime in mimes {
                        map.entry(mime).or_insert_with(|| t.clone());
                    }
                }
            }
        }
        debug!("thumb: {} types have a thumbnailer", map.len());
        map
    })
}

fn parse_thumbnailer(text: &str) -> Option<(Thumbnailer, Vec<String>)> {
    let (mut exec, mut try_exec, mut mimes) = (None, None, Vec::new());
    for line in text.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("Exec=") {
            exec = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("TryExec=") {
            try_exec = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("MimeType=") {
            mimes = v.split(';').filter(|m| !m.is_empty()).map(str::to_owned).collect();
        }
    }
    Some((Thumbnailer { exec: exec?, try_exec }, mimes))
}

/// Whether a program is on the PATH (or a path that is there).
fn on_path(prog: &str) -> bool {
    if prog.contains('/') {
        return Path::new(prog).is_file();
    }
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
}

/// The command's words, its `%` codes filled in.
fn fill(exec: &str, path: &str, out: &str, size: u32) -> Vec<String> {
    let uri = format!("file://{}", crate::trash::encode_path(Path::new(path)));
    exec.split_whitespace()
        .map(|w| {
            w.replace("%i", path)
                .replace("%u", &uri)
                .replace("%o", out)
                .replace("%s", &size.to_string())
                .replace("%%", "%")
        })
        .collect()
}

/// Run a program to its end, or `PATIENCE`; whether it went well.
fn run(words: &[String]) -> bool {
    let Some((prog, args)) = words.split_first() else { return false };
    let child = Command::new(prog).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            debug!("thumb: {prog} would not start: {e}");
            return false;
        }
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() < PATIENCE => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                warn!("thumb: {prog} took too long");
                return false;
            }
        }
    }
}

/// A scratch picture for one outside program, gone when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Option<Self> {
        let dir = std::env::temp_dir().join(format!("waverunner-thumbs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self(dir.join(format!("{tag}.png"))))
    }
    fn path(&self) -> &str {
        self.0.to_str().unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Through the system's thumbnailer for this file's type, if there is one.
pub(crate) fn by_thumbnailer(path: &str, decode: impl Fn(&str) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    let mime = mime_of(path)?;
    let t = thumbnailers().get(mime)?;
    if let Some(needs) = &t.try_exec {
        if !on_path(needs) {
            return None;
        }
    }
    let out = Scratch::new("thumbnailer")?;
    let words = fill(&t.exec, path, out.path(), ICON_SIZE);
    debug!("thumb: {mime} → {}", words.first().map_or("", String::as_str));
    (run(&words) && out.0.is_file()).then(|| decode(out.path())).flatten()
}

/// One picture out of FFMPEG: a video's frame a little in, a song's cover,
/// a picture `image` cannot read.
pub(crate) fn by_ffmpeg(path: &str, ffmpeg: &str, decode: impl Fn(&str) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    let out = Scratch::new("ffmpeg")?;
    let scale = format!("scale={ICON_SIZE}:{ICON_SIZE}:force_original_aspect_ratio=decrease");
    let words: Vec<String> = [ffmpeg, "-v", "error", "-y", "-i", path, "-an", "-frames:v", "1", "-vf", &scale, out.path()]
        .into_iter()
        .map(str::to_owned)
        .collect();
    (run(&words) && out.0.is_file()).then(|| decode(out.path())).flatten()
}

/// A camera's RAW photo: the JPEG it carries (`dcraw -e -c`).
pub(crate) fn by_dcraw(path: &str, decode: impl Fn(&str) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    if !on_path("dcraw") {
        return None;
    }
    let out = Scratch::new("dcraw")?;
    let file = std::fs::File::create(&out.0).ok()?;
    let ok = Command::new("dcraw")
        .args(["-e", "-c", path])
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    (ok && std::fs::metadata(&out.0).is_ok_and(|m| m.len() > 0)).then(|| decode(out.path())).flatten()
}

/// Camera RAW endings.
pub(crate) fn is_raw(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "cr2" | "cr3" | "crw" | "nef" | "nrw" | "arw" | "srf" | "sr2" | "dng" | "orf" | "rw2" | "raf" | "pef" | "srw" | "x3f" | "3fr" | "kdc" | "mrw" | "erf" | "mef"
    )
}

/// Font endings.
pub(crate) fn is_font(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "ttf" | "otf" | "ttc" | "otc" | "woff" | "woff2")
}

/// The first bytes of a file, if they read as text (UTF-8, no NULs, few
/// control characters).
pub(crate) fn text_head(path: &str) -> Option<String> {
    let mut buf = vec![0u8; 6 * 1024];
    let n = std::fs::File::open(path).ok()?.read(&mut buf).ok()?;
    buf.truncate(n);
    if buf.is_empty() || buf.contains(&0) {
        return None;
    }
    // (A cut in the middle of a character at the end is no reason to refuse.)
    let text = match std::str::from_utf8(&buf) {
        Ok(t) => t.to_owned(),
        Err(e) if e.valid_up_to() + 4 >= buf.len() => String::from_utf8_lossy(&buf[..e.valid_up_to()]).into_owned(),
        Err(_) => return None,
    };
    let odd = text.chars().filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')).count();
    (odd * 50 <= text.len().max(1)).then_some(text)
}

/// THE PAGE: a white sheet a little taller than wide, on a clear square,
/// premultiplied RGBA `ICON_SIZE`², and the box inside its margins.
fn page() -> (Vec<u8>, (f32, f32, f32, f32)) {
    let s = ICON_SIZE as f32;
    let (h, w) = (s * 0.94, s * 0.94 * 0.78);
    let (x0, y0) = ((s - w) / 2.0, (s - h) / 2.0);
    let r = s * 0.035;
    let mut px = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Signed distance to the rounded sheet, for a soft edge.
            let dx = (fx - (x0 + w / 2.0)).abs() - (w / 2.0 - r);
            let dy = (fy - (y0 + h / 2.0)).abs() - (h / 2.0 - r);
            let d = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - r;
            let a = (0.5 - d).clamp(0.0, 1.0);
            if a > 0.0 {
                let i = ((y * ICON_SIZE + x) * 4) as usize;
                let v = (250.0 * a) as u8;
                px[i..i + 4].copy_from_slice(&[v, v, v.saturating_sub(2), (255.0 * a) as u8]);
            }
        }
    }
    let m = s * 0.07;
    (px, (x0 + m, y0 + m, w - 2.0 * m, h - 2.0 * m))
}

/// Draw `text` into `px` (premultiplied, over), clipped to `clip`.
// (One knob per thing drawn: a struct for them would be read once.)
#[allow(clippy::too_many_arguments)]
fn draw_text(
    px: &mut [u8],
    text: &str,
    attrs: glyphon::Attrs,
    font_px: f32,
    at: (f32, f32),
    clip: (f32, f32, f32, f32),
    ink: [u8; 3],
    fonts: &mut glyphon::FontSystem,
) {
    thread_local! {
        static SWASH: std::cell::RefCell<glyphon::SwashCache> = std::cell::RefCell::new(glyphon::SwashCache::new());
    }
    let mut buffer = glyphon::Buffer::new(fonts, glyphon::Metrics::new(font_px, font_px * 1.32));
    buffer.set_size(fonts, Some(clip.2), Some(clip.3));
    buffer.set_wrap(fonts, glyphon::cosmic_text::Wrap::None);
    buffer.set_text(fonts, text, attrs, glyphon::Shaping::Advanced);
    buffer.shape_until_scroll(fonts, false);
    let (cx0, cy0, cx1, cy1) = (clip.0, clip.1, clip.0 + clip.2, clip.1 + clip.3);
    SWASH.with(|swash| {
        buffer.draw(fonts, &mut swash.borrow_mut(), glyphon::Color::rgb(ink[0], ink[1], ink[2]), |x, y, w, h, c| {
            for dy in 0..h as i32 {
                for dx in 0..w as i32 {
                    let (gx, gy) = (at.0 + (x + dx) as f32, at.1 + (y + dy) as f32);
                    if gx < cx0 || gy < cy0 || gx >= cx1 || gy >= cy1 {
                        continue;
                    }
                    let i = ((gy as u32 * ICON_SIZE + gx as u32) * 4) as usize;
                    let a = u32::from(c.a());
                    for k in 0..3 {
                        let src = u32::from([c.r(), c.g(), c.b()][k]) * a / 255;
                        px[i + k] = (src + u32::from(px[i + k]) * (255 - a) / 255) as u8;
                    }
                    px[i + 3] = (a + u32::from(px[i + 3]) * (255 - a) / 255) as u8;
                }
            }
        });
    });
}

thread_local! {
    /// This thread's fonts (the dock's index: no scan of every font file).
    static FONTS: std::cell::RefCell<Option<glyphon::FontSystem>> = const { std::cell::RefCell::new(None) };
}

/// With this thread's fonts; `None` with none at all (cosmic-text gives up
/// with a panic when it has no font) or when drawing failed — a page that
/// cannot be drawn must never take the thumbnailer's thread with it.
fn with_fonts<R>(f: impl FnOnce(&mut glyphon::FontSystem) -> Option<R>) -> Option<R> {
    FONTS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let fonts = cell.get_or_insert_with(|| {
            glyphon::FontSystem::new_with_locale_and_db(crate::font_index::locale(), crate::font_index::database())
        });
        if fonts.db().is_empty() {
            return None;
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(fonts))).ok().flatten()
    })
}

/// A TEXT file as a page with its first lines (code in the monospace face).
pub(crate) fn text_page(path: &str) -> Option<Vec<u8>> {
    let head = text_head(path)?;
    let lines: Vec<&str> = head.lines().take(40).collect();
    if lines.iter().all(|l| l.trim().is_empty()) {
        return None;
    }
    let body = lines.join("\n").replace('\t', "    ");
    let code = matches!(crate::files::file_asset_name(path), "asset-code") || mime_of(path).is_some_and(|m| !m.starts_with("text/plain"));
    let (mut px, clip) = page();
    with_fonts(|fonts| {
        let attrs = if code { glyphon::Attrs::new().family(glyphon::Family::Monospace) } else { glyphon::Attrs::new() };
        draw_text(&mut px, &body, attrs, 11.0, (clip.0, clip.1), clip, [60, 62, 68], fonts);
        Some(())
    })?;
    Some(crate::apps::with_mips(px))
}

/// A FONT as "Aa" in itself, its name under it.
pub(crate) fn font_page(path: &str) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let (mut px, clip) = page();
    let drawn = with_fonts(|fonts| {
        let before = fonts.db().len();
        fonts.db_mut().load_font_data(data);
        let face = fonts.db().faces().nth(before)?;
        let family = face.families.first()?.0.clone();
        let id = face.id;
        let attrs = glyphon::Attrs::new().family(glyphon::Family::Name(&family));
        let big = clip.2 * 0.6;
        draw_text(&mut px, "Aa", attrs, big, (clip.0, clip.1 + clip.3 * 0.2), clip, [40, 42, 48], fonts);
        let name_at = (clip.0, clip.1 + clip.3 * 0.78);
        draw_text(&mut px, &family, glyphon::Attrs::new(), 13.0, name_at, clip, [110, 114, 122], fonts);
        // (The face is this file's only: it goes with the drawing.)
        fonts.db_mut().remove_face(id);
        Some(())
    });
    drawn.map(|()| crate::apps::with_mips(px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_and_thumbnailers_read_as_the_spec_says() {
        let mut map = HashMap::new();
        parse_globs("# comment\n50:video/mp4:*.mp4\n80:application/x-compressed-tar:*.tar.gz\n10:text/plain:*.txt\n50:text/x-log:*.log:cs\n", &mut map);
        assert_eq!(map.get("mp4").map(|m| m.1.as_str()), Some("video/mp4"));
        assert_eq!(map.get("tar.gz").map(|m| m.1.as_str()), Some("application/x-compressed-tar"));
        assert_eq!(map.get("log").map(|m| m.1.as_str()), Some("text/x-log"));
        let (t, mimes) = parse_thumbnailer(
            "[Thumbnailer Entry]\nTryExec=ffmpegthumbnailer\nExec=ffmpegthumbnailer -s %s -m -i %i -o %o -c png -f -t 10\nMimeType=video/mp4;video/webm;\n",
        )
        .unwrap();
        assert_eq!(t.try_exec.as_deref(), Some("ffmpegthumbnailer"));
        assert_eq!(mimes, ["video/mp4", "video/webm"]);
        let words = fill(&t.exec, "/home/max/a b.mp4", "/tmp/o.png", 256);
        assert!(words.contains(&"/tmp/o.png".to_owned()) && words.contains(&"256".to_owned()));
    }

    #[test]
    fn a_text_file_is_a_page_and_a_binary_is_not() {
        let d = std::env::temp_dir().join(format!("wr-thumb-more-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (txt, bin) = (d.join("notes.txt"), d.join("blob.bin"));
        std::fs::write(&txt, "Hello\nworld\n").unwrap();
        std::fs::write(&bin, [0u8, 1, 2, 3, 0, 255]).unwrap();
        assert!(text_head(bin.to_str().unwrap()).is_none());
        assert!(text_head(txt.to_str().unwrap()).is_some());
        // (Drawn where there are fonts; none where the tests run bare.)
        if let Some(px) = text_page(txt.to_str().unwrap()) {
            assert_eq!(px.len(), crate::apps::with_mips(vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize]).len());
        }
        std::fs::remove_dir_all(&d).ok();
    }
}

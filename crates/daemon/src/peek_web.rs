//! QUICK LOOK on a WEB PAGE (Max, 2026-10-17: "an html i think should be
//! seen as a website, not text, and offer the pencil to edit as text"): the
//! page as Seam shows it, its first screen — taken by Seam's own engine
//! (Firefox) HEADLESS, in a profile of Quick Look's own, the network cut
//! (no name is ever looked up: a page read here can call nobody), its
//! scripts run. Kept in the cache by the file's size and time, so a second
//! look is a picture read from disk. The pencil writes its source.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A page is drawn this wide (CSS pixels, one to a pixel) and WHOLE, as
/// tall as it is (Max, 2026-10-17: "una captura de la página completa
/// scrolleable"), up to `MAX_H`; kept raw, so the part on screen is read
/// straight out of it.
const WIDTH: u32 = 1280;
const WIDTH_DOC: u32 = 900;
const DENSITY: &str = "1.0";
const MAX_H: u32 = 16_000;
/// What Quick Look holds of a page at once: a run `CROP` tall starting
/// every `STEP` (`k` = which), always more than the frame shows — the next
/// is read before the frame reaches its end (within the tiles at 1280 wide).
pub(crate) const STEP: u32 = 500;
pub(crate) const CROP: u32 = 2500;

/// The part of a long page a picture is: the page's size, and where in it
/// this run starts and how tall it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Long {
    pub w: u32,
    pub h: u32,
    pub y: u32,
    pub ch: u32,
}
/// The longest a page may take (a page that never settles is let go).
const PATIENCE: Duration = Duration::from_secs(12);

/// Whether a file of this name is a web page.
pub(crate) fn is_web(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".html", ".htm", ".xhtml"]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// The engine: the Firefox Seam's launcher runs (its profile is Seam's
/// own, so the launcher itself is not called), else any `firefox`.
fn engine() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let seam = dir.join("seam");
        if let Ok(script) = std::fs::read_to_string(&seam) {
            // (`exec "/nix/store/…-seam-158.0/bin/firefox" --name seam …`)
            if let Some(at) = script.find("/bin/firefox\"") {
                let start = script[..at].rfind('"').map_or(0, |i| i + 1);
                let bin = PathBuf::from(&script[start..at + "/bin/firefox".len()]);
                if bin.is_file() {
                    return Some(bin);
                }
            }
        }
    }
    crate::launch::on_path("firefox").then(|| PathBuf::from("firefox"))
}

/// Quick Look's own profile: no network, nothing asked at start.
fn profile() -> Option<PathBuf> {
    let dir = crate::apps::cache_base().join("waverunner/web-profile");
    std::fs::create_dir_all(&dir).ok()?;
    let prefs = [
        ("network.dns.disabled", "true"),
        ("network.proxy.type", "1"),
        ("network.proxy.socks", "\"127.0.0.1\""),
        ("network.proxy.socks_port", "9"),
        ("network.proxy.allow_hijacking_localhost", "true"),
        ("layout.css.devPixelsPerPx", &format!("\"{DENSITY}\"")),
        ("browser.shell.checkDefaultBrowser", "false"),
        ("datareporting.policy.dataSubmissionEnabled", "false"),
        ("toolkit.telemetry.enabled", "false"),
        ("app.update.enabled", "false"),
        ("browser.startup.page", "0"),
        ("browser.aboutwelcome.enabled", "false"),
        ("media.autoplay.default", "5"),
    ];
    let text: String = prefs
        .iter()
        .map(|(k, v)| format!("user_pref(\"{k}\", {v});\n"))
        .collect();
    std::fs::write(dir.join("user.js"), text).ok()?;
    Some(dir)
}

/// Where the picture of `path` as it is now is kept (`salt`: what else it
/// depends on — the colours a document is drawn in).
fn kept(path: &Path, salt: &str) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let changed = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    // FNV-1a: a name for a cache file, not a secret.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path
        .to_string_lossy()
        .bytes()
        .chain(meta.len().to_le_bytes())
        .chain(changed.to_le_bytes())
        .chain(salt.bytes())
    {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Some(crate::apps::cache_base().join(format!("waverunner/web/{h:016x}.rgba")))
}

/// What Quick Look shows of `path` drawn as a page — its run `k` — and the
/// words for its head: a web page as it is; Markdown formatted; a table
/// (CSV, TSV) as a table. `None`: not one of these, or it could not be drawn.
pub(crate) fn drawn(path: &Path, name: &str, k: u32) -> Option<(image::RgbaImage, Long, String)> {
    use crate::i18n::{fill, tr};
    let lower = name.to_ascii_lowercase();
    if is_web(&lower) {
        let out = kept(path, "")?;
        shoot(path, &out, WIDTH)?;
        let (img, long) = crop(&out, k)?;
        return Some((img, long, tr("as a web page").to_owned()));
    }
    if is_markdown(&lower) {
        let style = style();
        let out = kept(path, &style)?;
        made(&out, WIDTH_DOC, || markdown(path, &style))?;
        let (img, long) = crop(&out, k)?;
        return Some((img, long, tr("formatted").to_owned()));
    }
    if is_table(&lower) {
        let style = style();
        let out = kept(path, &style)?;
        made(&out, WIDTH, || table(path, &lower, &style).map(|t| t.0))?;
        let (img, long) = crop(&out, k)?;
        let rows = std::fs::read_to_string(path).map_or(0, |t| {
            parse_rows(&t, sep_of(&t, &lower)).len().saturating_sub(1)
        });
        let words = match rows {
            0 => tr("as a table").to_owned(),
            n => fill(tr("{n} rows"), &[("n", &n)]),
        };
        return Some((img, long, words));
    }
    None
}

/// Run `k` of the page kept raw at `out` (8 bytes of size, then its rows).
fn crop(out: &Path, k: u32) -> Option<(image::RgbaImage, Long)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(out).ok()?;
    let mut head = [0u8; 8];
    f.read_exact(&mut head).ok()?;
    let w = u32::from_le_bytes(head[..4].try_into().ok()?);
    let h = u32::from_le_bytes(head[4..].try_into().ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    let y = (k * STEP).min(h - 1);
    let ch = CROP.min(h - y);
    f.seek(SeekFrom::Start(8 + u64::from(y) * u64::from(w) * 4))
        .ok()?;
    let mut px = vec![0u8; (w * ch * 4) as usize];
    f.read_exact(&mut px).ok()?;
    Some((image::RgbaImage::from_raw(w, ch, px)?, Long { w, h, y, ch }))
}

/// Whether a file of this name is Markdown.
pub(crate) fn is_markdown(lower: &str) -> bool {
    [".md", ".markdown", ".mkd"]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// Whether a file of this name is a table of values.
pub(crate) fn is_table(lower: &str) -> bool {
    [".csv", ".tsv"].iter().any(|e| lower.ends_with(e))
}

/// The page `html` makes, drawn (or read from where it was kept as `out`).
fn made(out: &Path, width: u32, html: impl FnOnce() -> Option<String>) -> Option<()> {
    if out.is_file() {
        return Some(());
    }
    let page = out.with_extension(format!("{}.html", crate::peek_edit::nonce()));
    std::fs::create_dir_all(out.parent()?).ok()?;
    std::fs::write(&page, html()?).ok()?;
    let done = shoot(&page, out, width);
    let _ = std::fs::remove_file(&page);
    done
}

/// The page in the file `page`, drawn `view` big, kept as `out`.
fn shoot(page: &Path, out: &Path, width: u32) -> Option<()> {
    if out.is_file() {
        return Some(());
    }
    let engine = engine()?;
    let profile = profile()?;
    std::fs::create_dir_all(out.parent()?).ok()?;
    let tmp = out.with_extension(format!("{}.png", crate::peek_edit::nonce()));
    let url = format!("file://{}", page.display());
    let mut child = crate::peek_edit::dies_with_us(Command::new(engine))
        .args(["--headless", "--no-remote", "--profile"])
        .arg(&profile)
        .args([
            // (A width alone: the page is taken whole, as tall as it is.)
            "--window-size",
            &width.to_string(),
            "--screenshot",
        ])
        .arg(&tmp)
        .arg(url)
        .env("MOZ_CRASHREPORTER_DISABLE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let done = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() < PATIENCE => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                tracing::warn!("quick look: the page {} took too long", page.display());
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let img = done.then(|| image::open(&tmp).ok()).flatten();
    let _ = std::fs::remove_file(&tmp);
    let img = img?.to_rgba8();
    // Kept raw (a run is read out of it in a moment), cut at `MAX_H`.
    let (w, h) = (img.width(), img.height().min(MAX_H));
    let mut raw = Vec::with_capacity(8 + (w * h * 4) as usize);
    raw.extend_from_slice(&w.to_le_bytes());
    raw.extend_from_slice(&h.to_le_bytes());
    raw.extend_from_slice(&img.as_raw()[..(w * h * 4) as usize]);
    let part = out.with_extension(format!("{}.part", crate::peek_edit::nonce()));
    std::fs::write(&part, raw).ok()?;
    std::fs::rename(&part, out).ok()?;
    forget_old(out.parent()?);
    Some(())
}

/// The kept pages beyond the newest `KEEP` are let go (they are big).
fn forget_old(dir: &Path) {
    const KEEP: usize = 40;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kept: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if kept.len() <= KEEP {
        return;
    }
    kept.sort();
    for (_, p) in &kept[..kept.len() - KEEP] {
        let _ = std::fs::remove_file(p);
    }
}

/// The stylesheet documents are drawn with: the palette's own colours, in
/// the look in use (so a document reads like the rest of Golem).
fn style() -> String {
    use crate::palette::hex;
    let t = crate::palette::current();
    let c = |i: usize| format!("#{}", hex(t.c[i]));
    let accent = format!("#{}", hex(t.accent()));
    let (bg, ink, head, dim, rule, code, zebra) = (c(0), c(5), c(6), c(4), c(2), c(1), c(1));
    format!(
        "html{{background:{bg};color:{ink};}}\
         body{{font:16px/1.6 'DejaVu Sans',sans-serif;margin:0;padding:40px 56px;}}\
         h1,h2,h3,h4{{color:{head};line-height:1.25;margin:1.2em 0 .5em;}}\
         h1{{font-size:2em;border-bottom:1px solid {rule};padding-bottom:.3em;margin-top:0}}\
         h2{{font-size:1.5em;border-bottom:1px solid {rule};padding-bottom:.2em}}\
         a{{color:{accent};}}\
         code,pre{{font-family:'JetBrainsMono Nerd Font Mono',monospace;font-size:.88em;background:{code};border-radius:6px;}}\
         code{{padding:.15em .4em}} pre{{padding:14px 16px;overflow:hidden}} pre code{{padding:0;background:none}}\
         blockquote{{margin:0;padding:0 1em;border-left:4px solid {rule};color:{dim};}}\
         img{{max-width:100%}} hr{{border:0;border-top:1px solid {rule}}}\
         table{{border-collapse:collapse;font-size:14px}} th,td{{border:1px solid {rule};padding:6px 12px;text-align:left}} table.values td,table.values th{{white-space:nowrap}}\
         th{{background:{code};color:{head}}} tr:nth-child(even) td{{background:{zebra}}}\
         td.n{{text-align:right;font-variant-numeric:tabular-nums}}",
    )
}

/// Markdown as a page (its pictures found beside it).
fn markdown(path: &Path, style: &str) -> Option<String> {
    use pulldown_cmark::{html, Options, Parser};
    let mut text = String::new();
    use std::io::Read;
    std::fs::File::open(path)
        .ok()?
        .take(2 << 20)
        .read_to_string(&mut text)
        .ok()?;
    let mut body = String::new();
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    html::push_html(&mut body, Parser::new_ext(&text, options));
    Some(page_of(path, style, &body))
}

/// A table of values (comma, semicolon or tab between them) as a table, its
/// first row the head; its first `ROWS` rows, and how many there are.
fn table(path: &Path, lower: &str, style: &str) -> Option<(String, usize)> {
    const ROWS: usize = 300;
    let text = std::fs::read_to_string(path).ok()?;
    let rows = parse_rows(&text, sep_of(&text, lower));
    let n = rows.len().saturating_sub(1);
    let mut body = String::from("<table class=values>");
    for (i, row) in rows.iter().take(ROWS + 1).enumerate() {
        body.push_str("<tr>");
        for cell in row {
            let tag = if i == 0 { "th" } else { "td" };
            let num = i > 0 && !cell.is_empty() && cell.trim().parse::<f64>().is_ok();
            body.push_str(&format!(
                "<{tag}{}>{}</{tag}>",
                if num { " class=n" } else { "" },
                escape(cell)
            ));
        }
        body.push_str("</tr>");
    }
    body.push_str("</table>");
    Some((page_of(path, style, &body), n))
}

/// What stands between a table's values: a tab in a `.tsv`, else the one
/// its first line has most of (a comma, a semicolon, a tab).
fn sep_of(text: &str, lower: &str) -> char {
    if lower.ends_with(".tsv") {
        return '\t';
    }
    let first = text.lines().next().unwrap_or("");
    [',', ';', '\t']
        .into_iter()
        .max_by_key(|c| first.matches(*c).count())
        .unwrap_or(',')
}

/// Rows of cells: `sep` between them, quotes around a cell that holds one
/// (a doubled quote is a quote), a line end inside quotes kept.
fn parse_rows(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == sep && !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The whole page around `body`: its colours, and where its own pictures
/// and links are looked for (the document's folder).
fn page_of(path: &Path, style: &str, body: &str) -> String {
    let base = path
        .parent()
        .map(|d| format!("file://{}/", d.display()))
        .unwrap_or_default();
    format!(
        "<!doctype html><html><head><meta charset=utf-8><base href=\"{}\"><style>{style}</style></head><body>{body}</body></html>",
        escape(&base)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_pages_are_known_by_their_endings() {
        assert!(is_web("index.HTML"));
        assert!(is_web("a.htm"));
        assert!(!is_web("notes.md"));
    }

    #[test]
    fn a_table_reads_quotes_and_line_ends() {
        let rows = parse_rows(
            "a,b\n\"x, y\",\"say \"\"hi\"\"\"\r\n1,\"two\nlines\"\n",
            ',',
        );
        assert_eq!(
            rows,
            vec![
                vec!["a".to_owned(), "b".to_owned()],
                vec!["x, y".to_owned(), "say \"hi\"".to_owned()],
                vec!["1".to_owned(), "two\nlines".to_owned()],
            ]
        );
    }

    #[test]
    fn a_run_is_read_out_of_the_kept_page() {
        let dir = std::env::temp_dir().join(format!("peek-web-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("p.rgba");
        // A page 2 wide and STEP + 3 tall, each row its own number.
        let h = STEP + 3;
        let mut raw = Vec::new();
        raw.extend_from_slice(&2u32.to_le_bytes());
        raw.extend_from_slice(&h.to_le_bytes());
        for y in 0..h {
            raw.extend_from_slice(&[(y % 256) as u8; 8]);
        }
        std::fs::write(&out, raw).unwrap();
        let (img, long) = crop(&out, 1).unwrap();
        assert_eq!(
            long,
            Long {
                w: 2,
                h,
                y: STEP,
                ch: 3
            }
        );
        assert_eq!(img.dimensions(), (2, 3));
        assert_eq!(img.as_raw()[0], (STEP % 256) as u8);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn markdown_and_tables_are_known() {
        assert!(is_markdown("readme.md"));
        assert!(is_table("data.csv"));
        assert!(!is_table("data.json"));
    }
}

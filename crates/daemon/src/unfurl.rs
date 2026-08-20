//! Opt-in network link unfurl (the clipboard link "share card").
//!
//! When `[options] link_unfurl = true`, a copied link is enriched with the
//! page's own metadata: an `og:title`/`og:description`/`og:image` scrape, with
//! an **oEmbed** fast-path for known providers (YouTube), so e.g. a YouTube
//! link gets its real title + the official thumbnail rather than a screenshot.
//!
//! Kept off by default and on its own worker thread: enabling it makes the
//! daemon issue an outbound request per copied URL, and a slow fetch must never
//! block the render loop. We shell out to `curl` (as the clipboard already does
//! for `wl-paste`/`grim`), so there's no in-process TLS/HTTP stack to carry.

use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc;

use calloop::channel::Sender;
use tracing::{debug, warn};

/// Max HTML we parse for meta tags — they live in `<head>`, near the top.
const MAX_HTML: usize = 256 * 1024;
/// Per-request network budget for `curl` (seconds).
const TIMEOUT_SECS: &str = "6";
/// A browser-ish UA — some sites serve no OpenGraph tags to unknown agents.
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// A request to unfurl one link clip.
pub struct Request {
    /// The clip's stable id, echoed back so the loop can find the entry.
    pub id: u64,
    /// The URL to unfurl.
    pub url: String,
}

/// The result of an unfurl, applied to the clip with the matching `id`.
pub enum Event {
    /// Metadata resolved. Any field may be empty/`None` (partial success); the
    /// loop only overwrites what's present.
    Done {
        id: u64,
        title: String,
        description: String,
        image_path: Option<PathBuf>,
    },
}

/// Handle to the unfurl thread.
pub struct Unfurl {
    requests: mpsc::Sender<Request>,
}

impl Unfurl {
    /// Queue an unfurl for clip `id`'s `url` (dedup is the caller's business).
    pub fn request(&self, id: u64, url: &str) {
        let _ = self.requests.send(Request {
            id,
            url: url.to_owned(),
        });
    }
}

/// Spawn the unfurl worker. It exits when either channel closes.
pub fn spawn(results: Sender<Event>) -> Unfurl {
    let (requests, rx) = mpsc::channel::<Request>();
    let spawned = std::thread::Builder::new()
        .name("waverunner-unfurl".into())
        .spawn(move || {
            while let Ok(req) = rx.recv() {
                let Some(meta) = unfurl(&req.id, &req.url) else {
                    debug!("unfurl: nothing resolved for {}", req.url);
                    continue;
                };
                let event = Event::Done {
                    id: req.id,
                    title: meta.title,
                    description: meta.description,
                    image_path: meta.image_path,
                };
                if results.send(event).is_err() {
                    return; // event loop is gone
                }
            }
        });
    if let Err(e) = spawned {
        warn!("cannot spawn unfurl thread: {e}");
    }
    Unfurl { requests }
}

/// Resolved metadata for a link (before it's turned into an [`Event`]).
struct Resolved {
    title: String,
    description: String,
    image_path: Option<PathBuf>,
}

/// Unfurl one URL: try the oEmbed fast-path for known providers, then fall back
/// to an OpenGraph scrape. Downloads the chosen image into a side file. Returns
/// `None` only if nothing at all resolved.
fn unfurl(id: &u64, url: &str) -> Option<Resolved> {
    // oEmbed fast-path (clean title + official thumbnail, no HTML scraping).
    let (mut title, mut description, mut image_url) = youtube_oembed(url)
        .map(|(t, img)| (t, String::new(), Some(img)))
        .unwrap_or_default();

    // OpenGraph scrape fills whatever the fast-path didn't.
    if title.is_empty() || description.is_empty() || image_url.is_none() {
        if let Some(html) = fetch(url, &[]) {
            let m = extract_meta(&html);
            if title.is_empty() {
                title = m.title;
            }
            if description.is_empty() {
                description = m.description;
            }
            if image_url.is_none() {
                image_url = m.image.and_then(|i| absolutize(url, &i));
            }
        }
    }

    let image_path = image_url.and_then(|u| download_image(*id, &u));
    if title.is_empty() && description.is_empty() && image_path.is_none() {
        return None;
    }
    Some(Resolved {
        title,
        description,
        image_path,
    })
}

/// YouTube's oEmbed endpoint returns clean JSON (`title`, `thumbnail_url`) for a
/// watch/short/`youtu.be` link — no scraping, no API key.
fn youtube_oembed(url: &str) -> Option<(String, String)> {
    let host = url_host(url)?;
    if !(host.ends_with("youtube.com") || host == "youtu.be") {
        return None;
    }
    let endpoint = format!(
        "https://www.youtube.com/oembed?url={}&format=json",
        percent_encode(url)
    );
    let body = fetch(&endpoint, &[])?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let title = json["title"].as_str().unwrap_or("").to_owned();
    let thumb = json["thumbnail_url"].as_str().unwrap_or("").to_owned();
    if title.is_empty() && thumb.is_empty() {
        return None;
    }
    Some((title, thumb))
}

/// `curl` a URL and return its body (capped at [`MAX_HTML`]). `None` on any
/// failure (curl absent, network error, non-2xx). Extra args go before the URL.
fn fetch(url: &str, extra: &[&str]) -> Option<String> {
    let out = Command::new("curl")
        .args(["-sL", "--max-time", TIMEOUT_SECS, "-A", USER_AGENT, "--fail"])
        .args(extra)
        .arg(url)
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    let mut body = out.stdout;
    body.truncate(MAX_HTML);
    Some(String::from_utf8_lossy(&body).into_owned())
}

/// Download an image to a side file via `curl`. Returns its path on success.
fn download_image(id: u64, image_url: &str) -> Option<PathBuf> {
    let ext = image_ext(image_url);
    let path = crate::persist::data_path(&format!("clipboard-previews/unfurl-{id:016x}.{ext}"));
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return None;
        }
    }
    let ok = Command::new("curl")
        .args(["-sL", "--max-time", TIMEOUT_SECS, "-A", USER_AGENT, "--fail", "-o"])
        .arg(&path)
        .arg(image_url)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    // Guard against an empty/tiny file (a stray error page saved as the image).
    let big_enough = std::fs::metadata(&path).map(|m| m.len() > 256).unwrap_or(false);
    if ok && big_enough {
        Some(path)
    } else {
        let _ = std::fs::remove_file(&path);
        None
    }
}

/// Extracted page metadata (any field may be empty).
#[derive(Default, PartialEq, Debug)]
struct Meta {
    title: String,
    description: String,
    image: Option<String>,
}

/// Pull `og:`/`twitter:` title/description/image (and the `<title>` fallback)
/// out of a page's HTML. Deliberately small: it scans complete `<meta …>` tags
/// rather than parsing the whole document, which is enough for the standardised
/// social-card tags that live in `<head>`.
fn extract_meta(html: &str) -> Meta {
    let mut meta = Meta::default();
    let bytes = html.as_bytes();
    let mut i = 0;
    while let Some(rel) = html[i..].find("<meta") {
        let start = i + rel;
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b'>')
            .map(|p| start + p)
            .unwrap_or(html.len());
        let tag = &html[start..end];
        let key = attr(tag, "property").or_else(|| attr(tag, "name"));
        if let (Some(key), Some(content)) = (key, attr(tag, "content")) {
            let content = decode_entities(content.trim());
            match key.to_ascii_lowercase().as_str() {
                "og:title" | "twitter:title" if meta.title.is_empty() => meta.title = content,
                "og:description" | "twitter:description" | "description"
                    if meta.description.is_empty() =>
                {
                    meta.description = content
                }
                "og:image" | "og:image:url" | "twitter:image" if meta.image.is_none() => {
                    meta.image = Some(content)
                }
                _ => {}
            }
        }
        i = end;
    }
    if meta.title.is_empty() {
        if let (Some(a), Some(b)) = (html.find("<title"), html.find("</title>")) {
            if let Some(gt) = html[a..b].find('>') {
                let inner = html[a + gt + 1..b].trim();
                if !inner.is_empty() {
                    meta.title = decode_entities(inner);
                }
            }
        }
    }
    meta
}

/// The value of `name="…"` / `name='…'` in an HTML tag (double or single
/// quotes), if present.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(rel) = tag[from..].find(name) {
        let at = from + rel;
        // Require a word boundary before the name (so `name` doesn't match
        // inside `itemname`) and `=` (maybe spaced) after it.
        let before_ok = at == 0 || !tag.as_bytes()[at - 1].is_ascii_alphanumeric();
        let rest = tag[at + name.len()..].trim_start();
        if before_ok {
            if let Some(eq) = rest.strip_prefix('=') {
                let v = eq.trim_start();
                let (quote, body) = match v.as_bytes().first() {
                    Some(b'"') => ('"', &v[1..]),
                    Some(b'\'') => ('\'', &v[1..]),
                    _ => {
                        from = at + name.len();
                        continue;
                    }
                };
                return body.find(quote).map(|q| &body[..q]);
            }
        }
        from = at + name.len();
    }
    None
}

/// Minimal HTML entity decode for the handful that show up in titles.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// Resolve a possibly-relative image URL against the page URL.
fn absolutize(page: &str, img: &str) -> Option<String> {
    if img.starts_with("http://") || img.starts_with("https://") {
        Some(img.to_owned())
    } else if let Some(rest) = img.strip_prefix("//") {
        let scheme = page.split(':').next().unwrap_or("https");
        Some(format!("{scheme}://{rest}"))
    } else if img.starts_with('/') {
        let host = url_host(page)?;
        let scheme = page.split(':').next().unwrap_or("https");
        Some(format!("{scheme}://{host}{img}"))
    } else {
        None // a bare relative path — rare for og:image, skip
    }
}

/// The host portion of an http(s) URL.
fn url_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    Some(rest.split(['/', '?', '#']).next().unwrap_or("").to_ascii_lowercase())
}

/// A safe image file extension guessed from the URL path (default `jpg`).
fn image_ext(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    if path.ends_with(".png") {
        "png"
    } else if path.ends_with(".webp") {
        "webp"
    } else if path.ends_with(".gif") {
        "gif"
    } else {
        "jpg"
    }
}

/// Percent-encode a string for use as a URL query value (encode everything but
/// the RFC 3986 unreserved set).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_open_graph_tags() {
        let html = r#"<html><head>
            <title>Fallback - Site</title>
            <meta property="og:title" content="Never Gonna Give You Up">
            <meta name="twitter:image" content="https://i.ytimg.com/vi/x/hq.jpg">
            <meta property="og:description" content="Rick Astley &amp; friends">
        </head></html>"#;
        let m = extract_meta(html);
        assert_eq!(m.title, "Never Gonna Give You Up");
        assert_eq!(m.description, "Rick Astley & friends");
        assert_eq!(m.image.as_deref(), Some("https://i.ytimg.com/vi/x/hq.jpg"));
    }

    #[test]
    fn falls_back_to_title_tag() {
        let m = extract_meta("<head><title>Just a Page</title></head>");
        assert_eq!(m.title, "Just a Page");
        assert!(m.image.is_none());
    }

    #[test]
    fn attr_needs_word_boundary_and_handles_quotes() {
        assert_eq!(attr(r#"<meta name='og:image' content="x">"#, "name"), Some("og:image"));
        // `itemname` must not satisfy a request for `name`.
        assert_eq!(attr(r#"<meta itemname="no">"#, "name"), None);
    }

    #[test]
    fn absolutize_resolves_relative_forms() {
        let page = "https://example.com/watch?v=1";
        assert_eq!(
            absolutize(page, "//cdn.example.com/a.jpg").as_deref(),
            Some("https://cdn.example.com/a.jpg")
        );
        assert_eq!(
            absolutize(page, "/img/a.jpg").as_deref(),
            Some("https://example.com/img/a.jpg")
        );
        assert_eq!(absolutize(page, "a.jpg"), None);
    }

    #[test]
    fn encodes_url_query_value() {
        assert_eq!(
            percent_encode("https://youtu.be/a?b=c"),
            "https%3A%2F%2Fyoutu.be%2Fa%3Fb%3Dc"
        );
    }
}

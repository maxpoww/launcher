//! Layer 2 (part) — clipboard selection.
//!
//! What you've copied is strong intent: a URL you want to open, a snippet you
//! want to run or explain. This collector watches the Wayland clipboard and
//! reports a *classified* [`TextSelection`] (length, looks-like-code,
//! contains-URL) plus a length-bounded snippet.
//!
//! Implementation: a single long-lived `wl-paste --watch` subprocess (the
//! canonical wlr-data-control client, as used by cliphist et al.), with each
//! change delimited by a NUL so the stream parses cleanly. This is robust and
//! upgradeable to a native `zwlr_data_control` client later.
//!
//! Privacy: the clipboard can hold anything, so the stored snippet is capped
//! (`SNIPPET_CAP`) and this is clipboard only — not the primary/highlight
//! selection, which is far higher-frequency and more sensitive.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, Layer, TextSelection};

/// Max characters of clipboard text retained in the snapshot.
const SNIPPET_CAP: usize = 512;
const RESPAWN: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct SelectionCollector;

impl SelectionCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for SelectionCollector {
    fn name(&self) -> &'static str {
        "selection"
    }
    fn layer(&self) -> Layer {
        Layer::Selection
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            loop {
                match watch_clipboard(&tx).await {
                    Ok(()) => tracing::debug!("selection: wl-paste ended; respawning"),
                    Err(e) => tracing::debug!("selection: {e}"),
                }
                let _ = tx.send(Update::Health(Layer::Selection, false)).await;
                tokio::time::sleep(RESPAWN).await;
            }
        })
    }
}

/// Spawn `wl-paste --watch` and stream NUL-delimited clipboard contents until
/// it exits.
async fn watch_clipboard(tx: &mpsc::Sender<Update>) -> anyhow::Result<()> {
    // The helper prints the piped clipboard content then a NUL terminator, so
    // each change is one framed record on our stdout.
    let mut child = Command::new("wl-paste")
        .args(["--watch", "sh", "-c", "cat; printf '\\0'"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("no stdout"))?;
    tx.send(Update::Health(Layer::Selection, true)).await?;

    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    let mut last = String::new();
    loop {
        buf.clear();
        let n = reader.read_until(0, &mut buf).await?;
        if n == 0 {
            break; // wl-paste exited
        }
        if buf.last() == Some(&0) {
            buf.pop();
        }
        let content = String::from_utf8_lossy(&buf).into_owned();
        if content == last {
            continue;
        }
        last = content.clone();
        if tx
            .send(Update::Delta(
                Layer::Selection,
                ContextDelta::Selection(classify(&content)),
            ))
            .await
            .is_err()
        {
            return Ok(()); // aggregator gone
        }
    }
    let _ = child.kill().await;
    Ok(())
}

/// Classify clipboard text into a [`TextSelection`] with a bounded snippet.
fn classify(content: &str) -> TextSelection {
    if content.trim().is_empty() {
        return TextSelection::default();
    }
    let char_count = content.chars().count();
    let snippet: String = content.chars().take(SNIPPET_CAP).collect();
    // Classify from the bounded snippet, not the whole clipboard: a multi-MB
    // paste must not trigger a dozen full-content substring scans on every
    // clipboard change. The first SNIPPET_CAP chars are representative for the
    // heuristics (a path is < SNIPPET_CAP chars anyway; code/URL intent shows
    // early). `looks_like_path` short-circuits on length first, so it also stays
    // O(1) on huge input.
    let is_code = looks_like_code(&snippet);
    let contains_url = contains_url(&snippet);
    let is_path = looks_like_path(&snippet);
    let is_git_sha = looks_like_git_sha(snippet.trim());
    let (multi_path_count, multi_path_dir) =
        match classify_multi_paths(&snippet, char_count > SNIPPET_CAP) {
            Some((n, dir)) => (n, Some(dir)),
            None => (0, None),
        };
    TextSelection {
        highlighted_text: Some(snippet),
        char_count,
        is_code,
        contains_url,
        is_path,
        is_git_sha,
        multi_path_count,
        multi_path_dir,
    }
}

/// Detect a file-manager multi-file Copy: EVERY non-empty line of the snippet
/// is an absolute path or a `file://` URI, and there are at least two. Mixed
/// content (a path pasted inside prose) is not a multi-copy. Returns the count
/// seen and the files' deepest common parent folder (decoded plain path).
/// When the snippet was truncated the last line may be a cut-off path — it is
/// dropped before judging, so a partial line can neither fail nor corrupt the
/// classification.
fn classify_multi_paths(snippet: &str, truncated: bool) -> Option<(usize, String)> {
    let mut lines: Vec<&str> = snippet
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if truncated {
        lines.pop();
    }
    if lines.len() < 2 {
        return None;
    }
    let mut paths = Vec::with_capacity(lines.len());
    for line in lines {
        let p = if let Some(rest) = line.strip_prefix("file://") {
            percent_decode(rest)
        } else {
            line.to_string()
        };
        if !p.starts_with('/') || p.len() >= 512 {
            return None;
        }
        paths.push(p);
    }
    let mut dir = parent_of(&paths[0]).to_string();
    for p in &paths[1..] {
        dir = common_dir(&dir, parent_of(p));
    }
    Some((paths.len(), dir))
}

/// The parent folder of an absolute path (`/` for a root-level entry).
fn parent_of(p: &str) -> &str {
    match p.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &p[..i],
    }
}

/// Deepest common ancestor of two absolute directories, at component
/// boundaries (so `/a/bc` and `/a/bd` share `/a`, not `/a/b`).
fn common_dir(a: &str, b: &str) -> String {
    let mut out = String::new();
    for (ca, cb) in a.split('/').zip(b.split('/')) {
        if ca != cb {
            break;
        }
        if !ca.is_empty() {
            out.push('/');
            out.push_str(ca);
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// Minimal percent-decoding for `file://` URI paths (`%20` → space, etc.).
/// Malformed escapes pass through literally; non-UTF8 decodes lossily — the
/// worst case is a folder that xdg-open can't find, never a panic.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok());
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Heuristic: a bare git commit hash — 7 to 40 hexadecimal characters and
/// nothing else. (7 is git's default short-hash length; 40 is a full SHA-1.)
fn looks_like_git_sha(t: &str) -> bool {
    (7..=40).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Heuristic: a single-line absolute path (`/…`) or `file:///` URI (what a
/// file manager's single-file Copy puts on the clipboard). Kept pure — no
/// filesystem check (xdg-open simply no-ops on a path that isn't there), so
/// the classifier stays deterministic and testable. Only absolute paths, so
/// the open action needs no `~`/`$HOME` expansion (which shell-quoting would
/// defeat); xdg-open accepts the `file://` form as-is.
fn looks_like_path(t: &str) -> bool {
    let t = t.trim();
    // Length guard first so huge input exits before any scan.
    t.len() < 512
        && (t.starts_with('/') || t.starts_with("file:///"))
        && !t.contains('\n')
        && !contains_url(t)
}

fn contains_url(t: &str) -> bool {
    t.contains("http://") || t.contains("https://") || t.contains("www.")
}

/// Heuristic: several code-ish markers present suggests a code snippet (kept
/// deliberately simple — the mind only needs a hint, not a parser).
fn looks_like_code(t: &str) -> bool {
    const MARKERS: &[&str] = &[
        "{",
        "}",
        ";",
        "()",
        "=>",
        "::",
        "</",
        "def ",
        "fn ",
        "function ",
        "import ",
        "#include",
        "    ",
    ];
    // A bare URL shouldn't read as code even though it has "://" etc.
    if contains_url(t) && t.split_whitespace().count() <= 1 {
        return false;
    }
    MARKERS.iter().filter(|m| t.contains(**m)).count() >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_cleared() {
        assert_eq!(classify("   \n "), TextSelection::default());
    }

    #[test]
    fn detects_urls() {
        let s = classify("see https://developer.mozilla.org/en-US/");
        assert!(s.contains_url);
        assert!(!s.is_code);
    }

    #[test]
    fn detects_code() {
        let s = classify("fn main() {\n    println!(\"hi\");\n}");
        assert!(s.is_code);
        assert!(!s.contains_url);
        assert!(s.char_count > 0);
    }

    #[test]
    fn plain_prose_is_neither() {
        let s = classify("just a normal sentence to copy");
        assert!(!s.is_code);
        assert!(!s.contains_url);
    }

    #[test]
    fn snippet_is_capped() {
        let big = "x".repeat(2000);
        let s = classify(&big);
        assert_eq!(s.char_count, 2000);
        assert_eq!(s.highlighted_text.unwrap().chars().count(), SNIPPET_CAP);
    }

    #[test]
    fn huge_clipboard_is_classified_from_the_bounded_snippet() {
        // A URL that only appears *after* the snippet window is not scanned:
        // classification is bounded to the first SNIPPET_CAP chars regardless of
        // how large the clipboard is (a multi-MB paste stays cheap).
        let mut big = "a ".repeat(SNIPPET_CAP); // >> SNIPPET_CAP chars, no url early
        big.push_str("https://buried.example.com/");
        let s = classify(&big);
        assert_eq!(s.char_count, big.chars().count());
        assert_eq!(s.highlighted_text.unwrap().chars().count(), SNIPPET_CAP);
        assert!(!s.contains_url, "url past the snippet window isn't scanned");
        // A very long path-like blob exits the path check on length alone.
        assert!(!classify(&format!("/{}", "d/".repeat(SNIPPET_CAP))).is_path);
    }

    #[test]
    fn lossy_utf8_replacement_chars_classify_without_panic() {
        // watch_clipboard feeds from_utf8_lossy output; a snippet full of U+FFFD
        // must classify to a harmless plain selection, never panic.
        let lossy = "\u{FFFD}\u{FFFD} some text \u{FFFD}";
        let s = classify(lossy);
        assert!(s.char_count > 0);
        assert!(!s.is_code && !s.contains_url && !s.is_path);
        assert!(s.highlighted_text.is_some());
    }

    #[test]
    fn detects_bare_git_shas() {
        assert!(classify("a1b2c3d").is_git_sha); // 7-char short hash
        assert!(classify("  9f8e7d6c5b4a3021  ").is_git_sha); // trimmed
        assert!(classify(&"a".repeat(40)).is_git_sha); // full SHA-1
        assert!(!classify(&"a".repeat(41)).is_git_sha); // too long
        assert!(!classify("a1b2c3").is_git_sha); // too short (<7)
        assert!(!classify("g1b2c3d").is_git_sha); // non-hex
        assert!(!classify("a1b2c3d e").is_git_sha); // has a space → not bare
    }

    #[test]
    fn detects_absolute_paths() {
        assert!(classify("/home/max/notes/todo.md").is_path);
        assert!(classify("  /etc/hosts \n").is_path);
        assert!(!classify("just prose").is_path);
        assert!(!classify("https://example.com/x").is_path); // a url, not a path
        assert!(!classify("relative/path").is_path); // must be absolute
    }

    #[test]
    fn bare_url_is_not_code() {
        assert!(!looks_like_code("https://example.com/a/b?c=d"));
    }

    #[test]
    fn single_file_uri_is_a_path() {
        // A file manager's single-file Copy puts a file:// URI on the
        // clipboard — that is a path, not web-search fodder.
        assert!(classify("file:///home/max/photo.png").is_path);
        assert!(!classify("file:///a\nfile:///b").is_path); // multi-line → multi_path's
    }

    #[test]
    fn multi_path_detects_a_file_manager_copy() {
        let s = classify("/home/max/pics/a.png\n/home/max/pics/b.png\n");
        assert_eq!(s.multi_path_count, 2);
        assert_eq!(s.multi_path_dir.as_deref(), Some("/home/max/pics"));
        assert!(!s.is_path, "multi-line is not a single path");
    }

    #[test]
    fn multi_path_decodes_file_uris() {
        let s = classify("file:///home/max/My%20Docs/a.odt\nfile:///home/max/My%20Docs/b.odt");
        assert_eq!(s.multi_path_count, 2);
        assert_eq!(s.multi_path_dir.as_deref(), Some("/home/max/My Docs"));
    }

    #[test]
    fn multi_path_common_dir_is_component_bounded() {
        // /a/bc and /a/bd share /a — never the string prefix /a/b.
        let s = classify("/a/bc/x.txt\n/a/bd/y.txt");
        assert_eq!(s.multi_path_dir.as_deref(), Some("/a"));
        // Nothing in common beyond the root.
        let s = classify("/etc/hosts\n/home/max/x");
        assert_eq!(s.multi_path_dir.as_deref(), Some("/"));
    }

    #[test]
    fn multi_path_rejects_mixed_content_and_singles() {
        // A path quoted inside prose is not a multi-copy.
        assert_eq!(classify("see the file\n/etc/hosts").multi_path_count, 0);
        // A single path stays the single-path offer.
        let s = classify("/etc/hosts");
        assert_eq!(s.multi_path_count, 0);
        assert!(s.is_path);
        // Relative lines are not a copy set.
        assert_eq!(classify("a/x\nb/y").multi_path_count, 0);
    }

    #[test]
    fn multi_path_drops_the_truncated_tail_line() {
        // Three paths per snippet window, the last cut mid-path by the cap:
        // the partial line must neither fail the set nor corrupt the dir.
        let mut content = String::new();
        for i in 0..40 {
            content.push_str(&format!("/home/max/pics/photo-{i:04}.png\n"));
        }
        assert!(content.chars().count() > SNIPPET_CAP);
        let s = classify(&content);
        assert!(s.multi_path_count >= 2);
        assert_eq!(s.multi_path_dir.as_deref(), Some("/home/max/pics"));
    }

    #[test]
    fn percent_decode_edges() {
        assert_eq!(percent_decode("no-escapes"), "no-escapes");
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%zz%2"), "%zz%2"); // malformed passes through
        assert_eq!(percent_decode("%C3%A9"), "é"); // multi-byte UTF-8
    }
}

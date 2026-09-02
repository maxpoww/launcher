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
    TextSelection {
        highlighted_text: Some(snippet),
        char_count,
        is_code,
        contains_url,
        is_path,
    }
}

/// Heuristic: a single-line absolute path (`/…`). Kept pure — no filesystem
/// check (xdg-open simply no-ops on a path that isn't there), so the classifier
/// stays deterministic and testable. Only absolute paths, so the open action
/// needs no `~`/`$HOME` expansion (which shell-quoting would defeat).
fn looks_like_path(t: &str) -> bool {
    let t = t.trim();
    // Length guard first so huge input exits before any scan.
    t.len() < 512 && t.starts_with('/') && !t.contains('\n') && !contains_url(t)
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
}

//! Layer 4 (part) — a just-finished download in the Downloads folder.
//!
//! A pure timer-driven sensor: poll `~/Downloads` on an interval and report the
//! newest file whose mtime is within a short recency window — "you just
//! downloaded this", intent to open it. It clears once that file ages out, so
//! the offer is transient (the moment after a download), not a permanent pin.
//!
//! Deliberately ignores partial-download sidecars (`.part`, `.crdownload`,
//! `.tmp`) so it fires when the file is actually done, and hidden files.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, Layer};

const POLL: Duration = Duration::from_secs(3);
/// How recent an mtime must be to count as "just downloaded".
const RECENT: Duration = Duration::from_secs(90);
/// Partial-download extensions to ignore (file not finished yet).
const PARTIAL: &[&str] = &["part", "crdownload", "tmp", "download"];

#[derive(Default)]
pub struct DownloadsCollector;

impl DownloadsCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for DownloadsCollector {
    fn name(&self) -> &'static str {
        "downloads"
    }
    fn layer(&self) -> Layer {
        // Shares the hardware layer's liveness (a local FS poll, always
        // available); its own data recency gates the offer.
        Layer::Hardware
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            let dir = downloads_dir();
            let mut last: Option<Option<PathBuf>> = None;
            loop {
                let newest = dir.as_ref().and_then(|d| newest_recent(d));
                if last.as_ref() != Some(&newest) {
                    last = Some(newest.clone());
                    if tx
                        .send(Update::Delta(
                            Layer::Hardware,
                            ContextDelta::RecentDownload(newest),
                        ))
                        .await
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                tokio::time::sleep(POLL).await;
            }
        })
    }
}

/// `$XDG_DOWNLOAD_DIR`, else `~/Downloads`, if it exists.
fn downloads_dir() -> Option<PathBuf> {
    let d = std::env::var_os("XDG_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")))?;
    d.is_dir().then_some(d)
}

/// The newest regular file in `dir` whose mtime is within [`RECENT`], skipping
/// partial-download sidecars and hidden files.
fn newest_recent(dir: &std::path::Path) -> Option<PathBuf> {
    let now = SystemTime::now();
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if PARTIAL.iter().any(|p| ext.eq_ignore_ascii_case(p)) {
                continue;
            }
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(mtime) = meta.modified() else { continue };
        if now.duration_since(mtime).map(|d| d > RECENT).unwrap_or(true) {
            continue; // too old (or clock skew)
        }
        if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}

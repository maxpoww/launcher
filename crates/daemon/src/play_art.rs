//! Cover art for the playing box, decoded off the event loop.
//!
//! MPRIS hands over `mpris:artUrl`, and on this machine every player writes a
//! real file and points at it — Chromium into `/tmp`, Firefox into its profile,
//! kdeconnect into its cache (verified 2026-09-12). So the whole job is: read a
//! PNG, square it, premultiply it, and hand back a mip chain the icon array can
//! take.
//!
//! # Why it is not done on the loop
//!
//! Decoding a 130 KB PNG is single-digit milliseconds, and five rows of it is a
//! visible hitch on the machines this distro exists for — the same
//! single-threaded loop that #40 was about. A thread does the work and the
//! result arrives as an event, exactly like the notification icon resolver.
//!
//! # Why it never touches the network
//!
//! A `file://` URL is the only kind the engine keeps (see `Playing::art_url`),
//! and this reads what it is given and nothing else. Art from a browser could
//! have been an `https` URL; fetching one would have been the first network
//! call anywhere in the shell, and method §6 says the architecture is the
//! policy. Non-file schemes never arrive here to be refused.

use std::sync::mpsc;

use calloop::channel::Sender;
use tracing::debug;

use crate::apps::{with_mips, ICON_SIZE};

/// A decoded cover, ready for a texture layer.
pub struct Art {
    /// The `art_url` it came from — the cache key, so a track that comes back
    /// round is not decoded twice.
    pub key: String,
    /// Premultiplied RGBA8 `ICON_SIZE`² plus its mip chain, or `None` if the
    /// file could not be read or decoded (a player can point at art it has
    /// already deleted).
    pub chain: Option<Vec<u8>>,
}

/// The loader's handle. Dropping it ends the thread.
pub struct PlayArt {
    requests: mpsc::Sender<String>,
}

impl PlayArt {
    /// Ask for one cover. Duplicates are the caller's to avoid — `App` keys by
    /// url and tracks what is already in flight.
    pub fn request(&self, url: String) {
        let _ = self.requests.send(url);
    }
}

/// Start the loader thread.
pub fn spawn(results: Sender<Art>) -> PlayArt {
    let (tx, rx) = mpsc::channel::<String>();
    let spawned = std::thread::Builder::new()
        .name("play-art".into())
        .spawn(move || {
            while let Ok(url) = rx.recv() {
                let chain = decode(&url);
                if results.send(Art { key: url, chain }).is_err() {
                    return; // the event loop is gone
                }
            }
        });
    if let Err(e) = spawned {
        debug!("play-art: cannot spawn loader thread: {e}");
    }
    PlayArt { requests: tx }
}

/// `file://…` → a premultiplied, square, mipped RGBA chain.
fn decode(url: &str) -> Option<Vec<u8>> {
    let path = url.strip_prefix("file://")?;
    let img = image::open(path)
        .map_err(|e| debug!("play-art: {path}: {e}"))
        .ok()?;
    // Cover art is square far more often than not, but a video thumbnail is
    // 16:9. Fill the square and crop rather than letterbox: a black-barred
    // thumbnail in a 40px tile reads as a rendering fault, and the middle of a
    // cover is the part worth showing.
    let rgba = img
        .resize_to_fill(ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Triangle)
        .to_rgba8();
    let mut base = rgba.into_raw();
    // The icon pipeline draws premultiplied.
    for px in base.chunks_exact_mut(4) {
        let a = px[3] as u32;
        for c in px.iter_mut().take(3) {
            *c = ((*c as u32 * a) / 255) as u8;
        }
    }
    Some(with_mips(base))
}

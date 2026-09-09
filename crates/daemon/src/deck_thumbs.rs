//! Window thumbnails for the STAGE deck.
//!
//! Wayland forbids a client from reading another window's pixels, so the
//! pictures come from the one process that has them: the compositor. The
//! waveview plugin renders the named window into a small framebuffer —
//! wherever the window is, on-screen or not — and hands the pixels back as a
//! raw RGBA file (see `hypr::capture_deck`).
//!
//! A picture is taken at exactly two moments: every tile when the mode opens,
//! and a task when it *leaves* the stage. Nothing else — a hidden window gets
//! no frame callbacks and stops painting (verified: a playing video, hidden,
//! captured byte-identical five seconds apart), so re-photographing one buys a
//! render for an identical image. The departure frame is also the freshest a
//! tile can honestly show: it is the last thing the user actually saw.
//!
//! ## Why a worker thread
//!
//! A capture is a round trip through the compositor and a read back off the GPU.
//! Doing that on the event loop would stall the whole shell — the dock, the bar,
//! and the very animation this feature is judged on. So it follows the repo's
//! rule for background work: one dedicated thread, results returned over a
//! calloop channel.
//!
//! ## The squash, and the aspect it is squashed *to*
//!
//! The icon pipeline stores square `ICON_SIZE`² layers, but a tile is the
//! screen's aspect. The capture is therefore squashed **non-uniformly** into the
//! square, and drawing it into the tile stretches it back — the two distortions
//! cancel.
//!
//! They only cancel for a window that already has the tile's aspect, which was
//! the bug: a staged window (1.84) came out 15% too wide and a tall half-screen
//! window (0.82) nearly 2:1 wrong. So the tile's aspect is *sent with the
//! request*, and the compositor fits the window inside the square at
//! `source / tile` — landing back at the window's true shape, whatever it is.
//! What is left over is transparent and the tile's dark body shows through.

use std::sync::mpsc;

use calloop::channel::Sender;
use tracing::{debug, warn};

use crate::apps::ICON_SIZE;

/// How long to let the compositor settle before photographing.
///
/// A capture is taken right after a swap, and the swap moves windows between
/// workspaces, changes fullscreen states and re-lays out a workspace. Firing
/// into the middle of that catches a transitional frame — which is what a
/// "glitchy" tile is. Nobody is looking at the tile in this window, so the wait
/// is free.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(450);

/// A capture job.
///
/// A job carries a *set* because the plugin renders every workspace the set
/// touches in one pass — so filling several tiles costs about what filling one
/// does. Single-window jobs are just a set of one.
pub struct Request {
    pub addrs: Vec<String>,
    /// The shape the thumbnail will finally be drawn at. The atlas layer is
    /// square and the tile stretches it, so the capture has to be pre-squashed
    /// by exactly this to come out true — the compositor does that squash when
    /// it takes the picture, which is why it has to be told.
    pub aspect: f32,
}

/// A finished thumbnail: premultiplied RGBA8 `ICON_SIZE`² plus its mip chain,
/// ready for `Renderer::update_icon_layer`.
pub struct Event {
    pub addr: String,
    pub pixels: Vec<u8>,
}

/// Handle to the capture thread.
pub struct DeckThumbs {
    requests: mpsc::Sender<Request>,
}

impl DeckThumbs {
    /// Queue a capture of one window. Dropped silently if the worker is gone —
    /// a missing thumbnail must never be worth a crash.
    pub fn request(&self, addr: String, aspect: f32) {
        self.request_many(vec![addr], aspect);
    }

    /// Queue a capture of a whole set — one workspace pass for all of them.
    pub fn request_many(&self, addrs: Vec<String>, aspect: f32) {
        if addrs.is_empty() {
            return;
        }
        let _ = self.requests.send(Request { addrs, aspect });
    }
}

/// Spawn the capture thread. It exits when either channel closes.
pub fn spawn(results: Sender<Event>) -> DeckThumbs {
    let (requests, rx) = mpsc::channel::<Request>();
    let spawned = std::thread::Builder::new()
        .name("waverunner-deck-thumbs".into())
        .spawn(move || {
            while let Ok(req) = rx.recv() {
                // Let the swap finish before photographing. Waiting here rather
                // than on a timer keeps the delay off the event loop entirely.
                std::thread::sleep(SETTLE);
                for (addr, pixels) in capture(&req.addrs, req.aspect) {
                    if results.send(Event { addr, pixels }).is_err() {
                        return; // event loop is gone
                    }
                }
            }
        });
    if let Err(e) = spawned {
        warn!("cannot spawn deck thumbnail thread: {e}");
    }
    DeckThumbs { requests }
}

/// Photograph a set of windows through waveview and turn each into an
/// icon-array layer.
///
/// Best effort throughout: without the plugin the deck simply keeps title-only
/// tiles, which is a degraded look rather than a broken one.
fn capture(addrs: &[String], tile_aspect: f32) -> Vec<(String, Vec<u8>)> {
    // One directory per daemon, emptied as it is read: the plugin writes one
    // file per window and this is the only reader. Under `XDG_RUNTIME_DIR`
    // (0700, tmpfs) rather than /tmp — these are pictures of the user's
    // windows, and /tmp is world-readable.
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("waverunner/deck-{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        return Vec::new();
    }
    crate::hypr::capture_deck(addrs, ICON_SIZE, tile_aspect, &dir);

    let mut out = Vec::new();
    // Logged because every failure mode here is silent: the plugin may be
    // absent, an older build, or unable to reach a window, and each of those
    // just leaves a tile looking stale rather than raising anything.
    debug!("deck: asked for {} thumbnail(s)", addrs.len());
    for addr in addrs {
        let path = dir.join(format!("{addr}.rgba"));
        let raw = std::fs::read(&path).ok();
        let _ = std::fs::remove_file(&path);
        match raw {
            Some(raw) if raw.len() == (ICON_SIZE * ICON_SIZE * 4) as usize => {
                out.push((addr.clone(), to_layer(raw)))
            }
            _ => debug!("deck: no thumbnail for {addr}"),
        }
    }
    debug!("deck: {} of {} landed", out.len(), addrs.len());
    out
}

/// Turn one raw capture into a premultiplied, scrimmed icon-array layer.
fn to_layer(raw: Vec<u8>) -> Vec<u8> {
    // The image arrives already square and already the right size — the
    // compositor did the non-uniform squash on the GPU as part of the blit, so
    // there is nothing to decode and nothing to resize. All that is left is the
    // scrim and the premultiply.
    //
    // The identity scrim is baked in here rather than drawn as a rect over the
    // tile, because the scene draws every icon *above* every rect — a scrim rect
    // would end up underneath the photo. Baking it also means it scales with the
    // image and costs nothing per frame.
    let mut out = vec![0u8; raw.len()];
    for (i, px) in raw.chunks_exact(4).enumerate() {
        let y = i as u32 / ICON_SIZE;
        let dim = scrim_factor(y);
        let a = px[3] as u32;
        let at = i * 4;
        // Dim, then premultiply, in one step per channel.
        let ch = |c: u8| (((c as f32 * dim) as u32 * a) / 255).min(255) as u8;
        out[at] = ch(px[0]);
        out[at + 1] = ch(px[1]);
        out[at + 2] = ch(px[2]);
        out[at + 3] = a as u8;
    }
    crate::apps::with_mips(out)
}

/// Brightness multiplier for row `y`: 1.0 over most of the image, ramping down
/// across the bottom band so the tile's title stays readable over any content.
/// The band is the same fraction of the tile that the title strip occupies.
fn scrim_factor(y: u32) -> f32 {
    const BAND_FRAC: f32 = 0.30;
    let t = y as f32 / ICON_SIZE as f32;
    let start = 1.0 - BAND_FRAC;
    if t < start {
        return 1.0;
    }
    let k = (t - start) / BAND_FRAC; // 0 at the band's top, 1 at the bottom
                                     // Ease in so the darkening arrives gently rather than as a hard line.
    1.0 - 0.88 * (k * k)
}

//! Minimized windows on the dock (macOS "minimize to dock").
//!
//! The compositor half lives in the waveview plugin: it takes the window off
//! screen, renders a farewell thumbnail into a raw-RGBA file, and tells us
//! over the control socket (`min-add` / `min-del`). This side only keeps the
//! tiles — a click sends `restore_min` back to the plugin and NOTHING else,
//! because the plugin owns restore: the tile leaves only on its confirming
//! `min-del`, so a restore that fails leaves the window reachable rather
//! than silently lost.
//!
//! Tiles are keyed by the window's pointer ADDRESS, which Hyprland reuses
//! (see the same warning in `hypr.rs`), so the `closewindow` event also
//! evicts — a stale tile would "restore" (and picture) whatever unrelated
//! window later lands on the address.

use tracing::{debug, warn};
use waverunner_core::index::AppEntry;

use crate::apps::{self, ICON_SIZE};
use crate::App;

/// Texture layers reserved for minimized-window thumbnails — a block of its
/// own past the file thumbs (`thumbs::THUMB_CAP`) so the two can never
/// collide: file slots recycle round-robin, and a recycled slot under a tile
/// that sits on the dock indefinitely would repaint it with a stranger's
/// thumbnail. Minimized windows past this cap degrade to letter tiles.
pub const MIN_THUMB_CAP: usize = 16;

/// One minimized window: the backing state of its dock tile.
pub(crate) struct MinimizedWin {
    /// Hyprland window address (`0x…`) — the tile's identity.
    pub addr: String,
    /// Workspace it was minimized from. Informational only (the plugin owns
    /// restore); logged on the restore click so a misdirect can be traced.
    pub ws: i64,
    /// The window's width/height at minimize time — the tile's shape. A wide
    /// window gets a wide tile, a tall one a narrow tile (`content::layout`
    /// clamps the extremes). 1.0 tiles as a square, like a pinned icon.
    pub aspect: f32,
    /// The window's Hyprland class (`?` when unknown) — resolves the corner
    /// app-icon badge the same way `refresh_running` matches windows to apps.
    pub class: String,
    /// Window title at minimize time — the tile's name and dock tooltip.
    pub title: String,
    /// Reserved thumbnail slot, `None` when the thumbnail file was missing
    /// or malformed, or the block is full — the tile shows a letter tile.
    pub slot: Option<usize>,
    /// Texture layer of the badge app icon (the app matching `class`), or
    /// `None` when the class matched no indexed app (no badge, no
    /// placeholder). Resolved by `recompute_dock_order` — both the app set
    /// and its icon layers can shift under a rescan.
    pub badge_layer: Option<u32>,
    /// The uploaded mip chain, kept for re-upload after a rescan rebuilds
    /// the icon texture array (same reason `thumb_map` keeps its pixels).
    pub pixels: Option<Vec<u8>>,
}

/// The transient entry id of a minimized window's dock tile.
pub(crate) fn entry_id(addr: &str) -> String {
    format!("min:{addr}")
}

/// `0x…`-normalized window address — the same discipline the `closewindow`
/// handler applies, so both eviction paths key identically.
fn norm_addr(addr: &str) -> String {
    format!("0x{}", addr.trim().trim_start_matches("0x"))
}

/// Turn one raw capture into a premultiplied icon-array layer.
///
/// The wire format is straight-alpha RGBA, `ICON_SIZE`², the window content
/// FILLING the square (no letterbox padding) — the tile un-stretches it back
/// to the true window shape by drawing it into an aspect-ratio rect. No
/// bottom scrim (unlike `deck_thumbs`): a dock tile draws no title strip over
/// the picture, so there is nothing to keep readable.
fn to_dock_layer(raw: Vec<u8>) -> Vec<u8> {
    let mut out = vec![0u8; raw.len()];
    for (i, px) in raw.chunks_exact(4).enumerate() {
        let a = px[3] as u32;
        let at = i * 4;
        let ch = |c: u8| ((c as u32 * a) / 255) as u8;
        out[at] = ch(px[0]);
        out[at + 1] = ch(px[1]);
        out[at + 2] = ch(px[2]);
        out[at + 3] = px[3];
    }
    apps::with_mips(out)
}

impl App {
    /// First texture layer of the minimized-thumbnail block (right past the
    /// file thumbs — see [`MIN_THUMB_CAP`]).
    pub(crate) fn min_thumb_layer_base(&self) -> u32 {
        self.thumb_layer_base() + crate::thumbs::THUMB_CAP as u32
    }

    /// `min-add <addr> <ws> <aspect> <class> <path> <title…>` — the plugin
    /// minimized a window; grow it a dock tile. A re-add for an address
    /// already shown replaces the tile (fresh title/thumbnail) and re-ranks
    /// it as newest.
    pub(crate) fn on_min_add(&mut self, payload: &str) {
        // Six fields: only the title (last) may carry spaces, so it takes the
        // rest of the line — everything before it is a single token.
        let mut parts = payload.splitn(6, ' ');
        let (Some(addr), Some(ws), Some(aspect), Some(class), Some(path)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            warn!("min-add: malformed payload {payload:?}");
            return;
        };
        let addr = norm_addr(addr);
        let ws = ws.parse::<i64>().unwrap_or(0);
        // The tile shape. A missing/garbage/non-finite ratio falls back to a
        // square; the draw clamps the extremes (`MIN`/`MAX_TILE_ASPECT`).
        let aspect = aspect
            .parse::<f32>()
            .ok()
            .filter(|a| a.is_finite() && *a > 0.0)
            .unwrap_or(1.0);
        // The class resolves the corner badge later (`refresh_min_badges`);
        // `?` (unknown) simply matches no app, so the tile shows no badge.
        let class = class.to_owned();
        // The title is everything after the path (it may contain spaces); a
        // window with none still needs a letter for its placeholder tile.
        let title = parts.next().unwrap_or("").trim();
        let title = if title.is_empty() { "Window" } else { title }.to_owned();
        self.evict_minimized(&addr);
        // Best effort on the picture: a missing or short file (the plugin
        // may have failed its render) costs only the thumbnail — the tile
        // still stands, as a letter tile on the title. The file is the
        // plugin's, so it is read, never deleted.
        let want = (ICON_SIZE * ICON_SIZE * 4) as usize;
        let pixels = match std::fs::read(path) {
            Ok(raw) if raw.len() == want => Some(to_dock_layer(raw)),
            Ok(raw) => {
                debug!(
                    "min-add {addr}: thumbnail {path:?} is {} bytes, want {want}",
                    raw.len()
                );
                None
            }
            Err(e) => {
                debug!("min-add {addr}: no thumbnail at {path:?}: {e}");
                None
            }
        };
        // Lowest free slot in the reserved block. A full block (a 17th
        // simultaneous minimize) degrades that tile to a letter tile — its
        // pixels are dropped too, since nothing ever re-slots them.
        let (slot, pixels) = match pixels {
            Some(p) => {
                let free =
                    (0..MIN_THUMB_CAP).find(|s| !self.minimized.iter().any(|w| w.slot == Some(*s)));
                match free {
                    Some(s) => {
                        let layer = self.min_thumb_layer_base() + s as u32;
                        if let Some(renderer) = self.renderer.as_mut() {
                            renderer.update_icon_layer(layer, &p);
                        }
                        (Some(s), Some(p))
                    }
                    None => (None, None),
                }
            }
            None => (None, None),
        };
        self.minimized.push(MinimizedWin {
            addr,
            ws,
            aspect,
            class,
            title,
            slot,
            // Resolved on the refilter below (`recompute_dock_order` →
            // `refresh_min_badges`), which needs the current app index.
            badge_layer: None,
            pixels,
        });
        // Note the running count. The dock widens to fit these tiles rather
        // than clamp them (`content::layout`); the one ceiling is the
        // surface width, past which — ~a monitor's worth of tiles — the
        // newest are held back. Rare, but this is where to watch it build.
        debug!(
            "min-add: {} minimized window(s) on the dock",
            self.minimized.len()
        );
        // The tile is a transient entry: refilter rebuilds those and the
        // dock order in one move (and schedules the frame).
        self.refilter();
    }

    /// `min-del <addr>` — the window was restored or died; drop its tile.
    /// Unknown addresses are a no-op (the `closewindow` eviction may have
    /// beaten the plugin's message to it).
    pub(crate) fn on_min_del(&mut self, addr: &str) {
        if self.evict_minimized(addr) {
            self.refilter();
        }
    }

    /// Remove `addr`'s tile state, reporting whether there was one. The
    /// caller refilters when true (that is what drops the transient entry);
    /// shared by `min-del` and the `closewindow` eviction in `hypr.rs`.
    pub(crate) fn evict_minimized(&mut self, addr: &str) -> bool {
        let addr = norm_addr(addr);
        let before = self.minimized.len();
        self.minimized.retain(|w| w.addr != addr);
        self.minimized.len() != before
    }

    /// A click on a minimized tile: hand it to the plugin, which owns
    /// restore. Deliberately no eviction and no launch here — the tile
    /// leaves only on the plugin's confirming `min-del`.
    pub(crate) fn restore_minimized(&self, addr: &str) {
        let ws = self.minimized.iter().find(|w| w.addr == addr).map(|w| w.ws);
        debug!("minimized: restore {addr} (from ws {ws:?})");
        crate::hypr::eval(&format!("hl.plugin.waveview.restore_min(\"{addr}\")"));
    }

    /// One transient dock entry per minimized window (id `min:<addr>`, name
    /// = the window title, so the dock tooltip reads right). Called from
    /// `refilter` alongside the other transient builders; the ids are
    /// picked up by `recompute_dock_order`.
    pub(crate) fn minimized_entries(&mut self) {
        // Vanish while the launcher ("menubox") is open — Max: "the
        // thumbnails should vanish when I open the menubox." This is a
        // visibility gate ONLY: the `minimized` state stands (the windows
        // stay minimized, thumbnails and slots intact), the tiles just are
        // not built into `dock_order`, so they don't draw and don't widen
        // the dock. `handle_command` refilters on the open↔close edge, so
        // they return the instant the launcher collapses.
        if self.ui.target() == crate::state::Target::Open {
            return;
        }
        let base = self.min_thumb_layer_base();
        // Snapshot first: pushing mutates the entry arrays.
        let wins: Vec<(String, String, Option<usize>)> = self
            .minimized
            .iter()
            .map(|w| (w.addr.clone(), w.title.clone(), w.slot))
            .collect();
        for (addr, title, slot) in wins {
            let entry = AppEntry {
                id: entry_id(&addr),
                name: title,
                description: None,
                // Never launched — the dock click restores instead.
                exec: String::new(),
                icon: None,
                startup_wm_class: None,
                needs_terminal: false,
                path: None,
            };
            // No thumbnail → the letter tile on the title, the same
            // placeholder every unresolved icon wears.
            let (layer, placeholder) = match slot {
                Some(s) => (base + s as u32, false),
                None => (0, true),
            };
            self.push_transient(entry, apps::EntryKind::App, placeholder, layer);
        }
    }

    /// Re-upload every minimized thumbnail after a rescan rebuilt the icon
    /// texture array (the counterpart of `reupload_thumb_icons`).
    pub(crate) fn reupload_min_thumbs(&mut self) {
        let base = self.min_thumb_layer_base();
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        for w in &self.minimized {
            if let (Some(slot), Some(pixels)) = (w.slot, w.pixels.as_ref()) {
                renderer.update_icon_layer(base + slot as u32, pixels);
            }
        }
    }
}

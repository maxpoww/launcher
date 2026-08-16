//! Resolves a notification's app icon to pixels off the UI thread.
//!
//! A notification ships an `app_icon` (a themed icon name or an absolute path)
//! and/or a `desktop_entry`; the UI thread picks the best icon *name* for a
//! card (see `App::notif_icon_hint`) and hands it here. This worker owns an
//! [`crate::apps::IconLoader`] — the same theme-chain resolver + persistent
//! raster cache the app indexer uses — so the cold theme-directory walk never
//! blocks the compositor loop. It replies with a premultiplied-RGBA8
//! [`crate::apps::ICON_CHAIN_BYTES`] mip chain, ready for
//! [`crate::renderer::Renderer::set_notif_icons`].
//!
//! Talks to the calloop loop only via channels (a plain mpsc for requests up,
//! a calloop channel for results back), mirroring [`crate::thumbs`] /
//! [`crate::nix`]. It exits when either channel closes.

use std::sync::mpsc;

use calloop::channel::Sender;
use tracing::warn;

use crate::apps::IconLoader;
use waverunner_core::index::AppEntry;

/// A resolution job: `key` is the caller's stable identity for the icon (so the
/// reply can be routed back and deduplicated), `icon` the themed name or path
/// to resolve, `name` the app's display name (only used to seed a placeholder,
/// which we discard — a placeholder means "no real icon").
pub struct Request {
    pub key: String,
    pub icon: String,
    pub name: String,
}

/// A finished resolution: the same `key`, and the mip chain if a *real* icon
/// resolved (`None` when only a placeholder was available → the card keeps its
/// monogram tile).
pub struct Resolved {
    pub key: String,
    pub chain: Option<Vec<u8>>,
}

/// Handle to the resolver thread.
pub struct NotifIcons {
    requests: mpsc::Sender<Request>,
}

impl NotifIcons {
    /// Queue a resolution job (deduplication is the caller's business).
    pub fn request(&self, req: Request) {
        let _ = self.requests.send(req);
    }
}

/// Spawn the resolver thread with its own [`IconLoader`] for `icon_theme`.
pub fn spawn(icon_theme: String, results: Sender<Resolved>) -> NotifIcons {
    let (requests, rx) = mpsc::channel::<Request>();
    let spawned = std::thread::Builder::new()
        .name("waverunner-notif-icons".into())
        .spawn(move || {
            let mut loader = IconLoader::new(icon_theme);
            while let Ok(req) = rx.recv() {
                let chain = resolve(&mut loader, &req);
                if results
                    .send(Resolved {
                        key: req.key,
                        chain,
                    })
                    .is_err()
                {
                    return; // event loop is gone
                }
            }
        });
    if let Err(e) = spawned {
        warn!("cannot spawn notif-icon resolver thread: {e}");
    }
    NotifIcons { requests }
}

/// Resolve one request to a mip chain, or `None` if only a placeholder tile is
/// available (unresolvable icon name / missing file).
fn resolve(loader: &mut IconLoader, req: &Request) -> Option<Vec<u8>> {
    // A synthetic entry carries just what `icon_for` reads: the icon name/path
    // (for resolution) and a name (for the placeholder we then reject). The
    // `id` keys the loader's in-memory raster cache, so distinct sources with
    // the same themed icon still share correctly.
    let entry = AppEntry {
        id: format!("notif:{}", req.icon),
        name: req.name.clone(),
        description: None,
        exec: String::new(),
        icon: Some(req.icon.clone()),
        startup_wm_class: None,
        needs_terminal: false,
        path: None,
    };
    let (pixels, is_placeholder) = loader.icon_for(&entry);
    (!is_placeholder).then_some(pixels)
}

//! The Spine — Brain ↔ Body wiring (roadmap S2).
//!
//! Bridge design (the todo2 half-page): the [`options_engine::Engine`] is
//! tokio-async; the daemon is a single calloop loop. So the Brain runs on its
//! own worker thread ("options-brain") owning a current-thread tokio runtime,
//! and talks back the only way workers ever do here: a calloop channel.
//!
//! ```text
//!   collectors → aggregator → watch<ContextState>          (tokio thread)
//!                                   │ changed().await → clone
//!                                   ▼
//!                        calloop::channel::Sender          (this module)
//!                                   │
//!                                   ▼
//!                     App::on_brain(ContextState)          (event loop)
//! ```
//!
//! Rules of the seam:
//! - Snapshots are whole and cheap to clone (strings + small structs); the
//!   watch channel already coalesces bursts — the loop only ever sees the
//!   latest state, never a backlog.
//! - The daemon NEVER blocks on the Brain. If the thread dies or a collector
//!   goes dark, consumers see stale [`Health`] and fall back to their own
//!   sensing (each surface keeps a degrade path until the Brain has earned
//!   its place everywhere).
//! - Consumers read `App::brain`; the first surface driven this way is the
//!   OPTIONS window pill (`options.rs::refresh_options_content`), whose
//!   per-event `hyprctl` round-trips the snapshot replaces.
//!
//! [`Health`]: options_engine::Health

use calloop::channel::Sender;
use options_engine::{ContextState, Engine};
use tracing::{info, warn};

/// Spawn the Brain thread: engine + subscription, snapshots forwarded into
/// the event loop until the receiving end closes (UI gone).
pub fn start(tx: Sender<ContextState>) {
    let spawned = std::thread::Builder::new()
        .name("options-brain".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    warn!("brain: tokio runtime failed to start: {e}");
                    return;
                }
            };
            rt.block_on(async move {
                let engine = Engine::start();
                let mut rx = engine.subscribe();
                info!("brain: engine started, streaming context");
                loop {
                    if rx.changed().await.is_err() {
                        warn!("brain: engine watch closed");
                        return;
                    }
                    let snapshot = rx.borrow_and_update().clone();
                    if tx.send(snapshot).is_err() {
                        return; // event loop is gone
                    }
                }
            });
        });
    if let Err(e) = spawned {
        warn!("brain: cannot spawn thread: {e}");
    }
}

/// Whether the Brain's view of the compositor is trustworthy right now —
/// the compositor layer is alive. Consumers poll for themselves when not.
pub fn hypr_alive(ctx: &ContextState) -> bool {
    ctx.health.compositor.alive
}

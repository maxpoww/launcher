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
use options_engine::{ContextState, Engine, Mind, OptionSet, Tuning};
use tracing::{info, warn};

/// The daemon's decision tuning. A roomier `max_items` than the engine default
/// (3): the topbar's OPTION cluster wants the whole context-relevant control
/// set (e.g. a media cluster) available, and the surface budgets how many pills
/// it actually shows — the mind still ranks and drops the irrelevant.
fn daemon_tuning() -> Tuning {
    Tuning {
        min_relevance: 0.2,
        max_items: 8,
        skill: 0.5,
    }
}

/// Spawn the Brain thread: the sensing engine AND the deciding [`Mind`]. Raw
/// context snapshots go to `ctx_tx` (the window pill, battery); the Mind's
/// ranked [`OptionSet`] goes to `opt_tx` (the dynamic OPTION pills). Both flow
/// until their receiving end closes (UI gone).
pub fn start(ctx_tx: Sender<ContextState>, opt_tx: Sender<OptionSet>) {
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
                // The Mind must outlive the loop (its Drop aborts the decide
                // task), so bind it here for the whole block.
                let mind = Mind::new(&engine, daemon_tuning());
                let mut ctx_rx = engine.subscribe();
                let mut opt_rx = mind.subscribe();
                info!("brain: engine + mind started, streaming context and options");
                loop {
                    tokio::select! {
                        r = ctx_rx.changed() => {
                            if r.is_err() {
                                warn!("brain: engine watch closed");
                                return;
                            }
                            let snapshot = ctx_rx.borrow_and_update().clone();
                            if ctx_tx.send(snapshot).is_err() {
                                return; // event loop is gone
                            }
                        }
                        r = opt_rx.changed() => {
                            if r.is_err() {
                                warn!("brain: mind watch closed");
                                return;
                            }
                            let options = opt_rx.borrow_and_update().clone();
                            if opt_tx.send(options).is_err() {
                                return; // event loop is gone
                            }
                        }
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

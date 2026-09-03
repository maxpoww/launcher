//! The mind — the decision layer that sits above sensing.
//!
//! It subscribes to the engine's [`ContextState`](crate::ContextState) stream
//! and, for every change, decides the current [`OptionSet`]: the ranked,
//! de-cluttered affordances to surface. Sensing (the collectors) stays pure and
//! unaware of decisions; the mind is the only place context becomes *options*.
//!
//! The decision itself is the pure [`decide`] function; this module just wraps
//! it in a task and republishes on its own `watch`, so any surface can
//! subscribe to options exactly the way it subscribes to context.
//!
//! ```no_run
//! # async fn demo() {
//! let engine = options_engine::Engine::start();
//! let mind = options_engine::Mind::new(&engine, Default::default());
//! let mut rx = mind.subscribe();
//! loop {
//!     rx.changed().await.unwrap();
//!     for a in &rx.borrow().items {
//!         println!("[{:.2}] {} — {}", a.relevance, a.title, a.detail);
//!     }
//! }
//! # }
//! ```

mod activity;
mod affordance;
mod decide;
mod session;
mod settle;

pub use activity::{infer_activity, Activity};
pub use affordance::{Affordance, AffordanceAction, AffordanceKind, OptionSet};
pub use decide::{decide, decide_with, Tuning};
pub use session::Temporal;

use std::time::Instant;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::engine::Engine;

use session::Session;
use settle::Settle;

/// A live decision loop: context in, ranked options out.
pub struct Mind {
    rx: watch::Receiver<OptionSet>,
    task: JoinHandle<()>,
}

impl Mind {
    /// Start deciding from `engine`'s context with the given [`Tuning`]. Must be
    /// called within a Tokio runtime.
    pub fn new(engine: &Engine, tuning: Tuning) -> Self {
        let mut ctx_rx = engine.subscribe();
        let (tx, rx) = watch::channel(OptionSet::default());
        let task = tokio::spawn(async move {
            let mut session = Session::new(Instant::now());
            let mut settle = Settle::default();
            loop {
                // Decide from the latest snapshot plus the session's temporal
                // memory; scope the borrow so it's dropped before the await.
                // What is decided and what is SHOWN are two questions: the
                // decision is instantaneous, the bar is not (see `settle`).
                let options = {
                    let now = Instant::now();
                    let ctx = ctx_rx.borrow_and_update();
                    let temporal = session.observe(&ctx, now);
                    settle.apply(decide_with(&ctx, &temporal, &tuning), now)
                };
                // If an offer is still waiting out its settle dwell, arm a
                // wake for the moment it earns its pill: the loop is otherwise
                // purely change-driven, and a context that goes quiet right
                // after an offer arrives would leave it hidden until some
                // unrelated event re-ran the decision.
                let deadline = settle.next_deadline(Instant::now());
                if tx.send(options).is_err() {
                    break; // no subscribers and receiver dropped
                }
                match deadline {
                    Some(d) => tokio::select! {
                        changed = ctx_rx.changed() => {
                            if changed.is_err() {
                                break; // engine gone
                            }
                        }
                        () = tokio::time::sleep_until(d.into()) => {}
                    },
                    None => {
                        if ctx_rx.changed().await.is_err() {
                            break; // engine gone
                        }
                    }
                }
            }
        });
        Mind { rx, task }
    }

    /// A fresh subscription to the option stream.
    pub fn subscribe(&self) -> watch::Receiver<OptionSet> {
        self.rx.clone()
    }

    /// The current option set.
    pub fn current(&self) -> OptionSet {
        self.rx.borrow().clone()
    }
}

impl Drop for Mind {
    fn drop(&mut self) {
        self.task.abort();
    }
}

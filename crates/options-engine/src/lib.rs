//! # options-engine — the OPTIONS Context Core ("the Brain", sensing half)
//!
//! The headless perception engine behind **OPTIONS**, the reactive desktop
//! surface at the heart of **StandardOS** ("Standard OPTIONS"). OPTIONS gives
//! the user the right tool or information at the right moment and clears away
//! the rest — guiding diegetically, without breaking immersion. That requires
//! the system to continuously *read your actions* (pillar 2) so it can later
//! decide what is *logical to surface* (pillar 3) and *calibrate to your skill*
//! (pillar 4). This crate is that reading.
//!
//! Sensing and deciding are kept strictly apart. The **collectors** only
//! perceive — they sense and publish a unified [`ContextState`], deciding
//! nothing. The **[`Mind`]** is the separate decision layer: it subscribes to
//! those snapshots and produces a ranked, de-cluttered [`OptionSet`] (the
//! affordances to surface). Perception stays pure and reusable; the mind is the
//! only place context becomes *options*.
//!
//! ## Architecture
//!
//! Five asynchronous *collector* layers each stream small updates into one
//! aggregator, which owns the master state and republishes whole snapshots:
//!
//! ```text
//!   Layer 1 Compositor (Hyprland)  ┐
//!   Layer 2 Selection / Bridges    │  mpsc<Update>      watch<ContextState>
//!   Layer 3 Behaviour              ├──────────────►  aggregator  ──────────►  subscribers
//!   Layer 4 Media / Audio / HW     │  (per-source     (sole writer)          (the mind, UIs…)
//!   Layer 5 = this aggregator      ┘   backpressure)
//! ```
//!
//! One writer ⇒ every snapshot is whole and consistently ordered; the `mpsc`
//! gives each source independent backpressure, so high-frequency inputs (window
//! focus) never block low-frequency ones (battery). Each collector owns its own
//! reconnect policy and reports liveness, so a dead source degrades gracefully
//! (its [`Health`] goes dark) instead of stalling the engine.
//!
//! Implemented today: the spine (aggregator + `watch` + [`Collector`] trait)
//! and **Layer 1 (Hyprland)**. Layers 2–4 land next behind the same trait.
//!
//! ## Usage
//!
//! ```no_run
//! # async fn demo() {
//! let engine = options_engine::Engine::start();
//! let mut rx = engine.subscribe();
//! loop {
//!     rx.changed().await.unwrap();
//!     let ctx = rx.borrow();
//!     println!("focused: {} ({})", ctx.window.class, ctx.window.title);
//! }
//! # }
//! ```

mod collector;
mod collectors;
mod engine;
mod message;
mod mind;
mod state;

pub use collector::{Collector, CollectorFuture};
pub use engine::Engine;
pub use message::{ContextDelta, Update};
pub use mind::{
    decide, decide_with, infer_activity, Activity, Affordance, AffordanceKind, Mind, OptionSet,
    Temporal, Tuning,
};
pub use state::{
    ActiveWindow, AppInternalContext, AudioState, BehavioralMetrics, ContextState, DeployHealth,
    GitContext, Health, Layer, LayerHealth, MediaState, SystemMetrics, TextSelection,
};

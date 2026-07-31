//! The uniform shape of a telemetry source.
//!
//! Every layer (compositor, selection, bridges, behaviour, hardware) is a
//! [`Collector`]: a long-lived async task that owns its own connection and
//! reconnect policy, and streams [`Update`]s to the aggregator. The engine
//! only spawns them and logs if one exits — a collector is responsible for its
//! own resilience (reconnect with backoff, marking its layer dead on drop), so
//! the loss of one source degrades gracefully instead of taking down the mind.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::message::Update;
use crate::state::Layer;

/// A collector's run future: normally never resolves (it loops forever,
/// reconnecting), but may return an error the engine will log.
pub type CollectorFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

/// A telemetry source feeding one [`Layer`].
pub trait Collector: Send + 'static {
    /// Short name for logs.
    fn name(&self) -> &'static str;

    /// The layer this collector's health is tracked under.
    fn layer(&self) -> Layer;

    /// Run forever, emitting [`Update`]s on `tx`. Ownership is taken (`Box<Self>`)
    /// so the collector can move its state into the future; internal reconnect
    /// loops keep it running across transient failures.
    fn run(self: Box<Self>, tx: mpsc::Sender<Update>) -> CollectorFuture;
}

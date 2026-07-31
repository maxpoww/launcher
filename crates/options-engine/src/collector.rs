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

use tokio::sync::{mpsc, watch};

use crate::message::Update;
use crate::state::{ContextState, Layer};

/// A collector's run future: normally never resolves (it loops forever,
/// reconnecting), but may return an error the engine will log.
pub type CollectorFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

/// A telemetry source feeding one [`Layer`].
///
/// Two flavours share this trait:
/// - **Sensors** stream from an external source (Hyprland, D-Bus, /proc) and
///   ignore the `ctx` handle.
/// - **Derivers** read the current aggregate through `ctx` and compute *more*
///   context from it (e.g. git status from the focused window's PID). They must
///   gate on their input actually changing, so emitting never feeds itself.
pub trait Collector: Send + 'static {
    /// Short name for logs.
    fn name(&self) -> &'static str;

    /// The layer this collector's health is tracked under.
    fn layer(&self) -> Layer;

    /// Run forever, emitting [`Update`]s on `tx`. `ctx` is a read-only view of
    /// the current aggregate (for derivers). Ownership is taken (`Box<Self>`)
    /// so the collector can move its state into the future; internal reconnect
    /// loops keep it running across transient failures.
    fn run(
        self: Box<Self>,
        ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture;
}

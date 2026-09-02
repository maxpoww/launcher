//! The aggregator and the public handle.
//!
//! Topology: N collector tasks → one `mpsc<Update>` → the single aggregator
//! task (the *only* writer of the master state) → a `watch<ContextState>` that
//! any number of subscribers clone. One writer means snapshots are always
//! whole and consistently ordered; the `mpsc` gives each source independent
//! backpressure so fast layers never block slow ones.

use std::time::Instant;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::collector::Collector;
use crate::message::{ContextDelta, Update};
use crate::state::ContextState;

/// Buffer of the collector→aggregator channel. Generous, but bounded: a
/// runaway source gets backpressured (its `send` awaits) rather than growing
/// memory without limit.
const UPDATE_BUFFER: usize = 256;

/// The OPTIONS Context Core handle.
///
/// Holds the master subscription and owns the background tasks; dropping it
/// aborts them. Cheap to construct, cheap to `subscribe`.
pub struct Engine {
    rx: watch::Receiver<ContextState>,
    tasks: Vec<JoinHandle<()>>,
}

impl Engine {
    /// Start the engine with the default production collectors (Layer 1:
    /// Hyprland). Must be called from within a Tokio runtime.
    pub fn start() -> Self {
        Self::start_with(default_collectors())
    }

    /// Start the engine with a custom set of collectors (used by tests, and by
    /// callers who want to select layers). Must be called within a runtime.
    pub fn start_with(collectors: Vec<Box<dyn Collector>>) -> Self {
        let (watch_tx, rx) = watch::channel(ContextState::default());
        let (upd_tx, upd_rx) = mpsc::channel::<Update>(UPDATE_BUFFER);

        let mut tasks = Vec::with_capacity(collectors.len() + 1);
        tasks.push(tokio::spawn(aggregate(upd_rx, watch_tx)));

        for collector in collectors {
            let tx = upd_tx.clone();
            let ctx = rx.clone();
            let name = collector.name();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = collector.run(ctx, tx).await {
                    tracing::warn!(collector = name, "collector exited: {e:#}");
                }
            }));
        }
        // The aggregator holds the only long-lived `upd_tx` clones via the
        // collectors; drop ours so the channel closes once they all stop.
        drop(upd_tx);

        Engine { rx, tasks }
    }

    /// A fresh subscription to context snapshots. The receiver always yields the
    /// latest whole [`ContextState`]; use `borrow()` for the current value and
    /// `changed().await` to wait for the next.
    pub fn subscribe(&self) -> watch::Receiver<ContextState> {
        self.rx.clone()
    }

    /// A clone of the current context snapshot.
    pub fn current(&self) -> ContextState {
        self.rx.borrow().clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Production collector set. Extended as each layer lands.
fn default_collectors() -> Vec<Box<dyn Collector>> {
    vec![
        Box::new(crate::collectors::hyprland::HyprlandCollector::new()),
        Box::new(crate::collectors::system::SystemCollector::new()),
        Box::new(crate::collectors::git::GitCollector::new()),
        Box::new(crate::collectors::media::MediaCollector::new()),
        Box::new(crate::collectors::bridge::BridgeCollector::new()),
        Box::new(crate::collectors::selection::SelectionCollector::new()),
        Box::new(crate::collectors::audio::AudioCollector::new()),
        Box::new(crate::collectors::deploy::DeployHealthCollector::new()),
        Box::new(crate::collectors::notifications::NotificationCollector::new()),
        Box::new(crate::collectors::downloads::DownloadsCollector::new()),
    ]
}

/// The sole writer of the master state: apply each update, stamp provenance,
/// bump the generation, and republish the whole snapshot.
async fn aggregate(mut rx: mpsc::Receiver<Update>, tx: watch::Sender<ContextState>) {
    let start = Instant::now();
    let mut state = ContextState::default();
    while let Some(update) = rx.recv().await {
        let now_ms = start.elapsed().as_millis() as u64;
        match update {
            Update::Delta(layer, delta) => {
                apply(&mut state, delta);
                state.health.stamp(layer, now_ms);
            }
            Update::Health(layer, alive) => {
                state.health.set_alive(layer, alive);
            }
        }
        state.generation += 1;
        // Only fails if every subscriber is gone; the Engine keeps one, so in
        // practice this never errors — ignore either way.
        let _ = tx.send(state.clone());
    }
}

/// Fold one delta into the master state.
fn apply(state: &mut ContextState, delta: ContextDelta) {
    match delta {
        ContextDelta::Window(w) => state.window = w,
        ContextDelta::Submap(s) => state.hypr_submap = s,
        ContextDelta::ActiveLayout(l) => state.active_layout = l,
        ContextDelta::Screencasting(b) => state.is_screencasting = b,
        ContextDelta::FocusSwitchVelocity(v) => state.behavior.focus_switch_velocity = v,
        ContextDelta::Metrics(m) => state.metrics = m,
        ContextDelta::Git(g) => state.git = g,
        ContextDelta::Media(m) => state.media = m,
        ContextDelta::AppInternal(a) => state.app_internal = a,
        ContextDelta::Selection(s) => state.selection = s,
        ContextDelta::Audio(a) => state.audio = a,
        ContextDelta::Deploy(d) => state.deploy = d,
        ContextDelta::Notifications(n) => state.notifications = n,
        ContextDelta::RecentDownload(d) => state.recent_download = d,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::CollectorFuture;
    use crate::state::{ActiveWindow, Layer};

    /// A collector that emits a scripted list of updates then idles forever, so
    /// the aggregator/watch plumbing can be exercised without any real system.
    struct ScriptCollector {
        layer: Layer,
        script: Vec<Update>,
    }

    impl Collector for ScriptCollector {
        fn name(&self) -> &'static str {
            "script"
        }
        fn layer(&self) -> Layer {
            self.layer
        }
        fn run(
            self: Box<Self>,
            _ctx: watch::Receiver<ContextState>,
            tx: mpsc::Sender<Update>,
        ) -> CollectorFuture {
            Box::pin(async move {
                for u in self.script {
                    tx.send(u).await.map_err(|e| anyhow::anyhow!("{e}"))?;
                }
                // Idle so the channel stays open (mirrors a real long-lived task).
                std::future::pending::<()>().await;
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn aggregates_delta_and_stamps_health() {
        let win = ActiveWindow {
            address: "0x1".into(),
            class: "foot".into(),
            title: "zsh".into(),
            pid: 42,
            workspace_id: 1,
            is_fullscreen: false,
            is_floating: false,
        };
        let engine = Engine::start_with(vec![Box::new(ScriptCollector {
            layer: Layer::Compositor,
            script: vec![
                Update::Health(Layer::Compositor, true),
                Update::Delta(Layer::Compositor, ContextDelta::Window(win.clone())),
                Update::Delta(Layer::Compositor, ContextDelta::Submap("resize".into())),
            ],
        })]);

        let mut rx = engine.subscribe();
        // Wait until all three updates have landed (generation == 3).
        loop {
            rx.changed().await.unwrap();
            if rx.borrow().generation >= 3 {
                break;
            }
        }
        let snap = rx.borrow().clone();
        assert_eq!(snap.window.class, "foot");
        assert_eq!(snap.window.pid, 42);
        assert_eq!(snap.hypr_submap, "resize");
        assert!(snap.health.compositor.alive);
        assert!(snap.health.compositor.last_update_ms.is_some());
        // A layer we never fed stays dark.
        assert!(!snap.health.hardware.alive);
    }

    #[tokio::test]
    async fn untouched_layers_have_no_freshness() {
        let engine = Engine::start_with(vec![]);
        let snap = engine.current();
        assert_eq!(snap.generation, 0);
        assert!(snap.health.compositor.last_update_ms.is_none());
        assert!(!snap.is_screencasting);
    }
}

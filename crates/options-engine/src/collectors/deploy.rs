//! Layer 4 (part) — NixOS deploy health: is the running system the latest built?
//!
//! A pure timer-driven sensor comparing three symlinks NixOS maintains, fully
//! resolved to their store paths — no external services, no privileges:
//!
//! - `/run/booted-system`            — the system this boot is *running*.
//! - `/run/current-system`           — the *activated* system (after a switch).
//! - `/nix/var/nix/profiles/system`  — the *newest built* generation.
//!
//! When they diverge, deploy state has gone invisible-but-wrong: a switch that
//! didn't activate, or a newer generation built but not yet booted. Surfacing
//! that at the right moment is the StandardOS dogfood of the OPTIONS pillars.
//!
//! Born from a real incident (2026-07-31): a sticky systemd-boot EFI default
//! silently kept reboots on a 2-day-old generation while `nixos-rebuild switch`
//! kept building new ones. P0/P1 fixed the mechanism; this is the runtime
//! backstop that makes any remaining drift visible. See `~/safety.md` (P2).
//!
//! On non-NixOS hosts the three paths don't resolve, so the collector emits
//! nothing and the [`Layer::System`] health stays dark (the mind then never
//! surfaces deploy affordances) — rather than reporting a false "healthy".

use std::path::Path;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::collector::{Collector, CollectorFuture};
use crate::message::{ContextDelta, Update};
use crate::state::{ContextState, DeployHealth, Layer};

/// How often to check. Deploy state only changes on a rebuild or a reboot, so
/// this is deliberately slow — cheap `readlink`s, but no reason to poll fast.
const POLL: Duration = Duration::from_secs(15);

const BOOTED: &str = "/run/booted-system";
const CURRENT: &str = "/run/current-system";
const LATEST: &str = "/nix/var/nix/profiles/system";

#[derive(Default)]
pub struct DeployHealthCollector;

impl DeployHealthCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for DeployHealthCollector {
    fn name(&self) -> &'static str {
        "deploy_health"
    }
    fn layer(&self) -> Layer {
        Layer::System
    }
    fn run(
        self: Box<Self>,
        _ctx: watch::Receiver<ContextState>,
        tx: mpsc::Sender<Update>,
    ) -> CollectorFuture {
        Box::pin(async move {
            loop {
                // Only emit when we can actually resolve all three generations;
                // otherwise leave the layer dark rather than assert "healthy".
                if let Some(health) = read_deploy_health() {
                    if tx
                        .send(Update::Delta(Layer::System, ContextDelta::Deploy(health)))
                        .await
                        .is_err()
                    {
                        return Ok(()); // aggregator gone
                    }
                }
                tokio::time::sleep(POLL).await;
            }
        })
    }
}

/// Resolve the three generation symlinks to their store paths and assess drift,
/// or `None` if any can't be resolved (not NixOS, or a transient race).
fn read_deploy_health() -> Option<DeployHealth> {
    let booted = std::fs::canonicalize(BOOTED).ok()?;
    let current = std::fs::canonicalize(CURRENT).ok()?;
    let latest = std::fs::canonicalize(LATEST).ok()?;
    Some(assess(&booted, &current, &latest))
}

/// Compare the resolved store paths. Pure, so the drift logic is unit-testable
/// without a NixOS filesystem.
fn assess(booted: &Path, current: &Path, latest: &Path) -> DeployHealth {
    DeployHealth {
        not_activated: current != latest,
        stale_generation: booted != latest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn all_in_sync_is_healthy() {
        let g = p("/nix/store/gen-new");
        let h = assess(&g, &g, &g);
        assert!(!h.not_activated);
        assert!(!h.stale_generation);
    }

    #[test]
    fn built_but_not_rebooted_is_stale_only() {
        // Switched fine (current == latest), but still running the old kernel.
        let old = p("/nix/store/gen-old");
        let new = p("/nix/store/gen-new");
        let h = assess(&old, &new, &new);
        assert!(!h.not_activated, "the switch did activate");
        assert!(h.stale_generation, "booted is behind latest");
    }

    #[test]
    fn switch_that_didnt_activate_is_not_activated() {
        // A newer generation exists but current still points at the old one
        // (booted == current == old): the deploy silently failed to take.
        let old = p("/nix/store/gen-old");
        let new = p("/nix/store/gen-new");
        let h = assess(&old, &old, &new);
        assert!(h.not_activated, "current != latest");
        assert!(h.stale_generation, "booted != latest either");
    }
}
